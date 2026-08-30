    #[test]
    fn loop_index_shadows_same_named_aggregate_for_call_inference() {
        let source = r#"
def classify(x: i32) -> f32:
  return 1.0

def classify(x: f32) -> f32:
  return 2.0

ins:
  i: f32[2] = [0.0, 0.0]

sample:
  total = 0.0
  for i in 0..2:
    total = total + classify(i)
  out1 = total
"#;
        let program = parse_program(source).expect("loop shadowing source should parse");
        analyze(program).expect("the i32 loop index should shadow the outer array");
    }

    #[test]
    fn repeated_generic_scalar_constraints_choose_one_widened_type() {
        let src = r#"
outs:
  out1

def choose<T>(x: T, lo: T, hi: T) -> T:
  if x < lo:
    return lo
  if x > hi:
    return hi
  return x

sample:
  x: f32 = f32(16777216.0)
  lo: f64 = f64(16777217.0)
  hi: f64 = f64(16777218.0)
  out1 = f32(choose(x, lo, hi) - lo)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("mixed scalar constraints should widen T to f64");
        let choose = typed
            .defs
            .iter()
            .find(|function| function.name.starts_with("choose.__onda_mono__g_f64"))
            .expect("missing widened f64 specialization");
        assert!(
            choose.param_kinds.iter().all(|kind| matches!(
                kind,
                TypedFnParam::Scalar {
                    ty: Some(PrimitiveType::F64)
                }
            )),
            "all repeated T parameters must use the same f64 specialization: {:?}",
            choose.param_kinds
        );
        assert_eq!(choose.return_ty, ReturnType::Scalar(PrimitiveType::F64));
    }

    #[test]
    fn explicit_generic_scalar_type_rejects_typed_narrowing_argument() {
        let src = r#"
outs:
  out1

def id<T>(x: T) -> T:
  return x

sample:
  wide: f64 = 1.25
  out1 = id<f32>(wide)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("explicit f32 generic argument must not narrow f64");
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic.message.contains("function 'id")
                    && diagnostic.message.contains("cannot assign F64 to F32")
            }),
            "expected explicit-generic narrowing diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn array_constructor_initializers_validate_generic_type_arguments_before_rewriting() {
        let src = r#"
outs:
  out1

def id<T>(x: T) -> T:
  return x

sample:
  values: bool[1] = [id<bool>(true)]
  out1 = 0.0
