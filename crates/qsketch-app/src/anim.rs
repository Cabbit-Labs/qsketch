//! Animation in the app: playback, the frame / tag / cel commands behind the
//! Animation menu and the Timeline panel, and the import / export jobs.

use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

use qsketch_core::anim::{NewFrame, TagDirection};
use qsketch_core::io::anim_io::{self, SheetOptions};
use qsketch_core::{DocState, Raster, Rgba8};

use crate::state::{AppState, BackgroundJob, DocId};
use crate::ui::toasts::Level;
use crate::workspace::PanelKind;

/// A running playback.
pub struct Playback {
    pub doc: DocId,
    last: Instant,
    carry_ms: f32,
    forward: bool,
    loops: u16,
    /// The tag being looped, when playback started inside one (and the
    /// loop-tag setting is on).
    pub tag: Option<usize>,
}

/// Transient Timeline panel state (nothing here is saved).
#[derive(Default)]
pub struct TimelineUi {
    pub drag: Option<TimelineDrag>,
    /// What the open context menu is about.
    pub menu: Option<TimelineHit>,
    /// The frame the timeline last scrolled to, so it only follows changes.
    pub followed_frame: Option<(DocId, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineDrag {
    /// Press in the frame header: the frame follows the pointer and the
    /// selection grows from `anchor`.
    Scrub { anchor: usize },
    /// Dragging the selected frames to a new place (`to` = insertion index
    /// in the list without them).
    MoveFrames { to: usize },
    /// Eye sweep over layer rows: every eye crossed gets this visibility.
    Eyes(bool),
}

/// Where a pointer position lands in the timeline grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineHit {
    Frame(usize),
    Tag(usize),
    Layer(usize),
    Cel { layer: usize, frame: usize },
    Nothing,
}

/// Frame changes are safe only when nothing is half-done on the canvas.
pub fn can_change_frame(state: &AppState) -> bool {
    state.session.is_none() && state.floating.is_none() && state.text_edit.is_none()
}

/// The frames the timeline has selected (the current one when none).
pub fn selected_frames(state: &AppState, doc: DocId) -> Vec<usize> {
    let Some(e) = state.doc(doc) else { return Vec::new() };
    let n = e.doc.state().frame_count();
    match e.frame_sel {
        Some((a, b)) => (a.min(b).min(n - 1)..=a.max(b).min(n - 1)).collect(),
        None => vec![e.doc.frame().min(n - 1)],
    }
}

/// Show `frame`; the selection collapses to it unless `extend` keeps the
/// anchor (Shift+click).
pub fn set_frame(state: &mut AppState, doc: DocId, frame: usize, extend: bool) {
    if !can_change_frame(state) {
        return;
    }
    stop(state);
    let Some(e) = state.doc_mut(doc) else { return };
    let n = e.doc.state().frame_count();
    let frame = frame.min(n - 1);
    e.doc.set_frame(frame);
    e.frame_sel = match (extend, e.frame_sel) {
        (true, Some((a, _))) => Some((a, frame)),
        (true, None) => Some((e.doc.frame(), frame)),
        _ => None,
    };
}

/// Step by `delta` frames, wrapping around the ends (or the current tag's
/// ends while the loop-tag setting is on).
pub fn step(state: &mut AppState, doc: DocId, delta: isize) {
    let Some(e) = state.doc(doc) else { return };
    let s = e.doc.state();
    let n = s.frame_count() as isize;
    let (lo, hi) = match s.tag_at(s.frame) {
        Some(t) if state.settings.anim.loop_tag => (s.tags[t].from as isize, s.tags[t].to as isize),
        _ => (0, n - 1),
    };
    let len = hi - lo + 1;
    let f = lo + (s.frame as isize - lo + delta).rem_euclid(len.max(1));
    set_frame(state, doc, f.clamp(0, n - 1) as usize, false);
}

pub fn first_frame(state: &mut AppState, doc: DocId) {
    let lo = state.doc(doc).map_or(0, |e| {
        let s = e.doc.state();
        match s.tag_at(s.frame) {
            Some(t) if state.settings.anim.loop_tag => s.tags[t].from,
            _ => 0,
        }
    });
    set_frame(state, doc, lo, false);
}

pub fn last_frame(state: &mut AppState, doc: DocId) {
    let hi = state.doc(doc).map_or(0, |e| {
        let s = e.doc.state();
        match s.tag_at(s.frame) {
            Some(t) if state.settings.anim.loop_tag => s.tags[t].to,
            _ => s.frame_count() - 1,
        }
    });
    set_frame(state, doc, hi, false);
}

// --- playback --------------------------------------------------------------

pub fn is_playing(state: &AppState, doc: DocId) -> bool {
    state.playback.as_ref().is_some_and(|p| p.doc == doc)
}

pub fn toggle_play(state: &mut AppState, doc: DocId) {
    if is_playing(state, doc) {
        stop(state);
        return;
    }
    if !can_change_frame(state) {
        return;
    }
    let Some(e) = state.doc(doc) else { return };
    let s = e.doc.state();
    if !s.is_animated() {
        state.toasts.push(Level::Info, "Add a frame first (Animation › New Frame, Alt+N).");
        return;
    }
    let tag = if state.settings.anim.loop_tag { s.tag_at(s.frame) } else { None };
    let forward =
        !matches!(tag.map(|t| s.tags[t].direction), Some(TagDirection::Reverse) | Some(TagDirection::PingPongReverse));
    state.playback = Some(Playback { doc, last: Instant::now(), carry_ms: 0.0, forward, loops: 0, tag });
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = None;
    }
}

