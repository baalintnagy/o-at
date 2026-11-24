use std::f32::consts::TAU;

use super::{Engine, NoiseMode, Resolution, Settings};
use crate::dsp::level::power_to_db;
use crate::dsp::profile::ProfileStore;

/// Uniform white noise in −1..1.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }

    fn signal(&mut self, length: usize, amplitude: f32) -> Vec<f32> {
        (0..length).map(|_| amplitude * self.next()).collect()
    }
}

fn engine(rate: f32, channels: usize, resolution: Resolution) -> Engine {
    let mut engine = Engine::new(ProfileStore::default(), None);
    engine.configure(rate, channels, resolution);
    engine
}

/// Processes mono `input` in blocks of `block` samples.
fn run(engine: &mut Engine, input: &[f32], block: usize, settings: &Settings) -> Vec<f32> {
    let mut output = input.to_vec();
    for chunk in output.chunks_mut(block) {
        engine.process(&mut [chunk], settings);
    }
    output
}

fn rms_db(signal: &[f32]) -> f32 {
    power_to_db(signal.iter().map(|sample| sample * sample).sum::<f32>() / signal.len() as f32)
}

/// Level of the component at `frequency` Hz, by correlation, in dB.
fn tone_db(signal: &[f32], frequency: f32, rate: f32) -> f32 {
    let (sine, cosine) = signal
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(sine, cosine), (n, x)| {
            let phase = TAU * frequency * n as f32 / rate;
            (sine + x * phase.sin(), cosine + x * phase.cos())
        });
    let amplitude = 2.0 * (sine * sine + cosine * cosine).sqrt() / signal.len() as f32;
    20.0 * amplitude.log10()
}

fn unity() -> Settings {
    Settings {
        reduction_db: 0.0,
        ..Settings::default()
    }
}

#[test]
fn no_reduction_delays_the_input_exactly() {
    for rate in [22_050.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        for resolution in Resolution::ALL {
            for (channels, block) in [(1, 333), (2, 64), (2, 1000)] {
                let mut engine = engine(rate, channels, resolution);
                let settings = Settings {
                    resolution,
                    ..unity()
                };
                let latency = engine.latency_samples() as usize;
                let length = latency * 3 + 5_000;
                let mut noise = Noise(7);
                let inputs: Vec<Vec<f32>> =
                    (0..channels).map(|_| noise.signal(length, 0.5)).collect();
                let mut outputs = inputs.clone();
                let mut offset = 0;
                while offset < length {
                    let end = (offset + block).min(length);
                    let mut slices: Vec<&mut [f32]> = outputs
                        .iter_mut()
                        .map(|channel| &mut channel[offset..end])
                        .collect();
                    engine.process(&mut slices, &settings);
                    offset = end;
                }
                for (input, output) in inputs.iter().zip(&outputs) {
                    let error = input[..length - latency]
                        .iter()
                        .zip(&output[latency..])
                        .skip(latency)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0, f32::max);
                    assert!(
                        error < 1e-4,
                        "{rate} Hz {resolution:?} {channels} ch: error {error}"
                    );
                }
            }
        }
    }
}

#[test]
fn reported_latency_matches_impulse_delay() {
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for resolution in Resolution::ALL {
            let mut engine = engine(rate, 1, resolution);
            // Reduction on: the delay must not depend on the gains.
            let settings = Settings {
                resolution,
                mode: NoiseMode::Adaptive,
                ..Settings::default()
            };
            let length = rate as usize;
            let mut input = vec![0.0; length];
            input[length / 4] = 1.0;
            let output = run(&mut engine, &input, 256, &settings);
            let peak = output
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .map(|(index, _)| index)
                .unwrap_or_default();
            assert_eq!(
                peak - length / 4,
                engine.latency_samples() as usize,
                "{rate} Hz {resolution:?}"
            );
        }
    }
}

#[test]
fn listen_removed_complements_the_output() {
    let rate = 48_000.0;
    let mut noise = Noise(3);
    let input: Vec<f32> = noise
        .signal(rate as usize * 2, 0.05)
        .iter()
        .enumerate()
        .map(|(n, x)| x + 0.3 * (TAU * 440.0 * n as f32 / rate).sin())
        .collect();
    let settings = Settings {
        mode: NoiseMode::Adaptive,
        ..Settings::default()
    };
    let kept = run(
        &mut engine(rate, 1, Resolution::Medium),
        &input,
        512,
        &settings,
    );
    let removed = run(
        &mut engine(rate, 1, Resolution::Medium),
        &input,
        512,
        &Settings {
            listen_removed: true,
            ..settings
        },
    );
    let latency = Resolution::Medium.fft_size(rate);
    let error = kept[latency..]
        .iter()
        .zip(&removed[latency..])
        .zip(&input)
        .map(|((kept, removed), input)| (kept + removed - input).abs())
        .fold(0.0, f32::max);
    assert!(error < 1e-4, "error {error}");
}

