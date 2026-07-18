use std::collections::HashMap;
use std::fs;
use std::path::Path;

use onda_codegen_llvm::{
    lower_optimized_mir_to_object_artifact, lower_optimized_mir_to_target_llvm_ir, MirCodegenError,
    MirTargetOptions, TargetConfig,
};
use onda_frontend::{parse_program_file, PrimitiveType, Program};
use onda_semantics::{
    analyze_with_options, lower_graphs_for_inspection_with_options, lower_program_to_optimized_mir,
    AnalysisOptions, TypedArrayInfo, TypedProgram,
};

use crate::args::{default_metadata_output_path, default_object_output_path};
use crate::diag_print::format_diagnostics;
use crate::CompileEmit;
use onda_lsp::formatting::{format_program, primitive_type_name};

pub(crate) fn run_compile(
    input: &Path,
    emit: CompileEmit,
    output: Option<&Path>,
    meta_out: Option<&Path>,
    sample_rate_hz: u32,
    block_frames: usize,
    dump_graph: bool,
    show_meta: bool,
    fast_math: bool,
    target: TargetConfig,
) -> Result<(), String> {
    if dump_graph {
        let lowered = parse_and_lower_graphs(input, sample_rate_hz as f32, block_frames)?;
        print!("{}", format_program(&lowered));
    }
    let typed = parse_and_analyze(input, sample_rate_hz as f32, block_frames)?;
    if show_meta {
        print_program_meta(&typed);
    }
    let mir = lower_program_to_optimized_mir(&typed).map_err(|errors| {
        let details = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        format!("MIR lowering failed:\n{details}")
    })?;
    let codegen_options = MirTargetOptions {
        fast_math,
        target,
        ..MirTargetOptions::default()
    };

    match emit {
        CompileEmit::Check => {
            if output.is_some() {
                return Err(
                    "--output is only valid with --emit mir, --emit mir-json, --emit mir-messagepack, --emit llvm-ir, or --emit obj"
                        .to_owned(),
                );
            }
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            if !codegen_options.target.is_host_default() {
                lower_optimized_mir_to_target_llvm_ir(&mir, &codegen_options).map_err(
                    |errors| format_mir_codegen_errors("target codegen validation failed", &errors),
                )?;
            }
            println!("OK: {}", input.display());
        }
        CompileEmit::Mir => {
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            let dump = onda_mir::format_program(mir.as_program());
            if let Some(path) = output {
                fs::write(path, dump.as_bytes())
                    .map_err(|err| format!("failed to write MIR '{}': {err}", path.display()))?;
                println!("Wrote MIR: {}", path.display());
            } else {
                print!("{dump}");
            }
        }
        CompileEmit::MirJson => {
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            let json = onda_mir::to_json_pretty_optimized(&mir)
                .map_err(|err| format!("failed to encode MIR JSON: {err}"))?;
            if let Some(path) = output {
                fs::write(path, json.as_bytes()).map_err(|err| {
                    format!("failed to write MIR JSON '{}': {err}", path.display())
                })?;
                println!("Wrote MIR JSON: {}", path.display());
            } else {
                println!("{json}");
            }
        }
        CompileEmit::MirMessagePack => {
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            let output = output.ok_or("--emit mir-messagepack requires --output")?;
            let bytes = onda_mir::to_messagepack_optimized(&mir)
                .map_err(|err| format!("failed to encode MIR MessagePack: {err}"))?;
            fs::write(output, bytes).map_err(|err| {
                format!(
                    "failed to write MIR MessagePack '{}': {err}",
                    output.display()
                )
            })?;
            println!("Wrote MIR MessagePack: {}", output.display());
        }
        CompileEmit::LlvmIr => {
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            let ir = lower_optimized_mir_to_target_llvm_ir(&mir, &codegen_options)
                .map_err(|errors| format_mir_codegen_errors("IR lowering failed", &errors))?;
            if let Some(path) = output {
                fs::write(path, ir.as_bytes()).map_err(|err| {
                    format!("failed to write LLVM IR '{}': {err}", path.display())
                })?;
                println!("Wrote LLVM IR: {}", path.display());
            } else {
                println!("{ir}");
            }
        }
        CompileEmit::Object => {
            let artifact = lower_optimized_mir_to_object_artifact(&mir, &codegen_options)
                .map_err(|errors| format_mir_codegen_errors("object emission failed", &errors))?;
            let object_path = output.map(Path::to_path_buf).unwrap_or_else(|| {
                default_object_output_path(input, &artifact.metadata.target.triple)
            });
            fs::write(&object_path, &artifact.object_bytes).map_err(|err| {
                format!(
                    "failed to write object file '{}': {err}",
                    object_path.display()
                )
            })?;

            let metadata_path = meta_out
                .map(Path::to_path_buf)
                .unwrap_or_else(|| default_metadata_output_path(&object_path));
            let metadata_json = serde_json::to_string_pretty(&artifact.metadata)
                .map_err(|err| format!("failed to encode metadata JSON: {err}"))?;
            fs::write(&metadata_path, metadata_json.as_bytes()).map_err(|err| {
                format!(
                    "failed to write metadata sidecar '{}': {err}",
                    metadata_path.display()
                )
            })?;

            println!("Wrote object: {}", object_path.display());
            println!("Wrote metadata: {}", metadata_path.display());
        }
    }
    Ok(())
}

