//! Windows-only: watch the raw `WM_POINTER*` pen messages for the barrel
//! (side) button and eraser-end state. winit folds every pen contact into a
//! Touch event and drops the pen flags, so a barrel button the tablet driver
//! maps to right-click would otherwise arrive as a left press and the eraser
//! end would be indistinguishable from the tip. A window subclass runs ahead
//! of winit's procedure and records the flags; the app reads them through
//! [`barrel_held`] and [`eraser`].

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Input::Pointer::{
        GetPointerPenInfo, GetPointerType, POINTER_FLAG_SECONDBUTTON, POINTER_PEN_INFO,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        PEN_FLAG_BARREL, PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PT_PEN, WM_POINTERDOWN, WM_POINTERLEAVE, WM_POINTERUP,
        WM_POINTERUPDATE,
    };

    static BARREL: AtomicBool = AtomicBool::new(false);
    static ERASER: AtomicBool = AtomicBool::new(false);
    const SUBCLASS_ID: usize = 0x71534b; // "qSk"

    pub fn install(cc: &eframe::CreationContext<'_>) {
        let Ok(handle) = cc.window_handle() else { return };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
        let hwnd = h.hwnd.get() as HWND;
        // SAFETY: hwnd is the live winit window; the subclass proc only touches
        // process-wide atomics and pointer-info queries.
        let ok = unsafe { SetWindowSubclass(hwnd, Some(proc), SUBCLASS_ID, 0) };
        if ok == 0 {
            log::warn!("pen barrel watcher: SetWindowSubclass failed");
        } else {
            log::info!("pen barrel watcher installed");
        }
    }

    pub fn barrel_held() -> bool {
        BARREL.load(Ordering::Relaxed)
    }

    /// The pen is in range with its eraser end (inverted, or a dedicated
    /// eraser tip) towards the tablet.
    pub fn eraser() -> bool {
        ERASER.load(Ordering::Relaxed)
    }

    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match msg {
            WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
                let id = (wparam & 0xffff) as u32;
                let mut ty = 0;
                if GetPointerType(id, &mut ty) != 0 && ty == PT_PEN {
                    let mut info: POINTER_PEN_INFO = std::mem::zeroed();
                    if GetPointerPenInfo(id, &mut info) != 0 {
                        let held = info.penFlags & PEN_FLAG_BARREL != 0
                            || info.pointerInfo.pointerFlags & POINTER_FLAG_SECONDBUTTON != 0;
                        BARREL.store(held, Ordering::Relaxed);
                        let eraser = info.penFlags & (PEN_FLAG_INVERTED | PEN_FLAG_ERASER) != 0;
                        ERASER.store(eraser, Ordering::Relaxed);
                    }
                }
            }
            WM_POINTERLEAVE => {
                BARREL.store(false, Ordering::Relaxed);
                ERASER.store(false, Ordering::Relaxed);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}

#[cfg(windows)]
pub use imp::{barrel_held, eraser, install};

#[cfg(not(windows))]
pub fn install(_cc: &eframe::CreationContext<'_>) {}

#[cfg(not(windows))]
pub fn barrel_held() -> bool {
    false
}

#[cfg(not(windows))]
pub fn eraser() -> bool {
    false
}
