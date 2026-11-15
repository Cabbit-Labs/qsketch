//! Skeletal (cut-out) animation: a bone tree, layers attached to bones, and
//! keyframed animations that move the bones.
//!
//! * A [`Bone`] has a *setup pose* (position, rotation in degrees, scale)
//!   relative to its parent. Bones are stored parents-first.
//! * A pixel layer is attached to one bone with an [`Attachment`]; it follows
//!   that bone's motion relative to the setup pose, so the picture you drew
//!   is what shows when no animation is playing.
//! * A [`RigAnim`] holds per-bone timelines of keys. Key values are *offsets
//!   from the setup pose* (translate adds, rotate adds, scale multiplies), so
//!   re-posing the setup pose never breaks an animation.
//! * [`render_pose`] draws the document with a pose applied; [`bake`] turns
//!   an animation into ordinary frames/cels for the existing GIF, sheet and
//!   PNG exports.
//!
//! Layers keep their pixels untouched: the pose is applied to a copy at
//! render time, so rigging is non-destructive.

use serde::{Deserialize, Serialize};

use crate::anim::{Cel, Frame};
use crate::composite::flatten_par;
use crate::document::DocState;
use crate::geom::{IRect, Pt};
use crate::layer::{LayerId, LayerKind};
use crate::raster::{Raster, ResizeFilter};
use crate::smart::{SmartMode, SmartObject, SmartSource};

pub type BoneId = u32;

/// Most frames a bake will produce.
pub const MAX_BAKE_FRAMES: usize = 2000;

// ---------------------------------------------------------------------------
// Transforms

/// A 2D affine transform, laid out like an SVG `matrix(a b c d e f)`:
/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Xf {
    pub const IDENTITY: Xf = Xf { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// Translate by `(x, y)` after rotating `rot_deg` degrees (clockwise on
    /// screen) and scaling by `(sx, sy)`.
    pub fn trs(x: f32, y: f32, rot_deg: f32, sx: f32, sy: f32) -> Xf {
        let (s, c) = rot_deg.to_radians().sin_cos();
        Xf { a: c * sx, b: s * sx, c: -s * sy, d: c * sy, e: x, f: y }
    }

    pub fn inverse(self) -> Option<Xf> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-9 {
            return None;
        }
        let (ia, ib, ic, id) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Xf { a: ia, b: ib, c: ic, d: id, e: -(ia * self.e + ic * self.f), f: -(ib * self.e + id * self.f) })
    }

    pub fn apply(self, p: Pt) -> Pt {
        Pt::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }

    pub fn is_identity(self) -> bool {
        let n = |v: f32, t: f32| (v - t).abs() < 1e-4;
        n(self.a, 1.0) && n(self.b, 0.0) && n(self.c, 0.0) && n(self.d, 1.0) && n(self.e, 0.0) && n(self.f, 0.0)
    }
}

impl std::ops::Mul for Xf {
    type Output = Xf;

    /// `self * o`: apply `o` first, then `self`.
    fn mul(self, o: Xf) -> Xf {
        Xf {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }
}

// ---------------------------------------------------------------------------
// Data

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bone {
    pub id: BoneId,
    pub name: String,
    pub parent: Option<BoneId>,
    /// Setup pose, relative to the parent (document pixels for a root).
    pub x: f32,
    pub y: f32,
    /// Degrees, clockwise on screen.
    pub rotation: f32,
    pub scale: [f32; 2],
    /// Drawn length along the bone's x axis (display only).
    pub length: f32,
}

/// How a value eases from one key to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Curve {
    #[default]
    Linear,
    /// Holds this key's value until the next key.
    Stepped,
    /// Cubic Bézier through (0,0) and (1,1) with control points
    /// `(c[0], c[1])` and `(c[2], c[3])`; x is time, y is progress.
    Bezier([f32; 4]),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key<T> {
    pub time_ms: u32,
    pub value: T,
    #[serde(default)]
    pub curve: Curve,
}

/// The keys of one bone in one animation; each list is sorted by time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoneTrack {
    pub bone: BoneId,
    /// Offset added to the bone's setup position.
    #[serde(default)]
    pub translate: Vec<Key<[f32; 2]>>,
    /// Degrees added to the setup rotation.
    #[serde(default)]
    pub rotate: Vec<Key<f32>>,
    /// Factor multiplied into the setup scale.
    #[serde(default)]
    pub scale: Vec<Key<[f32; 2]>>,
}

impl BoneTrack {
    fn new(bone: BoneId) -> Self {
        Self { bone, translate: Vec::new(), rotate: Vec::new(), scale: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigAnim {
    pub name: String,
    pub duration_ms: u32,
    #[serde(default = "yes")]
    pub looped: bool,
    #[serde(default)]
    pub tracks: Vec<BoneTrack>,
    /// Mix / bend keys of the IK constraints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ik: Vec<IkTrack>,
}

fn yes() -> bool {
    true
}

/// Which handle of a bone is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoneDrag {
    /// The head: moves the bone.
    Move,
    /// The tip: turns the bone toward the pointer.
    Aim,
    /// The tip, in the setup pose: turns the bone and sets its length.
    AimStretch,
}

/// Which property of a bone a key sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyValue {
    Translate([f32; 2]),
    Rotate(f32),
    Scale([f32; 2]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub layer: LayerId,
    pub bone: BoneId,
}

/// A two-bone inverse-kinematics constraint: the child bone's tip is
/// brought to the `target` bone's head by turning both bones of the chain
/// (a limb: upper arm and forearm, thigh and shin).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IkConstraint {
    pub id: u32,
    pub name: String,
    /// `[parent, child]`; the child must be a direct child of the parent.
    pub bones: [BoneId; 2],
    pub target: BoneId,
    /// Which way the joint bends: `1` or `-1` (the side of the line to the
    /// target the child's head falls on, turned clockwise or not).
    pub bend: i8,
    /// How much IK replaces the keyed rotations, 0..=1.
    pub mix: f32,
}

/// What an IK key sets: the constraint's mix and bend.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct IkValue {
    pub mix: f32,
    pub bend: i8,
}

