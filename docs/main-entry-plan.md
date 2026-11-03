# Main processor entry implementation

An unqualified, non-generic `proc Main` in the entry or its included files is an
ordinary processor automatically constructed once per host instance. A compiler
adapter exposes its public interface using the existing top-level program model.
Bare top-level programs keep their existing path. Imported modules cannot supply
an entry, and Main and bare executable sections cannot be mixed.

## Language contract

- Main uses ordinary proc grammar, ownership, helpers, events, tasks, children,
  initialization, and output-rate restrictions. It declares either audio outputs
  or control outputs. Module declarations remain outside Main.
- Public params become host controls; private params remain internal. Existing
  parameter control attributes are accepted on ordinary proc declarations.
- Host controls and writable proc params retain separate storage. Construction
  supplies host controls; each logical block and exported event refreshes public
  params through ordinary stores and hooks. Refresh precedes proc block-pre.
  Intermediate process segments do not refresh params. Owner writes last until
  the next refresh and do not update host controls. Equal stores still run hooks.
- Inputs, outputs, logical arrays, buffers, authored events, and delegates retain
  their names, shapes, types, and order. The builtin proc init event is not an
  exported authored event. Buffer bindings remain live.
- Main retains ordinary oversampling. Interface projection preserves declaration
  contexts for dimensions/ranges and caller contexts for constructor defaults.
- Event-only Main is constructed but never stepped; declared audio outputs are
  neutral. Initialization preserves existing pinned-state and task policies.
- Generated names remain hygienic and authored Main state retains its provenance.
  MIR, processor ABI, backends, and hosts require no Main-specific behavior.

## Completed implementation

- [x] Retain Main as a ProcessorDef, enforce entry selection in loading, and extend
   ordinary proc parameter metadata syntax. Update frontend regression coverage.
- [x] Reuse resolved ordinary proc shapes, retaining their logical output ports,
   and share parameter-attribute validation. Keep host representability checks
   specific to exported controls.
- [x] Add a cohesive entry adapter in semantic processor lowering. Prepare host
   delegate exports after graph/task expansion and before delegate preparation.
   Connect the remaining host declarations and forwarding using resolved proc
   shapes. Finalize parameter-refresh scheduling after proc-call rewriting.
   Reuse ordinary construction, calls, stores, messages, and resource forwarding.
- [x] Preserve canonical interface metadata, compile contexts, generated-helper
   ownership, and authored instance-state provenance.
- [x] Update language/architecture documentation and metadata completion. Verify
   captured helpers, private params, arrays, hook ordering/counts, events,
   delegates, buffers, reinitialization, event-only entries, oversampling, and
   segmented processing through existing compiler/runtime interfaces.

## Validation

Verified with the complete library suites: 281 frontend, 1,131 semantics,
283 LSP, and 16 browser-compiler tests. The native examples suite passes 830
tests with one ignored. Native LLVM and Binaryen agree across 52 scenarios in
both optimized and unoptimized Wasm configurations, including Main state,
parameter hooks, event forwarding, delegate publication, interface name hygiene,
and exact typed parameter arrays and array message defaults.

Clippy passes with warnings denied for all targets in the affected crates.
Formatting, diff checks, and the 5,000-line limit for changed source files pass.

## Implementation notes

The adapter consumes resolved proc shapes instead of extracting a second source
inference pass. This keeps inferred ports, graph outputs, array slots, types,
and ranges owned by ordinary proc analysis. Delegate exports alone are prepared
earlier because delegate lowering consumes their declarations.

Host-only grid and descriptor constraints remain in host domain completion.
Reusable procs validate authored attributes without acquiring host integer-range
limits or implicit control steps. Delegate dispatch uses private generated names
while descriptors retain public names. Event payload shadowing uses the same
owner-field exclusion as proc-local function parameters in ordinary and nested
proc event lowering.

Host interface bindings use private generated names during semantic analysis;
an internal name map restores authored names when building MIR descriptors.
This keeps module helpers and Main's fields in their ordinary scopes without
adding host or backend behavior.

Parameter shapes retain an explicit array flag, independent of slot count.
Construction, init analysis, nested surfaces, and host projection preserve
one-element arrays. Constructor field initialization infers source element types
from the complete binding environment, retaining normal assignment checks and
integer precision. Exported message array defaults are evaluated through the
shared constant interpreter into the literal form expected by host declarations,
including constant references and calls.
