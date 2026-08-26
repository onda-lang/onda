# Onda examples

The examples are finished, compilable sounds organized by outcome. The online
[example cookbook](https://onda-lang.org/docs/examples/) provides a guided listening path and opens
the standalone sources directly in the browser playground.

| Directory | Contents |
| --- | --- |
| `basic/` | Compact synthesis, oversampling, typed events, and polyphony. |
| `buffers/` | Host-bound sample and typed-buffer processing. |
| `effects/` | Auditionable effects plus reusable processor implementations. |
| `feedback/` | Delay matrices, graph cycles, and nonlinear feedback systems. |
| `instruments/` | Self-playing and host-triggerable musical instruments. |
| `projects/` | Complete `.ondaproject` patches with source modules and embedded data. |
| `soundscapes/` | Granular, generative, and slowly evolving patches. |
| `spectral/` | Streaming FFT effects and reusable spectral processors. |
| `native/` | Native embedding and object-linking hosts. |
| `web/` | Browser and WebAssembly hosts. |

[`soundscapes/glass_garden.onda`](soundscapes/glass_garden.onda) is a generative glass-bell garden:
each resonance reports when it has faded, and that completion starts the voice's next bloom. The
top-level `bloom_faded` delegate also lets a host follow the evolving pattern; see
[Hosting Onda delegates](../docs/delegates.md) for storage sizing, decoding, and overflow handling.

Run or render a standalone example from the repository or an extracted release:

```bash
onda run examples/instruments/fm_bells.onda
onda run render examples/soundscapes/granular_cloud.onda --output cloud.wav --dur 12
```

Musical examples with built-in sequencing expose an `auto_play` parameter and a no-argument
`bang()` event. Effects audition themselves by default and expose `live_input` when they can process
host audio.

Project examples carry their source and data bindings together:

```bash
onda run examples/projects/embedded_room/embedded-room.ondaproject
```

The sample player requires a host-provided WAV or `.ondabuffer`; omitted buffers use neutral
one-frame storage, so unbound reads return zero and writes are discarded.
