# Onda examples

The examples are grouped by the same learning path used in the [example cookbook](../docs/examples.md).

| Directory | Contents |
| --- | --- |
| `foundations/` | Small programs introducing execution scopes, parameters, events, and basic synthesis. |
| `processors-and-graphs/` | Reusable processors, proc arrays, imperative composition, and declarative graphs. |
| `standard-library/` | Focused examples for modules under `stdlib/std`. |
| `buffers-fft-convolution/` | Host buffers, data structures, FFT processing, and convolution. |
| `larger-patches/` | Polyphonic, feedback, reverb, neural, and other larger programs. |
| `web/` | Browser and WebAssembly host examples. |

Run an example from the repository root:

```bash
onda run examples/foundations/sine.onda
```

Buffer-oriented examples can run without an asset: omitted buffers use neutral one-frame storage,
so reads return zero and writes are discarded. Bind a WAV or `.ondabuffer` when the example needs
meaningful source data; its source or local README describes any other setup.
