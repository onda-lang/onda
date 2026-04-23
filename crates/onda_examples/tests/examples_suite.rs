use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use onda_codegen_llvm::{CompileOptions, ExecutionBackend, TargetOptLevel};
use onda_frontend::{parse_program, parse_program_file, Diagnostic, PrimitiveType};
use onda_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, process_checked, process_unchecked,
    reset_instance_state, set_param_by_index, trigger_event_by_index, validate_bindings,
    validate_buffers, validate_outputs, InstanceConfig,
};
use onda_semantics::{analyze, analyze_with_options, AnalysisOptions};
include!("examples_suite/fixtures.rs");
include!("examples_suite/support.rs");

#[test]
fn polyphonic_saw_file_example_analyzes() {
    let parsed = parse_program_file(std::path::Path::new("../../examples/polyphonic_saw.onda"))
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
