use super::*;

#[test]
fn array_initializers_follow_required_functions_and_metadata() {
    let prefix = "const def element() -> i32:\n  return 2\nconst def build() -> i32[2]:\n  return [element(), element()]\nconst Data = build()\ndef read() -> i32:\n  return Data[0]\n";
    for (body, builds) in [
        ("sample:\n  out1 = 0.0\n", 0),
        ("sample:\n  out1 = f32(Data.len())\n", 0),
        ("sample:\n  out1 = f32(read() + read())\n", 1),
        ("const Required = Data[0]\nsample:\n  out1 = 0.0\n", 1),
        ("def unused() -> i32:\n  values: i32[Data[0]]\n  return values.len()\nsample:\n  out1 = 0.0\n", 1),
        ("namespace Library<N = 2>:\n  def value() -> i32:\n    return N\ndef unused() -> i32:\n  return Library<Data[0]>::value()\nsample:\n  out1 = 0.0\n", 1),
    ] {
        let source = format!("{prefix}{body}");
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let typed = crate::analyze(onda_frontend::parse_program(&source).unwrap())
            .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
        crate::lower_program_to_optimized_mir(&typed).unwrap();
        BODY_EVALUATIONS.with(|counts| {
            let counts = counts.borrow();
            assert_eq!(counts.get("build").copied().unwrap_or(0), builds, "{source}");
            assert_eq!(counts.get("element").copied().unwrap_or(0), 2 * builds, "{source}");
        });
        assert!(typed.const_arrays.is_empty(), "{source}");
    }
}

#[test]
fn sine_tables_are_built_once_only_for_required_processors() {
    for (body, builds) in [
        ("sample:\n  out1 = 0.0\n", 0),
        ("init:\n  oscillator = std::osc::Saw<f32>()\nsample:\n  out1 = oscillator()\n", 0),
        ("init:\n  audio = std::osc::Sine()\n  control = std::osc::KSine()\nsample:\n  out1 = audio() + control.kout1\n", 1),
        ("init:\n  voices: std::osc::Sine[3] = std::osc::Sine()\nsample:\n  out1 = voices[0]() + voices[1]() + voices[2]()\n", 1),
    ] {
        let source = format!("import std/osc\n{body}");
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let typed = crate::analyze(onda_frontend::parse_program(&source).unwrap())
            .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
        let mir = crate::lower_program_to_optimized_mir(&typed).unwrap();
        BODY_EVALUATIONS.with(|counts| {
            assert_eq!(counts.borrow().get("std::osc::_sine_wavetable").copied().unwrap_or(0), builds, "{source}");
        });
        assert_eq!(typed.const_arrays.len(), builds, "{source}");
        assert_eq!(mir.const_data.len(), builds, "{source}");
    }
}

#[test]
fn pipeline_evaluates_each_reached_condition_once() {
    let prefix = "const def enabled() -> bool:\n  total: i32 = 0\n  for i in 0..2:\n    total += 1\n  return total == 2\ndef identity(x: f32) -> f32:\n  return x\n";
    for body in [
        "sample:\n  if enabled():\n    out1 = identity(7.0)\n",
        "def read() -> f32:\n  if enabled():\n    return identity(7.0)\n  return 0.0\nsample:\n  out1 = read()\n",
    ] {
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let source = format!("{prefix}{body}");
        let typed = crate::analyze(onda_frontend::parse_program(&source).unwrap())
            .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
        crate::lower_program_to_optimized_mir(&typed).unwrap();
        BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().get("enabled"), Some(&1), "{source}"));
    }
}

#[test]
fn runtime_array_defaults_cache_and_export_only_used_contexts() {
    let prefix = "const def build() -> i32[1]:\n  return [i32(SR / 48000)]\ndef read(values: i32[1] = build()) -> i32:\n  return values[0]\n";
    for (body, expected) in [
        ("sample 2:\n  out1 = 0.0\n", vec![]),
        ("sample 2:\n  out1 = f32(read([7]))\n", vec![]),
                ("sample 2:\n  out1 = f32(read() + read())\n", vec![2]),
        ("init:\n  initial = read() + read()\nsample 2:\n  out1 = f32(initial + read() + read())\n", vec![1, 2]),
    ] {
        let source = format!("{prefix}{body}");
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let typed = crate::analyze(onda_frontend::parse_program(&source).unwrap()).unwrap();
        let mut values: Vec<_> = typed.const_arrays.iter().map(|array| {
            assert_eq!(array.len, 1);
            let TypedConstValue::I32(value) = array.values[0] else { panic!("unexpected array type") };
            value
        }).collect();
        values.sort();
        assert_eq!(values, expected, "{source}");
        assert_eq!(crate::lower_program_to_optimized_mir(&typed).unwrap().const_data.len(), expected.len());
        BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().get("build").copied().unwrap_or(0), expected.len(), "{source}"));
    }
}

#[test]
fn generic_array_defaults_build_once_per_used_specialization() {
    let prefix = "const def build(value: i32) -> i32[2]:\n  return [value, value + 1]\ndef read<T>(values: i32[2] = build(i32(T(16777217)))) -> i32:\n  return values[0]\n";
    for (body, expected) in [
        ("sample:\n  out1 = 0.0\n", vec![]),
        (
            "sample:\n  out1 = f32(read<i32>([7, 9]) + read<f32>([7, 9]))\n",
            vec![],
        ),
        (
            "sample:\n  out1 = f32(read<i32>() + read<i32>() + read<f32>() + read<f32>())\n",
            vec![16777216, 16777217],
        ),
    ] {
        let source = format!("{prefix}{body}");
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let typed = crate::analyze(onda_frontend::parse_program(&source).unwrap())
            .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
        let mut values = typed
            .const_arrays
            .iter()
            .map(|array| {
                let TypedConstValue::I32(value) = array.values[0] else {
                    panic!("unexpected array type")
                };
                value
            })
            .collect::<Vec<_>>();
        values.sort();
        assert_eq!(values, expected, "{source}");
        assert_eq!(
            crate::lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .len(),
            expected.len()
        );
        BODY_EVALUATIONS.with(|counts| {
            assert_eq!(
                counts.borrow().get("build").copied().unwrap_or(0),
                expected.len(),
                "{source}"
            );
        });
    }
}

