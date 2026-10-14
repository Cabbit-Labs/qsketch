//! The built-in grain textures: a catalogue of natural-media surfaces.
//!
//! Each generator returns a tileable [`Field`] in 0..1 where light = paint
//! passes (the paper's raised tooth) and dark = paint is held back (pits,
//! gaps, grooves). Continuous-tone ones are histogram-equalized into a
//! bell-shaped spread, so they use the whole 0..1 range and the Depth slider
//! behaves alike for all of them.

use std::f32::consts::{PI, TAU};

use super::field::{cells, fbm, fbm_xy, hash01, smooth, white, Field};
use super::Rng;

type Gen = fn(usize, u64) -> Field;

/// One catalogue entry. The seed comes from the name, so adding or reordering
/// entries never changes how an existing texture looks.
pub struct Spec {
    pub name: &'static str,
    pub category: &'static str,
    /// Tile side in texels.
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
    ($name:literal, $cat:literal, $size:literal, $make:ident) => {
        Spec { name: $name, category: $cat, size: $size, make: $make }
    };
}

/// Every built-in texture, grouped by category in display order. Fine Grain,
/// Coarse Noise, Rough Paper, Canvas, Dots, Diagonal Lines, Crosshatch and
/// Concrete shipped first; brushes refer to textures by name, so none of them
/// may be renamed or dropped.
pub const SPECS: &[Spec] = &[
    spec!("Fine Grain", "Grain", 128, fine_grain),
    spec!("Coarse Noise", "Grain", 256, coarse_noise),
    spec!("Pencil Tooth", "Grain", 256, pencil_tooth),
    spec!("Sandpaper", "Grain", 256, sandpaper),
    spec!("Pastel Tooth", "Grain", 256, pastel_tooth),
    spec!("Charcoal Tooth", "Grain", 256, charcoal_tooth),
    spec!("Chalk Dust", "Grain", 256, chalk_dust),
    spec!("Crayon", "Grain", 256, crayon),
    spec!("Rough Paper", "Paper", 256, rough_paper),
    spec!("Cold Press", "Paper", 256, cold_press),
    spec!("Hot Press", "Paper", 256, hot_press),
    spec!("Watercolor Paper", "Paper", 256, watercolor_paper),
    spec!("Newsprint", "Paper", 256, newsprint),
    spec!("Toned Paper", "Paper", 256, toned_paper),
    spec!("Handmade Paper", "Paper", 256, handmade_paper),
    spec!("Rice Paper", "Paper", 256, rice_paper),
    spec!("Laid Paper", "Paper", 240, laid_paper),
    spec!("Canvas", "Fabric", 240, canvas),
    spec!("Fine Canvas", "Fabric", 240, fine_canvas),
    spec!("Coarse Canvas", "Fabric", 240, coarse_canvas),
    spec!("Linen", "Fabric", 240, linen),
    spec!("Burlap", "Fabric", 240, burlap),
    spec!("Twill", "Fabric", 240, twill),
    spec!("Mesh", "Fabric", 240, mesh),
    spec!("Watercolor Bloom", "Wet Media", 256, watercolor_bloom),
    spec!("Granulation", "Wet Media", 256, granulation),
    spec!("Wash", "Wet Media", 256, wash),
    spec!("Salt", "Wet Media", 256, salt),
    spec!("Sponge", "Wet Media", 256, sponge),
    spec!("Spray Speckle", "Wet Media", 256, spray_speckle),
    spec!("Dry Brush", "Brush Marks", 256, dry_brush),
    spec!("Bristle Streaks", "Brush Marks", 256, bristle_streaks),
    spec!("Brushed Metal", "Brush Marks", 256, brushed_metal),
    spec!("Scratches", "Brush Marks", 256, scratches),
    spec!("Dots", "Halftone", 128, dots),
    spec!("Halftone Fine", "Halftone", 240, halftone_fine),
    spec!("Halftone Medium", "Halftone", 240, halftone_medium),
    spec!("Halftone Coarse", "Halftone", 240, halftone_coarse),
    spec!("Halftone Grid", "Halftone", 240, halftone_grid),
    spec!("Halftone Lines", "Halftone", 240, halftone_lines),
    spec!("Stipple", "Halftone", 256, stipple_fine),
    spec!("Stipple Bold", "Halftone", 256, stipple_bold),
    spec!("Ordered Dither", "Halftone", 256, ordered_dither),
    spec!("Diagonal Lines", "Lines", 128, diagonal_lines),
    spec!("Crosshatch", "Lines", 128, crosshatch),
    spec!("Hatch Fine", "Lines", 240, hatch_fine),
    spec!("Hatch Wide", "Lines", 240, hatch_wide),
    spec!("Hand Hatch", "Lines", 256, hand_hatch),
    spec!("Crosshatch Fine", "Lines", 240, crosshatch_fine),
    spec!("Hand Crosshatch", "Lines", 256, hand_crosshatch),
    spec!("Scanlines", "Lines", 240, scanlines),
    spec!("Concrete", "Stone & Earth", 256, concrete),
    spec!("Granite", "Stone & Earth", 256, granite),
    spec!("Stucco", "Stone & Earth", 256, stucco),
    spec!("Gravel", "Stone & Earth", 256, gravel),
    spec!("Flagstone", "Stone & Earth", 256, flagstone),
    spec!("Cells", "Stone & Earth", 256, cell_web),
    spec!("Cracks", "Stone & Earth", 256, cracks),
    spec!("Craquelure", "Stone & Earth", 256, craquelure),
    spec!("Leather", "Stone & Earth", 256, leather),
    spec!("Wood Grain", "Surface", 256, wood_grain),
    spec!("Bark", "Surface", 256, bark),
    spec!("Hammered Metal", "Surface", 256, hammered_metal),
    spec!("Fibers", "Surface", 256, felt_fibers),
];

