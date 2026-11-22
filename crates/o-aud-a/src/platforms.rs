//! How streaming platforms normalize loudness, and what they would do to a measured track.
//!
//! The references are the values the platforms publish or that are commonly measured;
//! only Spotify documents its behavior in full. They change from time to time: keep
//! them here, in one place, and say where they come from in `docs/o-aud-a.md`.

/// How a platform reaches its reference loudness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Normalization {
    /// Plays the track as it is.
    None,
    /// Turns loud tracks down and leaves quiet ones.
    DownOnly,
    /// Turns loud tracks down and quiet ones up, but only as far as the true peak
    /// stays below the ceiling.
    PeakLimited,
    /// Turns tracks up or down to the reference; a limiter holds the peaks at the
    /// ceiling.
    Limiter,
}

/// One platform's normalization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Platform {
    pub name: &'static str,
    /// Reference integrated loudness in LUFS; `None` without normalization.
    pub reference_lufs: Option<f32>,
    /// Highest true peak the platform asks for, in dBTP.
    pub ceiling_dbtp: f32,
    pub normalization: Normalization,
}

/// The platforms in the preview, by default playback setting.
pub const PLATFORMS: [Platform; 9] = [
    Platform {
        name: "Spotify",
        reference_lufs: Some(-14.0),
        ceiling_dbtp: -1.0,
        normalization: Normalization::PeakLimited,
    },
    Platform {
        name: "Spotify Loud",
        reference_lufs: Some(-11.0),
        ceiling_dbtp: -1.0,
        normalization: Normalization::Limiter,
    },
    Platform {
        name: "Spotify Quiet",
        reference_lufs: Some(-19.0),
        ceiling_dbtp: -1.0,
        normalization: Normalization::PeakLimited,
    },
    Platform {
        name: "YouTube",
        reference_lufs: Some(-14.0),
        ceiling_dbtp: -1.0,
        normalization: Normalization::DownOnly,
    },
    Platform {
        name: "Apple Music",
        reference_lufs: Some(-16.0),
        ceiling_dbtp: -1.0,
        normalization: Normalization::PeakLimited,
    },
    Platform {
        name: "Amazon Music",
        reference_lufs: Some(-14.0),
        ceiling_dbtp: -2.0,
        normalization: Normalization::DownOnly,
    },
    Platform {
        name: "Tidal",
        reference_lufs: Some(-14.0),
        ceiling_dbtp: -1.0,
        normalization: Normalization::DownOnly,
    },
    Platform {
        name: "Deezer",
        reference_lufs: Some(-15.0),
        ceiling_dbtp: -1.0,
        normalization: Normalization::DownOnly,
    },
    Platform {
        name: "SoundCloud",
        reference_lufs: None,
        ceiling_dbtp: -1.0,
        normalization: Normalization::None,
    },
];

/// What a platform does with a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Within 0.5 LU of the reference, or no normalization.
    Unchanged,
    TurnedDown,
    TurnedUp,
    /// Turned up less than to the reference, because of the true peak.
    HeldByPeak,
    /// Left quieter than the reference: the platform never turns up.
    LeftQuiet,
    /// Turned up into the limiter, which reduces the peaks.
    Limited,
}

impl Verdict {
    pub fn text(self) -> &'static str {
        match self {
            Self::Unchanged => "as is",
            Self::TurnedDown => "turned down",
            Self::TurnedUp => "turned up",
            Self::HeldByPeak => "held by peak",
            Self::LeftQuiet => "left quieter",
            Self::Limited => "limited",
        }
    }
}

/// A track's playback on a platform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playback {
    /// Gain the platform applies in dB.
    pub gain_db: f32,
    /// Integrated loudness after the gain in LUFS.
    pub loudness_lufs: f32,
    /// True peak after the gain, and after the limiter if one engages, in dBTP.
    pub true_peak_db: f32,
    pub verdict: Verdict,
    /// The true peak after the gain is above the platform's ceiling, where lossy
    /// encoding may clip.
    pub over_ceiling: bool,
}

/// Differences below this many dB count as none.
const TOLERANCE_DB: f32 = 0.05;

