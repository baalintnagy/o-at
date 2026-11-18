//! Memory layouts shared between processes.
//!
//! Every type here is `#[repr(C)]`, made of atomics only and valid when zero-filled, as
//! [`Shared`](crate::shm::Shared) requires. The layouts are versioned through their
//! region names and magic numbers; change [`PROTOCOL_VERSION`] when changing them.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Version of the shared memory protocol, part of every region name.
pub const PROTOCOL_VERSION: u32 = 1;

/// Number of buses that can exist at the same time.
pub const DIRECTORY_SLOTS: usize = 64;

/// Maximum channel count of a bus.
pub const MAX_CHANNELS: usize = 8;

/// Ring buffer length per channel in frames (1.37 s at 48 kHz).
pub const RING_FRAMES: usize = 1 << 16;
pub(crate) const RING_MASK: u64 = RING_FRAMES as u64 - 1;

/// Number of receivers counted per bus.
pub const READER_SLOTS: usize = 16;

/// Bytes stored for a bus name, a multiple of eight.
pub(crate) const NAME_WORDS: usize = 4;

pub(crate) const DIRECTORY_MAGIC: u64 = u64::from_le_bytes(*b"AUDBUSD1");
pub(crate) const BUS_MAGIC: u64 = u64::from_le_bytes(*b"AUDBUSR1");

/// Result of checking or stamping a region's magic number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MagicCheck {
    Valid,
    Incompatible,
}

/// Stamps a zero-filled region with `magic`, or verifies an existing stamp.
pub(crate) fn check_magic(slot: &AtomicU64, magic: u64) -> MagicCheck {
    match slot.compare_exchange(0, magic, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => MagicCheck::Valid,
        Err(existing) if existing == magic => MagicCheck::Valid,
        Err(_) => MagicCheck::Incompatible,
    }
}

/// The directory lists the buses that exist and which transmitter owns each one.
#[repr(C)]
pub(crate) struct DirectoryLayout {
    pub(crate) magic: AtomicU64,
    pub(crate) slots: [DirectorySlot; DIRECTORY_SLOTS],
}

#[repr(C)]
pub(crate) struct DirectorySlot {
    /// Lease token of the owning transmitter, zero while free.
    pub(crate) owner: AtomicU64,
    /// Last lease renewal in milliseconds since the Unix epoch.
    pub(crate) heartbeat_ms: AtomicU64,
    /// Sequence lock for the fields below: odd while they are written.
    pub(crate) sequence: AtomicU32,
    pub(crate) name_len: AtomicU32,
    /// UTF-8 bus name, little-endian packed.
    pub(crate) name: [AtomicU64; NAME_WORDS],
    /// Stream format in Hz and channels, zero before the stream starts.
    pub(crate) sample_rate: AtomicU32,
    pub(crate) channels: AtomicU32,
    pub(crate) process_id: AtomicU32,
    pub(crate) _reserved: AtomicU32,
}

/// One bus: a single-writer, multiple-reader ring of planar `f32` samples.
#[repr(C)]
pub(crate) struct BusLayout {
    pub(crate) magic: AtomicU64,
    /// Total frames written since the region was created. Readers derive ring indices
    /// from it; it never decreases.
    pub(crate) write_position: AtomicU64,
    /// Sequence lock for the stream format: odd while it changes. Every format change or
    /// stream restart moves it, so readers resynchronize.
    pub(crate) epoch: AtomicU32,
    pub(crate) sample_rate: AtomicU32,
    pub(crate) channels: AtomicU32,
    pub(crate) _reserved: AtomicU32,
    pub(crate) readers: [ReaderSlot; READER_SLOTS],
    /// `MAX_CHANNELS` rings of `RING_FRAMES` samples, as `f32` bits.
    pub(crate) samples: [AtomicU32; MAX_CHANNELS * RING_FRAMES],
}

#[repr(C)]
pub(crate) struct ReaderSlot {
    pub(crate) token: AtomicU64,
    pub(crate) heartbeat_ms: AtomicU64,
}

/// Name of the directory region of `namespace`.
pub(crate) fn directory_region_name(namespace: &str) -> String {
    format!("{namespace}.v{PROTOCOL_VERSION}.directory")
}

/// Name of a bus region for a name's object suffix.
pub(crate) fn bus_region_name(namespace: &str, suffix: &str) -> String {
    format!("{namespace}.v{PROTOCOL_VERSION}.bus.{suffix}")
}

/// Packs a name of at most `8 * NAME_WORDS` bytes into words.
pub(crate) fn pack_name(name: &str) -> [u64; NAME_WORDS] {
    let mut words = [0; NAME_WORDS];
    for (index, chunk) in name.as_bytes().chunks(8).take(NAME_WORDS).enumerate() {
        let mut bytes = [0; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words[index] = u64::from_le_bytes(bytes);
    }
    words
}

/// Unpacks a name; invalid UTF-8 yields `None`.
pub(crate) fn unpack_name(words: &[u64; NAME_WORDS], len: usize) -> Option<String> {
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let len = len.min(bytes.len());
    String::from_utf8(bytes[..len].to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::{NAME_WORDS, pack_name, unpack_name};

    #[test]
    fn names_round_trip_through_words() {
        let name = "Bühne – Monitor Mix 1";
        assert!(name.len() <= 8 * NAME_WORDS);
        assert_eq!(
            unpack_name(&pack_name(name), name.len()).as_deref(),
            Some(name)
        );
    }
}
