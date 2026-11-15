//! AutoEQ profile database.
//!
//! Wavelet ships `assets/auto_eq.db` (SQLite, table `autoeq(headphone,
//! equalization)`, 6,029 rows) and serves it through a native bridge. The same
//! dataset is pre-packed into `assets/autoeq.bin.z` here as:
//!
//! ```text
//! magic  "WTQ1"
//! u32    profile count
//! u32    grid point count
//! f32[grid]            frequency in Hz (identical for every profile)
//! per profile:
//!   u8   name length (UTF-8, ASCII in practice)
//!   u8[] name
//!   i16[grid]          gain in 0.1 dB steps
//! ```

use flate2::read::ZlibDecoder;
use std::io::Read;

use crate::dsp::{BAND_FREQ, N_BANDS};

const BLOB: &[u8] = include_bytes!("../assets/autoeq.bin.z");

pub struct Profile {
    pub name: String,
    pub gains: Vec<f32>,
}

pub struct AutoEq {
    pub freqs: Vec<f32>,
    pub profiles: Vec<Profile>,
    lower: Vec<String>,
}

impl AutoEq {
    pub fn load() -> Result<Self, String> {
        let mut decoder = ZlibDecoder::new(BLOB);
        let mut raw = Vec::with_capacity(2_000_000);
        decoder
            .read_to_end(&mut raw)
            .map_err(|e| format!("AutoEQ asset decompression failed: {e}"))?;

        let mut cur = 0usize;
        let magic = raw.get(0..4).ok_or("AutoEQ asset is truncated")?;
        if magic != b"WTQ1" {
            return Err("AutoEQ asset has an unexpected header".into());
        }
        cur += 4;

        let count = read_u32(&raw, &mut cur)? as usize;
        let grid = read_u32(&raw, &mut cur)? as usize;
        if count == 0 || grid < 2 || count > 100_000 || grid > 4_096 {
            return Err("AutoEQ asset header is out of range".into());
        }

        let mut freqs = Vec::with_capacity(grid);
        for _ in 0..grid {
            freqs.push(read_f32(&raw, &mut cur)?);
        }

        let mut profiles = Vec::with_capacity(count);
        for _ in 0..count {
            let name_len = *raw.get(cur).ok_or("truncated profile")? as usize;
            cur += 1;
            let name_bytes = raw.get(cur..cur + name_len).ok_or("truncated name")?;
            let name = std::str::from_utf8(name_bytes)
                .map_err(|_| "profile name is not UTF-8")?
                .to_string();
            cur += name_len;

            let mut gains = Vec::with_capacity(grid);
            for _ in 0..grid {
                let g = read_i16(&raw, &mut cur)? as f32 / 10.0;
                gains.push(g);
            }
            profiles.push(Profile { name, gains });
        }

        let lower = profiles.iter().map(|p| p.name.to_lowercase()).collect();
        Ok(Self {
            freqs,
            profiles,
            lower,
        })
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.profiles.iter().position(|p| p.name == name)
    }

    /// Precomputed brand and sound-signature folders for the profile browser.
    pub fn catalog(&self) -> crate::catalog::ProfileCatalog {
        let names: Vec<String> = self.profiles.iter().map(|p| p.name.clone()).collect();
        let curves: Vec<&[f32]> = self.profiles.iter().map(|p| p.gains.as_slice()).collect();
        crate::catalog::ProfileCatalog::build(&names, &self.freqs, &curves)
    }

