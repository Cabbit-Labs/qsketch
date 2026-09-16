//! Floating pixels under transform: a paste, or the layer / selection lifted
//! by Free Transform (Ctrl+T). The pixels hover over the active layer and can
//! be moved, scaled, rotated (Freeform / Resize / Rotate), have their corners
//! dragged (Deform) or
//! be bent on a control lattice (Warp) before being committed (Enter /
//! double-click / Apply) or discarded (Esc / Cancel). While floating, the
//! working state already contains the preview so the regular composite/render
//! path shows it; committing just records a history step.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Stroke, Vec2};
use qsketch_core::ops::{drop_floating, Floating, Orient};
use qsketch_core::raster::ResizeFilter;
use qsketch_core::warp::{self, Mesh, Quad};
use qsketch_core::{IRect, Mask, Pt, Raster};

use super::CanvasEvent;
use crate::state::{AppState, DocId};
use crate::ui::toasts::Level;

/// Widest or tallest a transform box may get, in document pixels. Past this
/// the numbers stop being useful and float precision starts to show.
pub const MAX_BOX_SIZE: f32 = 100_000.0;

/// Handle positions around the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Unit position of the handle on the box (0 = min edge, 1 = max edge).
    fn uv(self) -> (f32, f32) {
        match self {
            Handle::TopLeft => (0.0, 0.0),
            Handle::Top => (0.5, 0.0),
            Handle::TopRight => (1.0, 0.0),
            Handle::Right => (1.0, 0.5),
            Handle::BottomRight => (1.0, 1.0),
            Handle::Bottom => (0.5, 1.0),
            Handle::BottomLeft => (0.0, 1.0),
            Handle::Left => (0.0, 0.5),
        }
    }

    fn is_corner(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft)
    }

    fn moves_x(self) -> bool {
        !matches!(self, Handle::Top | Handle::Bottom)
    }

    fn moves_y(self) -> bool {
        !matches!(self, Handle::Left | Handle::Right)
    }

    /// Quad corner index for a corner handle (tl, tr, br, bl).
    fn corner_index(self) -> Option<usize> {
        match self {
            Handle::TopLeft => Some(0),
            Handle::TopRight => Some(1),
            Handle::BottomRight => Some(2),
            Handle::BottomLeft => Some(3),
            _ => None,
        }
    }

    /// The two quad corners an edge handle drags.
    fn edge_corners(self) -> Option<(usize, usize)> {
        match self {
            Handle::Top => Some((0, 1)),
            Handle::Right => Some((1, 2)),
            Handle::Bottom => Some((2, 3)),
            Handle::Left => Some((3, 0)),
            _ => None,
        }
    }
}

/// How the box is being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// Move, scale and rotate a rectangle.
    #[default]
    Freeform,
    /// Move and scale only.
    Resize,
    /// Rotate about the center only.
    Rotate,
    /// Drag the four corners independently (projective).
    Deform,
    /// Bend the pixels on a control lattice.
    Warp,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Freeform, Mode::Resize, Mode::Rotate, Mode::Deform, Mode::Warp];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Freeform => "Freeform",
            Mode::Resize => "Resize",
            Mode::Rotate => "Rotate",
            Mode::Deform => "Deform",
            Mode::Warp => "Warp",
        }
    }

    /// Modes that edit the rectangle parameters (center / size / angle).
    pub fn is_box(self) -> bool {
        matches!(self, Mode::Freeform | Mode::Resize | Mode::Rotate)
    }
}

/// Something the pointer grabbed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grab {
    Box(Handle),
    /// Warp lattice point index.
    Node(usize),
}

#[derive(Clone, Debug)]
pub enum FloatDrag {
    Move {
        start: Pt,
        start_center: Pt,
        start_quad: Quad,
        start_mesh: Vec<Pt>,
    },
    /// Free-mode scale in the box's local (unrotated) frame.
    Scale {
        handle: Handle,
        start: Pt,
        start_center: Pt,
        start_size: (f32, f32),
    },
    Rotate {
        start_angle: f32,
        start_pointer_angle: f32,
    },
    Corner {
        corner: usize,
        start: Pt,
        start_quad: Quad,
    },
    Edge {
        corners: (usize, usize),
        start: Pt,
        start_quad: Quad,
    },
    Node {
        index: usize,
        start: Pt,
        start_pt: Pt,
    },
}

/// Why the pixels are floating; decides the history label and what happens
/// to the selection on commit.
#[derive(Clone)]
pub enum Origin {
    Paste,
    /// Lifted from the layer; `mask` is the lifted selection (source-sized).
    Transform {
        mask: Option<Mask>,
        sel_before: Option<Arc<Mask>>,
        /// History step name ("Free Transform", "Move", "Duplicate").
        label: &'static str,
    },
}

pub struct FloatingPaste {
    pub doc: DocId,
    pub layer: usize,
    /// The pixels at their original size.
    pub source: Raster,
    /// The layer raster underneath (the preview is composited onto it).
    pub base: Raster,
    pub origin: Origin,
    pub mode: Mode,
    // Free-mode parameters.
    pub center: Pt,
    pub size: (f32, f32),
    /// Radians, clockwise on screen.
    pub angle: f32,
    /// Deform-mode corners (tl, tr, br, bl).
    pub quad: Quad,
    /// Warp-mode lattice.
    pub mesh: Mesh,
    pub warp_div: u32,
    /// Where the pixels started (identity placement).
    home: Pt,
    pub drag: Option<FloatDrag>,
    pub last_dirty: IRect,
    pub keep_aspect: bool,
    pub filter: ResizeFilter,
    /// The preview must be re-rendered into the working layer.
    dirty: bool,
}

