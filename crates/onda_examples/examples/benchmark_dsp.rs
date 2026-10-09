//! Isolated Onda/MIR worker. Use scripts/bench/run.py for bounded execution.
use std::{env, error::Error, fs, hint::black_box, path::PathBuf, time::Instant};

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, lower_optimized_mir_to_llvm_ir_with_options,
    lower_optimized_mir_to_object_artifact, MirCompileOptions, MirTargetOptions, TargetConfig,
    TargetOptLevel,
};
use onda_frontend::{
    load_program_file, stdlib_module_names, stdlib_module_source, PrimitiveType, SourceManifest,
};
use onda_runtime::{
    bind_input, bind_output, create_instance, init_checked, prepare_unchecked_process,
    process_unchecked, ExecutionOutput, InitMode, InstanceConfig,
};
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};
use serde::Deserialize;
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Deserialize)]
struct Config {
    schema_version: u32,
    source: PathBuf,
    prefix: PathBuf,
    mode: String,
    sample_rate: f32,
    block_size: usize,
    repetitions: usize,
    round_ms: f64,
    warmup_blocks: usize,
    validation_blocks: usize,
    latency_blocks: usize,
    input_seed: u32,
    input_pattern: String,
    opt_level: u8,
    fast_math: bool,
    ir: bool,
    assembly: bool,
    trust_mir: bool,
    max_output_abs: Option<f64>,
}

impl Config {
    fn artifact(&self, extension: &str) -> PathBuf {
        self.prefix.with_extension(extension)
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !matches!(self.mode.as_str(), "prepare" | "inspect" | "bench")
            || !matches!(
                self.input_pattern.as_str(),
                "noise" | "ramp" | "zero" | "impulse"
            )
            || !self.sample_rate.is_finite()
            || self.sample_rate <= 0.0
            || self.block_size == 0
            || self.block_size > i32::MAX as usize
            || self.repetitions == 0
            || self.validation_blocks == 0
            || self.latency_blocks == 0
            || !self.round_ms.is_finite()
            || self.round_ms <= 0.0
            || self.opt_level > 3
            || self
                .max_output_abs
                .is_some_and(|x| !x.is_finite() || x < 0.0)
        {
            return Err("invalid benchmark configuration".into());
        }
        Ok(())
    }
}

fn load_mir(config: &Config) -> Result<(onda_mir::OptimizedProgram, Option<SourceManifest>)> {
    let path = &config.source;
    if matches!(
        path.extension().and_then(|x| x.to_str()),
        Some("onda" | "on")
    ) {
        let loaded = load_program_file(path).map_err(|e| format!("parse: {:?}", e.diagnostics))?;
        let typed = analyze_with_options(
            loaded.program,
            AnalysisOptions {
                sample_rate: config.sample_rate,
                block_size: config.block_size,
            },
        )
        .map_err(|e| format!("analysis: {e:?}"))?;
        let mir = lower_program_to_optimized_mir(&typed).map_err(|e| format!("MIR: {e:?}"))?;
        return Ok((mir, Some(loaded.sources)));
    }
    let bytes = fs::read(path)?;
    let validated = if path.extension().and_then(|x| x.to_str()) == Some("json") {
        let text = std::str::from_utf8(&bytes)?;
        if config.trust_mir {
            // Explicit opt-in asserts the same producer proof contract as the CLI.
            unsafe { onda_mir::from_json_with_producer_proofs(text)? }
        } else {
            onda_mir::from_json(text)?
        }
    } else if path.extension().and_then(|x| x.to_str()) == Some("msgpack") {
        if config.trust_mir {
            unsafe { onda_mir::from_messagepack_with_producer_proofs(&bytes)? }
        } else {
            onda_mir::from_messagepack(&bytes)?
        }
    } else {
        return Err("source must be .onda, .on, .mir.json or .mir.msgpack".into());
    };
    let (mir, _) = onda_mir::optimize(validated).map_err(|e| format!("MIR: {e:?}"))?;
    if mir.config.sample_rate != config.sample_rate
        || mir.config.block_size as usize != config.block_size
    {
        return Err("MIR sample rate/block size differ from benchmark settings".into());
    }
    Ok((mir, None))
}