/// The keys of one constraint in one animation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IkTrack {
    pub constraint: u32,
    pub keys: Vec<Key<IkValue>>,
}

/// A document's skeleton, attachments and animations.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rig {
    #[serde(default)]
    pub bones: Vec<Bone>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub animations: Vec<RigAnim>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ik: Vec<IkConstraint>,
    #[serde(default)]
    next_bone_id: BoneId,
    #[serde(default)]
    next_ik_id: u32,
}

/// A bone's local transform values while a pose is being worked out.
#[derive(Clone, Copy, Debug)]
struct Local {
    x: f32,
    y: f32,
    rot: f32,
    sx: f32,
    sy: f32,
}

/// `deg` wrapped to -180..180.
fn wrap180(deg: f32) -> f32 {
    (deg + 540.0).rem_euclid(360.0) - 180.0
}

// ---------------------------------------------------------------------------
// Sampling

trait Lerp: Copy {
    fn lerp(self, o: Self, t: f32) -> Self;
}
impl Lerp for f32 {
    fn lerp(self, o: f32, t: f32) -> f32 {
        self + (o - self) * t
    }
}
impl Lerp for IkValue {
    /// The mix blends; the bend is stepped (it can't be halfway).
    fn lerp(self, o: IkValue, t: f32) -> IkValue {
        IkValue { mix: self.mix.lerp(o.mix, t), bend: self.bend }
    }
}
impl Lerp for [f32; 2] {
    fn lerp(self, o: [f32; 2], t: f32) -> [f32; 2] {
        [self[0].lerp(o[0], t), self[1].lerp(o[1], t)]
    }
}

impl Curve {
    /// Progress (0..=1 in, 0..=1 out for sane curves) after `t` of the way
    /// from one key to the next.
    pub fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => t,
            Curve::Stepped => 0.0,
            Curve::Bezier([x1, y1, x2, y2]) => {
                let bez = |a: f32, b: f32, s: f32| {
                    let u = 1.0 - s;
                    3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
                };
                // Solve x(s) = t by bisection (x is monotonic for x1, x2 in 0..=1).
                let (mut lo, mut hi) = (0.0f32, 1.0f32);
                for _ in 0..24 {
                    let mid = (lo + hi) / 2.0;
                    if bez(x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0), mid) < t {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                bez(y1, y2, (lo + hi) / 2.0)
            }
        }
    }
}

/// The value of a sorted key list at `t` ms, or `None` with no keys.
fn sample<T: Lerp>(keys: &[Key<T>], t: f32) -> Option<T> {
    let first = keys.first()?;
    if t <= first.time_ms as f32 {
        return Some(first.value);
    }
    let last = keys.last()?;
    if t >= last.time_ms as f32 {
        return Some(last.value);
    }
    let i = keys.partition_point(|k| (k.time_ms as f32) <= t);
    let (a, b) = (&keys[i - 1], &keys[i]);
    let span = (b.time_ms - a.time_ms).max(1) as f32;
    Some(a.value.lerp(b.value, a.curve.ease((t - a.time_ms as f32) / span)))
}

fn upsert<T>(keys: &mut Vec<Key<T>>, time_ms: u32, value: T, curve: Curve) {
    match keys.binary_search_by_key(&time_ms, |k| k.time_ms) {
        Ok(i) => {
            keys[i].value = value;
        }
        Err(i) => keys.insert(i, Key { time_ms, value, curve }),
    }
}

impl RigAnim {
    /// The time to sample at for a playhead of `t_ms` (wrapped when looped,
    /// clamped otherwise).
    pub fn local_time(&self, t_ms: u32) -> f32 {
        if self.duration_ms == 0 {
            return 0.0;
        }
        if self.looped {
            (t_ms % self.duration_ms) as f32
        } else {
            t_ms.min(self.duration_ms) as f32
        }
    }
}

// ---------------------------------------------------------------------------
// Editing and posing

impl Rig {
    pub fn is_empty(&self) -> bool {
        self.bones.is_empty() && self.animations.is_empty() && self.attachments.is_empty()
    }

    pub fn bone_index(&self, id: BoneId) -> Option<usize> {
        self.bones.iter().position(|b| b.id == id)
    }

    pub fn bone(&self, id: BoneId) -> Option<&Bone> {
        self.bones.iter().find(|b| b.id == id)
    }

    pub fn bone_mut(&mut self, id: BoneId) -> Option<&mut Bone> {
        self.bones.iter_mut().find(|b| b.id == id)
    }

    /// Add a bone under `parent` (a root when `None` or unknown), with its
    /// setup position relative to the parent. Returns its id.
    pub fn add_bone(&mut self, name: impl Into<String>, parent: Option<BoneId>, x: f32, y: f32, length: f32) -> BoneId {
        let parent = parent.filter(|&p| self.bone(p).is_some());
        let id = self.next_bone_id;
        self.next_bone_id += 1;
        self.bones.push(Bone { id, name: name.into(), parent, x, y, rotation: 0.0, scale: [1.0, 1.0], length });
        id
    }

