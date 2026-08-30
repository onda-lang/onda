#[test]
fn parses_namespace_local_alias_with_relative_template_target() {
    let src = r#"
namespace A:
  namespace Data<S = SR>:
    struct X:
      storage: f32[S]
  namespace D = Data<SR>
  def make():
    x = D::X()
    return 0.0

sample:
  out1 = A::make()
"#;
    let program = parse_program(src).expect("local namespace alias should parse");
    let make_def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "A" => ns.items.iter().find_map(|item| match item {
                NamespaceItem::Def(d) if d.name == "make" => Some(d),
                _ => None,
            }),
            _ => None,
        })
        .expect("make def");
    let call_name = match &make_def.body[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            _ => panic!("expected constructor call"),
        },
        _ => panic!("expected assignment"),
    };
    assert_eq!(call_name, "D::X");
}

#[test]
fn parses_namespace_local_alias_to_relative_template_generic_struct_ctor() {
    let src = r#"
namespace A:
  namespace Data<S = SR>:
    struct Store<T>:
      storage: T[S]
  namespace D = Data<SR>
  def make():
    s = D::Store<f64>()
    return 0.0

sample:
  out1 = A::make()
"#;
    let program = parse_program(src).expect("local namespace alias generic ctor should parse");
    let make_def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "A" => ns.items.iter().find_map(|item| match item {
                NamespaceItem::Def(d) if d.name == "make" => Some(d),
                _ => None,
            }),
            _ => None,
        })
        .expect("make def");
    let (call_name, type_args) = match &make_def.body[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall {
                name, type_args, ..
            } => (name.clone(), type_args.clone()),
            _ => panic!("expected constructor call"),
        },
        _ => panic!("expected assignment"),
    };
    assert_eq!(call_name, "D::Store");
    assert_eq!(type_args, vec![CallTypeArg::Primitive(PrimitiveType::F64)]);
}

#[test]
fn rejects_inconsistent_indentation() {
    let src = "outs:
  out1
sample:
  if (1.0 > 0.0):
    out1 = 1.0
 out1 = 0.0
";
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "parser should reject inconsistent indentation"
    );
}

#[test]
fn parses_block_wrapped_sample_section() {
    let src = r#"
outs { out1 }
init { x = 0.0 }
block {
  x = x + 1.0
  sample {
    out1 = x
  }
  x = x + 2.0
}
"#;
    let program = parse_program(src).expect("program with wrapped block sample should parse");
    let block_exec = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Block(exec) => Some(exec),
            _ => None,
        })
        .expect("block section");
    assert_eq!(block_exec.pre.len(), 1);
    assert_eq!(block_exec.sample.as_ref().map(|s| s.len()), Some(1));
    assert_eq!(block_exec.post.len(), 1);
}

#[test]
fn parses_proc_indexed_call_expression() {
    let src = r#"
sample {
  out1 = voices[1](0.25)
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert_eq!(name, PROC_INDEX_CALL_SENTINEL);
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_INDEX_BASE_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded index base argument"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_INDEX_EXPR_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded index expression argument"
            );
        }
        _ => panic!("expected encoded proc indexed call expression"),
    }
}

#[test]
fn parses_proc_indexed_field_call_expression() {
    let src = r#"
sample {
  out1 = voices[1](0.25).out2
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert!(
                name.starts_with(PROC_FIELD_SENTINEL_PREFIX),
                "expected proc field sentinel call"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_INDEX_BASE_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded index base argument"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_INDEX_EXPR_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded index expression argument"
            );
            assert!(
                args.iter().any(|a| {
                    a.name
                        .as_ref()
                        .map(|n| n == PROC_FIELD_SENTINEL_ARG)
                        .unwrap_or(false)
                }),
                "expected encoded field argument"
            );
        }
        _ => panic!("expected encoded proc indexed field call expression"),
    }
}

#[test]
fn parses_proc_indexed_param_field_expression() {
    let src = r#"
sample {
  out1 = voices[1].gain
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert!(
                name.starts_with(PROC_FIELD_SENTINEL_PREFIX),
                "expected proc field sentinel call"
            );
            assert!(args.iter().any(|a| {
                a.name.as_deref() == Some(PROC_INDEX_BASE_ARG)
                    && matches!(a.expr, Expr::Var { name: ref base, .. } if base == "voices")
            }));
            assert!(args.iter().any(|a| {
                a.name.as_deref() == Some(PROC_INDEX_EXPR_ARG)
                    && matches!(a.expr, Expr::Int { value: 1, .. })
            }));
            assert!(args.iter().any(|a| {
                a.name.as_deref() == Some(PROC_FIELD_SENTINEL_ARG)
                    && matches!(a.expr, Expr::Var { name: ref field, .. } if field == "gain")
            }));
        }
        other => panic!("expected encoded proc indexed param field expression, got {other:?}"),
    }
}

#[test]
fn parses_proc_indexed_event_call_expression() {
    let src = r#"
sample {
  voices[idx].note_on(0.25)
}
"#;

    let program = parse_program(src).expect("parse_program should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Expr { expr, .. } = &sample[0] else {
        panic!("expected expression statement");
    };
    match expr {
        Expr::UserCall { name, args, .. } => {
            assert_eq!(name, "note_on");
            assert!(matches!(
                args.first(),
                Some(CallArg {
                    name: Some(receiver_marker),
                    expr: Expr::Index { base, .. },
                }) if receiver_marker == METHOD_RECEIVER_ARG && base == "voices"
            ));
        }
        _ => panic!("expected receiver-desugared indexed method call"),
    }
}

#[test]
fn indexed_method_syntax_preserves_a_neutral_receiver_argument() {
    let src = r#"
sample:
  out1 = sources[slot].readCW(0, position)
"#;
    let program = parse_program(src).expect("indexed receiver method should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign {
        expr: Expr::UserCall { name, args, .. },
        ..
    } = &sample[0]
    else {
        panic!("expected receiver-marked call");
    };
    assert_eq!(name, "readCW");
    assert!(matches!(
        args.first(),
        Some(CallArg {
            name: Some(receiver_marker),
            expr: Expr::Index { base, .. },
        }) if receiver_marker == METHOD_RECEIVER_ARG && base == "sources"
    ));
    assert!(args.iter().all(|arg| {
        !matches!(
            arg.name.as_deref(),
            Some(PROC_INDEX_BASE_ARG | PROC_INDEX_EXPR_ARG)
        )
    }));
}

#[test]
fn parses_sample_oversample_factor_brace_form() {
    let src = r#"
outs { out1 }
sample 4 {
  out1 = in1
}
"#;
    let program = parse_program(src).expect("sample oversample factor should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.oversample_factor, Some(Expr::int(4)));
    assert_eq!(sample.len(), 1);
}

#[test]
fn parses_sample_oversample_factor_indentation_form() {
    let src = r#"
outs:
  out1
sample 8:
  out1 = in1
"#;
    let program = parse_program(src).expect("indented sample oversample factor should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(sample.oversample_factor, Some(Expr::int(8)));
    assert_eq!(sample.len(), 1);
}

#[test]
fn parses_proc_sample_oversample_factor() {
    let src = r#"
proc OS {
  outs { out1 }
  sample 2 { out1 = in1 }
}
sample { out1 = 0.0 }
"#;
    let program = parse_program(src).expect("proc sample oversample factor should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(proc) => Some(proc),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.sample_oversample_factor, Some(Expr::int(2)));
    assert_eq!(proc.sample.len(), 1);
}

#[test]
fn parses_block_wrapped_sample_oversample_factor() {
    let src = r#"
outs { out1 }
block {
  sample 16 {
    out1 = in1
  }
}
"#;
    let program = parse_program(src).expect("wrapped sample oversample factor should parse");
    let block_exec = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Block(exec) => Some(exec),
            _ => None,
        })
        .expect("block section");
    let sample = block_exec.sample.as_ref().expect("nested sample");
    assert_eq!(sample.oversample_factor, Some(Expr::int(16)));
    assert_eq!(sample.len(), 1);
}

#[test]
fn parses_array_capacity_expression() {
    let src = r#"
outs { out1 }
struct Delay { buf: f32[SR * 2] }
init {
  d = Delay()
  b: f32[BLOCK_SIZE + 4]
}
sample {
  out1 = d.buf[0] + b[0]
}
"#;
    let program = parse_program(src).expect("program with array capacity expressions should parse");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Struct(_))));
}

#[test]
fn parses_typed_array_syntax_and_f32_alias() {
    let src = r#"
outs { out1 }
struct Delay {
  wide: f64[SR * 2]
  mono: f32[64]
}
init {
  a: i32[BLOCK_SIZE + 1]
  b: f32[8]
}
sample {
  out1 = 0.0
}
"#;
    let program = parse_program(src).expect("typed array syntax should parse");

    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) => Some(s),
            _ => None,
        })
        .expect("struct block");

    match &st.fields[0].ty {
        FieldType::Array(spec) => {
            assert!(matches!(
                spec.elem,
                ArrayElemType::Primitive(crate::ast::PrimitiveType::F64)
            ));
        }
        _ => panic!("expected array field type"),
    }
    match &st.fields[1].ty {
        FieldType::Array(spec) => {
            assert!(matches!(
                spec.elem,
                ArrayElemType::Primitive(crate::ast::PrimitiveType::F32)
            ));
        }
        _ => panic!("expected array field type"),
    }

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");

    match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::ArrayCtor { spec, .. } => {
                assert!(matches!(
                    spec.elem,
                    ArrayElemType::Primitive(crate::ast::PrimitiveType::I32)
                ));
            }
            _ => panic!("expected array constructor"),
        },
        _ => panic!("expected assignment"),
    }
    match &init[1] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::ArrayCtor { spec, .. } => {
                assert!(matches!(
                    spec.elem,
                    ArrayElemType::Primitive(crate::ast::PrimitiveType::F32)
                ));
            }
            _ => panic!("expected array constructor"),
        },
        _ => panic!("expected assignment"),
    }
}

