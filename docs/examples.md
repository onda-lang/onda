---
title: Example cookbook
description: Musical instruments, effects, soundscapes, graph patches, and advanced DSP in Onda.
permalink: /docs/examples/
section: examples
eyebrow: Learn by listening
---

# Example cookbook

Open an example in the playground to hear it, adjust its controls, and edit the code. The examples
cover instruments, effects, soundscapes, and more advanced DSP techniques. Use the
[language guide]({{ '/docs/language/' | relative_url }}) and
[standard-library reference]({{ '/docs/stdlib/' | relative_url }}) when you want to understand a
particular feature or module.

## Instruments

| Example | Description |
| --- | --- |
| [Acid bassline]({{ '/playground/?example=instruments/acid_bassline.onda' | relative_url }}) | A resonant 16-step bass line with accents and slides |
| [Additive synth]({{ '/playground/?example=instruments/additive_synth.onda' | relative_url }}) | A plucked arpeggio with eight editable partials, spectral shimmer, harmonic stretch, and stereo echo |
| [Drum machine]({{ '/playground/?example=instruments/drum_machine.onda' | relative_url }}) | Synthesized kick, snare, and metallic hats |
| [FM bells]({{ '/playground/?example=instruments/fm_bells.onda' | relative_url }}) | A spacious polyphonic struck-metal pattern |
| [Formant percussion]({{ '/playground/?example=instruments/formant_percussion.onda' | relative_url }}) | Vowel-like resonant percussion |
| [Karplus–Strong]({{ '/playground/?example=instruments/karplus_strong.onda' | relative_url }}) | Warm plucked strings from short delay lines |

The polyphonic instruments play automatically and expose events for triggering individual notes.

## Effects

| Example | Description |
| --- | --- |
| [Compressor]({{ '/playground/?example=effects/compressor.onda' | relative_url }}) | Stereo-linked soft-knee compression |
| [FDN reverb]({{ '/playground/?example=effects/fdn_reverb.onda' | relative_url }}) | A dense eight-line matrix tail |
| [Live tape looper]({{ '/playground/?example=effects/live_tape_looper.onda' | relative_url }}) | Capture, varispeed, reverse, and overdub on a virtual tape |
| [Schroeder reverb]({{ '/playground/?example=effects/schroeder_reverb.onda' | relative_url }}) | A bright classic comb-and-allpass room |
| [Shimmer echo]({{ '/playground/?example=effects/shimmer_echo.onda' | relative_url }}) | Cross-fed echoes with octave-shifted regeneration |
| [Stereo chorus]({{ '/playground/?example=effects/stereo_chorus.onda' | relative_url }}) | Fractional-delay ensemble motion |
| [Tape echo]({{ '/playground/?example=effects/tape_echo.onda' | relative_url }}) | Dark saturated repeats with wow and flutter |
| [Wavefolder]({{ '/playground/?example=effects/wavefolder.onda' | relative_url }}) | Wavefolded harmonics |

Effects share the same built-in audition sequence.
Use `auto_play` to control the sequence or `bang()` to trigger a note. To process your own audio,
select an input and set `live_input` to `true`. The Schroeder and FDN examples also show how to
connect processors with a `graph` block.

## Soundscapes and experimental systems

| Example | Description |
| --- | --- |
| [Aurora Pad]({{ '/playground/?example=soundscapes/aurora_pad.onda' | relative_url }}) | A minor chord spread across a gently detuned stereo field and a slowly opening filter |
| [Benjolin]({{ '/playground/?example=feedback/benjolin.onda' | relative_url }}) | A Hordijk-inspired eight-bit rungler instrument |
| [Chaotic delay matrix]({{ '/playground/?example=feedback/chaotic_delay_matrix.onda' | relative_url }}) | Sine-feedback delays turn sparse chirps into competing rhythms through mutual inhibition and recovery |
| [Diffuse delay matrix]({{ '/playground/?example=feedback/diffuse_delay_matrix.onda' | relative_url }}) | A pitch-shifting feedback matrix turns scattered plucks into a stereo cloud that can freeze in place |
| [Granular cloud]({{ '/playground/?example=soundscapes/granular_cloud.onda' | relative_url }}) | A live tape resampled as a stereo grain cloud |
| [Orbital FM]({{ '/playground/?example=soundscapes/orbital_fm.onda' | relative_url }}) | An evolving phase-modulation drone with detuned carriers, sub-octave body, and orbiting shimmer |
| [Neural synth]({{ '/playground/?example=feedback/neural_synth.onda' | relative_url }}) | A self-oscillating neural network with resonant feedback |
| [Polyphonic saw]({{ '/playground/?example=basic/polyphonic_saw.onda' | relative_url }}) | An eight-voice randomized saw cascade |
| [Resonant delay matrix]({{ '/playground/?example=feedback/resonant_delay_matrix.onda' | relative_url }}) | Bow-like swells, coupled pitch bends, and self-oscillation in a nonlinear string network |
| [Wind chimes]({{ '/playground/?example=soundscapes/wind_chimes.onda' | relative_url }}) | Irregular modal chimes over filtered air |