"#;
        assert_analyze_error_contains(src, "'bool' is not valid as a generic type argument");
    }

    #[test]
    fn generic_def_return_annotation_specializes_through_monomorphization() {
        let src = "outs:\n  out1\ndef id<T>(x: T) -> T:\n  return x\nsample:\n  out1 = id(0.5)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("generic return annotation should analyze");
        assert!(
            typed.defs.iter().any(|def| {
                def.name.contains("id.__onda_mono")
                    && def.return_ty == ReturnType::Scalar(PrimitiveType::F32)
            }),
            "expected monomorphized id def with f32 return, got {:#?}",
            typed.defs
        );
    }

    #[test]
    fn specialized_return_diagnostics_use_source_function_names() {
        let src = r#"
def invalid<T>(value: T) -> T:
  return true

sample:
  out1 = f32(invalid(1))
"#;
        let errors = analyze(parse_program(src).expect("source should parse"))
            .expect_err("the specialized return type must reject bool");
        let diagnostic = errors
            .iter()
            .find(|diagnostic| diagnostic.message.contains("return in function 'invalid'"))
            .unwrap_or_else(|| panic!("missing source-like return diagnostic: {errors:?}"));
        assert!(!diagnostic.message.contains("__onda_"), "{diagnostic:?}");
    }

    #[test]
    fn explicit_tuple_return_type_analyzes_and_sets_typed_return() {
        let src = "outs:\n  out1\ndef pair(x: f32) -> (f32, i32):\n  return (x, 1)\nsample:\n  vals = pair(0.5)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("explicit tuple return annotation should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name == "pair")
            .expect("missing typed def");
        assert_eq!(
            def.return_ty,
            ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
        );
    }

    #[test]
    fn explicit_tuple_return_type_rejects_element_mismatch() {
        let src = "outs:\n  out1\ndef pair() -> (f32, i32):\n  return (1.0, 2.5)\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("tuple return element mismatch should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message.contains("return in function 'pair'")
                    && diag.message.contains("cannot assign F32 to I32")
            }),
            "expected tuple return mismatch diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn generic_tuple_return_annotation_specializes_through_monomorphization() {
        let src = "outs:\n  out1\ndef pair<T>(x: T, y: i32) -> (T, i32):\n  return (x, y)\nsample:\n  vals = pair(0.5, 2)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("generic tuple return annotation should analyze");
        assert!(
            typed.defs.iter().any(|def| {
                def.name.contains("pair.__onda_mono")
                    && def.return_ty
                        == ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
            }),
            "expected monomorphized pair def with tuple return, got {:#?}",
            typed.defs
        );
    }

    #[test]
    fn unannotated_defs_still_infer_tuple_returns() {
        let src = "outs:\n  out1\ndef pair(x):\n  return (x, 1)\nsample:\n  vals = pair(0.5)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("unannotated tuple return should still infer");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.contains("pair"))
            .expect("missing typed def");
        assert_eq!(
            def.return_ty,
            ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
        );
    }

    #[test]
    fn return_annotations_do_not_change_overload_resolution_behavior() {
        let src = "outs:\n  out1\ndef foo(x: f32) -> f32:\n  return x\ndef foo(x: f64) -> f32:\n  return f32(x)\nsample:\n  out1 = foo(1)\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("annotated ambiguous overload should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message
                    .contains("ambiguous overload for function 'foo'")
            }),
            "expected ambiguous overload diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn generic_struct_method_return_annotation_specializes_with_owner_generics() {
        let src = "struct Pair<T>:\n  a: T\n  b: T\n\n  def swap(self) -> (T, T):\n    return (self.b, self.a)\n\nouts:\n  out1\ninit:\n  p = Pair<f32>(1.0, 2.0)\nsample:\n  vals = p.swap()\n  out1 = vals[0] + vals[1]\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed =
            analyze(program).expect("generic struct method return annotation should analyze");
        assert!(
            typed.defs.iter().any(|def| {
                def.name.ends_with(".swap")
                    && def.method_of.is_some()
                    && def.return_ty
                        == ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::F32])
            }),
            "expected specialized swap method def with tuple return, got {:#?}",
            typed.defs
        );
    }

    #[test]
    fn duplicate_generated_generic_struct_specialization_is_deduped() {
        let src = "namespace sc:\n  struct CyclePhase<T>:\n    phase: T\n\n    def tick(self):\n      self.phase = self.phase + T(1.0)\n      return self.phase\n\n  namespace Sine:\n    proc ar<T>:\n      outs:\n        out1: T\n      init<T>:\n        core = sc::CyclePhase<T>()\n      sample:\n        out1 = core.tick()\n\nouts:\n  out1\ninit:\n  a = sc::Sine::ar()\n  z = sc::CyclePhase<f32>()\n\nsample:\n  out1 = a()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program)
            .expect("duplicate generated generic struct specialization should be deduped");
        let generated = typed
            .structs
            .iter()
            .filter(|s| s.name == "sc::CyclePhase.__gen__f32")
            .count();
        assert_eq!(generated, 1);
    }

    #[test]
    fn proc_local_def_return_annotation_lowers_and_validates() {
        let src = "proc Voice:\n  outs:\n    out1\n\n  def pair(x: f32) -> (f32, i32):\n    return (x, 1)\n\n  sample:\n    vals = pair(0.5)\n    out1 = vals[0] + f32(vals[1])\n\ninit:\n  voice = Voice()\n\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-local return annotation should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.contains("Voice.__onda_proc_local__pair"))
            .expect("missing lowered proc-local def");
        assert_eq!(
            def.return_ty,
            ReturnType::Tuple(vec![PrimitiveType::F32, PrimitiveType::I32])
        );
        assert!(matches!(
            def.param_kinds.first(),
            Some(TypedFnParam::Struct { struct_name }) if struct_name == "Voice"
        ));
    }

    #[test]
    fn struct_return_annotation_is_rejected() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef borrow(pair: Pair) -> Pair:\n  return pair\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("struct return annotation should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message
                    .contains("function 'borrow' return type 'Pair' is not supported")
            }),
            "expected unsupported struct return diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn namespaced_struct_return_annotation_is_rejected_after_rewrite() {
        let src = "namespace dsp:\n  struct Pair:\n    x\nouts:\n  out1\ndef borrow(pair: dsp::Pair) -> dsp::Pair:\n  return pair\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("namespaced struct return annotation should fail");
        assert!(
            errors.iter().any(|diag| {
                diag.message
                    .contains("function 'borrow' return type 'dsp::Pair' is not supported")
            }),
            "expected unsupported namespaced return diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn sample_tuple_local_survives_if_merge() {
        let src = "outs:\n  out1\nsample:\n  if true:\n    vals = (0.5, 1)\n  else:\n    vals = (0.25, 2)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("tuple local should survive if merge");
    }

    #[test]
    fn sample_tuple_local_survives_loop_reassignment() {
        let src = "outs:\n  out1\nsample:\n  vals = (0.0, 0)\n  for i in 0..2:\n    vals = (f32(i), i)\n  out1 = vals[0] + f32(vals[1])\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("tuple local should survive loop reassignment");
    }

    #[test]
    fn def_tuple_local_survives_if_merge() {
        let src = "outs:\n  out1\ndef pick(flag: bool) -> f32:\n  if flag:\n    vals = (0.5, 1)\n  else:\n    vals = (0.25, 2)\n  return vals[0] + f32(vals[1])\nsample:\n  out1 = pick(true)\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("tuple local in def should survive if merge");
    }

    #[test]
    fn tuple_bindings_reject_shape_changing_reassignment_during_semantics() {
        let cases = [
            (
                "outs:\n  out1\nsample:\n  vals = (0.5, 1)\n  vals = 0.5\n  out1 = 0.0\n",
                "assignment to tuple local 'vals' requires a tuple value",
            ),
            (
                "outs:\n  out1\nsample:\n  vals = 1\n  vals = (0.5, 1)\n  out1 = 0.0\n",
                "cannot assign a tuple value to scalar local 'vals'",
            ),
            (
                "def broken():\n  vals = (0.5, 1)\n  vals = 0.5\n  return vals[0]\nsample:\n  out1 = broken()\n",
                "assignment to tuple local 'vals' requires a tuple value",
            ),
            (
                "def broken():\n  vals = 1\n  vals = (0.5, 1)\n  return 0.0\nsample:\n  out1 = broken()\n",
                "cannot assign a tuple value to scalar local 'vals'",
            ),
            (
                "outs:\n  out1\nsample:\n  vals = (0.5, 1)\n  vals = (0.25, 2, true)\n  out1 = 0.0\n",
                "tuple assignment to 'vals' has arity 3, expected 2",
            ),
            (
                "outs:\n  out1\nsample:\n  vals = (1, 2)\n  vals = (0.5, 2)\n  out1 = 0.0\n",
                "tuple assignment to 'vals' element 0 type mismatch",
            ),
            (
                "outs:\n  out1\ninit:\n  vals = (1, 2)\nsample:\n  vals = (0.5, 2)\n  out1 = 0.0\n",
                "tuple assignment to 'vals' element 0 type mismatch",
            ),
            (
                "def broken(vals: (i32, i32)):\n  vals = (0.5, 2)\n  return 0.0\nsample:\n  out1 = broken((1, 2))\n",
                "tuple assignment to 'vals' element 0 type mismatch",
            ),
            (
                "init:\n  if true:\n    vals = (1, 2)\n    vals = (0.5, 2)\nsample:\n  out1 = 0.0\n",
                "tuple assignment to 'vals' element 0 type mismatch",
            ),
            (
                "struct Holder:\n  vals: (i32, i32) = (0, 0)\ninit:\n  holder = Holder()\n  holder.vals = (0.5, 2)\nsample:\n  out1 = 0.0\n",
                "tuple assignment to 'holder.vals' element 0 type mismatch",
            ),
            (
                "block:\n  vals: (i32, i32) = (1, 2)\n  vals: (i32, i32) = (3, 4)\n  sample:\n    out1 = 0.0\n",
                "typed tuple declaration for 'vals' is only allowed on first assignment",
            ),
        ];

        for (source, expected) in cases {
            let program = parse_program(source).expect("shape-change source should parse");
            let errors = analyze(program).expect_err("binding shape changes must be rejected");
            assert!(
                errors.iter().any(|diag| diag.message.contains(expected)),
                "expected '{expected}', got {errors:?}"
            );
        }
    }

    #[test]
    fn namespaced_proc_array_typed_declaration_analyzes() {
        let src = "import std/osc\nouts:\n  out1\ninit:\n  voices: std::osc::Sine[2] = std::osc::Sine()\nsample:\n  out1 = voices[0]()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("namespaced proc array typed declaration should analyze");
    }

    #[test]
    fn proc_array_broadcast_constructor_reuses_top_level_scalar_arguments() {
        let src = r#"
proc Filter:
  params:
    cutoff = 1000.0
    q = 0.707
  outs:
    out1
  sample:
    out1 = cutoff + q

params:
  cutoff = 920.0
  resonance = 1.5
outs:
  out1
init:
  filters: Filter[2] = Filter(cutoff = cutoff, q = resonance)
sample:
  out1 = filters[0]() + filters[1]()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("scalar constructor arguments should broadcast to every slot");
    }

    #[test]
    fn nested_proc_array_broadcast_constructor_reuses_owner_scalar_arguments() {
        let src = r#"
proc Filter:
  params:
    cutoff = 1000.0
  outs:
    out1
  sample:
    out1 = cutoff

proc Bank:
  params:
    cutoff = 920.0
  init:
    filters: Filter[2] = Filter(cutoff = cutoff)
  outs:
    out1
  sample:
    out1 = filters[0]() + filters[1]()

outs:
  out1
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program)
            .expect("owner scalar constructor arguments should broadcast to nested proc arrays");
    }

    #[test]
    fn def_accepts_proc_array_param_for_indexed_init_events() {
        let src = "import std/osc\nouts:\n  out1\ndef init_voices(voices):\n  for i in 0..2:\n    voices[i].init(freq = 110.0)\ninit:\n  voices: std::osc::Sine[2]\n  init_voices(voices)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-array def parameter should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.starts_with("init_voices"))
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::ProcArray {
                    proc_name,
                    len: 2
                }] if proc_name.starts_with("std::osc::Sine")
            ),
            "expected proc-array param kind, got {:#?}",
            def.param_kinds
        );
        assert!(
            !def.body.iter().any(def_stmt_contains_proc_index_sentinel),
            "proc-array indexed event call should be rewritten before typed def lowering: {:#?}",
            def.body
        );
    }

    #[test]
    fn def_accepts_proc_array_param_for_indexed_field_assignments() {
        let src = "proc Voice:\n  params:\n    gain = 0.0\n  outs:\n    out1\n  sample:\n    out1 = gain\nouts:\n  out1\ndef set_gains(voices, gain):\n  for i in 0..2:\n    voices[i].gain = gain + f32(i)\ninit:\n  voices: Voice[2] = Voice()\n  set_gains(voices, 1.0)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-array indexed field assignment should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.starts_with("set_gains"))
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::ProcArray { proc_name, len: 2 }, TypedFnParam::Scalar { .. }]
                    if proc_name == "Voice"
            ),
            "expected proc-array param kind, got {:#?}",
            def.param_kinds
        );
        assert!(
            !def.body.iter().any(def_stmt_contains_proc_index_sentinel),
            "proc-array indexed field assignment should be rewritten before typed def lowering: {:#?}",
            def.body
        );
    }

    #[test]
    fn def_accepts_proc_array_param_len_builtin() {
        let src = "proc Voice:\n  params:\n    gain = 0.0\n  outs:\n    out1\n  sample:\n    out1 = gain\nouts:\n  out1\ndef set_and_sum(voices):\n  total = 0.0\n  for i in 0..(voices.len()):\n    voices[i].gain = f32(i + 1)\n    total = total + voices[i]()\n  return total + f32(voices.len())\ninit:\n  voices: Voice[3] = Voice()\nsample:\n  out1 = set_and_sum(voices)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("proc-array len builtin should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.starts_with("set_and_sum"))
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::ProcArray { proc_name, len: 3 }] if proc_name == "Voice"
            ),
            "expected proc-array param kind, got {:#?}",
            def.param_kinds
        );
    }

    #[test]
    fn unused_untyped_proc_array_def_is_ignored() {
        let src = "import std/osc\nouts:\n  out1\ndef init_voices(voices):\n  for i in 0..2:\n    voices[i].init(freq = 110.0)\ninit:\n  voices: std::osc::Sine[2]\n  for i in 0..2:\n    voices[i].init(freq = 110.0)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("unused untyped proc-array def should be ignored");
        assert!(
            !typed.defs.iter().any(|def| def.name == "init_voices"),
            "unused def unexpectedly survived into typed program: {:#?}",
            typed.defs
        );
    }

    #[test]
    fn unused_explicitly_typed_struct_def_still_reports_body_errors() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef broken(pair: Pair):\n  return pair.y\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("unused explicitly typed struct def should still analyze");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("struct instance 'pair' (type 'Pair') has no field 'y'")),
            "expected unreachable explicit struct def diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn unused_explicitly_typed_proc_def_still_reports_body_errors() {
        let src = "proc Voice:\n  outs:\n    out1\n  sample:\n    out1 = 0.0\nouts:\n  out1\ndef broken(voice: Voice):\n  return voice.missing\nsample:\n  out1 = 0.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("unused explicitly typed proc def should still analyze");
        assert!(
            errors.iter().any(|diag| diag
                .message
                .contains("struct instance 'voice' (type 'Voice') has no field 'missing'")),
            "expected unreachable explicit proc def diagnostic, got {errors:?}"
        );
    }

    #[test]
    fn def_forwards_proc_array_params_across_calls() {
        let src = "import std/osc\nouts:\n  out1\ndef init_inner(voices, freq):\n  for i in 0..2:\n    voices[i].init(freq = freq * f32(i + 1))\ndef init_outer(voices, freq):\n  init_inner(voices, freq)\ninit:\n  voices: std::osc::Sine[2]\n  init_outer(voices, 110.0)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("forwarded proc-array params should analyze");
        for def_name in ["init_inner", "init_outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name.starts_with(def_name))
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::ProcArray { proc_name, len: 2 })
                        if proc_name.starts_with("std::osc::Sine")
                ),
                "expected proc-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn def_forwards_proc_array_params_across_multiple_layers() {
        let src = "import std/osc\nouts:\n  out1\ndef init_leaf(voices, freq):\n  for i in 0..2:\n    voice = voices[i]\n    voice.init(freq = freq * f32(i + 1))\ndef init_mid(voices, freq):\n  init_leaf(voices, freq)\ndef init_top(voices, freq):\n  init_mid(voices, freq)\ninit:\n  voices: std::osc::Sine[2]\n  init_top(voices, 110.0)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("multi-layer proc-array forwarding should analyze");
        for def_name in ["init_leaf", "init_mid", "init_top"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name.starts_with(def_name))
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::ProcArray { proc_name, len: 2 })
                        if proc_name.starts_with("std::osc::Sine")
                ),
                "expected proc-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn def_infers_struct_array_params_from_call_sites() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef sum_pairs(pairs):\n  total = 0.0\n  for i in 0..2:\n    total = total + pairs[i].x\n  return total\ninit:\n  pairs: Pair[2]\n  for i in 0..2:\n    p = pairs[i]\n    p.x = f32(i + 1)\nsample:\n  out1 = sum_pairs(pairs)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("struct-array def param should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.starts_with("sum_pairs"))
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::StructArray { struct_name }] if struct_name == "Pair"
            ),
            "expected struct-array param kind, got {:#?}",
            def.param_kinds
        );
    }

    #[test]
    fn def_infers_struct_array_params_from_len_builtin() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef set_and_sum(pairs):\n  total = 0.0\n  for i in 0..(pairs.len()):\n    pairs[i].x = f32(i + 1)\n    total = total + pairs[i].x\n  return total + f32(pairs.len())\ninit:\n  pairs: Pair[3]\nsample:\n  out1 = set_and_sum(pairs)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("struct-array len builtin should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.starts_with("set_and_sum"))
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::StructArray { struct_name }] if struct_name == "Pair"
            ),
            "expected struct-array param kind, got {:#?}",
            def.param_kinds
        );
    }

    #[test]
    fn def_infers_struct_array_params_from_indexed_field_assignments() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef seed_pairs(pairs):\n  for i in 0..2:\n    pairs[i].x = f32(i + 1)\ninit:\n  pairs: Pair[2]\n  seed_pairs(pairs)\nsample:\n  out1 = pairs[0].x + pairs[1].x\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("struct-array indexed field assignment should analyze");
        let def = typed
            .defs
            .iter()
            .find(|def| def.name.starts_with("seed_pairs"))
            .expect("missing typed def");
        assert!(
            matches!(
                def.param_kinds.as_slice(),
                [TypedFnParam::StructArray { struct_name }] if struct_name == "Pair"
            ),
            "expected struct-array param kind, got {:#?}",
            def.param_kinds
        );
    }

    #[test]
    fn def_forwards_struct_array_params_across_calls() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef sum_inner(pairs):\n  total = 0.0\n  for i in 0..2:\n    total = total + pairs[i].x\n  return total\ndef sum_outer(pairs):\n  return sum_inner(pairs)\ninit:\n  pairs: Pair[2]\n  for i in 0..2:\n    p = pairs[i]\n    p.x = f32(i + 1)\nsample:\n  out1 = sum_outer(pairs)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("forwarded struct-array params should analyze");
        for def_name in ["sum_inner", "sum_outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name.starts_with(def_name))
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::StructArray { struct_name }) if struct_name == "Pair"
                ),
                "expected struct-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn def_infers_struct_array_params_across_multiple_layers_with_methods() {
        let src = "struct Tap:\n  gain: f32\n\n  def read(self):\n    return self.gain * 2.0\n\nstruct Voice:\n  tap: Tap\n  bias: f32\n\n  def value(self):\n    return self.tap.read() + self.bias\n\nouts:\n  out1\ndef read_leaf(voice: Voice):\n  return voice.value()\ndef read_mid(voices, idx: i32):\n  return read_leaf(voices[idx])\ndef read_top(voices, idx: i32):\n  return read_mid(voices, idx)\ninit:\n  voices: Voice[2]\n  v = voices[0]\n  v.tap.gain = 1.0\n  v.bias = 0.5\nsample:\n  out1 = read_top(voices, 0)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed =
            analyze(program).expect("multi-layer struct-array method forwarding should analyze");
        let read_leaf = typed
            .defs
            .iter()
            .find(|def| def.name == "read_leaf")
            .expect("missing typed def 'read_leaf'");
        assert!(
            matches!(
                read_leaf.param_kinds.first(),
                Some(TypedFnParam::Struct { struct_name }) if struct_name == "Voice"
            ),
            "expected Voice owner param for 'read_leaf', got {:#?}",
            read_leaf.param_kinds
        );
        for def_name in ["read_mid", "read_top"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name.starts_with(def_name))
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::StructArray { struct_name }) if struct_name == "Voice"
                ),
                "expected struct-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn def_forwards_struct_array_alias_params_across_calls() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef sum_inner(pairs):\n  total = 0.0\n  for i in 0..2:\n    p = pairs[i]\n    total = total + p.x\n  return total\ndef sum_outer(pairs):\n  return sum_inner(pairs)\ninit:\n  pairs: Pair[2]\n  for i in 0..2:\n    p = pairs[i]\n    p.x = f32(i + 1)\nsample:\n  out1 = sum_outer(pairs)\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("forwarded struct-array alias params should analyze");
        for def_name in ["sum_inner", "sum_outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name.starts_with(def_name))
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::StructArray { struct_name }) if struct_name == "Pair"
                ),
                "expected struct-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn unused_untyped_struct_array_def_is_ignored() {
        let src = "struct Pair:\n  x\nouts:\n  out1\ndef sum_pairs(pairs):\n  total = 0.0\n  for i in 0..2:\n    total = total + pairs[i].x\n  return total\ninit:\n  pairs: Pair[2]\n  for i in 0..2:\n    p = pairs[i]\n    p.x = f32(i + 1)\nsample:\n  out1 = pairs[0].x + pairs[1].x\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("unused untyped struct-array def should be ignored");
        assert!(
            !typed.defs.iter().any(|def| def.name == "sum_pairs"),
            "unused def unexpectedly survived into typed program: {:#?}",
            typed.defs
        );
    }

    #[test]
    fn def_accepts_proc_array_alias_init_events() {
        let src = "import std/osc\nouts:\n  out1\ndef init_voices(voices):\n  for i in 0..2:\n    voice = voices[i]\n    voice.init(freq = 110.0 * f32(i + 1))\ninit:\n  voices: std::osc::Sine[2]\n  init_voices(voices)\nsample:\n  out1 = voices[0]() + voices[1]()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc-array alias init def should analyze");
    }

    #[test]
    fn def_forwarding_proc_params_preserves_nested_proc_array_block_hooks() {
        let src = r#"
proc Voice:
  outs:
    out1
  block:
    sample:
      out1 = 0.25

proc Bank:
  outs:
    out1
  init:
    voices: Voice[2] = Voice()
  sample:
    out1 = 0.0

outs:
  out1

def inner(bank: Bank, idx: i32):
  return bank.voices[idx]()

def outer(bank: Bank):
  idx: i32 = 1
  return inner(bank, idx)

init:
  bank = Bank()

sample:
  out1 = outer(bank)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("forwarded proc param call should analyze");

        let bank_struct = typed
            .structs
            .iter()
            .find(|st| st.name == "Bank")
            .expect("missing lowered Bank struct");
        assert!(
            bank_struct.fields.iter().any(|field| {
                field.name == "__onda_proc_block_active_voices"
                    && matches!(field.ty, TypedFieldType::Array(2))
            }),
            "expected Bank struct to own nested proc-array active slots, got {:#?}",
            bank_struct.fields
        );

        let inner = typed
            .defs
            .iter()
            .find(|def| def.name == "inner")
            .expect("missing typed inner def");
        assert!(
            inner
                .body
                .iter()
                .any(|stmt| stmt_contains_assign_to_index_base(
                    stmt,
                    "bank.__onda_proc_block_active_voices"
                )),
            "expected inner def to mark nested proc-array slots active: {:#?}",
            inner.body
        );

        assert!(
            typed
                .block_pre
                .iter()
                .any(|stmt| stmt_contains_user_call_name(stmt, "Bank.__onda_proc_block_pre")),
            "expected sample caller to inject Bank block_pre: {:#?}",
            typed.block_pre
        );
        assert!(
            typed
                .block_post
                .iter()
                .any(|stmt| stmt_contains_user_call_name(stmt, "Bank.__onda_proc_block_post")),
            "expected sample caller to inject Bank block_post: {:#?}",
            typed.block_post
        );

        let bank_block_post = typed
            .defs
            .iter()
            .find(|def| def.name == "Bank.__onda_proc_block_post")
            .expect("missing lowered Bank block_post def");
        assert!(
            bank_block_post
                .body
                .iter()
                .any(|stmt| stmt_contains_index_base(stmt, "self.__onda_proc_block_active_voices")),
            "expected Bank block_post to flush nested proc-array active slots: {:#?}",
            bank_block_post.body
        );
    }

    #[test]
    fn def_multi_layer_proc_array_forwarding_preserves_block_hooks() {
        let src = r#"
proc Voice:
  outs:
    out1
  block:
    sample:
      out1 = 0.25

outs:
  out1

def leaf(voices, idx: i32):
  return voices[idx]()

def mid(voices, idx: i32):
  return leaf(voices, idx)

def outer(voices, idx: i32):
  return mid(voices, idx)

init:
  voices: Voice[2] = Voice()
  idx: i32 = 1

sample:
  out1 = outer(voices, idx)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed =
            analyze(program).expect("multi-layer proc-array block-hook call should analyze");

        for def_name in ["leaf", "mid", "outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name.starts_with(def_name))
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::ProcArray { proc_name, len: 2 }) if proc_name == "Voice"
                ),
                "expected proc-array first param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }

        assert!(
            typed
                .block_pre
                .iter()
                .any(|stmt| stmt_contains_assign_to_index_base(
                    stmt,
                    "__onda_proc_block_active_voices"
                )),
            "expected sample caller to reset top-level proc-array active slots in block_pre: {:#?}",
            typed.block_pre
        );
        assert!(
            typed
                .block_post
                .iter()
                .any(|stmt| {
                    stmt_contains_user_call_name(stmt, "Voice.__onda_proc_block_post")
                        || stmt_contains_index_base(stmt, "__onda_proc_block_active_voices")
                }),
            "expected sample caller to flush top-level proc-array active slots in block_post: {:#?}",
            typed.block_post
        );
    }

    #[test]
    fn def_forwards_owner_proc_params_across_multiple_layers() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

