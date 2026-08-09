use std::fs;
use std::path::{Path, PathBuf};

use onda_codegen_llvm::{TargetCodeModel, TargetCpu, TargetOptLevel, TargetRelocMode};

use super::*;

#[derive(Debug, Clone)]
struct LoadedTargetSpec {
    target: TargetConfig,
    cpu_explicit: bool,
    features_explicit: bool,
}

impl Default for LoadedTargetSpec {
    fn default() -> Self {
        Self {
            target: TargetConfig::host(),
            cpu_explicit: false,
            features_explicit: false,
        }
    }
}

#[derive(Debug, Clone)]
enum TargetSpecValue {
    String(String),
    Integer(i64),
}

pub(crate) fn parse_args(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.skip(1);
    let Some(cmd) = args.next() else {
        return Ok(Command::Run(parse_run_window_args(std::iter::empty())?));
    };
    if cmd == "--help" || cmd == "-h" || cmd == "help" {
        return Err(usage().to_owned());
    }

    match cmd.as_str() {
        "project" => parse_project_args(args),
        "compile" => parse_compile_args(args),
        "lsp" => parse_lsp_args(args),
        "run" => parse_run_args(args),
        "daemon" => parse_daemon_args(args),
        _ => Err(format!("unknown command '{cmd}'\n{}", usage())),
    }
}

fn parse_project_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(destination) = args.next() else {
        return Err(format!(
            "project requires a destination directory\n\n{}",
            usage()
        ));
    };
    if destination == "--help" || destination == "-h" {
        return Err(usage().to_owned());
    }
    let mut source = None;
    let mut buffer_bindings = Vec::new();
    while let Some(option) = args.next() {
        match option.as_str() {
            "--from" => {
                let Some(value) = args.next() else {
                    return Err("--from requires an Onda source path".to_owned());
                };
                if source.replace(PathBuf::from(value)).is_some() {
                    return Err("--from may only be specified once".to_owned());
                }
            }
            "--buffer" => {
                let Some(value) = args.next() else {
                    return Err("--buffer requires a name=path pair".to_owned());
                };
                buffer_bindings.push(parse_buffer_binding(&value)?);
            }
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if option.starts_with("--from=") => {
                let value = &option["--from=".len()..];
                if value.is_empty() {
                    return Err("--from requires an Onda source path".to_owned());
                }
                if source.replace(PathBuf::from(value)).is_some() {
                    return Err("--from may only be specified once".to_owned());
                }
            }
            _ if option.starts_with("--buffer=") => {
                buffer_bindings.push(parse_buffer_binding(&option["--buffer=".len()..])?);
            }
            _ => {
                return Err(format!(
                    "unknown option '{option}' for onda project\n\n{}",
                    usage()
                ))
            }
        }
    }
    if source.is_none() && !buffer_bindings.is_empty() {
        return Err("--buffer requires --from when creating a project".to_owned());
    }
    Ok(Command::Project {
        destination: PathBuf::from(destination),
        source,
        buffer_bindings,
    })
}

fn parse_lsp_args(args: impl Iterator<Item = String>) -> Result<Command, String> {
    for arg in args {
        match arg.as_str() {
            "--stdio" => {}
            "--help" | "-h" => return Err(usage().to_owned()),
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }
    Ok(Command::Lsp)
}

fn parse_run_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let run = match args.next() {
        Some(subcommand) if subcommand == "play" => parse_run_play_args(args)?,
        Some(subcommand) if subcommand == "render" => parse_run_render_args(args)?,
        Some(first) => parse_run_window_args(std::iter::once(first).chain(args))?,
        None => parse_run_window_args(std::iter::empty())?,
    };
    Ok(Command::Run(run))
}

fn parse_daemon_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(subcommand) = args.next() else {
        return Err(format!("daemon requires a subcommand\n\n{}", usage()));
    };
    let daemon = match subcommand.as_str() {
        "stdio" => DaemonCommand::Stdio,
        "diagnose" => parse_daemon_diagnose_args(args)?,
        "play" => {
            return Err(format!(
                "daemon play was renamed to 'onda run play'\n\n{}",
                usage()
            ))
        }
        "run" => {
            return Err(format!(
                "daemon run was renamed to 'onda run render'\n\n{}",
                usage()
            ))
        }
        _ => {
            return Err(format!(
                "unknown daemon subcommand '{subcommand}'\n\n{}",
                usage()
            ))
        }
    };
    Ok(Command::Daemon(daemon))
}