// ---- shared recipes ---------------------------------------------------------

/// Spread a field over 0..1 with a bell-shaped histogram (mostly mid-tones,
/// tails reaching both ends) and bend the curve: `gamma < 1` lifts the
/// midtones so more of the surface holds paint, `> 1` darkens them.
fn tone(f: Field, gamma: f32) -> Field {
    f.equalize(1.0).map(|u| ((1.0 - 2.0 * u).clamp(-1.0, 1.0).acos() / PI).powf(gamma))
}

/// Push a 0..1 field toward black and white around `mid`: a soft threshold.
fn crisp(f: Field, lo: f32, hi: f32) -> Field {
    f.map(|x| smooth(lo, hi, x))
}

/// Short, thin, randomly bent fibers (paper pulp). `len` and `width` are
/// ranges in texels; `curl` is how far each fiber may bend per segment.
fn fibers(n: usize, count: usize, len: (f32, f32), width: (f32, f32), curl: f32, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let mut out = Field::new(n, 0.0);
    for _ in 0..count {
        let (mut x, mut y) = (rng.unit() * n as f32, rng.unit() * n as f32);
        let mut a = rng.unit() * TAU;
        let total = len.0 + rng.unit() * (len.1 - len.0);
        let w = width.0 + rng.unit() * (width.1 - width.0);
        let v = 0.4 + rng.unit() * 0.6;
        let steps = 4;
        let step = total / steps as f32;
        for _ in 0..steps {
            let (nx, ny) = (x + a.cos() * step, y + a.sin() * step);
            out.segment((x, y), (nx, ny), w, v, f32::max);
            (x, y) = (nx, ny);
            a += rng.signed() * curl;
        }
    }
    out
}

/// Paper fibers as a soft, bounded field to mix into a noise: standardized
/// but clipped, so a few long fibers do not drown the rest of the surface.
fn pulp(n: usize, count: usize, len: (f32, f32), width: (f32, f32), curl: f32, seed: u64) -> Field {
    fibers(n, count, len, width, curl, seed).blur(0.6).standardize().map(|x| x.min(2.2))
}

/// Domed bumps: a Worley field turned into rounded lumps, softened so the
/// valleys between them are not creases.
fn lumps(n: usize, count: usize, jitter: f32, fullness: f32, soften: f32, seed: u64) -> Field {
    cells(n, count, jitter, seed).f1.map(|d| 1.0 - smooth(0.0, fullness, d)).blur(soften)
}

/// Scattered soft spots of varied size (many small, few large), as peak
/// values 0..1. Used for pits, bubbles and flecks.
fn spots(n: usize, count: usize, radius: (f32, f32), seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let mut out = Field::new(n, 0.0);
    for _ in 0..count {
        let (x, y) = (rng.unit() * n as f32, rng.unit() * n as f32);
        let r = radius.0 + (radius.1 - radius.0) * rng.unit().powi(3);
        out.disc(x, y, r, 0.35 + 0.65 * rng.unit(), f32::max);
    }
    out.blur(0.5)
}

/// Periodic 1-D value noise: `period` random values around the tile, an
/// independent set per `row`.
fn noise1d(t: f32, period: u32, row: u32, seed: u64) -> f32 {
    let i = t.floor();
    let f = t - i;
    let s = f * f * (3.0 - 2.0 * f);
    let at = |k: i64| hash01(k.rem_euclid(period as i64) as u32, row, seed);
    let a = at(i as i64);
    a + (at(i as i64 + 1) - a) * s
}

// ---- grain and the original analytic patterns --------------------------------

/// The very first texture, kept exactly as it shipped: white noise.
fn fine_grain(n: usize, _: u64) -> Field {
    let mut r = Rng::new(100);
    Field::from_fn(n, |_, _| (0.55 + r.signed() * 0.45).clamp(0.0, 1.0))
}

fn coarse_noise(n: usize, seed: u64) -> Field {
    tone(fbm(n, 32, 4, 0.5, seed), 1.0)
}

/// Texel center in the old `-1..1` tile space the analytic patterns were
/// written in.
fn unit_xy(n: usize, x: usize, y: usize) -> (f32, f32) {
    ((x as f32 + 0.5) / n as f32 * 2.0 - 1.0, (y as f32 + 0.5) / n as f32 * 2.0 - 1.0)
}

/// An 8 × 8 grid of round dots.
fn dots(n: usize, _: u64) -> Field {
    Field::from_fn(n, |x, y| {
        let (x, y) = unit_xy(n, x, y);
        let u = ((x * 0.5 + 0.5) * 8.0).fract() - 0.5;
        let v = ((y * 0.5 + 0.5) * 8.0).fract() - 0.5;
        ((0.3 - (u * u + v * v).sqrt()) / 0.06).clamp(0.0, 1.0)
    })
}

/// 12 diagonal stripes across the tile. (The first version took `fract` of
/// negative numbers and left half of the tile bare.)
fn diagonal_lines(n: usize, _: u64) -> Field {
    Field::from_fn(n, |x, y| {
        let (x, y) = unit_xy(n, x, y);
        let t = ((x + y) * 0.5 * 12.0).rem_euclid(1.0);
        ((0.5 - (t - 0.5).abs()) / 0.2).clamp(0.0, 1.0)
    })
}

fn crosshatch(n: usize, _: u64) -> Field {
    Field::from_fn(n, |x, y| {
        let (x, y) = unit_xy(n, x, y);
        let a = ((x + y) * 0.5 * 10.0).rem_euclid(1.0);
        let b = ((x - y) * 0.5 * 10.0).rem_euclid(1.0);
        let la = ((0.5 - (a - 0.5).abs()) / 0.18).clamp(0.0, 1.0);
        let lb = ((0.5 - (b - 0.5).abs()) / 0.18).clamp(0.0, 1.0);
        la.max(lb)
    })
}