proc Bank:
  params:
    base = 0.0
  outs:
    out1
  init:
    voices: Voice[2] = Voice()
    voices[0].init(gain = base + 1.0)
    voices[1].init(gain = base + 2.0)
  sample:
    out1 = voices[1]()

proc Rack:
  outs:
    out1
  init:
    banks: Bank[2] = [Bank(base = 0.0), Bank(base = 10.0)]
  sample:
    out1 = 0.0

outs:
  out1

def read_leaf(rack: Rack, bank_idx: i32):
  return rack.banks[bank_idx]().out1

def read_mid(rack: Rack, bank_idx: i32):
  return read_leaf(rack, bank_idx)

def read_outer(rack: Rack):
  return read_mid(rack, 1)

init:
  rack = Rack()

sample:
  out1 = read_outer(rack)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("multi-layer owner-proc forwarding should analyze");

        for def_name in ["read_leaf", "read_mid", "read_outer"] {
            let def = typed
                .defs
                .iter()
                .find(|def| def.name == def_name)
                .unwrap_or_else(|| panic!("missing typed def '{def_name}'"));
            assert!(
                matches!(
                    def.param_kinds.first(),
                    Some(TypedFnParam::Struct { struct_name }) if struct_name == "Rack"
                ),
                "expected Rack owner param for '{def_name}', got {:#?}",
                def.param_kinds
            );
        }
    }

    #[test]
    fn top_level_event_proc_call_is_rejected_as_not_sample_only() {
        let src = r#"
proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1

events:
  fire():
    x = voice()

init:
  voice = Voice()

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs = analyze(program).expect_err("event proc operator call should fail");
        assert!(
            errs.iter().any(|diag| diag
                .message
                .contains("for sample-rate proc is only allowed in sample")),
            "expected sample-only proc operator diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn top_level_event_def_proc_array_call_is_rejected_as_not_sample_only() {
        let src = r#"
proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

outs:
  out1

def run_selected(voices, idx: i32):
  return voices[idx]()

events:
  fire():
    x = run_selected(voices, idx)

init:
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 0

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs = analyze(program).expect_err("event def proc-array operator call should fail");
        assert!(
            errs.iter()
                .any(|diag| diag.message.contains("not provably sample-only")),
            "expected not-provably-sample-only diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn top_level_event_def_owner_proc_call_is_rejected_as_not_sample_only() {
        let src = r#"
proc Voice:
  outs:
    out1
  sample:
    out1 = 0.0

proc Bank:
  outs:
    out1
  init:
    voices: Voice[2] = Voice()
  sample:
    out1 = 0.0

outs:
  out1

def run_selected(bank: Bank, idx: i32):
  return bank.voices[idx]()

events:
  fire():
    x = run_selected(bank, idx)

init:
  bank = Bank()
  idx: i32 = 0

sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs = analyze(program).expect_err("event owner-proc operator call should fail");
        assert!(
            errs.iter()
                .any(|diag| diag.message.contains("not provably sample-only")),
            "expected not-provably-sample-only diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn proc_local_def_called_from_proc_event_cannot_call_nested_proc_operator() {
        let src = r#"
proc Child:
  outs:
    out1
  sample:
    out1 = 0.75

proc Parent:
  outs:
    out1
  init:
    child = Child()

  def run_child():
    return child()

  events:
    ping():
      x = run_child()

  sample:
    out1 = 0.0

init:
  p = Parent()

sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs =
            analyze(program).expect_err("proc-event proc-local def operator call should fail");
        assert!(
            errs.iter()
                .any(|diag| diag.message.contains("not provably sample-only")),
            "expected not-provably-sample-only diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn proc_local_def_called_from_proc_event_cannot_call_nested_proc_array_alias() {
        let src = r#"
proc Child:
  outs:
    out1
  sample:
    out1 = 0.75

proc Parent:
  outs:
    out1
  init:
    children: Child[2] = Child()
    idx: i32 = 0

  def run_child():
    v = children[idx]
    return v()

  events:
    ping():
      x = run_child()
      idx = 1 - idx

  sample:
    out1 = 0.0

init:
  p = Parent()

sample:
  out1 = p()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs =
            analyze(program).expect_err("proc-event proc-local proc-array alias call should fail");
        assert!(
            errs.iter()
                .any(|diag| diag.message.contains("not provably sample-only")),
            "expected not-provably-sample-only diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn proc_event_proc_operator_is_rejected_even_when_called_from_sample() {
        let src = r#"
proc Child:
  outs:
    out1
  sample:
    out1 = 0.25

proc Parent:
  outs:
    out1
  init:
    child = Child()

  events:
    ping():
      x = child()

  sample:
    out1 = 0.0

outs:
  out1

init:
  parent = Parent()

sample:
  parent.ping()
  out1 = parent()
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errs = analyze(program)
            .expect_err("proc event body proc operator should fail even from sample caller");
        assert!(
            errs.iter().any(|diag| diag
                .message
                .contains("for sample-rate proc is only allowed in sample")),
            "expected sample-only proc operator diagnostic, got {errs:?}"
        );
    }

    #[test]
    fn unqualified_proc_event_call_reports_receiver_only_guidance() {
        let src = r#"
proc Phasor:
  params:
    freq = 1.0

  event set_freq(val):
    freq = val

  init:
    set_freq(freq)

  outs:
    out1

  sample:
    out1 = freq

outs:
  out1

init:
  phasor = Phasor()

sample:
  out1 = phasor()
"#;

        assert_analyze_error_contains(src, "proc event 'set_freq' is receiver-only");
    }

    fn def_stmt_contains_proc_index_sentinel(stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                expr_contains_proc_index_sentinel(expr)
            }
            Stmt::Print { values, .. } => values.iter().any(expr_contains_proc_index_sentinel),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                expr_contains_proc_index_sentinel(cond)
                    || then_branch
                        .iter()
                        .any(def_stmt_contains_proc_index_sentinel)
                    || else_branch
                        .iter()
                        .any(def_stmt_contains_proc_index_sentinel)
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                expr_contains_proc_index_sentinel(start)
                    || expr_contains_proc_index_sentinel(end)
                    || step.as_ref().is_some_and(expr_contains_proc_index_sentinel)
                    || body.iter().any(def_stmt_contains_proc_index_sentinel)
            }
            Stmt::While { cond, body, .. } => {
                expr_contains_proc_index_sentinel(cond)
                    || body.iter().any(def_stmt_contains_proc_index_sentinel)
            }
        }
    }

    fn stmt_contains_user_call_name(stmt: &Stmt, expected_name: &str) -> bool {
        match stmt {
            Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            } => name == expected_name,
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                then_branch
                    .iter()
                    .any(|stmt| stmt_contains_user_call_name(stmt, expected_name))
                    || else_branch
                        .iter()
                        .any(|stmt| stmt_contains_user_call_name(stmt, expected_name))
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => body
                .iter()
                .any(|stmt| stmt_contains_user_call_name(stmt, expected_name)),
            _ => false,
        }
    }

    fn stmt_contains_assign_to_index_base(stmt: &Stmt, expected_base: &str) -> bool {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Index { base, .. },
                ..
            } => base == expected_base,
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                then_branch
                    .iter()
                    .any(|stmt| stmt_contains_assign_to_index_base(stmt, expected_base))
                    || else_branch
                        .iter()
                        .any(|stmt| stmt_contains_assign_to_index_base(stmt, expected_base))
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => body
                .iter()
                .any(|stmt| stmt_contains_assign_to_index_base(stmt, expected_base)),
            _ => false,
        }
    }

    fn stmt_contains_index_base(stmt: &Stmt, expected_base: &str) -> bool {
        match stmt {
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                expr_contains_index_base(expr, expected_base)
            }
            Stmt::Print { values, .. } => values
                .iter()
                .any(|expr| expr_contains_index_base(expr, expected_base)),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                expr_contains_index_base(cond, expected_base)
                    || then_branch
                        .iter()
                        .any(|stmt| stmt_contains_index_base(stmt, expected_base))
                    || else_branch
                        .iter()
                        .any(|stmt| stmt_contains_index_base(stmt, expected_base))
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                expr_contains_index_base(start, expected_base)
                    || expr_contains_index_base(end, expected_base)
                    || step
                        .as_ref()
                        .is_some_and(|expr| expr_contains_index_base(expr, expected_base))
                    || body
                        .iter()
                        .any(|stmt| stmt_contains_index_base(stmt, expected_base))
            }
            Stmt::While { cond, body, .. } => {
                expr_contains_index_base(cond, expected_base)
                    || body
                        .iter()
                        .any(|stmt| stmt_contains_index_base(stmt, expected_base))
            }
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
        }
    }

    fn expr_contains_index_base(expr: &Expr, expected_base: &str) -> bool {
        match expr {
            Expr::Index { base, index, .. } => {
                base == expected_base || expr_contains_index_base(index, expected_base)
            }
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => values
                .iter()
                .any(|expr| expr_contains_index_base(expr, expected_base)),
            Expr::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => [selector, channel, start, end]
                .into_iter()
                .flatten()
                .any(|expr| expr_contains_index_base(expr, expected_base)),
            Expr::ArrayCtor { spec, init, .. } => {
                expr_contains_index_base(&spec.size, expected_base)
                    || init.as_ref().is_some_and(|values| {
                        values
                            .iter()
                            .any(|expr| expr_contains_index_base(expr, expected_base))
                    })
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                expr_contains_index_base(lhs, expected_base)
                    || expr_contains_index_base(rhs, expected_base)
            }
            Expr::Call { args, .. } => args
                .iter()
                .any(|expr| expr_contains_index_base(expr, expected_base)),
            Expr::UserCall { args, .. } => args
                .iter()
                .any(|arg| expr_contains_index_base(&arg.expr, expected_base)),
            Expr::Cast { expr: inner, .. }
            | Expr::UnaryNot { expr: inner, .. }
            | Expr::UnaryBitNot { expr: inner, .. } => {
                expr_contains_index_base(inner, expected_base)
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => false,
        }
    }

    fn expr_contains_proc_index_sentinel(expr: &Expr) -> bool {
        match expr {
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => false,
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                values.iter().any(expr_contains_proc_index_sentinel)
            }
            Expr::Index { index, .. } => expr_contains_proc_index_sentinel(index),
            Expr::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => [selector, channel, start, end]
                .into_iter()
                .flatten()
                .any(|expr| expr_contains_proc_index_sentinel(expr)),
            Expr::ArrayCtor { spec, init, .. } => {
                expr_contains_proc_index_sentinel(&spec.size)
                    || init
                        .as_ref()
                        .is_some_and(|values| values.iter().any(expr_contains_proc_index_sentinel))
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                expr_contains_proc_index_sentinel(lhs) || expr_contains_proc_index_sentinel(rhs)
            }
            Expr::Call { args, .. } => args.iter().any(expr_contains_proc_index_sentinel),
            Expr::UserCall { name, args, .. } => {
                name.starts_with("__onda_proc_index_call")
                    || args
                        .iter()
                        .any(|arg| expr_contains_proc_index_sentinel(&arg.expr))
            }
            Expr::Cast { expr, .. }
            | Expr::UnaryNot { expr, .. }
            | Expr::UnaryBitNot { expr, .. } => expr_contains_proc_index_sentinel(expr),
        }
    }

    #[test]
    fn namespace_errors_on_namespaced_calls_use_call_spans() {
        let src = "import std/osc\nouts:\n  out1\nsample:\n  out1 = std::osc::Missing()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("unknown namespaced call should fail");
        let diag = errors
            .iter()
            .find(|diag| {
                diag.message
                    .contains("unknown symbol 'Missing' in namespace 'std::osc'")
            })
            .expect("missing unknown namespaced call diagnostic");

        assert_eq!((diag.line, diag.column), (5, 10));
        assert_eq!(diag.end_line, 5);
    }

    #[test]
    fn init_branch_local_can_feed_top_level_state_but_not_escape_to_sample() {
        let src = "outs:\n  out1\ninit:\n  if true:\n    tmp = 1.0\n  else:\n    tmp = 2.0\n  carried = tmp\nsample:\n  out1 = carried\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("branch-local init value should feed later top-level init state");
    }

    #[test]
    fn init_loop_local_does_not_escape_loop() {
        let src = "outs:\n  out1\ninit:\n  for i in 0..2:\n    tmp = f32(i)\n  carried = tmp\nsample:\n  out1 = carried\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("loop-local init symbol should not escape");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'tmp'")),
            "missing unknown-symbol diagnostic for escaped init loop local: {errors:#?}"
        );
    }

    #[test]
    fn sample_scoped_local_name_can_be_reintroduced_with_a_different_type() {
        let src = r#"
outs:
  out1

sample:
  if in1 > 0.0:
    temp = 0.0
  for i in 0..1:
    temp = f32(i)
  temp = true
  if temp:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("branch- and loop-local bindings must not escape their scopes");
    }

    #[test]
    fn multi_argument_numeric_builtins_adapt_literals_to_a_concrete_peer() {
        let src = r#"
def bounded(x: f32) -> f32:
  return fma(max(x, 16777217.0), 1.0, 0.0)

outs:
  out1

sample:
  out1 = bounded(f32(16777216.0))
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("builtin literals should adopt the concrete f32 peer width");
    }

    #[test]
    fn comparison_literal_accepts_the_concrete_f32_peer_context() {
        let src = r#"
outs:
  out1

sample:
  x: f32 = f32(16777216.0)
  if x == 16777217.0:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("comparison literal should adopt its concrete f32 peer width");
    }

    #[test]
    fn sample_typed_declaration_stays_local() {
        let src = "outs:\n  out1\nsample:\n  tmp: f32 = 1.0\n  out1 = tmp\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("sample-local typed declaration should analyze");
        assert!(
            !typed.state_vars.iter().any(|name| name == "tmp"),
            "sample local typed declaration unexpectedly became state"
        );
    }

    #[test]
    fn block_pre_top_level_state_is_visible_in_sample_and_post() {
        let src = "outs:\n  out1\nblock:\n  pre_root = 1.0\n  sample:\n    mix = pre_root\n    out1 = mix\n  post_seen = pre_root\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("top-level block pre vars should be visible in sample and post");
    }

    #[test]
    fn block_pre_cannot_read_sample_rate_inputs() {
        let src = "ins:\n  in1\nouts:\n  out1\nblock:\n  held = in1\n  sample:\n    out1 = held\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre input read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "audio input 'in1' can only be read in sample; move this read into the block's nested sample section"
            )),
            "missing sample-section diagnostic for block pre input read: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_read_dynamic_inputs() {
        let src = "ins 1\nouts:\n  out1\nblock:\n  held = ins[0]\n  sample:\n    out1 = held\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre dynamic input read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "audio input 'ins' can only be read in sample; move this read into the block's nested sample section"
            )),
            "missing dynamic-input diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn block_post_cannot_read_dynamic_inputs() {
        let src = "ins 1\nouts:\n  out1\nblock:\n  sample:\n    out1 = in1\n  held = ins[0]\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block post dynamic input read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "audio input 'ins' can only be read in sample; move this read into the block's nested sample section"
            )),
            "missing dynamic-input diagnostic for block post: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_write_named_audio_outputs() {
        let src = "outs:\n  out1\nblock:\n  out1 = 0.0\n  sample:\n    out1 = 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre output write should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "audio output 'out1' can only be written in sample; move this assignment into the block's nested sample section"
            )),
            "missing sample-section diagnostic for block pre output write: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_write_dynamic_outputs() {
        let src = "outs 1\nblock:\n  outs[0] = 0.0\n  sample:\n    out1 = 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre dynamic output write should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "audio output array 'outs' can only be written in sample; move this assignment into the block's nested sample section"
            )),
            "missing dynamic-output diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_read_input_arrays() {
        let src = "ins:\n  freqs: f32[2] = [220, 440]\nouts:\n  out1\nblock:\n  held = freqs[0]\n  sample:\n    out1 = held\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre input array read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "audio input 'freqs' can only be read in sample; move this read into the block's nested sample section"
            )),
            "missing input-array diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_cannot_write_output_arrays() {
        let src = "outs:\n  stereo: f32[2]\nblock:\n  stereo[0] = 0.0\n  sample:\n    stereo[0] = 1.0\n    stereo[1] = 1.0\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("block pre output array write should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "audio output array 'stereo' can only be written in sample; move this assignment into the block's nested sample section"
            )),
            "missing output-array diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn block_pre_can_read_dynamic_params() {
        let src = "params 2\nouts:\n  out1\nblock:\n  held = params[0] + params[1]\n  sample:\n    out1 = held\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("block pre dynamic param read should analyze");
    }

    #[test]
    fn dynamic_params_are_not_first_class_values() {
        let cases = [
            (
                "top-level local alias",
                "params 2\nouts:\n  out1\nblock:\n  ps = params\n  sample:\n    out1 = ps[0]\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level slice alias",
                "params 2\nouts:\n  out1\nblock:\n  ps = params[0:2]\n  sample:\n    out1 = ps[0]\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level def argument",
                "def sum(ps: f32[]) -> f32:\n  return ps[0]\nparams 2\nouts:\n  out1\nsample:\n  out1 = sum(params)\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level def indexed read",
                "def get() -> f32:\n  return params[0]\nparams 2\nouts:\n  out1\nsample:\n  out1 = get()\n",
                "dynamic param indexing 'params[...]' is only allowed in block or sample",
            ),
            (
                "top-level init indexed read",
                "params 2\nouts:\n  out1\ninit:\n  held = params[0]\nsample:\n  out1 = held\n",
                "dynamic param indexing 'params[...]' is only allowed in block or sample",
            ),
            (
                "top-level block surface assignment",
                "params 2\nouts:\n  out1\nblock:\n  params = 1.0\n  sample:\n    out1 = params[0]\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level sample kins surface assignment",
                "kins 2\nouts:\n  out1\nsample:\n  kins = 1.0\n  out1 = kins[0]\n",
                "dynamic param array 'kins' is not a first-class value",
            ),
            (
                "proc init indexed read",
                "proc P:\n  params 2\n  outs:\n    out1\n  init:\n    held = params[0]\n  sample:\n    out1 = param1\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
                "dynamic param indexing 'params[...]' is only allowed in block or sample",
            ),
            (
                "proc local def indexed read",
                "proc P:\n  params 2\n  outs:\n    out1\n  def get() -> f32:\n    return params[0]\n  sample:\n    out1 = get()\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
                "dynamic param indexing 'params[...]' is only allowed in block or sample",
            ),
            (
                "proc local def surface assignment",
                "proc P:\n  params 2\n  outs:\n    out1\n  def set():\n    params = 1.0\n  sample:\n    set()\n    out1 = param1\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
                "dynamic param array 'params' is not a first-class value",
            ),
            (
                "top-level kins def indexed read",
                "def get() -> f32:\n  return kins[0]\nkins 2\nouts:\n  out1\nsample:\n  out1 = get()\n",
                "dynamic param indexing 'kins[...]' is only allowed in block or sample",
            ),
            (
                "top-level kins first-class argument",
                "def sum(ps: f32[]) -> f32:\n  return ps[0]\nkins 2\nouts:\n  out1\nsample:\n  out1 = sum(kins)\n",
                "dynamic param array 'kins' is not a first-class value",
            ),
            (
                "child param surface alias",
                "proc Child:\n  params:\n    a = 0.0\n    b = 0.0\n  outs:\n    out1\n  sample:\n    out1 = a\nproc Parent:\n  init:\n    child = Child()\n  outs:\n    out1\n  sample:\n    ps = child.params\n    out1 = ps[0]\nouts:\n  out1\ninit:\n  p = Parent()\nsample:\n  out1 = p()\n",
                "dynamic param array 'child.params' is not a first-class value",
            ),
            (
                "child param surface assignment",
                "proc Child:\n  params:\n    a = 0.0\n    b = 0.0\n  outs:\n    out1\n  sample:\n    out1 = a\nproc Parent:\n  init:\n    child = Child()\n  outs:\n    out1\n  sample:\n    child.params = 1.0\n    out1 = child()\nouts:\n  out1\ninit:\n  p = Parent()\nsample:\n  out1 = p()\n",
                "dynamic param array 'child.params' is not a first-class value",
            ),
        ];

        for (_label, src, expected) in cases {
            assert_analyze_error_contains(src, expected);
        }
    }

    #[test]
    fn io_surfaces_are_block_sample_bound_and_not_first_class_values() {
        let cases = [
            (
                "init scalar input read",
                "ins:\n  in1\nouts:\n  out1\ninit:\n  held = in1\nsample:\n  out1 = 0.0\n",
                "I/O symbol 'in1' is only available in block or sample",
            ),
            (
                "init named input read",
                "ins:\n  audio\nouts:\n  out1\ninit:\n  held = audio\nsample:\n  out1 = 0.0\n",
                "I/O symbol 'audio' is only available in block or sample",
            ),
            (
                "def scalar input read",
                "def get() -> f32:\n  return in1\nins:\n  in1\nouts:\n  out1\nsample:\n  out1 = get()\n",
                "I/O symbol 'in1' is only available in block or sample",
            ),
            (
                "def named output read",
                "def get() -> f32:\n  return wet\nouts:\n  wet\nsample:\n  wet = 0.0\n",
                "I/O symbol 'wet' is only available in block or sample",
            ),
            (
                "event scalar output write",
                "outs:\n  out1\nevent ping():\n  out1 = 1.0\nsample:\n  out1 = 0.0\n",
                "I/O symbol 'out1' is only available in block or sample",
            ),
            (
                "event named kout write",
                "kouts:\n  meter\nevent ping():\n  meter = 1.0\nblock:\n  meter = 0.0\n",
                "I/O symbol 'meter' is only available in block or sample",
            ),
            (
                "init dynamic input read",
                "ins 2\nouts:\n  out1\ninit:\n  held = ins[0]\nsample:\n  out1 = 0.0\n",
                "I/O symbol 'ins' is only available in block or sample",
            ),
            (
                "def dynamic kouts read",
                "def get() -> f32:\n  return kouts[0]\nkouts 2\nblock:\n  kouts[0] = 0.0\n  kouts[1] = get()\n",
                "I/O symbol 'kouts' is only available in block or sample",
            ),
            (
                "sample synthetic input assignment",
                "ins 2\nouts:\n  out1\nsample:\n  ins = 0.0\n  out1 = ins[0]\n",
                "I/O array 'ins' is not a first-class value",
            ),
            (
                "sample synthetic output assignment",
                "outs 2\nsample:\n  outs = 0.0\n  outs[0] = 0.0\n  outs[1] = 0.0\n",
                "I/O array 'outs' is not a first-class value",
            ),
            (
                "block synthetic kouts assignment",
                "kouts 2\nblock:\n  kouts = 0.0\n  kouts[0] = 0.0\n",
                "I/O array 'kouts' is not a first-class value",
            ),
            (
                "sample input array argument",
                "def first(xs: f32[]) -> f32:\n  return xs[0]\nins:\n  freqs: f32[2] = [220, 440]\nouts:\n  out1\nsample:\n  out1 = first(freqs)\n",
                "I/O array 'freqs' is not a first-class value",
            ),
            (
                "sample input array slice alias",
                "ins:\n  freqs: f32[2] = [220, 440]\nouts:\n  out1\nsample:\n  fs = freqs[0:2]\n  out1 = fs[0]\n",
                "I/O array 'freqs' is not a first-class value",
            ),
            (
                "sample synthetic output argument",
                "def poke(xs: f32[]):\n  xs[0] = 1.0\nouts 2\nsample:\n  poke(outs)\n  outs[0] = 0.0\n  outs[1] = 0.0\n",
                "I/O array 'outs' is not a first-class value",
            ),
            (
                "top-level event named input read",
                "ins:\n  audio\nouts:\n  out1\nevent ping():\n  held = audio\nsample:\n  out1 = audio\n",
                "I/O symbol 'audio' is only available in block or sample",
            ),
            (
                "proc init named input read",
                "proc P:\n  ins:\n    audio\n  outs:\n    out1\n  init:\n    held = audio\n  sample:\n    out1 = audio\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p(0.0)\n",
                "I/O symbol 'audio' is only available in block or sample",
            ),
            (
                "proc event named output write",
                "proc P:\n  outs:\n    wet\n  event ping():\n    wet = 1.0\n  sample:\n    wet = 0.0\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p()\n",
                "I/O symbol 'wet' is only available in block or sample",
            ),
            (
                "proc local def named input read",
                "proc P:\n  ins:\n    audio\n  outs:\n    out1\n  def get() -> f32:\n    return audio\n  sample:\n    out1 = get()\nouts:\n  out1\ninit:\n  p = P()\nsample:\n  out1 = p(0.0)\n",
                "I/O symbol 'audio' is only available in block or sample",
            ),
        ];

        for (_label, src, expected) in cases {
            assert_analyze_error_contains(src, expected);
        }
    }

    #[test]
    fn top_level_kins_alias_analyzes_as_params() {
        let src = r#"
kins:
  gain = 0.25

outs:
  out1

sample:
  out1 = gain
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("kins alias should analyze as params");

        assert_eq!(typed.params.len(), 1);
        assert_eq!(typed.params[0].name, "gain");
        assert_eq!(typed.param_types["gain"], PrimitiveType::F32);
    }

    #[test]
    fn top_level_infers_numbered_kins_as_params() {
        let src = r#"
sample:
  out1 = kin2
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("implicit kinN params should analyze");

        assert_eq!(
            typed
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["kin1", "kin2"]
        );
        assert_eq!(typed.outs, vec!["out1"]);
        assert!(!typed.params_explicit);
    }

    #[test]
    fn top_level_infers_numbered_params_as_params() {
        let src = r#"
sample:
  out1 = param2
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("implicit paramN params should analyze");

        assert_eq!(
            typed
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["param1", "param2"]
        );
        assert_eq!(typed.outs, vec!["out1"]);
    }

    #[test]
    fn top_level_dynamic_kins_indexes_explicit_kins() {
        let src = r#"
kins 2
sample:
  out1 = kins[1]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dynamic kins should analyze");

        assert_eq!(
            typed
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            vec!["kin1", "kin2"]
        );
        assert!(typed.params_explicit);
    }

    #[test]
    fn top_level_infers_numbered_kouts() {
        let src = r#"
block:
  kout2 = 0.5
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("implicit koutN outputs should analyze");

        assert!(typed.outs.is_empty());
        assert_eq!(typed.control_outs, vec!["kout1", "kout2"]);
    }

    #[test]
    fn top_level_dynamic_kouts_indexes_explicit_kouts() {
        let src = r#"
kouts 2
block:
  kouts[1] = 0.5
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("dynamic kouts should analyze");

        assert_eq!(typed.control_outs, vec!["kout1", "kout2"]);
    }

    #[test]
    fn top_level_kouts_arrays_use_array_storage_not_scalar_slots() {
        let src = r#"
kouts:
  meter: f32[2]

block:
  meter[0] = 0.25
  meter[1] = 0.75
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("kouts array should analyze");

        assert_eq!(typed.control_outs, vec!["meter[0]", "meter[1]"]);
        assert!(typed
            .array_vars
            .iter()
            .any(|array| array.name == "meter" && array.len == 2));
        assert!(!typed.state_vars.iter().any(|name| name == "meter[0]"));
        assert!(!typed.state_vars.iter().any(|name| name == "meter[1]"));
    }

    #[test]
    fn top_level_rejects_both_params_and_kins() {
        assert_analyze_error_contains(
            r#"
params:
  gain = 0.25

kins:
  freq = 440.0

outs:
  out1

sample:
  out1 = gain
"#,
            "duplicate block 'params'",
        );
    }

    #[test]
    fn top_level_mixed_sample_and_block_outputs_are_split() {
        let src = r#"
outs:
  out1
kouts:
  meter

block:
  meter = 1.0
  sample:
    out1 = 0.5
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("mixed output timing should analyze");

        assert_eq!(typed.outs, vec!["out1"]);
        assert_eq!(typed.control_outs, vec!["meter"]);
        assert_eq!(typed.out_types["out1"], PrimitiveType::F32);
        assert_eq!(typed.control_out_types["meter"], PrimitiveType::F32);
        assert!(typed.state_vars.iter().any(|name| name == "meter"));
    }

    #[test]
    fn top_level_rejects_output_and_control_output_name_conflict() {
        assert_analyze_error_contains(
            r#"
outs:
  myOut
kouts:
  myOut

block:
  myOut = 1.0
  sample:
    myOut = 0.5
"#,
            "control output 'myOut' conflicts with output 'myOut'",
        );
    }

    #[test]
    fn top_level_rejects_numbered_audio_names_in_kouts() {
        assert_analyze_error_contains(
            r#"
kouts:
  out1

block:
  out1 = 1.0
"#,
            "use 'koutN' for control outputs",
        );
    }

    #[test]
    fn top_level_control_only_block_output_does_not_require_sample() {
        let src = r#"
kouts:
  meter

block:
  meter = 1.0
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("control-only block should analyze");

        assert!(typed.outs.is_empty());
        assert_eq!(typed.control_outs, vec!["meter"]);
    }

    #[test]
    fn top_level_control_only_output_requires_block_entry() {
        assert_analyze_error_contains(
            r#"
kouts:
  meter
"#,
            "missing required 'block' section",
        );
    }

    #[test]
    fn legacy_outs_rate_aliases_are_rejected_by_parser() {
        let src = r#"
outs @block {
  meter
}

block {
  meter = 1.0
}
"#;
        let errors = parse_program(src).expect_err("outs @block should not parse");
        assert!(!errors.is_empty());
    }

    #[test]
    fn current_owner_outputs_are_write_only() {
        assert_analyze_error_contains(
            r#"
outs {
  out1
}

sample {
  out1 = out1
}
"#,
            "cannot read output symbol 'out1'",
        );
    }

    #[test]
    fn current_owner_output_arrays_are_write_only() {
        assert_analyze_error_contains(
            r#"
outs {
  stereo: f32[2]
}

sample {
  stereo[0] = 0.25
  stereo[1] = stereo[0]
}
"#,
            "cannot read output array symbol 'stereo[...]'",
        );
        assert_analyze_error_contains(
            r#"
kouts {
  meter: f32[2]
}

block {
  meter[0] = 1.0
  meter[1] = meter[0]
}
"#,
            "cannot read output array symbol 'meter[...]'",
        );
        assert_analyze_error_contains(
            r#"
outs {
  out1
}
kouts {
  meter: f32[2]
}

block {
  sample {
    out1 = 0.0
    meter[0] = 1.0
  }
}
"#,
            "cannot assign to output array symbol 'meter' in sample",
        );
    }

    #[test]
    fn current_owner_output_arrays_allow_matching_phase_writes() {
        let program = parse_program(
            r#"
outs {
  stereo: f32[2]
}
kouts {
  meter: f32[2]
}

block {
  meter[0] = 1.0
  meter[1] = 2.0
  sample {
    stereo[0] = 0.25
    stereo[1] = 0.5
  }
}
"#,
        )
        .expect("parse should succeed");
        analyze(program).expect("matching-phase output array writes should analyze");
    }

    #[test]
    fn protected_proc_views_cannot_be_passed_as_array_pointers() {
        assert_analyze_error_contains(
            r#"
def poke(ps: f32[]):
  ps[0] = 1.0

params {
  gain = 0.0
  trim = 0.0
}
outs {
  out1
}

sample {
  poke(params)
  out1 = gain
}
"#,
            "dynamic param array 'params' is not a first-class value",
        );
    }

    #[test]
    fn mixed_timing_outputs_are_write_only_in_all_runtime_scopes() {
        assert_analyze_error_contains(
            r#"
outs {
  out1
}
kouts {
  meter
}
block {
  meter = 1.0
  sample {
    out1 = meter
  }
}
"#,
            "cannot read output symbol 'meter'",
        );
        assert_analyze_error_contains(
            r#"
outs {
  out1
}
kouts {
  meter
}
block {
  held = out1
  meter = 1.0
  sample {
    out1 = 0.0
  }
}
"#,
            "cannot read output symbol 'out1'",
        );
        assert_analyze_error_contains(
            r#"
outs {
  out1
}
kouts {
  meter
}
block {
  meter = 1.0
  sample {
    out1 = kouts[0]
  }
}
"#,
            "cannot read output symbol 'kouts[i]'",
        );
    }

    #[test]
    fn mixed_timing_outputs_allow_matching_dynamic_indexing() {
        let program = parse_program(
            r#"
outs 2
kouts 2

block {
  kouts[1] = 1.0
  sample {
    outs[0] = 0.25
    outs[1] = 0.5
  }
}
"#,
        )
        .expect("parse should succeed");
        let typed = analyze(program).expect("mixed dynamic outputs should analyze");

        assert_eq!(typed.outs, vec!["out1", "out2"]);
        assert_eq!(typed.control_outs, vec!["kout1", "kout2"]);
    }

    #[test]
    fn proc_rejects_both_outs_and_kouts() {
        let errors = parse_program(
            r#"
proc P
{
  outs {
    out1
  }
  kouts {
    kout1
  }
  sample {
    out1 = 1.0
  }
}
"#,
        )
        .expect_err("proc cannot declare both outs and kouts");
        assert!(errors
            .iter()
            .any(|diag| diag.message.contains("duplicate proc output block")));
    }

    #[test]
    fn block_rate_proc_operator_can_be_called_from_block() {
        let src = r#"
proc Meter
{
  kouts {
    kout1
  }
  block {
    kout1 = 2.0
  }
}

kouts {
  meter
}

init {
  m = Meter()
}

block {
  meter = m()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("block-rate proc operator should analyze");

        assert!(typed.outs.is_empty());
        assert_eq!(typed.control_outs, vec!["meter"]);
    }

    #[test]
    fn block_rate_proc_ordinal_alias_uses_kout_prefix() {
        let src = r#"
proc Meter
{
  kouts {
    level
  }
  block {
    level = 2.0
  }
}

kouts {
  meter
}

init {
  m = Meter()
}

block {
  meter = m().kout1
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("kouts proc ordinal alias should use koutN");
    }

    #[test]
    fn block_rate_proc_rejects_audio_ordinal_alias() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  kouts {
    level
  }
  block {
    level = 2.0
  }
}

kouts {
  meter
}

init {
  m = Meter()
}

block {
  meter = m().out1
}
"#,
            "or koutN",
        );
    }

    #[test]
    fn sample_rate_proc_rejects_control_ordinal_alias() {
        assert_analyze_error_contains(
            r#"
proc Voice
{
  outs {
    wet
  }
  sample {
    wet = 1.0
  }
}

outs {
  out1
}

init {
  v = Voice()
}

sample {
  out1 = v().kout1
}
"#,
            "or outN",
        );
    }

    #[test]
    fn block_rate_proc_kout_alias_conflicts_with_event_name() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  kouts {
    level
  }
  init {
    held = 0.0
  }
  event kout1():
    held = 1.0
  block {
    level = held
  }
}