fn parse_compile_args(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let Some(input) = args.next() else {
        return Err(format!(
            "compile requires an input source or project\n\n{}",
            usage()
        ));
    };
    let mut emit = CompileEmit::Check;
    let mut emit_explicit = false;
    let mut output = None::<PathBuf>;
    let mut meta_out = None::<PathBuf>;
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_BLOCK_FRAMES;
    let mut dump_graph = false;
    let mut show_meta = false;
    let mut fast_math = false;
    let mut target_spec_path = None::<PathBuf>;
    let mut target_triple_override = None::<String>;
    let mut target_cpu_override = None::<TargetCpu>;
    let mut target_features_override = None::<String>;
    let mut target_abi_override = None::<String>;
    let mut reloc_model_override = None::<TargetRelocMode>;
    let mut code_model_override = None::<TargetCodeModel>;
    let mut opt_level_override = None::<TargetOptLevel>;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--emit" => {
                let Some(value) = args.next() else {
                    return Err(
                        "--emit requires check, mir, mir-json, mir-messagepack, llvm-ir, or obj"
                            .to_owned(),
                    );
                };
                if emit == CompileEmit::LlvmIr && !emit_explicit {
                    return Err("cannot use both --ir and --emit".to_owned());
                }
                emit = parse_compile_emit(&value)?;
                emit_explicit = true;
            }
            "--output" | "-o" => {
                let Some(value) = args.next() else {
                    return Err("--output requires a file path".to_owned());
                };
                output = Some(PathBuf::from(value));
            }
            "--meta-out" => {
                let Some(value) = args.next() else {
                    return Err("--meta-out requires a file path".to_owned());
                };
                meta_out = Some(PathBuf::from(value));
            }
            "--sample-rate" | "--sr" => {
                let Some(value) = args.next() else {
                    return Err("--sample-rate/--sr requires a positive integer value".to_owned());
                };
                sample_rate_hz = parse_sample_rate_hz(&value)?;
            }
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--dump-graph" => dump_graph = true,
            "--ir" => {
                if emit_explicit {
                    return Err("cannot use both --ir and --emit".to_owned());
                }
                emit = CompileEmit::LlvmIr;
            }
            "--meta" => show_meta = true,
            "--fast-math" => fast_math = true,
            "--target-triple" => {
                let Some(value) = args.next() else {
                    return Err("--target-triple requires a target triple".to_owned());
                };
                target_triple_override = Some(value);
            }
            "--target-spec" => {
                let Some(value) = args.next() else {
                    return Err("--target-spec requires a TOML file path".to_owned());
                };
                target_spec_path = Some(PathBuf::from(value));
            }
            "--target-cpu" => {
                let Some(value) = args.next() else {
                    return Err("--target-cpu requires a CPU name or 'host'".to_owned());
                };
                target_cpu_override = Some(parse_target_cpu(&value));
            }
            "--target-features" => {
                let Some(value) = args.next() else {
                    return Err(
                        "--target-features requires a comma-separated feature list".to_owned()
                    );
                };
                target_features_override = Some(value);
            }
            "--target-abi" => {
                let Some(value) = args.next() else {
                    return Err("--target-abi requires a non-empty ABI name".to_owned());
                };
                target_abi_override = Some(value);
            }
            "--reloc-model" => {
                let Some(value) = args.next() else {
                    return Err("--reloc-model requires a value".to_owned());
                };
                reloc_model_override = Some(parse_target_reloc_model(&value)?);
            }
            "--code-model" => {
                let Some(value) = args.next() else {
                    return Err("--code-model requires a value".to_owned());
                };
                code_model_override = Some(parse_target_code_model(&value)?);
            }
            "--opt-level" => {
                let Some(value) = args.next() else {
                    return Err("--opt-level requires a value".to_owned());
                };
                opt_level_override = Some(parse_target_opt_level(&value)?);
            }
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if arg.starts_with("--sample-rate=") => {
                let value = &arg["--sample-rate=".len()..];
                sample_rate_hz = parse_sample_rate_hz(value)?;
            }
            _ if arg.starts_with("--emit=") => {
                if emit == CompileEmit::LlvmIr && !emit_explicit {
                    return Err("cannot use both --ir and --emit".to_owned());
                }
                let value = &arg["--emit=".len()..];
                emit = parse_compile_emit(value)?;
                emit_explicit = true;
            }
            _ if arg.starts_with("--output=") => {
                let value = &arg["--output=".len()..];
                if value.is_empty() {
                    return Err("--output requires a file path".to_owned());
                }
                output = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--meta-out=") => {
                let value = &arg["--meta-out=".len()..];
                if value.is_empty() {
                    return Err("--meta-out requires a file path".to_owned());
                }
                meta_out = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--sr=") => {
                let value = &arg["--sr=".len()..];
                sample_rate_hz = parse_sample_rate_hz(value)?;
            }
            _ if arg.starts_with("--block-size=") => {
                let value = &arg["--block-size=".len()..];
                block_frames = parse_block_frames(value)?;
            }
            _ if arg.starts_with("--target-triple=") => {
                let value = &arg["--target-triple=".len()..];
                if value.is_empty() {
                    return Err("--target-triple requires a target triple".to_owned());
                }
                target_triple_override = Some(value.to_owned());
            }
            _ if arg.starts_with("--target-spec=") => {
                let value = &arg["--target-spec=".len()..];
                if value.is_empty() {
                    return Err("--target-spec requires a TOML file path".to_owned());
                }
                target_spec_path = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--target-cpu=") => {
                let value = &arg["--target-cpu=".len()..];
                target_cpu_override = Some(parse_target_cpu(value));
            }
            _ if arg.starts_with("--target-features=") => {
                let value = &arg["--target-features=".len()..];
                target_features_override = Some(value.to_owned());
            }
            _ if arg.starts_with("--target-abi=") => {
                let value = &arg["--target-abi=".len()..];
                if value.is_empty() {
                    return Err("--target-abi requires a non-empty ABI name".to_owned());
                }
                target_abi_override = Some(value.to_owned());
            }
            _ if arg.starts_with("--reloc-model=") => {
                let value = &arg["--reloc-model=".len()..];
                reloc_model_override = Some(parse_target_reloc_model(value)?);
            }
            _ if arg.starts_with("--code-model=") => {
                let value = &arg["--code-model=".len()..];
                code_model_override = Some(parse_target_code_model(value)?);
            }
            _ if arg.starts_with("--opt-level=") => {
                let value = &arg["--opt-level=".len()..];
                opt_level_override = Some(parse_target_opt_level(value)?);
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }
    let LoadedTargetSpec {
        mut target,
        mut cpu_explicit,
        mut features_explicit,
    } = match target_spec_path.as_deref() {
        Some(path) => load_target_spec(path)?,
        None => LoadedTargetSpec::default(),
    };

    if let Some(triple) = target_triple_override {
        target.triple = Some(triple);
    }
    if let Some(cpu) = target_cpu_override {
        target.cpu = cpu;
        cpu_explicit = true;
    }
    if let Some(features) = target_features_override {
        target.features = Some(features);
        features_explicit = true;
    }
    if let Some(abi_name) = target_abi_override {
        target.abi_name = Some(abi_name);
    }
    if let Some(reloc_model) = reloc_model_override {
        target.reloc_model = reloc_model;
    }
    if let Some(code_model) = code_model_override {
        target.code_model = code_model;
    }
    if let Some(opt_level) = opt_level_override {
        target.opt_level = opt_level;
    }

    if target.triple.is_some() && !cpu_explicit {
        target.cpu = TargetCpu::Explicit("generic".to_owned());
    }
    if target.triple.is_some() && !features_explicit && matches!(target.cpu, TargetCpu::Explicit(_))
    {
        target.features = Some(String::new());
    }
    Ok(Command::Compile {
        input: PathBuf::from(input),
        emit,
        output,
        meta_out,
        sample_rate_hz,
        block_frames,
        dump_graph,
        show_meta,
        fast_math,
        target,
    })
}

