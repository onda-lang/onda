use super::*;

#[test]
fn guarded_indices_preserve_outputs_after_local_mutation() {
    let source = r#"
init:
  values: f32[4] = [10.0, 20.0, 30.0, 40.0]
  cursor = -2
sample:
  index = cursor
  out1 = -1.0
  out2 = 0.0
  if index >= 0:
    if index < 4:
      out1 = values[index]
      index = 99
      out2 = values[index]
  cursor += 1
"#;
    let (_, mir) = source_program(source, 8);
    fn count(block: &Block, counts: &mut [usize; 2]) {
        for statement in &block.statements {
            match &statement.kind {
                StatementKind::Assign {
                    value: Rvalue::Load(place),
                    ..
                } => {
                    for projection in &place.projections {
                        if let Projection::Index {
                            index: Value::Local(_),
                            bounds,
                            ..
                        } = projection
                        {
                            if *bounds == onda_mir::BoundsMode::Unchecked {
                                counts[0] += 1;
                            } else {
                                counts[1] += 1;
                            }
                        }
                    }
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    count(then_block, counts);
                    count(else_block, counts);
                }
                StatementKind::Loop { body } => count(body, counts),
                _ => {}
            }
        }
    }
    let mut counts = [0; 2];
    count(
        &mir.functions[mir.entry_points.process.index()].body,
        &mut counts,
    );
    assert!(counts[0] > 0, "guarded access should carry a MIR proof");
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(
            run_native_outputs_with_opt_level(source, 8, level),
            [
                vec![-1.0, -1.0, 10.0, 20.0, 30.0, 40.0, -1.0, -1.0],
                vec![0.0, 0.0, 40.0, 40.0, 40.0, 40.0, 0.0, 0.0],
            ]
        );
    }
}

#[test]
fn derived_loop_indices_preserve_forward_and_reverse_results() {
    for width in ["i32", "i64"] {
        for reverse in [false, true] {
            let (initial, condition, update) = if reverse {
                ("limit - 1", "i >= 0", "i -= 1")
            } else {
                ("0", "i < limit", "i += 1")
            };
            let source = format!(
                "init:
  values: f32[8] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
  count: {width} = -1
sample:
  limit = count
  total = 0.0
  if limit >= 0:
    if limit <= 4:
      i: {width} = {initial}
      while {condition}:
        total += values[4 + i]
        {update}
  out1 = total
  count += 1
"
            );
            let (_, mir) = source_program(&source, 8);
            let dump = onda_mir::format_program(&mir);
            assert!(
                !dump.contains("] clamp"),
                "derived access still clamps:\n{dump}"
            );
            for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
                assert_eq!(
                    run_native_outputs_with_opt_level(&source, 8, level),
                    [vec![0.0, 0.0, 5.0, 11.0, 18.0, 26.0, 0.0, 0.0]],
                    "{width}, reverse={reverse}, level={level:?}",
                );
            }
        }
    }
}

#[test]
fn dynamic_slice_loops_preserve_empty_views_updates_and_ordered_sums() {
    let source = r#"
def update(values: f32[]):
  count = values.len()
  i = 0
  while i < count:
    values[i] += 1.0
    i += 1
  total = 0.0
  j = 0
  while j < count:
    total += values[j]
    j += 1
  return total
init:
  values: f32[4] = [10.0, 20.0, 30.0, 40.0]
  count = 0
sample:
  out1 = update(values[:min(count, 4)])
  count += 1
"#;
    fn check(block: &Block) -> usize {
        block
            .statements
            .iter()
            .map(|statement| match &statement.kind {
                StatementKind::Assign {
                    value: Rvalue::SliceLoad { bounds, .. },
                    ..
                }
                | StatementKind::SliceStore { bounds, .. } => {
                    assert_eq!(*bounds, onda_mir::BoundsMode::Unchecked);
                    1
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => check(then_block) + check(else_block),
                StatementKind::Loop { body } => check(body),
                _ => 0,
            })
            .sum()
    }
    for width in ["i32", "i64"] {
        let source = source
            .replace(
                "count = values.len()",
                &format!("count = {width}(values.len())"),
            )
            .replace("  i = 0", &format!("  i: {width} = 0"))
            .replace("  j = 0", &format!("  j: {width} = 0"));
        let (_, mir) = source_program(&source, 8);
        assert_eq!(
            mir.functions
                .iter()
                .map(|function| check(&function.body))
                .sum::<usize>(),
            3
        );
        // All release targets consume the same proven MIR. Cross-emission
        // checks lowering and object formats; execution below uses the host.
        let optimized = trusted_optimized(mir).unwrap();
        for (triple, magic) in [
            ("x86_64-unknown-linux-gnu", &b"\x7fELF"[..]),
            ("x86_64-pc-windows-msvc", &b"\x64\x86"[..]),
            ("aarch64-apple-darwin", &b"\xcf\xfa\xed\xfe"[..]),
        ] {
            let options = MirTargetOptions {
                fast_math: false,
                target: crate::TargetConfig::for_triple(triple),
            };
            let object = lower_optimized_mir_to_object(&optimized, &options).unwrap();
            assert!(object.starts_with(magic), "{triple}: invalid object format");
        }
        for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            assert_eq!(
                run_native_outputs_with_opt_level(&source, 8, level),
                [vec![0.0, 11.0, 33.0, 66.0, 110.0, 114.0, 118.0, 122.0]]
            );
        }
    }
}
