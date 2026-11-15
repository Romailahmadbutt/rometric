//! Core of Rometric: AutoEQ profile data and the DSP chain.
//!
//! This library is deliberately free of GUI and audio-device code so it can be
//! built and tested on any platform.

pub mod analysis;
pub mod autoeq;
pub mod branding;
pub mod catalog;
pub mod config;
pub mod dsp;
pub mod product;
pub mod resample;
pub mod telemetry;
