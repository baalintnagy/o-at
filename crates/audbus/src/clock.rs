//! Time base and identifiers shared between processes.

use std::hash::{BuildHasher, RandomState};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock time in milliseconds since the Unix epoch, comparable across processes.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Whether a heartbeat at `heartbeat_ms` is older than `timeout_ms` at `now_ms`.
/// Heartbeats from the future (clock adjustments) count as fresh.
pub(crate) fn is_stale(heartbeat_ms: u64, now_ms: u64, timeout_ms: u64) -> bool {
    now_ms.saturating_sub(heartbeat_ms) > timeout_ms
}

/// A random, non-zero token identifying a lease or a reader registration.
pub(crate) fn new_token() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    // `RandomState` is seeded from the operating system's random source.
    let token = RandomState::new().hash_one((process::id(), count, now_ms()));
    token.max(1)
}

#[cfg(test)]
mod tests {
    use super::{is_stale, new_token};

    #[test]
    fn tokens_are_unique_and_non_zero() {
        let first = new_token();
        let second = new_token();
        assert_ne!(first, 0);
        assert_ne!(first, second);
    }

    #[test]
    fn staleness_tolerates_future_heartbeats() {
        assert!(!is_stale(1_000, 1_500, 1_000));
        assert!(is_stale(1_000, 2_001, 1_000));
        assert!(!is_stale(5_000, 1_000, 1_000));
    }
}
