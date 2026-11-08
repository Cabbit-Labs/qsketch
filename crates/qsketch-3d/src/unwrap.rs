//! Rebuilds a mesh's texture layout so each section is one continuous island, and bakes the
//! existing texture onto the new layout. See `docs/Q3D.md`.
//!
//! Each section's connected pieces are flattened with least squares conformal maps (LSCM),
//! scaled to their true surface area, turned so they run lengthwise with the end nearest the
//! model's middle on the left, and packed in section order. Triangles keep their order and
//! their old UVs say exactly where each new pixel comes from, so baking needs no ray casting.

use std::collections::HashMap;

use crate::mesh::{Mesh, Vertex};
use crate::sections::Sections;

/// Empty pixels kept between islands, and painted outward from each so filtering does not bleed.
const PADDING_PIXELS: u32 = 8;
const BACKGROUND: [u8; 4] = [32, 32, 32, 255];
const MAX_SOLVER_STEPS: usize = 6000;
const SOLVER_TOLERANCE: f64 = 1e-7;
/// Pieces smaller than this share of the model's area keep to the end of the sheet.
const SMALL_PIECE: f64 = 0.005;
const RIGID_ITERATIONS: usize = 30;
const RIGID_SOLVER_STEPS: usize = 60;
/// Cotangent weights are clamped: lumpy meshes have sliver triangles that would otherwise wreck the solve.
const MIN_EDGE_WEIGHT: f64 = 0.01;
const MAX_EDGE_WEIGHT: f64 = 50.0;

pub struct Texture<'a> {
    pub width: u32,
    pub height: u32,
    /// RGBA8, top row first.
    pub rgba: &'a [u8],
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub islands: usize,
    /// Triangles turned over by flattening; each one is a small tear in the texture.
    pub flipped_triangles: usize,
    /// Pixels two triangles both claimed.
    pub overlapping_pixels: usize,
    /// How far triangles' texture area strays from their true area: 1 is perfect, 2 is twice as large or small.
    pub stretch_median: f32,
    pub stretch_p99: f32,
    pub texture_used: f32,
}

pub struct Rearranged {
    /// The same triangles in the same order, so existing sections still apply.
    pub mesh: Mesh,
    pub rgba: Vec<u8>,
    pub size: u32,
    pub report: Report,
}

type P3 = [f64; 3];

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: P3, b: P3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// A connected piece of a section, about to be laid flat.
struct Chart {
    section: u32,
    triangles: Vec<usize>,
    /// Position and original vertex of each chart vertex.
    points: Vec<P3>,
    source: Vec<u32>,
    /// Chart vertex numbers, three per triangle.
    corners: Vec<[u32; 3]>,
    uv: Vec<[f64; 2]>,
    area: f64,
    size: [f64; 2],
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let up = parent[parent[x as usize] as usize];
        parent[x as usize] = up;
        x = up;
    }
    x
}

fn build_charts(mesh: &Mesh, sections: &Sections, weld: &[u32]) -> Vec<Chart> {
    let triangle_count = mesh.triangle_count();
    let mut by_section: Vec<Vec<usize>> = vec![Vec::new(); sections.list.len().max(1)];
    for t in 0..triangle_count {
        by_section[sections.of_triangle[t] as usize].push(t);
    }

    let mut charts = Vec::new();
    for (section, triangles) in by_section.into_iter().enumerate() {
        // Pieces are found per section with welded vertices, so old UV seams do not split them.
        let mut number: HashMap<u32, u32> = HashMap::new();
        let mut parent: Vec<u32> = Vec::new();
        for &t in &triangles {
            let mut first = None;
            for k in 0..3 {
                let w = weld[mesh.indices[t * 3 + k] as usize];
                let next = number.len() as u32;
                let id = *number.entry(w).or_insert(next);
                if id == next {
                    parent.push(id);
                }
                match first {
                    None => first = Some(id),
                    Some(f) => {
                        let (a, b) = (find(&mut parent, f), find(&mut parent, id));
                        parent[a as usize] = b;
                    }
                }
            }
        }
        let mut grouped: HashMap<u32, Vec<usize>> = HashMap::new();
        for &t in &triangles {
            let w = weld[mesh.indices[t * 3] as usize];
            let root = find(&mut parent, number[&w]);
            grouped.entry(root).or_default().push(t);
        }
        let mut pieces: Vec<Vec<usize>> = grouped.into_values().collect();
        pieces.sort_by_key(|p| std::cmp::Reverse(p.len()));

        for tris in pieces {
            let mut local: HashMap<u32, u32> = HashMap::new();
            let (mut points, mut source, mut corners) = (Vec::new(), Vec::new(), Vec::with_capacity(tris.len()));
            for &t in &tris {
                let mut c = [0u32; 3];
                for (k, slot) in c.iter_mut().enumerate() {
                    let original = mesh.indices[t * 3 + k];
                    let next = local.len() as u32;
                    let id = *local.entry(weld[original as usize]).or_insert(next);
                    if id == next {
                        points.push(mesh.vertices[original as usize].position.map(f64::from));
                        source.push(original);
                    }
                    *slot = id;
                }
                corners.push(c);
            }
            let area = corners
                .iter()
                .map(|c| {
                    let n = cross(
                        sub(points[c[1] as usize], points[c[0] as usize]),
                        sub(points[c[2] as usize], points[c[0] as usize]),
                    );
                    dot(n, n).sqrt() * 0.5
                })
                .sum();
            let uv = vec![[0.0; 2]; points.len()];
            charts.push(Chart {
                section: section as u32,
                triangles: tris,
                points,
                source,
                corners,
                uv,
                area,
                size: [0.0; 2],
            });
        }
    }
    charts
}

