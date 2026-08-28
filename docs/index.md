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

Try Onda immediately in the [browser playground]({{ '/playground/' | relative_url }}): edit a patch,
compile it, and hear the result without installing anything.

Continue with [Getting started]({{ '/docs/getting-started/' | relative_url }}) to install the CLI, run a local patch, and render audio. Then use the [Example cookbook]({{ '/docs/examples/' | relative_url }}) to explore complete programs by topic or open them directly in the playground.

The [Language guide]({{ '/docs/language/' | relative_url }}) is the complete syntax and semantics reference. It covers the program surface, execution rates, types, functions, structs, processors, graphs, generics, events, delegates, printing, and modules.

The [Standard library reference]({{ '/docs/stdlib/' | relative_url }}) is generated directly from
the modules embedded in the compiler and lists their functions, structs, processors, parameters,
ports, and events.

## Use the toolchain

[Precompiled releases](https://github.com/onda-lang/onda/releases/latest) are available for Linux x64, macOS arm64, and Windows x64. The [CLI and editor guide]({{ '/docs/tooling/' | relative_url }}) explains compilation, real-time playback, offline rendering, diagnostics, VS Code, Neovim, and the C embedding API.

The [C API reference]({{ '/docs/api/' | relative_url }}) documents the complete `libonda` surface,
including compilation, metadata, instances, processing, events, delegates, printing, snapshots,
ownership, and release-SDK linking.

The [processor API and ABI reference]({{ '/docs/processor-api/' | relative_url }}) covers direct
hosting of native and WebAssembly processor objects without `libonda`. It documents storage,
entry points, parameters, events, delegates, printing, snapshots, target profiles, and every helper
in the shipped `onda_processor_abi.h` header.

The [Web API reference]({{ '/docs/web-api/' | relative_url }}) covers the complete public surface of
the compiler, artifact, Binaryen backend, and Web Audio npm packages. The same reference is included
as `api.md` in every published package.
