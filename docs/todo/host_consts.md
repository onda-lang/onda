# Host-configured compile constants

## Status

This document specifies a proposed source and compiler API for overriding executable top-level
constants before compilation.

## Summary

Every value `const` declared in the executable program's root namespace is a compile-time
configuration input. A host may inspect those declarations, supply typed overrides, and compile
multiple specialized programs from the same prepared source graph.

Overrides are applied before ordinary constant evaluation. They therefore propagate through const
expressions, const defs, namespace arguments, array shapes, processor specialization, graph
lowering, task lowering, assertions, and generated MIR exactly as source-provided values do.

Compile constants are not runtime state. They add no loads, storage, branching, reflection, or
mutation to a compiled processor instance.

## Motivation

Some program choices should produce distinct optimized artifacts rather than runtime-parametric
code. Examples include FFT size, channel count, filter order, table layout, oversampling topology,
algorithm selection, and fixed resource limits.

Without host configuration, producing variants requires rewriting or generating source text:

```onda
const FftSize: i32 = 2048
const PartitionCount: i32 = 16
```

A host should instead be able to prepare this source once and compile independent variants with,
for example, `FftSize = 4096` and `FftSize = 8192`. Because the selected value remains a true Onda
constant, each result can be folded and specialized completely.

## Design principles

- Root-level `const` already defines the correct configuration boundary; no `expose` annotation is
  required.
- Configuration happens before semantic analysis and cannot mutate a compiled program or instance.
- Overrides feed the existing constant evaluator instead of creating a second substitution system.
- Source defaults remain valid Onda constant expressions and are used whenever no override exists.
- Types and shapes are validated by the same rules as source constant declarations.
- Derived constants update naturally when an upstream constant is overridden.
- Invalid configurations produce ordinary compiler diagnostics with source context.
- Prepared source, selected configuration, and compiled artifacts remain deterministic and
  reproducible.

## Configurable declaration surface

The host-visible surface contains every value `const` in the executable root namespace:

```onda
const Channels = 8
const FftSize: i32 = 2048
const Window: f32[] = make_window(FftSize)
```

The following are not directly configurable:

- builtin and contextual constants such as sample rate and block size;
- `const def` declarations;
- constants local to a runtime or const def;
- proc-local constants;
- constants declared inside a namespace;
- constants belonging to an imported declaration module.

An `include` contributes declarations to the executable program and its root constants therefore
belong to the configurable surface. An `import` keeps declarations in a module or namespace and
does not add its constants to that surface. Imported and namespace constants can still depend on a
root constant through the language's existing namespace arguments and const evaluation rules.

The surface is collected from the author-provided source graph before automatic standard-library
injection and before namespace flattening. Host names are unqualified root identifiers. Existing
duplicate-symbol validation guarantees that each name selects at most one declaration.

## Resolution semantics

Each configurable declaration has two conceptual values:

- **default value**: the declaration evaluated with the complete source-default configuration;
- **selected value**: either a host override or the declaration expression evaluated under the
  currently selected values of earlier constants.

Constant declarations keep their existing source-order dependency rule. A host override replaces
the value produced at the declaration point; it does not replace syntax globally.

```onda
const Base: i32 = 4
const Size: i32 = Base * 2
```

With no overrides, `Base` is `4` and `Size` is `8`. After selecting `Base = 8`, `Size` becomes `16`.
If the host also selects `Size = 24`, that explicit override wins. Clearing the `Size` override
restores evaluation of `Base * 2` under the current configuration.

The compiler performs constant resolution in this order:

1. Parse and compose the exact executable source graph.
2. Discover root constant declarations and resolve the source-default descriptor table.
3. At each root constant declaration, validate its source expression and determine its declared or
   inferred value type.
4. If an override exists, coerce and select it; otherwise evaluate the source expression using the
   selected values already in scope.
5. Continue through ordinary const defs, namespace expansion, count and shape expansion, constant
   folding, assertions, and semantic analysis.

