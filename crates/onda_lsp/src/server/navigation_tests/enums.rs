use super::*;

const SOURCE: &str = "namespace DSP:\n  enum Mode:\n    LowPass\n    HighPass {label = \"High-pass\"}\nuse DSP::Mode as Choice\nparams:\n  mode: Choice = Choice::HighPass\nsample:\n  out1 = i32(Choice(1))\n";

#[test]
fn enum_aliases_and_member_qualifiers_resolve_to_their_declarations() {
    for (needle, offset) in [("mode: Choice", "mode: Ch".len()), ("Choice::HighPass", 2)] {
        let hover = hover_at(SOURCE, needle, offset).unwrap();
        let markdown = hover["contents"]["value"].as_str().unwrap();
        assert!(markdown.contains("enum Mode:"), "{markdown}");
        assert!(markdown.contains("High-pass"), "{markdown}");
        assert!(definition_at(SOURCE, needle, offset).is_some());
    }
    let hover = hover_at(SOURCE, "Choice::HighPass", "Choice::High".len()).unwrap();
    assert!(hover["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("HighPass = 1"));
    let definition = definition_at(SOURCE, "Choice::HighPass", "Choice::High".len()).unwrap();
    assert_eq!(definition["range"]["start"]["line"], 3);
    assert!(hover_at(SOURCE, "    HighPass", "    High".len()).is_some());
}

#[test]
fn enum_constructors_provide_ordinal_signature_help() {
    let signature = signature_at(SOURCE, "Choice(1)", "Choice(".len()).unwrap();
    assert_eq!(
        signature["signatures"][0]["label"],
        "Mode(ordinal = 0) -> Mode"
    );
    assert!(signature["signatures"][0]["documentation"]
        .as_str()
        .unwrap()
        .contains("clamps"));
}

#[test]
fn enum_document_symbols_contain_member_kinds_and_source_ranges() {
    let program = parse_program(SOURCE).unwrap();
    let symbols =
        document_symbols_for_document_with_parsed(SOURCE, None, &HashMap::new(), Some(&program));
    let enumeration = &symbols[0]["children"][0];
    assert_eq!(enumeration["name"], "Mode");
    assert_eq!(enumeration["kind"], 10);
    assert_eq!(enumeration["children"][1]["name"], "HighPass");
    assert_eq!(enumeration["children"][1]["kind"], 22);
    assert_eq!(
        enumeration["children"][1]["selectionRange"]["start"]["line"],
        3
    );
}

#[test]
fn imported_enum_members_navigate_to_the_stdlib_document() {
    let source = "import std/filter\nparams:\n  mode = std::filter::SvfMode::BandPass\n";
    let definition = definition_at(source, "SvfMode::BandPass", "SvfMode::Band".len()).unwrap();
    assert!(definition["uri"]
        .as_str()
        .unwrap()
        .ends_with("/std/filter.onda"));
}
