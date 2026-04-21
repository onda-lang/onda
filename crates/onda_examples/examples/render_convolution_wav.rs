use std::fs;
use std::path::{Path, PathBuf};

use onda_codegen_llvm::{CompileOptions, ExecutionBackend, TargetOptLevel};
use onda_frontend::{parse_program, Diagnostic};
use onda_runtime::{
    bind_output, create_instance, process_checked, trigger_event_by_index, InstanceConfig,
};
use onda_semantics::{analyze_with_options, AnalysisOptions};

fn read_wav_mono_f32(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("wav should open");
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "expected mono wav");

    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .expect("float wav samples"),
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|s| s.map(|v| v as f32 / i8::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int8 wav samples"),
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int16 wav samples"),
        (hound::SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 8_388_608.0))
            .collect::<Result<Vec<_>, _>>()
            .expect("int24 wav samples"),
        (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / i32::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int32 wav samples"),
        other => panic!("unsupported wav format: {other:?}"),
    }
}

fn write_wav_mono_f32(path: &Path, samples: &[f32], sample_rate: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    for sample in samples {
        writer.write_sample(*sample).expect("write sample");
    }
    writer.finalize().expect("finalize wav");
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn main() -> Result<(), Diagnostic> {
    let root = workspace_root();
    let example_path = root.join("examples").join("convolution_wav_impulse.onda");
    let ir_path = root.join("examples").join("impulse.wav");
    let target_dir = root.join("target");
    fs::create_dir_all(&target_dir).expect("create target dir");
    let out_path = target_dir.join("convolution_wav_impulse_output.wav");

    let src = fs::read_to_string(&example_path).expect("read example source");
    let ir = read_wav_mono_f32(&ir_path);
    let frames = 131_072usize;
    let sample_rate = 44_100.0f32;

    let parsed = parse_program(&src).expect("parse should succeed");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate,
            block_size: frames,
        },
    )
    .expect("semantic analysis should succeed");

    let jit = onda_codegen_llvm::lower_and_jit_with_options(
        typed,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate,
            block_size: frames,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("jit lowering should succeed");

    let mut instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate,
            frames_per_block: frames,
            in_channels: 0,
            out_channels: 1,
        },
    )
    .expect("instance should be created");

    let event_idx = instance.event_index("load_ir").expect("load_ir event");
    let mut payload =
        Vec::with_capacity(std::mem::size_of::<i32>() + ir.len() * std::mem::size_of::<f32>());
    payload.extend_from_slice(&(ir.len() as i32).to_ne_bytes());
    for sample in &ir {
        payload.extend_from_slice(&sample.to_ne_bytes());
    }
    trigger_event_by_index(&mut instance, event_idx, &payload)?;

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len())?;
    process_checked(&mut instance, frames)?;

    let full = out_bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().expect("chunk");
            f32::from_ne_bytes(arr)
        })
        .collect::<Vec<_>>();

    write_wav_mono_f32(&out_path, &full[..ir.len()], sample_rate as u32);

    println!("Wrote output: {}", out_path.display());
    println!("Output should match: {}", ir_path.display());

    Ok(())
}