struct Port {
    ty: PrimitiveType,
    storage: Vec<u64>,
    frames: usize,
}

impl Port {
    fn new(ty: PrimitiveType, frames: usize) -> Result<Self> {
        let bytes = frames.checked_mul(width(ty)).ok_or("port size overflow")?;
        Ok(Self {
            ty,
            storage: vec![0; bytes.div_ceil(8)],
            frames,
        })
    }

    fn bytes(&self) -> &[u8] {
        // u64 storage supplies alignment for every supported scalar port and stays live
        // without resizing for the entire lifetime of its runtime binding.
        unsafe {
            std::slice::from_raw_parts(self.storage.as_ptr().cast(), self.frames * width(self.ty))
        }
    }

    fn write_input(&mut self, frame: usize, x: f32) {
        let data = match self.ty {
            PrimitiveType::F32 => x.to_le_bytes().to_vec(),
            PrimitiveType::F64 => f64::from(x).to_le_bytes().to_vec(),
            PrimitiveType::I32 => ((x * 64.0) as i32).to_le_bytes().to_vec(),
            PrimitiveType::I64 => ((x * 64.0) as i64).to_le_bytes().to_vec(),
            PrimitiveType::Bool => vec![u8::from(x > 0.0)],
        };
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                self.storage.as_mut_ptr().cast::<u8>(),
                self.frames * width(self.ty),
            )
        };
        let start = frame * width(self.ty);
        bytes[start..start + data.len()].copy_from_slice(&data);
    }

    fn value(&self, frame: usize) -> f64 {
        let start = frame * width(self.ty);
        let b = &self.bytes()[start..start + width(self.ty)];
        match self.ty {
            PrimitiveType::F32 => {
                f64::from(f32::from_le_bytes(b.try_into().expect("scalar width")))
            }
            PrimitiveType::F64 => f64::from_le_bytes(b.try_into().expect("scalar width")),
            PrimitiveType::I32 => {
                f64::from(i32::from_le_bytes(b.try_into().expect("scalar width")))
            }
            PrimitiveType::I64 => i64::from_le_bytes(b.try_into().expect("scalar width")) as f64,
            PrimitiveType::Bool => f64::from(b[0]),
        }
    }
}

fn width(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::Bool => 1,
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
    }
}

fn capture(outputs: &[Port], buffer: &mut Vec<u8>, bound: Option<f64>) -> Result<()> {
    for output in outputs {
        for frame in 0..output.frames {
            let x = output.value(frame);
            if !x.is_finite() || bound.is_some_and(|limit| x.abs() > limit) {
                return Err(format!("output violates finite/error-bound contract: {x}").into());
            }
        }
        buffer.extend_from_slice(output.bytes());
    }
    Ok(())
}

