---
title: Example cookbook
description: Musical instruments, effects, soundscapes, graph patches, and advanced DSP in Onda.
permalink: /docs/examples/
section: examples
eyebrow: Learn by listening
---

# Example cookbook

The examples are a collection of patches that showcase different ways of implementing audio
algorithms and DSP code in Onda. The [language guide]({{ '/docs/language/' | relative_url }}) and
[standard-library reference]({{ '/docs/stdlib/' | relative_url }}) cover isolated syntax and APIs.

Examples with built-in sequencing expose an `auto_play` switch. It defaults to `true`; set it to `false`
to stop scheduling new notes while existing voices and effect tails decay naturally. Each also
exposes a no-argument `bang()` event that immediately triggers and advances the next sequencer
state. Live-input processing and any additional instrument events remain available.

## Instruments

| Example | Description |
| --- | --- |
| [Additive synth]({{ '/playground/?example=instruments/additive_synth.onda' | relative_url }}) | Eight sine partials with independent level, ratio, detune, pan, and mute controls |
| [Acid bassline]({{ '/playground/?example=instruments/acid_bassline.onda' | relative_url }}) | A resonant 16-step bass line with accents and slides |
| [Drum machine]({{ '/playground/?example=instruments/drum_machine.onda' | relative_url }}) | Synthesized kick, snare, and metallic hats |
| [FM bells]({{ '/playground/?example=instruments/fm_bells.onda' | relative_url }}) | A spacious polyphonic struck-metal pattern |
| [Formant percussion]({{ '/playground/?example=instruments/formant_percussion.onda' | relative_url }}) | Vowel-like resonant percussion |
| [Karplus–Strong]({{ '/playground/?example=instruments/karplus_strong.onda' | relative_url }}) | Warm plucked strings from short delay lines |

The polyphonic instruments self-play and also keep their host-facing strike/pluck events where that
interaction is part of the instrument.

## Effects

| Example | Description |
| --- | --- |
| [Stereo chorus]({{ '/playground/?example=effects/stereo_chorus.onda' | relative_url }}) | Fractional-delay ensemble motion |
| [Tape echo]({{ '/playground/?example=effects/tape_echo.onda' | relative_url }}) | Dark saturated repeats with wow and flutter |
| [Live tape looper]({{ '/playground/?example=effects/live_tape_looper.onda' | relative_url }}) | Capture, varispeed, reverse, and overdub on a virtual tape |
| [Shimmer echo]({{ '/playground/?example=effects/shimmer_echo.onda' | relative_url }}) | Cross-fed echoes with octave-shifted regeneration |
| [Wavefolder]({{ '/playground/?example=effects/wavefolder.onda' | relative_url }}) | Animated folded harmonics |
| [Compressor]({{ '/playground/?example=effects/compressor.onda' | relative_url }}) | Stereo-linked soft-knee compression |
| [Schroeder reverb]({{ '/playground/?example=effects/schroeder_reverb.onda' | relative_url }}) | A bright classic comb-and-allpass room |
| [FDN reverb]({{ '/playground/?example=effects/fdn_reverb.onda' | relative_url }}) | A dense eight-line matrix tail |

Effects audition themselves by default. Set `live_input` to `true` to process `in1` and `in2`. The
Schroeder and FDN wrappers are also compact, musical graph-syntax examples: their `graph` blocks
route the audition/live source, parameters, and stereo outputs declaratively.

## Soundscapes and experimental systems

| Example | Description |
| --- | --- |
| [Benjolin]({{ '/playground/?example=feedback/benjolin.onda' | relative_url }}) | A Hordijk-inspired eight-bit rungler instrument |
| [Granular cloud]({{ '/playground/?example=soundscapes/granular_cloud.onda' | relative_url }}) | A live tape resampled as a stereo grain cloud |
| [Wind chimes]({{ '/playground/?example=soundscapes/wind_chimes.onda' | relative_url }}) | Irregular modal chimes over filtered air |
| [Deep-space drone]({{ '/playground/?example=soundscapes/deep_space_drone.onda' | relative_url }}) | A slowly changing low-frequency chord |
| [Aurora Pad]({{ '/playground/?example=soundscapes/aurora_pad.onda' | relative_url }}) | A minor chord spread across a gently detuned stereo field and a slowly opening filter |
| [Orbital FM]({{ '/playground/?example=soundscapes/orbital_fm.onda' | relative_url }}) | An evolving phase-modulation drone with detuned carriers, sub-octave body, and orbiting shimmer |
| [Polyphonic saw]({{ '/playground/?example=basic/polyphonic_saw.onda' | relative_url }}) | An eight-voice randomized saw cascade |
| [Neural synth]({{ '/playground/?example=feedback/neural_synth.onda' | relative_url }}) | A recurrent nonlinear digital ecosystem |
| [Resonant delay matrix]({{ '/playground/?example=feedback/resonant_delay_matrix.onda' | relative_url }}) | Four nonlinear cross-coupled delays |
| [Diffuse delay matrix]({{ '/playground/?example=feedback/diffuse_delay_matrix.onda' | relative_url }}) | A soft, slowly evolving resonant cloud |
| [Chaotic delay matrix]({{ '/playground/?example=feedback/chaotic_delay_matrix.onda' | relative_url }}) | Burst-driven unstable resonances |

