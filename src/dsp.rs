//! Biquad equalizer and dynamics.
//!
//! This mirrors the intent of Android's `DynamicsProcessing` stages that
//! Wavelet configures: a pre-EQ shaped like AutoEQ's correction curve, an
//! input gain (preamp), a low-shelf bass boost and a limiter.

use serde::{Deserialize, Serialize};

/// Wavelet uses 11 bands at {0, 62.5, 125, 250, 500, 1k, 2k, 4k, 8k, 16k, 24k}
/// Hz. Band 0 and band 10 are shelves; the rest are peaking filters. The top
/// band is moved from 24 kHz (Nyquist on 48 kHz hardware, where a shelf cannot
/// be realised) to 16 kHz so it still shapes the audible treble.
pub const N_BANDS: usize = 11;

pub const BAND_FREQ: [f32; N_BANDS] = [
    40.0, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12000.0, 16000.0,
];

pub const BAND_LABEL: [&str; N_BANDS] = [
    "40", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "12k", "16k",
];

/// Peaking filters use roughly one-octave spacing, so Q = sqrt(2) is a good fit.
const PEAK_Q: f32 = 1.41;
const SHELF_SLOPE: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum BandKind {
    LowShelf,
    Peak,
    HighShelf,
}

impl BandKind {
    pub const ALL: [Self; 3] = [Self::Peak, Self::LowShelf, Self::HighShelf];

    pub fn label(self) -> &'static str {
        match self {
            Self::Peak => "Peak",
            Self::LowShelf => "Low shelf",
            Self::HighShelf => "High shelf",
        }
    }

    pub fn from_code(code: u32) -> Self {
        match code {
            0 => Self::LowShelf,
            2 => Self::HighShelf,
            _ => Self::Peak,
        }
    }
}

pub fn band_kind(index: usize) -> BandKind {
    if index == 0 {
        BandKind::LowShelf
    } else if index == N_BANDS - 1 {
        BandKind::HighShelf
    } else {
        BandKind::Peak
    }
}

/// Optional parametric metadata for the existing eleven gain controls.
/// Graphic mode continues to use the original frequencies and coefficients.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BandSettings {
    pub frequency_hz: f32,
    pub q: f32,
    pub kind: BandKind,
    pub enabled: bool,
}

impl Default for BandSettings {
    fn default() -> Self {
        Self {
            frequency_hz: 1000.0,
            q: PEAK_Q,
            kind: BandKind::Peak,
            enabled: true,
        }
    }
}

impl BandSettings {
    pub fn for_band(index: usize) -> Self {
        Self {
            frequency_hz: BAND_FREQ[index],
            q: if band_kind(index) == BandKind::Peak {
                PEAK_Q
            } else {
                std::f32::consts::FRAC_1_SQRT_2
            },
            kind: band_kind(index),
            enabled: true,
        }
    }

    pub fn sanitized(mut self, index: usize) -> Self {
        let default = Self::for_band(index);
        self.frequency_hz =
            finite_or(self.frequency_hz, default.frequency_hz).clamp(20.0, 20_000.0);
        self.q = finite_or(self.q, default.q).clamp(0.1, 12.0);
        self
    }
}

