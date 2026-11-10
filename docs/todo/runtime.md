# Runtime TODO

## Optimization / runtime follow-ups

- RT-safety verification suite
  - Extend allocation/deallocation regression checks to lock detection.
  - Add repeatable stress tests around bind/rebind/process paths.

- Actual runtime block length
  - Consider `block_samples()` (or `block_frames()`) to expose the actual number of sample
    frames in the current logical block, distinct from compile-time `BLOCK_SIZE` capacity.
  - Keep the count unchanged across segments of the same logical block, including MIDI splits,
    so block-rate timing can account for short blocks. Define its availability outside processing.

- Onda-owned denormal handling
  - Revisit denormal/subnormal suppression as a generic Onda feature across native JIT/AOT and
    Wasm, independent of fast math, so hosts no longer need the explicit
    `onda_configure_current_thread_audio_fp_mode` API.
  - Define the numerical contract and scope across initialization, events, tasks, and processing.
    Evaluate native host FP-mode preservation and Wasm software flushing, with scalar/SIMD f32/f64
    correctness, cross-backend parity, and representative DSP performance measurements.

Processor-array hook lowering and loop proofs belong in [compiler.md](compiler.md), target SIMD
selection in [backends.md](backends.md#optimization-follow-ups), and DSP algorithm work in
[stdlib.md](stdlib.md).
