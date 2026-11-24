//! The engine's processing settings and frequency resolutions.

use crate::dsp::MAX_FFT_SIZE;

/// Sample rate at which the resolutions have their nominal FFT sizes, in Hz.
pub(super) const NOMINAL_RATE: f32 = 48_000.0;

/// Smallest FFT size in samples, for low sample rates.
const MIN_FFT_SIZE: usize = 256;

/// Frequency resolution: the FFT size, which trades delay for how finely frequencies are
/// told apart. Large sizes separate steady tones (hum, whine) from the signal; small
/// sizes keep transients sharp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resolution {
    Low,
    #[default]
    Medium,
    High,
    Max,
}

impl Resolution {
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Max];

    pub(super) fn index(self) -> usize {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Max => 3,
        }
    }

    /// Ratio of the mean noise power to the minimum the adaptive tracker finds at this
    /// resolution's frame rate, measured on white noise (see the tests). The frame rate
    /// stays about the same at every sample rate, as the FFT size scales with it.
    pub(super) fn tracker_bias(self) -> f32 {
        match self {
            Self::Low => 2.45,
            Self::Medium => 3.29,
            Self::High => 4.71,
            Self::Max => 6.88,
        }
    }

    /// FFT size in samples at 48 kHz: 1024, 2048, 4096 or 8192.
    pub fn nominal_size(self) -> usize {
        1024 << self.index()
    }

    /// FFT size in samples at `rate` Hz: the nominal size scaled with the rate to the
    /// nearest power of two, so the frequency resolution stays about the same.
    pub fn fft_size(self, rate: f32) -> usize {
        let scaled = self.nominal_size() as f32 * rate / NOMINAL_RATE;
        let size = 2.0_f32.powf(scaled.max(1.0).log2().round()) as usize;
        size.clamp(MIN_FFT_SIZE, MAX_FFT_SIZE)
    }
}

/// Where the noise estimate comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoiseMode {
    /// The stored noise profile; the adaptive tracker stands in while there is none.
    #[default]
    Profile,
    /// The continuous noise floor tracker.
    Adaptive,
}

/// Processing settings, read from the parameters for every block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub mode: NoiseMode,
    /// Learns a profile while true; stores it when it turns false.
    pub learn: bool,
    /// Largest attenuation in dB; 0 leaves the signal unchanged.
    pub reduction_db: f32,
    /// Offset of the noise estimate in dB: higher values treat more as noise.
    pub threshold_db: f32,
    /// Time in ms in which a falling gain mostly reaches its new value.
    pub release_ms: f32,
    /// Smoothing of the gains across frequency, 0 to 1.
    pub smoothing: f32,
    pub resolution: Resolution,
    /// One gain for all channels, from their mean power.
    pub link: bool,
    /// Outputs what is removed instead of what is kept.
    pub listen_removed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: NoiseMode::Profile,
            learn: false,
            reduction_db: 18.0,
            threshold_db: 3.0,
            release_ms: 150.0,
            smoothing: 0.3,
            resolution: Resolution::Medium,
            link: true,
            listen_removed: false,
        }
    }
}
