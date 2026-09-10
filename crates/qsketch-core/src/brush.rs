//! Brush settings and the stroke engine.
//!
//! Photoshop semantics: **flow** is how much paint each dab deposits, and
//! **opacity** caps how much a single stroke can build up. The engine keeps a
//! per-stroke coverage buffer (`0..=1` per pixel, accumulated with flow) and
//! rewrites affected pixels as `original ⊕ color·(coverage·opacity)`, so
//! overlapping dabs inside one stroke never exceed the opacity cap.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::blend::src_over;
use crate::color::Rgba8;
use crate::geom::{IRect, Pt};
use crate::layer::Layer;
use crate::mask::Mask;
use crate::raster::{tile_rect, Raster, TILE, TILE_PX};

use crate::color::Hsv;
use crate::tip::{Rng, TipImage};

/// Name of the analytic round tip (the default when `tip` is empty).
pub const ROUND_TIP: &str = "Round";

/// What drives the dab angle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AngleControl {
    #[default]
    Off,
    /// Follow the stroke direction.
    Direction,
    /// Rotate with pen pressure.
    Pressure,
}

/// How a grain texture combines with the dab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextureMode {
    /// `dab × grain` — grain lightens.
    #[default]
    Multiply,
    /// `dab − (1 − grain)` — grain carves holes, hard contrast.
    Subtract,
    /// Only the darkest paper valleys stay white: `smoothstep` on grain.
    Height,
}

/// Input stabilizer applied on top of exponential `smoothing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StabilizerMode {
    #[default]
    Off,
    /// "Lazy brush": the stroke is dragged behind the pointer on a rope of
    /// fixed length, so jitter shorter than the rope never reaches the canvas.
    Rope,
    /// Moving average over the last N samples; smooths without lag on release.
    Average,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushSettings {
    pub name: String,
    /// Diameter in pixels.
    pub size: f32,
    /// 0 = fully soft, 1 = hard edge (still 1px anti-aliased if `antialias`).
    pub hardness: f32,
    /// Stroke opacity cap, 0..=1.
    pub opacity: f32,
    /// Per-dab flow, 0..=1.
    pub flow: f32,
    /// Dab spacing as a fraction of diameter.
    pub spacing: f32,
    pub pressure_size: bool,
    pub pressure_opacity: bool,
    /// Size at zero pressure as a fraction of `size` (when `pressure_size`).
    pub min_size: f32,
    /// Position smoothing 0..=1 (0 = raw input).
    pub smoothing: f32,
    /// Anti-aliased edges. Off gives Aseprite-style hard pixels (pencil).
    pub antialias: bool,
    pub stabilizer: StabilizerMode,
    /// Stabilizer strength 0..=1 (rope length / averaging window).
    pub stabilizer_strength: f32,
    /// Rope mode: finish the stroke at the pointer when released.
    pub stabilizer_catch_up: bool,

    // --- Brush tip shape ---
    /// Tip image name from the brush library; empty or "Round" = analytic round.
    pub tip: String,
    /// Tip rotation in degrees.
    pub angle: f32,
    /// Vertical squash of the tip, 0.01..=1.
    pub roundness: f32,
    pub flip_x: bool,
    pub flip_y: bool,

    // --- Shape dynamics ---
    pub shape_dynamics: bool,
    /// Random size variation, 0..=1 (fraction of size).
    pub size_jitter: f32,
    /// Random angle variation, 0..=1 (fraction of a full turn).
    pub angle_jitter: f32,
    pub angle_control: AngleControl,
    /// Random roundness variation, 0..=1.
    pub roundness_jitter: f32,
    /// Floor for jittered roundness, 0..=1.
    pub min_roundness: f32,

    // --- Scattering ---
    pub scattering: bool,
    /// Scatter distance as a multiple of the diameter, 0..=5.
    pub scatter: f32,
    /// Dabs per spacing step, 1..=16.
    pub scatter_count: u32,
    /// Scatter along the stroke as well as across it.
    pub scatter_both_axes: bool,
    /// Pressure reduces scatter.
    pub scatter_pressure: bool,

    // --- Texture (paper grain) ---
    pub texture_enabled: bool,
    /// Texture name from the brush library; empty = none.
    pub texture: String,
    /// Texture tile scale, 0.1..=8.
    pub texture_scale: f32,
    /// How strongly the grain shows, 0..=1.
    pub texture_depth: f32,
    pub texture_invert: bool,
    pub texture_mode: TextureMode,
    /// Sample the grain per dab (moves with the brush) instead of in canvas space.
    pub texture_each_tip: bool,

    // --- Transfer ---
    pub transfer: bool,
    /// Random per-dab flow variation, 0..=1.
    pub flow_jitter: f32,

    // --- Color dynamics ---
    pub color_dynamics: bool,
    /// Per-dab blend towards the background color, 0..=1.
    pub fg_bg_jitter: f32,
    /// Pressure controls the foreground/background blend.
    pub fg_bg_pressure: bool,
    /// Hue jitter, 0..=1 (fraction of the hue wheel).
    pub hue_jitter: f32,
    pub sat_jitter: f32,
    pub bri_jitter: f32,

    // --- Noise ---
    /// Add per-pixel noise to soft edges.
    pub noise: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            name: "Hard Round".into(),
            size: 23.0,
            hardness: 0.9,
            opacity: 1.0,
            flow: 0.7,
            spacing: 0.1,
            pressure_size: true,
            pressure_opacity: false,
            min_size: 0.1,
            smoothing: 0.2,
            antialias: true,
            stabilizer: StabilizerMode::Off,
            stabilizer_strength: 0.5,
            stabilizer_catch_up: true,
            tip: String::new(),
            angle: 0.0,
            roundness: 1.0,
            flip_x: false,
            flip_y: false,
            shape_dynamics: false,
            size_jitter: 0.0,
            angle_jitter: 0.0,
            angle_control: AngleControl::Off,
            roundness_jitter: 0.0,
            min_roundness: 0.25,
            scattering: false,
            scatter: 1.0,
            scatter_count: 1,
            scatter_both_axes: false,
            scatter_pressure: false,
            texture_enabled: false,
            texture: String::new(),
            texture_scale: 1.0,
            texture_depth: 0.5,
            texture_invert: false,
            texture_mode: TextureMode::Multiply,
            texture_each_tip: false,
            transfer: false,
            flow_jitter: 0.0,
            color_dynamics: false,
            fg_bg_jitter: 0.0,
            fg_bg_pressure: false,
            hue_jitter: 0.0,
            sat_jitter: 0.0,
            bri_jitter: 0.0,
            noise: false,
        }
    }
}

