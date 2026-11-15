//! Product-level state shared by the UI, preset files and tests.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::dsp::{default_band_settings, BandSettings, Params, N_BANDS};

pub const PRESET_SCHEMA: u32 = 2;
const MAX_RECENTS: usize = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    pub gains: [f32; N_BANDS],
    pub preamp_db: f32,
    pub bass_db: f32,
    pub width: f32,
    pub limiter: bool,
    pub correction_enabled: bool,
    pub auto_preamp: bool,
    pub parametric_enabled: bool,
    pub band_settings: [BandSettings; N_BANDS],
    pub crossfeed: f32,
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            gains: [0.0; N_BANDS],
            preamp_db: 0.0,
            bass_db: 0.0,
            width: 1.0,
            limiter: true,
            correction_enabled: true,
            auto_preamp: true,
            parametric_enabled: false,
            band_settings: default_band_settings(),
            crossfeed: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SanitizeReport {
    pub changed: bool,
}

impl SoundSettings {
    pub fn from_params(params: &Params, auto_preamp: bool) -> Self {
        Self {
            gains: params.gains,
            preamp_db: params.preamp_db,
            bass_db: params.bass_db,
            width: params.width,
            limiter: params.limiter,
            correction_enabled: params.enabled,
            auto_preamp,
            parametric_enabled: params.parametric_enabled,
            band_settings: params.band_settings,
            crossfeed: params.crossfeed,
        }
    }

    pub fn to_params(&self) -> Params {
        Params {
            gains: self.gains,
            preamp_db: self.preamp_db,
            bass_db: self.bass_db,
            width: self.width,
            limiter: self.limiter,
            enabled: self.correction_enabled,
            parametric_enabled: self.parametric_enabled,
            band_settings: self.band_settings,
            crossfeed: self.crossfeed,
            comparison_gain_db: 0.0,
        }
    }

    pub fn sanitize(&mut self) -> SanitizeReport {
        let before = self.clone();
        for gain in &mut self.gains {
            *gain = finite_or(*gain, 0.0).clamp(-12.0, 12.0);
        }
        self.preamp_db = finite_or(self.preamp_db, 0.0).clamp(-24.0, 12.0);
        self.bass_db = finite_or(self.bass_db, 0.0).clamp(0.0, 15.0);
        self.width = finite_or(self.width, 1.0).clamp(0.0, 2.0);
        self.crossfeed = finite_or(self.crossfeed, 0.0).clamp(0.0, 1.0);
        for (index, setting) in self.band_settings.iter_mut().enumerate() {
            *setting = setting.sanitized(index);
        }
        SanitizeReport {
            changed: *self != before,
        }
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub name: String,
    pub profile: Option<String>,
    pub sound: SoundSettings,
}

impl Preset {
    pub fn new(name: impl Into<String>, sound: SoundSettings) -> Self {
        Self {
            name: name.into(),
            profile: None,
            sound,
        }
    }

    pub fn validate(&mut self) -> Result<SanitizeReport, String> {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            return Err("Preset name cannot be empty.".into());
        }
        if self.name.chars().count() > 80 {
            return Err("Preset name must be 80 characters or fewer.".into());
        }
        if self
            .profile
            .as_ref()
            .is_some_and(|name| name.chars().count() > 200)
        {
            return Err("Profile name is too long.".into());
        }
        Ok(self.sound.sanitize())
    }

    pub fn is_modified(&self, sound: &SoundSettings, profile: Option<&str>) -> bool {
        self.sound != *sound || self.profile.as_deref() != profile
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PresetSaveResult {
    Saved,
    NeedsConfirmation(Box<Preset>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetFile {
    pub schema: u32,
    pub preset: Preset,
}

impl PresetFile {
    pub fn new(preset: Preset) -> Self {
        Self {
            schema: PRESET_SCHEMA,
            preset,
        }
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|err| err.to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut file: Self =
            serde_json::from_str(text).map_err(|err| format!("Invalid Rometric preset: {err}"))?;
        if !(1..=PRESET_SCHEMA).contains(&file.schema) {
            return Err(format!(
                "Unsupported preset schema {} (supported: 1–{}).",
                file.schema, PRESET_SCHEMA
            ));
        }
        file.preset.validate()?;
        Ok(file)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileLibrary {
    pub favorites: Vec<String>,
    pub recents: Vec<String>,
}

impl ProfileLibrary {
    /// Returns the favorite state after toggling.
    pub fn toggle_favorite(&mut self, profile: &str) -> bool {
        if let Some(index) = self.favorites.iter().position(|name| name == profile) {
            self.favorites.remove(index);
            false
        } else {
            self.favorites.push(profile.to_string());
            self.favorites.sort_by_key(|name| name.to_lowercase());
            true
        }
    }

    pub fn is_favorite(&self, profile: &str) -> bool {
        self.favorites.iter().any(|name| name == profile)
    }

    pub fn mark_recent(&mut self, profile: impl Into<String>) {
        let profile = profile.into();
        self.recents.retain(|name| name != &profile);
        self.recents.insert(0, profile);
        self.recents.truncate(MAX_RECENTS);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DevicePresetMap(BTreeMap<String, String>);

impl DevicePresetMap {
    pub fn assign(&mut self, device: impl Into<String>, preset: impl Into<String>) {
        self.0.insert(device.into(), preset.into());
    }

    pub fn remove(&mut self, device: &str) {
        self.0.remove(device);
    }

    pub fn preset_for(&self, device: &str) -> Option<&str> {
        self.0.get(device).map(String::as_str)
    }

    pub fn retain_presets<'a>(&mut self, names: impl Iterator<Item = &'a str>) {
        let valid: std::collections::BTreeSet<_> = names.collect();
        self.0.retain(|_, preset| valid.contains(preset.as_str()));
    }
}

#[derive(Clone, Debug)]
pub struct EditHistory {
    undo: Vec<SoundSettings>,
    redo: Vec<SoundSettings>,
    limit: usize,
}

impl EditHistory {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    pub fn record(&mut self, state: SoundSettings) {
        if self.undo.last() != Some(&state) {
            self.undo.push(state);
            if self.undo.len() > self.limit {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: &SoundSettings) -> Option<SoundSettings> {
        let previous = self.undo.pop()?;
        self.redo.push(current.clone());
        Some(previous)
    }

    pub fn redo(&mut self, current: &SoundSettings) -> Option<SoundSettings> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        Some(next)
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReconnectBackoff {
    attempts: u32,
}

impl ReconnectBackoff {
    pub fn next_delay(&mut self) -> Duration {
        let secs = if self.attempts >= 4 {
            10
        } else {
            1u64 << self.attempts
        };
        self.attempts = self.attempts.saturating_add(1);
        Duration::from_secs(secs)
    }

    pub fn reset(&mut self) {
        self.attempts = 0;
    }
}
