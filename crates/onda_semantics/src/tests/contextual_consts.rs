use super::*;

fn assert_state_type(typed: &TypedProgram, name: &str, expected: PrimitiveType) {
    let index = typed.state_vars.iter().position(|var| var == name).unwrap();
    assert_eq!(typed.state_types[index], expected, "{name}");
}

#[test]
fn contextual_constants_follow_concrete_generic_arguments_in_either_order() {
    for (ty, expected) in [
        ("i32", PrimitiveType::I32),
        ("i64", PrimitiveType::I64),
        ("f32", PrimitiveType::F32),
        ("f64", PrimitiveType::F64),
    ] {
        for value in if ty.starts_with('i') {
            vec!["64", "64 + 1"]
        } else {
            vec!["1.0000000000000002", "1.0000000000000002 + 0.0"]
        } {
            for (args, overload) in [
                ("value, Alias", ""),
                ("Alias, value", ""),
                (
                    "value, Alias",
                    "def combine(a: bool, b: bool) -> bool:\n  return a && b\n",
                ),
                (
                    "Alias, value",
                    "def combine(a: bool, b: bool) -> bool:\n  return a && b\n",
                ),
            ] {
                let source = format!("const Limit = {value}\nconst Alias = Limit\ndef combine<T>(a: T, b: T) -> T:\n  return a + b\n{overload}init:\n  value: {ty} = 2\n  result = combine({args})\nsample:\n  out1 = f32(result)\n");
                let typed = analyze_source(&source);
                assert_state_type(&typed, "result", expected);
                lower_program_to_optimized_mir(&typed).unwrap();
            }
        }
    }
}

#[test]
fn standalone_contextual_constants_take_ordinary_binding_defaults() {
    let typed = analyze_source("const Small = 64\nconst Large = 4294967297\nconst Float = 1.0000000000000002\nconst Fixed = f64(1.0000000000000002)\ninit:\n  small = Small\n  large = Large\n  float = Float\n  fixed = Fixed\nsample:\n  out1 = f32(small + large + float + fixed)\n");
    assert_state_type(&typed, "small", PrimitiveType::I32);
    assert_state_type(&typed, "large", PrimitiveType::I64);
    assert_state_type(&typed, "float", PrimitiveType::F32);
    assert_state_type(&typed, "fixed", PrimitiveType::F64);
    lower_program_to_optimized_mir(&typed).unwrap();
}

#[test]
fn tuple_assignments_retain_each_checked_component_type() {
    for assignment in [
        "pair = (Small, Large, Float, Fixed)",
        "small, large, float, fixed = (Small, Large, Float, Fixed)",
        "small, _, float, fixed = (Small, Large, Float, Fixed)",
    ] {
        let typed = analyze_source(&format!(
            "const Small = 64\nconst Large = 4294967297\nconst Float = 16777217.0\nconst Fixed = f64(Float)\ndef run() -> i32:\n  {assignment}\n  return 1\nsample:\n  out1 = f32(run())\n"
        ));
        let run = typed.defs.iter().find(|def| def.name == "run").unwrap();
        let Stmt::Assign { decl_ty, .. } = &run.body[0] else {
            panic!("expected tuple assignment");
        };
        assert_eq!(
            decl_ty.as_ref().and_then(DeclType::tuple).unwrap(),
            [
                PrimitiveType::I32,
                PrimitiveType::I64,
                PrimitiveType::F32,
                PrimitiveType::F64
            ]
        );
        lower_program_to_optimized_mir(&typed).unwrap();
    }
}

#[test]
fn tuple_destructuring_checks_each_existing_destination() {
    for (targets, values) in [
        ("value, _", "wide, 0"),
        ("value, fresh", "wide, 0"),
        ("_, value", "0, wide"),
        ("fresh, value", "0, wide"),
    ] {
        for source in [
            format!("def run(wide: i64) -> i32:\n  value: i32 = 0\n  {targets} = ({values})\n  return value\nsample:\n  out1 = f32(run(i64(7)))\n"),
            format!("init:\n  wide: i64 = 7\nsample:\n  value: i32 = 0\n  {targets} = ({values})\n  out1 = f32(value)\n"),
        ] {
            assert_analyze_error_contains(&source, "cannot assign I64 to I32");
        }
    }
}

