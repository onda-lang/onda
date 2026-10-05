use super::*;

#[test]
fn const_and_runtime_aliases_preserve_immutable_sources_without_evaluating_them() {
    for initializer in ["Table", "Table[:]"] {
        for write in ["alias[0] = 10", "alias = [10, 11]"] {
            for prefix in ["", "const "] {
                let source = format!(
                    "const Table: i32[2] = [1 / 0, 9]\n{prefix}def unused() -> i32:\n  alias = {initializer}\n  {write}\n  return 0\nsample:\n  out1 = 0.0\n"
                );
                let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
                assert!(
                    errors
                        .iter()
                        .all(|error| !error.message.contains("division by zero")),
                    "{source}\n{errors:?}"
                );
            }
        }
    }
    for parameter in ["i32[2]", "i32[]", "[]"] {
        for prefix in ["", "const "] {
            let source = format!(
                "const Table: i32[2] = [1 / 0, 9]\n{prefix}def change(xs: {parameter}) -> i32:\n  alias = xs\n  alias[0] = 10\n  return 0\n{prefix}def forward(xs: {parameter}) -> i32:\n  return change(xs)\n{prefix}def unused() -> i32:\n  return forward(Table[:])\nsample:\n  out1 = 0.0\n"
            );
            let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
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
        let source = format!(
            "const Table: i32[2] = [1 / 0, 9]\nconst def change(xs: {parameter} = Table) -> i32:\n  alias = xs\n  alias[0] = 10\n  return 0\nsample:\n  out1 = 0.0\n"
        );
        let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
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

#[test]
fn const_and_runtime_array_bindings_share_literal_element_defaults() {
    for (values, ty) in [
        ("[7, 9]", "i32"),
        ("[4294967297, 9]", "i64"),
        ("[0.5, 1.0]", "f32"),
        ("[f64(0.5), 1.0]", "f64"),
        ("[true, false]", "bool"),
        ("[1 + 2, 9]", "i32"),
        ("[sin(0.0), 1.0]", "f32"),
    ] {
        for prefix in ["", "const "] {
            let source = format!(
                "{prefix}def read() -> {ty}:\n  values = {values}\n  return values[0]\nsample:\n  out1 = f32(read())\n"
            );
            lower_program_to_optimized_mir(&analyze_source(&source)).unwrap();
        }
    }
}

#[test]
fn const_and_runtime_array_bindings_accept_the_same_initializer_forms() {
    let declarations =
        "const Table: i32[3] = [7, 9, 11]\nconst def build() -> i32[2]:\n  return [7, 9]\n";
    for initializer in ["[7, 9]", "Table", "Table[1:]", "build()"] {
        for prefix in ["", "const "] {
            let source = format!(
                "{declarations}{prefix}def read() -> i32:\n  values = {initializer}\n  alias = values\n  return alias[0] + alias.len()\nsample:\n  out1 = f32(read())\n"
            );
            lower_program_to_optimized_mir(&analyze_source(&source)).unwrap();
        }
    }
}

#[test]
fn inferred_const_array_bindings_preserve_lazy_payloads() {
    let declarations =
        "const Broken: i32[3] = [1 / 0, 0, 0]\nconst def build() -> i32[3]:\n  return Broken\n";
    for initializer in ["Broken", "Broken[1:]", "build()"] {
        let source = format!(
            "{declarations}const def size() -> i32:\n  values = {initializer}\n  alias = values\n  return alias.len()\nsample:\n  out1 = f32(size())\n"
        );
        let typed = analyze_source(&source);
        assert!(typed.const_arrays.is_empty(), "{source}");
        assert!(lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .is_empty());
    }
}

#[test]
fn inferred_const_array_bindings_defer_unknown_slice_element_types() {
    for initializer in ["xs", "xs[:]", "[xs[0]]"] {
        let declarations = format!(
            "const def first(xs: []) -> i32:\n  values = {initializer}\n  alias = values\n  return alias[0]\n"
        );
        analyze_source(&format!("{declarations}sample:\n  out1 = 0.0\n"));
        lower_program_to_optimized_mir(&analyze_source(&format!(
            "{declarations}sample:\n  out1 = f32(first([7, 9]))\n"
        )))
        .unwrap();
        for values in ["[true, false]", "[f64(7.0), 9.0]"] {
            let source = format!(
                "{declarations}const Unused: i32[1] = [first({values})]\nsample:\n  out1 = 0.0\n"
            );
            let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("cannot assign")),
                "{source}\n{errors:?}"
            );
        }
    }
}

#[test]
fn const_and_runtime_array_bindings_reject_invalid_literal_elements() {
    for (values, diagnostic) in [
        ("[]", "cannot be empty"),
        ("[7, true]", "cannot assign"),
        ("[0.5, true]", "cannot assign"),
        ("[7, Missing]", "Missing"),
    ] {
        for prefix in ["", "const "] {
            let source = format!(
                "{prefix}def read() -> i32:\n  values = {values}\n  return 0\nsample:\n  out1 = 0.0\n"
            );
            let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains(diagnostic)),
                "{source}\n{errors:?}"
            );
        }
    }
}

#[test]
fn constructor_initializer_shapes_use_inferred_binding_metadata() {
    for parameter in ["xs: i32[]", "xs: []"] {
        let source = format!(
            "const Broken: i32[3] = [1 / 0, 0, 0]\nconst def size({parameter}) -> i32:\n  alias = xs\n  values: i32[alias.len()] = [7, 9]\n  return values.len()\nconst Unused: i32[1] = [size(Broken)]\nsample:\n  out1 = 0.0\n"
        );
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("expects 3 elements, got 2")),
            "{source}\n{errors:?}"
        );
        assert!(errors
            .iter()
            .all(|error| !error.message.contains("division by zero")));
    }
}