fn main() -> Result<()> {
    if cfg!(target_endian = "big") {
        return Err("benchmark port snapshots currently require a little-endian host".into());
    }
    let mut args = env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: benchmark_dsp CONFIG.json (use scripts/bench/run.py)")?;
    if args.next().is_some() {
        return Err("expected one configuration file".into());
    }
    let config: Config = serde_json::from_slice(&fs::read(path)?)?;
    config.validate()?;
    let started = Instant::now();
    let (mir, sources) = load_mir(&config)?;
    let frontend_ms = started.elapsed().as_secs_f64() * 1000.0;
    if let Some(sources) = sources {
        // Snapshot construction and writing are outside compilation timing.
        // Embedded stdlib text comes from this binary, not the disk checkout.
        let documents: Vec<_> = sources
            .documents
            .iter()
            .map(|d| json!({"path": d.path, "contents": d.contents}))
            .collect();
        let stdlib: Vec<_> = stdlib_module_names()
            .map(|name| json!({"path": name, "contents": stdlib_module_source(name)}))
            .collect();
        let sources = json!({"documents": documents, "embedded_stdlib": stdlib});
        fs::write(
            config.artifact("sources.json"),
            serde_json::to_vec(&sources)?,
        )?;
    }
    fs::write(
        config.artifact("mir.msgpack"),
        onda_mir::to_messagepack_optimized(&mir)?,
    )?;
    if config.ir {
        fs::write(
            config.artifact("mir.json"),
            onda_mir::to_json_pretty_optimized(&mir)?,
        )?;
        fs::write(config.artifact("mir.txt"), onda_mir::format_program(&mir))?;
    }
    let options = MirCompileOptions {
        fast_math: config.fast_math,
        opt_level: match config.opt_level {
            0 => TargetOptLevel::O0,
            1 => TargetOptLevel::O1,
            2 => TargetOptLevel::O2,
            _ => TargetOptLevel::O3,
        },
    };
    // Inspection costs are kept outside the reported JIT and processing times.
    if config.ir {
        fs::write(
            config.artifact("ll"),
            lower_optimized_mir_to_llvm_ir_with_options(&mir, options)
                .map_err(|e| format!("IR: {e:?}"))?,
        )?;
    }
    if config.assembly {
        let artifact = lower_optimized_mir_to_object_artifact(
            &mir,
            &MirTargetOptions {
                fast_math: config.fast_math,
                target: TargetConfig {
                    opt_level: options.opt_level,
                    ..TargetConfig::host()
                },
            },
        )
        .map_err(|e| format!("object: {e:?}"))?;
        fs::write(config.artifact("o"), artifact.object_bytes)?;
        fs::write(
            config.artifact("object.json"),
            serde_json::to_vec_pretty(&artifact.metadata)?,
        )?;
    }
    let shape = json!({"functions": mir.functions.len(), "state_entries": mir.state.len(),
        "mir_bytes": fs::metadata(config.artifact("mir.msgpack"))?.len()});
    if config.mode != "bench" {
        println!("{}", json!({"frontend_ms": frontend_ms, "shape": shape}));
        return Ok(());
    }
    let started = Instant::now();
    let program = jit_program_from_optimized_mir_with_options(mir, options)
        .map_err(|e| format!("JIT: {e:?}"))?;
    let jit_ms = started.elapsed().as_secs_f64() * 1000.0;
    if program.outputs().is_empty()
        || program.buffer_count() != 0
        || program
            .inputs()
            .iter()
            .chain(program.outputs())
            .any(|p| p.byte_size() != width(p.elem_ty()))
    {
        return Err("runner requires scalar ports, an output, and no external buffers".into());
    }
    let state_bytes = program.physical_state_size_bytes();
    let mut inputs = Vec::new();
    for (channel, descriptor) in program.inputs().iter().enumerate() {
        let mut port = Port::new(descriptor.elem_ty(), config.block_size)?;
        let mut seed = config.input_seed.wrapping_add(channel as u32);
        for frame in 0..config.block_size {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = match config.input_pattern.as_str() {
                "zero" => 0.0,
                "impulse" => {
                    if frame == 0 {
                        1.0
                    } else {
                        0.0
                    }
                }
                "ramp" => ((frame % 257) as f32 - 128.0) / 128.0,
                _ => (seed >> 8) as f32 * (1.0 / 16777216.0) * 1.6 - 0.8,
            };
            port.write_input(frame, x);
        }
        inputs.push(port);
    }
    let mut outputs = program
        .outputs()
        .iter()
        .map(|p| Port::new(p.elem_ty(), config.block_size))
        .collect::<Result<Vec<_>>>()?;
    let input_types: Vec<_> = inputs
        .iter()
        .map(|p| format!("{:?}", p.ty).to_ascii_lowercase())
        .collect();
    let output_types: Vec<_> = outputs
        .iter()
        .map(|p| format!("{:?}", p.ty).to_ascii_lowercase())
        .collect();
    let mut instance = create_instance(
        program,
        InstanceConfig {
            sample_rate: config.sample_rate,
            frames_per_block: config.block_size,
            in_channels: inputs.len(),
            out_channels: outputs.len(),
        },
    )
    .map_err(|e| format!("instance: {e:?}"))?;
    for (channel, port) in inputs.iter().enumerate() {
        unsafe {
            bind_input(
                &mut instance,
                channel,
                port.storage.as_ptr().cast(),
                port.bytes().len(),
            )
        }
        .map_err(|e| format!("input: {e:?}"))?;
    }
    for (channel, port) in outputs.iter_mut().enumerate() {
        let bytes = port.frames * width(port.ty);
        unsafe {
            bind_output(
                &mut instance,
                channel,
                port.storage.as_mut_ptr().cast(),
                bytes,
            )
        }
        .map_err(|e| format!("output: {e:?}"))?;
    }
    let started = Instant::now();
    init_checked(&mut instance, InitMode::Full).map_err(|e| format!("init: {e:?}"))?;
    let init_ms = started.elapsed().as_secs_f64() * 1000.0;
    prepare_unchecked_process(&mut instance).map_err(|e| format!("prepare: {e:?}"))?;
    let mut process = || -> Result<()> {
        let status = unsafe { process_unchecked(&mut instance, ExecutionOutput::none()) }
            .map_err(|e| format!("process: {e:?}"))?;
        if status != 0 {
            return Err(format!("execution status {status}").into());
        }
        Ok(())
    };
    let mut validation = Vec::new();
    let mut cold_max_us = 0.0_f64;
    for _ in 0..config.validation_blocks {
        let started = Instant::now();
        process()?;
        cold_max_us = cold_max_us.max(started.elapsed().as_secs_f64() * 1e6);
        capture(&outputs, &mut validation, config.max_output_abs)?;
    }
    fs::write(config.artifact("output.bin"), validation)?;
    for _ in 0..config.warmup_blocks {
        process()?;
    }
    let mut validation = Vec::new();
    for _ in 0..config.validation_blocks {
        process()?;
        capture(&outputs, &mut validation, config.max_output_abs)?;
    }
    fs::write(config.artifact("warm-output.bin"), validation)?;
    let mut iterations = 256_usize;
    loop {
        let started = Instant::now();
        for _ in 0..iterations {
            process()?;
        }
        if started.elapsed().as_secs_f64() * 1000.0 >= config.round_ms {
            break;
        }
        iterations = iterations.checked_mul(2).ok_or("iteration overflow")?;
    }
    let mut rounds = Vec::new();
    for _ in 0..config.repetitions {
        let started = Instant::now();
        for _ in 0..iterations {
            process()?;
        }
        rounds.push(
            started.elapsed().as_secs_f64() * 1e9 / (iterations as f64 * config.block_size as f64),
        );
        black_box(&outputs);
    }
    let mut latencies = Vec::new();
    for _ in 0..config.latency_blocks {
        let started = Instant::now();
        process()?;
        latencies.push(started.elapsed().as_secs_f64() * 1e6);
    }
    capture(&outputs, &mut Vec::new(), config.max_output_abs)?;
    let median = percentile(&rounds, 0.5);
    let deviations: Vec<_> = rounds.iter().map(|x| (x - median).abs()).collect();
    let output: Value = json!({"frontend_ms": frontend_ms, "jit_ms": jit_ms, "init_ms": init_ms,
        "shape": shape, "state_bytes": state_bytes, "iterations": iterations,
        "ns_per_frame": median, "rounds_ns_per_frame": rounds, "mad_ns": percentile(&deviations, 0.5),
        "min_ns": percentile(&rounds, 0.0), "max_ns": percentile(&rounds, 1.0),
        "block_p50_us": percentile(&latencies, 0.5), "block_p99_us": percentile(&latencies, 0.99),
        "block_max_us": percentile(&latencies, 1.0), "cold_max_us": cold_max_us,
        "inputs": input_types, "outputs": output_types,
        "validation": if config.max_output_abs.is_some() { "bounded-error" } else { "finite" },
    });
    println!("{output}");
    Ok(())
}

fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}
