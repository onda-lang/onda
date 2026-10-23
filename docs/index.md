---
title: Documentation
description: Getting started, language and standard library references, tools, and embedding APIs.
permalink: /docs/
section: learn
eyebrow: Onda docs
---

# Documentation

## Start here

- [Getting started]({{ '/docs/getting-started/' | relative_url }}) — Install Onda, run a program, and render audio to a WAV file.
- [Playground]({{ '/playground/' | relative_url }}) — Edit and run Onda programs in your browser.
- [Examples]({{ '/docs/examples/' | relative_url }}) — Instruments, effects, and other programs you can open in the playground.
- [Projects]({{ '/docs/projects/' | relative_url }}) — Editable source projects, buffer assets, and portable project images.

## Language

- [Language guide]({{ '/docs/language/' | relative_url }}) — Syntax and semantics.
- [Standard library]({{ '/docs/stdlib/' | relative_url }}) — Built-in modules, functions, structs, and processors.

## Tooling

- [CLI and editors]({{ '/docs/tooling/' | relative_url }}) — Command-line usage, VS Code, and Neovim.
- [Releases](https://github.com/onda-lang/onda/releases/latest) — Precompiled packages for Linux, macOS, and Windows.
- [VST3 plugins](https://github.com/onda-lang/onda-plugin) — OndaSynth and OndaFX for running Onda in a DAW.

## Embedding

- [C API]({{ '/docs/api/' | relative_url }}) — Compile and run Onda programs with `libonda`.
- [Processor API and ABI]({{ '/docs/processor-api/' | relative_url }}) — Host compiled native and WebAssembly processors without `libonda`.
- [Web API]({{ '/docs/web-api/' | relative_url }}) — JavaScript packages for compilation, artifacts, and Web Audio playback.
- [Host output]({{ '/docs/host-output/' | relative_url }}) — Collect and present delegates and prints across host APIs.

## Compiler contributors

- [Architecture]({{ '/docs/architecture/' | relative_url }}) — Compiler pipeline, runtime, optimization ownership, and codebase map.
- [MIR reference]({{ '/docs/mir/' | relative_url }}) — Executable representation, validation, shared analyses, and backend contract.
- [Benchmark guide](https://github.com/onda-lang/onda/blob/main/scripts/bench/README.md) — Reproduce performance measurements and inspect dated evidence in the repository.
