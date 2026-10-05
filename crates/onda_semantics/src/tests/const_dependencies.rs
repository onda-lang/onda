use super::*;

#[test]
fn generic_array_default_initializers_wait_for_specialization() {
    let declarations = "const def build(value: i32) -> i32[2]:\n  return [value, value + 1]\ndef read<T>(values: T[2] = build(T(7))) -> T:\n  return values[0]\n";
    for call in ["read<i32>()", "read<i32>([i32(9), i32(10)])"] {
        let source = format!("{declarations}sample:\n  out1 = f32({call})\n");
        let typed = analyze_source(&source);
        lower_program_to_optimized_mir(&typed).unwrap();
    }
    let source = format!("{declarations}sample:\n  out1 = read<f32>()\n");
    let errors = analyze(parse_program(&source).unwrap())
        .expect_err("specialized defaults must still match the concrete array type");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("f32 array elements, got i32")),
        "{errors:?}"
    );
}

#[test]
fn generic_proc_array_default_initializers_wait_for_specialization() {
    let source = "const def build(value: i32) -> i32[2]:\n  return [value, value + 1]\nproc Voice<T>:\n  params:\n    values: T[2] = build(T(7))\n  sample:\n    out1 = f32(values[0] + values[1])\ninit:\n  voice = Voice<i32>()\nsample:\n  out1 = voice()\n";
    lower_program_to_optimized_mir(&analyze_source(source)).unwrap();
}

#[test]
fn const_array_arguments_defer_payloads_until_executed_reads_or_writes() {
    for param_type in ["i32[2]", "i32[]", "[]"] {
        for argument in ["Table", "Table[:]", "Table[-2:99]"] {
            for body in [
                "  return xs.len()",
                "  return xs.len() + i32(false && (xs[0] > 0))",
                "  copy: i32[2] = xs\n  size: i32 = copy.len()\n  return size",
            ] {
                let source = format!(
                    "const Table: i32[2] = [1 / 0, 0]\nconst def length(xs: {param_type}) -> i32:\n{body}\nconst def forward(xs: {param_type}) -> i32:\n  return length(xs)\nsample:\n  out1 = f32(forward({argument}))\n"
                );
                let typed = analyze_source(&source);
                assert!(typed.const_arrays.is_empty(), "{source}");
                assert!(lower_program_to_optimized_mir(&typed)
                    .unwrap()
                    .const_data
                    .is_empty());
            }
            let source = format!(
                "const Table: i32[2] = [1 / 0, 0]\nconst def read(xs: {param_type}) -> i32:\n  return xs[0]\nsample:\n  out1 = f32(read({argument}))\n"
            );
            let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("division by zero")),
                "{source}\n{errors:?}"
            );
        }
    }
    let source = "const Table: i32[2] = [1 / 0, 0]\nconst def write(source: i32[2]) -> i32:\n  xs: i32[2] = source\n  xs[0] = 7\n  return xs.len()\nsample:\n  out1 = f32(write(Table))\n";
    let errors = analyze(parse_program(source).unwrap()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("division by zero")),
        "{errors:?}"
    );
}

#[test]
fn runtime_fixed_array_defaults_share_lazy_declaration_normalization() {
    for default in ["Table[:]", "Table[-2:99]", "identity(Table)"] {
        let prefix = format!(
            "const Table: i32[2] = [1 / 0, 0]\nconst def identity(xs: i32[2]) -> i32[2]:\n  return xs\ndef read(xs: i32[2] = {default}) -> i32:\n  return xs[0]\n"
        );
        for expression in ["7", "read([7, 9])"] {
            let source = format!("{prefix}sample:\n  out1 = f32({expression})\n");
            assert!(analyze_source(&source).const_arrays.is_empty(), "{source}");
        }
        let source = format!("{prefix}sample:\n  out1 = f32(read())\n");
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("division by zero")),
            "{source}\n{errors:?}"
        );
        lower_program_to_optimized_mir(&analyze_source(&source.replace("1 / 0", "7"))).unwrap();
    }
}

#[test]
fn unused_runtime_array_defaults_check_types_and_shapes_without_payloads() {
    for (param_type, default, diagnostic) in [
        ("i32[2]", "Table[:1]", "array length 2, got 1"),
        ("f32[2]", "Table[:]", "f32 array elements, got i32"),
    ] {
        let source = format!(
            "const Table: i32[2] = [1 / 0, 0]\ndef read(xs: {param_type} = {default}):\n  return xs[0]\nsample:\n  out1 = 0.0\n"
        );
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
fn unused_large_event_array_defaults_do_not_demand_payloads() {
    let typed = analyze_source(
        "const def build() -> i32[131072]:\n  values: i32[131072]\n  values[0] = 1 / 0\n  return values\nconst Table = build()\nproc Unused:\n  event read(values: i32[131072] = Table):\n    local = values[0]\n  sample:\n    out1 = 0.0\nsample:\n  out1 = 0.0\n"
    );
    assert!(typed.const_arrays.is_empty());
    assert!(typed.defs.is_empty());
    assert!(lower_program_to_optimized_mir(&typed)
        .unwrap()
        .const_data
        .is_empty());
}

#[test]
fn loop_breaks_and_possible_zero_iterations_preserve_trailing_demand() {
    for body in [
        "while true:\n  break\n",
        "while true:\n  if enabled:\n    break\n  return 7\n",
        "while true:\n  if enabled:\n    break\n  else:\n    continue\n",
        "while enabled:\n  return 7\n",
        "for i in 0..0:\n  return 7\n",
        "for i in 0..1:\n  continue\n",
        "for i in 0..1:\n  if enabled:\n    break\n  return 7\n",
        "for i in 0..i32(enabled):\n  return 7\n",
    ] {
        let source = format!("const Broken: i32[1] = [1 / 0]\nparams:\n  enabled: bool = false\ndef read(enabled: bool) -> i32:\n{}  return Broken[0]\nsample:\n  out1 = f32(read(enabled))\n", body.lines().map(|line| format!("  {line}\n")).collect::<String>());
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("division by zero")),
            "{source}\n{errors:?}"
        );
    }
}

#[test]
fn runtime_statements_preserve_diagnostics_and_required_array_reads() {
    for source in [
        "sample:\n  if false:\n    out1 = Missing\n  else:\n    out1 = 7.0\n",
        "const Table: i32[1] = [1 / 0]\nsample:\n  if true:\n    out1 = f32(Table[0])\n  else:\n    out1 = 7.0\n",
        "const Table: i32[1] = [1 / 0]\nparams:\n  enabled: bool = false\nsample:\n  if enabled:\n    out1 = f32(Table[0])\n  else:\n    out1 = 7.0\n",
        "const Table: i32[1] = [1 / 0]\nsample:\n  for i in 0..=0:\n    value = Table[0]\n  out1 = 7.0\n",
        "sample:\n  for i @ 0 in 0..0:\n    value = 1\n  out1 = 7.0\n",
    ] {
        assert!(analyze(parse_program(source).unwrap()).is_err(), "{source}");
    }
}

#[test]
fn short_circuited_slice_bounds_do_not_build_arrays() {
    let prefix = "const Table: i32[2] = [7, 9]\nconst Bounds: i32[1] = [1 / 0]\ndef check(xs: i32[]) -> bool:\n  return xs[0] > 0\n";
    for expression in [
        "true || check(Table[Bounds[0]:])",
        "false && check(Table[Bounds[0]:])",
    ] {
        let typed = analyze_source(&format!("{prefix}sample:\n  out1 = f32({expression})\n"));
        assert!(typed.const_arrays.is_empty(), "{expression}");
        assert!(lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .is_empty());
    }
    let errors = analyze(
        parse_program(&format!(
            "{prefix}sample:\n  out1 = f32(check(Table[Bounds[0]:]))\n"
        ))
        .unwrap(),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("division by zero")),
        "{errors:?}"
    );
}

#[test]
fn executable_logical_operands_request_only_used_const_values() {
    let prefix =
        "const Table: i32[1] = [1 / 0]\nconst def fail() -> bool:\n  return Table[0] > 0\n";
    for expression in [
        "true || fail()",
        "false && fail()",
        "true || (Table[0] > 0)",
        "false && (Table[0] > 0)",
    ] {
        let typed = analyze_source(&format!("{prefix}sample:\n  out1 = f32({expression})\n"));
        assert!(typed.const_arrays.is_empty());
        lower_program_to_optimized_mir(&typed)
            .expect("discarded logical operands should stay lazy");
    }
    for expression in [
        "true || missing()",
        "true || 1",
        "true || fail(1)",
        "true || fail<f32>()",
        "(true || missing()) == true",
        "!(false && missing())",
        "true || missing() || fail()",
    ] {
        assert!(
            analyze(
                parse_program(&format!("{prefix}sample:\n  out1 = f32({expression})\n")).unwrap()
            )
            .is_err(),
            "{expression}"
        );
    }
}

