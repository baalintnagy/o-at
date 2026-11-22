//! Sample history, windowed FFTs of both channels and mappings from FFT bins to bands.

use std::f32::consts::TAU;
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};

use crate::settings::{FftSize, Signal, Window};

/// Level of empty bins and bands in dB.
pub const FLOOR_DB: f32 = -200.0;

/// Converts a power to dB, limited to [`FLOOR_DB`].
pub fn power_to_db(power: f32) -> f32 {
    if power <= 1e-20 {
        FLOOR_DB
    } else {
        (10.0 * power.log10()).max(FLOOR_DB)
    }
}

/// The most recent stereo frames, enough for the longest FFT.
pub struct History {
    frames: Vec<[f32; 2]>,
    /// Index of the next frame to write.
    head: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            frames: vec![[0.0; 2]; FftSize::MAX],
            head: 0,
        }
    }
}

impl History {
    pub fn push(&mut self, frame: [f32; 2]) {
        self.frames[self.head] = frame;
        self.head += 1;
        if self.head == self.frames.len() {
            self.head = 0;
        }
    }

    pub fn clear(&mut self) {
        self.frames.fill([0.0; 2]);
    }

    /// The last `count` frames, oldest first, as the two parts of the ring.
    pub fn latest_slices(&self, count: usize) -> (&[[f32; 2]], &[[f32; 2]]) {
        let count = count.min(self.frames.len());
        if count <= self.head {
            (&self.frames[self.head - count..self.head], &[])
        } else {
            let wrapped = count - self.head;
            (
                &self.frames[self.frames.len() - wrapped..],
                &self.frames[..self.head],
            )
        }
    }

    /// The last `count` frames, oldest first.
    pub fn latest(&self, count: usize) -> impl Iterator<Item = [f32; 2]> + '_ {
        let (older, newer) = self.latest_slices(count);
        older.iter().chain(newer).copied()
    }
}

/// Window coefficients for an FFT of `len` samples.
pub fn window_coefficients(window: Window, len: usize) -> Vec<f32> {
    let terms: &[f32] = match window {
        Window::Hann => &[0.5, 0.5],
        Window::BlackmanHarris => &[0.35875, 0.48829, 0.14128, 0.01168],
        Window::FlatTop => &[
            0.215_578_95,
            0.416_631_58,
            0.277_263_16,
            0.083_578_95,
            0.006_947_368,
        ],
    };
    (0..len)
        .map(|n| {
            let phase = TAU * n as f32 / len as f32;
            terms
                .iter()
                .enumerate()
                .map(|(order, term)| {
                    let sign = if order % 2 == 0 { 1.0 } else { -1.0 };
                    sign * term * (order as f32 * phase).cos()
                })
                .sum()
        })
        .collect()
}

/// Windowed FFTs of the left and right channel, scaled so a full-scale sine reads 0 dB.
pub struct FftTap {
    size: FftSize,
    window: Window,
    forward: Arc<dyn RealToComplex<f32>>,
    planner: RealFftPlanner<f32>,
    scratch: Vec<Complex32>,
    coefficients: Vec<f32>,
    /// Windowed input of the left and right channel.
    time: [Vec<f32>; 2],
    pub left: Vec<Complex32>,
    pub right: Vec<Complex32>,
    /// Power scale of a squared bin magnitude.
    normalization: f32,
}

impl FftTap {
    pub fn new(size: FftSize, window: Window) -> Self {
        let mut planner = RealFftPlanner::new();
        let forward = planner.plan_fft_forward(size.samples());
        let mut tap = Self {
            size,
            window,
            scratch: forward.make_scratch_vec(),
            forward,
            planner,
            coefficients: Vec::new(),
            time: [Vec::new(), Vec::new()],
            left: Vec::new(),
            right: Vec::new(),
            normalization: 1.0,
        };
        tap.rebuild();
        tap
    }

    /// Changes the size or window. Returns `true` if either changed.
    pub fn configure(&mut self, size: FftSize, window: Window) -> bool {
        if size == self.size && window == self.window {
            return false;
        }
        self.size = size;
        self.window = window;
        self.forward = self.planner.plan_fft_forward(size.samples());
        self.scratch = self.forward.make_scratch_vec();
        self.rebuild();
        true
    }

    fn rebuild(&mut self) {
        let len = self.size.samples();
        self.coefficients = window_coefficients(self.window, len);
        let sum: f32 = self.coefficients.iter().sum();
        self.normalization = 4.0 / (sum * sum);
        self.time = [vec![0.0; len], vec![0.0; len]];
        self.left = vec![Complex32::default(); self.bins()];
        self.right = vec![Complex32::default(); self.bins()];
    }

    pub fn samples(&self) -> usize {
        self.size.samples()
    }

    pub fn bins(&self) -> usize {
        self.size.samples() / 2 + 1
    }

    /// Transforms the latest frames of `history`.
    pub fn analyze(&mut self, history: &History) {
        let (older, newer) = history.latest_slices(self.size.samples());
        let [left, right] = &mut self.time;
        // Both channels in one pass over the history.
        for (((frame, weight), left), right) in older
            .iter()
            .chain(newer)
            .zip(&self.coefficients)
            .zip(left.iter_mut())
            .zip(right.iter_mut())
        {
            *left = frame[0] * weight;
            *right = frame[1] * weight;
        }
        // Buffer lengths match the plan, so the transforms cannot fail.
        let _ = self
            .forward
            .process_with_scratch(left, &mut self.left, &mut self.scratch);
        let _ = self
            .forward
            .process_with_scratch(right, &mut self.right, &mut self.scratch);
    }

