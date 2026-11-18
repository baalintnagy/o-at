# DSP design

O-NEC converts host audio to mono 10 ms frames at 48 kHz, runs a fixed chain on each
frame and converts back. All stages on the audio thread are allocation-free after
initialization; DeepFilterNet runs on a worker thread.

```text
host rate ─► block of 10 ms ─► FFT resampler ─► 480 samples @ 48 kHz
   mic ─► input gain ─► high-pass ─► echo canceller ─► residual echo suppressor
                                          ▲                    ▲
   reference (sidechain) ─────────────────┴── echo estimate ───┘
       ─► RNNoise or DeepFilterNet (noise suppression) ─► voice gate ─► speech leveler
       ─► output gain
          RNNoise always: voice probability for the gate and leveler, DeepFilterNet fallback
48 kHz ─► FFT resampler ─► host rate
```

## Framing and latency

[`StreamProcessor`](../src/dsp/stream.rs) collects one 10 ms block at the host rate,
which requires the host rate to be a multiple of 100 Hz. At 48 kHz the block is used
directly. Other rates use rubato's synchronous FFT resampler with both chunk sizes
fixed, so every host block maps to exactly one internal frame. Output lags input by:

| Part | Delay |
| --- | --- |
| Block collection | 10 ms at the host rate |
| Upsampler and downsampler | rubato's reported `output_delay` of each |
| Residual echo suppressor | 480 samples (one STFT hop) |
| RNNoise model | 480 samples (its analysis/synthesis overlap) |
| DeepFilterNet model, instead of RNNoise | 1440 samples (overlap plus two frames of lookahead) plus the worker pipeline |
| Speech leveler | 96 samples (2 ms limiter lookahead) |

Each stage keeps its delay when bypassed, so the reported latency only changes with the
noise model; the plugin reports the new latency when the model is switched. A test
measures the impulse delay at several rates and buffer sizes against the reported
value for both models.

## Echo canceller

[`EchoCanceller`](../src/dsp/aec.rs) is a partitioned-block frequency-domain adaptive
filter (PBFDAF) with 480-sample partitions and 960-point FFTs, covering 50–500 ms.
It uses overlap-save convolution and a gradient constraint on every partition, which
keeps the time-domain filter causal and avoids circular-convolution errors.

Step-size control follows Valin's MDF algorithm used in Speex:

1. For the first 1.2 s of reference activity, the filter adapts with a fixed
   normalized step size to bootstrap.
2. Afterwards, the per-bin step size is `leakage · |Ŷ|² / |E|²`, capped at 0.5 and
   normalized by the reference power. Leakage is the recursive correlation between
   error and echo-estimate power fluctuations divided by the echo-estimate variance:
   the fraction of the echo estimate that remains in the output.

During double talk the near-end speech raises `|E|²` without correlating with the echo,
so the step size shrinks automatically. No separate double-talk switch is needed.

A background filter adapts continuously. The foreground filter, which produces the
output, copies the background when the background error is clearly lower for two
consecutive frames. A background filter with much higher error than the foreground is
restored from it. If the foreground output exceeds four times the microphone energy
for 200 ms, the canceller resets.

ERLE for the meter is the smoothed ratio of microphone to output energy while the
reference is active. Double talk is flagged when the output energy is much larger than
the expected residual echo.

## Residual echo suppressor

[`EchoSuppressor`](../src/dsp/suppressor.rs) analyzes the canceller output and echo
estimate with a 960-point sqrt-Hann STFT at 50% overlap. The residual echo power per
bin is the echo estimate power scaled by the leakage and an overdrive derived from the
**Echo Suppression** setting, held with an exponential decay for reverberation. The
gain is `max(1 − residual / signal, floor)`, with a floor down to −45 dB at full
strength and asymmetric smoothing to avoid musical noise. At zero strength or without
echo, the stage is an exact delay.

## Noise suppression

