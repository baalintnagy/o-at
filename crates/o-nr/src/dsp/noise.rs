//! Noise estimation: learning a profile from a noise-only passage, and tracking the
//! noise floor continuously for the adaptive mode.

use crate::dsp::level::power_to_db;

/// Standard deviation in dB of one bin's level for stationary Gaussian noise. A learned
/// spread above it means the noise varies over time.
pub const STATIONARY_SPREAD_DB: f32 = 5.57;

/// How much of the spread above [`STATIONARY_SPREAD_DB`] raises the threshold.
pub const SPREAD_WEIGHT: f32 = 0.5;

/// Shortest passage a profile is learned from, in seconds.
pub const MIN_LEARN_S: f32 = 0.25;

/// Accumulates per-bin statistics while a profile is being learned.
pub struct Learner {
    power_sum: Vec<f64>,
    level_sum: Vec<f64>,
    level_square_sum: Vec<f64>,
    bins: usize,
    frames: u32,
}

impl Learner {
    pub fn new(max_bins: usize) -> Self {
        Self {
            power_sum: vec![0.0; max_bins],
            level_sum: vec![0.0; max_bins],
            level_square_sum: vec![0.0; max_bins],
            bins: 0,
            frames: 0,
        }
    }

    /// Starts learning `bins` bins.
    pub fn start(&mut self, bins: usize) {
        self.bins = bins;
        self.frames = 0;
        self.power_sum[..bins].fill(0.0);
        self.level_sum[..bins].fill(0.0);
        self.level_square_sum[..bins].fill(0.0);
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// Adds one frame's power spectral density.
    pub fn add(&mut self, power: &[f32]) {
        let bins = self.bins.min(power.len());
        let sums = self.power_sum[..bins]
            .iter_mut()
            .zip(&mut self.level_sum[..bins])
            .zip(&mut self.level_square_sum[..bins]);
        for (((power_sum, level_sum), square_sum), power) in sums.zip(power) {
            let level = f64::from(power_to_db(*power));
            *power_sum += f64::from(*power);
            *level_sum += level;
            *square_sum += level * level;
        }
        self.frames += 1;
    }

    /// Writes the mean level and its spread in dB of the learned bins.
    pub fn finish(&self, level_db: &mut [f32], spread_db: &mut [f32]) {
        let frames = f64::from(self.frames.max(1));
        let sums = self.power_sum[..self.bins]
            .iter()
            .zip(&self.level_sum[..self.bins])
            .zip(&self.level_square_sum[..self.bins]);
        for (((power_sum, level_sum), square_sum), (level, spread)) in
            sums.zip(level_db.iter_mut().zip(spread_db.iter_mut()))
        {
            *level = power_to_db((power_sum / frames) as f32);
            let mean = level_sum / frames;
            *spread = (square_sum / frames - mean * mean).max(0.0).sqrt() as f32;
        }
    }

    /// Number of bins being learned.
    pub fn bins(&self) -> usize {
        self.bins
    }
}

/// Time constant of the power smoothing before the minimum search, in seconds.
const TRACKER_SMOOTHING_S: f32 = 0.04;

/// Length of the minimum search, in seconds; the estimate follows rising noise within
/// about this time.
const TRACKER_WINDOW_S: f32 = 1.5;

/// The search is split into this many sub-windows.
const TRACKER_SUBWINDOWS: usize = 6;

/// Minimum statistics noise floor tracker (after Martin, 2001): the noise power is the
/// minimum of the smoothed power over the last [`TRACKER_WINDOW_S`] seconds, corrected for
/// the minimum's bias, which the caller measures for its frame rate. Speech and music rarely hold a bin's level up for that long, so
/// the minimum follows the noise floor beneath them.
pub struct MinimumTracker {
    smoothed: Vec<f32>,
    current: Vec<f32>,
    windows: Vec<f32>,
    estimate: Vec<f32>,
    bins: usize,
    alpha: f32,
    bias: f32,
    frames_per_window: u32,
    frame_in_window: u32,
    window_index: usize,
    filled: usize,
    started: bool,
}

impl MinimumTracker {
    pub fn new(max_bins: usize) -> Self {
        Self {
            smoothed: vec![0.0; max_bins],
            current: vec![f32::MAX; max_bins],
            windows: vec![f32::MAX; max_bins * TRACKER_SUBWINDOWS],
            estimate: vec![0.0; max_bins],
            bins: 0,
            alpha: 0.0,
            bias: 1.0,
            frames_per_window: 1,
            frame_in_window: 0,
            window_index: 0,
            filled: 0,
            started: false,
        }
    }

