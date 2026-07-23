# Onda website

The site implementation lives in this directory. Documentation remains in the
repository-level `docs/` directory so the website and repository share one
source of truth.

To preview locally using the same self-contained source tree as GitHub Pages,
build the versioned, content-addressed compiler/playground assets, stage the site, and serve it from the repository root:

```bash
npm ci
npm run build:website
bash ./website/stage.sh
jekyll serve --source _site_source --baseurl "" --livereload
```

The homepage links its displayed example into the browser playground without loading the compiler;
it does not start audio. The `/playground/` route loads the same `wasm-opt -O4` frontend and Binaryen
backend, runs the real `onda lsp` implementation in the compiler worker, and offers only 44100/48000 Hz
sample rates and 128/256/512/1024/2048-frame compile blocks (defaulting to 512). The editor keeps a
multi-file virtual project in local storage and uses the same shared run webview as `onda-vscode`
for audio, scope, parameters, events, and WAV buffers. Share links encode a compressed, versioned
multi-file source snapshot in the client-side URL fragment. Compile settings and selected WAV data
remain local to the device. Cookbook example links select projects from a versioned browser catalog
generated from the checked-in `examples/` sources, including any local Onda dependencies. GitHub
Actions runs the same versioned asset build and staging script before building the Pages artifact.
