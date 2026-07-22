fn compile_instance(src: &str, frames: usize) -> (onda_runtime::Instance, usize, usize) {
    compile_instance_with_options(
        src,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
}

fn compile_instance_file(path: &str, frames: usize) -> (onda_runtime::Instance, usize, usize) {
    compile_instance_file_with_options(
        path,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
}

fn compile_instance_with_options(
    src: &str,
    frames: usize,
    options: CompileOptions,
) -> (onda_runtime::Instance, usize, usize) {
    let parsed = parse_program(src).expect("parse should succeed");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: options.sample_rate,
            block_size: options.block_size,
        },
    )
    .expect("semantic analysis should succeed");
    let in_channels = typed.ins.len();
    let out_channels = typed.outs.len();
    let jit = lower_typed_and_jit(typed, options).expect("jit lowering should succeed");

    let instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: options.sample_rate,
            frames_per_block: frames,
            in_channels,
            out_channels,
        },
    )
    .expect("instance should be created");
    (instance, in_channels, out_channels)
}

fn compile_instance_file_with_options(
    path: &str,
    frames: usize,
    options: CompileOptions,
) -> (onda_runtime::Instance, usize, usize) {
    let parsed = parse_program_file(std::path::Path::new(path)).expect("parse should succeed");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: options.sample_rate,
            block_size: options.block_size,
        },
    )
    .expect("semantic analysis should succeed");
    let in_channels = typed.ins.len();
    let out_channels = typed.outs.len();
    let jit = lower_typed_and_jit(typed, options).expect("jit lowering should succeed");

    let instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: options.sample_rate,
            frames_per_block: frames,
            in_channels,
            out_channels,
        },
    )
    .expect("instance should be created");
    (instance, in_channels, out_channels)
}

fn lower_typed_and_jit(
    typed: onda_semantics::TypedProgram,
    options: CompileOptions,
) -> Result<onda_codegen_llvm::JitProgram, String> {
    if typed.analysis_options.sample_rate.to_bits() != options.sample_rate.to_bits()
        || typed.analysis_options.block_size != options.block_size
    {
        return Err(format!(
            "analysis/codegen configuration mismatch: analyzed at {} Hz / {} frames, requested {} Hz / {} frames",
            typed.analysis_options.sample_rate,
            typed.analysis_options.block_size,
            options.sample_rate,
            options.block_size,
        ));
    }
    let mir = onda_semantics::lower_program_to_optimized_mir(&typed)
        .map_err(|errors| format!("MIR lowering failed: {errors:?}"))?;
    jit_program_from_optimized_mir_with_options(
        mir,
        MirCompileOptions {
            fast_math: options.fast_math,
            opt_level: options.opt_level,
        },
    )
    .map_err(|errors| format!("LLVM JIT lowering failed: {errors:?}"))
}

fn state_type_of(typed: &onda_semantics::TypedProgram, name: &str) -> Option<PrimitiveType> {
    typed
        .state_vars
        .iter()
        .zip(typed.state_types.iter())
        .find_map(|(n, ty)| if n == name { Some(*ty) } else { None })
}

fn mk_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("onda_examples_{prefix}_{nanos}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
