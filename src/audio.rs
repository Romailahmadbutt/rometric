//! Windows audio backend.
//!
//! Wavelet attaches `DynamicsProcessing` to audio *sessions* through Android's
//! `AudioEffect` API. Windows has no user-mode equivalent, so this build uses
//! the standard desktop approach: capture a device (normally a virtual cable),
//! process the stream, and render it to the real output. See `README.md` for
//! the routing setup.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample, StreamConfig};

use rometric::dsp::{BandKind, EqChain, Params, N_BANDS};
use rometric::resample::Resampler;
use rometric::telemetry::{Telemetry, TelemetrySnapshot};

// ---------------------------------------------------------------------------
// Parameter sharing (UI thread -> audio callback)
// ---------------------------------------------------------------------------

pub struct SharedParams {
    generation: AtomicU64,
    gains: [AtomicU32; N_BANDS],
    preamp: AtomicU32,
    bass: AtomicU32,
    width: AtomicU32,
    limiter: AtomicBool,
    enabled: AtomicBool,
    parametric_enabled: AtomicBool,
    frequencies: [AtomicU32; N_BANDS],
    q: [AtomicU32; N_BANDS],
    kinds: [AtomicU32; N_BANDS],
    band_enabled: [AtomicBool; N_BANDS],
    crossfeed: AtomicU32,
    comparison_gain: AtomicU32,
}

impl SharedParams {
    pub fn new(params: &Params) -> Self {
        let this = Self {
            generation: AtomicU64::new(0),
            gains: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            preamp: AtomicU32::new(0.0f32.to_bits()),
            bass: AtomicU32::new(0.0f32.to_bits()),
            width: AtomicU32::new(1.0f32.to_bits()),
            limiter: AtomicBool::new(true),
            enabled: AtomicBool::new(true),
            parametric_enabled: AtomicBool::new(false),
            frequencies: std::array::from_fn(|_| AtomicU32::new(0)),
            q: std::array::from_fn(|_| AtomicU32::new(0)),
            kinds: std::array::from_fn(|_| AtomicU32::new(0)),
            band_enabled: std::array::from_fn(|_| AtomicBool::new(true)),
            crossfeed: AtomicU32::new(0),
            comparison_gain: AtomicU32::new(0),
        };
        this.store(params);
        this
    }