#[test]
fn generic_proc_array_defaults_share_specializations_between_instances() {
    let prefix = "const def build(value: i32) -> i32[2]:\n  return [value, value + 1]\nproc Voice<T>:\n  params:\n    values: i32[2] = build(i32(T(16777217)))\n  sample:\n    out1 = f32(values[0] - values[1])\n";
    for (body, builds) in [
        ("sample:\n  out1 = 0.0\n", 0),
        ("init:\n  first = Voice<i32>()\n  second = Voice<i32>()\n  third = Voice<f32>()\nsample:\n  out1 = first() + second() + third()\n", 2),
    ] {
        let source = format!("{prefix}{body}");
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let typed = crate::analyze(onda_frontend::parse_program(&source).unwrap())
            .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
        crate::lower_program_to_optimized_mir(&typed).unwrap();
        BODY_EVALUATIONS.with(|counts| {
            assert_eq!(counts.borrow().get("build").copied().unwrap_or(0), builds, "{source}");
        });
    }
}

#[test]
fn caller_context_failures_do_not_poison_other_contexts() {
    let mut artifacts =
        catalog("const def guarded() -> i32:\n  return 7 / (i32(SR / 48000) - 1)\n");
    let Block::Const(decl) = onda_frontend::parse_program("const Local: i32[1] = [guarded()]\n")
        .unwrap()
        .blocks
        .remove(0)
    else {
        unreachable!()
    };
    let host = AnalysisOptions::default();
    let high = AnalysisOptions {
        sample_rate: 96_000.0,
        ..host
    };
    let mut errors = Vec::new();
    record_const_array_artifact(
        &decl,
        &mut artifacts,
        ConstContext::Caller(host),
        &mut errors,
    );
    assert!(errors.is_empty(), "{errors:?}");
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let block = onda_frontend::parse_program("sample:\n  out1 = Local[0]\n")
        .unwrap()
        .blocks
        .remove(0);
    let Block::Sample(stmts) = block else {
        unreachable!()
    };
    let Stmt::Assign { expr, .. } = &stmts[0] else {
        unreachable!()
    };
    for options in [host, high, host, high] {
        errors.clear();
        let value = materialize_expression(expr, &artifacts, options, &mut errors);
        if options.sample_rate == host.sample_rate {
            assert!(value.is_none());
            assert!(errors
                .iter()
                .any(|error| error.message.contains("division by zero")));
        } else {
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(
                eval_const_expr_i64_exact(&value.unwrap(), options, "value", &mut errors),
                Some(7)
            );
        }
    }
    BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().get("guarded"), Some(&2)));
}

#[test]
fn nested_namespace_declarations_retain_canonical_caches_and_environments() {
    let declarations = "namespace Source<N = 2>:\n  const def build() -> i32[N]:\n    values: i32[N]\n    for i in 0..N:\n      values[i] = N + i\n    return values\n  const Data = build()\n  const Unused: i32 = 3\n  const def read() -> i32:\n    return N\nnamespace Wrapper<P = 2>:\n  const Alias: i32[] = Source<P>::Data\n  const Value: i32 = Source<P>::read()\nnamespace Second<Q = 2>:\n  const Alias: i32[] = Source<7>::Data\nparams:\n  index: i32 = 0\n";
    for expression in [
        "Wrapper<7>::Alias[index] + Source<7>::Data[index]",
        "Source<7>::Data[index] + Wrapper<7>::Alias[index]",
        "Second<1>::Alias[index] + Second<2>::Alias[index] + Wrapper<7>::Alias[index]",
        "Wrapper<7>::Alias[index] + Wrapper<8>::Alias[index]",
    ] {
        let source = format!("{declarations}sample:\n  out1 = f32({expression})\n");
        let mut program = onda_frontend::parse_program(&source).unwrap();
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let artifacts = flatten_namespaces_for_semantics(&mut program, AnalysisOptions::default())
            .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
        BODY_EVALUATIONS.with(|counts| {
            assert!(
                !counts.borrow().keys().any(|name| name.ends_with("::build")),
                "{source}"
            )
        });
        for name in artifacts.const_values.keys() {
            let entry = artifacts.const_values.entry(name).unwrap();
            assert!(
                entry.array.is_none() || entry.cache.evaluated.get().is_none(),
                "{source}\n{name}"
            );
            for dependency in entry.environment.values.keys() {
                assert!(
                    std::ptr::eq(
                        entry.environment.values.entry(dependency).unwrap(),
                        artifacts.const_values.entry(dependency).unwrap(),
                    ),
                    "{source}\n{name} -> {dependency}"
                );
            }
            for (dependency, def) in &entry.environment.defs {
                assert!(
                    std::rc::Rc::ptr_eq(def, &artifacts.const_defs[dependency]),
                    "{source}\n{name} -> {dependency}"
                );
            }
        }
        let mut errors = Vec::new();
        for (name, info) in &artifacts.const_array_infos {
            let Some(ResolvedConstValue::Array(array)) =
                artifacts.const_values.resolve(name, &mut errors)
            else {
                panic!("{source}\n{name}: {errors:?}");
            };
            assert_eq!(array.len(), info.len, "{source}\n{name}");
            assert_eq!(
                array.values[0],
                TypedConstValue::I32(info.len as i32),
                "{source}\n{name}"
            );
        }
        assert!(errors.is_empty(), "{source}\n{errors:?}");
        BODY_EVALUATIONS.with(|counts| {
            let counts = counts.borrow();
            assert_eq!(
                counts
                    .keys()
                    .filter(|name| name.ends_with("::build"))
                    .count(),
                artifacts
                    .const_defs
                    .keys()
                    .filter(|name| name.ends_with("::build"))
                    .count(),
                "{source}\n{counts:?}"
            );
            assert!(
                counts
                    .iter()
                    .filter(|(name, _)| name.ends_with("::build"))
                    .all(|(_, count)| *count == 1),
                "{source}\n{counts:?}"
            );
        });
        assert!(
            artifacts
                .const_values
                .keys()
                .filter(|name| name.ends_with("::Unused"))
                .all(|name| matches!(
                    artifacts.const_values.get(name),
                    Some(ResolvedConstValue::Scalar(_))
                )),
            "{source}"
        );
    }
}

