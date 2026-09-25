//! Startup trace: for the first few seconds after launch, every change to
//! the window's geometry, scale, focus or occlusion is appended to
//! `startup-trace.log` in the config directory, with a timestamp. It is the
//! evidence for "the window flashes when it opens" reports from machines we
//! cannot watch: each line is one thing the OS did to the window.

use std::fs::File;
use std::io::Write;
use std::time::Instant;

const SECONDS: f32 = 6.0;

pub struct StartupTrace {
    started: Instant,
    file: Option<File>,
    last: String,
    frames: u32,
    done: bool,
}

impl StartupTrace {
    pub fn new() -> Self {
        let file = crate::settings::Settings::config_dir().and_then(|d| {
            std::fs::create_dir_all(&d).ok()?;
            File::create(d.join("startup-trace.log")).ok()
        });
        let mut t = Self { started: Instant::now(), file, last: String::new(), frames: 0, done: false };
        t.line(&format!(
            "qsketch {} · {} · backend {}",
            qsketch_core::VERSION,
            std::env::consts::OS,
            std::env::var("WGPU_BACKEND").unwrap_or_else(|_| "auto".into())
        ));
        t
    }

    /// Note what the builder asked for, before the window exists.
    pub fn note(&mut self, what: &str) {
        self.line(what);
    }

    fn line(&mut self, s: &str) {
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{:>7.3}s  {s}", self.started.elapsed().as_secs_f32());
        }
    }

    /// Called with each frame's raw input. Cheap after the window is up.
    pub fn frame(&mut self, raw: &egui::RawInput) {
        if self.done {
            return;
        }
        if self.started.elapsed().as_secs_f32() > SECONDS {
            self.line(&format!("trace ends after {} frames", self.frames));
            self.done = true;
            self.file = None;
            return;
        }
        self.frames += 1;
        let v = raw.viewport();
        let state = format!(
            "screen={:?} ppp={:.2} inner={:?} outer={:?} monitor={:?} maximized={:?} fullscreen={:?} focused={:?} minimized={:?}",
            raw.screen_rect.map(|r| [r.width(), r.height()]),
            raw.viewport().native_pixels_per_point.unwrap_or(1.0),
            v.inner_rect.map(|r| [r.min.x, r.min.y, r.width(), r.height()]),
            v.outer_rect.map(|r| [r.min.x, r.min.y, r.width(), r.height()]),
            v.monitor_size.map(|m| [m.x, m.y]),
            v.maximized,
            v.fullscreen,
            v.focused,
            v.minimized,
        );
        if state != self.last {
            let n = self.frames;
            self.line(&format!("frame {n}: {state}"));
            self.last = state;
        }
        for e in &raw.events {
            if let egui::Event::WindowFocused(f) = e {
                let n = self.frames;
                self.line(&format!("frame {n}: window focused={f}"));
            }
        }
    }
}
