use super::*;

const DECLARATIONS: &str = "namespace DSP:\n  enum Mode:\n    LowPass\n    HighPass {label = \"High-pass\"}\nuse DSP::Mode as Choice\n";

#[test]
fn qualified_enum_members_complete_while_the_expression_is_incomplete() {
    for qualifier in ["DSP::Mode::", "Choice::"] {
        let source = format!("{DECLARATIONS}params:\n  mode = {qualifier}\n");
        let result = completion_items_for_document_with_index(
            &source,
            None,
            &HashMap::new(),
            None,
            None,
            position_at(&source, qualifier, qualifier.len()),
            true,
        );
        for (name, ordinal) in [("LowPass", 0), ("HighPass", 1)] {
            let item = encoded_item(&result.items, name);
            assert_eq!(item["kind"], 20);
            assert!(item["detail"]
                .as_str()
                .unwrap()
                .contains(&format!("= {ordinal}")));
        }
    }
}

#[test]
fn enum_types_complete_in_annotations_and_namespace_members() {
    for tail in ["struct Settings:\n  value: Ch", "params:\n  mode = DSP::Mo"] {
        let source = format!("{DECLARATIONS}{tail}\n");
        let result = completion_items_for_document_with_index(
            &source,
            None,
            &HashMap::new(),
            None,
            None,
            position_at(&source, tail, tail.len()),
            true,
        );
        let name = if tail.ends_with("Ch") {
            "Choice"
        } else {
            "Mode"
        };
        assert_eq!(encoded_item(&result.items, name)["kind"], 13);
    }
}

#[test]
fn imported_stdlib_enums_offer_their_members() {
    let source = "import std/filter\nparams:\n  mode = std::filter::SvfMode::\n";
    let result = completion_items_for_document_with_index(
        source,
        None,
        &HashMap::new(),
        None,
        None,
        position_at(source, "SvfMode::", "SvfMode::".len()),
        true,
    );
    for name in [
        "LowPass", "HighPass", "BandPass", "Notch", "Peak", "AllPass",
    ] {
        assert_eq!(encoded_item(&result.items, name)["kind"], 20);
    }
}
