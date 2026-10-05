use super::*;

#[test]
fn inferred_borrowed_parameters_reject_defaults_before_runtime_expansion() {
    for default in ["make()", "make(1.0)"] {
        for argument in ["", "Settings()"] {
            let source = format!(
                "struct Settings:\n  gain: f32 = 3.0\ndef make(value: f32 = 1.0) -> Settings:\n  return Settings(gain = value)\ndef use_default(value = {default}) -> f32:\n  return value.gain\nsample:\n  out1 = use_default({argument})\n"
            );
            let errors = analyze(parse_program(&source).unwrap())
                .expect_err("inferred struct parameters cannot have defaults");
            assert!(
                errors.iter().any(|error| error.message.contains(
                    "function parameter 'use_default.value' borrows storage and cannot have a default value"
                )),
                "{source}\n{errors:?}"
            );
        }
    }
}

fn fanout_source(depth: usize, use_site: &str) -> String {
    let mut source = "def d0(x: f32 = 7.0) -> f32:\n  return x\n".to_owned();
    for n in 1..=depth {
        source.push_str(&format!(
            "def d{n}(x: f32 = d{}() + d{}()) -> f32:\n  return x\n",
            n - 1,
            n - 1
        ));
    }
    source.push_str(use_site);
    source
}

#[test]
fn unused_and_overridden_default_graphs_are_not_expanded() {
    for use_site in [
        "def unused() -> f32:\n  return d28()\nsample:\n  out1 = 0.0\n",
        "sample:\n  out1 = d28(3.0)\n",
    ] {
        let typed = analyze_source(&fanout_source(28, use_site));
        assert!(typed.defs.len() <= 1);
        assert!(lower_program_to_optimized_mir(&typed).is_ok());
    }
}

#[test]
fn reached_default_graphs_have_linear_size() {
    let depth = 24;
    let typed = analyze_source(&fanout_source(depth, "sample:\n  out1 = d24()\n"));
    assert_eq!(typed.defs.len(), 2 * depth + 1);
    let mut nodes = 0;
    for def in &typed.defs {
        for stmt in &def.body {
            stmt.visit_exprs(|expr| nodes += expr.walk().count());
        }
    }
    assert!(
        nodes < 10 * (depth + 1),
        "default DAG expanded to {nodes} nodes"
    );
    assert!(lower_program_to_optimized_mir(&typed).is_ok());
}

#[test]
fn inferred_default_chains_use_bounded_worker_stack() {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let depth = 512;
            let source =
                fanout_source(depth, "sample:\n  out1 = d512()\n").replace("x: f32 =", "x =");
            let typed = analyze_source(&source);
            assert!(typed.defs.len() <= 2 * depth + 2);
            lower_program_to_optimized_mir(&typed).unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn dependent_defaults_and_explicit_arguments_select_the_same_concrete_instance() {
    let source = "def identity<T>(value: T) -> T:\n  return value\ndef read(value = identity(i64(4294967297))) -> i64:\n  return value\nsample:\n  out1 = f32(read() == read(identity(i64(4294967297))))\n";
    let typed = analyze_source(source);
    let instances = typed
        .defs
        .iter()
        .filter(|def| def.name.starts_with("read."))
        .collect::<Vec<_>>();
    assert_eq!(instances.len(), 1, "{instances:?}");
    assert_eq!(
        instances[0].param_kinds,
        [TypedFnParam::Scalar {
            ty: Some(PrimitiveType::I64)
        }]
    );
    lower_program_to_optimized_mir(&typed).unwrap();
}
