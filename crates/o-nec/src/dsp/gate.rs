//! Voice gate, the equivalent of automatic or manual input sensitivity in voice chat
//! applications. It opens on RNNoise's voice probability or on the signal level, holds,
//! and then releases to a configurable attenuation.

use crate::dsp::level::{db_to_gain, one_pole_coefficient};
use crate::dsp::{FRAME, INTERNAL_RATE};

/// What opens the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateTrigger {
    Off,
    Voice,
    Level,
}

/// Per-frame gate settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateSettings {
    pub trigger: GateTrigger,
    /// Voice probability (0..1) that opens the gate in [`GateTrigger::Voice`] mode.
    pub voice_threshold: f32,
    /// Level in dBFS that opens the gate in [`GateTrigger::Level`] mode.
    pub level_threshold_db: f32,
    /// Attenuation of the closed gate in dB (positive).
    pub range_db: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
}

/// Attack time of the opening gate.
const ATTACK_MS: f32 = 1.5;

/// Hysteresis below the threshold before the gate is allowed to close.
const VOICE_HYSTERESIS: f32 = 0.15;
const LEVEL_HYSTERESIS_DB: f32 = 4.0;

pub struct Gate {
    gain: f32,
    hold_remaining: f32,
    open: bool,
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Gate {
    pub fn new() -> Self {
        Self {
            gain: 1.0,
            hold_remaining: 0.0,
            open: true,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Whether the gate is currently open or holding.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Current linear gain of the gate.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Applies the gate to a frame using its detector values.
    pub fn process(
        &mut self,
        frame: &mut [f32],
        settings: &GateSettings,
        voice_probability: f32,
        level_db: f32,
    ) {
        let triggered = match settings.trigger {
            GateTrigger::Off => true,
            GateTrigger::Voice => {
                let threshold = if self.open {
                    settings.voice_threshold - VOICE_HYSTERESIS
                } else {
                    settings.voice_threshold
                };
                voice_probability >= threshold
            }
            GateTrigger::Level => {
                let threshold = if self.open {
                    settings.level_threshold_db - LEVEL_HYSTERESIS_DB
                } else {
                    settings.level_threshold_db
                };
                level_db >= threshold
            }
        };

        let frame_ms = 1000.0 * FRAME as f32 / INTERNAL_RATE as f32;
        if triggered {
            self.hold_remaining = settings.hold_ms;
            self.open = true;
        } else if self.hold_remaining > 0.0 {
            self.hold_remaining -= frame_ms;
        } else {
            self.open = false;
        }

        let target = if self.open {
            1.0
        } else {
            db_to_gain(-settings.range_db)
        };
        let rate = INTERNAL_RATE as f32;
        let coefficient = if target > self.gain {
            one_pole_coefficient(ATTACK_MS, rate)
        } else {
            one_pole_coefficient(settings.release_ms.max(1.0), rate)
        };
        for sample in frame.iter_mut() {
            self.gain = target + coefficient * (self.gain - target);
            *sample *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Gate, GateSettings, GateTrigger};
    use crate::dsp::FRAME;
    use crate::dsp::level::gain_to_db;

    fn settings(trigger: GateTrigger) -> GateSettings {
        GateSettings {
            trigger,
            voice_threshold: 0.6,
            level_threshold_db: -40.0,
            range_db: 40.0,
            hold_ms: 50.0,
            release_ms: 20.0,
        }
    }

    #[test]
    fn closes_after_hold_and_release() {
        let mut gate = Gate::new();
        let settings = settings(GateTrigger::Voice);
        let mut frame = [1.0; FRAME];
        gate.process(&mut frame, &settings, 0.9, 0.0);
        assert!(gate.is_open());
        for _ in 0..5 {
            frame.fill(1.0);
            gate.process(&mut frame, &settings, 0.1, 0.0);
            assert!(gate.is_open(), "gate should hold for 50 ms");
        }
        for _ in 0..30 {
            frame.fill(1.0);
            gate.process(&mut frame, &settings, 0.1, 0.0);
        }
        assert!(!gate.is_open());
        assert!((gain_to_db(gate.gain()) - -40.0).abs() < 0.5);
    }

    #[test]
    fn level_trigger_uses_hysteresis() {
        let mut gate = Gate::new();
        let mut settings = settings(GateTrigger::Level);
        settings.hold_ms = 0.0;
        let mut frame = [0.5; FRAME];
        gate.process(&mut frame, &settings, 0.0, -38.0);
        gate.process(&mut frame, &settings, 0.0, -42.0);
        assert!(gate.is_open(), "within hysteresis");
        gate.process(&mut frame, &settings, 0.0, -50.0);
        assert!(!gate.is_open());
        gate.process(&mut frame, &settings, 0.0, -42.0);
        assert!(!gate.is_open(), "must exceed the threshold to reopen");
    }

    #[test]
    fn off_mode_is_transparent() {
        let mut gate = Gate::new();
        let mut frame = [0.25; FRAME];
        gate.process(&mut frame, &settings(GateTrigger::Off), 0.0, -100.0);
        assert!(frame.iter().all(|sample| (*sample - 0.25).abs() < 1.0e-6));
    }
}
