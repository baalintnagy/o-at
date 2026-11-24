//! Sample delay lines whose delay can change on the audio thread without allocating.

use std::mem::swap;

/// Delays samples by up to a capacity fixed at construction or in
/// [`DelayLine::set_capacity`].
pub struct DelayLine {
    buffer: Vec<f32>,
    delay: usize,
    position: usize,
}

impl DelayLine {
    /// Creates a delay line of `delay` samples that can grow to `capacity` samples.
    pub fn new(capacity: usize, delay: usize) -> Self {
        let mut line = Self {
            buffer: vec![0.0; capacity],
            delay: 0,
            position: 0,
        };
        line.set_delay(delay);
        line
    }

    /// Changes the largest supported delay. Allocates; call it outside the audio callback.
    pub fn set_capacity(&mut self, capacity: usize) {
        if capacity != self.buffer.len() {
            self.buffer = vec![0.0; capacity];
        }
        let delay = self.delay;
        self.delay = usize::MAX;
        self.set_delay(delay);
    }

    /// Current delay in samples.
    pub fn delay(&self) -> usize {
        self.delay
    }

    /// Sets the delay in samples, clamped to the capacity. A change clears the line.
    pub fn set_delay(&mut self, delay: usize) {
        let delay = delay.min(self.buffer.len());
        if delay != self.delay {
            self.delay = delay;
            self.clear();
        }
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.position = 0;
    }

    /// Writes `input` delayed into `output`. Both slices must have the same length.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        if self.delay == 0 {
            output.copy_from_slice(input);
            return;
        }
        for (sample, target) in input.iter().zip(output.iter_mut()) {
            *target = self.buffer[self.position];
            self.buffer[self.position] = *sample;
            self.advance();
        }
    }

    /// Delays `samples` in place.
    pub fn process_in_place(&mut self, samples: &mut [f32]) {
        if self.delay == 0 {
            return;
        }
        for sample in samples.iter_mut() {
            swap(&mut self.buffer[self.position], sample);
            self.advance();
        }
    }

    /// Moves to the next slot of the line, wrapping after `delay` slots without a
    /// division per sample.
    fn advance(&mut self) {
        self.position += 1;
        if self.position == self.delay {
            self.position = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DelayLine;

    #[test]
    fn delays_by_the_set_amount() {
        let mut line = DelayLine::new(16, 5);
        let input: Vec<f32> = (1..=20).map(|n| n as f32).collect();
        let mut output = vec![0.0; input.len()];
        line.process(&input, &mut output);
        assert_eq!(&output[..5], &[0.0; 5]);
        assert_eq!(&output[5..], &input[..15]);

        line.set_delay(3);
        let mut samples = input.clone();
        line.process_in_place(&mut samples);
        assert_eq!(&samples[..3], &[0.0; 3]);
        assert_eq!(&samples[3..], &input[..17]);
    }
}
