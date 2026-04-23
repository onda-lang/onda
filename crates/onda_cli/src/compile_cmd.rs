use std::collections::HashMap;
use std::fs;
use std::path::Path;

use onda_codegen_llvm::{
    lower_to_object_with_options, lower_to_target_llvm_ir_with_options, CodegenOptions,
    TargetConfig,
};
use onda_frontend::{parse_program_file, PrimitiveType, Program};
use onda_semantics::{
    analyze_with_options, lower_graphs_for_inspection_with_options, AnalysisOptions,
    TypedArrayInfo, TypedProgram,
};

use crate::args::{default_metadata_output_path, default_object_output_path};
use crate::diag_print::format_diagnostics;
use crate::formatting::{format_program, primitive_type_name};
use crate::CompileEmit;

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
    let codegen_options = CodegenOptions {
        sample_rate: sample_rate_hz as f32,
        block_size: block_frames,
        fast_math,
        target,
    };

    match emit {
        CompileEmit::Check => {
            if output.is_some() {
                return Err("--output is only valid with --emit llvm-ir or --emit obj".to_owned());
            }
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            if !codegen_options.target.is_host_default() {
                lower_to_target_llvm_ir_with_options(typed, codegen_options).map_err(|diags| {
                    format_diagnostics("target codegen validation failed", &diags)
                })?;
            }
            println!("OK: {}", input.display());
        }
        CompileEmit::LlvmIr => {
            if meta_out.is_some() {
                return Err("--meta-out is only valid with --emit obj".to_owned());
            }
            let ir = lower_to_target_llvm_ir_with_options(typed, codegen_options)
                .map_err(|diags| format_diagnostics("IR lowering failed", &diags))?;
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
            let artifact = lower_to_object_with_options(typed, codegen_options)
                .map_err(|diags| format_diagnostics("object emission failed", &diags))?;
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