#[test]
fn tuple_destructuring_checks_constant_range_with_discarded_or_new_targets() {
    for (targets, values) in [
        ("value, _", "Wide, 0"),
        ("value, fresh", "Wide, 0"),
        ("_, value", "0, Wide"),
        ("fresh, value", "0, Wide"),
    ] {
        for declaration in ["const Wide = 2147483648", "const Wide = i64(2147483648)"] {
            for body in [
                format!("def run() -> i32:\n  value: i32 = 0\n  {targets} = ({values})\n  return value\nsample:\n  out1 = f32(run())\n"),
                format!("sample:\n  value: i32 = 0\n  {targets} = ({values})\n  out1 = f32(value)\n"),
                format!("init:\n  value: i32 = 0\n  {targets} = ({values})\nsample:\n  out1 = f32(value)\n"),
            ] {
                let source = format!("{declaration}\n{body}");
                let typed = analyze_source(&source);
                let errors = lower_program_to_optimized_mir(&typed).expect_err(&source);
                assert!(
                    errors.iter().any(|error| error.message.contains("out of range for i32")),
                    "{source}\n{errors:?}"
                );
            }
        }
    }
}

#[test]
fn contextual_constants_follow_binary_builtin_and_assignment_contexts() {
    for (ty, expected) in [
        ("i32", PrimitiveType::I32),
        ("i64", PrimitiveType::I64),
        ("f32", PrimitiveType::F32),
        ("f64", PrimitiveType::F64),
    ] {
        let source = format!("const Limit = 64\nconst Alias = Limit + 1\ninit:\n  value: {ty} = 8\n  direct: {ty} = Alias\n  forward = value + Alias\n  reverse = Alias + value\n  builtin = clamp(value, 1, Alias)\nsample:\n  out1 = f32(direct + forward + reverse + builtin)\n");
        let typed = analyze_source(&source);
        for name in ["direct", "forward", "reverse", "builtin"] {
            assert_state_type(&typed, name, expected);
        }
        lower_program_to_optimized_mir(&typed).unwrap();
    }
}

