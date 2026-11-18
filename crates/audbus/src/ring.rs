//! The audio ring of a bus: one writer, any number of readers.
//!
//! Readers never modify the ring, so each one keeps its own read position and several
//! receivers can play the same bus. A reader that falls more than the ring's safe
//! window behind the writer detects it and resynchronizes.
//!
//! Samples are stored as `AtomicU32` bits with relaxed ordering, which compiles to
//! plain loads and stores; the write position is published with release ordering after
//! the samples.

use std::sync::Arc;
use std::sync::atomic::{Ordering, fence};

use crate::clock::{is_stale, new_token, now_ms};
use crate::directory::{Directory, LEASE_TIMEOUT_MS, Lease};
use crate::error::BusError;
use crate::layout::{
    BUS_MAGIC, BusLayout, MAX_CHANNELS, MagicCheck, READER_SLOTS, RING_FRAMES, RING_MASK,
    check_magic,
};
use crate::name::BusName;
use crate::shm::{OpenMode, Shared};

/// Frames a reader may lag behind the writer before samples may be overwritten while
/// it copies them. The rest of the ring is headroom for the writer's next block.
pub const SAFE_WINDOW_FRAMES: u64 = (RING_FRAMES - RING_FRAMES / 4) as u64;

/// Largest block a writer stores at once; longer blocks keep only their end.
pub const MAX_WRITE_FRAMES: usize = RING_FRAMES / 4;

/// Format of the stream on a bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFormat {
    /// Changes whenever the stream restarts or its format changes.
    pub epoch: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    pub channels: usize,
}

type BusRegion = Arc<Shared<BusLayout>>;

fn open_region(
    directory: &Directory,
    name: &BusName,
    mode: OpenMode,
) -> Result<BusRegion, BusError> {
    // SAFETY: `BusLayout` satisfies the `Shared` contract (see `layout`).
    let shared: Shared<BusLayout> = unsafe { Shared::open(&directory.bus_region(name), mode)? };
    match check_magic(&shared.get().magic, BUS_MAGIC) {
        MagicCheck::Valid => Ok(Arc::new(shared)),
        MagicCheck::Incompatible => Err(BusError::Incompatible),
    }
}

/// Write side of a bus. Its methods are real-time safe, but dropping it unmaps memory,
/// so drop it outside the audio thread.
pub struct BusWriter {
    region: BusRegion,
    sample_rate: u32,
    channels: usize,
}

impl BusWriter {
    /// Creates or reopens the region of a leased bus. Receivers that kept the region open
    /// across a transmitter restart continue on the same ring.
    pub fn create(lease: &Lease) -> Result<Self, BusError> {
        let region = open_region(lease.directory(), lease.name(), OpenMode::CreateOrOpen)?;
        Ok(Self {
            region,
            sample_rate: 0,
            channels: 0,
        })
    }

    /// Starts a new stream, making receivers resynchronize.
    pub fn start_stream(&mut self, sample_rate: u32, channels: usize) {
        let bus = self.region.get();
        let channels = channels.clamp(1, MAX_CHANNELS);
        // Make the epoch odd while the format changes.
        let epoch = bus.epoch.load(Ordering::Relaxed);
        let odd = if epoch % 2 == 0 {
            epoch.wrapping_add(1)
        } else {
            epoch
        };
        bus.epoch.store(odd, Ordering::Relaxed);
        fence(Ordering::Release);
        bus.sample_rate.store(sample_rate, Ordering::Relaxed);
        bus.channels.store(channels as u32, Ordering::Relaxed);
        bus.epoch.store(odd.wrapping_add(1), Ordering::Release);
        self.sample_rate = sample_rate;
        self.channels = channels;
    }

    /// Stream rate in Hz, zero before [`start_stream`](Self::start_stream).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Stream channels, zero before [`start_stream`](Self::start_stream).
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Appends one block, scaled by `gain`. Stream channels without a matching input
    /// channel repeat the last input channel, so a mono input fills a stereo stream.
    pub fn write<C: AsRef<[f32]>>(&self, input: &[C], gain: f32) {
        self.write_ramped(input, gain, gain);
    }

    /// Like [`write`](Self::write), with the gain ramping linearly from `start_gain` to
    /// `end_gain` over the block.
    pub fn write_ramped<C: AsRef<[f32]>>(&self, input: &[C], start_gain: f32, end_gain: f32) {
        let Some(first) = input.first() else {
            return;
        };
        if self.channels == 0 {
            return;
        }
        let frames = first.as_ref().len();
        let skip = frames.saturating_sub(MAX_WRITE_FRAMES);
        let bus = self.region.get();
        let start = bus.write_position.load(Ordering::Relaxed);
        let step = if frames > 0 {
            (end_gain - start_gain) / frames as f32
        } else {
            0.0
        };
        for channel in 0..self.channels {
            let source = input[channel.min(input.len() - 1)].as_ref();
            let ring = &bus.samples[channel * RING_FRAMES..(channel + 1) * RING_FRAMES];
            for (offset, sample) in source.iter().enumerate().skip(skip) {
                let gain = start_gain + step * offset as f32;
                let index = ((start + (offset - skip) as u64) & RING_MASK) as usize;
                ring[index].store((sample * gain).to_bits(), Ordering::Relaxed);
            }
        }
        let written = (frames - skip) as u64;
        bus.write_position.store(start + written, Ordering::Release);
    }

