fn main() {
    println!("cargo:rerun-if-changed=../../assets/windows/onda.rc");
    println!("cargo:rerun-if-changed=../../assets/windows/onda.ico");

    embed_resource::compile("../../assets/windows/onda.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("failed to embed the Windows application icon");
}