#[test]
fn function_defaults_request_constants_only_when_omitted() {
    let prefix = "const Table: i32[1] = [1 / 0]\nconst def default_value() -> i32:\n  return Table[0]\ndef consume(value: i32 = default_value()) -> i32:\n  return value\n";
    for call in ["consume(7)", "consume(value=7)"] {
        let typed = analyze_source(&format!("{prefix}sample:\n  out1 = f32({call})\n"));
        assert!(typed.const_arrays.is_empty());
        assert!(lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .is_empty());
    }
    let errors =
        analyze(parse_program(&format!("{prefix}sample:\n  out1 = f32(consume())\n")).unwrap())
            .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("division by zero")),
        "{errors:?}"
    );
    let typed = analyze_source("const Table: i32[1] = [7]\nconst def default_value() -> i32:\n  return Table[0]\ndef consume(value: i32 = default_value()) -> i32:\n  return value\nsample:\n  out1 = f32(consume())\n");
    assert!(typed.const_arrays.is_empty());
    lower_program_to_optimized_mir(&typed).expect("used default should fold at its call site");
}

#[test]
fn runtime_defaults_accept_const_metadata_and_demand_only_omitted_values() {
    for (declaration, default, explicit) in [
        ("const Value: i32[1] = [1 / 0]", "Value[0]", "7"),
        ("const Table: i32[1] = [1 / 0]", "Table[0]", "7"),
        ("const Value: i32[1] = [1 / 0]", "min(Value[0], 7)", "7"),
        (
            "const Value: i32[1] = [1 / 0]",
            "i32(sqrt(f32(Value[0])))",
            "49",
        ),
    ] {
        let prefix =
            format!("{declaration}\ndef consume(value: i32 = {default}) -> i32:\n  return value\n");
        for expression in ["7", "consume(7)", "consume(value=7)"] {
            let typed = analyze_source(&format!("{prefix}sample:\n  out1 = f32({expression})\n"));
            assert!(typed.const_arrays.is_empty());
            assert!(lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty());
        }
        let source = format!("{prefix}sample:\n  out1 = f32(consume())\n");
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("division by zero")),
            "{source}\n{errors:?}"
        );

        let source = format!(
            "{}\nsample:\n  out1 = f32(consume())\n",
            prefix.replace("1 / 0", explicit)
        );
        let typed = analyze_source(&source);
        assert!(lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .is_empty());
    }
}

#[test]
fn runtime_array_defaults_preserve_lazy_const_payloads() {
    let prefix = "const Table: i32[1] = [1 / 0]\ndef consume(value: i32[1] = Table) -> i32:\n  return value[0]\n";
    for expression in ["7", "consume([7])", "consume(value=[7])"] {
        let typed = analyze_source(&format!("{prefix}sample:\n  out1 = f32({expression})\n"));
        assert!(typed.const_arrays.is_empty());
        assert!(lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .is_empty());
    }
    let source = format!("{prefix}sample:\n  out1 = f32(consume())\n");
    let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("division by zero")),
        "{source}\n{errors:?}"
    );
    lower_program_to_optimized_mir(&analyze_source(&source.replace("1 / 0", "7"))).unwrap();
}

#[test]
fn runtime_const_defaults_keep_callee_scope_and_namespace_specialization() {
    let source = "namespace N<V = 2>:\n  const Value: i32 = V\n  const Table: i32[1] = [V]\n  def read(value: i32 = Value + Table[0]) -> i32:\n    return value\nconst Value: i32 = 100\nconst Table: i32[1] = [100]\nsample:\n  out1 = f32(N<7>::read() + N<9>::read())\n";
    let typed = analyze_source(source);
    assert!(typed.const_arrays.is_empty());
    assert!(lower_program_to_optimized_mir(&typed)
        .unwrap()
        .const_data
        .is_empty());
}

