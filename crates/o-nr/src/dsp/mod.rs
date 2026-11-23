//! Signal processing: a short-time Fourier transform engine that attenuates each
//! frequency bin by an estimate of how much of it is noise.

pub mod engine;
pub mod gain;
pub mod level;
pub mod noise;
pub mod profile;

/// Frames overlap by this factor: the hop is a quarter of the FFT size.
pub const OVERLAP: usize = 4;

/// Largest FFT size in samples (the finest resolution at 192 kHz).
pub const MAX_FFT_SIZE: usize = 32_768;

/// Bins of the largest FFT.
pub const MAX_BINS: usize = MAX_FFT_SIZE / 2 + 1;
