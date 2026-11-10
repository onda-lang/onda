use super::*;

fn compile(source: &str) -> TypedProgram {
    let parsed = parse_program(source).unwrap();
    let typed = analyze(parsed).unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
    crate::lower_program_to_optimized_mir(&typed).unwrap();
    typed
}

#[test]
fn input_smoothing_compiles_at_each_owner_boundary() {
    for attribute in ["ramp", "lag"] {
        for source in [
            format!("ins:\n  x = 0.5 {{0, 1, {attribute} = 0.02}}\nsample:\n  out1 = x + ins[0]\n"),
            format!("proc Voice<T>:\n  ins<T>:\n    x = 0.5 {{{attribute} = 0.02}}\n  sample:\n    out1 = f32(x + ins[0])\ninit:\n  voice = Voice<f64>()\nsample:\n  out1 = voice(1.0)\n"),
            format!("proc Voice:\n  ins:\n    x: f32[2] = [0.25, 0.75] {{{attribute} = 0.02}}\n  sample:\n    out1 = x[0] + ins[1]\nproc Parent:\n  init:\n    voice = Voice()\n  sample:\n    values: f32[2] = [1.0, 0.0]\n    out1 = voice(values)\ninit:\n  parent = Parent()\nsample:\n  out1 = parent()\n"),
            format!("ins:\n  x: f64[2] = [0.25, 0.75] {{{attribute} = 0.02}}\nsample:\n  out1 = f32(x[0] + ins[1] + x.read_unsafe(0))\n"),
            format!("proc Main:\n  ins:\n    x = 0.5 {{{attribute} = 0.02}}\n  sample:\n    out1 = x\n"),
            format!("ins:\n  x {{{attribute} = 0.02}}\ngraph:\n  x >> out1\n"),
        ] {
            compile(&source);
        }
    }
}

#[test]
fn zero_input_smoothing_has_identical_state_and_sample_code() {
    for owner in [false, true] {
        let source = |attribute: &str| {
            if owner {
                format!("proc Main:\n  ins:\n    x {attribute}\n  sample:\n    out1 = x\n")
            } else {
                format!("ins:\n  x {attribute}\nsample:\n  out1 = x\n")
            }
        };
        let plain = compile(&source(""));
        for attribute in ["{ramp = 0}", "{lag = 0}"] {
            let zero = compile(&source(attribute));
            assert_eq!(zero.state_vars, plain.state_vars);
            assert_eq!(zero.structs.len(), plain.structs.len());
            assert_eq!(
                zero.defs.iter().map(|def| &def.name).collect::<Vec<_>>(),
                plain.defs.iter().map(|def| &def.name).collect::<Vec<_>>()
            );
            assert_eq!(zero.sample.len(), plain.sample.len());
            assert!(zero.input_smoothing_aliases.is_empty());
        }
    }
}

#[test]
fn smoothing_durations_resolve_namespace_and_const_def_values() {
    compile("namespace Settings<Ms = 20>:\n  const def seconds() -> f64:\n    return f64(Ms) / 1000.0\n  proc Voice<T>:\n    ins<T>:\n      x {lag = seconds()}\n    sample:\n      out1 = f32(x)\ninit:\n  voice = Settings<10>::Voice<f64>()\nsample:\n  out1 = voice(1.0)\n");
}

#[test]
fn input_smoothing_rejects_invalid_types_and_durations() {
    assert!(analyze(parse_program("ins:\n  x {lag = 0.02}\n").unwrap()).is_err());
    for declaration in [
        "x: i32 {ramp = 0.02}",
        "x: bool {lag = 0}",
        "x {lag = -0.02}",
        "x {ramp = 1.0 / 0.0}",
        "x {lag = x}",
    ] {
        let source = format!("ins:\n  {declaration}\nsample:\n  out1 = f32(x)\n");
        assert!(
            analyze(parse_program(&source).unwrap()).is_err(),
            "{source}"
        );
    }
    for source in [
        "ins:\n  x {ramp = 0, lag = 0}\n",
        "ins:\n  x {lag = 0.1, lag = 0.2}\n",
        "outs:\n  x {lag = 0.1}\n",
        "params:\n  x = 0.0 {ramp = 0.1}\n",
        "ins:\n  x {0, 1, 0.02}\n",
    ] {
        assert!(parse_program(source).is_err(), "{source}");
    }
}
