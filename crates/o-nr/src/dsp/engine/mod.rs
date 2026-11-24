//! The noise reduction engine: a short-time Fourier transform with Hann analysis and
//! synthesis windows overlapping by [`OVERLAP`], which attenuates each bin by the gain of
//! [`GainState`].
//!
//! The delay is the FFT size, constant for a resolution whether or not anything is
//! attenuated: the first finished hop of a frame is its oldest. With unity gains the output is the input delayed exactly.
//! Channels share one gain per bin when linked, computed from their mean power, which
//! keeps the stereo image steady.
//!
//! All buffers are allocated in [`Engine::configure`] for the largest resolution, so
//! switching resolution, learning and loading profiles are real-time safe.

use std::f32::consts::TAU;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use triple_buffer::Input;

use crate::dsp::gain::{GainShape, GainState};
use crate::dsp::level::{db_to_gain, db_to_power};
use crate::dsp::noise::{
    Learner, MIN_LEARN_S, MinimumTracker, SPREAD_WEIGHT, STATIONARY_SPREAD_DB,
};
use crate::dsp::profile::{LoadResult, ProfileStore};
use crate::dsp::{MAX_BINS, MAX_FFT_SIZE, OVERLAP};
use crate::telemetry::{NoiseSource, SPECTRUM_BANDS, Telemetry, band_hz};

mod publish;
mod settings;
#[cfg(test)]
mod tests;

use settings::NOMINAL_RATE;
pub use settings::{NoiseMode, Resolution, Settings};

struct Plan {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
}

/// Per-channel signal state.
struct Channel {
    /// The last FFT size of input samples.
    input: Vec<f32>,
    /// Overlap-add sum of synthesized frames.
    accumulator: Vec<f32>,
    /// One hop of finished output.
    output: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    /// Power spectral density of this channel's frame, for unlinked processing.
    power: Vec<f32>,
}

impl Channel {
    fn new() -> Self {
        Self {
            input: vec![0.0; MAX_FFT_SIZE],
            accumulator: vec![0.0; MAX_FFT_SIZE],
            output: vec![0.0; MAX_FFT_SIZE / OVERLAP],
            spectrum: vec![Complex::default(); MAX_BINS],
            power: vec![0.0; MAX_BINS],
        }
    }

    fn clear(&mut self) {
        self.input.fill(0.0);
        self.accumulator.fill(0.0);
        self.output.fill(0.0);
    }
}

/// Bin range of each display band.
#[derive(Clone, Copy)]
struct BandRange {
    first: usize,
    last: usize,
}

pub struct Engine {
    store: ProfileStore,
    publisher: Option<Input<Telemetry>>,
    configured: bool,
    rate: f32,

    plans: Vec<Plan>,
    scratch: Vec<Complex<f32>>,
    time: Vec<f32>,
    window: Vec<f32>,
    resolution: Resolution,
    size: usize,
    hop: usize,
    bins: usize,
    /// Write position in the channels' input, from the delay up to the FFT size.
    rover: usize,
    /// Converts squared magnitudes to power spectral density.
    power_scale: f32,
    /// Synthesis scale: inverse FFT normalization and window overlap.
    output_scale: f32,

    channels: Vec<Channel>,
    mean_power: Vec<f32>,
    output_power: Vec<f32>,
    trackers: Vec<MinimumTracker>,
    gains: Vec<GainState>,

    learner: Learner,
    learning: bool,
    /// A learned profile waits to be stored, with its bin spacing in Hz and bin count.
    pending_store: Option<(f32, usize)>,
    learned_level: Vec<f32>,
    learned_spread: Vec<f32>,
    profile_seen: Option<u32>,
    has_profile: bool,
    /// Profile noise per bin, raised where the learned noise varied.
    profile_noise: Vec<f32>,
    profile_spread: Vec<f32>,

    bands: [BandRange; SPECTRUM_BANDS],
    band_count: usize,
    /// Converts power spectral density to the level of a sine of the same peak.
    display_scale: f32,
    input_display: [f32; SPECTRUM_BANDS],
    output_display: [f32; SPECTRUM_BANDS],
    input_peak: f32,
    output_peak: f32,
    input_level: f32,
    output_level: f32,
    snapshot: Telemetry,
}

