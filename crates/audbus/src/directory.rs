//! Discovery of buses and exclusive ownership of bus names.
//!
//! A transmitter holds a [`Lease`] on a directory slot for as long as it owns a name and
//! renews it periodically. Leases that are not renewed expire, so buses of crashed
//! processes disappear and their names become free again.

use std::array::from_fn;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{Ordering, fence};

use crate::clock::{is_stale, new_token, now_ms};
use crate::error::BusError;
use crate::layout::{
    DIRECTORY_MAGIC, DIRECTORY_SLOTS, DirectoryLayout, DirectorySlot, MagicCheck, NAME_WORDS,
    bus_region_name, check_magic, directory_region_name, pack_name, unpack_name,
};
use crate::name::{BusName, MAX_NAME_BYTES};
use crate::shm::{OpenMode, Shared};

/// A lease that is not renewed for this long expires, in milliseconds.
pub const LEASE_TIMEOUT_MS: u64 = 1_500;

/// Recommended lease renewal interval in milliseconds.
pub const RENEW_INTERVAL_MS: u64 = 200;

/// Attempts to read a slot consistently while its owner rewrites it.
const SNAPSHOT_ATTEMPTS: usize = 16;

/// A live bus as listed in the directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusInfo {
    pub name: String,
    /// Stream rate in Hz, zero before the transmitter starts streaming.
    pub sample_rate: u32,
    /// Stream channels, zero before the transmitter starts streaming.
    pub channels: u32,
    pub process_id: u32,
}

/// Namespace of the buses shared by O-AUD-TX and O-AUD-RX.
pub const DEFAULT_NAMESPACE: &str = "audbus";

/// A bus directory. Buses in different namespaces are independent.
pub struct Directory {
    shared: Shared<DirectoryLayout>,
    namespace: String,
}

impl Directory {
    /// Opens the default directory, creating it if no other process has.
    pub fn open() -> Result<Arc<Self>, BusError> {
        Self::open_namespace(DEFAULT_NAMESPACE)
    }

    /// Opens the directory of `namespace`, which must be a short ASCII identifier. Tests
    /// use private namespaces to stay isolated from running applications.
    pub fn open_namespace(namespace: &str) -> Result<Arc<Self>, BusError> {
        // SAFETY: `DirectoryLayout` satisfies the `Shared` contract (see `layout`).
        let shared: Shared<DirectoryLayout> =
            unsafe { Shared::open(&directory_region_name(namespace), OpenMode::CreateOrOpen)? };
        match check_magic(&shared.get().magic, DIRECTORY_MAGIC) {
            MagicCheck::Valid => Ok(Arc::new(Self {
                shared,
                namespace: namespace.to_owned(),
            })),
            MagicCheck::Incompatible => Err(BusError::Incompatible),
        }
    }

    /// Region name of the bus `name` in this directory's namespace.
    pub(crate) fn bus_region(&self, name: &BusName) -> String {
        bus_region_name(&self.namespace, &name.object_suffix())
    }

    /// Live buses, sorted by name.
    pub fn buses(&self) -> Vec<BusInfo> {
        let now = now_ms();
        let mut buses: Vec<BusInfo> = self
            .live_slots(now)
            .map(|(_, snapshot)| snapshot.info)
            .collect();
        buses.sort_by_key(|info| info.name.to_lowercase());
        buses
    }

    /// The live bus named `name`, if any.
    pub fn find(&self, name: &BusName) -> Option<BusInfo> {
        self.live_slots(now_ms())
            .map(|(_, snapshot)| snapshot.info)
            .find(|info| name.matches(&info.name))
    }

    /// The first name of the form `"{prefix} {n}"` that no live bus uses.
    pub fn suggest_name(&self, prefix: &str) -> BusName {
        self.first_free(prefix, 1)
            .unwrap_or_else(|| BusName::new("Bus").expect("constant name is valid"))
    }

    /// A free variant of a taken name: a trailing number counts up (`"Bus 1"` becomes
    /// `"Bus 2"`), other names get one (`"Drums"` becomes `"Drums 2"`). The base is
    /// shortened to fit the length limit.
    pub fn free_variant(&self, name: &BusName) -> BusName {
        let text = name.as_str();
        let (mut base, first) = match text.rsplit_once(' ') {
            Some((base, number)) if !base.trim().is_empty() => match number.parse::<u32>() {
                Ok(number) => (base.to_owned(), number.saturating_add(1)),
                Err(_) => (text.to_owned(), 2),
            },
            _ => (text.to_owned(), 2),
        };
        // Leave room for a space and up to three digits.
        while base.len() + 4 > MAX_NAME_BYTES {
            base.pop();
        }
        self.first_free(base.trim_end(), first)
            .unwrap_or_else(|| self.suggest_name("Bus"))
    }

    fn first_free(&self, prefix: &str, first: u32) -> Option<BusName> {
        let taken = self.buses();
        (first..first + DIRECTORY_SLOTS as u32 + 1)
            .filter_map(|number| BusName::new(&format!("{prefix} {number}")).ok())
            .find(|candidate| !taken.iter().any(|info| candidate.matches(&info.name)))
    }

