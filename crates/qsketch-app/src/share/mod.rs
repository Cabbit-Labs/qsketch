//! Drawing together over Leyline.
//!
//! A shared document is an ordinary tab with a colored dot. Leyline (assumed
//! running and connected) is the transport: qsketch talks to its loopback
//! sketch link, and every change travels as an op in the conversation's
//! sketchpad log, so it fans out to the group, queues for whoever is offline,
//! and replays for whoever joins later.
//!
//! What travels is **committed state**, not brush strokes: after every history
//! step the session compares the committed state with the last state it sent
//! and ships the tiles that differ (copy-on-write tiles make that a pointer
//! comparison), plus the layer stack when it changed. What comes back from the
//! others is written into the working state *and every undo snapshot*, the
//! same way layer visibility is — so your undo only ever takes back your own
//! marks, and a peer's stroke never re-sends as if it were yours. Where two
//! people paint the same tile at once the later Lamport counter wins on both
//! sides, so everyone converges on the same picture.

pub mod codec;
pub mod link;

use std::collections::HashMap;
use std::time::Instant;

use egui::Color32;
use qsketch_core::{DocState, Document, Layer, LayerId, Pt, Raster, TileSet, TILE};

use crate::state::{AppState, DocEntry, DocId};
use link::{Conversation, Inbound, Link, Outbound, RemoteOp};

/// Op kinds, mirroring `leyline_core::sketch::kind::CANVAS_*`.
pub mod kind {
    pub const SNAPSHOT: u8 = 20;
    pub const PATCH: u8 = 21;
    pub const CURSOR: u8 = 22;
    pub const LAYERS: u8 = 23;
    pub const REQUEST: u8 = 24;
}

/// Tiles per patch op, so one fill of a huge layer becomes several bounded
/// payloads rather than one that trips Leyline's size cap.
const PATCH_TILES: usize = 256;
/// After this many patches a fresh snapshot goes out too, so a late joiner
/// replays from a recent base rather than from the first one ever sent.
const SNAPSHOT_EVERY: u32 = 150;
/// Cursor send rate cap.
const CURSOR_INTERVAL_MS: u128 = 50;
/// A remote cursor that has not moved for this long is hidden.
const CURSOR_TTL_SECS: f32 = 4.0;

/// A state op waiting to go out: conversation, kind, payload, document size,
/// and the tiles it carries (so their counters can be recorded when acked).
type QueuedOp = (Conversation, u8, Vec<u8>, Option<(u32, u32)>, Vec<(LayerId, usize)>);
/// A sent patch whose acknowledgment is still to come: conversation, request
/// ref, tiles.
type Ack = (String, u64, Vec<(LayerId, usize)>);

#[derive(Clone, Debug)]
pub struct RemoteCursor {
    pub name: String,
    pub pos: Pt,
    pub color: Color32,
    pub tool: String,
    pub at: Instant,
}

pub struct ShareSession {
    pub conv: Conversation,
    /// The committed state everyone else has seen (None until a joiner's first
    /// snapshot arrives).
    synced: Option<DocState>,
    synced_id: u64,
    /// Whether Leyline has finished replaying the stored log for us; ops before
    /// that are history, which a host (who already has the picture) ignores.
    joined: bool,
    host: bool,
    /// Lamport counter of the last op that wrote each tile, per layer.
    tile_lc: HashMap<(LayerId, usize), u64>,
    /// Sent patches awaiting their counter: request ref → tiles.
    inflight: HashMap<u64, Vec<(LayerId, usize)>>,
    patches_since_snapshot: u32,
    pub cursors: HashMap<String, RemoteCursor>,
    /// When each participant last moved, so a return after a long pause is
    /// announced again but a cursor blinking in and out of the TTL is not.
    seen: HashMap<String, Instant>,
    last_cursor: (Instant, Option<Pt>),
    /// Ops held back while a stroke or transform is in flight on this document.
    pending: Vec<RemoteOp>,
}

impl ShareSession {
    fn new(conv: Conversation, host: bool) -> Self {
        Self {
            conv,
            synced: None,
            synced_id: u64::MAX,
            joined: false,
            host,
            tile_lc: HashMap::new(),
            inflight: HashMap::new(),
            patches_since_snapshot: 0,
            cursors: HashMap::new(),
            seen: HashMap::new(),
            last_cursor: (Instant::now(), None),
            pending: Vec::new(),
        }
    }

