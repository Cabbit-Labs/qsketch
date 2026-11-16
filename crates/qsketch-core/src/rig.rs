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
    /// Opacity keys of layers (show, hide, fade).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alpha: Vec<AlphaTrack>,
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
    /// The tip, in an animation: scales the bone (and what hangs off it) by
    /// how far the tip is from the head, relative to its setup length.
    Scale,
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

/// A bone turned by another, like meshed gears or a belt: whatever an
/// animation turns `source` by (its keys plus its own drive), `bone` turns by
/// `ratio` times that, on top of its own keys. A ratio of -2 is a gear half
/// the size meshed with its source; 1 copies the turn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drive {
    pub bone: BoneId,
    pub source: BoneId,
    pub ratio: f32,
}

/// The opacity keys of one layer in one animation (0 = hidden, 1 = shown).
/// Use a stepped curve for an instant show or hide, linear to fade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlphaTrack {
    pub layer: LayerId,
    pub keys: Vec<Key<f32>>,
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

/// A named set of layers that show together (see `Rig::skins`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skin {
    pub name: String,
    pub layers: Vec<LayerId>,
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
    /// Layers that are not fully shown when no animation keys them (0 =
    /// hidden at rest); absent layers are fully shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rest_alpha: Vec<(LayerId, f32)>,
    /// Layers drawn through a weighted mesh (see `crate::rigmesh`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meshes: Vec<crate::rigmesh::PartMesh>,
    /// Bones turned by other bones (gears); at most one per bone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drives: Vec<Drive>,
    /// Skins: named sets of layers, to swap outfits or whole characters on
    /// one skeleton. A layer in any skin shows only while a skin holding it
    /// is the active one; layers in no skin always show.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skins: Vec<Skin>,
    /// The active skin (an index into `skins`); `None` shows every layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<usize>,
    /// Crossfade length between animations (see `mix_ms()`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mix_ms: Option<u32>,
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
        if self.looped && t_ms > self.duration_ms {
            (t_ms % self.duration_ms) as f32
        } else if self.looped {
            // The very end is its own moment (the last key), not the start
            // again: a playhead parked there edits the end pose.
            t_ms as f32
        } else {
            t_ms.min(self.duration_ms) as f32
        }
    }
}

// ---------------------------------------------------------------------------
// Editing and posing

impl Rig {
    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
            && self.animations.is_empty()
            && self.attachments.is_empty()
            && self.meshes.is_empty()
            && self.rest_alpha.is_empty()
            && self.skins.is_empty()
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

    /// Whether `bone` could hang under `parent` (`None`: become a root): it
    /// can't hang under itself or anything below it.
    pub fn can_parent(&self, bone: BoneId, parent: Option<BoneId>) -> bool {
        self.bone(bone).is_some() && parent.is_none_or(|p| self.bone(p).is_some() && !self.subtree(bone).contains(&p))
    }

    /// Hang `bone` under `parent` (`None`: make it a root) without moving it
    /// in the setup pose: its position, rotation and scale are rewritten
    /// relative to the new parent. False when that would make a loop.
    pub fn set_parent(&mut self, bone: BoneId, parent: Option<BoneId>) -> bool {
        if !self.can_parent(bone, parent) {
            return false;
        }
        let Some(bi) = self.bone_index(bone) else { return false };
        if self.bones[bi].parent == parent {
            return true;
        }
        let pose = self.pose(None);
        let world = pose[bi];
        let parent_world = parent.and_then(|p| self.bone_index(p)).map_or(Xf::IDENTITY, |i| pose[i]);
        let local = parent_world.inverse().map_or(world, |inv| inv * world);
        let b = &mut self.bones[bi];
        b.parent = parent;
        (b.x, b.y) = (local.e, local.f);
        b.rotation = local.b.atan2(local.a).to_degrees();
        let sx = (local.a * local.a + local.b * local.b).sqrt();
        let det = local.a * local.d - local.b * local.c;
        b.scale = [sx, if sx > 1e-6 { det / sx } else { 1.0 }];
        // Parents come before their children in `bones` (posing relies on it).
        let mut ordered: Vec<Bone> = Vec::with_capacity(self.bones.len());
        let mut rest: Vec<Bone> = std::mem::take(&mut self.bones);
        while !rest.is_empty() {
            let before = rest.len();
            let (ready, wait): (Vec<Bone>, Vec<Bone>) =
                rest.into_iter().partition(|b| b.parent.is_none_or(|p| ordered.iter().any(|o| o.id == p)));
            ordered.extend(ready);
            rest = wait;
            if rest.len() == before {
                // Can't happen (no loops), but never drop bones.
                ordered.append(&mut rest);
            }
        }
        self.bones = ordered;
        true
    }

    /// The drive turning `bone`, if any.
    pub fn drive_of(&self, bone: BoneId) -> Option<&Drive> {
        self.drives.iter().find(|d| d.bone == bone)
    }

    /// Whether `bone` could be driven by `source`: both exist, differ, and
    /// `source` isn't (directly or through other drives) driven by `bone`.
    pub fn can_drive(&self, bone: BoneId, source: BoneId) -> bool {
        if bone == source || self.bone(bone).is_none() || self.bone(source).is_none() {
            return false;
        }
        let mut cur = source;
        for _ in 0..=self.drives.len() {
            if cur == bone {
                return false;
            }
            match self.drive_of(cur) {
                Some(d) => cur = d.source,
                None => break,
            }
        }
        true
    }

    /// Make `source` turn `bone` by `ratio` (replacing the bone's drive).
    pub fn set_drive(&mut self, bone: BoneId, source: BoneId, ratio: f32) -> bool {
        if !self.can_drive(bone, source) {
            return false;
        }
        self.drives.retain(|d| d.bone != bone);
        self.drives.push(Drive { bone, source, ratio });
        true
    }

    pub fn remove_drive(&mut self, bone: BoneId) {
        self.drives.retain(|d| d.bone != bone);
    }

    /// Degrees `bone` is turned at local time `t` of `an` by its drive (and
    /// the drives behind it), not counting its own keys.
    fn driven_turn(&self, an: &RigAnim, bone: BoneId, t: f32) -> f32 {
        let (mut total, mut factor, mut cur) = (0.0f32, 1.0f32, bone);
        for _ in 0..self.drives.len() {
            let Some(d) = self.drives.iter().find(|d| d.bone == cur) else { break };
            factor *= d.ratio;
            let keyed = an.tracks.iter().find(|tr| tr.bone == d.source).and_then(|tr| sample(&tr.rotate, t));
            total += factor * keyed.unwrap_or(0.0);
            cur = d.source;
        }
        total
    }

    /// Degrees the drive turns `bone` at `t_ms` of animation `anim`.
    pub fn drive_turn(&self, anim: usize, bone: BoneId, t_ms: u32) -> f32 {
        self.animations.get(anim).map_or(0.0, |a| self.driven_turn(a, bone, a.local_time(t_ms)))
    }