impl BrushSettings {
    pub fn preset(name: &str) -> Self {
        Self::presets().into_iter().find(|p| p.name == name).unwrap_or_default()
    }

    pub fn presets() -> Vec<BrushSettings> {
        let d = BrushSettings::default();
        vec![
            BrushSettings { name: "Hard Round".into(), ..d.clone() },
            BrushSettings { name: "Soft Round".into(), hardness: 0.0, flow: 0.5, spacing: 0.08, ..d.clone() },
            BrushSettings {
                name: "Pencil".into(),
                size: 3.0,
                hardness: 1.0,
                flow: 1.0,
                spacing: 0.05,
                pressure_size: true,
                pressure_opacity: true,
                min_size: 0.3,
                smoothing: 0.1,
                ..d.clone()
            },
            BrushSettings {
                name: "Ink Pen".into(),
                size: 8.0,
                hardness: 1.0,
                flow: 1.0,
                spacing: 0.05,
                pressure_size: true,
                min_size: 0.05,
                smoothing: 0.4,
                ..d.clone()
            },
            BrushSettings {
                name: "Airbrush".into(),
                size: 80.0,
                hardness: 0.0,
                flow: 0.08,
                spacing: 0.05,
                pressure_size: false,
                pressure_opacity: true,
                ..d.clone()
            },
            BrushSettings {
                name: "Marker".into(),
                size: 30.0,
                hardness: 0.85,
                opacity: 0.6,
                flow: 1.0,
                spacing: 0.1,
                pressure_size: false,
                ..d.clone()
            },
            BrushSettings {
                name: "Pixel".into(),
                size: 1.0,
                hardness: 1.0,
                flow: 1.0,
                spacing: 0.1,
                pressure_size: false,
                pressure_opacity: false,
                smoothing: 0.0,
                antialias: false,
                ..d.clone()
            },
            BrushSettings {
                name: "Chalk".into(),
                size: 36.0,
                flow: 0.8,
                spacing: 0.12,
                tip: "Chalk".into(),
                pressure_opacity: true,
                shape_dynamics: true,
                angle_jitter: 1.0,
                texture_enabled: true,
                texture: "Rough Paper".into(),
                texture_depth: 0.6,
                ..d.clone()
            },
            BrushSettings {
                name: "Charcoal".into(),
                size: 28.0,
                flow: 0.6,
                spacing: 0.08,
                tip: "Charcoal".into(),
                angle: 35.0,
                roundness: 0.6,
                pressure_opacity: true,
                shape_dynamics: true,
                angle_control: AngleControl::Direction,
                size_jitter: 0.15,
                texture_enabled: true,
                texture: "Fine Grain".into(),
                texture_depth: 0.35,
                ..d.clone()
            },
            BrushSettings {
                name: "Spatter".into(),
                size: 60.0,
                flow: 0.5,
                spacing: 0.3,
                tip: "Spatter".into(),
                shape_dynamics: true,
                angle_jitter: 1.0,
                size_jitter: 0.4,
                scattering: true,
                scatter: 0.8,
                scatter_count: 2,
                scatter_both_axes: true,
                ..d.clone()
            },
            BrushSettings {
                name: "Scatter Leaves".into(),
                size: 40.0,
                flow: 1.0,
                spacing: 0.9,
                tip: "Leaf".into(),
                shape_dynamics: true,
                angle_jitter: 1.0,
                size_jitter: 0.5,
                scattering: true,
                scatter: 1.5,
                scatter_count: 2,
                scatter_both_axes: true,
                color_dynamics: true,
                fg_bg_jitter: 0.5,
                hue_jitter: 0.06,
                bri_jitter: 0.3,
                ..d.clone()
            },
            BrushSettings {
                name: "Flat Marker".into(),
                size: 40.0,
                opacity: 0.7,
                flow: 1.0,
                spacing: 0.06,
                tip: "Flat Marker".into(),
                angle: 45.0,
                pressure_size: false,
                ..d.clone()
            },
            BrushSettings {
                name: "Dry Bristle".into(),
                size: 48.0,
                flow: 0.55,
                spacing: 0.06,
                tip: "Bristle".into(),
                shape_dynamics: true,
                angle_control: AngleControl::Direction,
                transfer: true,
                flow_jitter: 0.4,
                texture_enabled: true,
                texture: "Canvas".into(),
                texture_depth: 0.4,
                ..d.clone()
            },
        ]
    }