#[test]
fn parses_struct_array_typed_field_in_indentation_and_brace_forms() {
    let src_indent = r#"
outs:
  out1

struct Tap:
  x: f32

struct Voice:
  taps: Tap[3]

sample:
  out1 = 0.0
"#;
    let program_indent = parse_program(src_indent).expect("indentation Struct[N] should parse");
    assert!(
        program_indent
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Struct(_))),
        "expected struct blocks in indentation source"
    );

    let src_brace = r#"
outs { out1 }
struct Tap { x: f32 }
struct Voice { taps: Tap[3] }
sample { out1 = 0.0 }
"#;
    let program_brace = parse_program(src_brace).expect("brace Struct[N] should parse");
    assert!(
        program_brace
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Struct(_))),
        "expected struct blocks in brace source"
    );
}

#[test]
fn parses_array_type_sugar_in_struct_fields_and_init_typed_decls() {
    let src = r#"
outs { out1 }
struct Voice { x: f32 }
struct Bank {
  taps: f32[4]
  voices: Voice[2]
}
init {
  a: f32[8]
  b: Voice[2]
}
sample {
  out1 = a[0]
}
"#;
    let program = parse_program(src).expect("array type sugar should parse");

    let bank = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "Bank" => Some(s),
            _ => None,
        })
        .expect("Bank struct");
    assert_eq!(bank.fields.len(), 2);
    match &bank.fields[0].ty {
        FieldType::Array(spec) => {
            assert!(matches!(
                spec.elem,
                ArrayElemType::Primitive(crate::ast::PrimitiveType::F32)
            ));
        }
        _ => panic!("expected array field from f32[4] sugar"),
    }
    match &bank.fields[1].ty {
        FieldType::Array(spec) => {
            assert!(matches!(spec.elem, ArrayElemType::Struct(ref s) if s == "Voice"));
        }
        _ => panic!("expected array field from Voice[2] sugar"),
    }

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.len(), 2);
    for stmt in init {
        match stmt {
            Stmt::Assign { decl_ty, expr, .. } => {
                assert!(decl_ty.is_none(), "array sugar should lower to array ctor");
                assert!(
                    matches!(expr, Expr::ArrayCtor { .. }),
                    "array sugar should emit array constructor"
                );
            }
            _ => panic!("expected assignment in init"),
        }
    }
}

#[test]
fn rejects_non_array_literal_typed_array_initializer_expression() {
    let src = r#"
outs { out1 }
init {
  a: f32[4] = 1.0
}
sample {
  out1 = 0.0
}
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "typed array declaration with non-array initializer should be rejected"
    );
}

#[test]
fn parses_typed_array_initializer_expression() {
    let src = r#"
outs { out1 }
init {
  a: f32[2] = [1.0, 2.0]
}
sample {
  out1 = a[0]
}
"#;
    let program = parse_program(src).expect("typed array initializer should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign { expr, .. } = &init[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::ArrayCtor {
            init: Some(values), ..
        } => assert_eq!(values.len(), 2),
        _ => panic!("expected ArrayCtor with array initializer"),
    }
}

#[test]
fn identifiers_starting_with_const_are_not_parsed_as_const_declarations() {
    let src = r#"
outs:
  out1
init:
  constructed: f32[1] = [1.0]
sample:
  out1 = constructed[0]
"#;
    let program = parse_program(src).expect("const-prefixed identifier should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");
    assert!(matches!(
        init.as_slice(),
        [Stmt::Assign {
            target: AssignTarget::Var(name),
            ..
        }] if name == "constructed"
    ));
}

#[test]
fn parses_untyped_array_literal_assignment_expression() {
    let src = r#"
outs { out1 }
sample {
  b = [1, 2, 3]
  out1 = b[1]
}
"#;
    let program = parse_program(src).expect("untyped array literal assignment should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sb) => Some(&sb.body),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign {
        target,
        decl_ty,
        expr,
        is_typed_decl,
        ..
    } = &sample[0]
    else {
        panic!("expected assignment");
    };
    assert!(matches!(target, AssignTarget::Var(name) if name == "b"));
    assert!(decl_ty.is_none());
    assert!(!is_typed_decl);
    match expr {
        Expr::ArrayLiteral { values, .. } => assert_eq!(values.len(), 3),
        _ => panic!("expected untyped array literal expression"),
    }
}

#[test]
fn parses_inferred_integer_binding_ranges_with_default_and_explicit_modes() {
    let src = r#"
params:
  test = 0

sample:
  clamped = 0 {1000}
  wrapped = 0 {1000, wrap}
  exclusive = 10 {10..1000}
  inclusive = 10 {10..=1000}
  named_count = 0 {count = 1000, mode = clamp}
  named_exclusive = 10 {range = 10..1000, mode = wrap}
  named_inclusive = 10 {range = 10..=1000}
  from_binding = test {0..1000}
"#;
    let program = parse_program(src).expect("inferred integer binding ranges should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(block) => Some(&block.body),
            _ => None,
        })
        .expect("sample block");
    let expected = [
        (BuiltinFn::BindingCountClamp, 0, 1000),
        (BuiltinFn::BindingCountWrap, 0, 1000),
        (BuiltinFn::BindingRangeClamp, 10, 1000),
        (BuiltinFn::BindingRangeInclusiveClamp, 10, 1000),
        (BuiltinFn::BindingCountClamp, 0, 1000),
        (BuiltinFn::BindingRangeWrap, 10, 1000),
        (BuiltinFn::BindingRangeInclusiveClamp, 10, 1000),
        (BuiltinFn::BindingRangeClamp, 0, 1000),
    ];
    for (statement, (expected_func, expected_begin, expected_end)) in sample.iter().zip(expected) {
        let Stmt::Assign {
            decl_ty,
            is_typed_decl,
            expr: Expr::Call { func, args, .. },
            ..
        } = statement
        else {
            panic!("expected an inferred ranged declaration");
        };
        assert_eq!(*decl_ty, Some(DeclType::Scalar(PrimitiveType::I32)));
        assert!(*is_typed_decl);
        assert_eq!(*func, expected_func);
        assert_eq!(args.len(), 3);
        assert!(matches!(args[1], Expr::Int { value, .. } if value == expected_begin));
        assert!(matches!(args[2], Expr::Int { value, .. } if value == expected_end));
    }
}

#[test]
fn parses_integer_binding_ranges_on_struct_fields() {
    let program = parse_program(
        r#"
struct Cursor:
  index: i32 = 0 {8, wrap}
  limit = 7 {0..=7}
"#,
    )
    .expect("integer struct field ranges should parse");
    let struct_def = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Struct(struct_def) => Some(struct_def),
            _ => None,
        })
        .expect("struct block");
    let expected = [
        BuiltinFn::BindingCountWrap,
        BuiltinFn::BindingRangeInclusiveClamp,
    ];
    for (field, expected_func) in struct_def.fields.iter().zip(expected) {
        let Some(Expr::Call { func, args, .. }) = &field.default else {
            panic!("expected a ranged struct field default");
        };
        assert_eq!(*func, expected_func);
        assert_eq!(args.len(), 3);
        assert_eq!(field.ty, FieldType::Scalar(PrimitiveType::I32));
    }
}

#[test]
fn rejects_binding_ranges_on_non_integer_struct_fields() {
    let errors = parse_program(
        r#"
struct Invalid:
  value: f32 = 0.0 {8, wrap}
"#,
    )
    .expect_err("non-integer struct field ranges should be rejected");
    assert!(errors.iter().any(|error| error
        .message
        .contains("binding ranges require an i32 or i64 struct field")));
}

#[test]
fn parses_pinned_init_state_independently_from_integer_ranges() {
    let program = parse_program(
        r#"
init:
  pin kernel: f32[8]
  pin cursor: i32 = 0 {8, wrap}
  pin gain = 1.0

sample:
  out1 = gain
"#,
    )
    .expect("pinned init state should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.pinned_roots, ["kernel", "cursor", "gain"]);
    assert_eq!(init.body.len(), 3);
    assert!(matches!(
        &init.body[1],
        Stmt::Assign {
            expr: Expr::Call {
                func: BuiltinFn::BindingCountWrap,
                ..
            },
            ..
        }
    ));
}

#[test]
fn rejects_pin_outside_direct_init_bindings() {
    for source in [
        "sample:\n  pin value = 1\n",
        "init:\n  if true:\n    pin value = 1\n",
        "init:\n  value = 1\n  pin value = 2\n",
        "init:\n  pin value += 1\n",
        "proc Voice:\n  params:\n    pin gain = 1.0\n",
    ] {
        assert!(
            parse_program(source).is_err(),
            "pin should be restricted to direct init bindings: {source}"
        );
    }
}

#[test]
fn pin_freshness_accounts_for_tuple_bindings() {
    let diagnostics = parse_program(
        "init:\n  (value, other) = (1.0, 2.0)\n  pin value = 3.0\nsample:\n  out1 = value\n",
    )
    .expect_err("a tuple-bound root must not be redeclared as pinned state");
    assert!(diagnostics.iter().any(|diagnostic| diagnostic
        .message
        .contains("'pin' requires a fresh state binding; 'value' was already assigned")));
}

#[test]
fn parses_bare_and_parenthesized_tuple_targets_with_discards() {
    let program = parse_program(
        "sample:\n  left, _, right = (1.0, 2.0, 3.0)\n  (a, _, b) = (4.0, 5.0, 6.0)\n  out1 = left + right + a + b\n",
    )
    .expect("tuple targets should allow optional parentheses and discard entries");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(&sample.body),
            _ => None,
        })
        .expect("sample block");

    for stmt in &sample[..2] {
        let Stmt::Assign {
            target: AssignTarget::Tuple(targets),
            ..
        } = stmt
        else {
            panic!("expected tuple assignment");
        };
        assert!(matches!(
            targets.as_slice(),
            [
                TupleAssignTarget::Binding(_),
                TupleAssignTarget::Discard,
                TupleAssignTarget::Binding(_)
            ]
        ));
    }
}