    /// Remove a bone and its children, with their keys and attachments.
    pub fn remove_bone(&mut self, id: BoneId) {
        let gone = self.subtree(id);
        self.bones.retain(|b| !gone.contains(&b.id));
        self.attachments.retain(|a| !gone.contains(&a.bone));
        for an in &mut self.animations {
            an.tracks.retain(|t| !gone.contains(&t.bone));
        }
        self.prune_mesh_bones(&gone);
        self.drives.retain(|d| !gone.contains(&d.bone) && !gone.contains(&d.source));
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
        self.meshes.retain(|m| doc_layers.contains(&m.layer));
        self.rest_alpha.retain(|(l, _)| doc_layers.contains(l));
        for s in &mut self.skins {
            s.layers.retain(|l| doc_layers.contains(l));
        }
        for a in &mut self.animations {
            a.alpha.retain(|t| doc_layers.contains(&t.layer));
        }
    }

    /// The active skin, if it still exists.
    pub fn active_skin(&self) -> Option<usize> {
        self.skin.filter(|&i| i < self.skins.len())
    }

    /// Whether the active skin lets `layer` show: always for a layer in no
    /// skin, and for every layer while no skin is active.
    pub fn skin_shows(&self, layer: LayerId) -> bool {
        let Some(active) = self.active_skin() else { return true };
        self.skins[active].layers.contains(&layer) || !self.skins.iter().any(|s| s.layers.contains(&layer))
    }

    /// Add a skin and make it the active one; returns its index. The name is
    /// made unique (the game export refers to skins by name).
    pub fn add_skin(&mut self, name: &str) -> usize {
        let name = self.unique_skin_name(name, None);
        self.skins.push(Skin { name, layers: Vec::new() });
        self.skin = Some(self.skins.len() - 1);
        self.skins.len() - 1
    }