[`Denoiser`](../src/dsp/denoise.rs) runs RNNoise through nnnoiseless on every frame.
DeepFilterNet, when selected, replaces RNNoise's output; RNNoise keeps running for the
voice probability.
RNNoise computes 22 band gains and a voice probability with a small recurrent network
and applies pitch filtering. The **Noise Reduction** setting limits attenuation by
mixing in the dry signal, delayed by RNNoise's 480-sample delay so the mix does not
comb-filter. RNNoise keeps running when suppression is off, so the voice gate always
has a voice probability.

### DeepFilterNet

[`DeepFilter`](../src/dsp/deep_filter.rs) runs libDF's DeepFilterNet3 model (ERB gains
plus deep filtering of the lower spectrum, 960-point STFT with 480-sample hop and two
frames of lookahead) through tract. Its inference allocates on every frame, so it runs on
a worker thread:

1. The audio thread pushes each frame, with a sequence number, into a lock-free `rtrb`
   ring buffer and unparks the worker.
2. The worker processes frames in order and pushes results into a second ring buffer.
   It loads the model when the first frame arrives, and restarts the model's stream when
   a sequence number is skipped (reset, dropped frames, re-enabling).
3. The audio thread takes the result of the frame submitted *K* frames earlier. *K* is one
   more than the number of frames a single host buffer can complete, so results computed
   during one host callback are ready for the next. Older results are discarded.

If the wanted result is missing in real-time mode, RNNoise's output, delayed to the same
total delay, is used for that frame and the editor shows the overload. When the host
renders offline, the audio thread waits for every result. The dry path and the gate's
voice probability are delayed to match, keeping RNNoise's 10 ms of gate lookahead.

libDF skips frames with a mean square below 1e-7 without advancing its state, which would
replay stale spectra after digital silence. Near-silent frames therefore get dither of
about −60 dBFS before inference.

## Voice gate

[`Gate`](../src/dsp/gate.rs) opens when RNNoise's voice probability or the frame level
crosses the threshold, with hysteresis so it does not chatter at the threshold. It
holds for the hold time, then releases to the range attenuation. Opening uses a 1.5 ms
attack. Because RNNoise's voice probability describes input that the RNNoise output
will only emit one frame later, the gate effectively has 10 ms of lookahead.

## Speech leveler

[`Leveler`](../src/dsp/leveler.rs) evens out speech levels after the gate, so it only
raises speech that the earlier stages have cleaned. It has two parts:

- **Gain control.** A frame counts as speech when RNNoise's voice probability is at
  least 50 % and its mean power is above −65 dBFS. Only speech frames update the
  estimated speech level, a one-pole average in dB. Each frame's deviation counts at
  most ±12 dB, so single syllables and clicks move it little. The time constant is
  3 s, 1.5 s or 0.6 s of speech for Slow, Medium and Fast. Getting louder is followed
  in 35 % of that time. The estimate starts from the first speech frame with a 100 ms
  time constant that lengthens as speech accumulates. The gain is the
  difference between target and estimate, scaled by Amount and limited to Max Boost and
  Max Cut. It moves at most 0.5 dB per frame and holds in pauses, so noise and breathing
  between words are not raised.
- **Peak limiter.** It catches shout onsets before the gain control reacts. Its ceiling
  is 16 dB above the target, at most −1 dBFS. For every sample it takes the gain that
  keeps that sample under the ceiling. It holds the smallest such gain over a 2 ms
  window with a monotonic queue, releases it over 80 ms, and averages it over the
  window again. The audio is delayed by the 2 ms lookahead, so the averaged gain reaches
  each peak's gain before the peak is output, without steps.

The limiter's delay is kept when the leveler is off. Listen Removed pauses the leveler,
so the removed signal is not mixed with gain changes.

## Metering

The engine keeps a fixed-size [`Telemetry`](../src/telemetry.rs) snapshot and
publishes it through a triple buffer when the editor is open. Spectra use a 2048-point
Hann-windowed FFT every frame, mapped to 160 logarithmic bands between 20 Hz and
20 kHz with fast attack and 120 dB/s release. A full-scale sine reads 0 dBFS. The input
spectrum and level history use the delayed dry signal, so input and output are time
aligned on screen.