    /// Ids of `id` and everything below it.
    pub fn subtree(&self, id: BoneId) -> Vec<BoneId> {
        let mut out = vec![id];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i];
            out.extend(self.bones.iter().filter(|b| b.parent == Some(cur)).map(|b| b.id));
            i += 1;
        }
        out
    }

    /// Remove a bone and its children, with their keys and attachments.
    pub fn remove_bone(&mut self, id: BoneId) {
        let gone = self.subtree(id);
        self.bones.retain(|b| !gone.contains(&b.id));
        self.attachments.retain(|a| !gone.contains(&a.bone));
        for an in &mut self.animations {
            an.tracks.retain(|t| !gone.contains(&t.bone));
        }
        let dead: Vec<u32> = self
            .ik
            .iter()
            .filter(|c| gone.contains(&c.target) || c.bones.iter().any(|b| gone.contains(b)))
            .map(|c| c.id)
            .collect();
        for id in dead {
            self.remove_ik(id);
        }
    }

    /// Make `layer` follow `bone` (replacing any earlier attachment).
    pub fn attach(&mut self, layer: LayerId, bone: BoneId) {
        if self.bone(bone).is_none() {
            return;
        }
        self.detach(layer);
        self.attachments.push(Attachment { layer, bone });
    }

    pub fn detach(&mut self, layer: LayerId) {
        self.attachments.retain(|a| a.layer != layer);
    }

    pub fn attached_bone(&self, layer: LayerId) -> Option<BoneId> {
        self.attachments.iter().find(|a| a.layer == layer).map(|a| a.bone)
    }

    /// Drop attachments to layers the document no longer has.
    pub fn prune(&mut self, doc_layers: &[LayerId]) {
        self.attachments.retain(|a| doc_layers.contains(&a.layer));
    }

    pub fn add_animation(&mut self, name: impl Into<String>, duration_ms: u32) -> usize {
        self.animations.push(RigAnim {
            name: name.into(),
            duration_ms: duration_ms.max(1),
            looped: true,
            ik: Vec::new(),
            tracks: Vec::new(),
        });
        self.animations.len() - 1
    }

    /// Set (or add) a key of `bone` in animation `anim` at `time_ms`.
    pub fn set_key(&mut self, anim: usize, bone: BoneId, time_ms: u32, value: KeyValue, curve: Curve) {
        if self.bone(bone).is_none() {
            return;
        }
        let Some(an) = self.animations.get_mut(anim) else { return };
        let time_ms = time_ms.min(an.duration_ms);
        let ti = match an.tracks.iter().position(|t| t.bone == bone) {
            Some(i) => i,
            None => {
                an.tracks.push(BoneTrack::new(bone));
                an.tracks.len() - 1
            }
        };
        let t = &mut an.tracks[ti];
        match value {
            KeyValue::Translate(v) => upsert(&mut t.translate, time_ms, v, curve),
            KeyValue::Rotate(v) => upsert(&mut t.rotate, time_ms, v, curve),
            KeyValue::Scale(v) => upsert(&mut t.scale, time_ms, v, curve),
        }
    }

    /// Remove every key of `bone` at `time_ms`.
    pub fn remove_keys_at(&mut self, anim: usize, bone: BoneId, time_ms: u32) {
        let Some(an) = self.animations.get_mut(anim) else { return };
        if let Some(t) = an.tracks.iter_mut().find(|t| t.bone == bone) {
            t.translate.retain(|k| k.time_ms != time_ms);
            t.rotate.retain(|k| k.time_ms != time_ms);
            t.scale.retain(|k| k.time_ms != time_ms);
        }
    }

    /// A bone's animated offsets at `t_ms` in animation `anim`: translate,
    /// rotate (degrees) and scale (factor); no keys means no change.
    pub fn offsets(&self, anim: usize, bone: BoneId, t_ms: u32) -> ([f32; 2], f32, [f32; 2]) {
        let Some(a) = self.animations.get(anim) else { return ([0.0; 2], 0.0, [1.0; 2]) };
        let t = a.local_time(t_ms);
        match a.tracks.iter().find(|tr| tr.bone == bone) {
            Some(tr) => (
                sample(&tr.translate, t).unwrap_or([0.0; 2]),
                sample(&tr.rotate, t).unwrap_or(0.0),
                sample(&tr.scale, t).unwrap_or([1.0; 2]),
            ),
            None => ([0.0; 2], 0.0, [1.0; 2]),
        }
    }

    /// The times (ms) at which `bone` has any key in animation `anim`.
    pub fn key_times(&self, anim: usize, bone: BoneId) -> Vec<u32> {
        let Some(tr) = self.animations.get(anim).and_then(|a| a.tracks.iter().find(|t| t.bone == bone)) else {
            return Vec::new();
        };
        let mut v: Vec<u32> = tr
            .translate
            .iter()
            .map(|k| k.time_ms)
            .chain(tr.rotate.iter().map(|k| k.time_ms))
            .chain(tr.scale.iter().map(|k| k.time_ms))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// World position of a bone's head (its origin) in `pose`.
    pub fn head(&self, bone: BoneId, pose: &[Xf]) -> Option<Pt> {
        Some(pose.get(self.bone_index(bone)?)?.apply(Pt::new(0.0, 0.0)))
    }

    /// World position of a bone's tip (one `length` along its x axis).
    pub fn tip(&self, bone: BoneId, pose: &[Xf]) -> Option<Pt> {
        let i = self.bone_index(bone)?;
        Some(pose.get(i)?.apply(Pt::new(self.bones[i].length, 0.0)))
    }

    /// Move or turn `bone` by dragging a handle to `target` (document
    /// space). Without an animation this edits the setup pose; with one it
    /// writes a key at `t_ms` (offsets from the setup pose), leaving the
    /// other properties alone. Keys keep their easing when overwritten.
    pub fn drag_bone(
        &mut self,
        anim: Option<usize>,
        t_ms: u32,
        bone: BoneId,
        drag: BoneDrag,
        target: Pt,
        curve: Curve,
    ) {
        let Some(bi) = self.bone_index(bone) else { return };
        // The parent's pose doesn't depend on this bone's own values.
        let pose = self.pose(anim.map(|a| (a, t_ms)));
        let parent = self.bones[bi].parent.and_then(|p| self.bone_index(p)).and_then(|i| pose.get(i).copied());
        let parent = parent.unwrap_or(Xf::IDENTITY);
        let head_world = pose[bi].apply(Pt::new(0.0, 0.0));
        let b = self.bones[bi].clone();
        match drag {
            BoneDrag::Move => {
                let local = parent.inverse().map_or(target, |inv| inv.apply(target));
                match anim {
                    None => {
                        let b = &mut self.bones[bi];
                        (b.x, b.y) = (local.x, local.y);
                    }
                    Some(a) => {
                        let off = [local.x - b.x, local.y - b.y];
                        self.set_key(a, bone, t_ms, KeyValue::Translate(off), curve);
                    }
                }
            }
            BoneDrag::Aim | BoneDrag::AimStretch => {
                let (dx, dy) = (target.x - head_world.x, target.y - head_world.y);
                if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
                    return;
                }
                let world_deg = dy.atan2(dx).to_degrees();
                let parent_deg = parent.b.atan2(parent.a).to_degrees();
                let local = world_deg - parent_deg;
                match anim {
                    None => {
                        let bm = &mut self.bones[bi];
                        bm.rotation = local;
                        if drag == BoneDrag::AimStretch {
                            bm.length = (dx * dx + dy * dy).sqrt().max(1.0);
                        }
                    }
                    Some(a) => {
                        let cur = self.offsets(a, bone, t_ms).1;
                        let want = local - b.rotation;
                        // Take the turn the short way from where it is now.
                        let off = cur + (want - cur + 540.0).rem_euclid(360.0) - 180.0;
                        self.set_key(a, bone, t_ms, KeyValue::Rotate(off), curve);
                    }
                }
            }
        }
    }

    /// World transform of every bone (in `bones` order). With an animation,
    /// its keys at `t_ms` are applied on top of the setup pose and the IK
    /// constraints are solved; the setup pose itself is plain forward
    /// kinematics (the pose the art was drawn in).
    pub fn pose(&self, anim: Option<(usize, u32)>) -> Vec<Xf> {
        let an = anim.and_then(|(i, t)| self.animations.get(i).map(|a| (a, a.local_time(t))));
        let mut locals: Vec<Local> = self
            .bones
            .iter()
            .map(|b| {
                let mut l = Local { x: b.x, y: b.y, rot: b.rotation, sx: b.scale[0], sy: b.scale[1] };
                if let Some((a, t)) = an {
                    if let Some(tr) = a.tracks.iter().find(|tr| tr.bone == b.id) {
                        if let Some(v) = sample(&tr.translate, t) {
                            l.x += v[0];
                            l.y += v[1];
                        }
                        if let Some(v) = sample(&tr.rotate, t) {
                            l.rot += v;
                        }
                        if let Some(v) = sample(&tr.scale, t) {
                            l.sx *= v[0];
                            l.sy *= v[1];
                        }
                    }
                }
                l
            })
            .collect();
        if let Some((a, t)) = an {
            self.solve_ik(&mut locals, Some((a, t)));
        }
        self.worlds(&locals)
    }

    fn worlds(&self, locals: &[Local]) -> Vec<Xf> {
        let mut world: Vec<Xf> = Vec::with_capacity(self.bones.len());
        for (b, l) in self.bones.iter().zip(locals) {
            let local = Xf::trs(l.x, l.y, l.rot, l.sx, l.sy);
            let parent = b.parent.and_then(|p| self.bone_index(p)).filter(|&i| i < world.len());
            world.push(parent.map_or(local, |i| world[i] * local));
        }
        world
    }

    /// A constraint's mix and bend at `t_ms` of animation `anim`.
    pub fn ik_at(&self, anim: usize, id: u32, t_ms: u32) -> Option<IkValue> {
        let c = self.ik.iter().find(|c| c.id == id)?;
        let a = self.animations.get(anim)?;
        let v = a.ik.iter().find(|t| t.constraint == id).and_then(|t| sample(&t.keys, a.local_time(t_ms)));
        Some(v.unwrap_or(IkValue { mix: c.mix, bend: c.bend }))
    }

    /// Turn the chains of the IK constraints toward their targets.
    fn solve_ik(&self, locals: &mut [Local], an: Option<(&RigAnim, f32)>) {
        for c in &self.ik {
            let v = an
                .and_then(|(a, t)| a.ik.iter().find(|tr| tr.constraint == c.id).and_then(|tr| sample(&tr.keys, t)))
                .unwrap_or(IkValue { mix: c.mix, bend: c.bend });
            let mix = v.mix.clamp(0.0, 1.0);
            if mix <= 0.0 {
                continue;
            }
            let (Some(pi), Some(ci), Some(ti)) =
                (self.bone_index(c.bones[0]), self.bone_index(c.bones[1]), self.bone_index(c.target))
            else {
                continue;
            };
            if self.bones[ci].parent != Some(c.bones[0]) {
                continue;
            }
            let world = self.worlds(locals);
            let target = world[ti].apply(Pt::new(0.0, 0.0));
            if let Some((rp, rc)) = self.ik_rotations(locals, pi, ci, target, v.bend) {
                let (fp, fc) = (locals[pi].rot, locals[ci].rot);
                locals[pi].rot = fp + mix * wrap180(rp - fp);
                // The child aims at the target from wherever the (blended)
                // parent put it.
                let rc = self.aim_child(locals, pi, ci, target).unwrap_or(rc);
                locals[ci].rot = fc + mix * wrap180(rc - fc);
            }
        }
    }

    /// The local rotations `[parent, child]` that put the child's tip on
    /// `target`, bending toward `bend` (±1). `None` when the chain has no
    /// length. Out of reach, the chain straightens toward the target.
    fn ik_rotations(&self, locals: &[Local], pi: usize, ci: usize, target: Pt, bend: i8) -> Option<(f32, f32)> {
        let world = self.worlds(locals);
        let (pw, cw) = (world[pi], world[ci]);
        let h = pw.apply(Pt::new(0.0, 0.0));
        let head = cw.apply(Pt::new(0.0, 0.0));
        let a = h.dist(head);
        let b = self.bones[ci].length.max(0.0) * (cw.a * cw.a + cw.b * cw.b).sqrt();
        if a < 1e-4 || b < 1e-4 {
            return None;
        }
        let d_raw = h.dist(target);
        if d_raw < 1e-4 {
            return None;
        }
        // Out of reach the chain straightens (or folds flat when the target
        // is too close); never a zero distance.
        let d = d_raw.clamp(((a - b).abs()).max(1e-4), a + b);
        let cos_a = ((a * a + d * d - b * b) / (2.0 * a * d)).clamp(-1.0, 1.0);
        let alpha = cos_a.acos();
        let to_target = (target.y - h.y).atan2(target.x - h.x);
        let head_dir = to_target + if bend < 0 { -alpha } else { alpha };
        // Where the child's head sits relative to the parent's own x axis.
        let offset = (head.y - h.y).atan2(head.x - h.x) - pw.b.atan2(pw.a);
        let parent_of_p = self.bones[pi].parent.and_then(|p| self.bone_index(p)).map_or(Xf::IDENTITY, |i| world[i]);
        let rp = (head_dir - offset - parent_of_p.b.atan2(parent_of_p.a)).to_degrees();
        let mut next = locals.to_vec();
        next[pi].rot = rp;
        let rc = self.aim_child(&next, pi, ci, target)?;
        Some((rp, rc))
    }

    /// The child's local rotation that points it at `target`, given the
    /// parent's current rotation.
    fn aim_child(&self, locals: &[Local], pi: usize, ci: usize, target: Pt) -> Option<f32> {
        let world = self.worlds(locals);
        let head = world[ci].apply(Pt::new(0.0, 0.0));
        let (dx, dy) = (target.x - head.x, target.y - head.y);
        if dx.abs() < 1e-4 && dy.abs() < 1e-4 {
            return None;
        }
        Some((dy.atan2(dx) - world[pi].b.atan2(world[pi].a)).to_degrees())
    }

    /// Add a two-bone IK constraint on `child` and its parent, aimed at a
    /// new root bone placed on the child's tip. The bend is the one that
    /// keeps the limb as drawn. Returns the constraint's id and the target
    /// bone's id.
    pub fn add_ik(&mut self, child: BoneId) -> Option<(u32, BoneId)> {
        let ci = self.bone_index(child)?;
        let parent = self.bones[ci].parent?;
        let pi = self.bone_index(parent)?;
        let setup = self.pose(None);
        let tip = setup[ci].apply(Pt::new(self.bones[ci].length, 0.0));
        let locals: Vec<Local> = self
            .bones
            .iter()
            .map(|b| Local { x: b.x, y: b.y, rot: b.rotation, sx: b.scale[0], sy: b.scale[1] })
            .collect();
        let fk = locals[pi].rot;
        let err =
            |bend: i8| self.ik_rotations(&locals, pi, ci, tip, bend).map_or(f32::MAX, |(rp, _)| wrap180(rp - fk).abs());
        let bend = if err(-1) < err(1) { -1 } else { 1 };
        let n = self.ik.len() + 1;
        let tname = format!("{} target", self.bones[ci].name);
        let target = self.add_bone(tname, None, tip.x, tip.y, (self.bones[ci].length * 0.4).max(6.0));
        let id = self.next_ik_id;
        self.next_ik_id += 1;
        self.ik.push(IkConstraint { id, name: format!("ik {n}"), bones: [parent, child], target, bend, mix: 1.0 });
        Some((id, target))
    }

    pub fn remove_ik(&mut self, id: u32) {
        self.ik.retain(|c| c.id != id);
        for a in &mut self.animations {
            a.ik.retain(|t| t.constraint != id);
        }
    }

    /// The constraint whose chain includes `bone`, if any.
    pub fn ik_of_bone(&self, bone: BoneId) -> Option<&IkConstraint> {
        self.ik.iter().find(|c| c.bones.contains(&bone))
    }

    /// Set (or add) an IK key at `time_ms` in animation `anim`.
    pub fn set_ik_key(&mut self, anim: usize, id: u32, time_ms: u32, value: IkValue, curve: Curve) {
        if !self.ik.iter().any(|c| c.id == id) {
            return;
        }
        let Some(an) = self.animations.get_mut(anim) else { return };
        let time_ms = time_ms.min(an.duration_ms);
        let ti = match an.ik.iter().position(|t| t.constraint == id) {
            Some(i) => i,
            None => {
                an.ik.push(IkTrack { constraint: id, keys: Vec::new() });
                an.ik.len() - 1
            }
        };
        upsert(&mut an.ik[ti].keys, time_ms, value, curve);
    }

    /// How far a layer attached to `bone` has moved from where it was drawn:
    /// `pose · setup⁻¹`.
    pub fn layer_delta(&self, bone: BoneId, pose: &[Xf], setup: &[Xf]) -> Option<Xf> {
        let i = self.bone_index(bone)?;
        Some(*pose.get(i)? * setup.get(i)?.inverse()?)
    }
}