fn parse_daemon_diagnose_args(
    mut args: impl Iterator<Item = String>,
) -> Result<DaemonCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!(
            "daemon diagnose requires an input file\n\n{}",
            usage()
        ));
    };
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_BLOCK_FRAMES;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sample-rate" | "--sr" => {
                let Some(value) = args.next() else {
                    return Err("--sample-rate/--sr requires a positive integer value".to_owned());
                };
                sample_rate_hz = parse_sample_rate_hz(&value)?;
            }
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block-size=") => {
                block_frames = parse_block_frames(&arg["--block-size=".len()..])?;
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }
    Ok(DaemonCommand::Diagnose {
        input: PathBuf::from(input),
        sample_rate_hz,
        block_frames,
    })
}

fn parse_run_render_args(mut args: impl Iterator<Item = String>) -> Result<RunCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!("run render requires an input file\n\n{}", usage()));
    };

    let mut output = PathBuf::from(DEFAULT_DAEMON_OUTPUT);
    let mut dur_seconds = DEFAULT_DUR_SECONDS;
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_BLOCK_FRAMES;
    let mut opt_level = TargetOptLevel::O3;
    let mut fast_math = false;
    let mut show_meta = false;
    let mut param_sets = Vec::new();
    let mut buffer_bindings = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" | "-o" => {
                let Some(value) = args.next() else {
                    return Err("--output requires a file path".to_owned());
                };
                output = PathBuf::from(value);
            }
            "--dur" | "-d" => {
                let Some(value) = args.next() else {
                    return Err("--dur requires a positive integer value".to_owned());
                };
                dur_seconds = parse_dur_seconds(&value)?;
            }
            "--sample-rate" | "--sr" => {
                let Some(value) = args.next() else {
                    return Err("--sample-rate/--sr requires a positive integer value".to_owned());
                };
                sample_rate_hz = parse_sample_rate_hz(&value)?;
            }
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--opt-level" => {
                let Some(value) = args.next() else {
                    return Err("--opt-level requires a value".to_owned());
                };
                opt_level = parse_target_opt_level(&value)?;
            }
            "--set" => {
                let Some(value) = args.next() else {
                    return Err("--set requires a name=value pair".to_owned());
                };
                param_sets.push(parse_param_setting(&value)?);
            }
            "--buffer" => {
                let Some(value) = args.next() else {
                    return Err("--buffer requires a name=path pair".to_owned());
                };
                buffer_bindings.push(parse_buffer_binding(&value)?);
            }
            "--fast-math" => fast_math = true,
            "--meta" => show_meta = true,
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if arg.starts_with("--output=") => {
                output = PathBuf::from(&arg["--output=".len()..]);
            }
            _ if arg.starts_with("--dur=") => {
                dur_seconds = parse_dur_seconds(&arg["--dur=".len()..])?;
            }
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block-size=") => {
                block_frames = parse_block_frames(&arg["--block-size=".len()..])?;
            }
            _ if arg.starts_with("--opt-level=") => {
                opt_level = parse_target_opt_level(&arg["--opt-level=".len()..])?;
            }
            _ if arg.starts_with("--set=") => {
                param_sets.push(parse_param_setting(&arg["--set=".len()..])?);
            }
            _ if arg.starts_with("--buffer=") => {
                buffer_bindings.push(parse_buffer_binding(&arg["--buffer=".len()..])?);
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }

    Ok(RunCommand::Render {
        input: PathBuf::from(input),
        output,
        dur_seconds,
        sample_rate_hz,
        block_frames,
        opt_level,
        fast_math,
        show_meta,
        param_sets,
        buffer_bindings,
    })
}

