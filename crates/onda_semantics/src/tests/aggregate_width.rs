use super::*;

#[test]
fn wide_aggregate_references_analyze_and_lower() {
    let width = 1100;
    let fields = (0..width)
        .map(|index| format!("  field{index}: f32 = {index}.0\n"))
        .collect::<String>();
    let sum = (0..width)
        .map(|index| format!("value.field{index}"))
        .collect::<Vec<_>>()
        .join(" + ");
    let source = format!(
        "struct Wide:\n{fields}def sum(value: Wide) -> f32:\n  return {sum}\ninit:\n  value = Wide()\nsample:\n  out1 = sum(value)\n"
    );

    let typed = analyze_source(&source);
    assert_eq!(
        typed
            .aggregate_layouts
            .layout_for_struct("Wide")
            .unwrap()
            .leaves
            .len(),
        width
    );
    lower_program_to_optimized_mir(&typed).unwrap();
}

#[test]
fn wide_processor_state_analyzes_and_lowers() {
    let width = 1100;
    let fields = (0..width)
        .map(|index| format!("    field{index} = {index}.0\n"))
        .collect::<String>();
    let source = format!(
        "proc Wide:\n  init:\n{fields}  sample:\n    out1 = field0 + field{}\ninit:\n  voice = Wide()\nsample:\n  out1 = voice()\n",
        width - 1,
    );

    let typed = analyze_source(&source);
    lower_program_to_optimized_mir(&typed).unwrap();
}