    /// Still waiting for the first snapshot (a joiner before anyone answered).
    pub fn waiting(&self) -> bool {
        self.synced.is_none()
    }

    /// Someone else did something here within the last half minute.
    pub fn active(&self) -> bool {
        self.seen.values().any(|t| t.elapsed().as_secs_f32() < 30.0)
    }

    /// Everyone who moved a cursor recently, for the status line.
    pub fn present(&self) -> Vec<&RemoteCursor> {
        let mut v: Vec<&RemoteCursor> = self.cursors.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

/// The color of a shared canvas's dot: green while someone else is active,
/// grey when connected but nobody is, none at all while Leyline is not there.
pub fn status_color(state: &AppState, sess: &ShareSession) -> Option<Color32> {
    if !state.share.as_ref().is_some_and(|l| l.connected) {
        return None;
    }
    Some(if sess.active() { Color32::from_rgb(70, 200, 110) } else { Color32::from_gray(140) })
}

/// The dot a shared canvas wears (tab, dialog, status bar): a filled circle
/// from the icon font, which every theme has, rather than a text glyph the UI
/// font may lack.
pub fn dot(color: Color32, size: f32) -> egui::RichText {
    egui::RichText::new(crate::ui::icons::CIRCLE)
        .family(egui::FontFamily::Name(crate::ui::theme::ICON_FONT_FILL.into()))
        .size(size)
        .color(color)
}

/// A stable, distinct color for an id (conversation or participant).
pub fn conv_color(id: &str) -> Color32 {
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32;
    let c = qsketch_core::Hsv::new(hue, 0.7, 0.95).to_rgba8(255);
    Color32::from_rgb(c.r, c.g, c.b)
}

/// Random per-process tag stamped on our state ops, so one that comes back
/// (from our own other device, or a replay) is recognized as ours.
fn session_tag() -> &'static str {
    use std::sync::OnceLock;
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let mut x = (t as u64) ^ ((std::process::id() as u64) << 32) ^ 0x9E3779B97F4A7C15;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51afd7ed558ccd);
        x ^= x >> 33;
        format!("{:08x}", (x & 0xffff_ffff) as u32)
    })
}

/// Give this participant its own range of layer ids so two people adding
/// layers at once cannot mint the same id.
fn randomize_layer_ids(doc: &mut Document) {
    let max = doc.state().layers.iter().map(|l| l.props.id).max().unwrap_or(0);
    let base = (u64::from_str_radix(session_tag(), 16).unwrap_or(1).max(1)) << 32;
    let next = base | (max + 1);
    doc.state_mut().next_layer_id = next;
    doc.history.for_each_state_mut(|s| s.next_layer_id = next);
}

impl AppState {
    /// The Leyline link, started on first use.
    pub fn share_link(&mut self, ctx: &egui::Context) -> &mut Link {
        if self.share.is_none() {
            self.share = Some(Link::connect(ctx.clone()));
        }
        self.share.as_mut().unwrap()
    }
}

/// Start sharing an open document with a conversation: the document becomes
/// the base everyone else draws on.
pub fn start_sharing(state: &mut AppState, ctx: &egui::Context, doc_id: DocId, conv: Conversation) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    if entry.share.is_some() {
        return;
    }
    randomize_layer_ids(&mut entry.doc);
    let mut sess = ShareSession::new(conv.clone(), true);
    sess.synced = Some(entry.doc.history.current().clone());
    sess.synced_id = entry.doc.history.current_id();
    let snapshot = codec::encode_snapshot(entry.doc.history.current(), &entry.doc.title);
    entry.share = Some(sess);
    let link = state.share_link(ctx);
    link.send(Outbound::Join { conv: conv.id.clone() });
    match snapshot {
        Ok(bytes) => {
            send_state_op(link, &conv, kind::SNAPSHOT, bytes, None);
        }
        Err(e) => state.toasts.error(format!("Couldn't share the canvas: {e}")),
    }
    state.toasts.success(format!("Sharing with {}", conv.name));
}

/// Open a conversation's shared canvas as a new tab. The tab fills in when the
/// replay (or a participant's answer to our request) delivers a snapshot.
pub fn join(state: &mut AppState, ctx: &egui::Context, conv: Conversation) -> DocId {
    if let Some(d) = state.docs.iter().find(|d| d.share.as_ref().is_some_and(|s| s.conv.id == conv.id)) {
        let id = d.id;
        state.active_doc = Some(id);
        return id;
    }
    let doc = Document::new(64, 64, None, format!("{} (shared)", conv.name));
    let id = state.add_document(doc);
    if let Some(entry) = state.doc_mut(id) {
        entry.share = Some(ShareSession::new(conv.clone(), false));
    }
    state.share_link(ctx).send(Outbound::Join { conv: conv.id });
    id
}

