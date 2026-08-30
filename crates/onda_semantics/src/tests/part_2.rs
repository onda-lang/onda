    #[test]
    fn const_defs_reject_forward_references_from_bodies() {
        let src = r#"
const def earlier() -> f32:
  return later()

const def later() -> f32:
  return 1.0

const Table: f32[1] = [earlier()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const def call should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'later' is not visible from const def 'earlier'")));
    }

    #[test]
    fn const_defs_reject_forward_references_from_param_defaults() {
        let src = r#"
const def earlier(x: f32 = later()) -> f32:
  return x

const def later() -> f32:
  return 1.0

const Table: f32[1] = [earlier()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("forward const def default should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'later' is not visible from const def 'earlier'")));
    }

    #[test]
    fn const_defs_reject_direct_recursion() {
        let src = r#"
const def recurse() -> f32:
  return recurse()

const Table: f32[1] = [recurse()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("recursive const def should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("recursive const def call involving 'recurse'")));
    }

    #[test]
    fn const_defs_reject_mutual_recursion() {
        let src = r#"
const def a() -> f32:
  return b()

const def b() -> f32:
  return a()

const Table: f32[1] = [b()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mutually recursive const defs should fail");

        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("recursive const def call involving")));
    }

    #[test]
    fn const_defs_reject_runtime_symbol_access() {
        let src = r#"
const def read_input() -> f32:
  return in1

ins:
  in1

const Table: f32[1] = [read_input()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("runtime symbol const def should fail");

        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("non-constant symbol 'in1'")));
    }

    #[test]
    fn const_defs_reject_ordinary_def_calls() {
        let src = r#"
def runtime_helper() -> f32:
  return 1.0

const def build() -> f32:
  return runtime_helper()

const Table: f32[1] = [build()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("ordinary def const call should fail");

        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("unknown const def 'runtime_helper'")));
    }

    #[test]
    fn const_defs_report_loop_iteration_cap() {
        let src = r#"
const def runaway() -> i32:
  loop 1000001:
    x = _
  return 0

const Table: i32[1] = [runaway()]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const def loop cap should fail");

        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("loop exceeded 1000000 iterations")));
    }

    #[test]
    fn const_defs_can_read_fixed_array_params() {
        let src = r#"
const Source: f32[3] = [0.25, 0.5, 1.0]

const def mix(xs: f32[3]) -> f32:
  return xs[0] + xs[2]

const Table: f32[2] = [mix(Source), mix([1.0, 2.0, 4.0])]

outs:
  out1

sample:
  out1 = Table[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("fixed-array const def params should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![TypedConstValue::F32(1.25), TypedConstValue::F32(5.0)]
        );
    }

    #[test]
    fn const_defs_can_read_any_length_array_params() {
        let src = r#"
const A: f32[] = [0.25, 0.5, 1.0]
const B: f32[] = [2.0, 4.0]

const def sum(xs: f32[]) -> f32:
  total = 0.0
  for i in 0..xs.len():
    total = total + xs[i]
  return total

const Table: f32[] = [sum(A), sum(B), sum([10.0, 20.0, 30.0, 40.0])]

outs:
  out1

sample:
  out1 = Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("typed slice const def params should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(table.len, 3);
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(1.75),
                TypedConstValue::F32(6.0),
                TypedConstValue::F32(100.0)
            ]
        );
    }

    #[test]
    fn const_defs_can_read_untyped_any_primitive_array_params() {
        let src = r#"
const F: f32[] = [0.25, 0.5]
const I: i32[] = [10, 20, 30]

const def size(xs: []) -> i32:
  return xs.len()

const Sizes: i32[] = [size(F), size(I), size([true, false, true, false])]

outs:
  out1

sample:
  out1 = f32(Sizes[2])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("untyped slice const def params should analyze");

        let sizes = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Sizes")
            .expect("typed const array");
        assert_eq!(
            sizes.values,
            vec![
                TypedConstValue::I32(2),
                TypedConstValue::I32(3),
                TypedConstValue::I32(4)
            ]
        );
    }

    #[test]
    fn const_def_slice_params_are_read_only() {
        let src = r#"
const Source: f32[] = [0.25, 0.5]

const def bad(xs: f32[]) -> f32:
  xs[0] = 1.0
  return xs[0]

const Value = bad(Source)

outs:
  out1

sample:
  out1 = Value
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("read-only const def slice write should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'bad' cannot write read-only array parameter 'xs'")));
    }

    #[test]
    fn const_def_typed_slice_params_reject_wrong_element_type() {
        let src = r#"
const Source: i32[] = [1, 2]

const def first(xs: f32[]) -> f32:
  return xs[0]

const Value = first(Source)

outs:
  out1

sample:
  out1 = Value
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-type const def slice arg should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'first' argument 'xs': expected f32[], got i32[2]")));
    }

    #[test]
    fn const_array_slice_annotations_infer_initializer_length() {
        let src = r#"
const Full: f32[] = [0.0, 0.25, 0.5, 0.75]
const Mid: f32[] = Full[1:-1]

outs:
  out1

sample:
  out1 = Mid[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const slice annotation should infer length");

        let full = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Full")
            .expect("typed full array");
        assert_eq!(full.len, 4);

        let mid = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Mid")
            .expect("typed mid array");
        assert_eq!(mid.len, 2);
        assert_eq!(
            mid.values,
            vec![TypedConstValue::F32(0.25), TypedConstValue::F32(0.5)]
        );
    }

    #[test]
    fn const_defs_can_return_arrays_derived_from_fixed_array_params() {
        let src = r#"
namespace LUT<N = 3>:
  const Base: f32[N] = [1.0, 2.0, 3.0]

  const def scale(xs: f32[N], gain: f32) -> f32[N]:
    values: f32[N]
    for i in 0..N:
      values[i] = xs[i] * gain
    return values

  const Table: f32[N] = scale(Base, 0.5)

outs:
  out1

sample:
  out1 = LUT::Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("array-param const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(0.5),
                TypedConstValue::F32(1.0),
                TypedConstValue::F32(1.5)
            ]
        );
    }

    #[test]
    fn const_defs_can_pass_local_arrays_to_fixed_array_params() {
        let src = r#"
const def copy(xs: f32[2]) -> f32[2]:
  return xs

const def swapped(xs: f32[2]) -> f32[2]:
  values: f32[2]
  values[0] = xs[1]
  values[1] = xs[0]
  return copy(values)

const Table: f32[2] = copy(swapped([1.0, 2.0]))

outs:
  out1

sample:
  out1 = Table[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("local array const-def arg should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![TypedConstValue::F32(2.0), TypedConstValue::F32(1.0)]
        );
    }

    #[test]
    fn const_def_fixed_array_params_require_matching_shape() {
        let src = r#"
const Source: f32[2] = [0.25, 0.5]

const def first(xs: f32[3]) -> f32:
  return xs[0]

const Table: f32[1] = [first(Source)]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-shape const def array arg should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("const def 'first' argument 'xs': expected f32[3], got f32[2]")));
    }

    #[test]
    fn namespaced_scalar_const_defs_can_initialize_const_arrays() {
        let src = r#"
namespace LUT:
  const def gain() -> f32:
    return 0.25

  const Table: f32[1] = [gain()]

outs:
  out1

sample:
  out1 = LUT::Table[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("namespace const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.values, vec![TypedConstValue::F32(0.25)]);
    }

    #[test]
    fn const_defs_can_return_array_literals_for_const_array_initializers() {
        let src = r#"
const def table() -> f32[3]:
  return [0.25, 0.5, 1.0]

const Table: f32[3] = table()

outs:
  out1

sample:
  out1 = Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("array-returning const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(0.25),
                TypedConstValue::F32(0.5),
                TypedConstValue::F32(1.0)
            ]
        );
        assert!(
            typed.defs.iter().all(|def| def.name != "table"),
            "const defs should not be emitted as runtime defs"
        );
    }

    #[test]
    fn untyped_const_can_infer_array_returning_const_def_initializer() {
        let src = r#"
const N = 3

const def harmonic_ratios() -> f32[N]:
  values: f32[N]
  for i in 0..N:
    values[i] = f32(i + 1)
  return values

const Ratios = harmonic_ratios()

outs:
  out1

sample:
  out1 = Ratios[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("untyped array const def initializer should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Ratios")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(1.0),
                TypedConstValue::F32(2.0),
                TypedConstValue::F32(3.0)
            ]
        );
    }

    #[test]
    fn const_defs_can_fill_local_arrays_with_compile_time_loops() {
        let src = r#"
const def ramp() -> f32[4]:
  values: f32[4]
  for i in 0..4:
    values[i] = f32(i) + 0.5
  return values

const Table: f32[4] = ramp()

outs:
  out1

sample:
  out1 = Table[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("loop-filled const def array should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name == "Table")
            .expect("typed const array");
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::F32(0.5),
                TypedConstValue::F32(1.5),
                TypedConstValue::F32(2.5),
                TypedConstValue::F32(3.5)
            ]
        );
    }

    #[test]
    fn namespace_const_defs_can_return_arrays_using_namespace_sizes() {
        let src = r#"
namespace LUT<N = 3>:
  const def ramp() -> i32[N]:
    values: i32[N]
    loop N:
      values[_] = _ * 2
    return values

  const Table: i32[N] = ramp()

outs:
  out1

sample:
  out1 = f32(LUT::Table[2])
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("namespaced array-returning const def should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("typed const array");
        assert_eq!(table.elem_ty, PrimitiveType::I32);
        assert_eq!(
            table.values,
            vec![
                TypedConstValue::I32(0),
                TypedConstValue::I32(2),
                TypedConstValue::I32(4)
            ]
        );
    }

    #[test]
    fn const_defs_can_build_window_tables_with_builtin_math() {
        let src = r#"
namespace Windows<N = 4>:
  const def hann() -> f32[N]:
    values: f32[N]
    for i in 0..N:
      phase = TWO_PI * f32(i) / f32(N - 1)
      values[i] = 0.5 - 0.5 * cos(phase)
    return values

  const Hann: f32[N] = hann()

outs:
  out1

sample:
  out1 = Windows::Hann[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("const def hann table should analyze");

        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Hann"))
            .expect("typed const array");
        let values = table
            .values
            .iter()
            .map(|value| match value {
                TypedConstValue::F32(value) => *value,
                other => panic!("expected f32 value, got {other:?}"),
            })
            .collect::<Vec<_>>();
        assert!((values[0] - 0.0).abs() < 1e-6);
        assert!((values[1] - 0.75).abs() < 1e-6);
        assert!((values[2] - 0.75).abs() < 1e-6);
        assert!((values[3] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn non_const_defs_reject_array_return_annotations() {
        let src = r#"
def table() -> f32[2]:
  return [0.0, 1.0]

outs:
  out1

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("ordinary def array return should fail");

        assert!(errors.iter().any(|diag| diag
            .message
            .contains("function 'table' array return types are only supported for const defs")));
    }

    #[test]
    fn overload_diagnostics_use_call_spans() {
        let src = "outs:\n  out1\ndef foo(x: f32):\n  return x\ndef foo(x: f64):\n  return f32(x)\nsample:\n  out1 = foo(1)\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("ambiguous overload should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("ambiguous overload for function 'foo'")
            })
            .expect("missing ambiguous overload diagnostic");

        assert_eq!((diag.line, diag.column), (8, 10));
        assert_eq!(diag.end_line, 8);
    }

    #[test]
    fn def_body_assignment_diagnostics_use_rhs_spans() {
        let src = "outs:\n  out1\ndef foo():\n  a = [0.0]\n  a[0] = false\n  return 0.0\nsample:\n  out1 = foo()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("def body type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("array/buffer write type mismatch"))
            .expect("missing def body assignment diagnostic");

        assert_eq!((diag.line, diag.column), (5, 10));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn def_body_assignment_diagnostics_use_target_spans() {
        let src = "outs:\n  out1\ndef foo():\n  PI = 1.0\n  return 0.0\nsample:\n  out1 = foo()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("builtin constant assignment should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("cannot assign to builtin constant 'PI'")
            })
            .expect("missing builtin constant assignment diagnostic");

        assert_eq!((diag.line, diag.column), (4, 3));
        assert_eq!(diag.end_line, 4);
        assert_eq!(diag.end_column, 5);
    }

    #[test]
    fn init_assignment_diagnostics_use_target_spans() {
        let src = "outs:\n  out1\ninit:\n  PI = 1.0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("init builtin constant assignment should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("cannot assign to builtin constant 'PI'")
            })
            .expect("missing init builtin constant assignment diagnostic");

        assert_eq!((diag.line, diag.column), (4, 3));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn runtime_assignment_diagnostics_use_target_spans() {
        let src = "outs:\n  out1\nsample:\n  PI = 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("sample builtin constant assignment should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("cannot assign to builtin constant 'PI'")
            })
            .expect("missing sample builtin constant assignment diagnostic");

        assert_eq!((diag.line, diag.column), (4, 3));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn init_assignment_diagnostics_use_rhs_spans() {
        let src = "outs:\n  out1\ninit:\n  a = [0.0]\n  a[0] = false\nsample:\n  out1 = a[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("init array write type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("array/buffer write type mismatch"))
            .expect("missing init array write diagnostic");

        assert_eq!((diag.line, diag.column), (5, 10));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn runtime_assignment_diagnostics_use_rhs_spans() {
        let src = "outs:\n  out1\nsample:\n  a = [0.0]\n  a[0] = false\n  out1 = a[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("sample array write type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("array/buffer write type mismatch"))
            .expect("missing sample array write diagnostic");

        assert_eq!((diag.line, diag.column), (5, 10));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn runtime_slice_bound_diagnostics_use_bound_spans() {
        let src = "outs:\n  out1\nsample:\n  a = [0.0, 0.0]\n  a[false:] = 0.5\n  out1 = a[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("slice bound type mismatch should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("slice start bound requires numeric type")
            })
            .expect("missing slice bound diagnostic");

        assert_eq!((diag.line, diag.column), (5, 5));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn runtime_slice_bound_diagnostics_use_const_use_site_spans() {
        let src = "const BAD = false\nouts:\n  out1\nsample:\n  a = [0.0, 0.0]\n  a[BAD:] = 0.5\n  out1 = a[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("const-expanded slice bound should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("slice start bound requires numeric type")
            })
            .expect("missing slice bound diagnostic");

        assert_eq!((diag.line, diag.column), (6, 5));
        assert_eq!(diag.end_line, 6);
        assert_eq!(diag.end_column, 8);
    }

    #[test]
    fn init_array_literal_empty_diagnostics_use_expr_spans() {
        let src = "outs:\n  out1\ninit:\n  a = []\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("empty init array literal should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("array initializer for symbol 'a' cannot be empty")
            })
            .expect("missing empty array initializer diagnostic");

        assert_eq!((diag.line, diag.column), (4, 7));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn runtime_typed_array_size_diagnostics_use_size_spans() {
        let src = "outs:\n  out1\nsample:\n  a: f32[1.5] = [1.0]\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("non-integer typed array size should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("typed array declaration size for symbol 'a' in sample must evaluate to an integer value")
            })
            .expect("missing typed array size diagnostic");

        assert_eq!((diag.line, diag.column), (4, 10));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn def_array_literal_empty_diagnostics_use_expr_spans() {
        let src = "outs:\n  out1\ndef foo():\n  a = []\n  return 0.0\nsample:\n  out1 = foo()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("empty def array literal should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("array initializer for symbol 'a' cannot be empty")
            })
            .expect("missing empty def array initializer diagnostic");

        assert_eq!((diag.line, diag.column), (4, 7));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn def_typed_array_size_diagnostics_use_size_spans() {
        let src =
            "outs:\n  out1\ndef foo():\n  a: f32[1.5] = [1.0]\n  return 0.0\nsample:\n  out1 = foo()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("non-integer def typed array size should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("typed array declaration size for symbol 'a' in def must evaluate to an integer value")
            })
            .expect("missing def typed array size diagnostic");

        assert_eq!((diag.line, diag.column), (4, 10));
        assert_eq!(diag.end_line, 4);
    }

    #[test]
    fn proc_array_size_diagnostics_use_size_spans() {
        let src = "proc Voice:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nouts:\n  out1\ninit:\n  voices: Voice[1.5] = Voice()\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("non-integer proc array size should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message.contains(
                    "top-level processor array 'voices' size must evaluate to an integer value",
                )
            })
            .expect("missing proc array size diagnostic");

        assert_eq!((diag.line, diag.column), (9, 17));
        assert_eq!(diag.end_line, 9);
    }

    #[test]
    fn proc_array_initializer_entry_diagnostics_use_entry_spans() {
        let src = "proc Voice:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nproc Other:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nouts:\n  out1\ninit:\n  voices: Voice[2] = [Other(), Voice()]\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mismatched proc array initializer should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message.contains(
                    "top-level processor array 'voices' initializer entry 0 uses constructor 'Other' but 'Voice' is required",
                )
            })
            .expect("missing proc array initializer entry diagnostic");

        assert_eq!((diag.line, diag.column), (14, 23));
        assert_eq!(diag.end_line, 14);
    }

    #[test]
    fn duplicate_block_diagnostics_use_block_spans() {
        let src =
            "outs:\n  out1\nparams:\n  gain = 0.5\nparams:\n  mix = 0.25\nsample:\n  out1 = gain\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("duplicate params block should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("duplicate block 'params'"))
            .expect("missing duplicate block diagnostic");

        assert_eq!((diag.line, diag.column), (5, 1));
    }

    #[test]
    fn missing_sample_diagnostic_uses_nearest_block_span() {
        let src = "outs:\n  out1\nparams:\n  gain = 0.5\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("missing sample block should fail");
        let diag = errors
            .iter()
            .find(|diag| diag.message.contains("missing required 'sample' block"))
            .expect("missing sample diagnostic");

        assert_eq!((diag.line, diag.column), (3, 1));
        assert!(!diag.editor_visible, "missing sample is compile-only");
    }

    #[test]
    fn untyped_top_level_params_infer_type_from_const_defaults() {
        let src = r#"
outs:
  out1
params:
  bare
  float_default = 0.0
  int_default = 0
  int_expr = 1 + 2
  float_expr = PI * 2.0
  explicit_f64: f64 = 0.0
  explicit_i64: i64 = 0
sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("param defaults should infer");

        assert_eq!(typed.param_types.get("bare"), Some(&PrimitiveType::F32));
        assert_eq!(
            typed.param_types.get("float_default"),
            Some(&PrimitiveType::F32)
        );
        assert_eq!(
            typed.param_types.get("int_default"),
            Some(&PrimitiveType::I32)
        );
        assert_eq!(typed.param_types.get("int_expr"), Some(&PrimitiveType::I32));
        assert_eq!(
            typed.param_types.get("float_expr"),
            Some(&PrimitiveType::F32)
        );
        assert_eq!(
            typed.param_types.get("explicit_f64"),
            Some(&PrimitiveType::F64)
        );
        assert_eq!(
            typed.param_types.get("explicit_i64"),
            Some(&PrimitiveType::I64)
        );
    }

    #[test]
    fn untyped_proc_params_infer_type_from_const_defaults() {
        let src = r#"
proc Voice:
  params:
    bare
    float_default = 0.0
    int_default = 0
    int_expr = 1 + 2
    float_expr = PI * 2.0
    explicit_f64: f64 = 0.0
    explicit_i64: i64 = 0
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1

init:
  voice = Voice()

sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc param defaults should infer");
        let voice = typed
            .structs
            .iter()
            .find(|s| s.name == "Voice")
            .expect("missing lowered Voice struct");

        let param_tys = voice
            .fields
            .iter()
            .filter_map(|field| {
                let ty = match field.ty {
                    TypedFieldType::Scalar(ty) => ty,
                    _ => return None,
                };
                match field.name.as_str() {
                    "bare" => Some(("bare", ty)),
                    "float_default" => Some(("float_default", ty)),
                    "int_default" => Some(("int_default", ty)),
                    "int_expr" => Some(("int_expr", ty)),
                    "float_expr" => Some(("float_expr", ty)),
                    "explicit_f64" => Some(("explicit_f64", ty)),
                    "explicit_i64" => Some(("explicit_i64", ty)),
                    _ => None,
                }
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(param_tys.get("bare"), Some(&PrimitiveType::F32));
        assert_eq!(param_tys.get("float_default"), Some(&PrimitiveType::F32));
        assert_eq!(param_tys.get("int_default"), Some(&PrimitiveType::I32));
        assert_eq!(param_tys.get("int_expr"), Some(&PrimitiveType::I32));
        assert_eq!(param_tys.get("float_expr"), Some(&PrimitiveType::F32));
        assert_eq!(param_tys.get("explicit_f64"), Some(&PrimitiveType::F64));
        assert_eq!(param_tys.get("explicit_i64"), Some(&PrimitiveType::I64));
    }

    #[test]
    fn top_level_input_array_defaults_are_typed_per_element() {
        let src = r#"
ins:
  freqs: f32[3] = [220, 440, 880]
outs:
  out1
sample:
  out1 = freqs[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("input array defaults should analyze");

        let freqs = typed
            .in_arrays
            .get("freqs")
            .expect("missing input array info");
        assert_eq!(freqs.elem_ty, PrimitiveType::F32);
        assert_eq!(freqs.len, 3);
        assert_eq!(freqs.offset, 0);
        assert_eq!(
            typed.in_defaults.get("freqs[0]"),
            Some(&TypedConstValue::F32(220.0))
        );
        assert_eq!(
            typed.in_defaults.get("freqs[1]"),
            Some(&TypedConstValue::F32(440.0))
        );
        assert_eq!(
            typed.in_defaults.get("freqs[2]"),
            Some(&TypedConstValue::F32(880.0))
        );
    }

    #[test]
    fn input_array_defaults_require_exact_length() {
        let src = r#"
ins:
  freqs: f32[3] = [220, 440]
outs:
  out1
sample:
  out1 = freqs[0]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("wrong-length input default should fail");

        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("input 'freqs' default expects 3 elements, got 2")),
            "expected array-length diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn proc_input_and_param_array_defaults_work_for_generic_processors() {
        let src = r#"
proc Voice<T>:
  ins:
    freqs: T[3] = [220, 440, 880]
  params:
    amps: T[2] = [0.5, 0.25]
  outs:
    out1
  sample:
    out1 = freqs[2] * amps[1]

outs:
  out1

init:
  voice = Voice<f32>()

sample:
  out1 = voice()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("generic proc array defaults should analyze");
    }

    #[test]
    fn uninstantiated_generic_proc_rejects_unknown_forwarded_ctor_type_arg() {
        let src = r#"
proc Child<T>:
  outs<T> 1
  sample:
    out1 = 0.0

proc Wrapper<T>:
  init:
    child = Child<thisisnotvalid>()
  sample:
    child()
"#;

        assert_analyze_error_contains(src, "unknown generic type argument 'thisisnotvalid'");
    }

    #[test]
    fn uninstantiated_generic_proc_allows_declared_forwarded_ctor_type_arg() {
        let src = r#"
proc Child<T>:
  outs<T> 1
  sample:
    out1 = 0.0

proc Wrapper<T>:
  init:
    child = Child<T>()
  sample:
    child()
"#;

        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("declared forwarded generic type arg should analyze");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_rejects_unknown_forwarded_ctor_type_arg() {
        let src = r#"
namespace dsp:
  proc Child<T>:
    outs<T> 1
    sample:
      out1 = 0.0

  namespace Wrap<N = 4>:
    namespace Mono:
      proc Parent<T>:
        init:
          child = Child<thisisnotvalid>()
        sample:
          child()
"#;

        assert_analyze_error_contains(src, "unknown generic type argument 'thisisnotvalid'");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_allows_declared_forwarded_ctor_type_arg() {
        let src = r#"
namespace dsp:
  proc Child<T>:
    outs<T> 1
    sample:
      out1 = 0.0

  namespace Wrap<N = 4>:
    namespace Mono:
      proc Parent<T>:
        init:
          child = Child<T>()
        sample:
          child()
"#;

        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("declared forwarded namespace template type arg should analyze");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_rejects_unknown_template_member() {
        let src = r#"
import std/convolution

namespace Test<FFTSize = 64, MaxKernel = 1024>:
  proc Wrapper<T>:
    outs<T> 1
    init:
      t = std::convolution<FFTSize, MaxKernel>::nope
    sample:
      out1 = 0.0
"#;

        assert_analyze_error_contains(src, "unknown symbol 'nope' in namespace 'std::convolution'");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_rejects_unknown_static_array_size() {
        let src = r#"
namespace Test<MaxKernel = 1024>:
  proc Wrapper<T>:
    outs<T> 1
    init:
      current_ir: T[MaxKrnel]
    sample:
      out1 = 0.0
"#;

        assert_analyze_error_contains(src, "unknown constant 'MaxKrnel'");
    }

    #[test]
    fn uninstantiated_namespace_template_proc_allows_declared_static_array_size() {
        let src = r#"
namespace Test<MaxKernel = 1024>:
  proc Wrapper<T>:
    outs<T> 1
    init:
      current_ir: T[MaxKernel]
    sample:
      out1 = 0.0
"#;

        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("declared namespace template size should analyze");
    }

    #[test]
    fn proc_input_and_param_array_indices_preserve_declared_element_type() {
        let src = r#"
proc Voice:
  ins:
    vals: i64[2] = [5, 6]
  params:
    gains: i64[2] = [1, 2]
  outs:
    out1
  init:
    start: i64 = gains[0]
  sample:
    total: i64 = vals[0] + gains[1] + start
    out1 = f32(total)

outs:
  out1

init:
  voice = Voice()

sample:
  out1 = voice(vals = [7, 8])
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc array port and param indices should preserve i64 type");
    }

    #[test]
    fn single_slot_processor_io_surfaces_support_dynamic_indexing() {
        let src = r#"
proc Mono:
  ins 1
  outs 1

  sample:
    outs[0] = ins[0]

init:
  mono = Mono()

sample:
  out1 = mono(0.25)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("single-slot dynamic surfaces should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("single-slot dynamic surfaces should lower to MIR");
    }

    #[test]
    fn declaration_only_library_file_does_not_require_sample_block() {
        let src = "proc Mix:\n  ins:\n    dry\n    fb\n  sample:\n    out1 = (dry + fb) * 0.5\n\ndef clip(x) {\n  return x\n}\nconst SCALE = 0.5\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("declaration-only library file should analyze");
    }

    #[test]
    fn nested_proc_array_state_len_analyzes() {
        let src = r#"
proc Inner:
  init:
    line: f32[32]

  sample:
    out1 = f32(line.len())

proc Outer:
  init:
    inner = Inner()

  sample:
    out1 = inner()

init:
  outer = Outer()

sample:
  out1 = outer()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("nested proc array-state methods should analyze");
    }

    #[test]
    fn nested_proc_buffer_methods_analyze() {
        let src = r#"
proc Inner:
  buffers:
    src: f32

  block:
    frames = src.len()
    channels = src.chans()
    rate = src.samplerate()
    sample:
      out1 = f32(frames + channels) + rate

proc Outer:
  buffers:
    src: f32

  init:
    inner = Inner(src = src)

  block:
    sample:
      out1 = inner()

buffers:
  src: f32

init:
  outer = Outer(src = src)

block:
  sample:
    out1 = outer()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("nested proc buffer methods should analyze");
    }

    #[test]
    fn proc_events_receive_bound_buffers() {
        let src = r#"
proc Player:
  buffers:
    clip: f32[]

  init:
    captured = 0.0

  event capture(frame: i32):
    captured = clip[0, frame] + f32(clip.len() + clip.chans()) + clip.samplerate()
    clip[0, frame] = captured

  sample:
    out1 = captured

buffers:
  source: f32[]

events:
  capture(frame: i32):
    player.capture(frame)

init:
  player = Player(clip = source)

sample:
  out1 = player()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc events should receive their bound buffers");
        lower_program_to_optimized_mir(&typed)
            .expect("proc event buffer access should lower to MIR");
    }

    #[test]
    fn nested_proc_events_forward_bound_buffers() {
        let src = r#"
proc Player:
  buffers:
    clip: f32

  init:
    captured = 0.0

  event capture(frame: i32):
    captured = clip[frame]

  sample:
    out1 = captured

proc Parent:
  buffers:
    source: f32

  init:
    player = Player(clip = source)

  event capture(frame: i32):
    player.capture(frame)

  sample:
    out1 = player()

buffers:
  source: f32

events:
  capture(frame: i32):
    parent.capture(frame)

init:
  parent = Parent(source = source)

sample:
  out1 = parent()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("nested proc events should forward bound buffers");
        lower_program_to_optimized_mir(&typed)
            .expect("nested proc event buffer access should lower to MIR");
    }

    #[test]
    fn indexed_proc_events_forward_bound_buffers() {
        let src = r#"
proc Player:
  buffers:
    clip: f32

  init:
    captured = 0.0

  event capture(frame: i32):
    captured = clip[frame]

  sample:
    out1 = captured

buffers:
  source: f32

events:
  capture(index: i32, frame: i32):
    players[index].capture(frame)

init:
  players: Player[2] = Player(clip = source)

sample:
  out1 = players[0]() + players[1]()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("indexed proc events should forward bound buffers");
        lower_program_to_optimized_mir(&typed)
            .expect("indexed proc event buffer access should lower to MIR");
    }

    #[test]
    fn init_buffer_len_is_allowed_semantically() {
        let src = "buffers:\n  src: buffer<f32>\nouts:\n  out1\ninit:\n  n = src.len()\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("buffer len in init should analyze");
    }

    #[test]
    fn buffer_bound_is_available_in_init_and_on_selected_collection_entries() {
        let src = r#"
buffers:
  src: f32
  bank: f32 {2}
outs:
  out1
init:
  source_bound = src.bound()
  entry_bound = bank[1].bound()
sample:
  if source_bound || entry_bound:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("buffer bound queries should analyze");
    }

    #[test]
    fn init_buffer_index_is_allowed_semantically() {
        let src = "buffers:\n  src: buffer<f32>\nouts:\n  out1\ninit:\n  first = src[0]\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("buffer indexing in init should analyze");
    }

    #[test]
    fn buffer_collection_metadata_requires_a_selected_slot() {
        let src = r#"
buffers:
  bank: f32[] {2}

block:
  channels = bank.chans()
  rate = bank.samplerate()
  bound = bank.bound()
  sample:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("collection metadata should require a slot");

        for method in [".chans()", ".samplerate()", ".bound()"] {
            let diagnostic = errors
                .iter()
                .find(|diagnostic| diagnostic.message.contains(method))
                .unwrap_or_else(|| panic!("missing {method} diagnostic: {errors:?}"));
            assert!(diagnostic.message.contains("select a slot"));
            assert!(diagnostic.editor_visible);
        }
    }

    #[test]
    fn buffer_collection_argument_requires_a_selected_slot() {
        let src = r#"
buffers:
  bank: f32 {2}

def first(buf: buffer<f32>):
  return buf[0]

sample:
  out1 = first(bank)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("collection arguments should require a slot");
        let diagnostic = errors
            .iter()
            .find(|diagnostic| diagnostic.message.contains("select a slot"))
            .unwrap_or_else(|| panic!("missing collection argument diagnostic: {errors:?}"));
        assert!(diagnostic.message.contains("collection 'bank'"));
        assert!(diagnostic.editor_visible);
    }

    #[test]
    fn def_param_shadows_same_named_top_level_buffer_during_monomorphization() {
        let src = r#"
buffers:
  buf: buffer<f32>

outs:
  out1

def read_first(buf: buffer<f32>, index: i32):
  return buf[index]

sample:
  out1 = read_first(buf, 0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("def parameter should shadow the top-level buffer binding");
    }

    #[test]
    fn block_without_nested_sample_reports_only_block_specific_error() {
        let src = "outs { out1 }\nblock { x = 0.0 }\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block without nested sample should fail");

        let diagnostic = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("sample-rate outputs must include nested 'sample' block")
            })
            .expect("missing block-specific diagnostic");
        assert!(!diagnostic.editor_visible, "missing sample is compile-only");
        assert!(
            !errors
                .iter()
                .any(|diag| diag.message.contains("missing required 'sample' block")),
            "unexpected duplicate missing-sample diagnostic"
        );
    }

    #[test]
    fn def_returns_must_share_a_compatible_type() {
        let src = "outs:\n  out1\ndef test():\n  if true:\n    return 0\n  return (0, 1)\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mixed scalar/tuple returns should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("return in function 'test' type mismatch")
                    && diag.message.contains("cannot assign (i32, i32)")
            })
            .expect("missing incompatible return diagnostic");

        assert_eq!((diag.line, diag.column), (6, 10));
        assert_eq!(diag.end_line, 6);
    }

    #[test]
    fn value_returning_def_requires_both_if_branches_to_return() {
        let src = "outs:\n  out1\ndef choose(flag: bool) -> f32:\n  if flag:\n    return 1.0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("partial return should fail");
        let diagnostic = errors
            .iter()
            .find(|diag| {
                diag.message.contains("function 'choose' returns a value")
                    && diag.message.contains("not all reachable paths")
            })
            .expect("missing partial-return diagnostic");
        assert_eq!((diagnostic.line, diagnostic.column), (3, 1));
    }

    #[test]
    fn value_returning_def_accepts_complete_if_else() {
        let src = "outs:\n  out1\ndef choose(flag: bool) -> f32:\n  if flag:\n    return 1.0\n  else:\n    return 2.0\nsample:\n  out1 = choose(true)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("complete branch returns should analyze");
        let choose = typed
            .defs
            .iter()
            .find(|def| def.name == "choose")
            .expect("missing choose def");
        assert!(choose.returns_value);
    }

    #[test]
    fn loop_nested_return_is_conservatively_not_total() {
        let src = "outs:\n  out1\ndef first(n: i32) -> i32:\n  for i in 0..n:\n    return i\nsample:\n  out1 = f32(first(1))\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("loop-only return should fail");
        assert!(errors.iter().any(|diag| {
            diag.message.contains("function 'first' returns a value")
                && diag.message.contains("not all reachable paths")
        }));
    }

    #[test]
    fn nested_branch_return_does_not_cover_outer_fallthrough() {
        let src = "outs:\n  out1\ndef nested(a: bool, b: bool):\n  if a:\n    if b:\n      return 1.0\n    else:\n      return 2.0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("outer fallthrough should fail");
        assert!(errors.iter().any(|diag| {
            diag.message.contains("function 'nested' returns a value")
                && diag.message.contains("not all reachable paths")
        }));
    }

    #[test]
    fn return_after_conservative_loop_covers_fallthrough() {
        let src = "outs:\n  out1\ndef first_or_zero(n: i32) -> i32:\n  for i in 0..n:\n    if i > 0:\n      return i\n  return 0\nsample:\n  out1 = f32(first_or_zero(1))\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("post-loop return should cover loop fallthrough");
    }

    #[test]
    fn no_result_def_may_fall_through() {
        let src = "outs:\n  out1\ndef observe(flag: bool):\n  if flag:\n    value = 1.0\n  while flag:\n    break\nsample:\n  observe(false)\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("no-result def should be allowed to fall through");
        let observe = typed
            .defs
            .iter()
            .find(|def| def.name == "observe")
            .expect("missing observe def");
        assert!(!observe.returns_value);
    }

    #[test]
    fn no_result_def_accepts_bare_early_return() {
        let src = "outs:\n  out1\ndef observe(flag: bool):\n  if flag:\n    return\n  value = 1.0\nsample:\n  observe(false)\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("bare return should analyze");
        let observe = typed
            .defs
            .iter()
            .find(|def| def.name == "observe")
            .expect("missing observe def");
        assert!(!observe.returns_value);
    }

    #[test]
    fn rejects_mixed_bare_and_value_returns() {
        let src = "outs:\n  out1\ndef choose(flag: bool):\n  if flag:\n    return\n  return 1.0\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mixed returns should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("cannot mix bare returns with value returns")));
    }

    #[test]
    fn rejects_bare_return_with_explicit_return_type() {
        let src = "outs:\n  out1\ndef choose() -> f32:\n  return\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("bare typed return should fail");
        assert!(errors.iter().any(|diag| diag
            .message
            .contains("cannot mix bare returns with value returns or an explicit return type")));
    }

    #[test]
    fn proc_local_value_helper_requires_total_return() {
        let src = "proc Voice:\n  outs:\n    out1\n  def choose(flag: bool) -> f32:\n    if flag:\n      return 1.0\n  sample:\n    out1 = 0.0\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("partial proc-local helper should fail");
        assert!(errors.iter().any(|diag| {
            diag.message.contains("Voice")
                && diag.message.contains("returns a value")
                && diag.message.contains("not all reachable paths")
        }));
    }

    #[test]
    fn explicit_def_return_type_allows_implicit_widening() {
        let src = "outs:\n  out1\ndef widen(x: i32) -> i64:\n  return x\nsample:\n  out1 = f32(widen(1))\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("explicit widening return annotation should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "widen")
            .expect("missing typed def");
        assert_eq!(def.return_ty, ReturnType::Scalar(PrimitiveType::I64));
    }

    #[test]
    fn explicit_def_return_type_rejects_implicit_narrowing() {
        let src =
            "outs:\n  out1\ndef narrow() -> i32:\n  return 3.5\nsample:\n  out1 = f32(narrow())\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("implicit narrowing return should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("return in function 'narrow'")
                    && diag.message.contains("cannot assign F32 to I32")
            }),
            "expected return mismatch diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn typed_scalar_call_argument_rejects_implicit_narrowing() {
        let src = r#"
outs:
  out1

def take(x: f32) -> f32:
  return x

sample:
  wide: f64 = 1.25
  out1 = take(wide)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("typed f64 call argument must not narrow to f32");
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic.message.contains("function 'take'")
                    && diagnostic.message.contains("cannot assign F64 to F32")
            }),
            "expected call-argument narrowing diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn typed_scalar_call_argument_accepts_contextual_numeric_literal() {
        let src = r#"
