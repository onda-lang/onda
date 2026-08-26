# Host-configured compile constants

## Status

Implemented. This document defines the explicit source declaration and compiler input API for
selecting compile-time program variants.

## Summary

An executable may declare typed configuration constants:

~~~onda
config const FftSize: i32 = 2048
config const Channels: i32 = 2
~~~

A host compiles the program with an immutable map of optional overrides. An override replaces the
initializer at that declaration for one compilation. The selected value then flows through the
ordinary constant evaluator, namespace expansion, array and interface shapes, processor
specialization, graph and task lowering, assertions, and generated MIR.

Configuration constants remain true compile-time values. They add no runtime storage, loads,
branching, reflection, or mutation to a compiled processor.

There is no required prepare/set/clear lifecycle. Parsing or loading may be reused independently of
this feature, and optional configuration inspection is a separate semantic query.

## Motivation

Some choices should produce distinct optimized artifacts rather than runtime-parametric code:

- FFT and partition sizes;
- channel and voice counts;
- filter order and oversampling topology;
- lookup-table contents and layout;
- algorithm selection;
- fixed resource limits.

Without compile inputs, a host must rewrite or generate source text:

~~~onda
config const FftSize: i32 = 2048
const HopSize: i32 = FftSize / 2
~~~

The host should instead compile the same loaded source with FftSize set to 4096 or 8192. HopSize and
all downstream structure then update through normal constant evaluation and specialization.

## Design principles

- The source explicitly declares its public compile-time configuration surface.
- Every configuration declaration has an explicit, stable type.
- Configuration declarations support every value type supported by ordinary const declarations;
  they do not define a narrower parallel type system.
- Overrides are immutable inputs to one compilation.
- An override participates in the existing constant evaluator rather than a second substitution
  system.
- Derived ordinary constants are not independently configurable unless the author marks them as
  configuration declarations.
- Type, shape, assertion, and semantic errors use ordinary source diagnostics.
- Parsing and source-graph reuse are general compiler capabilities, not a host-constant-specific
  prepared object.
- Configuration is deterministic and suitable for build identities and caching.

## Source model

### Declaration syntax

A host-configurable constant uses config const and must declare its type:

~~~onda
config const Enabled: bool = true
config const Channels: i32 = 2
config const Seed: i64 = i64(1234)
config const Gain: f32 = 0.5
config const Phase: f64 = 0.0
config const Window: f32[] = make_window(1024)
config const Coefficients: f64[4] = [0.1, 0.2, 0.3, 0.4]
~~~

Untyped configuration declarations are rejected. Ordinary untyped const declarations retain their
existing contextual numeric semantics and are never forced into one host-visible scalar type.

The explicit type may be any type accepted by an ordinary value const declaration. Today that is a
primitive scalar, fixed primitive array, or primitive const slice. If the language gains additional
const value types, configuration declarations inherit them through the same AST, type checker, and
constant-value representation rather than requiring a separate feature extension.

### Where declarations are allowed

Configuration declarations are allowed only in the executable root namespace:

- the entry source may declare them;
- an included source contributes to the executable and may declare them;
- an imported declaration module may not declare them;
- a namespace, proc, const def, runtime def, task, event, init, block, or sample scope may not
  declare them;
- builtin and contextual constants such as sample rate and block size are not configurable through
  this mechanism.

Rejecting config const while loading an imported file preserves the entry/include boundary before
the loader composes imported declarations into the executable Program.

Names are the unqualified root identifiers already subject to normal duplicate-symbol validation.
Automatic standard-library injection cannot add configuration declarations.

### Configuration inputs and derived constants

Only config const declarations accept host inputs:

~~~onda
config const BaseSize: i32 = 1024
config const PartitionCount: i32 = 16

const FftSize: i32 = BaseSize * 2
const TableSize: i32 = FftSize * PartitionCount
~~~

Overriding BaseSize recomputes FftSize and TableSize. The host cannot override either derived
ordinary constant.

One configuration default may depend on earlier configuration constants:

~~~onda
config const Base: i32 = 4
config const Size: i32 = Base * 2
~~~

With Base set to 8 and no Size override, Size resolves to 16. If Size is also supplied, the explicit
Size input wins.

Existing source-order rules remain unchanged. Forward references are still rejected.

## Resolution semantics

The compiler resolves one immutable input set as follows:

1. Parse and compose the exact executable source graph.
2. Collect config const declarations before automatic standard-library injection and namespace
   flattening.
3. Reject unknown input names and inputs targeting ordinary constants or other declarations.
4. At each configuration declaration in source order, either:
   - convert the host value to a typed constant expression and select it; or
   - evaluate the source initializer using previously selected constants.
5. Continue through the existing namespace, const-def, count, shape, folding, assertion, processor,
   graph, task, and semantic passes.

An override replaces the initializer for that compilation; the source initializer is not separately
evaluated. The explicit declaration type and every downstream use are still checked. Compiling
without that override evaluates and validates the source default.

