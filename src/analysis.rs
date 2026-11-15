//! UI-thread audio analysis. Nothing here runs in a real-time callback.

use crate::dsp::{max_response_db, EqChain, Params};

pub const SPECTRUM_BANDS: usize = 96;
pub const SPECTRUM_FLOOR_DB: f32 = -80.0;

/// Peak amplitude relative to full scale. Silence has no finite dB level.
pub fn peak_dbfs(peak: f32) -> f32 {
    if peak.is_finite() && peak > 0.0 {
        20.0 * peak.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// Hann-windowed radix-two FFT mapped to logarithmic frequency bands. The
/// window is normalized for amplitude, and silence never creates fake bars.
pub fn spectrum_levels(samples: &[f32], sample_rate: f32) -> [f32; SPECTRUM_BANDS] {
    let mut levels = [SPECTRUM_FLOOR_DB; SPECTRUM_BANDS];
    if samples.len() < 64 || !sample_rate.is_finite() || sample_rate < 8000.0 {
        return levels;
    }
    let available = samples.len().min(4096);
    let len = 1usize << (usize::BITS - 1 - available.leading_zeros());
    let samples = &samples[samples.len() - len..];
    let finite = |sample: f32| if sample.is_finite() { sample } else { 0.0 };
    let mean = samples.iter().map(|sample| finite(*sample)).sum::<f32>() / len as f32;
    let mut real = Vec::with_capacity(len);
    let mut imaginary = vec![0.0; len];
    let mut window_sum = 0.0;
    for (index, sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / (len - 1) as f32).cos();
        window_sum += window;
        real.push((finite(*sample) - mean) * window);
    }
    fft(&mut real, &mut imaginary);
    let amplitudes: Vec<_> = (0..=len / 2)
        .map(|index| 2.0 * real[index].hypot(imaginary[index]) / window_sum)
        .collect();
    let bin_hz = sample_rate / len as f32;
    let half_band = 1000f32.powf(0.5 / (SPECTRUM_BANDS - 1) as f32);
    for (index, level) in levels.iter_mut().enumerate() {
        let frequency = 20.0 * 1000f32.powf(index as f32 / (SPECTRUM_BANDS - 1) as f32);
        if frequency >= sample_rate * 0.5 {
            continue;
        }
        let low = (frequency / half_band / bin_hz).ceil().max(1.0) as usize;
        let high = (frequency * half_band / bin_hz)
            .floor()
            .min((len / 2) as f32) as usize;
        let amplitude = if low <= high {
            amplitudes[low..=high].iter().copied().fold(0.0, f32::max)
        } else {
            let bin = (frequency / bin_hz).clamp(0.0, (len / 2 - 1) as f32);
            let left = bin.floor() as usize;
            let t = bin - left as f32;
            amplitudes[left] * (1.0 - t) + amplitudes[left + 1] * t
        };
        *level = peak_dbfs(amplitude).clamp(SPECTRUM_FLOOR_DB, 6.0);
    }
    levels
}

fn fft(real: &mut [f32], imaginary: &mut [f32]) {
    let len = real.len();
    let mut reversed = 0;
    for index in 1..len {
        let mut bit = len >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            real.swap(index, reversed);
            imaginary.swap(index, reversed);
        }
    }
    let mut size = 2;
    while size <= len {
        let angle = -std::f32::consts::TAU / size as f32;
        let (step_im, step_re) = angle.sin_cos();
        for start in (0..len).step_by(size) {
            let (mut wr, mut wi) = (1.0, 0.0);
            for offset in 0..size / 2 {
                let left = start + offset;
                let right = left + size / 2;
                let re = wr * real[right] - wi * imaginary[right];
                let im = wr * imaginary[right] + wi * real[right];
                real[right] = real[left] - re;
                imaginary[right] = imaginary[left] - im;
                real[left] += re;
                imaginary[left] += im;
                let next_wr = wr * step_re - wi * step_im;
                wi = wr * step_im + wi * step_re;
                wr = next_wr;
            }
        }
        size *= 2;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelMatch {
    pub trims_db: [f32; 2],
    pub limited: bool,
}

/// Compare both setups against the SAME real, resampled stereo input window.
/// Measurement is pre-limiter sample RMS, not LUFS or acoustic loudness. The
/// temporary trim only attenuates, is capped at 24 dB, and is never persisted.
pub fn level_match(
    sample_rate: f32,
    frames: &[[f32; 2]],
    slots: &[Params; 2],
) -> Option<LevelMatch> {
    if slots.iter().any(|params| !params.enabled)
        || frames.len() < 2048
        || !sample_rate.is_finite()
        || !(8000.0..=384000.0).contains(&sample_rate)
    {
        return None;
    }
    let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
    let input_peak = frames
        .iter()
        .flat_map(|frame| frame.iter())
        .map(|x| finite(*x).abs())
        .fold(0.0, f32::max);
    if input_peak < 1e-4 {
        return None;
    }
    let slots = slots.map(|mut params| {
        params.comparison_gain_db = 0.0;
        params.limiter = false;
        params
    });
    // A common, conservative scale keeps the existing safety clipper out of
    // the measurement. It cancels out in the ratio; no user setting is changed.
    let peak_db = slots
        .iter()
        .map(|p| max_response_db(sample_rate, p))
        .fold(0.0, f32::max);
    let scale = (0.05 / (10f32.powf(peak_db / 20.0) * input_peak * 2.0)).min(1.0);
    let mut rms = [0.0f64; 2];
    for (slot, params) in slots.iter().enumerate() {
        let mut chain = EqChain::new(sample_rate, params);
        // Warm the filter states on this same signal instead of measuring the
        // initial zero-delay-line transient as a difference between presets.
        for index in 0..(sample_rate * 0.25) as usize {
            let frame = frames[index % frames.len()];
            chain.process(finite(frame[0]) * scale, finite(frame[1]) * scale);
        }
        for frame in frames {
            let (l, r) = chain.process(finite(frame[0]) * scale, finite(frame[1]) * scale);
            rms[slot] += 0.5 * (f64::from(l).powi(2) + f64::from(r).powi(2));
        }
        rms[slot] = (rms[slot] / frames.len() as f64).sqrt();
    }
    if rms
        .iter()
        .any(|x| !x.is_finite() || *x <= f64::MIN_POSITIVE)
    {
        return None;
    }
    let reference = rms[0].min(rms[1]);
    let requested = rms.map(|value| (20.0 * (reference / value).log10()) as f32);
    Some(LevelMatch {
        trims_db: requested.map(|db| db.clamp(-24.0, 0.0)),
        limited: requested.iter().any(|db| *db < -24.0),
    })
}