impl Engine {
    /// An engine that stores learned profiles in, and reads them from, `store`, and
    /// publishes its state to `publisher`. It passes audio through until configured.
    pub fn new(store: ProfileStore, publisher: Option<Input<Telemetry>>) -> Self {
        Self {
            store,
            publisher,
            configured: false,
            rate: NOMINAL_RATE,
            plans: Vec::new(),
            scratch: Vec::new(),
            time: Vec::new(),
            window: Vec::new(),
            resolution: Resolution::default(),
            size: 0,
            hop: 0,
            bins: 0,
            rover: 0,
            power_scale: 1.0,
            output_scale: 1.0,
            channels: Vec::new(),
            mean_power: Vec::new(),
            output_power: Vec::new(),
            trackers: Vec::new(),
            gains: Vec::new(),
            learner: Learner::new(0),
            learning: false,
            pending_store: None,
            learned_level: Vec::new(),
            learned_spread: Vec::new(),
            profile_seen: None,
            has_profile: false,
            profile_noise: Vec::new(),
            profile_spread: Vec::new(),
            bands: [BandRange { first: 0, last: 0 }; SPECTRUM_BANDS],
            band_count: 0,
            display_scale: 1.0,
            input_display: [0.0; SPECTRUM_BANDS],
            output_display: [0.0; SPECTRUM_BANDS],
            input_peak: 0.0,
            output_peak: 0.0,
            input_level: 0.0,
            output_level: 0.0,
            snapshot: Telemetry::default(),
        }
    }

    /// Allocates everything for `channels` channels at `rate` Hz. Not real-time safe.
    pub fn configure(&mut self, rate: f32, channels: usize, resolution: Resolution) {
        self.rate = rate;
        let mut planner = RealFftPlanner::<f32>::new();
        self.plans = Resolution::ALL
            .iter()
            .map(|resolution| {
                let size = resolution.fft_size(rate);
                Plan {
                    forward: planner.plan_fft_forward(size),
                    inverse: planner.plan_fft_inverse(size),
                }
            })
            .collect();
        let scratch = self
            .plans
            .iter()
            .map(|plan| {
                plan.forward
                    .get_scratch_len()
                    .max(plan.inverse.get_scratch_len())
            })
            .max()
            .unwrap_or_default();
        self.scratch = vec![Complex::default(); scratch];
        self.time = vec![0.0; MAX_FFT_SIZE];
        self.window = vec![0.0; MAX_FFT_SIZE];
        let channels = channels.max(1);
        self.channels = (0..channels).map(|_| Channel::new()).collect();
        self.mean_power = vec![0.0; MAX_BINS];
        self.output_power = vec![0.0; MAX_BINS];
        self.trackers = (0..channels)
            .map(|_| MinimumTracker::new(MAX_BINS))
            .collect();
        self.gains = (0..channels).map(|_| GainState::new(MAX_BINS)).collect();
        self.learner = Learner::new(MAX_BINS);
        self.learned_level = vec![0.0; MAX_BINS];
        self.learned_spread = vec![0.0; MAX_BINS];
        self.profile_noise = vec![0.0; MAX_BINS];
        self.profile_spread = vec![0.0; MAX_BINS];
        self.configured = true;
        self.set_resolution(resolution);
    }

    /// Delay in samples of the current resolution.
    pub fn latency_samples(&self) -> u32 {
        if self.configured { self.size as u32 } else { 0 }
    }

    /// Clears the signal state. Real-time safe.
    pub fn reset(&mut self) {
        if !self.configured {
            return;
        }
        self.channels.iter_mut().for_each(Channel::clear);
        self.trackers.iter_mut().for_each(MinimumTracker::reset);
        self.gains.iter_mut().for_each(GainState::reset);
        self.rover = self.size - self.hop;
        self.finish_learning();
        self.input_display.fill(0.0);
        self.output_display.fill(0.0);
        self.input_peak = 0.0;
        self.output_peak = 0.0;
        self.input_level = 0.0;
        self.output_level = 0.0;
    }

    /// Ends learning and, if the passage was long enough, queues the profile for storing.
    fn finish_learning(&mut self) {
        if !self.learning {
            return;
        }
        self.learning = false;
        let bins = self.learner.bins();
        let hop_s = self.hop as f32 / self.rate;
        if self.learner.frames() as f32 * hop_s >= MIN_LEARN_S && bins == self.bins {
            self.learner.finish(
                &mut self.learned_level[..bins],
                &mut self.learned_spread[..bins],
            );
            self.pending_store = Some((self.rate / self.size as f32, bins));
        }
    }

