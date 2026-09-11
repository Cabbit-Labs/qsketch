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
mod single_instance;
mod state;
mod tablet;
mod tools;
mod ui;
mod update;
mod win_pointer;
mod workspace;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .format_timestamp_millis()
    .init();

    let icon = load_icon();
    // The app draws its own title strip unless the user asked for the OS one.
    let native_frame = settings::Settings::load().ui.native_frame;
    let viewport = egui::ViewportBuilder::default()
        .with_title("qsketch")
        .with_app_id("qsketch")
        .with_inner_size([1600.0, 950.0])
        .with_min_inner_size([900.0, 560.0])
        .with_decorations(native_frame)
        .with_icon(icon);

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        persist_window: true,
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
            Ok(Box::new(app::QSketchApp::new(cc, files, primary)))
        }),
    )
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
