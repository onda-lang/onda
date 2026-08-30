    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use onda_frontend::{parse_program, parse_program_file};

    fn mk_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onda_semantics_{prefix}_{nanos}"));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, text).expect("write test file");
    }

    fn assert_analyze_error_contains(src: &str, expected: &str) {
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("analysis should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(expected)),
            "expected diagnostic containing '{expected}', got {errors:?}"
        );
    }

    #[test]
    fn print_rejects_non_scalar_values_and_const_def_bodies() {
        assert_analyze_error_contains(
            "init:\n  values = [1, 2]\n  print(values)\nsample:\n  out1 = 0.0\n",
            "print scalar values explicitly",
        );
        assert_analyze_error_contains(
            "const def invalid() -> i32:\n  print(\"compile time\")\n  return 1\nsample:\n  out1 = 0.0\n",
            "print is not allowed in const def",
        );
    }

    #[test]
    fn rejects_pin_on_processor_instances_and_arrays() {
        let cases = [
            (
                r#"
proc Child:
  sample:
    out1 = 0.0
init:
  pin child = Child()
sample:
  out1 = child()
"#,
                "'pin' cannot be applied to processor instance 'child'",
            ),
            (
                r#"
proc Child:
  sample:
    out1 = 0.0
proc Parent:
  init:
    pin child = Child()
  sample:
    out1 = child()
init:
  parent = Parent()
sample:
  out1 = parent()
"#,
                "'pin' cannot be applied to processor instance 'child'",
            ),
            (
                r#"
proc Voice:
  sample:
    out1 = 0.0
init:
  pin voices: Voice[2] = Voice()
sample:
  out1 = voices[0]()
"#,
                "'pin' cannot be applied to processor array 'voices'",
            ),
            (
                r#"
proc Voice:
  sample:
    out1 = 0.0
proc Parent:
  init:
    pin voices: Voice[2] = Voice()
  sample:
    out1 = voices[0]()
init:
  parent = Parent()
sample:
  out1 = parent()
"#,
                "'pin' cannot be applied to processor array 'voices'",
            ),
        ];

        for (source, expected) in cases {
            assert_analyze_error_contains(source, expected);
        }
    }

    #[test]
    fn pin_requires_a_fresh_state_binding() {
        for source in [
            r#"
params:
  gain = 1.0
init:
  pin gain = 0.5
sample:
  out1 = gain
"#,
            r#"
proc Voice:
  params:
    private gain = 1.0
  init:
    pin gain = 0.5
  sample:
    out1 = gain
init:
  voice = Voice()
sample:
  out1 = voice()
"#,
        ] {
            assert_analyze_error_contains(source, "'pin' requires a fresh state binding");
        }
    }

    #[test]
    fn pin_supports_structs_and_fixed_struct_arrays() {
        let source = r#"
struct State:
  value: i32 = 1

init:
  pin one = State()
  pin many: State[2] = State()
sample:
  out1 = f32(one.value + many[0].value + many[1].value)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("pinned struct aggregates should analyze");
        let mir =
            lower_program_to_optimized_mir(&typed).expect("pinned struct aggregates should lower");
        for name in ["one.value", "many.value"] {
            let state = mir
                .state
                .iter()
                .find(|state| state.name == name)
                .unwrap_or_else(|| panic!("missing flattened aggregate state '{name}'"));
            assert!(state.pinned);
        }
    }

    #[test]
    fn convolution_pins_prepared_kernel_but_not_signal_history() {
        let source = r#"
import std/convolution
use std::convolution<256, 1024> as Conv

init:
  conv = Conv::ZeroLatencyConvolver()

sample:
  out1 = conv(0.0)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("convolver should analyze");
        let mir = lower_program_to_optimized_mir(&typed).expect("convolver should lower");
        let is_pinned = |name: &str| {
            mir.state
                .iter()
                .find(|state| state.name == name)
                .unwrap_or_else(|| panic!("missing convolver state '{name}'"))
                .pinned
        };

        for name in [
            "conv.td__impulse",
            "conv.td__active_taps",
            "conv.head__impulse_real",
            "conv.head__impulse_imag",
            "conv.head__active_partitions",
        ] {
            assert!(is_pinned(name));
        }
        for name in [
            "conv.td__delay",
            "conv.head__pending",
            "conv.head__overlap",
            "conv.head__input_real",
        ] {
            assert!(!is_pinned(name));
        }
    }

    #[test]
    fn reserves_unsafe_index_operation_names() {
        for name in [READ_UNSAFE_FN, WRITE_UNSAFE_FN] {
            assert_analyze_error_contains(
                &format!(
                    r#"
def {name}(value: f32) -> f32:
  return value

sample:
  out1 = 0.0
"#
                ),
                &format!("cannot redefine builtin function '{name}'"),
            );
        }
        assert_analyze_error_contains(
            r#"
struct Wrapper:
  def read_unsafe(self) -> f32:
    return 0.0

sample:
  out1 = 0.0
"#,
            "cannot redefine builtin method 'Wrapper.read_unsafe'",
        );
    }

    #[test]
    fn rejects_write_unsafe_in_value_contexts() {
        assert_analyze_error_contains(
            r#"
init:
  values: f32[2] = [0.0, 0.0]

sample:
  out1 = write_unsafe(values, 0, 1.0)
"#,
            "'write_unsafe' is a statement and cannot be used as a value",
        );
    }

    #[test]
    fn rejects_static_buffer_channels_beyond_signed_byte_extent() {
        assert_analyze_error_contains(
            r#"
buffers:
  huge: f32[536870912]
sample:
  out1 = 0
"#,
            "signed i32 buffer byte-extent limit",
        );
    }

    #[test]
    fn accepts_scoped_buffer_element_aliases_semantically() {
        let program = parse_program(
            r#"
buffers:
  bank: f32 {2}
sample:
  selected = bank[0]
  out1 = selected[0] + f32(selected.len())
"#,
        )
        .expect("parse should succeed");
        analyze(program).expect("buffer element aliases should analyze");
    }

    #[test]
    fn rejects_standalone_buffer_collection_spans_semantically() {
        assert_analyze_error_contains(
            r#"
buffers:
  bank: f32 {2}
sample:
  value = bank[:]
  out1 = 0
"#,
            "buffer collection slice",
        );
    }

    #[test]
    fn rejects_buffer_reference_alias_rebinding() {
        for replacement in ["bank[1]", "0.0"] {
            assert_analyze_error_contains(
                &format!(
                    r#"
buffers:
  bank: f32 {{2}}
sample:
  selected = bank[0]
  selected = {replacement}
  out1 = selected[0]
"#
                ),
                "is immutable and cannot be rebound",
            );
        }
    }

    #[test]
    fn validates_top_level_parameter_control_domains() {
        let program = parse_program(
            r#"
params {
  cutoff = 440.0 {20, 20000, log, "Hz"}
  mode: i32 = 4 {0, 10, step = 2}
  mix = 0.5 {0, 1, curve = -4}
}
outs { out1 }
sample { out1 = cutoff + mode + mix }
"#,
        )
        .expect("parse should succeed");
        let typed = analyze(program).expect("valid parameter domains should analyze");

        assert_eq!(typed.params[0].control.scale, ParamScale::Log);
        assert_eq!(typed.params[0].control.unit.as_deref(), Some("Hz"));
        assert_eq!(typed.params[0].control.step, None);
        assert_eq!(typed.params[1].control.step, Some(TypedConstValue::I32(2)));
        assert_eq!(typed.params[1].control.step_count, Some(5));
        assert_eq!(typed.params[2].control.curve, Some(-4.0));

        let mir =
            lower_program_to_optimized_mir(&typed).expect("parameter domains should lower to MIR");
        let params = &mir.as_program().interface.params;
        assert_eq!(params[0].control.scale, onda_mir::ParamScale::Log);
        assert_eq!(params[0].control.unit.as_deref(), Some("Hz"));
        assert_eq!(params[1].control.step, Some(onda_mir::ScalarValue::I32(2)));
        assert_eq!(params[1].control.step_count, Some(5));
        assert_eq!(params[2].control.curve, Some(-4.0));
    }

    #[test]
    fn parameter_curves_accept_the_full_constant_expression_pipeline() {
        let program = parse_program(
            r#"
const def curve_value() -> f64:
  return -2.0

const Curve = -3.0
const Curves: f64[1] = [-4.0]

params:
  scalar = 0.5 {0, 1, curve = Curve}
  array = 0.5 {0, 1, curve = Curves[0]}
  function = 0.5 {0, 1, curve = curve_value()}

outs:
  out1

sample:
  out1 = scalar + array + function
"#,
        )
        .expect("constant curve expressions should parse");
        let typed = analyze(program).expect("constant curve expressions should analyze");
        let curves = typed
            .params
            .iter()
            .map(|param| param.control.curve)
            .collect::<Vec<_>>();

        assert_eq!(curves, vec![Some(-3.0), Some(-4.0), Some(-2.0)]);
    }

    #[test]
    fn parameter_curves_reject_forward_constant_references() {
        assert_analyze_error_contains(
            r#"
params:
  mix = 0.5 {0, 1, curve = Curve}

const Curve = -4.0

outs:
  out1

sample:
  out1 = mix
"#,
            "constant 'Curve' is not visible before its declaration",
        );
    }

    #[test]
    fn integer_parameter_ranges_have_an_implicit_unit_step() {
        let program = parse_program(
            r#"
params { mode: i32 = 4 {0, 10} }
outs { out1 }
sample { out1 = mode }
"#,
        )
        .expect("parse should succeed");
        let typed = analyze(program).expect("integer domain should analyze");
        assert_eq!(typed.params[0].control.step, Some(TypedConstValue::I32(1)));
        assert_eq!(typed.params[0].control.step_count, Some(10));
    }

    #[test]
    fn float_parameter_grids_validate_at_the_declared_storage_precision() {
        let program = parse_program(
            r#"
params:
  value: f32 = 50000.0 {0, 100000, step = 0.1}
outs:
  out1
sample:
  out1 = value
"#,
        )
        .expect("parse should succeed");
        let typed = analyze(program).expect("representable f32 grid should analyze");
        assert_eq!(typed.params[0].control.step_count, Some(1_000_000));

        assert_analyze_error_contains(
            "params { p: f32 = 50000.5 {0, 100000, step = 1} }\n\
             outs { out1 }\nsample { out1 = p }\n",
            "default must lie on the step grid",
        );
        assert_analyze_error_contains(
            "params { p: f32 = 0 {0, 100000.5, step = 1} }\n\
             outs { out1 }\nsample { out1 = p }\n",
            "step must divide the range exactly",
        );
    }

    #[test]
    fn accepts_the_exact_host_i64_control_boundary() {
        let program = parse_program(
            r#"
params { p: i64 = 0 {0, 9007199254740991, step = 9007199254740991} }
outs<i64> { out1 }
sample { out1 = p }
"#,
        )
        .expect("parse should succeed");
        let typed = analyze(program).expect("exact host boundary should analyze");

        assert_eq!(
            typed.params[0].control.step,
            Some(TypedConstValue::I64(9_007_199_254_740_991))
        );
        assert_eq!(typed.params[0].control.step_count, Some(1));
    }

    #[test]
    fn rejects_invalid_top_level_parameter_control_domains() {
        for (domain, expected) in [
            ("{-20, 20000, log}", "0 < min < max"),
            (
                "{20, 20000, log, curve = -4}",
                "cannot combine logarithmic scale with curve",
            ),
            ("{0, 1, curve = 1.0 / 0.0}", "must be finite"),
            ("{20, 20000, log, step = 10}", "cannot combine"),
            ("{0, 10, step = 3}", "divide the range exactly"),
            ("{0, 10, step = 2}", "default must lie on the step grid"),
        ] {
            assert_analyze_error_contains(
                &format!("params {{ p = 3.0 {domain} }}\nouts {{ out1 }}\nsample {{ out1 = p }}\n"),
                expected,
            );
        }
        assert_analyze_error_contains(
            "params { p: i32 = 1 {0, 10, log} }\nouts { out1 }\nsample { out1 = p }\n",
            "logarithmic scale requires f32 or f64",
        );
        assert_analyze_error_contains(
            "params { p: i64 = 9007199254740992 {9007199254740992, 9007199254741002} }\n\
             outs<i64> { out1 }\nsample { out1 = p }\n",
            "must fit the exact host integer range",
        );
        assert_analyze_error_contains(
            "params { p: i64 = -9007199254740991 {-9007199254740991, 9007199254740991, step = 2} }\n\
             outs<i64> { out1 }\nsample { out1 = p }\n",
            "must fit the exact host integer range",
        );
        assert_analyze_error_contains(
            "params { p = 11.0 {0, 10, step = 2} }\n\
             outs { out1 }\nsample { out1 = p }\n",
            "default must lie on the step grid",
        );
        assert_analyze_error_contains(
            "params { p: i32 = 12 {0, 10, step = 2} }\n\
             outs<i32> { out1 }\nsample { out1 = p }\n",
            "default must lie on the step grid",
        );
    }

    #[test]
    fn expression_diagnostics_use_identifier_spans() {
        let src = "outs:\n  out1\nsample:\n  out1 = missing + 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unknown symbol should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("unknown symbol 'missing'"))
            .expect("missing unresolved symbol diagnostic");

        assert_eq!((diag.line, diag.column), (4, 10));
        assert_eq!(diag.end_line, 4);
        assert_eq!(diag.end_column, 17);
    }

    #[test]
    fn declaration_diagnostics_use_param_spans() {
        let src = "outs:\n  out1\nparams:\n  gain = 0.5\n  gain = 1.0\nsample:\n  out1 = gain\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("duplicate param should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("duplicate param 'gain'"))
            .expect("missing duplicate param diagnostic");

        assert_eq!((diag.line, diag.column), (5, 3));
    }

    #[test]
    fn bound_proc_param_hooks_lower_after_assignments_but_not_constructor_setup() {
        let src = r#"
proc Voice:
  params:
    gain = 1.0 {0.0, 1.0} => update
  init:
    cached = 0.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 2.0)
  v.gain = 0.25
sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("bound param hook program should analyze");
        let hook_name = "Voice.__onda_proc_local__update";

        let top_level_hook_count = typed
            .init
            .iter()
            .filter(|stmt| {
                matches!(
                    stmt,
                    Stmt::Expr {
                        expr: Expr::UserCall { name, .. },
                        ..
                    } if name == hook_name
                )
            })
            .count();
        assert_eq!(
            top_level_hook_count, 1,
            "constructor setup stores should not inject top-level hooks: {:?}",
            typed.init
        );

        let user_assign_idx = typed
            .init
            .iter()
            .rposition(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(name),
                        ..
                    } if name == "v.gain"
                )
            })
            .expect("missing user param assignment");
        assert!(matches!(
            typed.init.get(user_assign_idx + 1),
            Some(Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            }) if name == hook_name
        ));

        let init_def = typed
            .defs
            .iter()
            .find(|def| def.name == "Voice.__onda_proc_init")
            .expect("missing generated proc init def");
        assert!(matches!(
            init_def.body.last(),
            Some(Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            }) if name == hook_name
        ));
    }

    #[test]
    fn bound_proc_param_hooks_inject_child_param_cascade_calls() {
        let src = r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = 0.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    child = Child()
  def update():
    child.gain = gain
  outs:
    out1
  sample:
    out1 = child()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("child bind propagation should analyze");
        let parent_hook = typed
            .defs
            .iter()
            .find(|def| def.name == "Parent.__onda_proc_local__update")
            .expect("missing parent bind hook def");
        let assign_idx = parent_hook
            .body
            .iter()
            .position(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(name),
                        ..
                    } if name == "self.child__gain"
                )
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing child param assignment in parent hook: {:?}",
                    parent_hook.body
                )
            });
        assert!(matches!(
            parent_hook.body.get(assign_idx + 1),
            Some(Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            }) if name == "Parent.__onda_proc_local__nested__child__update"
        ));
    }

    #[test]
    fn bound_proc_param_hooks_inject_dynamic_child_array_cascade_calls() {
        let src = r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = 0.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    children: Child[2] = Child()
  def update():
    for i in 0..2:
      children[i].gain = gain + f32(i)
  outs:
    out1
  sample:
    out1 = children[0]() + children[1]()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dynamic child array bind propagation should analyze");
        let helper = typed
            .defs
            .iter()
            .find(|def| {
                def.name.starts_with("Parent.__arr_write_clamp_")
                    && def.body.iter().any(|stmt| {
                        stmt_contains_user_call_name(
                            stmt,
                            "Parent.__onda_proc_local__nested__children_0___update",
                        )
                    })
                    && def.body.iter().any(|stmt| {
                        stmt_contains_user_call_name(
                            stmt,
                            "Parent.__onda_proc_local__nested__children_1___update",
                        )
                    })
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing dynamic child array write helper with cascade hooks: {:?}",
                    typed
                        .defs
                        .iter()
                        .filter(|def| def.name.starts_with("Parent.__arr_write_clamp_"))
                        .map(|def| (&def.name, &def.body))
                        .collect::<Vec<_>>()
                )
            });

        assert!(
            helper.body.iter().any(|stmt| {
                stmt_contains_user_call_name(
                    stmt,
                    "Parent.__onda_proc_local__nested__children_0___update",
                )
            }),
            "missing slot 0 cascade hook in {:?}",
            helper.body
        );
        assert!(
            helper.body.iter().any(|stmt| {
                stmt_contains_user_call_name(
                    stmt,
                    "Parent.__onda_proc_local__nested__children_1___update",
                )
            }),
            "missing slot 1 cascade hook in {:?}",
            helper.body
        );
    }

    #[test]
    fn bound_proc_param_hooks_share_dynamic_proc_array_index_temps() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
  idx = 0
  voices[idx].gain = 0.5
