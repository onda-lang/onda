Read README.md, docs/architecture.md.

Only read docs/syntax.md if the request is about the language use or design.

Keep the implementation concise, precise and easy to understand. Strive for elegant, well-thought-out and correct design rather than settling for the first draft. Aim for the simplest solution that works robustly, and strictly avoid bad practices that introduce fragile workarounds, one-off solutions that only work in narrow cases and don't scale, dead code and over-engineering. 

No hand-authored source file may exceed 5,000 lines of code. Generated, vendored, and lock files are exempt. Split files before they reach the limit.

Organize production-code splits around cohesive responsibilities with clear ownership, narrow interfaces, and private implementation details. Do not satisfy the line limit there with arbitrary chunks or numbered `part` modules; the resulting structure must make the code easier to navigate and reason about.

Large test suites may use numbered `part_N` files when they are purely `include!` partitions of one cohesive test module and share its setup and imports. Prefer named test modules whenever the tests have stable subject-area boundaries.

Ignore all concerns about backward compatibility.
