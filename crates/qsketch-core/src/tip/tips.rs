//! The built-in round brush tips: a graded family of soft rounds, falloff
//! curves, rings, and natural-media dots.
//!
//! A tip is an alpha mask sampled in the brush's own space, where the tile is
//! the square around a circle of radius 1. Every tip here is centered on the
//! middle of the tile (texel centers, so there is no half-pixel offset) and
//! falls to exactly 0 before the circle's rim, so a dab never shows the edge
//! of the square. They are 256 texels across: big enough that a brush of
//! several hundred pixels still samples a smooth gradient.

use std::f32::consts::TAU;

use super::field::{fbm, fbm_xy, smooth, white, Field};
use super::Rng;

type Gen = fn(usize, u64) -> Field;

pub struct Spec {
    pub name: &'static str,
    pub category: &'static str,
    pub size: usize,
    pub make: Gen,
}

impl Spec {
    pub fn build(&self) -> Field {
        let seed =
            self.name.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01B3));
        (self.make)(self.size, seed)
    }
}

macro_rules! spec {
    ($name:literal, $cat:literal, $make:ident) => {
        Spec { name: $name, category: $cat, size: TIP_SIZE, make: $make }
    };
}

/// Side of a round tip, in texels.
const TIP_SIZE: usize = 256;

/// The round tips, plain soft rounds first.
pub const SPECS: &[Spec] = &[
    spec!("Soft Round 0%", "Soft Rounds", soft_0),
    spec!("Soft Round 10%", "Soft Rounds", soft_10),
    spec!("Soft Round 25%", "Soft Rounds", soft_25),
    spec!("Soft Round 40%", "Soft Rounds", soft_40),
    spec!("Soft Round 60%", "Soft Rounds", soft_60),
    spec!("Soft Round 75%", "Soft Rounds", soft_75),
    spec!("Soft Round 90%", "Soft Rounds", soft_90),
    spec!("Hard Round AA", "Soft Rounds", hard_round),
    spec!("Linear Falloff", "Falloff Curves", linear_falloff),
    spec!("Quadratic Falloff", "Falloff Curves", quadratic_falloff),
    spec!("Cosine Falloff", "Falloff Curves", cosine_falloff),
    spec!("Quintic Falloff", "Falloff Curves", quintic_falloff),
    spec!("Gaussian Soft", "Falloff Curves", gaussian_soft),
    spec!("Gaussian Tight", "Falloff Curves", gaussian_tight),
    spec!("Airbrush Tail", "Falloff Curves", airbrush_tail),
    spec!("Glow", "Falloff Curves", glow),
    spec!("Dome", "Falloff Curves", dome),
    spec!("Core and Halo", "Rings and Halos", core_and_halo),
    spec!("Small Core Halo", "Rings and Halos", small_core_halo),
    spec!("Large Core Halo", "Rings and Halos", large_core_halo),
    spec!("Soft Ring", "Rings and Halos", soft_ring),
    spec!("Wide Ring", "Rings and Halos", wide_ring),
    spec!("Thin Ring", "Rings and Halos", thin_ring),
    spec!("Double Ring", "Rings and Halos", double_ring),
    spec!("Halo Dot", "Rings and Halos", halo_dot),
    spec!("Hollow Soft", "Rings and Halos", hollow_soft),
    spec!("Inverted Soft", "Rings and Halos", inverted_soft),
    spec!("Soft Charcoal Dot", "Natural Rounds", soft_charcoal_dot),
    spec!("Soft Pastel Dot", "Natural Rounds", soft_pastel_dot),
    spec!("Chalk Dot", "Natural Rounds", chalk_dot),
    spec!("Sand Round", "Natural Rounds", sand_round),
    spec!("Noisy Soft Round", "Natural Rounds", noisy_soft_round),
    spec!("Noisy Edge Round", "Natural Rounds", noisy_edge_round),
    spec!("Speckled Airbrush", "Natural Rounds", speckled_airbrush),
    spec!("Spray Dots", "Natural Rounds", spray_dots),
    spec!("Stipple Round", "Natural Rounds", stipple_round),
    spec!("Watercolor Bleed", "Natural Rounds", watercolor_bleed),
    spec!("Dry Brush Round", "Natural Rounds", dry_brush_round),
    spec!("Sponge Round", "Natural Rounds", sponge_round),
    spec!("Torn Edge Round", "Natural Rounds", torn_edge_round),
    spec!("Splatter Edge Round", "Natural Rounds", splatter_edge_round),
    spec!("Ink Blot Round", "Natural Rounds", ink_blot_round),
];

// ---- building blocks --------------------------------------------------------

