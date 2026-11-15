//! Stereo sample-rate conversion by linear interpolation.
//!
//! The capture and render devices do not always share a sample rate. Audio is
//! pulled from the capture queue at a fractional step; the resampler keeps one
//! frame of look-ahead so the first output frame is not duplicated.

#[derive(Clone, Copy, Debug)]
pub struct Resampler {
    s0: [f32; 2],
    s1: [f32; 2],
    frac: f32,
    step: f32,
    primed: bool,
}

impl Resampler {
    /// `step` is `capture_rate / render_rate`.
    pub fn new(step: f32) -> Self {
        let step = if step.is_finite() && step > 0.0 {
            step
        } else {
            1.0
        };
        Self {
            s0: [0.0; 2],
            s1: [0.0; 2],
            frac: 0.0,
            step,
            primed: false,
        }
    }

    pub fn step(&self) -> f32 {
        self.step
    }

    pub fn reset(&mut self) {
        self.primed = false;
        self.frac = 0.0;
    }

    /// Produces the next output frame. Returns `None` when the source runs dry;
    /// the next call re-primes from fresh input.
    pub fn next(&mut self, mut source: impl FnMut() -> Option<[f32; 2]>) -> Option<[f32; 2]> {
        if !self.primed {
            let first = source()?;
            let second = source().unwrap_or(first);
            self.s0 = first;
            self.s1 = second;
            self.frac = 0.0;
            self.primed = true;
            return Some(first);
        }

        self.frac += self.step;
        while self.frac >= 1.0 {
            self.s0 = self.s1;
            match source() {
                Some(frame) => self.s1 = frame,
                None => {
                    self.primed = false;
                    self.frac = 0.0;
                    return None;
                }
            }
            self.frac -= 1.0;
        }

        let t = self.frac.clamp(0.0, 1.0);
        Some([
            self.s0[0] + (self.s1[0] - self.s0[0]) * t,
            self.s0[1] + (self.s1[1] - self.s0[1]) * t,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_from(values: &[f32]) -> impl FnMut() -> Option<[f32; 2]> + '_ {
        let mut i = 0;
        move || {
            let v = *values.get(i)?;
            i += 1;
            Some([v, v])
        }
    }

    #[test]
    fn unity_step_is_passthrough_without_repeats() {
        let mut r = Resampler::new(1.0);
        // The resampler keeps one frame of look-ahead, so the final source
        // frame is never emitted at end-of-stream.
        let mut src = source_from(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let mut out = Vec::new();
        while let Some(frame) = r.next(&mut src) {
            out.push(frame[0]);
        }
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn upsampling_interpolates() {
        let mut r = Resampler::new(0.5);
        let mut src = source_from(&[0.0, 1.0, 0.0, 1.0]);
        let out: Vec<f32> = std::iter::from_fn(|| r.next(&mut src))
            .map(|f| f[0])
            .collect();
        assert!(out.len() >= 4);
        // First frame is exact, then values land between neighbours.
        assert_eq!(out[0], 0.0);
        assert!((0.0..=1.0).contains(&out[1]));
    }

    #[test]
    fn downsampling_consumes_multiple_frames() {
        let mut r = Resampler::new(2.0);
        let mut src = source_from(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let first = r.next(&mut src).unwrap()[0];
        let second = r.next(&mut src).unwrap()[0];
        assert_eq!(first, 1.0);
        assert_eq!(second, 3.0);
    }

    #[test]
    fn underrun_reports_none_and_reprimes() {
        let values = std::cell::RefCell::new(vec![1.0, 2.0, 3.0]);
        let mut r = Resampler::new(1.0);
        let mut src = || {
            let mut v = values.borrow_mut();
            if v.is_empty() {
                None
            } else {
                let x = v.remove(0);
                Some([x, x])
            }
        };
        assert_eq!(r.next(&mut src).unwrap()[0], 1.0);
        assert_eq!(r.next(&mut src).unwrap()[0], 2.0);
        assert!(r.next(&mut src).is_none());
        // Feed it again and it resumes.
        values.borrow_mut().push(9.0);
        assert_eq!(r.next(&mut src).unwrap()[0], 9.0);
    }

    #[test]
    fn invalid_step_defaults_to_unity() {
        assert_eq!(Resampler::new(0.0).step(), 1.0);
        assert_eq!(Resampler::new(f32::NAN).step(), 1.0);
    }
}
