use rometric::analysis::level_match;
use rometric::config::Config;
use rometric::dsp::{BandKind, EqChain, Params, BAND_FREQ, N_BANDS};
use rometric::product::{Preset, PresetFile, SoundSettings, PRESET_SCHEMA};

fn linear() -> Params {
    Params {
        limiter: false,
        ..Params::default()
    }
}

fn tone(frequency: f32, frames: usize) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|n| {
            let x = (std::f32::consts::TAU * frequency * n as f32 / 48_000.0).sin() * 0.025;
            [x, x * 0.7]
        })
        .collect()
}

fn output_rms(params: &Params, frames: &[[f32; 2]]) -> f32 {
    let mut chain = EqChain::new(48_000.0, params);
    let mut power = 0.0;
    let skip = frames.len() / 2;
    for (index, frame) in frames.iter().enumerate() {
        let (l, r) = chain.process(frame[0], frame[1]);
        if index >= skip {
            power += 0.5 * (l * l + r * r);
        }
    }
    (power / (frames.len() - skip) as f32).sqrt()
}

#[test]
fn advanced_effects_are_off_for_existing_sound_and_config() {
    let sound: SoundSettings = serde_json::from_str(r#"{"preamp_db":-4.0}"#).unwrap();
    assert!(!sound.parametric_enabled);
    assert_eq!(sound.crossfeed, 0.0);
    assert_eq!(sound.band_settings.map(|band| band.frequency_hz), BAND_FREQ);
    assert_eq!(sound.to_params().comparison_gain_db, 0.0);

    let config = Config::from_json(r#"{"preamp_db":-4.0}"#).unwrap();
    assert!(!config.level_match_ab);
    assert_eq!(config.sound_settings(), sound);
}

#[test]
fn parametric_defaults_preserve_the_original_graphic_response() {
    let mut graphic = linear();
    graphic.gains = [3.0, -2.0, 1.0, 0.5, -4.0, 6.0, -3.0, 2.0, -1.0, 4.0, 2.0];
    let mut parametric = graphic;
    parametric.parametric_enabled = true;
    let graphic_chain = EqChain::new(48_000.0, &graphic);
    let parametric_chain = EqChain::new(48_000.0, &parametric);
    for frequency in [25.0, 63.0, 125.0, 500.0, 1000.0, 4000.0, 12000.0, 19000.0] {
        assert!(
            (graphic_chain.response_db(frequency) - parametric_chain.response_db(frequency)).abs()
                < 0.002,
            "defaults changed the response at {frequency} Hz"
        );
    }
}

#[test]
fn parametric_peak_uses_its_selected_frequency_and_gain() {
    let mut params = linear();
    params.parametric_enabled = true;
    params.band_settings[5].frequency_hz = 1500.0;
    params.band_settings[5].q = 3.0;
    params.gains[5] = 6.0;
    let chain = EqChain::new(48_000.0, &params);
    assert!((chain.response_db(1500.0) - 6.0).abs() < 0.03);
    assert!(chain.response_db(500.0).abs() < 1.0);
}

#[test]
fn parametric_q_changes_bandwidth_without_changing_centre_gain() {
    let mut wide = linear();
    wide.parametric_enabled = true;
    wide.gains[5] = 8.0;
    wide.band_settings[5].q = 0.5;
    let mut narrow = wide;
    narrow.band_settings[5].q = 6.0;
    let wide = EqChain::new(48_000.0, &wide);
    let narrow = EqChain::new(48_000.0, &narrow);
    assert!((wide.response_db(1000.0) - narrow.response_db(1000.0)).abs() < 0.03);
    assert!(wide.response_db(700.0) > narrow.response_db(700.0) + 3.0);
}

#[test]
fn parametric_filter_types_and_band_bypass_are_real_controls() {
    let mut params = linear();
    params.parametric_enabled = true;
    params.gains[5] = 6.0;
    params.band_settings[5].kind = BandKind::LowShelf;
    params.band_settings[5].q = std::f32::consts::FRAC_1_SQRT_2;
    let low = EqChain::new(48_000.0, &params);
    assert!(low.response_db(40.0) > 5.8);
    assert!(low.response_db(15000.0).abs() < 0.1);

    params.band_settings[5].kind = BandKind::HighShelf;
    let high = EqChain::new(48_000.0, &params);
    assert!(high.response_db(15000.0) > 5.8);
    assert!(high.response_db(40.0).abs() < 0.1);

    params.band_settings[5].enabled = false;
    let bypassed = EqChain::new(48_000.0, &params);
    assert!(bypassed.response_db(1000.0).abs() < 0.01);
}

#[test]
fn graphic_mode_ignores_optional_band_settings() {
    let mut params = linear();
    params.gains[5] = 6.0;
    params.band_settings[5].frequency_hz = 9000.0;
    params.band_settings[5].enabled = false;
    params.band_settings[5].kind = BandKind::HighShelf;
    assert!((EqChain::new(48_000.0, &params).response_db(1000.0) - 6.0).abs() < 0.2);
}

#[test]
fn advanced_settings_are_sanitized_before_persistence_and_dsp() {
    let mut sound = SoundSettings {
        parametric_enabled: true,
        crossfeed: f32::INFINITY,
        ..Default::default()
    };
    sound.band_settings[0].frequency_hz = f32::NAN;
    sound.band_settings[1].frequency_hz = 999_999.0;
    sound.band_settings[2].q = -3.0;
    sound.band_settings[3].q = f32::INFINITY;
    assert!(sound.sanitize().changed);
    assert_eq!(sound.crossfeed, 0.0);
    assert_eq!(sound.band_settings[0].frequency_hz, BAND_FREQ[0]);
    assert_eq!(sound.band_settings[1].frequency_hz, 20_000.0);
    assert_eq!(sound.band_settings[2].q, 0.1);
    assert!(sound.band_settings.iter().all(|band| band.q.is_finite()));
}

#[test]
fn parametric_filters_remain_finite_across_sample_rates_and_extremes() {
    for rate in [8_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        for kind in [BandKind::Peak, BandKind::LowShelf, BandKind::HighShelf] {
            let mut params = Params {
                parametric_enabled: true,
                crossfeed: 1.0,
                ..Default::default()
            };
            for index in 0..N_BANDS {
                params.gains[index] = if index % 2 == 0 { 12.0 } else { -12.0 };
                params.band_settings[index].frequency_hz =
                    if index % 2 == 0 { 20.0 } else { 20000.0 };
                params.band_settings[index].q = if index % 2 == 0 { 0.1 } else { 12.0 };
                params.band_settings[index].kind = kind;
            }
            let mut chain = EqChain::new(rate, &params);
            for n in 0..12_000 {
                let x = (n as f32 * 0.027).sin() * 0.5;
                let (l, r) = chain.process(x, -x * 0.8);
                assert!(l.is_finite() && r.is_finite(), "{rate} Hz, {kind:?}");
                assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
            }
        }
    }
}

#[test]
fn disabled_advanced_effects_preserve_original_output() {
    let params = linear();
    let mut chain = EqChain::new(48_000.0, &params);
    for n in 0..4096 {
        let left = (n as f32 * 0.071).sin() * 0.1;
        let right = (n as f32 * 0.053).sin() * 0.1;
        assert_eq!(chain.process(left, right), (left, right));
    }
}

#[test]
fn crossfeed_preserves_mono_and_routes_low_frequencies_to_the_other_ear() {
    let mut params = linear();
    params.crossfeed = 1.0;
    let mut mono = EqChain::new(48_000.0, &params);
    for n in 0..4096 {
        let x = (n as f32 * 0.03).sin() * 0.1;
        let (l, r) = mono.process(x, x);
        assert!((l - x).abs() < 1e-6 && (r - x).abs() < 1e-6);
    }

    let mut left_only = EqChain::new(48_000.0, &params);
    let mut sum = (0.0, 0.0);
    for _ in 0..4096 {
        sum = left_only.process(0.1, 0.0);
    }
    assert!(sum.1 > 0.02 && sum.1 < 0.05);
    assert!((sum.0 + sum.1 - 0.1).abs() < 1e-5);
}

#[test]
fn crossfeed_is_frequency_dependent_not_just_a_width_control() {
    let mut params = linear();
    params.crossfeed = 1.0;
    let opposite_rms = |frequency: f32| {
        let mut chain = EqChain::new(48_000.0, &params);
        let mut power = 0.0;
        for n in 0..12_000 {
            let x = (std::f32::consts::TAU * frequency * n as f32 / 48_000.0).sin() * 0.1;
            let (_, r) = chain.process(x, 0.0);
            if n >= 6000 {
                power += r * r;
            }
        }
        (power / 6000.0).sqrt()
    };
    assert!(opposite_rms(100.0) > opposite_rms(10000.0) * 5.0);
}

#[test]
fn correction_bypass_bypasses_every_new_audio_effect() {
    let params = Params {
        enabled: false,
        parametric_enabled: true,
        crossfeed: 1.0,
        comparison_gain_db: -12.0,
        ..Params::default()
    };
    let mut chain = EqChain::new(48_000.0, &params);
    assert_eq!(chain.process(0.3, -0.6), (0.3, -0.6));
}

#[test]
fn comparison_trim_attenuates_and_cannot_boost_audio() {
    let frames = tone(1000.0, 20_000);
    let base = linear();
    let attenuated = Params {
        comparison_gain_db: -6.0,
        ..base
    };
    let attempted_boost = Params {
        comparison_gain_db: 6.0,
        ..base
    };
    assert!(
        (output_rms(&attenuated, &frames) / output_rms(&base, &frames) - 0.501187).abs() < 0.002
    );
    assert!((output_rms(&attempted_boost, &frames) - output_rms(&base, &frames)).abs() < 1e-6);
}

#[test]
fn live_advanced_updates_preserve_limiter_envelope_and_smooth_comparison_trim() {
    let mut params = Params {
        preamp_db: 18.0,
        ..Params::default()
    };
    let mut chain = EqChain::new(48_000.0, &params);
    for _ in 0..2000 {
        chain.process(1.0, 1.0);
    }
    let envelope = chain.limiter_gain();
    params.parametric_enabled = true;
    params.band_settings[5].q = 4.0;
    params.crossfeed = 0.5;
    params.comparison_gain_db = -12.0;
    chain.update_params(&params);
    assert!((chain.limiter_gain() - envelope).abs() < 1e-6);

    let mut smooth = EqChain::new(48_000.0, &linear());
    let before = smooth.process(0.1, 0.1).0;
    smooth.update_params(&Params {
        comparison_gain_db: -12.0,
        ..linear()
    });
    let first = smooth.process(0.1, 0.1).0;
    assert!((first - before).abs() < 0.001, "trim stepped abruptly");
    for _ in 0..12_000 {
        smooth.process(0.1, 0.1);
    }
    assert!((smooth.process(0.1, 0.1).0 - 0.025119).abs() < 0.001);
}

#[test]
fn level_match_uses_the_same_signal_for_both_slots_and_only_attenuates() {
    let frames = tone(1000.0, 8192);
    let mut boosted = linear();
    boosted.gains[5] = 6.0;
    let mut slots = [linear(), boosted];
    let calibration = level_match(48_000.0, &frames, &slots).expect("non-silent signal");
    assert_eq!(calibration.trims_db[0], 0.0);
    assert!((calibration.trims_db[1] + 6.0).abs() < 0.15);
    assert!(!calibration.limited);
    for (slot, trim) in slots.iter_mut().zip(calibration.trims_db) {
        slot.comparison_gain_db = trim;
    }
    let long = tone(1000.0, 24_000);
    assert!(
        (20.0 * (output_rms(&slots[0], &long) / output_rms(&slots[1], &long)).log10()).abs() < 0.2
    );
}

#[test]
fn level_match_waits_for_real_non_silent_samples() {
    let slots = [linear(), linear()];
    assert!(level_match(48_000.0, &[], &slots).is_none());
    assert!(level_match(48_000.0, &vec![[0.0, 0.0]; 8192], &slots).is_none());
    assert!(level_match(48_000.0, &vec![[f32::NAN, f32::INFINITY]; 8192], &slots).is_none());
    assert!(level_match(f32::NAN, &tone(1000.0, 8192), &slots).is_none());
}

#[test]
fn level_match_does_not_claim_to_match_a_slot_that_bypasses_all_processing() {
    let slots = [
        linear(),
        Params {
            enabled: false,
            preamp_db: 6.0,
            ..linear()
        },
    ];
    assert!(level_match(48_000.0, &tone(1000.0, 8192), &slots).is_none());
}

#[test]
fn level_match_never_mutates_presets_or_uses_a_saved_comparison_trim() {
    let slots = [
        Params {
            comparison_gain_db: -15.0,
            ..linear()
        },
        linear(),
    ];
    let before = slots;
    let calibration = level_match(48_000.0, &tone(1000.0, 8192), &slots).unwrap();
    assert_eq!(slots, before);
    assert!(calibration.trims_db.iter().all(|db| db.abs() < 0.01));
}

#[test]
fn level_match_reports_when_the_safety_attenuation_limit_is_reached() {
    let slots = [
        linear(),
        Params {
            preamp_db: -50.0,
            ..linear()
        },
    ];
    let calibration = level_match(48_000.0, &tone(1000.0, 8192), &slots).unwrap();
    assert!(calibration.limited);
    assert_eq!(calibration.trims_db[0], -24.0);
    assert_eq!(calibration.trims_db[1], 0.0);
}

#[test]
fn advanced_sound_round_trips_through_config_and_versioned_presets() {
    let mut sound = SoundSettings {
        parametric_enabled: true,
        crossfeed: 0.45,
        ..Default::default()
    };
    sound.band_settings[2].frequency_hz = 350.0;
    sound.band_settings[2].q = 2.5;
    sound.band_settings[2].kind = BandKind::LowShelf;
    sound.band_settings[3].enabled = false;
    sound.gains[2] = -3.5;
    let mut config = Config::default();
    config.apply_sound_settings(&sound);
    config.level_match_ab = true;
    let restored = Config::from_json(&serde_json::to_string(&config).unwrap()).unwrap();
    assert_eq!(restored.sound_settings(), sound);
    assert!(restored.level_match_ab);

    let file = PresetFile::new(Preset::new("Parametric desk", sound.clone()));
    assert_eq!(
        PRESET_SCHEMA, 2,
        "new effects must not silently disappear in older apps"
    );
    let json = file.to_json().unwrap();
    assert!(!json.contains("comparison_gain_db"));
    assert_eq!(PresetFile::from_json(&json).unwrap().preset.sound, sound);
}

#[test]
fn original_version_one_presets_still_import_with_effects_disabled() {
    let old =
        r#"{"schema":1,"preset":{"name":"Original","profile":null,"sound":{"preamp_db":-3.0}}}"#;
    let file = PresetFile::from_json(old).unwrap();
    assert_eq!(file.preset.sound.preamp_db, -3.0);
    assert!(!file.preset.sound.parametric_enabled);
    assert_eq!(file.preset.sound.crossfeed, 0.0);
    assert_eq!(
        file.preset.sound.band_settings.map(|b| b.frequency_hz),
        BAND_FREQ
    );
    assert!(PresetFile::from_json(&old.replace("\"schema\":1", "\"schema\":0")).is_err());
    assert!(PresetFile::from_json(&old.replace("\"schema\":1", "\"schema\":99")).is_err());
}