pub fn stop(state: &mut AppState) {
    state.playback = None;
}

/// Advance a running playback by the time since the last frame; called
/// once per UI frame.
pub fn tick(state: &mut AppState, ctx: &egui::Context) {
    if state.playback.is_none() {
        return;
    }
    // A stroke or a paste in progress ends playback (the frame must hold still).
    if !can_change_frame(state) {
        stop(state);
        return;
    }
    let Some(pb) = state.playback.as_mut() else { return };
    let Some(e) = state.docs.iter_mut().find(|d| d.id == pb.doc) else {
        state.playback = None;
        return;
    };
    let now = Instant::now();
    pb.carry_ms += now.duration_since(pb.last).as_secs_f32() * 1000.0;
    pb.last = now;
    let s = e.doc.state();
    let n = s.frame_count();
    if n < 2 {
        state.playback = None;
        return;
    }
    let (lo, hi, dir, repeat) = match pb.tag.and_then(|t| s.tags.get(t)) {
        Some(t) => (t.from.min(n - 1), t.to.min(n - 1), t.direction, t.repeat),
        None => (0, n - 1, TagDirection::Forward, 0),
    };
    let mut frame = s.frame.clamp(lo, hi);
    let mut finished = false;
    // Catch up in whole frames; a long stall skips ahead rather than racing.
    let mut guard = 0;
    while pb.carry_ms >= s.frame_duration(frame) as f32 && guard < 1000 {
        pb.carry_ms -= s.frame_duration(frame) as f32;
        guard += 1;
        let next = match dir {
            TagDirection::Forward => {
                if frame >= hi {
                    pb.loops += 1;
                    lo
                } else {
                    frame + 1
                }
            }
            TagDirection::Reverse => {
                if frame <= lo {
                    pb.loops += 1;
                    hi
                } else {
                    frame - 1
                }
            }
            TagDirection::PingPong | TagDirection::PingPongReverse => {
                if pb.forward {
                    if frame >= hi {
                        pb.forward = false;
                        if dir == TagDirection::PingPongReverse {
                            pb.loops += 1;
                        }
                        frame.saturating_sub(1).max(lo)
                    } else {
                        frame + 1
                    }
                } else if frame <= lo {
                    pb.forward = true;
                    if dir == TagDirection::PingPong {
                        pb.loops += 1;
                    }
                    (frame + 1).min(hi)
                } else {
                    frame - 1
                }
            }
        };
        if repeat > 0 && pb.loops >= repeat {
            finished = true;
            break;
        }
        frame = next;
    }
    let dur = s.frame_duration(frame) as f32;
    if frame != s.frame {
        e.doc.set_frame(frame);
    }
    if finished {
        state.playback = None;
        return;
    }
    let wait = (dur - pb.carry_ms).clamp(1.0, 1000.0);
    ctx.request_repaint_after(Duration::from_millis(wait as u64));
}

// --- frames ----------------------------------------------------------------

