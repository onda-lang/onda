# Standard library TODO

## Completed scalar DSP improvements

The pitch-shift, conversion and compressor pass is implemented and validated. `DualWindow` uses
complementary Hann gains and correctly types its f64 phase/block state. Math conversion helpers
share the levels/pitch arithmetic, and the compressor caches curve constants and skips
transcendental work below the knee while continuing detector processing. Validation includes
the stdlib integration suite and startup/warmed native and Wasm numerical checks.

## Deferred DSP performance work

The FFT/STFT implementation pass is complete for now. At N=1024, the controlled retry measured
3.58x faster f32 real FFT and 5.52x faster f32 STFT against the original source using the same
compiler. Across tested sizes, FFTW remains 1.63–2.14x faster for f32 and 2.19–2.60x for f64.
The remaining work is deferred.

Keep stdlib DSP expressed with scalar Onda loops and functions. General proofs belong in the
[compiler](compiler.md#dsp-loop-proofs-and-diagnostics), SIMD selection in the
[backends](backends.md#optimization-follow-ups), and source ergonomics in the
[language](language.md#language-follow-ups). Algorithm changes should remain stdlib work.

- Tune FFT decomposition and small codelets, including stage fusion, register reuse, coefficient
  layout, and permutation/cache traffic. Evaluate several bounded schedules for compile-time sizes
  and targets; select from measurements rather than hard-coding compiler behavior for FFT names.
  Retain the shared half-size real-transform path used by real FFT and STFT, full-spectrum API
  behavior, short-input padding, and independent forward/inverse numerical checks.
- Evaluate explicit `fma` in portable scalar butterfly code with an intentional numerical contract.
  Measure native and Wasm separately: the current Wasm FMA math kernel has a different cost from
  native hardware. Track static table storage and compilation cost as well as throughput.
- Improve direct FIR throughput after addressing bounds proofs and vectorization. The measured
  four-accumulator rewrite did not help. Define reduction order/error tolerance before adopting
  transformations that reassociate floating-point arithmetic.
- Tune convolution for throughput and block latency across partition sizes, stereo and multiple
  instances. Include impulse loading/replacement, long tails, and scheduling of FFT work; avoid
  choosing a default from average throughput alone.
- Preserve already inexpensive filter, delay, gain, and mixing paths unless representative workloads
  expose a bottleneck; repeat the scalar SVF comparison with the updated independent C reference.
- Evaluate DSP quality separately from throughput: oscillator aliasing and frequency/modulation
  extremes, filter responses, interpolation error, noise statistics, and reverb/pitch-shift quality.
  Consider triangle corner correction and RNG alternatives only against explicit quality targets.

## Deferred measurement coverage

- Complete warmed native/Wasm validation for the other stdlib components, including convolution
  tails, pitch shifting, envelopes over a note lifecycle, and external-buffer sample playback.
- Add representative block sizes, bounded voice/channel banks, ARM targets, and browser AudioWorklet
  runs. Compare equivalent interfaces/layouts and numerical policies with independent references.
- Measure isolated FFT stages and reconstruction, inspect generated assembly, and use performance
  counters where available to attribute the remaining FFTW gap. IR observations alone do not
  establish each optimization's runtime benefit.
- Keep benchmarks sequential, explicitly selected, resource-bounded, and persisted outside `/tmp`.
  Record source hashes, timing variation, initialization/compilation costs, throughput, and block
  latency. Preserve independent full-spectrum and roundtrip checks for both precisions and backends.