#[test]
fn primitive_destination_conversions_distinguish_runtime_and_constants() {
    use PrimitiveType::{Bool, F32, F64, I32, I64};
    let destinations = [I32, I64, F32, F64, Bool];
    for (src, value, runtime_allowed, constant_allowed) in [
        (
            I32,
            "7",
            [true, true, true, true, false],
            [true, true, true, true, false],
        ),
        (
            I64,
            "7",
            [false, true, false, true, false],
            [true, true, true, true, false],
        ),
        (
            F32,
            "7.0",
            [false, false, true, true, false],
            [false, false, true, true, false],
        ),
        (
            F64,
            "7.0",
            [false, false, false, true, false],
            [false, false, true, true, false],
        ),
        (
            Bool,
            "true",
            [false, false, false, false, true],
            [false, false, false, false, true],
        ),
    ] {
        for (declaration, allowed) in [
            (
                format!("init:\n  original: {} = {value}\n", src.name()),
                runtime_allowed,
            ),
            (
                format!("const original: {} = {value}\ninit:\n", src.name()),
                constant_allowed,
            ),
        ] {
            for (dst, allowed) in destinations.into_iter().zip(allowed) {
                let source = format!(
                    "{declaration}  converted: {} = original\nsample:\n  out1 = f32(converted)\n",
                    dst.name()
                );
                let result = analyze(parse_program(&source).unwrap());
                if allowed {
                    let typed = result.unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
                    assert_state_type(&typed, "converted", dst);
                    lower_program_to_optimized_mir(&typed).unwrap();
                } else {
                    let errors = result.expect_err(&source);
                    assert!(
                        errors
                            .iter()
                            .any(|error| error.message.contains("cannot assign")),
                        "{source}\n{errors:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn contextual_scalar_overloads_use_the_same_defaults_as_bindings() {
    let typed = analyze_source(
        r#"
const Fraction = 0.5
def select(value: f32) -> f32:
  return value
def select(value: f64) -> f64:
  return value
init:
  builtin_binding = PI
  mixed_binding = 4294967297 + 0.5
  builtin_call = select(PI)
  mixed_call = select(4294967297 + 0.5)
  named_call = select(Fraction)
  fixed_call = select(f64(PI))
sample:
  out1 = f32(builtin_binding + mixed_binding + builtin_call + mixed_call + named_call + fixed_call)
"#,
    );
    for name in [
        "builtin_binding",
        "mixed_binding",
        "builtin_call",
        "mixed_call",
        "named_call",
    ] {
        assert_state_type(&typed, name, PrimitiveType::F32);
    }
    assert_state_type(&typed, "fixed_call", PrimitiveType::F64);
    lower_program_to_optimized_mir(&typed).unwrap();
}

#[test]
fn contextual_defaults_use_literal_requirements_and_evaluated_const_values() {
    let typed = analyze_source(
        "const Folded = 2147483647 + 1\ninit:\n  inline = 2147483647 + 1\n  folded = Folded\n  cancelled = 4294967297 - 4294967296\n  builtin = PI\nsample:\n  out1 = f32(inline + folded + cancelled + builtin)\n",
    );
    assert_state_type(&typed, "inline", PrimitiveType::I32);
    assert_state_type(&typed, "folded", PrimitiveType::I64);
    assert_state_type(&typed, "cancelled", PrimitiveType::I64);
    assert_state_type(&typed, "builtin", PrimitiveType::F32);
    lower_program_to_optimized_mir(&typed).unwrap();
}

#[test]
fn contextual_integer_destinations_check_range_after_width_selection() {
    for destination in ["value: i32 = Huge", "value = combine(i32(0), Huge)"] {
        let source = format!(
            "const Huge = 4294967297\ndef combine<T>(a: T, b: T) -> T:\n  return a + b\ninit:\n  {destination}\nsample:\n  out1 = f32(value)\n"
        );
        let typed = analyze_source(&source);
        assert_state_type(&typed, "value", PrimitiveType::I32);
        let errors = lower_program_to_optimized_mir(&typed).expect_err(&source);
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("out of range for i32")),
            "{source}\n{errors:?}"
        );
    }
}

#[test]
fn contextual_const_defaults_agree_in_namespace_templates_and_instances() {
    for (initializer, ty) in [
        ("64", "i32"),
        ("64 + 1", "i32"),
        ("BS", "i32"),
        ("BS * BS * BS * BS", "i64"),
    ] {
        let typed = analyze_source(&format!("namespace N<X = 1>:\n  const Value = {initializer}\n  const Alias = Value\n  const Values = [Alias]\n  def selected<T>(value: T, extra: T) -> T:\n    return value + extra\nsample:\n  xs = N<1>::Values\n  out1 = f32(xs[0] + N<1>::selected({ty}(1), N<1>::Alias))\n"));
        lower_program_to_optimized_mir(&typed).unwrap();
    }
}

#[test]
fn contextual_builtin_constants_evaluate_wide_before_binding_defaults() {
    for expression in [
        "Count * Count * Count * Count",
        "BS * BS * BS * BS",
        "max(BS, BS) * BS * BS * BS",
    ] {
        let source = format!("const Count = BS\nconst Huge = {expression}\nconst Alias = Huge\ninit:\n  direct = Huge\n  alias = Alias\n  values = [Alias]\nsample:\n  out1 = f32(direct + alias + values[0])\n");
        let typed = analyze_source(&source);
        for name in ["direct", "alias"] {
            assert_state_type(&typed, name, PrimitiveType::I64);
        }
        lower_program_to_optimized_mir(&typed).unwrap();
    }
}

#[test]
fn concrete_constant_types_participate_in_numeric_joins() {
    for declaration in [
        "const Wide: f64 = 1.0000000000000002",
        "const Wide = f64(1.0000000000000002)",
        "const Values: f64[1] = [1.0000000000000002]\nconst Wide = Values[0]",
    ] {
        for expression in [
            "combine(value, Wide)",
            "combine(Wide, value)",
            "value + Wide",
            "Wide + value",
            "max(value, Wide)",
        ] {
            let typed = analyze_source(&format!("{declaration}\ndef combine<T>(a: T, b: T) -> T:\n  return a + b\ninit:\n  value: f32 = 0.0\n  result = {expression}\nsample:\n  out1 = f32(result)\n"));
            assert_state_type(&typed, "result", PrimitiveType::F64);
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }
}

#[test]
fn concrete_constants_convert_at_typed_destinations() {
    for (wide, narrow, value, expected) in [
        ("f64", "f32", "0.25", PrimitiveType::F32),
        ("i64", "i32", "7", PrimitiveType::I32),
        ("i64", "f32", "4611686293305294849", PrimitiveType::F32),
    ] {
        for expression in ["Fixed", "Values[0]", "computed()"] {
            let typed = analyze_source(&format!(
                "const Fixed: {wide} = {value}\nconst Values: {wide}[1] = [Fixed]\nconst def computed() -> {wide}:\n  return Fixed\ndef take(value: {narrow} = {expression}) -> {narrow}:\n  return value\ndef result() -> {narrow}:\n  return {expression}\ninit:\n  inferred = Fixed\n  direct: {narrow} = {expression}\n  explicit = take({expression})\n  default = take()\n  returned = result()\n  values: {narrow}[1] = [{expression}]\nsample:\n  direct = {expression}\n  out1 = f32(direct + explicit + default + returned + values[0])\n"
            ));
            assert_state_type(
                &typed,
                "inferred",
                if wide == "f64" {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::I64
                },
            );
            for name in ["direct", "explicit", "default", "returned"] {
                assert_state_type(&typed, name, expected);
            }
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }
}

#[test]
fn constant_graph_sources_convert_at_each_destination() {
    for (outputs, edge) in [
        ("outs 2", "Values[0] >> { out1, out2 }"),
        ("outs:\n  pair: f32[2]", "Values >> pair"),
    ] {
        let typed = analyze_source(&format!(
            "const Values: f64[2] = [0.25, 0.5]\n{outputs}\ngraph:\n  {edge}\n"
        ));
        lower_program_to_optimized_mir(&typed).unwrap();
    }
}

#[test]
fn delayed_constant_graph_fanout_converts_at_each_destination_in_either_order() {
    for (outputs, source) in [
        ("narrow: f32\n  wide: f64", "Precise"),
        ("narrow: f32[2]\n  wide: f64[2]", "Values"),
        ("narrow: f32[2]\n  wide: f64[3]", "Precise"),
    ] {
        for destinations in ["narrow, wide", "wide, narrow"] {
            let typed = analyze_source(&format!(
                "const Precise: f64 = 16777217.0\nconst Values = [Precise, Precise]\nouts:\n  {outputs}\ngraph:\n  {source} >>[1] {{ {destinations} }}\n"
            ));
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }
}

#[test]
fn delayed_runtime_graph_fanout_evaluates_each_source_component_once() {
    for (inputs, outputs, expression, input_loads, sine_calls) in [
        ("ins 1", "narrow: f32\n  wide: f64", "sin(in1)", 1, 1),
        (
            "ins 1",
            "narrow: f32[1]\n  wide: f64[1]",
            "[sin(in1)]",
            1,
            1,
        ),
        (
            "ins 1",
            "narrow: f32[2]\n  wide: f64[2]",
            "[sin(in1), cos(in1)]",
            2,
            1,
        ),
        ("ins 1", "narrow: f32[2]\n  wide: f64[3]", "sin(in1)", 1, 1),
        ("ins 1", "narrow: f32[2]\n  wide: f32[2]", "sin(in1)", 1, 1),
        (
            "ins:\n  signal: f32[1]",
            "narrow: f32[1]\n  wide: f64[1]",
            "signal",
            1,
            0,
        ),
        (
            "ins:\n  signal: f32[2]",
            "narrow: f32[2]\n  wide: f64[2]",
            "signal",
            2,
            0,
        ),
    ] {
        for destinations in ["narrow, wide", "wide, narrow"] {
            for delay in [1, 19] {
                let typed = analyze_source(&format!(
                    "{inputs}\nouts:\n  {outputs}\ngraph:\n  {expression} >>[{delay}] {{ {destinations} }}\n"
                ));
                let mir = lower_program_to_optimized_mir(&typed)
                    .unwrap()
                    .into_program();
                let dump = onda_mir::format_program(&mir);
                assert_eq!(dump.matches("intrinsic sin(").count(), sine_calls, "{dump}");
                assert_eq!(dump.matches("load_input ").count(), input_loads, "{dump}");
            }
        }
    }
}

#[test]
fn one_element_processor_inputs_keep_array_shape() {
    for argument in ["[Precise]", "values", ""] {
        let typed = analyze_source(&format!(
            "const Precise = 0.25\nproc Value:\n  ins:\n    in1: f64[1] = [Precise]\n  sample:\n    out1 = f32(in1[0])\ninit:\n  value = Value()\n  values: f64[1] = [Precise]\nsample:\n  out1 = value({argument})\n"
        ));
        lower_program_to_optimized_mir(&typed).unwrap();
    }
    let errors = analyze(
        parse_program(
            "proc Value:\n  ins 1\n  sample:\n    out1 = in1\ninit:\n  value = Value()\nsample:\n  out1 = value([0.25])\n",
        )
        .unwrap(),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expects a scalar value")),
        "{errors:?}"
    );
}

#[test]
fn generic_constraint_diagnostics_point_to_the_offending_argument() {
    for (params, bindings, call, message, offending) in [
        (
            "values: T[1], value: T",
            "values: f32[1] = [0.0]\n  wide: f64 = 1.0",
            "add(values, wide)",
            "resolves to f32, but argument has type f64",
            "wide",
        ),
        (
            "value: T, values: T[1]",
            "values: f32[1] = [0.0]\n  wide: f64 = 1.0",
            "add(wide, values)",
            "resolves to f32, but argument has type f64",
            "wide",
        ),
        (
            "first: T[1], second: T[1]",
            "narrow: f32[1] = [0.0]\n  wide: f64[1] = [1.0]",
            "add(narrow, wide)",
            "incompatible exact argument types f32 and f64",
            "wide",
        ),
        (
            "first: T, second: T",
            "value: f32 = 0.0\n  flag: bool = true",
            "add(value, flag)",
            "incompatible argument types f32 and bool",
            "flag",
        ),
    ] {
        let source = format!(
            "def add<T>({params}) -> T:\n  return T(0)\nsample:\n  {bindings}\n  out1 = f32({call})\n"
        );
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        let diagnostic = errors
            .iter()
            .find(|error| error.message.contains(message))
            .unwrap_or_else(|| panic!("{source}\n{errors:?}"));
        let call_line = source.lines().last().unwrap();
        let start = call_line.find(call).unwrap() + call.find(offending).unwrap() + 1;
        assert_eq!(diagnostic.line, source.lines().count(), "{source}");
        assert_eq!(diagnostic.column, start, "{source}");
        assert_eq!(diagnostic.end_column, start + offending.len(), "{source}");
    }
}

#[test]
fn resource_constant_writes_check_integer_range() {
    for source in [
        "init:\n  values: i32[1] = [0]\nsample:\n  write_unsafe(values, 0, VALUE)\n  out1 = f32(values[0])\n",
        "sample:\n  values: i32[1] = [0]\n  write_unsafe(values, 0, VALUE)\n  out1 = f32(values[0])\n",
        "init:\n  values: i32[2] = [0, 0]\nsample:\n  xs = values[:1]\n  xs.write_unsafe(0, VALUE)\n  out1 = f32(xs[0])\n",
        "def write(values: i32[1]):\n  write_unsafe(values, 0, VALUE)\ninit:\n  values: i32[1] = [0]\nsample:\n  write(values)\n  out1 = f32(values[0])\n",
        "def write(values: i32[]):\n  write_unsafe(values, 0, VALUE)\ninit:\n  values: i32[1] = [0]\nsample:\n  write(values)\n  out1 = f32(values[0])\n",
        "outs:\n  values: i32[1]\nsample:\n  write_unsafe(values, 0, VALUE)\n",
        "outs<i32> 1\nsample:\n  write_unsafe(outs, 0, VALUE)\n",
        "kouts:\n  values: i32[1]\nblock:\n  write_unsafe(values, 0, VALUE)\n  sample:\n    out1 = 0.0\n",
        "kouts<i32> 1\nblock:\n  write_unsafe(kouts, 0, VALUE)\n  sample:\n    out1 = 0.0\n",
        "buffers:\n  values: i32\nsample:\n  write_unsafe(values, 0, VALUE)\n  out1 = f32(values[0])\n",
        "buffers:\n  values: i32 {2}\nsample:\n  write_unsafe(values, 0, 0, VALUE)\n  out1 = f32(values[0][0])\n",
        "buffers:\n  values: i32 {2}\nsample:\n  write_unsafe(values[0], 0, VALUE)\n  out1 = f32(values[0][0])\n",
        "def write(values: buffer<i32>):\n  write_unsafe(values, 0, VALUE)\nbuffers:\n  values: i32\nsample:\n  write(values)\n  out1 = f32(values[0])\n",
        "def write(values):\n  write_unsafe(values, 0, 0, VALUE)\nbuffers:\n  values: i32 {2}\nsample:\n  write(values)\n  out1 = f32(values[0][0])\n",
    ] {
        for expression in ["Fixed", "i64(-2147483649)", "i64(2147483647) + 1"] {
            let source = format!(
                "const Fixed: i64 = 2147483648\n{}",
                source.replace("VALUE", expression)
            );
            let typed = analyze_source(&source);
            let errors = lower_program_to_optimized_mir(&typed)
                .expect_err(&format!("out-of-range write should fail:\n{source}"));
            assert!(
                errors.iter().any(|error| error.message.contains("out of range for i32")),
                "{source}\n{errors:?}"
            );
        }
    }
}

#[test]
fn constant_tuple_arguments_and_defaults_convert_at_typed_destinations() {
    let typed = analyze_source(
        "def choose(values: (f32, i32) = (f64(1.0), i64(2))) -> f32:\n  return values[0]\nsample:\n  out1 = choose() + choose((f64(1.0), i64(2)))\n",
    );
    lower_program_to_optimized_mir(&typed).unwrap();
}

#[test]
fn concrete_integer_destinations_check_range_before_conversion() {
    for expression in [
        "Fixed",
        "Values[0]",
        "Fixed + i64(1)",
        "Fixed * i64(true && true)",
    ] {
        let typed = analyze_source(&format!(
            "const Fixed: i64 = 2147483648\nconst Values: i64[1] = [Fixed]\ninit:\n  value: i32 = {expression}\nsample:\n  out1 = f32(value)\n"
        ));
        let errors = lower_program_to_optimized_mir(&typed).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("out of range for i32")),
            "{errors:?}"
        );
    }
}

#[test]
fn runtime_values_still_require_explicit_narrowing() {
    for expression in [
        "value",
        "Values[index]",
        "values[index]",
        "value + f64(0.0)",
    ] {
        let source = format!(
            "const Values: f64[1] = [0.25]\ninit:\n  value: f64 = 0.25\n  index = 0\n  values = Values\nsample:\n  out1 = {expression}\n"
        );
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("cannot assign F64 to F32")),
            "{errors:?}"
        );
    }
}

#[test]
fn contextual_float_constants_preserve_numeric_family_with_integer_peers() {
    for (ty, expected) in [("i32", PrimitiveType::F32), ("i64", PrimitiveType::F64)] {
        for expression in [
            "combine(value, Fraction)",
            "combine(Fraction, value)",
            "value + Fraction",
            "Fraction + value",
            "max(value, Fraction)",
            "min(Fraction, value)",
            "clamp(value, Fraction, 3.0)",
            "pow(value, Fraction)",
            "pow(Fraction, value)",
        ] {
            let typed = analyze_source(&format!("const Fraction = 0.25\ndef combine<T>(a: T, b: T) -> T:\n  return a + b\ninit:\n  value: {ty} = 2\n  result = {expression}\nsample:\n  out1 = f32(result)\n"));
            assert_state_type(&typed, "result", expected);
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }
}

#[test]
fn concrete_integers_and_floats_share_one_promotion_rule() {
    for (integer, float, expected) in [
        ("i32", "f32", PrimitiveType::F32),
        ("i64", "f32", PrimitiveType::F64),
        ("i32", "f64", PrimitiveType::F64),
        ("i64", "f64", PrimitiveType::F64),
    ] {
        let typed = analyze_source(&format!(
            r#"
def combine<T>(a: T, b: T) -> T:
  return a + b
init:
  integer: {integer} = 2
  float: {float} = 0.5
  forward = integer + float
  reverse = float + integer
  generic_forward = combine(integer, float)
  generic_reverse = combine(float, integer)
  minimum = min(integer, float)
  maximum = max(float, integer)
  clamped = clamp(integer, float, float)
  power = pow(integer, float)
  reverse_power = pow(float, integer)
sample:
  out1 = f32(forward + reverse + generic_forward + generic_reverse + minimum + maximum + clamped + power + reverse_power)
"#
        ));
        for name in [
            "forward",
            "reverse",
            "generic_forward",
            "generic_reverse",
            "minimum",
            "maximum",
            "clamped",
            "power",
            "reverse_power",
        ] {
            assert_state_type(&typed, name, expected);
        }
        lower_program_to_optimized_mir(&typed).unwrap();
    }
}

#[test]
fn concrete_integer_constants_promote_float_operations_before_folding() {
    for declaration in [
        "const Integer: i64 = 16777217",
        "const Integer = i64(16777217)",
        "const Values: i64[1] = [16777217]\nconst Integer = Values[0]",
    ] {
        let typed = analyze_source(&format!(
            r#"
{declaration}
const Alias = Integer
const Half: f32 = 0.5
const Sum = Alias + Half
def combine<T>(a: T, b: T) -> T:
  return a + b
init:
  folded = Sum
  direct = Alias + Half
  contextual = Alias + 0.5
  builtin = max(Half, Alias)
  generic = combine(Alias, Half)
sample:
  out1 = f32(folded + direct + contextual + builtin + generic)
"#
        ));
        for name in ["folded", "direct", "contextual", "builtin", "generic"] {
            assert_state_type(&typed, name, PrimitiveType::F64);
        }
        lower_program_to_optimized_mir(&typed).unwrap();
    }
}

#[test]
fn contextual_only_generic_calls_default_after_combining_numeric_families() {
    let typed = analyze_source("const Big = 4294967297\nconst Half = 0.5\ndef combine<T>(a: T, b: T) -> T:\n  return a + b\ninit:\n  forward = combine(Big, Half)\n  reverse = combine(Half, Big)\n  binary = Big + Half\n  builtin = max(Big, Half)\nsample:\n  out1 = forward + reverse + binary + builtin\n");
    for name in ["forward", "reverse", "binary", "builtin"] {
        assert_state_type(&typed, name, PrimitiveType::F32);
    }
    lower_program_to_optimized_mir(&typed).unwrap();
}
