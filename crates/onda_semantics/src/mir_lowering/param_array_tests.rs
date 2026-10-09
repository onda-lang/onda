use super::tests::lower_test_program;
use crate::analyze;
use onda_frontend::parse_program;
use onda_mir::{ConstantValue, ScalarValue};

#[test]
fn array_defaults_broadcast_and_singletons_keep_array_metadata() {
    for ty in ["f32", "f64", "i32", "i64", "bool"] {
        let (default, range) = if ty == "bool" {
            ("true", "")
        } else {
            ("2", "{0, 10}")
        };
        let source =
            format!("params:\n  values: {ty}[1] = {default} {range}\nsample:\n  out1 = 0.0\n");
        let typed = analyze(parse_program(&source).unwrap()).unwrap();
        let mir = lower_test_program(&typed).unwrap();
        let param = &mir.interface.params[0];
        assert!(matches!(
            mir.types[param.ty.index()],
            onda_mir::Type::Array { len: 1, .. }
        ));
        assert!(matches!(&param.default, ConstantValue::Aggregate(values) if values.len() == 1));
        assert_eq!(param.range.is_some(), ty != "bool");
        // Unused ranged arrays should not allocate a mirror or generate copies.
        assert!(!mir
            .state
            .iter()
            .any(|state| state.name.contains("clamped_array")));
    }
    let typed = analyze(
        parse_program("params:\n  values: f32[3] = 2.0 {0, 1}\nsample:\n  out1 = values[0]\n")
            .unwrap(),
    )
    .unwrap();
    let mir = lower_test_program(&typed).unwrap();
    assert_eq!(
        mir.interface.params[0].default,
        ConstantValue::Aggregate(vec![ConstantValue::Scalar(ScalarValue::F32(1.0)); 3])
    );
}

#[test]
fn array_domains_validate_every_default_and_reject_invalid_shapes() {
    for (declaration, message) in [
        ("values: f32[2] = [0.0] {0, 1}", "expects 2 elements"),
        ("values: f32[2] = [0.0, 0.3] {0, 1, step = 0.5}", "step"),
        ("values: i32[2] = [0, 3] {0, 10, step = 2}", "step"),
        ("values: f64[2] = 1.0 {0, 10, log}", "log"),
        ("values: i64[2] = 0 {0, 9007199254740992}", "exact"),
        ("values: bool[2] = true {0, 1}", "bool"),
    ] {
        let source = format!("params:\n  {declaration}\nsample:\n  out1 = 0.0\n");
        let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(message)),
            "{declaration}: {errors:?}"
        );
    }
}

#[test]
fn proc_array_ranges_validate_element_types_and_defaults() {
    for declaration in [
        "values: bool[2] = true {0, 1}",
        "values: f32[2] = 0 {2, 1}",
        "values: f32[2] = [0] {0, 1}",
        "values: f32[2] = 0 {0, 1, unit = \"Hz\"}",
    ] {
        let source = format!("proc Voice:\n  params:\n    {declaration}\n  sample:\n    out1 = 0.0\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
        assert!(
            parse_program(&source).and_then(analyze).is_err(),
            "{declaration}"
        );
    }
    let source = "proc Voice:\n  params:\n    values: i64[2] = 8 {0, 4}\n  sample:\n    out1 = f32(values[0] + values[1])\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
    let typed = analyze(parse_program(source).unwrap()).unwrap();
    lower_test_program(&typed).unwrap();
}

#[test]
fn unread_array_arguments_do_not_allocate_snapshots() {
    for (ty, argument) in [
        ("f32[1024]", "gains"),
        ("f32[]", "gains"),
        ("f32[]", "gains[0:1024]"),
    ] {
        for result in ["1.0", "f32(values.len())"] {
            let source = format!(
                "params:\n  gains: f32[1024] = 0.5 {{0, 1}}\n\
                 def ignore(values: {ty}):\n  return {result}\n\
                 def forward(values: {ty}):\n  return ignore(values)\n\
                 init:\n  captured = forward({argument})\n\
                 event capture():\n  captured = forward({argument})\n\
                 sample:\n  out1 = forward({argument}) + captured\n"
            );
            let typed = analyze(parse_program(&source).unwrap()).unwrap();
            let mir = lower_test_program(&typed).unwrap();
            assert!(
                mir.state
                    .iter()
                    .all(|state| !state.name.contains("clamped_array")),
                "{ty}: {argument}"
            );
            assert!(
                mir.functions
                    .iter()
                    .all(|function| function.locals.iter().all(|local| {
                        !matches!(mir.types[local.ty.index()], onda_mir::Type::Array { .. })
                    })),
                "{ty}: {argument}"
            );
            let text = onda_mir::format_program(&mir);
            assert!(!text.contains("range_clamp"), "{ty}: {argument}\n{text}");
        }
    }
}
