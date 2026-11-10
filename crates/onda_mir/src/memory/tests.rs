use super::*;

fn integer(value: i32) -> Value {
    Value::Constant(ScalarValue::I32(value))
}

fn origin(state: u32, start: u32, len: u32) -> StorageOrigins {
    StorageOrigins {
        states: BTreeMap::from([(
            StateId::new(state),
            StateOrigin {
                ranges: Some(BTreeSet::from([ElementRange { start, len }])),
                offset_multiple: Some(integer_multiple(i64::from(start))),
            },
        )]),
        ..StorageOrigins::default()
    }
}

fn range(origins: &StorageOrigins) -> Option<(u32, u32)> {
    let region = origins.state_regions()?.next()?;
    Some((region.start, region.len))
}

#[test]
fn nested_views_compose_offsets_and_preserve_empty_extents() {
    let source = origin(0, 0, 16);
    let nested = source
        .clone()
        .window(integer(4), integer(8), BoundsMode::Checked, false, &[])
        .window(integer(2), integer(3), BoundsMode::Unchecked, false, &[]);
    assert_eq!(range(&nested), Some((6, 3)));
    for (start, len, expected) in [
        (-4, 3, (0, 3)),
        (14, 9, (14, 2)),
        (99, 4, (16, 0)),
        (3, -1, (3, 0)),
    ] {
        assert_eq!(
            range(&source.clone().window(
                integer(start),
                integer(len),
                BoundsMode::Clamp,
                false,
                &[]
            )),
            Some(expected)
        );
    }
}

#[test]
fn fixed_windows_clamp_the_start_without_shrinking_the_shape() {
    let source = origin(0, 4, 16);
    assert_eq!(
        range(
            &source
                .clone()
                .window(integer(99), integer(4), BoundsMode::Clamp, true, &[])
        ),
        Some((16, 4))
    );
    for (start, len, bounds) in [
        (0, 17, BoundsMode::Clamp),
        (15, 2, BoundsMode::Checked),
        (-1, 1, BoundsMode::Unchecked),
    ] {
        let result = source
            .clone()
            .window(integer(start), integer(len), bounds, true, &[]);
        assert_eq!(range(&result), None);
        assert_eq!(
            result.only_states().unwrap().collect::<Vec<_>>(),
            [StateId::new(0)]
        );
    }
    let empty = origin(0, 16, 0);
    assert_eq!(
        range(
            &empty
                .clone()
                .window(integer(0), integer(0), BoundsMode::Checked, true, &[])
        ),
        Some((16, 0))
    );
    assert_eq!(
        range(&empty.window(integer(0), integer(1), BoundsMode::Clamp, true, &[])),
        None
    );
}

#[test]
fn joins_keep_all_alternative_ranges_and_unknown_origins_remain_unknown() {
    let mut origins = origin(0, 0, 16);
    assert!(!origins.merge(&origin(0, 0, 16)));
    assert!(origins.merge(&origin(1, 4, 16)));
    assert_eq!(origins.state_regions().unwrap().count(), 2);
    assert!(origins.merge(&origin(0, 1, 16)));
    assert_eq!(origins.state_regions().unwrap().count(), 3);
    assert_eq!(origins.only_states().unwrap().count(), 2);
    assert!(!origins.merge(&origin(0, 0, 16)));
    assert!(origins.merge(&origin(0, 0, 16).forget_ranges()));
    assert!(!origins.merge(&origin(0, 1, 16)));
    assert!(origins.state_regions().is_none());
    for other in [StorageOrigins::unknown(), StorageOrigins::non_state()] {
        let mut known = origin(0, 0, 16);
        known.merge(&other);
        assert!(known.state_regions().is_none());
    }
}

#[test]
fn range_budget_bounds_growing_view_sets_without_losing_identity() {
    let mut origins = origin(0, 0, 8);
    for start in 1..MAX_RANGES_PER_STATE as u32 {
        assert!(origins.merge(&origin(0, start, 8)));
    }
    assert_eq!(
        origins.state_regions().unwrap().count(),
        MAX_RANGES_PER_STATE
    );
    assert!(origins.merge(&origin(0, MAX_RANGES_PER_STATE as u32, 8)));
    assert!(origins.state_regions().is_none());
    assert!(!origins.merge(&origin(0, 0, 8)));
    assert_eq!(
        origins.only_states().unwrap().collect::<Vec<_>>(),
        [StateId::new(0)]
    );
}

#[test]
fn nested_windows_apply_clamping_to_every_alternative() {
    let mut origins = origin(0, 0, 16);
    origins.merge(&origin(0, 32, 8));
    let views = origins
        .clone()
        .window(integer(6), integer(10), BoundsMode::Clamp, false, &[]);
    assert_eq!(
        views
            .state_regions()
            .unwrap()
            .map(|r| (r.start, r.len))
            .collect::<Vec<_>>(),
        [(6, 10), (38, 2)]
    );
    let fixed = origins.window(integer(6), integer(4), BoundsMode::Clamp, true, &[]);
    assert_eq!(
        fixed
            .state_regions()
            .unwrap()
            .map(|r| (r.start, r.len))
            .collect::<Vec<_>>(),
        [(6, 4), (36, 4)]
    );
}

#[test]
fn unresolved_offsets_do_not_inherit_the_base_address() {
    let origins = origin(0, 0, 16).window(
        Value::Local(LocalId::new(0)),
        integer(4),
        BoundsMode::Clamp,
        true,
        &[],
    );
    assert!(origins.state_regions().is_none());
    assert_eq!(
        origins.only_states().unwrap().collect::<Vec<_>>(),
        [StateId::new(0)]
    );
    assert_eq!(
        origins.state_offset_multiples().unwrap().next().unwrap().1,
        1
    );
}