kouts {
  meter
}

init {
  m = Meter()
}

block {
  meter = m()
}
"#,
            "event name conflicts",
        );
    }

    #[test]
    fn block_rate_proc_operator_can_be_called_from_proc_block() {
        let src = r#"
proc Meter
{
  kouts<i32> {
    kout1
  }
  block {
    kout1 = 2
  }
}

proc Outer
{
  kouts<i32> {
    kout1
  }
  init {
    m = Meter()
  }
  block {
    kout1 = m()
  }
}

kouts {
  meter: i32
}

init {
  o = Outer()
}

block {
  meter = o()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("nested block-rate proc operator should analyze");

        assert!(typed.outs.is_empty());
        assert_eq!(typed.control_outs, vec!["meter"]);
    }

    #[test]
    fn block_rate_proc_operator_is_rejected_from_proc_sample() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  kouts {
    kout1
  }
  block {
    kout1 = 2.0
  }
}

proc Voice
{
  outs {
    out1
  }
  init {
    m = Meter()
  }
  sample {
    out1 = m()
  }
}

outs {
  out1
}

init {
  v = Voice()
}

sample {
  out1 = v()
}
"#,
            "for block-rate proc is only allowed in block",
        );
    }

    #[test]
    fn proc_infers_numbered_params() {
        let src = r#"
proc Voice
{
  sample {
    out1 = param2
  }
}

init {
  v = Voice(param2 = 0.25)
}

sample {
  out1 = v()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("implicit proc paramN should analyze");
    }

    #[test]
    fn proc_rejects_numbered_kins() {
        assert_analyze_error_contains(
            r#"
proc Voice
{
  sample {
    out1 = kin1
  }
}

sample {
  out1 = Voice()
}
"#,
            "unknown symbol 'kin1'",
        );
    }

    #[test]
    fn proc_rejects_dynamic_kins() {
        assert_analyze_error_contains(
            r#"
proc Voice
{
  sample {
    out1 = kins[0]
  }
}

sample {
  out1 = Voice()
}
"#,
            "'kins[i]' requires",
        );
    }

    #[test]
    fn proc_infers_numbered_kouts_and_block_timing() {
        let src = r#"
proc Meter
{
  block {
    kout1 = param1
  }
}

init {
  m = Meter(param1 = 0.5)
}

kouts {
  meter
}

block {
  meter = m()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("implicit proc koutN should analyze as block-rate proc");
    }

    #[test]
    fn nested_proc_surface_infers_child_kout_fields() {
        let src = r#"
proc Meter
{
  block {
    kout1 = 2.0
  }
}

proc Outer
{
  outs {
    out1
  }
  init {
    m = Meter()
  }
  sample {
    out1 = m.kout1
  }
}

outs {
  out1
}

init {
  o = Outer()
}

sample {
  out1 = o()
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("implicit child kout field should analyze");
    }

    #[test]
    fn proc_rejects_explicit_outs_with_inferred_kouts() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  outs {
    out1
  }
  block {
    kout1 = 1.0
  }
}

sample {
  out1 = Meter()
}
"#,
            "cannot mix outs and inferred control koutN outputs",
        );
    }

    #[test]
    fn proc_dynamic_kouts_indexes_control_outputs() {
        let src = r#"
proc Meter
{
  kouts 2
  block {
    kouts[1] = param1
  }
}

init {
  m = Meter(param1 = 0.5)
}

kouts {
  meter
}

block {
  meter = m().kout2
}
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc dynamic kouts should analyze");
    }

    #[test]
    fn top_level_graph_rejects_block_timed_outputs() {
        assert_analyze_error_contains(
            r#"
kouts {
  meter
}

graph {
  1.0 >> meter
}
"#,
            "top-level graph block does not support kouts",
        );
    }

    #[test]
    fn graph_source_rejects_block_rate_proc_output() {
        assert_analyze_error_contains(
            r#"
proc Meter
{
  kouts {
    kout1
  }
  block {
    kout1 = 2.0
  }
}

outs {
  out1
}

init {
  m = Meter()
}

graph {
  m.kout1 >> out1
}
"#,
            "graph source cannot read block-rate processor output",
        );
    }

    #[test]
    fn nested_block_local_is_not_visible_in_sample() {
        let src =
            "outs:\n  out1\nblock:\n  if true:\n    nested = 1.0\n  sample:\n    out1 = nested\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("nested block local should not escape into sample");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'nested'")),
            "missing unknown-symbol diagnostic for nested block local: {errors:#?}"
        );
    }

    #[test]
    fn proc_init_branch_local_can_feed_top_level_proc_state() {
        let src = "proc Voice:\n  outs:\n    out1\n  init:\n    if true:\n      tmp = 1.0\n    else:\n      tmp = 2.0\n    carried = tmp\n  sample:\n    out1 = carried\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc init branch-local value should feed later proc state");
    }

    #[test]
    fn proc_block_pre_top_level_state_is_visible_in_sample_and_post() {
        let src = "proc Voice:\n  outs:\n    out1\n  block:\n    pre_root = 1.0\n    sample:\n      out1 = pre_root\n    post_seen = pre_root\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc block pre vars should be visible in sample and post");
    }

    #[test]
    fn proc_block_pre_cannot_read_dynamic_inputs() {
        let src = "proc Voice:\n  ins 1\n  outs:\n    out1\n  block:\n    held = ins[0]\n    sample:\n      out1 = held\nins:\n  in1\nouts:\n  out1\ninit:\n  voice = Voice()\nsample:\n  out1 = voice(in1)\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("proc block pre dynamic input read should fail");
        assert!(
            errors.iter().any(|diag| diag.message.contains(
                "audio input 'ins' can only be read in sample; move this read into the block's nested sample section"
            )),
            "missing proc dynamic-input diagnostic for block pre: {errors:#?}"
        );
    }

    #[test]
    fn proc_nested_block_local_is_not_visible_in_sample() {
        let src = "proc Voice:\n  outs:\n    out1\n  block:\n    if true:\n      nested = 1.0\n    sample:\n      out1 = nested\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors =
            analyze(program).expect_err("nested proc block local should not escape into sample");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'nested'")),
            "missing unknown-symbol diagnostic for nested proc block local: {errors:#?}"
        );
    }

    #[test]
    fn event_branch_local_can_feed_later_event_state_write() {
        let src = "outs:\n  out1\ninit:\n  phase = 0.0\nevents:\n  ping():\n    if true:\n      tmp = 1.0\n    else:\n      tmp = 2.0\n    phase = tmp\nsample:\n  out1 = phase\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("event branch-local should feed later event state write");
    }

    #[test]
    fn individual_event_syntax_merges_with_events_block_during_analysis() {
        let src = "outs:\n  out1\nevent ping(x: i32):\n  phase = f32(x)\nevents:\n  reset():\n    phase = 0.0\ninit:\n  phase = 0.0\nsample:\n  out1 = phase\n";
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("merged event syntax should analyze");
        assert_eq!(typed.events.len(), 2);
        assert_eq!(typed.events[0].name, "ping");
        assert_eq!(typed.events[1].name, "reset");
    }

    #[test]
    fn event_loop_local_does_not_escape_loop() {
        let src = "outs:\n  out1\ninit:\n  phase = 0.0\nevents:\n  ping():\n    for i in 0..2:\n      tmp = f32(i)\n    phase = tmp\nsample:\n  out1 = phase\n";
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("event loop-local symbol should not escape loop");
        assert!(
            errors
                .iter()
                .any(|diag| diag.message.contains("unknown symbol 'tmp'")),
            "missing unknown-symbol diagnostic for escaped event loop local: {errors:#?}"
        );
    }

    #[test]
    fn proc_event_branch_local_can_feed_later_proc_state_write() {
        let src = "proc Voice:\n  outs:\n    out1\n  init:\n    phase = 0.0\n  events:\n    ping():\n      if true:\n        tmp = 1.0\n      else:\n        tmp = 2.0\n      phase = tmp\n  sample:\n    out1 = phase\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("proc event branch-local should feed later proc state write");
    }

    #[test]
    fn generic_proc_event_scalar_params_specialize_with_defaults() {
        let src = r#"
proc Filter<T>:
  outs:
    out1
  init<T>:
    freq = 0.0
    rq = 0.0
  events:
    set(freqv: T = 1200.0, rqv: T = 1.0):
      freq = freqv
      rq = rqv
  sample:
    out1 = f32(freq + rq)

outs:
  out1
init:
  a = Filter<f32>()
  b = Filter<f64>()
  a.set()
  b.set(rqv = 0.5)
sample:
  out1 = a() + b()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("generic scalar proc event params should specialize");
    }

    #[test]
    fn generic_proc_event_fixed_array_params_specialize_with_defaults() {
        let src = r#"
proc Loader<T>:
  outs:
    out1
  init<T>:
    sum = 0.0
  events:
    load(values: T[2] = [1.0, 2.0]):
      sum = values[0] + values[1]
  sample:
    out1 = f32(sum)

outs:
  out1
init:
  loader = Loader<f64>()
  loader.load()
sample:
  out1 = loader()
"#;
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("generic fixed-array proc event params should specialize");
    }

    #[test]
    fn individual_proc_event_syntax_merges_with_proc_events_block_during_analysis() {
        let src = "proc Voice:\n  outs:\n    out1\n  event ping(x: i32):\n    phase = f32(x)\n  events:\n    reset():\n      phase = 0.0\n  init:\n    phase = 0.0\n  sample:\n    out1 = phase\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n";
        let program = parse_program(src).expect("parse should succeed");
        analyze(program).expect("merged proc event syntax should analyze");
    }

    #[test]
    fn runtime_defs_reject_direct_recursion_as_unbounded_realtime_work() {
        let src = r#"
def recurse(x: f32) -> f32:
  return recurse(x)

sample:
  out1 = recurse(0.0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("recursive runtime def should fail");
        assert!(errors.iter().any(|diagnostic| diagnostic
            .message
            .contains("recursive runtime def cycle is not realtime-safe: recurse -> recurse")));
    }

    #[test]
    fn runtime_defs_reject_mutual_recursion_as_unbounded_realtime_work() {
        let src = r#"
def first(x: f32) -> f32:
  return second(x)

def second(x: f32) -> f32:
  return first(x)

sample:
  out1 = first(0.0)
"#;
        let program = parse_program(src).expect("parse should succeed");
        let errors = analyze(program).expect_err("mutually recursive runtime defs should fail");
        assert!(errors.iter().any(|diagnostic| {
            diagnostic.message.contains(
                "recursive runtime def cycle is not realtime-safe: first -> second -> first",
            )
        }));
    }

    #[test]
    fn typed_program_resolves_dynamic_interface_views_to_concrete_slots() {
        let src = r#"
ins:
  dry: f32 = 0.0
  bands: f32[2] = [0.0, 0.0]

outs:
  main: f32
  pair: f32[2]

kouts:
  meter: f32
  leds: f32[2]

params:
  gain: f32 = 0.5
  controls: f32[2] = [0.25, 0.75]

block:
  kouts[0] = params[0]
  sample:
    outs[0] = ins[0]
    outs[1] = ins[1] * params[1]
    outs[2] = ins[2] * params[2]
"#;
        let program = parse_program(src).expect("parse should succeed");
        let typed = analyze(program).expect("uniform interface views should resolve");

        let inputs = typed.interface_views.inputs.as_ref().expect("input view");
        assert_eq!(inputs.element_type, PrimitiveType::F32);
        assert_eq!(
            inputs
                .slots
                .iter()
                .map(|slot| (slot.id.raw(), slot.root.as_str(), slot.element))
                .collect::<Vec<_>>(),
            vec![
                (0, "dry", None),
                (1, "bands", Some(0)),
                (2, "bands", Some(1))
            ]
        );
        let outputs = typed
            .interface_views
            .audio_outputs
            .as_ref()
            .expect("audio output view");
        assert_eq!(outputs.slots[0].root, "main");
        assert_eq!(outputs.slots[1].root, "pair");
        assert_eq!(outputs.slots[1].element, Some(0));
        assert_eq!(outputs.slots[2].element, Some(1));
        assert_eq!(
            typed
                .interface_views
                .control_outputs
                .as_ref()
                .expect("control output view")
                .slots
                .len(),
            3
        );
        assert_eq!(
            typed
                .interface_views
                .params
                .as_ref()
                .expect("parameter view")
                .slots
                .len(),
            3
        );
    }

    #[test]
    fn generic_buffer_specialization_preserves_declared_channel_contract() {
        let src = r#"
def channels<T>(buf: buffer<T>):
  return 1

buffers:
  stereo: f32[2]

sample:
  out1 = f32(channels(stereo))
"#;
        let program = parse_program(src).expect("source should parse");
        let errors = analyze(program).expect_err("mono buffer contract should reject stereo");
        assert!(errors.iter().any(|diagnostic| {
            diagnostic.message.contains("expects mono buffer")
                && diagnostic.message.contains("stereo")
        }));

        let symbolic = r#"
const Channels = 2

def read_right<T>(buf: buffer<T[Channels]>):
  return buf[1, 0]

buffers:
  stereo: f32[2]

sample:
  out1 = read_right(stereo)
"#;
        let typed = analyze(parse_program(symbolic).expect("source should parse"))
            .expect("symbolic channel contract should specialize");
        let specialization = typed
            .defs
            .iter()
            .find(|function| function.name.starts_with("read_right.__onda_mono"))
            .expect("missing symbolic buffer specialization");
        assert!(matches!(
            specialization.param_kinds.as_slice(),
            [TypedFnParam::Buffer {
                elem_ty: PrimitiveType::F32,
                channels: TypedBufferChannels::Static(2),
            }]
        ));
    }

    #[test]
    fn structural_buffer_collection_specialization_preserves_collection_length() {
        let src = r#"
def collection_len(buffers):
  return buffers.len()

buffers:
  bank: f32 {3}

sample:
  out1 = f32(collection_len(bank))
"#;
        let typed = analyze(parse_program(src).expect("source should parse"))
            .expect("buffer collection should specialize as a collection");
        let specialization = typed
            .defs
            .iter()
            .find(|function| function.name.starts_with("collection_len.__onda_mono"))
            .expect("missing buffer collection specialization");
        assert!(matches!(
            specialization.param_kinds.as_slice(),
            [TypedFnParam::BufferArray {
                elem_ty: PrimitiveType::F32,
                channels: TypedBufferChannels::Mono,
                len: 3,
            }]
        ));
        lower_program_to_optimized_mir(&typed)
            .expect("buffer collection specialization should lower to MIR");
    }

    #[test]
    fn nested_struct_array_return_type_selects_scalar_overload() {
        let src = r#"
struct Item:
  value: i32

def read_first(items):
  return items[0].value

def classify(value: f32):
  return 1

def classify(value: i32):
  return 2

init:
  items: Item[1] = [Item(value = 7)]

sample:
  out1 = f32(classify(read_first(items)))
"#;
        let program = parse_program(src).expect("source should parse");
        analyze(program).expect("struct-array specialization should publish its return type");
    }

    #[test]
    fn indexed_nominal_elements_specialize_structural_parameters() {
        let src = r#"
struct Item:
  value: i32

def read(item):
  return item.value

def read_first(items):
  item = items[0]
  return read(item)

init:
  items: Item[2] = [Item(value = 1), Item(value = 2)]

sample:
  item = items[1]
  out1 = f32(read(item) + read_first(items))
"#;
        let typed = analyze(parse_program(src).expect("source should parse"))
            .expect("indexed nominal arguments should specialize structurally");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name.starts_with("read.__onda_mono")));
        lower_program_to_optimized_mir(&typed)
            .expect("indexed nominal specialization should lower to MIR");
    }

    #[test]
    fn nested_proc_array_return_type_selects_scalar_overload() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

