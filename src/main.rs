#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;

use std::path::PathBuf;
use std::sync::Arc;

use yt4k_downloader::log::Logger;

fn main() -> eframe::Result<()> {
    // Folder data aplikasi: %APPDATA%\YT4KDownloader (Windows) /
    // ~/.local/share/YT4KDownloader (Linux). Log di subfolder logs/,
    // binari sidecar (yt-dlp, ffmpeg) di subfolder bin/.
    let base: PathBuf = dirs::data_dir()
        .map(|d| d.join("YT4KDownloader"))
        .unwrap_or_else(|| PathBuf::from("YT4KDownloader"));
    let log_dir = base.join("logs");
    let bin_dir = base.join("bin");
    let _ = std::fs::create_dir_all(&log_dir);
    let _ = std::fs::create_dir_all(&bin_dir);

    let date = chrono::Local::now().format("%Y-%m-%d");
    let log_file = log_dir.join(format!("main-{date}.log"));
    let logger: Arc<Logger> = Logger::new(log_file);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 760.0])
            .with_min_inner_size([760.0, 560.0])
            .with_title("YT 4K Downloader"),
        ..Default::default()
    };

    let log_dir_clone = log_dir.clone();
    eframe::run_native(
        "YT 4K Downloader",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(app::App::new(
                logger.clone(),
                bin_dir.clone(),
                log_dir_clone.clone(),
            )))
        }),
    )
}
