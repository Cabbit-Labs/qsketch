//! The small amount of matrix math the Model panel needs. Matrices are column-major, as WGSL expects.

pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `a * b`: applies `b` first.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let at = |r: usize| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r];
    [at(0), at(1), at(2)]
}

fn translation(t: [f32; 3]) -> Mat4 {
    let mut m = IDENTITY;
    m[12..15].copy_from_slice(&t);
    m
}

fn scale(s: f32) -> Mat4 {
    let mut m = IDENTITY;
    m[0] = s;
    m[5] = s;
    m[10] = s;
    m
}

/// Rotation about X by `turns` quarter turns, using exact values so repeated turns do not drift.
fn quarter_turns_x(turns: u8) -> Mat4 {
    let (sin, cos) = [(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-1.0, 0.0)][usize::from(turns % 4)];
    let mut m = IDENTITY;
    m[5] = cos;
    m[6] = sin;
    m[9] = -sin;
    m[10] = cos;
    m
}

/// Centers a mesh on the origin, scales it to fit the unit sphere, then applies the user's up-axis correction.
pub fn model_matrix(center: [f32; 3], radius: f32, turns: u8) -> Mat4 {
    let fit = mul(&scale(1.0 / radius.max(1e-6)), &translation([-center[0], -center[1], -center[2]]));
    mul(&quarter_turns_x(turns), &fit)
}

/// The mesh-space direction that ends up pointing up after `turns` quarter turns about X.
pub fn up_in_mesh(turns: u8) -> [f32; 3] {
    [[0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]][usize::from(turns % 4)]
}

/// The nearest triangle under a point in normalized device coordinates, for clicking on a model.
pub fn pick_triangle(mesh: &crate::mesh::Mesh, matrix: &Mat4, ndc: [f32; 2]) -> Option<usize> {
    let project = |p: [f32; 3]| {
        let at = |r: usize| matrix[r] * p[0] + matrix[4 + r] * p[1] + matrix[8 + r] * p[2] + matrix[12 + r];
        let w = at(3);
        (w > 1e-6).then(|| [at(0) / w, at(1) / w, at(2) / w])
    };
    let projected: Vec<Option<[f32; 3]>> = mesh.vertices.iter().map(|v| project(v.position)).collect();
    let mut best: Option<(usize, f32)> = None;
    for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
        let (Some(a), Some(b), Some(c)) =
            (projected[tri[0] as usize], projected[tri[1] as usize], projected[tri[2] as usize])
        else {
            continue;
        };
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area == 0.0 {
            continue;
        }
        let u = ((ndc[0] - a[0]) * (c[1] - a[1]) - (ndc[1] - a[1]) * (c[0] - a[0])) / area;
        let v = ((b[0] - a[0]) * (ndc[1] - a[1]) - (b[1] - a[1]) * (ndc[0] - a[0])) / area;
        if u < 0.0 || v < 0.0 || u + v > 1.0 {
            continue;
        }
        let depth = a[2] + u * (b[2] - a[2]) + v * (c[2] - a[2]);
        if best.is_none_or(|(_, d)| depth < d) {
            best = Some((t, depth));
        }
    }
    best.map(|(t, _)| t)
}

/// An orbit camera around the origin, for a model that fits the unit sphere.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub yaw: f32,
    /// Radians above the horizon.
    pub pitch: f32,
    /// Multiplies the distance that exactly frames the model; below 1 is closer.
    pub zoom: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self { yaw: 0.6, pitch: 0.45, zoom: 1.0 }
    }
}

impl Orbit {
    const FOV_Y: f32 = 35.0 * std::f32::consts::PI / 180.0;
    const MARGIN: f32 = 1.05;
    const MAX_PITCH: f32 = 1.55;

    pub fn rotate(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw = (self.yaw + dyaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + dpitch).clamp(-Self::MAX_PITCH, Self::MAX_PITCH);
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.zoom = (self.zoom * factor).clamp(0.1, 5.0);
    }

    /// World to clip space, with wgpu's 0..1 depth.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(1e-3);
        let half = Self::FOV_Y / 2.0;
        // A narrow panel must back away until the sphere also fits horizontally.
        let half_fit = if aspect < 1.0 { (half.tan() * aspect).atan() } else { half };
        let distance = Self::MARGIN / half_fit.sin() * self.zoom;
        let (near, far) = ((distance - 2.0).max(distance * 0.05), distance + 2.0);