    pub fn clamp(&mut self) {
        self.size = self.size.clamp(1.0, 2000.0);
        self.hardness = self.hardness.clamp(0.0, 1.0);
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self.flow = self.flow.clamp(0.01, 1.0);
        self.spacing = self.spacing.clamp(0.01, 5.0);
        self.min_size = self.min_size.clamp(0.0, 1.0);
        self.smoothing = self.smoothing.clamp(0.0, 1.0);
        self.stabilizer_strength = self.stabilizer_strength.clamp(0.0, 1.0);
        self.angle = self.angle.rem_euclid(360.0);
        self.roundness = self.roundness.clamp(0.01, 1.0);
        self.size_jitter = self.size_jitter.clamp(0.0, 1.0);
        self.angle_jitter = self.angle_jitter.clamp(0.0, 1.0);
        self.roundness_jitter = self.roundness_jitter.clamp(0.0, 1.0);
        self.min_roundness = self.min_roundness.clamp(0.01, 1.0);
        self.scatter = self.scatter.clamp(0.0, 5.0);
        self.scatter_count = self.scatter_count.clamp(1, 16);
        self.texture_scale = self.texture_scale.clamp(0.1, 8.0);
        self.texture_depth = self.texture_depth.clamp(0.0, 1.0);
        self.flow_jitter = self.flow_jitter.clamp(0.0, 1.0);
        self.fg_bg_jitter = self.fg_bg_jitter.clamp(0.0, 1.0);
        self.hue_jitter = self.hue_jitter.clamp(0.0, 1.0);
        self.sat_jitter = self.sat_jitter.clamp(0.0, 1.0);
        self.bri_jitter = self.bri_jitter.clamp(0.0, 1.0);
        if self.tip == ROUND_TIP {
            self.tip.clear();
        }
    }

    /// True when the tip is the analytic round dab.
    pub fn is_round(&self) -> bool {
        self.tip.is_empty() || self.tip == ROUND_TIP
    }

