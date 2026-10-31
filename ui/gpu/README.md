# Shared GPU UI development

`onda_ui` supplies the run screen used by standalone Onda, VS Code, the VST3
plugin, and both browser editors. This directory contains its browser adapter,
fixtures, and generated Wasm bundle.

- [UI host API](../../docs/ui-api.md): embedding, state, commands, input, and lifecycle.
- [Architecture](../../docs/architecture.md#shared-gpu-run-screen-cratesonda_ui-cratesonda_gpu-cratesonda_scope-uigpu): module ownership and rendering/capture invariants.
- [CLI and editor tools](../../docs/tooling.md#real-time-playback): end-user controls.
- [Browser playground](../playground/README.md): browser compiler/audio integration.

## Native development

From the repository root, with the native build prerequisites in the
[root README](../../README.md#building-onda-from-source):

```sh
cargo run --release -p onda_cli --bin onda -- run examples/basic/sine.onda
cargo test -p onda_ui -p onda_gpu -p onda_scope --lib --features onda_ui/ffi
```

For embedding without a compiler or audio engine, run the
[host-owned device example](../../crates/onda_ui/examples/host.rs):

```sh
cargo run -p onda_ui --example host
```

The standalone adapter supports native Wayland and X11 on Linux. The C adapter
supports Xlib, Win32, and AppKit; see the host API for handle lifetimes and thread
requirements. Build its shared library with:

```sh
cargo build --release -p onda_ui --features ffi
```

Plugin build and native/DAW checks belong to the
[plugin README](https://github.com/onda-lang/onda-plugin#build) and
[test guide](https://github.com/onda-lang/onda-plugin/blob/main/tests/README.md).
The [VS Code README](https://github.com/onda-lang/onda-vscode#development) owns
extension staging and packaging instructions.

## Browser build and staging

Requires Rust, `wasm-pack`, the `wasm32-unknown-unknown` target, and Node.js/npm.
Run from the repository root:

```sh
npm ci
npm run build:gpu-ui
```

The build writes `ui/gpu/pkg`. Its `build.json` records the Onda version and
checksums of the Wasm, JavaScript adapter, fonts, and notices. Rebuild after
changing any runtime asset or the product version. Staging validates the complete
bundle before copying it; generated assets are ignored by Git.

To stage a custom browser host:

```sh
node scripts/stage-gpu-ui.mjs /path/to/host/gpu
```

Serve the host over localhost or HTTPS. WebGPU is selected when a non-fallback
adapter is available; otherwise the adapter uses WebGL2. `?backend=webgl` forces
WebGL2. Embedding and message envelopes are specified in the host API.

To build and serve the complete browser IDE, use the
[playground example](../../examples/web/onda_wasm_playground/README.md#build-and-run).
After rebuilding only the GPU bundle, its `--skip-build` mode restages the IDE
without rebuilding the compiler.

## Verification

The browser suites require Node.js 22+ and a Chrome/Chromium executable:

```sh
CHROME_BIN=/path/to/chromium npm run test:gpu-ui
```

`test:gpu-ui` checks asset staging and the real canvas renderer. It covers input,
composition, presentation, capture visibility, resizing, and renderer failures.
It runs WebGPU and WebGL2 cases; set `ONDA_UI_TEST_WEBGPU=0` to skip cases that
require a hardware WebGPU adapter. Linux hosts without a desktop can use
`xvfb-run -a`. `ONDA_UI_CHROME_ARGS` accepts a JSON array of graphics flags.

The compiler/AudioWorklet suite needs both browser hosts staged first. It also
requires `cargo-about` 0.9.2 with its `cli` feature for compiler license generation:

```sh
npm run build --workspace @onda-lang/wasm-compiler
npm run build:gpu-ui
bash examples/web/onda_wasm_playground/build-demo.sh --skip-build
node scripts/build-website-playground.mjs
CHROME_BIN=/path/to/chromium npm run test:gpu-web-editor
```

This suite exercises both hosts with real compilation and muted AudioWorklet
processing, including autoplay blocking and hosting without cross-origin isolation.
Hardware WebGPU and WebGL2 cases use the same skip switch as the canvas suite.

Additional native tests require a display and are ignored by default. Run the
X11 `native_x11` tests and `ffi::tests` by name with `-- --ignored`; run
`standalone_prefers_native_wayland_over_the_xwayland_bridge` on a Wayland display.
The ignored clipboard test replaces the display's clipboard and requires an
isolated X11 session.

## Measurements and historical evidence

See [GPU UI measurements](../../scripts/bench/README.md#gpu-ui-measurements) for
offscreen rendering and live transport benchmarks. These measure different costs
from window presentation and end-to-end interaction latency.

[Native host](host-validation.json) and [browser editor](web-editor-validation.json)
records describe the dated builds and environments recorded in those files. They
are historical evidence, not certification of the current checkout or other platforms.