    /// A handle for observing the bus from another thread.
    pub fn monitor(&self) -> BusMonitor {
        BusMonitor {
            region: Arc::clone(&self.region),
        }
    }
}

/// Observes a bus without reading audio.
#[derive(Clone)]
pub struct BusMonitor {
    region: BusRegion,
}

impl BusMonitor {
    /// Receivers that renewed their presence recently.
    pub fn listeners(&self) -> usize {
        let now = now_ms();
        self.region
            .get()
            .readers
            .iter()
            .filter(|slot| {
                slot.token.load(Ordering::Acquire) != 0
                    && !is_stale(
                        slot.heartbeat_ms.load(Ordering::Acquire),
                        now,
                        LEASE_TIMEOUT_MS,
                    )
            })
            .count()
    }

    pub fn write_position(&self) -> u64 {
        self.region.get().write_position.load(Ordering::Acquire)
    }
}

/// Read side of a bus. Its methods are real-time safe, but dropping it may unmap
/// memory, so drop it outside the audio thread. Clones share the mapping.
#[derive(Clone)]
pub struct BusReader {
    region: BusRegion,
}

impl BusReader {
    /// Opens a bus that a transmitter created.
    pub fn open(directory: &Directory, name: &BusName) -> Result<Self, BusError> {
        let region = open_region(directory, name, OpenMode::Existing)?;
        Ok(Self { region })
    }

    /// The current stream format, or `None` before the stream starts or while it changes.
    pub fn format(&self) -> Option<StreamFormat> {
        let bus = self.region.get();
        let epoch = bus.epoch.load(Ordering::Acquire);
        let sample_rate = bus.sample_rate.load(Ordering::Relaxed);
        let channels = bus.channels.load(Ordering::Relaxed) as usize;
        fence(Ordering::Acquire);
        let stable = epoch % 2 == 0 && bus.epoch.load(Ordering::Relaxed) == epoch;
        (stable && sample_rate > 0 && (1..=MAX_CHANNELS).contains(&channels)).then_some(
            StreamFormat {
                epoch,
                sample_rate,
                channels,
            },
        )
    }

    /// Total frames written to the bus so far.
    pub fn write_position(&self) -> u64 {
        self.region.get().write_position.load(Ordering::Acquire)
    }

    /// Copies `frames` frames starting at `position` into `output`, planar with a
    /// stride of `frames` per channel, for the first `channels` channels.
    ///
    /// Returns `false` if the data is unusable: not written yet, overwritten while it
    /// was copied, or from a different stream than `epoch`.
    pub fn read(
        &self,
        position: u64,
        frames: usize,
        channels: usize,
        epoch: u32,
        output: &mut [f32],
    ) -> bool {
        let bus = self.region.get();
        let end = position + frames as u64;
        let available = bus.write_position.load(Ordering::Acquire);
        if end > available || available - position > SAFE_WINDOW_FRAMES {
            return false;
        }
        for channel in 0..channels.min(MAX_CHANNELS) {
            let ring = &bus.samples[channel * RING_FRAMES..(channel + 1) * RING_FRAMES];
            let target = &mut output[channel * frames..(channel + 1) * frames];
            for (offset, sample) in target.iter_mut().enumerate() {
                let index = ((position + offset as u64) & RING_MASK) as usize;
                *sample = f32::from_bits(ring[index].load(Ordering::Relaxed));
            }
        }
        // Order the sample loads before re-checking the writer's progress.
        fence(Ordering::Acquire);
        let after = bus.write_position.load(Ordering::Relaxed);
        after - position <= SAFE_WINDOW_FRAMES && bus.epoch.load(Ordering::Relaxed) == epoch
    }

    /// Registers this receiver so the transmitter can count its listeners.
    pub fn presence(&self) -> Presence {
        Presence {
            region: Arc::clone(&self.region),
            token: new_token(),
            slot: None,
        }
    }
}

/// A receiver's registration on a bus, renewed from a non-real-time thread.
pub struct Presence {
    region: BusRegion,
    token: u64,
    slot: Option<usize>,
}

impl Presence {
    /// Claims or renews a reader slot. Does nothing when all slots are taken.
    pub fn renew(&mut self) {
        let now = now_ms();
        let readers = &self.region.get().readers;
        if let Some(slot) = self.slot {
            if readers[slot].token.load(Ordering::Acquire) == self.token {
                readers[slot].heartbeat_ms.store(now, Ordering::Release);
                return;
            }
        }
        self.slot = (0..READER_SLOTS).find(|&index| {
            let slot = &readers[index];
            let token = slot.token.load(Ordering::Acquire);
            let free = token == 0
                || is_stale(
                    slot.heartbeat_ms.load(Ordering::Acquire),
                    now,
                    LEASE_TIMEOUT_MS,
                );
            free && slot
                .token
                .compare_exchange(token, self.token, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        });
        if let Some(slot) = self.slot {
            readers[slot].heartbeat_ms.store(now, Ordering::Release);
        }
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        if let Some(slot) = self.slot {
            let _ = self.region.get().readers[slot].token.compare_exchange(
                self.token,
                0,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}