#[test]
fn length_only_array_arguments_do_not_execute_or_cache_their_builder() {
    for len in [131072, i32::MAX] {
        let source = format!("const def build() -> i32[{len}]:\n  values: i32[{len}]\n  values[0] = 1 / 0\n  return values\nconst Table: i32[{len}] = build()\nconst def length(xs: i32[]) -> i32:\n  copy: i32[Table.len() - 1] = xs\n  return copy.len()\nconst def forward(xs: []) -> i32:\n  return length(xs[1:])\nconst Selected: i32 = forward(Table)\nconst Total: i32 = Table.len()\n");
        let artifacts = catalog(&source);
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let mut errors = Vec::new();
        for (name, expected) in [("Selected", len - 1), ("Total", len)] {
            assert_eq!(
                artifacts.const_values.resolve(name, &mut errors),
                Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(expected))),
                "{source}\n{errors:?}"
            );
        }
        assert!(errors.is_empty(), "{errors:?}");
        assert!(artifacts
            .const_values
            .entry("Table")
            .unwrap()
            .cache
            .evaluated
            .get()
            .is_none());
        BODY_EVALUATIONS.with(|counts| assert!(!counts.borrow().contains_key("build")));
    }
}

#[test]
fn deferred_array_arguments_share_the_first_payload_evaluation() {
    let artifacts = catalog("const def build() -> i32[2]:\n  values: i32[2] = [7, 9]\n  return values\nconst Table: i32[2] = build()\nconst def first(xs: i32[]) -> i32:\n  return xs[0]\nconst def change(source: i32[2]) -> i32:\n  xs: i32[2] = source\n  xs[0] = 99\n  return xs[0]\nconst Selected: i32 = first(Table) + first(Table[1:]) + change(Table) + first(Table)\n");
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let mut errors = Vec::new();
    assert_eq!(
        artifacts.const_values.resolve("Selected", &mut errors),
        Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(122))),
        "{errors:?}"
    );
    assert!(errors.is_empty(), "{errors:?}");
    BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().get("build"), Some(&1)));
}