sample:
  out1 = voices[0]()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dynamic proc-array hook should analyze");
        let assign_idx = typed
            .init
            .iter()
            .position(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Index {
                            base,
                            index: Expr::Var { name, .. },
                        },
                        ..
                    } if base == "voices.gain" && name.starts_with("__onda_bound_hook_index_tmp_")
                )
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing indexed proc-array param assignment using a hook index temp: {:?}",
                    typed.init
                )
            });
        assert!(
            assign_idx >= 2,
            "hook temp prelude should precede assignment: {:?}",
            typed.init
        );

        let index_tmp = match &typed.init[assign_idx] {
            Stmt::Assign {
                target:
                    AssignTarget::Index {
                        index: Expr::Var { name, .. },
                        ..
                    },
                ..
            } => name.clone(),
            other => panic!("unexpected assignment shape: {other:?}"),
        };
        assert!(
            matches!(
                &typed.init[assign_idx - 2],
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    ..
                } if name.starts_with("__onda_bound_hook_value_tmp_")
            ),
            "missing value temp before indexed assignment: {:?}",
            typed.init
        );
        assert!(
            matches!(
                &typed.init[assign_idx - 1],
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr: Expr::Var { name: idx_name, .. },
                    ..
                } if name == &index_tmp && idx_name == "idx"
            ),
            "missing index temp before indexed assignment: {:?}",
            typed.init
        );
        assert!(
            matches!(
                typed.init.get(assign_idx + 1),
                Some(Stmt::Expr {
                    expr:
                        Expr::UserCall {
                            name,
                            args,
                            ..
                        },
                    ..
                }) if name == "Voice.__onda_proc_local__update"
                    && matches!(
                        args.first(),
                        Some(CallArg {
                            expr:
                                Expr::Index {
                                    base,
                                    index,
                                    ..
                                },
                            ..
                        }) if base == "voices"
                            && matches!(
                                index.as_ref(),
                                Expr::Var { name, .. } if name == &index_tmp
                            )
                    )
            ),
            "hook call should reuse the assignment index temp: {:?}",
            typed.init
        );
    }

    #[test]
    fn bound_proc_param_hook_rules_are_validated() {
        let cases = [
            (
                "top-level bind",
                "params:\n  gain = 1.0 => update\nouts:\n  out1\nsample:\n  out1 = gain\n",
                "binds are only supported on processor params",
            ),
            (
                "array bind",
                "proc Voice:\n  params:\n    gains: f32[2] = [0.0, 1.0] => update\n  outs:\n    out1\n  def update():\n    cached = gains[0]\n  sample:\n    out1 = gains[0]\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "binds are only supported on primitive scalar params",
            ),
            (
                "missing target",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "param bind target 'update' is missing",
            ),
            (
                "target params",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update(x):\n    cached = x\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "bind target 'update' must take zero parameters",
            ),
            (
                "target return type",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update() -> f32:\n    cached = gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "bind target 'update' must not declare a return type",
            ),
            (
                "target return",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    return gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "bind target 'update' must not return a value",
            ),
            (
                "owner param write",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    gain = gain + 1.0\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot assign owner param 'gain'",
            ),
            (
                "input write",
                "proc Voice:\n  ins:\n    in1\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    in1 = gain\n  sample:\n    out1 = in1\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v(0.0)\n",
                "cannot write input 'in1'",
            ),
            (
                "output write",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    out1 = gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write output 'out1'",
            ),
            (
                "child input write",
                "proc Child:\n  ins:\n    in1\n  outs:\n    out1\n  sample:\n    out1 = in1\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child.in1 = gain\n  sample:\n    out1 = child(0.0)\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write child proc input 'child.in1'",
            ),
            (
                "child output write",
                "proc Child:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child.out1 = gain\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write child proc output 'child.out1'",
            ),
            (
                "child internal state write",
                "proc Child:\n  init:\n    cached = 0.0\n  outs:\n    out1\n  sample:\n    out1 = cached\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child.cached = gain\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "can only assign child proc params; 'child.cached' is child proc state",
            ),
            (
                "child dynamic params write",
                "proc Child:\n  params:\n    gain = 0.0\n  outs:\n    out1\n  sample:\n    out1 = gain\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child.params[0] = gain\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot assign child proc dynamic params 'child.params'",
            ),
            (
                "child proc array input write",
                "proc Child:\n  ins:\n    in1\n  outs:\n    out1\n  sample:\n    out1 = in1\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    children: Child[2] = Child()\n  outs:\n    out1\n  def update():\n    for i in 0..2:\n      children[i].in1 = gain\n  sample:\n    out1 = children[0](0.0)\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write child proc input 'children.in1'",
            ),
            (
                "child proc array output write",
                "proc Child:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    children: Child[2] = Child()\n  outs:\n    out1\n  def update():\n    for i in 0..2:\n      children[i].out1 = gain\n  sample:\n    out1 = children[0]()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot write child proc output 'children.out1'",
            ),
            (
                "child proc array internal state write",
                "proc Child:\n  init:\n    cached = 0.0\n  outs:\n    out1\n  sample:\n    out1 = cached\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    children: Child[2] = Child()\n  outs:\n    out1\n  def update():\n    for i in 0..2:\n      children[i].cached = gain\n  sample:\n    out1 = children[0]()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "can only assign child proc params; 'children.cached' is child proc state",
            ),
            (
                "child proc array dynamic params write",
                "proc Child:\n  params:\n    gain = 0.0\n  outs:\n    out1\n  sample:\n    out1 = gain\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    children: Child[2] = Child()\n  outs:\n    out1\n  def update():\n    children.params[0] = gain\n  sample:\n    out1 = children[0]()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot assign child proc dynamic params 'children.params'",
            ),
            (
                "child receiver call",
                "proc Child:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    child()\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot call child proc receiver 'child(...)'",
            ),
            (
                "dynamic params assignment",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    params[0] = 1.0\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "assignment through dynamic params[...] is not supported",
            ),
            (
                "dynamic params assignment in event",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  events:\n    set():\n      params[0] = 1.0\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "assignment through dynamic params[...] is not supported",
            ),
            (
                "dynamic params assignment in def",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    cached = gain\n  def unsafe_set():\n    params[0] = 1.0\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "assignment through dynamic params[...] is not supported",
            ),
            (
                "dynamic params assignment in sample",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    params[0] = 1.0\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "assignment through dynamic params[...] is not supported",
            ),
            (
                "top-level child dynamic params assignment to bound proc",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\n  v.params[0] = 1.0\nsample:\n  out1 = v()\n",
                "dynamic 'v.params[...]' is not supported",
            ),
            (
                "owner child dynamic params assignment to bound proc",
                "proc Child:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    cached = gain\n  sample:\n    out1 = cached\nproc Parent:\n  init:\n    child = Child()\n  outs:\n    out1\n  sample:\n    child.params[0] = 1.0\n    out1 = child()\nouts:\n  out1\ninit:\n  p = Parent()\nsample:\n  out1 = p()\n",
                "dynamic 'child.params[...]' is not supported",
            ),
            (
                "top-level helper cannot receive owner params view",
                "def poke(ps: f32[]):\n  ps[0] = 1.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n    trim = 0.0\n  outs:\n    out1\n  def update():\n    poke(params)\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot use dynamic param array 'params'",
            ),
            (
                "proc-local helper cannot receive owner params view",
                "proc Voice:\n  params:\n    gain = 0.0 => update\n    trim = 0.0\n  outs:\n    out1\n  def poke(ps: f32[]):\n    ps[0] = 1.0\n  def update():\n    poke(params)\n  sample:\n    out1 = gain\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot use dynamic param array 'params'",
            ),
            (
                "top-level helper cannot receive child params view",
                "def poke(ps: f32[]):\n  ps[0] = 1.0\nproc Child:\n  params:\n    a = 0.0\n    b = 0.0\n  outs:\n    out1\n  sample:\n    out1 = a\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    poke(child.params)\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "cannot use dynamic param array 'child.params'",
            ),
            (
                "top-level helper cannot mutate child state array",
                "def poke(xs: f32[]):\n  xs[0] = 1.0\nproc Child:\n  init:\n    table: f32[2] = [0.0, 0.0]\n  outs:\n    out1\n  sample:\n    out1 = table[0]\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    poke(child.table)\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "can only assign child proc params; 'child.table' is child proc state",
            ),
            (
                "transitive helper cannot mutate child state array",
                "def poke(xs: f32[]):\n  xs[0] = 1.0\ndef forward(xs: f32[]):\n  poke(xs)\nproc Child:\n  init:\n    table: f32[2] = [0.0, 0.0]\n  outs:\n    out1\n  sample:\n    out1 = table[0]\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    child = Child()\n  outs:\n    out1\n  def update():\n    forward(child.table)\n  sample:\n    out1 = child()\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
                "can only assign child proc params; 'child.table' is child proc state",
            ),
        ];

        for (_label, src, expected) in cases {
            assert_analyze_error_contains(src, expected);
        }

        let scalar_helper = "def coeff(x) -> f32:\n  return x * 2.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    cached = coeff(gain)\n  sample:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n";
        let program = parse_program(scalar_helper).expect("parse should succeed");
        analyze(program).expect("scalar helper in bind hook should analyze");

        let untyped_scalar_local = "def shadow(x):\n  x = x + 1.0\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    shadow(gain)\n    cached = gain\n  sample:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n";
        let program = parse_program(untyped_scalar_local).expect("parse should succeed");
        analyze(program).expect("untyped scalar helper local should analyze");

        let bare_return = "proc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  outs:\n    out1\n  def update():\n    if gain == 0.0:\n      return\n    cached = gain\n  sample:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n";
        let program = parse_program(bare_return).expect("parse should succeed");
        analyze(program).expect("bare return in bind hook should analyze");
    }

    #[test]
    fn oversampled_bind_hooks_accept_sr_dependent_consts() {
        let cases = [
            "const INV_SR = 1.0 / SR\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  def update():\n    cached = gain * INV_SR\n  outs:\n    out1\n  sample 2:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
            "proc Voice:\n  const INV_SR = 1.0 / SR\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  def update():\n    cached = gain * INV_SR\n  outs:\n    out1\n  sample 2:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
            "proc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  def update():\n    const INV_SR = 1.0 / SR\n    cached = gain * INV_SR\n  outs:\n    out1\n  sample 2:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
            "const def inv_sr() -> f32:\n  return 1.0 / SR\nproc Voice:\n  params:\n    gain = 0.0 => update\n  init:\n    cached = 0.0\n  def update():\n    cached = gain * inv_sr()\n  outs:\n    out1\n  sample 2:\n    out1 = cached\nouts:\n  out1\ninit:\n  v = Voice()\nsample:\n  out1 = v()\n",
        ];

        for src in cases {
            let program = parse_program(src).expect("parse should succeed");
            analyze(program).expect("SR-dependent constants in bind hooks should analyze");
        }
    }

    #[test]
    fn oversampled_proc_state_shapes_use_runtime_sr() {
        let src = r#"
proc Voice:
  const Len = SR
  init:
    table: f32[Len] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
  outs:
    out1
  sample 2:
    out1 = table[7]

outs:
  out1

init:
  v = Voice()

sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate: 4.0,
                block_size: 4,
            },
        )
        .expect("proc-local SR should size proc state arrays with effective runtime SR");
    }

    #[test]
    fn oversampled_proc_declared_arrays_and_proc_arrays_use_runtime_sr() {
        let src = r#"
proc Child:
  outs:
    out1
  sample:
    out1 = 0.0

proc Voice:
  const Len = SR
  params:
    gains: f32[Len] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]
  init:
    children: Child[Len] = Child()
  outs:
    out1
  sample 2:
    out1 = gains[7] + children[7]()

outs:
  out1

init:
  v = Voice()

sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate: 4.0,
                block_size: 4,
            },
        )
        .expect("proc param arrays and child proc arrays should use effective runtime SR");
    }

    #[test]
    fn external_sr_consts_keep_host_values_inside_oversampled_procs() {
        let src = r#"
namespace Host:
  const Len = SR

proc Voice:
  init:
    table: f32[Host::Len] = [0.0, 0.0, 0.0, 1.0]
  outs:
    out1
  sample 2:
    out1 = table[3]

outs:
  out1

init:
  v = Voice()

sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate: 4.0,
                block_size: 4,
            },
        )
        .expect("namespace SR constants should keep the host-rate value where they are defined");
    }

    #[test]
    fn host_sr_builtin_stays_host_in_oversampled_proc_contexts() {
        let src = r#"
proc Voice:
  const HostLen = HOST_SR
  const HostLenFromSampleRate = HOST_SAMPLE_RATE
  const HostLenFromSamplerate = HOST_SAMPLERATE
  const HostLenLowerSampleRate = host_sample_rate
  const HostLenLowerSamplerate = host_samplerate
  const RuntimeLen = SR
  params:
    gains: f32[HostLen] = [0.0, 0.0, 0.0, 1.0]
    more: f32[HostLenFromSampleRate] = [0.0, 0.0, 0.0, 1.0]
  init:
    table: f32[RuntimeLen] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]
    table_host_samplerate: f32[HostLenFromSamplerate] = [0.0, 0.0, 0.0, 1.0]
  outs:
    out1
  sample 2:
    const LocalHostLen = host_sample_rate
    const LocalHostLen2 = host_samplerate
    out1 = gains[LocalHostLen - 1] + more[HostLenLowerSampleRate - 1] + table_host_samplerate[LocalHostLen2 - 1] + table[RuntimeLen - 1] + f32(HostLenLowerSamplerate)