    fn set_resolution(&mut self, resolution: Resolution) {
        // A profile being learned keeps the bins it was learned with.
        self.finish_learning();
        self.resolution = resolution;
        self.size = resolution.fft_size(self.rate);
        self.hop = self.size / OVERLAP;
        self.bins = self.size / 2 + 1;
        let size = self.size;
        // Periodic Hann window, used for analysis and synthesis.
        for (index, value) in self.window[..size].iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (TAU * index as f32 / size as f32).cos();
        }
        let window_sum: f32 = self.window[..size].iter().sum();
        let window_energy: f32 = self.window[..size].iter().map(|value| value * value).sum();
        self.power_scale = 1.0 / window_energy;
        self.display_scale = window_energy / (0.5 * window_sum).powi(2);
        // The squared windows of overlapping frames sum to window_energy / hop.
        self.output_scale = self.hop as f32 / (window_energy * size as f32);

        let hop_s = self.hop as f32 / self.rate;
        let bins = self.bins;
        let bias = resolution.tracker_bias();
        self.trackers
            .iter_mut()
            .for_each(|tracker| tracker.configure(bins, hop_s, bias));
        self.profile_seen = None;
        self.update_bands();
        self.reset();
    }

    fn update_bands(&mut self) {
        let bin_hz = self.rate / self.size as f32;
        let nyquist = 0.5 * self.rate;
        let last_bin = self.bins - 1;
        self.band_count = 0;
        for (band, range) in self.bands.iter_mut().enumerate() {
            let low = band_hz(band as f32);
            let high = band_hz(band as f32 + 1.0);
            if low >= nyquist {
                *range = BandRange {
                    first: last_bin,
                    last: last_bin,
                };
                continue;
            }
            self.band_count = band + 1;
            let first = (low / bin_hz).ceil() as usize;
            let last = ((high / bin_hz).ceil() as usize).saturating_sub(1);
            *range = if first <= last {
                BandRange {
                    first: first.min(last_bin),
                    last: last.min(last_bin),
                }
            } else {
                let nearest = ((0.5 * (low + high) / bin_hz).round() as usize).min(last_bin);
                BandRange {
                    first: nearest,
                    last: nearest,
                }
            };
        }
    }

    /// Processes the channels in place. Real-time safe.
    pub fn process(&mut self, channels: &mut [&mut [f32]], settings: &Settings) {
        if !self.configured || channels.is_empty() {
            return;
        }
        if settings.resolution != self.resolution {
            self.set_resolution(settings.resolution);
        }
        let length = channels
            .iter()
            .map(|channel| channel.len())
            .min()
            .unwrap_or(0);
        // The input is written from here to the FFT size between frames.
        let start = self.size - self.hop;
        let mut offset = 0;
        while offset < length {
            let step = (self.size - self.rover).min(length - offset);
            let rover = self.rover;
            for (state, samples) in self.channels.iter_mut().zip(channels.iter_mut()) {
                let samples = &mut samples[offset..offset + step];
                state.input[rover..rover + step].copy_from_slice(samples);
                samples.copy_from_slice(&state.output[rover - start..rover - start + step]);
            }
            self.rover += step;
            offset += step;
            if self.rover == self.size {
                self.frame(settings);
                self.rover = start;
            }
        }
    }

    fn frame(&mut self, settings: &Settings) {
        let size = self.size;
        let hop = self.hop;
        let bins = self.bins;
        let hop_s = hop as f32 / self.rate;
        let bin_hz = self.rate / size as f32;
        let channel_count = self.channels.len();
        // Clones of the plans' handles (no allocation), so `self` stays free to borrow.
        let plan = &self.plans[self.resolution.index()];
        let (forward, inverse) = (plan.forward.clone(), plan.inverse.clone());

        // Analysis.
        let mut input_peak = 0.0_f32;
        self.mean_power[..bins].fill(0.0);
        let mean_scale = self.power_scale / channel_count as f32;
        for channel in &mut self.channels {
            input_peak = channel.input[size - hop..size]
                .iter()
                .fold(input_peak, |peak, sample| peak.max(sample.abs()));
            for ((time, input), window) in self.time[..size]
                .iter_mut()
                .zip(&channel.input[..size])
                .zip(&self.window[..size])
            {
                *time = input * window;
            }
            channel.input.copy_within(hop..size, 0);
            // The lengths match the plan, the only error realfft reports.
            let _ = forward.process_with_scratch(
                &mut self.time[..size],
                &mut channel.spectrum[..bins],
                &mut self.scratch,
            );
            for ((power, mean), bin) in channel.power[..bins]
                .iter_mut()
                .zip(&mut self.mean_power[..bins])
                .zip(&channel.spectrum[..bins])
            {
                let squared = bin.norm_sqr();
                *power = squared * self.power_scale;
                *mean += squared * mean_scale;
            }
        }

        // Learning.
        if settings.learn {
            if !self.learning {
                self.learner.start(bins);
                self.learning = true;
            }
            self.learner.add(&self.mean_power[..bins]);
        } else {
            self.finish_learning();
        }
        if let Some((learned_hz, learned_bins)) = self.pending_store {
            let stored = self.store.try_store(
                learned_hz,
                &self.learned_level[..learned_bins],
                &self.learned_spread[..learned_bins],
            );
            if stored {
                self.pending_store = None;
            }
        }

        // The stored profile, converted to the current bins.
        match self.store.load_into(
            &mut self.profile_seen,
            bin_hz,
            &mut self.profile_noise[..bins],
            &mut self.profile_spread[..bins],
        ) {
            LoadResult::Loaded => {
                self.has_profile = true;
                for (noise, spread) in self.profile_noise[..bins]
                    .iter_mut()
                    .zip(&self.profile_spread[..bins])
                {
                    let excess = (spread - STATIONARY_SPREAD_DB).max(0.0);
                    *noise *= db_to_power(SPREAD_WEIGHT * excess);
                }
            }
            LoadResult::Empty => self.has_profile = false,
            LoadResult::Unchanged | LoadResult::Busy => {}
        }

        // Gains.
        let shape = GainShape {
            threshold: db_to_power(settings.threshold_db),
            floor: db_to_gain(-settings.reduction_db.max(0.0)),
            release: (-hop_s / (0.001 * settings.release_ms).max(1e-4)).exp(),
            smoothing: 0.9 * settings.smoothing.clamp(0.0, 1.0),
        };
        let use_profile = settings.mode == NoiseMode::Profile && self.has_profile;
        let analyses = if settings.link { 1 } else { channel_count };
        for (index, (tracker, gains)) in self
            .trackers
            .iter_mut()
            .zip(&mut self.gains)
            .take(analyses)
            .enumerate()
        {
            let power = if settings.link {
                &self.mean_power[..bins]
            } else {
                &self.channels[index].power[..bins]
            };
            tracker.update(power);
            if self.learning {
                gains.pass(power);
            } else {
                let noise = if use_profile {
                    &self.profile_noise[..bins]
                } else {
                    tracker.estimate()
                };
                gains.update(power, noise, &shape);
            }
        }

        // Synthesis.
        let mut output_peak = 0.0_f32;
        self.output_power[..bins].fill(0.0);
        for (index, channel) in self.channels.iter_mut().enumerate() {
            let gains = self.gains[if settings.link { 0 } else { index }].gains();
            for ((bin, gain), output_power) in channel.spectrum[..bins]
                .iter_mut()
                .zip(&gains[..bins])
                .zip(&mut self.output_power[..bins])
            {
                let kept = *bin * *gain;
                *output_power += kept.norm_sqr() * mean_scale;
                *bin = if settings.listen_removed {
                    *bin - kept
                } else {
                    kept
                };
            }
            channel.spectrum[0].im = 0.0;
            channel.spectrum[bins - 1].im = 0.0;
            let _ = inverse.process_with_scratch(
                &mut channel.spectrum[..bins],
                &mut self.time[..size],
                &mut self.scratch,
            );
            let scale = self.output_scale;
            for ((sum, time), window) in channel.accumulator[..size]
                .iter_mut()
                .zip(&self.time[..size])
                .zip(&self.window[..size])
            {
                *sum += time * window * scale;
            }
            channel.output[..hop].copy_from_slice(&channel.accumulator[..hop]);
            channel.accumulator.copy_within(hop..size, 0);
            channel.accumulator[size - hop..size].fill(0.0);
            output_peak = channel.output[..hop]
                .iter()
                .fold(output_peak, |peak, sample| peak.max(sample.abs()));
        }

        let source = if self.learning {
            NoiseSource::Learning
        } else if settings.mode == NoiseMode::Adaptive {
            NoiseSource::Adaptive
        } else if use_profile {
            NoiseSource::Profile
        } else {
            NoiseSource::NoProfile
        };
        self.publish(source, input_peak, output_peak, hop_s, &shape, use_profile);
    }
}
