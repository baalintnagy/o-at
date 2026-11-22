//! Measures how much faster than real time the analysis engine runs: once with only the
//! measurements that always run (loudness and the stereo meters), once with every view's
//! analysis. Run it as a release build:
//!
//! ```text
//! cargo run --release -p o-aud-a --example throughput
//! ```

use std::f32::consts::TAU;
use std::time::Instant;

use o_aud_a::analysis::{Engine, SignalReading, Wants};
use o_aud_a::settings::AnalyzerSettings;

const RATE: u32 = 48_000;
const SECONDS: u32 = 30;
/// Frames between publications, as in a worker round of 4 ms.
const ROUND_FRAMES: u32 = 192;
/// Runs per case.
const RUNS: usize = 5;

/// Seconds to analyze [`SECONDS`] of `signal` with `wants`.
fn run(settings: &AnalyzerSettings, wants: Wants, signal: &[[f32; 2]]) -> f64 {
    let mut engine = Engine::new(RATE, 2, settings);
    engine.configure(settings);
    engine.set_wants(wants);
    let mut reading = SignalReading::default();
    let start = Instant::now();
    for (n, frame) in (0..RATE * SECONDS).zip(signal.iter().cycle()) {
        engine.process(*frame, true);
        if n % ROUND_FRAMES == 0 {
            engine.publish(&mut reading, false);
            // The editor takes these; here they are dropped.
            reading.sonogram.columns.clear();
            reading.spectral.columns.clear();
            reading.envelope.clear();
        }
    }
    start.elapsed().as_secs_f64()
}

fn main() {
    let settings = AnalyzerSettings::default();
    let always = Wants {
        spectrum: false,
        sonogram: false,
        spectral: false,
        surface: false,
        scope: false,
        envelope: false,
    };
    let cases = [
        ("loudness and stereo", always),
        (
            "+ spectrum",
            Wants {
                spectrum: true,
                ..always
            },
        ),
        (
            "+ sonogram",
            Wants {
                sonogram: true,
                ..always
            },
        ),
        (
            "+ 3D",
            Wants {
                surface: true,
                ..always
            },
        ),
        (
            "+ scope and envelope",
            Wants {
                scope: true,
                envelope: true,
                ..always
            },
        ),
        ("every view", Wants::ALL),
    ];
    // One second of signal, repeated, so the timing measures only the analysis.
    let signal: Vec<[f32; 2]> = (0..RATE)
        .map(|n| {
            let t = n as f32 / RATE as f32;
            let value = 0.5 * (TAU * 440.0 * t).sin() + 0.1 * (TAU * 5_000.0 * t).sin();
            [value, 0.8 * value]
        })
        .collect();
    for (label, wants) in cases {
        // The best of several runs, as other work on the machine only slows a run down.
        let seconds = (0..RUNS)
            .map(|_| run(&settings, wants, &signal))
            .fold(f64::INFINITY, f64::min);
        println!(
            "{label:<20} {:>7.1}× real time ({seconds:.2} s for {SECONDS} s of audio)",
            f64::from(SECONDS) / seconds
        );
    }
}
