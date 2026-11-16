//! Mesh deformation for rigged layers: a grid laid over the layer, every
//! vertex weighted to up to [`MAX_INFLUENCES`] bones, so the picture bends
//! where bones with shared weights move differently (an elbow, a tail).
//!
//! A vertex is skinned by blending how far each of its bones has moved from
//! the setup pose: `p' = Σ wᵢ · (poseᵢ · setupᵢ⁻¹)(p) / Σ wᵢ`. A layer with
//! a mesh ignores its single-bone attachment when drawn; vertices with no
//! weights stay where they are.

use serde::{Deserialize, Serialize};

use crate::geom::{IRect, Pt};
use crate::layer::LayerId;
use crate::raster::{Raster, ResizeFilter};
use crate::rig::{BoneId, Rig, Xf};
use crate::smart::{SmartMode, SmartObject, SmartSource};

/// Most bones a single vertex follows.
pub const MAX_INFLUENCES: usize = 4;
/// Most cells along either side of a mesh.
pub const MAX_GRID: u32 = 48;
/// Weights below this are dropped.
const MIN_WEIGHT: f32 = 0.01;

/// How strongly one bone pulls a vertex.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Influence {
    pub bone: BoneId,
    pub weight: f32,
}

/// The grid mesh of one layer and its vertex weights.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartMesh {
    pub layer: LayerId,
    /// Where the mesh sits in the setup pose (document pixels). The layer's
    /// picture is cropped to it, and the exported part image is exactly it.
    pub rect: IRect,
    pub cols: u32,
    pub rows: u32,
    /// One list per vertex, row-major: `(cols + 1) × (rows + 1)` of them.
    pub weights: Vec<Vec<Influence>>,
}

impl PartMesh {
    pub fn vertex_count(&self) -> usize {
        ((self.cols + 1) * (self.rows + 1)) as usize
    }

    /// Vertex `(i, j)`'s index.
    pub fn index(&self, i: u32, j: u32) -> usize {
        (j * (self.cols + 1) + i) as usize
    }

    /// The vertices in the setup pose (document pixels).
    pub fn setup_points(&self) -> Vec<Pt> {
        let (w, h) = (self.rect.w as f32, self.rect.h as f32);
        let mut out = Vec::with_capacity(self.vertex_count());
        for j in 0..=self.rows {
            for i in 0..=self.cols {
                out.push(Pt::new(
                    self.rect.x as f32 + w * i as f32 / self.cols as f32,
                    self.rect.y as f32 + h * j as f32 / self.rows as f32,
                ));
            }
        }
        out
    }
}