impl FloatingPaste {
    fn new(doc: DocId, layer: usize, source: Raster, base: Raster, place: IRect, origin: Origin) -> Self {
        let (w, h) = (place.w.max(1) as f32, place.h.max(1) as f32);
        let center = Pt::new(place.x as f32 + w / 2.0, place.y as f32 + h / 2.0);
        let quad = free_quad(center, (w, h), 0.0);
        Self {
            doc,
            layer,
            source,
            base,
            origin,
            mode: Mode::Freeform,
            center,
            size: (w, h),
            angle: 0.0,
            quad,
            mesh: Mesh::from_quad(quad, 3, 3),
            warp_div: 3,
            home: center,
            drag: None,
            last_dirty: IRect::EMPTY,
            keep_aspect: false,
            filter: ResizeFilter::Bilinear,
            dirty: true,
        }
    }

    /// A plain selection move / duplicate needs no mode panel: it would cover
    /// the canvas on every drag. Ctrl+T and pastes still show it.
    pub fn shows_panel(&self) -> bool {
        !self.is_selection_drag()
    }

    /// Pixels lifted by dragging the selection box itself (as opposed to a
    /// paste or an explicit Ctrl+T), so there is a selection to stamp the next
    /// copy from.
    fn is_selection_drag(&self) -> bool {
        matches!(self.origin, Origin::Transform { mask: Some(_), label: "Move" | "Duplicate", .. })
    }

    pub fn is_paste(&self) -> bool {
        matches!(self.origin, Origin::Paste)
    }

    /// The current outer corners (tl, tr, br, bl) in document space.
    pub fn corners(&self) -> Quad {
        match self.mode {
            Mode::Freeform | Mode::Resize | Mode::Rotate => free_quad(self.center, self.size, self.angle),
            Mode::Deform => self.quad,
            Mode::Warp => self.mesh.corners(),
        }
    }

    /// The mesh the pixels are rendered through.
    fn render_mesh(&self) -> Mesh {
        match self.mode {
            Mode::Warp => self.mesh.clone(),
            _ => Mesh::from_quad(self.corners(), 1, 1),
        }
    }

    /// Scale relative to the source, in percent (Free mode numbers).
    pub fn scale_pct(&self) -> (f32, f32) {
        let (sw, sh) = (self.source.width().max(1) as f32, self.source.height().max(1) as f32);
        (self.size.0 / sw * 100.0, self.size.1 / sh * 100.0)
    }

    /// True when the pixels are unscaled, unrotated and unbent (they may still
    /// have been moved).
    pub fn is_identity(&self) -> bool {
        let (sw, sh) = (self.source.width() as f32, self.source.height() as f32);
        self.mode.is_box() && self.angle == 0.0 && (self.size.0 - sw).abs() < 0.5 && (self.size.1 - sh).abs() < 0.5
    }

    /// True when nothing at all changed since the pixels were lifted.
    pub fn is_untouched(&self) -> bool {
        self.is_identity() && (self.center.x - self.home.x).abs() < 0.5 && (self.center.y - self.home.y).abs() < 0.5
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        if mode.is_box() && self.mode.is_box() {
            self.mode = mode;
            self.drag = None;
            return;
        }
        let quad = self.corners();
        match mode {
            Mode::Freeform | Mode::Resize | Mode::Rotate => {
                // Straighten: keep the center, derive size/angle from the edges.
                let c = Pt::new(
                    (quad[0].x + quad[1].x + quad[2].x + quad[3].x) / 4.0,
                    (quad[0].y + quad[1].y + quad[2].y + quad[3].y) / 4.0,
                );
                let w = (quad[0].dist(quad[1]) + quad[3].dist(quad[2])) / 2.0;
                let h = (quad[0].dist(quad[3]) + quad[1].dist(quad[2])) / 2.0;
                self.center = c;
                self.size = (w.max(1.0), h.max(1.0));
                self.angle = (quad[1].y - quad[0].y).atan2(quad[1].x - quad[0].x);
            }
            Mode::Deform => self.quad = quad,
            Mode::Warp => self.mesh = Mesh::from_quad(quad, self.warp_div, self.warp_div),
        }
        self.mode = mode;
        self.drag = None;
        self.dirty = true;
    }

    /// Change the warp lattice density (resets any bending).
    pub fn set_warp_div(&mut self, div: u32) {
        self.warp_div = div.clamp(1, 8);
        if self.mode == Mode::Warp {
            self.mesh = Mesh::from_quad(self.corners(), self.warp_div, self.warp_div);
            self.dirty = true;
        }
    }

    /// Re-render after the resampling filter changed.
    pub fn reset_filter(&mut self) {
        self.dirty = true;
    }

    pub fn set_size(&mut self, w: f32, h: f32) {
        self.size = (clamp_size(w), clamp_size(h));
        self.dirty = true;
    }

    pub fn set_angle(&mut self, deg: f32) {
        self.angle = deg.to_radians();
        self.dirty = true;
    }

    pub fn set_center(&mut self, c: Pt) {
        let d = Pt::new(c.x - self.center.x, c.y - self.center.y);
        self.translate(d.x, d.y);
    }

    fn translate(&mut self, dx: f32, dy: f32) {
        self.center = Pt::new(self.center.x + dx, self.center.y + dy);
        for p in &mut self.quad {
            p.x += dx;
            p.y += dy;
        }
        self.mesh.translate(dx, dy);
        self.dirty = true;
    }

