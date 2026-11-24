//! Adapts arbitrary host buffer sizes and sample rates to the engine's 10 ms frames at
//! 48 kHz.
//!
//! Host audio is collected into blocks of 10 ms at the host rate. A full block is
//! resampled to one 480-sample frame with rubato's synchronous FFT resampler, processed,
//! and resampled back. The output therefore lags by exactly one block plus the
//! resampler and engine delays, which is reported to the host as latency. The engine
//! delay depends on the selected noise model, so the latency can change with it.

use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{Fft, FixedSync, Resampler};

use crate::dsp::deep_filter::Pacing;
use crate::dsp::engine::Engine;
use crate::dsp::{FRAME, INTERNAL_RATE};

/// Why a host configuration cannot be processed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamError {
    /// The host rate is not a whole number of samples per 10 ms.
    UnsupportedRate(u32),
    /// The resampler could not be built with matching block sizes.
    Resampler(u32),
}

struct Resampling {
    upsampler: Fft<f32>,
    downsampler: Fft<f32>,
    /// Mic and reference block, channel after channel.
    block_input: Vec<f32>,
    /// Mic and reference frame at 48 kHz, channel after channel.
    frame_input: Vec<f32>,
}

pub struct StreamProcessor {
    engine: Engine,
    host_rate: u32,
    block: usize,
    position: usize,
    mic_block: Vec<f32>,
    reference_block: Vec<f32>,
    output_block: Vec<f32>,
    frame: Vec<f32>,
    resampling: Option<Resampling>,
    /// Resampler delays in samples at 48 kHz (upsampler) and the host rate (downsampler).
    upsampler_delay: usize,
    downsampler_delay: usize,
}