#[test]
fn const_def_loops_preserve_induction_types_and_stop_before_overflow() {
    for (range, expected) in [
        ("i: i32 in 4294967296..1", 7),
        ("i: i32 @ -4294967295 in 0..1", 7),
        ("i: i32 in 0..4294967296", 0),
        ("i: i64 in 1..2", 8),
        ("i: i32 in 2147483647..=2147483647", 2147483654),
        (
            "i: i64 in 9223372036854775807..=9223372036854775807",
            i64::MIN + 6,
        ),
        (
            "i: i64 @ -9223372036854775808 in 9223372036854775807..=-1",
            6,
        ),
    ] {
        let source = format!("const def read() -> i64:\n  result: i64 = 0\n  for {range}:\n    result = i64(i) + 7\n  return result\nconst Selected: i64 = read()\n");
        let artifacts = catalog(&source);
        let mut errors = Vec::new();
        assert_eq!(
            artifacts.const_values.resolve("Selected", &mut errors),
            Some(&ResolvedConstValue::Scalar(TypedConstValue::I64(expected))),
            "{source}\n{errors:?}"
        );
        assert!(errors.is_empty(), "{source}\n{errors:?}");
    }
    let artifacts = catalog("const def wide() -> i64:\n  for i: i64 in 1..2:\n    return i + 2147483647\n  return 0\nconst Selected = wide()\n");
    let mut errors = Vec::new();
    assert_eq!(
        artifacts.const_values.resolve("Selected", &mut errors),
        Some(&ResolvedConstValue::Scalar(TypedConstValue::I64(
            2147483648
        )))
    );
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn float_destinations_preserve_concrete_operand_precision() {
    for (expression, expected) in [
        ("f32(16777216.0) + f32(1.0)", 16777216.0),
        ("f32(16777216.0) + 1.0", 16777216.0),
        ("1.0 + f32(16777216.0)", 16777216.0),
        ("f32(16777217.0 - 16777216.0)", 1.0),
        ("f32(f64(16777217.0) - f64(16777216.0))", 1.0),
        ("f64(f32(16777216.0)) + 1.0", 16777217.0),
        ("16777216.0 + 1.0", 16777217.0),
    ] {
        let source = format!("const Scalar: f64 = {expression}\nconst Array: f64[1] = [{expression}]\nconst def read() -> f64:\n  return {expression}\nconst FromDef: f64 = read()\nconst def defaulted(value: f64 = {expression}) -> f64:\n  return value\nconst FromDefault: f64 = defaulted()\n");
        let artifacts = catalog(&source);
        let mut errors = Vec::new();
        for name in ["Scalar", "FromDef", "FromDefault"] {
            assert_eq!(
                artifacts.const_values.resolve(name, &mut errors),
                Some(&ResolvedConstValue::Scalar(TypedConstValue::F64(expected))),
                "{source}\n{name}: {errors:?}"
            );
        }
        let Some(ResolvedConstValue::Array(array)) =
            artifacts.const_values.resolve("Array", &mut errors)
        else {
            panic!("{source}\n{errors:?}")
        };
        assert_eq!(*array.values, [TypedConstValue::F64(expected)], "{source}");
        assert!(errors.is_empty(), "{source}\n{errors:?}");
    }
}

#[test]
fn float_literal_context_preserves_exact_integer_subexpressions() {
    let artifacts = catalog("const Rounded: f32 = 16777217.0 - 16777216.0\nconst Exact: f32 = (9007199254740993 - 9007199254740992) * 1.0\n");
    let mut errors = Vec::new();
    for (name, expected) in [("Rounded", 0.0), ("Exact", 1.0)] {
        assert_eq!(
            artifacts.const_values.resolve(name, &mut errors),
            Some(&ResolvedConstValue::Scalar(TypedConstValue::F32(expected))),
            "{name}: {errors:?}"
        );
    }
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn cached_array_arguments_share_storage_and_mutation_copies_only_the_written_value() {
    let artifacts = catalog("const Table: i32[2] = [7, 9]\nconst def identity(xs: i32[2]) -> i32[2]:\n  return xs\nconst def view(xs: i32[]) -> i32[2]:\n  return xs\nconst def change(source: i32[2]) -> i32[2]:\n  xs: i32[2] = source\n  xs[0] = 99\n  return xs\nconst Alias: i32[2] = Table\nconst Copy: i32[2] = identity(Table)\nconst View: i32[2] = view(Table)\nconst Changed: i32[2] = change(Table)\n");
    let mut errors = Vec::new();
    let resolve = |name: &str, errors: &mut Vec<Diagnostic>| {
        let Some(ResolvedConstValue::Array(array)) = artifacts.const_values.resolve(name, errors)
        else {
            panic!("missing array {name}")
        };
        array
    };
    let table = resolve("Table", &mut errors);
    for name in ["Alias", "Copy", "View"] {
        let array = resolve(name, &mut errors);
        assert!(
            std::sync::Arc::ptr_eq(&table.values, &array.values),
            "{name} copied the array"
        );
    }
    let changed = resolve("Changed", &mut errors);
    assert!(!std::sync::Arc::ptr_eq(&table.values, &changed.values));
    assert_eq!(
        *table.values,
        [TypedConstValue::I32(7), TypedConstValue::I32(9)]
    );
    assert_eq!(
        *changed.values,
        [TypedConstValue::I32(99), TypedConstValue::I32(9)]
    );
    assert!(errors.is_empty(), "{errors:?}");
}

fn catalog(source: &str) -> SemanticConstArtifacts {
    let program = onda_frontend::parse_program(source).unwrap();
    let mut artifacts = SemanticConstArtifacts::default();
    let mut errors = Vec::new();
    for block in program.blocks {
        match block {
            Block::Def(def) => record_const_def_artifact(
                &mut artifacts,
                def,
                AnalysisOptions::default(),
                &mut errors,
            ),
            Block::Const(decl) if is_const_array_decl(&decl) => record_const_array_artifact(
                &decl,
                &mut artifacts,
                ConstContext::Declaration(AnalysisOptions::default()),
                &mut errors,
            ),
            Block::Const(decl) => register_scalar_const(
                &decl,
                &mut artifacts,
                AnalysisOptions::default(),
                &mut errors,
            ),
            _ => unreachable!(),
        }
    }
    assert!(errors.is_empty(), "{errors:?}");
    artifacts
}

#[test]
fn dependency_reads_resume_without_repeating_calls_or_local_initializers() {
    let artifacts = catalog(
        r#"
const def expensive() -> i32:
  sum: i32 = 0
  for i in 0..1024:
    sum = sum + i
  return sum
const A: i32 = 7
const B: i32 = 11
const def build() -> i32:
  prefix = expensive()
  if true:
    prefix = prefix + A
  return prefix + B
const Result: i32 = build()
"#,
    );
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let mut errors = Vec::new();
    for _ in 0..3 {
        assert_eq!(
            artifacts.const_values.resolve("Result", &mut errors),
            Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(
                1023 * 1024 / 2 + 18
            )))
        );
    }
    assert!(errors.is_empty(), "{errors:?}");
    BODY_EVALUATIONS.with(|counts| {
        let counts = counts.borrow();
        assert_eq!(counts.get("expensive"), Some(&1));
        assert_eq!(counts.get("build"), Some(&1));
    });
}

#[test]
fn failures_are_cached_without_reexecuting_suspended_initializers() {
    let artifacts = catalog(
        r#"
const Invalid: i32 = 1 / 0
const def build() -> i32:
  sum: i32 = 0
  for i in 0..1024:
    sum = sum + i
  return sum + Invalid
const Result: i32 = build()
"#,
    );
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let mut errors = Vec::new();
    assert!(artifacts
        .const_values
        .resolve("Result", &mut errors)
        .is_none());
    let first_errors = errors.clone();
    assert!(!first_errors.is_empty());
    assert!(artifacts
        .const_values
        .resolve("Result", &mut errors)
        .is_none());
    assert!(artifacts
        .const_values
        .resolve("Invalid", &mut errors)
        .is_none());
    assert_eq!(errors, first_errors);
    BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().get("build"), Some(&1)));
}

