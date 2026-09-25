#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod app;
mod autosave;
mod backups;
mod brush_library;
mod canvas;
mod clipboard;
mod dialogs;
mod files;
mod fonts;
mod panels;
mod settings;
mod share;
mod single_instance;
mod startup_cloak;
mod startup_trace;
mod state;
mod tablet;
mod tools;
mod ui;
mod update;
mod win_pointer;
mod wintab;
mod workspace;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .format_timestamp_millis()
    .init();

    let icon = load_icon();
    // The app draws its own title strip unless the user asked for the OS one.
    let saved = settings::Settings::load();
    let native_frame = saved.ui.native_frame;
    // The window's final geometry goes on the builder so the first painted
    // frame is already the right size: a resize after the window is shown
    // rebuilds the swapchain and flashes a cleared frame.
    let rect = saved.window_rect.filter(|r| r[2] >= 900.0 && r[3] >= 560.0 && r[0] > -8000.0 && r[1] > -8000.0);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("qsketch")
        .with_app_id("qsketch")
        .with_inner_size(rect.map(|r| [r[2], r[3]]).unwrap_or([1600.0, 950.0]))
        .with_min_inner_size([900.0, 560.0])
        .with_decorations(native_frame)
        .with_maximized(startup_cloak::builder_maximized(saved.window_maximized))
        .with_icon(icon);
    if let Some(r) = rect {
        viewport = viewport.with_position([r[0], r[1]]);
    }

    let mut trace = startup_trace::StartupTrace::new();
    if forget_eframe_window_state() {
        trace.note("removed stale window state from eframe storage");
    }
    trace.note(&format!(
        "builder: saved_rect={rect:?} maximized={} (on builder: {}) centered={} native_frame={native_frame}",
        saved.window_maximized,
        startup_cloak::builder_maximized(saved.window_maximized),
        rect.is_none()
    ));

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        centered: rect.is_none(),
        // Geometry is remembered in settings.toml (see `window_rect`), not
        // in eframe's own store, so it can be applied before the window shows.
        persist_window: false,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            surface: egui_wgpu::SurfaceConfig {
                present_mode: wgpu::PresentMode::AutoVsync,
                ..egui_wgpu::SurfaceConfig::LOW_LATENCY
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(std::path::PathBuf::from).collect();
    // Hand the files to an already-running qsketch (they open as new tabs)
    // instead of starting a second window.
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<egui::Context>();
    let wake = std::sync::Mutex::new(None::<egui::Context>);
    let primary = match single_instance::acquire(&files, move || {
        let mut g = wake.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = wake_rx.try_recv().ok();
        }
        if let Some(ctx) = g.as_ref() {
            ctx.request_repaint();
        }
    }) {
        single_instance::Outcome::Forwarded => {
            log::info!("forwarded {} file(s) to the running qsketch instance", files.len());
            return Ok(());
        }
        single_instance::Outcome::Primary(p) => p,
    };
    eframe::run_native(
        "qsketch",
        options,
        Box::new(move |cc| {
            let _ = wake_tx.send(cc.egui_ctx.clone());
            trace.note("window created; first frame next");
            Ok(Box::new(app::QSketchApp::new(cc, files, primary, trace)))
        }),
    )
}

/// Before 0.43 eframe saved the window geometry in its own storage. It still
/// loads that entry (`persist_window: false` only stops the saving) and
/// applies it over the builder, so a window maximized back then opened
/// maximized forever, flashing blank on Windows (see `startup_cloak`).
/// Returns whether an entry was removed.
fn forget_eframe_window_state() -> bool {
    let Some(path) = eframe::storage_dir("qsketch").map(|d| d.join("app.ron")) else { return false };
    let Ok(text) = std::fs::read_to_string(&path) else { return false };
    let Ok(mut kv) = ron::from_str::<std::collections::HashMap<String, String>>(&text) else { return false };
    if kv.remove("window").is_none() {
        return false;
    }
    match ron::ser::to_string_pretty(&kv, ron::ser::PrettyConfig::default()) {
        Ok(out) => std::fs::write(&path, out).is_ok(),
        Err(_) => false,
    }
}

fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../../../assets/icon/icon-256.png");
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            egui::IconData { rgba: rgba.into_raw(), width: w, height: h }
        }
        Err(_) => egui::IconData::default(),
    }
}