#[derive(Clone)]
struct CliDeclaredIo {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    offset: usize,
}

fn primitive_type_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn declared_type_repr(elem: PrimitiveType, len: usize) -> String {
    if len == 1 {
        primitive_type_name(elem).to_owned()
    } else {
        format!("{}[{len}]", primitive_type_name(elem))
    }
}

fn build_declared_ports(
    flat: &[String],
    types: &HashMap<String, PrimitiveType>,
    arrays: &HashMap<String, TypedArrayInfo>,
) -> Vec<CliDeclaredIo> {
    let arrays_by_offset = arrays
        .iter()
        .map(|(name, info)| (info.offset, (name, info)))
        .collect::<HashMap<_, _>>();
    let mut out = Vec::new();
    let mut slot = 0usize;
    while slot < flat.len() {
        if let Some((name, info)) = arrays_by_offset.get(&slot) {
            out.push(CliDeclaredIo {
                name: (*name).clone(),
                elem_ty: info.elem_ty,
                array_len: info.len,
                offset: slot,
            });
            slot += info.len;
            continue;
        }
        let name = flat[slot].clone();
        let ty = *types.get(&name).unwrap_or(&PrimitiveType::F32);
        out.push(CliDeclaredIo {
            name,
            elem_ty: ty,
            array_len: 1,
            offset: slot,
        });
        slot += 1;
    }
    out
}

fn build_declared_params(typed: &TypedProgram) -> Vec<CliDeclaredIo> {
    let arrays_by_offset = typed
        .param_arrays
        .iter()
        .map(|(name, info)| (info.offset, (name, info)))
        .collect::<HashMap<_, _>>();
    let mut out = Vec::new();
    let mut slot = 0usize;
    while slot < typed.params.len() {
        if let Some((name, info)) = arrays_by_offset.get(&slot) {
            out.push(CliDeclaredIo {
                name: (*name).clone(),
                elem_ty: info.elem_ty,
                array_len: info.len,
                offset: slot,
            });
            slot += info.len;
            continue;
        }
        let p = &typed.params[slot];
        out.push(CliDeclaredIo {
            name: p.name.clone(),
            elem_ty: p.ty,
            array_len: 1,
            offset: slot,
        });
        slot += 1;
    }
    out
}

fn print_declared_table(label: &str, entries: &[CliDeclaredIo]) {
    println!("{label}:");
    if entries.is_empty() {
        println!("  (none)");
        return;
    }
    for (idx, entry) in entries.iter().enumerate() {
        let ty = declared_type_repr(entry.elem_ty, entry.array_len);
        let bytes = primitive_type_bytes(entry.elem_ty) * entry.array_len;
        println!(
            "  [{idx}] name={} type={} bytes={} offset={}",
            entry.name, ty, bytes, entry.offset
        );
    }
}

fn print_program_meta(typed: &TypedProgram) {
    let ins = build_declared_ports(&typed.ins, &typed.in_types, &typed.in_arrays);
    let outs = build_declared_ports(&typed.outs, &typed.out_types, &typed.out_arrays);
    let params = build_declared_params(typed);
    print_declared_table("ins", &ins);
    print_declared_table("outs", &outs);
    print_declared_table("params", &params);
}

fn format_mir_codegen_errors(prefix: &str, errors: &[MirCodegenError]) -> String {
    let details = errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    format!("{prefix}:\n{details}")
}

fn parse_and_analyze(
    input: &Path,
    sample_rate: f32,
    block_size: usize,
) -> Result<TypedProgram, String> {
    let parsed =
        parse_program_file(input).map_err(|diags| format_diagnostics("parse failed", &diags))?;
    analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate,
            block_size,
        },
    )
    .map_err(|diags| format_diagnostics("semantic analysis failed", &diags))
}

fn parse_and_lower_graphs(
    input: &Path,
    sample_rate: f32,
    block_size: usize,
) -> Result<Program, String> {
    let parsed =
        parse_program_file(input).map_err(|diags| format_diagnostics("parse failed", &diags))?;
    lower_graphs_for_inspection_with_options(
        parsed,
        AnalysisOptions {
            sample_rate,
            block_size,
        },
    )
    .map_err(|diags| format_diagnostics("graph lowering failed", &diags))
}