fn parse_run_window_args(mut args: impl Iterator<Item = String>) -> Result<RunCommand, String> {
    let mut input = None;
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_PLAY_BLOCK_FRAMES;
    let mut opt_level = TargetOptLevel::O3;
    let mut input_device = None;
    let mut output_device = None;
    let mut fast_math = false;
    let mut show_meta = false;
    let mut theme = RunThemeMode::Auto;
    let mut host = RunHostKind::Auto;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sample-rate" | "--sr" => {
                let Some(value) = args.next() else {
                    return Err("--sample-rate/--sr requires a positive integer value".to_owned());
                };
                sample_rate_hz = parse_sample_rate_hz(&value)?;
            }
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--opt-level" => {
                let Some(value) = args.next() else {
                    return Err("--opt-level requires a value".to_owned());
                };
                opt_level = parse_target_opt_level(&value)?;
            }
            "--input-device" => {
                let Some(value) = args.next() else {
                    return Err("--input-device requires a device name".to_owned());
                };
                input_device = Some(value);
            }
            "--output-device" => {
                let Some(value) = args.next() else {
                    return Err("--output-device requires a device name".to_owned());
                };
                output_device = Some(value);
            }
            "--theme" => {
                let Some(value) = args.next() else {
                    return Err("--theme requires one of: auto, dark, light".to_owned());
                };
                theme = parse_run_theme_mode(&value)?;
            }
            "--webview" => host = RunHostKind::Webview,
            "--fast-math" => fast_math = true,
            "--meta" => show_meta = true,
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block-size=") => {
                block_frames = parse_block_frames(&arg["--block-size=".len()..])?;
            }
            _ if arg.starts_with("--opt-level=") => {
                opt_level = parse_target_opt_level(&arg["--opt-level=".len()..])?;
            }
            _ if arg.starts_with("--input-device=") => {
                input_device = Some(arg["--input-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--output-device=") => {
                output_device = Some(arg["--output-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--theme=") => {
                theme = parse_run_theme_mode(&arg["--theme=".len()..])?;
            }
            _ if arg.starts_with('-') => {
                return Err(format!("unknown option '{arg}'\n\n{}", usage()));
            }
            _ if input.is_none() => input = Some(PathBuf::from(arg)),
            _ => return Err(format!("unexpected input file '{arg}'\n\n{}", usage())),
        }
    }

    Ok(RunCommand::Window {
        input,
        sample_rate_hz,
        block_frames,
        opt_level,
        input_device,
        output_device,
        fast_math,
        show_meta,
        theme,
        host,
    })
}

fn parse_run_theme_mode(value: &str) -> Result<RunThemeMode, String> {
    match value {
        "auto" => Ok(RunThemeMode::Auto),
        "dark" => Ok(RunThemeMode::Dark),
        "light" => Ok(RunThemeMode::Light),
        _ => Err(format!(
            "invalid --theme value '{value}'; expected auto, dark, or light"
        )),
    }
}

fn parse_run_play_args(mut args: impl Iterator<Item = String>) -> Result<RunCommand, String> {
    let Some(input) = args.next() else {
        return Err(format!("run play requires an input file\n\n{}", usage()));
    };

    let mut dur_seconds = Some(DEFAULT_DUR_SECONDS);
    let mut sample_rate_hz = DEFAULT_SAMPLE_RATE;
    let mut block_frames = DEFAULT_PLAY_BLOCK_FRAMES;
    let mut opt_level = TargetOptLevel::O3;
    let mut input_device = None;
    let mut output_device = None;
    let mut fast_math = false;
    let mut show_meta = false;
    let mut control_json = false;
    let mut param_sets = Vec::new();
    let mut buffer_bindings = Vec::new();
    let mut forever = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dur" | "-d" => {
                let Some(value) = args.next() else {
                    return Err("--dur requires a positive integer value".to_owned());
                };
                if forever {
                    return Err("--dur cannot be combined with --forever".to_owned());
                }
                dur_seconds = Some(parse_dur_seconds(&value)?);
            }
            "--sample-rate" | "--sr" => {
                let Some(value) = args.next() else {
                    return Err("--sample-rate/--sr requires a positive integer value".to_owned());
                };
                sample_rate_hz = parse_sample_rate_hz(&value)?;
            }
            "--block-size" | "-b" => {
                let Some(value) = args.next() else {
                    return Err("--block-size requires a positive integer value".to_owned());
                };
                block_frames = parse_block_frames(&value)?;
            }
            "--opt-level" => {
                let Some(value) = args.next() else {
                    return Err("--opt-level requires a value".to_owned());
                };
                opt_level = parse_target_opt_level(&value)?;
            }
            "--set" => {
                let Some(value) = args.next() else {
                    return Err("--set requires a name=value pair".to_owned());
                };
                param_sets.push(parse_param_setting(&value)?);
            }
            "--buffer" => {
                let Some(value) = args.next() else {
                    return Err("--buffer requires a name=path pair".to_owned());
                };
                buffer_bindings.push(parse_buffer_binding(&value)?);
            }
            "--input-device" => {
                let Some(value) = args.next() else {
                    return Err("--input-device requires a device name".to_owned());
                };
                input_device = Some(value);
            }
            "--output-device" => {
                let Some(value) = args.next() else {
                    return Err("--output-device requires a device name".to_owned());
                };
                output_device = Some(value);
            }
            "--forever" => {
                if dur_seconds != Some(DEFAULT_DUR_SECONDS) {
                    return Err("--forever cannot be combined with --dur".to_owned());
                }
                forever = true;
                dur_seconds = None;
            }
            "--fast-math" => fast_math = true,
            "--meta" => show_meta = true,
            "--control-json" => control_json = true,
            "--help" | "-h" => return Err(usage().to_owned()),
            _ if arg.starts_with("--dur=") => {
                if forever {
                    return Err("--dur cannot be combined with --forever".to_owned());
                }
                dur_seconds = Some(parse_dur_seconds(&arg["--dur=".len()..])?);
            }
            _ if arg.starts_with("--sample-rate=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sample-rate=".len()..])?;
            }
            _ if arg.starts_with("--sr=") => {
                sample_rate_hz = parse_sample_rate_hz(&arg["--sr=".len()..])?;
            }
            _ if arg.starts_with("--block-size=") => {
                block_frames = parse_block_frames(&arg["--block-size=".len()..])?;
            }
            _ if arg.starts_with("--opt-level=") => {
                opt_level = parse_target_opt_level(&arg["--opt-level=".len()..])?;
            }
            _ if arg.starts_with("--input-device=") => {
                input_device = Some(arg["--input-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--output-device=") => {
                output_device = Some(arg["--output-device=".len()..].to_owned());
            }
            _ if arg.starts_with("--set=") => {
                param_sets.push(parse_param_setting(&arg["--set=".len()..])?);
            }
            _ if arg.starts_with("--buffer=") => {
                buffer_bindings.push(parse_buffer_binding(&arg["--buffer=".len()..])?);
            }
            _ => return Err(format!("unknown option '{arg}'\n\n{}", usage())),
        }
    }

    Ok(RunCommand::Play {
        input: PathBuf::from(input),
        dur_seconds,
        sample_rate_hz,
        block_frames,
        opt_level,
        input_device,
        output_device,
        fast_math,
        show_meta,
        control_json,
        param_sets,
        buffer_bindings,
    })
}

