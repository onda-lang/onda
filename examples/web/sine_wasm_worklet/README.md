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
- it first looks for `bin/omni(.exe)` in the release/package root
- then falls back to `omni` on `PATH`
- otherwise, in a source checkout, it runs `cargo build --release -p omni_cli` and then uses `target/release/omni(.exe)`

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

Open `http://127.0.0.1:8787/` and click `Start Audio`. The page loads the same `omni-sine-processor.js` worklet and runs the Omni patch in a real `AudioContext` with live frequency and gain controls.