// ---- dry grain --------------------------------------------------------------

fn pencil_tooth(n: usize, seed: u64) -> Field {
    let teeth = lumps(n, 80, 1.0, 0.9, 0.5, seed).standardize();
    let rolling = fbm(n, 40, 3, 0.6, seed + 1);
    let fine = white(n, seed + 2).blur(0.6).standardize();
    tone(teeth.add(&rolling, 0.5).add(&fine, 0.3), 0.95)
}

fn sandpaper(n: usize, seed: u64) -> Field {
    let grit = white(n, seed).blur(0.55).standardize();
    let grains = crisp(grit.normalize(), 0.5, 0.78);
    let haze = fbm(n, 24, 3, 0.5, seed + 1);
    tone(grains.standardize().add(&haze, 0.35), 1.0)
}

fn pastel_tooth(n: usize, seed: u64) -> Field {
    let sand = white(n, seed).blur(0.9).standardize();
    let clumps = fbm(n, 48, 3, 0.6, seed + 1);
    let cloud = fbm(n, 12, 3, 0.5, seed + 2);
    tone(sand.add(&clumps, 0.7).add(&cloud, 0.35), 1.15)
}

fn charcoal_tooth(n: usize, seed: u64) -> Field {
    let teeth = lumps(n, 34, 1.0, 0.85, 0.8, seed).standardize();
    let rough = fbm(n, 24, 5, 0.65, seed + 1);
    tone(teeth.add(&rough, 0.9), 1.0)
}

fn chalk_dust(n: usize, seed: u64) -> Field {
    let cloud = fbm(n, 9, 4, 0.55, seed);
    let grain = fbm(n, 90, 2, 0.7, seed + 1).add(&white(n, seed + 2).blur(0.5).standardize(), 0.6);
    crisp(tone(cloud.add(&grain.standardize(), 0.9), 1.0), 0.28, 0.72)
}

/// Wax skipping over paper tooth: more wax where the pressure is up, only the
/// peaks of the tooth where it is light.
fn crayon(n: usize, seed: u64) -> Field {
    let tooth = tone(fbm(n, 70, 3, 0.65, seed).add(&lumps(n, 64, 1.0, 0.9, 0.5, seed + 1).standardize(), 0.6), 1.0);
    let pressure = tone(fbm(n, 5, 3, 0.5, seed + 2), 1.0);
    Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let level = 0.62 - 0.5 * pressure.v[i];
        smooth(level - 0.07, level + 0.07, tooth.v[i])
    })
    .blur(0.4)
}

// ---- paper ------------------------------------------------------------------

fn rough_paper(n: usize, seed: u64) -> Field {
    let base = fbm(n, 16, 5, 0.6, seed);
    let tooth = fbm(n, 64, 2, 0.7, seed + 1);
    let grit = white(n, seed + 2).blur(0.6).standardize();
    tone(base.add(&tooth, 0.6).add(&grit, 0.35), 0.8)
}

fn cold_press(n: usize, seed: u64) -> Field {
    let bumps = lumps(n, 40, 0.9, 0.8, 0.8, seed).standardize();
    let rolling = fbm(n, 24, 4, 0.55, seed + 1);
    let fine = white(n, seed + 2).blur(0.7).standardize();
    tone(bumps.add(&rolling, 0.9).add(&fine, 0.3), 0.85)
}

fn hot_press(n: usize, seed: u64) -> Field {
    let fine = white(n, seed).blur(0.8).standardize();
    let tooth = fbm(n, 96, 2, 0.5, seed + 1);
    let swell = fbm(n, 6, 3, 0.5, seed + 2);
    tone(fine.add(&tooth, 0.7).add(&swell, 0.25), 0.9)
}

fn watercolor_paper(n: usize, seed: u64) -> Field {
    let warp_x = fbm(n, 6, 3, 0.5, seed + 3).equalize(1.0);
    let warp_y = fbm(n, 6, 3, 0.5, seed + 4).equalize(1.0);
    let bumps = lumps(n, 20, 1.0, 0.85, 1.6, seed).warped(&warp_x, &warp_y, 5.0).standardize();
    let rolling = fbm(n, 12, 5, 0.6, seed + 1);
    let fine = white(n, seed + 2).blur(0.6).standardize();
    tone(bumps.add(&rolling, 0.8).add(&fine, 0.25), 0.9)
}

fn newsprint(n: usize, seed: u64) -> Field {
    let fibers = pulp(n, 900, (5.0, 14.0), (0.7, 1.3), 0.5, seed);
    let mottle = fbm(n, 32, 4, 0.55, seed + 1);
    let grit = white(n, seed + 2).blur(0.5).standardize();
    tone(mottle.add(&fibers, 0.8).add(&grit, 0.45), 0.9)
}

fn toned_paper(n: usize, seed: u64) -> Field {
    let fibers = pulp(n, 260, (6.0, 16.0), (0.6, 1.0), 0.4, seed);
    let cloud = fbm(n, 10, 4, 0.5, seed + 1);
    let grit = white(n, seed + 2).blur(0.9).standardize();
    tone(cloud.add(&grit, 0.8).add(&fibers, 0.5), 0.9)
}

fn handmade_paper(n: usize, seed: u64) -> Field {
    let fibers = pulp(n, 700, (10.0, 28.0), (0.8, 1.6), 0.8, seed);
    let cloud = fbm(n, 8, 5, 0.6, seed + 1);
    let bumps = lumps(n, 22, 1.0, 0.9, 1.0, seed + 2).standardize();
    tone(cloud.add(&fibers, 0.9).add(&bumps, 0.5), 0.9)
}