/// Center of texel `(x, y)` in tip space: -1..1 across the tile, 0 at the
/// middle (which falls between texels, so the tile is symmetric).
fn tip_xy(n: usize, x: usize, y: usize) -> (f32, f32) {
    ((x as f32 + 0.5) / n as f32 * 2.0 - 1.0, (y as f32 + 0.5) / n as f32 * 2.0 - 1.0)
}

/// `f(r)` at every texel, `r` being the distance from the center in tip radii
/// (0 at the middle, 1 at the circle). Nothing outside the circle.
fn radial(n: usize, f: impl Fn(f32) -> f32) -> Field {
    Field::from_fn(n, |x, y| {
        let (nx, ny) = tip_xy(n, x, y);
        let r = (nx * nx + ny * ny).sqrt();
        if r >= 1.0 {
            0.0
        } else {
            f(r).clamp(0.0, 1.0)
        }
    })
}

/// Like [`radial`], also handing over the texel's index and tip-space position
/// so the closure can read noise fields. The result is faded to 0 across the
/// last few percent of the radius, so even a ragged edge stays inside the circle.
fn polar(n: usize, f: impl Fn(usize, f32, f32, f32) -> f32) -> Field {
    Field::from_fn(n, |x, y| {
        let (nx, ny) = tip_xy(n, x, y);
        let r = (nx * nx + ny * ny).sqrt();
        if r >= 1.0 {
            return 0.0;
        }
        f(y * n + x, r, nx, ny).clamp(0.0, 1.0) * (1.0 - smooth(0.93, 1.0, r))
    })
}

/// A flat core out to `hardness` of the radius, easing to nothing at the rim:
/// the same profile as the analytic round brush's hardness.
fn hardness_profile(r: f32, hardness: f32) -> f32 {
    if r <= hardness {
        1.0
    } else {
        1.0 - smooth(hardness, 1.0, r)
    }
}

/// A Gaussian of width `sigma`, shifted and scaled so it is 1 at the center
/// and exactly 0 at the rim.
fn gauss(r: f32, sigma: f32) -> f32 {
    let g = |r: f32| (-(r / sigma) * (r / sigma)).exp();
    (g(r) - g(1.0)) / (1.0 - g(1.0))
}

/// A bump of width `w` around radius `r0`, faded out before the rim.
fn ring(r: f32, r0: f32, w: f32) -> f32 {
    (-((r - r0) / w).powi(2)).exp() * (1.0 - smooth(0.9, 1.0, r))
}

/// Smooth 0..1 noise (bell-shaped), for edges and grain.
fn noise(n: usize, cells: usize, octaves: u32, gain: f32, seed: u64) -> Field {
    fbm(n, cells, octaves, gain, seed).equalize(1.0)
}

/// Fine grain 0..1 with an even spread.
fn grain(n: usize, seed: u64, soften: f32) -> Field {
    white(n, seed).blur(soften).equalize(1.0)
}

/// Dots over a base tip (max-blended, then softened a hair): `(x, y, radius,
/// strength)` with positions and radius in tip radii.
fn dots(base: Field, dots: &[(f32, f32, f32, f32)]) -> Field {
    let n = base.n;
    let half = n as f32 * 0.5;
    let mut out = base;
    for &(x, y, r, v) in dots {
        out.disc(half + x * half, half + y * half, (r * half).max(0.8), v, f32::max);
    }
    // A touch of blur, so a dot a texel or two wide has a soft rim, not a
    // step, then back inside the circle.
    let soft = out.blur(0.5);
    Field::from_fn(n, |x, y| {
        let (nx, ny) = tip_xy(n, x, y);
        soft.v[y * n + x] * (1.0 - smooth(0.96, 0.99, (nx * nx + ny * ny).sqrt()))
    })
}

/// A point at distance `d` from the center in a random direction.
fn at_radius(rng: &mut Rng, d: f32) -> (f32, f32) {
    let a = rng.unit() * TAU;
    (a.cos() * d, a.sin() * d)
}

// ---- soft rounds ------------------------------------------------------------

macro_rules! soft_round {
    ($fn:ident, $hardness:expr) => {
        fn $fn(n: usize, _: u64) -> Field {
            radial(n, |r| hardness_profile(r, $hardness))
        }
    };
}

soft_round!(soft_0, 0.0);
soft_round!(soft_10, 0.1);
soft_round!(soft_25, 0.25);
soft_round!(soft_40, 0.4);
soft_round!(soft_60, 0.6);
soft_round!(soft_75, 0.75);
soft_round!(soft_90, 0.9);