fn edit(state: &mut AppState, doc: DocId, label: &str, f: impl FnOnce(&mut DocState)) {
    if !can_change_frame(state) {
        state.settle();
    }
    stop(state);
    let was_animated = state.doc(doc).is_some_and(|e| e.doc.state().is_animated());
    let Some(e) = state.doc_mut(doc) else { return };
    f(e.doc.state_mut());
    e.doc.mark_all_dirty();
    e.doc.commit(label);
    e.frame_sel = None;
    let now_animated = e.doc.state().is_animated();
    if now_animated && !was_animated {
        state.show_panel_requests.push(PanelKind::Timeline);
    }
}

/// New Frame (a copy of the current one) after the current frame.
pub fn new_frame(state: &mut AppState, doc: DocId, empty: bool) {
    edit(state, doc, if empty { "New Empty Frame" } else { "New Frame" }, |s| {
        let cur = s.frame;
        s.insert_frame(cur + 1, if empty { NewFrame::Empty } else { NewFrame::Duplicate(cur) });
    });
}

/// Duplicate the selected frames after the last of them.
pub fn duplicate_frames(state: &mut AppState, doc: DocId) {
    let sel = selected_frames(state, doc);
    if sel.is_empty() {
        return;
    }
    edit(state, doc, "Duplicate Frames", |s| {
        let at = sel[sel.len() - 1] + 1;
        for (k, &f) in sel.iter().enumerate() {
            s.insert_frame(at + k, NewFrame::Duplicate(f));
        }
        s.set_frame(at);
    });
}

pub fn delete_frames(state: &mut AppState, doc: DocId) {
    let sel = selected_frames(state, doc);
    let n = state.doc(doc).map_or(1, |e| e.doc.state().frame_count());
    if n <= 1 {
        state.toasts.push(Level::Info, "A document keeps at least one frame.");
        return;
    }
    edit(state, doc, if sel.len() > 1 { "Delete Frames" } else { "Delete Frame" }, |s| {
        s.remove_frames(&sel);
    });
}

pub fn reverse_frames(state: &mut AppState, doc: DocId) {
    let sel = selected_frames(state, doc);
    let (Some(&a), Some(&b)) = (sel.first(), sel.last()) else { return };
    if a == b {
        state.toasts.push(Level::Info, "Select a run of frames in the Timeline to reverse.");
        return;
    }
    edit(state, doc, "Reverse Frames", |s| s.reverse_frames(a, b));
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = Some((a, b));
    }
}

pub fn move_frames(state: &mut AppState, doc: DocId, which: &[usize], to: usize) {
    let n = which.len();
    edit(state, doc, "Move Frames", |s| s.move_frames(which, to));
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = Some((to, to + n.saturating_sub(1)));
    }
}

pub fn set_duration(state: &mut AppState, doc: DocId, frames: &[usize], ms: u32) {
    let unchanged = state.doc(doc).is_some_and(|e| frames.iter().all(|&f| e.doc.state().frame_duration(f) == ms));
    if unchanged {
        return;
    }
    let keep = state.doc(doc).and_then(|e| e.frame_sel);
    edit(state, doc, "Frame Duration", |s| {
        for &f in frames {
            s.set_frame_duration(f, ms);
        }
    });
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = keep;
    }
}

// --- cels ------------------------------------------------------------------

pub fn clear_cel(state: &mut AppState, doc: DocId) {
    let sel = selected_frames(state, doc);
    edit(state, doc, "Clear Cel", |s| {
        let li = s.active;
        for &f in &sel {
            s.clear_cel(li, f);
        }
    });
}

pub fn link_cels(state: &mut AppState, doc: DocId) {
    let sel = selected_frames(state, doc);
    if sel.len() < 2 {
        state.toasts.push(Level::Info, "Select two or more frames in the Timeline to link their cels.");
        return;
    }
    edit(state, doc, "Link Cels", |s| {
        let li = s.active;
        s.link_cels(li, &sel);
    });
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = Some((sel[0], sel[sel.len() - 1]));
    }
}

pub fn unlink_cel(state: &mut AppState, doc: DocId) {
    let sel = selected_frames(state, doc);
    edit(state, doc, "Unlink Cel", |s| {
        let li = s.active;
        for &f in &sel {
            s.unlink_cel(li, f);
        }
    });
}

