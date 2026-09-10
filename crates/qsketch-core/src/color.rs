//! Color types: 8-bit straight-alpha RGBA, float RGBA helpers, and HSV.

use serde::{Deserialize, Serialize};

/// 8-bit straight (non-premultiplied) RGBA.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    pub const TRANSPARENT: Rgba8 = Rgba8::new(0, 0, 0, 0);
    pub const BLACK: Rgba8 = Rgba8::new(0, 0, 0, 255);
    pub const WHITE: Rgba8 = Rgba8::new(255, 255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const fn from_array(v: [u8; 4]) -> Self {
        Self { r: v[0], g: v[1], b: v[2], a: v[3] }
    }
    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }
    pub fn is_transparent(self) -> bool {
        self.a == 0
    }

    /// Straight RGBA in `0..=1`.
    pub fn to_f32(self) -> [f32; 4] {
        [self.r as f32 / 255.0, self.g as f32 / 255.0, self.b as f32 / 255.0, self.a as f32 / 255.0]
    }

    /// From straight RGBA floats in `0..=1` (clamped, rounded).
    pub fn from_f32(c: [f32; 4]) -> Self {
        Self { r: unit_to_u8(c[0]), g: unit_to_u8(c[1]), b: unit_to_u8(c[2]), a: unit_to_u8(c[3]) }
    }

    /// Premultiplied 8-bit representation.
    pub fn to_premul(self) -> [u8; 4] {
        if self.a == 255 {
            return self.to_array();
        }
        if self.a == 0 {
            return [0; 4];
        }
        let a = self.a as u32;
        [
            ((self.r as u32 * a + 127) / 255) as u8,
            ((self.g as u32 * a + 127) / 255) as u8,
            ((self.b as u32 * a + 127) / 255) as u8,
            self.a,
        ]
    }

    /// `#rrggbb` or `#rrggbbaa` (also accepts 3/4 digit shorthand and no `#`).
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        let expand = |c: u8| -> Option<u8> {
            let v = (c as char).to_digit(16)? as u8;
            Some(v * 17)
        };
        match s.len() {
            3 | 4 => {
                let b = s.as_bytes();
                let r = expand(b[0])?;
                let g = expand(b[1])?;
                let bl = expand(b[2])?;
                let a = if s.len() == 4 { expand(b[3])? } else { 255 };
                Some(Self::new(r, g, bl, a))
            }
            6 | 8 => {
                let v = u32::from_str_radix(s, 16).ok()?;
                if s.len() == 6 {
                    Some(Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8, 255))
                } else {
                    Some(Self::new((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8))
                }
            }
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    pub fn to_hsv(self) -> Hsv {
        let [r, g, b, _] = self.to_f32();
        Hsv::from_rgb(r, g, b)
    }

    /// Perceived luminance (Rec. 601), 0..=1.
    pub fn luma(self) -> f32 {
        let [r, g, b, _] = self.to_f32();
        0.299 * r + 0.587 * g + 0.114 * b
    }

    /// Max per-channel absolute difference, used by tolerance-based fills.
    pub fn max_channel_diff(self, o: Rgba8) -> u8 {
        let d = |a: u8, b: u8| a.abs_diff(b);
        d(self.r, o.r).max(d(self.g, o.g)).max(d(self.b, o.b)).max(d(self.a, o.a))
    }
}

#[inline]
pub fn unit_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// HSV color with hue in degrees `0..360`, saturation and value in `0..=1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    pub fn new(h: f32, s: f32, v: f32) -> Self {
        Self { h: h.rem_euclid(360.0), s: s.clamp(0.0, 1.0), v: v.clamp(0.0, 1.0) }
    }

    pub fn from_rgb(r: f32, g: f32, b: f32) -> Self {
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d <= 1e-6 {
            0.0
        } else if max == r {
            60.0 * (((g - b) / d) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        let s = if max <= 1e-6 { 0.0 } else { d / max };
        Self::new(h, s, max)
    }

    pub fn to_rgb(self) -> [f32; 3] {
        let c = self.v * self.s;
        let hp = self.h / 60.0;
        let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
        let (r, g, b) = match hp as i32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = self.v - c;
        [r + m, g + m, b + m]
    }

    pub fn to_rgba8(self, a: u8) -> Rgba8 {
        let [r, g, b] = self.to_rgb();
        Rgba8::new(unit_to_u8(r), unit_to_u8(g), unit_to_u8(b), a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let c = Rgba8::new(0x12, 0xab, 0xff, 0x80);
        assert_eq!(Rgba8::from_hex(&c.to_hex()), Some(c));
        assert_eq!(Rgba8::from_hex("#fff"), Some(Rgba8::WHITE));
        assert_eq!(Rgba8::from_hex("000000"), Some(Rgba8::BLACK));
        assert_eq!(Rgba8::from_hex("zz"), None);
    }

    #[test]
    fn hsv_roundtrip() {
        for c in [Rgba8::rgb(255, 0, 0), Rgba8::rgb(10, 200, 90), Rgba8::rgb(128, 128, 128)] {
            let back = c.to_hsv().to_rgba8(255);
            assert!(c.r.abs_diff(back.r) <= 1 && c.g.abs_diff(back.g) <= 1 && c.b.abs_diff(back.b) <= 1);
        }
    }

    #[test]
    fn premul() {
        assert_eq!(Rgba8::new(255, 255, 255, 128).to_premul(), [128, 128, 128, 128]);
    }
}
