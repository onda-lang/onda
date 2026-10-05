mod array_bindings;
mod const_arrays;
mod const_dependencies;
mod const_validation;
mod runtime_defaults;

fn analyze_source(source: &str) -> TypedProgram {
    analyze(parse_program(source).expect("source should parse"))
        .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"))
}

include!("part_1.rs");
include!("part_2.rs");
include!("part_3.rs");
include!("part_4.rs");
