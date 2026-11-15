//! Lock-free audio telemetry shared from real-time callbacks to the UI.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TelemetrySnapshot {
    pub input_peak: f32,
    pub output_peak: f32,
    pub limiter_reduction_db: f32,
    pub underruns: u64,
    pub overruns: u64,
    pub processed_frames: u64,
    pub spectrum: Vec<f32>,
    /// Resampled stereo input at the output rate, before any correction.
    pub analysis_input: Vec<[f32; 2]>,
}

pub struct Telemetry {
    input_peak: AtomicU32,
    output_peak: AtomicU32,
    limiter_gain: AtomicU32,
    underruns: AtomicU64,
    overruns: AtomicU64,
    processed_frames: AtomicU64,
    spectrum: Box<[AtomicU32]>,
    spectrum_write: AtomicUsize,
    analysis_input: Box<[AtomicU64]>,
    analysis_write: AtomicUsize,
}

impl Telemetry {
    pub fn new(spectrum_samples: usize) -> Self {
        let len = spectrum_samples.max(1).next_power_of_two();
        let spectrum = (0..len)
            .map(|_| AtomicU32::new(0.0f32.to_bits()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            input_peak: AtomicU32::new(0.0f32.to_bits()),
            output_peak: AtomicU32::new(0.0f32.to_bits()),
            limiter_gain: AtomicU32::new(1.0f32.to_bits()),
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            processed_frames: AtomicU64::new(0),
            spectrum,
            spectrum_write: AtomicUsize::new(0),
            analysis_input: (0..len)
                .map(|_| AtomicU64::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            analysis_write: AtomicUsize::new(0),
        }
    }

    pub fn observe_input(&self, left: f32, right: f32) {
        atomic_peak(&self.input_peak, left.abs().max(right.abs()));
    }

    pub fn observe_output(&self, left: f32, right: f32, limiter_gain: f32) {
        atomic_peak(&self.output_peak, left.abs().max(right.abs()));
        self.limiter_gain
            .store(limiter_gain.clamp(1e-6, 1.0).to_bits(), Ordering::Relaxed);
        self.processed_frames.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_overrun(&self) {
        self.overruns.fetch_add(1, Ordering::Relaxed);
    }

    pub fn push_spectrum(&self, sample: f32) {
        let write = self.spectrum_write.fetch_add(1, Ordering::Relaxed);
        let index = write & (self.spectrum.len() - 1);
        self.spectrum[index].store(sample.to_bits(), Ordering::Relaxed);
    }

    /// Single producer (render callback). Packing L/R in one atomic prevents
    /// torn channel pairs. Publication follows the write; no callback allocation.
    pub fn push_analysis_input(&self, frame: [f32; 2]) {
        let frame = frame.map(|sample| if sample.is_finite() { sample } else { 0.0 });
        let packed = frame[0].to_bits() as u64 | ((frame[1].to_bits() as u64) << 32);
        let write = self.analysis_write.load(Ordering::Relaxed);
        self.analysis_input[write & (self.analysis_input.len() - 1)]
            .store(packed, Ordering::Relaxed);
        self.analysis_write
            .store(write.wrapping_add(1), Ordering::Release);
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        let written = self.spectrum_write.load(Ordering::Acquire);
        let count = written.min(self.spectrum.len());
        let start = written.saturating_sub(count);
        let mut spectrum = Vec::with_capacity(count);
        for logical in start..written {
            let index = logical & (self.spectrum.len() - 1);
            let value = f32::from_bits(self.spectrum[index].load(Ordering::Relaxed));
            spectrum.push(if value.is_finite() { value } else { 0.0 });
        }
        let gain = f32::from_bits(self.limiter_gain.load(Ordering::Relaxed)).clamp(1e-6, 1.0);
        let written = self.analysis_write.load(Ordering::Acquire);
        let count = written.min(self.analysis_input.len());
        let analysis_input = (written.saturating_sub(count)..written)
            .map(|logical| {
                let packed = self.analysis_input[logical & (self.analysis_input.len() - 1)]
                    .load(Ordering::Relaxed);
                [
                    f32::from_bits(packed as u32),
                    f32::from_bits((packed >> 32) as u32),
                ]
            })
            .collect();
        TelemetrySnapshot {
            input_peak: read_finite(&self.input_peak).max(0.0),
            output_peak: read_finite(&self.output_peak).max(0.0),
            limiter_reduction_db: -20.0 * gain.log10(),
            underruns: self.underruns.load(Ordering::Relaxed),
            overruns: self.overruns.load(Ordering::Relaxed),
            processed_frames: self.processed_frames.load(Ordering::Relaxed),
            spectrum,
            analysis_input,
        }
    }

    /// Read and clear peak holds while leaving cumulative diagnostics intact.
    pub fn take_peaks(&self) -> (f32, f32) {
        let input = f32::from_bits(self.input_peak.swap(0.0f32.to_bits(), Ordering::AcqRel));
        let output = f32::from_bits(self.output_peak.swap(0.0f32.to_bits(), Ordering::AcqRel));
        (input.max(0.0), output.max(0.0))
    }
}

fn read_finite(value: &AtomicU32) -> f32 {
    let value = f32::from_bits(value.load(Ordering::Relaxed));
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn atomic_peak(slot: &AtomicU32, value: f32) {
    if !value.is_finite() || value <= 0.0 {
        return;
    }
    let mut current = slot.load(Ordering::Relaxed);
    loop {
        let previous = f32::from_bits(current);
        if previous.is_finite() && previous >= value {
            return;
        }
        match slot.compare_exchange_weak(
            current,
            value.to_bits(),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}
