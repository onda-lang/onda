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

Musical examples with built-in sequencing expose an `auto_play` parameter and a no-argument
`bang()` event. Effects audition themselves by default and expose `live_input` when they can process
host audio.

Project examples carry their source and data bindings together:

```bash
onda run examples/projects/embedded_room/embedded-room.ondaproject
```