#[test]
fn length_and_discarded_operands_do_not_execute_array_initializers() {
    let artifacts = catalog(
        r#"
const def build() -> i32[2]:
  values: i32[2]
  values[0] = 1 / 0
  return values
const Table: i32[2] = build()
const Size: i32 = Table.len()
const Skip: bool = true || Table[0] > 0
"#,
    );
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let mut errors = Vec::new();
    assert_eq!(
        artifacts.const_values.resolve("Size", &mut errors),
        Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(2)))
    );
    assert_eq!(
        artifacts.const_values.resolve("Skip", &mut errors),
        Some(&ResolvedConstValue::Scalar(TypedConstValue::Bool(true)))
    );
    assert!(artifacts.const_values.get("Table").is_none());
    assert!(errors.is_empty(), "{errors:?}");
    BODY_EVALUATIONS.with(|counts| assert!(counts.borrow().is_empty()));
}

#[test]
fn cached_parent_failures_retain_previously_reported_dependency_diagnostics() {
    let artifacts = catalog("const Invalid: i32 = 1 / 0\nconst Parent: i32 = Invalid + 1\n");
    let mut errors = Vec::new();
    assert!(artifacts
        .const_values
        .resolve("Invalid", &mut errors)
        .is_none());
    let expected = errors.clone();
    assert!(artifacts
        .const_values
        .resolve("Parent", &mut errors)
        .is_none());
    assert_eq!(errors, expected);
    let mut fresh = Vec::new();
    assert!(artifacts
        .const_values
        .resolve("Parent", &mut fresh)
        .is_none());
    assert_eq!(fresh, expected);
}

#[test]
fn array_elements_execute_once_and_stop_at_the_first_invalid_element() {
    let artifacts = catalog(
        r#"
const def element(x: i32) -> i32:
  return x
const Table = [element(2), element(3)]
const Invalid: i32[2] = [1 / 0, element(4)]
"#,
    );
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let mut errors = Vec::new();
    assert!(artifacts
        .const_values
        .resolve("Table", &mut errors)
        .is_some());
    assert!(errors.is_empty());
    assert!(artifacts
        .const_values
        .resolve("Invalid", &mut errors)
        .is_none());
    assert!(!errors.is_empty());
    BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().get("element"), Some(&2)));
}

#[test]
fn declarations_capture_direct_references_once_without_evaluating_unused_helpers() {
    let count = 1024;
    let mut source = "const def f0() -> i32:\n  return 7\n".to_owned();
    for index in 1..count {
        source.push_str(&format!(
            "const def f{index}() -> i32:\n  return f{}()\n",
            index - 1
        ));
    }
    for index in 0..count {
        source.push_str(&format!("const C{index}: i32 = f{}()\n", count - 1));
    }
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let artifacts = catalog(&source);
    let def_references = artifacts
        .const_defs
        .values()
        .map(|def| def.environment.defs.len())
        .sum::<usize>();
    let initializer_references = artifacts
        .const_values
        .keys()
        .map(|name| {
            artifacts
                .const_values
                .entry(name)
                .unwrap()
                .environment
                .defs
                .len()
        })
        .sum::<usize>();
    assert_eq!(def_references, count - 1);
    assert_eq!(initializer_references, count);
    assert!(artifacts.const_values.iter().next().is_none());
    BODY_EVALUATIONS.with(|counts| assert!(counts.borrow().is_empty()));
    let mut errors = Vec::new();
    assert_eq!(
        artifacts.const_values.resolve("C0", &mut errors),
        Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(7)))
    );
    assert!(errors.is_empty(), "{errors:?}");
    BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().len(), count));
}

#[test]
fn captured_def_environments_keep_global_reads_and_defaults_lexical() {
    let artifacts = catalog("const Value: i32 = 7\nconst def read(Value: i32, x: i32 = Value) -> i32:\n  return x\nconst def outer(Value: i32) -> i32:\n  return read(42)\nconst Result: i32 = outer(99)\n");
    let mut errors = Vec::new();
    assert_eq!(
        artifacts.const_values.resolve("Result", &mut errors),
        Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(7)))
    );
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn deep_lexical_def_environments_evaluate_and_drop_on_a_worker_stack() {
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            let count = 16384;
            let mut source = "const def f0() -> i32:\n  return 7\n".to_owned();
            for index in 1..count {
                source.push_str(&format!(
                    "const def f{index}() -> i32:\n  return f{}()\n",
                    index - 1
                ));
            }
            source.push_str(&format!("const Result: i32 = f{}()\n", count - 1));
            let artifacts = catalog(&source);
            let mut errors = Vec::new();
            assert_eq!(
                artifacts.const_values.resolve("Result", &mut errors),
                Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(7)))
            );
            assert!(errors.is_empty(), "{errors:?}");
            drop(artifacts);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn namespace_specializations_evaluate_once_each_and_length_queries_stay_lazy() {
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let source = "namespace LUT<N = 2>:\n  const def build() -> i32[N]:\n    values: i32[N]\n    for i in 0..N:\n      values[i] = i + N\n    return values\n  const Table = build()\nnamespace A = LUT<2>\nnamespace B = LUT<3>\nnamespace C = LUT<4>\nconst View = C::Table\nsample:\n  out1 = f32(A::Table[0] + A::Table[1] + B::Table[0] + B::Table[2] + View.len())\n";
    let typed = analyze(onda_frontend::parse_program(source).unwrap()).unwrap();
    assert!(typed.const_arrays.is_empty());
    BODY_EVALUATIONS.with(|counts| {
        let counts = counts.borrow();
        assert_eq!(counts.len(), 2);
        assert!(counts.values().all(|count| *count == 1));
    });
    assert!(lower_program_to_optimized_mir(&typed)
        .unwrap()
        .const_data
        .is_empty());
}

