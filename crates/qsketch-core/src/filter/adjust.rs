//! Color adjustments that run through the filter pipeline so they get the
//! same live preview, selection handling and multi-layer application.

use super::{map_rgb, Img, Levels, Src};

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
