use super::*;

#[test]
fn borrowed_processor_params_reject_private_reads_and_writes() {
    for binding in [
        "v = voices[0]",
        "if first:\n      v = voices[0]\n    else:\n      v = voices[1]",
    ] {
        for (access, expected) in [
            ("v.gain = 9", "private and cannot be assigned"),
            ("read = v.gain", "private and cannot be read"),
            ("v.coeffs[0] = 9", "private and cannot be assigned"),
            ("read = v.coeffs[0]", "private and cannot be read"),
            ("v.params[0] = 9", "has private params"),
            ("read = v.params[0]", "has private params"),
        ] {
            let source = format!(
                "proc Voice:\n  params:\n    private gain: i32 = 0\n    private coeffs: i32[2] = [0, 0]\n  sample:\n    out1 = f32(gain)\nproc Bank:\n  init:\n    voices: Voice[2] = Voice()\n    first = true\n  sample:\n    {binding}\n    {access}\n    out1 = v()\ninit:\n  bank = Bank()\nsample:\n  out1 = bank()\n"
            );
            let errors =
                analyze(parse_program(&source).expect("source should parse")).expect_err(&source);
            assert!(
                errors.iter().any(|error| error.message.contains(expected)),
                "{source}\nexpected {expected}, got {errors:?}"
            );
        }
    }
}

#[test]
fn sliced_processor_params_reject_private_reads_and_writes() {
    for (access, expected) in [
        ("selected[0].gain = 9", "private and cannot be assigned"),
        ("read = selected[0].gain", "private and cannot be read"),
    ] {
        let source = format!(
            "proc Voice:\n  params:\n    private gain: i32 = 0\n  sample:\n    out1 = f32(gain)\nproc Bank:\n  init:\n    voices: Voice[2] = Voice()\n  sample:\n    selected = voices[:]\n    {access}\n    out1 = 0.0\ninit:\n  bank = Bank()\nsample:\n  out1 = bank()\n"
        );
        let errors =
            analyze(parse_program(&source).expect("source should parse")).expect_err(&source);
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "{errors:?}"
        );
    }
}

#[test]
fn borrowed_processor_param_setters_preserve_assignment_type_checks() {
    let source = r#"
proc Voice:
  params:
    gain: i32 = 0 => update
  init:
    cached: i32 = 0
  def update():
    cached = gain
  sample:
    out1 = f32(cached)
def set_gain(v, value: i64):
  v.gain = value
proc Bank:
  ins 1
  init:
    voices: Voice[2] = Voice()
  sample:
    v = voices[0]
    set_gain(v, i64(in1))
    out1 = v()
ins 1
init:
  bank = Bank()
sample:
  out1 = bank(in1)
"#;
    let errors = analyze(parse_program(source).unwrap())
        .expect_err("an implicit i64-to-i32 narrowing must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("cannot assign I64 to I32")),
        "expected assignment type diagnostic, got {errors:?}"
    );
}