outs:
  out1

def take(x: f32) -> f32:
  return x

sample:
  out1 = take(1.25)
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("a numeric literal should adopt the f32 parameter context");
    }

    #[test]
    fn generic_calls_use_argument_types_consistently_in_every_executable_owner() {
        let cases = [
            (
                "sample",
                r#"
def float_only<T>(x: T):
  return exp(x)
params:
  value = 1 {0, 10}
sample:
  out1 = float_only(value)
"#,
            ),
            (
                "block",
                r#"
def float_only<T>(x: T):
  return exp(x)
params:
  value = 1 {0, 10}
block:
  held = float_only(value)
  sample:
    out1 = held
"#,
            ),
            (
                "event",
                r#"
def float_only<T>(x: T):
  return exp(x)
init:
  held = 0.0
events:
  set(value: i32):
    held = float_only(value)
sample:
  out1 = held
"#,
            ),
            (
                "def",
                r#"
def float_only<T>(x: T):
  return exp(x)
def caller(value: i32):
  return float_only(value)
sample:
  out1 = caller(1)
"#,
            ),
            (
                "proc",
                r#"
def float_only<T>(x: T):
  return exp(x)
proc Voice:
  params:
    value: i32 = 1
  sample:
    out1 = float_only(value)
init:
  voice = Voice()
sample:
  out1 = voice()
"#,
            ),
        ];

        for (owner, source) in cases {
            let program = parse_program(source)
                .unwrap_or_else(|error| panic!("{owner} source should parse: {error:?}"));
            let errors = match analyze(program) {
                Err(errors) => errors,
                Ok(_) => panic!("{owner} must specialize float_only as i32"),
            };
            assert!(
                errors.iter().any(|diagnostic| {
                    diagnostic.message.contains("float_only")
                        && diagnostic.message.contains("requires float arguments")
                        && diagnostic.message.contains("I32")
                }),
                "{owner} used a different generic specialization rule: {errors:?}"
            );
        }
    }

    #[test]
    fn call_inference_uses_runtime_numeric_merge_rules() {
        let source = r#"
def identity(value):
  return value

params:
  narrow: f32 = 1.0
  wide_integer: i64 = 2

sample:
  from_binary = identity(narrow + wide_integer)
  from_builtin = identity(max(narrow, wide_integer))
  out1 = from_binary + from_builtin
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("call inference must agree with runtime expression typing");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "identity.__onda_mono__scalar_f32"));
        assert!(!typed
            .defs
            .iter()
            .any(|function| function.name == "identity.__onda_mono__scalar_f64"));
        lower_program_to_optimized_mir(&typed)
            .expect("consistently inferred numeric expressions should lower to MIR");
    }

    #[test]
    fn preexisting_scalar_type_controls_specialization_across_branches() {
        let source = r#"
def identity(value):
  return value

params:
  select: bool = true

sample:
  chosen = f64(0)
  if select:
    chosen = f32(1)
  else:
    chosen = i64(2)
  out1 = f32(identity(chosen))
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("branch assignments should retain the established scalar type");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "identity.__onda_mono__scalar_f64"));
        lower_program_to_optimized_mir(&typed)
            .expect("joined branch scalar types should lower to MIR");
    }

    #[test]
    fn branch_local_numeric_types_join_before_specialization_and_lowering() {
        let source = r#"
def identity(value):
  return value

def tuple_id(value):
  return value

def pick(flag: bool):
  if flag:
    value = f32(1)
  else:
    value = i64(2)
  return value

params:
  select: bool = true

sample:
  if select:
    chosen = i64(1)
    pair = (i32(2), f32(3))
  else:
    chosen = i32(4)
    pair = (i64(5), i32(6))
  joined_pair = tuple_id(pair)
  out1 = f32(identity(chosen) + joined_pair[0] + pick(select)) + joined_pair[1]
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("branch-local numeric values should have deterministic common types");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "identity.__onda_mono__scalar_i64"));
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "tuple_id.__onda_mono__tup_i64_f32"));
        assert!(typed.defs.iter().any(|function| function.name == "pick"
            && function.return_ty == ReturnType::Scalar(PrimitiveType::F64)));
        lower_program_to_optimized_mir(&typed)
            .expect("branch-local numeric joins should be represented in MIR");
    }

    #[test]
    fn incompatible_branch_local_scalar_types_are_semantic_errors() {
        let source = r#"
params:
  select: bool = true

sample:
  if select:
    chosen = true
  else:
    chosen = 1
  out1 = f32(chosen)
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("a branch-local value needs one representable type");
        assert!(errors.iter().any(|diagnostic| diagnostic
            .message
            .contains("binding 'chosen' has incompatible branch types: bool and i32")));
    }

    #[test]
    fn incompatible_branch_local_integer_ranges_are_semantic_errors() {
        for (case, then_range, else_range, expected) in [
            (
                "bounds",
                "{0..10}",
                "{0..100}",
                "clamp i32(0..=9) and clamp i32(0..=99)",
            ),
            (
                "mode",
                "{0..10}",
                "{0..10, wrap}",
                "clamp i32(0..=9) and wrap i32(0..=9)",
            ),
            ("presence", "{0..10}", "", "clamp i32(0..=9) and unbounded"),
        ] {
            let source = format!(
                r#"
params:
  select: bool = true

sample:
  if select:
    chosen: i32 = 5 {then_range}
  else:
    chosen: i32 = 6 {else_range}
  out1 = f32(chosen)
"#
            );
            let errors = analyze(parse_program(&source).expect("source should parse"))
                .expect_err("branch range mismatch should be rejected");
            assert!(
                errors
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(&format!(
                    "binding 'chosen' has incompatible branch integer range contracts: {expected}"
                ))),
                "missing {case} range mismatch diagnostic: {errors:?}"
            );
        }
    }

    #[test]
    fn identical_branch_local_integer_ranges_preserve_the_storage_contract() {
        let source = r#"
params:
  select: bool = true

sample:
  if select:
    chosen: i32 = 5 {0..10, wrap}
  else:
    chosen: i32 = 6 {0..10, wrap}
  chosen += 10
  out1 = f32(chosen)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("identical branch range contracts should merge");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("a compatible ranged branch binding should lower to MIR");
        let expected = onda_mir::IntegerRangeInvariant {
            min: onda_mir::ScalarValue::I32(0),
            max: onda_mir::ScalarValue::I32(9),
            mode: onda_mir::IntegerRangeMode::Wrap,
        };
        let process = &mir.functions[mir.entry_points.process.index()];
        let chosen = process
            .locals
            .iter()
            .filter(|local| local.name.as_deref() == Some("chosen"))
            .collect::<Vec<_>>();
        assert!(!chosen.is_empty());
        assert!(chosen
            .iter()
            .all(|local| local.integer_range == Some(expected)));
    }

    #[test]
    fn first_assignment_defaults_drive_call_specialization() {
        let source = r#"
def identity(value):
  return value

def first(values: []):
  return values[0]

def local_first():
  values = [PI]
  return first(values)

init:
  state_values = [PI]

sample:
  scalar = PI
  values = [PI]
  out1 = identity(scalar) + first(values) + first(state_values) + local_first()
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("call inference should use the types assigned to untyped locals");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "identity.__onda_mono__scalar_f32"));
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "first.__onda_mono__arr_f32"));
        lower_program_to_optimized_mir(&typed)
            .expect("defaulted scalar and array locals should lower consistently");
    }

    #[test]
    fn generic_constraints_contextualize_pure_numeric_arguments() {
        let source = r#"
def identity<T>(value: T) -> T:
  return value

def choose<T>(left: T, right: T) -> T:
  return left + right

def first<T>(values: T[]) -> T:
  return values[0]

params:
  narrow: f32 = 1.0

sample:
  out1 = identity(PI) + choose(PI, narrow) + first([PI]) + f32(identity(2147483648 + 0))
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("pure numeric arguments should adopt an available call-site context");
        for specialization in [
            "identity.__onda_mono__g_f32",
            "identity.__onda_mono__g_i64",
            "choose.__onda_mono__g_f32__g_f32",
            "first.__onda_mono__arr_f32",
        ] {
            assert!(
                typed
                    .defs
                    .iter()
                    .any(|function| function.name == specialization),
                "missing f32 specialization '{specialization}': {:?}",
                typed
                    .defs
                    .iter()
                    .map(|function| function.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
        lower_program_to_optimized_mir(&typed)
            .expect("contextual generic constraints should lower consistently");
    }

    #[test]
    fn explicit_cast_selects_float_generic_specialization_in_every_executable_owner() {
        let cases = [
            r#"
def float_only<T>(x: T):
  return exp(x)
params:
  value = 1 {0, 10}
sample:
  out1 = float_only(f32(value))
"#,
            r#"
def float_only<T>(x: T):
  return exp(x)
params:
  value = 1 {0, 10}
block:
  held = float_only(f32(value))
  sample:
    out1 = held
"#,
            r#"
def float_only<T>(x: T):
  return exp(x)
init:
  held = 0.0
events:
  set(value: i32):
    held = float_only(f32(value))
sample:
  out1 = held
"#,
            r#"
def float_only<T>(x: T):
  return exp(x)
def caller(value: i32):
  return float_only(f32(value))
sample:
  out1 = caller(1)
"#,
            r#"
def float_only<T>(x: T):
  return exp(x)
proc Voice:
  params:
    value: i32 = 1
  sample:
    out1 = float_only(f32(value))
init:
  voice = Voice()
sample:
  out1 = voice()
"#,
        ];

        for source in cases {
            let program = parse_program(source).expect("explicit-cast source should parse");
            analyze(program).expect("an explicit cast should select the f32 specialization");
        }
    }

    #[test]
    fn overload_resolution_uses_the_same_call_type_environment() {
        let cases = [
            (
                "compiler-generated parameter alias",
                r#"
def classify(x: i32) -> f32:
  return 1.0
def classify(x: f32) -> f32:
  return 2.0
params:
  value = 1 {0, 10}
sample:
  out1 = classify(value)
"#,
            ),
            (
                "event parameter",
                r#"
def classify(x: i32) -> f32:
  return 1.0
def classify(x: f32) -> f32:
  return 2.0
init:
  held = 0.0
events:
  set(value: i32):
    held = classify(value)
sample:
  out1 = held
"#,
            ),
            (
                "loop index",
                r#"
def classify(x: i32) -> f32:
  return 1.0
def classify(x: f32) -> f32:
  return 2.0
sample:
  total = 0.0
  for i in 0..2:
    total = total + classify(i)
  out1 = total
"#,
            ),
        ];

        for (binding, source) in cases {
            let program = parse_program(source)
                .unwrap_or_else(|error| panic!("{binding} source should parse: {error:?}"));
            analyze(program).unwrap_or_else(|errors| {
                panic!("{binding} should select the i32 overload: {errors:?}")
            });
        }
    }

    #[test]
    fn processor_validation_uses_resolved_overloads() {
        let source = r#"
def classify(value: bool) -> f32:
  return 1.0

def classify(value: i32) -> f32:
  return 2.0

def fetch(buf, frame: i32):
  return buf[frame]

def fetch(buf, channel: i32, frame: i32):
  return buf[channel, frame]

struct Classifier:
  def value(self, input: bool) -> f32:
    return 3.0

  def value(self, input: i32) -> f32:
    return 4.0

proc Player:
  buffers:
    clip: f32[]

  params:
    mode: i32 = 0

  init:
    classifier = Classifier()

  outs 1

  sample:
    out1 = classify(mode) + classifier.value(mode) + fetch(clip, 0, 0)

buffers:
  source: f32[]

init:
  player = Player(clip = source)

sample:
  out1 = player()
"#;
        let program = parse_program(source).expect("processor overload source should parse");
        let typed = analyze(program)
            .expect("processor validation should use the overload selected by type and arity");
        lower_program_to_optimized_mir(&typed)
            .expect("resolved processor overload calls should lower to MIR");
    }

    #[test]
    fn overload_resolution_applies_contextual_aggregate_conversions() {
        let source = r#"
def array_choice(values: f64[]) -> f64:
  return values[0]

def array_choice(values: bool[]) -> f64:
  return 0.0

def tuple_choice(values: (f64, i32)) -> f64:
  return values[0]

def tuple_choice(values: (bool, bool)) -> f64:
  return 0.0

sample:
  values = (1.0, 2)
  from_array = array_choice([1.0])
  from_literal_tuple = tuple_choice((1.0, 2))
  from_tuple_value = tuple_choice(values)
  out1 = f32(from_array + from_literal_tuple + from_tuple_value)
"#;
        let program = parse_program(source).expect("aggregate overload source should parse");
        let typed = analyze(program)
            .expect("contextually assignable aggregates should select the numeric overloads");
        lower_program_to_optimized_mir(&typed)
            .expect("contextual aggregate conversions should lower to MIR");
    }

    #[test]
    fn overload_resolution_applies_contextual_constant_conversions() {
        let source = r#"
def choose(value: f32) -> f32:
  return value

def choose(value: bool) -> f32:
  return 0.0

def tuple_id(value):
  return value

sample:
  tuple_value = tuple_id((PI, 1))
  out1 = choose(PI) + tuple_value[0]
"#;
        let program = parse_program(source).expect("constant overload source should parse");
        let typed = analyze(program)
            .expect("a pure numeric constant should select its assignable f32 overload");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "tuple_id.__onda_mono__tup_f32_i32"));
        lower_program_to_optimized_mir(&typed)
            .expect("contextual constant conversion should lower to MIR");
    }

    #[test]
    fn tuple_call_arguments_reject_implicit_narrowing() {
        let cases = [
            r#"
def choose(values: (f32, i32)):
  return values[0]

sample:
  values = (f64(1.0), 2)
  out1 = choose(values)
"#,
            r#"
def make() -> (f64, i32):
  return (f64(1.0), 2)

def choose(values: (f32, i32)):
  return values[0]

sample:
  out1 = choose(make())
"#,
        ];

        for source in cases {
            let program = parse_program(source).expect("tuple narrowing source should parse");
            let errors = analyze(program).expect_err("tuple arguments must not narrow implicitly");
            assert!(
                errors.iter().any(|diagnostic| {
                    diagnostic.message.contains("tuple element 0 type mismatch")
                        && diagnostic.message.contains("f64")
                        && diagnostic.message.contains("f32")
                }),
                "missing tuple narrowing diagnostic: {errors:?}"
            );
        }

        let default_source = r#"
def choose(values: (f32, i32) = (f64(1.0), 2)):
  return values[0]

sample:
  out1 = choose()
"#;
        let program = parse_program(default_source).expect("tuple default source should parse");
        let errors = analyze(program).expect_err("tuple defaults must not narrow implicitly");
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("function 'choose' argument 'values'")
                    && diagnostic.message.contains("cannot assign F64 to F32")
            }),
            "missing tuple default narrowing diagnostic: {errors:?}"
        );

        let scalar_source = r#"