fn rice_paper(n: usize, seed: u64) -> Field {
    let long = pulp(n, 220, (24.0, 70.0), (0.7, 1.4), 0.35, seed);
    let short = pulp(n, 500, (6.0, 16.0), (0.5, 0.9), 0.5, seed + 1);
    let cloud = fbm(n, 14, 3, 0.5, seed + 2);
    tone(cloud.add(&long, 0.9).add(&short, 0.5), 1.0)
}

fn laid_paper(n: usize, seed: u64) -> Field {
    let tooth = fbm(n, 60, 3, 0.55, seed);
    let cloud = fbm(n, 10, 3, 0.5, seed + 1);
    let chain = n as f32 / 8.0;
    let laid = Field::from_fn(n, |x, y| {
        // Fine laid lines every 3 texels, a chain line every `chain`.
        let l = 0.5 + 0.5 * (y as f32 / 3.0 * TAU).cos();
        let c = 0.5 + 0.5 * (x as f32 / chain * TAU).cos();
        l * 0.8 + c.powi(12) * 1.4
    })
    .standardize();
    tone(tooth.add(&cloud, 0.4).add(&laid, 0.4), 0.95)
}

// ---- fabric -----------------------------------------------------------------

/// One thread's profile across its width: 1 on the crown, 0 at the edges,
/// with `gap` of the width left empty between neighbors.
fn crown(f: f32, gap: f32) -> f32 {
    let c = 1.0 - (2.0 * f - 1.0).powi(2);
    ((c - gap) / (1.0 - gap)).max(0.0).sqrt()
}

/// Which thread is on top at crossing `(i, j)`: true = the warp (vertical).
type Interlace = fn(i64, i64) -> bool;

fn plain(i: i64, j: i64) -> bool {
    (i + j) % 2 == 0
}

fn twill_2_2(i: i64, j: i64) -> bool {
    (i + j).rem_euclid(4) < 2
}

struct Weave {
    /// Texels per thread.
    pitch: f32,
    /// Share of a thread's width left empty (0 = threads touch).
    gap: f32,
    /// Thread-to-thread thickness variation, 0..1.
    uneven: f32,
    /// Slubs: thick/thin places along each thread, 0..1.
    slub: f32,
    interlace: Interlace,
}

fn weave(n: usize, w: Weave, seed: u64) -> Field {
    let threads = (n as f32 / w.pitch).round() as u32;
    let mut grain = Rng::new(seed + 1);
    let f = Field::from_fn(n, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / w.pitch, (y as f32 + 0.5) / w.pitch);
        let (i, j) = (u.floor() as i64, v.floor() as i64);
        let (fx, fy) = (u - i as f32, v - j as f32);
        let (iw, jw) = (i.rem_euclid(threads as i64) as u32, j.rem_euclid(threads as i64) as u32);
        let over_warp = (w.interlace)(i, j);
        // Warp thread (vertical) i and weft thread (horizontal) j.
        let thick_w = 1.0 - w.uneven * hash01(iw, 1, seed);
        let thick_f = 1.0 - w.uneven * hash01(jw, 2, seed);
        // Slubs: thick and thin places along each thread, a few per tile.
        let slub_w = 1.0 + w.slub * (noise1d(v / threads as f32 * 24.0, 24, iw, seed + 3) - 0.5) * 2.0;
        let slub_f = 1.0 + w.slub * (noise1d(u / threads as f32 * 24.0, 24, jw, seed + 4) - 0.5) * 2.0;
        // The thread on top rides high over the crossing; the one below dips.
        let lift = |over: bool, t: f32| if over { 0.62 + 0.38 * (PI * t).sin() } else { 0.18 + 0.12 * (PI * t).sin() };
        let hw = crown(fx, w.gap) * thick_w * slub_w.clamp(0.0, 1.4) * lift(over_warp, fy);
        let hf = crown(fy, w.gap) * thick_f * slub_f.clamp(0.0, 1.4) * lift(!over_warp, fx);
        hw.max(hf) * (1.0 + (grain.unit() - 0.5) * 0.12)
    });
    tone(f, 1.0)
}

fn canvas(n: usize, seed: u64) -> Field {
    weave(n, Weave { pitch: 4.0, gap: 0.08, uneven: 0.12, slub: 0.12, interlace: plain }, seed)
}

fn fine_canvas(n: usize, seed: u64) -> Field {
    weave(n, Weave { pitch: 3.0, gap: 0.05, uneven: 0.1, slub: 0.1, interlace: plain }, seed)
}

fn coarse_canvas(n: usize, seed: u64) -> Field {
    weave(n, Weave { pitch: 8.0, gap: 0.1, uneven: 0.2, slub: 0.2, interlace: plain }, seed)
}

fn linen(n: usize, seed: u64) -> Field {
    weave(n, Weave { pitch: 5.0, gap: 0.04, uneven: 0.3, slub: 0.55, interlace: plain }, seed)
}

fn burlap(n: usize, seed: u64) -> Field {
    weave(n, Weave { pitch: 12.0, gap: 0.6, uneven: 0.4, slub: 0.45, interlace: plain }, seed)
}

fn twill(n: usize, seed: u64) -> Field {
    weave(n, Weave { pitch: 4.0, gap: 0.04, uneven: 0.08, slub: 0.1, interlace: twill_2_2 }, seed)
}

fn mesh(n: usize, seed: u64) -> Field {
    weave(n, Weave { pitch: 6.0, gap: 0.62, uneven: 0.1, slub: 0.05, interlace: plain }, seed)
}

// ---- wet media --------------------------------------------------------------

