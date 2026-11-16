//! Signal Bench desktop UI.

#[path = "app/theme.rs"]
mod theme;

#[cfg(test)]
#[path = "app/tests.rs"]
mod tests;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::audio::{self, AudioEngine, DeviceList, SharedParams};
use crate::platform::{self, TrayAction, TrayController};
use rometric::analysis::{
    level_match, spectrum_levels, LevelMatch, SPECTRUM_BANDS, SPECTRUM_FLOOR_DB,
};
use rometric::autoeq::AutoEq;
use rometric::branding;
use rometric::catalog::{filter_groups, Group, Grouping, ProfileCatalog};
use rometric::config::Config;
use rometric::dsp::{
    default_band_settings, max_response_db, BandKind, EqChain, Params, BAND_LABEL, N_BANDS,
};
use rometric::product::{
    EditHistory, Preset, PresetFile, PresetSaveResult, ReconnectBackoff, SoundSettings,
};
use rometric::telemetry::TelemetrySnapshot;
use theme::Icon;

const SHELL: egui::Color32 = theme::BG;
const INK: egui::Color32 = theme::INK;
const MUTED: egui::Color32 = theme::MUTED;
const TRACE: egui::Color32 = theme::BLUE;
const OVERLOAD: egui::Color32 = theme::WARN;
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
enum PresetOrigin {
    Save,
    Import(String),
}

#[derive(Clone, Debug)]
struct PendingPreset {
    preset: Preset,
    origin: PresetOrigin,
    focus_cancel: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum CompactView {
    #[default]
    Equalizer,
    Profiles,
    Sound,
}

/// Geometry recorded from the actual widgets, not assumed screen dimensions.
/// Used by native/headless regression checks and responsive layout diagnostics.
#[derive(Clone, Debug)]
struct LayoutTrace {
    toolbar: egui::Rect,
    center: egui::Rect,
    graph: egui::Rect,
    faders: [egui::Rect; N_BANDS],
    actions: egui::Rect,
    spectrum: Option<egui::Rect>,
    compare: egui::Rect,
    presets: egui::Rect,
    center_scroll: bool,
    inspector_scroll: bool,
}

impl Default for LayoutTrace {
    fn default() -> Self {
        Self {
            toolbar: egui::Rect::NOTHING,
            center: egui::Rect::NOTHING,
            graph: egui::Rect::NOTHING,
            faders: [egui::Rect::NOTHING; N_BANDS],
            actions: egui::Rect::NOTHING,
            spectrum: None,
            compare: egui::Rect::NOTHING,
            presets: egui::Rect::NOTHING,
            center_scroll: false,
            inspector_scroll: false,
        }
    }
}

#[derive(Clone, Debug)]
enum EngineStatus {
    Stopped,
    Starting,
    Running(String),
    Reconnecting(String),
    Error(String),
}

impl EngineStatus {
    fn label(&self, correction_enabled: bool) -> String {
        match self {
            Self::Stopped => "Audio stopped".into(),
            Self::Starting => "Connecting audio…".into(),
            Self::Running(route) if correction_enabled => format!("Processing · {route}"),
            Self::Running(route) => format!("Bypassed · {route}"),
            Self::Reconnecting(message) => format!("Reconnecting · {message}"),
            Self::Error(message) => format!("Audio error · {message}"),
        }
    }

    fn color(&self, correction_enabled: bool) -> egui::Color32 {
        match self {
            Self::Running(_) if correction_enabled => theme::OK,
            Self::Running(_) | Self::Stopped => MUTED,
            Self::Starting | Self::Reconnecting(_) => TRACE,
            Self::Error(_) => OVERLOAD,
        }
    }
}

pub struct App {
    logo: Option<egui::TextureHandle>,
    db: Result<AutoEq, String>,
    catalog: Option<ProfileCatalog>,
    query: String,
    filtered: Vec<usize>,
    groups: Vec<Group>,
    selected: Option<usize>,
    grouping: Grouping,

    sound: SoundSettings,
    shared: Arc<SharedParams>,
    engine: Option<AudioEngine>,
    devices: DeviceList,
    capture: Option<String>,
    render: Option<String>,
    last_render: Option<String>,
    engine_status: EngineStatus,
    feedback_override: bool,

    cfg: Config,
    dirty_since: Option<Instant>,
    save_error: Option<String>,
    history: EditHistory,
    edit_baseline: Option<SoundSettings>,
    skip_history_once: bool,

    ab_slots: [SoundSettings; 2],
    active_ab: usize,
    preset_name: String,
    preset_error: Option<String>,
    pending_preset: Option<PendingPreset>,
    help_open: bool,
    match_calibration: Option<LevelMatch>,
    match_key: Option<[SoundSettings; 2]>,
    next_calibration: Instant,
    shared_key: Option<Params>,
    compact_view: CompactView,
    layout: LayoutTrace,

    plot: Vec<f32>,
    plot_key: Option<(Params, u32)>,
    plot_active_band: usize,
    telemetry: TelemetrySnapshot,
    meter_input: f32,
    meter_output: f32,
    last_meter_update: Instant,
    spectrum_levels: [f32; SPECTRUM_BANDS],
    spectrum_target: [f32; SPECTRUM_BANDS],
    spectrum_frame: u64,

    reconnect: ReconnectBackoff,
    retry_at: Option<Instant>,
    next_device_poll: Instant,
    diagnostics: VecDeque<String>,
    diagnostics_open: bool,

    tray: Option<TrayController>,
    tray_signature: String,
    tray_error: Option<String>,
    autostart: bool,
    quitting: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = Config::load();
        theme::install_fonts(&cc.egui_ctx, theme::bundled_fonts());
        theme::install(&cc.egui_ctx, cfg.reduced_motion);
        let logo = branding::logo_rgba().ok().map(|logo| {
            cc.egui_ctx.load_texture(
                "rometric-logo",
                egui::ColorImage::from_rgba_unmultiplied(
                    [logo.width as usize, logo.height as usize],
                    &logo.rgba,
                ),
                egui::TextureOptions::LINEAR,
            )
        });

        let db = AutoEq::load();
        let catalog = db.as_ref().ok().map(|db| db.catalog());
        let selected = cfg
            .profile
            .as_ref()
            .and_then(|name| db.as_ref().ok().and_then(|db| db.index_of(name)));
        let devices = audio::list_devices();
        let (default_in, default_out) = audio::default_names();
        let capture = cfg
            .capture_device
            .clone()
            .filter(|name| devices.inputs.contains(name))
            .or(default_in)
            .or_else(|| devices.inputs.first().cloned());
        let render = cfg
            .render_device
            .clone()
            .filter(|name| devices.outputs.contains(name))
            .or(default_out)
            .or_else(|| devices.outputs.first().cloned());

        let sound = cfg.sound_settings();
        let params = sound.to_params();
        let shared = Arc::new(SharedParams::new(&params));
        let filtered = db.as_ref().map(|db| db.search("")).unwrap_or_default();
        let autostart = platform::autostart_enabled();
        let preset_names: Vec<_> = cfg
            .presets
            .iter()
            .map(|preset| preset.name.clone())
            .collect();
        let (tray, tray_error) =
            match TrayController::new(&preset_names, false, sound.correction_enabled, autostart) {
                Ok(tray) => (Some(tray), None),
                Err(err) => (None, Some(err)),
            };
        let now = Instant::now();
        let preset_name = cfg.selected_preset.clone().unwrap_or_default();
        let mut app = Self {
            logo,
            db,
            catalog,
            query: String::new(),
            filtered,
            groups: Vec::new(),
            selected,
            grouping: cfg.profile_grouping,
            sound: sound.clone(),
            shared,
            engine: None,
            devices,
            capture,
            render: render.clone(),
            last_render: render,
            engine_status: EngineStatus::Stopped,
            feedback_override: false,
            cfg,
            dirty_since: None,
            save_error: None,
            history: EditHistory::new(100),
            edit_baseline: None,
            skip_history_once: false,
            ab_slots: [sound.clone(), sound],
            active_ab: 0,
            preset_name,
            preset_error: None,
            pending_preset: None,
            help_open: false,
            match_calibration: None,
            match_key: None,
            next_calibration: now,
            shared_key: None,
            compact_view: CompactView::Equalizer,
            layout: LayoutTrace::default(),
            plot: Vec::new(),
            plot_key: None,
            plot_active_band: 5,
            telemetry: TelemetrySnapshot::default(),
            meter_input: 0.0,
            meter_output: 0.0,
            last_meter_update: now,
            spectrum_levels: [SPECTRUM_FLOOR_DB; SPECTRUM_BANDS],
            spectrum_target: [SPECTRUM_FLOOR_DB; SPECTRUM_BANDS],
            spectrum_frame: 0,
            reconnect: ReconnectBackoff::default(),
            retry_at: None,
            next_device_poll: now + Duration::from_secs(2),
            diagnostics: VecDeque::new(),
            diagnostics_open: false,
            tray,
            tray_signature: String::new(),
            tray_error,
            autostart,
            quitting: false,
        };
        app.refresh_filter();
        if app.cfg.auto_start_audio {
            app.start_engine();
        }
        app
    }

    fn params(&self) -> Params {
        let mut params = self.sound.to_params();
        if self.cfg.level_match_ab
            && self.engine.is_some()
            && self.match_key.as_ref() == Some(&self.current_ab_slots())
        {
            if let Some(calibration) = self.match_calibration {
                params.comparison_gain_db = calibration.trims_db[self.active_ab];
            }
        }
        params
    }

    fn current_ab_slots(&self) -> [SoundSettings; 2] {
        let mut slots = self.ab_slots.clone();
        slots[self.active_ab] = self.sound.clone();
        slots
    }

