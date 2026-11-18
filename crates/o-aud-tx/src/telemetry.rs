//! Transmitter levels published from the audio thread to the editor.

use audbus::atomic::AtomicF32;

/// Meter channels shown by the editor.
pub const METER_CHANNELS: usize = 2;

/// Fixed-size transmitter telemetry. Written by the audio thread, read by the editor.
#[derive(Default)]
pub struct TxTelemetry {
    /// Highest absolute sent sample per channel since the editor last read it.
    pub peaks: [AtomicF32; METER_CHANNELS],
}