fn parse_dur_seconds(value: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("invalid duration '{value}', expected positive integer seconds"))?;
    if parsed == 0 {
        return Err("duration must be greater than zero".to_owned());
    }
    Ok(parsed)
}

fn parse_sample_rate_hz(value: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("invalid sample rate '{value}', expected positive integer Hz"))?;
    if parsed == 0 {
        return Err("sample rate must be greater than zero".to_owned());
    }
    Ok(parsed)
}

fn parse_block_frames(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid block size '{value}', expected positive integer frames"))?;
    if parsed == 0 {
        return Err("block size must be greater than zero".to_owned());
    }
    Ok(parsed)
}

fn parse_compile_emit(value: &str) -> Result<CompileEmit, String> {
    match value {
        "check" => Ok(CompileEmit::Check),
        "mir" => Ok(CompileEmit::Mir),
        "mir-json" => Ok(CompileEmit::MirJson),
        "mir-messagepack" => Ok(CompileEmit::MirMessagePack),
        "llvm-ir" => Ok(CompileEmit::LlvmIr),
        "obj" => Ok(CompileEmit::Object),
        _ => Err(format!(
            "invalid compile emit mode '{value}', expected check|mir|mir-json|mir-messagepack|llvm-ir|obj"
        )),
    }
}

fn parse_target_cpu(value: &str) -> TargetCpu {
    if value.eq_ignore_ascii_case("host") {
        TargetCpu::Host
    } else {
        TargetCpu::Explicit(value.to_owned())
    }
}