The source initializer is still checked when an override is present. An override cannot make an
ill-typed constant declaration valid. Program behavior that depends on the selected configuration,
including assertions and downstream shape constraints, is checked against the selected values.

## Types and shapes

The first implementation supports the complete existing top-level value-constant surface:

- primitive scalars: `bool`, `i32`, `i64`, `f32`, and `f64`;
- fixed primitive arrays;
- inferred or explicitly sliced primitive constant arrays.

An inferred declaration's source-default resolution establishes its element or scalar type. An
override must preserve that type; host configuration never changes a constant from, for example,
`i32` to `f32`.

For fixed arrays, the selected value must match the currently resolved declared length. For sliced
or inferred constant arrays, the selected length may differ between compilations. This deliberately
allows one source program to produce artifacts with different compile-time table sizes.

Shapes may depend on earlier configurable constants:

```onda
const Size: i32 = 4
const Values: f32[Size] = [0.0, 0.25, 0.5, 1.0]
```

Changing `Size` changes the current descriptor for `Values`. Any existing `Values` override must
match the newly resolved shape. Updating an upstream constant is transactional: if it makes an
existing downstream override impossible to coerce or makes constant resolution fail, the prepared
compilation retains its previous valid configuration and reports a diagnostic.

Floating-point values preserve their exact width and bit pattern. `i64` values are never routed
through `f64`. Arrays use the canonical element ordering of Onda array literals.

## Prepared compilation lifecycle

Inspection and mutation belong to a prepared compilation object:

```text
source / source graph / project image + contextual compile options
                |
                v
      prepared compilation
       - immutable parsed source graph
       - fixed sample rate and block size
       - default constant table
       - current override set
       - current resolved constant table
                |
                +---- set / clear override
                |
                +---- compile variant A
                |
                +---- compile variant B
```

Preparing resolves source inputs and the default constant table under fixed contextual analysis
options, including sample rate and block size, but does not generate MIR or native code. Those
options must be known because source const expressions and shapes may depend on the corresponding
builtin constants. Compilation clones the prepared AST, applies the current override set through
constant resolution, and runs the normal analysis and backend pipeline. A prepared compilation can
therefore produce multiple independent artifacts without reparsing or reloading unchanged source
files.

Changing source text, a source-graph resolution, an included/imported document, a project image,
sample rate, or block size requires a new preparation. Overrides are configuration of one exact
prepared source graph and analysis context and are not implicitly carried to a different one.

Constant updates are transactional. A failed setter leaves the previous override set and resolved
metadata unchanged. Full semantic or backend errors found only during compilation likewise do not
mutate the prepared compilation.

The compiled program and instance APIs expose selected constants as read-only provenance if useful,
but provide no setter. Creating, resetting, initializing, snapshotting, or restoring an instance
has no relationship to compile constants.

## Metadata model

Each root constant descriptor contains at least:

```text
name
element type: bool | i32 | i64 | f32 | f64
kind: scalar | fixed array | variable-length const array
current array length (1 for scalars at the generic byte boundary)
element byte width
total value byte count
source-default value
currently selected value
has explicit host override
source location
```

Descriptors are returned in deterministic source declaration order. Name lookup is also provided.
After a successful update, descriptor queries observe the fully recomputed current table, including
derived constants and shapes affected by the override.

Rust uses a typed enum rather than unstructured bytes. C exposes element type and shape metadata
plus exact canonical value bytes, with scalar convenience setters where useful. JavaScript maps
`bool` to `boolean`, `i32`/`f32`/`f64` to `number`, `i64` to `bigint`, and arrays to the matching typed
array representation.

Compiled artifact metadata records the selected root constant table. This is build provenance, not
part of the runtime parameter or state interface. It allows tools to identify how an artifact was
specialized without implying that its constants can be modified.

## Rust API

The concrete names may follow existing compiler ownership conventions, but the intended shape is:

```rust
let mut compilation = Compiler::prepare_source(source, analysis_options)?;

for descriptor in compilation.consts() {
    println!("{}: {:?}", descriptor.name(), descriptor.selected_value());
}

compilation.set_const("FftSize", CompileConstValue::I32(4096))?;
let program_4096 = compilation.compile(codegen_options)?;

compilation.set_const("FftSize", CompileConstValue::I32(8192))?;
let program_8192 = compilation.compile(codegen_options)?;

compilation.clear_const("FftSize")?;
```

The same prepared-compilation abstraction accepts a single source, filesystem entry, exact virtual
source graph, or immutable project image. Lower-level callers that already own a parsed `Program`
can construct it directly from that program and its source identity metadata.

Constant overrides should not be added directly to `AnalysisOptions`. That type is a small copied
set of contextual compiler constants used throughout analysis. A separately owned compilation
configuration avoids cloning maps through every semantic helper and keeps contextual constants and
source-declared constants conceptually distinct.

## C API

The C API introduces an opaque prepared-compilation handle. Representative operations are:

```c
onda_compilation_t* onda_compilation_prepare(
  const char* source_utf8,
  const onda_compile_options_t* options,
  onda_diag_t* out_diag
);

int onda_compilation_const_count(const onda_compilation_t* compilation);
int onda_compilation_const_index(
  const onda_compilation_t* compilation,
  const char* name_utf8
);
const char* onda_compilation_const_name(
  const onda_compilation_t* compilation,
  int index
);
int onda_compilation_const_elem_type(
  const onda_compilation_t* compilation,
  int index
);
int onda_compilation_const_array_len(
  const onda_compilation_t* compilation,
  int index
);
int onda_compilation_const_value_bytes(
  const onda_compilation_t* compilation,
  int index
);

int onda_compilation_get_const(
  const onda_compilation_t* compilation,
  int index,
  void* out_value,
  int out_capacity,
  onda_diag_t* out_diag
);

int onda_compilation_get_const_default(
  const onda_compilation_t* compilation,
  int index,
  void* out_value,
  int out_capacity,
  onda_diag_t* out_diag
);

int onda_compilation_set_const(
  onda_compilation_t* compilation,
  int index,
  const void* value,
  int value_bytes,
  onda_diag_t* out_diag
);

int onda_compilation_clear_const(
  onda_compilation_t* compilation,
  int index,
  onda_diag_t* out_diag
);

onda_program_t* onda_compilation_compile(
  const onda_compilation_t* compilation,
  onda_diag_t* out_diag
);

void onda_compilation_destroy(onda_compilation_t* compilation);
```

Equivalent preparation entry points are required for filesystem input, exact source graphs, and
project images. They preserve the existing source-manifest and project-validation behavior. The
one-shot `onda_compile*` functions remain implementable as prepare-plus-compile conveniences.
Preparation binds the supplied sample rate and block size so descriptor metadata cannot drift from
the later compilation. Backend-only options may remain compile-time arguments if the public options
structure is split in the future; options that affect constant evaluation are always preparation
inputs.

Generic value bytes use one documented canonical encoding shared with artifact provenance. Queries
return the required size when the destination is null or too small. Setters require an exact byte
count and validate the descriptor's current type and shape before committing the update.

## JavaScript and browser API

The packaged compiler exposes a prepared object while retaining the one-shot convenience API:

```js
const compilation = await compiler.prepareSource(source, {
  sampleRate: 48_000,
  blockSize: 128,
});

for (const descriptor of compilation.constants) {
  console.log(descriptor.name, descriptor.type, descriptor.value);
}

compilation.setConst("FftSize", 4096);
const first = await compilation.compile();

compilation.setConst("FftSize", 8192);
const second = await compilation.compile();

compilation.clearConst("FftSize");
```

The direct form accepts the same override map without requiring explicit preparation:

```js
const result = await compiler.compileSource(source, {
  constants: {
    FftSize: 4096,
    Window: new Float32Array([0.0, 0.5, 1.0, 0.5]),
  },
});
```

Workspace and project-image compilation accept the same `constants` option. Worker transport
serializes `bigint` and typed arrays without converting them through JSON numbers.

## CLI

