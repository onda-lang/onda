use std::env;
use std::fs;
use std::path::PathBuf;

use onda_compiler_web::compile_source_to_mir_messagepack;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let input = PathBuf::from(args.next().ok_or(
        "usage: compile_file_to_mir <input.onda> <output.mir.msgpack> <sample-rate> <block-size>",
    )?);
    let output = PathBuf::from(
        args.next()
            .ok_or("compile_file_to_mir requires an output path")?,
    );
    let sample_rate = args
        .next()
        .ok_or("compile_file_to_mir requires a sample rate")?
        .to_string_lossy()
        .parse::<f32>()
        .map_err(|error| format!("invalid sample rate: {error}"))?;
    let block_size = args
        .next()
        .ok_or("compile_file_to_mir requires a block size")?
        .to_string_lossy()
        .parse::<u32>()
        .map_err(|error| format!("invalid block size: {error}"))?;
    if args.next().is_some() {
        return Err("compile_file_to_mir received unexpected arguments".to_owned());
    }

    let source = fs::read_to_string(&input)
        .map_err(|error| format!("failed to read '{}': {error}", input.display()))?;
    let mir = compile_source_to_mir_messagepack(&source, sample_rate, block_size).map_err(
        |diagnostics| {
            diagnostics
                .into_iter()
                .map(|diagnostic| {
                    format!(
                        "{}:{}:{} [{}] {}",
                        input.display(),
                        diagnostic.line,
                        diagnostic.column,
                        diagnostic.stage,
                        diagnostic.message,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        },
    )?;
    fs::write(&output, mir)
        .map_err(|error| format!("failed to write '{}': {error}", output.display()))?;
    println!("Wrote MIR MessagePack: {}", output.display());
    Ok(())
}
