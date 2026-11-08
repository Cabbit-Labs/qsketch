//! Turns a mesh into a handful of named parts, independent of its UV layout. See `docs/Q3D.md`.
//!
//! 1. Weld vertices that UV seams split, and connect triangles across shared edges.
//! 2. Find the protruding tips: repeatedly take the surface point farthest from every tip so far.
//! 3. Give each triangle to a tip by surface distance, scaled by how far that tip reaches from
//!    the hub, so a short protrusion such as a stem does not lose ground to long neighbors.
//! 4. Split flat parts into their two sides by which way each triangle faces.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use crate::mesh::Mesh;

/// A new tip must be at least this fraction of the surface's longest path from every other tip.
const TIP_SEPARATION: f32 = 0.4;
const MAX_PARTS: usize = 16;
/// Tips closer than this fraction of the longest path, in straight-line distance, are one protrusion
/// (a thick sheet's end is reached from both faces).
const TIP_MERGE: f32 = 0.12;
/// A tip whose reach from the hub is under this fraction of the longest reach is a bump, not a part.
const MIN_REACH: f32 = 0.4;
/// A part gets two sides if it is flat (width over thickness) or a long strip (length over width),
/// compared as variance ratios. A curved peel strip passes the second test but not the first.
const FLAT_RATIO: f32 = 2.5;
const LONG_RATIO: f32 = 3.0;
/// Each part is cut into this many slices along its length, since a curved strip has no single plane.
const SLICES: usize = 8;
const SMOOTHING_PASSES: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    pub triangles: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Sections {
    /// The section of each triangle.
    pub of_triangle: Vec<u32>,
    pub list: Vec<Section>,
}

struct Graph {
    offsets: Vec<u32>,
    neighbors: Vec<u32>,
}

impl Graph {
    fn of(&self, t: usize) -> &[u32] {
        &self.neighbors[self.offsets[t] as usize..self.offsets[t + 1] as usize]
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    dot(d, d).sqrt()
}

/// Connects triangles that share an edge, after merging vertices at the same position.
fn build_graph(mesh: &Mesh, radius: f32) -> Graph {
    let step = radius * 1e-5;
    let mut welded: HashMap<[i64; 3], u32> = HashMap::with_capacity(mesh.vertices.len());
    let weld_ids: Vec<u32> = mesh
        .vertices
        .iter()
        .map(|v| {
            let key = v.position.map(|c| (c / step).round() as i64);
            let next = welded.len() as u32;
            *welded.entry(key).or_insert(next)
        })
        .collect();

    let mut edges: Vec<(u32, u32, u32)> = Vec::with_capacity(mesh.indices.len());
    for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
        let ids = [weld_ids[tri[0] as usize], weld_ids[tri[1] as usize], weld_ids[tri[2] as usize]];
        for (a, b) in [(ids[0], ids[1]), (ids[1], ids[2]), (ids[2], ids[0])] {
            if a != b {
                edges.push((a.min(b), a.max(b), t as u32));
            }
        }
    }
    edges.sort_unstable();

    let count = mesh.triangle_count();
    let mut links: Vec<(u32, u32)> = Vec::with_capacity(edges.len() * 2);
    for pair in edges.windows(2) {
        if (pair[0].0, pair[0].1) == (pair[1].0, pair[1].1) && pair[0].2 != pair[1].2 {
            links.push((pair[0].2, pair[1].2));
            links.push((pair[1].2, pair[0].2));
        }
    }
    links.sort_unstable();
    links.dedup();
    let mut offsets = vec![0u32; count + 1];
    for &(a, _) in &links {
        offsets[a as usize + 1] += 1;
    }
    for t in 0..count {
        offsets[t + 1] += offsets[t];
    }
    Graph { offsets, neighbors: links.into_iter().map(|(_, b)| b).collect() }
}

#[derive(PartialEq)]
struct Entry(f32, u32);
impl Eq for Entry {}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Distance along the surface from `start` to every triangle (infinite when unreachable).
fn surface_distances(graph: &Graph, centers: &[[f32; 3]], start: usize) -> Vec<f32> {
    let mut best = vec![f32::INFINITY; centers.len()];
    let mut heap = BinaryHeap::new();
    best[start] = 0.0;
    heap.push(Entry(0.0, start as u32));
    while let Some(Entry(d, t)) = heap.pop() {
        let t = t as usize;
        if d > best[t] {
            continue;
        }
        for &n in graph.of(t) {
            let n = n as usize;
            let nd = d + distance(centers[t], centers[n]);
            if nd < best[n] {
                best[n] = nd;
                heap.push(Entry(nd, n as u32));
            }
        }
    }
    best
}

