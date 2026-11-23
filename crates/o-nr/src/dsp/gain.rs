//! Spectral gains from signal and noise power.
//!
//! Each bin's gain is the optimally modified log-spectral amplitude estimator (OM-LSA,
//! Cohen and Berdugo, 2001): the log-spectral amplitude MMSE gain (Ephraim and Malah,
//! 1985), which avoids most of the "musical noise" of spectral subtraction, weighted by
//! the probability that the bin holds signal, so bins of pure noise fall to the floor.
//! The a priori SNR is the decision-directed estimate.
//!
//! Gains then fall back slowly (release). They rise at once where a bin and both its
//! neighbours rise together, as a tone or an attack does, so transients pass unsmoothed;
//! a lone bin, as a random noise peak, rises only at the release rate. Smoothing across
//! frequency then fills narrow dips between kept bins, and the reduction amount limits
//! the attenuation.

/// Weight of the previous frame's clean estimate in the decision-directed a priori SNR.
const DECISION_DIRECTED: f32 = 0.98;

/// Lowest a priori SNR (−35 dB).
const MIN_PRIOR_SNR: f32 = 3.2e-4;

/// Prior probability that a bin holds no signal.
const ABSENCE_PRIOR: f32 = 0.5;

/// Gain of bins without signal (−60 dB) before the reduction floor is applied.
const ABSENT_GAIN: f32 = 1e-3;

/// Exponential integral E1(x) for x > 0, within about 1e-4 relative error
/// (Abramowitz and Stegun 5.1.53 and 5.1.56).
pub fn e1(x: f32) -> f32 {
    let x = x.max(1e-12);
    if x < 1.0 {
        let series = -0.577_215_7
            + x * (0.999_991_93
                + x * (-0.249_910_55
                    + x * (0.055_199_68 + x * (-0.009_760_04 + x * 0.001_078_57))));
        series - x.ln()
    } else if x > 60.0 {
        0.0
    } else {
        let numerator = x * x + 2.334_733 * x + 0.250_621;
        let denominator = x * x + 3.330_657 * x + 1.681_534;
        numerator / denominator * (-x).exp() / x
    }
}

/// Log-spectral amplitude gain for a priori SNR `prior` and a posteriori SNR
/// `posterior`, limited to 1.
pub fn lsa_gain(prior: f32, posterior: f32) -> f32 {
    let ratio = prior / (1.0 + prior);
    let v = ratio * posterior;
    (ratio * (0.5 * e1(v)).exp()).min(1.0)
}

/// Probability that a bin holds signal, for a priori SNR `prior` and a posteriori SNR
/// `posterior`.
pub fn presence(prior: f32, posterior: f32) -> f32 {
    let v = prior / (1.0 + prior) * posterior;
    let odds = ABSENCE_PRIOR / (1.0 - ABSENCE_PRIOR) * (1.0 + prior) * (-v.min(80.0)).exp();
    1.0 / (1.0 + odds)
}

/// How the gains are shaped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainShape {
    /// Factor applied to the noise estimate (the threshold as a power ratio).
    pub threshold: f32,
    /// Lowest gain (amplitude), from the reduction amount; 1 leaves the signal unchanged.
    pub floor: f32,
    /// Per-frame coefficient with which a gain approaches a lower value, or a higher one
    /// that its neighbours do not share.
    pub release: f32,
    /// One-pole coefficient of the smoothing across frequency, which only raises gains;
    /// 0 for none.
    pub smoothing: f32,
}

/// Per-bin state of the gain estimator for one analysis channel.
pub struct GainState {
    previous_clean: Vec<f32>,
    raw: Vec<f32>,
    held: Vec<f32>,
    gains: Vec<f32>,
}

impl GainState {
    pub fn new(max_bins: usize) -> Self {
        Self {
            previous_clean: vec![0.0; max_bins],
            raw: vec![1.0; max_bins],
            held: vec![1.0; max_bins],
            gains: vec![1.0; max_bins],
        }
    }

    pub fn reset(&mut self) {
        self.previous_clean.fill(0.0);
        self.raw.fill(1.0);
        self.held.fill(1.0);
        self.gains.fill(1.0);
    }

