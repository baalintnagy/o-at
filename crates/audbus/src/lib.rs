//! Named audio buses between processes.
//!
//! An O-AUD-TX transmitter [leases](Directory::lease) a bus name and writes its audio to
//! the bus with a [`BusWriter`]. Any number of O-AUD-RX receivers find buses in the
//! [`Directory`], open them with a [`BusReader`] and play them at their own pace.
//!
//! Buses live in shared memory and need no server process. Writing and reading are
//! wait-free and real-time safe; opening, leasing and dropping are not and belong on a
//! control thread, which [`handoff`] connects to the audio thread.

pub mod atomic;
mod clock;
mod directory;
mod error;
pub mod handoff;
mod layout;
mod name;
mod ring;
mod shm;

pub use crate::clock::now_ms;
pub use crate::directory::{
    BusInfo, DEFAULT_NAMESPACE, Directory, LEASE_TIMEOUT_MS, Lease, RENEW_INTERVAL_MS,
};
pub use crate::error::BusError;
pub use crate::layout::{MAX_CHANNELS, PROTOCOL_VERSION, RING_FRAMES};
pub use crate::name::{BusName, MAX_NAME_BYTES, NameError};
pub use crate::ring::{
    BusMonitor, BusReader, BusWriter, MAX_WRITE_FRAMES, Presence, SAFE_WINDOW_FRAMES, StreamFormat,
};