fn watercolor_bloom(n: usize, seed: u64) -> Field {
    let wx = fbm(n, 4, 3, 0.5, seed + 1).equalize(1.0);
    let wy = fbm(n, 4, 3, 0.5, seed + 2).equalize(1.0);
    let pool = tone(fbm(n, 4, 4, 0.55, seed).warped(&wx, &wy, 16.0), 1.0);
    // Pigment pushed outward piles up in a darker rim around each bloom.
    let rim = pool.clone().map(|p| (-((p - 0.5) / 0.035).powi(2)).exp());
    let step = pool.clone().map(|p| smooth(0.47, 0.53, p));
    let paper = fbm(n, 70, 3, 0.6, seed + 3);
    pool.mix(&step, 0.45).add(&rim, 0.35).add(&paper, 0.04).normalize()
}

fn granulation(n: usize, seed: u64) -> Field {
    let grains = fbm(n, 72, 3, 0.7, seed);
    let clumps = fbm(n, 9, 3, 0.5, seed + 1);
    let fine = white(n, seed + 2).blur(0.7).standardize();
    tone(grains.add(&clumps, 0.7).add(&fine, 0.4), 1.0)
}

fn wash(n: usize, seed: u64) -> Field {
    let streaks = fbm_xy(n, 3, 9, 4, 0.55, seed);
    let cloud = fbm(n, 4, 4, 0.5, seed + 1);
    tone(streaks.add(&cloud, 0.8), 1.0)
}

/// Salt on a wet wash: each grain pushes the pigment away, leaving a pale,
/// feathered star where paint is held back.
fn salt(n: usize, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let mut cleared = Field::new(n, 0.0);
    for _ in 0..70 {
        let (cx, cy) = (rng.unit() * n as f32, rng.unit() * n as f32);
        let core = 1.6 + rng.unit() * 2.2;
        let reach = core * (3.0 + rng.unit() * 2.0);
        let arms = (5 + (rng.unit() * 4.0) as i32) as f32;
        let phase = rng.unit() * TAU;
        cleared.stamp(
            (cx - reach - 1.0, cy - reach - 1.0),
            (cx + reach + 1.0, cy + reach + 1.0),
            |px, py| {
                let (dx, dy) = (px - cx, py - cy);
                let d = (dx * dx + dy * dy).sqrt();
                // The reach swells along the arms of the star.
                let spike = (arms * dy.atan2(dx) + phase).cos().abs().powi(2);
                let halo = (1.0 - d / (reach * (0.35 + 0.65 * spike))).clamp(0.0, 1.0);
                let kern = (core - d + 0.5).clamp(0.0, 1.0);
                (halo.powf(0.8) * 0.8).max(kern)
            },
            f32::max,
        );
    }
    let wash = tone(fbm(n, 10, 4, 0.55, seed + 1), 1.0).map(|x| 0.55 + 0.45 * x);
    wash.combine(&cleared, |w, c| w * (1.0 - c))
}

fn sponge(n: usize, seed: u64) -> Field {
    let blobs = tone(fbm(n, 10, 5, 0.65, seed), 1.0);
    let holes = spots(n, 700, (0.8, 3.4), seed + 1);
    crisp(blobs, 0.3, 0.7).combine(&holes, |b, h| b * (1.0 - 0.85 * h)).blur(0.5)
}

/// Dots of spray paint: tiny and many, bunched into clouds.
fn spray_speckle(n: usize, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let density = tone(fbm(n, 6, 3, 0.5, seed + 1), 1.0);
    let mut out = Field::new(n, 0.0);
    let mut placed = 0;
    while placed < 5200 {
        let (x, y) = (rng.unit() * n as f32, rng.unit() * n as f32);
        if rng.unit() > 0.2 + 0.8 * density.at(x as i64, y as i64) {
            continue;
        }
        placed += 1;
        let r = 0.45 + rng.unit().powi(3) * 1.5;
        out.disc(x, y, r, 0.5 + 0.5 * rng.unit(), f32::max);
    }
    out
}

// ---- brush marks ------------------------------------------------------------

/// Horizontal bristle tracks: each row of texels is a bristle with its own
/// load of paint, running out here and there along the stroke. `breaks` is
/// how many dry gaps a track has across the tile.
fn bristles(n: usize, breaks: u32, ragged: f32, seed: u64) -> Field {
    let f = Field::from_fn(n, |x, y| {
        let t = x as f32 / n as f32;
        let row = y as u32;
        let load = hash01(row, 9, seed);
        // Two scales of dry spots along the track.
        let run = noise1d(t * breaks as f32, breaks, row, seed + 1) * 0.65
            + noise1d(t * (breaks * 3) as f32, breaks * 3, row, seed + 2) * 0.35;
        load * (1.0 - ragged) + run * ragged
    });
    // Rows next to each other bleed a little, as real bristles overlap.
    let soft = f.blur(0.45);
    tone(soft, 1.0)
}

fn dry_brush(n: usize, seed: u64) -> Field {
    bristles(n, 5, 0.75, seed)
}

fn bristle_streaks(n: usize, seed: u64) -> Field {
    bristles(n, 3, 0.35, seed)
}

fn brushed_metal(n: usize, seed: u64) -> Field {
    let f = Field::from_fn(n, |x, y| {
        let t = x as f32 / n as f32;
        let row = y as u32;
        0.5 * noise1d(t * 6.0, 6, row, seed)
            + 0.3 * noise1d(t * 22.0, 22, row, seed + 1)
            + 0.2 * hash01(x as u32, row, seed + 2)
    });
    let fine = fbm(n, 4, 2, 0.5, seed + 3);
    tone(f.blur(0.4).standardize().add(&fine, 0.2), 1.0)
}