def make() -> f32:
  return 1.0

def choose(values: (f32, i32)):
  return values[0]

sample:
  out1 = choose(make())
"#;
        let program =
            parse_program(scalar_source).expect("scalar tuple argument source should parse");
        let errors = analyze(program).expect_err("a scalar return is not a tuple argument");
        assert!(
            errors.iter().any(|diagnostic| diagnostic
                .message
                .contains("parameter 'values' expects a tuple value")),
            "missing scalar-to-tuple diagnostic: {errors:?}"
        );

        let tuple_source = r#"
def make() -> (f32, i32):
  return (1.0, 2)

def choose(value: f32):
  return value

sample:
  out1 = choose(make())
"#;
        let program =
            parse_program(tuple_source).expect("tuple scalar argument source should parse");
        let errors = analyze(program).expect_err("a tuple return is not a scalar argument");
        assert!(
            errors.iter().any(|diagnostic| diagnostic
                .message
                .contains("parameter 'value' expects a scalar value")),
            "missing tuple-to-scalar diagnostic: {errors:?}"
        );

        let shape_cases = [
            (
                r#"
def choose(value: (f32, i32)):
  return value[0]

sample:
  values = [1.0, 2.0]
  out1 = choose(values)
"#,
                "parameter 'value' expects a tuple value",
            ),
            (
                r#"
def choose(value: f32[]):
  return value[0]

sample:
  values = (1.0, 2)
  out1 = choose(values)
"#,
                "parameter 'value' expects an array value",
            ),
            (
                r#"
def choose(value: f32):
  return value

sample:
  values = [1.0, 2.0]
  out1 = choose(values)
"#,
                "parameter 'value' expects a scalar value",
            ),
        ];
        for (source, expected) in shape_cases {
            let program = parse_program(source).expect("aggregate shape source should parse");
            let errors = analyze(program).expect_err("aggregate argument shape must match");
            assert!(
                errors
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(expected)),
                "missing aggregate shape diagnostic '{expected}': {errors:?}"
            );
        }
    }

    #[test]
    fn overload_resolution_uses_builtin_result_types() {
        let source = r#"
def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

params:
  value = 1 {0, 10}

sample:
  selected: i32 = classify(abs(value))
  out1 = f32(selected)
"#;
        let program = parse_program(source).expect("builtin overload source should parse");
        analyze(program).expect("abs(i32) should select the i32 overload");
    }

    #[test]
    fn overload_resolution_uses_user_call_return_types() {
        let source = r#"
def make() -> i32:
  return 1

def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

sample:
  selected: i32 = classify(make())
  out1 = f32(selected)
"#;
        let program = parse_program(source).expect("nested call overload source should parse");
        analyze(program).expect("an i32-returning call should select the i32 overload");
    }

    #[test]
    fn user_methods_named_like_resource_builtins_keep_their_declared_return_types() {
        let source = r#"
struct Ops:
  def len(self) -> f64:
    return f64(1)

  def chans(self) -> f64:
    return f64(2)

  def samplerate(self) -> i64:
    return i64(3)

def identity<T>(value: T) -> T:
  return value

def classify(value: i32) -> f64:
  return f64(value)

def classify(value: f64) -> f64:
  return value

init:
  ops = Ops()

sample:
  selected: f64 = classify(ops.len())
  out1 = f32(selected + identity(ops.chans()) + f64(identity(ops.samplerate())))
"#;
        let program = parse_program(source).expect("resource-named method source should parse");
        let typed = analyze(program)
            .expect("user methods must take precedence over builtin instance method spellings");
        assert!(typed.defs.iter().any(|function| {
            function.name == "identity.__onda_mono__g_f64"
                && function.return_ty == ReturnType::Scalar(PrimitiveType::F64)
        }));
        assert!(typed.defs.iter().any(|function| {
            function.name == "identity.__onda_mono__g_i64"
                && function.return_ty == ReturnType::Scalar(PrimitiveType::I64)
        }));
        lower_program_to_optimized_mir(&typed)
            .expect("resource-named method calls should lower with their declared types");
    }

    #[test]
    fn method_self_fields_drive_generic_specialization() {
        let source = r#"
def float_only<T>(x: T):
  return exp(x)

struct Counter<T>:
  value: T

  def read(self):
    return float_only(self.value)

init:
  counter = Counter<i32>(1)

sample:
  out1 = counter.read()
"#;
        let program = parse_program(source).expect("method specialization source should parse");
        let errors = analyze(program).expect_err("self.value must specialize as i32");
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic.message.contains("float_only")
                    && diagnostic.message.contains("requires float arguments")
                    && diagnostic.message.contains("I32")
            }),
            "method self field selected the wrong specialization: {errors:?}"
        );
    }

    #[test]
    fn concrete_method_self_publishes_return_types_for_nested_specialization() {
        let source = r#"
struct Cell<T>:
  value: T

  def read(self):
    return self.value

  def set(self, value):
    self.value = value

  def copy_value(self):
    self.set(self.read())

init:
  cell = Cell<f64>(f64(1))
  cell.copy_value()

sample:
  out1 = f32(cell.read())
"#;
        let program = parse_program(source).expect("nested method source should parse");
        let typed = analyze(program).expect("the concrete self type must publish read() as f64");
        assert!(typed.defs.iter().any(|function| {
            function
                .name
                .contains("Cell.__gen__f64.set.__onda_mono__pass__scalar_f64")
        }));
        lower_program_to_optimized_mir(&typed)
            .expect("the nested f64 method specialization should lower to MIR");
    }

    #[test]
    fn synthetic_param_surface_preserves_its_element_type_for_specialization() {
        let source = r#"
def float_only<T>(x: T):
  return exp(x)

params<i32> 2

sample:
  out1 = float_only(params[0])
"#;
        let program = parse_program(source).expect("indexed param source should parse");
        let errors = analyze(program).expect_err("params[i] must specialize as i32");
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic.message.contains("float_only")
                    && diagnostic.message.contains("requires float arguments")
                    && diagnostic.message.contains("I32")
            }),
            "synthetic params surface selected the wrong specialization: {errors:?}"
        );
    }

    #[test]
    fn overloads_in_generic_templates_resolve_after_specialization() {
        let source = r#"
def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

def relay<T>(x: T) -> T:
  return classify(x)

sample:
  out1 = f32(relay(1))
"#;
        let program = parse_program(source).expect("generic overload source should parse");
        let typed = analyze(program).expect("the generated relay must replay overload resolution");
        assert!(typed.defs.iter().any(|function| {
            function.name.contains("relay.__onda_mono__g_i32")
                && function.return_ty == ReturnType::Scalar(PrimitiveType::I32)
        }));
    }

    #[test]
    fn dependent_tuple_elements_defer_until_specialization() {
        let source = r#"
def integer_first(values):
  return ~values[0]

def relay<T>(x: T):
  return integer_first((x, x))

sample:
  out1 = f32(relay(1))
"#;
        let program = parse_program(source).expect("dependent tuple source should parse");
        let typed = analyze(program).expect("dependent tuple elements should resolve as i32");
        assert!(typed.defs.iter().any(|function| function
            .name
            .contains("integer_first.__onda_mono__tup_i32_i32")));
    }

    #[test]
    fn slice_aliases_preserve_element_types_for_specialization() {
        let source = r#"
const Values: i32[1] = [1]

def integer_first(values: []):
  return ~values[0]

sample:
  alias = Values[0:1]
  out1 = f32(integer_first(alias))
"#;
        let program = parse_program(source).expect("slice alias source should parse");
        let typed = analyze(program).expect("a slice alias should preserve its i32 elements");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_first.__onda_mono__arr_i32")));
    }

    #[test]
    fn branch_call_types_use_the_runtime_numeric_join() {
        let source = r#"
def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

sample:
  if true:
    value = 1
  else:
    value = 1.0
  out1 = f32(classify(value))
"#;
        let program = parse_program(source).expect("branch inference source should parse");
        let typed =
            analyze(program).expect("numeric branches should select one predictable common type");
        assert!(typed.defs.iter().any(|function| matches!(
            function.param_kinds.as_slice(),
            [TypedFnParam::Scalar {
                ty: Some(PrimitiveType::F32)
            }]
        )));
        lower_program_to_optimized_mir(&typed)
            .expect("the overload-selected branch join should lower to MIR");
    }

    #[test]
    fn loop_index_fully_shadows_same_named_aggregate_root() {
        let source = r#"
ins:
  i: f32[2] = [0.0, 0.0]

sample:
  for i in 0..2:
    out1 = i[0]
"#;
        let program = parse_program(source).expect("loop root shadowing source should parse");
        let errors = analyze(program).expect_err("a scalar loop index cannot retain array shape");
        assert!(
            errors.iter().any(|diagnostic| diagnostic
                .message
                .contains("loop variable 'i' is scalar and cannot be indexed")),
            "missing lexical-root shadowing diagnostic: {errors:?}"
        );
    }

    #[test]
    fn loop_index_fully_shadows_same_named_assignment_root() {
        let source = r#"
init:
  i: f32[2]
  for i in 0..2:
    i[0] = 1.0

sample:
  out1 = 0.0
"#;
        let program = parse_program(source).expect("loop target shadowing source should parse");
        let errors = analyze(program).expect_err("a scalar loop index cannot mutate an array");
        assert!(
            errors.iter().any(|diagnostic| diagnostic
                .message
                .contains("loop variable 'i' is scalar and cannot be indexed")),
            "missing assignment-root shadowing diagnostic: {errors:?}"
        );
    }

    #[test]
    fn buffer_alias_preserves_shape_for_monomorphization() {
        let source = r#"
def read_first(buf: buffer):
  return buf[0]
buffers:
  bank: f64 {2}
sample:
  source = bank[0]
  out1 = f32(read_first(source))
"#;
        let program = parse_program(source).expect("buffer alias source should parse");
        let typed = analyze(program).expect("buffer alias type should remain f64");
        let specialization = typed
            .defs
            .iter()
            .find(|function| function.name.contains("read_first.__onda_mono__buf_f64"))
            .expect("missing f64 buffer specialization");
        assert!(matches!(
            specialization.param_kinds.as_slice(),
            [TypedFnParam::Buffer {
                elem_ty: PrimitiveType::F64,
                ..
            }]
        ));
        assert_eq!(
            specialization.return_ty,
            ReturnType::Scalar(PrimitiveType::F64)
        );
    }

    #[test]
    fn reassignment_preserves_the_binding_type_for_call_inference() {
        let source = r#"
def float_only<T>(x: T):
  return exp(x)

init:
  held = 0.0

block:
  held = 1

sample:
  out1 = float_only(held)
"#;
        let program = parse_program(source).expect("reassignment source should parse");
        let typed = analyze(program).expect("held remains f32 after assigning an integer literal");
        assert!(
            typed
                .defs
                .iter()
                .any(|function| function.name.contains("float_only.__onda_mono__g_f32")),
            "the call must use the target binding's f32 type"
        );
    }

    #[test]
    fn reassignment_preserves_the_binding_type_for_overload_resolution() {
        let source = r#"
def classify(x: i32) -> f32:
  return 1.0

def classify(x: f32) -> f32:
  return 2.0

init:
  held = 0.0

block:
  held = 1

sample:
  out1 = classify(held)
"#;
        let program = parse_program(source).expect("overload reassignment source should parse");
        analyze(program).expect("held should continue selecting the f32 overload");
    }

    #[test]
    fn struct_field_reassignment_preserves_declared_call_type() {
        let source = r#"
def identity<T>(x: T) -> T:
  return x

struct Holder:
  value: f64

init:
  holder = Holder(f64(0))
  holder.value = 1.0

sample:
  out1 = f32(identity(holder.value))
"#;
        let program = parse_program(source).expect("field reassignment source should parse");
        let typed = analyze(program).expect("holder.value must retain its declared f64 type");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("identity.__onda_mono__g_f64")));
        lower_program_to_optimized_mir(&typed)
            .expect("the f64 field specialization should lower to MIR");
    }

    #[test]
    fn concrete_tuple_parameters_seed_nested_call_inference() {
        let source = r#"
def integer_only<T>(x: T) -> T:
  return ~x

def relay(values: (i32, f32)):
  return integer_only(values[0])

sample:
  out1 = f32(relay((1, 2.0)))
"#;
        let program = parse_program(source).expect("tuple parameter source should parse");
        let typed = analyze(program).expect("values[0] must specialize integer_only as i32");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_only.__onda_mono__g_i32")));
        lower_program_to_optimized_mir(&typed)
            .expect("the tuple-driven specialization should lower to MIR");
    }

    #[test]
    fn tuple_struct_field_aliases_and_destructuring_preserve_call_types() {
        let source = r#"
struct Holder:
  values: (i32, f32) = (1, 2.0)

def integer_only<T>(x: T) -> T:
  return ~x

def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

init:
  holder = Holder()

sample:
  holder.values = (2, 3.0)
  alias = holder.values
  (first, second) = holder.values
  selected: i32 = classify(alias[0])
  out1 = f32(integer_only(alias[0]) + integer_only(first) + selected) + second
"#;
        let program = parse_program(source).expect("tuple field alias source should parse");
        let typed = analyze(program)
            .expect("tuple field aliases and destructuring must retain their element types");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_only.__onda_mono__g_i32")));
        lower_program_to_optimized_mir(&typed)
            .expect("tuple field aliases should lower with concrete call types");
    }

    #[test]
    fn def_tuple_aliases_preserve_parameter_and_return_element_types() {
        let source = r#"
def integer_only<T>(x: T) -> T:
  return ~x

def make_pair() -> (i32, f32):
  return (1, 2.0)

def relay(values: (i32, f32)) -> i32:
  alias = values
  (first, second) = alias
  return integer_only(alias[0]) + integer_only(first) + i32(second)

sample:
  pair = make_pair()
  out1 = f32(relay(pair) + integer_only(pair[0]))
"#;
        let program = parse_program(source).expect("def tuple alias source should parse");
        let typed = analyze(program)
            .expect("tuple aliases must preserve element types in defs and executable scopes");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_only.__onda_mono__g_i32")));
        lower_program_to_optimized_mir(&typed)
            .expect("tuple parameter and return aliases should lower to MIR");
    }

    #[test]
    fn inferred_returns_use_contextual_literal_types() {
        let source = r#"
def choose(x: f32) -> f32:
  return x

def choose(x: f64) -> f64:
  return x

def computed(x: f32):
  return x + 2147483648

sample:
  out1 = choose(computed(1.0))
"#;
        let program = parse_program(source).expect("contextual return source should parse");
        let typed = analyze(program)
            .expect("return inference and overload resolution must agree on f32 context");
        assert!(typed.defs.iter().any(|function| {
            function.name == "computed"
                && function.return_ty == ReturnType::Scalar(PrimitiveType::F32)
        }));
        lower_program_to_optimized_mir(&typed)
            .expect("contextually typed inferred returns should lower to MIR");
    }

    #[test]
    fn inferred_scalar_and_tuple_returns_share_literal_defaulting_rules() {
        let source = r#"
def scalar():
  return PI

def tuple():
  return (PI, 1)

sample:
  values = tuple()
  out1 = scalar() + values[0] + f32(values[1])
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("scalar and tuple return inference should use the same defaults");
        assert!(typed.defs.iter().any(|function| {
            function.name == "scalar"
                && function.return_ty == ReturnType::Scalar(PrimitiveType::F32)
        }));
        assert!(typed.defs.iter().any(|function| {
            function.name == "tuple"
                && function.return_ty
                    == ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
        }));
        lower_program_to_optimized_mir(&typed)
            .expect("consistently defaulted inferred returns should lower to MIR");
    }

    #[test]
    fn tuple_destructuring_publishes_inferred_return_types() {
        let source = r#"
def integer_only<T>(x: T) -> T:
  return ~x

def pair() -> (i32, f32):
  return (1, 2.0)

def first():
  (value, ignored) = pair()
  return value

sample:
  out1 = f32(integer_only(first()))
"#;
        let program = parse_program(source).expect("destructured return source should parse");
        let typed =
            analyze(program).expect("destructured tuple elements must feed nested specialization");
        assert!(typed.defs.iter().any(|function| {
            function.name == "first" && function.return_ty == ReturnType::Scalar(PrimitiveType::I32)
        }));
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_only.__onda_mono__g_i32")));
        lower_program_to_optimized_mir(&typed)
            .expect("destructured inferred returns should lower to MIR");
    }

    #[test]
    fn bare_tuple_destructuring_discards_unneeded_values() {
        let source = r#"
def triple() -> (i32, f32, bool):
  return (7, 2.5, true)

def select() -> f32:
  first, _, _ = triple()
  _, second, _ = triple()
  return f32(first) + second

sample:
  out1 = select()
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("discarded tuple entries must not introduce bindings");
        lower_program_to_optimized_mir(&typed)
            .expect("tuple discards should lower without storage");
    }

    #[test]
    fn typed_tuple_assignments_work_in_init_defs_tasks_and_executable_blocks() {
        let source = r#"
def pair() -> (f32, i32):
  return (1.0, 2)

def consume() -> f32:
  local: (f64, i64) = pair()
  return f32(local[0]) + f32(local[1])

init:
  state: (f64, i64) = pair()
  state = pair()

event reset():
  state = pair()

task worker():
  local: (f64, i64) = pair()
  yield

block:
  before: (f64, i64) = pair()
  sample:
    local: (f64, i64) = pair()
    out1 = f32(local[0]) + f32(local[1]) + f32(state[0]) + consume()
  after: (f64, i64) = pair()
"#;
        let typed = analyze(parse_program(source).expect("typed tuple source should parse"))
            .expect("typed tuple declarations should analyze in every supported assignment owner");
        assert_eq!(
            typed.state_tuples.get("state"),
            Some(&vec![PrimitiveType::F64, PrimitiveType::I64])
        );
        assert_eq!(
            typed.state_tuples.get("before"),
            Some(&vec![PrimitiveType::F64, PrimitiveType::I64])
        );
        lower_program_to_optimized_mir(&typed)
            .expect("typed tuple declarations should lower with their declared types");
    }

    #[test]
    fn nested_init_tuples_remain_local_while_owner_tuples_become_state() {
        let source = r#"
init:
  state = (1.0, 2)
  if true:
    local = (3.0, 4)
    local = (5.0, 6)
    state = local

sample:
  out1 = state[0] + f32(state[1])
"#;
        let typed = analyze(parse_program(source).expect("tuple scope source should parse"))
            .expect("init tuple scopes should analyze");
        assert!(typed.state_tuples.contains_key("state"));
        assert!(!typed.state_tuples.contains_key("local"));
        assert!(!typed
            .state_vars
            .iter()
            .any(|name| name.starts_with("local.__")));
        lower_program_to_optimized_mir(&typed)
            .expect("nested init tuple locals should lower without persistent state");
    }

    #[test]
    fn typed_tuple_assignments_work_in_processor_owners() {
        let source = r#"
def pair() -> (f32, i32):
  return (1.0, 2)

proc Voice:
  init:
    state: (f64, i64) = pair()
    state = pair()

  event reset():
    state = pair()

  task worker():
    local: (f64, i64) = pair()
    yield

  block:
    before: (f64, i64) = pair()
    sample:
      local: (f64, i64) = pair()
      out1 = f32(local[0]) + f32(local[1]) + f32(state[0])
    after: (f64, i64) = pair()

proc Wrapper:
  init:
    voice = Voice()
  sample:
    out1 = voice()

init:
  wrapper = Wrapper()

sample:
  out1 = wrapper()
"#;
        let typed = analyze(parse_program(source).expect("processor tuple source should parse"))
            .expect("typed tuple declarations should analyze in processor owners");
        lower_program_to_optimized_mir(&typed)
            .expect("processor typed tuple declarations should lower to MIR");
    }

    #[test]
    fn typed_tuple_assignments_validate_shape_and_element_types() {
        for (assignment, expected) in [
            ("value: (f32, i32) = (true, 2)", "element 0 type mismatch"),
            ("value: (f32, i32) = (1.0, 2, 3)", "has arity 3, expected 2"),
            ("value: (f32, i32) = 1.0", "requires a tuple value"),
        ] {
            let source = format!("sample:\n  {assignment}\n  out1 = 0.0\n");
            let errors =
                analyze(parse_program(&source).expect("invalid tuple source should parse"))
                    .expect_err("invalid typed tuple assignment should fail semantic analysis");
            assert!(
                errors.iter().any(|error| error.message.contains(expected)),
                "expected '{expected}' for '{assignment}', got {errors:?}"
            );
        }
    }

    #[test]
    fn struct_aggregate_fields_publish_inferred_return_types() {
        let source = r#"
struct Values:
  samples: i32[2]
  count: i32

struct Voice:
  values: Values

def integer_only<T>(x: T) -> T:
  return ~x

def read_array(holder):
  return holder.samples[0]

def read_nested(voice):
  return voice.values.count

init:
  holder = Values()
  voice = Voice()

sample:
  out1 = f32(integer_only(read_array(holder)) + integer_only(read_nested(voice)))
"#;
        let program = parse_program(source).expect("struct aggregate return source should parse");
        let typed =
            analyze(program).expect("concrete struct field paths must feed nested specialization");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_only.__onda_mono__g_i32")));
        lower_program_to_optimized_mir(&typed)
            .expect("struct aggregate inferred returns should lower to MIR");
    }

    #[test]
    fn generic_calls_in_parameter_defaults_are_monomorphized() {
        let source = r#"
def identity<T>(x: T) -> T:
  return x

def consume(value: i32 = identity(1)) -> i32:
  return value

def with_default<T>(value: T = identity(T(1))) -> T:
  return value

sample:
  out1 = f32(consume() + with_default<i32>())
"#;
        let program = parse_program(source).expect("generic default source should parse");
        let typed = analyze(program).expect("the generic default call should specialize");
        let specialized_name = "identity.__onda_mono__g_i32";
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == specialized_name));
        let consume = typed
            .defs
            .iter()
            .find(|function| function.name == "consume")
            .expect("missing consume definition");
        assert!(matches!(
            consume.param_defaults.as_slice(),
            [Some(Expr::UserCall { name, .. })] if name == specialized_name
        ));
        let with_default = typed
            .defs
            .iter()
            .find(|function| function.name.contains("with_default.__onda_mono__g_i32"))
            .expect("missing specialized with_default definition");
        assert!(matches!(
            with_default.param_defaults.as_slice(),
            [Some(Expr::UserCall { name, .. })] if name == specialized_name
        ));
        lower_program_to_optimized_mir(&typed)
            .expect("the rewritten default should lower without an unresolved call");
    }

    #[test]
    fn generic_type_arguments_in_parameter_defaults_are_validated_before_specialization() {
        let source = r#"
def identity<T>(value: T) -> T:
  return value

def consume(value: f32 = identity<bool>(true)) -> f32:
  return value

sample:
  out1 = consume()
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("bool must not be accepted as a generic default type argument");
        assert!(errors.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("'bool' is not valid as a generic type argument for 'identity'")
        }));
    }

    #[test]
    fn overload_calls_in_slice_assignment_targets_are_rewritten() {
        let source = r#"
def pick(value: i32) -> i32:
  return value

def pick(value: f32) -> i32:
  return i32(value)

sample:
  index: i32 = 0
  values = [0.0, 0.0]
  values[pick(index):] = 1.0
  out1 = values[0]
"#;
        let program = parse_program(source).expect("slice target overload source should parse");
        let typed = analyze(program).expect("the i32 slice-bound overload should resolve");
        lower_program_to_optimized_mir(&typed)
            .expect("the rewritten slice target should lower to MIR");
    }

    #[test]
    fn dependent_generic_scalar_calls_defer_until_the_owner_is_concrete() {
        let source = r#"
def integer_only<T>(x: T) -> T:
  return ~x

def relay<T>(x: T) -> T:
  return integer_only(x)

sample:
  out1 = f32(relay(1))
"#;
        let program = parse_program(source).expect("dependent scalar source should parse");
        let typed = analyze(program).expect("dependent scalar call should specialize as i32");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_only.__onda_mono__g_i32")));
    }

    #[test]
    fn untyped_scalar_owners_defer_dependent_generic_calls_until_specialization() {
        let source = r#"
def integer_only<T>(x: T) -> T:
  return ~x

def relay(x):
  return integer_only(x)

sample:
  out1 = f32(relay(1))
"#;
        let program = parse_program(source).expect("dependent untyped source should parse");
        let typed = analyze(program)
            .expect("the nested generic call should use the owner's concrete i32 type");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "integer_only.__onda_mono__g_i32"));
        assert!(!typed
            .defs
            .iter()
            .any(|function| function.name == "integer_only.__onda_mono__g_f32"));
        lower_program_to_optimized_mir(&typed)
            .expect("the deferred nested specialization should lower to MIR");
    }

    #[test]
    fn dependent_call_returns_do_not_select_overloads_before_specialization() {
        let source = r#"
def identity<T>(x: T) -> T:
  return x

def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

def relay<T>(x: T) -> T:
  value = identity(x)
  return classify(value)

sample:
  out1 = f32(relay(1))
"#;
        let program = parse_program(source).expect("dependent return source should parse");
        let typed = analyze(program).expect("the concrete i32 return must select classify(i32)");
        let relay = typed
            .defs
            .iter()
            .find(|function| function.name.contains("relay.__onda_mono__g_i32"))
            .expect("missing i32 relay specialization");
        assert_eq!(relay.return_ty, ReturnType::Scalar(PrimitiveType::I32));
        assert!(relay.body.iter().any(|statement| {
            matches!(
                statement,
                Stmt::Return {
                    expr: Expr::UserCall { name, .. },
                    ..
                } if name.starts_with("__onda_ovl_classify") && name.ends_with("_1")
            )
        }));
    }

    #[test]
    fn concrete_owners_defer_nested_generic_calls_to_the_fixed_point() {
        let source = r#"
def identity<T>(x: T) -> T:
  return x

def relay<T>(x: T):
  return identity(x)

def passthrough<T>(x: T) -> T:
  return x

def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

def concrete(x: i32) -> i32:
  return classify(passthrough(relay(x)))

sample:
  out1 = f32(concrete(1))
"#;
        let program = parse_program(source).expect("concrete dependent source should parse");
        let typed = analyze(program)
            .expect("a concrete owner must wait for nested generic return specialization");
        for name in [
            "identity.__onda_mono__g_i32",
            "relay.__onda_mono__g_i32",
            "passthrough.__onda_mono__g_i32",
        ] {
            assert!(
                typed.defs.iter().any(|function| function.name == name),
                "missing i32 specialization '{name}'"
            );
        }
        assert!(!typed
            .defs
            .iter()
            .any(|function| function.name == "passthrough.__onda_mono__g_f32"));
        lower_program_to_optimized_mir(&typed)
            .expect("the converged concrete wrapper should lower to MIR");
    }

    #[test]
    fn terminating_if_branch_preserves_the_continuing_branch_call_types() {
        let source = r#"
def identity<T>(x: T) -> T:
  return x

def classify(x: i64) -> i64:
  return x

def classify(x: f32) -> f32:
  return x

def generic_after_return(flag: bool) -> i64:
  if flag:
    return i64(1)
  else:
    value: i64 = 2
  return identity(value)

def overload_after_return(flag: bool) -> i64:
  if flag:
    return i64(3)
  else:
    value: i64 = 4
  return classify(value)

sample:
  out1 = f32(generic_after_return(true) + overload_after_return(false))
"#;
        let program = parse_program(source).expect("early-return branch source should parse");
        let typed = analyze(program)
            .expect("only the continuing branch should constrain calls after the join");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "identity.__onda_mono__g_i64"));
        let overload = typed
            .defs
            .iter()
            .find(|function| function.name == "overload_after_return")
            .expect("missing overload wrapper");
        assert!(overload.body.iter().any(|statement| {
            matches!(
                statement,
                Stmt::Return {
                    expr: Expr::UserCall { name, .. },
                    ..
                } if name.starts_with("__onda_ovl_classify") && name.ends_with("_1")
            )
        }));
        lower_program_to_optimized_mir(&typed)
            .expect("reachability-aware call typing should lower to MIR");
    }

    #[test]
    fn continuing_loop_branch_retains_types_after_continue() {
        let source = r#"
def identity<T>(x: T) -> T:
  return x

def accumulate() -> i64:
  total: i64 = 0
  for i in 0..2:
    if i == 0:
      continue
    else:
      value: i64 = i64(i)
    total = total + identity(value)
  return total

sample:
  out1 = f32(accumulate())
"#;
        let program = parse_program(source).expect("continue branch source should parse");
        let typed = analyze(program)
            .expect("the continuing loop branch should retain its concrete local type");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "identity.__onda_mono__g_i64"));
        lower_program_to_optimized_mir(&typed)
            .expect("continue-aware call typing should lower to MIR");
    }

    #[test]
    fn runtime_loop_branch_retains_locals_after_continue() {
        let source = r#"
sample:
  for i in 0..2:
    if i == 0:
      continue
    else:
      value: f32 = f32(i)
    out1 = value
"#;
        let program = parse_program(source).expect("runtime continue branch source should parse");
        let typed = analyze(program)
            .expect("the continuing runtime branch should retain its local binding");
        lower_program_to_optimized_mir(&typed)
            .expect("runtime continue-aware bindings should lower to MIR");
    }

    #[test]
    fn unresolved_bindings_do_not_publish_reassignment_types_before_specialization() {
        let source = r#"
def identity<T>(x: T) -> T:
  return x

def replace_param(x):
  x = 1
  return x

def replace_local<T>(x: T):
  value = identity(x)
  value = 1
  return value

def replace_typed_local<T>(x: T):
  value: T = 1
  return value

def replace_typed_array<T>(x: T):
  values: T[2] = [x, x]
  return values[0]

def classify(x: i32) -> i32:
  return x

def classify(x: f64) -> f64:
  return x

params:
  source: f64 = 4.0

sample:
  out1 = f32(classify(replace_param(source)) + classify(replace_local(source)) + classify(replace_typed_local(source)) + classify(replace_typed_array(source)))
"#;
        let program = parse_program(source).expect("unresolved reassignment source should parse");
        let typed = analyze(program)
            .expect("reassignments must retain the concrete specialization binding type");
        for name in [
            "replace_param.__onda_mono__scalar_f64",
            "replace_local.__onda_mono__g_f64",
            "replace_typed_local.__onda_mono__g_f64",
            "replace_typed_array.__onda_mono__g_f64",
        ] {
            let function = typed
                .defs
                .iter()
                .find(|function| function.name == name)
                .unwrap_or_else(|| panic!("missing f64 specialization '{name}'"));
            assert_eq!(function.return_ty, ReturnType::Scalar(PrimitiveType::F64));
        }
        lower_program_to_optimized_mir(&typed)
            .expect("reassignment specializations should lower to MIR");
    }

    #[test]
    fn structural_params_publish_argument_independent_return_types() {
        let source = r#"
struct Holder:
  value: f32

def constant(holder):
  return 1

def constants(holder):
  return (1, 2.0)

def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

def integer_only<T>(x: T) -> T:
  return ~x

init:
  holder = Holder()

sample:
  pair = constants(holder)
  selected: i32 = classify(constant(holder))
  out1 = f32(selected + integer_only(constant(holder)) + integer_only(pair[0])) + pair[1]
"#;
        let program = parse_program(source).expect("independent return source should parse");
        let typed = analyze(program)
            .expect("an open structural parameter must not hide an independent i32 return");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_only.__onda_mono__g_i32")));
        lower_program_to_optimized_mir(&typed)
            .expect("independent structural-template returns should lower to MIR");
    }

    #[test]
    fn structural_param_returns_specialize_per_concrete_struct() {
        let source = r#"
struct IntHolder:
  value: i32 = 1

struct FloatHolder:
  value: f32 = 2.0

def read(holder):
  return holder.value

def classify(x: i32) -> i32:
  return x

def classify(x: f32) -> f32:
  return x

init:
  integers = IntHolder()
  floats = FloatHolder()

sample:
  integer_value: i32 = classify(read(integers))
  float_value: f32 = classify(read(floats))
  out1 = f32(integer_value) + float_value
"#;
        let program = parse_program(source).expect("structural return source should parse");
        let typed = analyze(program)
            .expect("each concrete struct call must publish its own field-derived return type");
        for struct_name in ["IntHolder", "FloatHolder"] {
            assert!(typed.defs.iter().any(|function| {
                function.name.contains("read.__onda_mono") && function.name.contains(struct_name)
            }));
        }
        lower_program_to_optimized_mir(&typed)
            .expect("concrete structural return specializations should lower to MIR");
    }

    #[test]
    fn concrete_f32_untyped_calls_have_concrete_nested_call_types() {
        let source = r#"
def classify(x: i32) -> f32:
  return 1.0

def classify(x: f32) -> f32:
  return 2.0

def relay(x):
  return classify(x)

sample:
  out1 = relay(1.0)
"#;
        let program = parse_program(source).expect("f32 relay source should parse");
        let typed = analyze(program).expect("the concrete f32 call must select classify(f32)");
        let relay = typed
            .defs
            .iter()
            .find(|function| function.name == "relay.__onda_mono__scalar_f32")
            .expect("missing concrete f32 relay specialization");
        assert!(matches!(
            relay.param_kinds.as_slice(),
            [TypedFnParam::Scalar {
                ty: Some(PrimitiveType::F32)
            }]
        ));
        assert!(relay.body.iter().any(|statement| {
            matches!(
                statement,
                Stmt::Return {
                    expr: Expr::UserCall { name, .. },
                    ..
                } if name.starts_with("__onda_ovl_classify") && name.ends_with("_2")
            )
        }));
    }

    #[test]
    fn explicit_type_arguments_filter_overload_candidates() {
        let source = r#"
def choose<T>(x: T) -> T:
  return x

def choose(x: f32) -> f32:
  return x

sample:
  out1 = f32(choose<i32>(1))
"#;
        let program = parse_program(source).expect("generic overload source should parse");
        let typed = analyze(program).expect("explicit type args must select the generic overload");
        assert!(typed.defs.iter().any(|function| {
            function.name.starts_with("__onda_ovl_choose")
                && function.name.contains(".__onda_mono__g_i32")
                && function.return_ty == ReturnType::Scalar(PrimitiveType::I32)
        }));
    }

    #[test]
    fn inferred_bool_generic_type_arguments_are_rejected() {
        let source = r#"
def identity<T>(x: T) -> T:
  return x

sample:
  if identity(true):
    out1 = 1.0
  else:
    out1 = 0.0
"#;
        let program = parse_program(source).expect("bool generic source should parse");
        let errors = analyze(program).expect_err("inferred bool must obey the generic domain");
        assert!(errors.iter().any(|diagnostic| {
            diagnostic.message.contains("inferred as bool")
                && diagnostic
                    .message
                    .contains("generic type arguments must be numeric")
        }));
    }

    #[test]
    fn unresolved_monomorphized_calls_keep_source_call_diagnostics() {
        let source = r#"
def first(values: []):
  return values[0]

sample:
  out1 = first()
"#;
        let program = parse_program(source).expect("missing-argument source should parse");
        let errors = analyze(program).expect_err("the missing array argument must be rejected");
        assert!(errors.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("missing required argument 'values'")
        }));
        assert!(!errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown function 'first'")));
    }

    #[test]
    fn unresolved_monomorphized_calls_fail_before_mir_lowering() {
        let source = r#"
def first(values: []):
  return values[0]

sample:
  out1 = first([])
"#;
        let program = parse_program(source).expect("underconstrained source should parse");
        let errors = analyze(program)
            .expect_err("an underconstrained specialization must be a semantic error");
        assert!(errors.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("does not provide concrete argument types required for specialization")
        }));
        assert!(!errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown function 'first'")));
    }

    #[test]
    fn unresolved_monomorphized_array_arguments_validate_their_elements() {
        let source = r#"
def first(values: []):
  return values[0]

sample:
  out1 = first([missing])
"#;
        let program = parse_program(source).expect("unknown-element source should parse");
        let errors = analyze(program).expect_err("the unknown array element must be rejected");
        assert!(errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown symbol 'missing'")));
        assert!(!errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unknown function 'first'")));
    }

    #[test]
    fn dependent_generic_array_calls_defer_until_the_owner_is_concrete() {
        let source = r#"
const Values: i32[1] = [1]

def integer_first<T>(xs: T[]) -> T:
  return ~xs[0]

def relay(xs: []):
  return integer_first(xs)

sample:
  out1 = f32(relay(Values))
"#;
        let program = parse_program(source).expect("dependent array source should parse");
        let typed = analyze(program).expect("dependent array call should specialize as i32");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("integer_first.__onda_mono__arr_i32")));
    }

    #[test]
    fn argument_free_generic_type_parameters_still_default_to_f32() {
        let source = r#"
def zero<T>() -> T:
  return T(0)

sample:
  out1 = zero()
"#;
        let program = parse_program(source).expect("argument-free generic source should parse");
        let typed = analyze(program).expect("argument-free T should retain its f32 default");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.contains("zero.__onda_mono__g_f32")));
    }

