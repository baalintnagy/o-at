//! The audio thread's side of the analyzer: copies the input into a lock-free ring for
//! the analysis worker.
//!
//! [`Capture::push`] is called from `Plugin::process`. It never allocates, locks or
//! waits; when the worker falls behind, frames that do not fit are counted as dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

/// Capacity of the sample ring in stereo frames (2.7 s at 48 kHz).
pub const RING_FRAMES: usize = 131_072;

/// Stream format and counters written by the audio thread.
#[derive(Debug, Default)]
pub struct StreamState {
    /// Sample rate in Hz, zero before the host activates the plugin.
    pub sample_rate: AtomicU32,
    /// Input channels: one or two.
    pub channels: AtomicU32,
    /// Frames that did not fit into the ring.
    pub dropped_frames: AtomicU64,
    /// Frames written to the ring.
    pub captured_frames: AtomicU64,
}

impl StreamState {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    pub fn channels(&self) -> u32 {
        self.channels.load(Ordering::Relaxed)
    }
}

/// Writes input frames into the ring.
pub struct Capture {
    producer: Producer<[f32; 2]>,
    stream: Arc<StreamState>,
}

/// Creates the capture side and the consumer the worker reads.
pub fn capture() -> (Capture, Consumer<[f32; 2]>, Arc<StreamState>) {
    let (producer, consumer) = RingBuffer::new(RING_FRAMES);
    let stream = Arc::new(StreamState::default());
    let capture = Capture {
        producer,
        stream: Arc::clone(&stream),
    };
    (capture, consumer, stream)
}

impl Capture {
    /// Sets the stream format. Called from `initialize`.
    pub fn set_format(&self, sample_rate: u32, channels: u32) {
        self.stream
            .channels
            .store(channels.clamp(1, 2), Ordering::Relaxed);
        self.stream
            .sample_rate
            .store(sample_rate, Ordering::Release);
    }

    /// Copies one block of planar input. A single channel fills both sides of a frame.
    /// Real-time safe.
    pub fn push<C: AsRef<[f32]>>(&mut self, channels: &[C]) {
        let Some(left) = channels.first().map(AsRef::as_ref) else {
            return;
        };
        let right = channels.get(1).map_or(left, AsRef::as_ref);
        let frames = left.len().min(right.len());
        let fits = frames.min(self.producer.slots());
        if fits > 0 {
            // `fits` never exceeds the free slots, so the chunk is always available.
            if let Ok(chunk) = self.producer.write_chunk_uninit(fits) {
                chunk.fill_from_iter(left.iter().zip(right).map(|(&l, &r)| [l, r]));
            }
            self.stream
                .captured_frames
                .fetch_add(fits as u64, Ordering::Relaxed);
        }
        if frames > fits {
            self.stream
                .dropped_frames
                .fetch_add((frames - fits) as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::capture;

    #[test]
    fn mono_fills_both_sides_and_overflow_is_counted() {
        let (mut capture, mut consumer, stream) = capture();
        capture.set_format(48_000, 1);
        let block = [0.5_f32; 4];
        capture.push(&[&block[..]]);
        assert_eq!(consumer.pop(), Ok([0.5, 0.5]));
        let big = vec![0.0_f32; super::RING_FRAMES + 10];
        capture.push(&[&big[..], &big[..]]);
        assert_eq!(stream.dropped_frames.load(Ordering::Relaxed), 13);
        assert_eq!(stream.channels(), 1);
    }
}
