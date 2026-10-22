//! Build the target-feature shim and register scalar libm addresses for ORC.
use std::{env, fmt::Write, fs, path::Path};

#[path = "../src/vector_math_spec.rs"]
mod spec;

pub fn build(llvm: &Path) {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=build/vector_math.rs");
    println!("cargo:rerun-if-changed=src/vector_math_spec.rs");
    let out = env::var_os("OUT_DIR").unwrap();
    let out = Path::new(&out);

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .opt_level(2)
        // The shim uses only inline accessors from this LLVM installation's
        // headers; LLVM-C shared libraries need not export C++ Support symbols.
        .define("LLVM_DISABLE_ABI_BREAKING_CHECKS_ENFORCING", "1")
        .include(llvm.join("include"))
        .file("native/target_features.cpp")
        .warnings(false)
        .compile("onda_target_features");

    // Scalar tails still use platform libm. Register addresses for ORC because
    // a statically linked Windows CRT need not export them for dynamic lookup.
    let mut names = Vec::new();
    for &(operation, _, _) in spec::FUNCTIONS {
        names.push(operation.to_owned());
        names.push(format!("{operation}f"));
    }
    let mut symbols = format!(
        "fn scalar_symbols() -> [(&'static CStr, u64); {}] {{\n[\n",
        names.len()
    );
    for name in names {
        // Only the address is observed; LLVM owns the scalar function signature.
        writeln!(symbols, "{{ unsafe extern \"C\" {{ #[link_name = \"{name}\"] fn symbol(); }} (c\"{name}\", symbol as *const () as u64) }},").unwrap();
    }
    symbols.push_str("]\n}\n");
    fs::write(out.join("scalar_math_symbols.rs"), symbols).unwrap();
}
