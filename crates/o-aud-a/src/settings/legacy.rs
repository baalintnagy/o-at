//! Readers for settings that earlier versions stored as choices and that are now
//! continuous. Their variant names appear in saved sessions: never rename them.

use serde::{Deserialize, Deserializer};

use super::SpectrumSettings;
use super::range::LEVEL_RANGE_DB;

/// Display tilt choices of earlier versions, kept to read their sessions; the tilt is
/// now continuous ([`SpectrumSettings::slope`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
enum Slope {
    Flat,
    Db3,
    Db45,
}

impl Slope {
    /// Tilt in dB per octave.
    fn db_per_octave(self) -> f32 {
        match self {
            Self::Flat => 0.0,
            Self::Db3 => 3.0,
            Self::Db45 => 4.5,
        }
    }
}

/// Level range choices of earlier versions, kept to read their sessions; the range is
/// now continuous ([`LEVEL_RANGE_DB`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
enum DbRange {
    R60,
    R90,
    R120,
}

impl DbRange {
    /// Range in dB.
    fn db(self) -> f32 {
        match self {
            Self::R60 => 60.0,
            Self::R90 => 90.0,
            Self::R120 => 120.0,
        }
    }
}

/// A continuous setting as stored: a number, or the choice that earlier versions stored.
#[derive(Deserialize)]
#[serde(untagged)]
enum Stored<T> {
    Number(f32),
    Choice(T),
}

/// Reads a level range in dB, also from a [`DbRange`] variant name.
pub(super) fn level_range_db<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    Ok(match Stored::<DbRange>::deserialize(deserializer)? {
        Stored::Number(db) => LEVEL_RANGE_DB.clamp(db),
        Stored::Choice(range) => range.db(),
    })
}

/// Reads a display tilt in dB per octave, also from a [`Slope`] variant name.
pub(super) fn slope_db<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    Ok(match Stored::<Slope>::deserialize(deserializer)? {
        Stored::Number(db) => SpectrumSettings::SLOPE_DB.clamp(db),
        Stored::Choice(slope) => slope.db_per_octave(),
    })
}