    pub fn nudge(&mut self, dx: i32, dy: i32) {
        self.translate(dx as f32, dy as f32);
    }

    /// Back to the untransformed source at the current center.
    pub fn reset(&mut self) {
        let c = self.center;
        self.mode = Mode::Freeform;
        self.size = (self.source.width().max(1) as f32, self.source.height().max(1) as f32);
        self.angle = 0.0;
        self.center = Pt::new(
            (c.x - self.size.0 / 2.0).round() + self.size.0 / 2.0,
            (c.y - self.size.1 / 2.0).round() + self.size.1 / 2.0,
        );
        self.quad = self.corners();
        self.mesh = Mesh::from_quad(self.quad, self.warp_div, self.warp_div);
        self.drag = None;
        self.dirty = true;
    }

    /// Rotate / flip the floating pixels about their center.
    pub fn orient(&mut self, o: Orient) {
        self.source = o.apply_raster(&self.source);
        if let Origin::Transform { mask: Some(m), .. } = &mut self.origin {
            *m = o.apply_mask(m);
        }
        if let Orient::Rotate(t) = o {
            if self.mode.is_box() && t % 2 == 1 {
                self.size = (self.size.1, self.size.0);
            }
        }
        self.dirty = true;
    }

    /// Screen-space grab points for hit-testing and drawing. Points that fall
    /// outside the canvas view are parked on its edge (see `clamp_into_view`).
    pub fn grabs(&self, view: &crate::canvas::view::CanvasView) -> Vec<(Grab, Pos2)> {
        let vp = view.viewport;
        self.raw_grabs(view).into_iter().map(|(g, p)| (g, clamp_into_view(p, vp))).collect()
    }

    fn raw_grabs(&self, view: &crate::canvas::view::CanvasView) -> Vec<(Grab, Pos2)> {
        match self.mode {
            Mode::Freeform | Mode::Resize | Mode::Rotate | Mode::Deform => {
                let q = self.corners();
                Handle::ALL
                    .iter()
                    .map(|hd| {
                        let (u, v) = hd.uv();
                        let top = q[0].lerp(q[1], u);
                        let bot = q[3].lerp(q[2], u);
                        (Grab::Box(*hd), view.doc_to_screen(top.lerp(bot, v)))
                    })
                    .collect()
            }
            Mode::Warp => {
                self.mesh.pts.iter().enumerate().map(|(i, p)| (Grab::Node(i), view.doc_to_screen(*p))).collect()
            }
        }
    }

    fn grab_at(&self, view: &crate::canvas::view::CanvasView, pos: Pos2) -> Option<Grab> {
        nearest_grab(self.grabs(view), pos, |g| matches!(g, Grab::Box(h) if h.is_corner()))
    }

    fn contains(&self, p: Pt) -> bool {
        point_in_quad(self.corners(), p)
    }

    /// Render the preview into the working layer if anything changed.
    fn render_into(&mut self, entry: &mut crate::state::DocEntry, filter: ResizeFilter) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let doc_rect = entry.doc.state().rect();
        let mesh = self.render_mesh();
        let (raster, r) = warp::warp_raster(&self.source, &mesh, filter, doc_rect);
        let s = entry.doc.state_mut();
        if self.layer < s.layers.len() {
            s.layers[self.layer].raster = if r.is_empty() {
                self.base.clone()
            } else {
                drop_floating(&self.base, &Floating { raster, origin: (r.x, r.y), mask: None }, 0, 0)
            };
        }
        let dirty = r.union(&self.last_dirty).intersect(&doc_rect);
        entry.doc.mark_dirty_rect(dirty);
        self.last_dirty = r;
    }
}

/// A box edge length that stays inside sane numeric territory.
fn clamp_size(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(1.0, MAX_BOX_SIZE)
    } else {
        1.0
    }
}

/// Corners of a rotated rectangle.
fn free_quad(center: Pt, size: (f32, f32), angle: f32) -> Quad {
    let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
    let (s, c) = angle.sin_cos();
    let rot = |x: f32, y: f32| Pt::new(center.x + x * c - y * s, center.y + x * s + y * c);
    [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)]
}