pub fn copy_cel(state: &mut AppState, doc: DocId) {
    let Some(e) = state.doc(doc) else { return };
    let s = e.doc.state();
    let li = s.active;
    if !s.layers[li].animated() {
        state.toasts.push(Level::Info, "Only a pixel layer's cel can be copied.");
        return;
    }
    let image = s.cel_image(li, s.frame).cloned().unwrap_or_else(|| Raster::new(s.width, s.height));
    let opacity = s.layers[li].cel_opacity(s.frame);
    state.cel_clipboard = Some((image, opacity));
    state.toasts.push(Level::Info, "Cel copied.");
}

pub fn paste_cel(state: &mut AppState, doc: DocId) {
    let Some((image, opacity)) = state.cel_clipboard.clone() else {
        state.toasts.push(Level::Info, "Copy a cel first (Animation › Copy Cel).");
        return;
    };
    let sel = selected_frames(state, doc);
    edit(state, doc, "Paste Cel", |s| {
        let li = s.active;
        if !s.layers[li].animated() {
            return;
        }
        let (w, h) = (s.width, s.height);
        let image = if image.width() != w || image.height() != h { image.with_canvas_size(w, h, 0, 0) } else { image };
        for &f in &sel {
            s.set_cel_image(li, f, image.clone());
            if let Some(c) = s.layers[li].cel_mut(f) {
                c.opacity = opacity;
            }
        }
    });
}

pub fn toggle_continuous(state: &mut AppState, doc: DocId) {
    let keep = state.doc(doc).and_then(|e| e.frame_sel);
    edit(state, doc, "Continuous Layer", |s| {
        let l = s.active_layer_mut();
        l.props.continuous = !l.props.continuous;
    });
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = keep;
    }
}

// --- tags ------------------------------------------------------------------

/// Start a tag over the selected frames and open its properties.
pub fn new_tag(state: &mut AppState, doc: DocId) {
    let sel = selected_frames(state, doc);
    let (Some(&a), Some(&b)) = (sel.first(), sel.last()) else { return };
    let Some(e) = state.doc(doc) else { return };
    let s = e.doc.state();
    let color = qsketch_core::anim::TAG_COLORS[s.tags.len() % qsketch_core::anim::TAG_COLORS.len()];
    let tag = qsketch_core::Tag::new(s.unique_tag_name(), a, b, color);
    state.dialogs.tag_props = Some(crate::dialogs::anim::TagPropsDialog { doc, index: None, tag });
}

pub fn tag_properties(state: &mut AppState, doc: DocId, index: Option<usize>) {
    let Some(e) = state.doc(doc) else { return };
    let s = e.doc.state();
    let index = index.or_else(|| s.tag_at(s.frame));
    let Some(i) = index else {
        state.toasts.push(Level::Info, "The current frame isn't inside a tag. Animation › New Tag makes one.");
        return;
    };
    let Some(tag) = s.tags.get(i).cloned() else { return };
    state.dialogs.tag_props = Some(crate::dialogs::anim::TagPropsDialog { doc, index: Some(i), tag });
}

pub fn delete_tag(state: &mut AppState, doc: DocId, index: Option<usize>) {
    let index = index.or_else(|| {
        let e = state.doc(doc)?;
        let s = e.doc.state();
        s.tag_at(s.frame)
    });
    let Some(i) = index else {
        state.toasts.push(Level::Info, "The current frame isn't inside a tag.");
        return;
    };
    let keep = state.doc(doc).and_then(|e| e.frame_sel);
    edit(state, doc, "Delete Tag", |s| {
        if i < s.tags.len() {
            s.tags.remove(i);
        }
    });
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = keep;
    }
}

/// Select a tag's frames (and go to its first frame when outside it).
pub fn select_tag(state: &mut AppState, doc: DocId, index: usize) {
    let Some(e) = state.doc(doc) else { return };
    let s = e.doc.state();
    let Some(t) = s.tags.get(index) else { return };
    let (from, to, inside) = (t.from, t.to, t.contains(s.frame));
    if !inside {
        set_frame(state, doc, from, false);
    } else {
        stop(state);
    }
    if let Some(e) = state.doc_mut(doc) {
        e.frame_sel = Some((from, to));
    }
}

// --- frame / cel properties ---------------------------------------------------

