use super::*;

fn integer(value: i32) -> Value {
    Value::Constant(ScalarValue::I32(value))
}

fn origin(state: u32, start: u32, len: u32) -> StorageOrigins {
    StorageOrigins {
        states: BTreeMap::from([(
            StateId::new(state),
            Some(BTreeSet::from([ElementRange { start, len }])),
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
        .window(integer(4), integer(8), BoundsMode::Checked, false)
        .window(integer(2), integer(3), BoundsMode::Unchecked, false);
    assert_eq!(range(&nested), Some((6, 3)));
    for (start, len, expected) in [
        (-4, 3, (0, 3)),
        (14, 9, (14, 2)),
        (99, 4, (16, 0)),
        (3, -1, (3, 0)),
    ] {
        assert_eq!(
            range(
                &source
                    .clone()
                    .window(integer(start), integer(len), BoundsMode::Clamp, false)
            ),
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
                .window(integer(99), integer(4), BoundsMode::Clamp, true)
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
            .window(integer(start), integer(len), bounds, true);
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
                .window(integer(0), integer(0), BoundsMode::Checked, true)
        ),
        Some((16, 0))
    );
    assert_eq!(
        range(&empty.window(integer(0), integer(1), BoundsMode::Clamp, true)),
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
        .window(integer(6), integer(10), BoundsMode::Clamp, false);
    assert_eq!(
        views
            .state_regions()
            .unwrap()
            .map(|r| (r.start, r.len))
            .collect::<Vec<_>>(),
        [(6, 10), (38, 2)]
    );
    let fixed = origins.window(integer(6), integer(4), BoundsMode::Clamp, true);
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
    );
    assert!(origins.state_regions().is_none());
    assert_eq!(
        origins.only_states().unwrap().collect::<Vec<_>>(),
        [StateId::new(0)]
    );
}
