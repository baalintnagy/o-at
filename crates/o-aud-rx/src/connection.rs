//! A receiver's connection to one bus stream: reading, resampling to the output rate and
//! keeping the queued audio at the target latency.
//!
//! A connection is built off the audio thread, because the resampler allocates, and is
//! replaced as a whole when the stream format or the output rate changes.

use audbus::{BusReader, SAFE_WINDOW_FRAMES, StreamFormat};
use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, ResamplerConstructionError,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::sync::{DriftController, MAX_CORRECTION};
use crate::telemetry::{PlayState, RxTelemetry};

/// Output frames produced per resampler call.
pub const CHUNK_FRAMES: usize = 64;

/// Fade-in length after (re)starting playback, in output frames.
const FADE_IN_FRAMES: usize = 256;

/// Length of the windows over which block bursts and jitter are measured, in seconds.
const WINDOW_S: f64 = 0.5;

/// Headroom over the measured block bursts, as a factor.
const BURST_HEADROOM: f64 = 1.5;

/// Queued audio above this multiple of the target counts as an overrun.
const OVERRUN_FACTOR: f64 = 3.0;

/// Where an output channel takes its audio from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Channel(usize),
    /// Average of all stream channels, for a mono output.
    Mix,
    Silent,
}

fn source_for(output: usize, outputs: usize, sources: usize) -> Source {
    if sources == 1 {
        Source::Channel(0)
    } else if outputs == 1 {
        Source::Mix
    } else if output < sources {
        Source::Channel(output)
    } else {
        Source::Silent
    }
}

/// Finished windows over which the largest block burst is remembered (4 s).
const BURST_WINDOWS: usize = 8;

/// Time constant of the smoothed queue length that dips are measured from, in seconds.
const MEAN_S: f64 = 1.0;

/// Playing time before dips count, in seconds, so start-up settling is not mistaken for
/// irregular blocks.
const WARMUP_S: f64 = 1.0;

/// Per-window decay of the remembered queue dip: halves in about 17 s, so a latency
/// raised after dropouts comes down slowly once the transmitter is steady again.
const DIP_DECAY: f64 = 0.98;

/// Block-timing statistics over consecutive windows.
#[derive(Clone, Copy, Debug, Default)]
struct Timing {
    /// Whether `last_write` holds a position seen by `observe`.
    primed: bool,
    last_write: u64,
    /// Largest write-position step per window, in source frames: the current window
    /// first, then the finished ones from the newest.
    bursts: [u64; BURST_WINDOWS + 1],
    window_min_fill: u64,
    window_max_fill: u64,
    window_frames: usize,
    /// Peak-to-peak fill variation of the last finished window, in source frames.
    jitter: u64,
    /// Smoothed queue length while playing, in source frames.
    mean_fill: f64,
    /// Time since playback started, in seconds.
    playing_s: f64,
    /// Deepest recent fall of the queue below its smoothed length, in source frames,
    /// decaying.
    dip: f64,
}

impl Timing {
    fn restart(&mut self) {
        *self = Self {
            window_min_fill: u64::MAX,
            ..Self::default()
        };
    }

    /// Largest step between two blocks recently seen, in source frames. It rises at
    /// once and falls only after a burst left the remembered windows, so the target
    /// latency stays steady.
    fn burst(&self) -> u64 {
        self.bursts.iter().copied().max().unwrap_or(0)
    }

    /// Restarts the smoothed queue length at playback start.
    fn start_playing(&mut self, fill: f64) {
        self.mean_fill = fill;
        self.playing_s = 0.0;
    }

    /// Records the queue length after a played block of `elapsed_s` seconds. Irregular
    /// transmitter timing, such as late blocks that are never caught up, shows as dips
    /// below the smoothed length rather than in the block sizes. Measuring from the
    /// smoothed length, not the target, keeps a raised target from raising itself.
    fn observe_playing(&mut self, fill: u64, elapsed_s: f64) {
        let fill = fill as f64;
        self.mean_fill += (fill - self.mean_fill) * elapsed_s / (MEAN_S + elapsed_s);
        self.playing_s += elapsed_s;
        if self.playing_s >= WARMUP_S {
            self.dip = self.dip.max(self.mean_fill - fill);
        }
    }

