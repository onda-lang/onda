use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinkMode {
    Shared,
    Static,
}

fn main() {
    if env::var_os("CARGO_FEATURE_LLVM_ORC").is_none() {
        return;
    }

    println!("cargo:rerun-if-env-changed=LLVM_SYS_211_PREFIX");
    println!("cargo:rerun-if-env-changed=OMNI_LLVM_LINK_MODE");

    let Some(prefix) = env::var_os("LLVM_SYS_211_PREFIX") else {
        println!("cargo:warning=LLVM_SYS_211_PREFIX is not set; ORC linking may fail");
        return;
    };

    let prefix_path = PathBuf::from(prefix);
    let lib_dir = prefix_path.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    match detect_link_mode(&prefix_path) {
        LinkMode::Shared => {
            println!("cargo:rustc-link-lib=dylib=LLVM-C");
            #[cfg(target_os = "windows")]
            copy_llvm_runtime_dlls(&prefix_path);
        }
        LinkMode::Static => {
            link_static_llvm_components(&prefix_path);
        }
    }
}

fn detect_link_mode(prefix_path: &Path) -> LinkMode {
    let requested = env::var("OMNI_LLVM_LINK_MODE")
        .ok()
        .map(|v| v.to_ascii_lowercase());

    match requested.as_deref() {
        Some("static") => LinkMode::Static,
        Some("shared") | Some("dynamic") | Some("dylib") => {
            if has_shared_llvm_c(prefix_path) {
                LinkMode::Shared
            } else {
                panic!(
                    "OMNI_LLVM_LINK_MODE=shared requested, but no shared LLVM-C runtime found under '{}'.",
                    prefix_path.display()
                );
            }
        }
        Some("auto") | None => {
            if has_shared_llvm_c(prefix_path) {
                LinkMode::Shared
            } else {
                LinkMode::Static
            }
        }
        Some(other) => {
            panic!("Unsupported OMNI_LLVM_LINK_MODE='{other}'. Use auto|shared|static.");
        }
    }
}

fn has_shared_llvm_c(prefix_path: &Path) -> bool {
    let lib_dir = prefix_path.join("lib");
    let bin_dir = prefix_path.join("bin");

    let has_import_or_stub = lib_dir.join("LLVM-C.lib").exists()
        || lib_dir.join("libLLVM-C.so").exists()
        || lib_dir.join("libLLVM-C.dylib").exists();

    let has_runtime = bin_dir.join("LLVM-C.dll").exists()
        || bin_dir.join("libLLVM-C.so").exists()
        || bin_dir.join("libLLVM-C.dylib").exists()
        || lib_dir.join("libLLVM-C.so").exists()
        || lib_dir.join("libLLVM-C.dylib").exists();

    has_import_or_stub && has_runtime
}

fn link_static_llvm_components(prefix_path: &Path) {
    let llvm_config = llvm_config_path(prefix_path);
    let llvm_libs = run_llvm_config(
        &llvm_config,
        &[
            "--link-static",
            "--libnames",
            "core",
            "orcjit",
            "native",
            "passes",
        ],
    );
    let system_libs = run_llvm_config(
        &llvm_config,
        &[
            "--link-static",
            "--system-libs",
            "core",
            "orcjit",
            "native",
            "passes",
        ],
    );

    for token in llvm_libs.split_whitespace() {
        if let Some(name) = parse_lib_token(token) {
            println!("cargo:rustc-link-lib=static={name}");
        }
    }

    for token in system_libs.split_whitespace() {
        if let Some(name) = parse_lib_token(token) {
            println!("cargo:rustc-link-lib=dylib={name}");
        }
    }
}

fn llvm_config_path(prefix_path: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    let path = prefix_path.join("bin").join("llvm-config.exe");
    #[cfg(not(target_os = "windows"))]
    let path = prefix_path.join("bin").join("llvm-config");

    if !path.exists() {
        panic!(
            "llvm-config not found at '{}'. Set LLVM_SYS_211_PREFIX to a valid LLVM install.",
            path.display()
        );
    }

    path
}

fn run_llvm_config(llvm_config: &Path, args: &[&str]) -> String {
    let out = Command::new(llvm_config)
        .args(args)
        .output()
        .unwrap_or_else(|err| {
            panic!(
                "failed to execute '{}' with args {:?}: {err}",
                llvm_config.display(),
                args
            )
        });

    if !out.status.success() {
        panic!(
            "llvm-config failed with args {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn parse_lib_token(token: &str) -> Option<String> {
    let t = token.trim().trim_matches('"');
    if t.is_empty() {
        return None;
    }

    if let Some(rest) = t.strip_prefix("-l") {
        return Some(rest.to_string());
    }

    let name = Path::new(t).file_stem()?.to_str()?.to_string();
    if let Some(stripped) = name.strip_prefix("lib") {
        return Some(stripped.to_string());
    }
    Some(name)
}

#[cfg(target_os = "windows")]
fn copy_llvm_runtime_dlls(prefix_path: &Path) {
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
