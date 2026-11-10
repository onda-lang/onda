use super::*;

const CHILD_EVENTS_SOURCE: &str = r#"
proc Child:
  init:
    value = 0.0
  event strike(next: f32):
    value = next
  sample:
    out1 = value

proc Parent:
  init:
    children: Child[2] = Child()
  event send(index: i32, next: f32):
    children[index].strike(next)
  sample:
    children[0].strike(3.0)
    out1 = children[0]()

init:
  parent = Parent()
event Send(index: i32, next: f32):
  parent.send(index, next)
sample:
  out1 = parent()
"#;

#[test]
fn indexed_child_event_names_are_independent_of_enclosing_proc_events() {
    let typed = analyze_source(CHILD_EVENTS_SOURCE);
    lower_program_to_optimized_mir(&typed)
        .expect("indexed child events should lower from both proc events and sample");
}

#[test]
fn indexed_child_events_validate_the_selected_processors_members() {
    let source = CHILD_EVENTS_SOURCE.replace(".strike(", ".missing(");
    let errors = analyze(parse_program(&source).unwrap()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("unknown processor event")
                && error.message.contains("missing")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| !error.message.contains("unknown function")),
        "{errors:?}"
    );
}

#[test]
fn indexed_child_event_recognition_preserves_owning_proc_event_restrictions() {
    let source =
        CHILD_EVENTS_SOURCE.replace("children[index].strike(next)", "self.send(index, next)");
    assert_analyze_error_contains(
        &source,
        "cannot call event 'Parent.send' on the owning proc instance",
    );
}

#[test]
fn indexed_receivers_preserve_ordinary_function_argument_validation() {
    let source = format!(
        "def strike(receiver: f32, next: f32, required: i32):\n  return next\n{CHILD_EVENTS_SOURCE}"
    );
    assert_analyze_error_contains(&source, "required");
    assert_analyze_error_contains(&source, "function 'strike'");
}

#[test]
fn indexed_receivers_do_not_allow_unqualified_calls_to_own_events() {
    for invocation in ["strike(3.0)", "strike(children[0], 3.0)"] {
        let source = format!(
            r#"
proc Child:
  event strike(next: f32):
    value = next
  init:
    value = 0.0
  sample:
    out1 = value
proc Parent:
  init:
    children: Child[2] = Child()
  event strike(next: f32):
    children[0].strike(next)
  event send():
    {invocation}
  sample:
    out1 = children[0]()
init:
  parent = Parent()
sample:
  out1 = parent()
"#
        );
        assert_analyze_error_contains(&source, "proc event 'strike' is receiver-only");
    }
}

#[test]
fn indexed_imported_child_events_keep_their_qualified_receiver_types() {
    let dir = mk_temp_dir("indexed_imported_child_events");
    write_file(
        &dir.join("child.onda"),
        r#"
namespace Voices:
  proc Child:
    init:
      value = 0.0
    event strike(next: f32):
      value = next
    sample:
      out1 = value
"#,
    );
    write_file(
        &dir.join("main.onda"),
        r#"
import child
proc Parent:
  init:
    children: Voices::Child[2] = Voices::Child()
  event send(index: i32, next: f32):
    children[index].strike(next)
  sample:
    children[0].strike(3.0)
    out1 = children[0]()
init:
  parent = Parent()
sample:
  out1 = parent()
"#,
    );
    let program = parse_program_file(&dir.join("main.onda")).expect("child module should load");
    let typed = analyze(program).expect("qualified child events should analyze");
    lower_program_to_optimized_mir(&typed).expect("qualified child events should lower to MIR");
}