fn parse_target_reloc_model(value: &str) -> Result<TargetRelocMode, String> {
    match value {
        "default" => Ok(TargetRelocMode::Default),
        "static" => Ok(TargetRelocMode::Static),
        "pic" => Ok(TargetRelocMode::Pic),
        "dynamic-no-pic" => Ok(TargetRelocMode::DynamicNoPic),
        _ => Err(format!(
            "invalid reloc model '{value}', expected default|static|pic|dynamic-no-pic"
        )),
    }
}

fn parse_target_code_model(value: &str) -> Result<TargetCodeModel, String> {
    match value {
        "default" => Ok(TargetCodeModel::Default),
        "small" => Ok(TargetCodeModel::Small),
        "kernel" => Ok(TargetCodeModel::Kernel),
        "medium" => Ok(TargetCodeModel::Medium),
        "large" => Ok(TargetCodeModel::Large),
        _ => Err(format!(
            "invalid code model '{value}', expected default|small|kernel|medium|large"
        )),
    }
}

fn parse_target_opt_level(value: &str) -> Result<TargetOptLevel, String> {
    match value {
        "0" => Ok(TargetOptLevel::O0),
        "1" => Ok(TargetOptLevel::O1),
        "2" => Ok(TargetOptLevel::O2),
        "3" => Ok(TargetOptLevel::O3),
        _ => Err(format!(
            "invalid optimization level '{value}', expected 0|1|2|3"
        )),
    }
}