impl Platform {
    /// What the platform does with a track of `integrated_lufs` and `true_peak_db`;
    /// `None` before the loudness is measured.
    pub fn playback(&self, integrated_lufs: f32, true_peak_db: f32) -> Option<Playback> {
        if !integrated_lufs.is_finite() {
            return None;
        }
        let wanted = self
            .reference_lufs
            .map_or(0.0, |reference| reference - integrated_lufs);
        // A silent peak meter gives no limit.
        let headroom = if true_peak_db.is_finite() {
            self.ceiling_dbtp - true_peak_db
        } else {
            f32::INFINITY
        };
        let gain_db = match self.normalization {
            Normalization::None => 0.0,
            Normalization::DownOnly => wanted.min(0.0),
            Normalization::PeakLimited if wanted > 0.0 => wanted.min(headroom).max(0.0),
            Normalization::PeakLimited | Normalization::Limiter => wanted,
        };
        let raised_peak = true_peak_db + gain_db;
        let limited = self.normalization == Normalization::Limiter
            && raised_peak > self.ceiling_dbtp + TOLERANCE_DB;
        let verdict = if limited {
            Verdict::Limited
        } else if gain_db < -TOLERANCE_DB {
            Verdict::TurnedDown
        } else if wanted > 0.5 && gain_db < wanted - TOLERANCE_DB {
            if self.normalization == Normalization::DownOnly {
                Verdict::LeftQuiet
            } else {
                Verdict::HeldByPeak
            }
        } else if gain_db > TOLERANCE_DB {
            Verdict::TurnedUp
        } else {
            Verdict::Unchanged
        };
        let true_peak_db = if limited {
            self.ceiling_dbtp
        } else {
            raised_peak
        };
        Some(Playback {
            gain_db,
            loudness_lufs: integrated_lufs + gain_db,
            true_peak_db,
            verdict,
            over_ceiling: true_peak_db > self.ceiling_dbtp + TOLERANCE_DB,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Normalization, PLATFORMS, Platform, Verdict};

    fn platform(name: &str) -> Platform {
        *PLATFORMS
            .iter()
            .find(|platform| platform.name == name)
            .expect("a platform")
    }

    #[test]
    fn loud_masters_are_turned_down() {
        // A −8 LUFS master peaking at −0.3 dBTP.
        let spotify = platform("Spotify").playback(-8.0, -0.3).expect("measured");
        assert!((spotify.gain_db + 6.0).abs() < 1e-4);
        assert!((spotify.loudness_lufs + 14.0).abs() < 1e-4);
        assert_eq!(spotify.verdict, Verdict::TurnedDown);
        assert!(!spotify.over_ceiling, "−6.3 dBTP after the gain");
        let apple = platform("Apple Music")
            .playback(-8.0, -0.3)
            .expect("measured");
        assert!((apple.gain_db + 8.0).abs() < 1e-4);
    }

    #[test]
    fn quiet_masters_depend_on_the_platform() {
        // A −20 LUFS master peaking at −4 dBTP.
        let youtube = platform("YouTube").playback(-20.0, -4.0).expect("measured");
        assert_eq!(youtube.gain_db, 0.0);
        assert_eq!(youtube.verdict, Verdict::LeftQuiet);
        // Spotify wants +6 dB, but only 3 dB fit below −1 dBTP.
        let spotify = platform("Spotify").playback(-20.0, -4.0).expect("measured");
        assert!((spotify.gain_db - 3.0).abs() < 1e-4);
        assert_eq!(spotify.verdict, Verdict::HeldByPeak);
        assert!((spotify.true_peak_db + 1.0).abs() < 1e-4);
        // Spotify Loud turns it up all the way and limits the peaks.
        let loud = platform("Spotify Loud")
            .playback(-20.0, -4.0)
            .expect("measured");
        assert!((loud.gain_db - 9.0).abs() < 1e-4);
        assert_eq!(loud.verdict, Verdict::Limited);
        assert_eq!(loud.true_peak_db, -1.0);
        assert!(!loud.over_ceiling);
    }

    #[test]
    fn unnormalized_and_unmeasured_tracks() {
        let soundcloud = platform("SoundCloud");
        assert_eq!(soundcloud.normalization, Normalization::None);
        let playback = soundcloud.playback(-9.0, 0.5).expect("measured");
        assert_eq!(playback.gain_db, 0.0);
        assert_eq!(playback.verdict, Verdict::Unchanged);
        assert!(playback.over_ceiling, "peaks above −1 dBTP stay");
        assert!(soundcloud.playback(f32::NEG_INFINITY, -3.0).is_none());
        // A track at the reference with no measured peak yet.
        let tidal = platform("Tidal")
            .playback(-14.02, f32::NEG_INFINITY)
            .expect("measured");
        assert_eq!(tidal.verdict, Verdict::Unchanged);
    }
}