#[test]
fn typed_tuple_assignments_retain_the_declared_element_types() {
    let program = parse_program(
        "sample:\n  pair: (f64, i32, bool) = (1.0, 2, true)\n  out1 = f32(pair[0])\n",
    )
    .expect("typed tuple assignment should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(&sample.body),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign {
        decl_ty,
        is_typed_decl,
        ..
    } = &sample[0]
    else {
        panic!("expected typed tuple assignment");
    };
    assert_eq!(
        decl_ty,
        &Some(DeclType::Tuple(vec![
            PrimitiveType::F64,
            PrimitiveType::I32,
            PrimitiveType::Bool,
        ]))
    );
    assert!(*is_typed_decl);
}

#[test]
fn parameter_domains_remain_distinct_from_integer_binding_ranges() {
    let src = r#"
params:
  top: i32 = 6 {0, 6, step = 1}

proc Voice:
  params:
    selector: i32 = 6 {0, 6}
  outs:
    out1
  sample:
    index = selector {count = 7}
    out1 = f32(index)
"#;
    let program = parse_program(src).expect("parameter and binding domains should parse");

    let top_param = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Params(params) => params.first(),
            _ => None,
        })
        .expect("top-level parameter");
    let top_range = top_param.range.as_ref().expect("top-level parameter range");
    assert!(matches!(top_range.min, Some(Expr::Int { value: 0, .. })));
    assert!(matches!(top_range.max, Expr::Int { value: 6, .. }));
    assert!(matches!(
        top_param.control.step,
        Some(Expr::Int { value: 1, .. })
    ));

    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) => Some(proc),
            _ => None,
        })
        .expect("processor");
    let proc_param = proc.params.first().expect("processor parameter");
    let proc_range = proc_param
        .range
        .as_ref()
        .expect("processor parameter range");
    assert!(matches!(proc_range.min, Some(Expr::Int { value: 0, .. })));
    assert!(matches!(proc_range.max, Expr::Int { value: 6, .. }));
    assert!(proc_param.control.step.is_none());

    assert!(matches!(
        proc.sample.first(),
        Some(Stmt::Assign {
            expr: Expr::Call {
                func: BuiltinFn::BindingCountClamp,
                ..
            },
            ..
        })
    ));
}

#[test]
fn rejects_incomplete_and_duplicate_named_binding_ranges() {
    for (range, expected) in [
        (
            "{count = 8, range = 0..8}",
            "count and range domains are mutually exclusive",
        ),
        (
            "{0..8, count = 8}",
            "count and range domains are mutually exclusive",
        ),
        ("{0, 8}", "count and range domains are mutually exclusive"),
        ("{wrap, 8}", "positional binding range domain must precede"),
    ] {
        let source = format!("sample:\n  value = 0 {range}\n");
        let errors = parse_program(&source).expect_err("invalid binding range should fail");
        assert!(
            errors
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{errors:?}"
        );
    }
}

#[test]
fn parses_indexed_member_assignment_target_as_flat_index_target() {
    let src = r#"
outs { out1 }
sample {
  voices[i].freq = hz
}
"#;
    let program = parse_program(src).expect("indexed member assignment should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sb) => Some(&sb.body),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { target, .. } = &sample[0] else {
        panic!("expected assignment");
    };
    match target {
        AssignTarget::Index { base, index } => {
            assert_eq!(base, "voices.freq");
            assert!(matches!(index, Expr::Var { name, .. } if name == "i"));
        }
        _ => panic!("expected indexed assignment target"),
    }
}

