//! Noise profiles: the noise power and its spread per frequency bin.
//!
//! A [`ProfileStore`] holds one profile in fixed-size atomic storage, so the audio thread
//! can store a learned profile and read a restored one without locking or allocating.
//! A sequence counter guards it like a seqlock: writers make it odd while they write,
//! and readers discard a read during which it changed.
//!
//! Profiles keep the bin spacing they were learned with. [`ProfileStore::load_into`]
//! converts them to the bins of the current FFT size and sample rate, so a profile
//! learned at one rate or resolution works at another. Levels are power spectral
//! densities (power per bin divided by the window's energy), which keeps broadband noise
//! at the same level for every FFT size.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering, fence};
use std::thread;

use nih_plug::params::persist::PersistentField;
use serde::{Deserialize, Serialize};

use crate::dsp::MAX_BINS;
use crate::dsp::level::db_to_power;

/// A noise profile as stored in sessions. Empty level lists mean no profile.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileData {
    /// Spacing of the bins in Hz.
    pub bin_hz: f32,
    /// Mean noise power spectral density per bin in dB.
    pub level_db: Vec<f32>,
    /// Standard deviation of the per-frame level per bin in dB.
    pub spread_db: Vec<f32>,
}

impl ProfileData {
    pub fn is_empty(&self) -> bool {
        self.level_db.is_empty() || self.bin_hz.is_nan() || self.bin_hz <= 0.0
    }
}

/// Outcome of [`ProfileStore::load_into`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadResult {
    /// The profile did not change since the last load.
    Unchanged,
    /// A profile was converted into the target bins.
    Loaded,
    /// The store holds no profile.
    Empty,
    /// A write was in progress; try again later.
    Busy,
}

struct Shared {
    /// Even while stable, odd while a writer writes.
    sequence: AtomicU32,
    bins: AtomicU32,
    bin_hz: AtomicU32,
    level_db: Box<[AtomicU32]>,
    spread_db: Box<[AtomicU32]>,
}

/// Lock-free storage for one noise profile, shared by the audio thread, the editor and
/// the host's state handling. Clones share the same profile.
#[derive(Clone)]
pub struct ProfileStore {
    shared: Arc<Shared>,
}

impl Default for ProfileStore {
    fn default() -> Self {
        let atomics = || (0..MAX_BINS).map(|_| AtomicU32::new(0)).collect();
        Self {
            shared: Arc::new(Shared {
                sequence: AtomicU32::new(0),
                bins: AtomicU32::new(0),
                bin_hz: AtomicU32::new(0),
                level_db: atomics(),
                spread_db: atomics(),
            }),
        }
    }
}