    pub fn store(&self, params: &Params) {
        // A tiny seqlock keeps multi-field snapshots coherent without taking a
        // lock in the real-time callback. Odd generations are being written.
        self.generation.fetch_add(1, Ordering::AcqRel);
        for (slot, gain) in self.gains.iter().zip(params.gains.iter()) {
            slot.store(gain.to_bits(), Ordering::Relaxed);
        }
        self.preamp
            .store(params.preamp_db.to_bits(), Ordering::Relaxed);
        self.bass.store(params.bass_db.to_bits(), Ordering::Relaxed);
        self.width.store(params.width.to_bits(), Ordering::Relaxed);
        self.limiter.store(params.limiter, Ordering::Relaxed);
        self.enabled.store(params.enabled, Ordering::Relaxed);
        self.parametric_enabled
            .store(params.parametric_enabled, Ordering::Relaxed);
        for (index, setting) in params.band_settings.iter().enumerate() {
            self.frequencies[index].store(setting.frequency_hz.to_bits(), Ordering::Relaxed);
            self.q[index].store(setting.q.to_bits(), Ordering::Relaxed);
            self.kinds[index].store(setting.kind as u32, Ordering::Relaxed);
            self.band_enabled[index].store(setting.enabled, Ordering::Relaxed);
        }
        self.crossfeed
            .store(params.crossfeed.to_bits(), Ordering::Relaxed);
        self.comparison_gain
            .store(params.comparison_gain_db.to_bits(), Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn snapshot(&self) -> (u64, Params) {
        loop {
            let before = self.generation.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let mut params = Params::default();
            for (slot, gain) in self.gains.iter().zip(params.gains.iter_mut()) {
                *gain = f32::from_bits(slot.load(Ordering::Relaxed));
            }
            params.preamp_db = f32::from_bits(self.preamp.load(Ordering::Relaxed));
            params.bass_db = f32::from_bits(self.bass.load(Ordering::Relaxed));
            params.width = f32::from_bits(self.width.load(Ordering::Relaxed));
            params.limiter = self.limiter.load(Ordering::Relaxed);
            params.enabled = self.enabled.load(Ordering::Relaxed);
            params.parametric_enabled = self.parametric_enabled.load(Ordering::Relaxed);
            for (index, setting) in params.band_settings.iter_mut().enumerate() {
                setting.frequency_hz =
                    f32::from_bits(self.frequencies[index].load(Ordering::Relaxed));
                setting.q = f32::from_bits(self.q[index].load(Ordering::Relaxed));
                setting.kind = BandKind::from_code(self.kinds[index].load(Ordering::Relaxed));
                setting.enabled = self.band_enabled[index].load(Ordering::Relaxed);
            }
            params.crossfeed = f32::from_bits(self.crossfeed.load(Ordering::Relaxed));
            params.comparison_gain_db =
                f32::from_bits(self.comparison_gain.load(Ordering::Relaxed));
            let after = self.generation.load(Ordering::Acquire);
            if before == after {
                return (after, params);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lock-free-ish stereo queue (capture -> render)
// ---------------------------------------------------------------------------

struct StereoQueue {
    buf: Mutex<VecDeque<f32>>,
    capacity: usize,
    telemetry: Arc<Telemetry>,
}

#[cfg(test)]
mod parameter_tests {
    use super::*;

    #[test]
    fn every_advanced_parameter_reaches_the_audio_snapshot() {
        let mut params = Params {
            parametric_enabled: true,
            crossfeed: 0.6,
            comparison_gain_db: -4.5,
            ..Default::default()
        };
        params.band_settings[5].frequency_hz = 1500.0;
        params.band_settings[5].q = 3.2;
        params.band_settings[5].kind = BandKind::HighShelf;
        params.band_settings[2].enabled = false;
        let shared = SharedParams::new(&params);
        let (generation, snapshot) = shared.snapshot();
        assert_eq!(generation % 2, 0);
        assert_eq!(snapshot, params);
    }

    #[test]
    fn audio_snapshots_never_mix_fields_from_different_ui_edits() {
        let first = Params::default();
        let mut second = first;
        second.parametric_enabled = true;
        second.crossfeed = 0.75;
        second.comparison_gain_db = -7.0;
        second.gains = [6.0; N_BANDS];
        for setting in &mut second.band_settings {
            setting.frequency_hz = 3500.0;
            setting.q = 4.0;
            setting.kind = BandKind::LowShelf;
            setting.enabled = false;
        }
        let shared = Arc::new(SharedParams::new(&first));
        let writer = shared.clone();
        let thread = std::thread::spawn(move || {
            for index in 0..5000 {
                writer.store(if index % 2 == 0 { &first } else { &second });
            }
        });
        for _ in 0..5000 {
            let (_, snapshot) = shared.snapshot();
            assert!(
                snapshot == first || snapshot == second,
                "torn parameter snapshot"
            );
        }
        thread.join().unwrap();
    }
}

impl StereoQueue {
    fn new(frames: usize, telemetry: Arc<Telemetry>) -> Self {
        let capacity = frames.max(256) * 2;
        Self {
            buf: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            telemetry,
        }
    }

    fn push(&self, frame: [f32; 2]) {
        if let Ok(mut q) = self.buf.try_lock() {
            if q.len() + 2 > self.capacity {
                // Drop the oldest frame rather than grow without bound.
                q.pop_front();
                q.pop_front();
                self.telemetry.record_overrun();
            }
            q.push_back(frame[0]);
            q.push_back(frame[1]);
        } else {
            self.telemetry.record_overrun();
        }
    }
}

struct Reader<'a> {
    q: &'a mut VecDeque<f32>,
}

impl Reader<'_> {
    #[inline]
    fn pop(&mut self) -> Option<[f32; 2]> {
        let l = self.q.pop_front()?;
        let r = self.q.pop_front().unwrap_or(l);
        Some([l, r])
    }
}

// ---------------------------------------------------------------------------
// Sample conversion (avoids depending on cpal's lossy trait bounds)
// ---------------------------------------------------------------------------

trait ToF32: Copy {
    fn to_f32(self) -> f32;
}

impl ToF32 for f32 {
    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
}
impl ToF32 for f64 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
}
impl ToF32 for i16 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32 / 32_768.0
    }
}
impl ToF32 for u16 {
    #[inline]
    fn to_f32(self) -> f32 {
        (self as f32 - 32_768.0) / 32_768.0
    }
}
impl ToF32 for i8 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32 / 128.0
    }
}
impl ToF32 for u8 {
    #[inline]
    fn to_f32(self) -> f32 {
        (self as f32 - 128.0) / 128.0
    }
}
impl ToF32 for i32 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32 / 2_147_483_648.0
    }
}
impl ToF32 for i64 {
    #[inline]
    fn to_f32(self) -> f32 {
        (self as f64 / 9_223_372_036_854_775_808.0) as f32
    }
}
impl ToF32 for u32 {
    #[inline]
    fn to_f32(self) -> f32 {
        (self as f64 / 2_147_483_648.0 - 1.0) as f32
    }
}
impl ToF32 for u64 {
    #[inline]
    fn to_f32(self) -> f32 {
        (self as f64 / 9_223_372_036_854_775_808.0 - 1.0) as f32
    }
}

trait FromF32 {
    fn from_f32(v: f32) -> Self;
}