## Advanced DSP and graph syntax

| Example | Description |
| --- | --- |
| [Cybernetic feedback graph]({{ '/playground/?example=feedback/cybernetic_feedback_graph.onda' | relative_url }}) | Cross-coupled graph cycles made causal with `>>[1]` delayed edges |
| [Dual FM oscillator, 8×]({{ '/playground/?example=basic/dual_fm_osc.onda' | relative_url }}) | A compact musical use of local oversampling and feedback phase modulation |
| [PaulStretch]({{ '/playground/?example=spectral/paul_stretch.onda' | relative_url }}) | Extreme time stretching of a supplied recording, with adjustable playback position |
| [Spectral delay]({{ '/playground/?example=spectral/spectral_delay.onda' | relative_url }}) | Frequency-dependent stereo echoes with adjustable spectral slope and feedback |
| [Spectral freeze]({{ '/playground/?example=spectral/spectral_freeze.onda' | relative_url }}) | Capture and sustain a spectral frame, with transposition and stereo phase drift |

## MIDI and DAW examples

These patches are shared with the [Onda VST3 plugin](https://github.com/onda-lang/onda-plugin).
Open either instrument with `onda run` to play it using the piano, computer keyboard, or a connected
MIDI device. The computer keyboard is active by default. In the browser, choose **Connect MIDI
device…** for hardware MIDI or play the on-screen keys.

| Example | Description |
| --- | --- |
| [MIDI FM bells]({{ '/playground/?example=plugins/instruments/fm_bells.onda' | relative_url }}) | Velocity-sensitive FM bells |
| [MIDI poly saw]({{ '/playground/?example=plugins/instruments/poly_saw.onda' | relative_url }}) | An eight-voice saw synth with per-channel pitch bend |
| [Orbit flanger]({{ '/playground/?example=plugins/effects/orbit_flanger.onda' | relative_url }}) | Stereo live input with quadrature modulation |
| [Reactive wavefolder]({{ '/playground/?example=plugins/effects/reactive_wavefolder.onda' | relative_url }}) | Stereo live input with envelope-driven folding |
| [Tempo ping-pong]({{ '/playground/?example=plugins/effects/tempo_ping_pong.onda' | relative_url }}) | DAW tempo in a plug-in; 120 BPM fallback elsewhere |
| [Transient sculptor]({{ '/playground/?example=plugins/effects/transient_sculptor.onda' | relative_url }}) | Stereo live input with fast/slow envelope separation |

The four effects require a selected audio input or browser microphone. A DAW supplies tempo and
transport inputs through the plug-in; those inputs are inactive in standalone playback.

## Self-contained projects

These projects include the source files and data needed to play them. Each link opens the project
folder in the repository:

| Project | Description |
| --- | --- |
| [Embedded Room](https://github.com/onda-lang/onda/tree/main/examples/projects/embedded_room) | Room reverb using an included stereo impulse response |
| [Score-driven Resonator](https://github.com/onda-lang/onda/tree/main/examples/projects/score_driven_resonator) | A modal instrument played by an included note sequence |
| [Wavetable Garden](https://github.com/onda-lang/onda/tree/main/examples/projects/wavetable_garden) | Morphing between four included wavetables |

From a repository checkout, open a project's `.ondaproject` file with Onda Run:

```bash
onda run examples/projects/embedded_room/embedded-room.ondaproject
```

In the playground, use **Open project** with a ZIP containing the complete project. See
[opening projects in the browser]({{ '/docs/projects/' | relative_url }}#in-the-browser-playground)
for packaging instructions.
