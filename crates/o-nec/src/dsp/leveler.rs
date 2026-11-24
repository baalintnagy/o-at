//! Speech leveler: evens out the level of speech, raising quiet or distant talk and
//! lowering loud talk, and limits the peaks of sudden shouts.
//!
//! The leveler has two parts. A gain control measures the speaker's level only in frames
//! that the voice detector marks as speech, and moves the gain slowly towards the
//! target, within the boost and cut limits. In pauses it holds the gain, so noise and
//! breathing between words are not raised. A peak limiter with [`LEVELER_DELAY`] samples
//! of lookahead then catches the onsets of shouts before the gain control has reacted.
//!
//! The stage always delays by [`LEVELER_DELAY`] samples, also when it is disabled.

use crate::dsp::level::{db_to_gain, gain_to_db, mean_power, one_pole_coefficient, power_to_db};
use crate::dsp::{FRAME, INTERNAL_RATE};

/// Lookahead of the peak limiter in samples (2 ms at 48 kHz).
pub const LEVELER_DELAY: usize = 96;

/// Samples covered by the limiter's minimum and smoothing windows.
const WINDOW: usize = LEVELER_DELAY + 1;

/// Voice probability (0..1) from which a frame counts as speech.
const SPEECH_PROBABILITY: f32 = 0.5;

/// Frames quieter than this mean power in dBFS never count as speech.
const SPEECH_FLOOR_DB: f32 = -65.0;

/// Largest deviation of one frame from the level estimate that is taken into account,
/// in dB, so that single syllables or clicks move the estimate less.
const OUTLIER_DB: f32 = 12.0;

/// Getting louder is followed faster than getting quieter, by this factor of the time.
const LOUDER_TIME_FACTOR: f32 = 0.35;

/// The estimate starts with this time constant in ms and slows down to the speed's time
/// constant as speech accumulates, so the first words are leveled quickly.
const WARM_UP_MS: f32 = 100.0;

/// Largest gain change per frame in dB (50 dB/s).
const GAIN_SLEW_DB: f32 = 0.5;

/// The limiter ceiling sits this far above the target, in dB, leaving room for the
/// natural peaks of speech at the target level.
const PEAK_HEADROOM_DB: f32 = 16.0;

/// Highest limiter ceiling in dBFS.
const MAX_CEILING_DB: f32 = -1.0;

/// Limiter release time in ms.
const LIMITER_RELEASE_MS: f32 = 80.0;

const FRAME_MS: f32 = 1000.0 * FRAME as f32 / INTERNAL_RATE as f32;
const FRAMES_PER_SECOND: f32 = INTERNAL_RATE as f32 / FRAME as f32;

/// How quickly the gain control follows the speaker's level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelerSpeed {
    Slow,
    Medium,
    Fast,
}

impl LevelerSpeed {
    /// Time constant of the speech level estimate in ms of speech.
    fn time_ms(self) -> f32 {
        match self {
            LevelerSpeed::Slow => 3000.0,
            LevelerSpeed::Medium => 1500.0,
            LevelerSpeed::Fast => 600.0,
        }
    }
}

/// Per-frame leveler settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelerSettings {
    pub enabled: bool,
    /// Speech level to reach, as the mean power of speech frames in dBFS.
    pub target_db: f32,
    /// Share of the difference between speech level and target that is corrected (0..1).
    pub amount: f32,
    /// Largest gain in dB.
    pub max_boost_db: f32,
    /// Largest attenuation in dB (positive).
    pub max_cut_db: f32,
    pub speed: LevelerSpeed,
}

impl Default for LevelerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            target_db: -20.0,
            amount: 0.8,
            max_boost_db: 12.0,
            max_cut_db: 12.0,
            speed: LevelerSpeed::Medium,
        }
    }
}

pub struct Leveler {
    /// Estimated speech level in dBFS, unknown until the first speech frame.
    level_db: Option<f32>,
    /// Speech heard since the estimate started, in ms.
    speech_ms: f32,
    gain_db: f32,
    limiter: PeakLimiter,
}

