//! End-to-end checks of the complete chain through the host-rate stream adapter.

use std::f32::consts::{PI, TAU};
use std::time::Instant;

use o_nec::dsp::deep_filter::Pacing;
use o_nec::dsp::denoise::NoiseModel;
use o_nec::dsp::engine::{Engine, EngineSettings};
use o_nec::dsp::gate::GateTrigger;
use o_nec::dsp::level::{db_to_gain, energy, power_to_db};
use o_nec::dsp::leveler::LevelerSettings;
use o_nec::dsp::stream::StreamProcessor;

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Far-end signal with speech-like syllable envelopes over coloured noise.
fn far_end(rate: usize, seconds: usize) -> Vec<f32> {
    let mut noise = Noise(17);
    let mut coloured = 0.0;
    (0..rate * seconds)
        .map(|n| {
            coloured = 0.8 * coloured + 0.2 * noise.next();
            let t = n as f32 / rate as f32;
            let syllables = 0.6 + 0.4 * (TAU * 4.0 * t).sin();
            0.4 * syllables * coloured
        })
        .collect()
}

/// Voiced speech-like signal: a 120 Hz glottal pulse train through three vowel formant
/// resonators, in syllables of 200 ms with short pauses, scaled to a mean power of
/// `level_db` dBFS during the syllables.
fn vowels(rate: usize, seconds: usize, level_db: f32) -> Vec<f32> {
    let formants = [(700.0, 110.0), (1_220.0, 120.0), (2_600.0, 160.0)];
    let mut resonators: Vec<(f32, f32, f32, f32)> = formants
        .iter()
        .map(|(frequency, bandwidth)| {
            let radius = (-PI * bandwidth / rate as f32).exp();
            let theta = TAU * frequency / rate as f32;
            (2.0 * radius * theta.cos(), -radius * radius, 0.0, 0.0)
        })
        .collect();
    let period = rate / 120;
    let mut signal: Vec<f32> = (0..rate * seconds)
        .map(|n| {
            let mut sample = if n % period == 0 { 1.0 } else { 0.0 };
            for (a1, a2, y1, y2) in resonators.iter_mut() {
                let y = sample + *a1 * *y1 + *a2 * *y2;
                *y2 = *y1;
                *y1 = y;
                sample = y;
            }
            let in_syllable = (n * 1000 / rate) % 250 < 200;
            if in_syllable { sample } else { 0.0 }
        })
        .collect();
    let voiced: Vec<f32> = signal
        .iter()
        .copied()
        .filter(|sample| *sample != 0.0)
        .collect();
    let scale = db_to_gain(level_db) / (energy(&voiced) / voiced.len() as f32).sqrt();
    signal.iter_mut().for_each(|sample| *sample *= scale);
    signal
}

/// Echo path: 40 ms delay, a reflection and a short decay.
fn echo_of(reference: &[f32], rate: usize) -> Vec<f32> {
    let delay = rate * 40 / 1000;
    let reflection = rate * 55 / 1000;
    (0..reference.len())
        .map(|n| {
            let direct = n.checked_sub(delay).map_or(0.0, |i| 0.5 * reference[i]);
            let late = n
                .checked_sub(reflection)
                .map_or(0.0, |i| -0.25 * reference[i]);
            direct + late
        })
        .collect()
}

fn run(processor: &mut StreamProcessor, mic: &[f32], reference: &[f32], buffer: usize) -> Vec<f32> {
    let mut output = vec![0.0; mic.len()];
    for ((mic, reference), output) in mic
        .chunks(buffer)
        .zip(reference.chunks(buffer))
        .zip(output.chunks_mut(buffer))
    {
        processor.process(mic, reference, output);
    }
    output
}

/// Echo with background hum through the complete chain, with either noise model.
fn removes_echo_and_background_noise(noise_model: NoiseModel) {
    let rate = 44_100;
    let seconds = 8;
    let reference = far_end(rate, seconds);
    let echo = echo_of(&reference, rate);
    let mut noise = Noise(5);
    let mut hum = 0.0;
    let mic: Vec<f32> = echo
        .iter()
        .map(|sample| {
            hum = 0.97 * hum + 0.03 * noise.next();
            sample + 0.05 * hum
        })
        .collect();

    let mut engine = Engine::new(None);
    engine.set_settings(EngineSettings {
        noise_model,
        ..EngineSettings::default()
    });
    let mut processor = StreamProcessor::new(engine);
    processor
        .configure(rate as u32, 256, Pacing::Offline)
        .expect("44.1 kHz is supported");
    let output = run(&mut processor, &mic, &reference, 256);

    let tail = rate * (seconds - 2)..;
    let reduction = power_to_db(energy(&output[tail.clone()]) / energy(&mic[tail]));
    assert!(
        reduction < -30.0,
        "{noise_model:?}: echo and noise reduction {reduction:.1} dB"
    );
}

#[test]
fn removes_echo_and_background_noise_with_rnnoise() {
    removes_echo_and_background_noise(NoiseModel::RnNoise);
}

#[test]
fn removes_echo_and_background_noise_with_deep_filter() {
    removes_echo_and_background_noise(NoiseModel::DeepFilterNet);
}