pub fn default_band_settings() -> [BandSettings; N_BANDS] {
    std::array::from_fn(BandSettings::for_band)
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

/// Direct Form II Transposed biquad. Coefficients are normalised so a0 == 1.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Biquad {
    pub const IDENTITY: Biquad = Biquad {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalised(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        if a0.abs() < f32::EPSILON || !a0.is_finite() {
            return Self::IDENTITY;
        }
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// RBJ cookbook peaking EQ.
    pub fn peak(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        if gain_db.abs() < 1e-4 || !gain_db.is_finite() {
            return Self::IDENTITY;
        }
        let f = clamp_freq(sample_rate, freq);
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * f / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q.max(0.1));
        Self::normalised(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        )
    }

    /// RBJ cookbook low shelf.
    pub fn low_shelf(sample_rate: f32, freq: f32, slope: f32, gain_db: f32) -> Self {
        if gain_db.abs() < 1e-4 || !gain_db.is_finite() {
            return Self::IDENTITY;
        }
        let f = clamp_freq(sample_rate, freq);
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * f / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / 2.0 * ((a + 1.0 / a) * (1.0 / slope.max(0.1) - 1.0) + 2.0).sqrt();
        let beta = 2.0 * a.sqrt() * alpha;
        Self::normalised(
            a * ((a + 1.0) - (a - 1.0) * cos + beta),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - beta),
            (a + 1.0) + (a - 1.0) * cos + beta,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - beta,
        )
    }

    /// RBJ cookbook high shelf.
    pub fn high_shelf(sample_rate: f32, freq: f32, slope: f32, gain_db: f32) -> Self {
        if gain_db.abs() < 1e-4 || !gain_db.is_finite() {
            return Self::IDENTITY;
        }
        let f = clamp_freq(sample_rate, freq);
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * f / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / 2.0 * ((a + 1.0 / a) * (1.0 / slope.max(0.1) - 1.0) + 2.0).sqrt();
        let beta = 2.0 * a.sqrt() * alpha;
        Self::normalised(
            a * ((a + 1.0) + (a - 1.0) * cos + beta),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - beta),
            (a + 1.0) - (a - 1.0) * cos + beta,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - beta,
        )
    }

    /// RBJ shelving coefficients with Q as the resonance control. The default
    /// Q of 1/sqrt(2) is equivalent to the original shelves' slope of one.
    pub fn shelf_q(sample_rate: f32, freq: f32, q: f32, gain_db: f32, high: bool) -> Self {
        if gain_db.abs() < 1e-4 || !gain_db.is_finite() {
            return Self::IDENTITY;
        }
        let f = clamp_freq(sample_rate, freq);
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * f / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let beta = 2.0 * a.sqrt() * sin / (2.0 * q.max(0.1));
        if high {
            Self::normalised(
                a * ((a + 1.0) + (a - 1.0) * cos + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - beta),
                (a + 1.0) - (a - 1.0) * cos + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - beta,
            )
        } else {
            Self::normalised(
                a * ((a + 1.0) - (a - 1.0) * cos + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - beta),
                (a + 1.0) + (a - 1.0) * cos + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - beta,
            )
        }
    }

    #[inline]
    pub fn process(&self, x: f32, z: &mut [f32; 2]) -> f32 {
        let y = self.b0 * x + z[0];
        z[0] = self.b1 * x - self.a1 * y + z[1];
        z[1] = self.b2 * x - self.a2 * y;
        if y.is_finite() {
            y
        } else {
            *z = [0.0, 0.0];
            0.0
        }
    }

    /// Magnitude response in dB at `freq`.
    pub fn magnitude_db(&self, sample_rate: f32, freq: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        let num = (num_re * num_re + num_im * num_im).sqrt();
        let den = (den_re * den_re + den_im * den_im).sqrt();
        if den <= f32::EPSILON || !num.is_finite() || !den.is_finite() {
            return 0.0;
        }
        20.0 * (num / den).log10()
    }
}

fn clamp_freq(sample_rate: f32, freq: f32) -> f32 {
    let nyquist = sample_rate * 0.49;
    freq.clamp(10.0, nyquist.max(10.0))
}

/// Stereo-linked peak limiter with a soft ceiling.
#[derive(Clone, Copy, Debug)]
pub struct Limiter {
    pub enabled: bool,
    threshold: f32,
    attack: f32,
    release: f32,
    gain: f32,
}

impl Limiter {
    pub fn new(sample_rate: f32, enabled: bool) -> Self {
        Self {
            enabled,
            threshold: 0.95,
            attack: 1.0 - (-1.0 / (sample_rate * 0.002)).exp(),
            release: 1.0 - (-1.0 / (sample_rate * 0.100)).exp(),
            gain: 1.0,
        }
    }