pub fn frame_properties(state: &mut AppState, doc: DocId) {
    let sel = selected_frames(state, doc);
    let Some(e) = state.doc(doc) else { return };
    let ms = e.doc.state().frame_duration(sel.first().copied().unwrap_or(0));
    state.dialogs.frame_props = Some(crate::dialogs::anim::FramePropsDialog { doc, frames: sel, duration_ms: ms });
}

pub fn cel_properties(state: &mut AppState, doc: DocId) {
    let Some(e) = state.doc(doc) else { return };
    let s = e.doc.state();
    let l = s.active_layer();
    if !l.animated() || !s.is_animated() {
        state.toasts.push(Level::Info, "Cel properties belong to a pixel layer of an animation.");
        return;
    }
    let frame = s.frame;
    let (opacity, z) = l.cels.get(l.cel_owner(frame)).map_or((1.0, 0), |c| (c.opacity, c.z_index));
    state.dialogs.cel_props =
        Some(crate::dialogs::anim::CelPropsDialog { doc, layer: l.props.id, frame, opacity, z_index: z });
}

// --- import ----------------------------------------------------------------

/// Pick image files and add each as a frame after the current one (into
/// the active layer), or as a new document when none is open.
pub fn import_frames(state: &mut AppState) {
    let files = rfd::FileDialog::new()
        .set_title("Import Frames (one file per frame)")
        .add_filter("Images", &["png", "gif", "jpg", "jpeg", "bmp", "tga", "webp", "tif", "tiff", "apng"])
        .pick_files();
    let Some(mut files) = files else { return };
    files.sort();
    let mut pictures: Vec<(Raster, u32)> = Vec::new();
    let default_ms = state.settings.anim.default_duration_ms.max(1);
    for p in &files {
        let animated =
            anim_io::might_be_animated(p).then(|| anim_io::import_animated(p)).and_then(|r| r.ok()).flatten();
        match animated {
            Some(doc) => {
                for f in 0..doc.frame_count() {
                    pictures.push((qsketch_core::anim::render_frame(&doc, f), doc.frame_duration(f)));
                }
            }
            None => match qsketch_core::io::image_io::import(p) {
                Ok(r) => pictures.push((r, default_ms)),
                Err(e) => state.toasts.push(Level::Error, format!("Couldn't read {}: {e:#}", p.display())),
            },
        }
    }
    if pictures.is_empty() {
        return;
    }
    let count = pictures.len();
    match state.active_doc {
        Some(doc) => {
            let ok = state.doc(doc).is_some_and(|e| e.doc.state().active_layer().animated());
            if !ok {
                state.toasts.push(Level::Info, "Pick a pixel layer to import frames into.");
                return;
            }
            edit(state, doc, "Import Frames", |s| {
                let li = s.active;
                let (w, h) = (s.width, s.height);
                let mut at = s.frame + 1;
                for (pic, ms) in pictures {
                    s.insert_frame(at, NewFrame::Empty);
                    s.frames[at].duration_ms = ms.max(1);
                    let pic =
                        if pic.width() != w || pic.height() != h { pic.with_canvas_size(w, h, 0, 0) } else { pic };
                    s.layers[li].raster = pic;
                    at += 1;
                }
                s.set_frame(at - 1);
            });
            state.toasts.push(Level::Success, format!("Imported {count} frames."));
        }
        _ => {
            let name = files[0].file_stem().and_then(|s| s.to_str()).unwrap_or("Frames").to_string();
            let doc = anim_io::from_pictures(&name, pictures);
            let title = state.untitled_title();
            let d = qsketch_core::Document::from_state(doc, title, None, "Import Frames");
            state.add_document(d);
            state.show_panel_requests.push(PanelKind::Timeline);
        }
    }
}

/// Pick a sprite sheet and open the dialog that cuts it into frames.
pub fn import_sheet(state: &mut AppState) {
    let file = rfd::FileDialog::new()
        .set_title("Import Sprite Sheet")
        .add_filter("Images", &["png", "gif", "jpg", "jpeg", "bmp", "tga", "webp", "tif", "tiff"])
        .pick_file();
    let Some(path) = file else { return };
    match qsketch_core::io::image_io::import(&path) {
        Ok(sheet) => {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Sheet").to_string();
            let cell = state.settings.canvas.grid_size.max(1);
            let (cell_w, cell_h) = (cell.min(sheet.width()), cell.min(sheet.height()));
            state.dialogs.import_sheet = Some(crate::dialogs::anim::ImportSheetDialog {
                name,
                sheet: Arc::new(sheet),
                cell_w,
                cell_h,
                count: 0,
                duration_ms: state.settings.anim.default_duration_ms.max(1),
            });
        }
        Err(e) => state.toasts.push(Level::Error, format!("Couldn't read {}: {e:#}", path.display())),
    }
}