#[test]
fn parses_struct_typed_array_single_ctor_initializer_expression() {
    let src = r#"
proc Voice {
  outs { out1 }
  sample {
    out1 = 0.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = Voice()
}
sample {
  out1 = 0.0
}
"#;
    let program =
        parse_program(src).expect("struct typed array single ctor initializer should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(stmts) => Some(&stmts.body),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign { expr, .. } = &init[0] else {
        panic!("expected assignment");
    };
    match expr {
        Expr::ArrayCtor {
            init: Some(values), ..
        } => {
            assert_eq!(values.len(), 1);
            assert!(matches!(values[0], Expr::UserCall { .. }));
        }
        _ => panic!("expected ArrayCtor with single ctor initializer"),
    }
}

#[test]
fn parse_program_file_resolves_include_and_import() {
    let dir = mk_temp_dir("include_import");
    let main = dir.join("main.onda");
    let filter = dir.join("filter.onda");
    let shared = dir.join("shared.onda");

    write_file(
        &shared,
        r#"
def shared_gain(x) {
  return x * 0.5
}
"#,
    );
    write_file(
        &filter,
        r#"
include "./shared.onda"
namespace DSP:
  struct OnePole:
    z: f32
  def process(x):
    return shared_gain(x)
"#,
    );
    write_file(
        &main,
        r#"
import filter
outs { out1 }
sample {
  s = DSP::OnePole()
  out1 = DSP::process(2.0) + s.z
}
"#,
    );

    let program = parse_program_file(&main).expect("parse_program_file should succeed");
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "DSP"
            && ns.items.iter().any(|item| matches!(item, NamespaceItem::Struct(_))))),
        "expected imported struct to be present"
    );
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "DSP"
            && ns.items.iter().any(|item| matches!(item, NamespaceItem::Def(_)))))
            && program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
        "expected imported defs to be present"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_with_path_uses_entry_overlay_for_relative_imports() {
    let dir = mk_temp_dir("entry_overlay_import");
    let main = dir.join("main.onda");
    let filter = dir.join("filter.onda");
    let shared = dir.join("shared.onda");

    write_file(&main, "outs { out1 }\nsample { out1 = 0.0 }\n");
    write_file(
        &shared,
        r#"
def shared_gain(x) {
  return x * 0.5
}
"#,
    );
    write_file(
        &filter,
        r#"
include "./shared.onda"
namespace DSP:
  struct OnePole:
    z: f32
  def process(x):
    return shared_gain(x)
"#,
    );

    let overlay = r#"
import filter
outs { out1 }
sample {
  s = DSP::OnePole()
  out1 = DSP::process(2.0) + s.z
}
"#;

    let program = parse_program_with_path(overlay, &main).expect("overlay parse should succeed");
    assert!(program
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "DSP"
        && ns.items.iter().any(|item| matches!(item, NamespaceItem::Struct(_))))));
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_with_path_accepts_unsaved_entry_overlay() {
    let dir = mk_temp_dir("unsaved_entry_overlay");
    let main = dir.join("new.onda");

    let program = parse_program_with_path("outs 1\nsample:\n  out1 = 0.0\n", &main)
        .expect("an unsaved entry overlay should not require an on-disk file");

    assert!(program
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Sample(_))));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_with_overlays_accepts_unsaved_import() {
    let dir = mk_temp_dir("unsaved_import_overlay");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");
    let mut overlays = std::collections::HashMap::new();
    overlays.insert(
        main.clone(),
        "import lib\nouts 1\nsample:\n  out1 = Lib::value()\n".to_owned(),
    );
    overlays.insert(
        lib.parent().expect("lib parent").join(".").join("lib.onda"),
        "namespace Lib:\n  def value():\n    return 0.0\n".to_owned(),
    );

    let program = parse_program_file_with_overlays(&main, &overlays)
        .expect("an unsaved imported overlay should resolve before disk lookup");

    assert!(program
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Namespace(namespace) if namespace.name == "Lib")));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_with_overlays_uses_dependency_overlay_contents() {
    let dir = mk_temp_dir("dependency_overlay_import");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");

    write_file(
        &main,
        r#"
import lib
outs { out1 }
sample { out1 = twice(SCALE) }
"#,
    );
    write_file(
        &lib,
        r#"
const SCALE = invalid
"#,
    );

    let mut overlays = std::collections::HashMap::new();
    overlays.insert(
        dir.join(".").join("lib.onda"),
        r#"
const SCALE = 0.25
def twice(x) { return x + x }
"#
        .to_owned(),
    );

    let program =
        parse_program_file_with_overlays(&main, &overlays).expect("overlay parse should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    let Expr::UserCall { args, .. } = expr else {
        panic!("expected rewritten user call");
    };
    assert!(matches!(
        args[0].expr,
        Expr::Var { ref name, .. } if name == "SCALE"
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_rejects_import_module_with_runtime_blocks() {
    let dir = mk_temp_dir("import_runtime_reject");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");

    write_file(
        &lib,
        r#"
outs { out1 }
sample { out1 = 0.0 }
"#,
    );
    write_file(
        &main,
        r#"
import lib
outs { out1 }
sample { out1 = 0.0 }
"#,
    );

    let result = parse_program_file(&main);
    assert!(
        result.is_err(),
        "imported module with runtime blocks should be rejected"
    );
    let errors = result.expect_err("expected parse error");
    assert!(!errors.is_empty(), "expected at least one diagnostic");
    let first = &errors[0];
    let canonical_lib = fs::canonicalize(&lib).expect("canonical lib path");
    let expected_file = canonical_lib
        .to_string_lossy()
        .to_string()
        .trim_start_matches(r"\\?\")
        .to_owned();
    assert_eq!(
        first.file.as_deref(),
        Some(expected_file.as_str()),
        "expected leaf diagnostic file to point at imported module"
    );
    assert_eq!((first.line, first.column), (2, 1));
    assert_eq!(first.end_line, 3);
    assert!(
        first.trace.iter().any(|t| t.contains("import 'lib'")),
        "expected trace to include import site"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_imports_top_level_consts() {
    let dir = mk_temp_dir("import_top_level_consts");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");

    write_file(
        &lib,
        r#"
const SCALE = 0.25
def twice(x) { return x + x }
"#,
    );
    write_file(
        &main,
        r#"
import lib
outs { out1 }
sample { out1 = twice(SCALE) }
"#,
    );

    let program = parse_program_file(&main).expect("parse_program_file should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Const(decl) if decl.name == "SCALE")),
        "top-level consts should be retained for semantics"
    );
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    let Expr::UserCall { args, .. } = expr else {
        panic!("expected rewritten user call");
    };
    assert!(matches!(
        args[0].expr,
        Expr::Var { ref name, .. } if name == "SCALE"
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn pest_parse_errors_map_back_to_original_indentation_lines() {
    let src = r#"proc Mix:
  ins:
    dry
    fb

  sample:
    out1 = (dry + fb) * 0.5

proc Saturate:
  sample:
    x = in1
    out1 = x - (x * x * x) * 0.1

init:
  mix = ()
  sat = Saturate()

graph:
  in1 >> mix.dry
  sat.out1 >>[1] mix.fb
  mix.out1 >> sat.in1
  mix.out1 >> out1
"#;

    let errors = parse_program(src).expect_err("expected parse error");
    let diag = errors
        .iter()
        .find(|diag| diag.message.contains("expected expr"))
        .expect("missing expected expr diagnostic");

    assert_eq!((diag.line, diag.column), (15, 10));
    assert!(
        !diag.message.contains("-->"),
        "diagnostic message should not include preprocessed-source location: {}",
        diag.message
    );
    assert!(
        !diag.message.contains("\n15 |"),
        "diagnostic message should not include a source snippet: {}",
        diag.message
    );
}

#[test]
fn parse_program_file_includes_top_level_consts() {
    let dir = mk_temp_dir("include_top_level_consts");
    let main = dir.join("main.onda");
    let lib = dir.join("lib.onda");

    write_file(
        &lib,
        r#"
const SCALE = 0.25
"#,
    );
    write_file(
        &main,
        r#"
include "./lib.onda"
outs { out1 }
sample { out1 = SCALE }
"#,
    );

    let program = parse_program_file(&main).expect("parse_program_file should succeed");
    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(stmts) => Some(stmts),
            _ => None,
        })
        .expect("sample block");
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Const(decl) if decl.name == "SCALE")),
        "included top-level consts should be retained for semantics"
    );
    let Stmt::Assign { expr, .. } = &sample[0] else {
        panic!("expected sample assignment");
    };
    assert!(matches!(
        expr,
        Expr::Var { name, .. } if name == "SCALE"
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_file_rejects_import_include_same_file_mix() {
    let dir = mk_temp_dir("import_include_mix");
    let main = dir.join("main.onda");
    let dep = dir.join("dep.onda");

    write_file(
        &dep,
        r#"
def f(x) { return x }
"#,
    );
    write_file(
        &main,
        r#"
import dep
include "./dep.onda"
outs { out1 }
sample { out1 = f(1.0) }
"#,
    );

    let result = parse_program_file(&main);
    assert!(
        result.is_err(),
        "same file cannot be both imported and included"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parse_program_in_memory_supports_builtin_std_imports() {
    let src = r#"
import std/math
outs { out1 }
sample { out1 = clamp(2.0, 0.0, 1.0) }
"#;
    let program = parse_program(src).expect("in-memory std import should parse");
    assert!(
        program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
        "expected std module declarations to be imported"
    );
}

#[test]
fn parse_program_in_memory_supports_std_prelude_module() {
    let src = r#"
import std/prelude
outs { out1 }
sample { out1 = clamp(2.0, 0.0, 1.0) + read([1.0, 2.0], 1) }
"#;
    let program = parse_program(src).expect("in-memory std/prelude import should parse");
    assert!(
        program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
        "expected std/prelude declarations to be imported"
    );
}

#[test]
fn parse_program_in_memory_supports_std_data_module() {
    let src = r#"
import std/data
outs { out1 }
init {
  d = std::data::Data<f32>()
}
sample {
  out1 = d.readL(0.5)
}
"#;
    let program = parse_program(src).expect("in-memory std/data import should parse");
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "std::data"
                && ns.items.iter().any(|item| matches!(item, NamespaceItem::Struct(s) if s.name == "Data")))),
        "expected std/data declarations to be imported"
    );
}

#[test]
fn parses_typed_init_struct_decl_with_explicit_type_args_and_ctor() {
    let src = r#"
import std/data
init {
  line: std::data::Data<f32> = std::data::Data()
}
"#;
    let program = parse_program(src).expect("typed init struct declaration should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        !is_typed_decl,
        "typed struct decl should desugar to constructor-typed assignment"
    );
    let Expr::UserCall {
        name, type_args, ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::Data"));
    assert_eq!(
        type_args.as_slice(),
        &[CallTypeArg::Primitive(PrimitiveType::F32)]
    );
}

#[test]
fn parses_typed_init_struct_decl_with_explicit_type_args_and_default_ctor() {
    let src = r#"
import std/data
init {
  line: std::data::Data<f32>
}
"#;
    let program = parse_program(src).expect("typed init struct declaration should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        !is_typed_decl,
        "typed struct decl should desugar to constructor-typed assignment"
    );
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::Data"));
    assert!(args.is_empty(), "default ctor should be argument-less");
    assert_eq!(
        type_args.as_slice(),
        &[CallTypeArg::Primitive(PrimitiveType::F32)]
    );
}

#[test]
fn rejects_old_bracket_generic_type_args_in_typed_decl() {
    let src = r#"
import std/data
init {
  line: std::data::Data[f32]
}
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "old bracket generic type-arg syntax should be rejected"
    );
}

#[test]
fn rejects_old_bracket_namespace_and_generic_instantiation_syntax() {
    let src = r#"
import std/fft
init {
  fft: std::fft[8]::FFT[f32]
}
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "old bracket namespace/generic instantiation syntax should be rejected"
    );
}

#[test]
fn parses_typed_init_struct_decl_without_type_args_and_default_ctor() {
    let src = r#"
import std/data
init {
  line: std::data::Data
}
"#;
    let program = parse_program(src).expect("typed init struct declaration should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        *is_typed_decl,
        "typed struct decl without explicit type args should remain a typed declaration"
    );
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::Data"));
    assert!(args.is_empty(), "default ctor should be argument-less");
    assert!(type_args.is_empty(), "type args should be inferred later");
}

#[test]
fn parses_typed_init_namespace_instantiated_struct_decl_without_type_args() {
    let src = r#"
import std/data
init {
  line: std::data<SR, 1>::Data
}
"#;
    let program = parse_program(src).expect("typed init namespace-instantiated decl should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        *is_typed_decl,
        "typed struct decl without explicit type args should remain a typed declaration"
    );
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::Data"));
    assert!(args.is_empty(), "default ctor should be argument-less");
    assert!(type_args.is_empty(), "type args should be inferred later");
}

#[test]
fn parses_typed_init_namespaced_generic_struct_array_decl() {
    let src = r#"
import std/complex
init {
  bins: std::complex::Complex<f32>[4]
}
"#;
    let program =
        parse_program(src).expect("typed init namespaced generic struct array should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign { expr, .. } = &init[0] else {
        panic!("expected assignment in init");
    };
    match expr {
        Expr::ArrayCtor { spec, init, .. } => {
            assert!(
                matches!(spec.elem, ArrayElemType::Struct(ref s) if s == "std::complex::Complex<f32>")
            );
            assert!(matches!(spec.size.as_ref(), Expr::Int { value: 4, .. }));
            assert!(
                init.is_none(),
                "default ctor array decl should have no explicit initializer"
            );
        }
        other => panic!("expected array ctor, got {other:?}"),
    }
}

#[test]
fn parses_typed_init_namespace_instantiated_struct_decl_with_explicit_type_args() {
    let src = r#"
import std/fft
init {
  fft: std::fft<8>::FFT<f32>
}
"#;
    let program = parse_program(src)
        .expect("typed init namespace-instantiated struct with type args should parse");
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(init) => Some(init),
            _ => None,
        })
        .expect("init block");
    let Stmt::Assign {
        is_typed_decl,
        expr,
        ..
    } = &init[0]
    else {
        panic!("expected assignment in init");
    };
    assert!(
        !is_typed_decl,
        "typed struct decl with explicit type args should desugar to constructor-typed assignment"
    );
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        panic!("expected constructor call");
    };
    assert!(name.ends_with("::FFT"));
    assert!(args.is_empty(), "default ctor should be argument-less");
    assert_eq!(
        type_args.as_slice(),
        &[CallTypeArg::Primitive(PrimitiveType::F32)]
    );
}

#[test]
fn parse_program_in_memory_supports_std_lookup_module() {
    let src = r#"
import std/lookup
buffers { b: buffer<f32[2]> }
outs { out1 }
sample {
  out1 = std::lookup::read(b, 0, 1) + std::lookup::readL(b, 1, 0.5)
}
"#;
    let program = parse_program(src).expect("in-memory std/lookup import should parse");
    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "std::lookup"
                && ns.items.iter().any(|item| matches!(item, NamespaceItem::Def(d) if d.name == "read")))),
        "expected std/lookup declarations to be imported"
    );
}

#[test]
fn parse_program_in_memory_rejects_non_std_imports() {
    let src = r#"
import my_lib
outs { out1 }
sample { out1 = 0.0 }
"#;
    let result = parse_program(src);
    assert!(
        result.is_err(),
        "in-memory parser should reject non-std imports without file context"
    );
}

#[test]
fn parses_top_level_events_block_with_scalar_and_array_params() {
    let src = r#"
outs { out1 }
events {
  note_on(note: i32, vel: i32) {
    gate = vel
  }
  set_curve(values: f32[8]) {
    gate = values[0]
  }
}
init { gate = 0.0 }
sample { out1 = gate }
"#;

    let program = parse_program(src).expect("events block should parse");
    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("events block");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name, "note_on");
    assert_eq!(events[0].params.len(), 2);
    assert_eq!(events[1].name, "set_curve");
    assert_eq!(events[1].params.len(), 1);
    match &events[1].params[0].ty {
        EventParamType::Array { elem, .. } => assert_eq!(*elem, PrimitiveType::F32),
        _ => panic!("expected fixed-size primitive array param"),
    }
}

#[test]
fn parses_multiline_event_param_list_in_indentation_syntax() {
    let src = r#"
events:
  test(
    a: f32,
    b: f32
  ):
    value = a + b
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("multiline event params should parse");
    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("events block");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "test");
    assert_eq!(events[0].params.len(), 2);
    assert_eq!(events[0].params[0].name, "a");
    assert_eq!(events[0].params[1].name, "b");
}

#[test]
fn parses_top_level_individual_event_syntax_and_merges_with_events_block() {
    let src = r#"
outs { out1 }
event note_on(note: i32) {
  gate = f32(note)
}
events:
  note_off():
    gate = 0.0
init:
  gate = 0.0
sample:
  out1 = gate
"#;

    let program = parse_program(src).expect("individual event syntax should parse");
    let events = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1, "expected merged top-level events block");
    assert_eq!(events[0].len(), 2);
    assert_eq!(events[0][0].name, "note_on");
    assert_eq!(events[0][1].name, "note_off");
}