    /// Returns the gain to apply for a given instantaneous peak.
    #[inline]
    pub fn gain_for(&mut self, peak: f32) -> f32 {
        if !self.enabled {
            return 1.0;
        }
        let desired = if peak > self.threshold && peak > 0.0 {
            self.threshold / peak
        } else {
            1.0
        };
        let coef = if desired < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain += (desired - self.gain) * coef;
        self.gain = self.gain.clamp(0.0, 1.0);
        self.gain
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }
}

#[inline]
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.96;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        let over = (a - KNEE) / (1.0 - KNEE);
        x.signum() * (KNEE + (1.0 - KNEE) * over.tanh())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub gains: [f32; N_BANDS],
    pub preamp_db: f32,
    pub bass_db: f32,
    pub width: f32,
    pub limiter: bool,
    pub enabled: bool,
    pub parametric_enabled: bool,
    pub band_settings: [BandSettings; N_BANDS],
    pub crossfeed: f32,
    /// Temporary, attenuation-only A/B calibration. Never stored in a preset.
    pub comparison_gain_db: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            gains: [0.0; N_BANDS],
            preamp_db: 0.0,
            bass_db: 0.0,
            width: 1.0,
            limiter: true,
            enabled: true,
            parametric_enabled: false,
            band_settings: default_band_settings(),
            crossfeed: 0.0,
            comparison_gain_db: 0.0,
        }
    }
}

impl Params {
    pub fn effective_band(&self, index: usize) -> BandSettings {
        if self.parametric_enabled {
            self.band_settings[index].sanitized(index)
        } else {
            BandSettings::for_band(index)
        }
    }
}

fn make_band(sample_rate: f32, params: &Params, index: usize) -> Biquad {
    let setting = params.effective_band(index);
    let gain = params.gains[index];
    if !setting.enabled {
        return Biquad::IDENTITY;
    }
    if params.parametric_enabled {
        match setting.kind {
            BandKind::Peak => Biquad::peak(sample_rate, setting.frequency_hz, setting.q, gain),
            BandKind::LowShelf => {
                Biquad::shelf_q(sample_rate, setting.frequency_hz, setting.q, gain, false)
            }
            BandKind::HighShelf => {
                Biquad::shelf_q(sample_rate, setting.frequency_hz, setting.q, gain, true)
            }
        }
    } else {
        // Keep the original graphic/AutoEQ path bit-for-bit when optional
        // parametric mode is off.
        match setting.kind {
            BandKind::LowShelf => {
                Biquad::low_shelf(sample_rate, setting.frequency_hz, SHELF_SLOPE, gain)
            }
            BandKind::Peak => Biquad::peak(sample_rate, setting.frequency_hz, PEAK_Q, gain),
            BandKind::HighShelf => {
                Biquad::high_shelf(sample_rate, setting.frequency_hz, SHELF_SLOPE, gain)
            }
        }
    }
}

/// A complete stereo processing chain.
pub struct EqChain {
    sample_rate: f32,
    preamp: f32,
    bands: [Biquad; N_BANDS],
    bass: Biquad,
    limiter: Limiter,
    width: f32,
    enabled: bool,
    band_z: [[[f32; 2]; 2]; N_BANDS],
    bass_z: [[f32; 2]; 2],
    crossfeed: f32,
    crossfeed_target: f32,
    crossfeed_low_side: f32,
    crossfeed_alpha: f32,
    comparison_gain: f32,
    comparison_target: f32,
    control_smoothing: f32,
}

