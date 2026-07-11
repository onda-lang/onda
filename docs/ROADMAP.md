---
title: Ideas and roadmap
description: "Where Onda is headed: language design, visual tools, WebAssembly, and runtime work."
permalink: /docs/roadmap/
section: roadmap
eyebrow: What comes next
---

# Ideas and roadmap

Onda currently includes a compiler, LLVM JIT, real-time and offline hosts, an LSP, a C API, processors, graphs, events, generics, and a standard library. The roadmap covers planned language, tooling, backend, and runtime work.

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

The WebAssembly roadmap builds from capabilities that already exist: the LLVM backend can emit WebAssembly objects, and the repository includes an AudioWorklet example.

Planned product layers include:

1. A first-class `.wasm` export command around the existing object and linker path.
2. Reusable JavaScript/TypeScript AudioWorklet host glue.
3. A browser playground with editing, diagnostics, playback, and generated parameter controls.
4. Potential in-browser compilation through a smaller Binaryen-based backend.

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
- Better metadata packaging for ahead-of-time artifacts.

## Help shape the direction

Open an issue in the [Onda repository](https://github.com/onda-lang/onda/issues) to discuss and propose ideas for further development of the language.