#[test]
fn concrete_const_body_checks_cache_types_and_shapes_without_values() {
    let source = "const def read(xs: []) -> i32:\n  return i32(xs[0])\nconst A: i32 = read([7])\nconst B: i32 = read([9])\nconst C: i32 = read([11, 13])\nconst D: i32 = read([true])\nconst E: i32 = read([f32(17.0)])\n";
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let artifacts = catalog(source);
    let def = &artifacts.const_defs["read"];
    assert_eq!(def.type_checks.borrow().len(), 4);
    assert!(artifacts.const_values.iter().next().is_none());
    BODY_EVALUATIONS.with(|counts| assert!(counts.borrow().is_empty()));
    let mut errors = Vec::new();
    for (name, expected) in [("A", 7), ("B", 9), ("C", 11), ("D", 1), ("E", 17)] {
        assert_eq!(
            artifacts.const_values.resolve(name, &mut errors),
            Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(expected)))
        );
    }
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        def.type_checks.borrow().len(),
        4,
        "evaluation reuses metadata checks"
    );
    BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().get("read"), Some(&5)));
}

#[test]
fn concrete_const_body_check_failures_are_cached() {
    let artifacts = catalog("const def read(xs: []) -> i32:\n  return xs[0]\n");
    let def = &artifacts.const_defs["read"];
    let metadata = [ConstArrayExpectation::fixed(PrimitiveType::Bool, 1)];
    let mut errors = Vec::new();
    assert!(validate_const_call_metadata(def, &metadata, &mut errors).is_none());
    assert!(validate_const_call_metadata(def, &metadata, &mut errors).is_none());
    assert_eq!(def.type_checks.borrow().len(), 1);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("cannot assign Bool to I32"));
}

