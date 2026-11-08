//! Finds triangles that can never be seen from outside the model, such as the inner faces of a
//! hollow shell. Generated models are often thin double-sided shells, and those inner faces take
//! up texture space that nothing can ever show.
//!
//! A triangle is visible if rays from just above it, spread over the hemisphere it faces, escape
//! the model. Rays are walked through a uniform grid, which suits meshes of many small triangles.

use rayon::prelude::*;

use crate::mesh::Mesh;

const RAYS: usize = 32;
/// Rays that must escape. More than one, so that a stray ray through a tiny gap does not count.
const MIN_ESCAPES: usize = 2;
/// Grid cells along the longest side of the model.
const GRID_RESOLUTION: f32 = 96.0;
/// How far above a triangle rays start, as a fraction of the model's radius.
const RAY_OFFSET: f32 = 5e-5;

type P = [f32; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: P, b: P) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: P, b: P) -> P {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

struct Grid {
    min: P,
    cell: f32,
    size: [i32; 3],
    /// Start of each cell's list in `items`, with one extra entry at the end.
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Grid {
    fn build(triangles: &[[P; 3]]) -> Self {
        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for p in triangles.iter().flatten() {
            for a in 0..3 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        let longest = (0..3).map(|a| max[a] - min[a]).fold(1e-6, f32::max);
        let cell = longest / GRID_RESOLUTION;
        let size = [0, 1, 2].map(|a| (((max[a] - min[a]) / cell).ceil() as i32).max(1));
        let cell_of = |p: f32, a: usize| (((p - min[a]) / cell) as i32).clamp(0, size[a] - 1);
        let index = |x: i32, y: i32, z: i32| (x + y * size[0] + z * size[0] * size[1]) as usize;

        // Each triangle goes in every cell its bounding box touches, counted first and then filled.
        let total = (size[0] * size[1] * size[2]) as usize;
        let span = |t: &[P; 3], a: usize| {
            let (lo, hi) = t.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |m, p| (m.0.min(p[a]), m.1.max(p[a])));
            cell_of(lo, a)..=cell_of(hi, a)
        };
        let mut counts = vec![0u32; total + 1];
        for t in triangles {
            for z in span(t, 2) {
                for y in span(t, 1) {
                    for x in span(t, 0) {
                        counts[index(x, y, z) + 1] += 1;
                    }
                }
            }
        }
        for i in 0..total {
            counts[i + 1] += counts[i];
        }
        let mut next = counts.clone();
        let mut items = vec![0u32; counts[total] as usize];
        for (i, t) in triangles.iter().enumerate() {
            for z in span(t, 2) {
                for y in span(t, 1) {
                    for x in span(t, 0) {
                        let c = index(x, y, z);
                        items[next[c] as usize] = i as u32;
                        next[c] += 1;
                    }
                }
            }
        }
        Self { min, cell, size, starts: counts, items }
    }

    /// True if the ray hits any triangle other than `skip`.
    fn blocked(&self, triangles: &[[P; 3]], origin: P, direction: P, skip: usize) -> bool {
        let mut cell = [0i32; 3];
        let (mut step, mut next, mut delta) = ([0i32; 3], [f32::INFINITY; 3], [f32::INFINITY; 3]);
        for a in 0..3 {
            let position = (origin[a] - self.min[a]) / self.cell;
            cell[a] = (position.floor() as i32).clamp(0, self.size[a] - 1);
            if direction[a] > 1e-9 {
                step[a] = 1;
                delta[a] = self.cell / direction[a];
                next[a] = ((cell[a] + 1) as f32 - position) * delta[a];
            } else if direction[a] < -1e-9 {
                step[a] = -1;
                delta[a] = self.cell / -direction[a];
                next[a] = (position - cell[a] as f32) * delta[a];
            }
        }
        loop {
            let c = (cell[0] + cell[1] * self.size[0] + cell[2] * self.size[0] * self.size[1]) as usize;
            for &t in &self.items[self.starts[c] as usize..self.starts[c + 1] as usize] {
                if t as usize != skip && hits(&triangles[t as usize], origin, direction) {
                    return true;
                }
            }
            let a = if next[0] <= next[1] && next[0] <= next[2] {
                0
            } else if next[1] <= next[2] {
                1
            } else {
                2
            };
            cell[a] += step[a];
            if step[a] == 0 || cell[a] < 0 || cell[a] >= self.size[a] {
                return false;
            }
            next[a] += delta[a];
        }
    }
}

/// Möller-Trumbore, any hit in front of the origin.
fn hits(t: &[P; 3], origin: P, direction: P) -> bool {
    let (e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]));
    let p = cross(direction, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-14 {
        return false;
    }
    let inverse = 1.0 / det;
    let s = sub(origin, t[0]);
    let u = dot(s, p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = cross(s, e1);
    let v = dot(direction, q) * inverse;
    v >= 0.0 && u + v <= 1.0 && dot(e2, q) * inverse > 0.0
}

/// Evenly spread directions over the hemisphere around +Z.
fn hemisphere() -> [P; RAYS] {
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    std::array::from_fn(|i| {
        // Stay a little above the horizon, where rays only graze the triangle's own neighbors.
        let z = 0.08 + 0.92 * (i as f32 + 0.5) / RAYS as f32;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let (sin, cos) = (i as f32 * golden).sin_cos();
        [r * cos, r * sin, z]
    })
}

/// For each triangle, whether it can never be seen from outside.
pub fn find_hidden(mesh: &Mesh) -> Vec<bool> {
    let (_, radius) = mesh.bounds();
    let triangles: Vec<[P; 3]> =
        mesh.indices.chunks_exact(3).map(|t| [0, 1, 2].map(|k| mesh.vertices[t[k] as usize].position)).collect();
    let grid = Grid::build(&triangles);
    let rays = hemisphere();
    let offset = radius * RAY_OFFSET;

    (0..triangles.len())
        .into_par_iter()
        .map(|i| {
            let t = &triangles[i];
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            let length = dot(n, n).sqrt();
            if length < 1e-14 {
                return false; // degenerate: draws nothing, so nothing is gained by judging it
            }
            let n = n.map(|c| c / length);
            let helper = if n[0].abs() < 0.8 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            let u = cross(n, helper);
            let u_length = dot(u, u).sqrt();
            let u = u.map(|c| c / u_length);
            let v = cross(n, u);
            let center = [0, 1, 2].map(|a| (t[0][a] + t[1][a] + t[2][a]) / 3.0 + n[a] * offset);
            let mut escaped = 0;
            for d in &rays {
                let direction = [0, 1, 2].map(|a| u[a] * d[0] + v[a] * d[1] + n[a] * d[2]);
                if !grid.blocked(&triangles, center, direction, i) {
                    escaped += 1;
                    if escaped >= MIN_ESCAPES {
                        return false;
                    }
                }
            }
            true
        })
        .collect()
}

/// A copy of `mesh` without the triangles flagged in `remove`, keeping the others in order.
pub fn without_triangles(mesh: &Mesh, remove: &[bool]) -> Mesh {
    let mut map = vec![u32::MAX; mesh.vertices.len()];
    let mut out = Mesh::default();
    for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
        if remove[t] {
            continue;
        }
        for &i in tri {
            if map[i as usize] == u32::MAX {
                map[i as usize] = out.vertices.len() as u32;
                out.vertices.push(mesh.vertices[i as usize]);
            }
            out.indices.push(map[i as usize]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Vertex;

    fn add_quad(mesh: &mut Mesh, corners: [P; 4]) {
        let base = mesh.vertices.len() as u32;
        for position in corners {
            mesh.vertices.push(Vertex { position, normal: [0.0, 0.0, 1.0], uv: [0.0, 0.0] });
        }
        mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A closed box with outward faces, and a smaller closed box inside it with outward faces.
    fn box_faces(mesh: &mut Mesh, a: f32) {
        add_quad(mesh, [[-a, -a, a], [a, -a, a], [a, a, a], [-a, a, a]]);
        add_quad(mesh, [[a, -a, -a], [-a, -a, -a], [-a, a, -a], [a, a, -a]]);
        add_quad(mesh, [[a, -a, a], [a, -a, -a], [a, a, -a], [a, a, a]]);
        add_quad(mesh, [[-a, -a, -a], [-a, -a, a], [-a, a, a], [-a, a, -a]]);
        add_quad(mesh, [[-a, a, a], [a, a, a], [a, a, -a], [-a, a, -a]]);
        add_quad(mesh, [[-a, -a, -a], [a, -a, -a], [a, -a, a], [-a, -a, a]]);
    }

    #[test]
    fn a_box_inside_a_box_is_hidden_and_the_outer_one_is_not() {
        let mut mesh = Mesh::default();
        box_faces(&mut mesh, 1.0);
        box_faces(&mut mesh, 0.5);
        let hidden = find_hidden(&mesh);
        assert!(hidden[..12].iter().all(|h| !h), "outer: {:?}", &hidden[..12]);
        assert!(hidden[12..].iter().all(|h| *h), "inner: {:?}", &hidden[12..]);
    }

    #[test]
    fn a_flat_open_sheet_is_visible() {
        let mut mesh = Mesh::default();
        add_quad(&mut mesh, [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]]);
        assert!(find_hidden(&mesh).iter().all(|h| !h));
    }

    #[test]
    fn removing_triangles_keeps_the_rest_in_order() {
        let mut mesh = Mesh::default();
        add_quad(&mut mesh, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]);
        let out = without_triangles(&mesh, &[true, false]);
        assert_eq!(out.triangle_count(), 1);
        assert_eq!(out.vertices.len(), 3);
        assert_eq!(out.vertices[out.indices[1] as usize].position, [1.0, 1.0, 0.0]);
    }
}