impl FromF32 for f32 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        v
    }
}
impl FromF32 for f64 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        v as f64
    }
}
impl FromF32 for i16 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        (v.clamp(-1.0, 1.0) * 32_767.0) as i16
    }
}
impl FromF32 for u16 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65_535.0) as u16
    }
}
impl FromF32 for i8 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        (v.clamp(-1.0, 1.0) * 127.0) as i8
    }
}
impl FromF32 for u8 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0) as u8
    }
}
impl FromF32 for i32 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        (v.clamp(-1.0, 1.0) as f64 * 2_147_483_647.0) as i32
    }
}
impl FromF32 for u32 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        ((v.clamp(-1.0, 1.0) as f64 * 0.5 + 0.5) * 4_294_967_295.0) as u32
    }
}
impl FromF32 for i64 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        (v.clamp(-1.0, 1.0) as f64 * 9_223_372_036_854_775_807.0) as i64
    }
}
impl FromF32 for u64 {
    #[inline]
    fn from_f32(v: f32) -> Self {
        ((v.clamp(-1.0, 1.0) as f64 * 0.5 + 0.5) * 18_446_744_073_709_551_615.0) as u64
    }
}

// ---------------------------------------------------------------------------
// Public device discovery
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct DeviceList {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

pub fn list_devices() -> DeviceList {
    let host = cpal::default_host();
    DeviceList {
        inputs: host
            .input_devices()
            .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default(),
        outputs: host
            .output_devices()
            .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default(),
    }
}

pub fn default_names() -> (Option<String>, Option<String>) {
    let host = cpal::default_host();
    (
        host.default_input_device().and_then(|d| d.name().ok()),
        host.default_output_device().and_then(|d| d.name().ok()),
    )
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct OutState {
    chain: EqChain,
    generation: u64,
    resampler: Resampler,
}

pub struct AudioEngine {
    _input: cpal::Stream,
    _output: cpal::Stream,
    pub capture: String,
    pub render: String,
    pub capture_rate: u32,
    pub render_rate: u32,
    pub capture_channels: u16,
    pub render_channels: u16,
    error: Arc<Mutex<Option<String>>>,
    telemetry: Arc<Telemetry>,
}

impl AudioEngine {
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }

    pub fn telemetry(&self) -> TelemetrySnapshot {
        self.telemetry.snapshot()
    }

    pub fn telemetry_handle(&self) -> Arc<Telemetry> {
        self.telemetry.clone()
    }

    pub fn start(capture: &str, render: &str, shared: Arc<SharedParams>) -> Result<Self, String> {
        let host = cpal::default_host();

        let input_dev = find_device(&host, capture, true)
            .ok_or_else(|| format!("Capture device not found: {capture}"))?;
        let output_dev = find_device(&host, render, false)
            .ok_or_else(|| format!("Output device not found: {render}"))?;

        let input_support = input_dev
            .default_input_config()
            .map_err(|e| format!("Capture device has no usable input format: {e}"))?;
        let output_support = output_dev
            .default_output_config()
            .map_err(|e| format!("Output device has no usable output format: {e}"))?;

        let capture_rate = input_support.sample_rate().0;
        let render_rate = output_support.sample_rate().0;
        let capture_channels = input_support.channels();
        let render_channels = output_support.channels();

        let input_config: StreamConfig = input_support.config();
        let output_config: StreamConfig = output_support.config();

        let telemetry = Arc::new(Telemetry::new(8192));
        let queue = Arc::new(StereoQueue::new(
            (render_rate as usize / 5).max(2048),
            telemetry.clone(),
        ));
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let (generation, params) = shared.snapshot();
        let out_state = Arc::new(Mutex::new(OutState {
            chain: EqChain::new(render_rate as f32, &params),
            generation,
            resampler: Resampler::new(capture_rate as f32 / render_rate as f32),
        }));

        let input_stream = build_input(
            &input_dev,
            &input_config,
            input_support.sample_format(),
            capture_channels,
            queue.clone(),
            error.clone(),
            telemetry.clone(),
        )?;

        let output_stream = build_output(
            &output_dev,
            &output_config,
            output_support.sample_format(),
            render_channels,
            shared,
            queue,
            out_state,
            error.clone(),
            telemetry.clone(),
        )?;

        input_stream
            .play()
            .map_err(|e| format!("Could not start capture stream: {e}"))?;
        output_stream
            .play()
            .map_err(|e| format!("Could not start output stream: {e}"))?;

        Ok(Self {
            _input: input_stream,
            _output: output_stream,
            capture: capture.to_string(),
            render: render.to_string(),
            capture_rate,
            render_rate,
            capture_channels,
            render_channels,
            error,
            telemetry,
        })
    }
}

fn find_device(host: &cpal::Host, name: &str, input: bool) -> Option<cpal::Device> {
    if input {
        host.input_devices()
            .ok()?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
    } else {
        host.output_devices()
            .ok()?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
    }
}

fn make_error_cb(
    slot: Arc<Mutex<Option<String>>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err: cpal::StreamError| {
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(err.to_string());
        }
    }
}

