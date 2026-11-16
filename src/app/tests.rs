use super::*;
use rometric::dsp::BAND_FREQ;

impl App {
    /// Real product/UI state without device enumeration, tray registration or
    /// audio startup. Only compiled in tests; the production controls are reused.
    fn test_instance(ctx: &egui::Context, cfg: Config) -> Self {
        // Match production font metrics so geometry tests measure the shipped UI.
        theme::install_fonts(ctx, theme::bundled_fonts());
        theme::install(ctx, cfg.reduced_motion);
        let db = AutoEq::load();
        let catalog = db.as_ref().ok().map(|db| db.catalog());
        let sound = cfg.sound_settings();
        let now = Instant::now();
        let mut app = Self {
            logo: None,
            db,
            catalog,
            query: String::new(),
            filtered: Vec::new(),
            groups: Vec::new(),
            selected: None,
            grouping: cfg.profile_grouping,
            shared: Arc::new(SharedParams::new(&sound.to_params())),
            engine: None,
            devices: DeviceList {
                inputs: vec![],
                outputs: vec![],
            },
            capture: None,
            render: None,
            last_render: None,
            engine_status: EngineStatus::Stopped,
            feedback_override: false,
            dirty_since: None,
            save_error: None,
            history: EditHistory::new(100),
            edit_baseline: None,
            skip_history_once: false,
            ab_slots: [sound.clone(), sound.clone()],
            active_ab: 0,
            preset_name: cfg.selected_preset.clone().unwrap_or_default(),
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
            tray: None,
            tray_signature: String::new(),
            tray_error: None,
            autostart: false,
            quitting: false,
            sound,
            cfg,
        };
        app.refresh_filter();
        app
    }
}

fn app(ctx: &egui::Context) -> App {
    let cfg = Config {
        spectrum_enabled: true,
        reduced_motion: true,
        auto_start_audio: false,
        auto_reconnect: false,
        minimize_to_tray: false,
        ..Config::default()
    };
    App::test_instance(ctx, cfg)
}

fn draw(ctx: &egui::Context, app: &mut App, size: egui::Vec2, time: f64, events: Vec<egui::Event>) {
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        time: Some(time),
        events,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .focused = Some(true);
    let _ = ctx.run(input, |ctx| {
        app.handle_shortcuts(ctx);
        app.show_dashboard(ctx);
        app.help_window(ctx);
        app.preset_overwrite_window(ctx);
    });
}

#[test]
fn cancelling_the_ui_overwrite_request_keeps_the_preset_and_selection() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.preset_name = "Desk".into();
    app.save_current_preset();
    let original = app.cfg.preset("Desk").unwrap().clone();
    app.sound.preamp_db = -6.0;
    app.save_current_preset();
    assert!(app.pending_preset.is_some());
    assert_eq!(app.cfg.preset("Desk"), Some(&original));
    app.resolve_preset_overwrite(false);
    assert!(app.pending_preset.is_none());
    assert_eq!(app.cfg.preset("Desk"), Some(&original));
    assert!(app.selected_preset_modified());
}

#[test]
fn confirming_the_ui_overwrite_clears_the_unsaved_indicator() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.preset_name = "Desk".into();
    app.save_current_preset();
    assert!(!app.selected_preset_modified());
    app.sound.crossfeed = 0.4;
    app.save_current_preset();
    app.resolve_preset_overwrite(true);
    assert_eq!(app.cfg.preset("Desk").unwrap().sound.crossfeed, 0.4);
    assert!(!app.selected_preset_modified());
}

#[test]
fn a_preset_without_a_headphone_profile_does_not_retain_the_old_profile() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.selected = Some(0);
    app.cfg
        .upsert_preset(Preset::new("No profile", SoundSettings::default()));
    app.apply_preset("No profile");
    assert_eq!(app.selected, None);
    assert_eq!(app.cfg.profile, None);
    assert!(!app.selected_preset_modified());
}

#[test]
fn restoring_autoeq_returns_to_the_original_graphic_band_mapping() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.sound.parametric_enabled = true;
    app.sound.band_settings[5].frequency_hz = 3500.0;
    app.select_profile(0);
    assert!(!app.sound.parametric_enabled);
    assert_eq!(app.sound.band_settings.map(|b| b.frequency_hz), BAND_FREQ);
    assert_eq!(app.sound.gains, app.db.as_ref().unwrap().bands_for(0));
}

#[test]
fn all_eleven_faders_and_the_integrated_spectrum_fit_at_desktop_sizes() {
    for size in [egui::vec2(1600.0, 900.0), egui::vec2(1920.0, 1080.0)] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        app.sound.parametric_enabled = true;
        let db = app.db.as_ref().unwrap();
        app.selected = db
            .profiles
            .iter()
            .enumerate()
            .max_by_key(|(_, p)| p.name.len())
            .map(|(i, _)| i);
        for step in 0..4 {
            draw(&ctx, &mut app, size, step as f64 * 0.2, vec![]);
        }
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        assert!(
            !app.layout.center_scroll,
            "equalizer overflow at {size:?}: {:?}",
            app.layout
        );
        for (index, rect) in app.layout.faders.iter().enumerate() {
            assert!(
                rect.height() >= 74.0 && screen.contains_rect(*rect),
                "band {index} clipped at {size:?}: {rect:?}"
            );
        }
        let spectrum = app
            .layout
            .spectrum
            .expect("spectrum inside the existing EQ card");
        assert!(screen.contains_rect(spectrum));
        assert!(spectrum.top() > app.layout.actions.bottom());
        assert!(screen.contains_rect(app.layout.compare));
        assert!(screen.contains_rect(app.layout.presets));
    }
}

