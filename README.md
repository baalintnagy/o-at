# O-AT

**Open Audio Tools**: open source audio plugins and applications written in Rust.

| Tool | Purpose |
| --- | --- |
| [O-NEC](#o-nec) | Voice cleanup: noise suppression, echo cancellation, voice gate and speech leveler. |
| [O-AUD-TX and O-AUD-RX](#o-aud-tx-and-o-aud-rx) | Route audio between hosts, applications and devices through named buses. |
| [O-AUD-A](#o-aud-a) | Audio analyzer: spectrum, sonogram, stereo imager, oscilloscope, loudness meter and 3D views. |

The tools are listed under the vendor `o-at` in plugin hosts. O-NEC, O-AUD-TX and
O-AUD-RX replace the earlier ONEC, AUD-TX and AUD-RX plugins. Their plugin IDs are new
(CLAP `o-at.o-nec`, `o-at.o-aud-tx`, `o-at.o-aud-rx`), so sessions saved with the earlier
plugins do not load them; replace the earlier plugins in those sessions.

## O-NEC

**Open Noise and Echo Cancelation** is a VST3 and CLAP voice cleanup plugin written in
Rust. It does what the noise suppression and echo cancellation built into voice chat
applications do, as a plugin you can put on any microphone chain, with adjustable
parameters and detailed visual feedback.

![O-NEC editor](docs/screenshot.png)

### What it does

| Stage | Purpose |
| --- | --- |
| Input gain and high-pass | Level trim and rumble removal (2nd-order Butterworth, 20–400 Hz). |
| Echo canceller | Learns the loudspeaker-to-microphone echo path from the sidechain and subtracts it. |
| Residual echo suppressor | Removes echo left by the linear canceller (non-linearities, tail, path changes). |
| Noise suppression | Removes background noise while keeping speech, with a choice of two neural models: RNNoise or DeepFilterNet3. |
| Voice gate | Opens on detected speech or above a level, like automatic or manual input sensitivity. |
| Speech leveler | Optional. Raises whispering or distant talk and lowers shouting towards a target level, measured only while someone speaks, and limits the peaks of sudden shouts. |
| Output gain | Final level trim. |

The editor shows:

- A spectrum analyzer with input, output and echo reference spectra. The pink area
  between input and output is what O-NEC removes. Hover for per-frequency readouts.
- A draggable high-pass filter handle with its response curve (double-click toggles it).
- A 4-second activity timeline of input and output levels, voice probability, gate
  state, echo reference activity and the speech leveler's gain, with a draggable gate
  threshold.
- Removed noise, voice probability, echo cancellation (ERLE), gate attenuation, speech
  level, leveler gain and peak limiting meters; echo canceller state (waiting, learning, locked, double talk); peak meters.
- **Listen removed** solos what the chain takes away, to tune settings without
  damaging speech.

The noise panel's **RNNoise / DeepFilter** switch selects the noise model. A line below
its meters shows the model's state, including when RNNoise fills in for DeepFilterNet
while it loads or during CPU overload.

Knobs: drag vertically, hold Shift for fine adjustment, use the mouse wheel, and
double-click to reset.

### Noise models

| | RNNoise | DeepFilterNet3 |
| --- | --- | --- |
| Designed for | Steady noise: fans, hum, traffic | Also non-stationary noise, such as keyboards |
| Stage delay | 10 ms | 30 ms plus 20–30 ms of scheduling slack |
| Total latency at 48 kHz | 32 ms | 72 ms (buffers up to 480 samples), 82 ms (up to 960) |
| CPU | Lowest | About 0.6 ms per 10 ms frame, on its own thread |

RNNoise is the default. Select DeepFilterNet for the strongest suppression when the extra
delay is acceptable. Switching models changes the latency reported to the host.

DeepFilterNet's inference allocates memory, so it cannot run on the audio thread. It runs
on a worker thread, and its results are used a fixed number of frames later: one more
than a single host buffer can hold, which is the scheduling slack above. If the computer
cannot keep up, RNNoise's time-aligned output fills in for the missing frames. RNNoise also
keeps running in DeepFilterNet mode to provide the voice probability for the gate.

### Echo cancellation needs a reference

A voice chat application cancels echo because it knows what it plays to your
speakers. A plugin only knows what its host gives it, so O-NEC takes the far-end
signal (what your speakers play) on its **Echo Reference** sidechain input. Without a
reference, the echo stages stay idle and O-NEC works as a noise suppressor and gate.

Route the speaker signal to the sidechain in your host: a DAW's sidechain routing, a
loopback device in a virtual mixer, or any VST3 host that exposes auxiliary inputs.
The reference must not pass through O-NEC's output. Speaker-to-microphone delays up to
the **Tail** setting (50–500 ms) are covered; set it a little above your system's
output-plus-input latency plus room reverberation.

### Parameters

| Parameter | Range | Default | Notes |
| --- | --- | --- | --- |
| Noise Suppression | on/off | on | RNNoise keeps running for the voice detector. |
| Noise Model | RNNoise, DeepFilter | RNNoise | Changes the latency. |
| Noise Reduction | 0–100 dB, Max | Max | Limits attenuation by mixing in the aligned dry signal. |
| High-Pass, Frequency | on/off, 20–400 Hz | on, 80 Hz | |
| Echo Cancellation | on/off | on | Needs the sidechain reference. |
| Echo Suppression | 0–100 % | 50 % | Residual echo suppression strength. |
| Echo Tail | 50–500 ms | 250 ms | Modeled echo path length. |
| Gate | Off, Voice, Level | Voice | |
| Voice / Level Threshold | 0–100 %, −80–0 dB | 50 %, −50 dB | |
| Gate Range, Hold, Release | 0–80 dB, 0–1000 ms, 10–1000 ms | 40 dB, 250 ms, 150 ms | |
| Speech Leveler | on/off | off | |
| Leveler Speed | Slow, Medium, Fast | Medium | How quickly the gain follows the speaker. |
| Leveler Target | −40 to −6 dBFS | −20 dBFS | Mean level of speech to reach. |
| Leveler Amount | 0–100 % | 80 % | Share of the level difference corrected. |
| Leveler Max Boost, Max Cut | 0–30 dB | 12 dB each | Limits of the gain. |
| Input / Output Gain | ±24 dB | 0 dB | |
| Listen Removed | on/off | off | Outputs input minus output. |

### Latency and formats

O-NEC processes 10 ms frames at 48 kHz internally, the native format of RNNoise and
DeepFilterNet.
Other host rates that are whole multiples of 100 Hz (8–192 kHz, including 44.1 kHz)
are converted with rubato's synchronous FFT resampler. Latency is reported to the host:
32 ms at 48 kHz with RNNoise (frame collection, residual echo suppressor and RNNoise, 10 ms
each, and the leveler's 2 ms lookahead), slightly more with resampling. With DeepFilterNet, see [Noise models](#noise-models).
22.05 and 11.025 kHz are not supported.

Mono and stereo layouts are available. Stereo input is mixed to mono, processed, and
written to both outputs.

### Build

Requires Rust 1.85 or later (edition 2024). On Windows, use the MSVC toolchain.

```bash
cargo xtask bundle o-nec --release
```

Bundles are written to `target/bundled/O-NEC.vst3` and `target/bundled/O-NEC.clap`.
Copy the VST3 bundle to your VST3 folder, for example
`C:\Program Files\Common Files\VST3` on Windows.

To try the plugin without a host:

```bash
cargo run --release --features standalone --bin o-nec-standalone -- --help
```

On Linux, building the editor needs X11, XCB and OpenGL development packages.

### Verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features o-nec/standalone,o-aud-tx/standalone,o-aud-a/standalone -- -D warnings
cargo test --release
cargo test --test realtime
cargo test -p audbus -p o-aud-tx -p o-aud-rx
cargo test -p o-aud-a
```

The tests cover echo canceller convergence (over 25 dB ERLE) and near-end
preservation during double talk, residual echo suppression, latency alignment and noise
reduction of both noise models, the RNNoise fallback for missing DeepFilterNet frames, gate timing and hysteresis, analyzer calibration, reported versus
measured latency at several host rates and buffer sizes for both models, and the
complete chain at 44.1 kHz with both models. The `realtime` test runs the chain with both
models, editor analysis and reset under NIH-plug's allocation guard; it only checks in
debug builds. A performance test requires the full chain with a 500 ms tail to run at
least 20 times (RNNoise) or 5 times (DeepFilterNet) faster than real time in release
builds.

The VST3 bundle passes [pluginval](https://github.com/Tracktion/pluginval) 1.0.4 at
strictness level 10, both as a release build and as a debug build with allocation
checks enabled:

```bash
pluginval --strictness-level 10 --validate target/bundled/O-NEC.vst3
```

See [DSP design](docs/dsp.md) and [source style](docs/source-style.md).

## O-AUD-TX and O-AUD-RX

Two companion plugins and an application route audio between hosts and devices
through named buses, for example from a DAW to a voice chat application's device or to
headphones on another interface.

- **O-AUD-TX** (VST3, CLAP) sends its track's audio to a bus with a name you choose.
  Its own output passes through unchanged, or is muted with **Thru** off.
- **O-AUD-RX** (VST3, CLAP) plays a bus inside any host, mixed with the track's input.
- The standalone **O-AUD-RX** application plays a bus on any output device. It uses the
  same receiver core and editor as the plugin.

| O-AUD-TX | O-AUD-RX application |
| --- | --- |
| ![O-AUD-TX editor](docs/o-aud-tx.png) | ![O-AUD-RX application](docs/o-aud-rx.png) |

Buses live in shared memory and need no driver or server. Any number of receivers can
play one bus. O-AUD-RX resamples to its output rate and locks to the transmitter's clock,
so different devices, sample rates and buffer sizes work together without drift; the
editor shows the queued latency, its target, jitter, clock drift in ppm, dropouts and a
latency history.

Using them:

1. Insert O-AUD-TX where the audio should leave the host. It names itself `Bus 1`,
   `Bus 2` and so on; type another name if you like (up to 32 bytes, case-insensitive).
   If another O-AUD-TX uses the name, the editor offers a free one. Hosts such as Ableton
   Live keep keystrokes for their own shortcuts: then click **EDIT**, which opens the
   name in a small window of its own that always receives the keyboard.
2. In O-AUD-RX, pick the bus from the list, or type its name (or use **EDIT**). A name
   without a transmitter yet waits for one.
3. Set **Latency** to the buffering you want (2–250 ms, default 20 ms). O-AUD-RX raises
   its target automatically when the transmitter's or the device's blocks need more,
   and lowers it again slowly once they are steady.

The application remembers its bus, device and settings, and accepts
`--bus NAME` and `--device NAME`. On Windows the settings are stored in
`%APPDATA%\O-AUD-RX\settings.ini`.

Build them with:

```bash
cargo xtask bundle o-aud-tx --release
```

```bash
cargo xtask bundle o-aud-rx --release
```

```bash
cargo build --release -p o-aud-rx-app
```

The bundles are written to `target/bundled/`, the application to
`target/release/o-aud-rx.exe`. O-AUD-TX also builds as a standalone application that sends
an audio input to a bus (`cargo run --release -p o-aud-tx --features standalone --bin
o-aud-tx-standalone`).

Limitations: buses carry up to 8 channels (the plugins use mono and stereo layouts);
transmitter and receivers must run on the same computer and, on Windows, in the same
user session; a transmitter whose clock is more than 0.5 % off cannot be followed
without dropouts. See [Audio bus design](docs/audio-bus.md).

## O-AUD-A

**O-AUD-A** (VST3, CLAP and a standalone application) is an audio analyzer. Insert it
anywhere; it passes the audio through unchanged and reports no latency. Resize the
editor from its lower right corner. **FULL SCREEN** (or F11) takes the plugin window
full screen on Windows, without its frame; **RESTORE** or Esc brings it back. Hosts
cannot resize NIH-plug editors from their own window frame, so these are the ways to
change its size.

![O-AUD-A overview with the spectral view, stereo imager, spectrum, oscilloscope and loudness meter](docs/o-aud-a/overview.png)

The tabs at the top select a page: the overview, or one view filling the editor. The
header shows the sample rate and channel layout, and whether a signal arrives:
**ANALYZING**, **NO SIGNAL**, or **OVERLOAD** when samples were dropped because the
analysis fell behind. Its pages, in this order, show:

| Page | What it shows | Options |
| --- | --- | --- |
| Overview | Any of the views at once, arranged as you like: switch each one on or off under **VIEWS**, drag a view's title onto another to move it beside, above or below it, and drag the gaps to resize. Narrow loudness panels show their numbers in a strip above the meters, and the history when it fits. | Seven views, up to four rows; reset layout |
| Spectral | Two layers: a sonogram, and on top of it the waveform of the same signal, aligned column by column (peak extent and RMS core), as in spectral editors. Its sonogram is independent of the Sonogram page: it has its own FFT, channels, scale, speed and range. | Sonogram layer: opacity, brightness, contrast and every sonogram option; waveform layer: opacity, gain (vertical zoom), linear or dB scale. The opacity sliders are logarithmic. |
| Spectrum | Spectrum of left and right, mid and side, or one signal, with peak hold and a hover readout of frequency, note and level. | FFT 1K–32K, Hann, Blackman-Harris or flat-top window, averaging (off to infinite), 1/24–1/3 octave smoothing, 0–6 dB/oct slope, 30–150 dB range, 20 kHz or Nyquist, log, linear or semitone (ST) frequency scale |
| Sonogram | Scrolling time × frequency image, newest on the right, with a hover readout. | FFT 1K–32K, one signal (mid, side, left or right) or two stacked lanes (L · R or M · S), log, linear or semitone (ST) frequency, 15–240 columns/s, 30–150 dB range |
| Scope | Oscilloscope of one or two signals, overlaid or stacked, triggered on a rising or falling level crossing with sub-sample alignment so periodic signals stand still; drag the T marker to set the level. Shows the frequency between crossings and a hover readout. | 1 ms – 60 s span (above 500 ms a scrolling roll view of the min/max envelope, without trigger); L·R, M·S or one signal; free, rising or falling trigger on mid, side, left or right; automatic or ×1–×32 gain |
| Stereo | Goniometer, correlation and balance, mid and side levels, the side-to-mid ratio (in the header), and the stereo position and width of 30 third-octave bands (red where they are out of phase). | M/S goniometer, polar sample cloud or L/R scope, each drawn as dots or lines; 512–4096 samples; automatic or fixed gain; band FFT length (shared with the spectrum) |
| Loudness | EBU R 128 / ITU-R BS.1770-4 momentary, short-term and integrated loudness, loudness range, true peak (4× oversampled), sample peak and PLR, bar meters and a loudness history, all on the diagrams' shared background and grid. The header shows the target and the measured time. | Targets −14, −16, −23, −24 LUFS; LUFS or LU; 30 s to 10 min history; RESET in the panel header or in its settings |
| 3D | The spectrum history as ridges or a shaded surface, or the stereogram: level over stereo position and frequency. Drag to rotate, shift-drag or right-drag to move, wheel to zoom, double-click to reset. | 64–256 bands, 32–128 rows, 30–150 dB range, a 3D grid (floor, back walls and dB levels), camera rotation, elevation and zoom with RESET VIEW, FFT, one scene or two stacked ones (L · R or M · S, shared with the Sonogram page); its heat map is a color |

| | |
| --- | --- |
| ![Spectral page: waveform over sonogram](docs/o-aud-a/spectral.png) | ![Spectrum page](docs/o-aud-a/spectrum.png) |
| **Spectral**: the waveform's peak extent and RMS core drawn over the sonogram; quiet passages and drum hits line up with their spectra. | **Spectrum**: left and right over a filled area; the header shows the FFT size, bin width and window length. |
| ![Sonogram page](docs/o-aud-a/sonogram.png) | ![Oscilloscope page](docs/o-aud-a/scope.png) |
| **Sonogram**: the last 17 s at 60 columns/s, with the heat map's level scale on the right. | **Scope**: left and right overlaid, triggered at the T marker; the header shows the time base, the vertical scale, the gain and the measured frequency. |
| ![Stereo page](docs/o-aud-a/stereo.png) | ![Loudness page](docs/o-aud-a/loudness.png) |
| **Stereo**: the goniometer, the meters, and each third-octave band's position (dot) and width (bar). | **Loudness**: integrated loudness against the target, the readouts, M, S and I bars, the history of short-term and momentary loudness and true peak per channel. |
| ![3D page](docs/o-aud-a/3d.png) | ![Spectrum settings popup](docs/o-aud-a/settings.png) |
| **3D**: the spectrum history as a shaded surface with the 3D grid on; the footer shows the camera's yaw, pitch and zoom. | **Settings**: the spectrum's SETTINGS tab, with segmented selectors and sliders. |

The screenshots show a synthetic test piece: chords, bass, drums and a melody, panned
across the stereo field.

### Settings and colors

Every view has a popup, opened with the button at the right end of its header or by
**right-clicking the diagram**, on its page and in the overview alike.

- Its **SETTINGS** tab holds what the view shows: segmented selectors for kinds (window,
  channels, scale) and sliders for ranges and steps (FFT length, level range, slope,
  speed, time span). Double-click a slider to reset it. Headings mark settings shared
  with another view, for example the 3D view's analysis, which is the Sonogram page's.
- Its **COLORS** tab holds the view's colors, each with a color picker and, where it
  applies, an opacity, brightness or contrast slider. Examples: the spectrum's left/mid,
  right and side curves, fill and peak hold; the loudness meter's short-term and
  momentary lines and its on-target, too-loud and too-quiet colors; the heat map of
  the sonogram, spectral and 3D views (four built-in maps or three custom colors).
  **Reset this view's colors** restores the defaults.
- Every view keeps its own colors, on its page and in the overview. The diagrams'
  background and grid are shared by every view and appear in each COLORS tab.
- Esc or a click outside closes the popup.

![Sonogram COLORS tab with the heat map, brightness, contrast, background and grid](docs/o-aud-a/colors.png)

**THEME** colors only the controls (buttons, tabs, sliders, panel marks), with a preset
(O-AT, Phosphor, Sunset, Ocean, Mono, Contrast with Okabe-Ito colors) or an accent of
your own. **FREEZE** holds the display while the measurements continue. The settings,
the layout and the selected page are stored with the session.

In the overview, the **VIEWS** button at the right end of the tabs opens a panel that
shows or hides each view; its **RESET LAYOUT** restores the default arrangement. Drag
a view's title onto another view to move it beside, above or below that view, and drag
the gaps between views to resize them.

![VIEWS panel with a switch per view and RESET LAYOUT](docs/o-aud-a/views.png)

The audio thread only copies samples into a lock-free ring. A worker thread measures
them: loudness and stereo meters all the time, so integrated loudness keeps counting
while the editor is closed, and the FFT views only while the editor is open. See
[Analyzer design](docs/o-aud-a.md).

```bash
cargo xtask bundle o-aud-a --release
```

```bash
cargo run --release -p o-aud-a --features standalone --bin o-aud-a-standalone -- --backend wasapi --input-device "In 1-2 (Your Interface)"
```

The standalone application analyzes the input device named by `--input-device` (without
it, no input is connected; an unknown name lists the available devices) and outputs
silence.

## Built on

- [NIH-plug](https://github.com/robbert-vdh/nih-plug) for the VST3/CLAP wrappers and
  [egui](https://github.com/emilk/egui) editor integration (pinned revision).
- [nnnoiseless](https://github.com/jneem/nnnoiseless), a Rust port of Xiph's
  [RNNoise](https://github.com/xiph/rnnoise).
- [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet) 0.5.6 (libDF) with its
  DeepFilterNet3 model, running on [tract](https://github.com/sonos/tract), and
  [rtrb](https://github.com/mgeier/rtrb) for the lock-free worker queues.
- [realfft](https://github.com/HEnquist/realfft)/[RustFFT](https://github.com/ejmahler/RustFFT),
  [rubato](https://github.com/HEnquist/rubato), [biquad](https://github.com/korken89/biquad-rs)
  and [triple_buffer](https://github.com/HadrienG2/triple-buffer).

The echo canceller follows the multidelay block frequency-domain design used by
Speex (Valin, 2007) and is implemented here on top of realfft.

## Known limitations of O-NEC

- RNNoise targets realistic noise such as fans, hum and traffic. Loud, flat white
  noise is reduced less. Very non-stationary noise (keyboard clicks) may leak; use
  DeepFilterNet for it.
- DeepFilterNet treats steady synthetic tones as noise, so test it with real speech.
  Both models are tested with synthetic signals only, not with recorded speech.
- Echo cancellation needs a clean reference; with clock drift between separate
  playback and capture devices, performance degrades over time.
- The editor has a fixed size of 1200 × 680 logical pixels. Hosts restore the editor
  size saved with a session, so sessions saved with an earlier, narrower O-NEC open
  with the right-hand panels cut off until O-NEC is added again.
- The speech leveler relies on the voice detector. Whispering has little voicing and
  may be detected less reliably than normal speech; it is tested with synthetic vowels,
  not recorded whispering.

## Releases

Pushing a tag `vX.Y.Z` that matches the version in `Cargo.toml` runs the
[release workflow](.github/workflows/release.yml): it runs every CI check, builds the
VST3 and CLAP bundles and the four applications, and publishes a GitHub release with
`O-AT-X.Y.Z-windows-x64.zip` (installation notes in
[INSTALL.txt](packaging/INSTALL.txt)) and its SHA-256 checksum. The workflow can also be
started by hand with a version.

The executables embed their icons and version information. The icons are generated
from vector shapes; regenerate them after changing [the generator](tools/icons/generate.py)
with `python tools/icons/generate.py` (requires Pillow).

## License

The O-AT source code is licensed under the [MIT License](LICENSE). Built VST3 binaries
include GPLv3-licensed VST3 bindings; see [NOTICE](NOTICE.md).