    /// Records a dropout: the queue fell from its smoothed length to empty.
    fn observe_underrun(&mut self) {
        self.dip = self.dip.max(self.mean_fill);
    }

    fn observe(&mut self, write: u64, fill: u64, frames: usize, window_frames: usize) {
        let step = if self.primed {
            write.saturating_sub(self.last_write)
        } else {
            0
        };
        self.primed = true;
        self.last_write = write;
        self.bursts[0] = self.bursts[0].max(step);
        self.window_min_fill = self.window_min_fill.min(fill);
        self.window_max_fill = self.window_max_fill.max(fill);
        self.window_frames += frames;
        if self.window_frames >= window_frames {
            self.bursts.copy_within(..BURST_WINDOWS, 1);
            self.bursts[0] = 0;
            self.dip *= DIP_DECAY;
            self.jitter = self.window_max_fill.saturating_sub(self.window_min_fill);
            self.window_min_fill = u64::MAX;
            self.window_max_fill = 0;
            self.window_frames = 0;
        }
    }
}

pub struct Connection {
    reader: BusReader,
    format: StreamFormat,
    device_rate: u32,
    resampler: Async<f32>,
    /// Planar input scratch for the largest resampler input.
    input: Vec<f32>,
    /// Planar resampler output.
    chunk: Vec<f32>,
    /// Frames of `chunk` already played; `CHUNK_FRAMES` when empty.
    chunk_read: usize,
    /// Next bus frame to read.
    position: u64,
    playing: bool,
    /// Stop after the current (faded-out) chunk.
    draining: bool,
    fade_in_remaining: usize,
    controller: DriftController,
    timing: Timing,
}