/// Full strength to a hair inside the rim, with a 1.5 texel antialiased edge.
fn hard_round(n: usize, _: u64) -> Field {
    let texel = 1.0 / (n as f32 * 0.5);
    radial(n, |r| 1.0 - smooth(1.0 - 2.5 * texel, 1.0 - texel, r))
}

// ---- falloff curves ---------------------------------------------------------

fn linear_falloff(n: usize, _: u64) -> Field {
    radial(n, |r| 1.0 - r)
}

fn quadratic_falloff(n: usize, _: u64) -> Field {
    radial(n, |r| (1.0 - r) * (1.0 - r))
}

fn cosine_falloff(n: usize, _: u64) -> Field {
    radial(n, |r| 0.5 + 0.5 * (std::f32::consts::PI * r).cos())
}

/// Smootherstep: flat at both ends, a little steeper in the middle than the
/// cubic of the soft rounds.
fn quintic_falloff(n: usize, _: u64) -> Field {
    radial(n, |r| 1.0 - r * r * r * (r * (r * 6.0 - 15.0) + 10.0))
}

fn gaussian_soft(n: usize, _: u64) -> Field {
    radial(n, |r| gauss(r, 0.42))
}

fn gaussian_tight(n: usize, _: u64) -> Field {
    radial(n, |r| gauss(r, 0.26))
}

/// A hot core in a wide, faint haze: two Gaussians, the way sprayed paint
/// really spreads.
fn airbrush_tail(n: usize, _: u64) -> Field {
    radial(n, |r| 0.6 * gauss(r, 0.16) + 0.4 * gauss(r, 0.5))
}

/// Inverse-square glow: a heavy tail that stays visible far from the center.
fn glow(n: usize, _: u64) -> Field {
    let g = |r: f32| 1.0 / (1.0 + (r / 0.16).powi(2));
    radial(n, |r| (g(r) - g(1.0)) / (1.0 - g(1.0)))
}

/// A sphere seen from above: gentle in the middle, dropping steeply at the
/// rim (which it reaches a texel early, so the drop stays inside the tile).
fn dome(n: usize, _: u64) -> Field {
    let edge = 1.0 - 1.0 / (n as f32 * 0.5);
    radial(n, |r| (1.0 - (r / edge).powi(2)).max(0.0).sqrt())
}

// ---- rings and halos --------------------------------------------------------

/// A solid core of `core` radius inside a halo that starts at `halo` strength
/// and fades to nothing at the rim.
fn core_halo(n: usize, core: f32, halo: f32) -> Field {
    radial(n, |r| {
        let solid = 1.0 - smooth(core, core * 1.5, r);
        let haze = halo * (1.0 - smooth(core * 0.5, 1.0, r));
        solid.max(haze)
    })
}

fn core_and_halo(n: usize, _: u64) -> Field {
    core_halo(n, 0.24, 0.38)
}

fn small_core_halo(n: usize, _: u64) -> Field {
    core_halo(n, 0.1, 0.3)
}

fn large_core_halo(n: usize, _: u64) -> Field {
    core_halo(n, 0.5, 0.22)
}

fn soft_ring(n: usize, _: u64) -> Field {
    radial(n, |r| ring(r, 0.62, 0.2))
}

fn wide_ring(n: usize, _: u64) -> Field {
    radial(n, |r| ring(r, 0.55, 0.3))
}

fn thin_ring(n: usize, _: u64) -> Field {
    radial(n, |r| ring(r, 0.8, 0.07))
}

fn double_ring(n: usize, _: u64) -> Field {
    radial(n, |r| ring(r, 0.4, 0.08).max(ring(r, 0.78, 0.09)))
}

fn halo_dot(n: usize, _: u64) -> Field {
    radial(n, |r| (1.0 - smooth(0.1, 0.22, r)).max(0.55 * ring(r, 0.72, 0.1)))
}

/// A soft disc that is thin in the middle: paint pools toward the rim.
fn hollow_soft(n: usize, _: u64) -> Field {
    radial(n, |r| (0.28 + 0.72 * smooth(0.05, 0.6, r)) * (1.0 - smooth(0.6, 1.0, r)))
}

/// The opposite of a soft round: clear in the middle, filling in toward the rim.
fn inverted_soft(n: usize, _: u64) -> Field {
    radial(n, |r| (1.0 - gauss(r, 0.36)) * (1.0 - smooth(0.78, 1.0, r)))
}

// ---- natural media ----------------------------------------------------------

/// Where the grain breaks the edge: 0 in the core, rising to 1 at the rim.
fn edge_weight(r: f32, from: f32, to: f32) -> f32 {
    smooth(from, to, r)
}

