use onda_frontend::{parse_program_with_path, SourceLoc};
use std::path::Path;

#[test]
fn source_locations_survive_transfer_and_other_compilations() {
    let program = parse_program_with_path(
        "sample:\n  out1 = unknown_value\n",
        Path::new("/tmp/original.onda"),
    )
    .unwrap();
    let location = program.blocks[0].loc();
    drop(program);
    std::thread::spawn(move || {
        let _unrelated = SourceLoc::new(Some("/tmp/unrelated.onda".into()), 1, 1, 1, 2, vec![]);
        assert_eq!(location.file().as_deref(), Some("/tmp/original.onda"));
    })
    .join()
    .unwrap();
}

#[test]
fn source_contexts_do_not_alias_at_sixteen_bit_boundary() {
    let locations: Vec<_> = (0..65_537)
        .map(|index| {
            SourceLoc::new(
                Some(format!("source-{index}.onda")),
                1,
                1,
                1,
                2,
                vec![format!("included at {index}")],
            )
        })
        .collect();
    for index in [0, 65_534, 65_535, 65_536] {
        assert_eq!(
            locations[index].file().unwrap(),
            format!("source-{index}.onda")
        );
        assert_eq!(
            locations[index].trace(),
            vec![format!("included at {index}")]
        );
    }
}