impl Connection {
    /// Prepares playback of `reader`'s stream in `format` at `device_rate` Hz.
    pub fn new(
        reader: BusReader,
        format: StreamFormat,
        device_rate: u32,
    ) -> Result<Self, ResamplerConstructionError> {
        let parameters = SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: None,
            interpolation: SincInterpolationType::Cubic,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        };
        let ratio = f64::from(device_rate) / f64::from(format.sample_rate);
        let resampler = Async::<f32>::new_sinc(
            ratio,
            1.0 + 2.0 * MAX_CORRECTION,
            &parameters,
            CHUNK_FRAMES,
            format.channels,
            FixedAsync::Output,
        )?;
        let input = vec![0.0; format.channels * resampler.input_frames_max()];
        let position = reader.write_position();
        let mut timing = Timing::default();
        timing.restart();
        Ok(Self {
            reader,
            format,
            device_rate,
            resampler,
            input,
            chunk: vec![0.0; format.channels * CHUNK_FRAMES],
            chunk_read: CHUNK_FRAMES,
            position,
            playing: false,
            draining: false,
            fade_in_remaining: 0,
            controller: DriftController::default(),
            timing,
        })
    }

    pub fn format(&self) -> StreamFormat {
        self.format
    }

    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// Writes the next block of bus audio to `outputs` (equal-length channels) and
    /// publishes the synchronization state. Real-time safe.
    pub fn render<C: AsMut<[f32]>>(
        &mut self,
        outputs: &mut [C],
        latency_ms: f32,
        telemetry: &RxTelemetry,
    ) {
        let frames = outputs
            .first_mut()
            .map_or(0, |channel| channel.as_mut().len());
        if self.reader.format() != Some(self.format) {
            // The stream restarted; the link replaces this connection.
            self.playing = false;
            silence(outputs);
            telemetry.set_state(PlayState::Buffering);
            return;
        }
        let write = self.reader.write_position();
        let fill = write.saturating_sub(self.position);
        self.timing.observe(
            write,
            fill,
            frames,
            (WINDOW_S * f64::from(self.device_rate)) as usize,
        );
        let target = self.target_frames(latency_ms, frames);
        telemetry.requested_ms.store(latency_ms);

        let was_playing = self.playing;
        if !self.playing {
            if (fill as f64) < target {
                silence(outputs);
                self.publish(telemetry, PlayState::Buffering, fill, target);
                return;
            }
            self.start(write, target);
        }

        let mut written = 0;
        while written < frames {
            if self.chunk_read == CHUNK_FRAMES {
                if self.draining {
                    self.draining = false;
                    self.playing = false;
                    break;
                }
                if !self.next_chunk(write, target, telemetry) {
                    self.playing = false;
                    break;
                }
            }
            let count = (frames - written).min(CHUNK_FRAMES - self.chunk_read);
            self.copy_chunk(outputs, written, count);
            self.chunk_read += count;
            written += count;
        }
        for channel in outputs.iter_mut() {
            channel.as_mut()[written..].fill(0.0);
        }
        let state = if self.playing {
            PlayState::Playing
        } else {
            PlayState::Buffering
        };
        let fill = write.saturating_sub(self.position);
        if was_playing {
            let elapsed_s = frames as f64 / f64::from(self.device_rate);
            self.timing.observe_playing(fill, elapsed_s);
        }
        self.publish(telemetry, state, fill, target);
    }

    /// Target queue length in source frames: the requested latency, raised so that the
    /// transmitter's and the device's blocks fit.
    ///
    /// The queue swings by about one transmitter burst plus one device block around its
    /// mean, or by the deepest dip measured recently, and must keep one resampler input
    /// in reserve at its low point.
    fn target_frames(&self, latency_ms: f32, device_frames: usize) -> f64 {
        let source_rate = f64::from(self.format.sample_rate);
        let requested = f64::from(latency_ms) * 0.001 * source_rate;
        let device = device_frames as f64 * source_rate / f64::from(self.device_rate);
        let swing = (0.5 * (self.timing.burst() as f64 + device)).max(self.timing.dip);
        let reserve = self.resampler.input_frames_max() as f64;
        let ceiling = SAFE_WINDOW_FRAMES as f64 / OVERRUN_FACTOR;
        requested
            .max(BURST_HEADROOM * (swing + reserve))
            .min(ceiling)
    }

    fn start(&mut self, write: u64, target: f64) {
        self.position = write.saturating_sub(target as u64);
        self.timing.start_playing(target);
        self.resampler.reset();
        self.controller.restart();
        self.chunk_read = CHUNK_FRAMES;
        self.draining = false;
        self.fade_in_remaining = FADE_IN_FRAMES;
        self.playing = true;
    }

    /// Produces the next chunk. Returns `false` if there is nothing to play.
    fn next_chunk(&mut self, write: u64, target: f64, telemetry: &RxTelemetry) -> bool {
        let channels = self.format.channels;
        let available = write.saturating_sub(self.position);
        if available > SAFE_WINDOW_FRAMES || available as f64 > OVERRUN_FACTOR * target {
            RxTelemetry::count(&telemetry.overruns);
            self.position = write.saturating_sub(target as u64);
            self.controller.restart();
        }
        let available = write.saturating_sub(self.position) as usize;
        let needed = self.resampler.input_frames_next();
        let frames = needed.min(available);
        if frames < needed {
            RxTelemetry::count(&telemetry.underruns);
            self.timing.observe_underrun();
            self.draining = true;
        }
        let input = &mut self.input[..channels * needed];
        if !self
            .reader
            .read(self.position, frames, channels, self.format.epoch, input)
        {
            RxTelemetry::count(&telemetry.overruns);
            return false;
        }
        if frames < needed {
            // `read` packs channels with a stride of `frames`; spread them to `needed`
            // and pad with silence, from the last channel down so nothing is overwritten.
            for channel in (0..channels).rev() {
                input.copy_within(channel * frames..(channel + 1) * frames, channel * needed);
                input[channel * needed + frames..(channel + 1) * needed].fill(0.0);
            }
        }
        let consumed = SequentialSlice::new(&self.input[..channels * needed], channels, needed)
            .ok()
            .zip(SequentialSlice::new_mut(&mut self.chunk, channels, CHUNK_FRAMES).ok())
            .and_then(|(input, mut output)| {
                self.resampler
                    .process_into_buffer(&input, &mut output, None)
                    .ok()
            });
        let Some((consumed, _)) = consumed else {
            return false;
        };
        // The resampler may use fewer frames than it asked for; the rest is read again.
        self.position += consumed.min(frames) as u64;

        if self.draining {
            self.fade_chunk_out();
        } else {
            let source_rate = f64::from(self.format.sample_rate);
            let queued = write.saturating_sub(self.position) as f64;
            let ratio = self.controller.update(
                (queued - target) / source_rate,
                CHUNK_FRAMES as f64 / f64::from(self.device_rate),
            );
            // Within the bounds given at construction, so this cannot fail.
            let _ = self.resampler.set_resample_ratio_relative(ratio, true);
            self.fade_chunk_in();
        }
        self.chunk_read = 0;
        true
    }

    fn fade_chunk_in(&mut self) {
        if self.fade_in_remaining == 0 {
            return;
        }
        let done = FADE_IN_FRAMES - self.fade_in_remaining;
        for channel in self.chunk.chunks_mut(CHUNK_FRAMES) {
            for (offset, sample) in channel.iter_mut().enumerate() {
                let progress = ((done + offset) as f32 / FADE_IN_FRAMES as f32).min(1.0);
                *sample *= progress;
            }
        }
        self.fade_in_remaining = self.fade_in_remaining.saturating_sub(CHUNK_FRAMES);
    }

    fn fade_chunk_out(&mut self) {
        for channel in self.chunk.chunks_mut(CHUNK_FRAMES) {
            for (offset, sample) in channel.iter_mut().enumerate() {
                *sample *= 1.0 - offset as f32 / CHUNK_FRAMES as f32;
            }
        }
    }

    fn copy_chunk<C: AsMut<[f32]>>(&self, outputs: &mut [C], offset: usize, count: usize) {
        let channels = self.format.channels;
        let outputs_len = outputs.len();
        let chunk = |channel: usize| {
            let start = channel * CHUNK_FRAMES + self.chunk_read;
            &self.chunk[start..start + count]
        };
        for (index, output) in outputs.iter_mut().enumerate() {
            let target = &mut output.as_mut()[offset..offset + count];
            match source_for(index, outputs_len, channels) {
                Source::Channel(channel) => target.copy_from_slice(chunk(channel)),
                Source::Mix => {
                    target.copy_from_slice(chunk(0));
                    for channel in 1..channels {
                        for (sum, sample) in target.iter_mut().zip(chunk(channel)) {
                            *sum += sample;
                        }
                    }
                    let scale = 1.0 / channels as f32;
                    target.iter_mut().for_each(|sample| *sample *= scale);
                }
                Source::Silent => target.fill(0.0),
            }
        }
    }

    fn publish(&self, telemetry: &RxTelemetry, state: PlayState, fill: u64, target: f64) {
        let to_ms = 1_000.0 / f64::from(self.format.sample_rate);
        telemetry.set_state(state);
        telemetry.fill_ms.store((fill as f64 * to_ms) as f32);
        telemetry.target_ms.store((target * to_ms) as f32);
        telemetry
            .jitter_ms
            .store((self.timing.jitter as f64 * to_ms) as f32);
        telemetry
            .drift_ppm
            .store(self.controller.drift_ppm() as f32);
        telemetry.set_source(self.format.sample_rate, self.format.channels);
    }
}

fn silence<C: AsMut<[f32]>>(outputs: &mut [C]) {
    outputs
        .iter_mut()
        .for_each(|channel| channel.as_mut().fill(0.0));
}

#[cfg(test)]
mod tests {
    use super::{Source, source_for};

    #[test]
    fn channels_are_mapped_between_layouts() {
        assert_eq!(source_for(1, 2, 1), Source::Channel(0), "mono fills stereo");
        assert_eq!(source_for(0, 1, 2), Source::Mix, "stereo folds to mono");
        assert_eq!(source_for(1, 2, 2), Source::Channel(1));
        assert_eq!(source_for(3, 4, 2), Source::Silent);
    }
}
