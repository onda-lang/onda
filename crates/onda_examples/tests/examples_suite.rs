use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, MirCompileOptions, TargetOptLevel,
};
use onda_frontend::{parse_program, parse_program_file, Diagnostic, PrimitiveType};
use onda_runtime::{
    bind_buffer as bind_buffer_raw, bind_input as bind_input_raw, bind_output as bind_output_raw,
    create_instance, prepare_unchecked_process, process_checked, process_checked_segment,
    process_unchecked, process_unchecked_segment, reset_instance_state, set_param_by_index,
    trigger_event_by_index, validate_bindings, validate_buffers, validate_outputs, Instance,
    InstanceConfig, PROCESS_BEGIN_BLOCK, PROCESS_END_BLOCK, PROCESS_FULL_BLOCK,
};
use onda_semantics::{analyze, analyze_with_options, AnalysisOptions};
include!("examples_suite/fixtures.rs");
include!("examples_suite/support.rs");

#[derive(Debug, Clone, Copy)]
struct CompileOptions {
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    opt_level: TargetOptLevel,
}

fn bind_input(
    instance: &mut Instance,
    index: usize,
    ptr: *const u8,
    bytes: usize,
) -> Result<(), Diagnostic> {
    // Test buffers are owned by the calling scope and outlive processing.
    unsafe { bind_input_raw(instance, index, ptr, bytes) }
}

fn bind_output(
    instance: &mut Instance,
    index: usize,
    ptr: *mut u8,
    bytes: usize,
) -> Result<(), Diagnostic> {
    // Test buffers are owned by the calling scope and outlive processing.
    unsafe { bind_output_raw(instance, index, ptr, bytes) }
}

#[allow(clippy::too_many_arguments)]
fn bind_buffer(
    instance: &mut Instance,
    index: usize,
    ptr: *mut u8,
    frames: usize,
    channels: usize,
    sample_rate_hz: f32,
    elem_ty: PrimitiveType,
) -> Result<(), Diagnostic> {
    // Test buffers are owned by the calling scope and outlive processing.
    unsafe {
        bind_buffer_raw(
            instance,
            index,
            ptr,
            frames,
            channels,
            sample_rate_hz,
            elem_ty,
        )
    }
}

#[test]
fn polyphonic_saw_file_example_analyzes() {
    let parsed = parse_program_file(std::path::Path::new(
        "../../examples/larger-patches/polyphonic_saw.onda",
    ))
    .expect("parse should succeed");
    analyze(parsed).expect("analysis should succeed");
}

#[path = "examples_suite/analysis_and_stdlib.rs"]
mod analysis_and_stdlib;
#[path = "examples_suite/execution_and_runtime.rs"]
mod execution_and_runtime;
#[path = "examples_suite/generic_defs.rs"]
mod generic_defs;
#[path = "examples_suite/language_core.rs"]
mod language_core;
#[path = "examples_suite/proc_local_defs.rs"]
mod proc_local_defs;
#[path = "examples_suite/slices_and_ports.rs"]
mod slices_and_ports;
#[path = "examples_suite/tuples.rs"]
mod tuples;
