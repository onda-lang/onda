---
title: Ideas and roadmap
description: "Where Onda is headed: language design, visual tools, WebAssembly, and runtime work."
permalink: /docs/roadmap/
section: roadmap
eyebrow: What comes next
---

# Ideas and roadmap

Onda currently includes a MIR-first compiler, LLVM JIT/AOT codegen, a browser compiler and
Binaryen WebAssembly backend, real-time and offline hosts, an LSP, a C API, processors, graphs,
events, generics, and a standard library. The roadmap covers planned language, tooling, backend,
and runtime work.

This page summarizes design directions. The detailed working notes live in [`docs/todo`](https://github.com/onda-lang/onda/tree/main/docs/todo).

## Visual graphs that remain code

A visual graph editor is a major tooling direction. Its source of truth would remain ordinary Onda:

```onda
init:
  osc = Sine()
  filter = OnePole()

graph:
  freq >> osc.freq
  osc.out1 >> filter.in1
  filter.out1 >> out1
```

The editor would create processor instances in `init`, emit connections into `graph`, preserve surrounding hand-written code, and show compiler diagnostics directly on nodes and edges. The likely first home is a VS Code webview, followed by a standalone graph workflow if real projects justify it.

## Onda in the browser

The browser path now works end to end:

```text
editable in-memory Onda source
  -> onda_compiler_web + embedded stdlib
  -> validated schema-5 MIR MessagePack
  -> Binaryen.js backend
  -> DSP Wasm + host metadata
  -> AudioWorklet
```

The checked-in playground includes a source editor, structured diagnostics, generated parameter and
event controls, reset, and playback. `onda_compiler_web` also exposes a virtual multi-file project
API even though the current UI edits one source at a time. The Binaryen backend consumes schema 5,
and the reference AudioWorklet maps arbitrary Web Audio callback sizes onto the segmented
`(start_frame, frames, flags)` contract with a persistent compile-block cursor.

The remaining work is:

1. Package, compress, cache, and version the compiler Wasm, Binaryen assets, and reusable JavaScript/
   TypeScript AudioWorklet host glue.
2. Extend the playground from its current single-source editor to multi-file projects, external
   buffer loading/inspection, control-output display, microphone/input routing, export/download,
   and shareable project URLs.
3. Add seamless or crossfaded hot swap with an explicit state/parameter migration policy; the
   current editor recompiles by restarting audio and initializing fresh DSP state.
4. Add automated browser audio smoke coverage and compatibility passes for Chromium, Firefox, and
   Safari, then harden secure-context deployment, accessibility, mobile/autoplay behavior, and
   requested-versus-actual AudioContext sample-rate handling.
5. Add a first-class `.wasm` CLI export around the independent native-hosted LLVM object and linker
   path.
6. Continue cross-backend performance work. Exact WebAssembly FMA and transcendental operations now
   use self-contained software helpers with no audio-thread JavaScript boundary; the pinned math,
   Binaryen, and compiler assets still need production transfer-size and browser compile-latency
   measurements. The current reproducible development measurements live in
   [the backend benchmark report](BACKEND_BENCHMARKS.md).
7. Move compiler and Binaryen work off the page's main thread so larger programs do not stall editor
   interaction.

The long-term experience is a shareable Onda patch that compiles and produces sound without a local toolchain.

## Standard library and metadata

The standard library will continue growing around graph-friendly, consistent processor surfaces: oscillators, filters, envelopes, delay/reverb blocks, waveshaping, lookup helpers, noise, pitch, smoothing, FFT, and convolution.

Tool-facing metadata is equally important. Labels, categories, ranges, preferred controls, endpoint groups, graph nodes, and stable diagnostic anchors can let the compiler drive good interfaces rather than requiring every host to duplicate knowledge.

## Tooling and editor intelligence

Language tooling is moving toward richer hover information, completions, navigation, symbols, references, and source-aware information for specialized generic and namespace declarations. Run panels can gain clearer audio state, more expressive controls, and broader buffer support.

The daemon remains the shared boundary for analysis and live run sessions, allowing native tools and future browser clients to reuse compiler behavior.

## Runtime and export work

Runtime work includes stronger real-time safety verification, optimization of processor-array block hooks, explicit SIMD strategy, and a clearer diagnostics lifecycle in the C ABI.

Beyond native JIT and object emission, possible export targets include:

- A convenient WebAssembly module and AudioWorklet package.
- A self-contained C++ header backend for direct host integration.
- Embedded or bundled metadata options beyond the current AOT sidecar.

## Help shape the direction

Open an issue in the [Onda repository](https://github.com/onda-lang/onda/issues) to discuss and propose ideas for further development of the language.