/// The two boundary vertices farthest apart, which anchor the flattening.
fn choose_pins(chart: &Chart) -> (usize, usize) {
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for c in &chart.corners {
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    let mut boundary: Vec<usize> =
        edges.iter().filter(|(_, &n)| n == 1).flat_map(|(&(a, b), _)| [a as usize, b as usize]).collect();
    boundary.sort_unstable();
    boundary.dedup();
    if boundary.len() < 2 {
        boundary = (0..chart.points.len()).collect();
    }
    let far_from = |from: usize| {
        let mut best = (from, -1.0);
        for &b in &boundary {
            let d = sub(chart.points[b], chart.points[from]);
            let distance = dot(d, d);
            if distance > best.1 {
                best = (b, distance);
            }
        }
        best.0
    };
    let a = far_from(boundary[0]);
    (a, far_from(a))
}

/// One triangle's LSCM coefficients: `(real, imaginary)` for each corner.
fn lscm_terms(p: [P3; 3]) -> Option<[(f64, f64); 3]> {
    let ab = sub(p[1], p[0]);
    let ac = sub(p[2], p[0]);
    let length = dot(ab, ab).sqrt();
    let n = cross(ab, ac);
    let double_area = dot(n, n).sqrt();
    if length < 1e-12 || double_area < 1e-14 {
        return None;
    }
    let x2 = length;
    let x3 = dot(ac, ab) / length;
    let y3 = double_area / length;
    let root = double_area.sqrt();
    Some([((x3 - x2) / root, y3 / root), (-x3 / root, -y3 / root), (x2 / root, 0.0)])
}

type TriangleTerms = ([usize; 3], [(f64, f64); 3]);

/// Solves LSCM by Jacobi-preconditioned conjugate gradients, without assembling a matrix.
fn flatten(chart: &mut Chart) {
    let n = chart.points.len();
    if n < 3 {
        return;
    }
    let terms: Vec<TriangleTerms> = chart
        .corners
        .iter()
        .filter_map(|c| {
            let ids = c.map(|i| i as usize);
            lscm_terms(ids.map(|i| chart.points[i])).map(|t| (ids, t))
        })
        .collect();

    let (pin_a, pin_b) = choose_pins(chart);
    let chord = sub(chart.points[pin_a], chart.points[pin_b]);
    let mut pinned = vec![[0.0f64; 2]; n];
    pinned[pin_b] = [dot(chord, chord).sqrt().max(1e-9), 0.0];
    let is_pin = |i: usize| i == pin_a || i == pin_b;

    // A^T A applied to x, where x holds u then v for each vertex.
    let apply = |x: &[f64], out: &mut [f64]| {
        out.fill(0.0);
        for (ids, t) in &terms {
            let (mut re, mut im) = (0.0, 0.0);
            for k in 0..3 {
                let (u, v) = (x[ids[k] * 2], x[ids[k] * 2 + 1]);
                re += t[k].0 * u - t[k].1 * v;
                im += t[k].1 * u + t[k].0 * v;
            }
            for k in 0..3 {
                out[ids[k] * 2] += t[k].0 * re + t[k].1 * im;
                out[ids[k] * 2 + 1] += -t[k].1 * re + t[k].0 * im;
            }
        }
    };

    let mut diagonal = vec![0.0f64; n * 2];
    for (ids, t) in &terms {
        for k in 0..3 {
            let d = t[k].0 * t[k].0 + t[k].1 * t[k].1;
            diagonal[ids[k] * 2] += d;
            diagonal[ids[k] * 2 + 1] += d;
        }
    }
    let inverse: Vec<f64> = diagonal.iter().map(|&d| if d > 1e-20 { 1.0 / d } else { 0.0 }).collect();

    let mut fixed = vec![0.0f64; n * 2];
    for i in [pin_a, pin_b] {
        fixed[i * 2] = pinned[i][0];
        fixed[i * 2 + 1] = pinned[i][1];
    }
    let clear_pins = |v: &mut [f64]| {
        for i in [pin_a, pin_b] {
            v[i * 2] = 0.0;
            v[i * 2 + 1] = 0.0;
        }
    };

    // Start from a straight spread between the pins, which is close for long strips.
    let along = sub(chart.points[pin_b], chart.points[pin_a]);
    let along_length = dot(along, along).sqrt().max(1e-9);
    let mut x = vec![0.0f64; n * 2];
    for (i, p) in chart.points.iter().enumerate() {
        if !is_pin(i) {
            x[i * 2] = dot(sub(*p, chart.points[pin_a]), along) / along_length;
        }
    }

    let mut scratch = vec![0.0f64; n * 2];
    apply(&fixed, &mut scratch);
    let mut rhs: Vec<f64> = scratch.iter().map(|v| -v).collect();
    clear_pins(&mut rhs);

    apply(&x, &mut scratch);
    let mut r: Vec<f64> = rhs.iter().zip(&scratch).map(|(b, a)| b - a).collect();
    clear_pins(&mut r);
    let mut z: Vec<f64> = r.iter().zip(&inverse).map(|(a, b)| a * b).collect();
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let target = rhs.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-12) * SOLVER_TOLERANCE;
    let mut ap = vec![0.0f64; n * 2];
    for _ in 0..MAX_SOLVER_STEPS {
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() < target {
            break;
        }
        apply(&p, &mut ap);
        clear_pins(&mut ap);
        let curvature: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if curvature.abs() < 1e-30 {
            break;
        }
        let step = rz / curvature;
        for i in 0..n * 2 {
            x[i] += step * p[i];
            r[i] -= step * ap[i];
        }
        for (z, (r, inv)) in z.iter_mut().zip(r.iter().zip(&inverse)) {
            *z = r * inv;
        }
        let next: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = next / rz;
        rz = next;
        for (p, z) in p.iter_mut().zip(&z) {
            *p = z + beta * *p;
        }
    }

    for i in 0..n {
        chart.uv[i] = if is_pin(i) { pinned[i] } else { [x[i * 2], x[i * 2 + 1]] };
    }

    // Refine: LSCM preserves angles but piles scale up near its pins.
    let signed: f64 = chart.corners.iter().map(|c| signed_area(&chart.uv, c)).sum();
    let absolute: f64 = chart.corners.iter().map(|c| signed_area(&chart.uv, c).abs()).sum::<f64>().max(1e-18);
    let scale = (chart.area / absolute).sqrt();
    let mirror = if signed < 0.0 { -1.0 } else { 1.0 };
    for uv in &mut chart.uv {
        *uv = [uv[0] * scale, uv[1] * scale * mirror];
    }
    refine_rigidly(chart);
}

