fn compile_instance(src: &str, frames: usize) -> (onda_runtime::Instance, usize, usize) {
    compile_instance_with_options(
        src,
        frames,
        CompileOptions {
            backend: ExecutionBackend::Auto,
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
            backend: ExecutionBackend::Auto,
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
    let jit = onda_codegen_llvm::lower_and_jit_with_options(typed, options)
        .expect("jit lowering should succeed");

    let instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: 48_000.0,
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
    let jit = onda_codegen_llvm::lower_and_jit_with_options(typed, options)
        .expect("jit lowering should succeed");

    let instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: frames,
            in_channels,
            out_channels,
        },
    )
    .expect("instance should be created");
    (instance, in_channels, out_channels)
}

fn emit_ir(src: &str) -> String {
    let parsed = parse_program(src).expect("parse should succeed");
    let typed = analyze(parsed).expect("analysis should succeed");
    onda_codegen_llvm::lower_to_llvm_ir_with_options(
        typed,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: 4,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("IR emission should succeed")
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
