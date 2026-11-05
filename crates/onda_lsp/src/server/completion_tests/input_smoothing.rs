use super::*;

fn fields(source: &str) -> Vec<Value> {
    completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        CompletionPosition {
            line: source.lines().count() as u32 - 1,
            character: source.lines().last().unwrap().len() as u32,
        },
        true,
    )
    .items
}

#[test]
fn input_smoothing_completes_in_root_proc_and_braced_inputs() {
    for source in [
        "ins:\n  cutoff {",
        "inputs<f64>:\n  cutoff {0, 100, ",
        "proc Voice:\n  ins:\n    cutoff {",
        "proc Main { ins { cutoff {",
    ] {
        let items = fields(source);
        for name in ["ramp", "lag"] {
            let item = items
                .iter()
                .find(|item| item["label"] == name)
                .unwrap_or_else(|| panic!("{source}: {items:?}"));
            assert!(item["insertText"]
                .as_str()
                .unwrap()
                .starts_with(&format!("{name} = ")));
            assert!(item["detail"].as_str().unwrap().contains("seconds"));
        }
        for invalid in ["scale", "curve", "step", "unit", "mode"] {
            assert!(
                !items.iter().any(|item| item["label"] == invalid),
                "{source}: {invalid}"
            );
        }
    }
}

#[test]
fn input_smoothing_completion_excludes_used_and_conflicting_attributes() {
    for source in [
        "ins:\n  cutoff {ramp = 0.02, ",
        "proc Voice:\n  ins:\n    cutoff {lag = 0, ",
        "params:\n  cutoff = 440.0 {20, 20000, ",
    ] {
        let items = fields(source);
        assert!(
            !items
                .iter()
                .any(|item| matches!(item["label"].as_str(), Some("ramp" | "lag"))),
            "{source}: {items:?}"
        );
    }
}

#[test]
fn input_smoothing_duration_values_complete_expressions() {
    let source = "const Fade = 0.02\nins:\n  cutoff {ramp = Fa";
    let items = fields(source);
    assert!(
        items.iter().any(|item| item["label"] == "Fade"),
        "{items:?}"
    );
}
