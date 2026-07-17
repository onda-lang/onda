# Onda AOT Wasm sample player

This example consumes a processor that was compiled before the page loads:

```text
sample_player.onda
  -> native compiler frontend: optimized schema-5 MIR MessagePack
  -> Binaryen O4 at build time: complete DSP Wasm + descriptor
  -> static browser page: AudioWorklet only
```

The browser does not load `onda_compiler_web`, Binaryen, MIR, or an Onda source file. It fetches the
finished `sample-player.wasm` and integrity-checked `sample-player.onda.json`, decodes
[`impulse.wav`](../../buffers-fft-convolution/impulse.wav), binds it to the processor's `clip`
buffer, and supplies the processor's amplitude input with a Web Audio `ConstantSourceNode`.

The processor is the exact shared
[`sample_player.onda`](../../buffers-fft-convolution/sample_player.onda) used by the native raw-object
example. `play(bool)` starts or stops the clip, the `speed` parameter changes playback rate, and the
single audio input controls output amplitude.

## Build and run

macOS/Linux:

```bash
bash ./examples/web/onda_wasm_aot_sample_player/build-demo.sh --serve
```

Windows PowerShell:

```powershell
.\examples\web\onda_wasm_aot_sample_player\build-demo.ps1 -Serve
```

Open `http://127.0.0.1:8788/`. **Start audio** instantiates the already-built module, loads the WAV,
and sends `play(true)`. The other controls replay the clip, send `play(false)`, update `speed`, and
change the audio-rate amplitude input.

Without `--serve` or `-Serve`, the scripts only build the static assets. They require Rust/Cargo
and Node/npm, but not LLVM, the native audio/GUI stack, `wasm-pack`, or a Wasm linker. Binaryen is a
build-time dependency only.

The scripts deliberately show the two AOT stages separately:

1. The compiler-only `onda_compiler_web` native helper produces optimized, validated MIR
   MessagePack without pulling in the native runtime or LLVM backend.
2. `build-artifact.mjs` lowers MIR with Binaryen O4 and writes the executable `.wasm` plus its
   integrity-associated `.onda.json` descriptor.

For the source-editing, in-browser compilation path, use the separate
[`onda_wasm_playground`](../onda_wasm_playground/README.md) embedded-compiler example.