#[test]
fn voice_gate_silences_noise_only_input() {
    let rate = 48_000;
    let mut noise = Noise(23);
    let mut coloured = 0.0;
    let mic: Vec<f32> = (0..rate * 3)
        .map(|_| {
            coloured = 0.9 * coloured + 0.1 * noise.next();
            0.2 * coloured
        })
        .collect();
    let reference = vec![0.0; mic.len()];

    let mut engine = Engine::new(None);
    let mut settings = EngineSettings {
        denoise_enabled: false,
        ..EngineSettings::default()
    };
    settings.gate.trigger = GateTrigger::Voice;
    settings.gate.range_db = 40.0;
    engine.set_settings(settings);
    let mut processor = StreamProcessor::new(engine);
    processor
        .configure(rate as u32, 480, Pacing::Realtime)
        .expect("48 kHz is supported");
    let output = run(&mut processor, &mic, &reference, 480);

    let tail = rate * 2..;
    let reduction = power_to_db(energy(&output[tail.clone()]) / energy(&mic[tail]));
    assert!(reduction < -35.0, "gated reduction {reduction:.1} dB");
}

/// The full chain must comfortably run in real time. With DeepFilterNet, offline pacing
/// waits for every worker result, so this measures the worker's throughput too.
///
/// The floors guard against regressions that threaten real-time use, not against small
/// slowdowns: shared CI runners measure about half of what a desktop does, and their
/// speed varies from run to run.
#[test]
fn runs_faster_than_real_time() {
    for (noise_model, required) in [(NoiseModel::RnNoise, 8.0), (NoiseModel::DeepFilterNet, 3.0)] {
        let rate = 48_000;
        let seconds = 10;
        let reference = far_end(rate, seconds);
        let mic = echo_of(&reference, rate);
        let mut engine = Engine::new(None);
        engine.set_settings(EngineSettings {
            echo_tail_ms: 500,
            analyze: true,
            noise_model,
            ..EngineSettings::default()
        });
        let mut processor = StreamProcessor::new(engine);
        processor
            .configure(rate as u32, 512, Pacing::Offline)
            .expect("48 kHz is supported");
        // Load DeepFilterNet's model before timing.
        run(&mut processor, &mic[..rate], &reference[..rate], 512);

        let start = Instant::now();
        run(&mut processor, &mic, &reference, 512);
        let elapsed = start.elapsed().as_secs_f32();
        let realtime_factor = seconds as f32 / elapsed;
        println!("{noise_model:?} chain with 500 ms tail: {realtime_factor:.0}x real time");
        if !cfg!(debug_assertions) {
            assert!(
                realtime_factor > required,
                "{noise_model:?}: only {realtime_factor:.1}x real time"
            );
        }
    }
}

/// Levels both quiet and loud speech towards the target through the whole chain.
#[test]
fn leveler_evens_out_quiet_and_loud_speech() {
    let rate = 48_000;
    let seconds = 8;
    let level_of = |input_db: f32| {
        let mic = vowels(rate, seconds, input_db);
        let reference = vec![0.0; mic.len()];
        let mut engine = Engine::new(None);
        engine.set_settings(EngineSettings {
            leveler: LevelerSettings {
                enabled: true,
                target_db: -20.0,
                amount: 1.0,
                max_boost_db: 24.0,
                max_cut_db: 24.0,
                ..LevelerSettings::default()
            },
            ..EngineSettings::default()
        });
        let mut processor = StreamProcessor::new(engine);
        processor
            .configure(rate as u32, 480, Pacing::Offline)
            .expect("48 kHz is supported");
        let output = run(&mut processor, &mic, &reference, 480);
        let tail = rate * (seconds - 3)..;
        let input = power_to_db(energy(&mic[tail.clone()]) / (3 * rate) as f32);
        let output = power_to_db(energy(&output[tail]) / (3 * rate) as f32);
        (input, output)
    };
    let (quiet_in, quiet_out) = level_of(-40.0);
    let (loud_in, loud_out) = level_of(-8.0);
    println!("quiet {quiet_in:.1} -> {quiet_out:.1} dB, loud {loud_in:.1} -> {loud_out:.1} dB");
    assert!(
        loud_in - quiet_in > 30.0 && (loud_out - quiet_out).abs() < 4.0,
        "quiet {quiet_in:.1} -> {quiet_out:.1} dB, loud {loud_in:.1} -> {loud_out:.1} dB"
    );
}

/// Noise without speech must not be raised, even with a large boost.
#[test]
fn leveler_does_not_raise_noise() {
    let rate = 48_000;
    let mut noise = Noise(29);
    let mic: Vec<f32> = (0..rate * 4).map(|_| 0.003 * noise.next()).collect();
    let reference = vec![0.0; mic.len()];
    let output_level = |enabled: bool| {
        let mut engine = Engine::new(None);
        engine.set_settings(EngineSettings {
            denoise_enabled: false,
            leveler: LevelerSettings {
                enabled,
                max_boost_db: 30.0,
                ..LevelerSettings::default()
            },
            ..EngineSettings::default()
        });
        let mut processor = StreamProcessor::new(engine);
        processor
            .configure(rate as u32, 480, Pacing::Realtime)
            .expect("48 kHz is supported");
        let output = run(&mut processor, &mic, &reference, 480);
        power_to_db(energy(&output[rate..]))
    };
    let raised = output_level(true) - output_level(false);
    assert!(raised.abs() < 0.5, "noise raised by {raised:.1} dB");
}