#[test]
fn compact_sizes_keep_faders_inside_the_center_column_without_horizontal_overflow() {
    for size in [
        egui::vec2(1280.0, 720.0),
        egui::vec2(1366.0, 768.0),
        egui::vec2(900.0, 600.0),
    ] {
        let ctx = egui::Context::default();
        let mut app = app(&ctx);
        app.sound.parametric_enabled = true;
        for step in 0..4 {
            draw(&ctx, &mut app, size, step as f64 * 0.2, vec![]);
        }
        for rect in app.layout.faders {
            assert!(
                rect.left() >= app.layout.center.left()
                    && rect.right() <= app.layout.center.right(),
                "horizontal overflow at {size:?}: {rect:?}"
            );
        }
    }
}

#[test]
fn a_real_pointer_drag_updates_the_existing_fader() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let size = egui::vec2(1600.0, 900.0);
    for step in 0..3 {
        draw(&ctx, &mut app, size, step as f64 * 0.2, vec![]);
    }
    let rect = app.layout.faders[5];
    let start = rect.center();
    let end = start - egui::vec2(0.0, 24.0);
    draw(
        &ctx,
        &mut app,
        size,
        0.8,
        vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    draw(
        &ctx,
        &mut app,
        size,
        1.0,
        vec![egui::Event::PointerMoved(end)],
    );
    draw(
        &ctx,
        &mut app,
        size,
        1.2,
        vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert!(app.sound.gains[5] > 3.0);
    assert_eq!(app.plot_active_band, 5);
}

#[test]
fn comparison_only_settings_do_not_mark_a_saved_sound_as_modified() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.preset_name = "Desk".into();
    app.save_current_preset();
    app.cfg.level_match_ab = true;
    assert!(!app.selected_preset_modified());
    assert_eq!(
        app.params().comparison_gain_db,
        0.0,
        "no calibration without real running audio"
    );
}

#[test]
fn f1_opens_help_and_escape_closes_it_without_editing_sound() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let before = app.sound.clone();
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    draw(
        &ctx,
        &mut app,
        egui::vec2(1280.0, 720.0),
        0.0,
        vec![key(egui::Key::F1)],
    );
    assert!(app.help_open);
    draw(
        &ctx,
        &mut app,
        egui::vec2(1280.0, 720.0),
        0.2,
        vec![key(egui::Key::Escape)],
    );
    assert!(!app.help_open);
    assert_eq!(app.sound, before);
}

#[test]
fn importing_an_existing_name_uses_the_same_confirmation_without_early_application() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.preset_name = "Desk".into();
    app.save_current_preset();
    let before = app.sound.clone();
    let imported = Preset::new(
        "Desk",
        SoundSettings {
            crossfeed: 0.5,
            ..Default::default()
        },
    );
    app.request_preset_save(
        imported.clone(),
        PresetOrigin::Import("fixture.json".into()),
    );
    assert_eq!(app.sound, before);
    assert_eq!(app.cfg.preset("Desk").unwrap().sound, before);
    app.resolve_preset_overwrite(false);
    assert_eq!(app.sound, before);
    app.request_preset_save(
        imported.clone(),
        PresetOrigin::Import("fixture.json".into()),
    );
    app.resolve_preset_overwrite(true);
    assert_eq!(app.sound, imported.sound);
    assert!(!app.selected_preset_modified());
}

#[test]
fn q_editor_accepts_incremental_keyboard_decimal_entry() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    app.sound.parametric_enabled = true;
    let size = egui::vec2(1600.0, 900.0);
    for step in 0..3 {
        draw(&ctx, &mut app, size, step as f64 * 0.2, vec![]);
    }
    let rect = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("qa_q_editor")))
        .unwrap();
    let pos = rect.center();
    draw(
        &ctx,
        &mut app,
        size,
        0.6,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    draw(
        &ctx,
        &mut app,
        size,
        0.8,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    for (step, text) in ["3", ".", "0"].into_iter().enumerate() {
        draw(
            &ctx,
            &mut app,
            size,
            1.0 + step as f64 * 0.2,
            vec![egui::Event::Text(text.into())],
        );
    }
    draw(
        &ctx,
        &mut app,
        size,
        1.6,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(app.sound.band_settings[5].q, 3.0);
}

#[test]
fn a_new_audio_route_discards_old_meter_spectrum_and_calibration_samples() {
    let ctx = egui::Context::default();
    let mut app = app(&ctx);
    let sound = app.sound.clone();
    app.cfg.level_match_ab = true;
    app.telemetry.spectrum = vec![0.1; 4096];
    app.telemetry.analysis_input = vec![[0.1, 0.1]; 8192];
    app.telemetry.processed_frames = 50_000;
    app.meter_input = 0.5;
    app.meter_output = 0.3;
    app.spectrum_levels = [-20.0; SPECTRUM_BANDS];
    app.spectrum_target = app.spectrum_levels;
    app.spectrum_frame = 50_000;
    app.match_calibration = Some(LevelMatch {
        trims_db: [-6.0, 0.0],
        limited: false,
    });
    app.match_key = Some(app.current_ab_slots());
    app.clear_route_analysis();
    assert_eq!(app.telemetry, TelemetrySnapshot::default());
    assert_eq!((app.meter_input, app.meter_output), (0.0, 0.0));
    assert_eq!(app.spectrum_levels, [SPECTRUM_FLOOR_DB; SPECTRUM_BANDS]);
    assert_eq!(app.spectrum_frame, 0);
    assert!(app.match_calibration.is_none() && app.match_key.is_none());
    assert!(app.cfg.level_match_ab);
    assert_eq!(app.sound, sound);
}