// ---------------------------------------------------------------------------
// Rendering

/// `raster` moved by `xf` (document space), resampled with `filter`.
pub fn transform_raster(raster: &Raster, xf: Xf, width: u32, height: u32, filter: ResizeFilter) -> Raster {
    if xf.is_identity() {
        return raster.clone();
    }
    let Some(b) = raster.full_bounds() else { return raster.clone() };
    let (x0, y0, x1, y1) = (b.x as f32, b.y as f32, b.right() as f32, b.bottom() as f32);
    let mut so = SmartObject::identity(IRect::new(b.x, b.y, b.w, b.h));
    so.mode = SmartMode::Deform;
    so.quad =
        [xf.apply(Pt::new(x0, y0)), xf.apply(Pt::new(x1, y0)), xf.apply(Pt::new(x1, y1)), xf.apply(Pt::new(x0, y1))];
    so.filter = filter;
    so.render(&SmartSource::Pixels(raster.crop(b)), width, height)
}

/// A copy of `doc` with the pose of `anim` at `t_ms` applied to every
/// attached layer (their masks baked in, since a mask can't follow a bone).
pub fn posed_copy(doc: &DocState, anim: Option<usize>, t_ms: u32, filter: ResizeFilter) -> DocState {
    let mut out = doc.clone();
    if doc.rig.attachments.is_empty() {
        return out;
    }
    let setup = doc.rig.pose(None);
    let pose = doc.rig.pose(anim.map(|a| (a, t_ms)));
    let (w, h) = (doc.width, doc.height);
    for l in &mut out.layers {
        let Some(bone) = doc.rig.attached_bone(l.id()) else { continue };
        let Some(delta) = doc.rig.layer_delta(bone, &pose, &setup) else { continue };
        if !l.owns_pixels() || delta.is_identity() {
            continue;
        }
        l.apply_mask();
        l.raster = transform_raster(&l.raster, delta, w, h, filter);
    }
    out
}

