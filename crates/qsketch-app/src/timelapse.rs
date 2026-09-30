//! Timelapse capture and export on the app side: a worker thread encodes the
//! snapshots `qsketch_core::timelapse::snapshot` takes of each recording
//! document as its edits are committed, and exports run on their own thread
//! as background jobs with progress in the status bar.

use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use qsketch_core::timelapse;

use crate::state::{AppState, BackgroundJob, DocId};
use crate::ui::toasts::Level;

struct Job {
    doc: DocId,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

/// Encodes captured frames off the UI thread, in order.
#[derive(Default)]
pub struct Recorder {
    tx: Option<Sender<Job>>,
    rx: Option<Receiver<(DocId, Arc<[u8]>)>>,
}

impl Recorder {
    fn sender(&mut self, ctx: &egui::Context) -> Option<&Sender<Job>> {
        if self.tx.is_none() {
            let (job_tx, job_rx) = channel::<Job>();
            let (out_tx, out_rx) = channel::<(DocId, Arc<[u8]>)>();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new().name("timelapse".into()).spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    match timelapse::encode(job.w, job.h, &job.rgba) {
                        Ok(png) => {
                            if out_tx.send((job.doc, png.into())).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                        Err(e) => log::warn!("timelapse frame: {e:#}"),
                    }
                }
            });
            if spawned.is_err() {
                return None;
            }
            self.tx = Some(job_tx);
            self.rx = Some(out_rx);
        }
        self.tx.as_ref()
    }
}

/// Once per frame, after the UI: snapshot every recording document whose
/// edits have moved on (and whose composite is up to date), and file the
/// frames the worker has finished.
pub fn tick(state: &mut AppState, ctx: &egui::Context) {
    let max_side = state.settings.general.timelapse_size.clamp(128, 2048);
    let AppState { docs, recorder, .. } = state;
    for d in docs.iter_mut() {
        let edits = d.doc.stats.edits;
        if !d.doc.timelapse.due(edits) || d.doc.has_pending_composite() {
            continue;
        }
        d.doc.timelapse.last_edits = Some(edits);
        let (w, h, rgba) = timelapse::snapshot(&d.doc.composite, max_side);
        if let Some(tx) = recorder.sender(ctx) {
            let _ = tx.send(Job { doc: d.id, w, h, rgba });
        }
    }
    if let Some(rx) = &recorder.rx {
        while let Ok((id, png)) = rx.try_recv() {
            if let Some(d) = docs.iter_mut().find(|d| d.id == id) {
                d.doc.timelapse.push(png);
            }
        }
    }
}

/// Turn recording on or off for a document. Starting captures a first frame
/// right away.
pub fn toggle(state: &mut AppState, doc: DocId) {
    if let Some(d) = state.doc_mut(doc) {
        let t = &mut d.doc.timelapse;
        t.recording = !t.recording;
        if t.recording {
            t.last_edits = None;
        }
        let msg = if t.recording {
            "Recording a timelapse of this document. It is saved in the .qsk."
        } else {
            "Timelapse recording paused."
        };
        state.status_msg = Some((msg.to_string(), std::time::Instant::now()));
    }
}

/// Export the document's timelapse as an animated GIF, or as numbered PNG
/// frames into a folder, on a background thread.
pub fn export(state: &mut AppState, doc_id: DocId, gif: bool) {
    let Some(entry) = state.doc(doc_id) else { return };
    let frames = entry.doc.timelapse.frames.clone();
    if frames.is_empty() {
        state.toasts.push(
            Level::Info,
            "This document has no timelapse yet. Turn on File › Timelapse › Record Timelapse, then draw.",
        );
        return;
    }
    let base = entry.doc.title.trim_end_matches('*').to_string();
    let stem = std::path::Path::new(&base).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(base);
    let dir = entry.doc.path.as_ref().and_then(|p| p.parent()).filter(|d| d.exists()).map(|d| d.to_path_buf());
    let fps = state.settings.general.timelapse_fps.clamp(1, 60);
    let target = if gif {
        let mut dlg = rfd::FileDialog::new()
            .set_title("Export Timelapse")
            .add_filter("Animated GIF", &["gif"])
            .set_file_name(format!("{stem} timelapse.gif"));
        if let Some(d) = &dir {
            dlg = dlg.set_directory(d);
        }
        dlg.save_file().map(|mut p| {
            if p.extension().is_none() {
                p.set_extension("gif");
            }
            p
        })
    } else {
        let mut dlg = rfd::FileDialog::new().set_title("Export Timelapse Frames To Folder");
        if let Some(d) = &dir {
            dlg = dlg.set_directory(d);
        }
        dlg.pick_folder().map(|d| d.join(format!("{stem} timelapse")))
    };
    let Some(target) = target else { return };
    let done = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = channel::<Result<String, String>>();
    let total = frames.len();
    let counter = done.clone();
    let spawned = std::thread::Builder::new().name("timelapse-export".into()).spawn(move || {
        let result = if gif {
            timelapse::export_gif(&frames, &target, fps, 4096, 2000, &counter)
                .map(|()| format!("Exported timelapse to {}", target.display()))
        } else {
            timelapse::export_frames(&frames, &target, 4096, &counter)
                .map(|n| format!("Exported {n} timelapse frames to {}", target.display()))
        };
        let _ = tx.send(result.map_err(|e| format!("Timelapse export failed: {e:#}")));
    });
    match spawned {
        Ok(_) => state.jobs.push(BackgroundJob { label: "Exporting timelapse".into(), done, total, rx }),
        Err(e) => state.toasts.push(Level::Error, format!("Couldn't start the export: {e}")),
    }
}

/// Toast finished background jobs and drop them. Returns whether any are
/// still running (the caller keeps repainting for their progress).
pub fn poll_jobs(state: &mut AppState) -> bool {
    let mut finished: Vec<Result<String, String>> = Vec::new();
    state.jobs.retain(|j| match j.rx.try_recv() {
        Ok(r) => {
            finished.push(r);
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            finished.push(Err(format!("{} stopped unexpectedly", j.label)));
            false
        }
    });
    for r in finished {
        match r {
            Ok(m) => state.toasts.push(Level::Success, m),
            Err(e) => state.toasts.push(Level::Error, e),
        }
    }
    !state.jobs.is_empty()
}
