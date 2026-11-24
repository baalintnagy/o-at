//! Runs test code as the audio thread must run: without allocating and, with the `rtsan`
//! feature on Linux, without locking, blocking or making system calls.
//!
//! Allocations abort only in test binaries with an allocation-checking global allocator
//! (NIH-plug's `assert_process_allocs` or `assert_no_alloc::AllocDisabler`) and in debug
//! builds. RealtimeSanitizer reports every real-time unsafe call made on this thread inside
//! [`realtime`] and halts the test; set `RTSAN_ENABLE=1` when building, as the
//! `rtsan-standalone` crate is otherwise inert.

use assert_no_alloc::assert_no_alloc;

/// Runs `body` in a real-time context and returns its result.
pub fn realtime<T>(body: impl FnOnce() -> T) -> T {
    let _context = sanitizer::RealtimeContext::enter();
    assert_no_alloc(body)
}

#[cfg(all(feature = "rtsan", target_os = "linux"))]
mod sanitizer {
    use rtsan_standalone::{realtime_enter, realtime_exit};

    /// RealtimeSanitizer's real-time context on this thread, left when dropped, also when
    /// the body panics.
    pub(super) struct RealtimeContext;

    impl RealtimeContext {
        pub(super) fn enter() -> Self {
            realtime_enter();
            Self
        }
    }

    impl Drop for RealtimeContext {
        fn drop(&mut self) {
            realtime_exit();
        }
    }
}

#[cfg(not(all(feature = "rtsan", target_os = "linux")))]
mod sanitizer {
    /// Without RealtimeSanitizer only allocations are checked.
    pub(super) struct RealtimeContext;

    impl RealtimeContext {
        pub(super) fn enter() -> Self {
            Self
        }
    }
}
