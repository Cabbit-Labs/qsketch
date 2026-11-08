//! Frame pacing log for diagnosing stutter. Set `QSKETCH_FRAME_STATS=1` and
//! every two seconds of painting logs the frame count and interval spread.
//! Idle gaps (the app paints on demand) are left out of the numbers.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_secs(2);
/// Longer than this between frames means the app was idle, not stuttering.
const IDLE_GAP: Duration = Duration::from_millis(250);
/// Frame-rate cap when presenting without vsync (about 120 fps).
const MIN_FRAME_INTERVAL: Duration = Duration::from_micros(8300);

struct Stats {
    last: Option<Instant>,
    window_start: Instant,
    intervals_ms: Vec<f32>,
}

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("QSKETCH_FRAME_STATS").is_some())
}

/// Whether to present without waiting for vblank: X11 on Linux, unless
/// `QSKETCH_VSYNC` is set. Vsynced Vulkan presents there were measured
/// stalling for 5-7 refreshes at a time (NVIDIA, Cinnamon's compositor), and
/// the compositor already syncs what reaches the screen.
pub fn x11_uncapped_present() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        cfg!(target_os = "linux")
            && std::env::var_os("WAYLAND_DISPLAY").is_none()
            && std::env::var_os("QSKETCH_VSYNC").is_none()
    })
}

/// Without vsync nothing else limits a continuously repainting app.
fn cap_frame_rate() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(wait) = last.and_then(|l| MIN_FRAME_INTERVAL.checked_sub(l.elapsed())) {
        std::thread::sleep(wait);
    }
    *last = Some(Instant::now());
}

/// Call once per frame.
pub fn tick() {
    if x11_uncapped_present() {
        cap_frame_rate();
    }
    if !enabled() {
        return;
    }
    static STATS: Mutex<Option<Stats>> = Mutex::new(None);
    let mut guard = STATS.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let s = guard.get_or_insert_with(|| Stats { last: None, window_start: now, intervals_ms: Vec::new() });
    if let Some(dt) = s.last.map(|l| now - l).filter(|dt| *dt < IDLE_GAP) {
        s.intervals_ms.push(dt.as_secs_f32() * 1000.0);
    }
    s.last = Some(now);
    if now - s.window_start >= REPORT_EVERY {
        if !s.intervals_ms.is_empty() {
            s.intervals_ms.sort_by(f32::total_cmp);
            let v = &s.intervals_ms;
            let at = |q: f32| v[((v.len() - 1) as f32 * q) as usize];
            log::info!(
                "frame stats: n={} median={:.1}ms p95={:.1}ms p99={:.1}ms max={:.1}ms",
                v.len(),
                at(0.5),
                at(0.95),
                at(0.99),
                v[v.len() - 1]
            );
        }
        s.intervals_ms.clear();
        s.window_start = now;
    }
}
