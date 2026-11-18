//! The audio-thread side of a receiver.

use std::sync::Arc;

use audbus::handoff::HandoffReceiver;

use crate::connection::Connection;
use crate::settings::RxSettings;
use crate::telemetry::{METER_CHANNELS, PlayState, RxTelemetry};

/// Time for gain and mute changes to settle, in seconds.
const GAIN_RAMP_S: f32 = 0.02;

/// Plays the connection the [`RxLink`](crate::link::RxLink) provides. Real-time safe.
pub struct Receiver {
    connections: HandoffReceiver<Connection>,
    telemetry: Arc<RxTelemetry>,
    device_rate: u32,
    /// Gain applied at the end of the previous block.
    gain: f32,
}

impl Receiver {
    pub(crate) fn new(
        connections: HandoffReceiver<Connection>,
        telemetry: Arc<RxTelemetry>,
    ) -> Self {
        Self {
            connections,
            telemetry,
            device_rate: 0,
            gain: 0.0,
        }
    }

    /// Sets the output rate in Hz. Connections built for another rate stay silent until
    /// the link replaces them; tell the link with
    /// [`RxLink::set_device_rate`](crate::link::RxLink::set_device_rate) too.
    pub fn set_device_rate(&mut self, device_rate: u32) {
        self.device_rate = device_rate;
    }

    pub fn telemetry(&self) -> &Arc<RxTelemetry> {
        &self.telemetry
    }

    /// Fills `outputs` (equal-length channels) with bus audio.
    pub fn process<C: AsMut<[f32]>>(&mut self, outputs: &mut [C], settings: &RxSettings) {
        if self.connections.poll() {
            self.telemetry.reset_counters();
        }
        let frames = outputs
            .first_mut()
            .map_or(0, |channel| channel.as_mut().len());
        self.telemetry.set_device(self.device_rate, frames);
        let device_rate = self.device_rate;
        match self
            .connections
            .current_mut()
            .filter(|connection| connection.device_rate() == device_rate)
        {
            Some(connection) => connection.render(outputs, settings.latency_ms, &self.telemetry),
            None => {
                outputs
                    .iter_mut()
                    .for_each(|channel| channel.as_mut().fill(0.0));
                self.telemetry.set_state(PlayState::Idle);
            }
        }
        self.apply_gain(outputs, settings, frames);
    }

    /// Applies the gain with a linear ramp towards its new value and updates the meters.
    fn apply_gain<C: AsMut<[f32]>>(
        &mut self,
        outputs: &mut [C],
        settings: &RxSettings,
        frames: usize,
    ) {
        let target = if settings.mute { 0.0 } else { settings.gain };
        let ramp_frames = (GAIN_RAMP_S * self.device_rate.max(1) as f32).max(1.0);
        let progress = (frames as f32 / ramp_frames).min(1.0);
        let start = self.gain;
        let end = start + (target - start) * progress;
        let step = if frames > 0 {
            (end - start) / frames as f32
        } else {
            0.0
        };
        for (index, channel) in outputs.iter_mut().enumerate() {
            let samples = channel.as_mut();
            let mut peak = 0.0_f32;
            for (offset, sample) in samples.iter_mut().enumerate() {
                *sample *= start + step * offset as f32;
                peak = peak.max(sample.abs());
            }
            if let Some(meter) = self.telemetry.peaks.get(index.min(METER_CHANNELS - 1)) {
                meter.fetch_max(peak);
            }
        }
        self.gain = end;
    }
}
