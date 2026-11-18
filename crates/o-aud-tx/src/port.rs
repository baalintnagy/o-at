//! The audio-thread side of a transmitter.

use std::sync::Arc;

use audbus::BusWriter;
use audbus::handoff::HandoffReceiver;

use crate::telemetry::{METER_CHANNELS, TxTelemetry};

/// Time for gain changes to settle, in seconds.
const GAIN_RAMP_S: f32 = 0.02;

/// Writes blocks to the bus the [`TxLink`](crate::link::TxLink) provides. Real-time safe.
pub struct TxPort {
    writers: HandoffReceiver<BusWriter>,
    telemetry: Arc<TxTelemetry>,
    sample_rate: u32,
    channels: usize,
    /// Gain applied at the end of the previous block.
    gain: f32,
}

impl TxPort {
    pub(crate) fn new(writers: HandoffReceiver<BusWriter>, telemetry: Arc<TxTelemetry>) -> Self {
        Self {
            writers,
            telemetry,
            sample_rate: 0,
            channels: 0,
            gain: 0.0,
        }
    }

    /// Sets the stream format: rate in Hz and channel count. Also tell the link with
    /// [`TxLink::set_format`](crate::link::TxLink::set_format).
    pub fn set_format(&mut self, sample_rate: u32, channels: usize) {
        self.sample_rate = sample_rate;
        self.channels = channels;
    }

    /// Sends one block of equal-length channels at `gain` (a linear factor).
    pub fn process<C: AsRef<[f32]>>(&mut self, input: &[C], gain: f32) {
        let _ = self.writers.poll();
        let frames = input.first().map_or(0, |channel| channel.as_ref().len());
        let ramp_frames = (GAIN_RAMP_S * self.sample_rate.max(1) as f32).max(1.0);
        let start = self.gain;
        let end = start + (gain - start) * (frames as f32 / ramp_frames).min(1.0);
        self.gain = end;

        for (index, channel) in input.iter().enumerate() {
            let peak = channel
                .as_ref()
                .iter()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            if let Some(meter) = self.telemetry.peaks.get(index.min(METER_CHANNELS - 1)) {
                meter.fetch_max(peak * start.max(end));
            }
        }

        let (sample_rate, channels) = (self.sample_rate, self.channels);
        if let Some(writer) = self.writers.current_mut() {
            if sample_rate == 0 || channels == 0 {
                return;
            }
            if writer.sample_rate() != sample_rate || writer.channels() != channels {
                writer.start_stream(sample_rate, channels);
            }
            writer.write_ramped(input, start, end);
        }
    }
}