    /// Takes exclusive ownership of `name`.
    pub fn lease(self: &Arc<Self>, name: &BusName) -> Result<Lease, BusError> {
        let now = now_ms();
        if self
            .live_slots(now)
            .any(|(_, s)| name.matches(&s.info.name))
        {
            return Err(BusError::NameTaken);
        }
        let token = new_token();
        let slots = &self.shared.get().slots;
        for (index, slot) in slots.iter().enumerate() {
            let owner = slot.owner.load(Ordering::Acquire);
            let free = owner == 0
                || is_stale(
                    slot.heartbeat_ms.load(Ordering::Acquire),
                    now,
                    LEASE_TIMEOUT_MS,
                );
            if !free
                || slot
                    .owner
                    .compare_exchange(owner, token, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
            {
                continue;
            }
            slot.heartbeat_ms.store(now, Ordering::Release);
            write_descriptor(slot, name.as_str(), 0, 0);
            // Pairs with the same fence in a concurrent claim of the same name, so at
            // least one of the two sees the other and backs off.
            fence(Ordering::SeqCst);
            let lease = Lease {
                directory: Arc::clone(self),
                slot: index,
                token,
                name: name.clone(),
            };
            let conflict = self
                .live_slots(now_ms())
                .any(|(other, s)| other != index && name.matches(&s.info.name));
            return if conflict {
                Err(BusError::NameTaken)
            } else {
                Ok(lease)
            };
        }
        Err(BusError::DirectoryFull)
    }

    fn live_slots(&self, now: u64) -> impl Iterator<Item = (usize, Snapshot)> + '_ {
        self.shared
            .get()
            .slots
            .iter()
            .enumerate()
            .filter_map(move |(index, slot)| {
                let snapshot = snapshot(slot)?;
                let alive =
                    snapshot.owner != 0 && !is_stale(snapshot.heartbeat_ms, now, LEASE_TIMEOUT_MS);
                alive.then_some((index, snapshot))
            })
    }

    fn slot(&self, index: usize) -> &DirectorySlot {
        &self.shared.get().slots[index]
    }
}

/// Exclusive ownership of a bus name. Dropping it frees the name.
pub struct Lease {
    directory: Arc<Directory>,
    slot: usize,
    token: u64,
    name: BusName,
}

impl Lease {
    pub fn name(&self) -> &BusName {
        &self.name
    }

    pub fn directory(&self) -> &Arc<Directory> {
        &self.directory
    }

    /// Renews the lease. Returns `false` if it expired and another transmitter took the
    /// name, for example after the computer was suspended.
    pub fn renew(&self) -> bool {
        let slot = self.directory.slot(self.slot);
        if slot.owner.load(Ordering::Acquire) != self.token {
            return false;
        }
        slot.heartbeat_ms.store(now_ms(), Ordering::Release);
        true
    }

    /// Publishes the stream format for receivers browsing the directory.
    pub fn set_format(&self, sample_rate: u32, channels: u32) {
        let slot = self.directory.slot(self.slot);
        if slot.owner.load(Ordering::Acquire) == self.token {
            write_descriptor(slot, self.name.as_str(), sample_rate, channels);
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let slot = self.directory.slot(self.slot);
        // Fails harmlessly if the lease expired and was taken over.
        let _ = slot
            .owner
            .compare_exchange(self.token, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

struct Snapshot {
    owner: u64,
    heartbeat_ms: u64,
    info: BusInfo,
}

fn snapshot(slot: &DirectorySlot) -> Option<Snapshot> {
    let owner = slot.owner.load(Ordering::Acquire);
    if owner == 0 {
        return None;
    }
    (0..SNAPSHOT_ATTEMPTS).find_map(|_| {
        let before = slot.sequence.load(Ordering::Acquire);
        if before % 2 == 1 {
            return None;
        }
        let words: [u64; NAME_WORDS] = from_fn(|index| slot.name[index].load(Ordering::Relaxed));
        let len = slot.name_len.load(Ordering::Relaxed) as usize;
        let sample_rate = slot.sample_rate.load(Ordering::Relaxed);
        let channels = slot.channels.load(Ordering::Relaxed);
        let process_id = slot.process_id.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if slot.sequence.load(Ordering::Relaxed) != before {
            return None;
        }
        let name = unpack_name(&words, len)?;
        (!name.is_empty()).then(|| Snapshot {
            owner,
            heartbeat_ms: slot.heartbeat_ms.load(Ordering::Acquire),
            info: BusInfo {
                name,
                sample_rate,
                channels,
                process_id,
            },
        })
    })
}

/// Writes a slot's descriptive fields under its sequence lock. Only the owner writes.
fn write_descriptor(slot: &DirectorySlot, name: &str, sample_rate: u32, channels: u32) {
    slot.sequence.fetch_add(1, Ordering::AcqRel);
    fence(Ordering::Release);
    for (target, word) in slot.name.iter().zip(pack_name(name)) {
        target.store(word, Ordering::Relaxed);
    }
    slot.name_len.store(name.len() as u32, Ordering::Relaxed);
    slot.sample_rate.store(sample_rate, Ordering::Relaxed);
    slot.channels.store(channels, Ordering::Relaxed);
    slot.process_id.store(process::id(), Ordering::Relaxed);
    slot.sequence.fetch_add(1, Ordering::Release);
}
