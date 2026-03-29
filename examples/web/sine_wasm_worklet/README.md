# Sine Wasm AudioWorklet Demo

Build the wasm artifact directly into this folder.

Windows PowerShell:

```powershell
.\examples\web\sine_wasm_worklet\build-demo.ps1
```

macOS/Linux:

```bash
bash ./examples/web/sine_wasm_worklet/build-demo.sh
```

The script is just a thin wrapper around the Omni CLI:
- it prefers `target/release/omni(.exe)`
- then falls back to `target/debug/omni(.exe)`
- otherwise it runs `cargo run -p omni_cli -- ...`

Build and serve in one command:

```powershell
.\examples\web\sine_wasm_worklet\build-demo.ps1 -Serve
```

```bash
bash ./examples/web/sine_wasm_worklet/build-demo.sh --serve
```

Optional knobs:

```powershell
.\examples\web\sine_wasm_worklet\build-demo.ps1 -SampleRate 48000 -BlockSize 128
```

```bash
bash ./examples/web/sine_wasm_worklet/build-demo.sh --sample-rate 48000 --block-size 128
```

If script execution is blocked, use:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\examples\web\sine_wasm_worklet\build-demo.ps1
```

The page uses `OfflineAudioContext` plus `AudioWorkletNode` and reports a JSON summary with `ok`, `maxAbs`, `meanAbs`, and `estimatedHz`.

For automation, the page also POSTs its final render summary to `./__result`, and the local server exposes that value at `GET /__result`.

For audible playback, open `http://127.0.0.1:8787/live.html` and click `Start Audio`. That page uses the same `omni-sine-processor.js` worklet, but drives it from a real `AudioContext` with live frequency and gain controls.