fn build_input(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: u16,
    queue: Arc<StereoQueue>,
    error: Arc<Mutex<Option<String>>>,
    telemetry: Arc<Telemetry>,
) -> Result<cpal::Stream, String> {
    macro_rules! run {
        ($ty:ty) => {
            build_input_typed::<$ty>(device, config, channels, queue, error, telemetry)
        };
    }
    match format {
        SampleFormat::F32 => run!(f32),
        SampleFormat::F64 => run!(f64),
        SampleFormat::I16 => run!(i16),
        SampleFormat::U16 => run!(u16),
        SampleFormat::I8 => run!(i8),
        SampleFormat::U8 => run!(u8),
        SampleFormat::I32 => run!(i32),
        SampleFormat::U32 => run!(u32),
        SampleFormat::I64 => run!(i64),
        SampleFormat::U64 => run!(u64),
        other => Err(format!("Unsupported capture sample format: {other:?}")),
    }
}

fn build_input_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: u16,
    queue: Arc<StereoQueue>,
    error: Arc<Mutex<Option<String>>>,
    telemetry: Arc<Telemetry>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + ToF32 + Send + 'static,
{
    let channels = channels.max(1) as usize;
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                for frame in data.chunks(channels) {
                    let l = frame[0].to_f32();
                    let r = if frame.len() > 1 {
                        frame[1].to_f32()
                    } else {
                        l
                    };
                    telemetry.observe_input(l, r);
                    queue.push([l, r]);
                }
            },
            make_error_cb(error),
            None,
        )
        .map_err(|e| format!("Could not open capture stream: {e}"))
}

#[allow(clippy::too_many_arguments)]
fn build_output(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: u16,
    shared: Arc<SharedParams>,
    queue: Arc<StereoQueue>,
    state: Arc<Mutex<OutState>>,
    error: Arc<Mutex<Option<String>>>,
    telemetry: Arc<Telemetry>,
) -> Result<cpal::Stream, String> {
    macro_rules! run {
        ($ty:ty) => {
            build_output_typed::<$ty>(
                device, config, channels, shared, queue, state, error, telemetry,
            )
        };
    }
    match format {
        SampleFormat::F32 => run!(f32),
        SampleFormat::F64 => run!(f64),
        SampleFormat::I16 => run!(i16),
        SampleFormat::U16 => run!(u16),
        SampleFormat::I8 => run!(i8),
        SampleFormat::U8 => run!(u8),
        SampleFormat::I32 => run!(i32),
        SampleFormat::U32 => run!(u32),
        SampleFormat::I64 => run!(i64),
        SampleFormat::U64 => run!(u64),
        other => Err(format!("Unsupported output sample format: {other:?}")),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_output_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: u16,
    shared: Arc<SharedParams>,
    queue: Arc<StereoQueue>,
    state: Arc<Mutex<OutState>>,
    error: Arc<Mutex<Option<String>>>,
    telemetry: Arc<Telemetry>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromF32 + Send + 'static,
{
    let channels = channels.max(1) as usize;
    let zero = T::from_f32(0.0);

    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                for sample in data.iter_mut() {
                    *sample = zero;
                }

                let (generation, params) = shared.snapshot();
                let Ok(mut st) = state.try_lock() else {
                    return;
                };
                if st.generation != generation {
                    st.chain.update_params(&params);
                    st.generation = generation;
                }
                let Ok(mut guard) = queue.buf.try_lock() else {
                    return;
                };
                let mut reader = Reader { q: &mut guard };

                for frame in data.chunks_mut(channels) {
                    let input = st.resampler.next(|| reader.pop());
                    // Keep the calibration window recent during underruns too;
                    // otherwise an old non-silent input window could live forever.
                    telemetry.push_analysis_input(input.unwrap_or([0.0, 0.0]));
                    let (l, r) = match input {
                        Some([l, r]) => st.chain.process(l, r),
                        None => {
                            telemetry.record_underrun();
                            (0.0, 0.0)
                        }
                    };
                    telemetry.observe_output(l, r, st.chain.limiter_gain());
                    telemetry.push_spectrum(0.5 * (l + r));
                    match frame.len() {
                        1 => frame[0] = T::from_f32(0.5 * (l + r)),
                        _ => {
                            frame[0] = T::from_f32(l);
                            frame[1] = T::from_f32(r);
                            for extra in frame.iter_mut().skip(2) {
                                *extra = zero;
                            }
                        }
                    }
                }
            },
            make_error_cb(error),
            None,
        )
        .map_err(|e| format!("Could not open output stream: {e}"))
}
