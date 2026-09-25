//! Startup animation: the app icon and wordmark on the theme background for
//! a moment, then the whole thing dissolves into the workspace. Painted on
//! top of everything for the first `DURATION` seconds; any key or click
//! ends it early. Toggle in Preferences ▸ Interface.

use std::time::Instant;

use egui::{Align2, Color32, Context, FontId, Id, LayerId, Order, Pos2, Rect, Stroke, TextureHandle};

use crate::ui::theme::Palette;

/// Whole animation, in seconds.
pub const DURATION: f32 = 1.9;

pub struct Splash {
    /// Set on the second painted frame: the first one is painted before the
    /// window is shown (and can take a while on a cold GPU), so timing from
    /// it would eat into the animation.
    started: Option<Instant>,
}

impl Default for Splash {
    fn default() -> Self {
        Self::new()
    }
}

impl Splash {
    pub fn new() -> Self {
        Self { started: None }
    }

    /// Keep the animation at its first frame (restart the clock).
    pub fn hold(&mut self) {
        self.started = None;
    }

    /// Paint this frame of the animation. Returns false once it is over (or
    /// was skipped), so the caller can drop it.
    pub fn paint(&mut self, ctx: &Context, p: &Palette, icon: &TextureHandle) -> bool {
        let t = match self.started {
            Some(s) => s.elapsed().as_secs_f32() / DURATION,
            None => {
                self.started = Some(Instant::now());
                0.0
            }
        };
        if t >= 1.0 {
            return false;
        }
        // Skip on any input once it has been visible for a beat (a click
        // that launched the app must not cancel it).
        let skipped = ctx.input(|i| {
            i.pointer.any_pressed() || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
        });
        if t > 0.15 && skipped {
            return false;
        }
        ctx.request_repaint();

        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("qsketch_splash")));
        // Overall opacity: opaque, then a dissolve over the last third.
        let a = 1.0 - smoothstep(0.68, 1.0, t);
        let with_a = |c: Color32, k: f32| c.gamma_multiply(k);
        painter.rect_filled(screen, 0.0, with_a(p.bg, a));

        let center = screen.center() - egui::vec2(0.0, 24.0);
        // Everything drifts up a little as it dissolves.
        let lift = -14.0 * smoothstep(0.66, 1.0, t);
        let c = Pos2::new(center.x, center.y + lift);

        // Icon: eases in from slightly small, with a soft accent halo.
        let appear = ease_out(clamp01(t / 0.35));
        let size = 104.0 * (0.86 + 0.14 * appear);
        let halo_r = size * (0.95 + 0.25 * ease_out(clamp01((t - 0.1) / 0.6)));
        let halo = with_a(p.accent, 0.10 * appear * a);
        painter.circle_filled(c, halo_r, halo);
        painter.circle_stroke(c, halo_r * 1.18, Stroke::new(1.0, with_a(p.accent, 0.12 * appear * a)));
        let icon_rect = Rect::from_center_size(c, egui::Vec2::splat(size));
        painter.image(
            icon.id(),
            icon_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE.gamma_multiply(appear * a),
        );

        // Wordmark, then a hairline in the accent color that grows from the
        // middle outward under it.
        let word_a = ease_out(clamp01((t - 0.12) / 0.3)) * a;
        let word_pos = Pos2::new(c.x, icon_rect.bottom() + 22.0);
        painter.text(word_pos, Align2::CENTER_TOP, "qsketch", FontId::proportional(30.0), with_a(p.text, word_a));
        let line_w = 150.0 * ease_out(clamp01((t - 0.22) / 0.4));
        let line_y = word_pos.y + 44.0;
        if line_w > 1.0 {
            painter.line_segment(
                [Pos2::new(c.x - line_w / 2.0, line_y), Pos2::new(c.x + line_w / 2.0, line_y)],
                Stroke::new(1.5, with_a(p.accent, a)),
            );
        }
        let ver_a = ease_out(clamp01((t - 0.3) / 0.3)) * a;
        painter.text(
            Pos2::new(c.x, line_y + 10.0),
            Align2::CENTER_TOP,
            format!("v{}", qsketch_core::VERSION),
            FontId::proportional(12.0),
            with_a(p.text_dim, ver_a),
        );
        true
    }
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn ease_out(v: f32) -> f32 {
    1.0 - (1.0 - v) * (1.0 - v) * (1.0 - v)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}
