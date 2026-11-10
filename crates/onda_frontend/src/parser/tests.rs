include!("tests/part_1.rs");
include!("tests/part_2.rs");
include!("tests/part_3.rs");

#[test]
fn enum_members_require_nonempty_unique_names() {
    for (source, message) in [
        ("enum Mode {}", "at least one member"),
        ("enum Mode:\n  A\n  A\n", "duplicate enum member"),
    ] {
        let errors = parse_program(source).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(message)),
            "{errors:?}"
        );
    }
    assert!(parse_program("enum Mode:\n  A = 4\n").is_err());
}

#[test]
fn indexed_cached_fields_remain_distinct_from_explicit_call_fields() {
    let program = parse_program(
        "sample:\n  cached = voices[1].aux\n  called = voices[1]().aux\n  voices[1].gain += 1.0\n",
    )
    .unwrap();
    let Block::Sample(statements) = &program.blocks[0] else {
        panic!("sample block expected")
    };
    let args = |statement: &Stmt| match statement {
        Stmt::Assign {
            expr: Expr::UserCall { args, .. },
            ..
        } => args.clone(),
        _ => panic!("field expression expected: {statement:?}"),
    };
    assert!(args(&statements[0])
        .iter()
        .any(|arg| arg.name.as_deref() == Some(super::PROC_CACHED_FIELD_ARG)));
    assert!(args(&statements[1])
        .iter()
        .all(|arg| arg.name.as_deref() != Some(super::PROC_CACHED_FIELD_ARG)));
    let Stmt::Assign { expr, .. } = &statements[2] else {
        panic!("compound assignment expected")
    };
    assert!(expr
        .walk()
        .any(|node| matches!(node, Expr::UserCall { args, .. }
        if args.iter().any(|arg| arg.name.as_deref() == Some(super::PROC_CACHED_FIELD_ARG)))));
}