/// As-rigid-as-possible refinement (Liu et al. 2008): repeatedly fit each triangle's best rotation,
/// then solve for the layout closest to all of them.
fn refine_rigidly(chart: &mut Chart) {
    struct Tri {
        ids: [usize; 3],
        rest: [[f64; 2]; 3],
        cot: [f64; 3], // of the corner opposite each edge (k + 1, k + 2)
    }
    let mut tris = Vec::with_capacity(chart.corners.len());
    for c in &chart.corners {
        let ids = c.map(|i| i as usize);
        let [a, b, d] = ids.map(|i| chart.points[i]);
        let ab = sub(b, a);
        let ad = sub(d, a);
        let length = dot(ab, ab).sqrt();
        let n = cross(ab, ad);
        let double_area = dot(n, n).sqrt();
        if length < 1e-12 || double_area < 1e-14 {
            continue;
        }
        let rest = [[0.0, 0.0], [length, 0.0], [dot(ad, ab) / length, double_area / length]];
        let cot = [0, 1, 2].map(|k: usize| {
            let e1 = [rest[(k + 1) % 3][0] - rest[k][0], rest[(k + 1) % 3][1] - rest[k][1]];
            let e2 = [rest[(k + 2) % 3][0] - rest[k][0], rest[(k + 2) % 3][1] - rest[k][1]];
            let value = (e1[0] * e2[0] + e1[1] * e2[1]) / (e1[0] * e2[1] - e1[1] * e2[0]).abs().max(1e-12);
            value.clamp(MIN_EDGE_WEIGHT, MAX_EDGE_WEIGHT)
        });
        tris.push(Tri { ids, rest, cot });
    }
    let n = chart.points.len();
    if tris.is_empty() || n < 3 {
        return;
    }

    let mut pairs: Vec<(u32, u32, f64)> = Vec::with_capacity(tris.len() * 6);
    let mut diagonal = vec![0.0f64; n];
    for tri in &tris {
        for k in 0..3 {
            let (i, j) = (tri.ids[(k + 1) % 3], tri.ids[(k + 2) % 3]);
            let w = tri.cot[k] * 0.5;
            pairs.push((i as u32, j as u32, w));
            pairs.push((j as u32, i as u32, w));
            diagonal[i] += w;
            diagonal[j] += w;
        }
    }
    let apply = |x: &[f64], out: &mut [f64]| {
        for (o, (v, d)) in out.iter_mut().zip(x.iter().zip(&diagonal)) {
            *o = v * d;
        }
        for &(i, j, w) in &pairs {
            out[i as usize] -= w * x[j as usize];
        }
    };

    let mut rhs = vec![[0.0f64; 2]; n];
    let (mut r, mut z, mut p, mut ap, mut lu) = (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]);
    for _ in 0..RIGID_ITERATIONS {
        rhs.iter_mut().for_each(|v| *v = [0.0; 2]);
        for tri in &tris {
            let mut s = [0.0f64; 4];
            for k in 0..3 {
                let (i, j) = ((k + 1) % 3, (k + 2) % 3);
                let du = [
                    chart.uv[tri.ids[i]][0] - chart.uv[tri.ids[j]][0],
                    chart.uv[tri.ids[i]][1] - chart.uv[tri.ids[j]][1],
                ];
                let dx = [tri.rest[i][0] - tri.rest[j][0], tri.rest[i][1] - tri.rest[j][1]];
                s[0] += tri.cot[k] * du[0] * dx[0];
                s[1] += tri.cot[k] * du[0] * dx[1];
                s[2] += tri.cot[k] * du[1] * dx[0];
                s[3] += tri.cot[k] * du[1] * dx[1];
            }
            let angle = (s[2] - s[1]).atan2(s[0] + s[3]);
            let (sin, cos) = angle.sin_cos();
            for k in 0..3 {
                let (i, j) = ((k + 1) % 3, (k + 2) % 3);
                let dx = [tri.rest[i][0] - tri.rest[j][0], tri.rest[i][1] - tri.rest[j][1]];
                let target = [cos * dx[0] - sin * dx[1], sin * dx[0] + cos * dx[1]];
                let w = tri.cot[k] * 0.5;
                rhs[tri.ids[i]][0] += w * target[0];
                rhs[tri.ids[i]][1] += w * target[1];
                rhs[tri.ids[j]][0] -= w * target[0];
                rhs[tri.ids[j]][1] -= w * target[1];
            }
        }
        // Solve each axis for a correction from the current layout, holding vertex 0 still.
        for axis in 0..2 {
            let u: Vec<f64> = chart.uv.iter().map(|v| v[axis]).collect();
            apply(&u, &mut lu);
            for i in 0..n {
                r[i] = rhs[i][axis] - lu[i];
            }
            r[0] = 0.0;
            let mut d = vec![0.0f64; n];
            for i in 0..n {
                z[i] = r[i] / diagonal[i].max(1e-20);
                p[i] = z[i];
            }
            let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
            for _ in 0..RIGID_SOLVER_STEPS {
                apply(&p, &mut ap);
                ap[0] = 0.0;
                let curvature: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
                if curvature.abs() < 1e-30 {
                    break;
                }
                let step = rz / curvature;
                for i in 0..n {
                    d[i] += step * p[i];
                    r[i] -= step * ap[i];
                    z[i] = r[i] / diagonal[i].max(1e-20);
                }
                let next: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
                if next.abs() < 1e-20 {
                    break;
                }
                let beta = next / rz;
                rz = next;
                for (p, z) in p.iter_mut().zip(&z) {
                    *p = z + beta * *p;
                }
                p[0] = 0.0;
            }
            for (uv, d) in chart.uv.iter_mut().zip(&d) {
                uv[axis] += d;
            }
        }
    }
}

