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
| `plugins/` | MIDI instruments and live-input effects shared with `onda-plugin`. |
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

Explore parameter arrays with the sustained [additive synth](instruments/additive_synth.onda):

```bash
onda run examples/instruments/additive_synth.onda
```

Start with only the first partial enabled, then add harmonics using the `levels` controls.
Each partial also has independent `ratios`, `detune_cents`, `pan`, and `enabled` controls.
Mute the even harmonics for a hollow tone, or set two ratios equal and detune one slightly
for beating. Non-integer ratios create inharmonic, metallic timbres.

[Structured filter](basic/structured_filter.onda) turns a slowly swept minor-triad saw pad into a
warm, evolving texture. Its low-pass design returns one coefficient struct per block, while a
separate memory struct carries the filter history through the sample loop.

[Structured messages](basic/structured_messages.onda) is a gently animated phase-modulation drone.
Its `configure` event accepts a nested tone-and-motion patch, installs it atomically, and publishes
the accepted patch through a structured delegate; edit the event JSON to reshape the timbre live.