outs:
  out1

init:
  v = Voice()

sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate: 4.0,
                block_size: 4,
            },
        )
        .expect("HOST_SR should keep the host sample rate inside oversampled proc contexts");
    }

    #[test]
    fn proc_call_named_param_arg_errors_are_validated() {
        let cases = [
            (
                r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(0.25)
"#,
                "too many positional arguments",
            ),
            (
                r#"
proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(freq = 440.0)
"#,
                "unknown named argument 'freq'",
            ),
            (
                r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(gain = 0.25, gain = 0.5)
"#,
                "duplicate named argument 'gain'",
            ),
        ];

        for (src, expected) in cases {
            assert_analyze_error_contains(src, expected);
        }
    }

    #[test]
    fn proc_call_named_param_args_preserve_expression_call_order() {
        let src = r#"
proc Voice:
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  v.gain = 1.0
  out1 = v() + v(gain = 2.0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("analysis should succeed");
        assert_eq!(typed.sample.len(), 7, "{:#?}", typed.sample);
        assert!(matches!(
            &typed.sample[2],
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::UserCall { name: call_name, .. },
                ..
            } if name == "__onda_proc_call_result_tmp_0"
                && call_name == "Voice.__onda_proc_call_out0"
        ));
        assert!(matches!(
            &typed.sample[3],
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::Number { value, .. },
                ..
            } if name == "v.gain" && (*value - 2.0).abs() <= f64::EPSILON
        ));
        assert!(matches!(
            &typed.sample[4],
            Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            } if name == "Voice.__onda_proc_local__update"
        ));
        assert!(matches!(
            &typed.sample[5],
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr: Expr::UserCall { name: call_name, .. },
                ..
            } if name == "__onda_proc_call_result_tmp_1"
                && call_name == "Voice.__onda_proc_call_out0"
        ));
    }

    #[test]
    fn proc_call_named_param_args_reject_control_flow_unsafe_contexts() {
        assert_analyze_error_contains(
            r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1
params:
  ready: bool = 0
init:
  v = Voice()
sample:
  if ready && v(gain = 0.25) > 0.0:
    out1 = 1.0
  else:
    out1 = 0.0
"#,
            "named param arguments are not supported in logical expressions",
        );

        assert_analyze_error_contains(
            r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1
init:
  v = Voice()
sample:
  while v(gain = 0.25) > 0.0:
    break
  out1 = 0.0
"#,
            "named param arguments are not supported in while conditions",
        );
    }

    #[test]
    fn private_proc_params_accept_constructor_and_builtin_init() {
        let src = r#"
proc Voice:
  params:
    private cutoff = 1000.0
    private coeffs: f32[2] = [0.5, 0.25]
    gain = 1.0
  init:
    cached = cutoff + coeffs[0] + coeffs[1] + gain
  event refresh(cutoff_v: f32, coeffs_v: f32[2]):
    cutoff = cutoff_v
    coeffs[0] = coeffs_v[0]
    coeffs[1] = coeffs_v[1]
    cached = cutoff + coeffs[0] + coeffs[1] + gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
events:
  reset():
    voice.init(cutoff = 1500.0, coeffs = [0.2, 0.3], gain = 0.75)
init:
  voice = Voice(cutoff = 1200.0, coeffs = [0.1, 0.2], gain = 0.5)
sample:
  out1 = voice(gain = 0.25)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("private constructor/init params should analyze");
    }

    #[test]
    fn nested_proc_events_may_update_their_own_private_params() {
        let src = r#"
proc Child:
  params:
    private value = 0.0
  event set(value_v: f32):
    value = value_v
  outs:
    out1
  sample:
    out1 = value

proc Parent:
  init:
    child = Child()
  outs:
    out1
  sample:
    child.set(0.75)
    out1 = child()

init:
  parent = Parent()
sample:
  out1 = parent()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program)
            .expect("a nested child event should retain authority over its private params");
    }

    #[test]
    fn private_proc_params_reject_external_access() {
        let cases = [
            (
                "field assignment",
                "proc Voice:\n  params:\n    private cutoff = 1000.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  voice.cutoff = 1200.0\n  out1 = voice()\n",
                "param 'cutoff' is private and cannot be assigned",
            ),
            (
                "field read",
                "proc Voice:\n  params:\n    private cutoff = 1000.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice.cutoff\n",
                "param 'cutoff' is private and cannot be read",
            ),
            (
                "field read from user def",
                "proc Voice:\n  params:\n    private cutoff = 1000.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff\ndef leak(voice: Voice):\n  return voice.cutoff\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = leak(voice)\n",
                "param 'cutoff' is private and cannot be read",
            ),
            (
                "field read from __proc-prefixed user method",
                "proc Voice:\n  params:\n    private cutoff = 1000.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff\nstruct Inspector:\n  def __proc_read(self, voice: Voice):\n    return voice.cutoff\nouts:\n  out1\ninit:\n  voice = Voice()\n  inspector = Inspector()\nsample:\n  out1 = inspector.__proc_read(voice)\n",
                "param 'cutoff' is private and cannot be read",
            ),
            (
                "array assignment",
                "proc Voice:\n  params:\n    private coeffs: f32[2] = [0.5, 0.25]\n  outs:\n    out1\n  sample:\n    out1 = coeffs[0]\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  voice.coeffs[0] = 0.1\n  out1 = voice()\n",
                "param 'coeffs' is private and cannot be assigned",
            ),
            (
                "array read",
                "proc Voice:\n  params:\n    private coeffs: f32[2] = [0.5, 0.25]\n  outs:\n    out1\n  sample:\n    out1 = coeffs[0]\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice.coeffs[0]\n",
                "param 'coeffs' is private and cannot be read",
            ),
            (
                "named call arg",
                "proc Voice:\n  params:\n    private cutoff = 1000.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice(cutoff = 1200.0)\n",
                "named argument 'cutoff' is private",
            ),
            (
                "dynamic params read",
                "proc Voice:\n  params:\n    private cutoff = 1000.0\n    gain = 1.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff + gain\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice.params[0]\n",
                "has private params, so dynamic param access",
            ),
            (
                "dynamic params assignment",
                "proc Voice:\n  params:\n    private cutoff = 1000.0\n    gain = 1.0\n  outs:\n    out1\n  sample:\n    out1 = cutoff + gain\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  voice.params[0] = 0.5\n  out1 = voice()\n",
                "has private params, so assignment through dynamic",
            ),
        ];

        for (label, src, expected) in cases {
            let program = parse_program(src).expect(label);
            let errors = analyze(program).expect_err(label);
            assert!(
                errors.iter().any(|diag| diag.message.contains(expected)),
                "case '{label}' expected diagnostic containing '{expected}', got {errors:?}"
            );
        }
    }

    #[test]
    fn proc_call_named_param_args_in_nested_wrappers_inject_hooks() {
        let src = r#"
proc Leaf:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Mid:
  params:
    gain = 0.0
  init:
    leaf = Leaf()
  outs:
    out1
  sample:
    out1 = leaf(gain = gain)

proc Parent:
  init:
    mid = Mid()
  outs:
    out1
  sample:
    mid.gain = 0.25
    out1 = mid()

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("analysis should succeed");
        let step = typed
            .defs
            .iter()
            .find(|def| def.name == "Parent.__onda_proc_nested_mid_step")
            .expect("missing nested mid step");
        let assign_idx = step
            .body
            .iter()
            .position(|stmt| {
                matches!(
                    stmt,
                    Stmt::Assign {
                        target: AssignTarget::Var(name),
                        ..
                    } if name == "self.mid__leaf__gain"
                )
            })
            .expect("missing lowered nested child param assignment");
        assert!(matches!(
            step.body.get(assign_idx + 1),
            Some(Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            }) if name == "Parent.__onda_proc_local__nested__mid__leaf__update"
        ));
    }

    #[test]
    fn oversampled_bound_proc_param_hooks_can_use_runtime_sample_rate_directly() {
        let src = r#"
const HostBS = BS

proc Voice:
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain / SR + f32(BS) + f32(HostBS)
  outs:
    out1
  sample 2:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("direct runtime SR in hook should analyze");
        assert_eq!(
            typed
                .def_sample_oversample_factors
                .get("Voice.__onda_proc_local__update")
                .copied(),
            Some(2)
        );
    }

    #[test]
    fn top_level_const_array_reads_analyze() {
        let src = r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

outs:
  out1

sample:
  out1 = Table[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const array read should analyze");
        assert_eq!(typed.const_arrays.len(), 1);
        let table = &typed.const_arrays[0];
        assert_eq!(table.name, "Table");
        assert_eq!(table.elem_ty, PrimitiveType::F32);
        assert_eq!(table.len, 3);
        assert_eq!(table.values[1], TypedConstValue::F32(0.5));
    }

    #[test]
    fn proc_bodies_can_read_const_arrays_with_runtime_index() {
        let src = r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

proc Voice:
  params:
    idx: i32 = 1
  outs:
    out1
  sample:
    out1 = Table[idx]

outs:
  out1

init:
  voice = Voice()

sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc dynamic const array read should analyze");
    }

    #[test]
    fn count_shorthand_expands_in_semantic_preprocessing_from_const_defs() {
        let src = r#"
const def count() -> i32:
  return 3

const N = count()

ins N
outs N
params N
buffers N

sample:
  outs[0] = ins[0] + param1
  outs[1] = ins[1] + param2
  outs[2] = ins[2] + param3
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("semantic count expansion should analyze");

        assert_eq!(typed.ins, vec!["in1", "in2", "in3"]);
        assert_eq!(typed.outs, vec!["out1", "out2", "out3"]);
        assert_eq!(typed.params.len(), 3);
        assert_eq!(typed.buffers.len(), 3);
    }

    #[test]
    fn count_shorthand_accepts_direct_const_def_calls() {
        let src = r#"
const def count() -> i32:
  return 2

ins (count())
outs (count())

sample:
  outs[0] = ins[0]
  outs[1] = ins[1]
"#;
        let program = parse_program(src).expect("parse should preserve direct const def count");
        let typed = analyze(program).expect("direct const def count should analyze");

        assert_eq!(typed.ins, vec!["in1", "in2"]);
        assert_eq!(typed.outs, vec!["out1", "out2"]);
    }

    #[test]
    fn proc_local_scalar_consts_expand_counts_in_semantics() {
        let src = r#"
const def count() -> i32:
  return 2

proc Voice:
  const N = count()
  ins N
  outs N
  sample:
    out1 = in1
    out2 = in2

outs 2
init:
  v = Voice()
sample:
  v(0.25, 0.5)
  outs[0] = v.out1
  outs[1] = v.out2
"#;
        let program = parse_program(src).expect("parse should preserve proc local consts");
        let typed = analyze(program).expect("proc local const counts should analyze");

        assert_eq!(typed.outs, vec!["out1", "out2"]);
    }

    #[test]
    fn statement_local_scalar_consts_call_const_defs_in_semantics() {
        let src = r#"
const def gain() -> f32:
  return 0.5

outs:
  out1

sample:
  const G = gain()
  out1 = G
"#;
        let program = parse_program(src).expect("parse should preserve local consts");
        let typed = analyze(program).expect("statement local const should analyze");

        assert_eq!(typed.sample.len(), 1);
        assert!(!matches!(typed.sample[0], Stmt::Const { .. }));
    }

    #[test]
    fn assignment_to_statement_local_const_is_rejected_in_semantics() {
        let src = r#"
outs:
  out1

sample:
  const X = 1
  X = 2
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should preserve local consts");
        let errors = analyze(program).expect_err("assignment to local const should fail");
        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("cannot assign to constant 'X'")));
    }

    #[test]
    fn proc_local_const_arrays_are_rejected_in_semantics() {
        let src = r#"
proc Voice:
  const Table = [1, 2]
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v.out1
"#;
        let program = parse_program(src).expect("parse should preserve proc local const array");
        let errors = analyze(program).expect_err("proc local const array should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const arrays are only supported at top-level and namespace scope")));
    }

    #[test]
    fn count_shorthand_rejects_forward_const_def_calls() {
        let src = r#"
ins N
outs 1

const def count() -> i32:
  return 2

const N = count()

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward count const def should fail");
        assert!(errors.iter().any(|diag| {
            diag.message
                .contains("ins count expression uses non-constant symbol 'N'")
        }));
    }

    #[test]
    fn count_prefix_mismatch_is_reported_in_semantics() {
        let src = r#"
const def count() -> i32:
  return 2

const N = count()

ins N:
  in1

outs 1
sample:
  out1 = in1
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("count/list mismatch should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("ins block count (2) does not match explicit declaration count (1)")));
    }

    #[test]
    fn count_shorthand_zero_diagnostic_uses_count_span() {
        let src = "outs 0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("zero count should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("outs count expression must be greater than zero")
            })
            .expect("missing count diagnostic");

        assert_eq!((diag.line, diag.column), (1, 6));
        assert_eq!(diag.end_line, 1);
        assert_eq!(diag.end_column, 7);
    }

    #[test]
    fn scalar_const_validation_diagnostics_use_expr_span() {
        let src = "const X = foo\nouts:\n  out1\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should preserve invalid const");
        let errors = analyze(program).expect_err("invalid const should fail in semantics");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("const scalar 'X' uses non-constant symbol 'foo'")
            })
            .expect("missing const validation diagnostic");

        assert_eq!((diag.line, diag.column), (1, 11));
        assert_eq!(diag.end_line, 1);
        assert_eq!(diag.end_column, 14);
    }

    #[test]
    fn direct_const_def_calls_fold_in_array_sizes_defaults_and_oversampling() {
        let src = r#"
const def count() -> i32:
  return 2

const def values() -> f32[2]:
  return [0.25, 0.75]

params:
  taps: f32[count()] = values()

outs:
  out1

init:
  state: f32[count()]

sample count():
  out1 = taps[0] + taps[1] + state[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("direct const def preprocessing should analyze");

        assert_eq!(typed.sample_oversample_factor, 2);
        assert_eq!(typed.param_arrays.get("taps").map(|info| info.len), Some(2));
        let tap_defaults = typed
            .params
            .iter()
            .map(|param| (param.name.as_str(), param.default))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            tap_defaults.get("taps[0]"),
            Some(&TypedConstValue::F32(0.25))
        );
        assert_eq!(
            tap_defaults.get("taps[1]"),
            Some(&TypedConstValue::F32(0.75))
        );
        let state = typed
            .array_vars
            .iter()
            .find(|array| array.name == "state")
            .expect("typed state array");
        assert_eq!(state.len, 2);
    }

    #[test]
    fn direct_const_def_calls_fold_in_asserts_and_graph_delays() {
        let src = r#"
namespace Check:
  const def ok() -> bool:
    return true

  assert(ok())

const def delay() -> i32:
  return 2

ins 1
outs 1

graph:
  in1 >>[delay()] out1
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("direct const def graph delay should analyze");
    }

    #[test]
    fn const_def_signature_sizes_can_call_earlier_const_defs() {
        let src = r#"
const def count() -> i32:
  return 3

const def values(xs: f32[count()]) -> f32[count()]:
  return xs

const Table: f32[count()] = values([0.25, 0.5, 0.75])

outs:
  out1

sample:
  out1 = Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def signature sizes should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const table");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::F32(0.75));
    }

    #[test]
    fn namespace_template_args_accept_semantic_scalar_consts() {
        let src = r#"
const def count() -> i32:
  return 4

const Size = count()

namespace LUT<N = 2>:
  const Value = N
  const Table: i32[N] = [0, 1, 2, 3]

outs:
  out1

sample:
  out1 = f32(LUT<Size>::Value)
"#;
        let program = parse_program(src).expect("parse should preserve semantic template arg");
        let typed = analyze(program).expect("semantic namespace template arg should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 4);
        assert_eq!(table.values[3], TypedConstValue::I32(3));
    }

    #[test]
    fn namespace_template_args_can_shadow_semantic_scalar_const_names() {
        let src = r#"
const def count() -> i32:
  return 5

const N = count()

namespace LUT<N = 2>:
  const Value = N

outs:
  out1

sample:
  out1 = f32(LUT<N>::Value)
"#;
        let program = parse_program(src).expect("parse should preserve shadowing template arg");
        analyze(program).expect("shadowing semantic namespace template arg should analyze");
    }

    #[test]
    fn namespace_alias_args_accept_semantic_scalar_consts() {
        let src = r#"
const def count() -> i32:
  return 3

const Size = count()

namespace LUT<N = 2>:
  const Table: i32[N] = [10, 20, 30]

namespace Picked = LUT<Size>

outs:
  out1

sample:
  out1 = f32(Picked::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve semantic alias arg");
        let typed = analyze(program).expect("semantic namespace alias arg should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::I32(30));
    }

    #[test]
    fn use_namespace_brings_members_into_unqualified_scope() {
        let src = r#"
import std/math
use std::math

outs:
  out1

sample:
  out1 = clamp(2.0, 0.0, 1.0) + lerp(0.0, 2.0, 0.25)
"#;
        let program = parse_program(src).expect("parse should preserve use namespace");
        analyze(program).expect("namespace use should analyze");
    }

    #[test]
    fn use_single_namespace_brings_child_namespaces_into_unqualified_scope() {
        let src = r#"
namespace sc:
  namespace SinOsc:
    def ar():
      return 0.25

use sc

outs:
  out1

sample:
  out1 = SinOsc::ar()
"#;
        let program = parse_program(src).expect("parse should preserve single namespace use");
        analyze(program).expect("single namespace use should analyze child namespace prefix");
    }

    #[test]
    fn use_single_namespace_resolves_child_namespace_templates() {
        let src = r#"
namespace dsp:
  namespace Table<N = 2>:
    const Size = N

use dsp

outs:
  out1

sample:
  out1 = f32(Table<4>::Size)
"#;
        let program =
            parse_program(src).expect("parse should preserve single namespace template use");
        analyze(program).expect("single namespace use should analyze child namespace template");
    }

    #[test]
    fn use_single_namespace_resolves_child_namespace_aliases() {
        let src = r#"
namespace ugens:
  namespace LocalOsc:
    def ar():
      return 0.5

  namespace Osc = LocalOsc

use ugens

outs:
  out1

sample:
  out1 = Osc::ar()
"#;
        let program =
            parse_program(src).expect("parse should preserve child namespace alias through use");
        analyze(program).expect("single namespace use should analyze child namespace alias");
    }

    #[test]
    fn use_single_namespace_child_collision_requires_qualified_namespace_root() {
        let src = r#"
namespace imported:
  namespace Osc:
    def ar():
      return 0.5

namespace Osc:
  def ar():
    return 0.25

use imported

outs:
  out1

sample:
  out1 = Osc::ar()
"#;
        assert_analyze_error_contains(src, "ambiguous unqualified namespace 'Osc'");
    }

    #[test]
    fn use_single_namespace_child_collision_allows_qualified_namespace_root() {
        let src = r#"
namespace imported:
  namespace Osc:
    def ar():
      return 0.5

namespace Osc:
  def ar():
    return 0.25

use imported

outs:
  out1

sample:
  out1 = imported::Osc::ar()
"#;
        let program = parse_program(src).expect("parse should preserve qualified namespace root");
        analyze(program).expect("qualified namespace root should avoid use ambiguity");
    }

    #[test]
    fn use_symbol_brings_one_member_into_unqualified_scope() {
        let src = r#"
import std/random
use std::random::Rng

outs:
  out1

init:
  rng = Rng<f32>(state = 123)

sample:
  out1 = rng.next()
"#;
        let program = parse_program(src).expect("parse should preserve use symbol");
        analyze(program).expect("symbol use should analyze");
    }

    #[test]
    fn use_const_assignment_targets_resolve_to_imported_const() {
        let src = r#"
namespace NS:
  const X = 1

use NS::X

outs:
  out1

init:
  X = 2

sample:
  out1 = f32(X)
"#;
        assert_analyze_error_contains(src, "cannot assign to constant 'NS::X'");
    }

    #[test]
    fn use_const_array_assignment_targets_resolve_to_imported_const_array() {
        let src = r#"
namespace NS:
  const Table: f32[2] = [0.25, 0.5]

use NS::Table

outs:
  out1

sample:
  Table[0] = 0.0
  out1 = Table[1]
"#;
        assert_analyze_error_contains(src, "cannot assign to immutable array alias 'NS::Table'");
    }

    #[test]
    fn use_namespace_aliases_can_coexist_for_template_instantiations() {
        let src = r#"
import std/fft
use std::fft<8> as fft8
use std::fft<16> as fft16

outs:
  out1

init:
  a = fft8::FFT<f32>()
  b = fft16::FFT<f32>()

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should preserve namespace use aliases");
        analyze(program).expect("namespace use aliases should analyze");
    }

    #[test]
    fn use_symbol_alias_can_name_template_member() {
        let src = r#"
import std/fft
use std::fft<8>::FFT as FFT8

outs:
  out1

init:
  fft = FFT8<f32>()

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should preserve symbol use alias");
        analyze(program).expect("symbol use alias should analyze");
    }

    #[test]
    fn explicit_use_collision_requires_qualified_name() {
        let src = r#"
import std/math
use std::math

outs:
  out1

def clamp(x, lo, hi):
  return x

sample:
  out1 = clamp(2.0, 0.0, 1.0)
"#;
        assert_analyze_error_contains(src, "ambiguous unqualified symbol 'clamp'");
    }

    #[test]
    fn explicit_use_collision_allows_qualified_name() {
        let src = r#"
import std/math
use std::math

outs:
  out1

def clamp(x, lo, hi):
  return x

sample:
  out1 = std::math::clamp(2.0, 0.0, 1.0)
"#;
        let program = parse_program(src).expect("parse should preserve qualified use collision");
        analyze(program).expect("qualified name should avoid explicit use ambiguity");
    }

    #[test]
    fn use_namespace_does_not_capture_function_parameter_reads() {
        let src = r#"
import std/math
use std::math

outs:
  out1

def id(clamp):
  return clamp

sample:
  out1 = id(0.5)
"#;
        let program = parse_program(src).expect("parse should preserve local shadowing");
        analyze(program).expect("function parameter should shadow explicit use member");
    }

    #[test]
    fn use_namespace_does_not_capture_local_variable_reads() {
        let src = r#"
import std/math
use std::math

outs:
  out1

sample:
  clamp = 0.5
  out1 = clamp
"#;
        let program = parse_program(src).expect("parse should preserve local assignment");
        analyze(program)
            .expect("local variable should shadow explicit use member after assignment");
    }

    #[test]
    fn imported_private_use_is_not_reexported() {
        let dir = mk_temp_dir("private_use_not_reexported");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers

def shaped(x):
  return shape(x)
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = shape(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        let errors = analyze(program).expect_err("private imported use should not reexport");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("unknown symbol 'shape'")
                    || diag.message.contains("unknown function 'shape'")
            }),
            "expected unknown private use symbol, got {errors:?}"
        );
    }

    #[test]
    fn imported_private_use_still_works_inside_imported_file() {
        let dir = mk_temp_dir("private_use_internal");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers

def shaped(x):
  return shape(x)
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = shaped(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        analyze(program).expect("private imported use should work inside imported file");
    }

    #[test]
    fn imported_pub_use_is_reexported() {
        let dir = mk_temp_dir("pub_use_reexported");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

pub use helpers
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = shape(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        analyze(program).expect("pub use from imported file should reexport");
    }

    #[test]
    fn imported_pub_use_alias_is_reexported() {
        let dir = mk_temp_dir("pub_use_alias_reexported");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

pub use helpers as h
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = h::shape(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        analyze(program).expect("pub use alias from imported file should reexport");
    }

    #[test]
    fn imported_private_use_alias_is_file_scoped() {
        let dir = mk_temp_dir("private_use_alias_scoped");
        write_file(
            &dir.join("lib.onda"),
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers as h

def shaped(x):
  return h::shape(x)
"#,
        );
        write_file(
            &dir.join("main.onda"),
            r#"
import lib

outs:
  out1

sample:
  out1 = h::shape(in1)
"#,
        );

        let program = parse_program_file(&dir.join("main.onda")).expect("parse should load import");
        let errors = analyze(program).expect_err("private imported alias should not reexport");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("unknown namespace 'h'")
                    || diag.message.contains("unknown symbol 'h'")
            }),
            "expected unknown private alias, got {errors:?}"
        );
    }

    #[test]
    fn namespace_template_args_accept_direct_const_def_calls() {
        let src = r#"
const def count() -> i32:
  return 3

namespace LUT<N = 2>:
  const Table: i32[N] = [10, 20, 30]

namespace Picked = LUT<count()>

outs:
  out1

sample:
  out1 = f32(Picked::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve direct const def arg");
        let typed = analyze(program).expect("direct const def namespace arg should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::I32(30));
    }

    #[test]
    fn namespace_template_defaults_accept_direct_const_def_calls() {
        let src = r#"
namespace Outer:
  const def count() -> i32:
    return 3

  namespace Inner<N = count()>:
    const Table: i32[N] = [0, 2, 4]

outs:
  out1

sample:
  out1 = f32(Outer::Inner::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve direct const def default");
        let typed = analyze(program).expect("direct const def namespace default should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::I32(4));
    }

    #[test]
    fn nested_namespace_template_defaults_accept_semantic_scalar_consts() {
        let src = r#"
namespace Outer:
  const def count() -> i32:
    return 3

  const Size = count()

  namespace Inner<N = Size>:
    const Table: i32[N] = [0, 2, 4]

outs:
  out1

sample:
  out1 = f32(Outer::Inner::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve semantic nested default");
        let typed = analyze(program).expect("semantic namespace default should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(table.values[2], TypedConstValue::I32(4));
    }

    #[test]
    fn namespace_template_defaults_use_definition_scope_when_instantiated_elsewhere() {
        let src = r#"
const Size = 3

namespace LUT<N = Size>:
  const Value = N

namespace Consumer:
  const Size = 5
  const Picked: i32[1] = [LUT::Value]

outs:
  out1

sample:
  out1 = f32(Consumer::Picked[0])
"#;
        let program = parse_program(src).expect("parse should preserve namespace default");
        let typed = analyze(program).expect("definition-scoped namespace default should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Consumer::Picked")
            .expect("typed const array");
        assert_eq!(table.values, vec![TypedConstValue::I32(3)]);
    }

    #[test]
    fn nested_namespace_const_arrays_and_const_defs_are_usable_from_code() {
        let src = r#"
namespace Outer<A = 2>:
  namespace Inner<B = 3>:
    const def ramp() -> f32[B]:
      values: f32[B]
      for i in 0..B:
        values[i] = f32(A + i)
      return values

    const Table: f32[B] = ramp()

    namespace Leaf<C = A + B>:
      const Value = Table[1] + f32(C)

outs:
  out1

sample:
  out1 = Outer<2>::Inner<3>::Leaf::Value + Outer<2>::Inner<3>::Table[2]
"#;
        let program = parse_program(src).expect("parse should preserve nested namespace use");
        let typed = analyze(program).expect("nested namespace consts should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("nested namespace const table");
        assert_eq!(table.len, 3);
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(2.0),
                TypedConstValue::F32(3.0),
                TypedConstValue::F32(4.0)
            ]
        );
    }

    #[test]
    fn namespace_template_instantiations_dedup_by_evaluated_const_values() {
        let src = r#"
const def count() -> i32:
  return 3

namespace LUT<N = 2>:
  const Table: i32[N] = [10, 20, 30]

outs:
  out1

sample:
  out1 = f32(LUT<3>::Table[0] + LUT<count()>::Table[2])
"#;
        let program = parse_program(src).expect("parse should preserve namespace instantiations");
        let typed = analyze(program).expect("deduped namespace instantiations should analyze");
        let tables = typed
            .const_arrays
            .iter()
            .filter(|array| array.name.contains("LUT__nsinst") && array.name.ends_with("::Table"))
            .collect::<Vec<_>>();

        assert_eq!(tables.len(), 1, "const arrays: {:?}", typed.const_arrays);
        assert_eq!(tables[0].values[2], TypedConstValue::I32(30));
    }

    #[test]
    fn imported_namespaces_can_provide_const_arrays_and_const_defs() {
        let dir = mk_temp_dir("imported_namespaced_consts");
        let main = dir.join("main.onda");
        let lib = dir.join("lib.onda");

        write_file(
            &lib,
            r#"
namespace Imported:
  const def offset() -> i32:
    return 2

  namespace Tables<N = offset()>:
    const def ramp() -> f32[N]:
      values: f32[N]
      for i in 0..N:
        values[i] = f32(i + offset())
      return values

    const Table: f32[N] = ramp()
"#,
        );
        write_file(
            &main,
            r#"
import lib

outs:
  out1

sample:
  out1 = Imported::Tables::Table[1]
"#,
        );

        let program =
            parse_program_file(&main).expect("program with namespace import should parse");
        let typed = analyze(program).expect("imported namespace consts should analyze");
        fs::remove_dir_all(&dir).ok();

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("imported namespace const table");
        assert_eq!(table.len, 2);
        assert_eq!(
            table.values,
            vec![TypedConstValue::F32(2.0), TypedConstValue::F32(3.0)]
        );
    }

    #[test]
    fn namespace_template_args_with_runtime_symbols_fail_in_semantics() {
        let src = r#"
namespace LUT<N = 2>:
  const Value = N

outs:
  out1

sample:
  idx = 3
  out1 = f32(LUT<idx>::Value)
"#;
        let program = parse_program(src).expect("parse should preserve template arg for semantics");
        let errors = analyze(program).expect_err("runtime namespace template arg should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("namespace template")
                    && diag.message.contains("LUT")
                    && diag.message.contains("uses non-constant symbol 'idx'")
            }),
            "diagnostics: {errors:?}"
        );
    }

    #[test]
    fn namespace_template_bodies_use_definition_scope_for_consts() {
        let src = r#"
namespace LUT<N = 1>:
  const Table: f32[1] = [Gain * f32(N)]

const Gain = 0.5

outs:
  out1

sample:
  out1 = LUT<2>::Table[0]
"#;
        let program = parse_program(src).expect("parse should preserve template body");
        let errors = analyze(program).expect_err("template body should not see later scalar const");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "const array 'LUT__nsinst0::Table' element 0 uses non-constant symbol 'Gain'"
            )),
            "expected definition-scope diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn duplicate_namespace_templates_and_aliases_fail_in_semantics() {
        let src = r#"
namespace Config<N = 1>:
  const Value = N

namespace Config<N = 1>:
  const Value = N

namespace Picked = Config<1>
namespace Picked = Config<1>

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("duplicates should parse for semantic diagnostics");
        let errors = analyze(program).expect_err("duplicate namespaces should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("duplicate namespace template 'Config'")));
        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("duplicate namespace alias 'Picked'")));
    }

    #[test]
    fn namespace_template_argument_errors_are_semantic_diagnostics() {
        let cases = [
            (
                "too many positional arguments",
                r#"
namespace Data<S = 1, C = 1>:
  const Value = S + C

outs:
  out1

sample:
  out1 = f32(Data<1, 2, 3>::Value)
"#,
                "namespace template 'Data' received too many positional arguments",
            ),
            (
                "unknown named argument",
                r#"
namespace Data<S = 1, C = 1>:
  const Value = S + C

outs:
  out1

sample:
  out1 = f32(Data<Rows = 4>::Value)
"#,
                "namespace template 'Data' received unknown named arguments: Rows",
            ),
            (
                "duplicate named argument",
                r#"
namespace Data<S = 1, C = 1>:
  const Value = S + C

outs:
  out1

sample:
  out1 = f32(Data<C = 2, C = 3>::Value)
"#,
                "namespace template 'Data' argument 'C' specified more than once",
            ),
            (
                "unknown namespace template",
                r#"
outs:
  out1

sample:
  out1 = f32(Missing<1>::Value)
"#,
                "unknown namespace template 'Missing'",
            ),
        ];

        for (label, src, expected) in cases {
            let program = parse_program(src).unwrap_or_else(|err| panic!("{label}: {err:?}"));
            let errors = analyze(program).expect_err(label);
            assert!(
                errors.iter().any(|diag| diag.message.contains(expected)),
                "{label}: expected {expected:?}, got {errors:?}"
            );
        }
    }

    #[test]
    fn const_array_size_with_runtime_symbol_fails_in_semantics() {
        let src = r#"
const Table: f32[BadSize] = [1.0]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should preserve const array size");
        let errors = analyze(program).expect_err("invalid const array size should fail");
        assert!(
            errors.iter().any(|diag| {
                let message = &diag.message;
                message.contains("const array 'Table' size uses non-constant symbol 'BadSize'")
                    || message.contains(
                        "const array 'Table' size must be a compile-time integer constant expression",
                    )
            }),
            "diagnostics: {errors:?}"
        );
    }

    #[test]
    fn const_array_writes_are_rejected() {
        let src = r#"
const Table = [1, 2, 3]

outs:
  out1

sample:
  Table[0] = 4
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array write should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("cannot assign to immutable array alias 'Table'")));
    }

    #[test]
    fn namespaced_const_array_alias_writes_are_rejected() {
        let src = r#"
namespace LUT:
  const Table: f32[2] = [0.25, 0.5]

namespace Picked = LUT

outs:
  out1

sample:
  Picked::Table[0] = 0.0
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("namespaced const array write should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("cannot assign to immutable array alias 'LUT::Table'")),
            "diagnostics: {errors:?}"
        );
    }
    #[test]
    fn const_arrays_can_be_passed_to_readonly_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def sum_first_last(arr: f32[]):
  return arr[0] + arr[arr.len() - 1]