    /// Gains of the last [`update`](Self::update) or [`pass`](Self::pass).
    pub fn gains(&self) -> &[f32] {
        &self.gains
    }

    /// Unity gains for the first `bins` bins, keeping the estimator ready to resume.
    pub fn pass(&mut self, power: &[f32]) {
        let bins = power.len();
        self.raw[..bins].fill(1.0);
        self.gains[..bins].fill(1.0);
        self.held[..bins].fill(1.0);
        self.previous_clean[..bins].copy_from_slice(power);
    }

    /// Computes the gains of one frame from its power and the noise power per bin.
    pub fn update(&mut self, power: &[f32], noise: &[f32], shape: &GainShape) {
        let bins = power.len();
        let states = self.previous_clean[..bins]
            .iter_mut()
            .zip(&mut self.raw[..bins]);
        for ((previous, raw), (power, noise)) in states.zip(power.iter().zip(noise)) {
            let noise = (noise * shape.threshold).max(1e-30);
            let posterior = power / noise;
            let prior = (DECISION_DIRECTED * *previous / noise
                + (1.0 - DECISION_DIRECTED) * (posterior - 1.0).max(0.0))
            .max(MIN_PRIOR_SNR);
            let probability = presence(prior, posterior);
            let gain =
                lsa_gain(prior, posterior).powf(probability) * ABSENT_GAIN.powf(1.0 - probability);
            *previous = gain * gain * power;
            *raw = gain;
        }

        let raw = &self.raw[..bins];
        for (bin, (held, gain)) in self.held[..bins]
            .iter_mut()
            .zip(&mut self.gains[..bins])
            .enumerate()
        {
            let below = raw[bin.saturating_sub(1)];
            let above = raw[(bin + 1).min(bins - 1)];
            let shared_rise = raw[bin].min(below).min(above);
            *held = if shared_rise > *held {
                raw[bin]
            } else {
                *held * shape.release + raw[bin] * (1.0 - shape.release)
            };
            *gain = *held;
        }

        let gains = &mut self.gains[..bins];
        if shape.smoothing > 0.0 {
            // Zero-phase one-pole smoothing that only fills dips, so signal peaks keep
            // their gain.
            let keep = shape.smoothing;
            let smoothed = &mut self.raw[..bins];
            let mut state = gains[0];
            for (smoothed, gain) in smoothed.iter_mut().zip(gains.iter()) {
                state = keep * state + (1.0 - keep) * *gain;
                *smoothed = state;
            }
            for (smoothed, gain) in smoothed.iter_mut().zip(gains.iter_mut()).rev() {
                state = keep * state + (1.0 - keep) * *smoothed;
                *gain = gain.max(state);
            }
        }
        let floor = shape.floor;
        gains
            .iter_mut()
            .for_each(|gain| *gain = floor + (1.0 - floor) * *gain);
    }
}

#[cfg(test)]
mod tests {
    use super::{GainShape, GainState, e1, lsa_gain};

    #[test]
    fn exponential_integral_matches_tables() {
        for (x, expected) in [
            (0.01, 4.037_93),
            (0.1, 1.822_92),
            (0.5, 0.559_77),
            (1.0, 0.219_38),
            (2.0, 0.048_90),
            (5.0, 0.001_148_3),
            (10.0, 4.157e-6),
        ] {
            let value = e1(x);
            assert!(
                (value - expected).abs() / expected < 1e-3,
                "E1({x}) = {value}, expected {expected}"
            );
        }
    }

    #[test]
    fn gain_follows_snr() {
        assert!(lsa_gain(100.0, 101.0) > 0.98);
        assert!(lsa_gain(1e-3, 1.0) < 0.1);
        assert!(lsa_gain(1.0, 2.0) > lsa_gain(0.1, 2.0));
    }

    #[test]
    fn full_floor_leaves_the_signal_unchanged() {
        let mut state = GainState::new(8);
        let shape = GainShape {
            threshold: 1.0,
            floor: 1.0,
            release: 0.5,
            smoothing: 0.5,
        };
        state.update(&[1.0, 0.0, 3.0, 1e-9], &[1.0, 1.0, 1.0, 1.0], &shape);
        assert!(state.gains()[..4].iter().all(|gain| *gain == 1.0));
    }
}
