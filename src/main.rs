#![windows_subsystem = "windows"]

mod agent;
mod app;
mod config;
mod download;
mod ffmpeg;
mod history;
mod llm;
mod quota;
mod ranking;
mod worker;
mod youtube;
mod ytdlp;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PlaylistYT")
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PlaylistYT",
        options,
        Box::new(|cc| Ok(Box::new(app::PlaylistApp::new(cc)))),
    )
}