#[test]
fn parses_proc_events_block_indentation_syntax() {
    let src = r#"
proc Voice:
  outs 1
  events:
    note_on(note: i32):
      gate = note
  init:
    gate = 0.0
  sample:
    out1 = gate
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("proc events indentation syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 1);
    assert_eq!(proc.events[0].name, "note_on");
    assert_eq!(proc.events[0].params.len(), 1);
}

#[test]
fn parses_proc_individual_event_syntax_and_merges_with_events_block() {
    let src = r#"
proc Voice:
  outs 1
  event note_on(note: i32):
    gate = f32(note)
  events:
    note_off():
      gate = 0.0
  init:
    gate = 0.0
  sample:
    out1 = gate
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("individual proc event syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 2);
    assert_eq!(proc.events[0].name, "note_on");
    assert_eq!(proc.events[1].name, "note_off");
}

#[test]
fn parses_proc_event_slice_param_syntax() {
    let src = r#"
proc Loader:
  events:
    set_ir(values: f32[]):
      last = values[0]
  init:
    last = 0.0
  sample:
    out1 = last
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("proc event slice syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 1);
    assert_eq!(proc.events[0].params.len(), 1);
    match &proc.events[0].params[0].ty {
        EventParamType::Slice { elem } => assert_eq!(*elem, PrimitiveType::F32),
        other => panic!("expected proc event slice param, got {other:?}"),
    }
}

#[test]
fn parses_generic_proc_event_slice_param_syntax() {
    let src = r#"
proc Loader<T>:
  init:
    last = 0.0
  events:
    set_ir(values: T[]):
      last = f32(values[0])
  sample:
    out1 = last
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("generic proc event slice syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.type_params, vec!["T".to_owned()]);
    match &proc.events[0].params[0].ty {
        EventParamType::GenericSlice { elem } => assert_eq!(elem, "T"),
        other => panic!("expected generic proc event slice param, got {other:?}"),
    }
}

#[test]
fn parses_generic_proc_event_scalar_param_defaults() {
    let src = r#"
proc Filter<T>:
  events:
    set(freqv: T = 1200.0, rqv: T = 1.0):
      freq = freqv
      rq = rqv
  init:
    freq = 0.0
    rq = 0.0
  sample:
    out1 = f32(freq + rq)
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("generic proc event scalar defaults should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.type_params, vec!["T".to_owned()]);
    assert_eq!(proc.events[0].params.len(), 2);
    match &proc.events[0].params[0].ty {
        EventParamType::GenericScalar { name } => assert_eq!(name, "T"),
        other => panic!("expected generic scalar proc event param, got {other:?}"),
    }
    assert!(matches!(
        proc.events[0].params[0].default,
        Some(Expr::Number { value, .. }) if (value - 1200.0).abs() < f64::EPSILON
    ));
    match &proc.events[0].params[1].ty {
        EventParamType::GenericScalar { name } => assert_eq!(name, "T"),
        other => panic!("expected generic scalar proc event param, got {other:?}"),
    }
    assert!(matches!(
        proc.events[0].params[1].default,
        Some(Expr::Number { value, .. }) if (value - 1.0).abs() < f64::EPSILON
    ));
}

#[test]
fn parses_generic_proc_event_fixed_array_param_defaults() {
    let src = r#"
proc Loader<T>:
  events:
    load(values: T[2] = [1.0, 2.0]):
      last = f32(values[0] + values[1])
  init:
    last = 0.0
  sample:
    out1 = last
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("generic proc event fixed-array defaults should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    match &proc.events[0].params[0].ty {
        EventParamType::GenericArray { elem, size } => {
            assert_eq!(elem, "T");
            assert!(matches!(size, Expr::Int { value: 2, .. }));
        }
        other => panic!("expected generic fixed-array proc event param, got {other:?}"),
    }
    assert!(matches!(
        proc.events[0].params[0].default.as_ref(),
        Some(Expr::ArrayLiteral { values, .. }) if values.len() == 2
    ));
}

#[test]
fn parses_generic_proc_event_slice_with_scalar_params() {
    let src = r#"
proc Loader<T>:
  events:
    set_ir(values: T[], start: i32, limit: i32):
      x = start + limit
  sample:
    out1 = 0.0
outs 1
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("generic proc event mixed param syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 1);
    assert_eq!(proc.events[0].params.len(), 3);
    match &proc.events[0].params[0].ty {
        EventParamType::GenericSlice { elem } => assert_eq!(elem, "T"),
        other => panic!("expected generic proc event slice param, got {other:?}"),
    }
    match proc.events[0].params[1].ty {
        EventParamType::Scalar(PrimitiveType::I32) => {}
        ref other => panic!("expected i32 event param, got {other:?}"),
    }
    match proc.events[0].params[2].ty {
        EventParamType::Scalar(PrimitiveType::I32) => {}
        ref other => panic!("expected i32 event param, got {other:?}"),
    }
}

#[test]
fn defaults_untyped_event_params_to_f32() {
    let src = r#"
outs { out1 }
events {
  note_on(note, vel: i32, curve: f32[2]) {
    gate = note + f32(vel) + curve[0]
  }
}
init { gate = 0.0 }
sample { out1 = gate }
"#;

    let program = parse_program(src).expect("events block should parse");
    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("events block");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].params.len(), 3);
    match events[0].params[0].ty {
        EventParamType::Scalar(PrimitiveType::F32) => {}
        ref other => panic!("expected default f32 event param, got {other:?}"),
    }
    match events[0].params[1].ty {
        EventParamType::Scalar(PrimitiveType::I32) => {}
        ref other => panic!("expected explicit i32 event param, got {other:?}"),
    }
    match &events[0].params[2].ty {
        EventParamType::Array { elem, .. } => assert_eq!(*elem, PrimitiveType::F32),
        _ => panic!("expected fixed-size primitive array param"),
    }
}

#[test]
fn parses_event_param_defaults() {
    let src = r#"
events:
  note_on(freq_hz: f32 = 440.0, offsets: i32[2] = [1, 2], accent: bool = true):
    gate = freq_hz
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("event defaults should parse");
    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("events block");
    assert_eq!(events[0].params.len(), 3);
    assert!(matches!(
        events[0].params[0].default,
        Some(Expr::Number { value, .. }) if (value - 440.0).abs() < f64::EPSILON
    ));
    assert!(matches!(
        events[0].params[1].default.as_ref(),
        Some(Expr::ArrayLiteral { values, .. }) if values.len() == 2
    ));
    assert!(matches!(
        events[0].params[2].default,
        Some(Expr::Bool { value: true, .. })
    ));
}

#[test]
fn defaults_untyped_proc_event_params_to_f32() {
    let src = r#"
proc Voice:
  outs 1
  events:
    note_on(note):
      gate = note
  init:
    gate = 0.0
  sample:
    out1 = gate
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("proc events indentation syntax should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert_eq!(proc.events.len(), 1);
    assert_eq!(proc.events[0].params.len(), 1);
    match proc.events[0].params[0].ty {
        EventParamType::Scalar(PrimitiveType::F32) => {}
        ref other => panic!("expected default f32 proc event param, got {other:?}"),
    }
}

#[test]
fn parses_proc_event_param_defaults() {
    let src = r#"
proc Voice:
  outs 1
  events:
    note_on(note: f32 = 440.0, accent: bool = false):
      gate = note
  init:
    gate = 0.0
  sample:
    out1 = gate
sample:
  out1 = 0.0
"#;

    let program = parse_program(src).expect("proc event defaults should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) => Some(p),
            _ => None,
        })
        .expect("proc block");
    assert!(matches!(
        proc.events[0].params[0].default,
        Some(Expr::Number { value, .. }) if (value - 440.0).abs() < f64::EPSILON
    ));
    assert!(matches!(
        proc.events[0].params[1].default,
        Some(Expr::Bool { value: false, .. })
    ));
}

// ---- Phase 0: Namespace Const Fixes — Parser-level tests ----

#[test]
fn parses_2_level_nested_namespace_template() {
    let src = r#"
namespace Outer<A = 1>:
  namespace Inner<B = 2>:
    struct S:
      x: f32

init:
  s = Outer<10>::Inner<20>::S()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("2-level nested namespace template should parse");

    let outer = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Outer" => Some(ns),
            _ => None,
        })
        .expect("Outer namespace");
    assert!(outer.items.iter().any(|item| {
        matches!(item, NamespaceItem::Namespace(inner) if inner.name == "Inner"
            && inner.items.iter().any(|nested| matches!(nested, NamespaceItem::Struct(s) if s.name == "S")))
    }));

    // The init block should have a constructor call referencing the flattened struct
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.len(), 1);
    let call_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(call_name, "Outer<10>::Inner<20>::S");
}

#[test]
fn parses_3_level_nested_namespace_template() {
    let src = r#"
namespace L1<A = 1>:
  namespace L2<B = 2>:
    namespace L3<C = 3>:
      struct S:
        x: f32

init:
  s = L1<10>::L2<20>::L3<30>::S()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("3-level nested namespace template should parse");

    let l1 = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "L1" => Some(ns),
            _ => None,
        })
        .expect("L1 namespace");
    assert!(l1.items.iter().any(|item| {
        matches!(item, NamespaceItem::Namespace(l2) if l2.name == "L2"
            && l2.items.iter().any(|nested| {
                matches!(nested, NamespaceItem::Namespace(l3) if l3.name == "L3"
                    && l3.items.iter().any(|leaf| matches!(leaf, NamespaceItem::Struct(s) if s.name == "S")))
            }))
    }));

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let call_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(call_name, "L1<10>::L2<20>::L3<30>::S");
}