def read_at(voices, index: i32):
  return voices[index].gain

def read_at(values: f32[], index: i32):
  return values[index]

def classify(value: f32):
  return 1

def classify(value: i32):
  return 2

init:
  voices: Voice[2] = Voice()

sample:
  out1 = f32(classify(read_at(voices, 1)))
"#;
        let program = parse_program(src).expect("source should parse");
        let typed =
            analyze(program).expect("proc-array specialization should publish its return type");
        lower_program_to_optimized_mir(&typed)
            .expect("dynamic proc-array field reads should lower to MIR");
    }

    #[test]
    fn explicitly_typed_proc_array_views_specialize_for_each_capacity() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

def read_first(voices: Voice[]):
  return voices[0].gain

def read_outer(voices: Voice[]):
  return read_first(voices)

init:
  pair: Voice[2] = Voice(gain = 1.0)
  trio: Voice[3] = Voice(gain = 2.0)

sample:
  out1 = read_outer(pair) + read_outer(trio)
"#;
        let program = parse_program(src).expect("typed proc-array source should parse");
        let typed = analyze(program)
            .expect("a proc-array view should specialize independently for each capacity");
        for base in ["read_first", "read_outer"] {
            let capacities = typed
                .defs
                .iter()
                .filter(|function| function.name.starts_with(&format!("{base}.__onda_mono")))
                .filter_map(|function| match function.param_kinds.first() {
                    Some(TypedFnParam::ProcArray { len, .. }) => Some(*len),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            assert_eq!(capacities, HashSet::from([2, 3]), "{base}");
        }
        lower_program_to_optimized_mir(&typed)
            .expect("capacity-specialized proc-array calls should lower to MIR");
    }

    #[test]
    fn fixed_proc_array_parameters_are_concrete_without_monomorphization() {
        let src = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

def read_pair(voices: Voice[2]):
  return voices[0].gain + voices[1].gain

init:
  voices: Voice[2] = Voice(gain = 1.0)

sample:
  out1 = read_pair(voices)
"#;
        let program = parse_program(src).expect("fixed proc-array source should parse");
        let typed = analyze(program)
            .expect("a fixed proc-array signature should have a complete source ABI");
        let function = typed
            .defs
            .iter()
            .find(|function| function.name == "read_pair")
            .expect("the concrete function should retain its source name");
        assert!(matches!(
            function.param_kinds.first(),
            Some(TypedFnParam::ProcArray {
                proc_name,
                len: 2,
            }) if proc_name == "Voice"
        ));
        assert!(!typed
            .defs
            .iter()
            .any(|function| function.name.starts_with("read_pair.__onda_mono")));
        lower_program_to_optimized_mir(&typed)
            .expect("the concrete proc-array function should lower to MIR");
    }

    #[test]
    fn structural_data_array_specialization_is_independent_of_runtime_length() {
        let src = r#"
struct Item:
  value: f32

def read_first(items):
  return items[0].value

init:
  pair: Item[2] = [Item(value = 1.0), Item(value = 2.0)]
  trio: Item[3] = [Item(value = 3.0), Item(value = 4.0), Item(value = 5.0)]

sample:
  out1 = read_first(pair) + read_first(trio)
"#;
        let program = parse_program(src).expect("struct-array source should parse");
        let typed = analyze(program)
            .expect("data-struct array views should share one structural specialization");
        let specializations = typed
            .defs
            .iter()
            .filter(|function| function.name.starts_with("read_first.__onda_mono"))
            .collect::<Vec<_>>();
        assert_eq!(specializations.len(), 1, "{specializations:#?}");
        assert!(matches!(
            specializations[0].param_kinds.first(),
            Some(TypedFnParam::StructArray { struct_name }) if struct_name == "Item"
        ));
        lower_program_to_optimized_mir(&typed)
            .expect("the shared struct-array specialization should lower to MIR");
    }

    #[test]
    fn sized_array_overloads_match_length_and_have_source_like_diagnostics() {
        let valid = r#"
def choose(values: f32[2]):
  return 2

def choose(values: f32[3]):
  return 3

init:
  values: f32[2] = [1.0, 2.0]

sample:
  out1 = f32(choose(values))
"#;
        analyze(parse_program(valid).expect("source should parse"))
            .expect("fixed array length should select a unique overload");

        let invalid = valid.replace(
            "values: f32[2] = [1.0, 2.0]",
            "values: f32[4] = [1.0, 2.0, 3.0, 4.0]",
        );
        let errors = analyze(parse_program(&invalid).expect("source should parse"))
            .expect_err("an unmatched fixed length should fail");
        let message = errors
            .iter()
            .find(|diagnostic| diagnostic.message.contains("no matching overload"))
            .map(|diagnostic| diagnostic.message.as_str())
            .expect("missing overload diagnostic");
        assert!(message.contains("f32[2]") && message.contains("f32[3]"));
        assert!(!message.contains("Span") && !message.contains("Expr::"));
    }

    #[test]
    fn concrete_array_overload_outranks_generic_array_overload() {
        let src = r#"
def choose<T>(values: T[]):
  return 1

def choose(values: f32[]):
  return 2

init:
  values: f32[2] = [1.0, 2.0]

sample:
  out1 = f32(choose(values))
"#;
        analyze(parse_program(src).expect("source should parse"))
            .expect("the concrete array overload should win without ambiguity");
    }

    #[test]
    fn sized_array_overload_outranks_unsized_array_overload() {
        for generic in [false, true] {
            let type_params = if generic { "<T>" } else { "" };
            let elem = if generic { "T" } else { "f32" };
            let src = format!(
                r#"
def choose{type_params}(values: {elem}[]):
  return 1

def choose{type_params}(values: {elem}[2]):
  return 2

init:
  values: f32[2] = [1.0, 2.0]

sample:
  out1 = f32(choose(values))
"#
            );
            analyze(parse_program(&src).expect("source should parse"))
                .expect("the fixed-shape overload should win without ambiguity");
        }
    }

    #[test]
    fn overload_matching_unifies_repeated_generic_type_parameters() {
        let src = r#"
def choose<T>(left: T, right: T):
  return 1

def choose(left: bool, right: bool):
  return 2

sample:
  out1 = f32(choose(f64(1.0), true))
"#;
        let errors = analyze(parse_program(src).expect("source should parse"))
            .expect_err("the generic candidate has no consistent type binding");
        assert!(errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("no matching overload")));

        let aggregate_constraint = r#"
def choose<T>(values: T[], fallback: T):
  return fallback

def choose(values: bool[], fallback: bool):
  return fallback

init:
  values: f32[2] = [1.0, 2.0]

sample:
  out1 = choose(values, 1)
"#;
        analyze(parse_program(aggregate_constraint).expect("source should parse"))
            .expect("an exact aggregate binding should contextually convert scalar literals");
    }

    #[test]
    fn monomorphized_nominal_symbols_do_not_collide_after_sanitization() {
        let src = r#"
namespace A:
  struct B:
    value: i32

struct A__B:
  value: i32

def read(item):
  return item.value

init:
  left = A::B(value = 1)
  right = A__B(value = 2)

sample:
  out1 = f32(read(left) + read(right))
"#;
        analyze(parse_program(src).expect("source should parse"))
            .expect("distinct nominal types should have distinct mono symbols");
    }

    #[test]
    fn direct_array_calls_enforce_element_type_and_fixed_length_semantically() {
        let wrong_element = r#"
def first(values: f32[]):
  return values[0]

init:
  values: i32[2] = [1, 2]

sample:
  out1 = f32(first(values))
"#;
        let errors = analyze(parse_program(wrong_element).expect("source should parse"))
            .expect_err("array element mismatch should fail semantic analysis");
        assert!(errors.iter().any(|diagnostic| diagnostic
            .message
            .contains("expects f32 array elements, got i32")));

        let wrong_length = r#"
def first<T>(values: T[2]):
  return values[0]

init:
  values: f32[3] = [1.0, 2.0, 3.0]

sample:
  out1 = first(values)
"#;
        let errors = analyze(parse_program(wrong_length).expect("source should parse"))
            .expect_err("specialized fixed-array length mismatch should fail semantic analysis");
        assert!(errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("expects array length 2, got 3")));

        let wrong_nominal_kind = r#"