impl EqChain {
    pub fn new(sample_rate: f32, params: &Params) -> Self {
        let fs = if sample_rate.is_finite() && sample_rate >= 8000.0 {
            sample_rate
        } else {
            48_000.0
        };
        let bands = std::array::from_fn(|index| make_band(fs, params, index));
        let crossfeed = finite_or(params.crossfeed, 0.0).clamp(0.0, 1.0);
        let comparison =
            10f32.powf(finite_or(params.comparison_gain_db, 0.0).clamp(-24.0, 0.0) / 20.0);
        Self {
            sample_rate: fs,
            preamp: 10f32.powf(finite_or(params.preamp_db, 0.0) / 20.0),
            bands,
            bass: Biquad::low_shelf(fs, 80.0, 0.7, params.bass_db),
            limiter: Limiter::new(fs, params.limiter),
            width: params.width.clamp(0.0, 2.0),
            enabled: params.enabled,
            band_z: [[[0.0; 2]; 2]; N_BANDS],
            bass_z: [[0.0; 2]; 2],
            crossfeed,
            crossfeed_target: crossfeed,
            crossfeed_low_side: 0.0,
            crossfeed_alpha: 1.0 - (-std::f32::consts::TAU * 700.0 / fs).exp(),
            comparison_gain: comparison,
            comparison_target: comparison,
            control_smoothing: 1.0 - (-1.0 / (fs * 0.030)).exp(),
        }
    }

    /// Update coefficients and controls without clearing delay lines or the
    /// limiter envelope. This keeps live slider, preset and A/B changes from
    /// introducing the hard discontinuity caused by rebuilding the chain.
    pub fn update_params(&mut self, params: &Params) {
        self.preamp = 10f32.powf(finite_or(params.preamp_db, 0.0) / 20.0);
        for (index, band) in self.bands.iter_mut().enumerate() {
            *band = make_band(self.sample_rate, params, index);
        }
        self.bass = Biquad::low_shelf(self.sample_rate, 80.0, 0.7, params.bass_db);
        self.limiter.enabled = params.limiter;
        self.width = params.width.clamp(0.0, 2.0);
        self.enabled = params.enabled;
        self.crossfeed_target = finite_or(params.crossfeed, 0.0).clamp(0.0, 1.0);
        self.comparison_target =
            10f32.powf(finite_or(params.comparison_gain_db, 0.0).clamp(-24.0, 0.0) / 20.0);
    }

    #[inline]
    pub fn process(&mut self, mut l: f32, mut r: f32) -> (f32, f32) {
        if !self.enabled {
            return (l, r);
        }
        l *= self.preamp;
        r *= self.preamp;

        for i in 0..N_BANDS {
            l = self.bands[i].process(l, &mut self.band_z[i][0]);
            r = self.bands[i].process(r, &mut self.band_z[i][1]);
        }
        l = self.bass.process(l, &mut self.bass_z[0]);
        r = self.bass.process(r, &mut self.bass_z[1]);

        if (self.width - 1.0).abs() > 1e-4 {
            let mid = 0.5 * (l + r);
            let side = 0.5 * (l - r) * self.width;
            l = mid + side;
            r = mid - side;
        }

        // A frequency-dependent interaural feed, not another width control.
        // Reducing only the low-frequency side preserves mono and total L+R.
        self.crossfeed += self.control_smoothing * (self.crossfeed_target - self.crossfeed);
        let side = 0.5 * (l - r);
        self.crossfeed_low_side += self.crossfeed_alpha * (side - self.crossfeed_low_side);
        if self.crossfeed > 0.0 {
            let feed = 0.7 * self.crossfeed * self.crossfeed_low_side;
            l -= feed;
            r += feed;
        }

        self.comparison_gain +=
            self.control_smoothing * (self.comparison_target - self.comparison_gain);
        if self.comparison_gain != 1.0 {
            l *= self.comparison_gain;
            r *= self.comparison_gain;
        }

        let peak = l.abs().max(r.abs());
        let g = self.limiter.gain_for(peak);
        (soft_clip(l * g), soft_clip(r * g))
    }

    /// Combined magnitude response including preamp, bands and bass shelf.
    pub fn response_db(&self, freq: f32) -> f32 {
        let mut db = 20.0 * (self.preamp * self.comparison_target).max(1e-9).log10();
        for band in &self.bands {
            db += band.magnitude_db(self.sample_rate, freq);
        }
        db += self.bass.magnitude_db(self.sample_rate, freq);
        db
    }

