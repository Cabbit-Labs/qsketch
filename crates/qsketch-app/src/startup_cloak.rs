//! Flash-free first show on Windows.
//!
//! Two things flashed a blank white window before the first frame:
//! - winit 0.30 calls `ShowWindow(SW_MAXIMIZE)` on every style update of a
//!   window whose maximized flag is set, and that shows it. Creating the
//!   window hidden *and* maximized therefore shows and re-hides it several
//!   times during setup, long before anything is painted. So on Windows the
//!   window is created at its normal size and maximized from here instead.
//! - eframe shows the window after painting the first frame, but a frame
//!   presented to a hidden window is not kept, so it appears blank until the
//!   next present lands.
//!
//! The window is DWM-cloaked as soon as the app gets its handle, maximized
//! and painted while cloaked, and uncloaked once frames at the final size
//! have been presented while visible. Everywhere else this does nothing.

/// Frames to present at the final geometry before uncloaking: one while
/// visible, plus one more so that present has been flipped.
const SETTLED_FRAMES: u32 = 2;
/// Give up waiting for the maximize (and uncloak anyway) after this many frames.
const MAX_FRAMES: u32 = 90;

/// Whether the window builder may ask for a maximized window. See the module
/// docs: on Windows the maximize is applied by [`StartupCloak`] instead.
pub fn builder_maximized(maximized: bool) -> bool {
    maximized && !cfg!(windows)
}

pub struct StartupCloak {
    #[cfg(windows)]
    hwnd: windows_sys::Win32::Foundation::HWND,
    want_maximized: bool,
    frames: u32,
    settled: u32,
}

impl StartupCloak {
    #[cfg(windows)]
    pub fn install(cc: &eframe::CreationContext<'_>, maximized: bool) -> Option<Self> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let hwnd = cc.window_handle().ok().and_then(|handle| match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as windows_sys::Win32::Foundation::HWND),
            _ => None,
        });
        if maximized {
            // Even uncloaked, a late maximize beats a blank flash.
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        let hwnd = hwnd.filter(|&h| set_cloak(h, true))?;
        Some(Self { hwnd, want_maximized: maximized, frames: 0, settled: 0 })
    }

    #[cfg(not(windows))]
    pub fn install(_cc: &eframe::CreationContext<'_>, _maximized: bool) -> Option<Self> {
        None
    }

    /// Call once per frame, after painting. Returns false once the window
    /// has been uncloaked.
    pub fn tick(&mut self, ctx: &egui::Context) -> bool {
        self.frames += 1;
        // The first frame is painted before eframe shows the window.
        let geometry_final = !self.want_maximized || ctx.input(|i| i.viewport().maximized).unwrap_or(false);
        if self.frames > 1 && geometry_final {
            self.settled += 1;
        }
        if self.settled <= SETTLED_FRAMES && self.frames < MAX_FRAMES {
            // Nothing else may ask for these frames (startup animation off).
            ctx.request_repaint();
            return true;
        }
        #[cfg(windows)]
        set_cloak(self.hwnd, false);
        false
    }
}

#[cfg(windows)]
fn set_cloak(hwnd: windows_sys::Win32::Foundation::HWND, on: bool) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
    let v = i32::from(on);
    let hr = unsafe { DwmSetWindowAttribute(hwnd, DWMWA_CLOAK as u32, (&raw const v).cast(), size_of::<i32>() as u32) };
    hr >= 0
}