/// Clamp a vertex's weights: non-negative, the strongest few, summing to 1,
/// with negligible ones dropped.
pub fn normalize(w: &mut Vec<Influence>) {
    fn scale_to_one(w: &mut Vec<Influence>) {
        let sum: f32 = w.iter().map(|i| i.weight).sum();
        if sum <= 0.0 {
            w.clear();
        } else {
            for i in w.iter_mut() {
                i.weight /= sum;
            }
        }
    }
    w.retain(|i| i.weight.is_finite() && i.weight > 0.0);
    w.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    w.truncate(MAX_INFLUENCES);
    scale_to_one(w);
    if w.len() > 1 {
        // The strongest is at least 1/MAX_INFLUENCES, so it always stays.
        w.retain(|i| i.weight >= MIN_WEIGHT);
        scale_to_one(w);
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f32 {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let l2 = abx * abx + aby * aby;
    if l2 < 1e-9 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / l2).clamp(0.0, 1.0);
    p.dist(Pt::new(a.x + abx * t, a.y + aby * t))
}

impl Rig {
    pub fn mesh_of(&self, layer: LayerId) -> Option<&PartMesh> {
        self.meshes.iter().find(|m| m.layer == layer)
    }

    /// Lay a grid of roughly `cell`-pixel cells over `rect` for `layer`,
    /// every vertex starting fully on the layer's bone (or unweighted when
    /// it has none). Replaces any earlier mesh. False for an empty `rect`.
    pub fn add_mesh(&mut self, layer: LayerId, rect: IRect, cell: u32) -> bool {
        if rect.w <= 0 || rect.h <= 0 {
            return false;
        }
        let cell = cell.max(2);
        let cols = (rect.w as u32).div_ceil(cell).clamp(1, MAX_GRID);
        let rows = (rect.h as u32).div_ceil(cell).clamp(1, MAX_GRID);
        let start = self.attached_bone(layer).map(|b| vec![Influence { bone: b, weight: 1.0 }]).unwrap_or_default();
        let mesh = PartMesh { layer, rect, cols, rows, weights: vec![start; ((cols + 1) * (rows + 1)) as usize] };
        self.meshes.retain(|m| m.layer != layer);
        self.meshes.push(mesh);
        true
    }

    pub fn remove_mesh(&mut self, layer: LayerId) {
        self.meshes.retain(|m| m.layer != layer);
    }

    /// Weight every vertex of `layer`'s mesh by closeness to bones, in the
    /// setup pose. With `nearby`, only the layer's own bone, its parent and
    /// the bones that hang from it (children, their children and so on: a
    /// tail's whole chain) compete, so a limb's picture follows the limb;
    /// otherwise every bone does.
    pub fn auto_weights(&mut self, layer: LayerId, nearby: bool) {
        let Some(mi) = self.meshes.iter().position(|m| m.layer == layer) else { return };
        let setup = self.pose(None);
        let mut candidates: Vec<usize> = (0..self.bones.len()).collect();
        if let (true, Some(own)) = (nearby, self.attached_bone(layer).and_then(|b| self.bone(b))) {
            let (own_id, own_parent) = (own.id, own.parent);
            candidates = (0..self.bones.len())
                .filter(|&i| {
                    let b = &self.bones[i];
                    // Walk up from the bone: under the layer's bone?
                    let mut up = b.parent;
                    let mut under = false;
                    for _ in 0..self.bones.len() {
                        match up {
                            Some(p) if p == own_id => {
                                under = true;
                                break;
                            }
                            Some(p) => up = self.bone(p).and_then(|x| x.parent),
                            None => break,
                        }
                    }
                    b.id == own_id || Some(b.id) == own_parent || under
                })
                .collect();
        }
        if candidates.is_empty() {
            return;
        }
        let segs: Vec<(BoneId, Pt, Pt)> = candidates
            .iter()
            .filter_map(|&i| {
                Some((self.bones[i].id, self.head(self.bones[i].id, &setup)?, self.tip(self.bones[i].id, &setup)?))
            })
            .collect();
        let pts = self.meshes[mi].setup_points();
        let weights: Vec<Vec<Influence>> = pts
            .iter()
            .map(|&p| {
                let mut w: Vec<Influence> = segs
                    .iter()
                    .map(|&(bone, a, b)| Influence { bone, weight: 1.0 / (seg_dist(p, a, b) + 4.0).powi(3) })
                    .collect();
                normalize(&mut w);
                w
            })
            .collect();
        self.meshes[mi].weights = weights;
    }

    /// Paint `bone`'s weight: each `(vertex, amount)` moves that vertex's
    /// weight on `bone` toward 1 (or, with `remove`, toward 0) by `amount`,
    /// taking from or giving back to its other bones.
    pub fn paint_weights(&mut self, layer: LayerId, bone: BoneId, hits: &[(usize, f32)], remove: bool) {
        if self.bone(bone).is_none() {
            return;
        }
        let Some(m) = self.meshes.iter_mut().find(|m| m.layer == layer) else { return };
        for &(v, amount) in hits {
            let Some(w) = m.weights.get_mut(v) else { continue };
            let cur = w.iter().find(|i| i.bone == bone).map_or(0.0, |i| i.weight);
            let target = if remove { (cur - amount).max(0.0) } else { (cur + amount).min(1.0) };
            let others: f32 = w.iter().filter(|i| i.bone != bone).map(|i| i.weight).sum();
            w.retain(|i| i.bone != bone);
            if others > 0.0 {
                let k = (1.0 - target) / others;
                for i in w.iter_mut() {
                    i.weight *= k;
                }
            }
            if target > 0.0 {
                w.push(Influence { bone, weight: target });
            }
            normalize(w);
        }
    }

    /// Where `mesh`'s vertices are under `pose`.
    pub fn skin_points(&self, mesh: &PartMesh, pose: &[Xf], setup: &[Xf]) -> Vec<Pt> {
        let deltas: Vec<Option<Xf>> =
            self.bones.iter().enumerate().map(|(i, _)| Some(*pose.get(i)? * setup.get(i)?.inverse()?)).collect();
        let delta = |bone: BoneId| self.bone_index(bone).and_then(|i| deltas[i]);
        mesh.setup_points()
            .into_iter()
            .zip(&mesh.weights)
            .map(|(p, w)| {
                let (mut x, mut y, mut sum) = (0.0f32, 0.0f32, 0.0f32);
                for inf in w {
                    if let Some(d) = delta(inf.bone) {
                        let q = d.apply(p);
                        x += q.x * inf.weight;
                        y += q.y * inf.weight;
                        sum += inf.weight;
                    }
                }
                if sum <= 0.0 {
                    p
                } else {
                    Pt::new(x / sum, y / sum)
                }
            })
            .collect()
    }

    /// Forget `gone` bones in every mesh's weights. A vertex left with none
    /// falls back to its layer's own bone, if that is still there.
    pub(crate) fn prune_mesh_bones(&mut self, gone: &[BoneId]) {
        let fallbacks: Vec<Option<BoneId>> =
            self.meshes.iter().map(|m| self.attached_bone(m.layer).filter(|b| !gone.contains(b))).collect();
        for (m, fallback) in self.meshes.iter_mut().zip(fallbacks) {
            for w in &mut m.weights {
                w.retain(|i| !gone.contains(&i.bone));
                normalize(w);
                if w.is_empty() {
                    if let Some(b) = fallback {
                        w.push(Influence { bone: b, weight: 1.0 });
                    }
                }
            }
        }
    }
}

/// `raster` drawn through `mesh` with its vertices moved to `points`
/// (document space), resampled with `filter`.
pub fn render_mesh(
    raster: &Raster,
    mesh: &PartMesh,
    points: &[Pt],
    width: u32,
    height: u32,
    filter: ResizeFilter,
) -> Raster {
    if points.len() != mesh.vertex_count() {
        return raster.clone();
    }
    let mut so = SmartObject::identity(mesh.rect);
    so.mode = SmartMode::Warp;
    so.mesh_cols = mesh.cols;
    so.mesh_rows = mesh.rows;
    so.mesh = points.to_vec();
    so.filter = filter;
    so.render(&SmartSource::Pixels(raster.crop(mesh.rect)), width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::{Curve, KeyValue};

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// Two 20-long bones laid end to end along y=5, and a 4×1 mesh over a
    /// 40×10 layer.
    fn strip() -> (Rig, BoneId, BoneId, LayerId) {
        let mut rig = Rig::default();
        let a = rig.add_bone("a", None, 0.0, 5.0, 20.0);
        let b = rig.add_bone("b", Some(a), 20.0, 0.0, 20.0);
        rig.attach(7, a);
        assert!(rig.add_mesh(7, IRect::new(0, 0, 40, 10), 10));
        (rig, a, b, 7)
    }

    #[test]
    fn a_new_mesh_starts_on_the_layers_bone_and_skins_to_itself_at_rest() {
        let (rig, a, _, layer) = strip();
        let m = rig.mesh_of(layer).unwrap();
        assert_eq!((m.cols, m.rows, m.weights.len()), (4, 1, 10));
        assert!(m.weights.iter().all(|w| w == &vec![Influence { bone: a, weight: 1.0 }]));
        let setup = rig.pose(None);
        let pts = rig.skin_points(m, &setup, &setup);
        for (p, q) in pts.iter().zip(m.setup_points()) {
            assert!(near(p.x, q.x) && near(p.y, q.y));
        }
    }

    #[test]
    fn auto_weights_split_the_strip_between_the_bones_and_it_bends() {
        let (mut rig, a, b, layer) = strip();
        rig.auto_weights(layer, true);
        let m = rig.mesh_of(layer).unwrap();
        let w_b = |v: usize| m.weights[v].iter().find(|i| i.bone == b).map_or(0.0, |i| i.weight);
        assert!(w_b(m.index(0, 0)) < 0.2, "far left follows a");
        assert!(w_b(m.index(4, 0)) > 0.8, "far right follows b");
        assert!(m.weights.iter().all(|w| (w.iter().map(|i| i.weight).sum::<f32>() - 1.0).abs() < 1e-4));

        // Turn b a quarter turn: the right end swings, the left end stays.
        let an = rig.add_animation("bend", 1000);
        rig.set_key(an, b, 0, KeyValue::Rotate(90.0), Curve::Linear);
        let (setup, pose) = (rig.pose(None), rig.pose(Some((an, 0))));
        let m = rig.mesh_of(layer).unwrap();
        let pts = rig.skin_points(m, &pose, &setup);
        let (left, mid, right) = (pts[m.index(0, 0)], pts[m.index(2, 0)], pts[m.index(4, 0)]);
        assert!(left.dist(Pt::new(0.0, 0.0)) < 3.0, "{left:?}");
        assert!(right.y > 12.0, "{right:?}");
        // A rigid attachment would put the middle straight above the right end's x.
        assert!(mid.y > 0.5 && mid.y < right.y, "{mid:?} {:?}", m.weights[m.index(2, 0)]);
        let _ = a;
    }

    #[test]
    fn painting_adds_and_removes_weight_and_keeps_the_sum_at_one() {
        let (mut rig, a, b, layer) = strip();
        let v = rig.mesh_of(layer).unwrap().index(2, 0);
        rig.paint_weights(layer, b, &[(v, 0.25)], false);
        let w = &rig.mesh_of(layer).unwrap().weights[v];
        let get = |bone: BoneId| w.iter().find(|i| i.bone == bone).map_or(0.0, |i| i.weight);
        assert!(near(get(b), 0.25) && near(get(a), 0.75), "{w:?}");
        rig.paint_weights(layer, b, &[(v, 5.0)], false);
        let w = &rig.mesh_of(layer).unwrap().weights[v];
        assert_eq!(w.len(), 1);
        assert!(near(w[0].weight, 1.0) && w[0].bone == b);
        rig.paint_weights(layer, b, &[(v, 0.5)], true);
        let w = &rig.mesh_of(layer).unwrap().weights[v];
        assert!(w.len() == 1 && w[0].bone == b && near(w[0].weight, 1.0), "nothing else to give it to: {w:?}");
        rig.paint_weights(layer, b, &[(v, 5.0)], true);
        assert!(rig.mesh_of(layer).unwrap().weights[v].is_empty(), "removed down to nothing");
    }

    #[test]
    fn normalize_keeps_the_strongest_four() {
        let mut w: Vec<Influence> = (0..6).map(|i| Influence { bone: i, weight: (i + 1) as f32 }).collect();
        normalize(&mut w);
        assert_eq!(w.len(), MAX_INFLUENCES);
        assert!(w.iter().all(|i| i.bone >= 2));
        assert!(near(w.iter().map(|i| i.weight).sum(), 1.0));
    }

    #[test]
    fn deleting_a_bone_drops_its_weights_and_the_rest_renormalize() {
        let (mut rig, a, b, layer) = strip();
        rig.auto_weights(layer, true);
        rig.remove_bone(b);
        let m = rig.mesh_of(layer).unwrap();
        assert!(m.weights.iter().all(|w| w.len() == 1 && w[0].bone == a && near(w[0].weight, 1.0)));
    }

    #[test]
    fn the_mesh_survives_json() {
        let (mut rig, ..) = strip();
        rig.auto_weights(7, true);
        let back: Rig = serde_json::from_str(&serde_json::to_string(&rig).unwrap()).unwrap();
        assert_eq!(rig, back);
    }
}
