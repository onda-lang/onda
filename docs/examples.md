---
title: Example cookbook
description: Musical instruments, effects, soundscapes, graph patches, and advanced DSP in Onda.
permalink: /docs/examples/
section: examples
eyebrow: Learn by listening
---

# Example cookbook

Onda's examples are a collection of finished sounds, not a second language reference. Every
playground entry makes sound at its defaults and is meant to be heard, changed, and reused. The
[language guide]({{ '/docs/language/' | relative_url }}) and
[standard-library reference]({{ '/docs/stdlib/' | relative_url }}) cover isolated syntax and APIs.

Run or render any source example from a checkout:

```bash
onda run examples/instruments/fm_bells.onda
onda run render examples/soundscapes/granular_cloud.onda --output cloud.wav --dur 12
```

Examples with built-in sequencing expose an `auto_play` switch. It defaults to `1`; set it to `0`
to stop scheduling new notes while existing voices and effect tails decay naturally. Each also
exposes a no-argument `bang()` event that immediately triggers and advances the next sequencer
state. Live-input processing and any additional instrument events remain available.

## A listening path

| Example | Listen for | Onda idea |
| --- | --- | --- |
| [Saw/filter/saturator]({{ '/playground/?example=basic/saw_filter_saturator.onda' | relative_url }}) | Wide subtractive motion and clean drive | Stdlib processors plus local oversampling |
| [FM bells]({{ '/playground/?example=instruments/fm_bells.onda' | relative_url }}) | Independent amplitude and FM-index decays | Proc arrays, events, and voice panning |
| [Formant percussion]({{ '/playground/?example=instruments/formant_percussion.onda' | relative_url }}) | Pitched clicks opening into vowel resonances | Parallel resonators and reusable reverb |
| [Granular cloud]({{ '/playground/?example=soundscapes/granular_cloud.onda' | relative_url }}) | A recorded phrase dissolving into moving grains | Circular recording and independent playheads |
| [Spectral delay]({{ '/playground/?example=spectral/spectral_delay.onda' | relative_url }}) | Frequency bands arriving at different times | Per-bin FFT history and resynthesis |
| [Benjolin]({{ '/playground/?example=feedback/benjolin.onda' | relative_url }}) | Stepped rungler melodies tipping into insects and bass | Clocked bit feedback and audio-rate modulation |

## Instruments

| Example | What it makes | Change first |
| --- | --- | --- |
| [Acid bassline]({{ '/playground/?example=instruments/acid_bassline.onda' | relative_url }}) | A resonant 16-step bass line with accents and slides | `cutoff`, `env_amount`, `drive` |
| [Drum machine]({{ '/playground/?example=instruments/drum_machine.onda' | relative_url }}) | Synthesized kick, snare, and metallic hats | `swing`, decay times, `drive` |
| [FM bells]({{ '/playground/?example=instruments/fm_bells.onda' | relative_url }}) | A spacious polyphonic struck-metal pattern | `brightness`, `decay_s`, `root_note` |
| [Formant percussion]({{ '/playground/?example=instruments/formant_percussion.onda' | relative_url }}) | Vowel-like resonant percussion | `vowel`, `resonance`, `decay_s` |
| [Karplus–Strong]({{ '/playground/?example=instruments/karplus_strong.onda' | relative_url }}) | Warm plucked strings from short delay lines | `damping`, `brightness` |

The polyphonic instruments self-play and also keep their host-facing strike/pluck events where that
interaction is part of the instrument.

## Effects

| Example | What it does | Change first |
| --- | --- | --- |
| [Stereo chorus]({{ '/playground/?example=effects/stereo_chorus.onda' | relative_url }}) | Fractional-delay ensemble motion | `depth_ms`, `width` |
| [Tape echo]({{ '/playground/?example=effects/tape_echo.onda' | relative_url }}) | Dark saturated repeats with wow and flutter | `feedback`, `tone`, `wow` |
| [Live tape looper]({{ '/playground/?example=effects/live_tape_looper.onda' | relative_url }}) | Capture, varispeed, reverse, and overdub on a virtual tape | `speed`, `overdub`, the `reverse` event |
| [Shimmer echo]({{ '/playground/?example=effects/shimmer_echo.onda' | relative_url }}) | Cross-fed echoes with octave-shifted regeneration | `shimmer`, `shift`, `window_s` |
| [Wavefolder]({{ '/playground/?example=effects/wavefolder.onda' | relative_url }}) | Animated folded harmonics | `folds`, `bias` |
| [Compressor]({{ '/playground/?example=effects/compressor.onda' | relative_url }}) | Stereo-linked soft-knee compression | `threshold_db`, `attack_ms` |
| [Schroeder reverb]({{ '/playground/?example=effects/schroeder_reverb.onda' | relative_url }}) | A bright classic comb-and-allpass room | `room_size`, `damping` |
| [FDN reverb]({{ '/playground/?example=effects/fdn_reverb.onda' | relative_url }}) | A dense eight-line matrix tail | `decay`, `diffusion` |