outs:
  out1

sample:
  out1 = sum_first_last(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("const array readonly def arg should analyze");
    }

    #[test]
    fn const_array_slices_can_be_passed_to_readonly_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def first(arr: f32[]):
  return arr[0]

outs:
  out1

sample:
  out1 = first(Table[:])
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("const array slice readonly def arg should analyze");
    }

    #[test]
    fn const_arrays_cannot_be_passed_to_mutating_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def write_first(arr: f32[]):
  arr[0] = 0.0
  return arr[0]

outs:
  out1

sample:
  out1 = write_first(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array mutable def arg should fail");
        assert!(errors.iter().any(|diag| diag.message.contains(
            "cannot pass immutable array alias 'Table' to mutable array parameter 'arr'"
        )));
    }

    #[test]
    fn namespaced_const_arrays_cannot_be_passed_to_mutating_array_params() {
        let src = r#"
namespace LUT:
  const Table: f32[3] = [1.0, 2.0, 3.0]

namespace Picked = LUT

def write_first(arr: f32[]):
  arr[0] = 0.0
  return arr[0]

outs:
  out1

sample:
  out1 = write_first(Picked::Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("namespaced const array mutable arg should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "cannot pass immutable array alias 'LUT::Table' to mutable array parameter 'arr'"
            )),
            "diagnostics: {errors:?}"
        );
    }
    #[test]
    fn const_arrays_cannot_be_passed_through_mutating_array_alias_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def write_alias(arr: f32[]):
  view = arr[:]
  view[0] = 0.0
  return view[0]

