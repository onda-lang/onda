---
title: Documentation
description: Learn Onda, explore the language reference, and understand the compiler toolchain.
permalink: /docs/
section: learn
eyebrow: Onda docs
---

# Documentation

Onda is a language and toolchain for real-time audio programming. These docs cover the path from a first oscillator to reusable processors, declarative signal graphs, host integration, and compiler internals.

## Learn the language

Start with [Getting started]({{ '/docs/getting-started/' | relative_url }}) to build the CLI, run a patch, and render audio. Then use the [Example cookbook]({{ '/docs/examples/' | relative_url }}) to explore complete programs by topic.

The [Language guide]({{ '/docs/language/' | relative_url }}) is the complete syntax and semantics reference. It covers the program surface, execution rates, types, functions, structs, processors, graphs, generics, events, and modules.

## Use the toolchain

[Precompiled releases](https://github.com/onda-lang/onda/releases/latest) are available for Linux x64, macOS arm64, and Windows x64. The [CLI and editor guide]({{ '/docs/tooling/' | relative_url }}) explains compilation, real-time playback, offline rendering, diagnostics, VS Code, Neovim, and the C embedding API.

## Understand the project

The [Compiler architecture]({{ '/docs/architecture/' | relative_url }}) maps each workspace crate and the main implementation paths through parsing, semantic analysis, LLVM lowering, runtime processing, the daemon, and language tooling.

The [Ideas and roadmap]({{ '/docs/roadmap/' | relative_url }}) summarizes active directions including musical scheduling, visual graph editing, WebAssembly, browser tooling, standard-library growth, and runtime verification.
