//! Silence for a capture that stops delivering.
//!
//! Some captures deliver nothing while their source is quiet, for example process
//! loopback of an application, which would stall the bus. Once nothing has arrived for
//! [`STALL`], [`SilenceFill`] reports how much silence makes up the time since the last
//! delivery, to send with
//! [`InterleavedSender::send_silence`](crate::interleaved::InterleavedSender::send_silence),
//! so the bus keeps running at its rate.

use std::time::{Duration, Instant};

/// How long a capture may deliver nothing before silence is sent.
pub const STALL: Duration = Duration::from_millis(30);
/// Time since the last delivery that is not filled, so that a late packet does not land
/// after silence that already covered its time.
pub const LEAD: Duration = Duration::from_millis(10);

/// Counts the silent frames due after the last delivery. Real-time safe.
pub struct SilenceFill {
    /// Frames per second.
    sample_rate: f64,
    /// The last delivery, or the start.
    last_delivery: Instant,
    /// Frames of silence reported since `last_delivery`.
    filled: u64,
}

impl SilenceFill {
    /// Starts counting at `now` for a capture at `sample_rate` Hz.
    pub fn new(sample_rate: u32, now: Instant) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            last_delivery: now,
            filled: 0,
        }
    }

    /// Records that the capture delivered audio at `now`.
    pub fn delivered(&mut self, now: Instant) {
        self.last_delivery = now;
        self.filled = 0;
    }

    /// Frames of silence to send at `now`; zero while the capture delivers.
    pub fn due(&mut self, now: Instant) -> u64 {
        let idle = now.saturating_duration_since(self.last_delivery);
        if idle < STALL {
            return 0;
        }
        let total = ((idle - LEAD).as_secs_f64() * self.sample_rate) as u64;
        let due = total.saturating_sub(self.filled);
        self.filled += due;
        due
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{LEAD, STALL, SilenceFill};

    const RATE: u32 = 48_000;

    fn frames(duration: Duration) -> u64 {
        (duration.as_secs_f64() * f64::from(RATE)) as u64
    }

    #[test]
    fn nothing_is_due_while_audio_arrives() {
        let start = Instant::now();
        let mut fill = SilenceFill::new(RATE, start);
        for step in 1..100 {
            let now = start + Duration::from_millis(10 * step);
            fill.delivered(now);
            assert_eq!(fill.due(now + Duration::from_millis(5)), 0);
        }
    }

    #[test]
    fn a_stall_is_filled_up_to_the_lead() {
        let start = Instant::now();
        let mut fill = SilenceFill::new(RATE, start);
        assert_eq!(fill.due(start + STALL - Duration::from_millis(1)), 0);
        let first = fill.due(start + STALL);
        assert_eq!(first, frames(STALL - LEAD));
        // Later calls add only the time that passed since.
        let later = start + Duration::from_secs(1);
        let total = first + fill.due(later);
        assert_eq!(total, frames(Duration::from_secs(1) - LEAD));
        assert_eq!(fill.due(later), 0);
    }

    #[test]
    fn a_delivery_ends_the_fill() {
        let start = Instant::now();
        let mut fill = SilenceFill::new(RATE, start);
        assert!(fill.due(start + Duration::from_millis(500)) > 0);
        let resumed = start + Duration::from_millis(510);
        fill.delivered(resumed);
        assert_eq!(fill.due(resumed + Duration::from_millis(10)), 0);
        assert_eq!(
            fill.due(resumed + STALL),
            frames(STALL - LEAD),
            "a new stall is counted from the delivery"
        );
    }
}