/// The flattened document with the pose of `anim` at `t_ms` applied.
pub fn render_pose(doc: &DocState, anim: Option<usize>, t_ms: u32, filter: ResizeFilter) -> Raster {
    flatten_par(&posed_copy(doc, anim, t_ms, filter))
}

/// How many frames a bake of `anim` at `fps` makes: a looping animation
/// leaves out the last instant (it equals the first); one that doesn't loop
/// includes it.
pub fn bake_frame_count(anim: &RigAnim, fps: u32) -> usize {
    let fps = fps.clamp(1, 240) as u64;
    let total = anim.duration_ms as u64 * fps;
    let n = if anim.looped { (total + 500) / 1000 } else { total / 1000 + 1 };
    (n as usize).clamp(1, MAX_BAKE_FRAMES)
}

/// Turn animation `anim` into a new document of ordinary frames: attached
/// layers get one cel per frame; other pixel layers repeat their current
/// picture (as links). The result has no rig, so it exports like any other
/// animation (GIF, sprite sheet, PNG sequence, `.ase`).
pub fn bake(doc: &DocState, anim: usize, fps: u32, filter: ResizeFilter) -> Result<DocState, String> {
    let an = doc.rig.animations.get(anim).ok_or("No such animation")?;
    if fps == 0 {
        return Err("Frame rate must be at least 1".into());
    }
    let fps = fps.clamp(1, 240);
    let n = bake_frame_count(an, fps);
    let mut out = doc.clone();
    out.sync_cels();
    let (w, h) = (doc.width, doc.height);
    let setup = doc.rig.pose(None);
    let step = 1000.0 / fps as f32;

    // Each frame's pose, computed once for all layers.
    let poses: Vec<Vec<Xf>> = (0..n).map(|i| doc.rig.pose(Some((anim, (i as f32 * step).round() as u32)))).collect();

    for l in &mut out.layers {
        let rigged = doc.rig.attached_bone(l.id()).filter(|_| l.owns_pixels());
        if rigged.is_none() && !l.animated() {
            l.cels.clear();
            continue;
        }
        l.apply_mask();
        if let Some(bone) = rigged {
            // Shape, text and smart layers become plain pixels: their
            // frames can no longer be redrawn from points.
            l.props.kind = LayerKind::Raster;
            l.props.shape = None;
            l.props.text = None;
            l.props.smart = None;
            l.smart = None;
            let base = l.raster.clone();
            l.cels = poses
                .iter()
                .map(|pose| {
                    let delta = doc.rig.layer_delta(bone, pose, &setup).unwrap_or(Xf::IDENTITY);
                    Cel::own(transform_raster(&base, delta, w, h, filter))
                })
                .collect();
        } else {
            let base = l.raster.clone();
            l.cels = (0..n).map(|i| if i == 0 { Cel::own(base.clone()) } else { Cel::linked(0, w, h) }).collect();
        }
    }
    let ms = (1000.0 / fps as f32).round().max(1.0) as u32;
    out.frames = vec![Frame { duration_ms: ms }; n];
    out.tags.clear();
    out.frame = 0;
    out.rig = Rig::default();
    out.load_cels();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn xf_inverse_round_trips() {
        let m = Xf::trs(12.0, -7.0, 33.0, 1.5, 0.8);
        let p = Pt::new(4.0, 9.0);
        let q = m.inverse().unwrap().apply(m.apply(p));
        assert!(near(p.x, q.x) && near(p.y, q.y));
    }

    #[test]
    fn curves_ease() {
        assert!(near(Curve::Linear.ease(0.25), 0.25));
        assert!(near(Curve::Stepped.ease(0.9), 0.0));
        let c = Curve::Bezier([0.25, 0.1, 0.25, 1.0]);
        assert!(near(c.ease(0.0), 0.0) && near(c.ease(1.0), 1.0));
        assert!(c.ease(0.5) > 0.5, "ease-out curve runs ahead of linear");
    }

    #[test]
    fn sampling_holds_ends_and_interpolates() {
        let keys = vec![
            Key { time_ms: 100, value: 0.0f32, curve: Curve::Linear },
            Key { time_ms: 300, value: 40.0, curve: Curve::Stepped },
            Key { time_ms: 500, value: 80.0, curve: Curve::Linear },
        ];
        assert!(near(sample(&keys, 0.0).unwrap(), 0.0));
        assert!(near(sample(&keys, 200.0).unwrap(), 20.0));
        assert!(near(sample(&keys, 400.0).unwrap(), 40.0), "stepped holds");
        assert!(near(sample(&keys, 900.0).unwrap(), 80.0));
        assert!(sample::<f32>(&[], 1.0).is_none());
    }

    #[test]
    fn child_follows_parent() {
        let mut rig = Rig::default();
        let root = rig.add_bone("root", None, 0.0, 0.0, 10.0);
        let arm = rig.add_bone("arm", Some(root), 10.0, 0.0, 10.0);
        let a = rig.add_animation("swing", 1000);
        rig.set_key(a, root, 0, KeyValue::Rotate(90.0), Curve::Linear);
        let world = rig.pose(Some((a, 0)));
        let tip = world[rig.bone_index(arm).unwrap()].apply(Pt::new(0.0, 0.0));
        // The arm sits 10 along the root's x axis, which now points down.
        assert!(near(tip.x, 0.0) && near(tip.y, 10.0), "{tip:?}");
    }

    #[test]
    fn setup_pose_is_identity_delta() {
        let mut rig = Rig::default();
        let b = rig.add_bone("b", None, 30.0, 20.0, 5.0);
        let setup = rig.pose(None);
        assert!(rig.layer_delta(b, &setup, &setup).unwrap().is_identity());
    }

    #[test]
    fn remove_bone_takes_its_subtree() {
        let mut rig = Rig::default();
        let r = rig.add_bone("r", None, 0.0, 0.0, 1.0);
        let c = rig.add_bone("c", Some(r), 1.0, 0.0, 1.0);
        rig.attach(7, c);
        let a = rig.add_animation("a", 100);
        rig.set_key(a, c, 0, KeyValue::Rotate(5.0), Curve::Linear);
        rig.remove_bone(r);
        assert!(rig.bones.is_empty() && rig.attachments.is_empty() && rig.animations[0].tracks.is_empty());
    }

    #[test]
    fn bake_moves_an_attached_layer_frame_by_frame() {
        let mut doc = DocState::new(64, 32, None);
        let red = Rgba8::new(255, 0, 0, 255);
        doc.layers[0].raster.fill_rect(IRect::new(4, 4, 8, 8), red);
        let id = doc.layers[0].id();
        let bone = doc.rig.add_bone("body", None, 8.0, 8.0, 10.0);
        doc.rig.attach(id, bone);
        let a = doc.rig.add_animation("slide", 1000);
        doc.rig.set_key(a, bone, 0, KeyValue::Translate([0.0, 0.0]), Curve::Linear);
        doc.rig.set_key(a, bone, 1000, KeyValue::Translate([40.0, 0.0]), Curve::Linear);

        let baked = bake(&doc, a, 4, ResizeFilter::Nearest).unwrap();
        assert_eq!(baked.frames.len(), 4, "looped: 1 s at 4 fps leaves out the endpoint");
        assert!(baked.rig.is_empty());
        let left = |frame: usize| {
            let r = render_frame_raster(&baked, frame);
            r.bounds().map(|b| b.x)
        };
        assert_eq!(left(0), Some(4));
        assert_eq!(left(2), Some(24), "halfway through the slide");
    }

    #[test]
    fn dragging_the_tip_aims_the_bone_through_a_turned_parent() {
        let mut rig = Rig::default();
        let root = rig.add_bone("root", None, 0.0, 0.0, 10.0);
        let arm = rig.add_bone("arm", Some(root), 10.0, 0.0, 10.0);
        rig.bone_mut(root).unwrap().rotation = 90.0;
        // Setup: aim the arm at (10, 30), straight down the page from its head.
        rig.drag_bone(None, 0, arm, BoneDrag::AimStretch, Pt::new(0.0, 30.0), Curve::Linear);
        let pose = rig.pose(None);
        let (head, tip) = (rig.head(arm, &pose).unwrap(), rig.tip(arm, &pose).unwrap());
        assert!(near(head.x, 0.0) && near(head.y, 10.0), "{head:?}");
        assert!(near(tip.x, 0.0) && near(tip.y, 30.0), "{tip:?}");
        assert!(near(rig.bone(arm).unwrap().length, 20.0));
    }

    #[test]
    fn dragging_in_an_animation_keys_offsets_and_takes_the_short_turn() {
        let mut rig = Rig::default();
        let b = rig.add_bone("b", None, 5.0, 5.0, 10.0);
        let a = rig.add_animation("a", 1000);
        // Aim straight down (90°), then aim just past 180° the short way.
        rig.drag_bone(Some(a), 0, b, BoneDrag::Aim, Pt::new(5.0, 50.0), Curve::Linear);
        assert!(near(rig.offsets(a, b, 0).1, 90.0));
        rig.drag_bone(Some(a), 0, b, BoneDrag::Aim, Pt::new(-50.0, 6.0), Curve::Linear);
        let r = rig.offsets(a, b, 0).1;
        assert!(r > 90.0 && r < 181.0, "{r}");
        rig.drag_bone(Some(a), 500, b, BoneDrag::Move, Pt::new(25.0, 5.0), Curve::Linear);
        assert!(near(rig.offsets(a, b, 500).0[0], 20.0));
        assert!(rig.bone(b).unwrap().x == 5.0, "the setup pose is untouched");
    }

    /// An arm: upper bone 10 long from (0,0), forearm 10 long from its tip,
    /// both straight along +x, with an IK target.
    fn arm() -> (Rig, BoneId, BoneId, BoneId, usize) {
        let mut rig = Rig::default();
        let upper = rig.add_bone("upper", None, 0.0, 0.0, 10.0);
        let fore = rig.add_bone("fore", Some(upper), 10.0, 0.0, 10.0);
        let (_, target) = rig.add_ik(fore).unwrap();
        let a = rig.add_animation("a", 1000);
        (rig, upper, fore, target, a)
    }

    fn tip_at(rig: &Rig, bone: BoneId, a: usize, t: u32) -> Pt {
        rig.tip(bone, &rig.pose(Some((a, t)))).unwrap()
    }

    #[test]
    fn ik_reaches_a_target_within_reach() {
        let (mut rig, _, fore, target, a) = arm();
        rig.set_key(a, target, 0, KeyValue::Translate([-20.0, 10.0]), Curve::Linear); // tip (20,0) → (0,10)
        let tip = tip_at(&rig, fore, a, 0);
        assert!(near(tip.x, 0.0) && near(tip.y, 10.0), "{tip:?}");
    }

    #[test]
    fn ik_straightens_toward_a_target_out_of_reach() {
        let (mut rig, upper, fore, target, a) = arm();
        rig.set_key(a, target, 0, KeyValue::Translate([0.0, 80.0]), Curve::Linear); // target (20,80)
        let pose = rig.pose(Some((a, 0)));
        let (elbow, tip) = (rig.tip(upper, &pose).unwrap(), rig.tip(fore, &pose).unwrap());
        let dir = Pt::new(20.0, 80.0);
        let len = (dir.x * dir.x + dir.y * dir.y).sqrt();
        let close = |a: f32, b: f32| (a - b).abs() < 0.05;
        assert!(close(tip.x, 20.0 * 20.0 / len) && close(tip.y, 20.0 * 80.0 / len), "{tip:?}");
        // Straight: the elbow is halfway to the tip.
        assert!(close(elbow.x * 2.0, tip.x) && close(elbow.y * 2.0, tip.y), "{elbow:?} {tip:?}");
    }

    #[test]
    fn opposite_bends_put_the_elbow_on_opposite_sides() {
        let (mut rig, upper, _, target, a) = arm();
        rig.set_key(a, target, 0, KeyValue::Translate([-16.0, 0.0]), Curve::Linear); // tip (20,0) → (4,0)
        let side = |rig: &Rig| rig.tip(upper, &rig.pose(Some((a, 0)))).unwrap().y;
        let y1 = side(&rig);
        rig.ik[0].bend = -rig.ik[0].bend;
        let y2 = side(&rig);
        assert!(y1 * y2 < 0.0 && y1.abs() > 1.0, "{y1} {y2}");
    }

    #[test]
    fn ik_added_on_a_bent_limb_keeps_it_as_drawn() {
        let mut rig = Rig::default();
        let upper = rig.add_bone("upper", None, 0.0, 0.0, 10.0);
        let fore = rig.add_bone("fore", Some(upper), 10.0, 0.0, 10.0);
        rig.bone_mut(upper).unwrap().rotation = 20.0;
        rig.bone_mut(fore).unwrap().rotation = -70.0;
        let a = rig.add_animation("a", 1000);
        let before = rig.pose(None);
        rig.add_ik(fore).unwrap();
        let after = rig.pose(Some((a, 0)));
        for (b, w) in before.iter().zip(&after).take(2) {
            assert!(near(b.e, w.e) && near(b.f, w.f) && near(b.a, w.a) && near(b.b, w.b), "{b:?} vs {w:?}");
        }
    }

    #[test]
    fn mix_zero_is_forward_kinematics_and_half_is_between() {
        let (mut rig, upper, fore, target, a) = arm();
        rig.set_key(a, target, 0, KeyValue::Translate([-20.0, 10.0]), Curve::Linear);
        rig.ik[0].mix = 0.0;
        let fk = tip_at(&rig, fore, a, 0);
        assert!(near(fk.x, 20.0) && near(fk.y, 0.0), "{fk:?}");
        rig.ik[0].mix = 0.5;
        let half = rig.pose(Some((a, 0)));
        let r = half[rig.bone_index(upper).unwrap()].b.atan2(half[rig.bone_index(upper).unwrap()].a).to_degrees();
        rig.ik[0].mix = 1.0;
        let full = rig.pose(Some((a, 0)));
        let rf = full[rig.bone_index(upper).unwrap()].b.atan2(full[rig.bone_index(upper).unwrap()].a).to_degrees();
        assert!(r.abs() > 1.0 && r.abs() < rf.abs(), "{r} {rf}");
    }

    #[test]
    fn ik_keys_blend_the_mix_and_deleting_a_bone_drops_its_constraint() {
        let (mut rig, _, fore, target, a) = arm();
        let id = rig.ik[0].id;
        rig.set_ik_key(a, id, 0, IkValue { mix: 0.0, bend: 1 }, Curve::Linear);
        rig.set_ik_key(a, id, 1000, IkValue { mix: 1.0, bend: 1 }, Curve::Linear);
        assert!(near(rig.ik_at(a, id, 500).unwrap().mix, 0.5));
        let back: Rig = serde_json::from_str(&serde_json::to_string(&rig).unwrap()).unwrap();
        assert_eq!(rig, back);
        rig.remove_bone(target);
        assert!(rig.ik.is_empty() && rig.animations[0].ik.is_empty());
        let _ = fore;
    }

    #[test]
    fn rig_survives_json() {
        let mut rig = Rig::default();
        let b = rig.add_bone("b", None, 1.0, 2.0, 3.0);
        rig.attach(5, b);
        let a = rig.add_animation("a", 500);
        rig.set_key(a, b, 100, KeyValue::Rotate(12.0), Curve::Bezier([0.2, 0.0, 0.4, 1.0]));
        let back: Rig = serde_json::from_str(&serde_json::to_string(&rig).unwrap()).unwrap();
        assert_eq!(rig, back);
        // New bones keep getting fresh ids after a reload.
        let mut back = back;
        assert_ne!(back.add_bone("c", None, 0.0, 0.0, 1.0), b);
    }

    fn render_frame_raster(doc: &DocState, frame: usize) -> Raster {
        crate::anim::render_frame(doc, frame)
    }
}
