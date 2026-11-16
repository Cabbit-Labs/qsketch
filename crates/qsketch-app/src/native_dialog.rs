//! The system's file dialogs, shown so the app behaves while one is open.
//!
//! A dialog blocks the app until it is answered. Two things go wrong when
//! that is done naively: the dialog is a window of its own, so it can end up
//! behind the app with nothing saying it is there; and the app's window stops
//! answering the window manager's "are you alive?" pings, so clicking it gets
//! a "not responding: wait or force quit?" prompt. Here the dialog is made a
//! child of the app's window (it stays on top of it and takes the clicks),
//! and on X11 the window stops advertising that it answers pings for as long
//! as the dialog is open.

use std::path::PathBuf;
use std::sync::OnceLock;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

/// The app's window, as the dialogs' parent.
struct Parent {
    window: RawWindowHandle,
    display: RawDisplayHandle,
}

// The handles are plain ids and pointers owned by the windowing layer for the
// life of the process; they are only handed to the dialog backend, from the
// UI thread.
unsafe impl Send for Parent {}
unsafe impl Sync for Parent {}

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // Safety: the window lives as long as the app does.
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

impl HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // Safety: as above.
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

static PARENT: OnceLock<Parent> = OnceLock::new();

/// Remember the app's window (call once it exists; later calls do nothing).
pub fn set_parent(frame: &eframe::Frame) {
    if PARENT.get().is_some() {
        return;
    }
    if let (Ok(w), Ok(d)) = (frame.window_handle(), frame.display_handle()) {
        let _ = PARENT.set(Parent { window: w.as_raw(), display: d.as_raw() });
    }
}

/// Showing a dialog: `dlg.ask_file()` instead of `dlg.pick_file()`, and so on.
pub trait Ask {
    fn ask_file(self) -> Option<PathBuf>;
    fn ask_files(self) -> Option<Vec<PathBuf>>;
    fn ask_folder(self) -> Option<PathBuf>;
    fn ask_save(self) -> Option<PathBuf>;
}

fn child(dlg: rfd::FileDialog) -> rfd::FileDialog {
    match PARENT.get() {
        Some(p) => dlg.set_parent(p),
        None => dlg,
    }
}

impl Ask for rfd::FileDialog {
    fn ask_file(self) -> Option<PathBuf> {
        let _quiet = ping::Quiet::new();
        child(self).pick_file()
    }

    fn ask_files(self) -> Option<Vec<PathBuf>> {
        let _quiet = ping::Quiet::new();
        child(self).pick_files()
    }

    fn ask_folder(self) -> Option<PathBuf> {
        let _quiet = ping::Quiet::new();
        child(self).pick_folder()
    }

    fn ask_save(self) -> Option<PathBuf> {
        let _quiet = ping::Quiet::new();
        child(self).save_file()
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod ping {
    use raw_window_handle::RawWindowHandle;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
    use x11rb::wrapper::ConnectionExt as _;

    /// While this lives, the app's X11 window does not list `_NET_WM_PING`
    /// among its protocols, so the window manager does not ping it (and
    /// cannot find it unresponsive). Dropping it puts the list back. Does
    /// nothing on Wayland or when anything about X11 fails.
    pub struct Quiet(Option<(x11rb::rust_connection::RustConnection, u32, u32, Vec<u32>)>);

    impl Quiet {
        pub fn new() -> Self {
            Quiet(Self::silence())
        }

        fn silence() -> Option<(x11rb::rust_connection::RustConnection, u32, u32, Vec<u32>)> {
            let window = match super::PARENT.get()?.window {
                RawWindowHandle::Xlib(h) => h.window as u32,
                RawWindowHandle::Xcb(h) => h.window.get(),
                _ => return None,
            };
            let (conn, _) = x11rb::connect(None).ok()?;
            let protocols = conn.intern_atom(false, b"WM_PROTOCOLS").ok()?.reply().ok()?.atom;
            let ping = conn.intern_atom(false, b"_NET_WM_PING").ok()?.reply().ok()?.atom;
            let reply = conn.get_property(false, window, protocols, AtomEnum::ATOM, 0, 64).ok()?.reply().ok()?;
            let all: Vec<u32> = reply.value32()?.collect();
            if !all.contains(&ping) {
                return None;
            }
            let without: Vec<u32> = all.iter().copied().filter(|a| *a != ping).collect();
            conn.change_property32(PropMode::REPLACE, window, protocols, AtomEnum::ATOM, &without).ok()?;
            conn.flush().ok()?;
            Some((conn, window, protocols, all))
        }
    }

    impl Drop for Quiet {
        fn drop(&mut self) {
            if let Some((conn, window, protocols, all)) = self.0.take() {
                // Checked (a round trip), so the server has taken it before
                // the connection closes; unchecked, the change can be lost.
                if let Ok(sent) = conn.change_property32(PropMode::REPLACE, window, protocols, AtomEnum::ATOM, &all) {
                    let _ = sent.check();
                }
            }
        }
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
mod ping {
    /// Nothing to do: the system's dialogs keep the app's window alive here.
    pub struct Quiet;

    impl Quiet {
        pub fn new() -> Self {
            Quiet
        }
    }
}