/// Leave the conversation's canvas; the document stays open as a plain one.
pub fn stop(state: &mut AppState, doc_id: DocId) {
    let Some(entry) = state.doc_mut(doc_id) else { return };
    let Some(sess) = entry.share.take() else { return };
    if let Some(link) = state.share.as_ref() {
        link.send(Outbound::Leave { conv: sess.conv.id });
    }
}

fn send_state_op(link: &mut Link, conv: &Conversation, kind: u8, bytes: Vec<u8>, size: Option<(u32, u32)>) -> u64 {
    let r = link.next_ref();
    link.send(Outbound::Op {
        r#ref: r,
        conv: conv.id.clone(),
        group: conv.group,
        kind,
        points: size.map(|(w, h)| vec![w as f32, h as f32]).unwrap_or_default(),
        color: "#ffffff".into(),
        text: session_tag().to_string(),
        image: Some(data_encoding::BASE64.encode(&bytes)),
    });
    r
}

/// Once per frame: pump the link, apply what arrived, send what changed.
pub fn tick(state: &mut AppState, ctx: &egui::Context) {
    if state.share.is_none() {
        return;
    }
    let inbound = state.share.as_mut().map(|l| l.drain()).unwrap_or_default();
    let mut outgoing: Vec<Outbound> = Vec::new();
    for m in inbound {
        match m {
            Inbound::Connected => {
                // (Re)join every shared document; a host that already has the
                // picture treats the replay as history (see `joined`).
                for d in &mut state.docs {
                    if let Some(s) = d.share.as_mut() {
                        s.joined = false;
                        outgoing.push(Outbound::Join { conv: s.conv.id.clone() });
                    }
                }
            }
            Inbound::Disconnected => {
                for d in &mut state.docs {
                    if let Some(s) = d.share.as_mut() {
                        s.cursors.clear();
                    }
                }
            }
            Inbound::Joined { conv } => {
                for d in &mut state.docs {
                    if let Some(s) = d.share.as_mut().filter(|s| s.conv.id == conv) {
                        s.joined = true;
                        if s.synced.is_none() {
                            // Nothing stored to replay from: ask the room.
                            outgoing.push(Outbound::Op {
                                r#ref: 0,
                                conv: conv.clone(),
                                group: s.conv.group,
                                kind: kind::REQUEST,
                                points: vec![],
                                color: "#ffffff".into(),
                                text: session_tag().to_string(),
                                image: None,
                            });
                        }
                    }
                }
            }
            Inbound::Sent { r#ref, lc } => {
                for d in &mut state.docs {
                    if let Some(s) = d.share.as_mut() {
                        if let Some(tiles) = s.inflight.remove(&r#ref) {
                            for key in tiles {
                                let e = s.tile_lc.entry(key).or_default();
                                *e = (*e).max(lc);
                            }
                        }
                    }
                }
            }
            Inbound::Error { msg } => state.toasts.error(format!("Leyline: {msg}")),
            Inbound::Op(op) => {
                let Some(pos) = state.docs.iter().position(|d| d.share.as_ref().is_some_and(|s| s.conv.id == op.conv))
                else {
                    continue;
                };
                let id = state.docs[pos].id;
                let busy = state.session_doc == Some(id)
                    || state.floating.as_ref().is_some_and(|f| f.doc == id)
                    || state.text_edit.as_ref().is_some_and(|t| t.doc == id);
                let undo_limit = state.settings.general.undo_limit;
                let entry = &mut state.docs[pos];
                let sess = entry.share.as_mut().unwrap();
                if op.kind == kind::CURSOR {
                    if !op.mine {
                        if let Some(name) = remote_cursor(sess, &op) {
                            state.toasts.info(format!("{name} is drawing on {}", sess.conv.name));
                        }
                    }
                    continue;
                }
                if op.text == session_tag() {
                    continue; // our own, echoed
                }
                if !op.mine {
                    sess.seen.insert(op.author.clone(), Instant::now());
                }
                if op.kind == kind::REQUEST {
                    if sess.synced.is_some() && sess.joined {
                        if let Ok(bytes) = codec::encode_snapshot(entry.doc.history.current(), &entry.doc.title) {
                            outgoing.push(Outbound::Op {
                                r#ref: 0,
                                conv: op.conv.clone(),
                                group: sess.conv.group,
                                kind: kind::SNAPSHOT,
                                points: vec![entry.doc.width() as f32, entry.doc.height() as f32],
                                color: "#ffffff".into(),
                                text: session_tag().to_string(),
                                image: Some(data_encoding::BASE64.encode(&bytes)),
                            });
                        }
                    }
                    continue;
                }
                if sess.host && !sess.joined {
                    continue; // replayed history of a picture we already hold
                }
                if busy && sess.synced.is_some() {
                    sess.pending.push(op);
                    continue;
                }
                apply_remote(entry, undo_limit, op);
            }
            Inbound::Me { .. } | Inbound::List { .. } => {}
        }
    }

    // Held-back ops, once the document is idle again.
    let session_doc = state.session_doc;
    let floating_doc = state.floating.as_ref().map(|f| f.doc);
    let text_doc = state.text_edit.as_ref().map(|t| t.doc);
    let undo_limit = state.settings.general.undo_limit;
    for entry in &mut state.docs {
        let id = entry.id;
        let busy = session_doc == Some(id) || floating_doc == Some(id) || text_doc == Some(id);
        let Some(sess) = entry.share.as_mut() else { continue };
        if busy || sess.pending.is_empty() {
            continue;
        }
        let pending = std::mem::take(&mut sess.pending);
        for op in pending {
            apply_remote(entry, undo_limit, op);
        }
    }

    // Our own changes, and our cursor.
    let active = state.active_doc;
    let hover = state.hover_doc_pos;
    let tool = state.effective_tool().label();
    let my_id = state.share.as_ref().and_then(|l| l.me.as_ref()).map(|m| m.0.clone()).unwrap_or_default();
    let mut ops: Vec<QueuedOp> = Vec::new();
    for entry in &mut state.docs {
        let id = entry.id;
        let busy = session_doc == Some(id) || floating_doc == Some(id) || text_doc == Some(id);
        let Some(sess) = entry.share.as_mut() else { continue };
        let now = Instant::now();
        sess.cursors.retain(|_, c| now.duration_since(c.at).as_secs_f32() < CURSOR_TTL_SECS);
        if sess.synced.is_none() || !sess.joined {
            continue;
        }
        if !busy {
            diff_and_queue(entry, &mut ops);
        }
        let sess = entry.share.as_mut().unwrap();
        if active == Some(id) {
            if let Some(p) = hover {
                let moved = sess.last_cursor.1.is_none_or(|q| (q.x - p.x).abs() > 0.5 || (q.y - p.y).abs() > 0.5);
                if moved && now.duration_since(sess.last_cursor.0).as_millis() >= CURSOR_INTERVAL_MS {
                    sess.last_cursor = (now, Some(p));
                    let c = conv_color(&my_id);
                    outgoing.push(Outbound::Op {
                        r#ref: 0,
                        conv: sess.conv.id.clone(),
                        group: sess.conv.group,
                        kind: kind::CURSOR,
                        points: vec![p.x, p.y],
                        color: format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()),
                        text: tool.to_string(),
                        image: None,
                    });
                }
            }
        }
    }

    let mut acks: Vec<Ack> = Vec::new();
    {
        let link = state.share_link(ctx);
        for m in outgoing {
            link.send(m);
        }
        for (conv, k, bytes, size, tiles) in ops {
            let r = send_state_op(link, &conv, k, bytes, size);
            if !tiles.is_empty() {
                acks.push((conv.id, r, tiles));
            }
        }
    }
    for (conv, r, tiles) in acks {
        if let Some(s) = state.docs.iter_mut().filter_map(|d| d.share.as_mut()).find(|s| s.conv.id == conv) {
            s.inflight.insert(r, tiles);
        }
    }
}