fn scratches(n: usize, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let mut out = Field::new(n, 0.0);
    for _ in 0..150 {
        let (x, y) = (rng.unit() * n as f32, rng.unit() * n as f32);
        let a = rng.unit() * PI;
        let len = 12.0 + rng.unit().powi(2) * 90.0;
        let w = 0.8 + rng.unit() * 0.8;
        let v = 0.45 + rng.unit() * 0.55;
        // A scratch wobbles a little: two joined segments.
        let bend = a + rng.signed() * 0.12;
        let mid = (x + a.cos() * len * 0.5, y + a.sin() * len * 0.5);
        out.segment((x, y), mid, w, v, f32::max);
        out.segment(mid, (mid.0 + bend.cos() * len * 0.5, mid.1 + bend.sin() * len * 0.5), w, v, f32::max);
    }
    let haze = tone(fbm(n, 12, 3, 0.5, seed + 1), 1.0);
    out.add(&haze, 0.16).normalize()
}

// ---- halftone and stipple ---------------------------------------------------

/// Round dots on a square grid of `pitch` texels (`pitch` divides `n`).
fn dot_grid(n: usize, pitch: f32, coverage: f32) -> Field {
    let r = pitch * (coverage / PI).sqrt();
    Field::from_fn(n, |x, y| {
        let fx = ((x as f32 + 0.5) / pitch).rem_euclid(1.0) - 0.5;
        let fy = ((y as f32 + 0.5) / pitch).rem_euclid(1.0) - 0.5;
        let d = (fx * fx + fy * fy).sqrt() * pitch;
        (r - d + 0.5).clamp(0.0, 1.0)
    })
}

/// Round dots on a 45° grid, `s` texels between neighboring rows and columns
/// of the grid's own axes (`2 s` divides `n`): the classic newsprint screen.
fn dot_screen_45(n: usize, s: f32, coverage: f32) -> Field {
    let pitch = s * 2.0f32.sqrt();
    let r = pitch * (coverage / PI).sqrt();
    Field::from_fn(n, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let (u, v) = ((px + py) / (2.0 * s), (px - py) / (2.0 * s));
        let (du, dv) = (u - u.round(), v - v.round());
        let d = (du * du + dv * dv).sqrt() * pitch;
        (r - d + 0.5).clamp(0.0, 1.0)
    })
}

fn halftone_fine(n: usize, _: u64) -> Field {
    dot_screen_45(n, 3.0, 0.5)
}

fn halftone_medium(n: usize, _: u64) -> Field {
    dot_screen_45(n, 5.0, 0.5)
}

fn halftone_coarse(n: usize, _: u64) -> Field {
    dot_screen_45(n, 10.0, 0.5)
}

fn halftone_grid(n: usize, _: u64) -> Field {
    dot_grid(n, 8.0, 0.5)
}

/// A 45° line screen, lines `m` texels apart along x + y, half covered.
fn halftone_lines(n: usize, _: u64) -> Field {
    let m = 8.0;
    let spacing = m / 2.0f32.sqrt();
    Field::from_fn(n, |x, y| {
        let t = ((x as f32 + y as f32 + 1.0) / m).rem_euclid(1.0);
        let d = (t - 0.5).abs() * spacing;
        (spacing * 0.25 - d + 0.5).clamp(0.0, 1.0)
    })
}

/// Dots on a jittered grid (blue-noise-like, never touching), bunched or
/// thinned by a slow density cloud, as in hand stippling.
fn stipple(n: usize, count: usize, jitter: f32, radius: (f32, f32), clumping: f32, seed: u64) -> Field {
    let mut rng = Rng::new(seed);
    let density = tone(fbm(n, 5, 3, 0.5, seed + 1), 1.0);
    let cell = n as f32 / count as f32;
    let mut out = Field::new(n, 0.0);
    for j in 0..count {
        for i in 0..count {
            let x = (i as f32 + 0.5 + (rng.unit() - 0.5) * jitter) * cell;
            let y = (j as f32 + 0.5 + (rng.unit() - 0.5) * jitter) * cell;
            let keep = 1.0 - clumping + clumping * density.at(x as i64, y as i64) * 1.6;
            let r = radius.0 + (radius.1 - radius.0) * rng.unit();
            if rng.unit() < keep {
                out.disc(x, y, r, 1.0, f32::max);
            }
        }
    }
    out
}

fn stipple_fine(n: usize, seed: u64) -> Field {
    stipple(n, 64, 0.85, (0.7, 1.4), 0.55, seed)
}

fn stipple_bold(n: usize, seed: u64) -> Field {
    stipple(n, 24, 0.7, (1.6, 3.2), 0.5, seed)
}

/// 8 × 8 Bayer threshold matrix as a texture: with the Height mode Depth
/// slider sweeping the dither pattern from empty to full.
fn ordered_dither(n: usize, _: u64) -> Field {
    fn bayer(x: usize, y: usize, size: usize) -> usize {
        if size == 1 {
            return 0;
        }
        let h = size / 2;
        4 * bayer(x % h, y % h, h) + [[0, 2], [3, 1]][y / h][x / h]
    }
    Field::from_fn(n, |x, y| (bayer(x % 8, y % 8, 8) as f32 + 0.5) / 64.0)
}

// ---- hatching ---------------------------------------------------------------

