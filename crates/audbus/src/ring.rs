//! The audio ring of a bus: one writer, any number of readers.
//!
//! Readers never modify the ring, so each one keeps its own read position and several
//! receivers can play the same bus. A reader that falls more than the ring's safe
//! window behind the writer detects it and resynchronizes.
//!
//! Samples are stored as `AtomicU32` bits with relaxed ordering, which compiles to
//! plain loads and stores; the write position is published with release ordering after
//! the samples.

use std::ops::RangeInclusive;
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

/// Stream sample rates in Hz that readers accept. Other values in shared memory are
/// corrupt, and a stream with them reads as not started.
pub const SAMPLE_RATES_HZ: RangeInclusive<u32> = 1_000..=1_536_000;

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
        let odd = if epoch.is_multiple_of(2) {
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
                let index = (start.wrapping_add((offset - skip) as u64) & RING_MASK) as usize;
                ring[index].store((sample * gain).to_bits(), Ordering::Relaxed);
            }
        }
        // Another process can write any position; wrap rather than overflow.
        let written = (frames - skip) as u64;
        bus.write_position
            .store(start.wrapping_add(written), Ordering::Release);
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

    /// The current stream format, or `None` before the stream starts, while it changes or
    /// when its rate or channel count is out of range.
    pub fn format(&self) -> Option<StreamFormat> {
        let bus = self.region.get();
        let epoch = bus.epoch.load(Ordering::Acquire);
        let sample_rate = bus.sample_rate.load(Ordering::Relaxed);
        let channels = bus.channels.load(Ordering::Relaxed) as usize;
        fence(Ordering::Acquire);
        let stable = epoch.is_multiple_of(2) && bus.epoch.load(Ordering::Relaxed) == epoch;
        (stable && SAMPLE_RATES_HZ.contains(&sample_rate) && (1..=MAX_CHANNELS).contains(&channels))
            .then_some(StreamFormat {
                epoch,
                sample_rate,
                channels,
            })
    }

    /// Total frames written to the bus so far.
    pub fn write_position(&self) -> u64 {
        self.region.get().write_position.load(Ordering::Acquire)
    }

    /// Copies `frames` frames starting at `position` into `output`, planar with a
    /// stride of `frames` per channel, for the first `channels` channels.
    ///
    /// Returns `false` if the data is unusable: not written yet, overwritten while it
    /// was copied, or from a different stream than `epoch`. The write position comes from
    /// another process, so any value of it, also one that moves backwards, only makes the
    /// read fail.
    ///
    /// `output` must hold at least `frames` samples for each of the first `channels`
    /// channels (up to [`MAX_CHANNELS`]).
    pub fn read(
        &self,
        position: u64,
        frames: usize,
        channels: usize,
        epoch: u32,
        output: &mut [f32],
    ) -> bool {
        let bus = self.region.get();
        let Some(end) = position.checked_add(frames as u64) else {
            return false;
        };
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
        after
            .checked_sub(position)
            .is_some_and(|lag| lag <= SAFE_WINDOW_FRAMES)
            && bus.epoch.load(Ordering::Relaxed) == epoch
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
        if let Some(slot) = self.slot
            && readers[slot].token.load(Ordering::Acquire) == self.token
        {
            readers[slot].heartbeat_ms.store(now, Ordering::Release);
            return;
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

#[cfg(test)]
mod tests {
    use std::process;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{BusReader, BusWriter, SAFE_WINDOW_FRAMES, SAMPLE_RATES_HZ};
    use crate::directory::{Directory, Lease};
    use crate::layout::MAX_CHANNELS;
    use crate::name::BusName;

    /// Deterministic xorshift numbers, so failures reproduce.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, limit: u64) -> u64 {
            self.next() % limit
        }

        /// A position near the edges of the `u64` range, near `around` or anywhere.
        fn position(&mut self, around: u64) -> u64 {
            match self.below(6) {
                0 => self.below(4),
                1 => u64::MAX - self.below(4),
                2 => around.wrapping_add(self.below(1_024)),
                3 => around.wrapping_sub(self.below(1_024)),
                4 => around.wrapping_sub(SAFE_WINDOW_FRAMES - 2 + self.below(4)),
                _ => self.next(),
            }
        }
    }

    /// A bus in a namespace private to one test.
    fn bus(test: &str) -> (Lease, BusWriter, BusReader) {
        let directory = Directory::open_namespace(&format!("audbus-ring-{test}-{}", process::id()))
            .expect("directory opens");
        let lease = directory
            .lease(&BusName::new("Ring").expect("valid name"))
            .expect("free name");
        let writer = BusWriter::create(&lease).expect("bus opens");
        let reader = BusReader::open(&directory, lease.name()).expect("bus exists");
        (lease, writer, reader)
    }

    /// Another process may write anything into the shared region: positions near the
    /// ends of the range or moving backwards, odd epochs, impossible formats. Reading and
    /// writing must reject such data without panicking, as a panic on an audio thread
    /// takes the host down.
    #[test]
    fn corrupt_shared_state_never_panics() {
        let (_lease, mut writer, reader) = bus("corrupt");
        writer.start_stream(48_000, 2);
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        let mut output = vec![0.0; MAX_CHANNELS * 4_096];
        let input = [vec![0.25_f32; 512], vec![-0.25; 512]];
        for _ in 0..20_000 {
            let bus = writer.region.get();
            let previous = bus.write_position.load(Ordering::Relaxed);
            match random.below(4) {
                0 => bus
                    .write_position
                    .store(random.position(previous), Ordering::Relaxed),
                1 => bus.epoch.store(random.next() as u32, Ordering::Relaxed),
                2 => bus.channels.store(
                    [0, 1, 8, 9, u32::MAX][random.below(5) as usize],
                    Ordering::Relaxed,
                ),
                _ => bus
                    .sample_rate
                    .store(random.next() as u32, Ordering::Relaxed),
            }
            if let Some(format) = reader.format() {
                assert!((1..=MAX_CHANNELS).contains(&format.channels));
                assert!(SAMPLE_RATES_HZ.contains(&format.sample_rate));
            }
            let available = reader.write_position();
            let position = random.position(available);
            let frames = random.below(4_097) as usize;
            let channels = random.below(MAX_CHANNELS as u64 + 4) as usize;
            let epoch = bus.epoch.load(Ordering::Relaxed);
            let read = reader.read(position, frames, channels, epoch, &mut output);
            if read {
                let end = position.checked_add(frames as u64).expect("no overflow");
                assert!(end <= available, "never reads unwritten frames");
            }
            writer.write(&input, 1.0);
        }
    }

    #[test]
    fn formats_with_impossible_rates_read_as_not_started() {
        let (_lease, mut writer, reader) = bus("rates");
        for (rate, valid) in [(0, false), (999, false), (8_000, true), (2_000_000, false)] {
            writer.start_stream(rate, 2);
            assert_eq!(reader.format().is_some(), valid, "{rate} Hz");
        }
    }

    #[test]
    fn reads_fail_when_the_write_position_moves_backwards() {
        let (_lease, mut writer, reader) = bus("backwards");
        writer.start_stream(48_000, 1);
        let block = [vec![0.5_f32; 1_024]];
        (0..4).for_each(|_| writer.write(&block, 1.0));
        let epoch = reader.format().expect("stream started").epoch;
        let mut output = vec![0.0; 1_024];
        assert!(reader.read(2_048, 1_024, 1, epoch, &mut output));
        assert_eq!(output[0], 0.5);
        writer
            .region
            .get()
            .write_position
            .store(1_000, Ordering::Relaxed);
        assert!(!reader.read(2_048, 1_024, 1, epoch, &mut output));
        writer
            .region
            .get()
            .write_position
            .store(u64::MAX, Ordering::Relaxed);
        assert!(!reader.read(u64::MAX - 10, 1_024, 1, epoch, &mut output));
        writer.write(&block, 1.0);
        assert!(reader.write_position() < u64::MAX, "the position wraps");
    }

    /// A position that moves backwards between a read's two checks of it, as when another
    /// process corrupts it, fails the read instead of underflowing.
    #[test]
    fn positions_moving_backwards_during_reads_fail_them() {
        let (_lease, mut writer, reader) = bus("rewind");
        writer.start_stream(48_000, 1);
        let region = Arc::clone(&writer.region);
        let done = Arc::new(AtomicBool::new(false));
        let corrupter = {
            let done = Arc::clone(&done);
            thread::spawn(move || {
                let mut high = false;
                while !done.load(Ordering::Relaxed) {
                    high = !high;
                    let position = if high { 1 << 20 } else { 0 };
                    region
                        .get()
                        .write_position
                        .store(position, Ordering::Relaxed);
                }
            })
        };
        let epoch = reader.format().expect("stream started").epoch;
        let mut output = vec![0.0; 64];
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(200) {
            let position = reader.write_position().saturating_sub(64);
            let _ = reader.read(position, 64, 1, epoch, &mut output);
        }
        done.store(true, Ordering::Relaxed);
        corrupter.join().expect("corrupting thread");
    }

    /// The stream format is published under a sequence lock: readers on other threads
    /// see either complete format, never the rate of one with the channels of another.
    #[test]
    fn concurrent_format_changes_stay_consistent() {
        const FORMATS: [(u32, usize); 2] = [(44_100, 1), (96_000, MAX_CHANNELS)];
        let (lease, mut writer, reader) = bus("format");
        writer.start_stream(FORMATS[0].0, FORMATS[0].1);
        let done = Arc::new(AtomicBool::new(false));
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let reader = reader.clone();
                let done = Arc::clone(&done);
                thread::spawn(move || {
                    let mut seen = 0_u64;
                    while !done.load(Ordering::Relaxed) {
                        if let Some(format) = reader.format() {
                            assert!(
                                FORMATS.contains(&(format.sample_rate, format.channels)),
                                "torn format {format:?}"
                            );
                            assert_eq!(format.epoch % 2, 0);
                            seen += 1;
                        }
                    }
                    seen
                })
            })
            .collect();
        let start = Instant::now();
        let mut index = 0;
        while start.elapsed() < Duration::from_millis(300) {
            index = 1 - index;
            writer.start_stream(FORMATS[index].0, FORMATS[index].1);
        }
        done.store(true, Ordering::Relaxed);
        let seen: u64 = readers
            .into_iter()
            .map(|reader| reader.join().expect("reader thread"))
            .sum();
        assert!(seen > 0, "readers saw formats");
        drop(lease);
    }
}
