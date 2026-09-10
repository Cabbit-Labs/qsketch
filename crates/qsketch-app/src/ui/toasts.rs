//! Lightweight, non-blocking notifications shown in the bottom-right corner.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Error,
}

struct Toast {
    text: String,
    level: Level,
    created: Instant,
    ttl: Duration,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
        log::info!("toast: {text}");
        let ttl = Duration::from_millis(if level == Level::Error { 6000 } else { 3000 });
        self.items.push(Toast { text, level, created: Instant::now(), ttl });
        if self.items.len() > 5 {
            self.items.remove(0);
        }
    }

    #[allow(dead_code)]
    pub fn info(&mut self, text: impl Into<String>) {
        self.push(Level::Info, text);
    }
    #[allow(dead_code)]
    pub fn success(&mut self, text: impl Into<String>) {
        self.push(Level::Success, text);
    }
    #[allow(dead_code)]
    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Level::Error, text);
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.items.retain(|t| now.duration_since(t.created) < t.ttl);
        if self.items.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Area::new(egui::Id::new("qsketch_toasts"))
            .anchor(Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -32.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
                    for t in self.items.iter().rev() {
                        let age = now.duration_since(t.created).as_secs_f32();
                        let remaining = t.ttl.as_secs_f32() - age;
                        let alpha = (remaining / 0.4).clamp(0.0, 1.0);
                        let (bar, glyph) = match t.level {
                            Level::Info => (Color32::from_rgb(90, 150, 240), "i"),
                            Level::Success => (Color32::from_rgb(23, 200, 160), "✓"),
                            Level::Error => (Color32::from_rgb(235, 87, 87), "!"),
                        };
                        egui::Frame::new()
                            .fill(ui.visuals().window_fill.gamma_multiply(alpha))
                            .stroke(egui::Stroke::new(1.0, bar.gamma_multiply(alpha)))
                            .corner_radius(6)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(glyph).color(bar.gamma_multiply(alpha)).strong());
                                    ui.label(
                                        RichText::new(&t.text).color(ui.visuals().text_color().gamma_multiply(alpha)),
                                    );
                                });
                            });
                    }
                });
            });
    }
}