impl Default for Leveler {
    fn default() -> Self {
        Self::new()
    }
}

impl Leveler {
    pub fn new() -> Self {
        Self {
            level_db: None,
            speech_ms: 0.0,
            gain_db: 0.0,
            limiter: PeakLimiter::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Gain of the gain control in dB.
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// Attenuation of the peak limiter during the last frame in dB (zero or negative).
    pub fn limiter_db(&self) -> f32 {
        gain_to_db(self.limiter.lowest_gain)
    }

    /// Estimated speech level in dBFS, if speech was heard.
    pub fn speech_level_db(&self) -> Option<f32> {
        self.level_db
    }

    /// Levels one frame in place. `voice_probability` (0..1) must describe this frame.
    /// The output is delayed by [`LEVELER_DELAY`] samples.
    pub fn process(
        &mut self,
        frame: &mut [f32],
        settings: &LevelerSettings,
        voice_probability: f32,
    ) {
        debug_assert_eq!(frame.len(), FRAME);
        let frame_db = power_to_db(mean_power(frame));
        if settings.enabled && voice_probability >= SPEECH_PROBABILITY && frame_db > SPEECH_FLOOR_DB
        {
            self.track(frame_db, settings.speed);
        }

        let wanted_db = match self.level_db {
            Some(level_db) if settings.enabled => ((settings.target_db - level_db)
                * settings.amount)
                .clamp(-settings.max_cut_db, settings.max_boost_db),
            _ => 0.0,
        };
        let start = db_to_gain(self.gain_db);
        self.gain_db += (wanted_db - self.gain_db).clamp(-GAIN_SLEW_DB, GAIN_SLEW_DB);
        let step = (db_to_gain(self.gain_db) - start) / FRAME as f32;
        for (index, sample) in frame.iter_mut().enumerate() {
            *sample *= start + step * (index + 1) as f32;
        }

        let ceiling = if settings.enabled {
            db_to_gain((settings.target_db + PEAK_HEADROOM_DB).min(MAX_CEILING_DB))
        } else {
            f32::INFINITY
        };
        self.limiter.process(frame, ceiling);
    }

    fn track(&mut self, frame_db: f32, speed: LevelerSpeed) {
        let Some(level_db) = self.level_db.as_mut() else {
            self.level_db = Some(frame_db);
            return;
        };
        let deviation = (frame_db - *level_db).clamp(-OUTLIER_DB, OUTLIER_DB);
        let time_ms = speed.time_ms().min(WARM_UP_MS + self.speech_ms);
        let time_ms = if deviation > 0.0 {
            time_ms * LOUDER_TIME_FACTOR
        } else {
            time_ms
        };
        let coefficient = one_pole_coefficient(time_ms, FRAMES_PER_SECOND);
        *level_db += (1.0 - coefficient) * deviation;
        self.speech_ms = (self.speech_ms + FRAME_MS).min(speed.time_ms());
    }
}

/// Lookahead peak limiter. For each sample it takes the gain that keeps the sample below
/// the ceiling, holds the smallest such gain over the lookahead window, releases it
/// slowly and averages it over the window again. The averaged gain therefore reaches
/// every peak's gain by the time the delayed peak is output, without steps.
struct PeakLimiter {
    audio: [f32; LEVELER_DELAY],
    audio_position: usize,
    minimum: SlidingMinimum,
    released: f32,
    smoothing: [f32; WINDOW],
    smoothing_position: usize,
    smoothing_sum: f64,
    release: f32,
    /// Smallest gain applied in the last frame.
    lowest_gain: f32,
}

impl PeakLimiter {
    fn new() -> Self {
        Self {
            audio: [0.0; LEVELER_DELAY],
            audio_position: 0,
            minimum: SlidingMinimum::new(),
            released: 1.0,
            smoothing: [1.0; WINDOW],
            smoothing_position: 0,
            smoothing_sum: WINDOW as f64,
            release: one_pole_coefficient(LIMITER_RELEASE_MS, INTERNAL_RATE as f32),
            lowest_gain: 1.0,
        }
    }

    /// Limits `frame` in place to `ceiling` (linear) and delays it by the lookahead.
    fn process(&mut self, frame: &mut [f32], ceiling: f32) {
        let mut lowest_gain: f32 = 1.0;
        for sample in frame.iter_mut() {
            let magnitude = sample.abs();
            let required = if magnitude > ceiling {
                ceiling / magnitude
            } else {
                1.0
            };
            let held = self.minimum.push(required);
            self.released = if held < self.released {
                held
            } else {
                held + self.release * (self.released - held)
            };

            let oldest = &mut self.smoothing[self.smoothing_position];
            self.smoothing_sum += f64::from(self.released) - f64::from(*oldest);
            *oldest = self.released;
            self.smoothing_position = (self.smoothing_position + 1) % WINDOW;
            let gain = ((self.smoothing_sum / WINDOW as f64) as f32).min(1.0);
            lowest_gain = lowest_gain.min(gain);

            let delayed = self.audio[self.audio_position];
            self.audio[self.audio_position] = *sample;
            self.audio_position = (self.audio_position + 1) % LEVELER_DELAY;
            *sample = delayed * gain;
        }
        self.lowest_gain = lowest_gain;
    }
}

/// Minimum of the last [`WINDOW`] values, kept as a monotonic queue in fixed storage.
struct SlidingMinimum {
    values: [f32; WINDOW],
    indices: [u64; WINDOW],
    head: usize,
    len: usize,
    next_index: u64,
}

impl SlidingMinimum {
    fn new() -> Self {
        Self {
            values: [0.0; WINDOW],
            indices: [0; WINDOW],
            head: 0,
            len: 0,
            next_index: 0,
        }
    }

    /// Adds a value and returns the minimum of the window ending with it.
    fn push(&mut self, value: f32) -> f32 {
        let index = self.next_index;
        self.next_index += 1;
        // Expire values that leave the window, keeping room for the new one.
        while self.len > 0 && self.indices[self.head] + WINDOW as u64 <= index {
            self.head = (self.head + 1) % WINDOW;
            self.len -= 1;
        }
        // Values not smaller than the new one can never be the minimum again.
        while self.len > 0 && self.values[(self.head + self.len - 1) % WINDOW] >= value {
            self.len -= 1;
        }
        let back = (self.head + self.len) % WINDOW;
        self.values[back] = value;
        self.indices[back] = index;
        self.len += 1;
        self.values[self.head]
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{SQRT_2, TAU};

    use super::{LEVELER_DELAY, Leveler, LevelerSettings, LevelerSpeed, SlidingMinimum};
    use crate::dsp::FRAME;
    use crate::dsp::level::{db_to_gain, mean_power, peak, power_to_db};

    fn settings() -> LevelerSettings {
        LevelerSettings {
            enabled: true,
            target_db: -20.0,
            amount: 1.0,
            max_boost_db: 30.0,
            max_cut_db: 30.0,
            speed: LevelerSpeed::Fast,
        }
    }

    /// Runs `seconds` of a 220 Hz sine with mean power `level_db` and returns the output
    /// level of the last second in dBFS.
    fn run(
        leveler: &mut Leveler,
        settings: &LevelerSettings,
        level_db: f32,
        voice: f32,
        seconds: usize,
    ) -> f32 {
        let amplitude = SQRT_2 * db_to_gain(level_db);
        let frames = seconds * 100;
        let mut output = Vec::with_capacity(frames * FRAME);
        for frame_index in 0..frames {
            let mut frame: Vec<f32> = (0..FRAME)
                .map(|n| {
                    amplitude * (TAU * 220.0 * (frame_index * FRAME + n) as f32 / 48_000.0).sin()
                })
                .collect();
            leveler.process(&mut frame, settings, voice);
            output.extend_from_slice(&frame);
        }
        power_to_db(mean_power(&output[output.len() - 100 * FRAME..]))
    }

    #[test]
    fn raises_quiet_and_lowers_loud_speech_to_the_target() {
        for level_db in [-42.0, -30.0, -12.0] {
            let mut leveler = Leveler::new();
            let output_db = run(&mut leveler, &settings(), level_db, 0.9, 6);
            assert!(
                (output_db - -20.0).abs() < 1.0,
                "{level_db} dB in, {output_db} dB out"
            );
        }
    }

    #[test]
    fn amount_and_limits_bound_the_correction() {
        let mut leveler = Leveler::new();
        let half = LevelerSettings {
            amount: 0.5,
            ..settings()
        };
        let output_db = run(&mut leveler, &half, -40.0, 0.9, 6);
        assert!(
            (output_db - -30.0).abs() < 1.0,
            "half of 20 dB: {output_db}"
        );

        let mut leveler = Leveler::new();
        let limited = LevelerSettings {
            max_boost_db: 6.0,
            ..settings()
        };
        let output_db = run(&mut leveler, &limited, -40.0, 0.9, 6);
        assert!((output_db - -34.0).abs() < 0.5, "boost limit: {output_db}");
    }

    #[test]
    fn holds_the_gain_in_pauses() {
        let mut leveler = Leveler::new();
        run(&mut leveler, &settings(), -35.0, 0.9, 6);
        let gain_db = leveler.gain_db();
        assert!((gain_db - 15.0).abs() < 1.0, "gain {gain_db}");
        // Quiet noise without speech must neither be leveled up nor change the gain.
        run(&mut leveler, &settings(), -60.0, 0.05, 4);
        assert!((leveler.gain_db() - gain_db).abs() < 0.01);
    }

    #[test]
    fn limits_the_onset_of_a_shout() {
        let mut leveler = Leveler::new();
        run(&mut leveler, &settings(), -20.0, 0.9, 4);
        // A sudden shout 18 dB louder: the gain control has not reacted yet.
        let amplitude = SQRT_2 * db_to_gain(-2.0);
        let mut loudest: f32 = 0.0;
        let mut deepest_limiting: f32 = 0.0;
        for frame_index in 0..20 {
            let mut frame: Vec<f32> = (0..FRAME)
                .map(|n| {
                    amplitude * (TAU * 220.0 * (frame_index * FRAME + n) as f32 / 48_000.0).sin()
                })
                .collect();
            leveler.process(&mut frame, &settings(), 0.9);
            loudest = loudest.max(peak(&frame));
            deepest_limiting = deepest_limiting.min(leveler.limiter_db());
        }
        let ceiling = db_to_gain(-4.0);
        assert!(
            loudest <= ceiling * 1.001,
            "peak {loudest} above ceiling {ceiling}"
        );
        assert!(deepest_limiting < -1.0, "limiter did not act");
    }

    #[test]
    fn disabled_leveler_is_a_pure_delay() {
        let mut leveler = Leveler::new();
        let disabled = LevelerSettings::default();
        let input: Vec<f32> = (0..FRAME * 4)
            .map(|n| (n as f32 * 0.05).sin() * 0.9)
            .collect();
        let mut output = input.clone();
        for frame in output.chunks_mut(FRAME) {
            leveler.process(frame, &disabled, 0.9);
        }
        assert!(output[..LEVELER_DELAY].iter().all(|sample| *sample == 0.0));
        for (delayed, original) in output[LEVELER_DELAY..].iter().zip(&input) {
            assert!((delayed - original).abs() < 1.0e-6);
        }
    }

    #[test]
    fn sliding_minimum_matches_a_direct_search() {
        let mut minimum = SlidingMinimum::new();
        let values: Vec<f32> = (0..1_000)
            .map(|n| ((n * 7919) % 211) as f32 / 211.0)
            .collect();
        for (index, value) in values.iter().enumerate() {
            let expected = values[index.saturating_sub(LEVELER_DELAY)..=index]
                .iter()
                .fold(f32::INFINITY, |current, value| current.min(*value));
            assert_eq!(minimum.push(*value), expected, "index {index}");
        }
    }
}
