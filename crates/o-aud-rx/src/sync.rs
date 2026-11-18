//! Clock synchronization between a transmitter and the receiver's output device.
//!
//! The transmitter's and the output's sample clocks run from different crystals, so
//! the receiver consumes the bus slightly faster or slower than it fills. The
//! [`DriftController`] keeps the queued audio at its target by fine-tuning the
//! resampling ratio: a PI controller on the low-pass filtered queue length. Its integral
//! term converges to the clock offset, which the editor shows in ppm.

/// Largest ratio correction (0.5 %, 8.6 cents), applied only while catching up.
pub const MAX_CORRECTION: f64 = 0.005;

/// Proportional gain per second of queue error.
const PROPORTIONAL: f64 = 0.3;
/// Integral gain per second squared; critically damped with `PROPORTIONAL`.
const INTEGRAL: f64 = PROPORTIONAL * PROPORTIONAL / 4.0;
/// Time constant of the queue length filter in seconds. Long enough to remove the
/// sawtooth caused by block-wise writing and reading.
const FILTER_S: f64 = 1.0;

#[derive(Clone, Debug, Default)]
pub struct DriftController {
    filtered_s: Option<f64>,
    integral: f64,
}

impl DriftController {
    /// Restarts after a resynchronization. The clock offset estimate is kept, because
    /// the clocks did not change.
    pub fn restart(&mut self) {
        self.filtered_s = None;
    }

    /// Forgets the clock offset, for a new transmitter or output device.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Updates with the queue error (queued minus target) in seconds, measured
    /// `elapsed_s` after the previous update, and returns the relative resampling
    /// ratio: below one consumes the bus faster.
    pub fn update(&mut self, error_s: f64, elapsed_s: f64) -> f64 {
        let filtered = match self.filtered_s {
            Some(previous) => previous + (error_s - previous) * elapsed_s / (FILTER_S + elapsed_s),
            None => error_s,
        };
        self.filtered_s = Some(filtered);
        self.integral = (self.integral + INTEGRAL * filtered * elapsed_s)
            .clamp(-MAX_CORRECTION, MAX_CORRECTION);
        let correction =
            (PROPORTIONAL * filtered + self.integral).clamp(-MAX_CORRECTION, MAX_CORRECTION);
        1.0 - correction
    }

    /// Estimated transmitter clock speed relative to the output clock, in ppm.
    pub fn drift_ppm(&self) -> f64 {
        self.integral * 1e6
    }
}

#[cfg(test)]
mod tests {
    use super::{DriftController, MAX_CORRECTION};

    /// Simulates a queue fed at `1 + drift` and drained at the controlled ratio.
    fn simulate(drift: f64, seconds: f64) -> (DriftController, f64) {
        let mut controller = DriftController::default();
        let step = 0.002;
        let mut error = 0.0;
        let mut ratio = 1.0;
        for _ in 0..(seconds / step) as usize {
            // Output time `step` consumes `step / ratio` seconds of input.
            error += step * (1.0 + drift) - step / ratio;
            ratio = controller.update(error, step);
        }
        (controller, error)
    }

    #[test]
    fn locks_to_a_fast_and_a_slow_source() {
        for drift in [250e-6, -250e-6] {
            let (controller, error) = simulate(drift, 120.0);
            assert!(error.abs() < 0.000_5, "queue error {error} s");
            let estimated = controller.drift_ppm();
            assert!(
                (estimated - drift * 1e6).abs() < 10.0,
                "estimated {estimated} ppm for {} ppm",
                drift * 1e6
            );
        }
    }

    #[test]
    fn corrections_are_limited() {
        let mut controller = DriftController::default();
        let ratio = controller.update(10.0, 0.01);
        assert!((ratio - (1.0 - MAX_CORRECTION)).abs() < 1e-12);
    }
}