/// Learns a profile at `learn_at` from a second of noise, then processes noise and
/// noise with a sine at `resolution`. Returns the noise and the sine levels removed
/// in dB.
fn learned_reduction(resolution: Resolution, learn_at: Resolution) -> (f32, f32) {
    let result = learned_reduction_inner(resolution, learn_at);
    eprintln!("{resolution:?} (learned {learn_at:?}): {result:?}");
    result
}

fn learned_reduction_inner(resolution: Resolution, learn_at: Resolution) -> (f32, f32) {
    let rate = 48_000.0;
    let mut engine = engine(rate, 1, learn_at);
    let mut noise = Noise(11);
    let learn = Settings {
        learn: true,
        resolution: learn_at,
        ..Settings::default()
    };
    run(&mut engine, &noise.signal(rate as usize, 0.1), 480, &learn);
    let settings = Settings {
        resolution,
        reduction_db: 30.0,
        ..Settings::default()
    };
    // Stopping learning stores the profile.
    run(
        &mut engine,
        &noise.signal(rate as usize / 2, 0.1),
        480,
        &settings,
    );
    assert!(engine.has_profile, "profile stored");

    let length = rate as usize * 3;
    let tail = length / 3;
    let background = noise.signal(length, 0.1);
    let noise_out = run(&mut engine, &background, 480, &settings);
    let noise_removed = rms_db(&background[tail..]) - rms_db(&noise_out[tail..]);

    let tone: Vec<f32> = background
        .iter()
        .enumerate()
        .map(|(n, x)| x + 0.2 * (TAU * 1_000.0 * n as f32 / rate).sin())
        .collect();
    let tone_out = run(&mut engine, &tone, 480, &settings);
    let tone_removed =
        tone_db(&tone[tail..], 1_000.0, rate) - tone_db(&tone_out[tail..], 1_000.0, rate);
    (noise_removed, tone_removed)
}

#[test]
fn learned_profile_removes_noise_and_keeps_tones() {
    let results: Vec<_> = Resolution::ALL
        .iter()
        .map(|resolution| (resolution, learned_reduction(*resolution, *resolution)))
        .collect();
    for (resolution, (noise, tone)) in results {
        assert!(noise > 20.0, "{resolution:?}: noise reduced by {noise} dB");
        assert!(
            tone.abs() < 0.5,
            "{resolution:?}: tone changed by {tone} dB"
        );
    }
}

#[test]
fn profiles_work_at_other_resolutions() {
    let (noise, tone) = learned_reduction(Resolution::High, Resolution::Low);
    assert!(noise > 20.0, "noise reduced by {noise} dB");
    assert!(tone.abs() < 0.5, "tone changed by {tone} dB");
}

#[test]
fn adaptive_estimate_matches_the_noise_power() {
    let mut ratios = Vec::new();
    for (rate, resolution) in Resolution::ALL
        .iter()
        .flat_map(|resolution| [44_100.0, 48_000.0, 96_000.0].map(|rate| (rate, *resolution)))
    {
        let mut engine = engine(rate, 1, resolution);
        let settings = Settings {
            mode: NoiseMode::Adaptive,
            resolution,
            ..Settings::default()
        };
        let mut noise = Noise(5);
        let mut ratio_sum = 0.0;
        let mut count = 0;
        for block in 0..rate as usize * 8 / 480 {
            run(&mut engine, &noise.signal(480, 0.1), 480, &settings);
            if block > 400 {
                let bins = engine.bins;
                // Uniform noise of amplitude 0.1: power 0.1² / 3 per sample.
                let expected = 0.01 / 3.0;
                let estimate = engine.trackers[0].estimate()[4..bins - 4]
                    .iter()
                    .sum::<f32>()
                    / (bins - 8) as f32;
                ratio_sum += estimate / expected;
                count += 1;
            }
        }
        ratios.push((rate, resolution, ratio_sum / count as f32));
    }
    assert!(
        ratios
            .iter()
            .all(|(_, _, ratio)| (0.8..1.25).contains(ratio)),
        "estimates relative to the noise power: {ratios:?}"
    );
}

#[test]
fn adaptive_mode_removes_steady_noise() {
    let rate = 44_100.0;
    let mut engine = engine(rate, 2, Resolution::Medium);
    let settings = Settings {
        mode: NoiseMode::Adaptive,
        link: false,
        ..Settings::default()
    };
    let mut noise = Noise(9);
    let length = rate as usize * 4;
    let mut left = noise.signal(length, 0.05);
    let mut right = noise.signal(length, 0.05);
    let original = left.clone();
    for (left, right) in left.chunks_mut(441).zip(right.chunks_mut(441)) {
        engine.process(&mut [left, right], &settings);
    }
    let removed = rms_db(&original[length / 2..]) - rms_db(&left[length / 2..]);
    assert!(removed > 12.0, "noise reduced by {removed} dB");
}