#[test]
fn parses_nested_template_inner_uses_outer_const_as_default() {
    let src = r#"
namespace Outer<S = SR>:
  namespace Inner<N = S>:
    struct Buf:
      data: f32

init:
  b = Outer<48000>::Inner::Buf()
sample:
  out1 = 0.0
"#;
    let program =
        parse_program(src).expect("inner template using outer const as default should parse");

    let outer = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Outer" => Some(ns),
            _ => None,
        })
        .expect("Outer namespace");
    assert!(outer.items.iter().any(|item| {
        matches!(item, NamespaceItem::Namespace(inner) if inner.name == "Inner"
            && matches!(inner.params[0].default, Expr::Var { ref name, .. } if name == "S"))
    }));

    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let call_name = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(call_name, "Outer<48000>::Inner::Buf");
}

#[test]
fn deduplicates_nested_namespace_template_instantiations() {
    let src = r#"
namespace Outer<S = SR>:
  namespace Inner<N = 1>:
    struct S:
      x: f32

init:
  a = Outer<SR>::Inner<1>::S()
  b = Outer<SR>::Inner<1>::S()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("nested namespace dedup should parse");

    assert!(program
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Namespace(ns) if ns.name == "Outer")));

    // Both constructor calls should reference the same struct name
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    assert_eq!(init.len(), 2);
    let name_a = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    let name_b = match &init[1] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall { name, .. } => name.clone(),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(
        name_a, name_b,
        "both calls should preserve the same namespace reference"
    );
}

// ---- Phase 0: Generics × Namespace Const Interaction — Parser-level tests ----

#[test]
fn parses_generic_struct_inside_namespace_template_t_s_pattern() {
    let src = r#"
namespace Data<S = SR>:
  struct Store<T>:
    buf: T[S]

init:
  s = Data<1024>::Store<f32>()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("T[S] pattern should parse");

    let store = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Data" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Struct(s) if s.name == "Store" => Some(s),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("Store struct");

    // Generic type params should be preserved
    assert_eq!(store.type_params, vec!["T".to_owned()]);

    // The field `buf` should be an array with generic element type
    assert_eq!(store.fields.len(), 1);
    assert_eq!(store.fields[0].name, "buf");
    match &store.fields[0].ty {
        FieldType::Array(spec) => {
            assert!(
                matches!(spec.elem, ArrayElemType::Struct(ref n) if n == "T"),
                "expected ArrayElemType::Struct(\"T\"), got {:?}",
                spec.elem
            );
        }
        other => panic!("expected FieldType::Array, got {other:?}"),
    }

    // Constructor call should reference the instantiated namespace
    let init = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Init(v) => Some(&v.body),
            _ => None,
        })
        .expect("init block");
    let (call_name, call_type_args) = match &init[0] {
        Stmt::Assign { expr, .. } => match expr {
            Expr::UserCall {
                name, type_args, ..
            } => (name.clone(), type_args.clone()),
            other => panic!("expected constructor call, got {other:?}"),
        },
        other => panic!("expected assignment, got {other:?}"),
    };
    assert_eq!(call_name, "Data<1024>::Store");
    assert_eq!(call_type_args.len(), 1);
    assert!(matches!(
        call_type_args[0],
        CallTypeArg::Primitive(PrimitiveType::F32)
    ));
}

#[test]
fn parses_generic_proc_inside_namespace_template() {
    let src = r#"
namespace FX<S = SR>:
  proc Delay<T>:
    ins<T> 1
    outs<T> 1
    init:
      buf: T = T(0.0)
    sample:
      out1 = in1

init:
  d = FX<48000>::Delay<f32>()
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("generic proc inside ns template should parse");

    let proc_def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "FX" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Proc(p) if p.name == "Delay" => Some(p),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("Delay proc");

    assert_eq!(proc_def.type_params, vec!["T".to_owned()]);
}

#[test]
fn parses_generic_struct_method_array_param_type() {
    let src = r#"
struct Store<T>:
  buf: T[2]
  def load(self, input: T[]):
    self.buf[0] = input[0]
sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("generic struct method array param should parse");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "Store" => Some(s),
            _ => None,
        })
        .expect("Store struct");
    assert_eq!(st.type_params, vec!["T".to_owned()]);
    assert!(matches!(
        st.methods[0].params[1].ty,
        Some(FnParamType::ArrayGeneric(ref n)) if n == "T"
    ));
}

#[test]
fn parses_def_return_type_annotations() {
    let src = r#"
def scalar(x: f32) -> f64:
  return x

def pair<T>(x: T, y: i32) -> (T, i32):
  return (x, y)

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("def return annotations should parse");
    let defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(def) => Some(def),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(matches!(
        defs[0].return_ty,
        Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(
            PrimitiveType::F64
        )))
    ));
    assert!(matches!(
        defs[1].return_ty,
        Some(FnReturnType::Tuple(ref elems))
            if elems.as_slice()
                == [
                    FnReturnScalarType::Named("T".to_owned()),
                    FnReturnScalarType::Primitive(PrimitiveType::I32),
                ]
    ));
}

#[test]
fn parses_multiline_delimiters_across_core_constructs() {
    let src = r#"
namespace Math<
  N = (
    1 + 1
  )
>:
  const Size = N

def mix<
  T
>(
  a: T,
  b: T
) -> (
  T,
  T
):
  pair = (
    a,
    b
  )
  return pair

params:
  freq = 440.0 {
    20.0,
    20000.0
  }

init:
  arr: f32[
    Math<
      N = 2
    >::Size
  ] = [
    0.0,
    1.0
  ]
  x = arr[
    (
      0
    )
  ]

sample:
  out1 = mix<
    f32
  >(
    arr[
      0
    ],
    x
  )
"#;

    parse_program(src).expect("multiline delimiter forms should parse");
}

#[test]
fn parses_multiline_section_defaults_and_buffer_delimiters() {
    let src = r#"
outs<
  f32
>:
  out1

buffers<
  f32
>:
  line: buffer<
    f32[
      2
    ]
  >
  taps: f32[
    4
  ]

sample:
  out1 = 0.0
"#;

    parse_program(src).expect("multiline section defaults and buffer delimiters should parse");
}

#[test]
fn parses_multiline_graph_delay_and_endpoint_sets() {
    let src = r#"
outs:
  out1
  out2

graph:
  0.5 >>[
    2
  ] out1
  0.25 >> {
    out1,
    out2
  }
  {
    out1,
    out2
  } << 0.125
"#;

    parse_program(src).expect("multiline graph bracket forms should parse");
}

#[test]
fn parses_multiline_method_params_slices_and_indexes() {
    let src = r#"
struct Store:
  value: f32

  def set(
    self,
    value: f32
  ):
    self.value = value

init:
  data: f32[
    4
  ] = [
    0.0,
    1.0,
    2.0,
    3.0
  ]
  dst: f32[
    2
  ]
  s = Store()

sample:
  dst[
    0:
    2
  ] = data[
    1:
    3
  ]
  s.set(
    dst[
      0
    ]
  )
  out1 = data[
    0
  ]
"#;

    parse_program(src).expect("multiline method, slice, and index delimiters should parse");
}

#[test]
fn rejects_trailing_commas_in_comma_separated_language_forms() {
    let cases = [
        ("print arguments", "sample:\n  print(1,)\n"),
        ("call arguments", "sample:\n  value = f(1,)\n"),
        ("array literals", "init:\n  values = [1,]\n"),
        ("tuple expressions", "init:\n  value = (1, 2,)\n"),
        ("tuple targets", "sample:\n  (a, b,) = pair()\n"),
        (
            "function parameters",
            "def f(value: f32,):\n  return value\n",
        ),
        (
            "generic parameters",
            "def f<T,>(value: T):\n  return value\n",
        ),
        ("generic arguments", "sample:\n  value = f<f32,>(1.0)\n"),
        ("tuple types", "init:\n  value: (f32, i32,) = (1.0, 1)\n"),
        (
            "event parameters",
            "event ping(value: f32,):\n  print(value)\n",
        ),
        ("delegate parameters", "delegate ready(value: f32,)\n"),
        ("when bindings", "when ready(value,):\n  print(value)\n"),
        (
            "namespace parameters",
            "namespace N<Value = 1,>:\n  const Result = Value\n",
        ),
        ("namespace arguments", "use N<Value = 1,>\n"),
        ("binding ranges", "init:\n  value = 0 {8,}\n"),
        ("parameter domains", "params:\n  value = 0.0 {0.0, 1.0,}\n"),
        (
            "graph endpoint sets",
            "outs:\n  out1\ngraph:\n  0.0 >> {out1,}\n",
        ),
        ("section declaration lists", "outs { out1, }\n"),
        ("struct field lists", "struct Value { field: f32, }\n"),
    ];

    for (context, source) in cases {
        assert!(
            parse_program(source).is_err(),
            "{context} must reject a trailing comma"
        );
    }
}

#[test]
fn parses_struct_method_return_type_annotation() {
    let src = r#"
struct Pair<T>:
  a: T
  b: T

  def swap(self) -> (T, T):
    return (self.b, self.a)

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("method return annotation should parse");
    let st = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "Pair" => Some(s),
            _ => None,
        })
        .expect("Pair struct");

    assert!(matches!(
        st.methods[0].return_ty,
        Some(FnReturnType::Tuple(ref elems))
            if elems.as_slice()
                == [
                    FnReturnScalarType::Named("T".to_owned()),
                    FnReturnScalarType::Named("T".to_owned()),
                ]
    ));
}

#[test]
fn parses_proc_local_def_return_type_annotation() {
    let src = r#"
proc Voice:
  outs:
    out1

  def pair(x: f32) -> (f32, i32):
    return (x, 1)

  sample:
    out1 = 0.0

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("proc-local return annotation should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) if p.name == "Voice" => Some(p),
            _ => None,
        })
        .expect("Voice proc");

    assert!(matches!(
        proc.local_defs[0].return_ty,
        Some(FnReturnType::Tuple(ref elems))
            if elems.as_slice()
                == [
                    FnReturnScalarType::Primitive(PrimitiveType::F32),
                    FnReturnScalarType::Primitive(PrimitiveType::I32),
                ]
    ));
}

#[test]
fn parses_namespaced_def_return_type_annotation() {
    let src = r#"
namespace dsp:
  struct Pair:
    x

def borrow(pair: dsp::Pair) -> dsp::Pair:
  return pair

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("namespaced return annotation should parse");
    let def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Def(def) if def.name == "borrow" => Some(def),
            _ => None,
        })
        .expect("borrow def");

    assert!(matches!(
        def.return_ty,
        Some(FnReturnType::Scalar(FnReturnScalarType::Named(ref name)))
            if name == "dsp::Pair"
    ));
}

