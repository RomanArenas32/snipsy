//! Snipsy — editor de video.

mod app;
mod assistant_ui;
mod mascot;
mod brand_ui;
mod faces_ui;
mod fonts_ui;
mod inspector;
mod library_ui;
mod player;
mod proxies_ui;
mod subtitles_ui;
mod timeline_ui;
mod viewer;

/// Abierta desde Finder (Snipsy.app), la app recibe un PATH mínimo sin Homebrew:
/// se agregan sus carpetas para encontrar `ffmpeg` (miniaturas de la biblioteca).
fn extend_path() {
    let path = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<&str> = path.split(':').filter(|p| !p.is_empty()).collect();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if !parts.contains(&extra) {
            parts.push(extra);
        }
    }
    std::env::set_var("PATH", parts.join(":"));
}

fn main() -> eframe::Result {
    extend_path();
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
    // (macOS antiguos pasan «-psn_…» al abrir desde Finder: se ignora).
    let initial: Vec<std::path::PathBuf> =
        std::env::args_os().skip(1).filter(|a| !a.to_string_lossy().starts_with("-psn")).map(Into::into).collect();
    eframe::run_native(
        "Snipsy",
        options,
        Box::new(move |cc| Ok(Box::new(app::SnipsyApp::new(cc, initial)))),
    )
}
