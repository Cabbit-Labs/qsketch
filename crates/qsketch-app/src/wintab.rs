//! Windows-only WinTab backend (the Wacom driver API that works with
//! "Use Windows Ink" turned off). Loads `Wintab32.dll` at runtime, opens a
//! system context on the window and polls its packet queue every frame for
//! position, pressure, tilt, cursor type (eraser) and barrel buttons.
//!
//! Button presses and releases still come from the windowing system: with Ink
//! off the driver emulates a mouse, so a tip contact is a left press and a
//! barrel button is whatever the driver maps it to. WinTab supplies what mouse
//! emulation loses, and its packets replace the mouse motion while the tip is
//! down (see `tablet_samples`).

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    use crate::state::AppState;

    type Hctx = *mut c_void;

    // wintab.h constants.
    const WTI_DEFSYSCTX: u32 = 4;
    const WTI_DEVICES: u32 = 100;
    const WTI_CURSORS: u32 = 200;
    const DVC_NPRESSURE: u32 = 15;
    const DVC_ORIENTATION: u32 = 17;
    const CSR_NAME: u32 = 1;
    const CXO_SYSTEM: u32 = 0x0001;
    const CXO_MESSAGES: u32 = 0x0004;
    const PK_STATUS: u32 = 0x0002;
    const PK_CURSOR: u32 = 0x0020;
    const PK_BUTTONS: u32 = 0x0040;
    const PK_X: u32 = 0x0080;
    const PK_Y: u32 = 0x0100;
    const PK_NORMAL_PRESSURE: u32 = 0x0400;
    const PK_ORIENTATION: u32 = 0x1000;
    const PACKETDATA: u32 = PK_STATUS | PK_CURSOR | PK_BUTTONS | PK_X | PK_Y | PK_NORMAL_PRESSURE | PK_ORIENTATION;
    const TPS_INVERT: u32 = 0x0010;
    /// Default message base; `WT_PROXIMITY = WT_DEFBASE + 5` is watched by
    /// the window subclass in `win_pointer`.
    pub const WT_DEFBASE: u32 = 0x7FF0;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LogContextW {
        name: [u16; 40],
        options: u32,
        status: u32,
        locks: u32,
        msg_base: u32,
        device: u32,
        pkt_rate: u32,
        pkt_data: u32,
        pkt_mode: u32,
        move_mask: u32,
        btn_dn_mask: u32,
        btn_up_mask: u32,
        in_org: [i32; 3],
        in_ext: [i32; 3],
        out_org: [i32; 3],
        out_ext: [i32; 3],
        sens: [u32; 3],
        sys_mode: i32,
        sys_org: [i32; 2],
        sys_ext: [i32; 2],
        sys_sens: [u32; 2],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Axis {
        min: i32,
        max: i32,
        units: u32,
        resolution: u32,
    }

    /// Field order follows the `PK_*` bit order of `PACKETDATA`.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Packet {
        status: u32,
        cursor: u32,
        buttons: u32,
        x: i32,
        y: i32,
        pressure: u32,
        azimuth: i32,
        altitude: i32,
        twist: i32,
    }

    type WtInfoW = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
    type WtOpenW = unsafe extern "system" fn(HWND, *mut LogContextW, i32) -> Hctx;
    type WtClose = unsafe extern "system" fn(Hctx) -> i32;
    type WtPacketsGet = unsafe extern "system" fn(Hctx, i32, *mut c_void) -> i32;
    type WtOverlap = unsafe extern "system" fn(Hctx, i32) -> i32;
    type WtQueueSizeSet = unsafe extern "system" fn(Hctx, i32) -> i32;

    struct Api {
        info: WtInfoW,
        open: WtOpenW,
        close: WtClose,
        packets_get: WtPacketsGet,
        overlap: WtOverlap,
        queue_size_set: Option<WtQueueSizeSet>,
    }

    unsafe fn sym<T: Copy>(lib: *mut c_void, name: &[u8]) -> Option<T> {
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
        let p = GetProcAddress(lib, name.as_ptr())?;
        Some(std::mem::transmute_copy(&p))
    }

    impl Api {
        unsafe fn load() -> Option<Self> {
            let name: Vec<u16> = "Wintab32.dll\0".encode_utf16().collect();
            let lib = LoadLibraryW(name.as_ptr());
            if lib.is_null() {
                return None;
            }
            Some(Self {
                info: sym(lib, b"WTInfoW\0")?,
                open: sym(lib, b"WTOpenW\0")?,
                close: sym(lib, b"WTClose\0")?,
                packets_get: sym(lib, b"WTPacketsGet\0")?,
                overlap: sym(lib, b"WTOverlap\0")?,
                queue_size_set: sym(lib, b"WTQueueSizeSet\0"),
            })
        }
    }

    pub struct WinTab {
        api: Api,
        ctx: Hctx,
        hwnd: HWND,
        /// Virtual-desktop origin and extent the packets are mapped onto.
        sys_org: [i32; 2],
        sys_ext: [i32; 2],
        pressure_max: f32,
        /// `altitude` axis max (0 when the device reports no orientation).
        altitude_max: f32,
        was_focused: bool,
        /// Cursor index of the last packet; `WTI_CURSORS` names are logged once
        /// per cursor to help diagnose eraser detection.
        logged_cursors: Vec<u32>,
        in_contact: bool,
    }

    // SAFETY: the context handle is only used from the UI thread that owns it.
    unsafe impl Send for WinTab {}

    impl WinTab {
        pub fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
            let Ok(handle) = cc.window_handle() else { return None };
            let RawWindowHandle::Win32(h) = handle.as_raw() else { return None };
            let hwnd = h.hwnd.get() as HWND;
            // SAFETY: plain FFI into the tablet driver DLL with the documented
            // structure layouts; `ctx` is zero-initialized before use.
            unsafe {
                let api = Api::load()?;
                let mut lc: LogContextW = std::mem::zeroed();
                if (api.info)(WTI_DEFSYSCTX, 0, &mut lc as *mut _ as *mut c_void) == 0 {
                    log::info!("wintab: no default system context (driver not running?)");
                    return None;
                }
                lc.options |= CXO_SYSTEM | CXO_MESSAGES;
                lc.msg_base = WT_DEFBASE;
                lc.pkt_data = PACKETDATA;
                lc.pkt_mode = 0; // everything absolute
                lc.move_mask = PACKETDATA;
                lc.btn_up_mask = lc.btn_dn_mask;
                // Map the tablet onto the virtual desktop, WinTab's origin is
                // bottom-left so `y` is flipped in `pump`.
                let sys_org = lc.sys_org;
                let sys_ext = lc.sys_ext;
                lc.out_org = [0, 0, 0];
                lc.out_ext = [sys_ext[0], sys_ext[1], 0];
                let ctx = (api.open)(hwnd, &mut lc, 1);
                if ctx.is_null() {
                    log::warn!("wintab: WTOpen failed");
                    return None;
                }
                if let Some(qs) = api.queue_size_set {
                    qs(ctx, 256);
                }
                let mut axis = Axis::default();
                (api.info)(WTI_DEVICES + lc.device, DVC_NPRESSURE, &mut axis as *mut _ as *mut c_void);
                let pressure_max = (axis.max.max(1)) as f32;
                let mut orient = [Axis::default(); 3];
                let altitude_max =
                    if (api.info)(WTI_DEVICES + lc.device, DVC_ORIENTATION, orient.as_mut_ptr() as *mut c_void) != 0 {
                        orient[1].max as f32
                    } else {
                        0.0
                    };
                log::info!(
                    "wintab backend: device {} pressure 0..{} altitude max {} desktop {:?}+{:?}",
                    lc.device,
                    pressure_max,
                    altitude_max,
                    sys_org,
                    sys_ext
                );
                Some(Self {
                    api,
                    ctx,
                    hwnd,
                    sys_org,
                    sys_ext,
                    pressure_max,
                    altitude_max,
                    was_focused: false,
                    logged_cursors: Vec::new(),
                    in_contact: false,
                })
            }
        }

        /// Drain the packet queue, feeding pen state and stroke samples.
        pub fn pump(&mut self, state: &mut AppState, egui_ctx: &egui::Context) {
            let focused = egui_ctx.input(|i| i.focused);
            if focused && !self.was_focused {
                // Bring our context to the top of the driver's overlap order.
                // SAFETY: valid context handle.
                unsafe { (self.api.overlap)(self.ctx, 1) };
            }
            self.was_focused = focused;

            let proximity = crate::win_pointer::wintab_proximity();
            state.pen.tablet_active = proximity;
            if !proximity {
                state.pen.pressure = None;
                state.pen.in_contact = false;
                state.pen.eraser = false;
                state.pen.barrel_held = false;
                self.in_contact = false;
            }

            let mut origin = POINT { x: 0, y: 0 };
            // SAFETY: live window handle.
            unsafe { ClientToScreen(self.hwnd, &mut origin) };
            let ppp = egui_ctx.pixels_per_point();

            let mut buf = [Packet::default(); 64];
            loop {
                // SAFETY: the buffer matches the packet layout requested in the
                // context's `pkt_data`.
                let n = unsafe { (self.api.packets_get)(self.ctx, buf.len() as i32, buf.as_mut_ptr() as *mut c_void) };
                if n <= 0 {
                    break;
                }
                for pk in &buf[..n as usize] {
                    self.apply(pk, state, origin, ppp);
                }
                if (n as usize) < buf.len() {
                    break;
                }
            }
        }

        fn apply(&mut self, pk: &Packet, state: &mut AppState, origin: POINT, ppp: f32) {
            state.pen.tablet_active = true;
            if !self.logged_cursors.contains(&pk.cursor) {
                self.logged_cursors.push(pk.cursor);
                let mut name = [0u16; 64];
                // SAFETY: fixed-size output buffer for a string query.
                let len =
                    unsafe { (self.api.info)(WTI_CURSORS + pk.cursor, CSR_NAME, name.as_mut_ptr() as *mut c_void) };
                let name = String::from_utf16_lossy(&name[..(len as usize / 2).min(64)]);
                log::info!("wintab cursor {} {:?} status {:#x}", pk.cursor, name.trim_end_matches('\0'), pk.status);
            }
            // Cursor indices come in triples per device: puck, pen tip, eraser.
            state.pen.eraser = pk.cursor % 3 == 2 || pk.status & TPS_INVERT != 0;
            // Bit 0 is the tip; anything else is a barrel button.
            state.pen.barrel_held = pk.buttons & !1 != 0;

            if self.altitude_max > 0.0 {
                let az = (pk.azimuth as f32 / 10.0).to_radians();
                let alt = (pk.altitude.abs() as f32 / 10.0).to_radians();
                let tan_alt = alt.tan().max(1e-3);
                state.pen.tilt = [(az.sin() / tan_alt).atan(), -(az.cos() / tan_alt).atan()];
            }

            let p = (pk.pressure as f32 / self.pressure_max).clamp(0.0, 1.0);
            let contact = pk.buttons & 1 != 0 || p > 0.0;
            // Screen pixel (WinTab y grows upwards) -> window client -> egui points.
            let sx = self.sys_org[0] + pk.x;
            let sy = self.sys_org[1] + self.sys_ext[1] - pk.y;
            let pos = egui::pos2((sx - origin.x) as f32 / ppp, (sy - origin.y) as f32 / ppp);
            if contact {
                if !self.in_contact {
                    log::debug!("wintab contact at {pos:?} (packet {},{})", pk.x, pk.y);
                }
                state.pen.pressure = Some(p);
                state.pen.in_contact = true;
                state.tablet_samples.push((pos, p));
            } else {
                state.pen.pressure = None;
                state.pen.in_contact = false;
            }
            self.in_contact = contact;
        }
    }

    impl Drop for WinTab {
        fn drop(&mut self) {
            // SAFETY: closing the context we opened.
            unsafe { (self.api.close)(self.ctx) };
        }
    }
}

#[cfg(windows)]
pub use imp::{WinTab, WT_DEFBASE};

#[cfg(not(windows))]
pub struct WinTab;

#[cfg(not(windows))]
impl WinTab {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Option<Self> {
        None
    }
    pub fn pump(&mut self, _state: &mut crate::state::AppState, _ctx: &egui::Context) {}
}