#[test]
fn concrete_const_checks_and_evaluation_use_a_bounded_worker_stack() {
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            let mut source = "const def read0(xs: []) -> i32:\n  return i32(xs[0])\n".to_owned();
            for index in 1..1024 {
                source.push_str(&format!(
                    "const def read{index}(xs: []) -> i32:\n  return read{}(xs)\n",
                    index - 1
                ));
            }
            source.push_str("const Table: i32[1] = [7]\nconst Result: i32 = read1023(Table)\n");
            let artifacts = catalog(&source);
            let mut errors = Vec::new();
            assert_eq!(
                artifacts.const_values.resolve("Result", &mut errors),
                Some(&ResolvedConstValue::Scalar(TypedConstValue::I32(7)))
            );
            assert!(errors.is_empty(), "{errors:?}");
            for (name, def) in &artifacts.const_defs {
                assert_eq!(
                    def.type_checks.borrow().len(),
                    if name == "read1023" { 1 } else { 2 }
                );
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn concrete_const_check_failures_propagate_and_cache_through_calls() {
    let artifacts = catalog("const def first(xs: []) -> i32:\n  return xs[0]\nconst def twice(xs: []) -> i32:\n  return first(xs) + first(xs)\n");
    let def = &artifacts.const_defs["twice"];
    let metadata = [ConstArrayExpectation::fixed(PrimitiveType::Bool, 1)];
    let mut errors = Vec::new();
    assert!(validate_const_call_metadata(def, &metadata, &mut errors).is_none());
    assert_eq!(errors.len(), 1);
    errors.clear();
    assert!(validate_const_call_metadata(def, &metadata, &mut errors).is_none());
    assert_eq!(errors.len(), 1, "the caller caches transitive failures");
    assert!(errors[0].message.contains("cannot assign Bool to I32"));
}

#[test]
fn checked_local_types_survive_evaluation_and_branch_joins() {
    for (body, expected) in [
        ("  x: i64 = 2147483647\n  y: i32 = 1\n  z = x + y\n  return z - 1\n", 2147483647),
        ("  x: i64 = 2147483647\n  y: i32 = 1\n  z = x + y\n  z = z - 1\n  return z\n", 2147483647),
        ("  x: f64 = 16777217.0\n  y: f32 = 0.0\n  z = x + y\n  z = z + 1.0\n  return i64(z)\n", 16777218),
        ("  x: f64 = 16777217.0\n  return i64(x - f64(16777216.0))\n", 1),
        ("  if true:\n    x = i32(7)\n  else:\n    x = i64(4294967297)\n  return x + 2147483647\n", 2147483654),
        ("  if true:\n    x = f32(16777216.0)\n  else:\n    x = f64(1.0)\n  return i64(x + f64(1.0))\n", 16777217),
        ("  xs: i32[1] = [7]\n  ys: i32[1] = xs\n  ys[0] = 9\n  return i64(xs[0] + ys[0])\n", 16),
        ("  ys: i32[1] = make()\n  return i64(ys[0])\n", 7),
        ("  ys = make()\n  return i64(ys[0])\n", 7),
        ("  n = 1\n  ys: i32[n] = make()\n  return i64(ys[0])\n", 7),
    ] {
        let source = format!("const def make() -> i32[1]:\n  return [7]\nconst def read() -> i64:\n{body}const Result: i64 = read()\n");
        let artifacts = catalog(&source);
        let mut errors = Vec::new();
        assert_eq!(artifacts.const_values.resolve("Result", &mut errors), Some(&ResolvedConstValue::Scalar(TypedConstValue::I64(expected))), "{source}\n{errors:?}");
        assert!(errors.is_empty(), "{source}\n{errors:?}");
        assert_eq!(artifacts.const_defs["read"].type_checks.borrow().len(), 1);
    }
}

#[test]
fn runtime_boolean_chains_do_not_reprobe_unknown_prefixes() {
    let artifacts = catalog("const Broken: bool = 1 / 0 == 0\n");
    let mut expr = Expr::var("enabled");
    for _ in 0..4096 {
        expr = Expr::Logical {
            loc: Default::default(),
            op: onda_frontend::LogicalOp::And,
            lhs: Box::new(expr),
            rhs: Box::new(Expr::bool(true)),
        };
        expr = Expr::Compare {
            loc: Default::default(),
            op: CmpOp::Eq,
            lhs: Box::new(expr),
            rhs: Box::new(Expr::bool(true)),
        };
    }
    LOGICAL_PROBES.with(|count| count.set(0));
    let mut errors = Vec::new();
    let folded =
        materialize_expression(&expr, &artifacts, AnalysisOptions::default(), &mut errors).unwrap();
    assert_eq!(
        folded
            .walk()
            .filter(|node| matches!(node, Expr::Logical { .. }))
            .count(),
        4096
    );
    assert_eq!(
        folded
            .walk()
            .filter(|node| matches!(node, Expr::Compare { .. }))
            .count(),
        4096
    );
    assert!(errors.is_empty(), "{errors:?}");
    LOGICAL_PROBES.with(|count| assert_eq!(count.get(), 0));
    let expr = Expr::Logical {
        loc: Default::default(),
        op: onda_frontend::LogicalOp::And,
        lhs: Box::new(Expr::Compare {
            loc: Default::default(),
            op: CmpOp::Eq,
            lhs: Box::new(Expr::Logical {
                loc: Default::default(),
                op: onda_frontend::LogicalOp::And,
                lhs: Box::new(expr),
                rhs: Box::new(Expr::bool(false)),
            }),
            rhs: Box::new(Expr::bool(true)),
        }),
        rhs: Box::new(Expr::var("Broken")),
    };
    let (folded, facts) = materialize_expression_with_facts(
        &expr,
        &artifacts,
        AnalysisOptions::default(),
        None,
        &mut errors,
    )
    .unwrap();
    assert_eq!(facts.boolean, Some(false));
    assert!(!facts.constant);
    assert_eq!(
        folded
            .walk()
            .filter(|node| matches!(node, Expr::Logical { .. }))
            .count(),
        4097
    );
    assert_eq!(
        folded
            .walk()
            .filter(|node| matches!(node, Expr::Compare { .. }))
            .count(),
        4097
    );
    assert!(errors.is_empty(), "{errors:?}");
    LOGICAL_PROBES.with(|count| assert_eq!(count.get(), 0));
    let expr = Expr::Logical {
        loc: Default::default(),
        op: onda_frontend::LogicalOp::And,
        lhs: Box::new(Expr::bool(false)),
        rhs: Box::new(Expr::var("Broken")),
    };
    assert!(matches!(
        materialize_expression(&expr, &artifacts, AnalysisOptions::default(), &mut errors),
        Some(Expr::Bool { value: false, .. })
    ));
    assert!(artifacts.const_values.get("Broken").is_none());
}

#[test]
fn array_defaults_share_the_lazy_catalog_across_element_uses() {
    let mut artifacts = catalog("const def build() -> i32[2]:\n  return [7, 9]\n");
    let mut block = onda_frontend::parse_program(
        "proc Value:\n  params:\n    values: i32[2] = build()\n  sample:\n    out1 = 0.0\n",
    )
    .unwrap()
    .blocks
    .remove(0);
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let mut errors = Vec::new();
    normalize_declaration_metadata(
        &mut block,
        &mut artifacts,
        AnalysisOptions::default(),
        &mut errors,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(artifacts.const_values.iter().count(), 0);
    BODY_EVALUATIONS.with(|counts| assert!(counts.borrow().is_empty()));
    let Block::Proc(proc) = block else {
        unreachable!()
    };
    assert!(proc.params[0].default.as_ref().unwrap().walk().count() <= 3);
    let (specs, _) = crate::proc_call_rewrite::expand_proc_param_specs(
        &proc.name,
        &proc.params,
        AnalysisOptions::default(),
        &artifacts.const_array_infos,
        &mut errors,
    );
    for _ in 0..2 {
        assert_eq!(
            specs[0]
                .slots
                .iter()
                .map(|slot| {
                    let expr = materialize_expression(
                        slot.default.as_ref().unwrap(),
                        &artifacts,
                        AnalysisOptions::default(),
                        &mut errors,
                    )
                    .unwrap();
                    eval_const_expr_i64_exact(
                        &expr,
                        AnalysisOptions::default(),
                        "element",
                        &mut errors,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>(),
            [7, 9]
        );
    }
    assert!(errors.is_empty(), "{errors:?}");
    BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow()["build"], 1));
}

#[test]
fn unused_array_defaults_retain_bounded_metadata_without_building_elements() {
    for len in [1, 32768, 131072] {
        let mut artifacts = catalog(&format!(
            "const def build() -> i32[{len}]:\n  values: i32[{len}]\n  values[0] = 1 / 0\n  return values\nconst Table: i32[{len}] = build()\n"
        ));
        let mut block = onda_frontend::parse_program(&format!(
            "proc Unused:\n  event read(values: i32[{len}] = Table):\n    local = values[0]\n  event read_built(values: i32[{len}] = build()):\n    local = values[0]\n  sample:\n    out1 = 0.0\n"
        )).unwrap().blocks.remove(0);
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        let mut errors = Vec::new();
        normalize_declaration_metadata(
            &mut block,
            &mut artifacts,
            AnalysisOptions::default(),
            &mut errors,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let Block::Proc(proc) = block else {
            unreachable!()
        };
        for event in proc.events {
            let default = event.params[0].default.as_ref().unwrap();
            assert!(default.walk().count() <= 3);
        }
        assert_eq!(artifacts.const_values.iter().count(), 0);
        BODY_EVALUATIONS.with(|counts| assert!(counts.borrow().is_empty()));
    }
}

#[test]
fn fixed_copies_demand_and_share_deferred_slice_length_proofs() {
    let source = "const def bound() -> i32:\n  return 1\nconst Unused: i32[4] = [1 / 0, 0, 0, 0]\ninit:\n  values: f32[4] = [1.0, 2.0, 3.0, 4.0]\nsample:\n  view = values[bound() + Unused.len() - 4:]\n  alias: f32[] = view\n  nested = alias[:2]\n  first: f32[2] = nested\n  second: f32[2] = nested\n  out1 = first[0] + second[0]\n";
    BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
    let typed = crate::analyze(onda_frontend::parse_program(source).unwrap())
        .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
    assert!(typed.const_arrays.is_empty());
    crate::lower_program_to_optimized_mir(&typed).unwrap();
    BODY_EVALUATIONS.with(|counts| assert_eq!(counts.borrow().get("bound"), Some(&2)));
    // One evaluation supplies the required storage proof; another materializes
    // the executable bound. Repeated copies must reuse the same proof.
}

#[test]
fn literal_and_generated_defaults_cache_once_per_context_with_value_semantics() {
    for callable in ["", "const "] {
        let parameters = if callable.is_empty() {
            &["i32[1]"][..]
        } else {
            &["i32[1]", "i32[]", "[]"][..]
        };
        for parameter in parameters {
            for default in ["[element()]", "build()"] {
                let prefix = format!(
            "const def element() -> i32:\n  return i32(SR / 48000)\nconst def build() -> i32[1]:\n  return [element()]\n{callable}def change(values: {parameter} = {default}) -> i32:\n  original = values[0]\n  values[0] = 9\n  return original\n"
        );
                for (body, contexts) in [
            ("sample 2:\n  out1 = 0.0\n", 0),
            ("sample 2:\n  out1 = f32(change([7]))\n", 0),
            ("sample 2:\n  out1 = f32(change() + change())\n", 1),
            ("init:\n  initial = change() + change()\nsample 2:\n  out1 = f32(initial + change() + change())\n", 2),
        ] {
            let source = format!("{prefix}{body}");
            BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
            let typed = crate::analyze(onda_frontend::parse_program(&source).unwrap())
                .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
            assert_eq!(typed.const_arrays.len(), if callable.is_empty() { contexts } else { 0 }, "{source}");
            crate::lower_program_to_optimized_mir(&typed).unwrap();
            BODY_EVALUATIONS.with(|counts| {
                assert_eq!(counts.borrow().get("element").copied().unwrap_or(0), contexts, "{source}");
            });
        }
            }
        }
    }
}

#[test]
fn repeated_default_normalization_retains_one_declaration() {
    for default in ["[element()]", "build()"] {
        let mut artifacts = catalog("const def element() -> i32:\n  return 7\nconst def build() -> i32[1]:\n  return [element()]\n");
        let block = onda_frontend::parse_program(&format!(
            "def read(values: i32[1] = {default}) -> i32:\n  return values[0]\n"
        ))
        .unwrap()
        .blocks
        .remove(0);
        let Block::Def(mut def) = block else {
            unreachable!()
        };
        let mut errors = Vec::new();
        BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
        for _ in 0..4 {
            normalize_function_signature(
                &mut def,
                &HashSet::new(),
                &mut artifacts,
                AnalysisOptions::default(),
                &mut errors,
            );
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(artifacts.const_array_infos.len(), 1);
            assert_eq!(artifacts.const_values.keys().count(), 1);
            assert_eq!(artifacts.const_values.iter().count(), 0);
        }
        BODY_EVALUATIONS.with(|counts| assert!(counts.borrow().is_empty()));
    }
}

#[test]
fn default_length_queries_do_not_evaluate_initializer_elements() {
    for parameter in ["i32[1]", "i32[]", "[]"] {
        for default in ["[element()]", "build()"] {
            let source = format!(
                "const def element() -> i32:\n  return 1 / 0\nconst def build() -> i32[1]:\n  return [element()]\nconst def length(values: {parameter} = {default}) -> i32:\n  return values.len()\nsample:\n  out1 = f32(length() + length())\n"
            );
            BODY_EVALUATIONS.with(|counts| counts.borrow_mut().clear());
            let typed = crate::analyze(onda_frontend::parse_program(&source).unwrap())
                .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
            assert!(typed.const_arrays.is_empty());
            assert!(crate::lower_program_to_optimized_mir(&typed)
                .unwrap()
                .const_data
                .is_empty());
            BODY_EVALUATIONS.with(|counts| {
                assert!(!counts.borrow().contains_key("element"));
                assert!(!counts.borrow().contains_key("build"));
            });
        }
    }
}