/// Labels connected components, returning each triangle's label and each label's total area.
fn components(graph: &Graph, areas: &[f32]) -> (Vec<u32>, Vec<f32>) {
    let mut label = vec![u32::MAX; areas.len()];
    let mut totals = Vec::new();
    let mut stack = Vec::new();
    for start in 0..areas.len() {
        if label[start] != u32::MAX {
            continue;
        }
        let id = totals.len() as u32;
        let mut total = 0.0;
        label[start] = id;
        stack.push(start);
        while let Some(t) = stack.pop() {
            total += areas[t];
            for &n in graph.of(t) {
                if label[n as usize] == u32::MAX {
                    label[n as usize] = id;
                    stack.push(n as usize);
                }
            }
        }
        totals.push(total);
    }
    (label, totals)
}

fn farthest(values: &[f32], allowed: &[bool]) -> usize {
    let mut best = (0, f32::NEG_INFINITY);
    for (i, &v) in values.iter().enumerate() {
        if allowed[i] && v.is_finite() && v > best.1 {
            best = (i, v);
        }
    }
    best.0
}

/// Eigenvector of the smallest eigenvalue of a symmetric 3x3 matrix (cyclic Jacobi), and the
/// eigenvalues in descending order.
fn smallest_axis(mut m: [[f32; 3]; 3]) -> ([f32; 3], [f32; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0f32]];
    for _ in 0..32 {
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if m[p][q].abs() < 1e-12 {
                continue;
            }
            let theta = 0.5 * (2.0 * m[p][q]).atan2(m[q][q] - m[p][p]);
            let (s, c) = theta.sin_cos();
            let old = m;
            for k in 0..3 {
                m[k][p] = c * old[k][p] - s * old[k][q];
                m[k][q] = s * old[k][p] + c * old[k][q];
            }
            let mid = m;
            for k in 0..3 {
                m[p][k] = c * mid[p][k] - s * mid[q][k];
                m[q][k] = s * mid[p][k] + c * mid[q][k];
            }
            for row in &mut v {
                let (a, b) = (row[p], row[q]);
                row[p] = c * a - s * b;
                row[q] = s * a + c * b;
            }
        }
    }
    let mut order = [0, 1, 2];
    order.sort_by(|&a, &b| m[b][b].total_cmp(&m[a][a]));
    let axis = [v[0][order[2]], v[1][order[2]], v[2][order[2]]];
    (axis, [m[order[0]][order[0]], m[order[1]][order[1]], m[order[2]][order[2]]])
}

/// Eigenvector of the largest eigenvalue, by power iteration on a symmetric positive matrix.
fn largest_axis(m: [[f32; 3]; 3]) -> [f32; 3] {
    let mut v = [0.577f32, 0.577, 0.577];
    for _ in 0..64 {
        let next = [dot(m[0], v), dot(m[1], v), dot(m[2], v)];
        let length = dot(next, next).sqrt();
        if length < 1e-12 {
            break;
        }
        v = next.map(|c| c / length);
    }
    v
}

/// Weighted mean and covariance.
fn covariance_of(points: &[[f32; 3]], weights: &[f32]) -> ([f32; 3], [[f32; 3]; 3]) {
    let total = weights.iter().sum::<f32>().max(1e-12);
    let mut mean = [0.0f32; 3];
    for (p, w) in points.iter().zip(weights) {
        for a in 0..3 {
            mean[a] += p[a] * w / total;
        }
    }
    let mut covariance = [[0.0f32; 3]; 3];
    for (p, w) in points.iter().zip(weights) {
        let d = sub(*p, mean);
        for (r, row) in covariance.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell += d[r] * d[c] * w / total;
            }
        }
    }
    (mean, covariance)
}

fn part_name(index: usize) -> String {
    let letter = (b'A' + (index % 26) as u8) as char;
    format!("Part {letter}")
}