#[test]
fn parses_nested_generic_struct_field_type() {
    let src = r#"
struct Inner<T>:
  data: T[2]

struct Outer<T>:
  inner: Inner<T>
  banks: Inner<f32>[2]

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("nested generic struct fields should parse");
    let outer = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Struct(s) if s.name == "Outer" => Some(s),
            _ => None,
        })
        .expect("Outer struct");
    assert_eq!(outer.fields[0].name, "inner");
    assert!(matches!(
        outer.fields[1].ty,
        FieldType::Array(ref spec)
            if matches!(spec.elem, ArrayElemType::Struct(ref n) if n == "Inner<f32>")
    ));
}

#[test]
fn parses_namespace_qualified_generic_type_in_call_type_args() {
    let src = r#"
namespace NS:
  struct Pair<T>:
    a: T
    b: T

proc Container<T>:
  outs 1
  init:
    p = NS::Pair<T>(T(1.0), T(2.0))
  sample:
    out1 = f32(p.a)

sample:
  out1 = 0.0
"#;
    let program =
        parse_program(src).expect("ns-qualified generic type in call type args should parse");

    let container = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Proc(p) if p.name == "Container" => Some(p),
            _ => None,
        })
        .expect("Container proc");
    assert_eq!(container.type_params, vec!["T".to_owned()]);

    let pair = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "NS" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Struct(s) if s.name == "Pair" => Some(s),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("Pair struct");
    assert_eq!(pair.type_params, vec!["T".to_owned()]);
}

#[test]
fn parses_and_rewrites_const_decls_in_top_level_namespace_and_local_scopes() {
    let src = r#"
const N = 4

namespace NS:
  const M = N
  def value():
    return f32(M)

outs { out1 }
events {
  load(values: f32[N]) {}
}
sample {
  const X = N
  out1 = f32(X) + NS::value()
}
"#;
    let program = parse_program(src).expect("const rewriting should succeed");

    assert!(
        program
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Const(decl) if decl.name == "N")),
        "top-level scalar const declarations should be retained for semantics"
    );
    assert!(
        program.blocks.iter().any(|b| matches!(b, Block::Namespace(ns) if ns.name == "NS"
            && ns.items.iter().any(|item| matches!(item, NamespaceItem::Const(decl) if decl.name == "M")))),
        "namespace scalar const declarations should be retained for semantics"
    );

    let events = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Events(v) => Some(v),
            _ => None,
        })
        .expect("top-level events");
    assert!(matches!(
        events[0].params[0].ty,
        EventParamType::Array {
            elem: PrimitiveType::F32,
            size: Expr::Var { ref name, .. },
        }
            if name == "N"
    ));

    let def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "NS" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Def(d) if d.name == "value" => Some(d),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("value def");
    assert!(matches!(
        def.body[0],
        Stmt::Return {
            expr: Expr::Cast {
                to: PrimitiveType::F32,
                ref expr,
                ..
            },
            ..
        } if matches!(expr.as_ref(), Expr::Var { name, .. } if name == "M")
    ));

    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    assert_eq!(
        sample.body.len(),
        2,
        "local const should be retained for semantics"
    );
    assert!(matches!(
        &sample.body[0],
        Stmt::Const {
            decl: ConstDecl {
                name,
                expr: Expr::Var { name: expr_name, .. },
                ..
            },
            ..
        } if name == "X" && expr_name == "N"
    ));
    assert!(matches!(
        &sample.body[1],
        Stmt::Assign {
            expr:
                Expr::Binary {
                    lhs,
                    ..
                },
            ..
        } if expr_contains_var_with_suffix(lhs, "X")
    ));
}

#[test]
fn parses_explicit_top_level_config_constants() {
    let program = parse_program(
        "config const Size: i32 = 4\nconfig const Values: f32[Size] = [0.0, 1.0, 2.0, 3.0]\n",
    )
    .expect("configuration constants should parse");
    let declarations = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Const(decl) => Some(decl),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(declarations.len(), 2);
    assert!(declarations.iter().all(|decl| decl.configurable));
    assert_eq!(declarations[0].name, "Size");
    assert!(matches!(
        declarations[0].ty,
        Some(ConstType::Scalar(PrimitiveType::I32))
    ));
    assert!(matches!(
        declarations[1].ty,
        Some(ConstType::Array {
            elem: PrimitiveType::F32,
            ..
        })
    ));
}

#[test]
fn config_constants_are_top_level_only() {
    for source in [
        "namespace NS:\n  config const Value: i32 = 1\n",
        "sample:\n  config const Value: i32 = 1\n",
        "proc P:\n  config const Value: i32 = 1\n  sample:\n    out1 = 0.0\n",
    ] {
        assert!(
            parse_program(source).is_err(),
            "nested configuration constant unexpectedly parsed: {source}"
        );
    }
}

#[test]
fn includes_allow_config_constants_but_imports_reject_them() {
    let dir = mk_temp_dir("config_const_source_modes");
    let main = dir.join("main.onda");
    let shared = dir.join("shared.onda");
    let module = dir.join("module.onda");
    write_file(&shared, "config const Shared: i32 = 1\n");
    write_file(&module, "config const Imported: i32 = 2\n");

    write_file(&main, "include \"./shared.onda\"\n");
    let program = load_program_file(&main).expect("includes should share configuration constants");
    assert!(program.program.blocks.iter().any(
        |block| matches!(block, Block::Const(decl) if decl.name == "Shared" && decl.configurable)
    ));

    write_file(&main, "import module\n");
    let error = load_program_file(&main)
        .expect_err("declaration-only imports must not expose configuration constants");
    assert!(error.diagnostics.iter().any(|diagnostic| diagnostic
        .message
        .contains("configuration constants are not allowed")));

    fs::remove_dir_all(dir).ok();
}

#[test]
fn preserves_top_level_const_arrays_after_scalar_const_rewrite() {
    let src = r#"
const N = 3
const Table: f32[N] = [0.25, 0.5, 1.0]
"#;

    let program = parse_program(src).expect("top-level const array should parse");
    assert!(program
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Const(decl) if decl.name == "N")));
    let table = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Const(decl) if decl.name == "Table" => Some(decl),
            _ => None,
        })
        .expect("const array declaration should be retained");
    match &table.ty {
        Some(ConstType::Array { elem, size }) => {
            assert_eq!(*elem, PrimitiveType::F32);
            assert!(matches!(size, Expr::Var { name, .. } if name == "N"));
        }
        other => panic!("expected typed const array, got {other:?}"),
    }
    assert!(matches!(
        &table.expr,
        Expr::ArrayLiteral { values, .. } if values.len() == 3
    ));
}

#[test]
fn parses_const_array_slice_type_annotations() {
    let src = r#"
const Table: f32[] = [0.25, 0.5, 1.0]

namespace NS:
  const Flags: bool[] = [true, false]
"#;

    let program = parse_program(src).expect("const array slice annotations should parse");
    let table = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Const(decl) if decl.name == "Table" => Some(decl),
            _ => None,
        })
        .expect("top-level const array");
    assert!(matches!(
        &table.ty,
        Some(ConstType::Slice { elem }) if *elem == PrimitiveType::F32
    ));

    let flags = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Namespace(ns) if ns.name == "NS" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Const(decl) if decl.name == "Flags" => Some(decl),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("namespace const array");
    assert!(matches!(
        &flags.ty,
        Some(ConstType::Slice { elem }) if *elem == PrimitiveType::Bool
    ));
}

#[test]
fn parses_top_level_const_def() {
    let src = r#"
const def twice(x: f32) -> f32:
  return x * 2.0

const Table: f32[2] = [twice(0.5), twice(1.0)]
"#;

    let program = parse_program(src).expect("const def should parse");
    let def = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Def(def) if def.name == "twice" => Some(def),
            _ => None,
        })
        .expect("const def should be retained for semantics");
    assert!(def.is_const);
    assert_eq!(def.params.len(), 1);
    assert!(matches!(
        def.return_ty,
        Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(
            PrimitiveType::F32
        )))
    ));
}

#[test]
fn parses_const_def_array_return_type() {
    let src = r#"
const def table() -> f32[4]:
  return [0.0, 0.25, 0.5, 0.75]
"#;

    let program = parse_program(src).expect("const def array return should parse");
    let def = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Def(def) if def.name == "table" => Some(def),
            _ => None,
        })
        .expect("const def should be retained for semantics");
    assert!(def.is_const);
    assert!(matches!(
        def.return_ty,
        Some(FnReturnType::Array {
            elem: PrimitiveType::F32,
            size: Expr::Int { value: 4, .. },
        })
    ));
}

#[test]
fn qualifies_namespace_const_array_references() {
    let src = r#"
namespace LUT:
  const Table = [1, 2, 3]

outs:
  out1

sample:
  out1 = LUT::Table[0]
"#;

    let program = parse_program(src).expect("namespace const array should parse");
    assert!(program.blocks.iter().any(|block| {
        matches!(block, Block::Namespace(ns) if ns.name == "LUT"
            && ns.items.iter().any(|item| matches!(item, NamespaceItem::Const(decl) if decl.name == "Table")))
    }));
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");
    match &sample.body[0] {
        Stmt::Assign { expr, .. } => {
            assert!(matches!(expr, Expr::Index { base, .. } if base == "LUT::Table"));
        }
        other => panic!("expected assignment, got {other:?}"),
    }
}

#[test]
fn parses_index_and_slice_on_namespace_template_const_references() {
    let src = r#"
namespace LUT<N = 2>:
  const Table: f32[N] = [0.5, 1.0]

outs:
  out1

sample:
  out1 = LUT<2>::Table[1]
  dst[:] = LUT<2>::Table[:]
"#;

    let program = parse_program(src).expect("namespace-template const array refs should parse");
    let sample = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");

    match &sample.body[0] {
        Stmt::Assign { expr, .. } => {
            assert!(matches!(expr, Expr::Index { base, .. } if base == "LUT<2>::Table"));
        }
        other => panic!("expected index assignment, got {other:?}"),
    }
    match &sample.body[1] {
        Stmt::Assign { target, expr, .. } => {
            assert!(
                matches!(target, AssignTarget::Slice { base, .. } if base == "dst"),
                "expected slice assignment target, got {target:?}"
            );
            assert!(matches!(expr, Expr::Slice { base, .. } if base == "LUT<2>::Table"));
        }
        other => panic!("expected slice assignment, got {other:?}"),
    }
}