#[test]
fn deferred_calls_keep_consumer_options_and_global_declaration_options() {
    let typed = analyze_source("const Table: i32[1] = [i32(SR)]\nconst def rate() -> i32:\n  return i32(SR)\nproc Voice:\n  params:\n    index: i32 = 0\n  sample 2:\n    out1 = f32(rate() + Table[index])\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
    assert_eq!(
        *typed.const_arrays[0].values,
        [TypedConstValue::I32(48_000)]
    );
    assert!(
        typed.defs.iter().any(|def| def.body.iter().any(|stmt| {
            let mut found = false;
            stmt.visit_exprs(|expr| {
                found |= expr
                    .walk()
                    .any(|expr| matches!(expr, Expr::Int { value: 96_000, .. }));
            });
            found
        })),
        "oversampled const call should use 96 kHz"
    );
    lower_program_to_optimized_mir(&typed).expect("contextual constants should lower");
    let typed = analyze_source("const def table(values: f32[i32(SR / 48000)]) -> f32[i32(SR / 48000)]:\n  return values\ndef read(values: f32[1]):\n  return values[0]\nproc Voice:\n  sample 2:\n    out1 = read(table([0.25]))\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
    lower_program_to_optimized_mir(&typed)
        .expect("signature sizes should retain declaration options");
    for (factor, expected) in [(1, PrimitiveType::F32), (2, PrimitiveType::I32)] {
        let source = format!("const def slot() -> i32:\n  return i32(SR / 48000) - 1\ndef select(value: f32):\n  return 1.0\ndef select(value: i32):\n  return 2.0\nproc Voice:\n  sample {factor}:\n    pair = (f32(0.5), i32(7))\n    out1 = select(pair[slot()])\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
        let typed = analyze_source(&source);
        assert_integer_overload(&typed, expected);
        lower_program_to_optimized_mir(&typed).expect("selectors should use their owner context");
    }
}

#[test]
fn namespaced_const_calls_resolve_in_all_executable_owners() {
    let prefix = "namespace N:\n  const def selected() -> i32:\n    return 7\n";
    for owner in [
        "sample:\n  out1 = f32(N::selected())\n",
        "namespace N:\n  def read() -> i32:\n    return selected()\nsample:\n  out1 = f32(N::read())\n",
        "namespace N:\n  proc Voice:\n    sample:\n      out1 = f32(selected())\ninit:\n  voice = N::Voice()\nsample:\n  out1 = voice()\n",
        "event update():\n  value = N::selected()\nsample:\n  out1 = 0.0\n",
    ] {
        let typed = analyze_source(&format!("{prefix}{owner}"));
        assert!(typed.defs.iter().all(|def| !def.name.ends_with("selected")));
        lower_program_to_optimized_mir(&typed).expect("const calls should have folded");
    }
    let prefix = "namespace N<V = 2>:\n  const def selected() -> i32:\n    return V\n  def read() -> i32:\n    return selected()\n  proc Voice:\n    sample:\n      out1 = f32(selected())\n";
    for owner in [
        "sample:\n  out1 = f32(N<7>::selected() + N<9>::selected())\n",
        "namespace A = N<7>\nnamespace B = N<9>\nsample:\n  out1 = f32(A::read() + B::read())\n",
        "init:\n  voice = N<7>::Voice()\nsample:\n  out1 = voice()\n",
    ] {
        let typed = analyze_source(&format!("{prefix}{owner}"));
        lower_program_to_optimized_mir(&typed).expect("each specialized call should resolve");
    }
}

#[test]
fn array_payload_sharing_survives_export_and_program_cloning() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TypedProgram>();
    let mut source = String::from("const def make() -> i32[16384]:\n  values: i32[16384]\n  return values\nconst A0 = make()\n");
    for index in 1..=64 {
        source.push_str(&format!("const A{index} = A{}\n", index - 1));
    }
    source.push_str("params:\n  index: i32 = 0\nsample:\n  out1 = ");
    source.push_str(
        &(0..=64)
            .map(|n| format!("f32(A{n}[index])"))
            .collect::<Vec<_>>()
            .join(" + "),
    );
    source.push('\n');
    let typed = analyze_source(&source);
    assert_eq!(typed.const_arrays.len(), 65);
    let payload = &typed.const_arrays[0].values;
    let copy = typed.clone();
    for array in typed.const_arrays.iter().chain(&copy.const_arrays) {
        assert!(
            std::sync::Arc::ptr_eq(payload, &array.values),
            "{} copied its payload",
            array.name
        );
    }
    assert_eq!(
        lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .len(),
        1
    );
}

#[test]
fn inferred_integer_arrays_keep_their_element_width_before_and_after_evaluation() {
    for value in [7_i64, 4_294_967_297, -4_294_967_297] {
        let prefix = format!(
            "const Table: i64[1] = [{value}]\nconst Copy = [Table[0]]\nconst Alias = Copy\nconst Slice = Copy[:]\n"
        );
        let unused = analyze_source(&format!("{prefix}sample:\n  out1 = 0.0\n"));
        assert!(unused.const_arrays.is_empty());
        for array in ["Copy", "Alias", "Slice"] {
            let source = format!("{prefix}def select(value: i32):\n  return 1.0\ndef select(value: i64):\n  return 2.0\nproc Voice:\n  params:\n    index: i32 = 0\n  sample:\n    out1 = select({array}[index])\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
            let typed = analyze_source(&source);
            let evaluated = typed.const_arrays.iter().find(|a| a.name == array).unwrap();
            assert_eq!(evaluated.elem_ty, PrimitiveType::I64);
            assert_eq!(*evaluated.values, [TypedConstValue::I64(value)]);
            assert_integer_overload(&typed, PrimitiveType::I64);
            lower_program_to_optimized_mir(&typed).expect("inferred integer array should lower");
        }
    }
}

fn assert_integer_overload(typed: &TypedProgram, expected: PrimitiveType) {
    let selected = typed
        .defs
        .iter()
        .find(|def| def.name.starts_with("__onda_ovl_select"))
        .unwrap();
    assert!(
        matches!(
            selected.param_kinds.first(),
            Some(TypedFnParam::Scalar { ty: Some(actual) }) if *actual == expected
        ),
        "{selected:?}"
    );
}

#[test]
fn integer_constant_overloads_are_independent_of_evaluation_order_and_owner() {
    for ty in ["i32", "i64"] {
        let expected = if ty == "i32" {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        };
        let prefix = format!("const Table: {ty}[1] = [7]\nconst Selected = Table[0]\n");
        for expression in ["Selected", "Table[0]", "Table[i32(0)]", "Selected + 0"] {
            for forced in ["", "params:\n  unused: i32 = i32(Table[0])\n"] {
                let definitions = "def select(value: i32):\n  return 1.0\ndef select(value: i64):\n  return 2.0\n";
                for body in [
                    format!("sample:\n  out1 = select({expression})\n"),
                    format!("proc Voice:\n  sample:\n    out1 = select({expression})\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n"),
                    format!("struct Holder:\n  value: f32 = 0.0\n  def get(self):\n    return select({expression})\ninit:\n  holder = Holder()\nsample:\n  out1 = holder.get()\n"),
                ] {
                    let typed = analyze_source(&format!("{prefix}{forced}{definitions}{body}"));
                    assert_integer_overload(&typed, expected);
                    lower_program_to_optimized_mir(&typed).expect("integer constant call should lower");
                }
            }
        }
    }
}

#[test]
fn unused_wide_integer_array_metadata_stays_lazy() {
    let typed = analyze_source("const Table: i64[1] = [1 / 0]\nconst Selected = Table\nconst Copy = [Selected[0]]\nconst Alias = Copy\nsample:\n  out1 = 0.0\n");
    assert!(typed.const_arrays.is_empty());
}

#[test]
fn inferred_array_parameters_keep_integer_const_widths() {
    for (ty, expected) in [("i32", PrimitiveType::I32), ("i64", PrimitiveType::I64)] {
        for forced in ["", "params:\n  unused: i32 = i32(Table[0])\n"] {
            for expression in ["[Table[0]]", "[Selected]"] {
                let source = format!("const Table: {ty}[1] = [7]\nconst Selected = Table[0]\n{forced}def select(values: []):\n  return f32(values[0])\nsample:\n  out1 = select({expression})\n");
                let typed = analyze_source(&source);
                let selected = typed
                    .defs
                    .iter()
                    .find(|def| def.name.starts_with("select"))
                    .expect("selected specialization should be retained");
                assert!(
                    matches!(
                        selected.param_kinds.first(),
                        Some(TypedFnParam::Array { elem_ty, .. }) if *elem_ty == expected
                    ),
                    "{selected:?}"
                );
                lower_program_to_optimized_mir(&typed).expect("inferred arrays should lower");
            }
        }
    }
}

#[test]
fn folded_integer_constants_remain_static_selectors() {
    let prefix = "const Index: i64 = 1\n";
    for body in [
        "init:\n  pair: (f32, i32) = (0.25, 7)\nsample:\n  pair[Index] = 9\n  out1 = f32(pair[Index])\n",
        "struct Holder:\n  pair: (f32, i32) = (0.25, 7)\ninit:\n  holder = Holder()\nsample:\n  holder.pair[Index] = 9\n  out1 = f32(holder.pair[Index])\n",
        "init:\n  data: f32[3] = [0.25, 0.5, 0.75]\n  selected: f32[2] = data[Index:]\nsample:\n  out1 = selected[0]\n",
        "proc Voice:\n  sample:\n    out1 = 0.25\ninit:\n  voices: Voice[2]\nsample:\n  out1 = voices[Index]()\n",
    ] {
        let typed = analyze_source(&format!("{prefix}{body}"));
        lower_program_to_optimized_mir(&typed).expect("folded constant selectors should lower");
    }
}

#[test]
fn folded_integer_zero_is_rejected_as_a_loop_step() {
    let source = "const Step: i64 = 0\nsample:\n  for i: i32 @ Step in 0..2:\n    out1 = f32(i)\n";
    let errors = analyze(parse_program(source).expect("source should parse"))
        .expect_err("zero loop step must be diagnosed");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("for loop step cannot be zero")));
}

#[test]
fn direct_const_calls_resolve_transitive_arrays_in_code_and_metadata() {
    let prefix = r#"
const Values: f32[2] = [0.02, 0.75]
const Selected = Values[0]
const def select() -> f32:
  return Selected
const def forward() -> f32:
  return select()
"#;
    for body in [
        "sample:\n  out1 = forward()\n",
        "params:\n  gain = 0.5 {0, 1, smooth = forward()}\nsample:\n  out1 = gain\n",
        "proc Voice:\n  outs:\n    out1\n  sample:\n    out1 = forward()\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n",
        "def select_voice(value: i32):\n  return forward()\ndef select_voice(value: f32):\n  return value\nsample:\n  out1 = select_voice(1)\n",
    ] {
        let typed = analyze_source(&format!("{prefix}{body}"));
        assert!(typed.const_arrays.is_empty());
        let mir = lower_program_to_optimized_mir(&typed).expect("const calls should lower");
        assert!(mir.const_data.is_empty(), "folded values need no array data");
    }
}

#[test]
fn const_array_sizes_follow_const_calls() {
    let typed = analyze_source(
        r#"
const Sizes: i32[1] = [2]
const def count() -> i32:
  return Sizes[0]
const Values: f32[count()] = [0.25, 0.75]
params:
  index: i32 = 1
sample:
  out1 = Values[index]
"#,
    );
    assert_eq!(
        typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Values")
            .unwrap()
            .len,
        2
    );
}

#[test]
fn const_def_parameters_and_locals_do_not_demand_shadowed_arrays() {
    let prefix = "const Table: i32[1] = [1 / 0]\n";
    for (definition, call) in [
        ("const def first(Table: i32[1]) -> i32:\n  return Table[0]\n", "first([9])"),
        ("const def first(Table: i32[]) -> i32:\n  return Table[0] + Table.len()\n", "first([9])"),
        ("const def first() -> i32:\n  total = 0\n  for Table in 0..2:\n    total += Table\n  return total\n", "first()"),
    ] {
        // Exercise both preprocessing demand and late artifact resolution.
        for body in [
            format!("sample:\n  out1 = f32({call})\n"),
            format!("const Used: i32[1] = [{call}]\nsample:\n  out1 = f32(Used[0])\n"),
        ] {
            let typed = analyze_source(&format!("{prefix}{definition}{body}"));
            assert!(typed.const_arrays.iter().all(|array| array.name != "Table"));
        }
    }
}

#[test]
fn shadowing_preserves_global_reads_in_initializers_and_other_functions() {
    for definition in [
        "const def global() -> i32:\n  return Table[0]\nconst def first(Table: i32 = 0) -> i32:\n  return global()\n",
        "const def first() -> i32:\n  for Table in 0..1:\n    value = Table\n  return Table[0]\n",
    ] {
        let typed = analyze_source(&format!("const Table: i32[1] = [7]\n{definition}const Used: i32[1] = [first()]\nparams:\n  index: i32 = 0\nsample:\n  out1 = f32(Used[index])\n"));
        assert_eq!(typed.const_arrays.len(), 1);
        assert_eq!(typed.const_arrays[0].name, "Used");
        assert_eq!(typed.const_arrays[0].values[0], TypedConstValue::I32(7));
    }
}

#[test]
fn const_def_assignments_bind_lexically_without_demanding_shadowed_globals() {
    for (parameter, body, argument, initializer) in [
        ("Table: i32", "Table += 1\nreturn Table", "6", "1 / 0"),
        (
            "Table: i32[2]",
            "Table[0] = 7\nreturn Table[0]",
            "[0, 0]",
            "1 / 0",
        ),
        ("", "Table: i32 = 6\nTable += 1\nreturn Table", "", "1 / 0"),
        (
            "",
            "Table: i32[2] = [0, 0]\nTable[0] = 7\nreturn Table[0]",
            "",
            "1 / 0",
        ),
        ("", "Table = Table[0]\nTable += 1\nreturn Table", "", "6"),
        (
            "",
            "if true:\n  Table = 6\nelse:\n  Table = 8\nTable += 1\nreturn Table",
            "",
            "1 / 0",
        ),
    ] {
        let body = body
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect::<String>();
        let declarations = format!(
            "const Table: i32[2] = [{initializer}, 0]\nconst def read({parameter}) -> i32:\n{body}"
        );
        for namespace in [None, Some("Library"), Some("Library<N = 2>")] {
            let (declarations, call) = if let Some(namespace) = namespace {
                let items = declarations
                    .lines()
                    .map(|line| format!("  {line}\n"))
                    .collect::<String>();
                let qualifier = if namespace.contains('<') {
                    "Library<3>"
                } else {
                    "Library"
                };
                (
                    format!("namespace {namespace}:\n{items}"),
                    format!("{qualifier}::read({argument})"),
                )
            } else {
                (declarations.clone(), format!("read({argument})"))
            };
            let source = format!(
                "{declarations}const Used: i32[1] = [{call}]\nparams:\n  index: i32 = 0\nsample:\n  out1 = f32(Used[index])\n"
            );
            let typed = analyze_source(&source);
            let used = typed
                .const_arrays
                .iter()
                .find(|array| array.name == "Used")
                .unwrap();
            assert_eq!(used.values.as_ref(), &[TypedConstValue::I32(7)], "{source}");
            if initializer == "1 / 0" {
                assert_eq!(typed.const_arrays.len(), 1, "{source}");
            }
            assert_eq!(
                lower_program_to_optimized_mir(&typed)
                    .unwrap()
                    .const_data
                    .len(),
                1
            );
        }
    }
}

#[test]
fn const_def_assignments_can_shadow_scalar_globals() {
    let source = "const Value: i32 = 100\nconst def parameter(Value: i32) -> i32:\n  Value += 1\n  return Value\nconst def local() -> i32:\n  Value = 6\n  Value += 1\n  return Value\nconst Used: i32[2] = [parameter(6), local()]\nparams:\n  index: i32 = 0\nsample:\n  out1 = f32(Used[index] + Used[index + 1])\n";
    let typed = analyze_source(source);
    assert_eq!(
        typed.const_arrays[0].values.as_ref(),
        &[TypedConstValue::I32(7); 2]
    );
}

#[test]
fn const_def_locals_shadow_namespace_parameters_after_their_initializer() {
    let source = "namespace Library<N = 2>:\n  const def parameter(N: i32) -> i32:\n    N += 1\n    return N\n  const def local() -> i32:\n    N = N + 1\n    N += 1\n    return N\nconst Used: i32[2] = [Library<5>::parameter(6), Library<5>::local()]\nparams:\n  index: i32 = 0\nsample:\n  out1 = f32(Used[index] + Used[index + 1])\n";
    let typed = analyze_source(source);
    assert_eq!(
        typed.const_arrays[0].values.as_ref(),
        &[TypedConstValue::I32(7); 2]
    );
}

#[test]
fn const_def_execution_requests_only_taken_branches() {
    for condition in [
        "true",
        "false || true",
        "true || Table[0] > 0",
        "!(false && Table[0] > 0)",
    ] {
        let typed = analyze_source(&format!(
            "const Table: i32[1] = [1 / 0]\nconst def first() -> i32:\n  if {condition}:\n    return 9\n  return Table[0]\nconst Used: i32[1] = [first()]\nsample:\n  out1 = f32(Used[0])\n"
        ));
        assert!(typed.const_arrays.iter().all(|array| array.name != "Table"));
        assert!(lower_program_to_optimized_mir(&typed)
            .unwrap()
            .const_data
            .is_empty());
    }
}

#[test]
fn metadata_queries_do_not_execute_initializers_or_unused_const_calls() {
    let typed = analyze_source(
        r#"
const Broken: i32[2] = [1 / 0, 0]
const Unused: i32[1] = [1 / 0]
const def fail() -> f32:
  return f32(Broken[0])
def unused() -> f32:
  return fail()
proc UnusedVoice:
  sample:
    out1 = fail()
sample:
  out1 = f32(Broken.len())
"#,
    );
    assert!(typed.const_arrays.is_empty());
    assert!(typed.defs.is_empty());
    assert!(lower_program_to_optimized_mir(&typed)
        .unwrap()
        .const_data
        .is_empty());
}

#[test]
fn builtin_const_metadata_uses_types_without_placeholder_evaluation() {
    for forced in ["", "params:\n  unused: i32 = Table[0]\n"] {
        for expression in ["Selected", "Copy[0]"] {
            let typed = analyze_source(&format!(
                "const Table: i32[1] = [2]\nconst Selected = max(100 / Table[0], 1)\nconst Copy = [max(100 / Table[0], 1)]\n{forced}proc Voice:\n  sample:\n    out1 = f32({expression})\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n"
            ));
            assert!(lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty());
        }
    }
}

#[test]
fn inferred_arrays_use_one_descriptor_before_and_after_evaluation() {
    for (ty, expected) in [
        ("f32", PrimitiveType::F32),
        ("f64", PrimitiveType::F64),
        ("i32", PrimitiveType::I32),
        ("i64", PrimitiveType::I64),
    ] {
        let typed = analyze_source(&format!(
            "const def value() -> {ty}:\n  return {ty}(7)\nconst Values = [value()]\nproc Voice:\n  params:\n    index: i32 = 0\n  sample:\n    out1 = f32(Values[index])\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n"
        ));
        assert_eq!(typed.const_arrays[0].elem_ty, expected);
        lower_program_to_optimized_mir(&typed).expect("descriptor and value must agree");
    }
}

#[test]
fn const_integer_domains_and_tuple_selectors_resolve_at_their_consumers() {
    for ty in ["i32", "i64"] {
        for forced in ["", "params:\n  unused: i32 = i32(Values[0])\n"] {
            let source = format!("const Values: {ty}[2] = [4, 1]\n{forced}proc Voice:\n  init:\n    index: i32 = 0 {{Values[0], wrap}}\n    pair: (f32, i32) = (0.25, 7)\n  sample:\n    pair[Values[1]] = 9\n    out1 = f32(index + pair[Values[1]])\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
            let typed = analyze_source(&source);
            assert!(lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty());
        }
    }
}

#[test]
fn reachable_deferred_loop_steps_are_validated_after_resolution() {
    for ty in ["i32", "i64", "f32", "f64"] {
        let source = format!("const Steps: {ty}[1] = [0]\nproc Voice:\n  sample:\n    for i: i64 @ Steps[0] in 0..2:\n      out1 = f32(i)\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
        let errors = analyze(parse_program(&source).unwrap()).expect_err("zero step must fail");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("for loop step cannot be zero")),
            "{errors:?}"
        );
    }
}

fn scalar_dependency_chain(initial: &str) -> String {
    let mut source = format!("const Table: i32[1] = [1 / 0]\nconst S0 = {initial}\n");
    for index in 1..=30 {
        source.push_str(&format!(
            "const S{index} = S{} + S{}\n",
            index - 1,
            index - 1
        ));
    }
    source
}

#[test]
fn invalid_deferred_scalar_types_also_share_dependency_results() {
    let mut source = scalar_dependency_chain("Table[0] + Missing");
    source.push_str("sample:\n  out1 = 0.0\n");
    let errors =
        analyze(parse_program(&source).unwrap()).expect_err("missing symbol must be diagnosed");
    assert!(errors
        .iter()
        .any(|diagnostic| diagnostic.message.contains("Missing")));
}

#[test]
fn body_compile_time_dependencies_resolve_before_shapes() {
    let prefix = "const Unused: i32[1] = [1 / 0]\nconst Table: i32[2] = [2, 7]\n";
    for body in [
        "proc Voice:\n  sample:\n    out1 = f32(Table[1])\n",
        "proc Voice:\n  outs (Table[0])\n  sample:\n    out1 = 0.25\n    out2 = 0.75\n",
        "proc Voice:\n  init:\n    data: f32[Table[0]] = [0.25, 0.75]\n  sample:\n    out1 = data[0]\n",
        "proc Voice:\n  init:\n    data: f32[Table[0]] = [0.25, 0.75]\n  sample:\n    out1 = data[0]\n",
        "proc Voice:\n  def select(value: i32):\n    data: f32[Table[0]] = [0.25, 0.75]\n    return data[0] + f32(value)\n  sample:\n    out1 = select(1)\n",
    ] {
        let source = format!("{prefix}{body}init:\n  voice = Voice()\nsample:\n  voice()\n  out1 = voice.out1\n");
        let typed = analyze_source(&source);
        assert!(typed.const_arrays.is_empty(), "compile-time payloads must not escape");
        let mir = lower_program_to_optimized_mir(&typed).expect("processor metadata should lower");
        assert!(mir.const_data.is_empty(), "metadata uses need no runtime array");
    }
}

#[test]
fn overload_and_method_body_storage_sizes_resolve_before_typing() {
    let prefix = "const Table: i32[1] = [2]\n";
    for body in [
        "def select(value: i32):\n  data: f32[Table[0]] = [0.25, 0.75]\n  return data[0] + f32(value)\ndef select(value: f32):\n  return value\nsample:\n  out1 = select(1)\n",
        "def select(value: i32):\n  data: f32[Table[0]] = [0.25, 0.75]\n  return data[0] + f32(value)\ndef select(value: f32):\n  return value\nsample:\n  out1 = select(1)\n",
        "struct Holder:\n  value: f32 = 0.25\n  def select(self):\n    data: f32[Table[0]] = [0.25, 0.75]\n    return data[0] + self.value\ninit:\n  holder = Holder()\nsample:\n  out1 = holder.select()\n",
        "def select(value: i32):\n  if value > 0:\n    data: f32[Table[0]] = [0.25, 0.75]\n    return data[0]\n  return 0.0\ndef select(value: f32):\n  return value\nsample:\n  out1 = select(1)\n",
    ] {
        let typed = analyze_source(&format!("{prefix}{body}"));
        let mir = lower_program_to_optimized_mir(&typed).expect("body metadata should lower");
        assert!(mir.const_data.is_empty());
    }
}

#[test]
fn generic_local_constant_dependencies_remain_lazy_until_specialization() {
    let definition =
        "def select<T>() -> T:\n  First = T(Table[0])\n  Second = First + T(1)\n  return Second\n";
    let unused = analyze_source(&format!(
        "const Table: i32[1] = [1 / 0]\n{definition}sample:\n  out1 = 0.0\n"
    ));
    assert!(unused.const_arrays.is_empty());
    let used = analyze_source(&format!(
        "const Table: i32[1] = [7]\n{definition}sample:\n  out1 = select<f32>()\n"
    ));
    assert!(used.const_arrays.is_empty());
    assert!(lower_program_to_optimized_mir(&used)
        .unwrap()
        .const_data
        .is_empty());
}

#[test]
fn deferred_global_constants_keep_their_declaration_sample_rate() {
    // Local constants first resolve Values while preprocessing the processor
    // body with its effective sample rate.
    let source = r#"
const Values: f32[1] = [SR]
proc Voice:
  params:
    index: i32 = 0
  def select(value: i32):
    HostRate = Values[0]
    return HostRate + f32(value) + Values[index] * 0.0
  sample 2:
    out1 = select(1)
init:
  voice = Voice()
sample:
  out1 = voice()
"#;
    let typed = analyze_source(source);
    let values = typed
        .const_arrays
        .iter()
        .find(|array| array.name == "Values")
        .unwrap();
    assert_eq!(values.values[0], TypedConstValue::F32(48_000.0));
}

#[test]
fn long_deferred_value_and_type_chains_compile_on_a_worker_stack() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            for annotation in [": i32[1]", ""] {
                let mut source =
                    format!("const Table: i32[1] = [7]\nconst S0{annotation} = Table\n");
                for index in 1..=4096 {
                    source.push_str(&format!("const S{index}{annotation} = S{}\n", index - 1));
                }
                // Array aliases retain metadata without evaluating or copying contents.
                let unused = analyze_source(&format!(
                    "{source}const Copy = [S4096[0]]\nsample:\n  out1 = 0.0\n"
                ));
                assert!(unused.const_arrays.is_empty());
                let used = analyze_source(&format!("{source}sample:\n  out1 = f32(S4096[0])\n"));
                assert!(used.const_arrays.is_empty());
                assert!(lower_program_to_optimized_mir(&used)
                    .unwrap()
                    .const_data
                    .is_empty());
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn dynamically_requested_dependencies_use_the_heap_on_a_worker_stack() {
    std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(|| {
        let mut source = String::from("const C0: bool = true\n");
        for index in 1..=4096 {
            source.push_str(&format!("const C{index}: bool = false || C{}\n", index - 1));
        }
        let typed = analyze_source(&format!("{source}sample:\n  out1 = f32(C4096)\n"));
        assert!(typed.const_arrays.is_empty());
        lower_program_to_optimized_mir(&typed).expect("conditional dependency chains should lower");

        let mut source = String::from("const C0: i32 = 7\n");
        for index in 1..=512 {
            source.push_str(&format!("const def read{index}() -> i32:\n  return C{}\nconst C{index}: i32 = read{index}()\n", index - 1));
        }
        let typed = analyze_source(&format!("{source}sample:\n  out1 = f32(C512)\n"));
        lower_program_to_optimized_mir(&typed).expect("const-def dependency chains should lower");
    }).unwrap().join().unwrap();
}

#[test]
fn invalid_deferred_value_diamonds_report_the_failure_once() {
    let mut source = "const Table: i32[1] = [1 / 0]\nconst S0: i32 = Table[0]\n".to_owned();
    for index in 1..=30 {
        source.push_str(&format!(
            "const S{index}: i32 = S{} + S{}\n",
            index - 1,
            index - 1
        ));
    }
    source.push_str("sample:\n  out1 = f32(S30)\n");
    let errors =
        analyze(parse_program(&source).unwrap()).expect_err("invalid initializer must fail");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("zero"), "{errors:?}");
}

#[test]
fn deferred_numeric_aliases_preserve_contextual_coercion() {
    for (ty, value, target) in [("f64", "0.25", "f32"), ("i64", "7", "i32")] {
        let prefix = format!("const Table: {ty}[1] = [{value}]\nconst Selected = Table[0]\n");
        for body in [
            format!("proc Voice:\n  init:\n    value: {target} = Selected\n  sample:\n    out1 = f32(value)\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n"),
            format!("struct Holder:\n  value: {target} = 0\n  def set(self):\n    self.value = Selected\ninit:\n  holder = Holder()\nsample:\n  out1 = f32(holder.value)\n"),
            format!("def select(value: i32):\n  return 0.0\ndef select(value: f32):\n  result: {target} = Selected\n  return f32(result)\nsample:\n  out1 = select(0.0)\n"),
            format!("def select() -> {target}:\n  return Selected\nproc Voice:\n  sample:\n    out1 = f32(select())\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n"),
            format!("def select(value: {target}):\n  return value\nproc Voice:\n  init:\n    pair: ({target}, {target}) = (Selected, Selected)\n    data: {target}[1] = [Selected]\n  sample:\n    out1 = f32(select(Selected) + max(pair[0], Selected) + data[0])\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n"),
            format!("def select<T>(value: T, literal: T):\n  return value + literal\nproc Voice:\n  sample:\n    out1 = f32(select({target}(0), Selected))\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n"),
        ] {
            for expression in ["Selected", "Table[0]", "Table[i32(0)]"] {
                let body = body.replace("Selected", expression);
                let typed = analyze_source(&format!("{prefix}{body}"));
                lower_program_to_optimized_mir(&typed).expect("contextual constants should lower");
            }
        }
    }
}

#[test]
fn processor_tasks_share_const_scalar_array_and_return_metadata() {
    let prefix =
        "const Table: i32[1] = [7]\nconst Selected = Table[0]\ndef select():\n  return Selected\n";
    for expr in ["Selected", "Table[0]", "select()"] {
        let body = format!("proc Voice:\n  init:\n    value: i32 = 0\n  task work():\n    value = {expr}\n    yield\n  block:\n    await work()\n    sample:\n      out1 = f32(value)\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
        let typed = analyze_source(&format!("{prefix}{body}"));
        lower_program_to_optimized_mir(&typed).expect("task constants should lower");
    }
}

#[test]
fn unused_processor_graph_sources_do_not_demand_const_values() {
    let prefix =
        "const Table: i32[1] = [1 / 0]\nproc Voice:\n  graph:\n    f32(Table[0]) >> out1\n";
    let unused = analyze_source(&format!("{prefix}sample:\n  out1 = 0.0\n"));
    assert!(unused.const_arrays.is_empty());
    assert!(lower_program_to_optimized_mir(&unused)
        .unwrap()
        .const_data
        .is_empty());
    let source = format!("{prefix}init:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
    let errors =
        analyze(parse_program(&source).unwrap()).expect_err("reachable initializer must fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("division by zero")),
        "{errors:?}"
    );
}

#[test]
fn deferred_numeric_metadata_does_not_evaluate_unused_bodies_or_narrow_wide_integers() {
    let unused = analyze_source("const Table: f64[1] = [1.0 / 0.0]\nstruct Holder:\n  value: f32 = 0.0\n  def set(self):\n    self.value = Table[0]\nsample:\n  out1 = 0.0\n");
    assert!(unused.const_arrays.is_empty());
    let wide = analyze_source("const Table: i64[1] = [4294967297]\nconst Selected = Table[0]\nproc Voice:\n  init:\n    value = Selected\n  sample:\n    out1 = f32((value == 4294967297))\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
    lower_program_to_optimized_mir(&wide).expect("wide aliases should retain their value");
}

#[test]
fn unused_graph_delay_metadata_still_demands_const_values() {
    let source = "const Table: i32[1] = [1 / 0]\nproc Voice:\n  graph:\n    0.0 >>[Table[0]] out1\nsample:\n  out1 = 0.0\n";
    let errors = analyze(parse_program(source).unwrap()).expect_err("graph delay requires a value");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("division by zero")),
        "{errors:?}"
    );
}

#[test]
fn lazy_graph_numeric_sources_keep_contextual_coercion() {
    for expression in ["Table[0]", "Table[i32(0)]", "Selected[0]"] {
        let prefix = format!("const Table: f64[1] = [0.25]\nconst Selected = [Table[0]]\nproc Voice:\n  graph:\n    {expression} >> out1\n");
        let unused = analyze_source(&format!("{prefix}sample:\n  out1 = 0.0\n"));
        assert!(unused.const_arrays.is_empty());
        let used = analyze_source(&format!(
            "{prefix}init:\n  voice = Voice()\nsample:\n  out1 = voice()\n"
        ));
        lower_program_to_optimized_mir(&used).expect("graph literal coercion should lower");
    }
}

#[test]
fn runtime_const_array_indices_do_not_allow_literal_narrowing() {
    let source = "const Table: f64[1] = [0.25]\nproc Voice:\n  init:\n    index: i32 = 0\n  sample:\n    out1 = Table[index]\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
    let errors =
        analyze(parse_program(source).unwrap()).expect_err("runtime f64 reads require a cast");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("cannot assign F64 to F32")),
        "{errors:?}"
    );
}

