# UI TODO

## Rendering efficiency and frame pacing

The shared UI already skips unchanged frames, caches geometry and text preparation,
and tracks waveform/meter revisions separately. Each submitted frame still clears
and redraws the entire visible target. Stock hosts target 60 Hz presentation when
needed and a separate 30 Hz visualization stream; see the
[UI scheduling contract](../ui-api.md#host-lifecycle).

- Measure idle CPU wakeups, GPU submissions, and active-frame CPU/GPU cost across
  standalone, plugin, and browser/VS Code hosts. Cover a settled UI, silent audio,
  live meters/scope, knob drags, scrolling, and large control banks. Distinguish
  rendering from audio processing, capture polling, and source/child maintenance.
- Verify that settled views stop drawing and unnecessary UI timers, including
  after transient feedback expires. Preserve changed-revision checks, identical
  data suppression, hidden-view suspension, and required redraws on exposure or
  resize. Extend existing idle coverage where host behavior is not yet tested.
- Evaluate partial repainting or cached static layers to avoid redrawing the whole
  UI for a knob, meter, or waveform update. Use the measurements to choose an
  approach and implement it where it materially reduces work. Account for clipping,
  overlapping content, text, popups, scrolling, resize, and presentation-surface
  contents; retain a correct full-redraw path.
- Validate frame pacing under sustained input, visualization updates, mixed display
  refresh rates, and stalls. Measure frame intervals and input-to-presentation
  latency; preserve coalescing and avoid idle frame loops or catch-up queues.
- Experiment with GPUI using a representative run-screen prototype. Compare idle
  work, redraw cost, frame pacing, and input latency with the current renderer;
  assess native plugin embedding, browser support, text/input behavior, and
  integration complexity before considering adoption.