fn soft_charcoal_dot(n: usize, seed: u64) -> Field {
    let wobble = noise(n, 5, 3, 0.55, seed);
    let tooth = noise(n, 44, 3, 0.7, seed + 1).add(&grain(n, seed + 2, 0.7), 0.4).equalize(1.0);
    polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.2 * (wobble.v[i] - 0.5));
        let body = 1.0 - smooth(0.2, 0.84, reff);
        // Charcoal lifts off the paper at the edge, leaving holes in the tooth.
        let bite = smooth(0.35, 0.85, 1.0 - tooth.v[i]);
        body * (1.0 - edge_weight(reff, 0.2, 0.85) * 0.95 * bite) * (0.84 + 0.16 * tooth.v[i])
    })
}

fn soft_pastel_dot(n: usize, seed: u64) -> Field {
    let wobble = noise(n, 6, 3, 0.55, seed);
    let tooth = grain(n, seed + 1, 0.9).add(&noise(n, 50, 2, 0.6, seed + 2), 0.5).equalize(1.0);
    polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.1 * (wobble.v[i] - 0.5));
        let body = hardness_profile(reff, 0.35) * (1.0 - smooth(0.82, 0.92, reff));
        // Toward the edge the pastel catches only the peaks of the paper.
        let level = 0.9 * edge_weight(reff, 0.3, 0.9);
        let peaks = smooth(level - 0.12, level + 0.12, tooth.v[i]);
        body * (1.0 - edge_weight(reff, 0.3, 0.7) * (1.0 - peaks))
    })
}

fn chalk_dot(n: usize, seed: u64) -> Field {
    let wobble = noise(n, 8, 4, 0.6, seed);
    let dust = noise(n, 26, 3, 0.7, seed + 1).add(&grain(n, seed + 2, 0.6), 0.5).equalize(1.0);
    polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.12 * (wobble.v[i] - 0.5));
        let body = hardness_profile(reff, 0.62) * (1.0 - smooth(0.85, 0.93, reff));
        let level = 0.75 * edge_weight(reff, 0.35, 0.92);
        body * smooth(level - 0.1, level + 0.1, dust.v[i])
    })
}

/// A disc of fine sand grains, packed solid in the middle and thinning out.
fn sand_round(n: usize, seed: u64) -> Field {
    let g = grain(n, seed, 0.65);
    polar(n, |i, r, _, _| {
        let density = gauss(r, 0.55) * (1.0 - smooth(0.82, 0.97, r));
        1.0 - smooth(density - 0.05, density + 0.05, g.v[i])
    })
}

/// A soft round whose rim is chewed by fine noise, so it reads as soft but
/// never smooth.
fn noisy_edge_round(n: usize, seed: u64) -> Field {
    let rough = noise(n, 18, 5, 0.7, seed);
    let fine = grain(n, seed + 1, 0.6);
    polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.34 * (rough.v[i] - 0.5) + 0.12 * (fine.v[i] - 0.5));
        hardness_profile(reff, 0.2) * (1.0 - smooth(0.8, 0.9, reff))
    })
}

/// A smooth soft round under a grain, like an airbrush on rough paper.
fn noisy_soft_round(n: usize, seed: u64) -> Field {
    let g = grain(n, seed, 0.8);
    polar(n, |i, r, _, _| hardness_profile(r, 0.1) * (0.3 + 0.7 * g.v[i]))
}

fn speckled_airbrush(n: usize, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let mut specks = Vec::new();
    while specks.len() < 1100 {
        // Sprayed droplets cluster toward the middle.
        let (x, y) = (rng.signed(), rng.signed());
        let d = (x * x + y * y).sqrt();
        if d < 0.97 && rng.unit() < gauss(d, 0.5) {
            specks.push((x, y, 0.004 + rng.unit() * 0.006, 0.35 + 0.65 * rng.unit()));
        }
    }
    let haze = radial(n, |r| 0.55 * (0.6 * gauss(r, 0.2) + 0.4 * gauss(r, 0.5)));
    dots(haze, &specks)
}

fn spray_dots(n: usize, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let mut specks = Vec::new();
    while specks.len() < 3600 {
        let (x, y) = (rng.signed(), rng.signed());
        let d = (x * x + y * y).sqrt();
        if d < 0.97 && rng.unit() < gauss(d, 0.45) {
            specks.push((x, y, 0.004 + rng.unit().powi(2) * 0.012, 0.55 + 0.45 * rng.unit()));
        }
    }
    dots(Field::new(n, 0.0), &specks)
}