Applying an input map is atomic because no compiler-owned configuration is mutated. A failed
compilation has no state to roll back, and clearing an override means omitting it from the next
request.

Parsing itself never requires configuration values. They are needed before early semantic
preprocessing because namespace flattening and shape expansion already consume constants.

## Types and shapes

Compile inputs use the compiler's canonical typed constant-value model. The supported input types
are exactly the supported ordinary value-const types. At present these are:

- bool, i32, i64, f32, and f64 scalars;
- fixed primitive arrays;
- variable-length primitive const arrays declared with a slice type.

This list describes the current language rather than a permanent host-API restriction. Future
tuple, struct, or other const value types become valid compile inputs when they become valid value
const declarations.

The host value carries the same exact type information as a source constant. Numeric values are not
implicitly routed through f64, and i64 values retain their full width. JavaScript uses bigint for
i64 and typed arrays for the currently supported numeric arrays.

For a fixed array, the supplied value must match the selected declared length:

~~~onda
config const Size: i32 = 4
config const Values: f32[Size] = [0.0, 0.25, 0.5, 1.0]
~~~

If Size is set to 8, a Values input must contain eight f32 elements. If Values is omitted, its source
initializer is evaluated under Size = 8 and will produce the ordinary fixed-length diagnostic unless
it also resolves to eight elements.

Every compilation resolves the fixed length from its complete selected input map before accepting
the array value. Therefore changing an upstream constant without supplying a compatible downstream
array is an error whether the downstream value comes from another host input or from its source
default. There is no previous configuration whose array value or shape can remain stale.

For a slice declaration, the selected input determines the concrete length for that compilation:

~~~onda
config const Window: f32[] = make_window(1024)
~~~

Different compilations may supply different Window lengths.

## Compiler API

### Immutable compile inputs

Compile constants belong to a request-level input structure, separate from AnalysisOptions. The
value type is shared with ordinary semantic constant evaluation; the following names are
representative:

~~~rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompileInputs {
    pub constants: BTreeMap<String, ConstValue>,
}
~~~

ConstValue is not a host-constants-only enum. It is the canonical lossless representation used by
the constant evaluator for every supported scalar and aggregate const value.

AnalysisOptions remains the small copied set of contextual compiler values such as sample rate and
block size. CompileInputs is owned at the outer compiler boundary and is not cloned through
unrelated semantic helpers.

Representative lower-level use:

~~~rust
let loaded = load_program_file(path)?;

let first_inputs = CompileInputs {
    constants: BTreeMap::from([(
        "FftSize".to_owned(),
        ConstValue::Scalar(TypedConstValue::I32(4096)),
    )]),
};
let first = compile_loaded(
    &loaded,
    analysis_options,
    &first_inputs,
    codegen_options,
)?;

let second_inputs = CompileInputs {
    constants: BTreeMap::from([(
        "FftSize".to_owned(),
        ConstValue::Scalar(TypedConstValue::I32(8192)),
    )]),
};
let second = compile_loaded(
    &loaded,
    analysis_options,
    &second_inputs,
    codegen_options,
)?;
~~~

The exact helper names should follow the compiler's existing ownership boundaries. The important
properties are that the loaded source is immutable, compilation accepts all inputs at once, and
compiling one variant cannot affect another.

One-shot source, filesystem, virtual-source-graph, and project-image entry points accept the same
CompileInputs. Internally they parse or load and then call the same parsed-program path.

### Source reuse

If C or JavaScript consumers need to compile many variants without reparsing, expose a general
immutable loaded-source handle:

~~~text
source / source graph / project image
                |
                v
       immutable loaded source
                |
                +---- compile(options, inputs A)
                |
                +---- compile(options, inputs B)
~~~

That handle owns the parsed Program and exact SourceManifest. It does not own a current override
set, selected descriptor table, backend artifact, or mutable compiler session.

Source reuse may be added after measuring repeated-variant workloads. The one-shot API remains the
primary surface.

## Optional inspection

Tools may need to display configuration names, defaults, and resolved shapes before compiling an
artifact. Provide an explicit semantic query for that use case:

~~~rust
let descriptors = inspect_compile_constants(
    &loaded,
    analysis_options,
    &CompileInputs::default(),
)?;
~~~

The query runs the same configuration resolver used internally by compilation. It is not a required
preflight call, and callers may compile directly.

A descriptor contains only resolved information for the supplied immutable request:

~~~text
name
declared scalar or element type
kind: scalar | fixed array | variable-length array
resolved element count
resolved value
source location
~~~

It does not expose mutable current/default state or setter methods. To inspect another variant, call
the query again with another CompileInputs value.

## C API

The C boundary accepts all overrides with a compile request rather than introducing a mutable
prepared-compilation handle. Each entry contains:

~~~text
name
element type tag
element count
canonical value bytes
~~~

