---
title: Agentic programming
description: Give coding agents the Onda language and standard-library references to write idiomatic audio programs.
permalink: /docs/agentic-programming/
section: learn
eyebrow: Tools
---

# Agentic programming

Onda's syntax and semantics are documented in a single file, `docs/syntax.md`.
Give your coding agent this file to read before asking it to write Onda code:
it provides the complete language context for writing idiomatic programs, including
execution and state, processors, events, graphs, and generics.

For the standard library, also provide `docs/stdlib.md`. It documents the available
modules, functions, structs, and processors so the agent can use existing DSP building blocks.

## Get the references

Both `syntax.md` and `stdlib.md` ship in the `docs` directory of the
[packaged release](https://github.com/onda-lang/onda/releases/latest). Use these bundled files
to give the agent references that match your installed Onda version. They are also available
in the repository's `docs` directory.

The website serves the [language guide]({{ '/docs/language/' | relative_url }}) and
[standard-library reference]({{ '/docs/stdlib/' | relative_url }}) as static HTML, with readable
text and ordinary links. This makes the documentation easy for agents to read, crawl, and index
without executing JavaScript. Point an agent with web access at the
[documentation index]({{ '/docs/' | relative_url }}) to find these references and related guides.

## Give the agent a task

For example, from the extracted release directory:

```text
Read docs/syntax.md and docs/stdlib.md before writing Onda code.
Write a sine oscillator in sine.onda with frequency and level parameters.
Run onda compile sine.onda and fix any diagnostics.
```

Compile after changes to check syntax and semantics. To hear the result, open it with
`onda run sine.onda` or render a WAV file with
`onda run render sine.onda --output sine.wav --dur 5`.