struct Item:
  value: f32

def first(values: Item[]):
  return values[0].value

init:
  values: f32[1] = [1.0]

sample:
  out1 = first(values)
"#;
        let errors = analyze(parse_program(wrong_nominal_kind).expect("source should parse"))
            .expect_err("primitive arrays must not satisfy nominal array parameters");
        assert!(errors.iter().any(|diagnostic| diagnostic
            .message
            .contains("expects Item array elements, got f32")));

        let unknown_nested_length = r#"
struct Item:
  value: f32

def exactly_one(values: Item[1]):
  return values[0].value

def forward(values: Item[]):
  return exactly_one(values)

init:
  values: Item[2] = [Item(value = 1.0), Item(value = 2.0)]

sample:
  out1 = forward(values)
"#;
        let errors = analyze(parse_program(unknown_nested_length).expect("source should parse"))
            .expect_err("an unsized nominal view must not satisfy a fixed-length contract");
        assert!(errors.iter().any(|diagnostic| diagnostic.message.contains(
            "expects fixed array length 1, but the argument length is not statically known"
        )));
    }

    #[test]
    fn untyped_array_literal_specialization_merges_all_element_types() {
        for values in ["[1, 2.5]", "[2.5, 1]"] {
            let source = format!(
                r#"
def first(values: []):
  return values[0]

sample:
  out1 = first({values})
"#
            );
            let typed = analyze(parse_program(&source).expect("source should parse"))
                .unwrap_or_else(|errors| {
                    panic!("array literal '{values}' should infer f32 elements: {errors:?}")
                });
            assert!(typed
                .defs
                .iter()
                .any(|function| function.name == "first.__onda_mono__arr_f32"));
            lower_program_to_optimized_mir(&typed)
                .expect("the common array element type should lower to MIR");
        }
    }

    #[test]
    fn fixed_primitive_array_lengths_survive_def_forwarding() {
        let src = r#"
def first(values: f32[2]):
  return values[0]

def forward(values: f32[2]):
  return first(values)

init:
  values: f32[2] = [1.0, 2.0]

sample:
  out1 = forward(values)
"#;
        let typed = analyze(parse_program(src).expect("source should parse"))
            .expect("fixed primitive array contracts should survive forwarding");
        lower_program_to_optimized_mir(&typed)
            .expect("forwarded fixed primitive arrays should lower to MIR");
    }

    #[test]
    fn compile_time_call_shapes_are_resolved_before_overload_inference() {
        let arrays = r#"
def classify(values: f32[1 + 1]):
  return 2

def classify(values: f32[1 + 2]):
  return 3

def forward(values: f32[1 + 1]):
  return classify(values)

def local():
  values: f32[1 + 1] = [0.0, 0.0]
  return classify(values)

init:
  held = 0
  values: f32[2] = [1.0, 2.0]

events:
  update(event_values: f32[1 + 1]):
    held = classify(event_values)

sample:
  out1 = f32(forward(values) + local() + held)
"#;
        let typed = analyze(parse_program(arrays).expect("array source should parse"))
            .expect("array size expressions should participate in overload resolution");
        lower_program_to_optimized_mir(&typed)
            .expect("resolved array call shapes should lower to MIR");

        let buffers = r#"
def channel_count(buf: buffer<f32[1 + 1]>):
  return 2

def channel_count(buf: buffer<f32[1 + 2]>):
  return 3

buffers:
  stereo: f32[2]

sample:
  out1 = f32(channel_count(stereo))
"#;
        let typed = analyze(parse_program(buffers).expect("buffer source should parse"))
            .expect("buffer channel expressions should participate in overload resolution");
        lower_program_to_optimized_mir(&typed)
            .expect("resolved buffer call shapes should lower to MIR");
    }

    #[test]
    fn unsized_primitive_views_do_not_satisfy_fixed_array_contracts() {
        let forwarded = r#"
def exactly_one(values: f32[1]):
  return values[0]

def forward(values: f32[]):
  return exactly_one(values)

init:
  values: f32[2] = [1.0, 2.0]

sample:
  out1 = forward(values)
"#;
        let errors = analyze(parse_program(forwarded).expect("source should parse"))
            .expect_err("an unsized parameter must not become a one-element fixed array");
        assert!(errors.iter().any(|diagnostic| diagnostic.message.contains(
            "expects fixed array length 1, but the argument length is not statically known"
        )));

        let sliced = r#"
def exactly_one(values: f32[1]):
  return values[0]

params:
  end: i32 = 1

init:
  values: f32[2] = [1.0, 2.0]

sample:
  view = values[0:end]
  out1 = exactly_one(view)
"#;
        let errors = analyze(parse_program(sliced).expect("source should parse"))
            .expect_err("a slice alias must remain an unsized view");
        assert!(errors.iter().any(|diagnostic| diagnostic.message.contains(
            "expects fixed array length 1, but the argument length is not statically known"
        )));
    }

    #[test]
    fn branch_local_array_lengths_do_not_become_arbitrary_fixed_contracts() {
        let source = r#"
def exactly_two(values: f32[2]):
  return values[0]

params:
  select: bool = true

sample:
  if select:
    values: f32[2] = [1.0, 2.0]
  else:
    values: f32[3] = [1.0, 2.0, 3.0]
  out1 = exactly_two(values)
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("one branch's array length must not become the joined fixed contract");
        assert!(errors.iter().any(|diagnostic| diagnostic.message.contains(
            "binding 'values' has incompatible branch types: arrays have different element types or fixed lengths"
        )));
    }

    #[test]
    fn identical_branch_local_array_shapes_survive_with_their_element_type() {
        let source = r#"
def first(values: []):
  return values[0]

params:
  select: bool = true

sample:
  if select:
    values = [PI]
  else:
    values = [1.0]
  out1 = first(values)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("identical branch-local arrays should retain one concrete shape");
        assert!(typed
            .defs
            .iter()
            .any(|function| function.name == "first.__onda_mono__arr_f32"));
        lower_program_to_optimized_mir(&typed)
            .expect("compatible branch-local arrays should lower through their merged binding");
    }

    #[test]
    fn branch_local_struct_element_aliases_preserve_the_selected_element() {
        let source = r#"
struct Item:
  value: f32

params:
  select: bool = true

init:
  items: Item[2] = [Item(value = 1.0), Item(value = 2.0)]

sample:
  if select:
    item = items[0]
  else:
    item = items[1]
  out1 = item.value
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("a branch-local struct alias should retain its nominal type");
        lower_program_to_optimized_mir(&typed)
            .expect("a branch-local struct alias should retain its selected runtime element");
    }

    #[test]
    fn i32_and_i64_ranged_bindings_reach_mir_storage_and_eliminate_fixed_bounds() {
        let source = r#"
init:
  values: f32[8]
  index: i32 = 7 {0..8, wrap}
  wide: i64 = 9007199254740993 {9007199254740992..9007199254740996}

sample:
  values[index] = 1.0
  index += 1
  wide += 1
  out1 = values[index]
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("ranged bindings should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("ranged bindings should lower to optimized MIR");
        let index = mir
            .state
            .iter()
            .find(|state| state.name == "index")
            .and_then(|state| state.integer_range)
            .expect("i32 state should retain its integer range");
        assert_eq!(index.min, onda_mir::ScalarValue::I32(0));
        assert_eq!(index.max, onda_mir::ScalarValue::I32(7));
        assert_eq!(index.mode, onda_mir::IntegerRangeMode::Wrap);
        let wide = mir
            .state
            .iter()
            .find(|state| state.name == "wide")
            .and_then(|state| state.integer_range)
            .expect("i64 state should retain its integer range");
        assert_eq!(wide.min, onda_mir::ScalarValue::I64(9_007_199_254_740_992));
        assert_eq!(wide.max, onda_mir::ScalarValue::I64(9_007_199_254_740_995));
    }

    #[test]
    fn namespace_template_integer_binding_range_bounds_reach_mir() {
        let source = r#"
namespace Ring<Begin = 4, Size = 8>:
  proc Cursor:
    outs:
      out1
    init:
      cursor: i32 = Begin {range = Begin..=Begin + Size - 1, mode = wrap}
    sample:
      cursor += 1
      out1 = f32(cursor)

outs:
  out1
init:
  cursor = Ring<3, 8>::Cursor()
sample:
  out1 = cursor()
"#;

        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("namespace template integers should be valid binding-range bounds");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("namespace template integer bounds should lower to optimized MIR");
        let ranges = mir
            .state
            .iter()
            .filter_map(|state| state.integer_range)
            .collect::<Vec<_>>();

        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].min, onda_mir::ScalarValue::I32(3));
        assert_eq!(ranges[0].max, onda_mir::ScalarValue::I32(10));
        assert_eq!(ranges[0].mode, onda_mir::IntegerRangeMode::Wrap);
    }

    #[test]
    fn ranged_struct_fields_reach_flattened_state_and_method_parameters() {
        let source = r#"
struct Ring:
  values: f32[8]
  index: i32 = 0 {8, wrap}

  def write(self, value: f32):
    self.values[self.index] = value
    self.index += 1

init:
  ring = Ring(index = 15)

sample:
  ring.write(1.0)
  out1 = ring.values[ring.index]
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("ranged struct fields should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("ranged struct fields should lower to optimized MIR");
        let dump = onda_mir::format_program(mir.as_program());
        let range = mir
            .state
            .iter()
            .find(|state| state.name == "ring.index")
            .and_then(|state| state.integer_range)
            .expect("flattened struct state should retain its integer range");
        assert_eq!(range.min, onda_mir::ScalarValue::I32(0), "{dump}");
        assert_eq!(range.max, onda_mir::ScalarValue::I32(7), "{dump}");
        assert_eq!(range.mode, onda_mir::IntegerRangeMode::Wrap, "{dump}");
        assert!(
            mir.functions
                .iter()
                .find(|function| function.name.contains("Ring.write"))
                .and_then(|function| {
                    function
                        .params
                        .iter()
                        .find(|parameter| parameter.name == "self.index")
                })
                .and_then(|parameter| parameter.integer_range)
                .is_some(),
            "{dump}"
        );
        assert!(dump.contains("] unchecked"), "{dump}");
    }

    #[test]
    fn inferred_integer_binding_range_defaults_to_i32() {
        let source = r#"
params:
  test = 0

init:
  clamped = test {0..10}
  wrapped = test {0..10, wrap}

sample:
  clamped += 1
  wrapped += 1
  out1 = f32(clamped + wrapped)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("an inferred integer binding range should analyze");
        for statement in &typed.init {
            let Stmt::Assign {
                decl_ty,
                is_typed_decl,
                ..
            } = statement
            else {
                panic!("each init statement should remain a declaration");
            };
            assert_eq!(*decl_ty, Some(DeclType::Scalar(PrimitiveType::I32)));
            assert!(*is_typed_decl);
        }
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("inferred i32 binding ranges should lower to optimized MIR");
        for (name, expected_mode) in [
            ("clamped", onda_mir::IntegerRangeMode::Clamp),
            ("wrapped", onda_mir::IntegerRangeMode::Wrap),
        ] {
            let state = mir
                .state
                .iter()
                .find(|state| state.name == name)
                .unwrap_or_else(|| panic!("missing state '{name}'"));
            let range = state
                .integer_range
                .unwrap_or_else(|| panic!("missing integer range for '{name}'"));
            assert_eq!(range.min, onda_mir::ScalarValue::I32(0));
            assert_eq!(range.max, onda_mir::ScalarValue::I32(9));
            assert_eq!(range.mode, expected_mode);
        }

        assert_analyze_error_contains(
            r#"
init:
  source: i64 = 0
  clamped = source {0..10}

sample:
  out1 = f32(clamped)
"#,
            "cannot assign I64 to I32",
        );
    }