    /// `name`, or `name 2`, `name 3`... when another skin (not `except`)
    /// already has it.
    pub fn unique_skin_name(&self, name: &str, except: Option<usize>) -> String {
        let base = if name.trim().is_empty() { "skin" } else { name.trim() };
        let taken = |n: &str| self.skins.iter().enumerate().any(|(i, s)| Some(i) != except && s.name == n);
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|n| format!("{base} {n}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
    }

    /// A copy of skin `i` (same layers), made the active one.
    pub fn duplicate_skin(&mut self, i: usize) -> Option<usize> {
        let layers = self.skins.get(i)?.layers.clone();
        let n = self.add_skin(&format!("{} copy", self.skins[i].name));
        self.skins[n].layers = layers;
        Some(n)
    }

    /// Remove skin `i`; its layers show always again unless another skin
    /// holds them. No skin is active afterwards.
    pub fn remove_skin(&mut self, i: usize) {
        if i < self.skins.len() {
            self.skins.remove(i);
            self.skin = None;
        }
    }

    /// Put `layer` in skin `i` or take it out.
    pub fn set_in_skin(&mut self, i: usize, layer: LayerId, inside: bool) {
        let Some(s) = self.skins.get_mut(i) else { return };
        s.layers.retain(|l| *l != layer);
        if inside {
            s.layers.push(layer);
        }
    }

    /// How shown `layer` is when no animation keys it.
    pub fn rest_alpha_of(&self, layer: LayerId) -> f32 {
        self.rest_alpha.iter().find(|(l, _)| *l == layer).map_or(1.0, |(_, a)| *a)
    }

    /// Set how shown `layer` is at rest (1 removes the setting).
    pub fn set_rest_alpha(&mut self, layer: LayerId, alpha: f32) {
        self.rest_alpha.retain(|(l, _)| *l != layer);
        let a = alpha.clamp(0.0, 1.0);
        if a < 0.999 {
            self.rest_alpha.push((layer, a));
        }
    }

    /// How shown `layer` is: its keys in the animation at `t_ms` when it has
    /// any, else its rest value.
    pub fn alpha_at(&self, anim: Option<(usize, u32)>, layer: LayerId) -> f32 {
        let keyed = anim.and_then(|(i, t)| {
            let a = self.animations.get(i)?;
            let tr = a.alpha.iter().find(|tr| tr.layer == layer)?;
            sample(&tr.keys, a.local_time(t))
        });
        if !self.skin_shows(layer) {
            return 0.0;
        }
        keyed.unwrap_or_else(|| self.rest_alpha_of(layer)).clamp(0.0, 1.0)
    }

    /// Whether anything about `layer` is shown differently over time or at rest.
    pub fn alpha_matters(&self, anim: usize, layer: LayerId) -> bool {
        self.rest_alpha_of(layer) < 0.999
            || self.animations.get(anim).is_some_and(|a| a.alpha.iter().any(|t| t.layer == layer))
    }

    /// Set (or add) an opacity key of `layer` at `time_ms`.
    pub fn set_alpha_key(&mut self, anim: usize, layer: LayerId, time_ms: u32, alpha: f32, curve: Curve) {
        let Some(an) = self.animations.get_mut(anim) else { return };
        let time_ms = time_ms.min(an.duration_ms);
        let ti = match an.alpha.iter().position(|t| t.layer == layer) {
            Some(i) => i,
            None => {
                an.alpha.push(AlphaTrack { layer, keys: Vec::new() });
                an.alpha.len() - 1
            }
        };
        upsert(&mut an.alpha[ti].keys, time_ms, alpha.clamp(0.0, 1.0), curve);
    }

    /// Like `set_alpha_key`, as the editor keys opacity: a layer's first key,
    /// when later than the start, also keys its rest value at the start, so
    /// the layer is as it was until then.
    pub fn key_alpha_from_rest(&mut self, anim: usize, layer: LayerId, time_ms: u32, alpha: f32, curve: Curve) {
        let unkeyed =
            self.animations.get(anim).is_some_and(|a| !a.alpha.iter().any(|t| t.layer == layer && !t.keys.is_empty()));
        if unkeyed && time_ms > 0 {
            let rest = self.rest_alpha_of(layer);
            self.set_alpha_key(anim, layer, 0, rest, curve);
        }
        self.set_alpha_key(anim, layer, time_ms, alpha, curve);
    }

    /// Show (`alpha` 1) or hide (0) `layer` from `time_ms` on, at once: the
    /// key before it holds its value until then instead of fading.
    pub fn switch_alpha(&mut self, anim: usize, layer: LayerId, time_ms: u32, alpha: f32) {
        self.key_alpha_from_rest(anim, layer, time_ms, alpha, Curve::Stepped);
        let Some(an) = self.animations.get_mut(anim) else { return };
        let time_ms = time_ms.min(an.duration_ms);
        let Some(tr) = an.alpha.iter_mut().find(|t| t.layer == layer) else { return };
        let at = tr.keys.partition_point(|k| k.time_ms < time_ms);
        if let Some(k) = tr.keys.get_mut(at) {
            k.curve = Curve::Stepped;
        }
        if let Some(prev) = at.checked_sub(1).and_then(|i| tr.keys.get_mut(i)) {
            prev.curve = Curve::Stepped;
        }
    }

    /// The times of `layer`'s opacity keys in animation `anim`.
    pub fn alpha_key_times(&self, anim: usize, layer: LayerId) -> Vec<u32> {
        self.animations
            .get(anim)
            .and_then(|a| a.alpha.iter().find(|t| t.layer == layer))
            .map(|t| t.keys.iter().map(|k| k.time_ms).collect())
            .unwrap_or_default()
    }

    pub fn remove_alpha_keys_at(&mut self, anim: usize, layer: LayerId, time_ms: u32) {
        let Some(an) = self.animations.get_mut(anim) else { return };
        if let Some(t) = an.alpha.iter_mut().find(|t| t.layer == layer) {
            t.keys.retain(|k| k.time_ms != time_ms);
        }
        an.alpha.retain(|t| !t.keys.is_empty());
    }

    pub fn move_alpha_keys(&mut self, anim: usize, layer: LayerId, from_ms: u32, to_ms: u32) {
        let Some(an) = self.animations.get_mut(anim) else { return };
        let to_ms = to_ms.min(an.duration_ms);
        if from_ms == to_ms {
            return;
        }
        let Some(t) = an.alpha.iter_mut().find(|t| t.layer == layer) else { return };
        if let Some(i) = t.keys.iter().position(|k| k.time_ms == from_ms) {
            let mut k = t.keys.remove(i);
            t.keys.retain(|o| o.time_ms != to_ms);
            k.time_ms = to_ms;
            let at = t.keys.partition_point(|o| o.time_ms < to_ms);
            t.keys.insert(at, k);
        }
    }

    pub fn add_animation(&mut self, name: impl Into<String>, duration_ms: u32) -> usize {
        self.animations.push(RigAnim {
            name: name.into(),
            duration_ms: duration_ms.max(1),
            looped: true,
            ik: Vec::new(),
            alpha: Vec::new(),
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

    /// Like [`Rig::set_key`], for a value typed or dragged by hand: the first
    /// key of a property at a later time also keys its rest value at 0, so
    /// the animation starts from the setup pose.
    pub fn key_from_rest(&mut self, anim: usize, bone: BoneId, time_ms: u32, value: KeyValue, curve: Curve) {
        if time_ms > 0 {
            let (has, rest) = match value {
                KeyValue::Translate(_) => {
                    (self.animated_property(anim, bone, |t| !t.translate.is_empty()), KeyValue::Translate([0.0, 0.0]))
                }
                KeyValue::Rotate(_) => {
                    (self.animated_property(anim, bone, |t| !t.rotate.is_empty()), KeyValue::Rotate(0.0))
                }
                KeyValue::Scale(_) => {
                    (self.animated_property(anim, bone, |t| !t.scale.is_empty()), KeyValue::Scale([1.0, 1.0]))
                }
            };
            if !has {
                self.set_key(anim, bone, 0, rest, Curve::Linear);
            }
        }
        self.set_key(anim, bone, time_ms, value, curve);
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

    /// What `bone` is keyed to at `t_ms` of animation `anim`: its move, turn
    /// and scale there, each only when the bone has keys of that kind.
    pub fn keyed_pose(&self, anim: usize, bone: BoneId, t_ms: u32) -> [Option<KeyValue>; 3] {
        let Some(a) = self.animations.get(anim) else { return [None; 3] };
        let Some(tr) = a.tracks.iter().find(|t| t.bone == bone) else { return [None; 3] };
        let t = a.local_time(t_ms);
        [
            sample(&tr.translate, t).map(KeyValue::Translate),
            sample(&tr.rotate, t).map(KeyValue::Rotate),
            sample(&tr.scale, t).map(KeyValue::Scale),
        ]
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

    /// Whether `bone` already has keys of some kind in animation `anim`
    /// (`has` looks at its track).
    fn animated_property(&self, anim: usize, bone: BoneId, has: impl Fn(&BoneTrack) -> bool) -> bool {
        self.animations.get(anim).and_then(|a| a.tracks.iter().find(|t| t.bone == bone)).is_some_and(has)
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
                        // The first key of a property at t > 0 also keys its rest
                        // value at 0, so the animation starts from the setup pose.
                        if t_ms > 0 && !self.animated_property(a, bone, |t| !t.translate.is_empty()) {
                            self.set_key(a, bone, 0, KeyValue::Translate([0.0, 0.0]), Curve::Linear);
                        }
                        self.set_key(a, bone, t_ms, KeyValue::Translate(off), curve);
                    }
                }
            }
            BoneDrag::Scale => {
                let Some(a) = anim else { return };
                let parent_scale = (parent.a * parent.a + parent.b * parent.b).sqrt().max(1e-3);
                let reach = (b.length.max(1.0) * parent_scale * b.scale[0].abs().max(1e-3)).max(1e-3);
                let s = (head_world.dist(target) / reach).clamp(0.02, 8.0);
                if t_ms > 0 && !self.animated_property(a, bone, |t| !t.scale.is_empty()) {
                    self.set_key(a, bone, 0, KeyValue::Scale([1.0, 1.0]), Curve::Linear);
                }
                self.set_key(a, bone, t_ms, KeyValue::Scale([s, s]), curve);
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
                        bm.rotation = wrap180(local);
                        if drag == BoneDrag::AimStretch {
                            bm.length = (dx * dx + dy * dy).sqrt().max(1.0);
                        }
                    }
                    Some(a) => {
                        let cur = self.offsets(a, bone, t_ms).1;
                        // The key holds only this bone's own turn, not what its drive adds.
                        let want = local - b.rotation - self.drive_turn(a, bone, t_ms);
                        // Take the turn the short way from where it is now.
                        let off = cur + (want - cur + 540.0).rem_euclid(360.0) - 180.0;
                        if t_ms > 0 && !self.animated_property(a, bone, |t| !t.rotate.is_empty()) {
                            self.set_key(a, bone, 0, KeyValue::Rotate(0.0), Curve::Linear);
                        }
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
        self.worlds(&self.locals(anim))
    }

    /// The pose while crossfading from one animation to another: `weight` 0
    /// is all `from`, 1 all `to` (each an animation and a time, or `None`
    /// for the setup pose). Every bone's local position and scale blend in a
    /// straight line and its rotation the short way round, after each side's
    /// own IK, which is how a game runtime mixes two animations.
    pub fn pose_mix(&self, from: Option<(usize, u32)>, to: Option<(usize, u32)>, weight: f32) -> Vec<Xf> {
        let w = weight.clamp(0.0, 1.0);
        if w >= 1.0 {
            return self.pose(to);
        }
        let mut locals = self.locals(from);
        for (l, o) in locals.iter_mut().zip(self.locals(to)) {
            l.x += (o.x - l.x) * w;
            l.y += (o.y - l.y) * w;
            l.rot += wrap180(o.rot - l.rot) * w;
            l.sx += (o.sx - l.sx) * w;
            l.sy += (o.sy - l.sy) * w;
        }
        self.worlds(&locals)
    }

    /// How shown `layer` is during the same crossfade.
    pub fn alpha_mix(&self, from: Option<(usize, u32)>, to: Option<(usize, u32)>, weight: f32, layer: LayerId) -> f32 {
        let (a, b) = (self.alpha_at(from, layer), self.alpha_at(to, layer));
        a + (b - a) * weight.clamp(0.0, 1.0)
    }

    /// How long a crossfade between two animations takes (ms; 0 switches at
    /// once). A default for the preview and for game runtimes.
    pub fn mix_ms(&self) -> u32 {
        self.mix_ms.unwrap_or(200)
    }

    pub fn set_mix_ms(&mut self, ms: u32) {
        self.mix_ms = Some(ms);
    }

    /// Every bone's local transform in `anim` (after drives and IK).
    fn locals(&self, anim: Option<(usize, u32)>) -> Vec<Local> {
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
                    if !self.drives.is_empty() {
                        l.rot += self.driven_turn(a, b.id, t);
                    }
                }
                l
            })
            .collect();
        if let Some((a, t)) = an {
            self.solve_ik(&mut locals, Some((a, t)));
        }
        locals
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

    /// Move every key of `bone` at `from_ms` to `to_ms` (clamped to the
    /// animation), replacing any of the same kind already there.
    pub fn move_keys(&mut self, anim: usize, bone: BoneId, from_ms: u32, to_ms: u32) {
        let Some(an) = self.animations.get_mut(anim) else { return };
        let to_ms = to_ms.min(an.duration_ms);
        if from_ms == to_ms {
            return;
        }
        let Some(t) = an.tracks.iter_mut().find(|t| t.bone == bone) else { return };
        fn shift<T: Clone>(keys: &mut Vec<Key<T>>, from: u32, to: u32) {
            if let Some(i) = keys.iter().position(|k| k.time_ms == from) {
                let mut k = keys.remove(i);
                keys.retain(|o| o.time_ms != to);
                k.time_ms = to;
                let at = keys.partition_point(|o| o.time_ms < to);
                keys.insert(at, k);
            }
        }
        shift(&mut t.translate, from_ms, to_ms);
        shift(&mut t.rotate, from_ms, to_ms);
        shift(&mut t.scale, from_ms, to_ms);
    }

    /// Move the IK keys of constraint `id` from `from_ms` to `to_ms`.
    pub fn move_ik_keys(&mut self, anim: usize, id: u32, from_ms: u32, to_ms: u32) {
        let Some(an) = self.animations.get_mut(anim) else { return };
        let to_ms = to_ms.min(an.duration_ms);
        if from_ms == to_ms {
            return;
        }
        let Some(t) = an.ik.iter_mut().find(|t| t.constraint == id) else { return };
        if let Some(i) = t.keys.iter().position(|k| k.time_ms == from_ms) {
            let mut k = t.keys.remove(i);
            t.keys.retain(|o| o.time_ms != to_ms);
            k.time_ms = to_ms;
            let at = t.keys.partition_point(|o| o.time_ms < to_ms);
            t.keys.insert(at, k);
        }
    }

    /// Key every animated property of animation `anim` at `time_ms` with the
    /// value it has there now, so each bone holds its pose from that moment.
    /// Timelines that already have a key at `time_ms` are left alone. Returns
    /// how many keys it added.
    pub fn key_all(&mut self, anim: usize, time_ms: u32) -> usize {
        let Some(an) = self.animations.get_mut(anim) else { return 0 };
        let t = time_ms.min(an.duration_ms);
        fn hold<T: Lerp>(keys: &mut Vec<Key<T>>, t: u32) -> usize {
            if keys.is_empty() || keys.iter().any(|k| k.time_ms == t) {
                return 0;
            }
            match sample(keys, t as f32) {
                Some(v) => {
                    upsert(keys, t, v, Curve::Linear);
                    1
                }
                None => 0,
            }
        }
        let mut added = 0;
        for tr in &mut an.tracks {
            added += hold(&mut tr.translate, t) + hold(&mut tr.rotate, t) + hold(&mut tr.scale, t);
        }
        for tr in &mut an.ik {
            added += hold(&mut tr.keys, t);
        }
        for tr in &mut an.alpha {
            added += hold(&mut tr.keys, t);
        }
        added
    }

    /// Make animation `anim` loop seamlessly: every timeline that doesn't
    /// already have a key at the very end gets one copying its first key, so
    /// the last pose runs back into the first. Returns how many keys it added.
    pub fn close_loop(&mut self, anim: usize) -> usize {
        let Some(an) = self.animations.get_mut(anim) else { return 0 };
        let end = an.duration_ms;
        fn close<T: Clone>(keys: &mut Vec<Key<T>>, end: u32) -> usize {
            match keys.first().cloned() {
                Some(first) if keys.last().is_some_and(|k| k.time_ms < end) => {
                    keys.push(Key { time_ms: end, value: first.value, curve: first.curve });
                    1
                }
                _ => 0,
            }
        }
        let mut added = 0;
        for t in &mut an.tracks {
            added += close(&mut t.translate, end) + close(&mut t.rotate, end) + close(&mut t.scale, end);
        }
        for t in &mut an.ik {
            added += close(&mut t.keys, end);
        }
        for t in &mut an.alpha {
            added += close(&mut t.keys, end);
        }
        added
    }

    /// Remove the IK keys of constraint `id` at `time_ms`.
    pub fn remove_ik_keys_at(&mut self, anim: usize, id: u32, time_ms: u32) {
        let Some(an) = self.animations.get_mut(anim) else { return };
        if let Some(t) = an.ik.iter_mut().find(|t| t.constraint == id) {
            t.keys.retain(|k| k.time_ms != time_ms);
        }
        an.ik.retain(|t| !t.keys.is_empty());
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

/// How a pose changes one layer's picture.
enum PoseKind<'a> {
    /// Moved as a whole by its bone.
    Rigid(Xf),
    /// Bent through its weighted mesh, vertices moved to these points.
    Skin(&'a crate::rigmesh::PartMesh, Vec<Pt>),
}

/// What `pose` does to `layer`'s picture; `None` when it stays as drawn.
/// A mesh takes precedence over the layer's single bone.
fn pose_kind<'a>(rig: &'a Rig, layer: LayerId, pose: &[Xf], setup: &[Xf]) -> Option<PoseKind<'a>> {
    if let Some(m) = rig.mesh_of(layer) {
        let pts = rig.skin_points(m, pose, setup);
        let rest = m.setup_points();
        if pts.iter().zip(&rest).all(|(a, b)| a.dist(*b) < 1e-3) {
            return None;
        }
        return Some(PoseKind::Skin(m, pts));
    }
    let delta = rig.layer_delta(rig.attached_bone(layer)?, pose, setup)?;
    (!delta.is_identity()).then_some(PoseKind::Rigid(delta))
}

fn pose_picture(raster: &Raster, kind: &PoseKind, width: u32, height: u32, filter: ResizeFilter) -> Raster {
    match kind {
        PoseKind::Rigid(xf) => transform_raster(raster, *xf, width, height, filter),
        PoseKind::Skin(mesh, pts) => crate::rigmesh::render_mesh(raster, mesh, pts, width, height, filter),
    }
}

/// A copy of `doc` with the pose of `anim` at `t_ms` applied to every
/// attached layer (their masks baked in, since a mask can't follow a bone).
pub fn posed_copy(doc: &DocState, anim: Option<usize>, t_ms: u32, filter: ResizeFilter) -> DocState {
    let at = anim.map(|a| (a, t_ms));
    posed_copy_mix(doc, at, at, 1.0, filter)
}

/// Like `posed_copy`, for a crossfade between two animations (see
/// `Rig::pose_mix`).
pub fn posed_copy_mix(
    doc: &DocState,
    from: Option<(usize, u32)>,
    to: Option<(usize, u32)>,
    weight: f32,
    filter: ResizeFilter,
) -> DocState {
    let mut out = doc.clone();
    let rig = &doc.rig;
    if rig.attachments.is_empty()
        && rig.meshes.is_empty()
        && rig.rest_alpha.is_empty()
        && rig.active_skin().is_none()
        && rig.animations.iter().all(|a| a.alpha.is_empty())
    {
        return out;
    }
    let setup = doc.rig.pose(None);
    let mixing = from != to && weight < 1.0;
    let pose = if mixing { rig.pose_mix(from, to, weight) } else { rig.pose(to) };
    let (w, h) = (doc.width, doc.height);
    for l in &mut out.layers {
        let alpha = if mixing { rig.alpha_mix(from, to, weight, l.id()) } else { rig.alpha_at(to, l.id()) };
        if alpha < 0.999 {
            l.props.opacity *= alpha;
        }
        if !l.owns_pixels() {
            continue;
        }
        let Some(kind) = pose_kind(&doc.rig, l.id(), &pose, &setup) else { continue };
        l.apply_mask();
        l.raster = pose_picture(&l.raster, &kind, w, h, filter);
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

    // Each frame's time and pose, computed once for all layers.
    let times: Vec<u32> = (0..n).map(|i| (i as f32 * step).round() as u32).collect();
    let poses: Vec<Vec<Xf>> = times.iter().map(|&t| doc.rig.pose(Some((anim, t)))).collect();
    // A document that already has frames plays them alongside the rig: the
    // art of baked frame `i` is the frame of the document showing at its time
    // (wrapping for a looping animation). With one frame this is always 0.
    // The frames always cycle, even under an animation that plays once: a
    // short hand-drawn flicker should keep going for a whole fire breath.
    let src_frame: Vec<usize> = times.iter().map(|&t| frame_showing(doc, t, true)).collect();
    let current = out.frame;

    for l in &mut out.layers {
        let id = l.id();
        // A layer gets a cel per frame when its bones move it, or when it is
        // shown differently at rest or over time (a flame that comes and goes).
        let rigged =
            (doc.rig.attached_bone(id).is_some() || doc.rig.mesh_of(id).is_some() || doc.rig.alpha_matters(anim, id))
                && l.owns_pixels();
        if !rigged && !l.animated() {
            l.cels.clear();
            continue;
        }
        // The picture this layer has in each source frame, masks applied
        // (a mask belongs to the layer, not to one frame's picture).
        let pictures: Vec<(usize, Raster, f32)> = src_frame
            .iter()
            .map(|&f| {
                let pic = l.cel_image(f, current).cloned().unwrap_or_else(|| Raster::new(w, h));
                let pic = match l.active_mask() {
                    Some(_) => {
                        let mut tmp = l.clone();
                        tmp.raster = pic;
                        tmp.masked_raster()
                    }
                    None => pic,
                };
                (l.cel_owner(f), pic, l.cel_opacity(f))
            })
            .collect();
        l.mask = None;
        l.props.mask_enabled = true;
        if rigged {
            // Shape, text and smart layers become plain pixels: their
            // frames can no longer be redrawn from points.
            l.props.kind = LayerKind::Raster;
            l.props.shape = None;
            l.props.text = None;
            l.props.smart = None;
            l.smart = None;
            l.cels = pictures
                .iter()
                .zip(poses.iter().zip(&times))
                .map(|((_, base, cel_opacity), (pose, &t))| {
                    let mut cel = match pose_kind(&doc.rig, id, pose, &setup) {
                        Some(kind) => Cel::own(pose_picture(base, &kind, w, h, filter)),
                        None => Cel::own(base.clone()),
                    };
                    cel.opacity = doc.rig.alpha_at(Some((anim, t)), id) * cel_opacity;
                    cel
                })
                .collect();
        } else {
            // Not moved by the rig: each baked frame shows the source frame's
            // picture, linked to the first baked frame with the same one.
            let mut first_with: Vec<(usize, usize)> = Vec::new();
            l.cels = pictures
                .into_iter()
                .enumerate()
                .map(|(i, (owner, pic, opacity))| match first_with.iter().find(|(o, _)| *o == owner) {
                    Some(&(_, at)) => Cel::linked(at, w, h),
                    None => {
                        first_with.push((owner, i));
                        let mut cel = Cel::own(pic);
                        cel.opacity = opacity;
                        cel
                    }
                })
                .collect();
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

/// The frame of `doc` showing `t_ms` into its animation: its frames laid end
/// to end by duration, wrapping when `looped` and holding the last otherwise.
pub fn frame_showing(doc: &DocState, t_ms: u32, looped: bool) -> usize {
    let n = doc.frame_count();
    let total = doc.total_duration_ms().max(1);
    if n <= 1 {
        return 0;
    }
    let mut t = if looped { t_ms as u64 % total } else { (t_ms as u64).min(total - 1) };
    for f in 0..n {
        let d = doc.frame_duration(f) as u64;
        if t < d {
            return f;
        }
        t -= d;
    }
    n - 1
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
        // The first move key at 500 ms also keyed the rest position at 0, so
        // the bone starts where it was set up and glides over.
        assert!(near(rig.offsets(a, b, 0).0[0], 0.0) && near(rig.offsets(a, b, 250).0[0], 10.0));
        assert_eq!(rig.key_times(a, b), vec![0, 500]);
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
    fn a_meshed_layer_bends_in_the_render_while_a_rigid_one_slides() {
        use crate::geom::IRect;
        let mut doc = DocState::new(64, 48, None);
        let id = doc.layers[0].id();
        doc.layers[0].raster.fill_rect(IRect::new(0, 16, 40, 10), Rgba8::new(255, 0, 0, 255));
        let a = doc.rig.add_bone("a", None, 0.0, 21.0, 20.0);
        let b = doc.rig.add_bone("b", Some(a), 20.0, 0.0, 20.0);
        doc.rig.attach(id, a);
        assert!(doc.rig.add_mesh(id, IRect::new(0, 16, 40, 10), 5));
        doc.rig.auto_weights(id, true);
        let an = doc.rig.add_animation("bend", 1000);
        doc.rig.set_key(an, b, 0, KeyValue::Rotate(70.0), Curve::Linear);

        let bent = render_pose(&doc, Some(an), 0, ResizeFilter::Bilinear);
        let rest = render_pose(&doc, None, 0, ResizeFilter::Bilinear);
        let (bb, rb) = (bent.bounds().unwrap(), rest.bounds().unwrap());
        assert_eq!((rb.x, rb.y, rb.w, rb.h), (0, 16, 40, 10), "setup pose draws the layer as drawn");
        // The right end swung down below the strip while the left end stayed.
        assert!(bb.bottom() > rb.bottom() + 8, "{bb:?}");
        assert!(bent.get_pixel(2, 21).a > 200, "left end stays put");
        assert!(bent.get_pixel(38, 21).a < 50, "right end moved away from where it was");

        // Baking goes through the same mesh: its frame 0 (this animation holds
        // its key from t=0) has the bent render's extent.
        let baked = bake(&doc, an, 4, ResizeFilter::Bilinear).unwrap();
        let f0 = crate::anim::render_frame(&baked, 0);
        assert_eq!(f0.bounds().map(|r| (r.x, r.y, r.w, r.h)), Some((bb.x, bb.y, bb.w, bb.h)));
    }

    #[test]
    fn a_full_rig_survives_a_qsk_save_and_load() {
        use crate::geom::IRect;
        let mut doc = DocState::new(64, 64, None);
        let id = doc.layers[0].id();
        doc.layers[0].raster.fill_rect(IRect::new(10, 10, 20, 30), Rgba8::new(10, 200, 30, 255));
        let a = doc.rig.add_bone("upper", None, 20.0, 12.0, 14.0);
        let b = doc.rig.add_bone("lower", Some(a), 14.0, 0.0, 14.0);
        doc.rig.attach(id, a);
        assert!(doc.rig.add_mesh(id, IRect::new(10, 10, 20, 30), 8));
        doc.rig.auto_weights(id, false);
        let (ik, target) = doc.rig.add_ik(b).unwrap();
        let an = doc.rig.add_animation("reach", 900);
        doc.rig.set_key(an, target, 0, KeyValue::Translate([0.0, 0.0]), Curve::Linear);
        doc.rig.set_key(an, target, 900, KeyValue::Translate([-6.0, 9.0]), Curve::Bezier([0.2, 0.0, 0.8, 1.0]));
        doc.rig.set_ik_key(an, ik, 400, crate::rig::IkValue { mix: 0.5, bend: -1 }, Curve::Stepped);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rig.qsk");
        crate::io::qsk::save(&path, &doc).unwrap();
        let back = crate::io::qsk::load(&path).unwrap();
        assert_eq!(back.rig, doc.rig);
        // And it still poses the same.
        assert_eq!(back.rig.pose(Some((an, 450))), doc.rig.pose(Some((an, 450))));
    }

    #[test]
    fn scale_drag_keys_a_factor_and_close_loop_repeats_the_first_keys() {
        let mut rig = Rig::default();
        let b = rig.add_bone("fire", None, 0.0, 0.0, 10.0);
        let a = rig.add_animation("a", 1000);
        // Dragging the tip to twice the setup length keys scale 2 (and the rest
        // scale 1 at 0 so it grows from its setup size).
        rig.drag_bone(Some(a), 400, b, BoneDrag::Scale, Pt::new(20.0, 0.0), Curve::Linear);
        assert!(near(rig.offsets(a, b, 400).2[0], 2.0) && near(rig.offsets(a, b, 0).2[0], 1.0));
        assert!(near(rig.offsets(a, b, 200).2[0], 1.5));
        // Not in the setup pose.
        rig.drag_bone(None, 0, b, BoneDrag::Scale, Pt::new(40.0, 0.0), Curve::Linear);
        assert!(near(rig.bone(b).unwrap().scale[0], 1.0));
        // The loop runs out at 400 and holds; closing it returns to the start.
        assert_eq!(rig.close_loop(a), 1);
        assert!(near(rig.offsets(a, b, 1000).2[0], 1.0));
        assert_eq!(rig.close_loop(a), 0, "already closed");
    }

    #[test]
    fn key_all_holds_every_animated_property_where_it_is() {
        let mut rig = Rig::default();
        let a_bone = rig.add_bone("a", None, 0.0, 0.0, 10.0);
        let b_bone = rig.add_bone("b", None, 5.0, 0.0, 10.0);
        let an = rig.add_animation("hold", 1000);
        rig.set_key(an, a_bone, 0, KeyValue::Rotate(0.0), Curve::Linear);
        rig.set_key(an, a_bone, 400, KeyValue::Rotate(40.0), Curve::Linear);
        rig.set_key(an, b_bone, 0, KeyValue::Translate([0.0, 0.0]), Curve::Linear);
        rig.set_key(an, b_bone, 200, KeyValue::Translate([10.0, 0.0]), Curve::Linear);
        // At 700 ms both bones are past their last key: holding it adds keys
        // with those values, so later keys can bring them back from there.
        assert_eq!(rig.key_all(an, 700), 2);
        assert!(near(rig.offsets(an, a_bone, 700).1, 40.0) && near(rig.offsets(an, b_bone, 700).0[0], 10.0));
        rig.set_key(an, a_bone, 1000, KeyValue::Rotate(0.0), Curve::Linear);
        assert!(near(rig.offsets(an, a_bone, 700).1, 40.0), "held until 700");
        assert!(near(rig.offsets(an, a_bone, 850).1, 20.0), "then back to rest by 1000");
        // Between keys it keys the interpolated value; existing keys are untouched.
        assert_eq!(rig.key_all(an, 100), 2);
        assert!(near(rig.offsets(an, a_bone, 100).1, 10.0));
        assert_eq!(rig.key_all(an, 400), 1, "a already has a key at 400");
    }

    #[test]
    fn bake_plays_the_documents_frames_alongside_the_rig() {
        use crate::geom::IRect;
        use crate::NewFrame;
        // A 3-frame document: a layer whose square is a different color in
        // each frame (100 ms each), plus a bone moving it right.
        let mut doc = DocState::new(64, 16, None);
        let id = doc.layers[0].id();
        let colors = [Rgba8::new(255, 0, 0, 255), Rgba8::new(0, 255, 0, 255), Rgba8::new(0, 0, 255, 255)];
        doc.layers[0].raster.fill_rect(IRect::new(2, 2, 6, 6), colors[0]);
        doc.insert_frame(1, NewFrame::Duplicate(0));
        doc.layers[0].raster.fill_rect(IRect::new(2, 2, 6, 6), colors[1]);
        doc.insert_frame(2, NewFrame::Duplicate(1));
        doc.layers[0].raster.fill_rect(IRect::new(2, 2, 6, 6), colors[2]);
        for f in 0..3 {
            doc.set_frame_duration(f, 100);
        }
        doc.set_frame(1); // the "current" frame must not decide the art
        let bone = doc.rig.add_bone("b", None, 0.0, 0.0, 10.0);
        doc.rig.attach(id, bone);
        let an = doc.rig.add_animation("slide", 300);
        doc.rig.set_key(an, bone, 0, KeyValue::Translate([0.0, 0.0]), Curve::Linear);
        doc.rig.set_key(an, bone, 300, KeyValue::Translate([30.0, 0.0]), Curve::Linear);

        let baked = bake(&doc, an, 10, ResizeFilter::Nearest).unwrap();
        assert_eq!(baked.frames.len(), 3, "300 ms looped at 10 fps");
        for (f, want) in colors.iter().enumerate() {
            let img = crate::anim::render_frame(&baked, f);
            let b = img.bounds().unwrap();
            // Each baked frame shows its own source frame's color, further right.
            assert_eq!(b.x, 2 + 10 * f as i32, "frame {f} moved");
            assert_eq!(img.get_pixel(b.x + 2, 4), *want, "frame {f} drew source frame {f}");
        }
        assert_eq!(frame_showing(&doc, 0, true), 0);
        assert_eq!(frame_showing(&doc, 250, true), 2);
        assert_eq!(frame_showing(&doc, 310, true), 0, "wraps");
        assert_eq!(frame_showing(&doc, 999, false), 2, "holds the last");
    }

    #[test]
    fn a_layer_can_be_hidden_at_rest_and_shown_by_keys() {
        let mut rig = Rig::default();
        let an = rig.add_animation("breathe fire", 1000);
        let other = rig.add_animation("idle", 1000);
        rig.set_rest_alpha(9, 0.0);
        // Hidden at rest and in an animation that doesn't key it.
        assert!(near(rig.alpha_at(None, 9), 0.0) && near(rig.alpha_at(Some((other, 300)), 9), 0.0));
        // Stepped keys switch it on and off; a linear pair fades it.
        rig.set_alpha_key(an, 9, 0, 0.0, Curve::Stepped);
        rig.set_alpha_key(an, 9, 300, 1.0, Curve::Linear);
        rig.set_alpha_key(an, 9, 700, 1.0, Curve::Linear);
        rig.set_alpha_key(an, 9, 1000, 0.0, Curve::Linear);
        assert!(near(rig.alpha_at(Some((an, 100)), 9), 0.0), "stepped: still off before 300");
        assert!(near(rig.alpha_at(Some((an, 300)), 9), 1.0));
        assert!(near(rig.alpha_at(Some((an, 850)), 9), 0.5), "fading out");
        assert!(near(rig.alpha_at(Some((other, 300)), 9), 0.0), "other animations unaffected");
        assert_eq!(rig.alpha_key_times(an, 9), vec![0, 300, 700, 1000]);
        rig.move_alpha_keys(an, 9, 300, 400);
        assert_eq!(rig.alpha_key_times(an, 9), vec![0, 400, 700, 1000]);
        rig.remove_alpha_keys_at(an, 9, 0);
        assert_eq!(rig.alpha_key_times(an, 9), vec![400, 700, 1000]);
        // Showing it at rest again drops the setting; deleted layers drop theirs.
        rig.set_rest_alpha(9, 1.0);
        assert!(rig.rest_alpha.is_empty());
        rig.prune(&[1, 2]);
        assert!(rig.animations[an].alpha.is_empty());
    }

    #[test]
    fn hidden_layers_do_not_render_and_bake_into_cel_opacity() {
        use crate::geom::IRect;
        let mut doc = DocState::new(32, 32, None);
        let id = doc.layers[0].id();
        doc.layers[0].raster.fill_rect(IRect::new(4, 4, 8, 8), Rgba8::new(255, 0, 0, 255));
        let an = doc.rig.add_animation("pulse", 1000);
        doc.rig.set_rest_alpha(id, 0.0);
        doc.rig.set_alpha_key(an, id, 0, 0.0, Curve::Linear);
        doc.rig.set_alpha_key(an, id, 500, 1.0, Curve::Linear);
        doc.rig.set_alpha_key(an, id, 1000, 0.0, Curve::Linear);

        let rest = render_pose(&doc, None, 0, ResizeFilter::Nearest);
        let mid = render_pose(&doc, Some(an), 500, ResizeFilter::Nearest);
        let quarter = render_pose(&doc, Some(an), 250, ResizeFilter::Nearest);
        assert_eq!(rest.get_pixel(6, 6).a, 0, "hidden at rest");
        assert_eq!(mid.get_pixel(6, 6).a, 255);
        assert!((quarter.get_pixel(6, 6).a as i32 - 128).abs() <= 2, "half faded");

        let baked = bake(&doc, an, 4, ResizeFilter::Nearest).unwrap();
        let alpha_of = |f: usize| crate::anim::render_frame(&baked, f).get_pixel(6, 6).a as i32;
        assert_eq!(alpha_of(0), 0);
        assert_eq!(alpha_of(2), 255, "frame 2 is at 500 ms");
        assert!((alpha_of(1) - 128).abs() <= 2);
    }

    #[test]
    fn keys_can_be_moved_along_the_timeline() {
        let mut rig = Rig::default();
        let b = rig.add_bone("b", None, 0.0, 0.0, 10.0);
        let a = rig.add_animation("a", 1000);
        rig.set_key(a, b, 100, KeyValue::Rotate(10.0), Curve::Linear);
        rig.set_key(a, b, 100, KeyValue::Translate([1.0, 2.0]), Curve::Stepped);
        rig.set_key(a, b, 400, KeyValue::Rotate(40.0), Curve::Linear);
        rig.move_keys(a, b, 100, 250);
        assert_eq!(rig.key_times(a, b), vec![250, 400]);
        // Both kinds moved together and kept their easing.
        assert!(near(rig.offsets(a, b, 250).1, 10.0) && near(rig.offsets(a, b, 250).0[1], 2.0));
        assert_eq!(rig.animations[a].tracks[0].translate[0].curve, Curve::Stepped);
        // Dropping onto another key replaces it; past the end clamps to the end.
        rig.move_keys(a, b, 250, 400);
        assert_eq!(rig.key_times(a, b), vec![400]);
        assert!(near(rig.offsets(a, b, 400).1, 10.0));
        rig.move_keys(a, b, 400, 5000);
        assert_eq!(rig.key_times(a, b), vec![1000]);
        // IK keys move too.
        let c = rig.add_bone("c", Some(b), 10.0, 0.0, 10.0);
        let (ik, _) = rig.add_ik(c).unwrap();
        rig.set_ik_key(a, ik, 200, IkValue { mix: 0.5, bend: 1 }, Curve::Linear);
        rig.move_ik_keys(a, ik, 200, 600);
        assert!(near(rig.ik_at(a, ik, 600).unwrap().mix, 0.5));
        assert_eq!(rig.animations[a].ik[0].keys.len(), 1);
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
    #[test]
    fn show_and_hide_switch_at_once() {
        let mut rig = Rig::default();
        let an = rig.add_animation("blink", 2000);
        // An eye that closes at 900 and opens at 1050, keyed with fading
        // easing selected: it must still switch, and be open before 900.
        rig.switch_alpha(an, 5, 900, 0.0);
        rig.switch_alpha(an, 5, 1050, 1.0);
        let at = |rig: &Rig, t| rig.alpha_at(Some((an, t)), 5);
        assert_eq!([0, 899, 900, 1049, 1050, 1500].map(|t| at(&rig, t)), [1.0, 1.0, 0.0, 0.0, 1.0, 1.0]);
        // A fade key after a switch still fades from it.
        rig.set_alpha_key(an, 5, 1250, 0.0, Curve::Linear);
        rig.animations[an].alpha[0].keys[2].curve = Curve::Linear;
        assert!(near(at(&rig, 1150), 0.5));
        // A layer hidden at rest stays hidden until its first key.
        rig.set_rest_alpha(6, 0.0);
        rig.switch_alpha(an, 6, 900, 1.0);
        assert_eq!([rig.alpha_at(Some((an, 100)), 6), rig.alpha_at(Some((an, 950)), 6)], [0.0, 1.0]);
    }

    #[test]
    fn a_crossfade_blends_two_animations() {
        let mut rig = Rig::default();
        let b = rig.add_bone("arm", None, 10.0, 0.0, 20.0);
        let (up, down) = (rig.add_animation("up", 1000), rig.add_animation("down", 1000));
        rig.set_key(up, b, 0, KeyValue::Rotate(170.0), Curve::Linear);
        rig.set_key(up, b, 0, KeyValue::Translate([10.0, 0.0]), Curve::Linear);
        rig.set_key(down, b, 0, KeyValue::Rotate(-170.0), Curve::Linear);
        let angle = |m: Xf| m.b.atan2(m.a).to_degrees();
        // The ends are the animations themselves.
        assert_eq!(rig.pose_mix(Some((up, 0)), Some((down, 0)), 0.0), rig.pose(Some((up, 0))));
        assert_eq!(rig.pose_mix(Some((up, 0)), Some((down, 0)), 1.0), rig.pose(Some((down, 0))));
        // Halfway: the short way round (through 180, not through 0), and
        // half of the move.
        let mid = rig.pose_mix(Some((up, 0)), Some((down, 0)), 0.5)[0];
        assert!(near(angle(mid).abs(), 180.0), "{}", angle(mid));
        assert!(near(mid.e, 15.0));
        // From the setup pose too.
        let m = rig.pose_mix(None, Some((up, 0)), 0.5)[0];
        assert!(near(angle(m), 85.0) && near(m.e, 15.0));
        // Opacity fades across.
        rig.set_alpha_key(down, 7, 0, 0.0, Curve::Linear);
        assert!(near(rig.alpha_mix(Some((up, 0)), Some((down, 0)), 0.25, 7), 0.75));
        assert_eq!(rig.mix_ms(), 200);
    }

    #[test]
    fn skins_show_their_own_layers() {
        let mut rig = Rig::default();
        let knight = rig.add_skin("knight");
        let wizard = rig.add_skin("knight");
        assert_eq!(rig.skins[wizard].name, "knight 2");
        // Layer 1 is the knight's, 2 the wizard's, 3 both, 4 in no skin.
        rig.set_in_skin(knight, 1, true);
        rig.set_in_skin(wizard, 2, true);
        rig.set_in_skin(knight, 3, true);
        rig.set_in_skin(wizard, 3, true);
        rig.skin = Some(knight);
        let shown = |rig: &Rig| [1, 2, 3, 4].map(|l| rig.alpha_at(None, l) > 0.5);
        assert_eq!(shown(&rig), [true, false, true, true]);
        rig.skin = Some(wizard);
        assert_eq!(shown(&rig), [false, true, true, true]);
        // No active skin: everything, for working on the art.
        rig.skin = None;
        assert_eq!(shown(&rig), [true; 4]);
        // A removed skin frees its layers, and layers that are gone drop out.
        rig.skin = Some(wizard);
        rig.remove_skin(knight);
        assert_eq!(rig.active_skin(), None);
        rig.skin = Some(0);
        assert_eq!(shown(&rig), [true, true, true, true]);
        rig.prune(&[1, 4]);
        assert!(rig.skins[0].layers.is_empty());
    }

    #[test]
    fn reparenting_keeps_a_bone_where_it_is() {
        let mut rig = Rig::default();
        let a = rig.add_bone("a", None, 10.0, 20.0, 30.0);
        rig.bone_mut(a).unwrap().rotation = 40.0;
        let b = rig.add_bone("b", None, 60.0, 5.0, 12.0);
        rig.bone_mut(b).unwrap().rotation = -70.0;
        let c = rig.add_bone("c", Some(b), 12.0, 0.0, 8.0);
        let before = rig.pose(None);
        let world = |rig: &Rig, id: BoneId, pose: &[Xf]| pose[rig.bone_index(id).unwrap()];
        let (wa0, wb, wc) = (world(&rig, a, &before), world(&rig, b, &before), world(&rig, c, &before));
        // `a` was added first, so hanging it under `c` must also reorder the list.
        assert!(rig.set_parent(a, Some(c)));
        let after = rig.pose(None);
        let wa = world(&rig, a, &after);
        let close = |m: Xf, n: Xf| {
            [(m.a, n.a), (m.b, n.b), (m.c, n.c), (m.d, n.d), (m.e, n.e), (m.f, n.f)]
                .iter()
                .all(|(x, y)| (x - y).abs() < 1e-3)
        };
        assert!(close(wa, wa0), "{wa:?}");
        assert!(close(world(&rig, b, &after), wb) && close(world(&rig, c, &after), wc));
        let order: Vec<BoneId> = rig.bones.iter().map(|x| x.id).collect();
        assert_eq!(order, vec![b, c, a], "parents come first");
        // No loops, and a root again on request.
        assert!(!rig.set_parent(b, Some(a)));
        assert!(rig.set_parent(a, None));
        assert!(close(world(&rig, a, &rig.pose(None)), wa0));
    }

    #[test]
    fn full_turns_and_the_end_of_a_loop() {
        let mut rig = Rig::default();
        let g = rig.add_bone("gear", None, 0.0, 0.0, 10.0);
        let an = rig.add_animation("spin", 1000);
        // Typed at the end of the loop: two full turns, with the rest key added at 0.
        rig.key_from_rest(an, g, 1000, KeyValue::Rotate(720.0), Curve::Linear);
        assert_eq!(rig.key_times(an, g), vec![0, 1000]);
        assert!((rig.offsets(an, g, 250).1 - 180.0).abs() < 1e-3);
        // The playhead parked on the end reads the end key, not the start.
        assert!((rig.offsets(an, g, 1000).1 - 720.0).abs() < 1e-3);
        assert!((rig.offsets(an, g, 1250).1 - 180.0).abs() < 1e-3, "past the end it wraps");
        // A copied pose holds only what the bone keys.
        let pose = rig.keyed_pose(an, g, 500);
        assert_eq!(pose, [None, Some(KeyValue::Rotate(360.0)), None]);
    }
}