/// Parallel lines whose direction is the integer vector `(a, b)`: the phase
/// `a·u + b·v` (u, v across the tile in 0..1) completes a whole number of
/// cycles over the tile, so the tile wraps cleanly. `wobble` bends the lines
/// by up to `amount` cycles, `thicken` swells them, `breaks` gaps them.
struct Hatch<'a> {
    a: i32,
    b: i32,
    width: f32,
    wobble: Option<(&'a Field, f32)>,
    thicken: Option<(&'a Field, f32)>,
    breaks: Option<&'a Field>,
}

fn hatch(n: usize, h: Hatch) -> Field {
    let spacing = n as f32 / ((h.a * h.a + h.b * h.b) as f32).sqrt();
    Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let (u, v) = ((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
        let mut phase = h.a as f32 * u + h.b as f32 * v;
        if let Some((w, amt)) = h.wobble {
            phase += (w.v[i] - 0.5) * 2.0 * amt;
        }
        let d = (phase.rem_euclid(1.0) - 0.5).abs() * spacing;
        let mut half = h.width * 0.5;
        if let Some((t, amt)) = h.thicken {
            half *= 1.0 + (t.v[i] - 0.5) * 2.0 * amt;
        }
        let c = (half - d + 0.5).clamp(0.0, 1.0);
        c * h.breaks.map_or(1.0, |b| b.v[i])
    })
}

fn plain_hatch(n: usize, a: i32, b: i32, width: f32) -> Field {
    hatch(n, Hatch { a, b, width, wobble: None, thicken: None, breaks: None })
}

fn hatch_fine(n: usize, _: u64) -> Field {
    plain_hatch(n, 34, 34, 1.5)
}

fn hatch_wide(n: usize, _: u64) -> Field {
    plain_hatch(n, 17, 17, 3.4)
}

/// A hand-drawn set of strokes in direction `(a, b)`: wavering, uneven in
/// weight and lifting off the paper now and then.
fn hand_lines(n: usize, a: i32, b: i32, width: f32, seed: u64) -> Field {
    let sway = tone(fbm(n, 5, 3, 0.55, seed), 1.0);
    let thicken = tone(fbm(n, 16, 2, 0.5, seed + 1), 1.0);
    let lift = tone(fbm(n, 22, 2, 0.55, seed + 2), 1.0).map(|x| smooth(0.03, 0.17, x));
    hatch(n, Hatch { a, b, width, wobble: Some((&sway, 0.12)), thicken: Some((&thicken, 0.5)), breaks: Some(&lift) })
}

fn hand_hatch(n: usize, seed: u64) -> Field {
    hand_lines(n, 28, 16, 2.2, seed)
}

fn crosshatch_fine(n: usize, _: u64) -> Field {
    plain_hatch(n, 34, 34, 1.3).combine(&plain_hatch(n, 34, -34, 1.3), f32::max)
}

fn hand_crosshatch(n: usize, seed: u64) -> Field {
    hand_lines(n, 28, 16, 2.0, seed).combine(&hand_lines(n, 28, -16, 2.0, seed + 100), f32::max)
}

fn scanlines(n: usize, _: u64) -> Field {
    plain_hatch(n, 0, 80, 1.2)
}

// ---- stone and earth --------------------------------------------------------

fn concrete(n: usize, seed: u64) -> Field {
    let base = fbm(n, 12, 5, 0.6, seed);
    let fine = white(n, seed + 1).blur(0.6).standardize();
    let holes = spots(n, 380, (0.6, 2.6), seed + 2);
    tone(base.add(&fine, 0.3), 1.0).combine(&holes, |a, p| a * (1.0 - 0.9 * p))
}

fn granite(n: usize, seed: u64) -> Field {
    // Three minerals: dark mica and pale quartz fleck a mid feldspar ground.
    let a = tone(fbm(n, 56, 3, 0.65, seed), 1.0);
    let b = tone(fbm(n, 44, 3, 0.65, seed + 1), 1.0);
    let mottle = tone(fbm(n, 18, 3, 0.55, seed + 2), 1.0);
    Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let ground = 0.42 + 0.25 * mottle.v[i];
        let mica = smooth(0.7, 0.8, a.v[i]);
        let quartz = smooth(0.74, 0.84, b.v[i]);
        let v = ground * (1.0 - 0.85 * mica);
        v + (1.0 - v) * 0.9 * quartz
    })
    .blur(0.4)
}

fn stucco(n: usize, seed: u64) -> Field {
    let big = lumps(n, 18, 1.0, 0.9, 1.2, seed).standardize();
    let small = lumps(n, 48, 1.0, 0.9, 0.9, seed + 1).standardize();
    let fine = white(n, seed + 2).blur(0.7).standardize();
    tone(big.add(&small, 0.7).add(&fine, 0.25), 1.0)
}

fn gravel(n: usize, seed: u64) -> Field {
    let c = cells(n, 17, 0.8, seed);
    let rough = fbm(n, 48, 3, 0.6, seed + 1);
    let f = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let id = c.c1[i];
        let reach = 0.38 + 0.2 * hash01(id, 7, seed);
        let dome = (1.0 - (c.f1.v[i] / reach).powi(2)).max(0.0).sqrt();
        let shade = 0.45 + 0.55 * hash01(id, 8, seed);
        dome * shade + rough.v[i] * 0.06
    });
    tone(f, 0.9)
}

fn flagstone(n: usize, seed: u64) -> Field {
    let c = cells(n, 7, 0.85, seed);
    let rough = fbm(n, 36, 4, 0.6, seed + 1);
    let stone = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let edge = c.f2.v[i] - c.f1.v[i];
        let slab = 0.45 + 0.4 * hash01(c.c1[i], 5, seed);
        let bevel = smooth(0.0, 0.22, edge);
        let gap = smooth(0.0, 0.05, edge);
        (slab + rough.v[i] * 0.1) * (0.35 + 0.65 * bevel) * gap
    });
    tone(stone, 1.0)
}

fn cell_web(n: usize, seed: u64) -> Field {
    let c = cells(n, 11, 1.0, seed);
    let shade = fbm(n, 24, 3, 0.55, seed + 1);
    let f = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let membrane = smooth(0.02, 0.26, c.f2.v[i] - c.f1.v[i]);
        membrane * (1.0 - 0.45 * smooth(0.0, 0.7, c.f1.v[i])) + shade.v[i] * 0.05
    });
    tone(f, 1.0)
}