#[test]
fn deferred_float_overload_arguments_use_the_same_default_as_folded_literals() {
    for expression in ["Selected", "Table[0]", "Table[i32(0)]"] {
        let source = format!("const Table: f64[1] = [0.25]\nconst Selected = Table[0]\ndef select(value: f32):\n  return 1.0\ndef select(value: f64):\n  return 2.0\nproc Voice:\n  sample:\n    out1 = select({expression})\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
        let typed = analyze_source(&source);
        let selected = typed
            .defs
            .iter()
            .find(|def| def.name.starts_with("__onda_ovl_select"))
            .unwrap();
        assert!(
            matches!(
                selected.param_kinds.first(),
                Some(TypedFnParam::Scalar {
                    ty: Some(PrimitiveType::F32)
                })
            ),
            "{selected:?}"
        );
        lower_program_to_optimized_mir(&typed).expect("literal overload should lower");
    }
}

#[test]
fn scalar_const_def_bindings_hide_global_array_values_and_lengths() {
    for forced in ["", "params:\n  force: i32 = Table[0]\n"] {
        for binding in [
            "const def read(Table: i32) -> i32:\n  return EXPRESSION\n",
            "const def read(value: i32) -> i32:\n  Table = value\n  return EXPRESSION\n",
        ] {
            for expression in ["Table[0]", "Table.len()"] {
                let definition = binding.replace("EXPRESSION", expression);
                for consumer in [
                    "sample:\n  out1 = f32(read(2))\n",
                    "const Result: i32 = read(2)\nsample:\n  out1 = f32(Result)\n",
                ] {
                    let source =
                        format!("const Table: i32[1] = [7]\n{forced}{definition}{consumer}");
                    assert!(
                        analyze(parse_program(&source).unwrap()).is_err(),
                        "{source}"
                    );
                }
            }
        }
    }
}

#[test]
fn unused_owners_do_not_evaluate_const_reads() {
    let prefix = "const Table: i32[1] = [1 / 0]\n";
    for owner in [
        "def unused() -> i32:\n  return Table[0]\n",
        "proc Unused:\n  sample:\n    out1 = f32(Table[0])\n",
        "struct Unused:\n  value: i32 = 0\n  def get(self) -> i32:\n    return Table[0]\n",
    ] {
        let source = format!("{prefix}{owner}sample:\n  out1 = 0.0\n");
        let typed = analyze_source(&source);
        assert!(typed.const_arrays.is_empty());
    }
}

#[test]
fn const_def_materialization_preserves_array_return_types() {
    for (ty, value) in [("f64", "16777217.0"), ("i64", "4294967297")] {
        for call in ["select(read())", "select(values)"] {
            let source = format!("const def read() -> {ty}[1]:\n  values: {ty}[1] = [{value}]\n  return values\ndef select(xs: {ty}[]) -> f32:\n  return 2.0\ndef select(xs: f32[]) -> f32:\n  return 1.0\nsample:\n  values = read()\n  out1 = {call}\n");
            let typed = analyze_source(&source);
            lower_program_to_optimized_mir(&typed)
                .expect("concrete array return type must survive folding");
        }
    }
}

#[test]
fn const_array_aliases_emit_one_shared_payload() {
    let source = "const Table: f32[3] = [7.0, 9.0, 11.0]\nconst Alias: f32[3] = Table\nconst def identity(xs: f32[3]) -> f32[3]:\n  return xs\nconst Copy = identity(Alias)\nconst Independent: f32[3] = [7.0, 9.0, 11.0]\nparams:\n  index: i32 = 1\ndef read(xs: f32[], index: i32) -> f32:\n  return xs[index]\nsample:\n  out1 = Table[index] + Alias[index] + read(Copy, index) + Independent[index]\n";
    let typed = analyze_source(source);
    let mir = lower_program_to_optimized_mir(&typed).unwrap();
    assert_eq!(
        mir.const_data.len(),
        2,
        "shared aliases should use one data ID; independent arrays have their own identity"
    );
}

#[test]
fn namespace_specializations_share_nested_array_dependencies_in_any_use_order() {
    let declarations = "namespace Source<N = 2>:\n  const def build() -> i32[N]:\n    values: i32[N]\n    for i in 0..N:\n      values[i] = N + i\n    return values\n  const Data = build()\n  namespace Child<M = N>:\n    const Alias: i32[] = Data\nnamespace Wrapper<P = 2>:\n  const Alias: i32[] = Source<P>::Data\nnamespace Second<Q = 2>:\n  const Alias: i32[] = Source<7>::Data\nnamespace Chain<R = 2>:\n  const Alias: i32[] = Wrapper<R>::Alias\nnamespace Imported<P = 2>:\n  use Source<P>::Data as Values\n  const Alias: i32[] = Values\nnamespace Aliased<P = 2>:\n  namespace Selected = Source<P>\n  const Alias: i32[] = Selected::Data\nparams:\n  index: i32 = 0\n";
    for (expression, payloads) in [
        ("Wrapper<7>::Alias[index] + Source<7>::Data[index]", 1),
        ("Source<7>::Data[index] + Wrapper<7>::Alias[index]", 1),
        (
            "Wrapper<7>::Alias[index] + Second<1>::Alias[index] + Second<2>::Alias[index]",
            1,
        ),
        (
            "Second<2>::Alias[index] + Second<1>::Alias[index] + Wrapper<7>::Alias[index]",
            1,
        ),
        ("Chain<7>::Alias[index] + Source<7>::Data[index]", 1),
        ("Source<7>::Data[index] + Chain<7>::Alias[index]", 1),
        (
            "Source<7>::Child::Alias[index] + Wrapper<7>::Alias[index]",
            1,
        ),
        (
            "Wrapper<7>::Alias[index] + Source<7>::Child::Alias[index]",
            1,
        ),
        (
            "Source<7>::Child<3>::Alias[index] + Source<7>::Child<4>::Alias[index]",
            1,
        ),
        (
            "Source<7>::Data[index] + Imported<7>::Alias[index] + Aliased<7>::Alias[index]",
            1,
        ),
        (
            "Aliased<7>::Alias[index] + Imported<7>::Alias[index] + Source<7>::Data[index]",
            1,
        ),
        ("Wrapper<7>::Alias[index] + Wrapper<8>::Alias[index]", 2),
    ] {
        let source = format!("{declarations}sample:\n  out1 = f32({expression})\n");
        let typed = analyze_source(&source);
        for left in &typed.const_arrays {
            for right in &typed.const_arrays {
                assert_eq!(
                    std::sync::Arc::ptr_eq(&left.values, &right.values),
                    left.len == right.len,
                    "{source}\n{} and {}",
                    left.name,
                    right.name
                );
            }
        }
        assert_eq!(
            lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .len(),
            payloads,
            "{source}"
        );
    }
}

#[test]
fn namespace_specializations_resolve_nested_scalars_and_defs_in_any_use_order() {
    let declarations = "namespace Source<N = 2>:\n  const Value: i32 = N\n  const def read() -> i32:\n    return Value\nnamespace Wrapper<P = 2>:\n  const Value: i32 = Source<P>::Value\n  const FromDef: i32 = Source<P>::read()\n";
    for (first, second) in [
        ("Source<7>::Value", "Wrapper<7>::Value"),
        ("Source<7>::read()", "Wrapper<7>::FromDef"),
        ("Wrapper<7>::Value", "Wrapper<8>::FromDef"),
    ] {
        for expression in [format!("{first} + {second}"), format!("{second} + {first}")] {
            let source = format!("{declarations}sample:\n  out1 = f32({expression})\n");
            let typed = analyze_source(&source);
            assert!(typed.const_arrays.is_empty(), "{source}");
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }
}

#[test]
fn namespace_defaults_keep_bindings_visible_when_the_template_was_declared() {
    for parent in ["Parent", "Parent<P = 2>"] {
        let qualifier = if parent.contains('<') {
            "Parent<2>"
        } else {
            "Parent"
        };
        let source = format!("const Value: i32 = 7\nnamespace {parent}:\n  namespace Child<N = Value>:\n    const Answer: i32 = N\n  const Value: i32 = 100\nconst Used: i32[1] = [{qualifier}::Child::Answer]\nparams:\n  index: i32 = 0\nsample:\n  out1 = f32(Used[index])\n");
        let typed = analyze_source(&source);
        assert_eq!(
            typed.const_arrays[0].values.as_ref(),
            &[TypedConstValue::I32(7)],
            "{source}"
        );
    }
    let source = "namespace Library<N = Later::Value>:\n  const Answer: i32 = N\nnamespace Later:\n  const Value: i32 = 7\nsample:\n  out1 = f32(Library::Answer)\n";
    assert!(analyze(parse_program(source).unwrap()).is_err());
}

#[test]
fn namespace_specialization_defaults_resolve_dependencies_in_definition_scope() {
    let declarations = "const Bias: i32 = 1\nnamespace Source<N = 2>:\n  const Value: i32 = N + Bias\n  const def read() -> i32:\n    return Value\nnamespace Wrapper<P = Source<2>::Value, Q = Source<P>::read()>:\n  const def build() -> i32[Q]:\n    values: i32[Q]\n    for i in 0..Q:\n      values[i] = Q\n    return values\n  const Data = build()\nnamespace Consumer:\n  const Bias: i32 = 100\n  const Alias: i32[] = Wrapper::Data\nparams:\n  index: i32 = 0\n";
    for expression in [
        "Consumer::Alias[index] + Wrapper::Data[index]",
        "Wrapper::Data[index] + Consumer::Alias[index]",
    ] {
        let source = format!("{declarations}sample:\n  out1 = f32({expression})\n");
        let typed = analyze_source(&source);
        assert!(
            typed.const_arrays.iter().all(|array| array.len == 4
                && array
                    .values
                    .iter()
                    .all(|value| *value == TypedConstValue::I32(4))),
            "{source}"
        );
        assert_eq!(
            lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .len(),
            1,
            "{source}"
        );
    }
}

#[test]
fn namespace_specialization_metadata_keeps_nested_unused_payloads_lazy() {
    let declarations = "namespace Source<N = 2>:\n  const def build() -> i32[N]:\n    values: i32[N]\n    values[0] = 1 / 0\n    return values\n  const Data = build()\nnamespace Wrapper<P = 2>:\n  const Alias: i32[] = Source<P>::Data\n  def read() -> i32:\n    return Alias[0]\nconst def length(xs: i32[]) -> i32:\n  return xs.len()\n";
    for expression in [
        "length(Wrapper<7>::Alias) + length(Source<7>::Data)",
        "length(Source<7>::Data) + length(Wrapper<7>::Alias)",
        "length(Wrapper<7>::Alias) + length(Wrapper<8>::Alias)",
        "length(Wrapper<2147483647>::Alias)",
        "0",
    ] {
        let source = format!("{declarations}sample:\n  out1 = f32({expression})\n");
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
    let source = format!("{declarations}sample:\n  out1 = f32(Wrapper<7>::read())\n");
    assert!(analyze(parse_program(&source).unwrap())
        .unwrap_err()
        .iter()
        .any(|error| error.message.contains("division by zero")));
}

#[test]
fn const_integer_builtin_selectors_and_casts_fold_without_array_data() {
    for index in [
        "abs(i32(1.0))",
        "min(i32(1.5), i32(2.0))",
        "i32(sin(0.0))",
        "abs(i64(9007199254740993)) - 9007199254740992",
    ] {
        let source =
            format!("const Table: i32[2] = [7, 9]\nsample:\n  out1 = f32(Table[{index}])\n");
        let typed = analyze_source(&source);
        assert!(
            lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty(),
            "{source}"
        );
        let source = format!("const Table: i32[2] = [7, 9]\nconst Slice = Table[{index}:]\nsample:\n  out1 = f32(Slice[0])\n");
        let typed = analyze_source(&source);
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
fn struct_range_bounds_resolve_metadata_without_demanding_defaults() {
    for ty in ["i32", "i64"] {
        let declarations = format!(
            "const Begin: {ty} = -2\nconst Count: {ty} = 4\nconst Bounds: {ty}[2] = [-2, 4]\nconst Broken: {ty}[1] = [1 / 0]\nconst def begin() -> {ty}:\n  return Begin\nconst def count() -> {ty}:\n  return Count\n"
        );
        for (begin, end) in [
            ("Begin", "Count"),
            ("Bounds[0]", "Bounds[1]"),
            ("begin()", "count()"),
        ] {
            for (domain, min, max, wrap) in [
                (format!("{{{end}}}"), 0, 3, false),
                (format!("{{{end}, wrap}}"), 0, 3, true),
                (format!("{{{begin}..{end}}}"), -2, 3, false),
                (format!("{{{begin}..{end}, wrap}}"), -2, 3, true),
                (format!("{{{begin}..={end}}}"), -2, 4, false),
                (format!("{{{begin}..={end}, wrap}}"), -2, 4, true),
            ] {
                let prefix =
                    format!("{declarations}struct Value:\n  value: {ty} = Broken[0] {domain}\n");
                for use_site in [
                    "sample:\n  out1 = 0.0\n",
                    "init:\n  value = Value(value = 9)\nsample:\n  out1 = f32(value.value)\n",
                ] {
                    let source = format!("{prefix}{use_site}");
                    let typed = analyze_source(&source);
                    let range = typed.structs[0].fields[0].integer_range.unwrap();
                    assert_eq!(
                        (range.min, range.max, range.wrap),
                        (min, max, wrap),
                        "{source}"
                    );
                    assert!(
                        typed
                            .const_arrays
                            .iter()
                            .all(|array| array.name != "Broken"),
                        "{source}"
                    );
                    assert!(
                        lower_program_to_optimized_mir(&typed)
                            .unwrap()
                            .const_data
                            .is_empty(),
                        "{source}"
                    );
                }
                let source = format!(
                    "{prefix}init:\n  value = Value()\nsample:\n  out1 = f32(value.value)\n"
                );
                let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
                assert!(
                    errors
                        .iter()
                        .any(|error| error.message.contains("division by zero")),
                    "{source}\n{errors:?}"
                );
            }
        }
    }
}

#[test]
fn struct_range_bounds_use_concrete_namespace_specializations() {
    let source = r#"
namespace Ring<Size = 4>:
  const def counts() -> i32[Size]:
    values: i32[Size]
    for i in 0..Size:
      values[i] = Size
    return values
  const Counts: i32[Size] = counts()
  const Count: i32 = Counts[0]
  const def count() -> i32:
    return Count
  struct Cursor:
    index: i32 = 10 {count(), wrap}

init:
  small = Ring<4>::Cursor()
  large = Ring<8>::Cursor()
sample:
  out1 = f32(small.index + large.index)
"#;
    let typed = analyze_source(source);
    assert!(typed.const_arrays.is_empty());
    let mir = lower_program_to_optimized_mir(&typed).unwrap();
    assert!(mir.const_data.is_empty());
    for (name, max) in [("small.index", 3), ("large.index", 7)] {
        let range = mir
            .state
            .iter()
            .find(|state| state.name == name)
            .unwrap()
            .integer_range
            .unwrap();
        assert_eq!(range.min, onda_mir::ScalarValue::I32(0));
        assert_eq!(range.max, onda_mir::ScalarValue::I32(max));
        assert_eq!(range.mode, onda_mir::IntegerRangeMode::Wrap);
    }
}

#[test]
fn unused_struct_ranges_validate_required_bounds() {
    for (count, expected) in [
        ("1 / 0", "division by zero"),
        ("0", "integer binding count must be positive"),
    ] {
        let source = format!(
            "const Count: i32 = {count}\nstruct Value:\n  value: i32 = 1 {{Count, wrap}}\nsample:\n  out1 = 0.0\n"
        );
        assert_analyze_error_contains(&source, expected);
    }
}

#[test]
fn declaration_defaults_demand_values_only_when_used() {
    for (declaration, use_default, override_default) in [
        ("struct Value:\n  value: i32 = Broken[0]\n", "init:\n  value = Value()\nsample:\n  out1 = f32(value.value)\n", "init:\n  value = Value(value = 7)\nsample:\n  out1 = f32(value.value)\n"),
        ("proc Value:\n  params:\n    value: i32 = Broken[0]\n  sample:\n    out1 = f32(value)\n", "init:\n  value = Value()\nsample:\n  out1 = value()\n", "init:\n  value = Value(value = 7)\nsample:\n  out1 = value()\n"),
        ("proc Value:\n  params:\n    value: i32[1] = Broken\n  sample:\n    out1 = f32(value[0])\n", "init:\n  value = Value()\nsample:\n  out1 = value()\n", "init:\n  value = Value(value = 7)\nsample:\n  out1 = value()\n"),
        ("proc Value:\n  ins:\n    in1: i32 = Broken[0]\n  sample:\n    out1 = f32(in1)\n", "init:\n  value = Value()\nsample:\n  out1 = value()\n", "init:\n  value = Value()\nsample:\n  out1 = value(7)\n"),
        ("proc Value:\n  init:\n    selected: i32 = 0\n  event read(value: i32 = Broken[0]):\n    selected = value\n  sample:\n    out1 = f32(selected)\n", "init:\n  value = Value()\n  value.read()\nsample:\n  out1 = value()\n", "init:\n  value = Value()\n  value.read(7)\nsample:\n  out1 = value()\n"),
    ] {
        let prefix = format!("const Broken: i32[1] = [1 / 0]\n{declaration}");
        for use_site in ["sample:\n  out1 = 0.0\n", override_default] {
            let source = format!("{prefix}{use_site}");
            let typed = analyze_source(&source);
            assert!(typed.const_arrays.is_empty(), "{source}");
            assert!(lower_program_to_optimized_mir(&typed).unwrap().const_data.is_empty(), "{source}");
        }
        let source = format!("{prefix}{use_default}");
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(errors.iter().any(|error| error.message.contains("division by zero")), "{source}\n{errors:?}");
    }
}

#[test]
fn unused_defaults_still_check_known_types_and_shapes() {
    for declaration in [
        "struct Value:\n  value: bool = Broken[0]\n",
        "proc Value:\n  params:\n    value: bool = Broken[0]\n  sample:\n    out1 = 0.0\n",
        "proc Value:\n  params:\n    value: i32[2] = Broken\n  sample:\n    out1 = 0.0\n",
        "proc Value:\n  ins:\n    in1: bool = Broken[0]\n  sample:\n    out1 = 0.0\n",
    ] {
        let source =
            format!("const Broken: i32[1] = [1 / 0]\n{declaration}sample:\n  out1 = 0.0\n");
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            !errors
                .iter()
                .any(|error| error.message.contains("division by zero")),
            "{source}\n{errors:?}"
        );
    }
}

#[test]
fn const_local_numeric_context_preserves_branch_reachability() {
    let source = "const Broken: i32[1] = [1 / 0]\nconst def choose() -> i32:\n  x: i64 = 2147483647\n  y: i32 = 1\n  z = x + y\n  z = z - 1\n  if z > 0:\n    return 7\n  return Broken[0]\nsample:\n  out1 = f32(choose())\n";
    let typed = analyze_source(source);
    assert!(typed.const_arrays.is_empty());
    assert!(lower_program_to_optimized_mir(&typed)
        .unwrap()
        .const_data
        .is_empty());
}

#[test]
fn runtime_slice_metadata_preserves_fixed_copy_shapes() {
    let prefix =
        "const Shape: i32[4] = [1 / 0, 0, 0, 0]\ninit:\n  values: f32[4] = [1.0, 2.0, 3.0, 4.0]\n";
    for bounds in ["1:3", "-3:-1", "Shape.len() - 3:Shape.len() - 1"] {
        let body = format!("  view = values[{bounds}]\n  copy: f32[2] = view\n");
        for owner in [
            format!("def unused(values: f32[4]) -> f32:\n{body}  return copy[0]\nsample:\n  out1 = 0.0\n"),
            format!("sample:\n{body}  out1 = copy[0]\n"),
        ] {
            let source = format!("{prefix}{owner}");
            let typed = analyze_source(&source);
            assert!(typed.const_arrays.is_empty(), "{source}");
            lower_program_to_optimized_mir(&typed).unwrap();
        }
    }
    let source = "const Bound: i32[1] = [1]\ninit:\n  values: f32[4] = [1.0, 2.0, 3.0, 4.0]\nsample:\n  view = values[Bound[0]:]\n  copy: f32[3] = view\n  out1 = copy[0]\n";
    lower_program_to_optimized_mir(&analyze_source(source)).unwrap();

    for (other_bound, succeeds) in [("1", true), ("2", false)] {
        let source = format!("const Bounds: i32[2] = [1, {other_bound}]\nparams:\n  enabled: bool = true\ninit:\n  values: f32[4] = [1.0, 2.0, 3.0, 4.0]\nsample:\n  if enabled:\n    view = values[Bounds[0]:]\n  else:\n    view = values[Bounds[1]:]\n  copy: f32[3] = view\n  out1 = copy[0]\n");
        let result = analyze(parse_program(&source).unwrap());
        if succeeds {
            lower_program_to_optimized_mir(&result.unwrap()).unwrap();
        } else {
            assert!(
                result
                    .unwrap_err()
                    .iter()
                    .any(|error| error.message.contains("fixed array declaration")),
                "{source}"
            );
        }
    }
}

#[test]
fn const_reach_uses_each_transitive_runtime_call_context() {
    for condition in ["SR > 48000.0", "high_rate()"] {
        let source = format!(
            r#"
const def high_rate() -> bool:
  return SR > 48000.0
const def rate() -> i32:
  return i32(SR / 48000)
def choose() -> i32:
  if {condition}:
    return 7 + rate()
  return 9 + rate()
def relay() -> i32:
  return choose()
init:
  initial = relay()
sample 2:
  out1 = f32(initial + relay())
"#
        );
        let typed = analyze_source(&source);
        let mir = lower_program_to_optimized_mir(&typed).unwrap();
        let choices = mir
            .functions
            .iter()
            .filter(|function| function.name.starts_with("choose.__ctx"))
            .collect::<Vec<_>>();
        assert_eq!(choices.len(), 2, "{source}");
        for (suffix, expected) in [("473b8000_bs_00000200", 10), ("47bb8000_bs_00000200", 9)] {
            let choice = choices
                .iter()
                .find(|function| function.name.ends_with(suffix))
                .unwrap();
            assert!(
                choice.body.statements.iter().any(|statement| {
                    matches!(&statement.kind, onda_mir::StatementKind::Return { values }
                    if values == &[onda_mir::Value::Constant(onda_mir::ScalarValue::I32(expected))])
                }),
                "{source}\n{choice:?}"
            );
        }
    }
}

#[test]
fn sample_root_tuple_overloads_use_the_sample_context() {
    let source = "const Broken: i32[1] = [1 / 0]\ndef choose(value: i32) -> f32:\n  return f32(Broken[0])\ndef choose(value: f32) -> f32:\n  return value\nsample 2:\n  values = (i32(7), f32(9.5))\n  selected = values[i32(SR / 48000) - 1]\n  out1 = choose(selected)\n";
    let typed = analyze_source(source);
    assert!(typed.const_arrays.is_empty());
    assert!(lower_program_to_optimized_mir(&typed)
        .unwrap()
        .const_data
        .is_empty());
}