    pub fn reset(&mut self) {
        self.band_z = [[[0.0; 2]; 2]; N_BANDS];
        self.bass_z = [[0.0; 2]; 2];
        self.limiter.reset();
        self.crossfeed_low_side = 0.0;
        self.crossfeed = self.crossfeed_target;
        self.comparison_gain = self.comparison_target;
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn limiter_gain(&self) -> f32 {
        self.limiter.gain()
    }

    pub fn limiter_reduction_db(&self) -> f32 {
        -20.0 * self.limiter_gain().clamp(1e-6, 1.0).log10()
    }
}

/// Highest combined response across the audible range. Used for headroom
/// guidance and safe-preamp calculation in the desktop UI.
pub fn max_response_db(sample_rate: f32, params: &Params) -> f32 {
    let chain = EqChain::new(sample_rate, params);
    (0..=512)
        .map(|index| {
            let t = index as f32 / 512.0;
            chain.response_db(20.0 * 1000f32.powf(t))
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> Params {
        Params {
            limiter: false,
            ..Default::default()
        }
    }

    #[test]
    fn flat_chain_is_unity() {
        let chain = EqChain::new(48_000.0, &flat());
        for f in [20.0, 100.0, 1_000.0, 10_000.0, 19_000.0] {
            assert!(chain.response_db(f).abs() < 0.01, "f={f}");
        }
    }

    #[test]
    fn peak_gain_is_accurate() {
        let mut p = flat();
        p.gains[5] = 6.0; // 1 kHz
        let chain = EqChain::new(48_000.0, &p);
        let at = chain.response_db(1_000.0);
        assert!((at - 6.0).abs() < 0.2, "expected ~6 dB, got {at}");
    }

    #[test]
    fn low_shelf_boosts_lows_only() {
        let mut p = flat();
        p.gains[0] = 6.0;
        let chain = EqChain::new(48_000.0, &p);
        assert!(chain.response_db(20.0) > 5.0);
        assert!(chain.response_db(10_000.0).abs() < 0.5);
    }

    #[test]
    fn high_shelf_lifts_treble() {
        let mut p = flat();
        p.gains[10] = 6.0;
        let chain = EqChain::new(48_000.0, &p);
        assert!(chain.response_db(18_000.0) > 4.0);
        assert!(chain.response_db(100.0).abs() < 0.5);
    }

    #[test]
    fn preamp_scales() {
        let mut p = flat();
        p.preamp_db = 6.0;
        let chain = EqChain::new(48_000.0, &p);
        assert!((chain.response_db(1_000.0) - 6.0).abs() < 0.01);
    }

    #[test]
    fn limiter_never_exceeds_ceiling() {
        let mut p = flat();
        p.preamp_db = 20.0;
        p.limiter = true;
        let mut chain = EqChain::new(48_000.0, &p);
        let mut peak = 0.0f32;
        for n in 0..48_000 {
            let x = (2.0 * std::f32::consts::PI * 440.0 * n as f32 / 48_000.0).sin();
            let (l, r) = chain.process(x, x);
            peak = peak.max(l.abs()).max(r.abs());
            assert!(l.is_finite() && r.is_finite());
        }
        assert!(peak <= 1.0, "peak {peak}");
    }

    #[test]
    fn paused_chain_passes_through() {
        let mut p = flat();
        p.enabled = false;
        let mut chain = EqChain::new(48_000.0, &p);
        let (l, r) = chain.process(0.25, -0.5);
        assert_eq!((l, r), (0.25, -0.5));
    }

    #[test]
    fn extreme_gains_stay_stable() {
        let mut p = flat();
        p.gains = [12.0; N_BANDS];
        p.bass_db = 12.0;
        p.preamp_db = -12.0;
        let mut chain = EqChain::new(48_000.0, &p);
        let mut peak = 0.0f32;
        for n in 0..20_000 {
            let x = (n as f32 * 0.05).sin();
            let (l, r) = chain.process(x, x * 0.5);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs());
        }
        assert!(peak < 8.0);
    }
}