outs:
  out1

sample:
  out1 = write_alias(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const array alias mutable def arg should fail");
        assert!(errors.iter().any(|diag| diag.message.contains(
            "cannot pass immutable array alias 'Table' to mutable array parameter 'arr'"
        )));
    }

    #[test]
    fn const_arrays_can_be_forwarded_through_readonly_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def first(arr: f32[]):
  return arr[0]

def wrap(arr: f32[]):
  return first(arr)

outs:
  out1

sample:
  out1 = wrap(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("const array readonly forwarded def arg should analyze");
    }

    #[test]
    fn const_arrays_cannot_be_forwarded_to_mutating_array_params() {
        let src = r#"
const Table: f32[3] = [1.0, 2.0, 3.0]

def write_first(arr: f32[]):
  arr[0] = 0.0
  return arr[0]

def wrap(arr: f32[]):
  return write_first(arr)

outs:
  out1

sample:
  out1 = wrap(Table)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("const array forwarded mutable def arg should fail");
        assert!(errors.iter().any(|diag| diag.message.contains(
            "cannot pass immutable array alias 'Table' to mutable array parameter 'arr'"
        )));
    }

    #[test]
    fn def_bodies_can_read_const_arrays() {
        let src = r#"
const Table = [1.0, 2.0, 3.0]

def pick(i: i32):
  return Table[i]

outs:
  out1

sample:
  out1 = pick(1)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("def const array read should analyze");
    }

    #[test]
    fn runtime_const_array_indexes_publish_element_return_types() {
        let src = r#"
const Table: i32[2] = [1, 2]

def lookup(index: i32):
  return Table[index]

def consume(value: i32):
  return value

params:
  index: i32 = 0

sample:
  out1 = f32(consume(lookup(index)))
"#;
        let typed = analyze(parse_program(src).expect("source should parse"))
            .expect("const array element types should be visible to def return inference");

        assert!(typed.defs.iter().any(|function| {
            function.name == "lookup"
                && function.return_ty == ReturnType::Scalar(PrimitiveType::I32)
        }));
        lower_program_to_optimized_mir(&typed)
            .expect("runtime const-array lookup should lower to MIR");
    }

    #[test]
    fn const_array_len_and_static_index_are_compile_time_evaluable() {
        let src = r#"
const Table: i32[3] = [2, 4, 8]
const Picked = Table[2]

namespace Check:
  assert(Table.len() == 3)
  assert(Picked == 8)

outs:
  out1

sample:
  out1 = f32(Table[1])
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("const array compile-time reads should analyze");
    }

    #[test]
    fn const_array_static_index_oob_is_semantic_error() {
        let src = r#"
const Table = [1, 2]

namespace Check:
  assert(Table[2] == 0)

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("oob const array index should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const array 'Table' index 2 is out of bounds for length 2")));
    }

    #[test]
    fn const_array_values_can_initialize_fixed_array_defaults() {
        let src = r#"
const Spread: f32[2] = [0.2, 0.8]

ins:
  freqs: f32[2] = Spread

params:
  pan: f32[2] = Spread

outs:
  out1

sample:
  out1 = freqs[0] + pan[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const array defaults should analyze");

        assert_eq!(
            typed.in_defaults.get("freqs[0]"),
            Some(&TypedConstValue::F32(0.2))
        );
        assert_eq!(
            typed.in_defaults.get("freqs[1]"),
            Some(&TypedConstValue::F32(0.8))
        );

        let pan_defaults = typed
            .params
            .iter()
            .map(|param| (param.name.as_str(), param.default))
            .collect::<HashMap<_, _>>();
        assert_eq!(pan_defaults.get("pan[0]"), Some(&TypedConstValue::F32(0.2)));
        assert_eq!(pan_defaults.get("pan[1]"), Some(&TypedConstValue::F32(0.8)));
    }

    #[test]
    fn const_array_fixed_array_defaults_require_matching_length() {
        let src = r#"
const Spread: f32[3] = [0.2, 0.5, 0.8]

ins:
  freqs: f32[2] = Spread

outs:
  out1

sample:
  out1 = freqs[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-length const array default should fail");

        assert!(errors.iter().any(|diag| diag.message.contains(
            "input 'freqs' default const array 'Spread' has type f32[3], expected f32[2]"
        )));
    }

    #[test]
    fn const_array_fixed_array_defaults_require_matching_element_type() {
        let src = r#"
const Spread: f32[2] = [0.2, 0.8]

params:
  pan: f64[2] = Spread

outs:
  out1

sample:
  out1 = f32(pan[0])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-type const array default should fail");

        assert!(errors.iter().any(|diag| diag.message.contains(
            "param '<top-level>.pan' default const array 'Spread' has type f32[2], expected f64[2]"
        )));
    }

    #[test]
    fn const_array_event_defaults_require_matching_element_type() {
        let src = r#"
const Curve: f32[2] = [0.25, 0.75]

init:
  value = 0.0

event set_curve(curve: f64[2] = Curve):
  value = f32(curve[0])

outs:
  out1

sample:
  out1 = value
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("wrong-type event const array default should fail");

        assert!(errors.iter().any(|diag| diag.message.contains(
            "event 'set_curve.curve' default const array 'Curve' has type f32[2], expected f64[2]"
        )));
    }

    #[test]
    fn scalar_const_defs_can_initialize_const_array_elements() {
        let src = r#"
const def twice(x: f32) -> f32:
  return x * 2.0

const Table: f32[2] = [twice(0.5), twice(1.0)]

outs:
  out1

sample:
  out1 = Table[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def call in const array should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![TypedConstValue::F32(1.0), TypedConstValue::F32(2.0)]
        );
        assert!(
            typed.defs.iter().all(|def| def.name != "twice"),
            "const defs should not be emitted as runtime defs"
        );
    }

    #[test]
    fn scalar_const_defs_can_call_earlier_const_defs() {
        let src = r#"
const def base() -> i32:
  return 21

const def doubled() -> i32:
  return base() * 2

const Table = [doubled()]

outs:
  out1

sample:
  out1 = f32(Table[0])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def calling earlier const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.elem_ty, PrimitiveType::I32);
        assert_eq!(table.values, vec![TypedConstValue::I32(42)]);
    }

    #[test]
    fn const_def_names_conflict_with_runtime_defs() {
        let src = r#"
const def foo() -> f32:
  return 0.25

def foo(x: f32):
  return x * 2.0

outs:
  out1

sample:
  out1 = foo(0.5)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const def/runtime def name conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def name 'foo' conflicts with existing symbol")),
            "expected const def name conflict, got {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|diag| !diag.message.contains("expects 0 argument")),
            "runtime def call should not be intercepted by const folding: {errors:?}"
        );
    }

    #[test]
    fn scalar_const_names_conflict_with_params() {
        let src = r#"
const gain = 1.0

params:
  gain = 0.5

outs:
  out1

sample:
  out1 = gain
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const/param name conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant name 'gain' conflicts with existing symbol")),
            "expected const/param conflict, got {errors:?}"
        );
    }

    #[test]
    fn scalar_const_names_conflict_with_earlier_params_without_forward_ref_noise() {
        let src = r#"
params:
  gain = 0.5

outs:
  out1

sample:
  out1 = gain

const gain = 1.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("param/const name conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant name 'gain' conflicts with existing symbol")),
            "expected param/const conflict, got {errors:?}"
        );
        assert!(
            errors
                .iter()
                .all(|diag| !diag.message.contains("not visible before its declaration")),
            "runtime param read should not be reported as forward const use: {errors:?}"
        );
    }

    #[test]
    fn scalar_const_names_conflict_with_function_params() {
        let src = r#"
const X = 1.0

outs:
  out1

def f(X: f32):
  return X

sample:
  out1 = f(2.0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const/function parameter conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("function parameter 'X' conflicts with constant 'X'")),
            "expected function parameter/const conflict, got {errors:?}"
        );
    }

    #[test]
    fn const_array_names_conflict_with_array_function_params() {
        let src = r#"
const Table: f32[] = [1.0]

outs:
  out1

def first(Table: f32[]):
  return Table[0]

sample:
  out1 = first([2.0])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("const array/function parameter conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("function parameter 'Table' conflicts with constant 'Table'")),
            "expected function parameter/const array conflict, got {errors:?}"
        );
    }

    #[test]
    fn scalar_const_names_conflict_with_runtime_defs() {
        let src = r#"
const foo = 1.0

def foo(x: f32):
  return x * 2.0

outs:
  out1

sample:
  out1 = foo(0.5)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const/runtime def name conflict should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant name 'foo' conflicts with existing symbol")),
            "expected const/runtime def conflict, got {errors:?}"
        );
    }

    #[test]
    fn runtime_const_refs_reject_forward_scalar_consts() {
        let src = r#"
outs:
  out1

sample:
  out1 = Later

const Later = 0.5
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward scalar const use should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant 'Later' is not visible before its declaration")),
            "expected forward scalar const diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn runtime_const_refs_reject_forward_const_arrays() {
        let src = r#"
outs:
  out1

sample:
  idx = 0
  out1 = Table[idx]

const Table: f32[1] = [0.5]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const array use should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant 'Table' is not visible before its declaration")),
            "expected forward const array diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn declaration_defaults_reject_forward_const_arrays() {
        let src = r#"
params:
  taps: f32[2] = Table

const Table: f32[2] = [0.25, 0.75]

outs:
  out1

sample:
  out1 = taps[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const array default should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant 'Table' is not visible before its declaration")),
            "expected forward const array default diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn asserts_reject_forward_scalar_consts() {
        let src = r#"
namespace Check:
  assert(Later == 1)

const Later = 1

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const assert should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("constant 'Later' is not visible before its declaration")),
            "expected forward const assert diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn const_def_return_types_are_required_and_enforced() {
        let src = r#"
const def missing():
  return 1.0

const Table: f32[1] = [missing()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("missing return type should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'missing' must declare an explicit return type")));

        let src = r#"
