#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod app;
mod autosave;
mod brush_library;
mod canvas;
mod clipboard;
mod dialogs;
mod files;
mod fonts;
mod panels;
mod settings;
mod state;
mod tablet;
mod tools;
mod ui;
mod update;
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
    eframe::run_native("qsketch", options, Box::new(move |cc| Ok(Box::new(app::QSketchApp::new(cc, files)))))
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