Scalars have one element. The encoding must preserve i64 and each floating-point width exactly.
Unknown tags, malformed byte counts, type mismatches, and shape mismatches are reported through the
existing diagnostic mechanism.

One-shot compile entry points accept a pointer and count for these inputs. A future immutable
loaded-source handle may expose compile and inspection operations using the same request structure.
There are no set, clear, or rollback functions.

## JavaScript and browser API

One-shot compilation accepts a constants property:

~~~js
const first = await compiler.compileSource(source, {
  sampleRate: 48_000,
  blockSize: 128,
  constants: {
    FftSize: 4096,
    Window: new Float32Array([0.0, 0.5, 1.0, 0.5]),
  },
});
~~~

Workspace and project-image compilation accept the same property. JavaScript maps bool to boolean,
i32/f32/f64 to number, i64 to bigint, and arrays to the corresponding typed array representation.
Worker transport must preserve bigint and typed arrays without JSON-number conversion.

If parsed-source reuse is later exposed, the object remains immutable:

~~~js
const loaded = await compiler.loadSource(source);
const a = await compiler.compile(loaded, options, { FftSize: 4096 });
const b = await compiler.compile(loaded, options, { FftSize: 8192 });
~~~

Optional inspection is a separate operation on the source plus a complete input map.

## CLI

The compile command accepts repeatable overrides:

~~~text
onda compile program.onda --const FftSize=4096 --const Channels=2
~~~

CLI values use Onda literal syntax and are coerced against the explicitly declared configuration
type. Arrays use an unambiguous Onda array literal or a dedicated file form rather than comma
splitting.

An optional inspection mode resolves the source defaults without generating an artifact:

~~~text
onda compile program.onda --list-consts
~~~

Unknown, duplicate, or malformed command-line overrides are errors.

## Diagnostics

Configuration failures use normal structured diagnostics. Required cases include:

- config const without an explicit type;
- config const outside the executable root or inside an imported module;
- unknown input name;
- an input targeting an ordinary const or another declaration;
- scalar or element type mismatch;
- fixed-array length mismatch under the selected upstream configuration;
- malformed canonical bytes or JavaScript representation;
- duplicate CLI or C request entries;
- selected values violating an assertion or downstream semantic constraint.

Diagnostics point at the configuration declaration or affected downstream declaration and identify
the responsible host input where relevant.

## Determinism, caching, and provenance

A compilation identity contains:

- the exact source or project-image identity;
- contextual analysis options such as sample rate and block size;
- the sorted typed input map, encoded without floating-point or i64 loss;
- backend code-generation options.

Absence of an input is distinct from explicitly supplying a value equal to the source default.

The first implementation does not add source configuration to the MIR runtime interface or
processor descriptor. A compiler or packaging result may record the explicit input map as build
provenance. The source identity already captures defaults and derived constants.

## Performance and realtime behavior

Input lookup occurs during constant resolution and is not on a runtime path. Generated processors
contain only folded selected values and have the same realtime properties as equivalent source
initializers.

Compiling another input map reruns specialization-dependent semantic and backend work. Reusing a
loaded Program may avoid source I/O and parsing, but no incremental semantic or backend compilation
is promised initially.

## Explicit non-goals

This design does not include:

- changing a compile constant on an existing program or instance;
- a mutable prepared-compilation configuration;
- setters, clearing, transactional session state, or rollback;
- exposing compile constants as params, events, state, or snapshots;
- runtime specialization or recompilation from the audio thread;
- overriding ordinary, namespace-local, proc-local, or local constants;
- allowing an override to change its declared scalar or element type;
- transferring inputs automatically to a changed source graph;
- incremental semantic analysis or backend compilation;
- mandatory artifact or processor-descriptor provenance in the first implementation.

## Implementation structure

1. Add config const syntax and retain an explicit configuration marker on ConstDecl.
2. Require an explicit type accepted by ordinary value const declarations, without maintaining a
   separate configuration-only allowlist.
3. Reject configuration declarations in imported modules, namespaces, and executable-local scopes.
4. Expose the canonical semantic ConstValue through CompileInputs at the compiler boundary; do not
   introduce a second value hierarchy specifically for host inputs.
5. Before automatic standard-library injection and namespace flattening, validate the complete input
   map and apply selected typed expressions at configuration declaration points.
6. Route namespace flattening and the main const/count pass through the same selected constant
   environment, refactoring their duplicated declaration-resolution logic rather than adding a new
   evaluator.
7. Add optional descriptor inspection as a thin query over that resolver.
8. Add one-shot Rust and CLI inputs first, followed by C and compiler-Wasm/JavaScript surfaces using
   the same immutable request model.
9. Add general immutable loaded-source reuse only if repeated compilation measurements justify its
   public complexity.
10. Cover defaults, explicit overrides, derived constants, namespace arguments, arrays and changing
    shapes, const defs, processors, graphs, tasks, assertions, imports/includes, exact i64 and float
    values, diagnostics, source graphs, projects, and native/Binaryen parity.
