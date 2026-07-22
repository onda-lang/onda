---
title: Example cookbook
description: A guided tour through Onda's checked-in audio programming examples.
permalink: /docs/examples/
section: examples
eyebrow: Learn by reading
---

# Example cookbook

The repository contains complete, compilable patches ranging from a sine wave to convolution, polyphony, feedback matrices, and WebAssembly hosting. Select an Onda example below to open it directly in the browser playground, then edit and run it there. Start small and follow the path that matches what you want to build.

All paths below are relative to the repository root. Run audio examples with:

```bash
onda run examples/foundations/sine.onda
```

Or render one without opening an audio device:

```bash
onda run render examples/foundations/sine.onda --output sine.wav --dur 3
```

## Foundations

| Example | What it teaches |
| --- | --- |
| [`sine.onda`]({{ '/playground/?example=foundations/sine.onda' | relative_url }}) | Params, persistent phase, block and sample rates |
| [`sines.onda`]({{ '/playground/?example=foundations/sines.onda' | relative_url }}) | Combining multiple oscillators |
| [`saw_blep.onda`]({{ '/playground/?example=foundations/saw_blep.onda' | relative_url }}) | Band-limited waveform generation |
| [`dual_osc_oversampled_8x.onda`]({{ '/playground/?example=foundations/dual_osc_oversampled_8x.onda' | relative_url }}) | Oversampled sample code |
| [`simple_events.onda`]({{ '/playground/?example=foundations/simple_events.onda' | relative_url }}) | Host-triggered events and persistent state |

Read these after the [first-patch guide]({{ '/docs/getting-started/' | relative_url }}). They introduce the core execution model without a large abstraction layer.

## Processors and graphs

Onda processors package state and behavior into reusable DSP units. Graphs connect those processors declaratively and let the compiler check shapes, rates, and cycles.

[`saw_filter_saturator.onda`]({{ '/playground/?example=processors-and-graphs/saw_filter_saturator.onda' | relative_url }}) combines standard-library oscillator and resonant-filter processors with a custom oversampled drive processor and helper function. It is also the example shown on the project homepage.

| Imperative version | Graph version | Focus |
| --- | --- | --- |
| [`proc_gain.onda`]({{ '/playground/?example=processors-and-graphs/proc_gain.onda' | relative_url }}) | [`proc_gain_graph.onda`]({{ '/playground/?example=processors-and-graphs/proc_gain_graph.onda' | relative_url }}) | A minimal reusable processor |
| [`proc_split.onda`]({{ '/playground/?example=processors-and-graphs/proc_split.onda' | relative_url }}) | [`proc_split_graph.onda`]({{ '/playground/?example=processors-and-graphs/proc_split_graph.onda' | relative_url }}) | Multiple outputs and routing |
| [`proc_array_stereo_sine.onda`]({{ '/playground/?example=processors-and-graphs/proc_array_stereo_sine.onda' | relative_url }}) | [`proc_array_stereo_sine_graph.onda`]({{ '/playground/?example=processors-and-graphs/proc_array_stereo_sine_graph.onda' | relative_url }}) | Proc arrays and stereo composition |
| [`reverb_sample.onda`]({{ '/playground/?example=processors-and-graphs/reverb_sample.onda' | relative_url }}) | [`reverb_graph.onda`]({{ '/playground/?example=processors-and-graphs/reverb_graph.onda' | relative_url }}) | A larger effect in both styles |

Use `--dump-graph` to inspect the compiler's resolved graph:

```bash
onda compile examples/processors-and-graphs/proc_gain_graph.onda --dump-graph
```

## Standard library

The `std` modules provide oscillators, filters, envelopes, delay, FFT, convolution, mixing, noise, pitch, smoothing, math, data helpers, and more.

- [`std_osc_shapes.onda`]({{ '/playground/?example=standard-library/std_osc_shapes.onda' | relative_url }}) — oscillator shapes.
- [`std_filter_modes.onda`]({{ '/playground/?example=standard-library/std_filter_modes.onda' | relative_url }}) — filter modes.
- [`std_env_adsr.onda`]({{ '/playground/?example=standard-library/std_env_adsr.onda' | relative_url }}) — an ADSR envelope.
- [`std_noise.onda`]({{ '/playground/?example=standard-library/std_noise.onda' | relative_url }}) — noise generators.
- [`std_smoothing.onda`]({{ '/playground/?example=standard-library/std_smoothing.onda' | relative_url }}) — parameter smoothing.
- [`std_mix_gain_pitch.onda`]({{ '/playground/?example=standard-library/std_mix_gain_pitch.onda' | relative_url }}) — utilities composed together.

## Buffers, FFT, and convolution

These examples are more host- and data-oriented:

- [`buffer_looper_read.onda`]({{ '/playground/?example=buffers-fft-convolution/buffer_looper_read.onda' | relative_url }}) reads a host-bound buffer.
- [`sample_player.onda`]({{ '/playground/?example=buffers-fft-convolution/sample_player.onda' | relative_url }}) plays a host-bound clip under event, speed, and audio-rate amplitude control.
- [`fft_bin_shift.onda`]({{ '/playground/?example=buffers-fft-convolution/fft_bin_shift.onda' | relative_url }}) transforms frequency bins.
- [`convolution_impulse.onda`]({{ '/playground/?example=buffers-fft-convolution/convolution_impulse.onda' | relative_url }}) performs convolution with generated impulse data.
- [`convolution_wav_impulse.onda`]({{ '/playground/?example=buffers-fft-convolution/convolution_wav_impulse.onda' | relative_url }}) uses a WAV impulse response.

## Larger patches

Once the smaller examples feel familiar, explore:

- [`polyphonic_saw.onda`]({{ '/playground/?example=larger-patches/polyphonic_saw.onda' | relative_url }}) for event-driven polyphony.
- [`neural_synth.onda`]({{ '/playground/?example=larger-patches/neural_synth.onda' | relative_url }}) for a more unusual synthesis design.
- [`schroeder_reverb_impulse.onda`]({{ '/playground/?example=larger-patches/schroeder_reverb_impulse.onda' | relative_url }}) for a classic reverberator structure.
- [`matrix_feedback_blipblop.onda`]({{ '/playground/?example=larger-patches/matrix_feedback_blipblop.onda' | relative_url }}) and its `lush` and `chaos` variants for complex feedback systems.
- [`cybernetic_feedback_graph.onda`]({{ '/playground/?example=larger-patches/cybernetic_feedback_graph.onda' | relative_url }}) for a graph-heavy generative patch.