## Advanced DSP and graph syntax

| Example | Description |
| --- | --- |
| [Spectral delay]({{ '/playground/?example=spectral/spectral_delay.onda' | relative_url }}) | A 1024-point streaming FFT, per-bin delay frames, spectral feedback, and stereo IFFT resynthesis |
| [Spectral freeze]({{ '/playground/?example=spectral/spectral_freeze.onda' | relative_url }}) | Phase-coherent spectral capture, smear, transpose, and overlap-add resynthesis |
| [PaulStretch]({{ '/playground/?example=spectral/paul_stretch.onda' | relative_url }}) | A host-bound recording with normalized scrubbing, stretched through an event/delegate chain carrying structured time, magnitude, and complex-spectrum frames |
| [Cybernetic feedback graph]({{ '/playground/?example=feedback/cybernetic_feedback_graph.onda' | relative_url }}) | Cross-coupled graph cycles made causal with `>>[1]` delayed edges |
| [Dual FM oscillator, 8×]({{ '/playground/?example=basic/dual_fm_osc.onda' | relative_url }}) | A compact musical use of local oversampling and feedback phase modulation |

## MIDI and plug-in host surfaces

These patches are shared with `onda-plugin`. The instruments use the exact canonical MIDI events,
so opening either patch with `onda run` reveals the bottom piano and MIDI Input selector. The
computer keyboard is active by default; in the browser, choose **Connect MIDI device…** for hardware
MIDI or play the on-screen keys.

| Example | Description |
| --- | --- |
| [MIDI poly saw]({{ '/playground/?example=plugins/instruments/poly_saw.onda' | relative_url }}) | Eight-voice note on/off and per-channel pitch bend |
| [MIDI FM bells]({{ '/playground/?example=plugins/instruments/fm_bells.onda' | relative_url }}) | Velocity-sensitive note on/off allocation |
| [Tempo ping-pong]({{ '/playground/?example=plugins/effects/tempo_ping_pong.onda' | relative_url }}) | DAW tempo in a plug-in; 120 BPM fallback elsewhere |
| [Reactive wavefolder]({{ '/playground/?example=plugins/effects/reactive_wavefolder.onda' | relative_url }}) | Stereo live input with envelope-driven folding |
| [Transient sculptor]({{ '/playground/?example=plugins/effects/transient_sculptor.onda' | relative_url }}) | Stereo live input with fast/slow envelope separation |
| [Orbit flanger]({{ '/playground/?example=plugins/effects/orbit_flanger.onda' | relative_url }}) | Stereo live input with quadrature modulation |

The four effects require a selected audio input or browser microphone. Canonical `plugin_host`
events are hidden but inactive in standalone hosts; a DAW supplies them through the plug-in.

## Self-contained projects

Use `.ondaproject` when data is part of a patch's identity:

| Project | Description |
| --- | --- |
| [Wavetable Garden](https://github.com/onda-lang/onda/blob/main/examples/projects/wavetable_garden/wavetable-garden.ondaproject) | Four inline wavetables and a local oscillator module |
| [Score-driven Resonator](https://github.com/onda-lang/onda/blob/main/examples/projects/score_driven_resonator/score-driven-resonator.ondaproject) | Typed note, timing, velocity, and pan buffers |
| [Embedded Room](https://github.com/onda-lang/onda/blob/main/examples/projects/embedded_room/embedded-room.ondaproject) | A file-backed stereo `impulse.wav` cooperatively transformed into zero-latency convolution kernels, with an explicit FFT size and a configurable loading duration (0.5 seconds by default) |

These links open the repository because the hosted `?example=` catalog contains source workspaces,
not `.ondaproject` manifests and their buffer assets. To open a showcase in the browser playground,
download or clone the repository, ZIP the complete project contents, and select the ZIP with
**Open project**. The browser picker does not accept a bare `.ondaproject`: selecting only the
manifest would not give the page access to its referenced `code/` and `assets/` files.

For example:

```bash
cd examples/projects/embedded_room
zip -r embedded-room.zip embedded-room.ondaproject code assets
```

The native CLI and run hosts have filesystem access and can open the manifest directly:

```bash
onda run examples/projects/embedded_room/embedded-room.ondaproject
```

See [Onda projects]({{ '/docs/projects/' | relative_url }}) for the manifest format.

## Host integration

- [Sample player](https://github.com/onda-lang/onda/blob/main/examples/buffers/sample_player.onda) — event-driven, sample-rate-correct external-buffer playback.
- [Raw native processor object](https://github.com/onda-lang/onda/tree/main/examples/native/raw_processor_object) — compile and call an Onda processor from C.
- [Embedded compiler playground](https://github.com/onda-lang/onda/tree/main/examples/web/onda_wasm_playground) — compile editable Onda projects in the browser.
- [AOT WebAssembly sample player](https://github.com/onda-lang/onda/tree/main/examples/web/onda_wasm_aot_sample_player) — host a precompiled processor without shipping a compiler.
