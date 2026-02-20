use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    if env::var_os("CARGO_FEATURE_LLVM_ORC").is_none() {
        return;
    }

    let Some(prefix) = env::var_os("LLVM_SYS_211_PREFIX") else {
        println!("cargo:warning=LLVM_SYS_211_PREFIX is not set; ORC linking may fail");
        return;
    };

    let prefix_path = PathBuf::from(prefix);
    let lib_dir = prefix_path.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=LLVM-C");

    #[cfg(target_os = "windows")]
    copy_llvm_runtime_dlls(&prefix_path);
}

#[cfg(target_os = "windows")]
fn copy_llvm_runtime_dlls(prefix_path: &PathBuf) {
    let bin_dir = prefix_path.join("bin");
    if !bin_dir.exists() {
        println!(
            "cargo:warning=LLVM bin directory does not exist: {}",
            bin_dir.display()
        );
        return;
    }

    let out_dir = match env::var_os("OUT_DIR") {
        Some(v) => PathBuf::from(v),
        None => {
            println!("cargo:warning=OUT_DIR is not set; skipping LLVM DLL staging");
            return;
        }
    };
    let Some(profile_dir) = out_dir.ancestors().nth(3) else {
        println!(
            "cargo:warning=failed to resolve Cargo profile dir from OUT_DIR '{}'",
            out_dir.display()
        );
        return;
    };
    let deps_dir = profile_dir.join("deps");
    if let Err(err) = fs::create_dir_all(&deps_dir) {
        println!(
            "cargo:warning=failed to create deps dir '{}': {err}",
            deps_dir.display()
        );
        return;
    }

    let entries = match fs::read_dir(&bin_dir) {
        Ok(v) => v,
        Err(err) => {
            println!(
                "cargo:warning=failed to list LLVM bin dir '{}': {err}",
                bin_dir.display()
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|v| v.to_str())
            .map(|v| v.eq_ignore_ascii_case("dll"))
            .unwrap_or(false);
        if !ext {
            continue;
        }
        let Some(file_name) = path.file_name() else {
            continue;
        };
        let dst = deps_dir.join(file_name);
        if let Err(err) = fs::copy(&path, &dst) {
            println!(
                "cargo:warning=failed to copy '{}' -> '{}': {err}",
                path.display(),
                dst.display()
            );
        }
    }
}
