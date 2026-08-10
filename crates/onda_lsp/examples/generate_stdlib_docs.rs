use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut check = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--check" => check = true,
            _ => return Err(format!("unknown argument '{arg}'")),
        }
    }

    let generated = onda_lsp::stdlib_docs::generate_stdlib_reference()?;
    let output = repository_root().join("docs/stdlib.md");
    if check {
        let current = fs::read_to_string(&output)
            .map_err(|_| "docs/stdlib.md is missing; run 'npm run docs:stdlib'".to_owned())?;
        if current != generated {
            return Err("docs/stdlib.md is stale; run 'npm run docs:stdlib'".to_owned());
        }
        println!("{} is current", output.display());
        return Ok(());
    }

    fs::write(&output, generated)
        .map_err(|error| format!("failed to write '{}': {error}", output.display()))?;
    println!("generated {}", output.display());
    Ok(())
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