    /// Prepares tracking of `bins` bins for frames `hop_s` seconds apart. `bias` is the
    /// ratio of the mean noise power to the minimum the tracker finds at that frame rate.
    pub fn configure(&mut self, bins: usize, hop_s: f32, bias: f32) {
        self.bins = bins;
        self.bias = bias;
        self.alpha = (-hop_s / TRACKER_SMOOTHING_S).exp();
        let frames = TRACKER_WINDOW_S / TRACKER_SUBWINDOWS as f32 / hop_s;
        self.frames_per_window = (frames.round() as u32).max(1);
        self.reset();
    }

    pub fn reset(&mut self) {
        self.current.fill(f32::MAX);
        self.windows.fill(f32::MAX);
        self.estimate.fill(0.0);
        self.frame_in_window = 0;
        self.window_index = 0;
        self.filled = 0;
        self.started = false;
    }

    /// Noise power per bin.
    pub fn estimate(&self) -> &[f32] {
        &self.estimate[..self.bins]
    }

    /// Updates the estimate with one frame's power.
    pub fn update(&mut self, power: &[f32]) {
        let bins = self.bins.min(power.len());
        let alpha = self.alpha;
        let started = self.started;
        for ((smoothed, current), power) in self.smoothed[..bins]
            .iter_mut()
            .zip(&mut self.current[..bins])
            .zip(power)
        {
            *smoothed = if started {
                alpha * *smoothed + (1.0 - alpha) * power
            } else {
                *power
            };
            *current = current.min(*smoothed);
        }
        self.started = true;

        self.frame_in_window += 1;
        if self.frame_in_window >= self.frames_per_window {
            let start = self.window_index * self.bins;
            self.windows[start..start + bins].copy_from_slice(&self.current[..bins]);
            self.current[..bins].copy_from_slice(&self.smoothed[..bins]);
            self.window_index = (self.window_index + 1) % TRACKER_SUBWINDOWS;
            self.filled = (self.filled + 1).min(TRACKER_SUBWINDOWS);
            self.frame_in_window = 0;
        }

        let stride = self.bins;
        let filled = self.filled;
        let bias = self.bias;
        for (bin, (estimate, current)) in self.estimate[..bins]
            .iter_mut()
            .zip(&self.current[..bins])
            .enumerate()
        {
            let minimum = (0..filled)
                .map(|window| self.windows[window * stride + bin])
                .fold(*current, f32::min);
            *estimate = bias * minimum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Learner, STATIONARY_SPREAD_DB};

    /// Power of a complex Gaussian bin with unit mean: exponentially distributed.
    struct Exponential(u64);

    impl Exponential {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            let uniform = ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64;
            -uniform.ln() as f32
        }
    }

    #[test]
    fn learner_measures_level_and_stationary_spread() {
        let mut learner = Learner::new(64);
        learner.start(64);
        let mut random = Exponential(0x1234_5678);
        let mut frame = vec![0.0; 64];
        for _ in 0..4000 {
            frame
                .iter_mut()
                .for_each(|power| *power = 1e-6 * random.next());
            learner.add(&frame);
        }
        let mut level = vec![0.0; 64];
        let mut spread = vec![0.0; 64];
        learner.finish(&mut level, &mut spread);
        for (level, spread) in level.iter().zip(&spread) {
            assert!((level + 60.0).abs() < 0.3, "level {level}");
            assert!(
                (spread - STATIONARY_SPREAD_DB).abs() < 0.3,
                "spread {spread}"
            );
        }
    }
}