fn point_in_quad(q: Quad, p: Pt) -> bool {
    let mut inside = false;
    let mut j = 3;
    for i in 0..4 {
        let (a, b) = (q[i], q[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Re-render the floating preview into the working state (call once per frame
/// or after any change). Uses nearest-neighbor while a drag is active.
pub fn refresh(state: &mut AppState) {
    let Some(mut fp) = state.floating.take() else { return };
    let filter = match fp.drag {
        Some(FloatDrag::Move { .. }) | None => fp.filter,
        _ => ResizeFilter::Nearest,
    };
    if let Some(entry) = state.doc_mut(fp.doc) {
        fp.render_into(entry, filter);
    }
    state.floating = Some(fp);
}

/// Begin floating `source` over the active layer of `doc_id` at `(x, y)` with
/// the given display size. Any existing floating paste is committed first.
pub fn begin(state: &mut AppState, doc_id: DocId, source: Raster, x: i32, y: i32, w: u32, h: u32) -> bool {
    commit(state);
    state.cancel_session();
    let Some(entry) = state.doc_mut(doc_id) else { return false };
    let s = entry.doc.state_mut();
    let li = s.active;
    if !s.layer_editable(li) {
        state.toasts.push(Level::Info, "The active layer is locked or hidden.");
        return false;
    }
    let base = s.layers[li].raster.clone();
    s.selection = None;
    entry.sel_outline = None;
    let place = IRect::new(x, y, w as i32, h as i32);
    state.floating = Some(FloatingPaste::new(doc_id, li, source, base, place, Origin::Paste));
    refresh(state);
    true
}

/// Free Transform: lift the selected pixels (or the whole layer when nothing
/// is selected) and start transforming them. A paste already floating just
/// keeps floating.
pub fn begin_transform(state: &mut AppState, doc_id: DocId) -> bool {
    begin_transform_with(state, doc_id, false, "Free Transform")
}

/// Lift the selection into a transform box. `duplicate` leaves the original
/// pixels behind (Aseprite's Ctrl+drag), so the box carries a copy.
pub fn begin_transform_with(state: &mut AppState, doc_id: DocId, duplicate: bool, label: &'static str) -> bool {
    if state.floating.as_ref().is_some_and(|f| f.doc == doc_id) {
        return true;
    }
    commit(state);
    crate::tools::text::commit(state);
    state.cancel_session();
    let Some(entry) = state.doc_mut(doc_id) else { return false };
    let s = entry.doc.state_mut();
    let li = s.active;
    if !s.layer_editable(li) {
        state.toasts.push(Level::Info, "The active layer is locked or hidden.");
        return false;
    }
    let sel_before = s.selection.clone();
    let intact = duplicate.then(|| s.layers[li].raster.clone());
    let Some(lifted) = qsketch_core::ops::lift(s, li) else {
        state.toasts.push(Level::Info, "Nothing to transform: the layer is empty.");
        return false;
    };
    // A duplicate keeps the original pixels: put the layer back as it was and
    // float the copy above it.
    let base = match intact {
        Some(r) => {
            s.layers[li].raster = r.clone();
            r
        }
        None => s.layers[li].raster.clone(),
    };
    let place =
        IRect::new(lifted.origin.0, lifted.origin.1, lifted.raster.width() as i32, lifted.raster.height() as i32);
    // Hide the marching ants while transforming; the selection follows on commit.
    s.selection = None;
    entry.sel_outline = None;
    let origin = Origin::Transform { mask: lifted.mask, sel_before, label };
    state.floating = Some(FloatingPaste::new(doc_id, li, lifted.raster, base, place, origin));
    refresh(state);
    true
}

/// Screen-space slack around the selection box that still counts as grabbing
/// it: inside this band but outside the box starts a rotation, exactly as
/// pressing outside a Freeform box does.
const SELECTION_BAND: f32 = 20.0;

/// The document-space corners of the box drawn around the current selection
/// (its bounds), when there is one.
pub fn selection_quad(state: &AppState, doc_id: DocId) -> Option<Quad> {
    let entry = state.doc(doc_id)?;
    let sel = entry.doc.state().selection.as_ref()?;
    let b = sel.bounds();
    if b.is_empty() {
        return None;
    }
    let (l, t) = (b.x as f32, b.y as f32);
    let (r, bo) = (b.right() as f32, b.bottom() as f32);
    Some([Pt::new(l, t), Pt::new(r, t), Pt::new(r, bo), Pt::new(l, bo)])
}

/// The eight grab points of the idle selection box, in screen space.
fn selection_grabs(state: &AppState, doc_id: DocId) -> Option<Vec<(Handle, Pos2)>> {
    let q = selection_quad(state, doc_id)?;
    let view = &state.doc(doc_id)?.view;
    Some(
        Handle::ALL
            .iter()
            .map(|hd| {
                let (u, v) = hd.uv();
                let top = q[0].lerp(q[1], u);
                let bot = q[3].lerp(q[2], u);
                (*hd, clamp_into_view(view.doc_to_screen(top.lerp(bot, v)), view.viewport))
            })
            .collect(),
    )
}

/// The grab point under `pos`. Nearest wins, except that a corner beats an
/// edge handle it practically sits on: on a box only a few pixels thin the
/// three handles along a short side pile up, and grabbing "the corner" of a
/// hairline selection must scale both ways, not just along it.
fn nearest_grab<G: Copy>(grabs: Vec<(G, Pos2)>, pos: Pos2, is_corner: impl Fn(G) -> bool) -> Option<G> {
    let hit: Vec<(G, f32)> = grabs
        .into_iter()
        .filter(|(_, p)| Rect::from_center_size(*p, Vec2::splat(14.0)).contains(pos))
        .map(|(g, p)| (g, p.distance(pos)))
        .collect();
    let best = hit.iter().map(|(_, d)| *d).fold(f32::INFINITY, f32::min);
    if !best.is_finite() {
        return None;
    }
    // Anything within a few pixels of the nearest counts as a tie.
    let tied = hit.iter().filter(|(_, d)| *d <= best + 4.0);
    tied.clone().find(|(g, _)| is_corner(*g)).or_else(|| tied.min_by(|a, b| a.1.total_cmp(&b.1))).map(|(g, _)| *g)
}

/// Keep a grab point reachable: a transform box scaled past the window would
/// otherwise put every handle off-screen, leaving nothing to drag but the box
/// itself. Every drag is relative to where the pointer went down, so a parked
/// handle scales exactly like the real one.
fn clamp_into_view(p: Pos2, vp: Rect) -> Pos2 {
    let inner = vp.shrink(9.0);
    if !inner.is_positive() || !p.x.is_finite() || !p.y.is_finite() {
        return p;
    }
    egui::pos2(p.x.clamp(inner.left(), inner.right()), p.y.clamp(inner.top(), inner.bottom()))
}

fn selection_handle_at(state: &AppState, doc_id: DocId, pos: Pos2) -> Option<Handle> {
    nearest_grab(selection_grabs(state, doc_id)?, pos, Handle::is_corner)
}

/// True when `pos` grabs the idle selection box: a handle, the inside, or the
/// rotation band just outside it.
pub fn selection_hit(state: &AppState, doc_id: DocId, pos: Pos2) -> bool {
    if state.floating.is_some() {
        return false;
    }
    let Some(q) = selection_quad(state, doc_id) else { return false };
    let Some(entry) = state.doc(doc_id) else { return false };
    let view = &entry.view;
    if selection_handle_at(state, doc_id, pos).is_some() {
        return true;
    }
    if point_in_quad(q, view.screen_to_doc(pos)) {
        return true;
    }
    // Outside: only the band hugging the box counts, so a press well clear of
    // the selection still starts a new marquee.
    let mut bb = Rect::NOTHING;
    for p in q {
        bb = bb.union(Rect::from_min_size(view.doc_to_screen(p), Vec2::ZERO));
    }
    bb.expand(SELECTION_BAND).contains(pos)
}

/// True when `pos` is clear of the floating box and its rotation band, so a
/// selection tool should commit and start a fresh marquee there.
pub fn outside_box(state: &AppState, doc_id: DocId, pos: Pos2) -> bool {
    let Some(fp) = state.floating.as_ref() else { return true };
    if fp.doc != doc_id {
        return true;
    }
    let Some(entry) = state.doc(doc_id) else { return true };
    let view = &entry.view;
    if fp.grab_at(view, pos).is_some() || fp.contains(view.screen_to_doc(pos)) {
        return false;
    }
    let mut bb = Rect::NOTHING;
    for p in fp.corners() {
        bb = bb.union(Rect::from_min_size(view.doc_to_screen(p), Vec2::ZERO));
    }
    !bb.expand(SELECTION_BAND).contains(pos)
}

/// Aseprite-style selection drag: grabbing the box around the selection lifts
/// the selected pixels into a transform box. Ctrl duplicates them instead of
/// moving them. Returns true when the transform started.
pub fn begin_selection_transform(state: &mut AppState, doc_id: DocId, inp: super::CanvasInput) -> bool {
    if !selection_hit(state, doc_id, inp.screen) {
        return false;
    }
    let duplicate = inp.mods.command;
    let label = if duplicate { "Duplicate" } else { "Move" };
    begin_transform_with(state, doc_id, duplicate, label)
}

/// Ctrl+drag *inside* a box that is already floating stamps another copy:
/// the floating pixels are dropped where they are and a fresh duplicate is
/// lifted from the selection they leave behind, so Ctrl+drag keeps duplicating
/// for as long as the area stays selected. Returns true when a new duplicate
/// started. Ctrl on a handle keeps its own meaning (jump into Deform).
pub fn restamp_duplicate(state: &mut AppState, doc_id: DocId, inp: super::CanvasInput) -> bool {
    let Some(fp) = state.floating.as_ref() else { return false };
    // Only a selection drag stamps: a paste has no selection to lift the next
    // copy from, and Ctrl+T keeps Ctrl for its own handle shortcuts.
    if fp.doc != doc_id || !fp.is_selection_drag() {
        return false;
    }
    let Some(entry) = state.doc(doc_id) else { return false };
    let view = &entry.view;
    if fp.grab_at(view, inp.screen).is_some() || !fp.contains(view.screen_to_doc(inp.screen)) {
        return false;
    }
    commit(state);
    // The stamped copy leaves its selection behind; without one the next lift
    // would take the whole layer instead of the area the user is dragging.
    if state.doc(doc_id).is_none_or(|e| e.doc.state().selection.is_none()) {
        return false;
    }
    begin_transform_with(state, doc_id, true, "Duplicate")
}

/// Cursor over the idle selection box.
pub fn selection_cursor(state: &AppState, doc_id: DocId, pos: Option<Pos2>) -> Option<egui::CursorIcon> {
    use egui::CursorIcon as C;
    let pos = pos?;
    if !selection_hit(state, doc_id, pos) {
        return None;
    }
    if let Some(h) = selection_handle_at(state, doc_id, pos) {
        return Some(handle_cursor(h));
    }
    let q = selection_quad(state, doc_id)?;
    let view = &state.doc(doc_id)?.view;
    Some(if point_in_quad(q, view.screen_to_doc(pos)) { C::Move } else { C::Alias })
}

fn handle_cursor(h: Handle) -> egui::CursorIcon {
    use egui::CursorIcon as C;
    match h {
        Handle::TopLeft | Handle::BottomRight => C::ResizeNwSe,
        Handle::TopRight | Handle::BottomLeft => C::ResizeNeSw,
        Handle::Top | Handle::Bottom => C::ResizeVertical,
        Handle::Left | Handle::Right => C::ResizeHorizontal,
    }
}

/// The eight handles drawn around a selection that is not yet floating.
pub fn draw_selection_handles(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(grabs) = selection_grabs(state, doc_id) else { return };
    let accent = Color32::from_rgb(23, 227, 180);
    for (_, p) in grabs {
        let r = Rect::from_center_size(p, Vec2::splat(8.0));
        painter.rect_filled(r, 1.0, Color32::from_black_alpha(160));
        painter.rect_stroke(r.shrink(1.0), 1.0, Stroke::new(1.5, accent), egui::StrokeKind::Inside);
    }
}

/// Commit the floating pixels as a history step. Returns true if there were any.
pub fn commit(state: &mut AppState) -> bool {
    let Some(mut fp) = state.floating.take() else { return false };
    fp.drag = None;
    fp.dirty = true;
    if let Some(entry) = state.doc_mut(fp.doc) {
        fp.render_into(entry, fp.filter);
        match &fp.origin {
            Origin::Paste => entry.doc.commit("Paste"),
            Origin::Transform { mask, sel_before, label } => {
                let s = entry.doc.state_mut();
                let (w, h) = (s.width, s.height);
                if fp.is_untouched() {
                    // Nothing happened: leave the document untouched.
                    s.selection = sel_before.clone();
                    entry.doc.revert_working();
                    entry.sel_outline = None;
                    return true;
                }
                if let Some(m) = mask {
                    let nm = warp::warp_mask(m, &fp.render_mesh(), w, h);
                    s.selection = if nm.is_empty() { None } else { Some(Arc::new(nm)) };
                }
                // A box that started as a plain move but was scaled, rotated
                // or bent deserves a truthful history label.
                let label = if *label == "Move" && !fp.is_identity() { "Transform" } else { *label };
                entry.doc.commit(label);
            }
        }
        entry.sel_outline = None;
    }
    true
}

/// Discard the floating pixels, restoring the layer.
pub fn cancel(state: &mut AppState) -> bool {
    let Some(fp) = state.floating.take() else { return false };
    if let Some(entry) = state.doc_mut(fp.doc) {
        entry.doc.revert_working();
        if let Origin::Transform { sel_before, .. } = fp.origin {
            entry.doc.state_mut().selection = sel_before;
        }
        entry.sel_outline = None;
    }
    true
}

/// Pointer handling while pixels are floating. Returns true if the event was consumed.
pub fn handle(state: &mut AppState, doc_id: DocId, ev: CanvasEvent) -> bool {
    let Some(fp) = state.floating.as_mut() else { return false };
    if fp.doc != doc_id {
        return false;
    }
    let Some(view) = state.docs.iter().find(|d| d.id == doc_id).map(|d| d.view.clone()) else { return false };
    match ev {
        CanvasEvent::Press(inp) => {
            if inp.button != egui::PointerButton::Primary {
                return false;
            }
            let grab = fp.grab_at(&view, inp.screen);
            let start_quad = fp.corners();
            let rotate = FloatDrag::Rotate {
                start_angle: fp.angle,
                start_pointer_angle: (inp.doc.y - fp.center.y).atan2(inp.doc.x - fp.center.x),
            };
            fp.drag = Some(match (fp.mode, grab) {
                (Mode::Freeform, Some(Grab::Box(handle))) if inp.mods.command => {
                    // Ctrl+handle: jump into Deform and drag that corner / edge.
                    fp.set_mode(Mode::Deform);
                    match handle.corner_index() {
                        Some(corner) => FloatDrag::Corner { corner, start: inp.doc, start_quad },
                        None => FloatDrag::Edge { corners: handle.edge_corners().unwrap(), start: inp.doc, start_quad },
                    }
                }
                (Mode::Rotate, _) => rotate,
                (Mode::Freeform | Mode::Resize, Some(Grab::Box(handle))) => {
                    FloatDrag::Scale { handle, start: inp.doc, start_center: fp.center, start_size: fp.size }
                }
                (Mode::Deform, Some(Grab::Box(handle))) => match handle.corner_index() {
                    Some(corner) => FloatDrag::Corner { corner, start: inp.doc, start_quad },
                    None => FloatDrag::Edge { corners: handle.edge_corners().unwrap(), start: inp.doc, start_quad },
                },
                (_, Some(Grab::Node(index))) => FloatDrag::Node { index, start: inp.doc, start_pt: fp.mesh.pts[index] },
                (Mode::Freeform, None) if !fp.contains(inp.doc) => rotate,
                _ => FloatDrag::Move {
                    start: inp.doc,
                    start_center: fp.center,
                    start_quad,
                    start_mesh: fp.mesh.pts.clone(),
                },
            });
            true
        }
        CanvasEvent::Drag(inp) => {
            let Some(drag) = fp.drag.clone() else { return true };
            match drag {
                FloatDrag::Move { start, start_center, start_quad, start_mesh } => {
                    let mut dx = inp.doc.x - start.x;
                    let mut dy = inp.doc.y - start.y;
                    if inp.mods.shift {
                        if dx.abs() > dy.abs() {
                            dy = 0.0;
                        } else {
                            dx = 0.0;
                        }
                    }
                    let (dx, dy) = (dx.round(), dy.round());
                    fp.center = Pt::new(start_center.x + dx, start_center.y + dy);
                    for (i, p) in fp.quad.iter_mut().enumerate() {
                        *p = Pt::new(start_quad[i].x + dx, start_quad[i].y + dy);
                    }
                    for (p, s) in fp.mesh.pts.iter_mut().zip(&start_mesh) {
                        *p = Pt::new(s.x + dx, s.y + dy);
                    }
                    fp.dirty = true;
                }
                FloatDrag::Scale { handle, start, start_center, start_size } => {
                    // Work in the box's local frame.
                    let (s, c) = fp.angle.sin_cos();
                    let to_local = |p: Pt| {
                        let (x, y) = (p.x - start_center.x, p.y - start_center.y);
                        Pt::new(x * c + y * s, -x * s + y * c)
                    };
                    let (sw, sh) = start_size;
                    let (sx, sy) = (-sw / 2.0, -sh / 2.0);
                    let ls = to_local(start);
                    let lp = to_local(inp.doc);
                    let dx = if handle.moves_x() { lp.x - ls.x } else { 0.0 };
                    let dy = if handle.moves_y() { lp.y - ls.y } else { 0.0 };
                    let (u, v) = handle.uv();
                    // Anchor is the opposite edge/corner (Alt: the center).
                    let from_center = inp.mods.alt;
                    let (ax, ay) = if from_center { (0.0, 0.0) } else { (sx + sw * (1.0 - u), sy + sh * (1.0 - v)) };
                    let (mut mx, mut my) = (sx + sw * u + dx, sy + sh * v + dy);
                    let mut nw =
                        if handle.moves_x() { (mx - ax).abs() * if from_center { 2.0 } else { 1.0 } } else { sw };
                    let mut nh =
                        if handle.moves_y() { (my - ay).abs() * if from_center { 2.0 } else { 1.0 } } else { sh };
                    // Corners keep the aspect ratio unless Shift is held; edges never do.
                    let keep = fp.keep_aspect != inp.mods.shift;
                    if handle.is_corner() && keep && sw > 0.0 && sh > 0.0 {
                        // Project the dragged corner onto the anchor→corner
                        // diagonal. Taking the larger of the two axis ratios
                        // instead would let a thin box explode: a hairline
                        // selection dragged a few pixels across its thin side
                        // is a several-hundred-percent change on that axis.
                        let (hx, hy) = (sx + sw * u, sy + sh * v);
                        let (ddx, ddy) = (hx - ax, hy - ay);
                        let k = ((mx - ax) * ddx + (my - ay) * ddy) / (ddx * ddx + ddy * ddy);
                        let mult = if from_center { 2.0 } else { 1.0 };
                        nw = ddx.abs() * k.abs() * mult;
                        nh = ddy.abs() * k.abs() * mult;
                        // Where the corner ends up on that diagonal, so the
                        // placement below flips the box only when the pointer
                        // crosses the anchor.
                        mx = ax + ddx * k;
                        my = ay + ddy * k;
                    }
                    let nw = clamp_size(nw);
                    let nh = clamp_size(nh);
                    let (lcx, lcy) = if from_center {
                        (0.0, 0.0)
                    } else {
                        let nx = if !handle.moves_x() {
                            sx
                        } else if mx < ax {
                            ax - nw
                        } else {
                            ax
                        };
                        let ny = if !handle.moves_y() {
                            sy
                        } else if my < ay {
                            ay - nh
                        } else {
                            ay
                        };
                        (nx + nw / 2.0, ny + nh / 2.0)
                    };
                    // Local center back to document space.
                    fp.center = Pt::new(start_center.x + lcx * c - lcy * s, start_center.y + lcx * s + lcy * c);
                    fp.size = (nw, nh);
                    fp.dirty = true;
                }
                FloatDrag::Rotate { start_angle, start_pointer_angle } => {
                    let now = (inp.doc.y - fp.center.y).atan2(inp.doc.x - fp.center.x);
                    let mut a = start_angle + now - start_pointer_angle;
                    if inp.mods.shift {
                        let step = 15f32.to_radians();
                        a = (a / step).round() * step;
                    }
                    fp.angle = a;
                    fp.dirty = true;
                }
                FloatDrag::Corner { corner, start, start_quad } => {
                    let d = Pt::new(inp.doc.x - start.x, inp.doc.y - start.y);
                    fp.quad = start_quad;
                    fp.quad[corner] = Pt::new(start_quad[corner].x + d.x, start_quad[corner].y + d.y);
                    fp.dirty = true;
                }
                FloatDrag::Edge { corners, start, start_quad } => {
                    let d = Pt::new(inp.doc.x - start.x, inp.doc.y - start.y);
                    fp.quad = start_quad;
                    for i in [corners.0, corners.1] {
                        fp.quad[i] = Pt::new(start_quad[i].x + d.x, start_quad[i].y + d.y);
                    }
                    fp.dirty = true;
                }
                FloatDrag::Node { index, start, start_pt } => {
                    fp.mesh.pts[index] = Pt::new(start_pt.x + inp.doc.x - start.x, start_pt.y + inp.doc.y - start.y);
                    fp.dirty = true;
                }
            }
            true
        }
        CanvasEvent::Release(_) => {
            if fp.drag.take().is_some() {
                // Switch back from the nearest-neighbor drag preview.
                fp.dirty = true;
            }
            true
        }
        CanvasEvent::DoubleClick(_) => {
            commit(state);
            true
        }
        CanvasEvent::Hover(_) => true,
        CanvasEvent::Cancel => {
            cancel(state);
            true
        }
    }
}

/// Cursor for the pointer position while floating.
pub fn cursor(state: &AppState, doc_id: DocId, pos: Option<Pos2>) -> Option<egui::CursorIcon> {
    let fp = state.floating.as_ref()?;
    if fp.doc != doc_id {
        return None;
    }
    let view = &state.doc(doc_id)?.view;
    use egui::CursorIcon as C;
    Some(match &fp.drag {
        Some(FloatDrag::Scale { handle, .. }) => handle_cursor(*handle),
        Some(FloatDrag::Move { .. }) => C::Grabbing,
        Some(FloatDrag::Rotate { .. }) => C::Alias,
        Some(FloatDrag::Corner { .. } | FloatDrag::Edge { .. } | FloatDrag::Node { .. }) => C::Grabbing,
        None => {
            let Some(p) = pos else { return Some(C::Move) };
            match fp.grab_at(view, p) {
                _ if fp.mode == Mode::Rotate => C::Alias,
                Some(Grab::Box(h)) if matches!(fp.mode, Mode::Freeform | Mode::Resize) => handle_cursor(h),
                Some(_) => C::Grab,
                None if fp.mode == Mode::Freeform && !fp.contains(view.screen_to_doc(p)) => C::Alias,
                None => C::Move,
            }
        }
    })
}

/// Outline + handles overlay.
pub fn draw_overlay(state: &AppState, doc_id: DocId, painter: &egui::Painter) {
    let Some(fp) = state.floating.as_ref() else { return };
    if fp.doc != doc_id {
        return;
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let accent = Color32::from_rgb(23, 227, 180);
    let shadow = Stroke::new(3.0, Color32::from_black_alpha(120));
    let to_s = |p: Pt| view.doc_to_screen(p);
    let q = fp.corners();
    let pts: Vec<Pos2> = q.iter().map(|p| to_s(*p)).collect();
    painter.add(egui::Shape::closed_line(pts.clone(), shadow));
    painter.add(egui::Shape::dashed_line(
        &[pts[0], pts[1], pts[2], pts[3], pts[0]],
        Stroke::new(1.0, accent),
        6.0,
        4.0,
    ));
    if fp.mode == Mode::Warp {
        let thin = Stroke::new(1.0, Color32::from_rgba_unmultiplied(23, 227, 180, 150));
        let m = &fp.mesh;
        for j in 0..=m.rows {
            let line: Vec<Pos2> = (0..=m.cols).map(|i| to_s(m.point(i, j))).collect();
            painter.add(egui::Shape::line(line.clone(), shadow));
            painter.add(egui::Shape::line(line, thin));
        }
        for i in 0..=m.cols {
            let line: Vec<Pos2> = (0..=m.rows).map(|j| to_s(m.point(i, j))).collect();
            painter.add(egui::Shape::line(line.clone(), shadow));
            painter.add(egui::Shape::line(line, thin));
        }
    }
    // Center mark (rotation pivot).
    if matches!(fp.mode, Mode::Freeform | Mode::Rotate) {
        let c = to_s(fp.center);
        painter.circle_stroke(c, 4.0, Stroke::new(2.0, Color32::from_black_alpha(140)));
        painter.circle_stroke(c, 4.0, Stroke::new(1.0, accent));
    }
    for (g, p) in fp.grabs(view) {
        let r = Rect::from_center_size(p, Vec2::splat(8.0));
        match g {
            Grab::Box(_) => {
                painter.rect_filled(r, 1.0, Color32::from_black_alpha(160));
                painter.rect_stroke(r.shrink(1.0), 1.0, Stroke::new(1.5, accent), egui::StrokeKind::Inside);
            }
            Grab::Node(_) => {
                painter.circle_filled(p, 4.5, Color32::from_black_alpha(160));
                painter.circle_stroke(p, 3.5, Stroke::new(1.5, accent));
            }
        }
    }
}

/// The small SAI-style panel beside the box: mode radios + OK / Cancel.
pub fn panel_ui(ui: &mut egui::Ui, state: &mut AppState, doc_id: DocId) {
    let Some(fp) = state.floating.as_ref() else { return };
    if fp.doc != doc_id || !fp.shows_panel() {
        return;
    }
    let Some(entry) = state.doc(doc_id) else { return };
    let view = &entry.view;
    let ctx = ui.ctx().clone();
    // Sit just outside the box's top-left corner, clamped to the canvas.
    let q = fp.corners();
    let mut bb = egui::Rect::NOTHING;
    for p in q {
        bb = bb.union(egui::Rect::from_min_size(view.doc_to_screen(p), egui::Vec2::ZERO));
    }
    let size = egui::vec2(150.0, 118.0);
    let vp = view.viewport;
    let mut pos = egui::pos2(bb.left() - size.x - 12.0, bb.top());
    if pos.x < vp.left() + 4.0 {
        pos.x = (bb.right() + 12.0).min(vp.right() - size.x - 4.0).max(vp.left() + 4.0);
    }
    pos.y = pos.y.clamp(vp.top() + 4.0, (vp.bottom() - size.y - 4.0).max(vp.top() + 4.0));
    let mut apply = false;
    let mut discard = false;
    let mut new_mode = None;
    egui::Area::new(egui::Id::new(("float_panel", doc_id))).order(egui::Order::Foreground).fixed_pos(pos).show(
        &ctx,
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(size.x - 16.0);
                let Some(fp) = state.floating.as_ref() else { return };
                let mut mode = fp.mode;
                for m in Mode::ALL {
                    if ui.radio_value(&mut mode, m, m.label()).changed() {
                        new_mode = Some(mode);
                    }
                }
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                    apply = ui.add_sized([w, 20.0], egui::Button::new("OK")).on_hover_text("Enter").clicked();
                    discard = ui.add_sized([w, 20.0], egui::Button::new("Cancel")).on_hover_text("Esc").clicked();
                });
            });
        },
    );
    if let Some(m) = new_mode {
        if let Some(fp) = state.floating.as_mut() {
            fp.set_mode(m);
        }
    }
    if apply {
        commit(state);
    } else if discard {
        cancel(state);
    }
}