impl ProfileStore {
    /// Stores a profile if no other writer is busy. Real-time safe; returns false when
    /// the caller should retry later.
    pub fn try_store(&self, bin_hz: f32, level_db: &[f32], spread_db: &[f32]) -> bool {
        let shared = &*self.shared;
        let sequence = shared.sequence.load(Ordering::Relaxed);
        if sequence % 2 == 1
            || shared
                .sequence
                .compare_exchange(
                    sequence,
                    sequence.wrapping_add(1),
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_err()
        {
            return false;
        }
        fence(Ordering::Release);
        let bins = level_db.len().min(spread_db.len()).min(MAX_BINS);
        shared.bins.store(bins as u32, Ordering::Relaxed);
        shared.bin_hz.store(bin_hz.to_bits(), Ordering::Relaxed);
        for (target, value) in shared.level_db.iter().zip(&level_db[..bins]) {
            target.store(value.to_bits(), Ordering::Relaxed);
        }
        for (target, value) in shared.spread_db.iter().zip(&spread_db[..bins]) {
            target.store(value.to_bits(), Ordering::Relaxed);
        }
        shared
            .sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
        true
    }

    /// Stores a profile, waiting for a busy writer. Not for the audio thread.
    pub fn store(&self, data: &ProfileData) {
        let (level, spread) = if data.is_empty() {
            (&[][..], &[][..])
        } else {
            (&data.level_db[..], &data.spread_db[..])
        };
        while !self.try_store(data.bin_hz, level, spread) {
            thread::yield_now();
        }
    }

    /// Removes the profile. Not for the audio thread.
    pub fn clear(&self) {
        self.store(&ProfileData::default());
    }

    /// A copy of the stored profile. Not for the audio thread.
    pub fn snapshot(&self) -> ProfileData {
        let shared = &*self.shared;
        loop {
            let before = shared.sequence.load(Ordering::Acquire);
            if before % 2 == 1 {
                thread::yield_now();
                continue;
            }
            let bins = (shared.bins.load(Ordering::Relaxed) as usize).min(MAX_BINS);
            let data = ProfileData {
                bin_hz: f32::from_bits(shared.bin_hz.load(Ordering::Relaxed)),
                level_db: load_all(&shared.level_db[..bins]),
                spread_db: load_all(&shared.spread_db[..bins]),
            };
            fence(Ordering::Acquire);
            if shared.sequence.load(Ordering::Relaxed) == before {
                return data;
            }
        }
    }

    /// Whether a profile is stored.
    pub fn has_profile(&self) -> bool {
        self.shared.bins.load(Ordering::Relaxed) > 0
    }

    /// Converts the stored profile to bins spaced `bin_hz` apart, writing the noise power
    /// spectral density into `power` and the level spread in dB into `spread_db`.
    /// `seen` holds the sequence of the last load; `None` forces a load. Real-time safe.
    pub fn load_into(
        &self,
        seen: &mut Option<u32>,
        bin_hz: f32,
        power: &mut [f32],
        spread_db: &mut [f32],
    ) -> LoadResult {
        let shared = &*self.shared;
        let before = shared.sequence.load(Ordering::Acquire);
        if before % 2 == 1 {
            return LoadResult::Busy;
        }
        if *seen == Some(before) {
            return LoadResult::Unchanged;
        }
        let bins = (shared.bins.load(Ordering::Relaxed) as usize).min(MAX_BINS);
        let source_hz = f32::from_bits(shared.bin_hz.load(Ordering::Relaxed));
        let empty = bins == 0 || source_hz.is_nan() || source_hz <= 0.0;
        if !empty {
            convert(
                &shared.level_db[..bins],
                &shared.spread_db[..bins],
                source_hz,
                bin_hz,
                power,
                spread_db,
            );
        }
        fence(Ordering::Acquire);
        if shared.sequence.load(Ordering::Relaxed) != before {
            return LoadResult::Busy;
        }
        *seen = Some(before);
        if empty {
            LoadResult::Empty
        } else {
            LoadResult::Loaded
        }
    }
}

fn load_all(values: &[AtomicU32]) -> Vec<f32> {
    values
        .iter()
        .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
        .collect()
}

fn load(values: &[AtomicU32], index: usize) -> f32 {
    f32::from_bits(values[index].load(Ordering::Relaxed))
}

/// Resamples source bins to target bins by frequency: averages the power of the source
/// bins inside each target bin, or interpolates the level where target bins are finer.
fn convert(
    level_db: &[AtomicU32],
    spread_db: &[AtomicU32],
    source_hz: f32,
    target_hz: f32,
    power: &mut [f32],
    spread_out: &mut [f32],
) {
    let last = level_db.len() - 1;
    for (index, (power, spread)) in power.iter_mut().zip(spread_out.iter_mut()).enumerate() {
        let centre = index as f32 * target_hz;
        let low = ((centre - 0.5 * target_hz) / source_hz).ceil().max(0.0) as usize;
        let high = ((centre + 0.5 * target_hz) / source_hz).floor();
        if high >= 0.0 && low <= (high as usize).min(last) {
            let high = (high as usize).min(last);
            let count = (high - low + 1) as f32;
            *power = (low..=high)
                .map(|bin| db_to_power(load(level_db, bin)))
                .sum::<f32>()
                / count;
            *spread = (low..=high).map(|bin| load(spread_db, bin)).sum::<f32>() / count;
        } else {
            let position = (centre / source_hz).clamp(0.0, last as f32);
            let below = (position.floor() as usize).min(last);
            let above = (below + 1).min(last);
            let fraction = position - below as f32;
            let mix = |values: &[AtomicU32]| {
                load(values, below) + (load(values, above) - load(values, below)) * fraction
            };
            *power = db_to_power(mix(level_db));
            *spread = mix(spread_db);
        }
    }
}

impl<'a> PersistentField<'a, ProfileData> for ProfileStore {
    fn set(&self, new_value: ProfileData) {
        self.store(&new_value);
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&ProfileData) -> R,
    {
        f(&self.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use nih_plug::params::persist::PersistentField;

    use super::{LoadResult, ProfileData, ProfileStore};
    use crate::dsp::level::power_to_db;

    fn ramp(bins: usize, bin_hz: f32) -> ProfileData {
        ProfileData {
            bin_hz,
            level_db: (0..bins).map(|bin| -60.0 - 0.01 * bin as f32).collect(),
            spread_db: vec![6.0; bins],
        }
    }

    #[test]
    fn load_reports_changes_once() {
        let store = ProfileStore::default();
        let mut seen = None;
        let mut power = vec![0.0; 1025];
        let mut spread = vec![0.0; 1025];
        assert_eq!(
            store.load_into(&mut seen, 23.4, &mut power, &mut spread),
            LoadResult::Empty
        );
        assert_eq!(
            store.load_into(&mut seen, 23.4, &mut power, &mut spread),
            LoadResult::Unchanged
        );
        store.store(&ramp(1025, 23.4));
        assert_eq!(
            store.load_into(&mut seen, 23.4, &mut power, &mut spread),
            LoadResult::Loaded
        );
        assert!((power_to_db(power[100]) + 61.0).abs() < 0.01);
        assert_eq!(spread[100], 6.0);
        store.clear();
        assert_eq!(
            store.load_into(&mut seen, 23.4, &mut power, &mut spread),
            LoadResult::Empty
        );
    }

    #[test]
    fn conversion_keeps_flat_levels_at_any_resolution() {
        let store = ProfileStore::default();
        store.store(&ProfileData {
            bin_hz: 11.71875,
            level_db: vec![-70.0; 2049],
            spread_db: vec![5.6; 2049],
        });
        for (bin_hz, bins) in [(46.875, 513), (11.71875, 2049), (2.9296875, 8193)] {
            let mut power = vec![0.0; bins];
            let mut spread = vec![0.0; bins];
            store.load_into(&mut None, bin_hz, &mut power, &mut spread);
            for (bin, (power, spread)) in power.iter().zip(&spread).enumerate() {
                assert!(
                    (power_to_db(*power) + 70.0).abs() < 0.01,
                    "{bin_hz} Hz bin {bin}: {}",
                    power_to_db(*power)
                );
                assert!((spread - 5.6).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn coarser_bins_average_power() {
        let store = ProfileStore::default();
        // One loud bin among quiet ones, at 10 Hz spacing.
        let mut level_db = vec![-80.0; 101];
        level_db[50] = -40.0;
        store.store(&ProfileData {
            bin_hz: 10.0,
            spread_db: vec![0.0; 101],
            level_db,
        });
        let mut power = vec![0.0; 26];
        let mut spread = vec![0.0; 26];
        store.load_into(&mut None, 40.0, &mut power, &mut spread);
        // Target bin 12 (480 Hz) covers 460–500 Hz: source bins 46 to 50.
        let expected = (1e-4 + 4.0 * 1e-8) / 5.0;
        assert!((power[12] - expected).abs() / expected < 1e-3);
    }

    #[test]
    fn persisted_profile_round_trips() {
        let store = ProfileStore::default();
        let data = ramp(300, 46.875);
        PersistentField::set(&store, data.clone());
        let json = store.map(|value| serde_json::to_string(value).expect("serializes"));
        let restored: ProfileData = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(restored, data);
        assert!(store.has_profile());
    }
}
