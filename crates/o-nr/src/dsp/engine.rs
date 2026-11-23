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
use crate::dsp::level::{db_to_gain, db_to_power, gain_to_db, power_to_db};
use crate::dsp::noise::{
    Learner, MIN_LEARN_S, MinimumTracker, SPREAD_WEIGHT, STATIONARY_SPREAD_DB,
};
use crate::dsp::profile::{LoadResult, ProfileStore};
use crate::dsp::{MAX_BINS, MAX_FFT_SIZE, OVERLAP};
use crate::telemetry::{NoiseSource, SPECTRUM_BANDS, Telemetry, band_hz};

/// Sample rate at which the resolutions have their nominal FFT sizes, in Hz.
const NOMINAL_RATE: f32 = 48_000.0;

/// Smallest FFT size in samples, for low sample rates.
const MIN_FFT_SIZE: usize = 256;

/// Time constant of the spectrum display smoothing, in seconds.
const DISPLAY_SMOOTHING_S: f32 = 0.08;

/// Time constant of the reduction meter, in seconds.
const REDUCTION_SMOOTHING_S: f32 = 0.25;

/// Time constant with which the peak meters fall, in seconds.
const PEAK_FALL_S: f32 = 0.3;

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

    fn index(self) -> usize {
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
    fn tracker_bias(self) -> f32 {
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

    fn publish(
        &mut self,
        source: NoiseSource,
        input_peak: f32,
        output_peak: f32,
        hop_s: f32,
        shape: &GainShape,
        use_profile: bool,
    ) {
        let bins = self.bins;
        let display = 1.0 - (-hop_s / DISPLAY_SMOOTHING_S).exp();
        let level = 1.0 - (-hop_s / REDUCTION_SMOOTHING_S).exp();
        let fall = (-hop_s / PEAK_FALL_S).exp();
        self.input_peak = input_peak.max(self.input_peak * fall);
        self.output_peak = output_peak.max(self.output_peak * fall);
        let input_sum: f32 = self.mean_power[..bins].iter().sum();
        let output_sum: f32 = self.output_power[..bins].iter().sum();
        self.input_level += level * (input_sum - self.input_level);
        self.output_level += level * (output_sum - self.output_level);

        let noise = if use_profile {
            &self.profile_noise[..bins]
        } else {
            self.trackers[0].estimate()
        };
        let snapshot = &mut self.snapshot;
        let band_power = |values: &[f32], range: &BandRange| {
            values[range.first..=range.last].iter().sum::<f32>()
                / (range.last - range.first + 1) as f32
        };
        for (band, range) in self.bands.iter().enumerate().take(self.band_count) {
            let input = band_power(&self.mean_power, range);
            let output = band_power(&self.output_power, range);
            self.input_display[band] += display * (input - self.input_display[band]);
            self.output_display[band] += display * (output - self.output_display[band]);
            snapshot.input_db[band] = power_to_db(self.input_display[band] * self.display_scale);
            snapshot.output_db[band] = power_to_db(self.output_display[band] * self.display_scale);
            snapshot.noise_db[band] =
                power_to_db(band_power(noise, range) * shape.threshold * self.display_scale);
        }
        snapshot.bands = self.band_count;
        snapshot.input_peak_db = gain_to_db(self.input_peak);
        snapshot.output_peak_db = gain_to_db(self.output_peak);
        snapshot.reduction_db = if self.input_level > 1e-20 {
            (power_to_db(self.output_level) - power_to_db(self.input_level)).min(0.0)
        } else {
            0.0
        };
        snapshot.source = source;
        snapshot.learned_s = if self.learning {
            self.learner.frames() as f32 * hop_s
        } else {
            0.0
        };
        snapshot.has_profile = self.has_profile;
        snapshot.host_rate = self.rate;
        snapshot.fft_size = self.size as u32;
        snapshot.latency_samples = self.size as u32;
        snapshot.sequence += 1;
        if let Some(publisher) = self.publisher.as_mut() {
            publisher.input_buffer_mut().clone_from(snapshot);
            publisher.publish();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::{Engine, NoiseMode, Resolution, Settings};
    use crate::dsp::level::power_to_db;
    use crate::dsp::profile::ProfileStore;

    /// Uniform white noise in −1..1.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        }

        fn signal(&mut self, length: usize, amplitude: f32) -> Vec<f32> {
            (0..length).map(|_| amplitude * self.next()).collect()
        }
    }

    fn engine(rate: f32, channels: usize, resolution: Resolution) -> Engine {
        let mut engine = Engine::new(ProfileStore::default(), None);
        engine.configure(rate, channels, resolution);
        engine
    }

    /// Processes mono `input` in blocks of `block` samples.
    fn run(engine: &mut Engine, input: &[f32], block: usize, settings: &Settings) -> Vec<f32> {
        let mut output = input.to_vec();
        for chunk in output.chunks_mut(block) {
            engine.process(&mut [chunk], settings);
        }
        output
    }

    fn rms_db(signal: &[f32]) -> f32 {
        power_to_db(signal.iter().map(|sample| sample * sample).sum::<f32>() / signal.len() as f32)
    }

    /// Level of the component at `frequency` Hz, by correlation, in dB.
    fn tone_db(signal: &[f32], frequency: f32, rate: f32) -> f32 {
        let (sine, cosine) =
            signal
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(sine, cosine), (n, x)| {
                    let phase = TAU * frequency * n as f32 / rate;
                    (sine + x * phase.sin(), cosine + x * phase.cos())
                });
        let amplitude = 2.0 * (sine * sine + cosine * cosine).sqrt() / signal.len() as f32;
        20.0 * amplitude.log10()
    }

    fn unity() -> Settings {
        Settings {
            reduction_db: 0.0,
            ..Settings::default()
        }
    }

    #[test]
    fn no_reduction_delays_the_input_exactly() {
        for rate in [22_050.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for resolution in Resolution::ALL {
                for (channels, block) in [(1, 333), (2, 64), (2, 1000)] {
                    let mut engine = engine(rate, channels, resolution);
                    let settings = Settings {
                        resolution,
                        ..unity()
                    };
                    let latency = engine.latency_samples() as usize;
                    let length = latency * 3 + 5_000;
                    let mut noise = Noise(7);
                    let inputs: Vec<Vec<f32>> =
                        (0..channels).map(|_| noise.signal(length, 0.5)).collect();
                    let mut outputs = inputs.clone();
                    let mut offset = 0;
                    while offset < length {
                        let end = (offset + block).min(length);
                        let mut slices: Vec<&mut [f32]> = outputs
                            .iter_mut()
                            .map(|channel| &mut channel[offset..end])
                            .collect();
                        engine.process(&mut slices, &settings);
                        offset = end;
                    }
                    for (input, output) in inputs.iter().zip(&outputs) {
                        let error = input[..length - latency]
                            .iter()
                            .zip(&output[latency..])
                            .skip(latency)
                            .map(|(a, b)| (a - b).abs())
                            .fold(0.0, f32::max);
                        assert!(
                            error < 1e-4,
                            "{rate} Hz {resolution:?} {channels} ch: error {error}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn reported_latency_matches_impulse_delay() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for resolution in Resolution::ALL {
                let mut engine = engine(rate, 1, resolution);
                // Reduction on: the delay must not depend on the gains.
                let settings = Settings {
                    resolution,
                    mode: NoiseMode::Adaptive,
                    ..Settings::default()
                };
                let length = rate as usize;
                let mut input = vec![0.0; length];
                input[length / 4] = 1.0;
                let output = run(&mut engine, &input, 256, &settings);
                let peak = output
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                    .map(|(index, _)| index)
                    .unwrap_or_default();
                assert_eq!(
                    peak - length / 4,
                    engine.latency_samples() as usize,
                    "{rate} Hz {resolution:?}"
                );
            }
        }
    }

    #[test]
    fn listen_removed_complements_the_output() {
        let rate = 48_000.0;
        let mut noise = Noise(3);
        let input: Vec<f32> = noise
            .signal(rate as usize * 2, 0.05)
            .iter()
            .enumerate()
            .map(|(n, x)| x + 0.3 * (TAU * 440.0 * n as f32 / rate).sin())
            .collect();
        let settings = Settings {
            mode: NoiseMode::Adaptive,
            ..Settings::default()
        };
        let kept = run(
            &mut engine(rate, 1, Resolution::Medium),
            &input,
            512,
            &settings,
        );
        let removed = run(
            &mut engine(rate, 1, Resolution::Medium),
            &input,
            512,
            &Settings {
                listen_removed: true,
                ..settings
            },
        );
        let latency = Resolution::Medium.fft_size(rate);
        let error = kept[latency..]
            .iter()
            .zip(&removed[latency..])
            .zip(&input)
            .map(|((kept, removed), input)| (kept + removed - input).abs())
            .fold(0.0, f32::max);
        assert!(error < 1e-4, "error {error}");
    }

    /// Learns a profile at `learn_at` from a second of noise, then processes noise and
    /// noise with a sine at `resolution`. Returns the noise and the sine levels removed
    /// in dB.
    fn learned_reduction(resolution: Resolution, learn_at: Resolution) -> (f32, f32) {
        let result = learned_reduction_inner(resolution, learn_at);
        eprintln!("{resolution:?} (learned {learn_at:?}): {result:?}");
        result
    }

    fn learned_reduction_inner(resolution: Resolution, learn_at: Resolution) -> (f32, f32) {
        let rate = 48_000.0;
        let mut engine = engine(rate, 1, learn_at);
        let mut noise = Noise(11);
        let learn = Settings {
            learn: true,
            resolution: learn_at,
            ..Settings::default()
        };
        run(&mut engine, &noise.signal(rate as usize, 0.1), 480, &learn);
        let settings = Settings {
            resolution,
            reduction_db: 30.0,
            ..Settings::default()
        };
        // Stopping learning stores the profile.
        run(
            &mut engine,
            &noise.signal(rate as usize / 2, 0.1),
            480,
            &settings,
        );
        assert!(engine.has_profile, "profile stored");

        let length = rate as usize * 3;
        let tail = length / 3;
        let background = noise.signal(length, 0.1);
        let noise_out = run(&mut engine, &background, 480, &settings);
        let noise_removed = rms_db(&background[tail..]) - rms_db(&noise_out[tail..]);

        let tone: Vec<f32> = background
            .iter()
            .enumerate()
            .map(|(n, x)| x + 0.2 * (TAU * 1_000.0 * n as f32 / rate).sin())
            .collect();
        let tone_out = run(&mut engine, &tone, 480, &settings);
        let tone_removed =
            tone_db(&tone[tail..], 1_000.0, rate) - tone_db(&tone_out[tail..], 1_000.0, rate);
        (noise_removed, tone_removed)
    }

    #[test]
    fn learned_profile_removes_noise_and_keeps_tones() {
        let results: Vec<_> = Resolution::ALL
            .iter()
            .map(|resolution| (resolution, learned_reduction(*resolution, *resolution)))
            .collect();
        for (resolution, (noise, tone)) in results {
            assert!(noise > 20.0, "{resolution:?}: noise reduced by {noise} dB");
            assert!(
                tone.abs() < 0.5,
                "{resolution:?}: tone changed by {tone} dB"
            );
        }
    }

    #[test]
    fn profiles_work_at_other_resolutions() {
        let (noise, tone) = learned_reduction(Resolution::High, Resolution::Low);
        assert!(noise > 20.0, "noise reduced by {noise} dB");
        assert!(tone.abs() < 0.5, "tone changed by {tone} dB");
    }

    #[test]
    fn adaptive_estimate_matches_the_noise_power() {
        let mut ratios = Vec::new();
        for (rate, resolution) in Resolution::ALL
            .iter()
            .flat_map(|resolution| [44_100.0, 48_000.0, 96_000.0].map(|rate| (rate, *resolution)))
        {
            let mut engine = engine(rate, 1, resolution);
            let settings = Settings {
                mode: NoiseMode::Adaptive,
                resolution,
                ..Settings::default()
            };
            let mut noise = Noise(5);
            let mut ratio_sum = 0.0;
            let mut count = 0;
            for block in 0..rate as usize * 8 / 480 {
                run(&mut engine, &noise.signal(480, 0.1), 480, &settings);
                if block > 400 {
                    let bins = engine.bins;
                    // Uniform noise of amplitude 0.1: power 0.1² / 3 per sample.
                    let expected = 0.01 / 3.0;
                    let estimate = engine.trackers[0].estimate()[4..bins - 4]
                        .iter()
                        .sum::<f32>()
                        / (bins - 8) as f32;
                    ratio_sum += estimate / expected;
                    count += 1;
                }
            }
            ratios.push((rate, resolution, ratio_sum / count as f32));
        }
        assert!(
            ratios
                .iter()
                .all(|(_, _, ratio)| (0.8..1.25).contains(ratio)),
            "estimates relative to the noise power: {ratios:?}"
        );
    }

    #[test]
    fn adaptive_mode_removes_steady_noise() {
        let rate = 44_100.0;
        let mut engine = engine(rate, 2, Resolution::Medium);
        let settings = Settings {
            mode: NoiseMode::Adaptive,
            link: false,
            ..Settings::default()
        };
        let mut noise = Noise(9);
        let length = rate as usize * 4;
        let mut left = noise.signal(length, 0.05);
        let mut right = noise.signal(length, 0.05);
        let original = left.clone();
        for (left, right) in left.chunks_mut(441).zip(right.chunks_mut(441)) {
            engine.process(&mut [left, right], &settings);
        }
        let removed = rms_db(&original[length / 2..]) - rms_db(&left[length / 2..]);
        assert!(removed > 12.0, "noise reduced by {removed} dB");
    }
}