/// Record a participant's pointer. Returns their name the first time they
/// show up (or come back after a while away), for a "X is drawing" note.
fn remote_cursor(sess: &mut ShareSession, op: &RemoteOp) -> Option<String> {
    if op.points.len() < 2 {
        return None;
    }
    let newcomer = !sess.cursors.contains_key(&op.author)
        && sess.seen.get(&op.author).is_none_or(|t| t.elapsed().as_secs_f32() > 120.0);
    sess.seen.insert(op.author.clone(), Instant::now());
    let color = qsketch_core::Rgba8::from_hex(&op.color)
        .map(|c| Color32::from_rgb(c.r, c.g, c.b))
        .unwrap_or_else(|| conv_color(&op.author));
    let name = if op.by.is_empty() { op.author.chars().take(8).collect() } else { op.by.clone() };
    sess.cursors.insert(
        op.author.clone(),
        RemoteCursor {
            name: name.clone(),
            pos: Pt::new(op.points[0], op.points[1]),
            color,
            tool: op.text.clone(),
            at: Instant::now(),
        },
    );
    newcomer.then_some(name)
}

/// Ship whatever the committed state changed since the last sync.
fn diff_and_queue(entry: &mut DocEntry, ops: &mut Vec<QueuedOp>) {
    let sess = entry.share.as_mut().unwrap();
    let cur = entry.doc.history.current();
    let cur_id = entry.doc.history.current_id();
    let synced = sess.synced.as_ref().unwrap();
    let props_changed = cur.layers.len() != synced.layers.len()
        || cur.layers.iter().zip(&synced.layers).any(|(a, b)| a.props != b.props);
    if cur_id == sess.synced_id && !props_changed {
        return;
    }
    let size = (cur.width, cur.height);
    let conv = sess.conv.clone();
    if size != (synced.width, synced.height) || sess.patches_since_snapshot >= SNAPSHOT_EVERY {
        // A resize is a new base for everyone; so is a periodic refresh.
        if let Ok(bytes) = codec::encode_snapshot(cur, &entry.doc.title) {
            ops.push((conv, kind::SNAPSHOT, bytes, Some(size), Vec::new()));
        }
        sess.patches_since_snapshot = 0;
        sess.synced = Some(cur.clone());
        sess.synced_id = cur_id;
        return;
    }
    if props_changed {
        ops.push((conv.clone(), kind::LAYERS, codec::encode_layers(cur), Some(size), Vec::new()));
    }
    for l in &cur.layers {
        if l.is_group() {
            continue;
        }
        let old = synced.layers.iter().find(|s| s.props.id == l.props.id);
        let mask_same = match (old.and_then(|o| o.mask.as_ref()), l.mask.as_ref()) {
            (None, None) => true,
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
            _ => false,
        };
        if !mask_same {
            if let Ok(bytes) = codec::encode_mask_patch(cur, l.props.id) {
                ops.push((conv.clone(), kind::PATCH, bytes, Some(size), Vec::new()));
            }
        }
        let changed: Vec<usize> = (0..l.raster.tile_count())
            .filter(|&i| match old {
                Some(o) => !o.raster.tile_ptr_eq(&l.raster, i),
                None => l.raster.tile_at_index(i).is_some(),
            })
            .collect();
        for chunk in changed.chunks(PATCH_TILES) {
            if let Ok(bytes) = codec::encode_patch(cur, l.props.id, chunk) {
                let tiles = chunk.iter().map(|&i| (l.props.id, i)).collect();
                ops.push((conv.clone(), kind::PATCH, bytes, Some(size), tiles));
                sess.patches_since_snapshot += 1;
            }
        }
    }
    sess.synced = Some(cur.clone());
    sess.synced_id = cur_id;
}

