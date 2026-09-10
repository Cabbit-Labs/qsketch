//! Layer blend modes (W3C compositing spec semantics, matching Photoshop for
//! the common modes) and the straight-alpha source-over compositor.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    Overlay,
    SoftLight,
    HardLight,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    pub const ALL: [BlendMode; 20] = [
        BlendMode::Normal,
        BlendMode::Darken,
        BlendMode::Multiply,
        BlendMode::ColorBurn,
        BlendMode::LinearBurn,
        BlendMode::Lighten,
        BlendMode::Screen,
        BlendMode::ColorDodge,
        BlendMode::LinearDodge,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
        BlendMode::Divide,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Darken => "Darken",
            BlendMode::Multiply => "Multiply",
            BlendMode::ColorBurn => "Color Burn",
            BlendMode::LinearBurn => "Linear Burn",
            BlendMode::Lighten => "Lighten",
            BlendMode::Screen => "Screen",
            BlendMode::ColorDodge => "Color Dodge",
            BlendMode::LinearDodge => "Linear Dodge (Add)",
            BlendMode::Overlay => "Overlay",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::HardLight => "Hard Light",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::Subtract => "Subtract",
            BlendMode::Divide => "Divide",
            BlendMode::Hue => "Hue",
            BlendMode::Saturation => "Saturation",
            BlendMode::Color => "Color",
            BlendMode::Luminosity => "Luminosity",
        }
    }

    /// Group separators in menus, Photoshop-style: returns true if a separator
    /// should precede this mode.
    pub fn starts_group(self) -> bool {
        matches!(
            self,
            BlendMode::Darken | BlendMode::Lighten | BlendMode::Overlay | BlendMode::Difference | BlendMode::Hue
        )
    }

    pub fn is_separable(self) -> bool {
        !matches!(self, BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity)
    }
}

/// Separable blend function `B(cb, cs)` on one channel, inputs and output in `0..=1`.
#[inline]
pub fn blend_channel(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    match mode {
        BlendMode::Normal => cs,
        BlendMode::Darken => cb.min(cs),
        BlendMode::Multiply => cb * cs,
        BlendMode::ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        BlendMode::LinearBurn => (cb + cs - 1.0).max(0.0),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::ColorDodge => {
            if cb <= 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        BlendMode::LinearDodge => (cb + cs).min(1.0),
        BlendMode::Overlay => blend_channel(BlendMode::HardLight, cs, cb),
        BlendMode::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        BlendMode::HardLight => {
            if cs <= 0.5 {
                cb * 2.0 * cs
            } else {
                blend_channel(BlendMode::Screen, cb, 2.0 * cs - 1.0)
            }
        }
        BlendMode::Difference => (cb - cs).abs(),
        BlendMode::Exclusion => cb + cs - 2.0 * cb * cs,
        BlendMode::Subtract => (cb - cs).max(0.0),
        BlendMode::Divide => {
            if cs <= 0.0 {
                1.0
            } else {
                (cb / cs).min(1.0)
            }
        }
        // Non-separable modes are handled in `blend_rgb`.
        _ => cs,
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        for v in &mut out {
            *v = l + (*v - l) * l / (l - n).max(1e-6);
        }
    }
    if x > 1.0 {
        for v in &mut out {
            *v = l + (*v - l) * (1.0 - l) / (x - l).max(1e-6);
        }
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    // Sort channel indices by value.
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&a, &b| c[a].partial_cmp(&c[b]).unwrap_or(std::cmp::Ordering::Equal));
    let (imin, imid, imax) = (idx[0], idx[1], idx[2]);
    let mut out = [0.0; 3];
    let range = c[imax] - c[imin];
    if range > 0.0 {
        out[imid] = (c[imid] - c[imin]) * s / range;
        out[imax] = s;
    }
    out
}

/// Full RGB blend `B(cb, cs)` handling both separable and non-separable modes.
#[inline]
pub fn blend_rgb(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        BlendMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        BlendMode::Color => set_lum(cs, lum(cb)),
        BlendMode::Luminosity => set_lum(cb, lum(cs)),
        _ => [blend_channel(mode, cb[0], cs[0]), blend_channel(mode, cb[1], cs[1]), blend_channel(mode, cb[2], cs[2])],
    }
}

/// Composite `src` onto `backdrop` (both straight-alpha `[r,g,b,a]` in `0..=1`)
/// using `mode`, with `src` alpha scaled by `opacity`. Returns straight alpha.
#[inline]
pub fn composite_pixel(mode: BlendMode, backdrop: [f32; 4], src: [f32; 4], opacity: f32) -> [f32; 4] {
    let a_s = src[3] * opacity;
    if a_s <= 0.0 {
        return backdrop;
    }
    let a_b = backdrop[3];
    let cs = [src[0], src[1], src[2]];
    let cb = [backdrop[0], backdrop[1], backdrop[2]];
    // Blended source color: (1 - ab) * Cs + ab * B(Cb, Cs)
    let c = if mode == BlendMode::Normal || a_b <= 0.0 {
        cs
    } else {
        let b = blend_rgb(mode, cb, cs);
        [(1.0 - a_b) * cs[0] + a_b * b[0], (1.0 - a_b) * cs[1] + a_b * b[1], (1.0 - a_b) * cs[2] + a_b * b[2]]
    };
    let a_o = a_s + a_b * (1.0 - a_s);
    if a_o <= 0.0 {
        return [0.0; 4];
    }
    let inv = 1.0 / a_o;
    [
        (c[0] * a_s + cb[0] * a_b * (1.0 - a_s)) * inv,
        (c[1] * a_s + cb[1] * a_b * (1.0 - a_s)) * inv,
        (c[2] * a_s + cb[2] * a_b * (1.0 - a_s)) * inv,
        a_o,
    ]
}

/// Plain straight-alpha source-over, no blend mode.
#[inline]
pub fn src_over(backdrop: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    composite_pixel(BlendMode::Normal, backdrop, src, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_over() {
        let o = composite_pixel(BlendMode::Normal, [1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.5], 1.0);
        assert!((o[0] - 0.5).abs() < 1e-6 && (o[3] - 1.0).abs() < 1e-6);
        let o = composite_pixel(BlendMode::Normal, [0.0; 4], [0.2, 0.4, 0.6, 0.5], 1.0);
        assert!((o[0] - 0.2).abs() < 1e-6 && (o[3] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn multiply_screen() {
        let o = composite_pixel(BlendMode::Multiply, [0.5, 0.5, 0.5, 1.0], [0.5, 0.5, 0.5, 1.0], 1.0);
        assert!((o[0] - 0.25).abs() < 1e-6);
        let o = composite_pixel(BlendMode::Screen, [0.5, 0.5, 0.5, 1.0], [0.5, 0.5, 0.5, 1.0], 1.0);
        assert!((o[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn non_separable_stable() {
        let o = blend_rgb(BlendMode::Color, [0.2, 0.3, 0.4], [1.0, 0.0, 0.0]);
        assert!(o.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!((lum(o) - lum([0.2, 0.3, 0.4])).abs() < 1e-4);
    }
}