// --- export ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ExportFormat {
    #[default]
    Gif,
    Apng,
    Sequence,
    Sheet,
    Mp4,
    WebM,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 6] = [
        ExportFormat::Gif,
        ExportFormat::Apng,
        ExportFormat::Sheet,
        ExportFormat::Sequence,
        ExportFormat::Mp4,
        ExportFormat::WebM,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Gif => "Animated GIF",
            ExportFormat::Apng => "Animated PNG (APNG)",
            ExportFormat::Sequence => "PNG sequence (folder)",
            ExportFormat::Sheet => "Sprite sheet (PNG + JSON)",
            ExportFormat::Mp4 => "MP4 video (ffmpeg)",
            ExportFormat::WebM => "WebM video (ffmpeg)",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Gif => "gif",
            ExportFormat::Apng | ExportFormat::Sequence | ExportFormat::Sheet => "png",
            ExportFormat::Mp4 => "mp4",
            ExportFormat::WebM => "webm",
        }
    }
    pub fn needs_ffmpeg(self) -> bool {
        matches!(self, ExportFormat::Mp4 | ExportFormat::WebM)
    }
}

/// Which frames an export covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportRange {
    All,
    Tag(usize),
    Selected,
}

/// Everything the export job needs.
pub struct ExportSpec {
    pub format: ExportFormat,
    pub scale: u32,
    pub frames: Vec<usize>,
    pub repeat: bool,
    pub sheet: SheetOptions,
    pub video_bg: Rgba8,
    pub target: std::path::PathBuf,
    pub stem: String,
}

/// Run an export on a background thread with a progress job.
pub fn run_export(state: &mut AppState, doc_id: DocId, spec: ExportSpec) {
    let Some(entry) = state.doc(doc_id) else { return };
    let mut doc = entry.doc.state().clone();
    doc.sync_cels();
    let done = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = channel::<Result<String, String>>();
    let total = spec.frames.len();
    let counter = done.clone();
    let spawned = std::thread::Builder::new().name("animation-export".into()).spawn(move || {
        let result = (|| -> anyhow::Result<String> {
            // Rendering fills the bar once, writing fills it again.
            let frames = anim_io::render(&doc, &spec.frames, spec.scale, &counter)?;
            let n = frames.frames.len();
            counter.store(0, std::sync::atomic::Ordering::Relaxed);
            let msg = match spec.format {
                ExportFormat::Gif => {
                    anim_io::write_gif(&frames, &spec.target, spec.repeat, &counter)?;
                    format!("Exported {n} frames to {}", spec.target.display())
                }
                ExportFormat::Apng => {
                    anim_io::write_apng(&frames, &spec.target, spec.repeat, &counter)?;
                    format!("Exported {n} frames to {}", spec.target.display())
                }
                ExportFormat::Sequence => {
                    let paths = anim_io::write_sequence(&frames, &spec.target, &spec.stem, &counter)?;
                    format!("Exported {} frames to {}", paths.len(), spec.target.display())
                }
                ExportFormat::Sheet => {
                    anim_io::write_sheet(&frames, &spec.target, &spec.sheet, &doc, &spec.stem, &counter)?;
                    let json = if spec.sheet.json { " (+ .json)" } else { "" };
                    format!("Exported a {n}-frame sheet to {}{json}", spec.target.display())
                }
                ExportFormat::Mp4 | ExportFormat::WebM => {
                    anim_io::write_video(&frames, &spec.target, spec.video_bg, &counter)?;
                    format!("Exported {n} frames to {}", spec.target.display())
                }
            };
            Ok(msg)
        })();
        let _ = tx.send(result.map_err(|e| format!("Animation export failed: {e:#}")));
    });
    match spawned {
        Ok(_) => state.jobs.push(BackgroundJob { label: "Exporting animation".into(), done, total, rx }),
        Err(e) => state.toasts.push(Level::Error, format!("Couldn't start the export: {e}")),
    }
}
