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
