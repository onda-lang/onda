use super::*;

#[test]
fn deep_source_graphs_load_on_an_ordinary_thread_stack() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let root = Path::new("/virtual");
            let depth = 2048;
            for import in [false, true] {
                let sources = (0..depth)
                    .map(|index| {
                        let next = index + 1;
                        let source = if next == depth {
                            "const Leaf = 1\n".to_owned()
                        } else if import {
                            format!("import m{next}\n")
                        } else {
                            format!("include \"m{next}.onda\"\n")
                        };
                        (root.join(format!("m{index}.onda")), source)
                    })
                    .collect();
                let loaded =
                    load_program_file_from_virtual_sources(root, &root.join("m0.onda"), &sources)
                        .expect("deep graph loads without recursive stack growth");
                assert_eq!(loaded.sources.files.len(), depth);
                assert_eq!(loaded.sources.resolutions.len(), depth - 1);
                assert_eq!(loaded.program.blocks.len(), 1);
            }
        })
        .expect("spawn loader")
        .join()
        .expect("loader completes");
}

#[test]
fn includes_preserve_depth_first_declaration_order() {
    let root = Path::new("/virtual");
    let sources = HashMap::from([
        (
            root.join("main.onda"),
            "const Before = 1\ninclude \"child.onda\"\nconst After = 3\n".to_owned(),
        ),
        (root.join("child.onda"), "const Middle = 2\n".to_owned()),
    ]);
    let loaded = load_program_file_from_virtual_sources(root, &root.join("main.onda"), &sources)
        .expect("load");
    let names = loaded
        .program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Const(declaration) => Some(declaration.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["Before", "Middle", "After"]);
}
