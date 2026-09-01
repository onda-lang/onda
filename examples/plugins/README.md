# Plug-in-oriented examples

These patches are shared with `onda-plugin` and exercise the canonical host-event surfaces now
supported by `onda run`:

- `instruments/poly_saw.onda` is an eight-voice saw, filter, and saturation instrument with MIDI
  note on/off and per-channel pitch bend.
- `instruments/fm_bells.onda` is an eight-voice velocity-sensitive FM bell bank with ADSR
  articulation, an inharmonic partial, a filtered strike transient, and cross-fed stereo ambience.
- `effects/tempo_ping_pong.onda` is a filtered cross-feedback delay. Its optional canonical
  `tempo(bpm: f64)` event follows DAW tempo in a plug-in and retains 120 BPM in `onda run`.
- `effects/reactive_wavefolder.onda` is an oversampled stereo wavefolder whose fold depth and
  brightness follow the input envelope.
- `effects/transient_sculptor.onda` separates fast attacks from the slower body of a sound and
  provides independently bipolar attack and sustain controls.
- `effects/orbit_flanger.onda` uses quadrature delay modulation and filtered stereo cross-feedback
  for a wide, animated flanging field.

Run an instrument and play the computer or on-screen keyboard immediately, or choose a physical
device from the MIDI Input selector:

```bash
onda run examples/plugins/instruments/poly_saw.onda
```

The effect patches require a live audio input device when used with `onda run`. Canonical
`plugin_host` events remain hidden and inactive outside a DAW host.
