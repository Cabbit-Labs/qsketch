//! Undo history as a list of cheap document snapshots (see `raster.rs` for why
//! snapshots are cheap). Mirrors the Photoshop History panel: a linear list of
//! states with a cursor; committing after an undo discards the redo branch.

use crate::document::DocState;

pub struct HistoryEntry {
    pub label: String,
    pub state: DocState,
}

pub struct History {
    entries: Vec<HistoryEntry>,
    cursor: usize,
    limit: usize,
}

impl History {
    pub fn new(initial: DocState, label: impl Into<String>) -> Self {
        Self { entries: vec![HistoryEntry { label: label.into(), state: initial }], cursor: 0, limit: 200 }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Maximum number of states kept (minimum 2).
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(2);
        self.enforce_limit();
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn current(&self) -> &DocState {
        &self.entries[self.cursor].state
    }
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }
    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo().then(|| self.entries[self.cursor].label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.can_redo().then(|| self.entries[self.cursor + 1].label.as_str())
    }

    /// Record a new state after the cursor, discarding any redo states.
    pub fn push(&mut self, label: impl Into<String>, state: DocState) {
        self.entries.truncate(self.cursor + 1);
        self.entries.push(HistoryEntry { label: label.into(), state });
        self.cursor = self.entries.len() - 1;
        self.enforce_limit();
    }

    fn enforce_limit(&mut self) {
        while self.entries.len() > self.limit {
            self.entries.remove(0);
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn undo(&mut self) -> Option<&DocState> {
        if self.can_undo() {
            self.cursor -= 1;
            Some(self.current())
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<&DocState> {
        if self.can_redo() {
            self.cursor += 1;
            Some(self.current())
        } else {
            None
        }
    }

    pub fn jump_to(&mut self, index: usize) -> Option<&DocState> {
        if index < self.entries.len() {
            self.cursor = index;
            Some(self.current())
        } else {
            None
        }
    }

    /// Drop every state except the current one (e.g. to free memory).
    pub fn clear_to_current(&mut self) {
        let cur = self.entries.swap_remove(self.cursor);
        self.entries.clear();
        self.entries.push(cur);
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_history() {
        let s = DocState::new(8, 8, None);
        let mut h = History::new(s.clone(), "New");
        h.push("A", s.clone());
        h.push("B", s.clone());
        assert_eq!(h.cursor(), 2);
        assert!(h.undo().is_some());
        assert_eq!(h.undo_label(), Some("A"));
        assert_eq!(h.redo_label(), Some("B"));
        h.push("C", s.clone());
        assert_eq!(h.len(), 3);
        assert!(!h.can_redo());
        h.set_limit(2);
        assert_eq!(h.len(), 2);
        assert_eq!(h.cursor(), 1);
    }
}
