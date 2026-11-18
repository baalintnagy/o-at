//! Atomic values for publishing real-time state to editors.

use std::sync::atomic::{AtomicU32, Ordering};

/// An `f32` stored in an `AtomicU32`, with relaxed ordering.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Raises the stored value to `value` if larger. Negative values count as zero,
    /// because only non-negative bit patterns order like their values.
    pub fn fetch_max(&self, value: f32) {
        self.0
            .fetch_max(value.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// Returns the value and resets it to zero.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::AtomicF32;

    #[test]
    fn peaks_accumulate_and_reset() {
        let peak = AtomicF32::default();
        peak.fetch_max(0.25);
        peak.fetch_max(0.5);
        peak.fetch_max(-1.0);
        peak.fetch_max(0.125);
        assert_eq!(peak.take(), 0.5);
        assert_eq!(peak.load(), 0.0);
    }
}