fn signed_area(uv: &[[f64; 2]], c: &[u32; 3]) -> f64 {
    let (a, b, d) = (uv[c[0] as usize], uv[c[1] as usize], uv[c[2] as usize]);
    ((b[0] - a[0]) * (d[1] - a[1]) - (b[1] - a[1]) * (d[0] - a[0])) * 0.5
}

/// Fixes handedness and scale, turns the chart lengthwise, puts its inner end on the left, and moves it to the origin.
fn normalize(chart: &mut Chart, model_center: P3) {
    let total: f64 = chart.corners.iter().map(|c| signed_area(&chart.uv, c)).sum();
    if total < 0.0 {
        for uv in &mut chart.uv {
            uv[1] = -uv[1];
        }
    }
    // Absolute areas: on a closed fragment the signed ones cancel out.
    let area = chart.corners.iter().map(|c| signed_area(&chart.uv, c).abs()).sum::<f64>().max(1e-18);
    let scale = (chart.area / area).sqrt();
    let n = chart.uv.len().max(1) as f64;
    let mean = chart.uv.iter().fold([0.0; 2], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
    let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
    for p in &chart.uv {
        let (dx, dy) = (p[0] - mean[0], p[1] - mean[1]);
        xx += dx * dx;
        yy += dy * dy;
        xy += dx * dy;
    }
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    let (sin, cos) = angle.sin_cos();
    for p in &mut chart.uv {
        let (dx, dy) = (p[0] - mean[0], p[1] - mean[1]);
        *p = [(dx * cos + dy * sin) * scale, (-dx * sin + dy * cos) * scale];
    }

    let distance_from_center: Vec<f64> = chart
        .points
        .iter()
        .map(|p| {
            let d = sub(*p, model_center);
            dot(d, d).sqrt()
        })
        .collect();
    let average = distance_from_center.iter().sum::<f64>() / n;
    let trend: f64 = chart.uv.iter().zip(&distance_from_center).map(|(p, d)| p[0] * (d - average)).sum();
    if trend < 0.0 {
        for p in &mut chart.uv {
            *p = [-p[0], -p[1]];
        }
    }

    let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &chart.uv {
        for a in 0..2 {
            min[a] = min[a].min(p[a]);
            max[a] = max[a].max(p[a]);
        }
    }
    for p in &mut chart.uv {
        *p = [p[0] - min[0], p[1] - min[1]];
    }
    chart.size = [max[0] - min[0], max[1] - min[1]];
}

/// Places rectangles left to right in rows, in order; `None` when they do not fit the unit square.
fn pack_rows(sizes: &[[f64; 2]], scale: f64, pad: f64) -> Option<Vec<[f64; 2]>> {
    let (mut x, mut y, mut row) = (pad, pad, 0.0f64);
    let mut places = Vec::with_capacity(sizes.len());
    for s in sizes {
        let (w, h) = (s[0] * scale, s[1] * scale);
        if x + w + pad > 1.0 {
            x = pad;
            y += row + pad;
            row = 0.0;
        }
        if x + w + pad > 1.0 || y + h + pad > 1.0 {
            return None;
        }
        places.push([x, y]);
        x += w + pad;
        row = row.max(h);
    }
    Some(places)
}

impl Texture<'_> {
    pub(crate) fn sample(&self, u: f64, v: f64) -> [u8; 4] {
        sample(self, u, v)
    }
}

