//! Capture the actual eframe/OpenGL application at physical client sizes and
//! egui scaling factors, without changing Windows display settings or user data.
//!
//! cargo run --example capture_ui --target-dir target/feature-validation -- target/ui-validation/matrix

#[cfg(windows)]
#[path = "../src/app.rs"]
mod app;
#[cfg(windows)]
#[path = "../src/audio.rs"]
mod audio;
#[cfg(windows)]
#[path = "../src/platform.rs"]
mod platform;

#[cfg(windows)]
use eframe::egui;

#[cfg(windows)]
struct CaptureApp {
    app: app::App,
    output: std::path::PathBuf,
    shots: Vec<([u32; 2], f32)>,
    index: usize,
    frames: usize,
    requested: bool,
    started: std::time::Instant,
    report: Vec<serde_json::Value>,
}

#[cfg(windows)]
impl eframe::App for CaptureApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.index >= self.shots.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.started.elapsed().as_secs() > 120 {
            panic!("native capture did not finish within 120 seconds");
        }
        let screenshot = ctx.input(|input| {
            input.events.iter().find_map(|event| {
                if let egui::Event::Screenshot { image, .. } = event {
                    Some(image.clone())
                } else {
                    None
                }
            })
        });
        if let Some(image) = screenshot {
            let (size, scale) = self.shots[self.index];
            let name = format!(
                "{}x{}-{}pct.png",
                size[0],
                size[1],
                (scale * 100.0).round() as u32
            );
            let rgba: Vec<u8> = image
                .pixels
                .iter()
                .flat_map(|color| color.to_array())
                .collect();
            image::save_buffer_with_format(
                self.output.join(&name),
                &rgba,
                image.size[0] as u32,
                image.size[1] as u32,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .expect("write native framebuffer capture");
            self.report.push(serde_json::json!({
                "file": name,
                "requested_client_pixels": size,
                "actual_capture_pixels": image.size,
                "egui_pixels_per_point": ctx.pixels_per_point(),
                "native_pixels_per_point": ctx.native_pixels_per_point(),
                "logical_client_size": [ctx.screen_rect().width(), ctx.screen_rect().height()],
                "source": "actual eframe/OpenGL framebuffer; audio stopped; isolated QA configuration",
            }));
            assert_eq!(
                image.size,
                [size[0] as usize, size[1] as usize],
                "window manager changed the requested client size"
            );
            self.index += 1;
            self.frames = 0;
            self.requested = false;
            if self.index == self.shots.len() {
                std::fs::write(
                    self.output.join("captures.json"),
                    serde_json::to_string_pretty(&self.report).unwrap(),
                )
                .unwrap();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }
        let (size, scale) = self.shots[self.index];
        ctx.set_pixels_per_point(scale);
        if self.frames < 3 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
                360.0, 300.0,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                size[0] as f32 / scale,
                size[1] as f32 / scale,
            )));
        }
        self.app.update(ctx, frame);
        self.frames += 1;
        if self.frames >= 12 && !self.requested {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

#[cfg(windows)]
fn main() -> eframe::Result<()> {
    use rometric::{config::Config, product::Preset};
    let output = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/ui-validation/matrix"));
    std::fs::create_dir_all(&output).expect("create capture directory");
    let output = std::fs::canonicalize(output).unwrap();
    let sandbox = output.join(format!("sandbox-{}", std::process::id()));
    std::fs::create_dir_all(&sandbox).unwrap();
    // Only this process sees this environment. The listener's AppData and
    // Windows startup registry settings are never used or changed by the test.
    std::env::set_var("APPDATA", &sandbox);
    let mut cfg = Config {
        auto_start_audio: false,
        auto_reconnect: false,
        minimize_to_tray: false,
        spectrum_enabled: true,
        reduced_motion: true,
        parametric_enabled: true,
        preamp_db: -3.0,
        ..Config::default()
    };
    cfg.presets
        .push(Preset::new("QA Desk", cfg.sound_settings()));
    cfg.selected_preset = Some("QA Desk".into());
    cfg.save_result().unwrap();
    let shots = [1.0, 1.25, 1.5]
        .into_iter()
        .flat_map(|scale| {
            [[1280, 720], [1366, 768], [1600, 900], [1920, 1080]].map(|size| (size, scale))
        })
        .collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rometric UI capture")
            .with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Rometric UI capture",
        options,
        Box::new(move |cc| {
            Ok(Box::new(CaptureApp {
                app: app::App::new(cc),
                output,
                shots,
                index: 0,
                frames: 0,
                requested: false,
                started: std::time::Instant::now(),
                report: Vec::new(),
            }))
        }),
    )
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Native UI capture requires Windows.");
}
