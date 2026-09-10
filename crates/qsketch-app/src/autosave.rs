//! Crash recovery: unsaved documents are snapshotted to
//! `<config>/autosave/<pid>-<doc>.qsk` (plus a small `.json` sidecar with the
//! title and original path) on a timer, written from a worker thread. On the
//! next launch, snapshots left behind by a process that is no longer running
//! are offered for recovery. Snapshots are removed when the document is saved
//! or closed normally.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use qsketch_core::{io, Document};
use serde::{Deserialize, Serialize};

use crate::settings::{GeneralSettings, Settings};
use crate::state::{AppState, DocEntry, DocId};
use crate::ui::toasts::Level;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub pid: u32,
    pub title: String,
    pub path: Option<PathBuf>,
    /// Unix seconds when the snapshot was written.
    pub written: u64,
}

/// A snapshot found on disk that no running qsketch owns.
#[derive(Clone, Debug)]
pub struct Recoverable {
    pub qsk: PathBuf,
    pub meta: Meta,
}

pub fn dir() -> Option<PathBuf> {
    Settings::config_dir().map(|d| d.join("autosave"))
}

fn slug(doc_id: DocId) -> String {
    format!("{}-{}", std::process::id(), doc_id)
}

fn paths_for(doc_id: DocId) -> Option<(PathBuf, PathBuf)> {
    let d = dir()?;
    let s = slug(doc_id);
    Some((d.join(format!("{s}.qsk")), d.join(format!("{s}.json"))))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Delete this document's snapshot (after a save or a clean close).
pub fn remove(doc_id: DocId) {
    if let Some((qsk, json)) = paths_for(doc_id) {
        let _ = std::fs::remove_file(qsk);
        let _ = std::fs::remove_file(json);
    }
}

fn remove_files(qsk: &Path) {
    let _ = std::fs::remove_file(qsk);
    let _ = std::fs::remove_file(qsk.with_extension("json"));
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    pid != 0 && Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    // Without a cheap liveness probe, assume a stale pid; the worst case is
    // offering a snapshot that another running instance also holds.
    false
}

/// Snapshots on disk whose owning process is gone.
pub fn scan() -> Vec<Recoverable> {
    let Some(d) = dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&d) else { return Vec::new() };
    let me = std::process::id();
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("qsk") {
            continue;
        }
        let meta: Meta =
            match std::fs::read(p.with_extension("json")).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
                Some(m) => m,
                None => Meta {
                    pid: 0,
                    title: p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                    path: None,
                    written: 0,
                },
            };
        if meta.pid == me || process_alive(meta.pid) {
            continue;
        }
        // A half-written snapshot (crash during the write) is useless.
        if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) == 0 {
            remove_files(&p);
            continue;
        }
        out.push(Recoverable { qsk: p, meta });
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.meta.written));
    out
}

/// Open a snapshot as a modified document pointing at its original path.
pub fn recover(state: &mut AppState, r: &Recoverable) -> Option<DocId> {
    match io::qsk::load(&r.qsk) {
        Ok(ds) => {
            let path = r.meta.path.clone().filter(|p| io::is_document(p));
            // An untitled snapshot must not collide with this session's own Untitled-N.
            let taken = path.is_none() && state.docs.iter().any(|d| d.doc.title == r.meta.title);
            let title = if r.meta.title.is_empty() || taken { state.untitled_title() } else { r.meta.title.clone() };
            let mut doc = Document::from_state(ds, title, path.clone(), "Recovered");
            doc.mark_unsaved();
            // The file may already be open (e.g. reopened after an update
            // relaunch); the snapshot is newer, so it takes that tab's place.
            if let Some(p) = &path {
                let dup: Vec<DocId> =
                    state.docs.iter().filter(|d| d.doc.path.as_deref() == Some(p.as_path())).map(|d| d.id).collect();
                for id in dup {
                    crate::files::force_close(state, id);
                }
            }
            let id = state.add_document(doc);
            remove_files(&r.qsk);
            Some(id)
        }
        Err(e) => {
            state.toasts.push(Level::Error, format!("Couldn't recover \"{}\": {e:#}", r.meta.title));
            None
        }
    }
}

