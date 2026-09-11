//! Layer groups. The layer stack stays a flat, bottom-to-top `Vec<Layer>`:
//! a group is a `LayerKind::Group` entry whose members are the contiguous
//! run of layers immediately *below* it (so the Layers panel, which lists
//! top-first, shows the folder above its contents). Every member carries
//! `parent = Some(group id)`; nested groups nest their runs the same way.
//!
//! This module holds the structural operations that keep that invariant.

use std::collections::HashMap;
use std::ops::Range;

use crate::document::DocState;
use crate::layer::{Layer, LayerId, LayerKind};

impl DocState {
    /// Ids of the groups containing `idx`, innermost first.
    pub fn ancestors(&self, idx: usize) -> Vec<LayerId> {
        let mut out = Vec::new();
        let mut cur = self.layers.get(idx).and_then(|l| l.props.parent);
        while let Some(p) = cur {
            if out.contains(&p) || out.len() > self.layers.len() {
                break; // corrupt parent chain: stop rather than loop forever
            }
            out.push(p);
            cur = self.layer_by_id(p).and_then(|l| l.props.parent);
        }
        out
    }

    /// Nesting depth (0 = top level).
    pub fn depth(&self, idx: usize) -> usize {
        self.ancestors(idx).len()
    }

    pub fn is_descendant_of(&self, idx: usize, group: LayerId) -> bool {
        self.ancestors(idx).contains(&group)
    }

    /// Members of the group at `idx` (everything nested inside it), as the
    /// index range just below the entry. Empty for raster layers.
    pub fn members(&self, idx: usize) -> Range<usize> {
        let Some(l) = self.layers.get(idx) else { return idx..idx };
        if !l.is_group() {
            return idx..idx;
        }
        let gid = l.props.id;
        let mut start = idx;
        while start > 0 && self.is_descendant_of(start - 1, gid) {
            start -= 1;
        }
        start..idx
    }

    /// The layer plus, for a group, all its members: the range that moves,
    /// duplicates or deletes together.
    pub fn block(&self, idx: usize) -> Range<usize> {
        self.members(idx).start..(idx + 1).min(self.layers.len())
    }

    /// Visible, and every enclosing group visible too.
    pub fn effectively_visible(&self, idx: usize) -> bool {
        self.layers.get(idx).is_some_and(|l| l.props.visible)
            && self.ancestors(idx).iter().all(|g| self.layer_by_id(*g).is_some_and(|l| l.props.visible))
    }

    /// Can pixels be painted on layer `idx` right now (raster, visible,
    /// unlocked, and no enclosing group hidden or locked)?
    pub fn layer_editable(&self, idx: usize) -> bool {
        self.layers.get(idx).is_some_and(|l| l.editable())
            && self
                .ancestors(idx)
                .iter()
                .all(|g| self.layer_by_id(*g).is_some_and(|l| l.props.visible && !l.props.locked))
    }