    /// Whether any randomized feature is active (the preview needs a stable seed).
    pub fn is_randomized(&self) -> bool {
        (self.shape_dynamics && (self.size_jitter > 0.0 || self.angle_jitter > 0.0 || self.roundness_jitter > 0.0))
            || self.scattering
            || (self.transfer && self.flow_jitter > 0.0)
            || (self.color_dynamics
                && (self.fg_bg_jitter > 0.0 || self.hue_jitter > 0.0 || self.sat_jitter > 0.0 || self.bri_jitter > 0.0))
            || self.noise
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaintMode {
    Paint,
    Erase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeSample {
    pub pos: Pt,
    /// 0..=1 (1.0 for mouse).
    pub pressure: f32,
}

/// Per-dab geometry after dynamics are applied.
#[derive(Clone, Copy, Debug)]
struct DabShape {
    radius: f32,
    /// Rotation in radians.
    angle: f32,
    roundness: f32,
}

pub struct StrokeEngine {
    settings: BrushSettings,
    mode: PaintMode,
    color: Rgba8,
    /// Background color for `fg_bg_jitter`.
    bg: Rgba8,
    tip: Option<Arc<TipImage>>,
    texture: Option<Arc<TipImage>>,
    alpha_lock: bool,
    original: Raster,
    selection: Option<Arc<Mask>>,
    coverage: HashMap<usize, Box<[f32; TILE_PX]>>,
    /// Per-pixel accumulated dab color (only with color dynamics).
    colors: Option<HashMap<usize, Box<[[f32; 3]; TILE_PX]>>>,
    last: Option<StrokeSample>,
    smooth_pos: Option<Pt>,
    /// Stroke direction in radians from the last two positions.
    direction: f32,
    /// Distance to travel before the next dab.
    until_next_dab: f32,
    dabs: usize,
    dirty_total: IRect,
    rng: Rng,
}

impl StrokeEngine {
    /// Begin a stroke on `layer`. When erasing on an alpha-locked layer, pass
    /// the background color as `color` (Photoshop behaviour).
    pub fn new(
        mut settings: BrushSettings,
        mode: PaintMode,
        color: Rgba8,
        layer: &Layer,
        selection: Option<Arc<Mask>>,
    ) -> Self {
        settings.clamp();
        let colors = if settings.color_dynamics && mode == PaintMode::Paint { Some(HashMap::new()) } else { None };
        Self {
            settings,
            mode,
            color,
            bg: Rgba8::WHITE,
            tip: None,
            texture: None,
            alpha_lock: layer.props.alpha_locked,
            original: layer.raster.clone(),
            selection,
            coverage: HashMap::new(),
            colors,
            last: None,
            smooth_pos: None,
            direction: 0.0,
            until_next_dab: 0.0,
            dabs: 0,
            dirty_total: IRect::EMPTY,
            rng: Rng::new(0x5EED),
        }
    }

    /// Use a tip image instead of the analytic round dab.
    pub fn with_tip(mut self, tip: Option<Arc<TipImage>>) -> Self {
        self.tip = tip;
        self
    }

    /// Grain texture (only used when `texture_enabled`).
    pub fn with_texture(mut self, texture: Option<Arc<TipImage>>) -> Self {
        self.texture = texture;
        self
    }

    /// Background color for foreground/background color jitter.
    pub fn with_background(mut self, bg: Rgba8) -> Self {
        self.bg = bg;
        self
    }

    /// Seed for the per-stroke random source (previews want determinism).
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.rng = Rng::new(seed);
        self
    }

    pub fn settings(&self) -> &BrushSettings {
        &self.settings
    }

    pub fn dirty_total(&self) -> IRect {
        self.dirty_total
    }

    pub fn dab_count(&self) -> usize {
        self.dabs
    }

    fn radius_for(&self, pressure: f32) -> f32 {
        let s = &self.settings;
        let scale = if s.pressure_size { s.min_size + (1.0 - s.min_size) * pressure } else { 1.0 };
        (s.size * scale / 2.0).max(0.5)
    }

    fn alpha_for(&self, pressure: f32) -> f32 {
        let s = &self.settings;
        if s.pressure_opacity {
            s.flow * pressure
        } else {
            s.flow
        }
    }

    /// Feed a new input sample. Returns the pixel rect modified by this call.
    pub fn extend(&mut self, raster: &mut Raster, sample: StrokeSample) -> IRect {
        let pressure = sample.pressure.clamp(0.0, 1.0);
        // Exponential smoothing of position.
        let pos = match self.smooth_pos {
            None => sample.pos,
            Some(sp) => {
                let k = 1.0 - self.settings.smoothing * 0.9;
                sp.lerp(sample.pos, k)
            }
        };
        self.smooth_pos = Some(pos);
        let cur = StrokeSample { pos, pressure };

        let Some(last) = self.last else {
            let r = self.dab_group(raster, pos, pressure);
            self.last = Some(cur);
            self.until_next_dab = self.spacing_px(pressure);
            return r;
        };

        let mut dirty = IRect::EMPTY;
        let dist = last.pos.dist(pos);
        if dist <= 0.0 {
            self.last = Some(cur);
            return dirty;
        }
        self.direction = (pos.y - last.pos.y).atan2(pos.x - last.pos.x);
        let mut travelled = self.until_next_dab;
        while travelled <= dist {
            let t = travelled / dist;
            let p = last.pos.lerp(pos, t);
            let pr = last.pressure + (pressure - last.pressure) * t;
            dirty = dirty.union(&self.dab_group(raster, p, pr));
            travelled += self.spacing_px(pr);
        }
        self.until_next_dab = travelled - dist;
        self.last = Some(cur);
        dirty
    }

    /// Finish the stroke. Guarantees at least one dab (a click) was placed.
    pub fn finish(&mut self, raster: &mut Raster) -> IRect {
        let r = if self.dabs == 0 {
            if let Some(l) = self.last {
                self.dab_group(raster, l.pos, l.pressure)
            } else {
                IRect::EMPTY
            }
        } else {
            IRect::EMPTY
        };
        if self.mode == PaintMode::Erase {
            raster.prune_empty_tiles();
        }
        r
    }

    fn spacing_px(&self, pressure: f32) -> f32 {
        (self.settings.spacing * self.radius_for(pressure) * 2.0).max(0.25)
    }

    /// Resolve jitter/dynamics into a concrete dab shape.
    fn shape_for(&mut self, pressure: f32) -> DabShape {
        let s = &self.settings;
        let mut radius = self.radius_for(pressure);
        // The user angle is counter-clockwise on screen (Photoshop's dial); the
        // raster's y axis points down, so negate it. `direction` is already in
        // raster space and is added as-is.
        let mut angle = -s.angle.to_radians();
        let mut roundness = s.roundness;
        if s.shape_dynamics {
            if s.size_jitter > 0.0 {
                radius *= (1.0 - s.size_jitter * self.rng.unit()).max(0.02);
            }
            match s.angle_control {
                AngleControl::Off => {}
                AngleControl::Direction => angle += self.direction,
                AngleControl::Pressure => angle += pressure * std::f32::consts::TAU,
            }
            if s.angle_jitter > 0.0 {
                angle += self.rng.signed() * s.angle_jitter * std::f32::consts::PI;
            }
            if s.roundness_jitter > 0.0 {
                let r = roundness * (1.0 - s.roundness_jitter * self.rng.unit());
                roundness = r.max(s.min_roundness.min(roundness));
            }
        }
        DabShape { radius: radius.max(0.5), angle, roundness: roundness.clamp(0.01, 1.0) }
    }

    /// Per-dab flow after Transfer jitter.
    fn flow_for(&mut self, pressure: f32) -> f32 {
        let mut a = self.alpha_for(pressure);
        if self.settings.transfer && self.settings.flow_jitter > 0.0 {
            a *= 1.0 - self.settings.flow_jitter * self.rng.unit();
        }
        a
    }

    /// Per-dab color after Color Dynamics.
    fn color_for(&mut self, pressure: f32) -> [f32; 4] {
        let s = &self.settings;
        let fg = self.color.to_f32();
        if !s.color_dynamics || self.mode == PaintMode::Erase {
            return fg;
        }
        let bg = self.bg.to_f32();
        let mut c = fg;
        if s.fg_bg_jitter > 0.0 || s.fg_bg_pressure {
            let mut t = if s.fg_bg_pressure { 1.0 - pressure } else { 0.0 };
            if s.fg_bg_jitter > 0.0 {
                t = (t + s.fg_bg_jitter * self.rng.unit()).min(1.0);
            }
            for i in 0..3 {
                c[i] = fg[i] + (bg[i] - fg[i]) * t;
            }
        }
        if s.hue_jitter > 0.0 || s.sat_jitter > 0.0 || s.bri_jitter > 0.0 {
            let mut hsv = Hsv::from_rgb(c[0], c[1], c[2]);
            if s.hue_jitter > 0.0 {
                hsv.h = (hsv.h + self.rng.signed() * s.hue_jitter * 180.0).rem_euclid(360.0);
            }
            if s.sat_jitter > 0.0 {
                hsv.s = (hsv.s + self.rng.signed() * s.sat_jitter).clamp(0.0, 1.0);
            }
            if s.bri_jitter > 0.0 {
                hsv.v = (hsv.v + self.rng.signed() * s.bri_jitter).clamp(0.0, 1.0);
            }
            let [r, g, b] = hsv.to_rgb();
            c[0] = r;
            c[1] = g;
            c[2] = b;
        }
        c
    }

    /// One spacing step: a single dab, or a scattered cluster.
    fn dab_group(&mut self, raster: &mut Raster, center: Pt, pressure: f32) -> IRect {
        let s = &self.settings;
        if !s.scattering || (s.scatter <= 0.0 && s.scatter_count <= 1) {
            return self.dab(raster, center, pressure);
        }
        let count = s.scatter_count.max(1);
        let both = s.scatter_both_axes;
        let mut amount = s.scatter * self.radius_for(pressure) * 2.0;
        if s.scatter_pressure {
            amount *= 1.0 - pressure * 0.9;
        }
        let (dx, dy) = (self.direction.cos(), self.direction.sin());
        let mut dirty = IRect::EMPTY;
        for _ in 0..count {
            let across = self.rng.signed() * amount;
            let along = if both { self.rng.signed() * amount } else { 0.0 };
            // Perpendicular = (-dy, dx).
            let p = Pt::new(center.x - dy * across + dx * along, center.y + dx * across + dy * along);
            dirty = dirty.union(&self.dab(raster, p, pressure));
        }
        dirty
    }

    fn dab(&mut self, raster: &mut Raster, center: Pt, pressure: f32) -> IRect {
        self.dabs += 1;
        let shape = self.shape_for(pressure);
        let alpha = self.flow_for(pressure);
        if alpha <= 0.0 {
            return IRect::EMPTY;
        }
        let dab_color = self.color_for(pressure);
        let r = shape.radius;
        // Bounding radius: rotation of an r × r·roundness ellipse stays within r.
        let rect =
            IRect::from_f32_bounds(center.x - r - 1.0, center.y - r - 1.0, center.x + r + 1.0, center.y + r + 1.0)
                .intersect(&raster.rect());
        if rect.is_empty() {
            return IRect::EMPTY;
        }
        let s = &self.settings;
        let hardness = s.hardness;
        let aa = s.antialias;
        let (sin, cos) = shape.angle.sin_cos();
        let fx = if s.flip_x { -1.0 } else { 1.0 };
        let fy = if s.flip_y { -1.0 } else { 1.0 };
        let inv_round = 1.0 / shape.roundness;
        let tip = if s.is_round() { None } else { self.tip.clone() };
        let texture = if s.texture_enabled { self.texture.clone() } else { None };
        let tex_scale = s.texture_scale;
        let tex_depth = s.texture_depth;
        let tex_invert = s.texture_invert;
        let tex_mode = s.texture_mode;
        let tex_each_tip = s.texture_each_tip;
        let noise = s.noise;
        let noise_seed = self.rng.next_u64();
        let use_colors = self.colors.is_some();
        let dab_seed = self.dabs as u64;
        for (tx, ty) in raster.tiles_in_rect(rect) {
            let tr = tile_rect(tx, ty);
            let sub = rect.intersect(&tr);
            let idx = raster.tile_index(tx, ty);
            let cov = self.coverage.entry(idx).or_insert_with(|| Box::new([0.0; TILE_PX]));
            let mut col = self.colors.as_mut().map(|m| m.entry(idx).or_insert_with(|| Box::new([[0.0; 3]; TILE_PX])));
            for y in sub.y..sub.bottom() {
                let py = y as f32 + 0.5 - center.y;
                for x in sub.x..sub.right() {
                    let px = x as f32 + 0.5 - center.x;
                    // Into tip space: un-rotate, un-squash.
                    let lx = (px * cos + py * sin) * fx;
                    let ly = (-px * sin + py * cos) * inv_round * fy;
                    let mut fall = match &tip {
                        None => {
                            let dist = (lx * lx + ly * ly).sqrt();
                            falloff(dist, r, hardness, aa)
                        }
                        Some(t) => {
                            let v = t.sample_tip(lx / r, ly / r);
                            if aa {
                                v
                            } else if v >= 0.5 {
                                1.0
                            } else {
                                0.0
                            }
                        }
                    };
                    if fall <= 0.0 {
                        continue;
                    }
                    if let Some(tex) = &texture {
                        let (u, v) = if tex_each_tip {
                            ((lx + r) / tex_scale, (ly + r) / tex_scale)
                        } else {
                            (x as f32 / tex_scale, y as f32 / tex_scale)
                        };
                        let mut g = tex.sample_wrap(u, v);
                        if tex_invert {
                            g = 1.0 - g;
                        }
                        fall = match tex_mode {
                            TextureMode::Multiply => fall * (1.0 - tex_depth * (1.0 - g)),
                            TextureMode::Subtract => (fall - tex_depth * (1.0 - g)).max(0.0),
                            TextureMode::Height => {
                                let t = ((g - (1.0 - tex_depth)) / tex_depth.max(0.01)).clamp(0.0, 1.0);
                                fall * t * t * (3.0 - 2.0 * t)
                            }
                        };
                        if fall <= 0.0 {
                            continue;
                        }
                    }
                    if noise && fall < 0.999 {
                        let h = hash_pixel(x, y, noise_seed ^ dab_seed);
                        fall *= 1.0 - (1.0 - fall) * h;
                    }
                    let li = (y - tr.y) as usize * TILE + (x - tr.x) as usize;
                    let c = &mut cov[li];
                    let add = alpha * fall * (1.0 - *c);
                    if add <= 0.0 {
                        continue;
                    }
                    if use_colors {
                        if let Some(col) = col.as_deref_mut() {
                            let total = *c + add;
                            let w = add / total;
                            let pc = &mut col[li];
                            for i in 0..3 {
                                pc[i] += (dab_color[i] - pc[i]) * w;
                            }
                        }
                    }
                    *c += add;
                }
            }
        }
        self.apply(raster, rect);
        self.dirty_total = self.dirty_total.union(&rect);
        rect
    }

    /// Recompute `rect` pixels of `raster` from the original + coverage.
    fn apply(&self, raster: &mut Raster, rect: IRect) {
        let opacity = self.settings.opacity;
        let base_color = self.color.to_f32();
        for (tx, ty) in raster.tiles_in_rect(rect) {
            let idx = raster.tile_index(tx, ty);
            let Some(cov) = self.coverage.get(&idx) else { continue };
            let colors = self.colors.as_ref().and_then(|m| m.get(&idx));
            let tr = tile_rect(tx, ty);
            let sub = rect.intersect(&tr);
            let orig = self.original.tile(tx, ty).cloned();
            let sel = self.selection.clone();
            let dst = raster.tile_mut(tx, ty);
            for y in sub.y..sub.bottom() {
                for x in sub.x..sub.right() {
                    let li = (y - tr.y) as usize * TILE + (x - tr.x) as usize;
                    let c = cov[li];
                    if c <= 0.0 {
                        continue;
                    }
                    let mut eff = c * opacity;
                    if let Some(m) = &sel {
                        eff *= m.coverage(x, y);
                        if eff <= 0.0 {
                            continue;
                        }
                    }
                    let color = match colors {
                        Some(cm) => {
                            let pc = cm[li];
                            [pc[0], pc[1], pc[2], base_color[3]]
                        }
                        None => base_color,
                    };
                    let o = match &orig {
                        Some(t) => t.get(li % TILE, li / TILE),
                        None => Rgba8::TRANSPARENT,
                    };
                    let of = o.to_f32();
                    let out = match self.mode {
                        PaintMode::Paint if self.alpha_lock => {
                            if o.a == 0 {
                                continue;
                            }
                            let k = eff * color[3];
                            [
                                of[0] + (color[0] - of[0]) * k,
                                of[1] + (color[1] - of[1]) * k,
                                of[2] + (color[2] - of[2]) * k,
                                of[3],
                            ]
                        }
                        PaintMode::Paint => src_over(of, [color[0], color[1], color[2], color[3] * eff]),
                        PaintMode::Erase if self.alpha_lock => {
                            if o.a == 0 {
                                continue;
                            }
                            [
                                of[0] + (color[0] - of[0]) * eff,
                                of[1] + (color[1] - of[1]) * eff,
                                of[2] + (color[2] - of[2]) * eff,
                                of[3],
                            ]
                        }
                        PaintMode::Erase => [of[0], of[1], of[2], of[3] * (1.0 - eff)],
                    };
                    dst.set(li % TILE, li / TILE, Rgba8::from_f32(out));
                }
            }
        }
    }
}

/// Cheap per-pixel hash in `0..1` for edge noise.
#[inline]
fn hash_pixel(x: i32, y: i32, seed: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed;
    h ^= h >> 31;
    h = h.wrapping_mul(0x7FB5_D329_728E_A185);
    h ^= h >> 27;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Dab intensity at `dist` from the center for radius `r`.
#[inline]
fn falloff(dist: f32, r: f32, hardness: f32, antialias: bool) -> f32 {
    if !antialias {
        return if dist <= r { 1.0 } else { 0.0 };
    }
    // 1px anti-aliased rim.
    let edge = (r - dist + 0.5).clamp(0.0, 1.0);
    if edge <= 0.0 {
        return 0.0;
    }
    if hardness >= 0.999 {
        return edge;
    }
    let d = dist / r;
    let soft = if d <= hardness {
        1.0
    } else {
        let t = ((d - hardness) / (1.0 - hardness)).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        1.0 - s
    };
    edge * soft
}

/// Render a sample stroke (an S-curve with a pressure ramp) of `settings` into
/// a `w × h` straight-alpha RGBA8 image, for previews and thumbnails. The
/// brush is scaled so the stroke fits, and randomness is seeded for stability.
pub fn render_preview(
    settings: &BrushSettings,
    tip: Option<Arc<TipImage>>,
    texture: Option<Arc<TipImage>>,
    fg: Rgba8,
    bg: Rgba8,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let mut s = settings.clone();
    s.clamp();
    let max_size = (h as f32 * 0.5).max(2.0);
    if s.size > max_size {
        let k = max_size / s.size;
        s.size = max_size;
        // Keep tile-space grain proportional so the preview reads like the canvas.
        s.texture_scale = (s.texture_scale * k).max(0.1);
    }
    s.smoothing = 0.0;
    let layer = Layer::new(1, "preview", w, h);
    let mut raster = layer.raster.clone();
    let mut e = StrokeEngine::new(s, PaintMode::Paint, fg, &layer, None)
        .with_tip(tip)
        .with_texture(texture)
        .with_background(bg)
        .with_seed(7);
    let steps = (w * 2).max(32);
    let margin = w as f32 * 0.08;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = margin + t * (w as f32 - margin * 2.0);
        let y = h as f32 * 0.5 + (t * std::f32::consts::TAU).sin() * h as f32 * 0.22;
        // Pressure ramps 0.1 → 1 → 0.1 like a natural stroke.
        let p = 0.1 + 0.9 * (1.0 - (t * 2.0 - 1.0).abs());
        e.extend(&mut raster, StrokeSample { pos: Pt::new(x, y), pressure: p });
    }
    e.finish(&mut raster);
    raster.to_rgba()
}

/// Apply a pressure curve exponent: `gamma < 1` makes light touches stronger.
pub fn pressure_curve(p: f32, gamma: f32) -> f32 {
    p.clamp(0.0, 1.0).powf(gamma.clamp(0.1, 10.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer() -> Layer {
        Layer::new(1, "L", 128, 128)
    }

    #[test]
    fn click_paints_a_dot() {
        let l = layer();
        let mut raster = l.raster.clone();
        let s = BrushSettings { size: 10.0, flow: 1.0, pressure_size: false, ..Default::default() };
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None);
        e.extend(&mut raster, StrokeSample { pos: Pt::new(50.0, 50.0), pressure: 1.0 });
        e.finish(&mut raster);
        assert_eq!(raster.get_pixel(50, 50), Rgba8::BLACK);
        assert_eq!(raster.get_pixel(50, 60), Rgba8::TRANSPARENT);
        assert!(!e.dirty_total().is_empty());
    }

    #[test]
    fn opacity_caps_buildup() {
        let l = layer();
        let mut raster = l.raster.clone();
        let s = BrushSettings {
            size: 10.0,
            flow: 1.0,
            opacity: 0.5,
            pressure_size: false,
            smoothing: 0.0,
            ..Default::default()
        };
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None);
        for x in 0..20 {
            e.extend(&mut raster, StrokeSample { pos: Pt::new(40.0 + x as f32, 50.0), pressure: 1.0 });
        }
        e.finish(&mut raster);
        let a = raster.get_pixel(50, 50).a;
        assert!((a as i32 - 128).abs() <= 2, "alpha {a}");
    }

    #[test]
    fn erase_and_selection() {
        let mut l = layer();
        l.raster.fill_rect(IRect::new(0, 0, 128, 128), Rgba8::WHITE);
        let mut raster = l.raster.clone();
        let sel = Arc::new(Mask::from_rect(128, 128, IRect::new(0, 0, 50, 128)));
        let s = BrushSettings { size: 20.0, flow: 1.0, pressure_size: false, ..Default::default() };
        let mut e = StrokeEngine::new(s, PaintMode::Erase, Rgba8::WHITE, &l, Some(sel));
        e.extend(&mut raster, StrokeSample { pos: Pt::new(50.0, 50.0), pressure: 1.0 });
        e.finish(&mut raster);
        assert_eq!(raster.get_pixel(45, 50).a, 0);
        assert_eq!(raster.get_pixel(55, 50).a, 255);
    }

    #[test]
    fn pixel_brush_is_one_pixel() {
        let l = layer();
        let mut raster = l.raster.clone();
        let s = BrushSettings::preset("Pixel");
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None);
        e.extend(&mut raster, StrokeSample { pos: Pt::new(10.5, 10.5), pressure: 1.0 });
        e.finish(&mut raster);
        let mut count = 0;
        for y in 0..128 {
            for x in 0..128 {
                if raster.get_pixel(x, y).a != 0 {
                    count += 1;
                }
            }
        }
        assert_eq!(count, 1);
    }

    fn count_painted(raster: &Raster) -> usize {
        let mut n = 0;
        for y in 0..raster.height() as i32 {
            for x in 0..raster.width() as i32 {
                if raster.get_pixel(x, y).a != 0 {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn old_settings_without_new_fields_deserialize() {
        let json = r#"{"name":"Old","size":10.0,"hardness":1.0,"opacity":1.0,"flow":1.0,"spacing":0.1,
            "pressure_size":false,"pressure_opacity":false,"min_size":0.1,"smoothing":0.0,"antialias":true}"#;
        let b: BrushSettings = serde_json::from_str(json).unwrap();
        assert!(b.is_round());
        assert_eq!(b.roundness, 1.0);
        assert!(!b.texture_enabled);
    }

    #[test]
    fn roundness_squashes_the_dab() {
        let l = layer();
        let mut raster = l.raster.clone();
        let s = BrushSettings {
            size: 40.0,
            flow: 1.0,
            hardness: 1.0,
            pressure_size: false,
            roundness: 0.25,
            angle: 0.0,
            ..Default::default()
        };
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None);
        e.extend(&mut raster, StrokeSample { pos: Pt::new(64.0, 64.0), pressure: 1.0 });
        e.finish(&mut raster);
        assert_ne!(raster.get_pixel(80, 64).a, 0, "wide axis");
        assert_eq!(raster.get_pixel(64, 80).a, 0, "squashed axis");
        // Rotate 90°: the long axis is now vertical.
        let mut raster = l.raster.clone();
        let s = BrushSettings { angle: 90.0, ..e.settings().clone() };
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None);
        e.extend(&mut raster, StrokeSample { pos: Pt::new(64.0, 64.0), pressure: 1.0 });
        e.finish(&mut raster);
        assert_eq!(raster.get_pixel(80, 64).a, 0);
        assert_ne!(raster.get_pixel(64, 80).a, 0);
    }

    #[test]
    fn tip_image_shapes_the_dab() {
        let l = layer();
        let mut raster = l.raster.clone();
        // A tip that only covers its left half.
        let mut alpha = vec![0.0; 16 * 16];
        for y in 0..16 {
            for x in 0..8 {
                alpha[y * 16 + x] = 1.0;
            }
        }
        let tip = Arc::new(TipImage::from_alpha("half", 16, 16, alpha));
        let s = BrushSettings { size: 32.0, flow: 1.0, pressure_size: false, tip: "half".into(), ..Default::default() };
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None).with_tip(Some(tip));
        e.extend(&mut raster, StrokeSample { pos: Pt::new(64.0, 64.0), pressure: 1.0 });
        e.finish(&mut raster);
        assert_ne!(raster.get_pixel(56, 64).a, 0);
        assert_eq!(raster.get_pixel(72, 64).a, 0);
    }

    #[test]
    fn texture_lightens_coverage() {
        let l = layer();
        let s = BrushSettings {
            size: 20.0,
            flow: 1.0,
            hardness: 1.0,
            pressure_size: false,
            texture_enabled: true,
            texture: "g".into(),
            texture_depth: 1.0,
            texture_scale: 1.0,
            ..Default::default()
        };
        let tex = Arc::new(TipImage::from_alpha("g", 2, 1, vec![1.0, 0.0]));
        let mut raster = l.raster.clone();
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None).with_texture(Some(tex));
        e.extend(&mut raster, StrokeSample { pos: Pt::new(64.0, 64.0), pressure: 1.0 });
        e.finish(&mut raster);
        // Even columns hit grain=1 (full), odd columns grain=0 (nothing) with Multiply at depth 1.
        assert_eq!(raster.get_pixel(64, 64).a, 255);
        assert_eq!(raster.get_pixel(65, 64).a, 0);
    }

