//! Meter and spectrum telemetry for the editor.

use super::{BandRange, Engine};
use crate::dsp::gain::GainShape;
use crate::dsp::level::{gain_to_db, power_to_db};
use crate::telemetry::NoiseSource;

/// Time constant of the spectrum display smoothing, in seconds.
const DISPLAY_SMOOTHING_S: f32 = 0.08;

/// Time constant of the reduction meter, in seconds.
const REDUCTION_SMOOTHING_S: f32 = 0.25;

/// Time constant with which the peak meters fall, in seconds.
const PEAK_FALL_S: f32 = 0.3;

impl Engine {
    /// Updates the meters and the spectrum display and publishes them to the editor.
    pub(super) fn publish(
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
