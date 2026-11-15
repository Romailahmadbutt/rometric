use rometric::analysis::{peak_dbfs, spectrum_levels, SPECTRUM_BANDS, SPECTRUM_FLOOR_DB};
use rometric::config::Config;
use rometric::product::{Preset, PresetSaveResult, SoundSettings};
use rometric::telemetry::Telemetry;

#[test]
fn peak_readouts_use_dbfs_and_show_silence_without_nan() {
    assert_eq!(peak_dbfs(0.0), f32::NEG_INFINITY);
    assert_eq!(peak_dbfs(f32::NAN), f32::NEG_INFINITY);
    assert_eq!(peak_dbfs(f32::INFINITY), f32::NEG_INFINITY);
    assert_eq!(peak_dbfs(-1.0), f32::NEG_INFINITY);
    assert_eq!(peak_dbfs(1.0), 0.0);
    assert!((peak_dbfs(0.5) + 6.0206).abs() < 0.001);
    assert!(
        (peak_dbfs(2.0) - 6.0206).abs() < 0.001,
        "overload must not be clamped away"
    );
}

#[test]
fn unsaved_preset_indication_tracks_sound_and_profile_not_just_the_name() {
    let sound = SoundSettings::default();
    let mut preset = Preset::new("Desk", sound.clone());
    preset.profile = Some("HD 600".into());
    assert!(!preset.is_modified(&sound, Some("HD 600")));
    assert!(preset.is_modified(&sound, None));
    let mut modified = sound.clone();
    modified.gains[3] = 0.5;
    assert!(preset.is_modified(&modified, Some("HD 600")));
    modified = sound.clone();
    modified.crossfeed = 0.25;
    assert!(preset.is_modified(&modified, Some("HD 600")));
    modified = sound.clone();
    modified.parametric_enabled = true;
    assert!(preset.is_modified(&modified, Some("HD 600")));
    modified = sound.clone();
    modified.band_settings[5].q = 4.0;
    assert!(preset.is_modified(&modified, Some("HD 600")));
    modified = sound.clone();
    modified.auto_preamp = false;
    assert!(preset.is_modified(&modified, Some("HD 600")));
}

#[test]
fn saving_a_new_preset_does_not_require_confirmation() {
    let mut config = Config::default();
    assert_eq!(
        config
            .save_preset(Preset::new("New", SoundSettings::default()), false)
            .unwrap(),
        PresetSaveResult::Saved
    );
    assert_eq!(config.presets.len(), 1);
}

#[test]
fn preset_overwrite_is_not_applied_until_explicitly_confirmed() {
    let mut config = Config::default();
    let original = Preset::new("Desk", SoundSettings::default());
    config.upsert_preset(original.clone());
    config.selected_preset = Some("Desk".into());
    config.device_presets.assign("USB DAC", "Desk");
    let mut replacement = original.clone();
    replacement.sound.preamp_db = -6.0;
    let request = config.save_preset(replacement.clone(), false).unwrap();
    assert_eq!(
        request,
        PresetSaveResult::NeedsConfirmation(Box::new(replacement.clone()))
    );
    assert_eq!(config.preset("Desk"), Some(&original));
    assert_eq!(config.selected_preset.as_deref(), Some("Desk"));
    assert_eq!(config.device_presets.preset_for("USB DAC"), Some("Desk"));
    assert_eq!(
        config.save_preset(replacement.clone(), true).unwrap(),
        PresetSaveResult::Saved
    );
    assert_eq!(config.presets.len(), 1);
    assert_eq!(config.preset("Desk"), Some(&replacement));
}

#[test]
fn cancellation_and_invalid_preset_requests_leave_saved_data_unchanged() {
    let mut config = Config::default();
    config.upsert_preset(Preset::new("Desk", SoundSettings::default()));
    let before = serde_json::to_string(&config).unwrap();
    let _cancelled_request = config
        .save_preset(
            Preset::new(
                "Desk",
                SoundSettings {
                    preamp_db: -8.0,
                    ..Default::default()
                },
            ),
            false,
        )
        .unwrap();
    assert_eq!(serde_json::to_string(&config).unwrap(), before);
    assert!(config
        .save_preset(Preset::new("   ", SoundSettings::default()), true)
        .is_err());
    assert_eq!(serde_json::to_string(&config).unwrap(), before);
}

#[test]
fn overwrite_detection_uses_the_validated_trimmed_preset_name() {
    let mut config = Config::default();
    config.upsert_preset(Preset::new("Desk", SoundSettings::default()));
    assert!(matches!(
        config
            .save_preset(Preset::new("  Desk  ", SoundSettings::default()), false)
            .unwrap(),
        PresetSaveResult::NeedsConfirmation(_)
    ));
    assert_eq!(config.presets.len(), 1);
}

#[test]
fn spectrum_is_empty_at_silence_and_invalid_input() {
    for samples in [Vec::new(), vec![0.0; 2048], vec![f32::NAN; 2048]] {
        assert_eq!(
            spectrum_levels(&samples, 48_000.0),
            [SPECTRUM_FLOOR_DB; SPECTRUM_BANDS]
        );
    }
    assert_eq!(
        spectrum_levels(&vec![0.5; 2048], f32::NAN),
        [SPECTRUM_FLOOR_DB; SPECTRUM_BANDS]
    );
}

#[test]
fn spectrum_follows_signal_frequency_and_amplitude() {
    let tone = |frequency: f32, amplitude: f32| {
        (0..2048)
            .map(|n| (std::f32::consts::TAU * frequency * n as f32 / 48_000.0).sin() * amplitude)
            .collect::<Vec<_>>()
    };
    let loud = spectrum_levels(&tone(1000.0, 0.5), 48_000.0);
    let quiet = spectrum_levels(&tone(1000.0, 0.25), 48_000.0);
    let high = spectrum_levels(&tone(8000.0, 0.5), 48_000.0);
    let peak_index = |levels: &[f32; SPECTRUM_BANDS]| {
        levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0
    };
    let peak = peak_index(&loud);
    let frequency = 20.0 * 1000f32.powf(peak as f32 / (SPECTRUM_BANDS - 1) as f32);
    assert!(
        frequency > 800.0 && frequency < 1200.0,
        "peak at {frequency}"
    );
    assert!((loud[peak] - quiet[peak] - 6.0206).abs() < 0.1);
    assert!(peak_index(&high) > peak + 10);
    assert!(loud.iter().all(|level| level.is_finite()));
}

#[test]
fn telemetry_keeps_real_stereo_calibration_samples_in_chronological_order() {
    let telemetry = Telemetry::new(8);
    for index in 0..12 {
        telemetry.push_analysis_input([index as f32, -(index as f32)]);
    }
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.analysis_input.len(), 8);
    assert_eq!(snapshot.analysis_input[0], [4.0, -4.0]);
    assert_eq!(snapshot.analysis_input[7], [11.0, -11.0]);
}

#[test]
fn telemetry_sanitizes_invalid_calibration_samples_without_changing_other_counters() {
    let telemetry = Telemetry::new(8);
    telemetry.observe_output(0.5, 0.25, 1.0);
    telemetry.push_analysis_input([f32::NAN, f32::INFINITY]);
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.analysis_input, vec![[0.0, 0.0]]);
    assert_eq!(snapshot.processed_frames, 1);
    assert_eq!(snapshot.output_peak, 0.5);
}
