//! Optional dedicated tablet backend via octotablet (Windows Ink
//! RealTimeStylus, Wayland tablet-v2). Provides pressure, tilt and eraser-tip
//! detection. When disabled or unavailable, pen pressure still arrives through
//! the windowing system (winit pointer/touch events with force).

use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

use crate::state::{AppState, TempReason};
use crate::tools::ToolKind;

/// Holds raw handles copied from the eframe creation context so octotablet can
/// keep them for its lifetime.
struct Handles {
    window: RawWindowHandle,
    display: RawDisplayHandle,
}

// SAFETY: the raw handles are plain integers/pointers owned by the winit window,
// which outlives the tablet manager (the manager is dropped in `App::on_exit`).
unsafe impl Send for Handles {}
unsafe impl Sync for Handles {}

impl HasWindowHandle for Handles {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: see struct comment.
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}
impl HasDisplayHandle for Handles {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: see struct comment.
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

pub struct Tablet {
    manager: octotablet::Manager,
    _handles: Arc<Handles>,
    in_proximity: bool,
    eraser: bool,
    /// Last known window scale so positions can be mapped to egui points.
    last_pos: Option<egui::Pos2>,
}

impl Tablet {
    /// Try to start the backend. Returns `None` (logged) when unsupported.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let window = cc.window_handle().ok()?.as_raw();
        let display = cc.display_handle().ok()?.as_raw();
        let handles = Arc::new(Handles { window, display });
        match octotablet::Builder::new().emulate_tool_from_mouse(false).build_shared(&handles) {
            Ok(manager) => {
                log::info!("tablet backend: {:?}", manager.backed());
                Some(Self { manager, _handles: handles, in_proximity: false, eraser: false, last_pos: None })
            }
            Err(e) => {
                log::info!("tablet backend unavailable: {e}");
                None
            }
        }
    }

    /// Drain events, feeding pen samples into the app state.
    pub fn pump(&mut self, state: &mut AppState) {
        use octotablet::events::{Event, ToolEvent};
        let events = match self.manager.pump() {
            Ok(ev) => ev,
            Err(e) => {
                log::warn!("tablet pump: {e}");
                return;
            }
        };
        let switch_on_eraser = state.settings.tablet.eraser_tip_switches_tool;
        for ev in events {
            let Event::Tool { tool, event } = ev else { continue };
            match event {
                ToolEvent::In { .. } => {
                    self.in_proximity = true;
                    self.eraser = matches!(tool.tool_type, Some(octotablet::tool::Type::Eraser));
                    state.pen.tablet_active = true;
                    state.pen.eraser = self.eraser;
                    if self.eraser && switch_on_eraser && state.temp_tool.is_none() && state.tool != ToolKind::Eraser {
                        state.temp_tool = Some((ToolKind::Eraser, TempReason::EraserTip));
                    }
                }
                ToolEvent::Out => {
                    self.in_proximity = false;
                    state.pen.tablet_active = false;
                    state.pen.pressure = None;
                    state.pen.in_contact = false;
                    state.pen.eraser = false;
                    if matches!(state.temp_tool, Some((_, TempReason::EraserTip))) {
                        state.temp_tool = None;
                    }
                }
                ToolEvent::Down => {
                    state.pen.in_contact = true;
                }
                ToolEvent::Up => {
                    state.pen.in_contact = false;
                    state.pen.pressure = None;
                }
                ToolEvent::Pose(pose) => {
                    let pos = egui::pos2(pose.position[0], pose.position[1]);
                    self.last_pos = Some(pos);
                    if let Some(t) = pose.tilt {
                        state.pen.tilt = t;
                    }
                    if state.pen.in_contact {
                        let p = pose.pressure.get().unwrap_or(1.0).clamp(0.0, 1.0);
                        state.pen.pressure = Some(p);
                        state.tablet_samples.push((pos, p));
                    }
                }
                _ => {}
            }
        }
    }
}