    fn update_comparison(&mut self) {
        if !self.cfg.level_match_ab || self.engine.is_none() {
            self.match_key = None;
            self.match_calibration = None;
            return;
        }
        let slots = self.current_ab_slots();
        if self.match_key.as_ref() == Some(&slots) || Instant::now() < self.next_calibration {
            return;
        }
        self.next_calibration = Instant::now() + Duration::from_millis(500);
        let rate = self
            .engine
            .as_ref()
            .map(|engine| engine.render_rate)
            .unwrap_or(48_000);
        let params = [slots[0].to_params(), slots[1].to_params()];
        if let Some(calibration) = level_match(rate as f32, &self.telemetry.analysis_input, &params)
        {
            self.match_calibration = Some(calibration);
            self.match_key = Some(slots);
        }
    }

    fn selected_preset_modified(&self) -> bool {
        let profile = self.selected_profile_name();
        self.cfg
            .selected_preset
            .as_deref()
            .and_then(|name| self.cfg.preset(name))
            .is_some_and(|preset| preset.is_modified(&self.sound, profile.as_deref()))
    }

    fn selected_profile_name(&self) -> Option<String> {
        self.selected.and_then(|index| {
            self.db
                .as_ref()
                .ok()
                .map(|db| db.profiles[index].name.clone())
        })
    }

    fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    fn log(&mut self, message: impl Into<String>) {
        self.diagnostics.push_front(message.into());
        self.diagnostics.truncate(40);
    }

    fn refresh_filter(&mut self) {
        let Ok(db) = &self.db else {
            self.filtered.clear();
            self.groups.clear();
            return;
        };
        let query = self.query.trim().to_lowercase();
        self.groups.clear();
        match self.grouping {
            Grouping::All => {
                self.filtered = db.search(&self.query);
            }
            Grouping::Favorites | Grouping::Recent => {
                let source = if self.grouping == Grouping::Favorites {
                    &self.cfg.profile_library.favorites
                } else {
                    &self.cfg.profile_library.recents
                };
                self.filtered = source
                    .iter()
                    .filter(|name| query.is_empty() || name.to_lowercase().contains(&query))
                    .filter_map(|name| db.index_of(name))
                    .collect();
            }
            Grouping::Brand | Grouping::Sound => {
                self.filtered.clear();
                self.groups = match &self.catalog {
                    Some(catalog) => {
                        let groups = catalog.groups(self.grouping).unwrap_or(&[]);
                        filter_groups(groups, &catalog.names, &self.query)
                    }
                    None => Vec::new(),
                };
            }
        }
    }

    /// Number of profiles currently visible in the browser.
    fn shown_profiles(&self) -> usize {
        if self.grouping.is_grouped() {
            self.groups.iter().map(Group::len).sum()
        } else {
            self.filtered.len()
        }
    }

    fn select_profile(&mut self, index: usize) {
        let Ok(db) = &self.db else { return };
        self.selected = Some(index);
        self.sound.parametric_enabled = false;
        self.sound.band_settings = default_band_settings();
        self.sound.gains = db.bands_for(index);
        if self.sound.auto_preamp {
            self.sound.preamp_db = db.auto_preamp(index);
        }
        let name = db.profiles[index].name.clone();
        self.cfg.profile = Some(name.clone());
        self.cfg.selected_preset = None;
        self.cfg.profile_library.mark_recent(name);
        self.mark_dirty();
        if self.grouping == Grouping::Recent {
            self.refresh_filter();
        }
    }

    fn apply_preset(&mut self, name: &str) {
        let Some(preset) = self.cfg.preset(name).cloned() else {
            return;
        };
        self.sound = preset.sound;
        self.selected = preset
            .profile
            .as_deref()
            .and_then(|profile| self.db.as_ref().ok().and_then(|db| db.index_of(profile)));
        self.cfg.profile = preset.profile;
        self.cfg.selected_preset = Some(name.to_string());
        self.preset_name = name.to_string();
        self.mark_dirty();
        self.log(format!("Applied preset “{name}”."));
    }

    fn save_current_preset(&mut self) {
        if self.pending_preset.is_some() {
            return;
        }
        let name = self.preset_name.trim();
        if name.is_empty() {
            self.preset_error = Some("Enter a preset name before saving.".into());
            return;
        }
        let mut preset = Preset::new(name, self.sound.clone());
        preset.profile = self.selected_profile_name();
        self.request_preset_save(preset, PresetOrigin::Save);
    }

    fn request_preset_save(&mut self, mut preset: Preset, origin: PresetOrigin) {
        if let Err(error) = preset.validate() {
            self.preset_error = Some(error.clone());
            self.log(error);
            return;
        }
        let name = preset.name.clone();
        match self.cfg.save_preset(preset, false) {
            Ok(PresetSaveResult::Saved) => self.finish_preset_save(&name, origin),
            Ok(PresetSaveResult::NeedsConfirmation(preset)) => {
                self.pending_preset = Some(PendingPreset {
                    preset: *preset,
                    origin,
                    focus_cancel: true,
                });
                self.preset_error = None;
            }
            Err(error) => {
                self.preset_error = Some(error.clone());
                self.log(error);
            }
        }
    }

    fn finish_preset_save(&mut self, name: &str, origin: PresetOrigin) {
        self.preset_error = None;
        match origin {
            PresetOrigin::Save => {
                self.cfg.selected_preset = Some(name.to_string());
                self.preset_name = name.to_string();
                self.mark_dirty();
                self.log(format!("Saved preset “{name}”."));
            }
            PresetOrigin::Import(path) => {
                self.apply_preset(name);
                self.log(format!("Imported preset from {path}."));
            }
        }
    }

    fn resolve_preset_overwrite(&mut self, replace: bool) {
        let Some(pending) = self.pending_preset.take() else {
            return;
        };
        if !replace {
            return;
        }
        let name = pending.preset.name.clone();
        match self.cfg.save_preset(pending.preset, true) {
            Ok(PresetSaveResult::Saved) => self.finish_preset_save(&name, pending.origin),
            Err(error) => {
                self.preset_error = Some(error.clone());
                self.log(error);
            }
            Ok(PresetSaveResult::NeedsConfirmation(_)) => {
                unreachable!("overwrite explicitly approved")
            }
        }
    }

    fn start_engine(&mut self) {
        let (Some(capture), Some(render)) = (self.capture.clone(), self.render.clone()) else {
            self.engine_status = EngineStatus::Error("Choose a capture and output device.".into());
            return;
        };
        if capture == render && !self.feedback_override {
            self.engine_status = EngineStatus::Error(
                "Capture and output match. Confirm the feedback risk before starting.".into(),
            );
            return;
        }
        self.engine_status = EngineStatus::Starting;
        match AudioEngine::start(&capture, &render, self.shared.clone()) {
            Ok(engine) => {
                let route = format!(
                    "{} Hz / {} ch  ›  {} Hz / {} ch",
                    engine.capture_rate,
                    engine.capture_channels,
                    engine.render_rate,
                    engine.render_channels
                );
                self.engine = Some(engine);
                self.clear_route_analysis();
                self.engine_status = EngineStatus::Running(route.clone());
                self.retry_at = None;
                self.reconnect.reset();
                self.log(format!("Audio started: {capture} › {render} ({route})."));
            }
            Err(err) => {
                let kept_running = self.engine.is_some();
                self.engine_status = EngineStatus::Error(if kept_running {
                    format!("Restart failed; the previous route is still running. {err}")
                } else {
                    err.clone()
                });
                self.log(format!("Audio start failed: {err}"));
                if !kept_running && self.cfg.auto_reconnect {
                    self.schedule_reconnect();
                }
            }
        }
    }

    fn stop_engine(&mut self) {
        self.engine = None;
        self.retry_at = None;
        self.engine_status = EngineStatus::Stopped;
        self.clear_route_analysis();
        self.log("Audio stopped.");
    }

    fn clear_route_analysis(&mut self) {
        // A new render rate/route must never calibrate against the old stream's
        // resampled input. This is UI analysis state, not audio-device logic.
        self.telemetry = TelemetrySnapshot::default();
        self.meter_input = 0.0;
        self.meter_output = 0.0;
        self.spectrum_levels = [SPECTRUM_FLOOR_DB; SPECTRUM_BANDS];
        self.spectrum_target = self.spectrum_levels;
        self.spectrum_frame = 0;
        self.match_calibration = None;
        self.match_key = None;
        self.next_calibration = Instant::now();
    }

    fn schedule_reconnect(&mut self) {
        let delay = self.reconnect.next_delay();
        self.retry_at = Some(Instant::now() + delay);
        self.engine_status = EngineStatus::Reconnecting(format!("retry in {} s", delay.as_secs()));
    }

    fn poll_devices_and_reconnect(&mut self) {
        let now = Instant::now();
        if now >= self.next_device_poll {
            self.next_device_poll = now + Duration::from_secs(2);
            self.devices = audio::list_devices();
            let selected_missing = self
                .capture
                .as_ref()
                .is_some_and(|name| !self.devices.inputs.contains(name))
                || self
                    .render
                    .as_ref()
                    .is_some_and(|name| !self.devices.outputs.contains(name));
            if self.engine.is_some() && selected_missing {
                self.engine = None;
                self.log("A selected audio device disappeared.");
                if self.cfg.auto_reconnect {
                    self.schedule_reconnect();
                } else {
                    self.engine_status =
                        EngineStatus::Error("A selected device was disconnected.".into());
                }
            }
        }
        if self.engine.is_none() && self.cfg.auto_reconnect {
            if let Some(deadline) = self.retry_at {
                if now >= deadline {
                    self.start_engine();
                } else if matches!(self.engine_status, EngineStatus::Reconnecting(_)) {
                    let seconds = deadline.saturating_duration_since(now).as_secs().max(1);
                    self.engine_status =
                        EngineStatus::Reconnecting(format!("retry in {seconds} s"));
                }
            }
        }
    }

    fn process_engine_error(&mut self) {
        let error = self.engine.as_ref().and_then(|engine| engine.take_error());
        if let Some(error) = error {
            self.engine = None;
            self.log(format!("Audio stream error: {error}"));
            if self.cfg.auto_reconnect {
                self.schedule_reconnect();
            } else {
                self.engine_status = EngineStatus::Error(error);
            }
        }
    }