    #[test]
    fn scatter_spreads_dabs_and_stays_in_bounds() {
        let l = layer();
        let mut raster = l.raster.clone();
        let s = BrushSettings {
            size: 6.0,
            flow: 1.0,
            hardness: 1.0,
            pressure_size: false,
            smoothing: 0.0,
            scattering: true,
            scatter: 2.0,
            scatter_count: 4,
            scatter_both_axes: true,
            ..Default::default()
        };
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None).with_seed(3);
        for x in 0..40 {
            e.extend(&mut raster, StrokeSample { pos: Pt::new(20.0 + x as f32, 64.0), pressure: 1.0 });
        }
        e.finish(&mut raster);
        let mut far = 0;
        for y in 0..128 {
            for x in 0..128 {
                if raster.get_pixel(x, y).a != 0 && (y - 64).abs() > 6 {
                    far += 1;
                }
            }
        }
        assert!(far > 20, "scatter should place paint off the stroke line ({far})");
        assert!(e.dab_count() > 40);
    }

    #[test]
    fn color_dynamics_varies_color() {
        let l = layer();
        let mut raster = l.raster.clone();
        let s = BrushSettings {
            size: 4.0,
            flow: 1.0,
            hardness: 1.0,
            pressure_size: false,
            spacing: 2.0,
            smoothing: 0.0,
            color_dynamics: true,
            fg_bg_jitter: 1.0,
            ..Default::default()
        };
        let mut e =
            StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None).with_background(Rgba8::WHITE).with_seed(9);
        for x in 0..100 {
            e.extend(&mut raster, StrokeSample { pos: Pt::new(10.0 + x as f32, 64.0), pressure: 1.0 });
        }
        e.finish(&mut raster);
        let mut seen = std::collections::HashSet::new();
        for x in 0..128 {
            let p = raster.get_pixel(x, 64);
            if p.a == 255 {
                seen.insert(p.r);
            }
        }
        assert!(seen.len() > 3, "expected several distinct grays, got {seen:?}");
    }

    #[test]
    fn preview_renders_something() {
        let px = render_preview(&BrushSettings::preset("Chalk"), None, None, Rgba8::BLACK, Rgba8::WHITE, 120, 40);
        assert_eq!(px.len(), 120 * 40 * 4);
        assert!(px.chunks_exact(4).filter(|p| p[3] > 0).count() > 100);
        let r = Raster::from_rgba(120, 40, &px);
        assert!(count_painted(&r) > 100);
    }

    #[test]
    fn positive_angle_tilts_counter_clockwise_on_screen() {
        // A 45° tip: the long axis runs bottom-left → top-right on screen
        // (screen y down), i.e. through (+x, -y) and (-x, +y).
        let l = layer();
        let mut raster = l.raster.clone();
        let s = BrushSettings {
            size: 60.0,
            flow: 1.0,
            hardness: 1.0,
            pressure_size: false,
            roundness: 0.15,
            angle: 45.0,
            ..Default::default()
        };
        let mut e = StrokeEngine::new(s, PaintMode::Paint, Rgba8::BLACK, &l, None);
        e.extend(&mut raster, StrokeSample { pos: Pt::new(64.0, 64.0), pressure: 1.0 });
        e.finish(&mut raster);
        assert_ne!(raster.get_pixel(80, 48).a, 0, "up-right should be painted");
        assert_ne!(raster.get_pixel(48, 80).a, 0, "down-left should be painted");
        assert_eq!(raster.get_pixel(80, 80).a, 0, "down-right should be empty");
        assert_eq!(raster.get_pixel(48, 48).a, 0, "up-left should be empty");
    }
}