#[test]
fn unused_const_def_shapes_use_eager_scalar_metadata_without_reading_elements() {
    for (declarations, dimension) in [
        ("const N: i32 = 2\n", "N"),
        ("const N: i64 = 2\n", "N"),
        ("const N: f64 = 2.0\n", "i32(N)"),
        ("const N: bool = true\n", "i32(N) + 1"),
        ("const M: i32 = 1\nconst N: i32 = M + 1\n", "N"),
        ("const N: i32 = 2147483647 + 2147483647 + 4\n", "N"),
    ] {
        for (body, diagnostic) in [
            (
                format!("  xs: i32[{dimension}] = [1 / 0, 0, 0]\n  return xs\n"),
                "expects 2 elements, got 3",
            ),
            (
                format!("  xs: i32[{dimension}] = Broken\n  return xs\n"),
                "expects array length 2, got 3",
            ),
            (
                format!("  xs: i32[{dimension}]\n  return xs\n"),
                "expects array length 3, got 2",
            ),
            (
                format!("  xs: i32[{dimension}] = 0\n  return xs\n"),
                "expects array length 3, got 2",
            ),
            (
                format!("  xs: i32[3] = Broken[:{dimension}]\n  return xs\n"),
                "expects array length 3, got 2",
            ),
        ] {
            let source = format!(
                "{declarations}const Broken: i32[3] = [1 / 0, 0, 0]\nconst def make() -> i32[3]:\n{body}sample:\n  out1 = 0.0\n"
            );
            let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains(diagnostic)),
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
fn const_shape_scalar_metadata_respects_local_shadowing_and_deferred_calls() {
    for (params, body) in [
        ("N: i32", "  xs: i32[N] = [1 / 0, 0, 0]\n"),
        ("", "  N: i32 = 3\n  xs: i32[N] = [1 / 0, 0, 0]\n"),
        ("", "  N = [0, 0, 0]\n  xs: i32[N.len()] = Broken\n"),
        ("N: i32[]", "  xs: i32[N.len()] = Broken\n"),
        ("", "  xs: i32[size()] = [1 / 0, 0, 0]\n"),
    ] {
        let source = format!(
            "const N: i32 = 2\nconst Broken: i32[3] = [1 / 0, 0, 0]\nconst def size() -> i32:\n  return 1 / 0\nconst def make({params}) -> i32[3]:\n{body}  return xs\nsample:\n  out1 = 0.0\n"
        );
        let typed = analyze_source(&source);
        assert!(typed.const_arrays.is_empty(), "{source}");
        assert!(
            lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty(),
            "{source}"
        );
    }
}

#[test]
fn const_shape_scalar_metadata_follows_namespace_imports_and_specializations() {
    for source in [
        "const N: i32 = 2\nnamespace Library:\n  use N as Count\n  const def make() -> i32[3]:\n    xs: i32[Count] = [1 / 0, 0, 0]\n    return xs\nsample:\n  out1 = 0.0\n",
        "const N: i32 = 2\nnamespace Outer:\n  use N as Count\n  namespace Inner:\n    const def make() -> i32[3]:\n      xs: i32[Count] = [1 / 0, 0, 0]\n      return xs\nsample:\n  out1 = 0.0\n",
        "namespace Library<P = 1>:\n  const Count: i32 = P + 1\n  const def make() -> i32[3]:\n    xs: i32[Count] = [1 / 0, 0, 0]\n    return xs\nsample:\n  out1 = f32(Library<1>::Count)\n",
    ] {
        let errors = analyze(parse_program(source).unwrap()).expect_err(source);
        assert!(errors.iter().any(|error| error.message.contains("expects 2 elements, got 3")), "{source}\n{errors:?}");
        assert!(errors.iter().all(|error| !error.message.contains("division by zero")), "{source}\n{errors:?}");
    }
    analyze_source("const P: i32 = 2\nnamespace Library<P = 3>:\n  const def make() -> i32[3]:\n    xs: i32[P] = [1 / 0, 0, 0]\n    return xs\nsample:\n  out1 = 0.0\n");
}

#[test]
fn const_shape_scalar_metadata_keeps_declaration_compile_options() {
    let source = "const N: i32 = BLOCK_SIZE\nconst def make() -> i32[3]:\n  xs: i32[N] = [1 / 0, 0, 0]\n  return xs\nsample 4:\n  out1 = 0.0\n";
    let typed = analyze_with_options(
        parse_program(source).unwrap(),
        AnalysisOptions {
            block_size: 3,
            ..AnalysisOptions::default()
        },
    )
    .unwrap();
    assert!(typed.const_arrays.is_empty());
    assert!(lower_program_to_optimized_mir(&typed)
        .unwrap()
        .const_data
        .is_empty());
    let errors = analyze_with_options(
        parse_program(source).unwrap(),
        AnalysisOptions {
            block_size: 2,
            ..AnalysisOptions::default()
        },
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expects 2 elements, got 3")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !error.message.contains("division by zero")),
        "{errors:?}"
    );
}

#[test]
fn function_parameter_names_are_checked_in_every_declaration_scope() {
    let mut cases = vec![
        (
            "x: i32, x: i32".to_owned(),
            "duplicate function parameter 'x'".to_owned(),
        ),
        (
            "x: i32, x: i64".to_owned(),
            "duplicate function parameter 'x'".to_owned(),
        ),
        (
            "x: i32, x: i32[1]".to_owned(),
            "duplicate function parameter 'x'".to_owned(),
        ),
    ];
    cases.extend(builtin_constant_names().map(|name| {
        (
            format!("{name}: i32"),
            format!("function parameter '{name}'"),
        )
    }));
    for (params, diagnostic) in cases {
        for kind in ["def", "const def"] {
            for (namespace, call) in [
                (None, "f(7)"),
                (Some("N"), "N::f(7)"),
                (Some("N<P = 2>"), "N<2>::f(7)"),
            ] {
                let definition = format!("{kind} f({params}) -> i32:\n  return Broken[0]\n");
                let declaration = namespace.map_or_else(
                    || definition.clone(),
                    |namespace| {
                        format!(
                            "namespace {namespace}:\n{}",
                            definition
                                .lines()
                                .map(|line| format!("  {line}\n"))
                                .collect::<String>()
                        )
                    },
                );
                for output in ["7", call] {
                    let source = format!("const Broken: i32[1] = [1 / 0]\n{declaration}sample:\n  out1 = f32({output})\n");
                    let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
                    assert!(
                        errors
                            .iter()
                            .any(|error| error.message.contains(&diagnostic)),
                        "{source}\n{errors:?}"
                    );
                    assert!(
                        errors
                            .iter()
                            .filter(|error| error.message.contains(&diagnostic))
                            .all(|error| error.line > 0 && error.column > 0),
                        "{source}\n{errors:?}"
                    );
                    assert!(
                        !errors
                            .iter()
                            .any(|error| error.message.contains("division by zero")),
                        "{source}\n{errors:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn const_array_length_metadata_checks_concrete_namespace_limits() {
    let library = "namespace Library<Size = 2>:\n  const def build() -> i32[i64(Size) * 2 - 1]:\n    values: i32[i64(Size) * 2 - 1]\n    values[0] = 1 / 0\n    return values\n  const Table = build()\n  const Length: i32 = Table.len()\n  const def length() -> i32:\n    copy: i32[Table.len()] = Table\n    return copy.len()\n";
    for parameter in [2, 1073741824, 1073741825, i32::MAX] {
        let size = i64::from(parameter) * 2 - 1;
        let source = format!("{library}sample:\n  out1 = f32(Library<{parameter}>::Length == {size} && Library<{parameter}>::length() == {size})\n");
        let result = analyze(parse_program(&source).unwrap());
        if size <= i64::from(i32::MAX) {
            let typed = result.unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
            assert!(typed.const_arrays.is_empty());
            assert!(typed.defs.is_empty());
            assert!(lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty());
        } else {
            let errors = result.expect_err(&source);
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("i32::MAX")),
                "{source}\n{errors:?}"
            );
            assert!(
                !errors
                    .iter()
                    .any(|error| error.message.contains("division by zero")),
                "{source}\n{errors:?}"
            );
        }
    }
}

#[test]
fn const_def_dependent_array_sizes_are_checked_before_payload_evaluation() {
    for size in [2147483648_i64, 4294967296] {
        for declaration in ["values: i32[n]", "values: i32[n] = Table"] {
            let source = format!("const Table: i32[1] = [1 / 0]\nconst def length(n: i64) -> i32:\n  {declaration}\n  return values.len()\nsample:\n  out1 = f32(length(i64({size})))\n");
            let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("i32::MAX")),
                "{source}\n{errors:?}"
            );
            assert!(
                !errors
                    .iter()
                    .any(|error| error.message.contains("division by zero")),
                "{source}\n{errors:?}"
            );
        }
    }
}

#[test]
fn const_metadata_rejects_oversized_array_dimensions_without_evaluating_payloads() {
    for size in [2147483648_i64, 4294967296] {
        for declaration in [
            format!("const Table: i32[{size}] = [1 / 0]\n"),
            format!("const def build() -> i32[{size}]:\n  values: i32[{size}]\n  values[0] = 1 / 0\n  return values\nconst Table = build()\n"),
            format!("const def length(xs: i32[{size}]) -> i32:\n  return xs.len()\n"),
            format!("const def length() -> i32:\n  values: i32[{size}]\n  return values.len()\n"),
        ] {
            for namespace in [None, Some("N"), Some("N<P = 2>")] {
                let declaration = namespace.map_or_else(
                    || declaration.clone(),
                    |namespace| {
                        format!(
                            "namespace {namespace}:\n{}",
                            declaration
                                .lines()
                                .map(|line| format!("  {line}\n"))
                                .collect::<String>()
                        )
                    },
                );
                let source = format!("{declaration}sample:\n  out1 = 0.0\n");
                let errors = analyze(parse_program(&source).unwrap()).expect_err(&source);
                assert!(
                    errors.iter().any(|error| error.message.contains("i32::MAX")),
                    "{source}\n{errors:?}"
                );
                assert!(
                    errors.iter().filter(|error| error.message.contains("i32::MAX"))
                        .all(|error| error.line > 0 && error.column > 0),
                    "{source}\n{errors:?}"
                );
                assert!(
                    !errors.iter().any(|error| error.message.contains("division by zero")),
                    "{source}\n{errors:?}"
                );
            }
        }
    }
}

#[test]
fn const_metadata_rejects_invalid_scalar_expressions_without_evaluating_values() {
    for (expression, diagnostic) in [
        ("true + 1", "requires numeric operands"),
        ("i32(true + 1)", "requires numeric operands"),
        ("f32(1.0) & 2", "requires integer operands"),
        ("abs(true)", "requires numeric argument"),
        ("!1", "requires bool type"),
        ("true < false", "comparison"),
        ("true == 1", "comparison"),
        ("(true + 1) == 0", "requires numeric operands"),
        ("false && (abs(true) > 0)", "requires numeric argument"),
        ("accept(true + 1)", "requires numeric operands"),
        ("Table[true + 1]", "requires numeric operands"),
    ] {
        let declarations = "const Table: i32[1] = [1 / 0]\nconst def accept(x: i32) -> i32:\n  return x\nconst def pack(x: f32) -> f32[1]:\n  return [x]\nconst def copy(xs: []) -> f32[1]:\n  return [f32(xs[0])]\n";
        for declaration in [
            format!("const Bad = {expression}\n"),
            format!("const Bad: f32[1] = [f32({expression})]\n"),
            format!("const Bad: f32[1] = pack(f32({expression}))\n"),
            format!("const Bad: f32[1] = copy([f32({expression})])\n"),
            format!("const def bad() -> i32:\n  value = {expression}\n  return 7\n"),
            format!("const def bad() -> f32[1]:\n  return [f32({expression})]\n"),
        ] {
            for template in [false, true] {
                let declaration = if template {
                    format!(
                        "namespace N<P = 2>:\n{}",
                        declaration
                            .lines()
                            .map(|line| format!("  {line}\n"))
                            .collect::<String>()
                    )
                } else {
                    declaration.clone()
                };
                let source = format!("{declarations}{declaration}sample:\n  out1 = 7.0\n");
                let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
                assert!(
                    errors
                        .iter()
                        .any(|error| error.message.contains(diagnostic)),
                    "{source}\n{errors:?}"
                );
                assert!(
                    errors
                        .iter()
                        .filter(|error| error.message.contains(diagnostic))
                        .all(|error| error.line > 0 && error.column > 0),
                    "{source}\n{errors:?}"
                );
                assert!(
                    !errors
                        .iter()
                        .any(|error| error.message.contains("division by zero")),
                    "{source}\n{errors:?}"
                );
            }
        }
    }
}

#[test]
fn invalid_const_body_types_are_rejected_before_interpretation() {
    for binding in ["value"] {
        for (parameter, expression, call) in [
            ("", "true + 1", "bad()"),
            ("xs: []", "xs[0] + 1", "bad([true])"),
        ] {
            let declaration = format!(
                "const def bad({parameter}) -> i32:\n  {binding} = {expression}\n  return 7\n"
            );
            for usage in [
                format!("const Unused = {call}\nsample:\n  out1 = 7.0\n"),
                format!("sample:\n  out1 = f32({call})\n"),
            ] {
                let source = format!("{declaration}{usage}");
                let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
                assert!(
                    errors
                        .iter()
                        .any(|error| error.message.contains("requires numeric operands")),
                    "{source}\n{errors:?}"
                );
            }
        }
    }
    analyze_source("const def first(xs: []) -> i32:\n  value = xs[0] + 1\n  selected = value\n  return i32(selected)\nconst Unused = [first([1 / 0])]\nsample:\n  out1 = f32(first([6]))\n");
}

#[test]
fn template_scalar_constants_share_concrete_type_inference() {
    for (scalar, scalar_ty, array_ty) in [
        ("7", "i64", "i64"),
        ("16777217.0", "f64", "f32"),
        ("i32(7)", "i32", "i32"),
        ("f32(7.0)", "f32", "f32"),
    ] {
        let declarations = format!(
            "  const C = {scalar}\n  const A = [C]\n  const def read() -> {scalar_ty}:\n    return C\n  const def values() -> {array_ty}[1]:\n    return A\n  const def first(xs: {array_ty}[]) -> {array_ty}:\n    return xs[0]\n  const Selected = first(A)\n"
        );
        for (namespace, prefix) in [("N", "N"), ("N<X = 1>", "N<1>")] {
            for usage in ["out1 = 0.0".to_owned(), format!("values = {prefix}::values()\n  out1 = f32({prefix}::Selected + values[0] + {prefix}::read())")] {
                let typed = analyze_source(&format!("namespace {namespace}:\n{declarations}sample:\n  {usage}\n"));
                lower_program_to_optimized_mir(&typed).unwrap();
            }
        }
    }
}

#[test]
fn const_defs_share_ordinary_assignment_binding_checks() {
    for (parameter, body, call, diagnostic) in [
        (
            "",
            "x = 1\nx = make()",
            "read()",
            "conflicts with an existing binding",
        ),
        (
            "x: i32",
            "x = make()",
            "read(1)",
            "conflicts with an existing binding",
        ),
        (
            "xs: []",
            "x = xs[0]\nx = make()",
            "read([1])",
            "conflicts with an existing binding",
        ),
        (
            "",
            "x: i32 = 1\nx: f64 = 2.0",
            "read()",
            "only allowed on first assignment",
        ),
        (
            "x: i32",
            "x: i32 = 2",
            "read(1)",
            "only allowed on first assignment",
        ),
        (
            "",
            "x: i32[1] = [1]\nx: i32[2] = [2, 3]",
            "read()",
            "must introduce a new name",
        ),
        (
            "",
            "x: i32[1] = [1]\nx: i32[1] = make()",
            "read()",
            "must introduce a new name",
        ),
        (
            "x: i32[1]",
            "x: i32[1] = make()",
            "read([1])",
            "must introduce a new name",
        ),
        (
            "",
            "x: i32 = 1\nx: i32[1] = make()",
            "read()",
            "conflicts with an existing binding",
        ),
    ] {
        let body = body
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect::<String>();
        for prefix in ["", "const "] {
            let definitions = format!("{prefix}def make() -> i32[1]:\n  return Broken\n{prefix}def read({parameter}) -> i32:\n{body}  return 0\n");
            for namespace in [false, true] {
                let (definitions, call) = if namespace {
                    (
                        format!(
                            "namespace N<P = 2>:\n{}",
                            definitions
                                .lines()
                                .map(|line| format!("  {line}\n"))
                                .collect::<String>()
                        ),
                        format!("N<2>::{call}"),
                    )
                } else {
                    (definitions.clone(), call.to_owned())
                };
                for used in [false, true] {
                    if !used && prefix.is_empty() {
                        continue;
                    }
                    let usage = if used {
                        format!("out1 = f32({call})")
                    } else {
                        "out1 = 0.0".to_owned()
                    };
                    let source = format!(
                        "const Broken: i32[1] = [1 / 0]\n{definitions}sample:\n  {usage}\n"
                    );
                    let errors = analyze(parse_program(&source).unwrap())
                        .err()
                        .unwrap_or_else(|| panic!("expected a binding error:\n{source}"));
                    assert!(
                        errors
                            .iter()
                            .any(|error| error.message.contains(diagnostic)),
                        "{source}\n{errors:?}"
                    );
                    assert!(
                        errors
                            .iter()
                            .filter(|error| error.message.contains(diagnostic))
                            .all(|error| error.line > 0 && error.column > 0),
                        "{source}\n{errors:?}"
                    );
                    assert!(
                        !errors
                            .iter()
                            .any(|error| error.message.contains("division by zero")),
                        "{source}\n{errors:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn const_assignment_checks_preserve_replacements_and_lexical_shadowing() {
    for (parameter, body, call) in [
        ("", "x = make()\nx = [9]\nreturn x[0]", "read()"),
        ("", "x: i32[1] = [1]\nx = make()\nreturn x[0]", "read()"),
        ("", "x: i32 = 1\nx = 9\nreturn x", "read()"),
        ("x: i32", "x = 9\nreturn x", "read(1)"),
        ("xs: []", "x = xs[0]\nx = xs[0]\nreturn i32(x)", "read([1])"),
        (
            "",
            "if true:\n  x: i32[1] = make()\n  x = [9]\n  return x[0]\nelse:\n  return 0",
            "read()",
        ),
        ("Hidden: i32", "return Hidden", "read(9)"),
        (
            "",
            "x = 0\nfor Hidden in 0..2:\n  x += Hidden\nreturn x",
            "read()",
        ),
    ] {
        let body = body
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect::<String>();
        let definitions = format!("const Hidden: i32[1] = [1 / 0]\nconst def make() -> i32[1]:\n  return [9]\nconst def read({parameter}) -> i32:\n{body}");
        for usage in ["out1 = 0.0".to_owned(), format!("out1 = f32({call})")] {
            let source = format!("{definitions}sample:\n  {usage}\n");
            let typed = analyze_source(&source);
            assert!(typed.const_arrays.is_empty(), "{source}");
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }
}

#[test]
fn const_def_shadowing_keeps_global_and_read_only_writes_invalid() {
    for (declarations, definition, diagnostic) in [
        (
            "namespace Globals:\n  const Value: i32 = 100\n",
            "const def write() -> i32:\n  Globals::Value = 7\n  return 7\n",
            "cannot assign to constant",
        ),
        (
            "namespace Globals:\n  const Value: i32[1] = [1 / 0]\n",
            "const def write() -> i32:\n  Globals::Value = [7]\n  return 7\n",
            "cannot assign to constant",
        ),
        (
            "const Value: i32[1] = [1 / 0]\n",
            "const def write() -> i32:\n  Value[0] = 7\n  return Value[0]\n",
            "cannot write const array",
        ),
        (
            "const Value: i32[1] = [1 / 0]\n",
            "const def write() -> i32:\n  alias = Value\n  alias[0] = 7\n  return alias[0]\n",
            "immutable array alias",
        ),
        (
            "const Value: i32 = 100\n",
            "const def write() -> i32:\n  for Value in 0..1:\n    Value = 7\n  return 7\n",
            "cannot assign to loop variable",
        ),
    ] {
        let source = format!("{declarations}{definition}sample:\n  out1 = 0.0\n");
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains(diagnostic)),
            "{source}\n{errors:?}"
        );
        assert!(
            !errors
                .iter()
                .any(|error| error.message.contains("division by zero")),
            "{source}\n{errors:?}"
        );
    }
}

#[test]
fn const_defs_share_ordinary_branch_binding_checks() {
    for (then_binding, else_binding, diagnostic) in [
        (
            "xs: i32[2]",
            "xs: i32[3]",
            "arrays have different element types or fixed lengths",
        ),
        (
            "xs: i32[2]",
            "xs: i64[2]",
            "arrays have different element types or fixed lengths",
        ),
        ("xs: i32[2]", "xs: i32 = 0", "array and scalar"),
        ("xs: i32 = 0", "xs: bool = false", "i32 and bool"),
    ] {
        for prefix in ["", "const "] {
            for usage in ["out1 = 0.0", "out1 = f32(f(true))"] {
                let source = format!("{prefix}def f(b: bool) -> i32:\n  if b:\n    {then_binding}\n  else:\n    {else_binding}\n  return 0\nsample:\n  {usage}\n");
                let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
                assert!(
                    errors
                        .iter()
                        .any(|error| error.message.contains(diagnostic)),
                    "{source}\n{errors:?}"
                );
            }
        }
    }
    analyze_source("const def f(b: bool) -> i32:\n  if b:\n    xs: i32[2]\n    return xs[0]\n  else:\n    xs: i64[3]\n  return i32(xs[0])\nsample:\n  out1 = f32(f(false))\n");
}

#[test]
fn const_branch_array_shapes_check_concrete_namespace_sizes() {
    let template = "namespace N<P = 2>:\n  const def f(b: bool) -> i32:\n    if b:\n      xs: i32[P]\n    else:\n      xs: i32[2]\n    return xs[0]\n";
    let typed = analyze_source(&format!("{template}sample:\n  out1 = f32(N<2>::f(true))\n"));
    assert!(typed.const_arrays.is_empty());
    let errors = analyze(
        parse_program(&format!("{template}sample:\n  out1 = f32(N<3>::f(true))\n")).unwrap(),
    )
    .unwrap_err();
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("arrays have different element types or fixed lengths")),
        "{errors:?}"
    );
}

#[test]
fn const_branch_arrays_retain_initializer_lengths_with_deferred_dimensions() {
    let declarations = "const Size: i32 = 2\nconst Broken: i32[2] = [1 / 0, 0]\nconst def size() -> i32:\n  return 2\nconst def build() -> i32[2]:\n  return [1 / 0, 0]\n";
    for dimension in ["Size", "size()"] {
        for initializer in ["[1 / 0, 0]", "Broken", "Broken[:]", "build()"] {
            for parameter in ["b: bool", "xs: i32[], b: bool"] {
                let call = if parameter.starts_with("xs") {
                    "f([0], true)"
                } else {
                    "f(true)"
                };
                for usage in ["0.0".to_owned(), format!("f32({call})")] {
                    let source = format!("{declarations}const def f({parameter}) -> i32:\n  if b:\n    values: i32[{dimension}] = {initializer}\n  else:\n    values: i32[3] = [0, 0, 0]\n  return values[0]\nsample:\n  out1 = {usage}\n");
                    let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
                    assert!(
                        errors.iter().any(|error| error
                            .message
                            .contains("arrays have different element types or fixed lengths")),
                        "{source}\n{errors:?}"
                    );
                    assert!(
                        errors
                            .iter()
                            .all(|error| !error.message.contains("division by zero")),
                        "shape checking evaluated an initializer: {source}\n{errors:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn dependent_array_sizes_check_initializer_shapes_before_evaluating_payloads() {
    let declarations =
        "const Broken: i32[2] = [1 / 0, 0]\nconst def build() -> i32[2]:\n  return Broken\n";
    for initializer in ["[1 / 0, 0]", "Broken", "Broken[:]", "build()"] {
        let source = format!("{declarations}const def f(n: i32) -> i32:\n  values: i32[n] = {initializer}\n  return values[0]\nsample:\n  out1 = f32(f(3))\n");
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("expected i32[2], got i32[3]")),
            "{source}\n{errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|error| !error.message.contains("division by zero")),
            "shape checking evaluated an initializer: {source}\n{errors:?}"
        );
    }
}

#[test]
fn const_branch_shapes_complete_from_slice_metadata_without_payloads() {
    for parameter in ["xs: i32[]", "xs: []"] {
        let declarations = format!("const Broken: i32[3] = [1 / 0, 0, 0]\nconst def f({parameter}, b: bool) -> i32:\n  if b:\n    values: i32[xs.len()]\n  else:\n    values: i32[2]\n  return values[0]\nconst def forward({parameter}, b: bool) -> i32:\n  return f(xs, b)\n");
        // Unused defs retain unresolved dimensions; concrete call metadata
        // completes the same checks even when the initializer is unused.
        assert!(
            analyze_source(&format!("{declarations}sample:\n  out1 = 0.0\n"))
                .const_arrays
                .is_empty()
        );
        for callee in ["f", "forward"] {
            let source = format!("{declarations}const Unused: i32 = {callee}(Broken, false)\nsample:\n  out1 = 0.0\n");
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors.iter().any(|error| error
                    .message
                    .contains("arrays have different element types or fixed lengths")),
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
fn inferred_const_branch_shapes_do_not_demand_dead_dimensions_or_elements() {
    for initializer in ["[1 / 0, 0]", "Broken", "Broken[:]", "build()"] {
        let source = format!("const Broken: i32[2] = [1 / 0, 0]\nconst def size() -> i32:\n  return Broken[0]\nconst def build() -> i32[2]:\n  return Broken\nconst def f(b: bool) -> i32:\n  if b:\n    values: i32[size()] = {initializer}\n  else:\n    values: i32[2] = [7, 9]\n  return values[0]\nsample:\n  out1 = f32(f(false))\n");
        let typed = analyze_source(&source);
        assert!(typed.const_arrays.is_empty(), "{source}");
        assert!(
            lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty(),
            "{source}"
        );
    }
}

#[test]
fn deferred_const_branch_shapes_validate_each_executed_join() {
    for (then_binding, else_binding, invalid_flag) in [
        ("values: i32[n]", "values: i32[2]", true),
        ("values: i32[2]", "values: i32[n]", false),
        (
            "if b:\n      values: i32[n]\n    else:\n      values: i32[n]",
            "values: i32[2]",
            true,
        ),
    ] {
        let declaration = format!("const def f(n: i32, b: bool) -> i32:\n  if b:\n    {then_binding}\n  else:\n    {else_binding}\n  return values.len()\n");
        for usage in [
            format!("f(3, {invalid_flag})"),
            format!("f(2, {invalid_flag}) + f(3, {invalid_flag})"),
        ] {
            let source = format!("{declaration}sample:\n  out1 = f32({usage})\n");
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("expected i32[2], got i32[3]")),
                "{source}\n{errors:?}"
            );
        }
        // A discarded branch does not demand its dimension. Both executed
        // branches must still satisfy the checked join, on every invocation.
        for usage in [
            "f(2, true) + f(2, false)".to_owned(),
            format!("f(0, {})", !invalid_flag),
        ] {
            let source = format!("{declaration}sample:\n  out1 = f32({usage})\n");
            let typed = analyze_source(&source);
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }

    // Returning branches have no join with the continuing branch.
    let source = "const def f(n: i32, b: bool) -> i32:\n  if b:\n    values: i32[n]\n    return values.len()\n  else:\n    values: i32[2]\n  return values.len()\nsample:\n  out1 = f32(f(3, true) + f(0, false))\n";
    lower_program_to_optimized_mir(&analyze_source(source)).unwrap();
}

#[test]
fn namespace_const_shapes_retain_dependent_initializer_lengths() {
    let source = "namespace N<P = 2>:\n  const Table: i32[P] = [1 / 0, 0]\n  const def f(b: bool) -> i32:\n    if b:\n      values: i32[P] = Table\n    else:\n      values: i32[3] = [0, 0, 0]\n    return values[0]\nsample:\n  out1 = 0.0\n";
    let errors = analyze(parse_program(source).unwrap()).unwrap_err();
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("arrays have different element types or fixed lengths")),
        "{source}\n{errors:?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !error.message.contains("division by zero")),
        "{source}\n{errors:?}"
    );
}

#[test]
fn const_array_writes_check_selector_types_without_evaluating_values() {
    for index in ["true", "false", "1 == 1"] {
        let body = format!("const def write() -> i32:\n  values: i32[2]\n  values[{index}] = 7\n  return values[1]\n");
        for source in [
            body.clone(),
            format!("{body}sample:\n  out1 = f32(write())\n"),
            format!(
                "namespace N<P = 2>:\n{}",
                body.lines()
                    .map(|line| format!("  {line}\n"))
                    .collect::<String>()
            ),
        ] {
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors.iter().any(|error| error
                    .message
                    .contains("array index expression requires numeric type")),
                "{source}\n{errors:?}"
            );
        }
    }
    analyze_source("const def write() -> i32:\n  values: i32[2]\n  values[0.5] = 1 / 0\n  return values[0]\nsample:\n  out1 = 7.0\n");
    for index in ["values", "[0]", "i32[1]([0])"] {
        let source = format!("const def write() -> i32:\n  values: i32[2]\n  values[{index}] = 7\n  return values[0]\n");
        assert!(
            analyze(parse_program(&source).unwrap()).is_err(),
            "{source}"
        );
    }
}

#[test]
fn template_namespace_uses_bind_const_metadata_without_instantiation() {
    let base = "namespace Base:\n  const Value: i32 = 100\n  const Table: i32[2] = [1 / 0, 7]\n  const def read() -> i32:\n    return Table[0]\n  namespace Child:\n    const X: i32 = 7\n";
    for body in [
        "use Base\nconst X: i32 = Value\nconst Copy: i32[] = Table\nconst Y: i32 = read()\n",
        "use Base as B\nconst X: i32 = B::Value\nconst Copy: i32[] = B::Table\nconst Y: i32 = B::read()\n",
        "use Base::Value as V\nuse Base::Table\nuse Base::read\nconst X: i32 = V\nconst Copy: i32[] = Table\nconst Y: i32 = read()\n",
        "use Base\nnamespace Child:\n  const X: i32 = Value\n  const Copy: i32[] = Table\n  const Y: i32 = read()\n",
        "use Base\nconst X: i32 = Child::X + Child::X\nconst Y: i32 = Child::X\n",
        "use Base::Child\nconst Y: i32 = X\nconst Z: i32 = X\n",
    ] {
        let source = format!("{base}namespace N<P = 2>:\n{}sample:\n  out1 = 7.0\n", body.lines().map(|line| format!("  {line}\n")).collect::<String>());
        let typed = analyze_source(&source);
        assert!(typed.const_arrays.is_empty(), "{source}");
        assert!(typed.defs.is_empty(), "{source}");
    }
    let typed = analyze_source("namespace Base<N = 2>:\n  const Table: i32[N] = [1 / 0]\n  const def read() -> i32:\n    return Table[0]\nnamespace N<P = 2>:\n  use Base<P>\n  const Copy: i32[] = Table\n  const X: i32 = read()\nsample:\n  out1 = 7.0\n");
    assert!(typed.const_arrays.is_empty());
    let typed = analyze_source("namespace Sizes:\n  const Count: i32 = 2\nnamespace Base<N = 2>:\n  const Table: i32[N] = [1 / 0]\nnamespace Wrapper<P = 2>:\n  use Sizes\n  use Base<Count> as B\n  const Copy: i32[] = Base<Count>::Table\n  const Alias: i32[] = B::Table\nsample:\n  out1 = 7.0\n");
    assert!(typed.const_arrays.is_empty());
    analyze_source("const Value: bool = true\nnamespace Base:\n  const Value: i32 = 100\nnamespace N<P = 2>:\n  use Base\n  const def shadow(Value: i32) -> i32:\n    Copy = Value\n    return Copy\n  const def local() -> bool:\n    LocalValue = true\n    return LocalValue\n  const def scoped() -> i32:\n    if true:\n      BranchValue = 7\n      return BranchValue\n    else:\n      BranchValue = 9\n      return BranchValue\n");
    analyze_source("namespace Base:\n  const Size: bool = true\nnamespace Wrapper<Size = 2>:\n  use Base\n  const Table: i32[Size] = [1 / 0]\n");
}

#[test]
fn template_namespace_uses_preserve_unknown_names_types_and_ambiguity() {
    let base =
        "namespace Base:\n  const Value: bool = true\nnamespace Other:\n  const Value: i32 = 7\n";
    for body in [
        "use Base\nconst X: i32 = Value\n",
        "use Base\nconst X: i32 = Missing\n",
        "use Base\nuse Other\nconst X = Value\n",
        "use Base::Value\nuse Other::Value\nconst X = Value\n",
    ] {
        let source = format!(
            "{base}namespace N<P = 2>:\n{}",
            body.lines()
                .map(|line| format!("  {line}\n"))
                .collect::<String>()
        );
        assert!(
            analyze(parse_program(&source).unwrap()).is_err(),
            "{source}"
        );
    }
}

#[test]
fn explicit_const_types_reject_incompatible_initializers_even_when_unused() {
    for source in [
        "const X: i32 = [Missing]\n",
        "const X: i32 = [1]\n",
        "const Table: i64[1] = [4294967297]\nconst Copy: i32[] = Table\n",
        "const Table: i64[1] = [4294967297]\nconst Copy: i32[] = Table[0:1]\n",
        "const def build() -> bool[1]:\n  return [true]\nconst Copy: f32[] = build()\n",
        "const Table: i32[2] = [1, 2]\nconst Copy: bool[] = Table\n",
    ] {
        for consumer in ["", "sample:\n  out1 = 0.0\n"] {
            let source = format!("{source}{consumer}");
            assert!(
                analyze(parse_program(&source).unwrap()).is_err(),
                "{source}"
            );
        }
    }
}

#[test]
fn unused_namespace_templates_check_known_types_and_shapes() {
    for body in [
        "const X: i32 = true\n",
        "const X: i32 = [Missing]\n",
        "const X: i32[N] = [true]\n",
        "const X: i32[2] = [1]\n",
        "const X: i32[2.0] = [1]\n",
        "const Table: i32[2] = [1, 2]\nconst X: i32[Table.len()] = [1]\n",
        "const X: i32[true] = [1]\n",
        "const X: i32[] = [false]\n",
        "const X: i32[] = []\n",
        "const X: i32[N] = []\n",
        "const def bad() -> i32:\n  return 0.5\n",
        "const def bad() -> i32[N]:\n  return [true]\n",
        "const def bad() -> i32[N]:\n  return []\n",
        "const def bad() -> i32[2]:\n  return [1]\n",
        "const def bad() -> i32[true]:\n  return [1]\n",
        "const def bad(xs: i32[N]) -> i32:\n  return xs\n",
        "const def bad(xs: i32[N]) -> i32:\n  return true\n",
        "const def bad(xs: bool[N]) -> i32:\n  return xs[0]\n",
        "const def bad(xs: i32[2] = [1]) -> i32:\n  return xs[0]\n",
        "const def bad(xs: i32[N]) -> i32:\n  if true:\n    return xs[0]\n",
        "const Table: i32[N] = [1]\nconst def bad() -> i32:\n  alias = Table\n  alias[0] = 1\n  return alias[0]\n",
        "const Table: i32[N] = [1]\nconst def bad() -> i32:\n  Table[0] = 2\n  return Table[0]\n",
        "const def bad<T>(x: T) -> i32:\n  return 0\n",
        "const def bad() -> i32:\n  return bad()\n",
        "const def bad() -> i32:\n  return LUT::bad()\n",
        "const def first(xs: bool[N]) -> bool:\n  return xs[0]\nconst X: i32 = first([true])\n",
        "const def make() -> i32[N]:\n  return [1]\nconst X: bool[] = make()\n",
        "const def make() -> i32[N]:\n  return [1]\nconst X: i32 = make()\n",
        "const def make() -> i32[N]:\n  return [1]\nconst def bad() -> bool[N]:\n  return make()\n",
        "const def bad() -> i32[N]:\n  values: i32[N] = [true]\n  return values\n",
    ] {
        let indented = body
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect::<String>();
        for wrapper in [
            format!("namespace LUT<N = 2>:\n{indented}"),
            format!(
                "namespace Outer<P = 4>:\n  namespace LUT<N = P>:\n{}",
                indented
                    .lines()
                    .map(|line| format!("  {line}\n"))
                    .collect::<String>()
            ),
        ] {
            assert!(
                analyze(parse_program(&wrapper).unwrap()).is_err(),
                "{wrapper}"
            );
        }
    }
}

#[test]
fn template_type_checks_preserve_dependent_dimensions_and_lexical_metadata() {
    for source in [
        "namespace LUT<N = 2>:\n  const Table: i32[N] = [1 / 0]\n  const def first(xs: i32[N]) -> i32:\n    return xs[0]\n  const def make() -> i32[N]:\n    values: i32[N]\n    values[0] = 1 / 0\n    return values\n  const Copy: i32[] = make()\n  const Value: i32 = first(Copy)\n",
        "namespace LUT<N = 2>:\n  const def make() -> i32[N]:\n    values: i32[N]\n    return values\n  const def copy() -> i32[N]:\n    return make()\n  const Copy = copy()\n",
        "namespace LUT<N = 2>:\n  const def make() -> i32[2.0]:\n    values: i32[2.0]\n    return values\n  const Copy: i32[2.0] = make()\n",
        "const Value: bool = true\nnamespace Outer:\n  const Value: i32 = 100\n  namespace LUT<N = 2>:\n    const X: i32 = Value\n",
        "namespace A<N = 2>:\n  const def make() -> i32[N]:\n    values: i32[N]\n    values[0] = 1 / 0\n    return values\nnamespace B<M = 3>:\n  namespace Alias = A<M>\n  const Copy: i32[] = Alias::make()\n",
        "namespace Outer<N = 2>:\n  namespace Child<M = N>:\n    namespace Leaf:\n      const X: i32 = 7\n  const Y: i32 = Child::Leaf::X\n",
        "namespace A<N = 2>:\n  const def make() -> i32[N]:\n    values: i32[N]\n    return values\nnamespace B<M = 3>:\n  namespace Alias = A<M>\n  namespace Child<P = M>:\n    const Copy: i32[] = Alias::make()\n",
        "namespace Base:\n  const Table: i32[1] = [1 / 0]\n  const def read() -> i32:\n    return Table[0]\nnamespace LUT<N = 2>:\n  use Base::Table as Values\n  use Base::read as read\n  const Copy: i32[] = Values\n  const X: i32 = read()\n",
    ] {
        let typed = analyze(parse_program(source).unwrap()).unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
        assert!(typed.const_arrays.is_empty());
        assert!(typed.defs.is_empty());
    }
    let source = "namespace A<N = 2>:\n  const def make() -> bool[N]:\n    values: bool[N]\n    return values\nnamespace B<M = 3>:\n  const Copy: i32[] = A<M>::make()\n";
    assert!(analyze(parse_program(source).unwrap()).is_err());
}

#[test]
fn dependent_shapes_are_checked_when_a_namespace_is_specialized() {
    let template = "namespace LUT<N = 2>:\n  const def make() -> i32[N]:\n    return [7, 9]\n  const Table: i32[N] = make()\n";
    let unused = analyze(parse_program(template).unwrap()).unwrap();
    assert!(unused.const_arrays.is_empty());
    let source = format!("{template}sample:\n  out1 = f32(LUT<2>::Table[0])\n");
    analyze(parse_program(&source).unwrap()).unwrap();
    let source = format!("{template}sample:\n  out1 = f32(LUT<3>::Table[0])\n");
    let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expects 3 elements")),
        "{errors:?}"
    );
}

#[test]
fn template_const_names_receive_one_diagnostic_and_namespace_arguments_are_checked() {
    for body in [
        "const X: i32 = Missing\n",
        "const def bad() -> i32:\n  return Missing\n",
    ] {
        let source = format!(
            "namespace LUT<N = 2>:\n{}",
            body.lines()
                .map(|line| format!("  {line}\n"))
                .collect::<String>()
        );
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.message.contains("Missing"))
                .count(),
            1,
            "{errors:?}"
        );
    }
    let source = "namespace A<N = 2>:\n  const X: i32 = N\nnamespace B<M = 2>:\n  const X: i32 = A<Missing>::X\n";
    let errors = analyze(parse_program(source).unwrap()).unwrap_err();
    assert!(
        errors.iter().any(|error| error.message.contains("Missing")),
        "{errors:?}"
    );
}

#[test]
fn unused_library_constants_and_helpers_receive_static_diagnostics() {
    let cases = [
        ("const X: i32 = Missing\n", "Missing"),
        ("const X: i32 = i32(Missing)\n", "Missing"),
        ("const X: bool = true || Missing\n", "Missing"),
        ("const X: i32[2] = [1]\n", "expects 2 elements"),
        ("const X: i32[2] = [1, Missing]\n", "Missing"),
        ("const X: i32[1] = [true]\n", "type mismatch"),
        ("const def bad(x: i32) -> i32:\n  return Missing\n", "Missing"),
        ("const def bad(x: i32 = Missing) -> i32:\n  return x\n", "Missing"),
        ("const def bad() -> i32:\n  return 0.5\n", "type mismatch"),
        ("const def bad() -> i32[2]:\n  return [1]\n", "expects 2 elements"),
        ("const def bad(x: i32) -> i32:\n  return x[0]\n", "scalar"),
        ("const def bad(x: i32) -> i32:\n  return x.len()\n", "x.len"),
        ("const def bad(x: i32) -> i32:\n  if true:\n    return 1\n  else:\n    return Missing\n", "Missing"),
        ("const def bad(x: i32) -> i32:\n  if x > 0:\n    return x\n", "not all reachable paths"),
        ("const def bad() -> i32:\n  return bad()\n", "recursive"),
        ("const def bad(x: i32 = bad()) -> i32:\n  return x\n", "recursive"),
        ("const def bad() -> i32:\n  if false:\n    return bad()\n  return 7\n", "recursive"),
        ("const def first() -> i32:\n  return second()\nconst def second() -> i32:\n  return first()\n", "unknown function 'second'"),
        ("const def take(x: i32) -> i32:\n  return x\nconst X: i32 = take()\n", "missing"),
        ("const def take(x: i32) -> i32:\n  return x\nconst X: i32 = take(true)\n", "type mismatch"),
        ("const def take(x: i32[2]) -> i32:\n  return x[0]\nconst X: i32 = take([1])\n", "array length 2"),
        ("const def build() -> i32[1]:\n  return [1]\nconst X: i32 = build()\n", "scalar"),
        ("const def build() -> f32[1]:\n  return [1.0]\nconst X: f32 = build()\n", "scalar"),
        ("const def bad(xs: []) -> i32[1]:\n  return xs[0]\n", "array value"),
        ("const def bad(xs: f32[]) -> i32[1]:\n  return xs\n", "array elements"),
        ("const def bad(xs: f32[]) -> f32:\n  return xs\n", "scalar"),
        ("const def bad(xs: [] = Missing) -> i32:\n  return xs[0]\n", "Missing"),
        ("const def bad(xs: [] = 1) -> i32:\n  return xs[0]\n", "array value"),
    ];
    for (source, expected) in cases {
        let errors = analyze(parse_program(source).unwrap()).expect_err(source);
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "{source}\n{errors:?}"
        );
    }
}

#[test]
fn valid_unused_library_computations_remain_unevaluated() {
    let source = "const Table: i32[1] = [1 / 0]\nconst def generate() -> i32:\n  loop 1000001:\n    value = _\n  return Table[0]\nconst Value: i32[1] = [generate()]\ndef helper() -> i32:\n  Local = Value[0]\n  return Local\n";
    let typed = analyze(parse_program(source).unwrap()).unwrap();
    assert!(typed.const_arrays.is_empty());
    assert!(typed.defs.is_empty());
    let mir = lower_program_to_optimized_mir(&typed).unwrap();
    assert!(mir.const_data.is_empty());
    let used = format!("{source}sample:\n  out1 = f32(helper())\n");
    let errors = analyze(parse_program(&used).unwrap()).unwrap_err();
    assert!(errors
        .iter()
        .any(|error| error.message.contains("loop exceeded")));
}

#[test]
fn const_def_checks_preserve_valid_dependent_types_and_branch_bindings() {
    for source in [
        "const def first(xs: []) -> i32:\n  return xs[0]\nconst X: i32 = first([7])\nsample:\n  out1 = f32(X)\n",
        "const def first(xs: []) -> bool:\n  return xs[0]\nconst X: bool = first([true])\nsample:\n  out1 = f32(X)\n",
        "const def take(xs: i32[2]) -> i32:\n  return xs[0]\nconst def first(xs: []) -> i32:\n  return take(xs)\nconst X: i32 = first([7, 9])\nsample:\n  out1 = f32(X)\n",
        "const def first(flag: bool) -> i32:\n  if flag:\n    return 7\n  else:\n    value = 9\n  return value\nconst X: i32 = first(false)\nsample:\n  out1 = f32(X)\n",
        "const def first(n: i32) -> i32:\n  values: i32[n]\n  values[0] = 7\n  return values[0]\nconst X: i32 = first(2)\nsample:\n  out1 = f32(X)\n",
    ] {
        analyze(parse_program(source).unwrap()).unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
    }
}

#[test]
fn const_untyped_slice_calls_check_concrete_types_without_values() {
    let prefix = "const def first(xs: []) -> i32:\n  return xs[0]\n";
    analyze_source(&format!("{prefix}sample:\n  out1 = 0.0\n"));
    for arg in ["[true]", "[f32(7.0)]", "[f64(7.0)]"] {
        for consumer in [
            format!("const Unused: i32 = first({arg})\nsample:\n  out1 = 0.0\n"),
            format!("sample:\n  out1 = f32(first({arg}))\n"),
        ] {
            let source = format!("{prefix}{consumer}");
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors.iter().any(|e| e.message.contains("cannot assign")),
                "{source}\n{errors:?}"
            );
        }
    }
    for arg in ["[7]", "Table", "Table[:]"] {
        let source =
            format!("const Table: i32[1] = [7]\n{prefix}sample:\n  out1 = f32(first({arg}))\n");
        lower_program_to_optimized_mir(&analyze_source(&source))
            .expect("const calls do not require runtime monomorphization");
    }
    let source = format!("const Table: i32[1] = [1 / 0]\n{prefix}const Unused: i32[1] = [first(Table)]\nsample:\n  out1 = 0.0\n");
    assert!(analyze_source(&source).const_arrays.is_empty());
    let source = "const def first(xs: [] = [true]) -> i32:\n  return xs[0]\nconst Unused: i32 = first()\nsample:\n  out1 = 0.0\n";
    assert!(analyze(parse_program(source).unwrap()).is_err());
}

#[test]
fn const_untyped_slice_checks_follow_nested_calls_and_shapes() {
    let prefix = "const def first(xs: []) -> i32:\n  return xs[0]\nconst def forward(xs: []) -> i32:\n  return first(xs)\n";
    for arg in ["[true]", "[f32(7.0)]"] {
        let source = format!("{prefix}const Unused: i32 = forward({arg})\nsample:\n  out1 = 0.0\n");
        assert!(
            analyze(parse_program(&source).unwrap()).is_err(),
            "{source}"
        );
    }
    let prefix = "const def fixed(xs: i32[2]) -> i32:\n  return xs[0]\nconst def forward(xs: []) -> i32:\n  return fixed(xs)\n";
    for consumer in [
        "const Unused: i32 = forward([7])\nsample:\n  out1 = 0.0\n",
        "sample:\n  out1 = f32(forward([7, 9]))\n  out2 = f32(forward([7]))\n",
    ] {
        let source = format!("{prefix}{consumer}");
        assert!(
            analyze(parse_program(&source).unwrap()).is_err(),
            "{source}"
        );
    }
    let source = format!("{prefix}sample:\n  out1 = f32(forward([7, 9]))\n");
    lower_program_to_optimized_mir(&analyze_source(&source)).unwrap();
    let source = "const def first(xs: []) -> i32:\n  return i32(xs[0])\nsample:\n  out1 = f32(first([true]) + first([f32(7.0)]))\n";
    lower_program_to_optimized_mir(&analyze_source(source)).unwrap();
}

#[test]
fn const_slice_lengths_check_unused_declarations_and_calls_without_payloads() {
    let prefix = "const Table: i32[3] = [1 / 0, 2, 3]\nconst def fixed(xs: i32[2]) -> i32:\n  return xs[0]\nconst def forward(xs: []) -> i32:\n  return fixed(xs)\n";
    for slice in [
        "Table[2:]",
        "Table[:-2]",
        "Table[Table.len() - 1:]",
        "Table[2.0:]",
        "Table[sqrt(4.0):]",
        "Table[min(2, 9):]",
    ] {
        for consumer in [
            format!("const Bad: i32[2] = {slice}"),
            format!("const Bad: i32[1] = [fixed({slice})]"),
            format!("const Bad: i32[1] = [forward({slice})]"),
            format!(
                "const Good: i32[1] = [forward(Table[:2])]\nconst Bad: i32[1] = [forward({slice})]"
            ),
        ] {
            let source = format!("{prefix}{consumer}\nsample:\n  out1 = 0.0\n");
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| e.message.contains("array length 2, got 1")),
                "{source}\n{errors:?}"
            );
            assert!(
                !errors
                    .iter()
                    .any(|e| e.message.contains("division by zero")),
                "{source}\n{errors:?}"
            );
        }
    }
    for slice in [
        "Table[:2]",
        "Table[1:]",
        "Table[-2:]",
        "Table[-99:2]",
        "Table[1:99]",
        "Table[:Table.len() - 1]",
        "Table[1.0:]",
        "Table[-2.0:]",
        "Table[:sqrt(4.0)]",
    ] {
        let source = format!("{prefix}const Copy: i32[2] = {slice}\nconst Unused: i32[1] = [forward({slice})]\nsample:\n  out1 = 0.0\n");
        let typed = analyze_source(&source);
        assert!(typed.const_arrays.is_empty());
        assert!(lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .is_empty());
    }
}

#[test]
fn const_empty_slice_metadata_is_rejected_without_demanding_values() {
    let prefix =
        "const Table: i32[3] = [1 / 0, 2, 3]\nconst def first(xs: []) -> i32:\n  return xs[0]\n";
    for slice in ["Table[2:1]", "Table[3:]", "Table[:0]", "Table[-99:-99]"] {
        let source = format!("{prefix}const Bad: i32 = first({slice})\nsample:\n  out1 = 0.0\n");
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("slice must have positive length")),
            "{source}\n{errors:?}"
        );
        assert!(
            !errors
                .iter()
                .any(|e| e.message.contains("division by zero")),
            "{source}\n{errors:?}"
        );
    }
}