Effects audition themselves by default. Set `live_input` to `1` to process `in1` and `in2`. The
Schroeder and FDN wrappers are also compact, musical graph-syntax examples: their `graph` blocks
route the audition/live source, parameters, and stereo outputs declaratively.

## Soundscapes and experimental systems

| Example | What it makes | Main idea |
| --- | --- | --- |
| [Benjolin]({{ '/playground/?example=feedback/benjolin.onda' | relative_url }}) | A Hordijk-inspired eight-bit rungler instrument | BLEP oscillators, clocked shift register, selectable signals |
| [Granular cloud]({{ '/playground/?example=soundscapes/granular_cloud.onda' | relative_url }}) | A live tape resampled as a stereo grain cloud | Cubic lookup, Hann windows, randomized playheads |
| [Wind chimes]({{ '/playground/?example=soundscapes/wind_chimes.onda' | relative_url }}) | Irregular modal chimes over filtered air | Seeded scheduling and polyphonic decays |
| [Deep-space drone]({{ '/playground/?example=soundscapes/deep_space_drone.onda' | relative_url }}) | A slowly changing low-frequency chord | Layering, filtering, and long echoes |
| [Polyphonic saw]({{ '/playground/?example=basic/polyphonic_saw.onda' | relative_url }}) | An eight-voice randomized saw cascade | Event allocation and proc arrays |
| [Neural synth]({{ '/playground/?example=feedback/neural_synth.onda' | relative_url }}) | A recurrent nonlinear digital ecosystem | Parameterized topology and oversampled recurrence |
| [Resonant delay matrix]({{ '/playground/?example=feedback/resonant_delay_matrix.onda' | relative_url }}) | Four nonlinear cross-coupled delays | Resonant feedback and waveshaping |
| [Diffuse delay matrix]({{ '/playground/?example=feedback/diffuse_delay_matrix.onda' | relative_url }}) | A soft, slowly evolving resonant cloud | Filtered excitation and damped regeneration |
| [Chaotic delay matrix]({{ '/playground/?example=feedback/chaotic_delay_matrix.onda' | relative_url }}) | Burst-driven unstable resonances | Multi-rate modulation, irregular triggers, and damping |

## Advanced DSP and graph syntax

| Example | What it demonstrates |
| --- | --- |
| [Spectral delay]({{ '/playground/?example=spectral/spectral_delay.onda' | relative_url }}) | A 1024-point streaming FFT, per-bin delay frames, spectral feedback, and stereo IFFT resynthesis |
| [Spectral freeze]({{ '/playground/?example=spectral/spectral_freeze.onda' | relative_url }}) | Phase-coherent spectral capture, smear, transpose, and overlap-add resynthesis |
| [Cybernetic feedback graph]({{ '/playground/?example=feedback/cybernetic_feedback_graph.onda' | relative_url }}) | Cross-coupled graph cycles made causal with `>>[1]` delayed edges |
| [Dual FM oscillator, 8×]({{ '/playground/?example=basic/dual_fm_osc.onda' | relative_url }}) | A compact musical use of local oversampling and feedback phase modulation |

Inspect a lowered graph with:

```bash
onda compile examples/feedback/cybernetic_feedback_graph.onda --dump-graph
```

## Self-contained projects

Use `.ondaproject` when data is part of a patch's identity:

| Project | Data carried with the patch |
| --- | --- |
| [Wavetable Garden](https://github.com/onda-lang/onda/blob/main/examples/projects/wavetable_garden/wavetable-garden.ondaproject) | Four inline wavetables and a local oscillator module |
| [Score-driven Resonator](https://github.com/onda-lang/onda/blob/main/examples/projects/score_driven_resonator/score-driven-resonator.ondaproject) | Typed note, timing, velocity, and pan buffers |
| [Embedded Room](https://github.com/onda-lang/onda/blob/main/examples/projects/embedded_room/embedded-room.ondaproject) | A file-backed stereo `impulse.wav` loaded into zero-latency partitioned convolvers |

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

The breadth audit was informed by the outcome-first examples in
[MMMAudio](https://github.com/mmmaudio/mmmaudio/tree/main/examples). The granular, spectral-delay,
formant, and spatial ideas here are original Onda implementations; no source was copied.
