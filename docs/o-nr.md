# O-NR design

O-NR removes noise from any material. It estimates the noise power in every frequency
bin, from a learned profile or a tracked noise floor, and attenuates each bin by how
much of it is noise. Everything runs on the audio thread, in `Engine::process`; nothing
allocates there. The [README](../README.md#o-nr) describes the controls.

## Signal path

| Stage | Module | What it does |
| --- | --- | --- |
| STFT analysis | `dsp::engine` | Periodic Hann window, FFT size *N* from the resolution, hop *N*/4. |
| Power | `dsp::engine` | Power spectral density per bin: squared magnitude divided by the window energy, so broadband noise reads the same at every FFT size. Linked channels use their mean. |
| Noise estimate | `dsp::profile`, `dsp::noise` | The stored profile, converted to the current bins, or the minimum statistics tracker. |
| Gain | `dsp::gain` | OM-LSA gain per bin, release hold, smoothing across frequency, reduction floor. |
| Synthesis | `dsp::engine` | Inverse FFT, Hann synthesis window, overlap-add. With unity gains the output is the input delayed by exactly *N* samples. |

The delay is the FFT size: the first finished hop of a frame is its oldest. It is the
same whether anything is attenuated, so the host's compensation stays valid.
`reported_latency_matches_impulse_delay` checks it at 44.1, 48 and 96 kHz for every
resolution. The resolutions are 1024, 2048, 4096 and 8192 samples at 48 kHz; the size
scales with the sample rate to the nearest power of two, so the frequency resolution
and the frame rate stay about the same at every rate.

## Gains

Each bin's gain is the optimally modified log-spectral amplitude estimator (Cohen and
Berdugo, 2001). The log-spectral amplitude MMSE gain (Ephraim and Malah, 1985) avoids
most of the musical noise of plain spectral subtraction. Weighting it by the
probability that the bin holds signal lets bins of pure noise fall to the floor. The
a priori SNR is the decision-directed estimate (weight 0.98).

The threshold multiplies the noise estimate. A profile whose level varied more than
stationary noise does (5.57 dB standard deviation per bin) raises its own threshold by
half the excess, so fluctuating noise such as rain or crowds is not taken for signal
whenever it peaks.

Gains fall at the release rate. They rise at once only where a bin and both its
neighbours rise together: a tone or an attack spans several bins (the Hann main lobe is
four bins wide), a random noise peak usually one. Without that rule, every noise peak
would open a bin that then closes slowly, and white noise was only reduced by 12–17 dB
at a 30 dB setting. Smoothing across frequency is a zero-phase one-pole filter that only
raises gains, filling narrow dips between kept bins. Smoothing that also lowered peaks
took 0.5–1 dB off a steady tone.

Finally `gain = floor + (1 − floor) × gain`, where the floor comes from Reduction. At
0 dB the floor is exactly 1, so the output is the delayed input bit for bit (within
FFT rounding).

## Noise profiles

`Learner` accumulates per bin the mean power and the mean and variance of the level in
dB while LEARN is on. When learning stops after at least 0.25 s, the engine queues the
mean level and the spread for `ProfileStore`. While learning, gains are 1: the signal
passes unchanged.

`ProfileStore` holds one profile in fixed-size atomic arrays (up to 16 385 bins, the
largest FFT at 192 kHz) guarded by a sequence counter, like a seqlock. Writers make the
counter odd while they write; readers discard a read during which it changed and try
again on the next frame. The audio thread stores learned profiles with `try_store`,
which never waits, and reads with `load_into`, which converts the stored bins to the
current ones. The host's state handling and the editor's CLEAR button use the waiting
`store`. The store is the plugin's `#[persist = "profile"]` state, serialized as
`ProfileData` (bin spacing in Hz, level and spread in dB per bin).

Profiles keep the bin spacing they were learned with. The conversion averages the power
of the source bins inside each target bin, or interpolates the level where the target
bins are finer, so a profile learned at one rate or resolution works at another.
Switching resolution while learning finishes the profile with the bins it was learned
with.

## Adaptive tracking

`MinimumTracker` follows Martin's minimum statistics (2001): the noise power is the
minimum of the power smoothed over 40 ms, taken over the last 1.5 s in six sub-windows,
times a bias correction. Signals rarely hold a bin's level up for that long, so the
minimum follows the noise floor beneath them. The bias depends on how many frames the
window holds and how much the smoothing averages, so it is calibrated per resolution
(2.45, 3.29, 4.71 and 6.88 from Low to Max) on white noise.
`adaptive_estimate_matches_the_noise_power` keeps the estimate within 0.8–1.25 of the
true noise power at 44.1, 48 and 96 kHz. Profile mode falls back to the tracker while no
profile is stored, and the editor says so.

## Threads and the editor

| Thread | Work |
| --- | --- |
| Audio | The whole engine. Publishes a fixed-size `Telemetry` (192 display bands, levels, state) into a triple buffer once per frame. |
| Editor | Reads the telemetry, draws, and sets the parameters. LEARN is an `AtomicBool` shared with the plugin, not a parameter, so a session never restores a running learn. CLEAR writes the empty profile into the store. |
| Host | Serializes and restores the profile through the store. |

`Engine::configure`, called from `initialize`, allocates every buffer and FFT plan for
the four resolutions. Switching resolution only rewrites the window and clears the
state. `tests/realtime.rs` runs every resolution, mode and link setting, learning,
a restored profile at another bin spacing, listen removed, resolution switches during
learning and reset under NIH-plug's allocation guard (debug builds).

## Tests

`cargo test -p o-nr` covers, besides the real-time test:

- the exact delayed identity at 0 dB reduction for 22.05–192 kHz, every resolution,
  mono and stereo and odd block sizes;
- reported against measured latency;
- listen removed plus the output adding up to the delayed input;
- a learned profile lowering white noise by more than 20 dB (28 dB at a 30 dB setting)
  while a 1 kHz tone changes by less than 0.5 dB (under 0.05 dB measured), at every
  resolution and with a profile learned at another resolution;
- the adaptive estimate's calibration and noise reduction in unlinked stereo;
- profile conversion, storage and serialization, the exponential integral and the
  learner's statistics.

## Limits

- The tests use synthetic white noise and tones, not recordings.
- Adaptive mode treats a note held longer than about 1.5 s as noise floor and lowers
  it. Use a learned profile for sustained material.
- Changing Resolution clears the processing state, which is audible as a short gap, and
  changes the latency.
- Unlinked stereo uses the one profile, learned from the mean of the channels, for both
  channels.
