// Keep the AudioWorklet dependency graph relative-only so the published module can be loaded
// directly. The adapter tests verify these processor ABI values against @onda-lang/processor-abi.
export const PROCESSOR_EXECUTION_OK = 0;
export const PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE = 1;
export const PROCESSOR_EXECUTION_INPUT_REJECTED = 2;
export const PROCESSOR_BEGIN_BLOCK = 1 << 0;
export const PROCESSOR_END_BLOCK = 1 << 1;
export const PROCESSOR_FULL_BLOCK = PROCESSOR_BEGIN_BLOCK | PROCESSOR_END_BLOCK;
export const PROCESSOR_INIT_PRESERVE_PINNED = 0;
export const PROCESSOR_INIT_FULL = 1;