#[test]
fn preserves_proc_local_const_arrays_for_semantic_rejection() {
    let src = r#"
proc Voice:
  const Table = [1, 2]
  outs:
    out1
  sample:
    out1 = 0.0
"#;

    let program = parse_program(src).expect("proc-local const arrays should parse");
    let proc = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Proc(proc) if proc.name == "Voice" => Some(proc),
            _ => None,
        })
        .expect("Voice proc");
    assert!(matches!(
        proc.consts.as_slice(),
        [ConstDecl {
            name,
            expr: Expr::ArrayLiteral { values, .. },
            ..
        }] if name == "Table" && values.len() == 2
    ));
}

#[test]
fn rejects_proc_local_const_defs() {
    let src = r#"
proc Voice:
  const def gain() -> f32:
    return 1.0
  outs:
    out1
  sample:
    out1 = 0.0
"#;

    let errors = parse_program(src).expect_err("proc-local const defs should be rejected");
    assert!(errors.iter().any(|diag| diag
        .message
        .contains("const defs are only supported at top-level and namespace scope")));
}

#[test]
fn qualifies_qualified_namespace_const_paths_for_semantics() {
    let src = r#"
import std/convolution

outs {
  out1
  out2
}
sample {
  out1 = f32(std::convolution<8, 8>::HopSize)
  out2 = f32(std::convolution::HopSize)
}
"#;
    let program =
        parse_program(src).expect("qualified namespace const access should rewrite successfully");

    let sample = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Sample(sample) => Some(sample),
            _ => None,
        })
        .expect("sample block");

    for stmt in &sample.body {
        assert!(
            stmt_contains_var_with_suffix(stmt, "::HopSize"),
            "expected HopSize namespace const paths to be retained, got {stmt:?}"
        );
    }
}

#[test]
fn qualifies_relative_nested_namespace_const_paths_from_current_namespace() {
    let src = r#"
namespace Outer:
  namespace Inner:
    const VALUE = 3

  def read():
    return f32(Inner::VALUE)

outs {
  out1
}

sample {
  out1 = Outer::read()
}
"#;
    let program =
        parse_program(src).expect("relative nested namespace const access should rewrite");

    let def = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "Outer" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Def(def) if def.name == "read" => Some(def),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("read def");

    for stmt in &def.body {
        assert!(
            stmt_contains_var_with_suffix(stmt, "Inner::VALUE"),
            "expected relative nested namespace const path to be retained, got {stmt:?}"
        );
    }
}

fn stmt_contains_var_with_suffix(stmt: &Stmt, suffix: &str) -> bool {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => false,
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            expr_contains_var_with_suffix(expr, suffix)
        }
        Stmt::Print { values, .. } => values
            .iter()
            .any(|expr| expr_contains_var_with_suffix(expr, suffix)),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            expr_contains_var_with_suffix(cond, suffix)
                || then_branch
                    .iter()
                    .any(|stmt| stmt_contains_var_with_suffix(stmt, suffix))
                || else_branch
                    .iter()
                    .any(|stmt| stmt_contains_var_with_suffix(stmt, suffix))
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            expr_contains_var_with_suffix(start, suffix)
                || expr_contains_var_with_suffix(end, suffix)
                || step
                    .as_ref()
                    .map(|expr| expr_contains_var_with_suffix(expr, suffix))
                    .unwrap_or(false)
                || body
                    .iter()
                    .any(|stmt| stmt_contains_var_with_suffix(stmt, suffix))
        }
        Stmt::While { cond, body, .. } => {
            expr_contains_var_with_suffix(cond, suffix)
                || body
                    .iter()
                    .any(|stmt| stmt_contains_var_with_suffix(stmt, suffix))
        }
    }
}

fn expr_contains_var_with_suffix(expr: &Expr, suffix: &str) -> bool {
    match expr {
        Expr::Var { name, .. } => name.ends_with(suffix),
        Expr::Index { base, index, .. } => {
            base.ends_with(suffix) || expr_contains_var_with_suffix(index, suffix)
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            base.ends_with(suffix)
                || start
                    .as_ref()
                    .map(|expr| expr_contains_var_with_suffix(expr, suffix))
                    .unwrap_or(false)
                || end
                    .as_ref()
                    .map(|expr| expr_contains_var_with_suffix(expr, suffix))
                    .unwrap_or(false)
        }
        Expr::ArrayCtor { spec, init, .. } => {
            expr_contains_var_with_suffix(&spec.size, suffix)
                || init
                    .as_ref()
                    .map(|values| {
                        values
                            .iter()
                            .any(|expr| expr_contains_var_with_suffix(expr, suffix))
                    })
                    .unwrap_or(false)
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            expr_contains_var_with_suffix(lhs, suffix) || expr_contains_var_with_suffix(rhs, suffix)
        }
        Expr::Call { args, .. } | Expr::ArrayLiteral { values: args, .. } => args
            .iter()
            .any(|expr| expr_contains_var_with_suffix(expr, suffix)),
        Expr::UserCall { name, args, .. } => {
            name.ends_with(suffix)
                || args
                    .iter()
                    .any(|arg| expr_contains_var_with_suffix(&arg.expr, suffix))
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            expr_contains_var_with_suffix(expr, suffix)
        }
        Expr::Tuple { values, .. } => values
            .iter()
            .any(|v| expr_contains_var_with_suffix(v, suffix)),
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => false,
    }
}

#[test]
fn instantiates_std_convolution_with_user_consts_and_rewrites_nested_proc_calls() {
    let src = r#"
import std/convolution

const FFT_SIZE = 1024
const MAX_IR = 100000

proc Engine:
  init:
    conv = std::convolution<FFT_SIZE, MAX_IR>::ZeroLatencyConvolver<f32>()
  sample:
    out1 = 0.0

sample:
  out1 = 0.0
"#;
    let program = parse_program(src).expect("std::convolution const instantiation should parse");

    let zero_latency = program
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Namespace(ns) if ns.name == "std::convolution" => {
                ns.items.iter().find_map(|item| match item {
                    NamespaceItem::Proc(p) if p.name == "ZeroLatencyConvolver" => Some(p),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("ZeroLatencyConvolver proc");

    let set_impulse = zero_latency
        .events
        .iter()
        .find(|e| e.name == "set_impulse")
        .expect("set_impulse event");
    let event_calls: Vec<_> = set_impulse
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Expr {
                expr: Expr::UserCall { name, .. },
                ..
            } => Some(name.clone()),
            _ => None,
        })
        .collect();

    assert!(
        event_calls.iter().any(|name| name == "td.set_impulse"),
        "expected td event call to remain receiver-based, got {event_calls:?}"
    );
    assert!(
        event_calls.iter().any(|name| name == "final.set_impulse"),
        "expected final-stage event call to remain receiver-based, got {event_calls:?}"
    );
}

#[test]
fn parses_graph_block_with_rates_and_delay() {
    let src = r#"
outs { out1 }
params { mix = 0.25 }
proc OnePole {
  ins { in1 }
  params { cutoff = 1000.0 }
  outs { out1 }
  sample { out1 = in1 }
}
init {
  lp = OnePole()
}
graph {
  @sample mix >> lp.cutoff
  lp.out1 >>[1] out1
}
"#;

    let program = parse_program(src).expect("graph program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert_eq!(graph.edges.len(), 2);
    assert_eq!(graph.edges[0].rate, Some(GraphRate::Sample));
    assert!(graph.edges[0].delay.is_none());
    assert_eq!(graph.edges[0].dests.len(), 1);
    assert!(matches!(
        graph.edges[0].dests[0],
        GraphEndpoint::ProcField { ref proc, ref field, .. }
        if proc == "lp" && field == "cutoff"
    ));
    assert_eq!(graph.edges[1].delay, Some(Expr::int(1)));
    assert_eq!(graph.edges[1].dests.len(), 1);
    assert!(matches!(
        graph.edges[1].dests[0],
        GraphEndpoint::Symbol { ref name, .. } if name == "out1"
    ));
}

#[test]
fn parses_graph_proc_array_slot_endpoints() {
    let src = r#"
outs { out1 }
proc Voice {
  outs { out1 }
  sample { out1 = 0.0 }
}
init {
  voices: Voice[2] = Voice()
}
graph {
  voices[1].out1 >> out1
}
"#;

    let program = parse_program(src).expect("graph proc-array program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    assert!(matches!(
        graph.edges[0].source,
        Expr::UserCall { ref name, .. }
        if name == &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}")
    ));
}

#[test]
fn parses_graph_proc_array_param_slot_sources() {
    let src = r#"
proc Voice {
  params { gain = 0.0 }
  outs { out1 }
  sample { out1 = gain }
}
outs { out1 }
init {
  voices: Voice[2] = Voice()
}
graph {
  voices[1].gain >> out1
}
"#;

    let program = parse_program(src).expect("graph proc-array param source program should parse");
    let graph = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Graph(graph) => Some(graph),
            _ => None,
        })
        .expect("graph block");

    match &graph.edges[0].source {
        Expr::UserCall { name, args, .. } => {
            assert_eq!(
                name,
                &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}")
            );
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_INDEX_BASE_ARG)
                    && matches!(arg.expr, Expr::Var { name: ref base, .. } if base == "voices")
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG)
                    && matches!(arg.expr, Expr::Int { value: 1, .. })
            }));
            assert!(args.iter().any(|arg| {
                arg.name.as_deref() == Some(PROC_FIELD_SENTINEL_ARG)
                    && matches!(arg.expr, Expr::Var { name: ref field, .. } if field == "gain")
            }));
        }
        other => panic!("expected graph proc-array param source sentinel call, got {other:?}"),
    }
}