    /// Power of `bin` in `signal`, where a full-scale sine reads one.
    pub fn power(&self, signal: Signal, bin: usize) -> f32 {
        let left = self.left[bin];
        let right = self.right[bin];
        let value = match signal {
            Signal::Left => left.norm_sqr(),
            Signal::Right => right.norm_sqr(),
            Signal::Mid => ((left + right) * 0.5).norm_sqr(),
            Signal::Side => ((left - right) * 0.5).norm_sqr(),
        };
        value * self.normalization
    }

    /// Powers of the two channels and the real part of their cross spectrum.
    pub fn stereo_power(&self, bin: usize) -> (f32, f32, f32) {
        let left = self.left[bin];
        let right = self.right[bin];
        (
            left.norm_sqr() * self.normalization,
            right.norm_sqr() * self.normalization,
            (left * right.conj()).re * self.normalization,
        )
    }
}

/// Bins covered by one band: an inclusive range, or a fractional bin to interpolate at
/// for bands narrower than a bin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BandBins {
    Range(usize, usize),
    Between(f32),
}

/// Maps FFT bins to bands between two frequencies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BandMap {
    pub bands: Vec<BandBins>,
}

impl BandMap {
    /// `count` bands from `low_hz` to `high_hz`, logarithmic or linear, for `bins` bins of
    /// `bin_hz` Hz.
    pub fn new(
        count: usize,
        (low_hz, high_hz): (f32, f32),
        log: bool,
        bins: usize,
        bin_hz: f32,
    ) -> Self {
        let edge = |position: f32| {
            if log {
                low_hz * (high_hz / low_hz).powf(position)
            } else {
                low_hz + (high_hz - low_hz) * position
            }
        };
        let last = bins.saturating_sub(1);
        let bands = (0..count)
            .map(|band| {
                let lower = edge(band as f32 / count as f32) / bin_hz;
                let upper = edge((band + 1) as f32 / count as f32) / bin_hz;
                let first = lower.ceil() as usize;
                let end = upper.floor() as usize;
                if end >= first && first <= last {
                    BandBins::Range(first, end.min(last))
                } else {
                    let centre = edge((band as f32 + 0.5) / count as f32) / bin_hz;
                    BandBins::Between(centre.clamp(0.0, last as f32))
                }
            })
            .collect();
        Self { bands }
    }

    /// Band levels in dB from per-bin powers: the strongest bin of wide bands, linear
    /// interpolation for narrow ones.
    pub fn levels_db(&self, power: impl Fn(usize) -> f32, output: &mut [f32]) {
        for (value, band) in output.iter_mut().zip(&self.bands) {
            let level = match *band {
                BandBins::Range(first, last) => (first..=last).map(&power).fold(0.0, f32::max),
                BandBins::Between(position) => {
                    let index = position.floor() as usize;
                    let fraction = position - index as f32;
                    let here = power(index);
                    if fraction > 0.0 {
                        here + (power(index + 1) - here) * fraction
                    } else {
                        here
                    }
                }
            };
            *value = power_to_db(level);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::{BandBins, BandMap, FftTap, History, power_to_db};

    #[test]
    fn history_returns_the_latest_frames_across_the_wrap() {
        let mut history = History::default();
        let len = history.frames.len();
        for n in 0..len + 10 {
            history.push([n as f32, 0.0]);
        }
        let latest: Vec<f32> = history.latest(20).map(|frame| frame[0]).collect();
        let expected: Vec<f32> = (len - 10..len + 10).map(|n| n as f32).collect();
        assert_eq!(latest, expected);
        let (older, newer) = history.latest_slices(5);
        assert_eq!((older.len(), newer.len()), (5, 0), "within the newest part");
        assert_eq!(
            history.latest(len + 5).count(),
            len,
            "at most the whole ring"
        );
    }
    use crate::settings::{FftSize, Signal, Window};

    #[test]
    fn full_scale_sines_read_zero_db_with_every_window() {
        for window in [Window::Hann, Window::BlackmanHarris, Window::FlatTop] {
            let mut history = History::default();
            let rate = 48_000.0;
            let size = FftSize::S4k;
            // A sine exactly on bin 100.
            let frequency = 100.0 * rate / size.samples() as f32;
            for n in 0..size.samples() {
                let value = (TAU * frequency * n as f32 / rate).sin();
                history.push([value, -value]);
            }
            let mut tap = FftTap::new(size, window);
            tap.analyze(&history);
            let left = power_to_db(tap.power(Signal::Left, 100));
            let mid = power_to_db(tap.power(Signal::Mid, 100));
            let side = power_to_db(tap.power(Signal::Side, 100));
            assert!(left.abs() < 0.05, "{window:?} left {left:.3} dB");
            assert!(side.abs() < 0.05, "{window:?} side {side:.3} dB");
            assert!(mid < -100.0, "{window:?} anti-phase mid {mid:.1} dB");
        }
    }

    #[test]
    fn band_maps_cover_wide_and_narrow_bands() {
        let map = BandMap::new(64, (20.0, 20_000.0), true, 513, 46.875);
        assert!(matches!(map.bands[0], BandBins::Between(_)));
        assert!(
            matches!(map.bands[63], BandBins::Range(first, last) if first < last && last <= 512)
        );
        let mut levels = [0.0; 64];
        map.levels_db(|_| 1.0, &mut levels);
        assert!(levels.iter().all(|level| level.abs() < 1e-4));
    }
}