fn apply_remote(entry: &mut DocEntry, undo_limit: usize, op: RemoteOp) {
    let Some(bytes) = op.bytes() else { return };
    match op.kind {
        kind::SNAPSHOT => {
            let sess = entry.share.as_mut().unwrap();
            if sess.synced.is_some() {
                return; // we already have the picture; patches keep us current
            }
            let Ok((state, title)) = codec::decode_snapshot(&bytes) else { return };
            let (w, h) = (state.width, state.height);
            let title = if title.is_empty() { sess.conv.name.clone() } else { title };
            entry.doc = Document::from_state(state.clone(), format!("{title} (shared)"), None, "Joined");
            entry.doc.history.set_limit(undo_limit);
            randomize_layer_ids(&mut entry.doc);
            entry.needs_full_upload = true;
            entry.sel_outline = None;
            entry.sel_tint = None;
            entry.selected.clear();
            entry.generation += 1;
            entry.view.fit(w, h);
            let sess = entry.share.as_mut().unwrap();
            sess.synced = Some(entry.doc.history.current().clone());
            sess.synced_id = entry.doc.history.current_id();
            sess.tile_lc.clear();
        }
        kind::PATCH => {
            let Ok(p) = codec::decode_patch(&bytes) else { return };
            if (p.w, p.h) != (entry.doc.width(), entry.doc.height()) {
                return;
            }
            let sess = entry.share.as_mut().unwrap();
            if let Some(new_mask) = p.layer_mask {
                let put = |s: &mut DocState| {
                    if let Some(li) = s.index_of(p.layer) {
                        s.layers[li].mask = new_mask.clone();
                    }
                };
                put(entry.doc.state_mut());
                entry.doc.history.for_each_state_mut(put);
                if let Some(s) = sess.synced.as_mut() {
                    put(s);
                }
                entry.doc.mark_all_dirty();
                entry.generation += 1;
                return;
            }
            let mut set = TileSet::for_size(p.w, p.h);
            let txn = p.w.div_ceil(TILE as u32);
            let mut writes: Vec<(usize, Option<std::sync::Arc<qsketch_core::Tile>>)> = Vec::new();
            for (idx, tile) in p.tiles {
                if sess.tile_lc.get(&(p.layer, idx)).is_some_and(|&l| l > op.lc) {
                    continue; // we (or someone) painted this tile later
                }
                sess.tile_lc.insert((p.layer, idx), op.lc);
                set.insert_index(idx);
                writes.push((idx, tile));
            }
            if writes.is_empty() {
                return;
            }
            let put = |s: &mut DocState| {
                let Some(li) = s.index_of(p.layer) else { return };
                let r = &mut s.layers[li].raster;
                for (idx, tile) in &writes {
                    r.set_tile(*idx as u32 % txn, *idx as u32 / txn, tile.clone());
                }
            };
            put(entry.doc.state_mut());
            entry.doc.history.for_each_state_mut(put);
            if let Some(s) = sess.synced.as_mut() {
                put(s);
            }
            entry.doc.mark_dirty_tiles(&set);
            entry.generation += 1;
        }
        kind::LAYERS => {
            let Ok(h) = codec::decode_layers(&bytes) else { return };
            if (h.w, h.h) != (entry.doc.width(), entry.doc.height()) {
                return;
            }
            let rebuild = |s: &mut DocState| {
                let keep = s.active_layer().props.id;
                let layers: Vec<Layer> = h
                    .layers
                    .iter()
                    .map(|props| {
                        let raster = s
                            .layers
                            .iter()
                            .find(|l| l.props.id == props.id)
                            .map(|l| l.raster.clone())
                            .unwrap_or_else(|| Raster::new(s.width, s.height));
                        Layer { props: props.clone(), raster, mask: None }
                    })
                    .collect();
                s.layers = layers;
                s.active = s.index_of(keep).or_else(|| s.index_of(h.active)).unwrap_or(0);
            };
            rebuild(entry.doc.state_mut());
            entry.doc.history.for_each_state_mut(rebuild);
            let sess = entry.share.as_mut().unwrap();
            if let Some(s) = sess.synced.as_mut() {
                rebuild(s);
            }
            entry.doc.mark_all_dirty();
            entry.selected.clear();
            entry.generation += 1;
        }
        _ => {}
    }
}

/// The other participants' pointers, drawn Leyline-style: a dot and a name.
pub fn draw_cursors(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(entry) = state.doc(doc_id) else { return };
    let Some(sess) = entry.share.as_ref() else { return };
    if sess.waiting() {
        let rect = painter.clip_rect();
        let msg = format!("Waiting for {}'s shared canvas…", sess.conv.name);
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(90));
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(15.0), Color32::WHITE);
        return;
    }
    let font = egui::FontId::proportional(12.0);
    for c in sess.cursors.values() {
        let pos = entry.view.doc_to_screen(c.pos);
        painter.circle_filled(pos, 5.0, c.color);
        painter.circle_stroke(pos, 5.0, egui::Stroke::new(1.0, Color32::from_black_alpha(160)));
        let label = if c.tool.is_empty() { c.name.clone() } else { format!("{} · {}", c.name, c.tool) };
        let galley = painter.layout_no_wrap(label, font.clone(), Color32::WHITE);
        let at = pos + egui::vec2(9.0, -galley.size().y / 2.0);
        let bg = egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(4.0, 2.0));
        crate::ui::chrome::fill_box(painter, bg, 3.0, c.color.gamma_multiply(0.9));
        painter.galley(at, galley, Color32::WHITE);
    }
}