const def unused_missing():
  return 1.0

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unused missing return type should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'unused_missing' must declare an explicit return type")));

        let src = r#"
const def bad_scalar() -> i32:
  return 0.5

const Table: i32[1] = [bad_scalar()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong scalar return type should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'bad_scalar' return must be an integer constant")));

        let src = r#"
const def bad_array() -> f32[2]:
  return [0.25]

const Table: f32[2] = bad_array()

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong array return shape should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def 'bad_array' return: expected array length 2, got 1")),
            "expected const def return shape diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn unused_const_def_bodies_are_structurally_validated() {
        let src = r#"
const def unused_bad() -> f32:
  sin(0.0)
  return 1.0

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unused unsupported const def body should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def 'unused_bad' statement is not supported")),
            "expected unsupported const def statement diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn const_def_local_consts_are_immutable() {
        let src = r#"
const def bad() -> i32:
  const X = 1
  X = 2
  return X

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("local const reassignment should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def 'bad' cannot assign to local const 'X'")),
            "expected local const reassignment diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn unused_const_def_loop_vars_cannot_rebind_local_consts() {
        let src = r#"
const def bad() -> i32:
  const X = 1
  for X in 0..2:
    const Y = X
  return X

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("loop var local const reassignment should fail");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("const def 'bad' cannot assign to local const 'X'")),
            "expected loop var local const diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn scalar_const_defs_can_initialize_scalar_consts() {
        let src = r#"
