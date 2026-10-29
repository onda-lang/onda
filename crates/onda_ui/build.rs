//! Generate compact character coverage so Wasm can request only needed fonts.
use std::{collections::BTreeSet, env, fmt::Write, fs, path::PathBuf};
fn main() {
    let directory = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../assets/fonts/fallback");
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ttf" || e == "otf"))
        .collect();
    files.sort();
    let mut output = String::from("pub(crate) const FALLBACKS: &[Fallback] = &[\n");
    for path in files {
        let data = fs::read(&path).unwrap();
        let face = ttf_parser::Face::parse(&data, 0).expect("valid bundled font");
        let mut points = BTreeSet::new();
        for table in face.tables().cmap.unwrap().subtables {
            if table.is_unicode() {
                table.codepoints(|point| {
                    if char::from_u32(point)
                        .and_then(|c| face.glyph_index(c))
                        .is_some()
                    {
                        points.insert(point);
                    }
                });
            }
        }
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for point in points {
            if let Some(last) = ranges.last_mut().filter(|r| r.1 + 1 == point) {
                last.1 = point;
            } else {
                ranges.push((point, point));
            }
        }
        writeln!(output,"Fallback {{name:{:?}, ranges:&{:?}, #[cfg(not(target_arch=\"wasm32\"))] data:include_bytes!({:?})}},",
            path.file_name().unwrap().to_str().unwrap(),ranges,path.canonicalize().unwrap().to_str().unwrap()).unwrap();
    }
    output.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("fallbacks.rs"),
        output,
    )
    .unwrap();
}
