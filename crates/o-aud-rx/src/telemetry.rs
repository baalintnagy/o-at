//! Receiver state published from the audio thread to the editor through atomics.

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use audbus::atomic::AtomicF32;

/// Meter channels shown by the editor.
pub const METER_CHANNELS: usize = 2;

/// Playback state of a receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    /// No bus is connected.
    Idle,
    /// Waiting for enough audio to reach the target latency.
    Buffering,
    /// Playing with the clock locked to the transmitter.
    Playing,
}

impl PlayState {
    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::Buffering,
            2 => Self::Playing,
            _ => Self::Idle,
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Buffering => 1,
            Self::Playing => 2,
        }
    }
}

/// Fixed-size receiver telemetry. Written by the audio thread, read by editors.
#[derive(Default)]
pub struct RxTelemetry {
    state: AtomicU8,
    /// Audio queued on the bus for this receiver, in milliseconds.
    pub fill_ms: AtomicF32,
    /// Latency the receiver currently aims for, in milliseconds.
    pub target_ms: AtomicF32,
    /// Latency requested in the settings, in milliseconds.
    pub requested_ms: AtomicF32,
    /// Peak-to-peak variation of the fill over the last half second, in milliseconds.
    pub jitter_ms: AtomicF32,
    /// Transmitter clock speed relative to the output clock, in parts per million.
    pub drift_ppm: AtomicF32,
    /// Highest absolute output sample per channel since the editor last read it.
    pub peaks: [AtomicF32; METER_CHANNELS],
    pub(crate) underruns: AtomicU32,
    pub(crate) overruns: AtomicU32,
    /// Transmitter stream format in Hz and channels.
    pub(crate) source_rate: AtomicU32,
    pub(crate) source_channels: AtomicU32,
    /// Output rate in Hz and the last block length in frames.
    pub(crate) device_rate: AtomicU32,
    pub(crate) block_frames: AtomicU32,
}

impl RxTelemetry {
    pub fn state(&self) -> PlayState {
        PlayState::from_index(self.state.load(Ordering::Relaxed))
    }

    pub(crate) fn set_state(&self, state: PlayState) {
        self.state.store(state.index(), Ordering::Relaxed);
    }

    pub(crate) fn set_source(&self, sample_rate: u32, channels: usize) {
        self.source_rate.store(sample_rate, Ordering::Relaxed);
        self.source_channels
            .store(channels as u32, Ordering::Relaxed);
    }

    pub(crate) fn set_device(&self, sample_rate: u32, block_frames: usize) {
        self.device_rate.store(sample_rate, Ordering::Relaxed);
        self.block_frames
            .store(block_frames as u32, Ordering::Relaxed);
    }

    /// Clears the dropout and skip counters, for a new connection.
    pub(crate) fn reset_counters(&self) {
        self.underruns.store(0, Ordering::Relaxed);
        self.overruns.store(0, Ordering::Relaxed);
    }

    pub(crate) fn count(counter: &AtomicU32) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Transmitter stream rate in Hz, zero before the first connection.
    pub fn source_rate(&self) -> u32 {
        self.source_rate.load(Ordering::Relaxed)
    }

    pub fn source_channels(&self) -> u32 {
        self.source_channels.load(Ordering::Relaxed)
    }

    /// Output rate in Hz, zero before processing starts.
    pub fn device_rate(&self) -> u32 {
        self.device_rate.load(Ordering::Relaxed)
    }

    /// Length of the last output block in frames.
    pub fn block_frames(&self) -> u32 {
        self.block_frames.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> u32 {
        self.underruns.load(Ordering::Relaxed)
    }

    pub fn overruns(&self) -> u32 {
        self.overruns.load(Ordering::Relaxed)
    }
}