        let (sy, cy, sp, cp) = (self.yaw.sin(), self.yaw.cos(), self.pitch.sin(), self.pitch.cos());
        let back = [sy * cp, sp, cy * cp]; // from the origin toward the camera
        let right = [cy, 0.0, -sy];
        let up = [-sy * sp, cp, -cy * sp];
        let view = [
            right[0], up[0], back[0], 0.0, //
            right[1], up[1], back[1], 0.0, //
            right[2], up[2], back[2], 0.0, //
            0.0, 0.0, -distance, 1.0,
        ];

        let f = 1.0 / half.tan();
        let depth = far / (near - far);
        #[rustfmt::skip]
        let proj = [
            f / aspect, 0.0, 0.0, 0.0,
            0.0, f, 0.0, 0.0,
            0.0, 0.0, depth, -1.0,
            0.0, 0.0, near * depth, 0.0,
        ];
        mul(&proj, &view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
        let at = |r: usize| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r];
        let w = at(3);
        [at(0) / w, at(1) / w, at(2) / w]
    }

    #[test]
    fn up_in_mesh_matches_the_model_rotation() {
        for turns in 0..4 {
            let up = transform_point(&model_matrix([0.0; 3], 1.0, turns), up_in_mesh(turns));
            assert_eq!(up, [0.0, 1.0, 0.0], "turns {turns}");
        }
    }

    #[test]
    fn picking_takes_the_nearest_triangle_and_misses_outside() {
        use crate::mesh::{Mesh, Vertex};
        let v = |x: f32, y: f32, z: f32| Vertex { position: [x, y, z], normal: [0.0, 0.0, 1.0], uv: [0.0, 0.0] };
        let mut mesh = Mesh::default();
        for z in [-0.5, 0.5] {
            let base = mesh.vertices.len() as u32;
            mesh.vertices.extend([v(-1.0, -1.0, z), v(1.0, -1.0, z), v(0.0, 1.0, z)]);
            mesh.indices.extend([base, base + 1, base + 2]);
        }
        let vp = Orbit { yaw: 0.0, pitch: 0.0, zoom: 1.0 }.view_proj(1.0);
        assert_eq!(pick_triangle(&mesh, &vp, [0.0, 0.0]), Some(1), "the camera is on the +Z side");
        assert_eq!(pick_triangle(&mesh, &vp, [0.95, 0.95]), None);
    }

    #[test]
    fn multiply_applies_the_right_operand_first() {
        let m = mul(&scale(2.0), &translation([1.0, 0.0, 0.0]));
        assert_eq!(transform_point(&m, [0.0, 0.0, 0.0]), [2.0, 0.0, 0.0]);
    }

    #[test]
    fn unit_sphere_fits_at_any_aspect() {
        for aspect in [0.3, 1.0, 2.5] {
            let vp = Orbit { yaw: 0.0, pitch: 0.0, zoom: 1.0 }.view_proj(aspect);
            for p in [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0]] {
                let c = clip(&vp, p);
                assert!(c[0].abs() <= 1.0 && c[1].abs() <= 1.0, "{p:?} at aspect {aspect}: {c:?}");
            }
        }
    }

    #[test]
    fn depth_is_in_range_and_near_is_smaller() {
        let vp = Orbit { yaw: 0.0, pitch: 0.0, zoom: 1.0 }.view_proj(1.0);
        let (near, far) = (clip(&vp, [0.0, 0.0, 1.0])[2], clip(&vp, [0.0, 0.0, -1.0])[2]);
        assert!((0.0..=1.0).contains(&near) && (0.0..=1.0).contains(&far) && near < far, "{near} {far}");
    }

    #[test]
    fn up_stays_up_on_screen() {
        let vp = Orbit { yaw: 1.0, pitch: 0.4, zoom: 1.0 }.view_proj(1.0);
        assert!(clip(&vp, [0.0, 0.5, 0.0])[1] > clip(&vp, [0.0, -0.5, 0.0])[1]);
    }

    #[test]
    fn a_quarter_turn_stands_minus_z_up() {
        // The banana peel's stem points along -Z, and one quarter turn must stand it upright.
        let m = model_matrix([0.0; 3], 1.0, 1);
        let p = transform_point(&m, [0.0, 0.0, -1.0]);
        assert_eq!(p, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn model_matrix_centers_and_fits() {
        let m = model_matrix([1.0, 2.0, 3.0], 2.0, 0);
        assert_eq!(transform_point(&m, [1.0, 2.0, 3.0]), [0.0, 0.0, 0.0]);
        assert_eq!(transform_point(&m, [3.0, 2.0, 3.0]), [1.0, 0.0, 0.0]);
    }
}
