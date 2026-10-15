use super::*;

fn promoted_slots(source: &str) -> Vec<String> {
    let typed = analyze(parse_program(source).expect("parse")).expect("analyze");
    let mir = lower_program_to_optimized_mir(&typed).expect("lower");
    mir.functions[mir.entry_points.process.index()]
        .locals
        .iter()
        .filter_map(|local| local.name.as_deref()?.strip_prefix("$promoted.state."))
        .map(str::to_owned)
        .collect()
}

fn filter_promoted_slots(body: &str, instances: usize) -> Vec<String> {
    let body = body
        .lines()
        .map(|line| format!("    {line}\n"))
        .collect::<String>();
    let mut source = format!(
        r#"
proc Filter:
  params:
    coefficient = 0.5
  init:
    y = 0.0
  sample:
{body}    out1 = y
init:
"#
    );
    for index in 0..instances {
        source.push_str(&format!("  filter_{index} = Filter()\n"));
    }
    source.push_str("block:\n");
    for index in 0..instances {
        source.push_str(&format!("  filter_{index}.coefficient = 0.75\n"));
    }
    let output = (0..instances)
        .map(|index| format!("filter_{index}(in1)"))
        .collect::<Vec<_>>()
        .join(" + ");
    source.push_str(&format!("  sample:\n    out1 = {output}\n"));
    promoted_slots(&source)
}

const CONDITIONAL_FILTER: &str =
    "if in1 > y:\n  y = in1 + coefficient * (y - in1)\nelse:\n  y = in1";

#[test]
fn state_promotion_leaves_repeated_conditional_coefficients_in_memory() {
    let promoted = filter_promoted_slots(CONDITIONAL_FILTER, 4);
    for index in 0..4 {
        assert!(promoted.contains(&format!("filter_{index}.y")));
        assert!(!promoted.contains(&format!("filter_{index}.coefficient")));
    }
}

#[test]
fn state_promotion_keeps_single_conditional_coefficient_set() {
    let promoted = filter_promoted_slots(CONDITIONAL_FILTER, 1);
    assert!(promoted.contains(&"filter_0.coefficient".to_owned()));
}

#[test]
fn state_promotion_keeps_repeated_unconditional_coefficients() {
    let promoted = filter_promoted_slots("y = in1 + coefficient * (y - in1)", 4);
    for index in 0..4 {
        assert!(promoted.contains(&format!("filter_{index}.y")));
        assert!(promoted.contains(&format!("filter_{index}.coefficient")));
    }
}

#[test]
fn state_promotion_avoids_carrying_outer_state_across_nested_work() {
    let promoted = promoted_slots(
        r#"
init:
  history: f32[8]
  outer = 0.0
  inner = 0.0
sample:
  outer += 1.0
  for i in 0..8:
    inner += in1
    history[i] = inner
  out1 = history[0] + outer
"#,
    );
    assert!(promoted.iter().any(|name| name == "inner"));
    assert!(!promoted.iter().any(|name| name == "outer"));
}

#[test]
fn state_promotion_retains_outer_caching_around_array_only_inner_loops() {
    let promoted = promoted_slots(
        r#"
init:
  history: f32[8]
  outer = 0.0
sample:
  outer += in1
  for i in 0..8:
    history[i] = in1
  out1 = history[0] + outer
"#,
    );
    assert!(promoted.iter().any(|name| name == "outer"));
}

#[test]
fn state_promotion_retains_outer_caching_around_conditional_nested_work() {
    let promoted = promoted_slots(
        r#"
params:
  trigger = false
init:
  history: f32[8]
  outer = 0.0
  inner = 0.0
sample:
  outer += in1
  if trigger:
    for i in 0..8:
      inner += in1
      history[i] = inner
  out1 = history[0] + outer
"#,
    );
    assert!(promoted.iter().any(|name| name == "outer"));
    assert!(promoted.iter().any(|name| name == "inner"));
}

#[test]
fn state_promotion_accounts_for_aggregate_aliasing_of_direct_invariants() {
    for aggregate in [false, true] {
        let storage = if aggregate { "  storage: f32[8]\n" } else { "" };
        let store = if aggregate {
            "    storage[0] = history\n"
        } else {
            ""
        };
        let promoted = promoted_slots(&format!(
            r#"
init:
{storage}  block_count = 0.0
  history = 0.0
  coefficient = 0.5
block:
  block_count += 1.0
  coefficient = 1.0 / block_count
  sample:
    history = in1 + coefficient * history
{store}    out1 = history
"#,
        ));
        assert!(promoted.iter().any(|name| name == "history"));
        assert!(promoted.iter().any(|name| name == "block_count"));
        assert_eq!(promoted.iter().any(|name| name == "coefficient"), aggregate);
    }
}