/// Finds a mesh's sections. `up` is the direction, in mesh space, that "top" should face.
pub fn find_sections(mesh: &Mesh, up: [f32; 3]) -> Sections {
    let count = mesh.triangle_count();
    if count == 0 {
        return Sections::default();
    }
    let (_, radius) = mesh.bounds();
    let graph = build_graph(mesh, radius);

    let corners = |t: usize| {
        let i = &mesh.indices[t * 3..t * 3 + 3];
        [
            mesh.vertices[i[0] as usize].position,
            mesh.vertices[i[1] as usize].position,
            mesh.vertices[i[2] as usize].position,
        ]
    };
    let mut centers = Vec::with_capacity(count);
    let mut normals = Vec::with_capacity(count); // length is twice the area
    let mut areas = Vec::with_capacity(count);
    for t in 0..count {
        let [a, b, c] = corners(t);
        centers.push([(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0]);
        let n = cross(sub(b, a), sub(c, a));
        areas.push(dot(n, n).sqrt() * 0.5);
        normals.push(n);
    }

    // Only the largest connected piece is analyzed; loose fragments join the nearest part afterwards.
    let (label, totals) = components(&graph, &areas);
    let main_label = totals.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i as u32);
    let in_main: Vec<bool> = label.iter().map(|&l| l == main_label).collect();

    // Tips: start from the end of a longest path, then keep taking the point farthest from them all.
    let seed = in_main.iter().position(|&m| m).unwrap_or(0);
    let from_seed = surface_distances(&graph, &centers, seed);
    let first = farthest(&from_seed, &in_main);
    let mut fields = vec![surface_distances(&graph, &centers, first)];
    let longest = fields[0][farthest(&fields[0], &in_main)];
    let mut tips = vec![first];
    let mut nearest = fields[0].clone();
    while fields.len() < MAX_PARTS {
        let candidate = farthest(&nearest, &in_main);
        if nearest[candidate] < longest * TIP_SEPARATION {
            break;
        }
        let field = surface_distances(&graph, &centers, candidate);
        for (n, f) in nearest.iter_mut().zip(&field) {
            *n = n.min(*f);
        }
        fields.push(field);
        tips.push(candidate);
    }

    // One field per protrusion: merge tips that are really the same end.
    let mut merged: Vec<(Vec<usize>, Vec<f32>)> = Vec::new();
    for (i, field) in fields.into_iter().enumerate() {
        let near = |group: &(Vec<usize>, Vec<f32>)| {
            group.0.iter().any(|&j| distance(centers[tips[i]], centers[tips[j]]) < longest * TIP_MERGE)
        };
        match merged.iter_mut().find(|g| near(g)) {
            Some(group) => {
                group.0.push(i);
                for (a, b) in group.1.iter_mut().zip(&field) {
                    *a = a.min(*b);
                }
            }
            None => merged.push((vec![i], field)),
        }
    }
    let mut fields: Vec<Vec<f32>> = merged.into_iter().map(|(_, field)| field).collect();

    // The hub is the point closest to its farthest tip, and each tip is measured against its reach from there.
    let mut hub = (seed, f32::INFINITY);
    for t in (0..count).filter(|&t| in_main[t]) {
        let worst = fields.iter().map(|f| f[t]).fold(0.0, f32::max);
        if worst < hub.1 {
            hub = (t, worst);
        }
    }
    let longest_reach = fields.iter().map(|f| f[hub.0]).fold(0.0, f32::max);
    fields.retain(|f| f[hub.0] >= longest_reach * MIN_REACH);
    let reach: Vec<f32> = fields.iter().map(|f| f[hub.0].max(1e-6)).collect();

    let mut part = vec![0u32; count];
    for t in (0..count).filter(|&t| in_main[t]) {
        let mut best = (0, f32::INFINITY);
        for (i, f) in fields.iter().enumerate() {
            let score = f[t] / reach[i];
            if score < best.1 {
                best = (i, score);
            }
        }
        part[t] = best.0 as u32;
    }

    // Loose fragments join the part of the nearest sampled triangle.
    let sampled: Vec<usize> = (0..count).filter(|&t| in_main[t]).step_by(16).collect();
    let mut nearest_part_of_label: HashMap<u32, u32> = HashMap::new();
    for t in (0..count).filter(|&t| !in_main[t]) {
        let found = *nearest_part_of_label.entry(label[t]).or_insert_with(|| {
            sampled
                .iter()
                .min_by(|&&a, &&b| distance(centers[t], centers[a]).total_cmp(&distance(centers[t], centers[b])))
                .map_or(0, |&s| part[s])
        });
        part[t] = found;
    }

    // Split flat or strip-like parts into two sides, using each slice's own plane.
    let parts = fields.len();
    let mut side = vec![0u8; count]; // 0 is top or the whole part, 1 is under
    let mut has_sides = vec![false; parts];
    for (p, part_has_sides) in has_sides.iter_mut().enumerate() {
        let members: Vec<usize> = (0..count).filter(|&t| part[t] == p as u32).collect();
        let weights: Vec<f32> = members.iter().map(|&t| areas[t]).collect();
        let points: Vec<[f32; 3]> = members.iter().map(|&t| centers[t]).collect();
        let (mean, covariance) = covariance_of(&points, &weights);
        let (mut part_axis, values) = smallest_axis(covariance);
        let flat = values[1] >= FLAT_RATIO * values[2].max(1e-12);
        let long = values[0] >= LONG_RATIO * values[1].max(1e-12);
        if !flat && !long {
            continue;
        }
        *part_has_sides = true;
        if dot(part_axis, up) < 0.0 {
            part_axis = part_axis.map(|c| -c);
        }
        let along = largest_axis(covariance);

        let offsets: Vec<f32> = points.iter().map(|&c| dot(sub(c, mean), along)).collect();
        let lowest = offsets.iter().copied().fold(f32::INFINITY, f32::min);
        let span = (offsets.iter().copied().fold(f32::NEG_INFINITY, f32::max) - lowest).max(1e-6);
        let slice_of = |o: f32| (((o - lowest) / span * SLICES as f32) as usize).min(SLICES - 1);

        let mut axes = [part_axis; SLICES];
        for (s, axis) in axes.iter_mut().enumerate() {
            let chosen: Vec<usize> = (0..members.len()).filter(|&m| slice_of(offsets[m]) == s).collect();
            if chosen.len() < 8 {
                continue;
            }
            let slice_points: Vec<[f32; 3]> = chosen.iter().map(|&m| points[m]).collect();
            let slice_weights: Vec<f32> = chosen.iter().map(|&m| weights[m]).collect();
            let (_, mut c) = covariance_of(&slice_points, &slice_weights);
            // Weigh down the direction along the strip so the thin axis is found across it.
            for r in 0..3 {
                for k in 0..3 {
                    c[r][k] += along[r] * along[k] * values[0] * 100.0;
                }
            }
            let (found, _) = smallest_axis(c);
            *axis = if dot(found, part_axis) < 0.0 { found.map(|v| -v) } else { found };
        }
        for (m, &t) in members.iter().enumerate() {
            side[t] = u8::from(dot(normals[t], axes[slice_of(offsets[m])]) < 0.0);
        }
    }

    // Each triangle takes the majority side of its neighbors, which clears speckle along the rims.
    for _ in 0..SMOOTHING_PASSES {
        let previous = side.clone();
        for t in 0..count {
            if !has_sides[part[t] as usize] {
                continue;
            }
            let mut votes = [0.0f32; 2];
            votes[usize::from(previous[t])] += areas[t];
            for &n in graph.of(t) {
                let n = n as usize;
                if part[n] == part[t] {
                    votes[usize::from(previous[n])] += areas[n];
                }
            }
            side[t] = u8::from(votes[1] > votes[0]);
        }
    }

    // Number the sections, skipping any that ended up empty.
    let mut triangles_in = vec![[0u32; 2]; parts];
    for t in 0..count {
        triangles_in[part[t] as usize][usize::from(side[t])] += 1;
    }
    let mut list = Vec::new();
    let mut ids = vec![[0u32; 2]; parts];
    for p in 0..parts {
        for s in 0..2 {
            let n = triangles_in[p][s];
            if n == 0 {
                continue;
            }
            ids[p][s] = list.len() as u32;
            let name = match (has_sides[p], s) {
                (false, _) => part_name(p),
                (true, 0) => format!("{} top", part_name(p)),
                (true, _) => format!("{} under", part_name(p)),
            };
            list.push(Section { name, triangles: n });
        }
    }
    let of_triangle = (0..count).map(|t| ids[part[t] as usize][usize::from(side[t])]).collect();
    Sections { of_triangle, list }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Vertex;

    /// Adds a closed box from `min` to `max`, as twelve triangles subdivided `n` times along X and Z faces.
    fn add_box(mesh: &mut Mesh, min: [f32; 3], max: [f32; 3], n: u32) {
        // Each of the six faces is a grid; corners are shared by position, so the graph welds them.
        let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([min[0], min[1], max[2]], [max[0] - min[0], 0.0, 0.0], [0.0, max[1] - min[1], 0.0]),
            ([max[0], min[1], min[2]], [min[0] - max[0], 0.0, 0.0], [0.0, max[1] - min[1], 0.0]),
            ([min[0], max[1], max[2]], [max[0] - min[0], 0.0, 0.0], [0.0, 0.0, min[2] - max[2]]),
            ([min[0], min[1], min[2]], [max[0] - min[0], 0.0, 0.0], [0.0, 0.0, max[2] - min[2]]),
            ([max[0], min[1], max[2]], [0.0, 0.0, min[2] - max[2]], [0.0, max[1] - min[1], 0.0]),
            ([min[0], min[1], min[2]], [0.0, 0.0, max[2] - min[2]], [0.0, max[1] - min[1], 0.0]),
        ];
        for (origin, u, v) in faces {
            let base = mesh.vertices.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let (a, b) = (i as f32 / n as f32, j as f32 / n as f32);
                    let position = [
                        origin[0] + u[0] * a + v[0] * b,
                        origin[1] + u[1] * a + v[1] * b,
                        origin[2] + u[2] * a + v[2] * b,
                    ];
                    mesh.vertices.push(Vertex { position, normal: [0.0, 1.0, 0.0], uv: [a, b] });
                }
            }
            let at = |i: u32, j: u32| base + j * (n + 1) + i;
            for j in 0..n {
                for i in 0..n {
                    mesh.indices.extend([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                    mesh.indices.extend([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
        }
    }

    fn section_at(mesh: &Mesh, found: &Sections, point: [f32; 3]) -> usize {
        let center = |t: usize| {
            let i = &mesh.indices[t * 3..t * 3 + 3];
            let p = |k: usize| mesh.vertices[i[k] as usize].position;
            [
                (p(0)[0] + p(1)[0] + p(2)[0]) / 3.0,
                (p(0)[1] + p(1)[1] + p(2)[1]) / 3.0,
                (p(0)[2] + p(1)[2] + p(2)[2]) / 3.0,
            ]
        };
        let best = (0..mesh.triangle_count())
            .min_by(|&a, &b| distance(center(a), point).total_cmp(&distance(center(b), point)))
            .unwrap();
        found.of_triangle[best] as usize
    }

    #[test]
    fn a_cross_of_flat_arms_gets_a_part_per_arm_with_two_sides() {
        let mut mesh = Mesh::default();
        // A connected cross of thin flat arms, built as one box per arm meeting at a shared hub box.
        add_box(&mut mesh, [-0.1, -0.02, -0.1], [0.1, 0.02, 0.1], 8);
        add_box(&mut mesh, [0.1, -0.02, -0.1], [1.0, 0.02, 0.1], 24);
        add_box(&mut mesh, [-1.0, -0.02, -0.1], [-0.1, 0.02, 0.1], 24);
        add_box(&mut mesh, [-0.1, -0.02, 0.1], [0.1, 0.02, 1.0], 24);
        add_box(&mut mesh, [-0.1, -0.02, -1.0], [0.1, 0.02, -0.1], 24);

        let found = find_sections(&mesh, [0.0, 1.0, 0.0]);
        assert_eq!(found.of_triangle.len(), mesh.triangle_count());
        let tops = [[0.7, 0.02, 0.0], [-0.7, 0.02, 0.0], [0.0, 0.02, 0.7], [0.0, 0.02, -0.7]]
            .map(|p| section_at(&mesh, &found, p));
        let unders = [[0.7, -0.02, 0.0], [-0.7, -0.02, 0.0], [0.0, -0.02, 0.7], [0.0, -0.02, -0.7]]
            .map(|p| section_at(&mesh, &found, p));
        let mut all: Vec<usize> = tops.iter().chain(&unders).copied().collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 8, "four arms with two sides each: {:?}", found.list);
        for s in tops {
            assert!(found.list[s].name.ends_with("top"), "{}", found.list[s].name);
        }
        for s in unders {
            assert!(found.list[s].name.ends_with("under"), "{}", found.list[s].name);
        }
    }

    #[test]
    fn a_loose_fragment_joins_the_nearest_part() {
        let mut mesh = Mesh::default();
        add_box(&mut mesh, [-1.0, -0.1, -0.1], [1.0, 0.1, 0.1], 16);
        let parts_alone = find_sections(&mesh, [0.0, 1.0, 0.0]).list.len();
        add_box(&mut mesh, [1.2, -0.01, -0.01], [1.22, 0.01, 0.01], 1);
        assert_eq!(find_sections(&mesh, [0.0, 1.0, 0.0]).list.len(), parts_alone);
    }

    #[test]
    fn an_empty_mesh_has_no_sections() {
        assert!(find_sections(&Mesh::default(), [0.0, 1.0, 0.0]).list.is_empty());
    }

    #[test]
    fn smallest_axis_finds_the_thin_direction() {
        let (axis, values) = smallest_axis([[4.0, 0.0, 0.0], [0.0, 0.1, 0.0], [0.0, 0.0, 2.0]]);
        assert!(axis[1].abs() > 0.999, "{axis:?}");
        assert!(values[0] > values[1] && values[1] > values[2]);
    }
}