/// Dots on a jittered grid, dense in the middle and thinning to the rim.
fn stipple_round(n: usize, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let count = 54;
    let step = 2.0 / count as f32;
    let mut specks = Vec::new();
    for j in 0..count {
        for i in 0..count {
            let x = -1.0 + (i as f32 + 0.5 + (rng.unit() - 0.5) * 0.9) * step;
            let y = -1.0 + (j as f32 + 0.5 + (rng.unit() - 0.5) * 0.9) * step;
            let d = (x * x + y * y).sqrt();
            let keep = gauss(d, 0.6);
            if d < 0.96 && rng.unit() < keep {
                specks.push((x, y, 0.009 + rng.unit() * 0.011, 1.0));
            }
        }
    }
    dots(Field::new(n, 0.0), &specks)
}

fn watercolor_bleed(n: usize, seed: u64) -> Field {
    let outline = noise(n, 4, 4, 0.6, seed);
    let mottle = noise(n, 14, 4, 0.6, seed + 1);
    polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.22 * (outline.v[i] - 0.5));
        let body = 1.0 - smooth(0.74, 0.8, reff);
        // Pigment gathers in a darker band along the edge of the wash.
        let pooled = smooth(0.45, 0.78, reff);
        let feather = 0.1 * (1.0 - smooth(0.78, 0.96, reff));
        (body * (0.38 + 0.42 * pooled + 0.2 * mottle.v[i])).max(feather)
    })
}

fn dry_brush_round(n: usize, seed: u64) -> Field {
    let wobble = noise(n, 5, 3, 0.55, seed);
    // Bristle tracks along the tip's x axis, a few texels apart, running dry here and there.
    let streaks = noise_xy(n, 3, 84, seed + 1);
    polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.1 * (wobble.v[i] - 0.5));
        let body = hardness_profile(reff, 0.3) * (1.0 - smooth(0.86, 0.93, reff));
        body * (0.08 + 0.92 * smooth(0.18, 0.62, streaks.v[i]))
    })
}

/// Anisotropic noise, spread over 0..1.
fn noise_xy(n: usize, cx: usize, cy: usize, seed: u64) -> Field {
    fbm_xy(n, cx, cy, 2, 0.5, seed).equalize(1.0)
}

fn sponge_round(n: usize, seed: u64) -> Field {
    let wobble = noise(n, 5, 3, 0.55, seed);
    let pores = noise(n, 11, 5, 0.65, seed + 1);
    polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.14 * (wobble.v[i] - 0.5));
        let body = hardness_profile(reff, 0.45) * (1.0 - smooth(0.84, 0.93, reff));
        body * (0.2 + 0.8 * smooth(0.3, 0.7, pores.v[i]))
    })
}

fn torn_edge_round(n: usize, seed: u64) -> Field {
    let rough = noise(n, 12, 5, 0.7, seed);
    let mottle = noise(n, 18, 3, 0.5, seed + 1);
    polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.3 * (rough.v[i] - 0.5));
        (1.0 - smooth(0.73, 0.8, reff)) * (0.88 + 0.12 * mottle.v[i])
    })
}

fn splatter_edge_round(n: usize, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let wobble = noise(n, 7, 4, 0.6, seed + 1);
    let body = polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.1 * (wobble.v[i] - 0.5));
        hardness_profile(reff, 0.5) * (1.0 - smooth(0.7, 0.74, reff))
    });
    // Droplets thrown off the edge: bigger near the body, finer farther out.
    let mut drops = Vec::new();
    for _ in 0..260 {
        let d = 0.62 + 0.36 * rng.unit().powf(1.4);
        let size = (0.05 * (1.0 - (d - 0.62) / 0.36) + 0.005) * rng.unit().max(0.15);
        if d + size < 0.985 {
            let (x, y) = at_radius(&mut rng, d);
            drops.push((x, y, size, 0.8 + 0.2 * rng.unit()));
        }
    }
    dots(body, &drops)
}

fn ink_blot_round(n: usize, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let shape = noise(n, 3, 3, 0.6, seed + 1);
    let fine = noise(n, 14, 4, 0.7, seed + 2);
    let rim = 1.5 / (n as f32 * 0.5);
    let body = polar(n, |i, r, _, _| {
        let reff = r * (1.0 + 0.3 * (shape.v[i] - 0.5) + 0.07 * (fine.v[i] - 0.5));
        1.0 - smooth(0.7 - rim, 0.7, reff)
    });
    let mut drops = Vec::new();
    for _ in 0..16 {
        let d = 0.78 + 0.18 * rng.unit();
        let size = 0.012 + 0.03 * rng.unit() * (1.0 - (d - 0.78) / 0.18);
        let (x, y) = at_radius(&mut rng, d);
        drops.push((x, y, size, 1.0));
    }
    dots(body, &drops)
}