    /// Expand a set of selected layer ids into the editable raster layers
    /// they denote: a raster layer stands for itself, a group for every
    /// raster layer inside it. Sorted, without duplicates.
    pub fn raster_layers_in(&self, ids: &[LayerId]) -> Vec<usize> {
        let mut out = Vec::new();
        for &id in ids {
            let Some(idx) = self.index_of(id) else { continue };
            let range = if self.layers[idx].is_group() { self.members(idx) } else { idx..idx + 1 };
            for i in range {
                if !self.layers[i].is_group() && self.layer_editable(i) && !out.contains(&i) {
                    out.push(i);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// Direct children of `parent` within `range`, bottom to top.
    pub fn direct_children(&self, range: Range<usize>, parent: Option<LayerId>) -> Vec<usize> {
        range.filter(|&i| self.layers[i].props.parent == parent).collect()
    }

    /// The sibling directly below `idx` (same parent), if any.
    pub fn sibling_below(&self, idx: usize) -> Option<usize> {
        let start = self.block(idx).start;
        if start == 0 {
            return None;
        }
        (self.layers[start - 1].props.parent == self.layers[idx].props.parent).then_some(start - 1)
    }

    /// The sibling directly above `idx` (same parent), if any.
    pub fn sibling_above(&self, idx: usize) -> Option<usize> {
        let parent = self.layers.get(idx)?.props.parent;
        let end = self.block(idx).end;
        let limit = match parent {
            Some(p) => self.index_of(p)?,
            None => self.layers.len(),
        };
        (end..limit).find(|&j| self.layers[j].props.parent == parent)
    }

    /// Index range of the whole run owned by `parent` (the document for
    /// `None`), i.e. where siblings may be placed.
    pub fn parent_span(&self, parent: Option<LayerId>) -> Range<usize> {
        match parent.and_then(|p| self.index_of(p)) {
            Some(pi) => self.members(pi),
            None => 0..self.layers.len(),
        }
    }

    /// Move the block at `idx` so that it starts at position `to` of the
    /// *current* list (before removal), and give it `parent`. `to` must be a
    /// legal slot inside `parent`'s run (or at its ends). Members keep their
    /// own parents; only the moved entry is re-parented.
    pub fn move_block(&mut self, idx: usize, to: usize, parent: Option<LayerId>) {
        let block = self.block(idx);
        if block.is_empty() || to > self.layers.len() {
            return;
        }
        let id = self.layers[idx].props.id;
        if parent == Some(id) {
            return;
        }
        if let Some(p) = parent {
            let Some(pi) = self.index_of(p) else { return };
            // Only groups hold members, and a group can't move into itself.
            if !self.layers[pi].is_group() || self.is_descendant_of(pi, id) {
                return;
            }
        }
        let same_place = to >= block.start && to <= block.end;
        if same_place && parent == self.layers[idx].props.parent {
            return;
        }
        let len = block.len();
        let active_id = self.layers[self.active.min(self.layers.len() - 1)].props.id;
        let moved: Vec<Layer> = self.layers.drain(block.clone()).collect();
        let to = if to >= block.end {
            to - len
        } else if to > block.start {
            block.start
        } else {
            to
        };
        let mut moved = moved;
        moved[len - 1].props.parent = parent;
        for (k, l) in moved.into_iter().enumerate() {
            self.layers.insert(to + k, l);
        }
        self.active = self.index_of(active_id).unwrap_or(0);
    }

    /// Swap the block at `idx` with the sibling above (`up`) or below it.
    /// Returns false when it is already at that end of its group.
    pub fn move_sibling(&mut self, idx: usize, up: bool) -> bool {
        let parent = self.layers[idx].props.parent;
        if up {
            let Some(s) = self.sibling_above(idx) else { return false };
            let to = self.block(s).end;
            self.move_block(idx, to, parent);
        } else {
            let Some(s) = self.sibling_below(idx) else { return false };
            let to = self.block(s).start;
            self.move_block(idx, to, parent);
        }
        true
    }

    /// Move the block at `idx` to the top or bottom of its group.
    pub fn move_to_end(&mut self, idx: usize, top: bool) -> bool {
        let parent = self.layers[idx].props.parent;
        let span = self.parent_span(parent);
        let block = self.block(idx);
        let to = if top { span.end } else { span.start };
        if (top && block.end == span.end) || (!top && block.start == span.start) {
            return false;
        }
        self.move_block(idx, to, parent);
        true
    }

    /// Put the layers `ids` (and everything inside any selected group) into a
    /// new group placed where the topmost of them was. Returns the group id.
    pub fn group_layers(&mut self, ids: &[LayerId]) -> Option<LayerId> {
        let mut items: Vec<usize> = ids.iter().filter_map(|id| self.index_of(*id)).collect();
        items.sort_unstable();
        items.dedup();
        // Keep only outermost items: a member of a selected group moves with it.
        let items: Vec<usize> = items
            .iter()
            .copied()
            .filter(|&i| {
                !items
                    .iter()
                    .any(|&g| g != i && self.layers[g].is_group() && self.is_descendant_of(i, self.layers[g].props.id))
            })
            .collect();
        let top = *items.last()?;
        let parent = self.layers[top].props.parent;
        let name = self.unique_layer_name("Group");
        let gid = self.new_id();
        let mut group = Layer::new(gid, name, self.width, self.height);
        group.props.kind = LayerKind::Group;
        group.props.blend = crate::blend::BlendMode::PassThrough;
        group.props.parent = parent;

        let mut insert_at = self.block(top).end;
        let mut moved: Vec<Layer> = Vec::new();
        for &i in items.iter().rev() {
            let block = self.block(i);
            insert_at -= block.len();
            let mut chunk: Vec<Layer> = self.layers.drain(block).collect();
            if let Some(last) = chunk.last_mut() {
                last.props.parent = Some(gid);
            }
            chunk.append(&mut moved);
            moved = chunk;
        }
        let n = moved.len();
        for (k, l) in moved.into_iter().enumerate() {
            self.layers.insert(insert_at + k, l);
        }
        self.layers.insert(insert_at + n, group);
        self.active = insert_at + n;
        Some(gid)
    }

    /// Dissolve the group at `idx`, leaving its members in place. Returns
    /// false when `idx` is not a group.
    pub fn ungroup(&mut self, idx: usize) -> bool {
        let Some(l) = self.layers.get(idx) else { return false };
        if !l.is_group() {
            return false;
        }
        let gid = l.props.id;
        let parent = l.props.parent;
        for i in self.members(idx) {
            if self.layers[i].props.parent == Some(gid) {
                self.layers[i].props.parent = parent;
            }
        }
        self.layers.remove(idx);
        self.active = if idx == 0 { 0 } else { idx - 1 }.min(self.layers.len().saturating_sub(1));
        true
    }

    /// Replace the group at `idx` and its members by one raster layer holding
    /// their composite; the new layer keeps the group's name, opacity, blend
    /// mode and position.
    pub fn rasterize_group(&mut self, idx: usize) -> bool {
        let Some(l) = self.layers.get(idx) else { return false };
        if !l.is_group() {
            return false;
        }
        let members = self.members(idx);
        let raster = crate::composite::flatten_range(self, members.clone(), Some(l.props.id));
        let mut props = l.props.clone();
        props.kind = LayerKind::Raster;
        props.expanded = true;
        if props.blend == crate::blend::BlendMode::PassThrough {
            props.blend = crate::blend::BlendMode::Normal;
        }
        let new = Layer { props, raster };
        let block = self.block(idx);
        let active_in = (block.start..block.end).contains(&self.active);
        self.layers.drain(block.clone());
        self.layers.insert(block.start, new);
        if active_in {
            self.active = block.start;
        } else if self.active >= block.end {
            self.active -= block.len() - 1;
        }
        true
    }

    /// Clone a block with fresh ids (parents inside the block remapped).
    pub(crate) fn clone_block(&mut self, block: Range<usize>) -> Vec<Layer> {
        let mut map: HashMap<LayerId, LayerId> = HashMap::new();
        let mut out: Vec<Layer> = self.layers[block].to_vec();
        for l in &mut out {
            let id = self.new_id();
            map.insert(l.props.id, id);
            l.props.id = id;
        }
        for l in &mut out {
            if let Some(p) = l.props.parent {
                if let Some(np) = map.get(&p) {
                    l.props.parent = Some(*np);
                }
            }
        }
        out
    }

    /// Drop parent links that point nowhere (after loading a file edited by
    /// hand or by an older version) so the group invariants hold.
    pub fn repair_groups(&mut self) {
        let groups: Vec<LayerId> = self.layers.iter().filter(|l| l.is_group()).map(|l| l.props.id).collect();
        for l in &mut self.layers {
            if l.props.parent.is_some_and(|p| !groups.contains(&p)) {
                l.props.parent = None;
            }
        }
        // Members must sit directly below their group entry; anything that
        // strayed is hoisted to the top level.
        for i in 0..self.layers.len() {
            let Some(p) = self.layers[i].props.parent else { continue };
            let inside = self.index_of(p).is_some_and(|pi| {
                let mut j = pi;
                while j > 0 && self.layers[j - 1].props.parent.is_some() {
                    j -= 1;
                    if j == i {
                        return true;
                    }
                }
                false
            });
            if !inside {
                self.layers[i].props.parent = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    fn names(s: &DocState) -> String {
        s.layers
            .iter()
            .map(|l| format!("{}{}", l.props.name, l.props.parent.map_or(String::new(), |p| format!("@{p}"))))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn doc() -> (DocState, [LayerId; 3]) {
        let mut s = DocState::new(8, 8, Some(Rgba8::WHITE));
        let a = s.add_layer("A", None);
        let b = s.add_layer("B", None);
        let c = s.add_layer("C", None);
        (s, [a, b, c])
    }

    #[test]
    fn group_and_ungroup() {
        let (mut s, [a, b, c]) = doc();
        let g = s.group_layers(&[a, b]).unwrap();
        assert_eq!(names(&s), format!("Background,A@{g},B@{g},Group 5,C"));
        let gi = s.index_of(g).unwrap();
        assert_eq!(s.members(gi), 1..3);
        assert_eq!(s.block(gi), 1..4);
        assert_eq!(s.active, gi);
        assert_eq!(s.depth(1), 1);
        assert_eq!(s.sibling_above(gi), Some(4));
        assert_eq!(s.sibling_below(gi), Some(0));
        assert_eq!(s.sibling_above(1), Some(2));
        assert_eq!(s.sibling_above(2), None);
        // Nested: group the group with C.
        let g2 = s.group_layers(&[g, c]).unwrap();
        let g2i = s.index_of(g2).unwrap();
        assert_eq!(s.members(g2i), 1..5);
        assert_eq!(s.depth(1), 2);
        assert!(s.ungroup(g2i));
        assert_eq!(s.layers[3].props.parent, None);
        assert_eq!(s.layers[4].props.parent, None);
        assert_eq!(s.layers[1].props.parent, Some(g));
        // Hidden group hides members; editable respects it.
        s.layers[3].props.visible = false;
        assert!(!s.effectively_visible(1));
        assert!(!s.layer_editable(1));
        assert!(s.layer_editable(4));
        assert_eq!(s.raster_layers_in(&[g, c]), vec![4]);
        s.layers[3].props.visible = true;
        assert_eq!(s.raster_layers_in(&[g, c]), vec![1, 2, 4]);
    }

    #[test]
    fn move_delete_duplicate_blocks() {
        let (mut s, [a, b, c]) = doc();
        let g = s.group_layers(&[a, b]).unwrap();
        let gi = s.index_of(g).unwrap();
        // Group up above C.
        assert!(s.move_sibling(gi, true));
        assert_eq!(names(&s), format!("Background,C,A@{g},B@{g},Group 5"));
        assert_eq!(s.active, 4);
        assert!(!s.move_sibling(4, true));
        assert!(s.move_to_end(4, false));
        assert_eq!(s.layers[2].props.name, "Group 5");
        // C into the group (above B).
        let ci = s.index_of(c).unwrap();
        s.move_block(ci, 2, Some(g));
        assert_eq!(names(&s), format!("A@{g},B@{g},C@{g},Group 5,Background"));
        let gi = s.index_of(g).unwrap();
        s.duplicate_layer(gi);
        assert_eq!(s.layers.len(), 9);
        assert_eq!(s.layers[7].props.name, "Group 5 copy");
        assert_eq!(s.members(7), 4..7);
        assert_ne!(s.layers[4].props.parent, Some(g));
        assert!(s.remove_layer(7).is_some());
        assert_eq!(s.layers.len(), 5);
        assert!(s.rasterize_group(3));
        assert_eq!(names(&s), "Group 5,Background");
        assert!(!s.layers[0].is_group());
    }

    #[test]
    fn merge_and_add_inside_groups() {
        let (mut s, [a, b, _c]) = doc();
        s.layers[1].raster.set_pixel(0, 0, Rgba8::BLACK);
        let g = s.group_layers(&[a, b]).unwrap();
        let gi = s.index_of(g).unwrap();
        s.active = gi;
        let n = s.add_layer("Inside", None);
        let ni = s.index_of(n).unwrap();
        assert_eq!(s.layers[ni].props.parent, Some(g));
        assert_eq!(s.members(s.index_of(g).unwrap()), 1..4);
        // Merge C (top) down onto the group: group is rasterized first.
        let ci = s.layers.len() - 1;
        assert!(s.merge_down(ci));
        assert_eq!(s.layers.len(), 2);
        assert_eq!(s.layers[1].raster.get_pixel(0, 0), Rgba8::BLACK);
        assert!(!s.layers[1].is_group());
    }
}
