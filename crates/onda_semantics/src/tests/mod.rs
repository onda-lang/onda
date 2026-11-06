mod aggregate_width;
mod array_bindings;
mod const_arrays;
mod const_dependencies;
mod const_validation;
mod contextual_consts;
mod enums;
mod indexed_proc_events;
mod input_smoothing;
mod main_entry;
mod proc_array_borrowing;
mod processor_projections;
mod processor_storage;
mod runtime_defaults;

fn analyze_source(source: &str) -> TypedProgram {
    analyze(parse_program(source).expect("source should parse"))
        .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"))
}

include!("part_1.rs");
include!("part_2.rs");
include!("part_3.rs");
include!("part_4.rs");
