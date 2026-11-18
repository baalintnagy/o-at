//! Real-time voice processing chain.
//!
//! Everything below [`stream`] runs at a fixed internal rate of 48 kHz in frames of
//! 480 samples (10 ms). That is the native format of RNNoise and DeepFilterNet and keeps
//! the partitioned echo canceller and the STFT stages aligned on the same block grid.

pub mod aec;
pub mod analyzer;
pub mod deep_filter;
pub mod delay;
pub mod denoise;
pub mod engine;
pub mod gate;
pub mod level;
pub mod leveler;
pub mod stream;
pub mod suppressor;
#[cfg(test)]
mod test_signal;

/// Internal processing rate in Hz.
pub const INTERNAL_RATE: u32 = 48_000;

/// Internal frame length in samples (10 ms at [`INTERNAL_RATE`]).
pub const FRAME: usize = 480;