fn load_target_spec(path: &Path) -> Result<LoadedTargetSpec, String> {
    let text = fs::read_to_string(path)
        .map_err(|err| format!("failed to read target spec '{}': {err}", path.display()))?;
    let mut loaded = LoadedTargetSpec::default();
    let mut reloc_model_explicit = false;
    let mut code_model_explicit = false;
    let mut opt_level_explicit = false;

    for (line_idx, line) in text.lines().enumerate() {
        let line_no = line_idx + 1;
        let line = strip_target_spec_comment(line).trim();
        if line.is_empty() {
            continue;
        }

        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(format!(
                "failed to parse target spec '{}': expected 'key = value' at line {}",
                path.display(),
                line_no
            ));
        };
        let key = raw_key.trim();
        let value = parse_target_spec_value(raw_value.trim(), path, line_no)?;

        match key {
            "triple" => {
                if loaded.target.triple.is_some() {
                    return Err(format!(
                        "target spec '{}' defines 'triple' more than once",
                        path.display()
                    ));
                }
                let triple = expect_target_spec_string(&value, path, line_no, "triple")?;
                if triple.trim().is_empty() {
                    return Err(format!(
                        "target spec '{}' has an empty 'triple' field",
                        path.display()
                    ));
                }
                loaded.target.triple = Some(triple);
            }
            "cpu" => {
                if loaded.cpu_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'cpu' more than once",
                        path.display()
                    ));
                }
                let cpu = expect_target_spec_string(&value, path, line_no, "cpu")?;
                if cpu.trim().is_empty() {
                    return Err(format!(
                        "target spec '{}' has an empty 'cpu' field",
                        path.display()
                    ));
                }
                loaded.target.cpu = parse_target_cpu(&cpu);
                loaded.cpu_explicit = true;
            }
            "features" => {
                if loaded.features_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'features' more than once",
                        path.display()
                    ));
                }
                loaded.target.features = Some(expect_target_spec_string(
                    &value, path, line_no, "features",
                )?);
                loaded.features_explicit = true;
            }
            "abi_name" => {
                if loaded.target.abi_name.is_some() {
                    return Err(format!(
                        "target spec '{}' defines 'abi_name' more than once",
                        path.display()
                    ));
                }
                let abi_name = expect_target_spec_string(&value, path, line_no, "abi_name")?;
                let abi_name = abi_name.trim();
                if !abi_name.is_empty() {
                    loaded.target.abi_name = Some(abi_name.to_owned());
                }
            }
            "reloc_model" => {
                if reloc_model_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'reloc_model' more than once",
                        path.display()
                    ));
                }
                let reloc_model = expect_target_spec_string(&value, path, line_no, "reloc_model")?;
                loaded.target.reloc_model = parse_target_reloc_model(&reloc_model)?;
                reloc_model_explicit = true;
            }
            "code_model" => {
                if code_model_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'code_model' more than once",
                        path.display()
                    ));
                }
                let code_model = expect_target_spec_string(&value, path, line_no, "code_model")?;
                loaded.target.code_model = parse_target_code_model(&code_model)?;
                code_model_explicit = true;
            }
            "opt_level" => {
                if opt_level_explicit {
                    return Err(format!(
                        "target spec '{}' defines 'opt_level' more than once",
                        path.display()
                    ));
                }
                loaded.target.opt_level = match value {
                    TargetSpecValue::Integer(value) => parse_target_opt_level(&value.to_string())?,
                    TargetSpecValue::String(value) => parse_target_opt_level(&value)?,
                };
                opt_level_explicit = true;
            }
            _ => {
                return Err(format!(
                    "failed to parse target spec '{}': unsupported key '{}' at line {}",
                    path.display(),
                    key,
                    line_no
                ));
            }
        }
    }

    Ok(loaded)
}