fn sample(source: &Texture, u: f64, v: f64) -> [u8; 4] {
    let (w, h) = (source.width as i64, source.height as i64);
    let (x, y) = (u * w as f64 - 0.5, v * h as f64 - 0.5);
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let texel = |xi: i64, yi: i64, c: usize| {
        let at = (yi.clamp(0, h - 1) * w + xi.clamp(0, w - 1)) as usize * 4 + c;
        f64::from(source.rgba[at])
    };
    let (ix, iy) = (fx as i64, fy as i64);
    let mut out = [0u8; 4];
    for (c, value) in out.iter_mut().enumerate() {
        let top = texel(ix, iy, c) * (1.0 - tx) + texel(ix + 1, iy, c) * tx;
        let bottom = texel(ix, iy + 1, c) * (1.0 - tx) + texel(ix + 1, iy + 1, c) * tx;
        *value = (top * (1.0 - ty) + bottom * ty).round() as u8;
    }
    out
}

/// Paints each island's edge outward so texture filtering at its rim reads island color, not background.
fn dilate(rgba: &mut [u8], filled: &mut [bool], size: usize, passes: u32) {
    for _ in 0..passes {
        let before = filled.to_vec();
        let colors = rgba.to_vec();
        for y in 0..size {
            for x in 0..size {
                let at = y * size + x;
                if before[at] {
                    continue;
                }
                let (mut sum, mut count) = ([0u32; 4], 0u32);
                let mut add = |o: usize| {
                    if before[o] {
                        for (c, total) in sum.iter_mut().enumerate() {
                            *total += u32::from(colors[o * 4 + c]);
                        }
                        count += 1;
                    }
                };
                if x > 0 {
                    add(at - 1);
                }
                if x + 1 < size {
                    add(at + 1);
                }
                if y > 0 {
                    add(at - size);
                }
                if y + 1 < size {
                    add(at + size);
                }
                if let Some(divided) = sum.iter().map(|s| s.checked_div(count)).collect::<Option<Vec<u32>>>() {
                    for (c, value) in divided.iter().enumerate() {
                        rgba[at * 4 + c] = *value as u8;
                    }
                    filled[at] = true;
                }
            }
        }
    }
}

