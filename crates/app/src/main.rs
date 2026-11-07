//! Snipsy — editor de video.

mod app;
mod brand_ui;
mod fonts_ui;
mod inspector;
mod player;
mod proxies_ui;
mod subtitles_ui;
mod timeline_ui;
mod viewer;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,whisper_rs=warn")).init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Snipsy")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    // Archivos pasados por línea de comandos: un proyecto .snipsy o medios a importar.
    let initial: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    eframe::run_native(
        "Snipsy",
        options,
        Box::new(move |cc| Ok(Box::new(app::SnipsyApp::new(cc, initial)))),
    )
}
