---
layout: home
title: Onda
description: Expressive and performant JIT-compiled audio programming language
---

<main class="home-main">
  <section class="home-intro">
    <div class="home-title">
      <div class="home-logo" aria-hidden="true">
        <img class="theme-logo theme-logo-light" src="{{ '/assets/svg/onda-logo.svg' | relative_url }}" alt="">
        <img class="theme-logo theme-logo-dark" src="{{ '/assets/svg/onda-logo-dark.svg' | relative_url }}" alt="">
      </div>
      <h1>Onda</h1>
    </div>
    <p class="home-lead">Expressive and performant JIT-compiled audio programming language</p>
  </section>

  <section class="home-example">
    <div class="section-title">
      <h2>Code example</h2>
      <p>A subtractive synth with resonant filters and oversampled saturation</p>
    </div>
    <div class="home-example-code">
<pre><code class="language-onda">{% include home-example.onda %}</code></pre>
      <a class="primary-button" href="{{ '/playground/?example=basic/saw_filter_saturator.onda' | relative_url }}">Open in playground</a>
    </div>
  </section>

  <nav class="home-docs" aria-label="Documentation">
    <div class="doc-list">
      <a href="{{ '/docs/getting-started/' | relative_url }}"><strong>Getting started</strong><span>Try Onda in your browser, install the CLI, run a program, and render audio</span></a>
      <a href="{{ '/docs/language/' | relative_url }}"><strong>Language guide</strong><span>Syntax and semantics from basic programs through processors, graphs, generics, and modules</span></a>
      <a href="{{ '/docs/examples/' | relative_url }}"><strong>Examples</strong><span>Musical instruments, effects, soundscapes, embedded-buffer projects, advanced DSP, and focused language references</span></a>
      <a href="{{ '/docs/tooling/' | relative_url }}"><strong>CLI and editors</strong><span>Compilation, playback, rendering, language-server support, and embedding</span></a>
    </div>
  </nav>
</main>