impl StreamProcessor {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            host_rate: INTERNAL_RATE,
            block: FRAME,
            position: 0,
            mic_block: vec![0.0; FRAME],
            reference_block: vec![0.0; FRAME],
            output_block: vec![0.0; FRAME],
            frame: vec![0.0; FRAME],
            resampling: None,
            upsampler_delay: 0,
            downsampler_delay: 0,
        }
    }

    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Prepares processing at `host_rate` for host buffers of up to `max_buffer`
    /// samples. Allocates, so call it outside the audio callback.
    ///
    /// DeepFilterNet's worker gets one frame of slack more than the frames a single host
    /// buffer can complete, so that results computed during one host callback are ready
    /// for the next.
    pub fn configure(
        &mut self,
        host_rate: u32,
        max_buffer: usize,
        pacing: Pacing,
    ) -> Result<(), StreamError> {
        if host_rate == 0 || !host_rate.is_multiple_of(100) {
            return Err(StreamError::UnsupportedRate(host_rate));
        }
        let block = (host_rate / 100) as usize;
        let rate = host_rate as usize;
        let internal = INTERNAL_RATE as usize;
        let resampling = if host_rate == INTERNAL_RATE {
            None
        } else {
            let upsampler = Fft::<f32>::new(rate, internal, block, 2, FixedSync::Both)
                .map_err(|_| StreamError::Resampler(host_rate))?;
            let downsampler = Fft::<f32>::new(internal, rate, FRAME, 1, FixedSync::Both)
                .map_err(|_| StreamError::Resampler(host_rate))?;
            if upsampler.input_frames_next() != block
                || upsampler.output_frames_next() != FRAME
                || downsampler.input_frames_next() != FRAME
                || downsampler.output_frames_next() != block
            {
                return Err(StreamError::Resampler(host_rate));
            }
            Some(Resampling {
                upsampler,
                downsampler,
                block_input: vec![0.0; 2 * block],
                frame_input: vec![0.0; 2 * FRAME],
            })
        };

        self.upsampler_delay = resampling
            .as_ref()
            .map_or(0, |resampling| resampling.upsampler.output_delay());
        self.downsampler_delay = resampling
            .as_ref()
            .map_or(0, |resampling| resampling.downsampler.output_delay());
        let frames_per_buffer = max_buffer.max(1).div_ceil(block);
        self.engine.configure(frames_per_buffer + 1, pacing);

        self.host_rate = host_rate;
        self.block = block;
        self.mic_block = vec![0.0; block];
        self.reference_block = vec![0.0; block];
        self.output_block = vec![0.0; block];
        self.resampling = resampling;
        self.position = 0;
        self.engine
            .set_host_info(host_rate as f32, self.latency_samples());
        Ok(())
    }

    /// Total latency at the host rate for the current settings in samples.
    pub fn latency_samples(&self) -> u32 {
        let internal_delay = self.engine.delay() + self.upsampler_delay;
        let scaled = internal_delay as f64 * self.host_rate as f64 / INTERNAL_RATE as f64;
        (self.block + self.downsampler_delay) as u32 + scaled.round() as u32
    }

    pub fn host_rate(&self) -> u32 {
        self.host_rate
    }

    pub fn reset(&mut self) {
        self.engine.reset();
        self.mic_block.fill(0.0);
        self.reference_block.fill(0.0);
        self.output_block.fill(0.0);
        self.position = 0;
        if let Some(resampling) = self.resampling.as_mut() {
            resampling.upsampler.reset();
            resampling.downsampler.reset();
        }
    }

    /// Processes mono mic and reference samples into `output`. All slices must have the
    /// same length.
    pub fn process(&mut self, mic: &[f32], reference: &[f32], output: &mut [f32]) {
        debug_assert_eq!(mic.len(), output.len());
        debug_assert_eq!(reference.len(), output.len());
        self.engine
            .set_host_info(self.host_rate as f32, self.latency_samples());
        let mut offset = 0;
        while offset < output.len() {
            let count = (self.block - self.position).min(output.len() - offset);
            let block_range = self.position..self.position + count;
            let range = offset..offset + count;
            output[range.clone()].copy_from_slice(&self.output_block[block_range.clone()]);
            self.mic_block[block_range.clone()].copy_from_slice(&mic[range.clone()]);
            self.reference_block[block_range].copy_from_slice(&reference[range]);
            self.position += count;
            offset += count;
            if self.position == self.block {
                self.process_block();
                self.position = 0;
            }
        }
    }

    fn process_block(&mut self) {
        let Some(resampling) = self.resampling.as_mut() else {
            self.frame.copy_from_slice(&self.mic_block);
            self.engine
                .process_frame(&mut self.frame, &self.reference_block);
            self.output_block.copy_from_slice(&self.frame);
            return;
        };

        let block = self.block;
        resampling.block_input[..block].copy_from_slice(&self.mic_block);
        resampling.block_input[block..].copy_from_slice(&self.reference_block);
        let upsampled = SequentialSlice::new(&resampling.block_input, 2, block)
            .ok()
            .zip(SequentialSlice::new_mut(&mut resampling.frame_input, 2, FRAME).ok())
            .map(|(input, mut output)| {
                resampling
                    .upsampler
                    .process_into_buffer(&input, &mut output, None)
                    .is_ok()
            });
        if upsampled != Some(true) {
            self.output_block.fill(0.0);
            return;
        }

        let (mic, reference) = resampling.frame_input.split_at_mut(FRAME);
        self.engine.process_frame(mic, reference);

        let downsampled = SequentialSlice::new(&resampling.frame_input[..FRAME], 1, FRAME)
            .ok()
            .zip(SequentialSlice::new_mut(&mut self.output_block, 1, block).ok())
            .map(|(input, mut output)| {
                resampling
                    .downsampler
                    .process_into_buffer(&input, &mut output, None)
                    .is_ok()
            });
        if downsampled != Some(true) {
            self.output_block.fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{StreamError, StreamProcessor};
    use crate::dsp::deep_filter::Pacing;
    use crate::dsp::denoise::NoiseModel;
    use crate::dsp::engine::{Engine, EngineSettings, neutral_settings};

    fn neutral_processor(noise_model: NoiseModel) -> StreamProcessor {
        let mut engine = Engine::new(None);
        engine.set_settings(EngineSettings {
            noise_model,
            ..neutral_settings()
        });
        StreamProcessor::new(engine)
    }

    #[test]
    fn supports_common_host_rates() {
        for rate in [
            8_000, 16_000, 24_000, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
        ] {
            let mut processor = neutral_processor(NoiseModel::RnNoise);
            assert_eq!(
                processor.configure(rate, 512, Pacing::Realtime),
                Ok(()),
                "rate {rate}"
            );
        }
        let mut processor = neutral_processor(NoiseModel::RnNoise);
        assert_eq!(
            processor.configure(22_050, 512, Pacing::Realtime),
            Err(StreamError::UnsupportedRate(22_050))
        );
    }

    /// The reported latency must match the actual delay of an impulse, for any host
    /// buffer size.
    #[test]
    fn reported_latency_matches_impulse_delay() {
        let cases = [
            (NoiseModel::RnNoise, 48_000, 64),
            (NoiseModel::RnNoise, 48_000, 1000),
            (NoiseModel::RnNoise, 44_100, 128),
            (NoiseModel::RnNoise, 96_000, 512),
            (NoiseModel::DeepFilterNet, 48_000, 64),
            (NoiseModel::DeepFilterNet, 48_000, 1000),
            (NoiseModel::DeepFilterNet, 44_100, 512),
        ];
        for (noise_model, rate, buffer) in cases {
            let mut processor = neutral_processor(noise_model);
            processor
                .configure(rate, buffer, Pacing::Offline)
                .expect("supported rate");
            let length = rate as usize;
            let mut input = vec![0.0; length];
            input[rate as usize / 10] = 1.0;
            let reference = vec![0.0; length];
            let mut output = vec![0.0; length];
            for ((mic, reference), output) in input
                .chunks(buffer)
                .zip(reference.chunks(buffer))
                .zip(output.chunks_mut(buffer))
            {
                processor.process(mic, reference, output);
            }
            let peak = output
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .map(|(index, _)| index)
                .unwrap_or_default();
            let measured = (peak - rate as usize / 10) as i64;
            let reported = processor.latency_samples() as i64;
            assert!(
                (measured - reported).abs() <= 2,
                "{noise_model:?} at {rate}: measured {measured}, reported {reported}"
            );
        }
    }
}
