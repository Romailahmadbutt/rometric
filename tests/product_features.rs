use rometric::config::Config;
use rometric::dsp::{max_response_db, EqChain, Params};
use rometric::product::{
    DevicePresetMap, EditHistory, Preset, PresetFile, ProfileLibrary, ReconnectBackoff,
    SoundSettings, PRESET_SCHEMA,
};
use rometric::telemetry::Telemetry;

fn sound(preamp_db: f32) -> SoundSettings {
    SoundSettings {
        preamp_db,
        ..SoundSettings::default()
    }
}

#[test]
fn imported_sound_settings_are_sanitized() {
    let mut settings = sound(f32::NAN);
    settings.gains[0] = 99.0;
    settings.bass_db = -12.0;
    settings.width = 9.0;

    let report = settings.sanitize();

    assert!(report.changed);
    assert_eq!(settings.preamp_db, 0.0);
    assert_eq!(settings.gains[0], 12.0);
    assert_eq!(settings.bass_db, 0.0);
    assert_eq!(settings.width, 2.0);
}

#[test]
fn preset_files_round_trip_and_reject_unknown_schema() {
    let file = PresetFile::new(Preset::new("Desk headphones", sound(-5.5)));
    let json = file.to_json().expect("preset serializes");
    let restored = PresetFile::from_json(&json).expect("preset parses");
    assert_eq!(restored.preset.name, "Desk headphones");
    assert_eq!(restored.preset.sound.preamp_db, -5.5);

    let incompatible = json.replace(&format!("\"schema\": {PRESET_SCHEMA}"), "\"schema\": 99");
    assert!(PresetFile::from_json(&incompatible).is_err());
}

#[test]
fn profile_library_tracks_favorites_and_bounded_recents() {
    let mut library = ProfileLibrary::default();
    assert!(library.toggle_favorite("HD 600"));
    assert!(!library.toggle_favorite("HD 600"));

    for index in 0..12 {
        library.mark_recent(format!("Profile {index}"));
    }
    library.mark_recent("Profile 6");

    assert_eq!(library.recents.len(), 8);
    assert_eq!(library.recents[0], "Profile 6");
    assert_eq!(
        library.recents.iter().filter(|p| *p == "Profile 6").count(),
        1
    );
}

#[test]
fn edit_history_supports_undo_redo_and_branching() {
    let initial = sound(0.0);
    let second = sound(-3.0);
    let third = sound(-6.0);
    let branch = sound(-2.0);
    let mut history = EditHistory::new(3);

    history.record(initial.clone());
    assert_eq!(history.undo(&second), Some(initial.clone()));
    assert_eq!(history.redo(&initial), Some(second.clone()));

    history.record(second);
    history.record(third);
    history.record(branch.clone());
    assert!(history.redo(&branch).is_none());
    assert!(history.undo_len() <= 3);
}

#[test]
fn output_devices_can_restore_named_presets() {
    let mut mappings = DevicePresetMap::default();
    mappings.assign("USB DAC", "Studio");
    assert_eq!(mappings.preset_for("USB DAC"), Some("Studio"));
    mappings.remove("USB DAC");
    assert_eq!(mappings.preset_for("USB DAC"), None);
}

#[test]
fn response_headroom_detects_boost_and_safe_preamp() {
    let mut boosted = Params::default();
    boosted.gains[5] = 8.0;
    let peak = max_response_db(48_000.0, &boosted);
    assert!(peak > 7.5);

    boosted.preamp_db = -peak - 0.5;
    assert!(max_response_db(48_000.0, &boosted) <= -0.45);
}

#[test]
fn limiter_exposes_gain_reduction_for_metering() {
    let params = Params {
        preamp_db: 18.0,
        ..Default::default()
    };
    let mut chain = EqChain::new(48_000.0, &params);
    for _ in 0..2_000 {
        chain.process(1.0, 1.0);
    }
    assert!(chain.limiter_reduction_db() > 6.0);
    assert!(chain.limiter_gain() < 0.5);
}

#[test]
fn live_parameter_updates_preserve_limiter_state() {
    let mut params = Params {
        preamp_db: 18.0,
        ..Default::default()
    };
    let mut chain = EqChain::new(48_000.0, &params);
    for _ in 0..2_000 {
        chain.process(1.0, 1.0);
    }
    let gain_before = chain.limiter_gain();
    params.gains[5] = 4.0;
    chain.update_params(&params);

    assert!((chain.response_db(1_000.0) - 22.0).abs() < 0.3);
    assert!((chain.limiter_gain() - gain_before).abs() < 1e-6);
}

#[test]
fn telemetry_reports_peaks_counters_and_latest_spectrum_samples() {
    let telemetry = Telemetry::new(8);
    telemetry.observe_input(0.25, -0.5);
    telemetry.observe_output(0.75, -0.125, 0.8);
    telemetry.record_underrun();
    telemetry.record_overrun();
    for sample in 0..12 {
        telemetry.push_spectrum(sample as f32 / 12.0);
    }

    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.input_peak, 0.5);
    assert_eq!(snapshot.output_peak, 0.75);
    assert!(snapshot.limiter_reduction_db > 1.9);
    assert_eq!(snapshot.underruns, 1);
    assert_eq!(snapshot.overruns, 1);
    assert_eq!(snapshot.spectrum.len(), 8);
    assert!((snapshot.spectrum[0] - (4.0 / 12.0)).abs() < 1e-6);
}

#[test]
fn reconnect_backoff_is_bounded_and_resettable() {
    let mut retry = ReconnectBackoff::default();
    assert_eq!(retry.next_delay().as_secs(), 1);
    assert_eq!(retry.next_delay().as_secs(), 2);
    assert_eq!(retry.next_delay().as_secs(), 4);
    assert_eq!(retry.next_delay().as_secs(), 8);
    assert_eq!(retry.next_delay().as_secs(), 10);
    assert_eq!(retry.next_delay().as_secs(), 10);
    retry.reset();
    assert_eq!(retry.next_delay().as_secs(), 1);
}

#[test]
fn legacy_config_migrates_to_safe_product_defaults() {
    let config: Config =
        serde_json::from_str(r#"{"profile":"HD 600","preamp_db":-4.0,"enabled":false}"#)
            .expect("legacy config parses");

    assert_eq!(config.profile.as_deref(), Some("HD 600"));
    assert_eq!(config.preamp_db, -4.0);
    assert!(!config.enabled);
    assert!(config.auto_reconnect);
    assert!(!config.spectrum_enabled);
    assert!(config.presets.is_empty());
    assert!(config.profile_library.favorites.is_empty());
}

#[test]
fn config_parser_accepts_windows_utf8_bom() {
    let config = Config::from_json("\u{feff}{\"auto_start_audio\":false}")
        .expect("BOM-prefixed settings parse");
    assert!(!config.auto_start_audio);
}

#[test]
fn config_upserts_and_removes_presets_without_stale_device_mappings() {
    let mut config = Config::default();
    config.upsert_preset(Preset::new("Studio", sound(-3.0)));
    config.upsert_preset(Preset::new("Studio", sound(-7.0)));
    config.device_presets.assign("USB DAC", "Studio");

    assert_eq!(config.presets.len(), 1);
    assert_eq!(config.preset("Studio").unwrap().sound.preamp_db, -7.0);
    assert!(config.remove_preset("Studio"));
    assert_eq!(config.device_presets.preset_for("USB DAC"), None);
}
