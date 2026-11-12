---
title: Getting started
description: Install Onda and play your first audio patch.
permalink: /docs/getting-started/
section: learn
eyebrow: Learn Onda
---

# Getting started

## Try Onda in your browser

The quickest way to get started is the [Onda playground]({{ '/playground/' | relative_url }}). It
lets you edit, compile, and hear Onda programs directly in your browser, with no installation. Start
with the included patch or open a program from the [example cookbook]({{ '/docs/examples/' | relative_url }}).

Install Onda when you are ready to work with local files, use native audio tools, or embed it in an
application.

## Install a precompiled release

[Precompiled release packages](https://github.com/onda-lang/onda/releases/latest) are available for:

- Linux x64 and arm64
- macOS arm64
- Windows x64

Download the archive for your platform and extract it. It includes the `onda` executable,
documentation, and examples. Add its `bin` directory to your `PATH` to run `onda` from a terminal.
SHA-256 checksums are published with each release.

On Linux, run `./install.sh` from the extracted archive to install the CLI in `~/.local/bin` and
add **Onda Run** to your applications menu. Run `./uninstall.sh` from the archive to remove them.

Launching `onda` without command-line arguments opens the Onda Run file picker. The macOS package
also includes `Onda.app`, and Windows packages include a console-free `Onda.exe` launcher. Keep
the Windows launcher in the extracted package. Create a shortcut if you want to launch it from the
desktop or Start menu.

The precompiled macOS binaries, including `Onda.app`, are not signed with an Apple Developer ID and
are not notarized. If macOS blocks an app you trust, try opening it once, then go to
**System Settings → Privacy & Security → Open Anyway** and confirm **Open**. See
[Apple's instructions](https://support.apple.com/102445) for details. Separate executables or
plug-ins may require their own approval.

Windows binaries are not code-signed (Authenticode). Windows may warn about
unrecognized downloads and it may trigger Microsoft Defender
SmartScreen's **Windows protected your PC** dialog. To run `Onda.exe`, select
**More info → Run anyway**, when offered. See
[Microsoft's SmartScreen guidance](https://learn.microsoft.com/windows/apps/package-and-deploy/code-signing-options).

Check the installation:

```bash
onda --help
```

## Write your first patch

Create a file named `sine.onda`:

```onda
params:
  freq = 440.0 {20, 10000, scale = log}

init:
  phase = 0.0

block:
  incr = freq * TWO_PI / SR

  sample:
    out1 = sin(phase)
    phase = phase + incr
    if phase > TWO_PI:
      phase = phase - TWO_PI
```

The `freq` parameter controls the pitch. The program keeps track of oscillator phase between
samples and produces a sine wave.

## Check and run it

Open the standalone run window to play with the patch:

```bash
onda run sine.onda
```

Adjust `freq` in the window to change the pitch. Saving edits to the file reloads the program.

## Next steps

- [Examples]({{ '/docs/examples/' | relative_url }}) — try instruments, effects, and soundscapes.
- [CLI and editor tools]({{ '/docs/tooling/' | relative_url }}) — choose audio devices, use MIDI and samples, set up an editor, or compile WebAssembly.
- [Working with projects]({{ '/docs/projects/' | relative_url }}) — keep source files and assets together and share your programs.
- [Onda VST3 plugin](https://github.com/onda-lang/onda-plugin) — use `OndaSynth` for instruments and `OndaFX` for effects inside a DAW.
