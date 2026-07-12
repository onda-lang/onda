---
layout: home
title: Onda
description: Onda is a JIT-compiled audio programming language.
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
    <nav class="home-links" aria-label="Start here">
      <a href="{{ '/docs/getting-started/' | relative_url }}">Getting started</a>
      <a href="{{ '/docs/language/' | relative_url }}">Language guide</a>
      <a href="https://github.com/onda-lang/onda/releases/latest">Download</a>
      <a href="https://github.com/onda-lang/onda">GitHub ↗</a>
    </nav>
  </section>

  <section class="home-example">
    <div class="section-title">
      <h2>Code example</h2>
      <p>A standard-library oscillator and resonant filter with a custom oversampled saturator</p>
    </div>
<pre><code class="language-onda">import std/osc
import std/filter

params:
  freq = 110.0 {20.0, 880.0}
  cutoff = 1200.0 {40.0, 12000.0}
  resonance = 0.8 {0.1, 8.0}
  drive = 1.0 {1.0, 10.0}

def soft_clip(x):
  return tanh(x)

proc Saturator:
  params:
    amount = 1.0

  sample 4:
    out1 = soft_clip(in1 * amount)

init:
  osc = std::osc::Saw()
  filter = std::filter::Svf(cutoff = cutoff, q = resonance)
  saturator = Saturator()

block:
  filter.update_coeffs(cutoff, resonance)

  sample:
    tone = osc(freq = freq)
    out1 = saturator(filter(tone), amount = drive)</code></pre>
  </section>

  <nav class="home-docs" aria-label="Documentation">
    <div class="doc-list">
      <a href="{{ '/docs/getting-started/' | relative_url }}"><strong>Getting started</strong><span>Download or build Onda, run a program, and render audio</span></a>
      <a href="{{ '/docs/language/' | relative_url }}"><strong>Language guide</strong><span>Syntax and semantics from basic programs through processors, graphs, generics, and modules</span></a>
      <a href="{{ '/docs/examples/' | relative_url }}"><strong>Examples</strong><span>Programs covering oscillators, events, processors, graphs, the standard library, FFT, and convolution</span></a>
      <a href="{{ '/docs/tooling/' | relative_url }}"><strong>CLI and editors</strong><span>Compilation, playback, rendering, language-server support, and embedding</span></a>
      <a href="{{ '/docs/architecture/' | relative_url }}"><strong>Architecture</strong><span>The compiler, runtime, CLI, daemon, and host crates</span></a>
      <a href="{{ '/docs/roadmap/' | relative_url }}"><strong>Roadmap</strong><span>Current design ideas and planned work</span></a>
    </div>
  </nav>
</main>

<footer class="site-footer">Onda is open-source software. Source and issue tracking are on <a href="https://github.com/onda-lang/onda">GitHub</a>.</footer>
