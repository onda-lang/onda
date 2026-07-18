---
title: Getting started
description: Install Onda, run your first audio patch, and render it to a WAV file.
permalink: /docs/getting-started/
section: learn
eyebrow: Learn Onda
---

# Getting started

This guide takes you from installing Onda to a running sine oscillator. At the end, you will know how an Onda program is shaped, how to hear it in real time, and how to render it to a file.

## Install a precompiled release

[Precompiled release packages](https://github.com/onda-lang/onda/releases/latest) are available for:

- Linux x64 (`.tar.xz`)
- macOS arm64 (`.tar.xz`)
- Windows x64 (`.zip`)

Each package contains the `onda` executable, static and shared C libraries, `onda.h`, the language guide, and the examples. Download the archive for your platform, extract it, and add its `bin` directory to your `PATH`. SHA-256 checksums are published with each release.

Releases also contain portable npm tarballs for the WebAssembly compiler and Web Audio packages.

Check the installation:

```bash
onda --help
```

## Install the WebAssembly compiler

Browser and Node.js applications can install the source-to-WebAssembly compiler from its release
tarball or npm package:

```bash
npm install ./onda-lang-wasm-compiler-X.Y.Z.tgz
# After registry publication:
npm install @onda-lang/wasm-compiler
npx onda-wasm compile sine.onda --output sine.wasm
```

The command writes `sine.wasm` and its integrity-associated `sine.onda.json` processor descriptor.
It is a build-time compiler and does not require LLVM, Rust, or a Wasm linker after installation.
Applications compiling source in the browser can use the package's asynchronous JavaScript API and
worker mode. `@onda-lang/webaudio` remains an optional playback host rather than part of compilation.

## Write your first patch

Create a file named `sine.onda`:

```onda
params:
  freq = 440.0 {20.0, 20000.0}

init:
  phase = 0.0

block:
  incr = freq * TWO_PI / SR

  sample:
    phase = phase + incr
    if phase > TWO_PI:
      phase = phase - TWO_PI
    out1 = sin(phase)
```

This small program exposes one host parameter, stores oscillator phase between samples, calculates the phase increment once per block, and produces one output per sample.

## Check and run it

Compile the file to check its syntax and semantics:

```bash
onda compile sine.onda
```

Open the standalone run window:

```bash
onda run sine.onda
```

The default host uses the native egui interface. To select the webview host explicitly:

```bash
onda run sine.onda --webview
```

You can also run without the UI and set parameters from the command line:

```bash
onda run play sine.onda --dur 3 --set freq=220
```

## Render a WAV

Offline rendering uses the same run pipeline without opening an audio device:

```bash
onda run render sine.onda --output first.wav --dur 5 --set freq=330
```

This is useful for repeatable tests, inspecting generated audio, and workflows where real-time playback is not needed.

## Understand the execution model

An Onda patch is organized by when work happens:

| Section | Runs | Typical use |
| --- | --- | --- |
| `params` | Controlled by the host | Frequency, gain, mix, mode |
| `init` | At instance creation or reset | Persistent state and proc construction |
| `block` | Once per host audio block | Control-rate calculations |
| `sample` | Once per output sample | Oscillators, filters, mixing |
| `event` | When triggered by the host | Notes, gates, resets, one-shot changes |
| `graph` | Lowered into scheduled signal flow | Declarative processor routing |

That visible rate model is the central idea of the language. Read [the complete language guide]({{ '/docs/language/' | relative_url }}) next, or learn from the [example cookbook]({{ '/docs/examples/' | relative_url }}).

## Where to go next

1. Change the oscillator into a simple input gain: `out1 = in1 * gain`.
2. Open `examples/standard-library/std_osc_shapes.onda` to see standard-library oscillators.
3. Learn how [`proc` creates reusable DSP units]({{ '/docs/language/#10-processors-with-proc' | relative_url }}).
4. Try [declarative graphs]({{ '/docs/language/#11-graphs' | relative_url }}).
5. Set up [VS Code or Neovim]({{ '/docs/tooling/#editor-support' | relative_url }}) for diagnostics and language-aware editing.