/// Meandering dry cracks between plates of baked earth.
fn cracks(n: usize, seed: u64) -> Field {
    let c = cells(n, 7, 1.0, seed);
    let wobble_x = fbm(n, 6, 3, 0.55, seed + 1).equalize(1.0);
    let wobble_y = fbm(n, 6, 3, 0.55, seed + 2).equalize(1.0);
    let cell_px = n as f32 / 7.0;
    let weight = tone(fbm(n, 12, 2, 0.5, seed + 3), 1.0);
    // Distance to the nearest cell edge, in texels; only some edges are cracked.
    let line = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let (a, b) = (c.c1[i].min(c.c2[i]), c.c1[i].max(c.c2[i]));
        if hash01(a, b, seed + 4) > 0.62 {
            return 0.0;
        }
        let perp = (c.f2.v[i] - c.f1.v[i]) * 0.5 * cell_px;
        let half = 0.5 + 1.6 * weight.v[i];
        (half - perp + 0.5).clamp(0.0, 1.0)
    })
    .warped(&wobble_x, &wobble_y, 9.0);
    let plates = Field::from_fn(n, |x, y| 0.62 + 0.38 * hash01(c.c1[y * n + x], 3, seed + 5));
    let grain = tone(fbm(n, 20, 4, 0.6, seed + 6), 1.0);
    plates.mix(&grain, 0.55).combine(&line, |p, l| p * (1.0 - 0.95 * l))
}

/// The fine, even network of hairline cracks in old varnish.
fn craquelure(n: usize, seed: u64) -> Field {
    let c = cells(n, 22, 1.0, seed);
    let wobble_x = fbm(n, 24, 3, 0.5, seed + 1).equalize(1.0);
    let wobble_y = fbm(n, 24, 3, 0.5, seed + 2).equalize(1.0);
    let cell_px = n as f32 / 22.0;
    let line = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let perp = (c.f2.v[i] - c.f1.v[i]) * 0.5 * cell_px;
        (0.55 - perp + 0.5).clamp(0.0, 1.0)
    })
    .warped(&wobble_x, &wobble_y, 2.5);
    let flakes = Field::from_fn(n, |x, y| 0.7 + 0.3 * hash01(c.c1[y * n + x], 4, seed + 3));
    let mottle = tone(fbm(n, 14, 3, 0.55, seed + 4), 1.0);
    flakes.mix(&mottle, 0.4).combine(&line, |p, l| p * (1.0 - 0.9 * l))
}

fn leather(n: usize, seed: u64) -> Field {
    let c = cells(n, 32, 0.9, seed);
    let warp_x = fbm(n, 10, 3, 0.5, seed + 1).equalize(1.0);
    let warp_y = fbm(n, 10, 3, 0.5, seed + 2).equalize(1.0);
    let hide = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let crease = smooth(0.0, 0.2, c.f2.v[i] - c.f1.v[i]);
        let puff = 1.0 - 0.35 * smooth(0.0, 0.6, c.f1.v[i]);
        crease.powf(0.8) * puff * (0.85 + 0.15 * hash01(c.c1[i], 6, seed))
    })
    .warped(&warp_x, &warp_y, 3.0);
    tone(hide, 1.0)
}

// ---- surfaces ---------------------------------------------------------------

fn wood_grain(n: usize, seed: u64) -> Field {
    let sway = fbm_xy(n, 4, 2, 3, 0.55, seed);
    // `sway` bends the rings; `knotty` adds a smaller wander.
    let knotty = fbm_xy(n, 8, 3, 2, 0.5, seed + 1);
    let pores = fbm_xy(n, 100, 8, 2, 0.6, seed + 2);
    let rings = 15.0;
    let f = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        let u = (x as f32 + 0.5) / n as f32;
        let phase = rings * (u + 0.035 * sway.v[i] + 0.012 * knotty.v[i]);
        let r = phase.rem_euclid(1.0);
        // A pale early-wood ramp that snaps to a hard dark line of late wood.
        let ring = r.powf(1.6) * (1.0 - 0.9 * smooth(0.82, 0.97, r));
        ring + pores.v[i] * 0.14
    });
    tone(f, 1.0)
}

fn bark(n: usize, seed: u64) -> Field {
    let furrow = fbm_xy(n, 7, 2, 4, 0.55, seed);
    let cross = fbm_xy(n, 5, 18, 3, 0.55, seed + 1);
    let streak = fbm_xy(n, 90, 5, 2, 0.6, seed + 2);
    let f = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        // Plates rise between the furrows, which run where the noise is zero;
        // shallower cracks cut across them.
        let plate = smooth(0.0, 0.5, furrow.v[i].abs());
        let split = 0.45 + 0.55 * smooth(0.0, 0.2, cross.v[i].abs());
        plate * split * (0.8 + 0.1 * streak.v[i])
    });
    tone(f, 1.0)
}

fn hammered_metal(n: usize, seed: u64) -> Field {
    let c = cells(n, 12, 0.95, seed);
    let tilt = fbm(n, 12, 2, 0.5, seed + 1);
    let bowl = Field::from_fn(n, |x, y| {
        let i = y * n + x;
        // Each dent is a bowl, deeper in some cells than others.
        let depth = 0.6 + 0.4 * hash01(c.c1[i], 2, seed);
        smooth(0.0, 0.7, c.f1.v[i]) * depth + tilt.v[i] * 0.03
    });
    tone(bowl.blur(0.7).relief(-0.7, -0.7, 5.0), 1.0)
}

fn felt_fibers(n: usize, seed: u64) -> Field {
    let long = pulp(n, 1300, (10.0, 26.0), (0.8, 1.5), 0.8, seed);
    let short = pulp(n, 900, (4.0, 10.0), (0.6, 1.1), 1.0, seed + 1);
    let haze = fbm(n, 16, 3, 0.55, seed + 2);
    tone(haze.add(&long, 0.9).add(&short, 0.6), 1.0)
}
