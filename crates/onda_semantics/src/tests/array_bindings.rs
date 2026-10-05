use super::*;

#[test]
fn array_replacement_preserves_concrete_destination_shape() {
    for declaration in ["i32[1]", "i32[n]"] {
        for alias in ["", "  alias = values[:]\n"] {
            let prefix = format!(
                "const def replace(n: i32) -> i32:\n  values: {declaration}\n{alias}  values = [7, 9]\n  return values.len()\n"
            );
            // The dependent shape must still be checked after a successful
            // call has populated the shared body metadata cache.
            let source = format!("{prefix}sample:\n  out1 = f32(replace(2) + replace(1))\n");
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors.iter().any(|error| {
                    error.message.contains("expected i32[1]")
                        || error.message.contains("expected array length 1, got 2")
                        || error.message.contains("expects 1 elements, got 2")
                }),
                "{source}\n{errors:?}"
            );
            if declaration == "i32[n]" {
                let typed = analyze_source(&format!("{prefix}sample:\n  out1 = f32(replace(2))\n"));
                assert!(typed.const_arrays.is_empty());
                lower_program_to_optimized_mir(&typed).unwrap();
            }
        }
    }
}

#[test]
fn cached_defaults_preserve_array_values_and_reference_permissions() {
    let declarations =
        "const Table: i32[2] = [7, 9]\nconst def make() -> i32[2]:\n  return Table\n";
    for prefix in ["", "const "] {
        for initializer in ["[7, 9]", "make()"] {
            let source = format!(
                "{declarations}{prefix}def change(values: i32[2] = {initializer}) -> i32:\n  original = values[0]\n  values[0] = 11\n  return original\nsample:\n  out1 = f32(change() + change())\n"
            );
            lower_program_to_optimized_mir(&analyze_source(&source)).unwrap();
        }
        for reference in ["Table", "Table[:]"] {
            let source = format!(
                "const Table: i32[2] = [1 / 0, 9]\n{prefix}def change(values: i32[2] = {reference}) -> i32:\n  alias = values\n  alias[0] = 11\n  return alias[0]\nsample:\n  out1 = 0.0\n"
            );
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("immutable array alias")),
                "{source}\n{errors:?}"
            );
            assert!(
                errors
                    .iter()
                    .all(|error| !error.message.contains("division by zero")),
                "{source}\n{errors:?}"
            );
        }
    }
}

#[test]
fn unused_and_overridden_literal_defaults_never_demand_elements() {
    let declarations = "const def element() -> i32:\n  return 1 / 0\n";
    for default in ["[element()]", "[1 / 0]"] {
        let prefix = format!(
            "{declarations}def read(values: i32[1] = {default}) -> i32:\n  return values[0]\n"
        );
        for call in ["7", "read([7])"] {
            let typed = analyze_source(&format!("{prefix}sample:\n  out1 = f32({call})\n"));
            assert!(typed.const_arrays.is_empty());
            assert!(lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty());
        }
        let errors =
            analyze(parse_program(&format!("{prefix}sample:\n  out1 = f32(read())\n")).unwrap())
                .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("division by zero")),
            "{errors:?}"
        );
    }
}

#[test]
fn readonly_defaults_share_payloads_without_array_temporaries() {
    let typed = analyze_source("const def build() -> i32[4096]:\n  values: i32[4096]\n  values[0] = 7\n  return values\ndef read(values: i32[4096] = build()) -> i32:\n  return values[0]\nsample:\n  out1 = f32(read() + read())\n");
    let mir = lower_program_to_optimized_mir(&typed).unwrap();
    assert_eq!(mir.const_data.len(), 1);
    assert!(mir
        .functions
        .iter()
        .all(|function| function.locals.iter().all(|local| {
            !matches!(mir.types[local.ty.index()], onda_mir::Type::Array { .. })
        })));
}
