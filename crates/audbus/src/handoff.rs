//! Real-time safe replacement of resources owned by the audio thread.
//!
//! A control thread prepares a resource (which may allocate or map memory) and sends
//! it to the audio thread. The audio thread swaps it in and returns the previous one,
//! so allocation and deallocation both stay on the control thread.

use std::mem::{forget, replace};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

/// Pending replacements in flight. The control thread sends at most this many before
/// collecting retired values, so returning a value never fails.
const CAPACITY: usize = 4;

/// Control-thread side.
pub struct HandoffSender<T> {
    updates: Producer<Option<T>>,
    retired: Consumer<Option<T>>,
    in_flight: usize,
}

/// Audio-thread side, owning the current value.
pub struct HandoffReceiver<T> {
    updates: Consumer<Option<T>>,
    retired: Producer<Option<T>>,
    current: Option<T>,
}

/// Creates a connected sender and receiver. The receiver starts without a value.
pub fn handoff<T: Send>() -> (HandoffSender<T>, HandoffReceiver<T>) {
    let (updates, update_consumer) = RingBuffer::new(CAPACITY);
    let (retired_producer, retired) = RingBuffer::new(CAPACITY);
    (
        HandoffSender {
            updates,
            retired,
            in_flight: 0,
        },
        HandoffReceiver {
            updates: update_consumer,
            retired: retired_producer,
            current: None,
        },
    )
}

impl<T: Send> HandoffSender<T> {
    /// Sends a replacement (`None` removes the current value). Returns the value back if
    /// too many replacements are pending; call [`collect`](Self::collect) and retry.
    pub fn send(&mut self, value: Option<T>) -> Result<(), Option<T>> {
        self.collect();
        if self.in_flight >= CAPACITY {
            return Err(value);
        }
        match self.updates.push(value) {
            Ok(()) => {
                self.in_flight += 1;
                Ok(())
            }
            Err(PushError::Full(value)) => Err(value),
        }
    }

    /// Drops values the audio thread has retired.
    pub fn collect(&mut self) {
        while let Ok(value) = self.retired.pop() {
            self.in_flight = self.in_flight.saturating_sub(1);
            drop(value);
        }
    }

    /// Whether every sent replacement has been taken over by the audio thread.
    pub fn is_settled(&mut self) -> bool {
        self.collect();
        self.in_flight == 0
    }
}

impl<T: Send> HandoffReceiver<T> {
    /// Applies pending replacements and returns whether the value changed. Real-time
    /// safe.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(value) = self.updates.pop() {
            changed = true;
            let previous = replace(&mut self.current, value);
            // The sender never has more than `CAPACITY` values in flight, so this succeeds.
            if let Err(PushError::Full(previous)) = self.retired.push(previous) {
                // Unreachable by construction; keep the value alive instead of dropping
                // it on the audio thread.
                forget(previous);
            }
        }
        changed
    }

    pub fn current(&self) -> Option<&T> {
        self.current.as_ref()
    }

    pub fn current_mut(&mut self) -> Option<&mut T> {
        self.current.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, handoff};

    #[test]
    fn values_are_swapped_in_and_retired_to_the_sender() {
        let (mut sender, mut receiver) = handoff::<Vec<u8>>();
        assert!(receiver.current().is_none());
        sender.send(Some(vec![1])).expect("room");
        sender.send(Some(vec![2])).expect("room");
        receiver.poll();
        assert_eq!(receiver.current().map(Vec::as_slice), Some(&[2_u8][..]));
        assert!(sender.is_settled());
        sender.send(None).expect("room");
        receiver.poll();
        assert!(receiver.current().is_none());
    }

    #[test]
    fn sender_limits_values_in_flight() {
        let (mut sender, mut receiver) = handoff::<u32>();
        (0..CAPACITY as u32).for_each(|value| sender.send(Some(value)).expect("room"));
        assert_eq!(sender.send(Some(99)), Err(Some(99)));
        receiver.poll();
        assert!(sender.send(Some(99)).is_ok());
    }
}
