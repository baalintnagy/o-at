# Vendored code

## nih_plug

`nih_plug/` is the `nih_plug` crate (and its `nih_plug_derive`) from
[NIH-plug](https://github.com/robbert-vdh/nih-plug) at revision `de42101`, the revision the
workspace pins. It is ISC-licensed; see `nih_plug/LICENSE`. The root `Cargo.toml` uses it
through `[patch]`, so `nih_plug_egui` and the other NIH-plug crates build against it too.

It differs from upstream only in `src/wrapper/standalone/backend/cpal.rs`, the
standalone applications' audio backend (changes marked `O-AT patch`):

- Input samples arrive interleaved, but upstream filled the plugin's buffer one channel
  after the other, so each channel of a stereo input received alternating samples of both
  channels. The buffer is now filled frame by frame.
- The input ring held exactly one period. A device delivering larger blocks, such as a
  shared-mode WASAPI device with a 512-sample period, filled it before playback started
  and the application never received input. The ring now has room for at least 16 384
  frames, and playback starts as soon as the first input block arrives.

The upstream workspace table and profiles were removed from its `Cargo.toml`.

To update NIH-plug, move the pins in every `Cargo.toml` to the new revision, then either
remove `[patch]` and this directory, if the new revision has these fixes, or copy the
crate again from that revision and reapply the changes.