fn strip_target_spec_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in line.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '#' => return &line[..idx],
            _ => {}
        }
    }

    line
}

fn parse_target_spec_value(
    raw: &str,
    path: &Path,
    line_no: usize,
) -> Result<TargetSpecValue, String> {
    if raw.starts_with('"') {
        return Ok(TargetSpecValue::String(parse_target_spec_string_literal(
            raw, path, line_no,
        )?));
    }

    let value = raw.parse::<i64>().map_err(|_| {
        format!(
            "failed to parse target spec '{}': expected string or integer literal at line {}",
            path.display(),
            line_no
        )
    })?;
    Ok(TargetSpecValue::Integer(value))
}

fn parse_target_spec_string_literal(
    raw: &str,
    path: &Path,
    line_no: usize,
) -> Result<String, String> {
    if !raw.ends_with('"') || raw.len() < 2 {
        return Err(format!(
            "failed to parse target spec '{}': invalid string literal at line {}",
            path.display(),
            line_no
        ));
    }

    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }

        let Some(escaped) = chars.next() else {
            return Err(format!(
                "failed to parse target spec '{}': dangling escape in string literal at line {}",
                path.display(),
                line_no
            ));
        };
        match escaped {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            _ => {
                return Err(format!(
                    "failed to parse target spec '{}': unsupported escape '\\{}' at line {}",
                    path.display(),
                    escaped,
                    line_no
                ));
            }
        }
    }

    Ok(out)
}

fn expect_target_spec_string(
    value: &TargetSpecValue,
    path: &Path,
    line_no: usize,
    key: &str,
) -> Result<String, String> {
    match value {
        TargetSpecValue::String(value) => Ok(value.clone()),
        TargetSpecValue::Integer(_) => Err(format!(
            "failed to parse target spec '{}': '{}' must be a string at line {}",
            path.display(),
            key,
            line_no
        )),
    }
}

pub(crate) fn default_object_output_path(input: &Path, target_triple: &str) -> PathBuf {
    let ext = if is_coff_target_triple(target_triple) {
        "obj"
    } else {
        "o"
    };
    input.with_extension(ext)
}

pub(crate) fn default_metadata_output_path(object_path: &Path) -> PathBuf {
    object_path.with_extension("onda.json")
}

fn is_coff_target_triple(target_triple: &str) -> bool {
    let triple = target_triple.to_ascii_lowercase();
    triple.contains("windows") || triple.contains("msvc")
}

fn parse_param_setting(value: &str) -> Result<(String, f64), String> {
    let Some((name, raw_value)) = value.split_once('=') else {
        return Err(format!(
            "invalid parameter setting '{value}', expected name=value"
        ));
    };
    if name.is_empty() {
        return Err("parameter setting requires a non-empty name".to_owned());
    }
    let parsed = raw_value.parse::<f64>().map_err(|_| {
        format!("invalid parameter value '{raw_value}' for '{name}', expected number")
    })?;
    Ok((name.to_owned(), parsed))
}

fn parse_buffer_binding(value: &str) -> Result<(String, PathBuf), String> {
    let Some((name, path)) = value.split_once('=') else {
        return Err(format!(
            "invalid buffer binding '{value}', expected name=path"
        ));
    };
    if name.is_empty() {
        return Err("buffer binding requires a non-empty name".to_owned());
    }
    if path.is_empty() {
        return Err(format!("buffer binding for '{name}' requires a file path"));
    }
    Ok((name.to_owned(), PathBuf::from(path)))
}