pub fn discard(r: &Recoverable) {
    remove_files(&r.qsk);
}

/// The periodic snapshot driver, owned by the app.
pub struct Autosave {
    last_run: Instant,
    busy: Arc<AtomicBool>,
    /// History cursor each document had when last snapshotted.
    snapshotted: std::collections::HashMap<DocId, usize>,
}

impl Default for Autosave {
    fn default() -> Self {
        Self { last_run: Instant::now(), busy: Arc::new(AtomicBool::new(false)), snapshotted: Default::default() }
    }
}

impl Autosave {
    /// Call once per frame. Writes at most one document per interval tick so
    /// a multi-document session never stalls the UI thread.
    pub fn tick(&mut self, docs: &[DocEntry], g: &GeneralSettings, session_active: bool, ctx: &egui::Context) {
        if !g.autosave {
            return;
        }
        let interval = Duration::from_secs(g.autosave_interval_secs.max(15) as u64);
        let elapsed = self.last_run.elapsed();
        if elapsed < interval {
            // Keep a frame scheduled so an idle window still autosaves.
            ctx.request_repaint_after(interval - elapsed);
            return;
        }
        // Mid-stroke snapshots would capture a half-painted working state.
        if session_active || self.busy.load(Ordering::Relaxed) {
            ctx.request_repaint_after(Duration::from_secs(2));
            return;
        }
        self.last_run = Instant::now();
        ctx.request_repaint_after(interval);
        let Some(d) = dir() else { return };
        for e in docs {
            let cursor = e.doc.history.cursor();
            if !e.doc.is_modified() {
                if self.snapshotted.remove(&e.id).is_some() {
                    remove(e.id);
                }
                continue;
            }
            if self.snapshotted.get(&e.id) == Some(&cursor) {
                continue;
            }
            let Some((qsk, json)) = paths_for(e.id) else { continue };
            let meta = Meta {
                pid: std::process::id(),
                title: e.doc.title.clone(),
                path: e.doc.path.clone(),
                written: now_secs(),
            };
            // DocState is copy-on-write per tile, so this clone is cheap.
            let snapshot = e.doc.state().clone();
            self.snapshotted.insert(e.id, cursor);
            let busy = self.busy.clone();
            busy.store(true, Ordering::Relaxed);
            let repaint = ctx.clone();
            std::thread::Builder::new()
                .name("autosave".into())
                .spawn(move || {
                    let res = std::fs::create_dir_all(&d)
                        .map_err(anyhow::Error::from)
                        .and_then(|_| io::qsk::save(&qsk, &snapshot))
                        .and_then(|_| Ok(std::fs::write(&json, serde_json::to_vec(&meta)?)?));
                    if let Err(e) = res {
                        log::warn!("autosave {}: {e:#}", qsk.display());
                    }
                    busy.store(false, Ordering::Relaxed);
                    repaint.request_repaint();
                })
                .ok();
            break;
        }
    }

    /// Snapshot every modified document right now, on this thread. Used before
    /// the updater relaunches so unsaved work is offered for recovery.
    pub fn flush_all(&mut self, docs: &[DocEntry]) {
        let Some(d) = dir() else { return };
        // Let an in-flight background snapshot finish first.
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.busy.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        for e in docs {
            if !e.doc.is_modified() {
                continue;
            }
            let Some((qsk, json)) = paths_for(e.id) else { continue };
            let meta = Meta {
                pid: std::process::id(),
                title: e.doc.title.clone(),
                path: e.doc.path.clone(),
                written: now_secs(),
            };
            let res = std::fs::create_dir_all(&d)
                .map_err(anyhow::Error::from)
                .and_then(|_| io::qsk::save(&qsk, e.doc.state()))
                .and_then(|_| Ok(std::fs::write(&json, serde_json::to_vec(&meta)?)?));
            match res {
                Ok(()) => {
                    self.snapshotted.insert(e.id, e.doc.history.cursor());
                }
                Err(err) => log::warn!("autosave flush {}: {err:#}", qsk.display()),
            }
        }
    }

    /// Forget a document (closed or saved) so its next edit snapshots again.
    pub fn forget(&mut self, doc_id: DocId) {
        self.snapshotted.remove(&doc_id);
        remove(doc_id);
    }
}