const def curve_gain(x: f64) -> f64:
  return x * x + 0.12345678901234568

const Gain = curve_gain(0.5)
const Table: f64[1] = [Gain]

outs:
  out1

sample:
  out1 = f32(Gain)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def scalar const should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.elem_ty, PrimitiveType::F64);
        assert_eq!(
            table.values,
            vec![TypedConstValue::F64(
                0.5_f64 * 0.5_f64 + 0.12345678901234568
            )]
        );
    }

    #[test]
    fn scalar_consts_from_const_defs_preserve_i64_precision() {
        let src = r#"
const def big() -> i64:
  return 9007199254740993

const Big = big()
const Table: i64[1] = [Big]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("i64 const def scalar const should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.elem_ty, PrimitiveType::I64);
        assert_eq!(table.values, vec![TypedConstValue::I64(9007199254740993)]);
    }

    #[test]
    fn folded_intrinsics_preserve_every_numeric_scalar_type_and_precision() {
        let src = r#"
const I32Value = min(i32(1024), 4096)
const I64Value = min(i64(9007199254740993), 9007199254740995)
const F32Value = fma(f32(16777217), f32(1), f32(-16777216))
const F64Value = min(f64(1.0000000000000002), f64(1.0000000000000004))

const I32Values: i32[3] = [I32Value, max(i32(-7), -3), abs(i32(-11))]
const I64Values: i64[3] = [
  I64Value,
  max(i64(9007199254740993), 9007199254740995),
  abs(i64(-9007199254740993))
]
const F32Values: f32[1] = [F32Value]
const F64Values: f64[1] = [F64Value]

outs:
  out1

sample:
  out1 = f32(I32Value >> 3) + f32(I64Value >> 53) + F32Value + f32(F64Value)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("typed intrinsic constants should analyze");

        let values = |name: &str| {
            typed
                .const_arrays
                .iter()
                .find(|array| array.name == name)
                .unwrap_or_else(|| panic!("missing const array '{name}'"))
                .values
                .clone()
        };
        assert_eq!(
            values("I32Values"),
            vec![
                TypedConstValue::I32(1024),
                TypedConstValue::I32(-3),
                TypedConstValue::I32(11),
            ]
        );
        assert_eq!(
            values("I64Values"),
            vec![
                TypedConstValue::I64(9007199254740993),
                TypedConstValue::I64(9007199254740995),
                TypedConstValue::I64(9007199254740993),
            ]
        );
        assert_eq!(values("F32Values"), vec![TypedConstValue::F32(0.0)]);
        assert_eq!(
            values("F64Values"),
            vec![TypedConstValue::F64(1.0000000000000002)]
        );

        lower_program_to_optimized_mir(&typed)
            .expect("typed folded intrinsics should lower to MIR without casts at use sites");
    }

    #[test]
    fn scalar_consts_can_depend_on_semantic_scalar_consts() {
        let src = r#"
const def base() -> f64:
  return 0.25

const A = base()
const B: f64 = A + 0.125
const Table: f64[1] = [B]

outs:
  out1

sample:
  out1 = f32(B)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dependent semantic scalar consts should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.values, vec![TypedConstValue::F64(0.375)]);
    }

    #[test]
    fn namespace_scalar_consts_from_const_defs_initialize_const_arrays() {
        let src = r#"
namespace LUT:
  const def gain() -> f32:
    return 0.25

  const Gain = gain()
  const Table: f32[1] = [Gain]

outs:
  out1

sample:
  out1 = LUT::Table[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("namespaced semantic scalar const should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.values, vec![TypedConstValue::F32(0.25)]);
    }

