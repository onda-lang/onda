---
title: Getting started
description: Install Onda, run your first audio patch, and render it to a WAV file.
permalink: /docs/getting-started/
section: learn
eyebrow: Learn Onda
---

# Getting started

This guide takes you from trying Onda to a running sine oscillator. At the end, you will know how an Onda program is shaped, how to hear it in real time, and how to render it to a file.

## Try Onda in your browser

The quickest way to get started is the [Onda playground]({{ '/playground/' | relative_url }}). It
lets you edit, compile, and hear Onda programs directly in your browser, with no installation. Start
with the included patch or open a program from the [example cookbook]({{ '/docs/examples/' | relative_url }}).

Install Onda when you are ready to work with local files, use native audio tools, or embed it in an
application.

## Install a precompiled release

[Precompiled release packages](https://github.com/onda-lang/onda/releases/latest) are available for:

- Linux x64 (`.tar.xz`)
- macOS arm64 (`.tar.xz`)
- Windows x64 (`.zip`)

Each package contains the `onda` executable, static and shared C libraries, `onda.h`, the language
guide, and the examples. Download the archive for your platform and extract it. For portable use,
add its `bin` directory to your `PATH`. SHA-256 checksums are published with each release.

On Linux, run `./install.sh` from the extracted archive to copy `onda` to `~/.local/bin` and install
the **Onda Run** desktop entry and icon for the current user. The generated desktop entry points to
that stable executable location instead of the extracted archive. Run `./uninstall.sh` from the
archive to remove those three installed files.

Launching `onda` without command-line arguments opens the Onda Run file picker. The macOS package
also includes `Onda.app`, and Windows packages include a console-free `Onda Run.exe` launcher. Keep
the Windows launcher in the extracted package and create a shortcut when moving it to the desktop
or Start menu, because it launches the bundled `bin\onda.exe`.

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
npm install @onda-lang/wasm-compiler
npx onda-wasm compile sine.onda --output sine.wasm
```

The command writes `sine.wasm` and its integrity-associated `sine.onda.json` processor descriptor.
It is a build-time compiler and does not require LLVM, Rust, or a Wasm linker after installation.
Applications compiling source in the browser can use the package's asynchronous JavaScript API and
worker mode. `@onda-lang/webaudio` remains an optional playback host.

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

Programs with declared buffers can bind WAV files by name:

```bash
onda run play buffer_looper.onda --buffer src=sample.wav
```

## Render a WAV file

Offline rendering uses the same run pipeline without opening an audio device:

```bash
onda run render sine.onda --output first.wav --dur 5 --set freq=330
```

The same `--buffer name=path` option is available for offline rendering.
