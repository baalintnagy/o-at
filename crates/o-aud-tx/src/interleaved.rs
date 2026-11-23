//! Sends interleaved device buffers, as audio input callbacks deliver them, through a
//! [`TxPort`].

use std::array;

use audbus::MAX_CHANNELS;

use crate::port::TxPort;

/// Largest block sent at once; longer device buffers are split, in frames.
pub const MAX_BLOCK_FRAMES: usize = 8_192;

/// Splits interleaved buffers into planar blocks for the port. Real-time safe after
/// construction.
pub struct InterleavedSender {
    port: TxPort,
    /// Channels of the device buffers, at least one.
    device_channels: usize,
    /// One scratch channel per sent channel.
    planar: Vec<Vec<f32>>,
}

impl InterleavedSender {
    /// Sends buffers of `device_channels` interleaved channels at `sample_rate` Hz. The
    /// first [`MAX_CHANNELS`] channels reach the bus. Also tell the link the format with
    /// [`TxLink::set_format`](crate::link::TxLink::set_format), using
    /// [`channels`](Self::channels).
    pub fn new(mut port: TxPort, sample_rate: u32, device_channels: usize) -> Self {
        let device_channels = device_channels.max(1);
        let sent = device_channels.min(MAX_CHANNELS);
        port.set_format(sample_rate, sent);
        Self {
            port,
            device_channels,
            planar: vec![vec![0.0; MAX_BLOCK_FRAMES]; sent],
        }
    }

    /// Channels sent to the bus.
    pub fn channels(&self) -> usize {
        self.planar.len()
    }

    /// Sends an interleaved buffer at `gain` (a linear factor), converting samples with
    /// `to_f32`. A trailing partial frame is ignored. Real-time safe.
    pub fn send<T: Copy>(&mut self, data: &[T], gain: f32, to_f32: impl Fn(T) -> f32) {
        let device_channels = self.device_channels;
        for block in data.chunks(MAX_BLOCK_FRAMES * device_channels) {
            let frames = block.len() / device_channels;
            for (frame, samples) in block.chunks_exact(device_channels).enumerate() {
                for (channel, &sample) in self.planar.iter_mut().zip(samples) {
                    channel[frame] = to_f32(sample);
                }
            }
            self.process(frames, gain);
        }
    }

    /// Sends `frames` frames of silence, for a capture that delivers nothing while its
    /// source is quiet. `gain` keeps the port's gain ramp in step. Real-time safe.
    pub fn send_silence(&mut self, frames: u64, gain: f32) {
        let mut due = frames;
        while due > 0 {
            let block = due.min(MAX_BLOCK_FRAMES as u64) as usize;
            for channel in &mut self.planar {
                channel[..block].fill(0.0);
            }
            self.process(block, gain);
            due -= block as u64;
        }
    }

    /// Sends the first `frames` frames of the scratch channels.
    fn process(&mut self, frames: usize, gain: f32) {
        let planar = &self.planar;
        let views: [&[f32]; MAX_CHANNELS] =
            array::from_fn(|index| planar.get(index).map_or(&[][..], |c| &c[..frames]));
        self.port.process(&views[..planar.len()], gain);
    }
}
