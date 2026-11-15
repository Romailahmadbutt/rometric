//! Settings persistence in the user's config directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::catalog::Grouping;
use crate::dsp::{default_band_settings, BandSettings, Params, N_BANDS};
use crate::product::{DevicePresetMap, Preset, PresetSaveResult, ProfileLibrary, SoundSettings};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub profile: Option<String>,
    pub gains: [f32; N_BANDS],
    pub preamp_db: f32,
    pub bass_db: f32,
    pub width: f32,
    pub limiter: bool,
    pub enabled: bool,
    pub auto_preamp: bool,
    pub capture_device: Option<String>,
    pub render_device: Option<String>,
    pub profile_library: ProfileLibrary,
    pub presets: Vec<Preset>,
    pub selected_preset: Option<String>,
    pub device_presets: DevicePresetMap,
    pub auto_reconnect: bool,
    pub spectrum_enabled: bool,
    pub reduced_motion: bool,
    pub minimize_to_tray: bool,
    pub auto_start_audio: bool,
    pub profile_grouping: Grouping,
    pub parametric_enabled: bool,
    pub band_settings: [BandSettings; N_BANDS],
    pub crossfeed: f32,
    pub level_match_ab: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profile: None,
            gains: [0.0; N_BANDS],
            preamp_db: 0.0,
            bass_db: 0.0,
            width: 1.0,
            limiter: true,
            enabled: true,
            auto_preamp: true,
            capture_device: None,
            render_device: None,
            profile_library: ProfileLibrary::default(),
            presets: Vec::new(),
            selected_preset: None,
            device_presets: DevicePresetMap::default(),
            auto_reconnect: true,
            spectrum_enabled: false,
            reduced_motion: false,
            minimize_to_tray: true,
            auto_start_audio: true,
            profile_grouping: Grouping::All,
            parametric_enabled: false,
            band_settings: default_band_settings(),
            crossfeed: 0.0,
            level_match_ab: false,
        }
    }
}

impl Config {
    pub fn to_params(&self) -> Params {
        Params {
            gains: self.gains,
            preamp_db: self.preamp_db,
            bass_db: self.bass_db,
            width: self.width,
            limiter: self.limiter,
            enabled: self.enabled,
            parametric_enabled: self.parametric_enabled,
            band_settings: self.band_settings,
            crossfeed: self.crossfeed,
            comparison_gain_db: 0.0,
        }
    }

    pub fn sound_settings(&self) -> SoundSettings {
        SoundSettings::from_params(&self.to_params(), self.auto_preamp)
    }

    pub fn apply_sound_settings(&mut self, sound: &SoundSettings) {
        self.gains = sound.gains;
        self.preamp_db = sound.preamp_db;
        self.bass_db = sound.bass_db;
        self.width = sound.width;
        self.limiter = sound.limiter;
        self.enabled = sound.correction_enabled;
        self.auto_preamp = sound.auto_preamp;
        self.parametric_enabled = sound.parametric_enabled;
        self.band_settings = sound.band_settings;
        self.crossfeed = sound.crossfeed;
    }

    pub fn preset(&self, name: &str) -> Option<&Preset> {
        self.presets.iter().find(|preset| preset.name == name)
    }

    pub fn upsert_preset(&mut self, mut preset: Preset) {
        if preset.validate().is_err() {
            return;
        }
        if let Some(existing) = self
            .presets
            .iter_mut()
            .find(|item| item.name == preset.name)
        {
            *existing = preset;
        } else {
            self.presets.push(preset);
            self.presets.sort_by_key(|item| item.name.to_lowercase());
        }
    }

    /// Both Save and Import use this guard. Declining a collision leaves all
    /// saved data, output mappings and selection exactly as they were.
    pub fn save_preset(
        &mut self,
        mut preset: Preset,
        overwrite: bool,
    ) -> Result<PresetSaveResult, String> {
        preset.validate()?;
        if self.preset(&preset.name).is_some() && !overwrite {
            return Ok(PresetSaveResult::NeedsConfirmation(Box::new(preset)));
        }
        self.upsert_preset(preset);
        Ok(PresetSaveResult::Saved)
    }

    pub fn remove_preset(&mut self, name: &str) -> bool {
        let before = self.presets.len();
        self.presets.retain(|preset| preset.name != name);
        if self.selected_preset.as_deref() == Some(name) {
            self.selected_preset = None;
        }
        let names: Vec<_> = self
            .presets
            .iter()
            .map(|preset| preset.name.as_str())
            .collect();
        self.device_presets.retain_presets(names.into_iter());
        self.presets.len() != before
    }

    pub fn dir() -> PathBuf {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            PathBuf::from(appdata).join("Rometric")
        } else if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(".config").join("rometric")
        } else {
            PathBuf::from(".")
        }
    }

    fn legacy_path() -> PathBuf {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            PathBuf::from(appdata)
                .join("wavelet-win")
                .join("config.json")
        } else if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home)
                .join(".config")
                .join("wavelet-win")
                .join("config.json")
        } else {
            PathBuf::from("config.json")
        }
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.json")
    }

    pub fn load() -> Self {
        let text = std::fs::read_to_string(Self::path())
            .or_else(|_| std::fs::read_to_string(Self::legacy_path()));
        match text {
            Ok(text) => Self::from_json(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut config: Self = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|err| format!("Invalid settings file: {err}"))?;
        let mut sound = config.sound_settings();
        sound.sanitize();
        config.apply_sound_settings(&sound);
        for preset in &mut config.presets {
            preset.sound.sanitize();
        }
        Ok(config)
    }

    pub fn save(&self) {
        let _ = self.save_result();
    }

    pub fn save_result(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("Could not create settings directory: {err}"))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("Could not serialize settings: {err}"))?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .map_err(|err| format!("Could not write settings: {err}"))?;
        if path.exists() {
            let backup = path.with_extension("json.bak");
            let _ = std::fs::copy(&path, backup);
            std::fs::remove_file(&path)
                .map_err(|err| format!("Could not replace settings: {err}"))?;
        }
        std::fs::rename(&temporary, &path)
            .map_err(|err| format!("Could not finish saving settings: {err}"))
    }
}
