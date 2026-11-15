#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod platform;

#[cfg(windows)]
fn main() -> eframe::Result<()> {
    let start_hidden = std::env::args().any(|argument| argument == "--minimized");
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([700.0, 560.0])
        .with_visible(!start_hidden)
        .with_title(rometric::branding::APP_NAME);
    if let Ok(logo) = rometric::branding::logo_rgba() {
        viewport = viewport.with_icon(eframe::egui::IconData {
            rgba: logo.rgba,
            width: logo.width,
            height: logo.height,
        });
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        rometric::branding::APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This desktop binary targets Windows.");
    eprintln!("Build with: cargo xwin build --release --target x86_64-pc-windows-msvc");
}