    fn update_telemetry(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_meter_update).as_secs_f32();
        self.last_meter_update = now;
        let decay = 10f32.powf(-20.0 * dt / 20.0);
        self.meter_input *= decay;
        self.meter_output *= decay;
        if let Some(engine) = &self.engine {
            let telemetry = engine.telemetry_handle();
            let (input, output) = telemetry.take_peaks();
            self.meter_input = self.meter_input.max(input);
            self.meter_output = self.meter_output.max(output);
            self.telemetry = engine.telemetry();
        } else {
            self.telemetry = TelemetrySnapshot::default();
        }
        if !self.cfg.spectrum_enabled || self.engine.is_none() {
            self.spectrum_target = [SPECTRUM_FLOOR_DB; SPECTRUM_BANDS];
            self.spectrum_frame = 0;
        } else if self.telemetry.processed_frames != self.spectrum_frame {
            let rate = self
                .engine
                .as_ref()
                .map(|engine| engine.render_rate)
                .unwrap_or(48_000);
            self.spectrum_target = spectrum_levels(&self.telemetry.spectrum, rate as f32);
            self.spectrum_frame = self.telemetry.processed_frames;
        }
        for (level, target) in self.spectrum_levels.iter_mut().zip(self.spectrum_target) {
            if self.cfg.reduced_motion {
                *level = target;
            } else {
                let tau = if target > *level { 0.045 } else { 0.25 };
                *level += (1.0 - (-dt.min(0.2) / tau).exp()) * (target - *level);
            }
        }
    }

    fn sync_config(&mut self) {
        self.cfg.apply_sound_settings(&self.sound);
        self.cfg.capture_device = self.capture.clone();
        self.cfg.render_device = self.render.clone();
        self.cfg.profile = self.selected_profile_name();
        self.cfg.profile_grouping = self.grouping;
    }

    fn save_if_due(&mut self) {
        if self
            .dirty_since
            .is_some_and(|when| when.elapsed() >= SAVE_DEBOUNCE)
        {
            self.sync_config();
            match self.cfg.save_result() {
                Ok(()) => self.save_error = None,
                Err(err) => {
                    self.save_error = Some(err.clone());
                    self.log(err);
                }
            }
            self.dirty_since = None;
        }
    }

    fn undo(&mut self) {
        if let Some(previous) = self.history.undo(&self.sound) {
            self.sound = previous;
            self.skip_history_once = true;
            self.mark_dirty();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.history.redo(&self.sound) {
            self.sound = next;
            self.skip_history_once = true;
            self.mark_dirty();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.pending_preset.is_some() {
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F1)) {
            self.help_open = !self.help_open;
        }
        if self.help_open {
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
            self.save_current_preset();
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F)) {
            self.compact_view = if ctx.screen_rect().width() < 720.0 {
                CompactView::Profiles
            } else {
                CompactView::Equalizer
            };
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("profile_search")));
        }
        let editing_text = ctx
            .memory(|memory| memory.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
        if editing_text {
            return;
        }
        let undo = ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Z));
        let redo = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::Z)
                || input.consume_key(egui::Modifiers::CTRL, egui::Key::Y)
        });
        if undo {
            self.undo();
        } else if redo {
            self.redo();
        }
    }

    fn process_tray(&mut self, ctx: &egui::Context) {
        let actions = self
            .tray
            .as_ref()
            .map(TrayController::poll)
            .unwrap_or_default();
        for action in actions {
            match action {
                TrayAction::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                }
                TrayAction::ToggleEngine => {
                    if self.engine.is_some() {
                        self.stop_engine()
                    } else {
                        self.start_engine()
                    }
                }
                TrayAction::ToggleBypass => {
                    self.sound.correction_enabled = !self.sound.correction_enabled;
                }
                TrayAction::ToggleAutostart => self.toggle_autostart(),
                TrayAction::ApplyPreset(name) => self.apply_preset(&name),
                TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let preset_names: Vec<_> = self
            .cfg
            .presets
            .iter()
            .map(|preset| preset.name.clone())
            .collect();
        let signature = format!(
            "{}:{}:{}:{}",
            self.engine.is_some(),
            self.sound.correction_enabled,
            self.autostart,
            preset_names.join("\u{1f}")
        );
        if signature != self.tray_signature {
            if let Some(tray) = &mut self.tray {
                if let Err(err) = tray.refresh(
                    &preset_names,
                    self.engine.is_some(),
                    self.sound.correction_enabled,
                    self.autostart,
                ) {
                    self.tray_error = Some(err);
                }
            }
            self.tray_signature = signature;
        }
        if let Some(tray) = &self.tray {
            tray.set_tooltip(&self.engine_status.label(self.sound.correction_enabled));
        }
    }

    fn toggle_autostart(&mut self) {
        let requested = !self.autostart;
        match platform::set_autostart(requested) {
            Ok(()) => {
                self.autostart = requested;
                self.log(if requested {
                    "Windows startup enabled."
                } else {
                    "Windows startup disabled."
                });
            }
            Err(err) => self.log(err),
        }
    }

    fn apply_device_mapping_if_needed(&mut self) {
        if self.render == self.last_render {
            return;
        }
        self.last_render = self.render.clone();
        if let Some(device) = self.render.clone() {
            if let Some(preset) = self
                .cfg
                .device_presets
                .preset_for(&device)
                .map(str::to_string)
            {
                self.apply_preset(&preset);
                self.log(format!("Restored “{preset}” for {device}."));
            }
        }
        self.mark_dirty();
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new((logo.id(), egui::vec2(28.0, 28.0))));
            }
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(branding::APP_NAME)
                    .size(19.0)
                    .strong()
                    .family(theme::semibold())
                    .color(INK),
            );
            ui.label(egui::RichText::new("Signal Bench").size(11.0).color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(60.0, 20.0), egui::Sense::hover());
                theme::paint_inset(ui.painter(), rect, 7.0);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    concat!("v", env!("CARGO_PKG_VERSION")),
                    egui::FontId::proportional(10.0),
                    MUTED,
                );
            });
        });
        ui.add_space(4.0);
        self.routing_strip(ui);
    }

    fn routing_strip(&mut self, ui: &mut egui::Ui) {
        let inputs = self.devices.inputs.clone();
        let outputs = self.devices.outputs.clone();
        let same_device = self.capture.is_some() && self.capture == self.render;
        let compact = ui.available_width() < 900.0;
        if compact {
            let combo_width = (ui.available_width() - 360.0).clamp(130.0, 200.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new("Input").size(11.5).color(MUTED));
                egui::ComboBox::from_id_salt("route_capture_compact")
                    .width(combo_width)
                    .selected_text(short_device_with_limit(self.capture.as_deref(), 18))
                    .show_ui(ui, |ui| {
                        for name in &inputs {
                            ui.selectable_value(&mut self.capture, Some(name.clone()), name);
                        }
                    });
                self.correction_toggle(ui);
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new("Output").size(11.5).color(MUTED));
                egui::ComboBox::from_id_salt("route_output_compact")
                    .width(combo_width)
                    .selected_text(short_device_with_limit(self.render.as_deref(), 18))
                    .show_ui(ui, |ui| {
                        for name in &outputs {
                            ui.selectable_value(&mut self.render, Some(name.clone()), name);
                        }
                    });
                self.engine_buttons(ui, same_device);
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(egui::RichText::new("Input").size(11.5).color(MUTED));
                egui::ComboBox::from_id_salt("route_capture")
                    .width(150.0)
                    .selected_text(short_device(self.capture.as_deref()))
                    .show_ui(ui, |ui| {
                        for name in &inputs {
                            ui.selectable_value(&mut self.capture, Some(name.clone()), name);
                        }
                    });
                self.correction_toggle(ui);
                ui.label(egui::RichText::new("Output").size(11.5).color(MUTED));
                egui::ComboBox::from_id_salt("route_output")
                    .width(150.0)
                    .selected_text(short_device(self.render.as_deref()))
                    .show_ui(ui, |ui| {
                        for name in &outputs {
                            ui.selectable_value(&mut self.render, Some(name.clone()), name);
                        }
                    });
                self.engine_buttons(ui, same_device);
            });
        }
        self.apply_device_mapping_if_needed();
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().interact_size.y = 20.0;
            let status = self.engine_status.label(self.sound.correction_enabled);
            let color = self.engine_status.color(self.sound.correction_enabled);
            theme::status_dot(ui, color);
            ui.label(egui::RichText::new(status).size(10.5).color(color));
            ui.add_space(6.0);
            theme::meter(ui, "In", self.meter_input, theme::BLUE_LIGHT);
            ui.add_space(3.0);
            theme::meter(ui, "Out", self.meter_output, theme::BLUE);
            if same_device {
                ui.add_space(4.0);
                theme::warning(ui, "Feedback risk: same device for input and output.");
                theme::checkbox(ui, &mut self.feedback_override, "Allow anyway");
            }
        });
    }

    fn correction_toggle(&mut self, ui: &mut egui::Ui) {
        let text = if self.sound.correction_enabled {
            "Correction on"
        } else {
            "Bypassed"
        };
        if theme::toggle_pill(ui, text, self.sound.correction_enabled)
            .on_hover_text("Keep audio routed while toggling correction")
            .clicked()
        {
            self.sound.correction_enabled = !self.sound.correction_enabled;
        }
    }

    fn engine_buttons(&mut self, ui: &mut egui::Ui, same_device: bool) {
        if self.engine.is_some() {
            if theme::button_icon(ui, Icon::Stop, "Stop audio").clicked() {
                self.stop_engine();
            }
            if theme::button_icon(ui, Icon::Refresh, "Restart").clicked() {
                self.start_engine();
            }
        } else if theme::primary_button_enabled(
            ui,
            Icon::Play,
            "Start audio",
            self.capture.is_some()
                && self.render.is_some()
                && (!same_device || self.feedback_override),
        )
        .clicked()
        {
            self.start_engine();
        }
        if theme::button_icon(ui, Icon::Refresh, "Reload devices").clicked() {
            self.devices = audio::list_devices();
        }
    }

    fn profile_browser(&mut self, ui: &mut egui::Ui) {
        theme::heading(ui, Icon::Headphones, "Headphone profiles");
        ui.label(
            egui::RichText::new("AutoEQ correction library")
                .size(11.0)
                .color(MUTED),
        );
        ui.add_space(6.0);

        let mut grouping_changed = false;
        let gap = ui.spacing().item_spacing.x;
        let width = ui.available_width();
        if width >= 400.0 {
            let chip_width = (width - gap * 4.0) / 5.0;
            ui.horizontal(|ui| {
                for mode in Grouping::ALL {
                    if self.grouping_chip(ui, mode, chip_width) {
                        grouping_changed = true;
                    }
                }
            });
        } else {
            let chip_width = (width - gap * 2.0) / 3.0;
            ui.horizontal(|ui| {
                for mode in [Grouping::All, Grouping::Brand, Grouping::Sound] {
                    if self.grouping_chip(ui, mode, chip_width) {
                        grouping_changed = true;
                    }
                }
            });
            ui.add_space(4.0);
            let chip_width = (width - gap) / 2.0;
            ui.horizontal(|ui| {
                for mode in [Grouping::Favorites, Grouping::Recent] {
                    if self.grouping_chip(ui, mode, chip_width) {
                        grouping_changed = true;
                    }
                }
            });
        }
        if grouping_changed {
            self.refresh_filter();
            self.mark_dirty();
        }

        ui.add_space(6.0);
        let total = self.db.as_ref().map(|db| db.profiles.len()).unwrap_or(0);
        let search = theme::search_field(ui, &mut self.query, &format!("Search {total} profiles"));
        if search.changed() {
            self.refresh_filter();
        }

        let shown = self.shown_profiles();
        ui.horizontal(|ui| {
            let summary = if self.grouping.is_grouped() {
                let folders = self.groups.len();
                format!(
                    "{shown} profiles in {folders} folder{}",
                    if folders == 1 { "" } else { "s" }
                )
            } else {
                format!("{shown} shown")
            };
            ui.label(egui::RichText::new(summary).size(10.5).color(MUTED));
            if !self.query.is_empty() && theme::link_button(ui, "Clear search").clicked() {
                self.query.clear();
                self.refresh_filter();
            }
        });
        if self.grouping == Grouping::Sound {
            ui.label(
                egui::RichText::new("Folders describe what the correction does to the sound.")
                    .size(10.5)
                    .color(MUTED),
            );
        }
        ui.add_space(2.0);
        theme::divider(ui);

        let mut select = None;
        let mut toggle_favorite = None;
        match &self.db {
            Err(err) => {
                ui.colored_label(OVERLOAD, format!("Profile database unavailable: {err}"));
            }
            Ok(_) if shown == 0 => {
                ui.add_space(12.0);
                let message = match self.grouping {
                    Grouping::Favorites => "No favorites yet. Star a profile to keep it here.",
                    Grouping::Recent => "No recent profiles yet.",
                    _ => "No profiles match this view.",
                };
                ui.label(egui::RichText::new(message).size(12.0).color(MUTED));
                ui.add_space(8.0);
                if theme::primary_button(ui, Icon::Search, "Show all profiles").clicked() {
                    self.grouping = Grouping::All;
                    self.query.clear();
                    self.refresh_filter();
                    self.mark_dirty();
                }
            }
            Ok(db) if self.grouping.is_grouped() => {
                let groups = &self.groups;
                let searching = !self.query.trim().is_empty();
                egui::ScrollArea::vertical()
                    .id_salt(("profile_groups", self.grouping as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for group in groups {
                            let title = format!("{} · {}", group.label, group.len());
                            egui::CollapsingHeader::new(title)
                                .id_salt((self.grouping as u8, group.label.as_str()))
                                .open(searching.then_some(true))
                                .show(ui, |ui| {
                                    for index in &group.profiles {
                                        self.profile_row(
                                            ui,
                                            db,
                                            *index,
                                            &mut select,
                                            &mut toggle_favorite,
                                        );
                                    }
                                });
                        }
                    });
            }
            Ok(db) => {
                let rows = self.filtered.clone();
                let row_height = ui.spacing().interact_size.y.max(26.0);
                egui::ScrollArea::vertical()
                    .id_salt("profile_results")
                    .auto_shrink([false, false])
                    .show_rows(ui, row_height, rows.len(), |ui, range| {
                        for row in range {
                            self.profile_row(ui, db, rows[row], &mut select, &mut toggle_favorite);
                        }
                    });
            }
        }
        if let Some(name) = toggle_favorite {
            self.cfg.profile_library.toggle_favorite(&name);
            self.mark_dirty();
            if self.grouping == Grouping::Favorites {
                self.refresh_filter();
            }
        }
        if let Some(index) = select {
            self.select_profile(index);
        }
    }

    fn grouping_chip(&mut self, ui: &mut egui::Ui, mode: Grouping, width: f32) -> bool {
        let active = self.grouping == mode;
        let response = theme::chip(ui, mode.label(), active, width).on_hover_text(match mode {
            Grouping::All => "One alphabetical list",
            Grouping::Brand => "A folder per manufacturer",
            Grouping::Sound => "A folder per shape of correction",
            Grouping::Favorites => "Profiles you starred",
            Grouping::Recent => "Recently applied profiles",
        });
        if response.clicked() && !active {
            self.grouping = mode;
            return true;
        }
        false
    }

    fn profile_row(
        &self,
        ui: &mut egui::Ui,
        db: &AutoEq,
        index: usize,
        select: &mut Option<usize>,
        toggle_favorite: &mut Option<String>,
    ) {
        let name = &db.profiles[index].name;
        let favorite = self.cfg.profile_library.is_favorite(name);
        let selected = self.selected == Some(index);
        let slot = ui.painter().add(egui::Shape::Noop);
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if theme::star(ui, favorite)
                .on_hover_text(if favorite {
                    "Remove favorite"
                } else {
                    "Add favorite"
                })
                .clicked()
            {
                *toggle_favorite = Some(name.clone());
            }
            let inner = ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 26.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let mut response = ui.add(
                        egui::Label::new(egui::RichText::new(name).size(12.5).color(if selected {
                            egui::Color32::WHITE
                        } else {
                            INK
                        }))
                        .truncate()
                        .sense(egui::Sense::click()),
                    );
                    if let Some(catalog) = &self.catalog {
                        if let Some(signature) = catalog.signatures.get(index) {
                            let brand = catalog.brands.get(index).map(String::as_str).unwrap_or("");
                            response = response.on_hover_text(format!(
                                "{brand} · correction {}",
                                signature.label().to_lowercase()
                            ));
                        }
                    }
                    response
                },
            );
            if inner.inner.clicked() {
                *select = Some(index);
            }
            inner.inner
        });
        let hovered = row.inner.hovered();
        let rect = row.response.rect.expand2(egui::vec2(2.0, 1.0));
        let shape = if selected {
            theme::primary_shape(rect, 10.0)
        } else {
            theme::row_shape(
                rect,
                10.0,
                if hovered {
                    theme::CARD_HOVER
                } else {
                    theme::CARD
                },
            )
        };
        ui.painter().set(slot, shape);
    }

    fn response_plot(&mut self, ui: &mut egui::Ui, height: f32) {
        let params = self.params();
        let sample_rate = self
            .engine
            .as_ref()
            .map(|engine| engine.render_rate)
            .unwrap_or(48_000);
        if self.plot_key != Some((params, sample_rate)) {
            let chain = EqChain::new(sample_rate as f32, &params);
            self.plot = (0..=256)
                .map(|index| {
                    let t = index as f32 / 256.0;
                    chain.response_db(20.0 * 1000f32.powf(t))
                })
                .collect();
            self.plot_key = Some((params, sample_rate));
        }
        let extent = self
            .plot
            .iter()
            .fold(12.0f32, |max, value| max.max(value.abs()))
            .div_euclid(6.0)
            .mul_add(6.0, 6.0)
            .clamp(12.0, 180.0);
        let (response, painter) = ui.allocate_painter(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click_and_drag(),
        );
        let rect = response.rect;
        self.layout.graph = rect;
        theme::paint_inset_with(&painter, rect, 12.0, theme::GRAPH);

        let plot = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 42.0, rect.top() + 10.0),
            egui::pos2(rect.right() - 10.0, rect.bottom() - 23.0),
        );
        let y_for = |db: f32| plot.center().y - db / extent * plot.height() * 0.5;
        let x_for = |frequency: f32| {
            let t = (frequency / 20.0).ln() / 1000f32.ln();
            plot.left() + t * plot.width()
        };

        for frequency in [
            31.5, 63.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
        ] {
            let x = x_for(frequency);
            painter.line_segment(
                [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                egui::Stroke::new(1.0_f32, theme::GRID),
            );
        }
        for db in [
            -extent,
            -extent * 0.5,
            -extent * 0.25,
            0.0,
            extent * 0.25,
            extent * 0.5,
            extent,
        ] {
            if db.abs() <= extent {
                let y = y_for(db);
                let color = if db == 0.0 {
                    theme::GRID_STRONG
                } else {
                    theme::GRID
                };
                painter.line_segment(
                    [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                    egui::Stroke::new(if db == 0.0 { 1.2_f32 } else { 1.0_f32 }, color),
                );
                painter.text(
                    egui::pos2(rect.left() + 30.0, y),
                    egui::Align2::RIGHT_CENTER,
                    format!("{db:+.0} dB"),
                    egui::FontId::proportional(10.0),
                    MUTED,
                );
            }
        }

        for (frequency, label) in [
            (40.0_f32, "40"),
            (100.0, "100"),
            (1000.0, "1k"),
            (10000.0, "10k"),
            (20000.0, "20k Hz"),
        ] {
            painter.text(
                egui::pos2(x_for(frequency), rect.bottom() - 10.0),
                if frequency == 20000.0 {
                    egui::Align2::RIGHT_CENTER
                } else {
                    egui::Align2::CENTER_CENTER
                },
                label,
                egui::FontId::proportional(10.0),
                MUTED,
            );
        }

        if self.plot.len() > 1 {
            let points: Vec<_> = self
                .plot
                .iter()
                .enumerate()
                .map(|(index, db)| {
                    let t = index as f32 / (self.plot.len() - 1) as f32;
                    egui::pos2(plot.left() + t * plot.width(), y_for(*db))
                })
                .collect();
            painter.add(egui::Shape::line(
                points.clone(),
                egui::Stroke::new(
                    6.0_f32,
                    egui::Color32::from_rgba_unmultiplied(101, 165, 255, 30),
                ),
            ));
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(2.2_f32, theme::BLUE),
            ));
        }

        let chain = EqChain::new(sample_rate as f32, &params);
        let frequencies: [f32; N_BANDS] = std::array::from_fn(|index| {
            params
                .effective_band(index)
                .frequency_hz
                .min(sample_rate as f32 * 0.49)
        });
        for (index, frequency) in frequencies.iter().enumerate() {
            let position = egui::pos2(x_for(*frequency), y_for(chain.response_db(*frequency)));
            let active = index == self.plot_active_band;
            if active {
                painter.circle_filled(
                    position,
                    8.0,
                    egui::Color32::from_rgba_unmultiplied(255, 148, 31, 36),
                );
                painter.circle_filled(position, 6.0, theme::WHITE);
                painter.circle_filled(position, 4.5, theme::WARN);
            } else {
                painter.circle_filled(position, 5.0, theme::WHITE);
                painter.circle_stroke(
                    position,
                    5.0,
                    egui::Stroke::new(
                        2.0_f32,
                        if params.effective_band(index).enabled {
                            theme::BLUE
                        } else {
                            MUTED
                        },
                    ),
                );
            }
        }

        if response.clicked() {
            response.request_focus();
        }
        if let Some(pointer) = response.interact_pointer_pos() {
            let pointer = egui::pos2(pointer.x.clamp(plot.left(), plot.right()), pointer.y);
            let nearest = frequencies
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    (x_for(**a) - pointer.x)
                        .abs()
                        .partial_cmp(&(x_for(**b) - pointer.x).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(index, _)| index)
                .unwrap_or(self.plot_active_band);
            if response.drag_started() || response.clicked() {
                self.plot_active_band = nearest;
            }
            if response.dragged() && params.effective_band(self.plot_active_band).enabled {
                let desired = ((plot.center().y - pointer.y) / (plot.height() * 0.5) * extent)
                    .clamp(-12.0, 12.0);
                let current = chain.response_db(frequencies[self.plot_active_band]);
                self.sound.gains[self.plot_active_band] =
                    (self.sound.gains[self.plot_active_band] + desired - current)
                        .clamp(-12.0, 12.0);
            }
            if response.double_clicked() {
                self.sound.gains[nearest] = 0.0;
            }
            if response.hovered() || response.dragged() {
                theme::tooltip_chip(
                    &painter,
                    pointer,
                    &format!(
                        "{} Hz   {:+.1} dB",
                        frequency_label(params.effective_band(nearest).frequency_hz),
                        self.sound.gains[nearest]
                    ),
                    rect,
                );
            }
        }
        if response.has_focus() {
            let left = ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft));
            let right = ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight));
            let up =
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
            let down = ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
            if left {
                self.plot_active_band = self.plot_active_band.saturating_sub(1);
            }
            if right {
                self.plot_active_band = (self.plot_active_band + 1).min(N_BANDS - 1);
            }
            if up && params.effective_band(self.plot_active_band).enabled {
                self.sound.gains[self.plot_active_band] =
                    (self.sound.gains[self.plot_active_band] + 0.5).min(12.0);
            }
            if down && params.effective_band(self.plot_active_band).enabled {
                self.sound.gains[self.plot_active_band] =
                    (self.sound.gains[self.plot_active_band] - 0.5).max(-12.0);
            }
        }
        response.on_hover_text(
            "Drag a frequency handle to change its gain. Double-click to reset that band.",
        );
        ui.add_space(3.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().interact_size.y = 20.0;
            theme::status_dot(ui, theme::BLUE);
            ui.label(
                egui::RichText::new("Response: preamp + 11 bands + bass shelf")
                    .size(10.5)
                    .color(MUTED),
            );
            ui.label(
                egui::RichText::new(format!("{} Hz preview", sample_rate))
                    .size(10.5)
                    .color(MUTED),
            );
            ui.label(
                egui::RichText::new(format!("Width {:.2}", self.sound.width))
                    .size(10.5)
                    .color(MUTED),
            );
            ui.label(
                egui::RichText::new(if self.sound.limiter {
                    "Limiter on"
                } else {
                    "Limiter off"
                })
                .size(10.5)
                .color(MUTED),
            );
            if params.comparison_gain_db < -0.05 {
                ui.label(
                    egui::RichText::new(format!("A/B trim {:.1} dB", params.comparison_gain_db))
                        .size(10.5)
                        .color(MUTED),
                );
            }
        });
    }

    fn history_buttons(&mut self, ui: &mut egui::Ui) {
        if theme::compact_button_icon(ui, Icon::Redo, "Redo", self.history.can_redo())
            .on_hover_text("Redo · Ctrl+Y / Ctrl+Shift+Z")
            .clicked()
        {
            self.redo();
        }
        if theme::compact_button_icon(ui, Icon::Undo, "Undo", self.history.can_undo())
            .on_hover_text("Undo · Ctrl+Z")
            .clicked()
        {
            self.undo();
        }
    }

    fn equalizer(&mut self, ui: &mut egui::Ui, available: egui::Vec2) {
        let content_bottom = ui.cursor().top() + available.y - 24.0;
        let title = self
            .selected_profile_name()
            .unwrap_or_else(|| "Custom equalizer".into());
        let modified = self.selected_preset_modified();
        let preset_badge = if modified {
            "Unsaved changes"
        } else {
            "Custom preset"
        };
        let badge_width = if self.cfg.selected_preset.is_some() {
            ui.painter()
                .layout_no_wrap(preset_badge.into(), egui::FontId::proportional(10.5), MUTED)
                .size()
                .x
                + 12.0
        } else {
            0.0
        };
        let controls_width: f32 = ["Help", "Undo", "Redo"]
            .iter()
            .map(|label| {
                ui.painter()
                    .layout_no_wrap((*label).into(), egui::FontId::proportional(11.5), INK)
                    .size()
                    .x
                    + 34.0
                    + 6.0
            })
            .sum();
        let compact_header = ui.available_width() < controls_width + badge_width + 170.0;
        ui.horizontal(|ui| {
            theme::icon_badge(ui, Icon::Sliders);
            ui.add_space(2.0);
            let title_width = if compact_header {
                ui.available_width()
            } else {
                (ui.available_width() - controls_width - badge_width - 12.0).max(80.0)
            };
            ui.add_sized(
                [title_width, 30.0],
                egui::Label::new(
                    egui::RichText::new(&title)
                        .size(18.0)
                        .strong()
                        .family(theme::semibold())
                        .color(INK),
                )
                .truncate(),
            )
            .on_hover_text(&title);
            if !compact_header && self.cfg.selected_preset.is_some() {
                self.preset_badge(ui, modified);
            }
            if !compact_header {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if theme::compact_button_icon(ui, Icon::Help, "Help", true)
                        .on_hover_text("Shortcuts and audio help · F1")
                        .clicked()
                    {
                        self.help_open = true;
                    }
                    self.history_buttons(ui);
                });
            }
        });
        if compact_header {
            ui.horizontal(|ui| {
                if self.cfg.selected_preset.is_some() {
                    self.preset_badge(ui, modified);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if theme::compact_button_icon(ui, Icon::Help, "Help", true)
                        .on_hover_text("Shortcuts and audio help · F1")
                        .clicked()
                    {
                        self.help_open = true;
                    }
                    self.history_buttons(ui);
                });
            });
        }
        ui.add_space(6.0);
        self.parametric_controls(ui);
        let height = (available.y - 24.0).max(280.0);
        let graph_height = (height * 0.28).clamp(140.0, 210.0);
        let fader_height = (height * 0.17).clamp(84.0, 144.0);
        self.response_plot(ui, graph_height);
        ui.add_space(6.0);
        let params = self.params();
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.columns(N_BANDS, |columns| {
                for (index, column) in columns.iter_mut().enumerate() {
                    column.spacing_mut().item_spacing.y = 3.0;
                    column.vertical_centered(|ui| {
                        let setting = params.effective_band(index);
                        let label = if params.parametric_enabled {
                            frequency_label(setting.frequency_hz)
                        } else {
                            BAND_LABEL[index].to_string()
                        };
                        if ui
                            .add(
                                egui::Label::new(egui::RichText::new(&label).size(10.5).color(
                                    if index == self.plot_active_band {
                                        TRACE
                                    } else {
                                        MUTED
                                    },
                                ))
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Select this band for parametric editing")
                            .clicked()
                        {
                            self.plot_active_band = index;
                        }
                        ui.add_space(1.0);
                        let fader = ui
                            .add_enabled_ui(setting.enabled, |ui| {
                                theme::vertical_fader(
                                    ui,
                                    &mut self.sound.gains[index],
                                    -12.0,
                                    12.0,
                                    fader_height,
                                )
                            })
                            .inner;
                        fader.widget_info(|| {
                            egui::WidgetInfo::slider(
                                setting.enabled,
                                f64::from(self.sound.gains[index]),
                                format!("Band {} · {} Hz", index + 1, label),
                            )
                        });
                        self.layout.faders[index] = fader.rect;
                        if fader.clicked() || fader.dragged() || fader.has_focus() {
                            self.plot_active_band = index;
                        }
                        ui.label(
                            egui::RichText::new(format!("{:+.1}", self.sound.gains[index]))
                                .monospace()
                                .size(11.5)
                                .strong()
                                .color(INK),
                        );
                    });
                }
            });
        });
        ui.add_space(6.0);
        let actions = ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if theme::medium_button_icon(ui, Icon::Restore, "Restore AutoEQ").clicked() {
                if let Some(index) = self.selected {
                    self.select_profile(index);
                }
            }
            if theme::medium_button_icon(ui, Icon::Bars, "Flat bands").clicked() {
                self.sound.gains = [0.0; N_BANDS];
            }
            if theme::medium_button_icon(ui, Icon::Layers, "Reset shaping").clicked() {
                self.sound.bass_db = 0.0;
                self.sound.width = 1.0;
                self.sound.crossfeed = 0.0;
            }
            if theme::medium_button_icon(ui, Icon::SpeakerOff, "Reset all sound").clicked() {
                self.sound = SoundSettings::default();
                self.selected = None;
                self.cfg.selected_preset = None;
            }
        });
        self.layout.actions = actions.response.rect;
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().interact_size.y = 24.0;
            if theme::checkbox(ui, &mut self.cfg.spectrum_enabled, "Live spectrum")
                .on_hover_text("Show the existing output spectrum as mirrored rainbow dots")
                .changed()
            {
                self.mark_dirty();
            }
            ui.add_space(4.0);
            theme::eyebrow(ui, "Output spectrum · log frequency · auto-range");
        });
        self.layout.spectrum = if self.cfg.spectrum_enabled {
            // Allocate the actual remaining height after headers, wrapping
            // controls and actions, rather than guessing their combined size.
            let spectrum_height = (content_bottom - ui.cursor().top()).clamp(60.0, 200.0);
            let hint = if self.engine.is_none() {
                Some("Start audio to see the live spectrum")
            } else if self
                .spectrum_levels
                .iter()
                .all(|level| *level <= SPECTRUM_FLOOR_DB + 0.1)
            {
                Some("Waiting for an audio signal")
            } else {
                None
            };
            Some(theme::spectrum(
                ui,
                &self.spectrum_levels,
                spectrum_height,
                hint,
            ))
        } else {
            None
        };
    }

    fn preset_badge(&self, ui: &mut egui::Ui, modified: bool) {
        let name = self.cfg.selected_preset.as_deref().unwrap_or("");
        ui.label(egui::RichText::new(if modified { "Unsaved changes" } else { "Custom preset" }).size(10.5).color(if modified { theme::WARN_TEXT } else { TRACE }))
            .on_hover_text(if modified { format!("Current sound differs from “{name}”. Save with Ctrl+S; the saved preset is unchanged.") } else { format!("Current sound matches “{name}”.") });
    }

    fn parametric_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().interact_size.y = 24.0;
            theme::checkbox(ui, &mut self.sound.parametric_enabled, "Parametric EQ")
                .on_hover_text("Edit frequency, Q and filter type on the existing 11 bands. Off uses the original graphic/AutoEQ mapping.");
            if !self.sound.parametric_enabled {
                ui.label(egui::RichText::new("Graphic · 11 fixed bands").size(11.5).color(MUTED));
                return;
            }
            egui::ComboBox::from_id_salt("parametric_band")
                .width(76.0).truncate()
                .selected_text(egui::RichText::new(format!("Band {}", self.plot_active_band + 1)).size(11.5))
                .show_ui(ui, |ui| {
                    for index in 0..N_BANDS {
                        ui.selectable_value(&mut self.plot_active_band, index, format!("Band {} · {} Hz", index + 1, frequency_label(self.sound.band_settings[index].frequency_hz)));
                    }
                });
            let setting = &mut self.sound.band_settings[self.plot_active_band];
            ui.label(egui::RichText::new("Freq").size(11.5).color(MUTED));
            ui.add_sized([78.0, 26.0], egui::DragValue::new(&mut setting.frequency_hz).range(20.0..=20_000.0).speed(10.0).max_decimals(0).suffix(" Hz"))
                .on_hover_text("Click to type a frequency. Filters are limited below the active device's Nyquist frequency.");
            ui.label(egui::RichText::new("Q").size(11.5).color(MUTED));
            let _q_editor = ui.add_sized([48.0, 26.0], egui::DragValue::new(&mut setting.q).range(0.1..=12.0).speed(0.03).min_decimals(2).max_decimals(2))
                .on_hover_text("Higher Q makes peaks narrower and shelves more resonant.");
            #[cfg(test)]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("qa_q_editor"), _q_editor.rect));
            egui::ComboBox::from_id_salt("parametric_kind")
                .width(91.0).truncate().selected_text(egui::RichText::new(setting.kind.label()).size(11.5))
                .show_ui(ui, |ui| {
                    for kind in BandKind::ALL { ui.selectable_value(&mut setting.kind, kind, kind.label()); }
                });
            theme::checkbox(ui, &mut setting.enabled, "Enabled");
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui, available_height: f32) {
        self.sound_shaping_card(ui);
        ui.add_space(8.0);
        self.layout.compare = ui.scope(|ui| self.compare_card(ui)).response.rect;
        ui.add_space(8.0);
        self.layout.presets = ui.scope(|ui| self.presets_card(ui)).response.rect;
        ui.add_space(8.0);
        self.application_card(ui, available_height >= 840.0);
    }

    fn sound_shaping_card(&mut self, ui: &mut egui::Ui) {
        theme::section_card(ui, Icon::Speaker, "Sound shaping", |ui| {
            let preamp = format!("{:+.1} dB", self.sound.preamp_db);
            theme::param_slider(
                ui,
                "preamp",
                "Preamp",
                &mut self.sound.preamp_db,
                -24.0,
                12.0,
                preamp,
            );
            let bass = format!("{:+.1} dB", self.sound.bass_db);
            theme::param_slider(
                ui,
                "bass",
                "Bass boost",
                &mut self.sound.bass_db,
                0.0,
                15.0,
                bass,
            );
            let width = format!("{:.2}", self.sound.width);
            theme::param_slider(
                ui,
                "width",
                "Stereo width",
                &mut self.sound.width,
                0.0,
                2.0,
                width,
            );
            let crossfeed = format!("{:.0}%", self.sound.crossfeed * 100.0);
            theme::param_slider(
                ui,
                "crossfeed",
                "Crossfeed",
                &mut self.sound.crossfeed,
                0.0,
                1.0,
                crossfeed,
            )
            .on_hover_text(
                "Low-frequency feed to the opposite ear. 0% is off; mono content is preserved.",
            );
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                theme::checkbox(ui, &mut self.sound.limiter, "Limiter");
                theme::checkbox(ui, &mut self.sound.auto_preamp, "Automatic preamp");
            });

            let peak = max_response_db(
                self.engine
                    .as_ref()
                    .map(|e| e.render_rate as f32)
                    .unwrap_or(48_000.0),
                &self.sound.to_params(),
            );
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                if peak <= -0.5 {
                    theme::status_dot(ui, theme::OK);
                    ui.label(
                        egui::RichText::new(format!("Headroom {:.1} dB", -peak))
                            .size(11.5)
                            .color(theme::OK),
                    );
                } else if peak <= 0.0 {
                    theme::warning(ui, "Headroom is tight");
                } else {
                    theme::warning(ui, &format!("Overload risk +{peak:.1} dB"));
                }
                if peak > -0.5
                    && theme::small_button(ui, "Set safe preamp")
                        .on_hover_text(
                            "Adds 0.5 dB headroom, without relying on temporary A/B attenuation.",
                        )
                        .clicked()
                {
                    self.sound.preamp_db -= peak + 0.5;
                    self.sound.preamp_db = self.sound.preamp_db.clamp(-24.0, 12.0);
                }
            });
            ui.label(
                egui::RichText::new(format!(
                    "Limiter reduction {:.1} dB",
                    self.telemetry.limiter_reduction_db
                ))
                .size(10.5)
                .color(MUTED),
            );
        });
    }

    fn compare_card(&mut self, ui: &mut egui::Ui) {
        theme::section_card(ui, Icon::Compare, "Compare A/B", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for slot in 0..2 {
                    let active = self.active_ab == slot;
                    if theme::chip(ui, if slot == 0 { "A" } else { "B" }, active, 40.0).clicked() {
                        self.ab_slots[self.active_ab] = self.sound.clone();
                        self.active_ab = slot;
                        self.sound = self.ab_slots[slot].clone();
                        self.skip_history_once = true;
                    }
                }
                let copy_label = if self.active_ab == 0 {
                    "Copy A to B"
                } else {
                    "Copy B to A"
                };
                if theme::small_button(ui, copy_label).clicked() {
                    self.ab_slots[1 - self.active_ab] = self.sound.clone();
                }
            });
            ui.horizontal_wrapped(|ui| {
                if theme::checkbox(ui, &mut self.cfg.level_match_ab, "Level match A/B")
                    .on_hover_text("Calibrates both setups against the same real stereo input. Sample RMS before the limiter; attenuation only, not LUFS.").changed() {
                    self.match_key = None;
                    self.next_calibration = Instant::now();
                    self.mark_dirty();
                }
                if self.cfg.level_match_ab && theme::link_button(ui, "Recalibrate").clicked() {
                    self.match_key = None;
                    self.next_calibration = Instant::now();
                }
            });
            if self.cfg.level_match_ab {
                let label = if self
                    .current_ab_slots()
                    .iter()
                    .any(|sound| !sound.correction_enabled)
                {
                    "Enable correction in both slots to match".into()
                } else if self.engine.is_none() {
                    "Start audio to calibrate levels".into()
                } else if self.match_key.as_ref() != Some(&self.current_ab_slots()) {
                    "Play audio to calibrate levels".into()
                } else if let Some(calibration) = self.match_calibration {
                    format!(
                        "RMS trim {:.1} dB{}",
                        calibration.trims_db[self.active_ab],
                        if calibration.limited {
                            " · safety limit"
                        } else {
                            ""
                        }
                    )
                } else {
                    "Play audio to calibrate levels".into()
                };
                ui.label(egui::RichText::new(label).size(10.5).color(MUTED));
            }
        });
    }

    fn presets_card(&mut self, ui: &mut egui::Ui) {
        theme::section_card(ui, Icon::Preset, "Custom presets", |ui| {
            if theme::text_field(ui, &mut self.preset_name, "Preset name").changed() {
                self.preset_error = None;
            }
            if let Some(error) = &self.preset_error {
                ui.label(
                    egui::RichText::new(error)
                        .size(11.5)
                        .color(theme::WARN_TEXT),
                );
            }
            ui.add_space(5.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if theme::small_button(ui, "Save preset")
                    .on_hover_text("Save this complete sound setup · Ctrl+S")
                    .clicked()
                {
                    self.save_current_preset();
                }
                if theme::small_button(ui, "Import").clicked() {
                    self.import_preset();
                }
                if theme::small_button(ui, "Export").clicked() {
                    self.export_preset();
                }
            });
            let names: Vec<_> = self
                .cfg
                .presets
                .iter()
                .map(|preset| preset.name.clone())
                .collect();
            if !names.is_empty() {
                ui.add_space(2.0);
                let selected_text = self
                    .cfg
                    .selected_preset
                    .clone()
                    .unwrap_or_else(|| "Choose preset".into());
                egui::ComboBox::from_id_salt("custom_preset")
                    .selected_text(selected_text)
                    .width(ui.available_width())
                    .truncate()
                    .show_ui(ui, |ui| {
                        for name in &names {
                            if ui
                                .selectable_label(
                                    self.cfg.selected_preset.as_deref() == Some(name),
                                    name,
                                )
                                .clicked()
                            {
                                self.apply_preset(name);
                                ui.close_menu();
                            }
                        }
                    });
                if let Some(name) = self.cfg.selected_preset.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if theme::small_button(ui, "Delete preset").clicked() {
                            self.cfg.remove_preset(&name);
                            self.mark_dirty();
                        }
                        if let Some(device) = self.render.clone() {
                            let mapped =
                                self.cfg.device_presets.preset_for(&device) == Some(name.as_str());
                            let mut remember = mapped;
                            if theme::checkbox(ui, &mut remember, "Use for this output").changed() {
                                if remember {
                                    self.cfg.device_presets.assign(device, name);
                                } else {
                                    self.cfg.device_presets.remove(&device);
                                }
                                self.mark_dirty();
                            }
                        }
                    });
                }
            }
        });
    }

    fn application_card(&mut self, ui: &mut egui::Ui, expanded: bool) {
        theme::card_at(
            ui,
            theme::Elevation::Mid,
            16.0,
            egui::Margin::same(10.0),
            |ui| {
                let id = ui.make_persistent_id("application_settings");
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    expanded,
                )
                .show_header(ui, |ui| {
                    theme::section_title(ui, Icon::Settings, "Application")
                })
                .body(|ui| {
                    if theme::checkbox(
                        ui,
                        &mut self.cfg.auto_reconnect,
                        "Reconnect devices automatically",
                    )
                    .changed()
                    {
                        self.mark_dirty();
                    }
                    if theme::checkbox(
                        ui,
                        &mut self.cfg.auto_start_audio,
                        "Start audio when Rometric opens",
                    )
                    .changed()
                    {
                        self.mark_dirty();
                    }
                    if theme::checkbox(ui, &mut self.cfg.minimize_to_tray, "Close window to tray")
                        .changed()
                    {
                        self.mark_dirty();
                    }
                    if theme::checkbox(ui, &mut self.cfg.reduced_motion, "Reduce interface motion")
                        .changed()
                    {
                        theme::install(ui.ctx(), self.cfg.reduced_motion);
                        self.mark_dirty();
                    }
                    let mut autostart = self.autostart;
                    if theme::checkbox(ui, &mut autostart, "Start with Windows").changed() {
                        self.toggle_autostart();
                    }
                    if theme::medium_button_icon(ui, Icon::Diagnostics, "Open diagnostics")
                        .clicked()
                    {
                        self.diagnostics_open = true;
                    }
                });
            },
        );
    }

    fn import_preset(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Rometric preset", &["json"])
            .pick_file()
        else {
            return;
        };
        let result = std::fs::read_to_string(&path)
            .map_err(|err| format!("Could not read preset: {err}"))
            .and_then(|text| PresetFile::from_json(&text));
        match result {
            Ok(file) => {
                self.request_preset_save(
                    file.preset,
                    PresetOrigin::Import(path.display().to_string()),
                );
            }
            Err(err) => self.log(err),
        }
    }

    fn export_preset(&mut self) {
        let name = self
            .cfg
            .selected_preset
            .clone()
            .or_else(|| {
                (!self.preset_name.trim().is_empty()).then(|| self.preset_name.trim().to_string())
            })
            .unwrap_or_else(|| "Rometric preset".into());
        let mut preset = Preset::new(name.clone(), self.sound.clone());
        preset.profile = self.selected_profile_name();
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Rometric preset", &["json"])
            .set_file_name(format!("{}.json", safe_filename(&name)))
            .save_file()
        else {
            return;
        };
        let result = PresetFile::new(preset)
            .to_json()
            .and_then(|json| std::fs::write(&path, json).map_err(|err| err.to_string()));
        match result {
            Ok(()) => self.log(format!("Exported preset to {}.", path.display())),
            Err(err) => self.log(format!("Could not export preset: {err}")),
        }
    }

    fn show_dashboard(&mut self, ctx: &egui::Context) {
        self.layout = LayoutTrace::default();
        let enabled = self.pending_preset.is_none();
        let top = egui::TopBottomPanel::top("routing")
            .frame(egui::Frame::none().fill(SHELL).inner_margin(egui::Margin {
                left: 8.0,
                right: 8.0,
                top: 8.0,
                bottom: 8.0,
            }))
            .show(ctx, |ui| {
                if !enabled {
                    ui.disable();
                }
                theme::card_with(ui, theme::CARD_RADIUS, 10.0, |ui| self.toolbar(ui));
            });
        self.layout.toolbar = top.response.rect;
        let available = ctx.available_rect().size();
        let profiles_beside = available.x >= 720.0;
        // Below this content-driven minimum, use the existing sections through
        // compact navigation instead of squeezing eleven faders into a sliver.
        let three_columns = available.x >= 1040.0;

        if profiles_beside {
            let width = (available.x * 0.22).max(210.0);
            egui::SidePanel::left("profiles_wide")
                .resizable(true)
                .default_width(width)
                .min_width((width * 0.86).max(200.0))
                .max_width((width * 1.14).max(220.0))
                .frame(egui::Frame::none().fill(SHELL).inner_margin(egui::Margin {
                    left: 8.0,
                    right: 4.0,
                    top: 8.0,
                    bottom: 8.0,
                }))
                .show(ctx, |ui| {
                    if !enabled {
                        ui.disable();
                    }
                    theme::card(ui, |ui| self.profile_browser(ui));
                });
        }
        if three_columns {
            let width = (available.x * 0.25).max(250.0);
            egui::SidePanel::right("inspector_wide")
                .resizable(true)
                .default_width(width)
                .min_width((width * 0.86).max(240.0))
                .max_width((width * 1.14).max(260.0))
                .frame(egui::Frame::none().fill(SHELL).inner_margin(egui::Margin {
                    left: 4.0,
                    right: 8.0,
                    top: 8.0,
                    bottom: 8.0,
                }))
                .show(ctx, |ui| {
                    if !enabled {
                        ui.disable();
                    }
                    let height = ui.available_height();
                    let scroll = egui::ScrollArea::vertical()
                        .id_salt("inspector_scroll")
                        .max_height(height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.inspector(ui, height));
                    self.layout.inspector_scroll =
                        scroll.content_size.y > scroll.inner_rect.height() + 0.5;
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(SHELL).inner_margin(egui::Margin {
                left: if profiles_beside { 4.0 } else { 8.0 },
                right: if three_columns { 4.0 } else { 8.0 },
                top: 8.0,
                bottom: 8.0,
            }))
            .show(ctx, |ui| {
                if !enabled {
                    ui.disable();
                }
                self.layout.center = ui.max_rect();
                if !three_columns {
                    if profiles_beside && self.compact_view == CompactView::Profiles {
                        self.compact_view = CompactView::Equalizer;
                    }
                    ui.horizontal(|ui| {
                        for (view, label) in [
                            (CompactView::Equalizer, "Equalizer"),
                            (CompactView::Profiles, "Profiles"),
                            (CompactView::Sound, "Sound / presets"),
                        ] {
                            if view == CompactView::Profiles && profiles_beside {
                                continue;
                            }
                            if theme::chip(
                                ui,
                                label,
                                self.compact_view == view,
                                if view == CompactView::Sound {
                                    130.0
                                } else {
                                    96.0
                                },
                            )
                            .clicked()
                            {
                                self.compact_view = view;
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
                let content = ui.available_size();
                if !three_columns && self.compact_view == CompactView::Profiles {
                    theme::card(ui, |ui| self.profile_browser(ui));
                } else if !three_columns && self.compact_view == CompactView::Sound {
                    let scroll = egui::ScrollArea::vertical()
                        .id_salt("compact_inspector_scroll")
                        .max_height(content.y)
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.inspector(ui, content.y));
                    self.layout.inspector_scroll =
                        scroll.content_size.y > scroll.inner_rect.height() + 0.5;
                } else {
                    let scroll = egui::ScrollArea::vertical()
                        .id_salt("central_scroll")
                        .max_height(content.y)
                        .auto_shrink([false, false])
                        .show(ui, |ui| theme::card(ui, |ui| self.equalizer(ui, content)));
                    self.layout.center_scroll =
                        scroll.content_size.y > scroll.inner_rect.height() + 0.5;
                }
            });
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        if !self.help_open || self.pending_preset.is_some() {
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.help_open = false;
            return;
        }
        let mut open = self.help_open;
        egui::Window::new("Shortcuts and audio help").id(egui::Id::new("signal_bench_help"))
            .open(&mut open).collapsible(false).default_width(540.0)
            .max_width((ctx.screen_rect().width() - 32.0).max(280.0))
            .max_height(ctx.screen_rect().height() * 0.84)
            .constrain_to(ctx.screen_rect()).show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    theme::section_title(ui, Icon::Help, "Keyboard shortcuts");
                    egui::Grid::new("help_shortcuts").spacing(egui::vec2(16.0, 5.0)).show(ui, |ui| {
                        for (key, action) in [
                            ("F1", "Open or close this help"), ("Esc", "Close help / cancel preset replacement"),
                            ("Ctrl+F", "Focus headphone profile search"), ("Ctrl+S", "Save the current named preset"),
                            ("Ctrl+Z", "Undo a sound edit"), ("Ctrl+Y / Ctrl+Shift+Z", "Redo a sound edit"),
                            ("Up / Down", "Adjust a focused fader by 0.5 dB"),
                            ("Left / Right", "Choose a band in the focused response graph"),
                        ] {
                            ui.monospace(key); ui.label(action); ui.end_row();
                        }
                    });
                    ui.add_space(10.0);
                    theme::section_title(ui, Icon::Sliders, "Graph and equalizer");
                    ui.label("Drag vertically in the response graph to change the nearest band's gain. Double-click a point or fader to reset that gain to 0 dB.");
                    ui.label("Enable Parametric EQ above the graph, then select a band using its label, fader or band selector. Edit its frequency, Q, filter type and Enabled state in that same row. Higher Q narrows peaks or increases shelf resonance.");
                    ui.label("Turning parametric mode off uses the original 11 fixed bands. Restore AutoEQ restores the selected headphone's original mapping. Frequencies above a device's usable Nyquist range are limited in the DSP.");
                    ui.add_space(10.0);
                    theme::section_title(ui, Icon::Speaker, "Routing and headroom");
                    ui.label("For system audio, set Windows or your player to a virtual cable's playback endpoint. Choose the cable's capture endpoint as Input and your headphones/DAC as Output, then Start audio. Correction on/off keeps that route running.");
                    theme::warning(ui, "Do not route a virtual cable back into itself, or choose a microphone-to-speaker route that can feed back.");
                    ui.label("Set safe preamp lowers the predicted EQ response to leave 0.5 dB headroom. It does not rely on temporary A/B level matching. Keep the limiter on for unexpected peaks; neither control measures acoustic listening level.");
                    ui.add_space(10.0);
                    theme::section_title(ui, Icon::Compare, "Crossfeed and level matching");
                    ui.label("Crossfeed blends low-frequency information toward the other ear while preserving mono. 0% is off; it is separate from Stereo width.");
                    ui.label("Level match A/B measures both setups against the SAME recent, real stereo input. It matches pre-limiter sample RMS by attenuating only the louder setup, up to 24 dB. This is not LUFS or an acoustic/perceptual loudness measurement. A limit is shown if full matching is not possible.");
                    ui.label("Calibration refreshes when either setup changes. Use Recalibrate after changing program material. Matching trims are smoothed, temporary and never written into a preset.");
                    ui.label("Correction bypass remains a complete bypass. To level-match corrected versus uncorrected sound, use a flat, enabled setup in the other slot rather than bypassing it.");
                    ui.add_space(10.0);
                    theme::section_title(ui, Icon::Bars, "Live spectrum and meters");
                    ui.label("The centre-column rainbow dots show the real output spectrum on a logarithmic frequency axis. The mirrored display is visual only—not left/right channels or a waveform. Bars auto-range visually for quiet audio; hover dBFS remains absolute. Silence produces no colored bars.");
                    ui.label("Input/output readouts are sample peaks in dBFS. 0 dBFS means full scale; silence is −∞. Unsaved changes means the current sound or headphone selection differs from the saved preset; autosaving application settings does not replace that preset.");
                });
            });
        self.help_open = open;
    }

    fn preset_overwrite_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_preset.clone() else {
            return;
        };
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.resolve_preset_overwrite(false);
            return;
        }
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("preset_overwrite_scrim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(42));
            });
        let mut open = true;
        let mut decision = None;
        egui::Window::new("Replace saved preset?").id(egui::Id::new("preset_overwrite"))
            .order(egui::Order::Foreground).open(&mut open).collapsible(false).resizable(false)
            .default_width(390.0).max_width((screen.width() - 40.0).max(240.0))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).constrain_to(screen)
            .show(ctx, |ui| {
                ui.label(format!("A preset named “{}” already exists.", pending.preset.name));
                ui.label("Replace its saved sound setup? Cancel keeps the saved preset and output assignments unchanged.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel = theme::small_button(ui, "Cancel");
                    if pending.focus_cancel { cancel.request_focus(); }
                    if cancel.clicked() { decision = Some(false); }
                    if theme::primary_button(ui, Icon::Preset, "Replace preset").clicked() { decision = Some(true); }
                });
            });
        if let Some(pending) = &mut self.pending_preset {
            pending.focus_cancel = false;
        }
        if !open {
            decision = Some(false);
        }
        if let Some(replace) = decision {
            self.resolve_preset_overwrite(replace);
        }
    }

    fn diagnostics_window(&mut self, ctx: &egui::Context) {
        if !self.diagnostics_open {
            return;
        }
        let mut open = self.diagnostics_open;
        let text = self.diagnostics_text();
        egui::Window::new("Audio diagnostics")
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.monospace(&text);
                ui.horizontal(|ui| {
                    if ui.button("Copy diagnostics").clicked() {
                        ctx.copy_text(text.clone());
                    }
                    if ui.button("Retry audio now").clicked() {
                        self.start_engine();
                    }
                    if ui.button("Clear log").clicked() {
                        self.diagnostics.clear();
                    }
                });
            });
        self.diagnostics_open = open;
    }

    fn diagnostics_text(&self) -> String {
        let mut lines = vec![
            format!("{} {}", branding::APP_NAME, env!("CARGO_PKG_VERSION")),
            format!(
                "State: {}",
                self.engine_status.label(self.sound.correction_enabled)
            ),
            format!(
                "Capture: {}",
                self.capture.as_deref().unwrap_or("not selected")
            ),
            format!(
                "Output: {}",
                self.render.as_deref().unwrap_or("not selected")
            ),
            format!("Input peak: {:.3}", self.meter_input),
            format!("Output peak: {:.3}", self.meter_output),
            format!(
                "Limiter reduction: {:.2} dB",
                self.telemetry.limiter_reduction_db
            ),
            format!("Processed frames: {}", self.telemetry.processed_frames),
            format!("Queue overruns: {}", self.telemetry.overruns),
            format!("Queue underruns: {}", self.telemetry.underruns),
            format!("Config: {}", Config::path().display()),
        ];
        if let Some(engine) = &self.engine {
            lines.push(format!(
                "Applied route: {} > {}",
                engine.capture, engine.render
            ));
        }
        if let Some(error) = &self.save_error {
            lines.push(format!("Save error: {error}"));
        }
        if let Some(error) = &self.tray_error {
            lines.push(format!("Tray error: {error}"));
        }
        lines.push("Recent events:".into());
        lines.extend(self.diagnostics.iter().map(|entry| format!("  {entry}")));
        lines.join("\n")
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);
        self.process_tray(ctx);
        self.process_engine_error();
        self.poll_devices_and_reconnect();
        self.update_telemetry();
        let sound_before = self.sound.clone();

        if ctx.input(|input| input.viewport().close_requested())
            && self.cfg.minimize_to_tray
            && !self.quitting
            && self.tray.is_some()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        self.show_dashboard(ctx);
        self.diagnostics_window(ctx);
        self.help_window(ctx);
        self.preset_overwrite_window(ctx);

        let pointer_down = ctx.input(|input| input.pointer.any_down());
        if self.sound != sound_before {
            if self.edit_baseline.is_none() {
                self.edit_baseline = Some(sound_before);
            }
            self.ab_slots[self.active_ab] = self.sound.clone();
            self.mark_dirty();
        }
        if !pointer_down {
            if let Some(baseline) = self.edit_baseline.take() {
                if !self.skip_history_once && baseline != self.sound {
                    self.history.record(baseline);
                }
            }
            self.skip_history_once = false;
        }
        self.update_comparison();
        let params = self.params();
        if self.shared_key != Some(params) {
            self.shared.store(&params);
            self.shared_key = Some(params);
        }
        self.save_if_due();

        if self.engine.is_some()
            || self.cfg.spectrum_enabled
            || self.retry_at.is_some()
            || self.tray.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(if self.cfg.spectrum_enabled {
                33
            } else {
                200
            }));
        }
    }
}

fn frequency_label(frequency: f32) -> String {
    if frequency >= 1000.0 {
        let khz = frequency / 1000.0;
        if (khz - khz.round()).abs() < 0.01 {
            format!("{khz:.0}k")
        } else {
            format!("{khz:.1}k")
        }
    } else {
        format!("{:.0}", frequency.round())
    }
}

fn short_device(device: Option<&str>) -> String {
    short_device_with_limit(device, 28)
}

fn short_device_with_limit(device: Option<&str>, limit: usize) -> String {
    let Some(device) = device else {
        return "Select device".into();
    };
    let clean: String = device
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if clean.chars().count() > limit {
        format!(
            "{}…",
            clean
                .chars()
                .take(limit.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        clean
    }
}

fn safe_filename(name: &str) -> String {
    let value: String = name
        .chars()
        .map(|character| {
            if "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    if value.trim().is_empty() {
        "rometric-preset".into()
    } else {
        value
    }
}
