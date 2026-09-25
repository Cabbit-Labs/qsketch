//! Color adjustments that run through the filter pipeline so they get the
//! same live preview, selection handling and multi-layer application.

use super::{map_rgb, ColorBalance, Curves, Img, Levels, Src};

/// Photoshop-style Brightness/Contrast, both -100..=100.
pub fn brightness_contrast(src: &Src, brightness: f32, contrast: f32) -> Img {
    let b = (brightness / 100.0).clamp(-1.0, 1.0);
    let k = (1.0 + contrast / 100.0).max(0.0);
    src.map(|x, y| {
        map_rgb(src.at(x, y), |c| {
            let m = |v: f32| ((v - 0.5) * k + 0.5 + b).clamp(0.0, 1.0);
            [m(c[0]), m(c[1]), m(c[2])]
        })
    })
}

/// Levels: per-channel curves first, then the master RGB curve, through
/// 256-entry lookup tables.
pub fn levels(src: &Src, l: &Levels) -> Img {
    let lut =
        |ch: &super::LevelsCurve| -> Vec<f32> { (0..256).map(|i| l.rgb.apply(ch.apply(i as f32 / 255.0))).collect() };
    let (lr, lg, lb) = (lut(&l.red), lut(&l.green), lut(&l.blue));
    let idx = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as usize;
    src.map(|x, y| map_rgb(src.at(x, y), |c| [lr[idx(c[0])], lg[idx(c[1])], lb[idx(c[2])]]))
}

/// Curves: per-channel curves first, then the master curve, via 256-entry LUTs.
pub fn curves(src: &Src, c: &Curves) -> Img {
    let lut = |ch: &super::Curve| -> Vec<f32> { (0..256).map(|i| c.rgb.apply(ch.apply(i as f32 / 255.0))).collect() };
    let (lr, lg, lb) = (lut(&c.red), lut(&c.green), lut(&c.blue));
    let idx = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as usize;
    src.map(|x, y| map_rgb(src.at(x, y), |c| [lr[idx(c[0])], lg[idx(c[1])], lb[idx(c[2])]]))
}

/// Photoshop-style Color Balance. Each tonal range's three sliders shift
/// red, green and blue, weighted by how much of the pixel's luminance falls
/// in that range (shadows peak at black, highlights at white, midtones in
/// between); `preserve_luminosity` then restores the original lightness so
/// the tint does not also brighten or darken.
pub fn color_balance(src: &Src, b: &ColorBalance) -> Img {
    let scale = 1.0 / 100.0;
    let sh = b.shadows.map(|v| v * scale);
    let mid = b.midtones.map(|v| v * scale);
    let hi = b.highlights.map(|v| v * scale);
    src.map(|x, y| {
        map_rgb(src.at(x, y), |c| {
            let l = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
            // Smooth tonal weights that sum to about one across the range.
            let ws = (1.0 - l * 2.0).clamp(0.0, 1.0);
            let wh = (l * 2.0 - 1.0).clamp(0.0, 1.0);
            let wm = 1.0 - ws - wh;
            let mut out = [0.0f32; 3];
            for k in 0..3 {
                // Midtone adjustments are the gentlest, as in Photoshop.
                let d = sh[k] * ws * 0.6 + mid[k] * wm * 0.5 + hi[k] * wh * 0.6;
                out[k] = (c[k] + d).clamp(0.0, 1.0);
            }
            if b.preserve_luminosity {
                // Shift all three channels equally until the luma is back
                // where it was: a tint, not a lightening.
                let l1 = 0.299 * out[0] + 0.587 * out[1] + 0.114 * out[2];
                let d = l - l1;
                out = out.map(|v| (v + d).clamp(0.0, 1.0));
            }
            out
        })
    })
}

/// RGB (straight, 0..=1) to HSL with hue in degrees.
#[inline]
pub fn rgb_to_hsl(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d <= 1e-6 {
        return [0.0, 0.0, l];
    }
    let s = if l <= 0.5 { d / (max + min) } else { d / (2.0 - max - min) };
    let h = if max == c[0] {
        60.0 * (((c[1] - c[2]) / d).rem_euclid(6.0))
    } else if max == c[1] {
        60.0 * ((c[2] - c[0]) / d + 2.0)
    } else {
        60.0 * ((c[0] - c[1]) / d + 4.0)
    };
    [h, s, l]
}

#[inline]
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    [r + m, g + m, b + m]
}