fn offset_multiple(origins: &StorageOrigins) -> Option<u64> {
    Some(origins.state_offset_multiples()?.next()?.1)
}

#[test]
fn dynamic_offsets_preserve_divisibility_without_claiming_a_range() {
    let selected = origin(0, 0, 128).window(
        Value::Local(LocalId::new(0)),
        integer(4),
        BoundsMode::Clamp,
        true,
        &[4],
    );
    assert!(selected.state_regions().is_none());
    assert_eq!(offset_multiple(&selected), Some(4));
    let forwarded = selected
        .clone()
        .window(integer(0), integer(4), BoundsMode::Clamp, true, &[]);
    assert_eq!(offset_multiple(&forwarded), Some(4));
    let nested = selected
        .clone()
        .window(integer(2), integer(1), BoundsMode::Checked, true, &[]);
    assert_eq!(offset_multiple(&nested), Some(2));
    let mut joined = selected;
    joined.merge(&origin(0, 2, 4));
    assert_eq!(offset_multiple(&joined), Some(2));
    assert!(joined.state_regions().is_none());
    assert_eq!(offset_multiple(&joined.forget_ranges()), None);
}

#[test]
fn clamped_dynamic_windows_include_the_endpoint_in_the_divisibility_proof() {
    for (length, bounds, expected) in [
        (128, BoundsMode::Clamp, 4),
        (10, BoundsMode::Clamp, 2),
        (11, BoundsMode::Clamp, 1),
        (10, BoundsMode::Checked, 4),
        (10, BoundsMode::Unchecked, 4),
    ] {
        let selected = origin(0, 0, length).window(
            Value::Local(LocalId::new(0)),
            integer(4),
            bounds,
            true,
            &[4],
        );
        assert_eq!(offset_multiple(&selected), Some(expected));
        assert!(selected.state_regions().is_none());
    }
}

#[test]
fn exhausting_the_range_budget_preserves_common_offset_divisibility() {
    let mut origins = origin(0, 0, 4);
    for index in 1..=MAX_RANGES_PER_STATE as u32 {
        origins.merge(&origin(0, index * 4, 4));
    }
    assert!(origins.state_regions().is_none());
    assert_eq!(offset_multiple(&origins), Some(4));
}

fn integer_expressions(values: Vec<Rvalue>) -> (Program, Function) {
    let mut program = Program::new(
        CompileConfig::new(48000.0, 128).unwrap(),
        FunctionId::new(0),
        FunctionId::new(1),
    );
    program.types.push(Type::Scalar(ScalarType::I32));
    let function = Function {
        name: "expressions".to_owned(),
        kind: FunctionKind::User,
        attributes: FunctionAttributes::default(),
        params: vec![FunctionParam {
            name: "index".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::Value,
            integer_range: None,
        }],
        results: Vec::new(),
        locals: values
            .iter()
            .map(|_| Local {
                name: None,
                ty: TypeId::new(0),
                integer_range: None,
            })
            .collect(),
        body: Block {
            statements: values
                .into_iter()
                .enumerate()
                .map(|(index, value)| Statement {
                    kind: StatementKind::Assign {
                        destination: Place::local(LocalId::new(index as u32)),
                        value,
                    },
                    source: SourceSpan::UNKNOWN,
                })
                .collect(),
        },
        source: SourceSpan::UNKNOWN,
    };
    (program, function)
}

#[test]
fn integer_divisibility_follows_copies_products_masked_shifts_and_wrapping() {
    let local = |index| Value::Local(LocalId::new(index));
    let binary = |op, lhs, rhs| Rvalue::Binary { op, lhs, rhs };
    let (program, function) = integer_expressions(vec![
        Rvalue::Load(Place {
            base: PlaceBase::Parameter(ParameterId::new(0)),
            projections: Vec::new(),
        }),
        binary(BinaryOp::Multiply, local(0), integer(4)),
        Rvalue::Use(local(1)),
        binary(BinaryOp::ShiftLeft, local(2), integer(34)),
        binary(BinaryOp::Add, local(3), integer(8)),
        binary(BinaryOp::Subtract, local(4), integer(2)),
        Rvalue::Use(integer(-12)),
        binary(BinaryOp::Multiply, local(1), integer(1 << 30)),
    ]);
    assert_eq!(
        integer_multiples(&program, &function),
        [1, 4, 4, 16, 8, 2, 4, 0]
    );
}

#[test]
fn reassigned_and_escaped_integer_locals_are_conservative() {
    let (program, mut function) = integer_expressions(vec![Rvalue::Use(integer(8))]);
    function.body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::Use(integer(3)),
        },
        source: SourceSpan::UNKNOWN,
    });
    assert_eq!(integer_multiples(&program, &function), [1]);
    function.body.statements.pop();
    function.body.statements.push(Statement {
        kind: StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(0),
            args: vec![CallArgument::Place(Place::local(LocalId::new(0)))],
        },
        source: SourceSpan::UNKNOWN,
    });
    assert_eq!(integer_multiples(&program, &function), [1]);
}

#[test]
fn long_integer_copy_chains_use_the_dependency_worklist() {
    let mut values = vec![Rvalue::Use(integer(4))];
    values.extend((1..10_000).map(|index| Rvalue::Use(Value::Local(LocalId::new(index - 1)))));
    let (program, function) = integer_expressions(values);
    assert!(integer_multiples(&program, &function)
        .iter()
        .all(|&multiple| multiple == 4));
}