The compile command accepts repeatable root-constant overrides:

```text
onda compile program.onda --const FftSize=4096 --const Channels=2
```

Scalar CLI values use Onda literal syntax and are coerced against the discovered declaration type.
Array support should use an unambiguous Onda array literal or a dedicated file form rather than a
comma-splitting convention. Unknown, duplicate, or malformed overrides are errors.

An inspection mode exposes the prepared default surface without generating an artifact:

```text
onda compile program.onda --list-consts
```

Machine-readable output should use the CLI's established structured-output conventions rather than
requiring callers to scrape formatted diagnostic text.

## Determinism, caching, and artifacts

The canonical compilation identity contains:

- the exact source or project image identity already used by the compiler;
- contextual compile options such as sample rate and block size;
- the sorted set of explicit typed overrides, encoded without float or `i64` loss;
- backend code-generation options.

The source identity already captures declaration defaults, so a cache key need not duplicate every
default value. It must distinguish an absent override from an explicit override equal to the
current default because artifact provenance reports the explicit selection, even if generated MIR
happens to be identical.

Prepared metadata and output artifacts are deterministic for the same source identity and override
set. Project images remain immutable; overrides are external compilation inputs and do not mutate
or weaken image integrity. A packaged artifact records both its source/image association and its
selected compile-constant provenance.

## Diagnostics

Configuration failures use normal structured diagnostics. Required cases include:

- unknown root constant name;
- attempt to configure a namespace, proc-local, local, builtin, or const-def symbol;
- scalar or element type mismatch;
- fixed-array length mismatch;
- malformed canonical bytes or JavaScript value representation;
- non-finite value where the existing const rules reject it;
- an upstream update invalidating a downstream override or shape;
- selected values violating an `assert` or another semantic constraint;
- duplicate CLI overrides.

Diagnostics should name both the host-selected constant and the affected source declaration. When
an upstream selection invalidates a downstream declaration, the diagnostic should preserve the
downstream source location and mention the responsible override.

## Performance and realtime behavior

Preparation performs parsing, source-graph composition, and default constant resolution once.
Compiling a new variant reruns all specialization-dependent semantic and backend work; reusing a
prepared AST avoids source I/O and parsing but does not promise incremental code generation.

Override lookup during constant resolution is a deterministic name-table operation and is not on a
runtime path. Generated processors contain only the selected folded values and have exactly the
same realtime properties as equivalent source with those values written directly.

## Explicit non-goals

This design does not include:

- changing a compile constant on an existing program or instance;
- exposing constants as params, events, state, or snapshot entries;
- runtime specialization or lazy JIT recompilation from the audio thread;
- directly configuring namespace, proc-local, local, builtin, or const-def symbols;
- string-based source replacement or macro substitution;
- permitting an override to change a declaration's scalar or element type;
- automatic transfer of overrides to a changed source graph;
- incremental semantic analysis or backend compilation between variants in the first version.

## Implementation structure

1. Add shared typed compile-constant descriptors and values covering primitive scalars and const
   arrays.
2. Collect executable root `ConstDecl` declarations with stable source identity before auto-import
   and namespace lowering.
3. Refactor the existing const-coercion phase so it can resolve both the source-default table and a
   selected table with overrides applied at declaration points.
4. Implement transactional override set/clear plus dependency and shape recomputation on a prepared
   compilation.
5. Route the selected table into the existing count expansion, const-def evaluation, constant
   folding, namespace, proc, graph, task, and assertion passes.
6. Add the prepared-compilation Rust API while keeping one-shot compile paths as default-config
   conveniences.
7. Add matching C, compiler-Wasm, JavaScript, worker, and CLI surfaces for every supported source
   input form.
8. Record selected constants as read-only artifact provenance and include canonical overrides in
   compilation cache identities.
9. Add conformance coverage for dependency propagation, const defs, arrays and changing shapes,
   namespaces, processor specialization, graphs, tasks, assertions, projects, source graphs,
   diagnostics, exact `i64` and floating-point values, and native/Binaryen parity.
