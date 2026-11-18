//! Loudness after ITU-R BS.1770-4 and EBU R 128 (Tech 3341 and 3342): momentary,
//! short-term and integrated loudness, loudness range and true peak.

use std::f64::consts::PI;

/// Loudness of digital silence and the absolute gate in LUFS.
pub const ABSOLUTE_GATE_LUFS: f32 = -70.0;
/// Shown in place of a loudness that cannot be measured yet.
pub const SILENCE_LUFS: f32 = f32::NEG_INFINITY;

/// Relative gates in LU below the ungated mean.
const INTEGRATED_GATE_LU: f64 = -10.0;
const RANGE_GATE_LU: f64 = -20.0;

/// 100 ms blocks per momentary (400 ms) and short-term (3 s) window.
const MOMENTARY_BLOCKS: usize = 4;
const SHORT_BLOCKS: usize = 30;

/// Gating histogram resolution in LU, and its range in LUFS.
const HISTOGRAM_STEP_LU: f64 = 0.01;
const HISTOGRAM_MAX_LUFS: f64 = 10.0;
const HISTOGRAM_BINS: usize =
    ((HISTOGRAM_MAX_LUFS - ABSOLUTE_GATE_LUFS as f64) / HISTOGRAM_STEP_LU) as usize;

/// Interpolation taps per output phase of the true-peak oversampler.
const TRUE_PEAK_TAPS: usize = 16;

/// Converts a K-weighted mean square to LUFS.
pub fn power_to_lufs(power: f64) -> f32 {
    if power <= 0.0 {
        SILENCE_LUFS
    } else {
        (-0.691 + 10.0 * power.log10()) as f32
    }
}

/// Converts a linear peak to dB, with silence as negative infinity.
pub fn peak_to_db(peak: f32) -> f32 {
    if peak <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * peak.log10()
    }
}

/// Direct form I biquad in double precision.
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self {
            b,
            a,
            ..Self::default()
        }
    }

    fn process(&mut self, input: f64) -> f64 {
        let output = self.b[0] * input + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [input, self.x[0]];
        self.y = [output, self.y[0]];
        output
    }
}

/// The K-weighting filter: a high shelf modelling the head, then the RLB high-pass. The
/// coefficients follow BS.1770 at any rate (as derived by libebur128).
#[derive(Clone, Copy, Debug)]
struct KWeighting {
    shelf: Biquad,
    high_pass: Biquad,
}

