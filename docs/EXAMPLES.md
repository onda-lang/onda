---
title: Example cookbook
description: A guided tour through Onda's checked-in audio programming examples.
permalink: /docs/examples/
section: examples
eyebrow: Learn by reading
---

# Example cookbook

The repository contains complete, compilable patches ranging from a sine wave to convolution, polyphony, feedback matrices, and WebAssembly hosting. Start small and follow the path that matches what you want to build.

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
| [`sine.onda`](https://github.com/onda-lang/onda/blob/main/examples/foundations/sine.onda) | Params, persistent phase, block and sample rates |
| [`sines.onda`](https://github.com/onda-lang/onda/blob/main/examples/foundations/sines.onda) | Combining multiple oscillators |
| [`saw_blep.onda`](https://github.com/onda-lang/onda/blob/main/examples/foundations/saw_blep.onda) | Band-limited waveform generation |
| [`dual_osc_oversampled_8x.onda`](https://github.com/onda-lang/onda/blob/main/examples/foundations/dual_osc_oversampled_8x.onda) | Oversampled sample code |
| [`simple_events.onda`](https://github.com/onda-lang/onda/blob/main/examples/foundations/simple_events.onda) | Host-triggered events and persistent state |

Read these after the [first-patch guide]({{ '/docs/getting-started/' | relative_url }}). They introduce the core execution model without a large abstraction layer.

## Processors and graphs

Onda processors package state and behavior into reusable DSP units. Graphs connect those processors declaratively and let the compiler check shapes, rates, and cycles.

[`stdlib_custom_chain.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/stdlib_custom_chain.onda) combines standard-library oscillator and resonant-filter processors with a custom oversampled drive processor and helper function. It is also the example shown on the project homepage.

| Imperative version | Graph version | Focus |
| --- | --- | --- |
| [`proc_gain.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/proc_gain.onda) | [`proc_gain_graph.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/proc_gain_graph.onda) | A minimal reusable processor |
| [`proc_split.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/proc_split.onda) | [`proc_split_graph.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/proc_split_graph.onda) | Multiple outputs and routing |
| [`proc_array_stereo_sine.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/proc_array_stereo_sine.onda) | [`proc_array_stereo_sine_graph.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/proc_array_stereo_sine_graph.onda) | Proc arrays and stereo composition |
| [`reverb_sample.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/reverb_sample.onda) | [`reverb_graph.onda`](https://github.com/onda-lang/onda/blob/main/examples/processors-and-graphs/reverb_graph.onda) | A larger effect in both styles |

Use `--dump-graph` to inspect the compiler's resolved graph:

```bash
onda compile examples/processors-and-graphs/proc_gain_graph.onda --dump-graph
```

## Standard library

The `std` modules provide oscillators, filters, envelopes, delay, FFT, convolution, mixing, noise, pitch, smoothing, math, data helpers, and more.

- [`std_osc_shapes.onda`](https://github.com/onda-lang/onda/blob/main/examples/standard-library/std_osc_shapes.onda) — oscillator shapes.
- [`std_filter_modes.onda`](https://github.com/onda-lang/onda/blob/main/examples/standard-library/std_filter_modes.onda) — filter modes.
- [`std_env_adsr.onda`](https://github.com/onda-lang/onda/blob/main/examples/standard-library/std_env_adsr.onda) — an ADSR envelope.
- [`std_noise.onda`](https://github.com/onda-lang/onda/blob/main/examples/standard-library/std_noise.onda) — noise generators.
- [`std_smoothing.onda`](https://github.com/onda-lang/onda/blob/main/examples/standard-library/std_smoothing.onda) — parameter smoothing.
- [`std_mix_gain_pitch.onda`](https://github.com/onda-lang/onda/blob/main/examples/standard-library/std_mix_gain_pitch.onda) — utilities composed together.

## Buffers, FFT, and convolution

These examples are more host- and data-oriented:

- [`buffer_looper_read.onda`](https://github.com/onda-lang/onda/blob/main/examples/buffers-fft-convolution/buffer_looper_read.onda) reads a host-bound buffer.
- [`fft_bin_shift.onda`](https://github.com/onda-lang/onda/blob/main/examples/buffers-fft-convolution/fft_bin_shift.onda) transforms frequency bins.
- [`convolution_impulse.onda`](https://github.com/onda-lang/onda/blob/main/examples/buffers-fft-convolution/convolution_impulse.onda) performs convolution with generated impulse data.
- [`convolution_wav_impulse.onda`](https://github.com/onda-lang/onda/blob/main/examples/buffers-fft-convolution/convolution_wav_impulse.onda) uses a WAV impulse response.

## Larger patches

Once the smaller examples feel familiar, explore:

- [`polyphonic_saw.onda`](https://github.com/onda-lang/onda/blob/main/examples/larger-patches/polyphonic_saw.onda) for event-driven polyphony.
- [`neural_synth.onda`](https://github.com/onda-lang/onda/blob/main/examples/larger-patches/neural_synth.onda) for a more unusual synthesis design.
- [`schroeder_reverb_impulse.onda`](https://github.com/onda-lang/onda/blob/main/examples/larger-patches/schroeder_reverb_impulse.onda) for a classic reverberator structure.
- [`matrix_feedback_blipblop.onda`](https://github.com/onda-lang/onda/blob/main/examples/larger-patches/matrix_feedback_blipblop.onda) and its `lush` and `chaos` variants for complex feedback systems.
- [`cybernetic_feedback_graph.onda`](https://github.com/onda-lang/onda/blob/main/examples/larger-patches/cybernetic_feedback_graph.onda) for a graph-heavy generative patch.

## WebAssembly host example

[`examples/web/onda_wasm_playground`](https://github.com/onda-lang/onda/tree/main/examples/web/onda_wasm_playground) is the end-to-end browser playground. Its editor sends source to [`onda_compiler_web`](https://github.com/onda-lang/onda/tree/main/crates/onda_compiler_web), which resolves embedded `std/...` modules and emits validated schema-5 MIR in the page. [`onda_binaryen_web`](https://github.com/onda-lang/onda/tree/main/packages/onda_binaryen_web) lowers that MIR to DSP Wasm, and the AudioWorklet host supplies metadata-driven parameters, events, reset, external buffers, and segmented processing.

Build and serve it with `bash ./examples/web/onda_wasm_playground/build-demo.sh --serve` or
`.\examples\web\onda_wasm_playground\build-demo.ps1 -Serve`. This requires Node/npm and `wasm-pack`,
but not the native Onda CLI, LLVM, or a compiler service. The current playground exposes a
single-source editor even though the compiler API also supports virtual multi-file projects; a
first-class `--emit wasm` CLI workflow remains separate roadmap work. See the
[backend benchmark report](BACKEND_BENCHMARKS.md) for the reproducible development comparison
between native LLVM and Binaryen/Wasm.