    /// Case-insensitive substring search over profile names.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return (0..self.profiles.len()).collect();
        }
        self.lower
            .iter()
            .enumerate()
            .filter(|(_, name)| name.contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    /// AutoEQ correction in dB at an arbitrary frequency, linearly interpolated
    /// in log-frequency and clamped at the ends of the measurement grid.
    pub fn curve_gain(&self, index: usize, freq: f32) -> f32 {
        let Some(profile) = self.profiles.get(index) else {
            return 0.0;
        };
        let gains = &profile.gains;
        let freqs = &self.freqs;
        if freqs.len() < 2 || gains.len() != freqs.len() || !freq.is_finite() {
            return 0.0;
        }
        if freq <= freqs[0] {
            return gains[0];
        }
        if freq >= freqs[freqs.len() - 1] {
            return gains[gains.len() - 1];
        }
        let upper = freqs.partition_point(|&f| f < freq);
        let i = upper.saturating_sub(1).min(freqs.len() - 2);
        let (f0, f1) = (freqs[i], freqs[i + 1]);
        let (g0, g1) = (gains[i], gains[i + 1]);
        if f1 <= f0 {
            return g0;
        }
        let t = ((freq.max(1.0)).ln() - f0.max(1.0).ln()) / (f1.max(1.0).ln() - f0.max(1.0).ln());
        let t = t.clamp(0.0, 1.0);
        g0 + (g1 - g0) * t
    }

    /// The 11 slider gains for a profile.
    pub fn bands_for(&self, index: usize) -> [f32; N_BANDS] {
        let mut out = [0.0; N_BANDS];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.curve_gain(index, BAND_FREQ[i]);
        }
        out
    }

    /// Preamp that keeps the loudest part of the correction below 0 dBFS.
    pub fn auto_preamp(&self, index: usize) -> f32 {
        let Some(profile) = self.profiles.get(index) else {
            return 0.0;
        };
        let peak = profile
            .gains
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        if !peak.is_finite() {
            return 0.0;
        }
        (-peak).clamp(-24.0, 0.0)
    }
}

fn read_u32(buf: &[u8], cur: &mut usize) -> Result<u32, String> {
    let bytes = buf.get(*cur..*cur + 4).ok_or("AutoEQ asset is truncated")?;
    *cur += 4;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_i16(buf: &[u8], cur: &mut usize) -> Result<i16, String> {
    let bytes = buf.get(*cur..*cur + 2).ok_or("AutoEQ asset is truncated")?;
    *cur += 2;
    Ok(i16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_f32(buf: &[u8], cur: &mut usize) -> Result<f32, String> {
    Ok(f32::from_bits(read_u32(buf, cur)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_loads_completely() {
        let db = AutoEq::load().expect("embedded asset must parse");
        assert_eq!(db.len(), 6029);
        assert_eq!(db.freqs.len(), 127);
    }

    #[test]
    fn known_profile_matches_source_data() {
        let db = AutoEq::load().unwrap();
        let idx = db.index_of("1Custom SA02").expect("profile present");
        // First measurement point in the shipped database is 20 Hz / +6.1 dB.
        assert!((db.curve_gain(idx, 20.0) - 6.1).abs() < 1e-3);
        // And it is -0.1 dB at 115 Hz.
        assert!((db.curve_gain(idx, 115.0) + 0.1).abs() < 1e-3);
    }

    #[test]
    fn interpolation_is_monotonic_between_points() {
        let db = AutoEq::load().unwrap();
        let idx = db.index_of("1Custom SA02").unwrap();
        let a = db.curve_gain(idx, 100.0);
        let b = db.curve_gain(idx, 150.0);
        let m = db.curve_gain(idx, 125.0);
        assert!(m <= a.max(b) + 1e-3 && m >= a.min(b) - 1e-3);
    }

    #[test]
    fn search_is_case_insensitive() {
        let db = AutoEq::load().unwrap();
        let hits = db.search("sennheiser hd 60");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|&i| db.profiles[i]
            .name
            .to_lowercase()
            .contains("sennheiser hd 60")));
    }

    #[test]
    fn auto_preamp_is_never_positive() {
        let db = AutoEq::load().unwrap();
        for i in [0, 100, 3000, 6000] {
            let p = db.auto_preamp(i);
            assert!((-24.0..=0.0).contains(&p));
        }
    }

    #[test]
    fn bands_are_finite() {
        let db = AutoEq::load().unwrap();
        for i in [0, 1, 5000, 6028] {
            for g in db.bands_for(i) {
                assert!(g.is_finite());
            }
        }
    }
}
