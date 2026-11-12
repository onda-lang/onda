---
title: CLI and editor tools
description: Play and edit Onda programs with the CLI, MIDI, samples, and editor tools.
permalink: /docs/tooling/
section: tooling
eyebrow: Tools
---

# CLI and editor tools

Use `onda` to check and play programs. If you have not installed it yet,
follow [Getting started]({{ '/docs/getting-started/' | relative_url }}#install-a-precompiled-release).
Run `onda --help` for all commands and options.

The commands below use files from the included examples. Replace the example path with your own
`.onda`, `.on`, or `.ondaproject` file when working on a program.

## Real-time playback

Open a program in Onda Run:

```bash
onda run examples/basic/sine.onda
```

Onda reloads the program when you save changes to its source files. Projects also reload when you
change their project file or buffer assets.

Run `onda` without a filename to open the file picker. Use `--sample-rate`, `--block-size`,
`--input-device`, `--output-device`, and `--theme` to choose playback settings.

### Play with MIDI

Programs with a supported `note_on` event show a piano and MIDI input selector. Choose **Computer
Keyboard** to play with `A W S E D F T G Y H U J K O L P`, or play the piano with a pointer or touch.
Select a connected MIDI device to use a hardware keyboard.

The [browser playground]({{ '/playground/' | relative_url }}) also has a piano and computer-keyboard
input. In browsers that support Web MIDI, choose **Connect MIDI device…** to request access to
hardware.

For a ready-to-play instrument, open a [MIDI example]({{ '/docs/examples/' | relative_url }}#midi-and-daw-examples).
To play Onda inside a DAW, use the [Onda VST3 plugin](https://github.com/onda-lang/onda-plugin):
`OndaSynth` hosts instruments and `OndaFX` hosts effects. A DAW supplies tempo and transport inputs
through the plug-in; those inputs are inactive in standalone playback.

### Play without a window

Use `onda run play` for command-line playback. Set a duration or keep playing until you stop it:

```bash
onda run play examples/basic/sine.onda --dur 2
onda run play examples/basic/sine.onda --forever --set freq=220
```

Use `--set name=value` to choose parameter values. To connect a MIDI input, pass its exact device
name:

```bash
onda run play examples/plugins/instruments/poly_saw.onda \
  --forever --midi-input-device "Your MIDI Device"
```

### Supply samples and other buffers

For your own program with a declared `clip` buffer, bind a WAV or `.ondabuffer` file by name:

```bash
onda run play my-sampler.onda --buffer clip=sample.wav
```

For a buffer array, select a slot with a quoted name such as `--buffer 'bank[3]=snare.ondabuffer'`.
Unbound buffers read as zero and discard writes, so missing bindings do not prevent playback.

To save sources and assets together, see [Working with projects]({{ '/docs/projects/' | relative_url }}).

### WAV export

If you need an audio file, export a fixed duration from the command line:

```bash
onda run render examples/basic/sine.onda --output sine.wav --dur 5
```

`--dur` is the duration in seconds. Parameter and buffer overrides work as they do for command-line
playback.

## Compile and inspect

Check a program for syntax and semantic errors:

```bash
onda compile examples/basic/sine.onda
```

To inspect a graph or generated LLVM IR:

```bash
onda compile examples/feedback/cybernetic_feedback_graph.onda --dump-graph
onda compile examples/basic/sine.onda --emit llvm-ir
```

To compile a native object for use in another application:

```bash
onda compile examples/basic/sine.onda --emit obj
```

Use `--target-spec` with a target preset when compiling for another platform:

```bash
onda compile examples/basic/sine.onda --target-spec ./targets/arm64.toml --emit obj
```

The [Processor API]({{ '/docs/processor-api/' | relative_url }}) explains how to link and host the
resulting object.

## Editor support

### VS Code

Install the [Onda VS Code extension](https://github.com/onda-lang/onda-vscode) for `.onda` and `.on`
files, diagnostics, completion, hover, signature help, and go-to-definition. The extension bundles
the native compiler; you can select an external compiler in its settings. Run **Onda: Run** to open
the program inside the editor.

### Neovim

The [Onda Neovim plugin](https://github.com/onda-lang/onda-nvim) sets up filetype detection and the
language server. Use `:OndaRunFile` to open the current program in Onda Run.

### Other editors and tools

Configure a language-server client to start `onda lsp` over standard input and output. It provides
diagnostics, completion, hover, signature help, navigation, document symbols, and semantic
highlighting.

To run analysis once from the command line:

```bash
onda daemon diagnose examples/basic/sine.onda
```

Tool integrations can use `onda daemon stdio` for the daemon's JSON control interface.

## Compile WebAssembly

Install the WebAssembly compiler with Node.js 20 or newer:

```bash
npm install @onda-lang/wasm-compiler
```

If you downloaded a package tarball from a release, install that file instead, replacing `X.Y.Z`
with the downloaded version:

```bash
npm install ./onda-lang-wasm-compiler-X.Y.Z.tgz
```

Then compile a source file:

```bash
npx onda-wasm compile examples/basic/sine.onda \
  --root . \
  --output ./sine.wasm
```

This writes `sine.wasm` and its matching `sine.onda.json` descriptor. Compilation does not require
LLVM, Rust, or a Wasm linker after the package is installed. See the
[Web API]({{ '/docs/web-api/' | relative_url }}) to compile inside an application or play an artifact
through Web Audio.

## Embed Onda in an application

- [C API]({{ '/docs/api/' | relative_url }}) — compile source and run it through `libonda`.
- [Processor API]({{ '/docs/processor-api/' | relative_url }}) — link and call a compiled processor directly.
- [Web API]({{ '/docs/web-api/' | relative_url }}) — compile and play programs with JavaScript and WebAssembly.