/// Photoshop-style Hue/Saturation. `hue` in degrees (a shift, or the target
/// hue when `colorize`), `saturation` and `lightness` in -100..=100
/// (`colorize` uses saturation 0..=100 as an absolute amount).
pub fn hue_saturation(src: &Src, hue: f32, saturation: f32, lightness: f32, colorize: bool) -> Img {
    let sat = (saturation / 100.0).clamp(-1.0, 1.0);
    let light = (lightness / 100.0).clamp(-1.0, 1.0);
    src.map(|x, y| {
        map_rgb(src.at(x, y), |c| {
            let rgb = if colorize {
                let l = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
                hsl_to_rgb(hue, sat.max(0.0), l)
            } else {
                let [h, s, l] = rgb_to_hsl(c);
                let s = if sat < 0.0 { s * (1.0 + sat) } else { s + (1.0 - s) * sat };
                hsl_to_rgb(h + hue, s, l)
            };
            let m = |v: f32| if light >= 0.0 { v + (1.0 - v) * light } else { v * (1.0 + light) };
            [m(rgb[0]), m(rgb[1]), m(rgb[2])]
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default curve is the identity; a raised midpoint brightens the
    /// midtones, leaves the ends alone, and never overshoots.
    #[test]
    fn curve_is_monotone_and_identity_by_default() {
        let c = super::super::Curve::default();
        for i in 0..=255 {
            assert!((c.apply(i as f32 / 255.0) * 255.0 - i as f32).abs() < 0.01);
        }
        let mut s = super::super::Curve::default();
        s.set_point(None, 128.0, 192.0);
        assert_eq!(s.points.len(), 3);
        assert!((s.apply(0.5) - 192.0 / 255.0).abs() < 0.01);
        assert!(s.apply(0.0) < 0.001 && s.apply(1.0) > 0.999);
        let mut prev = -1.0;
        for i in 0..=255 {
            let v = s.apply(i as f32 / 255.0);
            assert!(v >= prev - 1e-6, "curve dips at {i}");
            prev = v;
        }
        // Two points cannot share an input column.
        s.set_point(None, 128.0, 10.0);
        let xs: Vec<f32> = s.points.iter().map(|p| p[0]).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]));
    }

    /// Pushing midtones toward red reddens a mid-gray, leaves lightness
    /// alone when asked to, and does not touch pure black.
    #[test]
    fn color_balance_tints_by_range() {
        use super::super::{apply_filter, Filter};
        use crate::DocState;
        let mut s = DocState::new(2, 1, None);
        s.layers[0].raster.set_pixel(0, 0, crate::Rgba8::new(128, 128, 128, 255));
        s.layers[0].raster.set_pixel(1, 0, crate::Rgba8::new(0, 0, 0, 255));
        let b = ColorBalance { midtones: [100.0, 0.0, 0.0], ..Default::default() };
        apply_filter(&mut s, 0, &Filter::ColorBalance(b));
        let p = s.layers[0].raster.get_pixel(0, 0);
        assert!(p.r > p.g && p.g == p.b, "{p:?}");
        let l = 0.299 * p.r as f32 + 0.587 * p.g as f32 + 0.114 * p.b as f32;
        assert!((l - 128.0).abs() < 8.0, "lightness kept: {l}");
        assert_eq!(s.layers[0].raster.get_pixel(1, 0), crate::Rgba8::new(0, 0, 0, 255));
    }

    #[test]
    fn hsl_roundtrip() {
        for c in [[0.2, 0.7, 0.1], [1.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0.0, 0.3, 0.9]] {
            let [h, s, l] = rgb_to_hsl(c);
            let back = hsl_to_rgb(h, s, l);
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-4, "{c:?} -> {back:?}");
            }
        }
        // A 120° shift turns red into green.
        let [h, s, l] = rgb_to_hsl([1.0, 0.0, 0.0]);
        let g = hsl_to_rgb(h + 120.0, s, l);
        assert!(g[1] > 0.99 && g[0] < 0.01 && g[2] < 0.01);
    }

    /// Hue/Saturation at its neutral settings must not move any color, or
    /// repeatedly opening it drifts the art.
    #[test]
    fn hsl_roundtrip_is_identity() {
        let mut worst = (0i32, [0u8; 3]);
        for r in (0..=255).step_by(5) {
            for g in (0..=255).step_by(5) {
                for b in (0..=255).step_by(5) {
                    let c = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
                    let [h, s, l] = rgb_to_hsl(c);
                    let back = hsl_to_rgb(h, s, l);
                    for k in 0..3 {
                        let d = ((back[k] - c[k]) * 255.0).round() as i32;
                        if d.abs() > worst.0.abs() {
                            worst = (d, [r as u8, g as u8, b as u8]);
                        }
                    }
                }
            }
        }
        assert!(worst.0.abs() <= 1, "rgb->hsl->rgb drifts by {} at {:?}", worst.0, worst.1);
    }
}
