Read README.md, docs/architecture.md.

Only read docs/syntax.md if the request is about the language use or design.

No hand-authored source file may exceed 5,000 lines of code. Generated,
vendored, and lock files are exempt. Split files before they reach the limit.

Organize production-code splits around cohesive responsibilities with clear
ownership, narrow interfaces, and private implementation details. Do not
satisfy the line limit there with arbitrary chunks or numbered `part` modules;
the resulting structure must make the code easier to navigate and reason about.

Large test suites may use numbered `part_N` files when they are purely
`include!` partitions of one cohesive test module and share its setup and
imports. Prefer named test modules whenever the tests have stable subject-area
boundaries.

Ignore all concerns about backward compatibility.