/// Rearranges `mesh`'s texture layout by `sections`, baking `source` to a `size` pixel square.
pub fn rearrange(mesh: &Mesh, sections: &Sections, source: &Texture, size: u32) -> Rearranged {
    let (center, radius) = mesh.bounds();
    let weld = crate::sections::weld_vertices(mesh, radius);
    let mut charts = build_charts(mesh, sections, &weld);
    let center = center.map(f64::from);
    for chart in &mut charts {
        flatten(chart);
        normalize(chart, center);
    }

    // Each section's main piece goes first, in section order, so a part's top sits by its underside.
    let total_area: f64 = charts.iter().map(|c| c.area).sum::<f64>().max(1e-18);
    let mut order: Vec<usize> = (0..charts.len()).collect();
    let is_small = |c: &Chart| c.area / total_area < SMALL_PIECE;
    order.sort_by(|&a, &b| {
        let (ca, cb) = (&charts[a], &charts[b]);
        is_small(ca).cmp(&is_small(cb)).then_with(|| {
            if is_small(ca) {
                cb.size[1].total_cmp(&ca.size[1])
            } else {
                ca.section.cmp(&cb.section).then(cb.area.total_cmp(&ca.area))
            }
        })
    });
    let sizes: Vec<[f64; 2]> = order.iter().map(|&c| charts[c].size).collect();
    let pad = f64::from(PADDING_PIXELS) * 2.0 / f64::from(size);
    let (mut low, mut high) = (0.0f64, 1.0 / sizes.iter().flatten().copied().fold(1e-9, f64::max));
    for _ in 0..40 {
        let middle = (low + high) / 2.0;
        if pack_rows(&sizes, middle, pad).is_some() {
            low = middle;
        } else {
            high = middle;
        }
    }
    let scale = low;
    let places = pack_rows(&sizes, scale, pad).unwrap_or_else(|| vec![[pad, pad]; sizes.len()]);

    // New vertices, one per chart vertex, and triangles back in their original order.
    let mut out = Mesh { vertices: Vec::new(), indices: vec![0; mesh.indices.len()] };
    let mut flipped = 0usize;
    let mut stretches = Vec::with_capacity(mesh.triangle_count());
    for (place, &c) in places.iter().zip(&order) {
        let chart = &charts[c];
        let base = out.vertices.len() as u32;
        for (i, uv) in chart.uv.iter().enumerate() {
            let original = mesh.vertices[chart.source[i] as usize];
            out.vertices.push(Vertex {
                position: original.position,
                normal: original.normal,
                uv: [(uv[0] * scale + place[0]) as f32, (uv[1] * scale + place[1]) as f32],
            });
        }
        for (&t, corners) in chart.triangles.iter().zip(&chart.corners) {
            for (k, corner) in corners.iter().enumerate() {
                out.indices[t * 3 + k] = base + corner;
            }
            let flat = signed_area(&chart.uv, corners);
            let n = cross(
                sub(chart.points[corners[1] as usize], chart.points[corners[0] as usize]),
                sub(chart.points[corners[2] as usize], chart.points[corners[0] as usize]),
            );
            let true_area = dot(n, n).sqrt() * 0.5;
            if flat <= 0.0 {
                flipped += 1;
            }
            if true_area > 1e-14 && flat > 0.0 {
                let ratio = flat / true_area;
                stretches.push(ratio.max(1.0 / ratio) as f32);
            }
        }
    }
    stretches.sort_by(f32::total_cmp);
    let percentile = |p: f64| stretches.get(((stretches.len().max(1) - 1) as f64 * p) as usize).copied().unwrap_or(1.0);

    // Bake: every new pixel reads the old texture through its own triangle's old UVs.
    let side = size as usize;
    let mut rgba = vec![0u8; side * side * 4];
    let mut filled = vec![false; side * side];
    let mut overlapping = 0usize;
    for t in 0..mesh.triangle_count() {
        let new =
            [0, 1, 2].map(|k| out.vertices[out.indices[t * 3 + k] as usize].uv.map(|c| f64::from(c) * f64::from(size)));
        let old = [0, 1, 2].map(|k| mesh.vertices[mesh.indices[t * 3 + k] as usize].uv.map(f64::from));
        let denominator =
            (new[1][1] - new[2][1]) * (new[0][0] - new[2][0]) + (new[2][0] - new[1][0]) * (new[0][1] - new[2][1]);
        if denominator.abs() < 1e-12 {
            continue;
        }
        let (min_x, max_x) =
            (new.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), new.iter().map(|p| p[0]).fold(0.0, f64::max));
        let (min_y, max_y) =
            (new.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min), new.iter().map(|p| p[1]).fold(0.0, f64::max));
        for y in (min_y.floor().max(0.0) as usize)..=(max_y.ceil() as usize).min(side - 1) {
            for x in (min_x.floor().max(0.0) as usize)..=(max_x.ceil() as usize).min(side - 1) {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let a = ((new[1][1] - new[2][1]) * (px - new[2][0]) + (new[2][0] - new[1][0]) * (py - new[2][1]))
                    / denominator;
                let b = ((new[2][1] - new[0][1]) * (px - new[2][0]) + (new[0][0] - new[2][0]) * (py - new[2][1]))
                    / denominator;
                let c = 1.0 - a - b;
                if a < 0.0 || b < 0.0 || c < 0.0 {
                    continue;
                }
                let at = y * side + x;
                if filled[at] {
                    overlapping += 1;
                }
                let color = sample(
                    source,
                    a * old[0][0] + b * old[1][0] + c * old[2][0],
                    a * old[0][1] + b * old[1][1] + c * old[2][1],
                );
                rgba[at * 4..at * 4 + 4].copy_from_slice(&color);
                filled[at] = true;
            }
        }
    }
    let used = filled.iter().filter(|&&f| f).count() as f32 / (side * side) as f32;
    dilate(&mut rgba, &mut filled, side, PADDING_PIXELS);
    for (pixel, &is_filled) in rgba.chunks_exact_mut(4).zip(&filled) {
        if !is_filled {
            pixel.copy_from_slice(&BACKGROUND);
        }
    }

    let report = Report {
        islands: charts.len(),
        flipped_triangles: flipped,
        overlapping_pixels: overlapping,
        stretch_median: percentile(0.5),
        stretch_p99: percentile(0.99),
        texture_used: used,
    };
    Rearranged { mesh: out, rgba, size, report }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sections::Section;

    /// A flat grid in the XZ plane with a UV layout scrambled per triangle.
    fn scrambled_grid(n: u32) -> Mesh {
        let mut mesh = Mesh::default();
        let mut cell = 0u32;
        for j in 0..n {
            for i in 0..n {
                let (x0, x1) = (i as f32 / n as f32 * 4.0, (i + 1) as f32 / n as f32 * 4.0);
                let (z0, z1) = (j as f32 / n as f32, (j + 1) as f32 / n as f32);
                let base = mesh.vertices.len() as u32;
                // Each cell's texture sits somewhere unrelated, so the old layout is useless to a neighbor.
                let u = ((cell * 7) % (n * n)) as f32 / (n * n) as f32;
                cell += 1;
                let w = 1.0 / (n * n) as f32;
                for (position, uv) in [
                    ([x0, 0.0, z0], [u, 0.0]),
                    ([x1, 0.0, z0], [u + w, 0.0]),
                    ([x1, 0.0, z1], [u + w, 1.0]),
                    ([x0, 0.0, z1], [u, 1.0]),
                ] {
                    mesh.vertices.push(Vertex { position, normal: [0.0, 1.0, 0.0], uv });
                }
                mesh.indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
            }
        }
        mesh
    }

    fn one_section(mesh: &Mesh) -> Sections {
        Sections {
            of_triangle: vec![0; mesh.triangle_count()],
            list: vec![Section { name: "All".into(), triangles: mesh.triangle_count() as u32 }],
        }
    }

    #[test]
    fn a_scrambled_grid_becomes_one_undistorted_island() {
        let mesh = scrambled_grid(10);
        let sections = one_section(&mesh);
        let rgba = vec![200u8; 16 * 16 * 4];
        let result = rearrange(&mesh, &sections, &Texture { width: 16, height: 16, rgba: &rgba }, 256);
        assert_eq!(result.report.islands, 1);
        assert_eq!(result.report.flipped_triangles, 0);
        assert_eq!(result.report.overlapping_pixels, 0);
        assert!(result.report.stretch_p99 < 1.05, "{:?}", result.report);
        // It is four times longer than wide, and should lie lengthwise.
        let (min_u, max_u) =
            result.mesh.vertices.iter().fold((1.0f32, 0.0f32), |m, v| (m.0.min(v.uv[0]), m.1.max(v.uv[0])));
        let (min_v, max_v) =
            result.mesh.vertices.iter().fold((1.0f32, 0.0f32), |m, v| (m.0.min(v.uv[1]), m.1.max(v.uv[1])));
        assert!((max_u - min_u) > 3.5 * (max_v - min_v), "u {min_u}..{max_u} v {min_v}..{max_v}");
    }

    #[test]
    fn triangles_keep_their_order_and_positions() {
        let mesh = scrambled_grid(4);
        let sections = one_section(&mesh);
        let rgba = vec![200u8; 16];
        let result = rearrange(&mesh, &sections, &Texture { width: 2, height: 2, rgba: &rgba }, 128);
        assert_eq!(result.mesh.indices.len(), mesh.indices.len());
        for (new, old) in result.mesh.indices.iter().zip(&mesh.indices) {
            assert_eq!(result.mesh.vertices[*new as usize].position, mesh.vertices[*old as usize].position);
        }
    }

    #[test]
    fn baking_carries_colors_through_old_uvs() {
        // One quad whose old UVs cover only the left, red, half of a red and blue texture.
        let mut mesh = scrambled_grid(1);
        for v in &mut mesh.vertices {
            v.uv[0] *= 0.25;
        }
        let rgba: Vec<u8> = (0..4).flat_map(|i| if i % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] }).collect();
        let sections = one_section(&mesh);
        let result = rearrange(&mesh, &sections, &Texture { width: 2, height: 2, rgba: &rgba }, 64);
        let reds = result.rgba.chunks_exact(4).filter(|p| p[0] > 200 && p[2] < 60).count();
        assert!(reds > 64, "{reds} red pixels");
        assert!(result.rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn pieces_that_do_not_touch_get_their_own_islands() {
        let mut mesh = scrambled_grid(3);
        let extra = scrambled_grid(3);
        let base = mesh.vertices.len() as u32;
        mesh.vertices
            .extend(extra.vertices.iter().map(|v| Vertex { position: [v.position[0], 5.0, v.position[2]], ..*v }));
        mesh.indices.extend(extra.indices.iter().map(|i| i + base));
        let sections = one_section(&mesh);
        let rgba = vec![200u8; 16];
        let result = rearrange(&mesh, &sections, &Texture { width: 2, height: 2, rgba: &rgba }, 128);
        assert_eq!(result.report.islands, 2);
        assert_eq!(result.report.overlapping_pixels, 0);
    }

    #[test]
    fn packing_refuses_what_does_not_fit() {
        assert!(pack_rows(&[[0.4, 0.4], [0.4, 0.4]], 1.0, 0.01).is_some());
        assert!(pack_rows(&[[2.0, 0.5]], 1.0, 0.01).is_none());
    }
}