impl KWeighting {
    fn new(sample_rate: f64) -> Self {
        let f0 = 1_681.974_450_955_533;
        let gain_db = 3.999_843_853_973_347;
        let q = 0.707_175_236_955_419_6;
        let k = (PI * f0 / sample_rate).tan();
        let vh = 10.0_f64.powf(gain_db / 20.0);
        let vb = vh.powf(0.499_666_774_154_541_6);
        let a0 = 1.0 + k / q + k * k;
        let shelf = Biquad::new(
            [
                (vh + vb * k / q + k * k) / a0,
                2.0 * (k * k - vh) / a0,
                (vh - vb * k / q + k * k) / a0,
            ],
            [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        );
        let f0 = 38.135_470_876_024_44;
        let q = 0.500_327_037_323_877_3;
        let k = (PI * f0 / sample_rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let high_pass = Biquad::new(
            [1.0, -2.0, 1.0],
            [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        );
        Self { shelf, high_pass }
    }

    fn process(&mut self, input: f64) -> f64 {
        self.high_pass.process(self.shelf.process(input))
    }
}

/// Polyphase interpolator that estimates the peaks between samples: 4× below 96 kHz, 2×
/// below 192 kHz, none above.
#[derive(Clone, Debug)]
struct TruePeak {
    /// `phases × TRUE_PEAK_TAPS` coefficients.
    coefficients: Vec<f64>,
    phases: usize,
    history: [f64; TRUE_PEAK_TAPS],
}

impl TruePeak {
    fn new(sample_rate: f64) -> Self {
        let phases = if sample_rate < 96_000.0 {
            4
        } else if sample_rate < 192_000.0 {
            2
        } else {
            1
        };
        let length = phases * TRUE_PEAK_TAPS;
        let centre = (length - 1) as f64 / 2.0;
        // Windowed sinc low-pass at the original Nyquist frequency, with unity gain per
        // phase.
        let prototype: Vec<f64> = (0..length)
            .map(|n| {
                let t = (n as f64 - centre) / phases as f64;
                let sinc = if t.abs() < 1e-12 {
                    1.0
                } else {
                    (PI * t).sin() / (PI * t)
                };
                let phase = 2.0 * PI * n as f64 / (length - 1) as f64;
                let window = 0.35875 - 0.48829 * phase.cos() + 0.14128 * (2.0 * phase).cos()
                    - 0.01168 * (3.0 * phase).cos();
                sinc * window
            })
            .collect();
        let mut coefficients = vec![0.0; length];
        for phase in 0..phases {
            let taps: Vec<f64> = (0..TRUE_PEAK_TAPS)
                .map(|tap| prototype[tap * phases + phase])
                .collect();
            let sum: f64 = taps.iter().sum();
            for (tap, value) in taps.iter().enumerate() {
                coefficients[phase * TRUE_PEAK_TAPS + tap] = value / sum;
            }
        }
        Self {
            coefficients,
            phases,
            history: [0.0; TRUE_PEAK_TAPS],
        }
    }

    /// Feeds a sample and returns the highest absolute interpolated value.
    fn process(&mut self, input: f64) -> f64 {
        self.history.copy_within(..TRUE_PEAK_TAPS - 1, 1);
        self.history[0] = input;
        if self.phases == 1 {
            return input.abs();
        }
        self.coefficients
            .chunks_exact(TRUE_PEAK_TAPS)
            .map(|taps| {
                taps.iter()
                    .zip(&self.history)
                    .map(|(tap, sample)| tap * sample)
                    .sum::<f64>()
                    .abs()
            })
            .fold(0.0, f64::max)
    }
}

/// Block loudness histogram from the absolute gate up, for gating without keeping every
/// block.
#[derive(Clone, Debug)]
struct GateHistogram {
    counts: Vec<u64>,
    powers: Vec<f64>,
    total: u64,
    total_power: f64,
}

impl GateHistogram {
    fn new() -> Self {
        Self {
            counts: vec![0; HISTOGRAM_BINS],
            powers: vec![0.0; HISTOGRAM_BINS],
            total: 0,
            total_power: 0.0,
        }
    }

    fn clear(&mut self) {
        self.counts.fill(0);
        self.powers.fill(0.0);
        self.total = 0;
        self.total_power = 0.0;
    }

    fn bin(lufs: f64) -> usize {
        (((lufs - f64::from(ABSOLUTE_GATE_LUFS)) / HISTOGRAM_STEP_LU) as usize)
            .min(HISTOGRAM_BINS - 1)
    }

    fn bin_lufs(bin: usize) -> f64 {
        f64::from(ABSOLUTE_GATE_LUFS) + (bin as f64 + 0.5) * HISTOGRAM_STEP_LU
    }

    /// Adds a block of mean square `power` if it passes the absolute gate.
    fn add(&mut self, power: f64) {
        let lufs = f64::from(power_to_lufs(power));
        if lufs < f64::from(ABSOLUTE_GATE_LUFS) {
            return;
        }
        let bin = Self::bin(lufs);
        self.counts[bin] += 1;
        self.powers[bin] += power;
        self.total += 1;
        self.total_power += power;
    }

    /// First bin at or above the relative gate `gate_lu` below the ungated mean.
    fn gate_bin(&self, gate_lu: f64) -> Option<usize> {
        if self.total == 0 {
            return None;
        }
        let mean = self.total_power / self.total as f64;
        let gate = f64::from(power_to_lufs(mean)) + gate_lu;
        Some(if gate < f64::from(ABSOLUTE_GATE_LUFS) {
            0
        } else {
            Self::bin(gate)
        })
    }

    /// Mean loudness of the blocks above the relative gate.
    fn gated_mean(&self, gate_lu: f64) -> f32 {
        let Some(first) = self.gate_bin(gate_lu) else {
            return SILENCE_LUFS;
        };
        let count: u64 = self.counts[first..].iter().sum();
        let power: f64 = self.powers[first..].iter().sum();
        if count == 0 {
            SILENCE_LUFS
        } else {
            power_to_lufs(power / count as f64)
        }
    }

    /// Difference between the 95th and 10th percentile of the blocks above the relative
    /// gate, in LU.
    fn range(&self, gate_lu: f64) -> f32 {
        let Some(first) = self.gate_bin(gate_lu) else {
            return 0.0;
        };
        let count: u64 = self.counts[first..].iter().sum();
        if count < 2 {
            return 0.0;
        }
        let percentile = |fraction: f64| {
            let target = ((count - 1) as f64 * fraction).round() as u64;
            let mut seen = 0;
            for (offset, bin_count) in self.counts[first..].iter().enumerate() {
                seen += bin_count;
                if seen > target {
                    return Self::bin_lufs(first + offset);
                }
            }
            Self::bin_lufs(HISTOGRAM_BINS - 1)
        };
        (percentile(0.95) - percentile(0.10)) as f32
    }
}

/// Current loudness readings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessReading {
    /// Loudness over the last 400 ms in LUFS.
    pub momentary_lufs: f32,
    /// Loudness over the last 3 s in LUFS.
    pub short_term_lufs: f32,
    /// Gated loudness since the last reset in LUFS.
    pub integrated_lufs: f32,
    /// Loudness range since the last reset in LU.
    pub range_lu: f32,
    /// Highest momentary and short-term loudness since the last reset in LUFS.
    pub momentary_max_lufs: f32,
    pub short_term_max_lufs: f32,
    /// Highest true peak per channel since the last reset in dBTP.
    pub true_peak_max_db: [f32; 2],
    /// True peak per channel over the last 100 ms block in dBTP.
    pub true_peak_db: [f32; 2],
    /// Highest sample peak per channel since the last reset in dBFS.
    pub sample_peak_max_db: [f32; 2],
    /// Measured time since the last reset in seconds.
    pub duration_s: f32,
}

impl Default for LoudnessReading {
    fn default() -> Self {
        Self {
            momentary_lufs: SILENCE_LUFS,
            short_term_lufs: SILENCE_LUFS,
            integrated_lufs: SILENCE_LUFS,
            range_lu: 0.0,
            momentary_max_lufs: SILENCE_LUFS,
            short_term_max_lufs: SILENCE_LUFS,
            true_peak_max_db: [f32::NEG_INFINITY; 2],
            true_peak_db: [f32::NEG_INFINITY; 2],
            sample_peak_max_db: [f32::NEG_INFINITY; 2],
            duration_s: 0.0,
        }
    }
}

/// The loudness meter. Feed it frames with [`process`](Self::process); every completed
/// 100 ms block updates the reading and returns `true`.
#[derive(Clone, Debug)]
pub struct LoudnessMeter {
    sample_rate: f64,
    channels: usize,
    weighting: [KWeighting; 2],
    true_peak: [TruePeak; 2],
    block_len: usize,
    block_fill: usize,
    block_power: f64,
    block_true_peak: [f64; 2],
    /// Mean squares of the last 30 blocks, newest at `head`.
    blocks: [f64; SHORT_BLOCKS],
    head: usize,
    filled: usize,
    integrated: GateHistogram,
    range: GateHistogram,
    blocks_total: u64,
    reading: LoudnessReading,
    peaks: [f64; 2],
    sample_peaks: [f32; 2],
}

impl LoudnessMeter {
    /// A meter for `channels` (one or two) at `sample_rate` Hz.
    pub fn new(sample_rate: f64, channels: usize) -> Self {
        let weighting = KWeighting::new(sample_rate);
        let true_peak = TruePeak::new(sample_rate);
        Self {
            sample_rate,
            channels: channels.clamp(1, 2),
            weighting: [weighting; 2],
            true_peak: [true_peak.clone(), true_peak],
            block_len: (sample_rate / 10.0).round().max(1.0) as usize,
            block_fill: 0,
            block_power: 0.0,
            block_true_peak: [0.0; 2],
            blocks: [0.0; SHORT_BLOCKS],
            head: 0,
            filled: 0,
            integrated: GateHistogram::new(),
            range: GateHistogram::new(),
            blocks_total: 0,
            reading: LoudnessReading::default(),
            peaks: [0.0; 2],
            sample_peaks: [0.0; 2],
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn reading(&self) -> &LoudnessReading {
        &self.reading
    }

    /// Clears the integrated measurements, maxima and peaks.
    pub fn reset(&mut self) {
        self.integrated.clear();
        self.range.clear();
        self.blocks_total = 0;
        self.peaks = [0.0; 2];
        self.sample_peaks = [0.0; 2];
        self.reading = LoudnessReading {
            momentary_lufs: self.reading.momentary_lufs,
            short_term_lufs: self.reading.short_term_lufs,
            ..LoudnessReading::default()
        };
    }

    /// Feeds one frame. Returns `true` when a 100 ms block completed.
    pub fn process(&mut self, frame: [f32; 2]) -> bool {
        for channel in 0..self.channels {
            let sample = f64::from(frame[channel]);
            let weighted = self.weighting[channel].process(sample);
            self.block_power += weighted * weighted;
            let peak = self.true_peak[channel].process(sample);
            self.block_true_peak[channel] = self.block_true_peak[channel].max(peak);
            self.sample_peaks[channel] = self.sample_peaks[channel].max(frame[channel].abs());
        }
        self.block_fill += 1;
        if self.block_fill < self.block_len {
            return false;
        }
        self.finish_block();
        true
    }

    fn finish_block(&mut self) {
        self.head = (self.head + 1) % SHORT_BLOCKS;
        self.blocks[self.head] = self.block_power / self.block_len as f64;
        self.filled = (self.filled + 1).min(SHORT_BLOCKS);
        self.block_fill = 0;
        self.block_power = 0.0;
        self.blocks_total += 1;

        let window = |count: usize| {
            (0..count)
                .map(|age| self.blocks[(self.head + SHORT_BLOCKS - age) % SHORT_BLOCKS])
                .sum::<f64>()
                / count as f64
        };
        let reading = &mut self.reading;
        if self.filled >= MOMENTARY_BLOCKS {
            let power = window(MOMENTARY_BLOCKS);
            self.integrated.add(power);
            reading.momentary_lufs = power_to_lufs(power);
            reading.momentary_max_lufs = reading.momentary_max_lufs.max(reading.momentary_lufs);
        }
        if self.filled >= SHORT_BLOCKS {
            let power = window(SHORT_BLOCKS);
            self.range.add(power);
            reading.short_term_lufs = power_to_lufs(power);
            reading.short_term_max_lufs = reading.short_term_max_lufs.max(reading.short_term_lufs);
        }
        reading.integrated_lufs = self.integrated.gated_mean(INTEGRATED_GATE_LU);
        reading.range_lu = self.range.range(RANGE_GATE_LU);
        for channel in 0..2 {
            let source = channel.min(self.channels - 1);
            let block_peak = self.block_true_peak[source];
            self.peaks[channel] = self.peaks[channel].max(block_peak);
            reading.true_peak_db[channel] = peak_to_db(block_peak as f32);
            reading.true_peak_max_db[channel] = peak_to_db(self.peaks[channel] as f32);
            reading.sample_peak_max_db[channel] = peak_to_db(self.sample_peaks[source]);
        }
        self.block_true_peak = [0.0; 2];
        reading.duration_s =
            self.blocks_total as f32 * self.block_len as f32 / self.sample_rate as f32;
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::{LoudnessMeter, power_to_lufs};

    fn sine(meter: &mut LoudnessMeter, rate: f64, frequency: f64, amplitude_db: f64, s: f64) {
        let amplitude = 10.0_f64.powf(amplitude_db / 20.0);
        let frames = (rate * s) as usize;
        for n in 0..frames {
            let value = (amplitude * (TAU * frequency * n as f64 / rate).sin()) as f32;
            meter.process([value, value]);
        }
    }

    #[test]
    fn tech_3341_stereo_sine_reads_its_level() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            let mut meter = LoudnessMeter::new(rate, 2);
            sine(&mut meter, rate, 1_000.0, -23.0, 20.0);
            let reading = meter.reading();
            for (name, value) in [
                ("momentary", reading.momentary_lufs),
                ("short-term", reading.short_term_lufs),
                ("integrated", reading.integrated_lufs),
            ] {
                assert!(
                    (value + 23.0).abs() < 0.1,
                    "{name} {value:.2} LUFS at {rate} Hz"
                );
            }
        }
    }

    #[test]
    fn tech_3342_level_steps_give_their_range() {
        let rate = 48_000.0;
        let mut meter = LoudnessMeter::new(rate, 2);
        sine(&mut meter, rate, 1_000.0, -20.0, 20.0);
        sine(&mut meter, rate, 1_000.0, -30.0, 20.0);
        let range = meter.reading().range_lu;
        assert!((range - 10.0).abs() < 1.0, "loudness range {range:.2} LU");
        meter.reset();
        assert_eq!(meter.reading().range_lu, 0.0);
    }

    #[test]
    fn relative_gate_ignores_quiet_passages() {
        let rate = 48_000.0;
        let mut meter = LoudnessMeter::new(rate, 2);
        sine(&mut meter, rate, 1_000.0, -20.0, 10.0);
        sine(&mut meter, rate, 1_000.0, -50.0, 10.0);
        let integrated = meter.reading().integrated_lufs;
        assert!(
            (integrated + 20.0).abs() < 0.2,
            "integrated {integrated:.2}"
        );
    }

    #[test]
    fn true_peak_finds_peaks_between_samples() {
        let rate = 48_000.0;
        let mut meter = LoudnessMeter::new(rate, 2);
        // A sine at a quarter of the rate, sampled 45° off its peaks: samples reach
        // only 0.707.
        for n in 0..48_000 {
            let value = (TAU * 0.25 * n as f64 + TAU / 8.0).sin() as f32 * 0.9;
            meter.process([value, value]);
        }
        let reading = meter.reading();
        let expected = 20.0 * 0.9_f32.log10();
        assert!(
            (reading.true_peak_max_db[0] - expected).abs() < 0.3,
            "true peak {:.2} dBTP",
            reading.true_peak_max_db[0]
        );
        assert!(reading.sample_peak_max_db[0] < expected - 2.5);
    }

    #[test]
    fn mono_counts_one_channel() {
        let rate = 48_000.0;
        let mut meter = LoudnessMeter::new(rate, 1);
        sine(&mut meter, rate, 1_000.0, -20.0, 5.0);
        // A single channel at -20 dBFS: 3 dB below the same sine in stereo.
        let value = meter.reading().momentary_lufs;
        assert!((value + 23.0).abs() < 0.1, "mono momentary {value:.2}");
        assert_eq!(power_to_lufs(0.0), f32::NEG_INFINITY);
    }
}
