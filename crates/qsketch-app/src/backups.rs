//! Rolling backups of overwritten files: every save over an existing
//! document first copies the old file to `<config>/backups/<path hash>/`,
//! keeping the newest N. File ▸ Restore Previous Version lists them and opens
//! one as a new document so nothing is overwritten by accident.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use qsketch_core::{io, Document};

use crate::settings::Settings;
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

pub fn dir() -> Option<PathBuf> {
    Settings::config_dir().map(|d| d.join("backups"))
}

fn dir_for(path: &Path) -> Option<PathBuf> {
    Some(dir_for_in(&dir()?, path))
}

fn dir_for_in(base: &Path, path: &Path) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().hash(&mut h);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let stem: String = stem.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').take(24).collect();
    base.join(format!("{stem}-{:016x}", h.finish()))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A backed-up version of a file.
#[derive(Clone, Debug)]
pub struct Backup {
    pub file: PathBuf,
    /// Unix seconds when the backup was taken (= when it was overwritten).
    pub written: u64,
    pub bytes: u64,
}

impl Backup {
    /// "3 min ago" style age.
    pub fn age(&self) -> String {
        let secs = now_secs().saturating_sub(self.written);
        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            format!("{} min ago", secs / 60)
        } else if secs < 86400 {
            format!("{} h ago", secs / 3600)
        } else {
            format!("{} d ago", secs / 86400)
        }
    }
}

/// Backups of `path`, newest first.
pub fn list(path: &Path) -> Vec<Backup> {
    let Some(d) = dir_for(path) else { return Vec::new() };
    list_in(&d)
}

fn list_in(d: &Path) -> Vec<Backup> {
    let Ok(rd) = std::fs::read_dir(d) else { return Vec::new() };
    let mut out: Vec<Backup> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_str()?;
            let written = stem.parse::<u64>().ok()?;
            let bytes = e.metadata().ok()?.len();
            Some(Backup { file: p, written, bytes })
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.written));
    out
}

/// Copy the file currently at `path` aside before it is overwritten, then
/// prune to `keep` versions. Silent on failure: a backup must never block a save.
pub fn take(path: &Path, keep: usize) {
    if let Some(base) = dir() {
        take_in(&base, path, keep);
    }
}

fn take_in(base: &Path, path: &Path, keep: usize) {
    if keep == 0 || !path.is_file() {
        return;
    }
    let d = dir_for_in(base, path);
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or(io::NATIVE_EXTENSION);
    // Two saves within a second still get distinct, increasing names.
    let newest = list_in(&d).first().map(|b| b.written).unwrap_or(0);
    let ts = now_secs().max(newest + 1);
    let tmp = d.join(format!("{ts}.{ext}.part"));
    if std::fs::copy(path, &tmp).is_ok() {
        let _ = std::fs::rename(&tmp, d.join(format!("{ts}.{ext}")));
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
    // Remember where these came from, for the folder's human readers.
    let _ = std::fs::write(d.join("source.txt"), path.to_string_lossy().as_bytes());
    for old in list_in(&d).into_iter().skip(keep) {
        let _ = std::fs::remove_file(old.file);
    }
}

/// Open a backup as a new, unsaved document (the original stays untouched).
pub fn restore(state: &mut AppState, original: &Path, b: &Backup) -> Option<DocId> {
    let loaded = if b.file.extension().and_then(|s| s.to_str()) == Some(io::psd::EXTENSION) {
        io::psd::load(&b.file)
    } else {
        io::qsk::load(&b.file)
    };
    match loaded {
        Ok(ds) => {
            let stem = original.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let title = format!("{stem} (backup {})", b.age());
            let mut doc = Document::from_state(ds, title, None, "Restored");
            doc.mark_unsaved();
            let id = state.add_document(doc);
            state.toasts.push(Level::Info, "Restored as a new document. Save As to keep it.");
            Some(id)
        }
        Err(e) => {
            state.toasts.push(Level::Error, format!("Couldn't restore backup: {e:#}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_keeps_newest_n() {
        let base = std::env::temp_dir().join(format!("qsketch-backups-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let file = base.join("drawing.qsk");
        std::fs::create_dir_all(&base).unwrap();
        for i in 0..5u8 {
            std::fs::write(&file, vec![i; 10 + i as usize]).unwrap();
            take_in(&base, &file, 3);
        }
        let l = list_in(&dir_for_in(&base, &file));
        assert_eq!(l.len(), 3, "pruned to 3: {l:?}");
        // Newest first: the most recent backup holds the 5th write (14 bytes).
        assert_eq!(l[0].bytes, 14);
        assert_eq!(l[2].bytes, 12);
        assert!(l[0].written >= l[1].written && l[1].written >= l[2].written);
        // Missing file: nothing happens.
        take_in(&base, &base.join("nope.qsk"), 3);
        assert!(list_in(&dir_for_in(&base, &base.join("nope.qsk"))).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}
