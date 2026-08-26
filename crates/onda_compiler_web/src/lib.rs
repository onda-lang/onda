#![deny(clippy::all)]

use std::borrow::Cow;
#[cfg(target_arch = "wasm32")]
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use onda_frontend::{
    load_program_file_from_virtual_sources, parse_program, DiagCode, Diagnostic, Program,
    SourceManifest,
};
#[cfg(target_arch = "wasm32")]
use onda_frontend::{Block, ConstType, PrimitiveType};
#[cfg(target_arch = "wasm32")]
use onda_project::{
    decode_buffer_bytes, decode_ondabuffer, encode_ondabuffer, validate_ondabuffer, AssetId,
    BufferAsset, BufferSamples, MaterializationPlan,
};
use onda_project::{
    BufferElement, ProjectBufferChannels, ProjectBufferDeclaration, ProjectImage, ProjectLimits,
    SourceImage,
};
use onda_semantics::{
    analyze_with_options_and_inputs, lower_program_to_optimized_mir, AnalysisOptions, CompileInputs,
};
#[cfg(target_arch = "wasm32")]
use onda_semantics::{
    inspect_compile_constants as inspect_semantic_compile_constants, CompileConstDescriptor,
    CompileConstKind,
};
#[cfg(any(test, target_arch = "wasm32"))]
use onda_semantics::{ConstValue, TypedConstValue};
#[cfg(target_arch = "wasm32")]
use serde::Deserialize;
use serde::Serialize;

pub const MIR_SCHEMA_VERSION: u32 = onda_mir::MIR_SCHEMA_VERSION;

#[derive(Clone, Copy)]
enum CompileInputRequest<'a> {
    Typed(&'a CompileInputs),
    #[cfg(target_arch = "wasm32")]
    Json(&'a str),
}

#[cfg(target_arch = "wasm32")]
#[derive(Deserialize)]
struct WebCompileInput {
    name: String,
    element: String,
    array: bool,
    values: Vec<serde_json::Value>,
}

impl<'a> CompileInputRequest<'a> {
    fn resolve(
        self,
        _program: &Program,
    ) -> Result<Cow<'a, CompileInputs>, Vec<CompilerDiagnostic>> {
        match self {
            Self::Typed(inputs) => Ok(Cow::Borrowed(inputs)),
            #[cfg(target_arch = "wasm32")]
            Self::Json(json) => resolve_web_compile_inputs(_program, json).map(Cow::Owned),
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn resolve_web_compile_inputs(
    program: &Program,
    json: &str,
) -> Result<CompileInputs, Vec<CompilerDiagnostic>> {
    let entries = serde_json::from_str::<Vec<WebCompileInput>>(json).map_err(|error| {
        vec![CompilerDiagnostic::configuration(format!(
            "invalid compile constants: {error}"
        ))]
    })?;
    let mut inputs = CompileInputs::default();
    for entry in entries {
        if inputs.constants.contains_key(&entry.name) {
            return Err(vec![CompilerDiagnostic::configuration(format!(
                "configuration constant '{}' is specified more than once",
                entry.name
            ))]);
        }
        let decl = program.blocks.iter().find_map(|block| match block {
            Block::Const(decl) if decl.name == entry.name => Some(decl),
            _ => None,
        });
        let Some(decl) = decl else {
            return Err(vec![CompilerDiagnostic::configuration(format!(
                "unknown configuration constant '{}'",
                entry.name
            ))]);
        };
        if !decl.configurable {
            return Err(vec![CompilerDiagnostic::source(
                "semantic",
                Diagnostic::semantic_span(
                    format!(
                        "constant '{}' is not host-configurable; declare it with 'config const'",
                        entry.name
                    ),
                    decl.loc.as_ref(),
                ),
            )]);
        }
        let Some(declared_ty) = decl.ty.as_ref() else {
            return Err(vec![CompilerDiagnostic::source(
                "semantic",
                Diagnostic::semantic_span(
                    format!(
                        "configuration constant '{}' requires an explicit type",
                        entry.name
                    ),
                    decl.loc.as_ref(),
                ),
            )]);
        };
        let expected_elem = match declared_ty {
            ConstType::Scalar(ty)
            | ConstType::Array { elem: ty, .. }
            | ConstType::Slice { elem: ty } => *ty,
        };
        if entry.element != "number" && web_element_type(&entry.element).is_none() {
            return Err(vec![CompilerDiagnostic::configuration(format!(
                "configuration constant '{}' has unknown element type '{}'",
                entry.name, entry.element
            ))]);
        }
        if entry.array && entry.element == "number" {
            return Err(vec![CompilerDiagnostic::configuration(format!(
                "configuration constant '{}' requires a typed array representation",
                entry.name
            ))]);
        }
        let values = entry
            .values
            .iter()
            .map(|value| web_typed_const_value(&entry.element, value, expected_elem))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|message| {
                vec![CompilerDiagnostic::source(
                    "semantic",
                    Diagnostic::semantic_span(
                        format!("configuration constant '{}': {message}", entry.name),
                        decl.loc.as_ref(),
                    ),
                )]
            })?;
        let value = if entry.array {
            let elem_ty = web_element_type(&entry.element).unwrap_or(expected_elem);
            ConstValue::Array {
                elem_ty,
                len: values.len(),
                values,
            }
        } else if values.len() == 1 {
            ConstValue::Scalar(values[0])
        } else {
            return Err(vec![CompilerDiagnostic::configuration(format!(
                "scalar configuration constant '{}' must contain exactly one value",
                entry.name
            ))]);
        };
        inputs.constants.insert(entry.name, value);
    }
    Ok(inputs)
}

#[cfg(target_arch = "wasm32")]
fn web_element_type(element: &str) -> Option<PrimitiveType> {
    match element {
        "bool" => Some(PrimitiveType::Bool),
        "i32" => Some(PrimitiveType::I32),
        "i64" => Some(PrimitiveType::I64),
        "f32" => Some(PrimitiveType::F32),
        "f64" => Some(PrimitiveType::F64),
        "number" => None,
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
fn web_typed_const_value(
    element: &str,
    value: &serde_json::Value,
    expected: PrimitiveType,
) -> Result<TypedConstValue, String> {
    match element {
        "bool" => value
            .as_bool()
            .map(TypedConstValue::Bool)
            .ok_or_else(|| "expected a boolean value".to_owned()),
        "i32" => {
            let value = value
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| "expected an i32 value".to_owned())?;
            Ok(TypedConstValue::I32(value))
        }
        "i64" => {
            let value = value
                .as_str()
                .ok_or_else(|| "expected an i64 decimal string".to_owned())?
                .parse::<i64>()
                .map_err(|_| "i64 value is out of range".to_owned())?;
            Ok(TypedConstValue::I64(value))
        }
        "f32" => {
            let value = web_number(value, "f32")? as f32;
            Ok(TypedConstValue::F32(value))
        }
        "f64" => {
            let value = web_number(value, "f64")?;
            Ok(TypedConstValue::F64(value))
        }
        "number" => {
            let number = web_number(value, "number")?;
            match expected {
                PrimitiveType::I32 => {
                    if number.fract() != 0.0
                        || number < f64::from(i32::MIN)
                        || number > f64::from(i32::MAX)
                    {
                        Err("number is not representable as i32".to_owned())
                    } else {
                        Ok(TypedConstValue::I32(number as i32))
                    }
                }
                PrimitiveType::F32 => {
                    let value = number as f32;
                    if number.is_finite() && !value.is_finite() {
                        Err("number is not representable as f32".to_owned())
                    } else {
                        Ok(TypedConstValue::F32(value))
                    }
                }
                PrimitiveType::F64 => Ok(TypedConstValue::F64(number)),
                PrimitiveType::I64 => {
                    Err("i64 configuration constants require bigint values".to_owned())
                }
                PrimitiveType::Bool => {
                    Err("bool configuration constants require boolean values".to_owned())
                }
            }
        }
        other => Err(format!("unknown compile constant element type '{other}'")),
    }
}

#[cfg(target_arch = "wasm32")]
fn web_number(value: &serde_json::Value, label: &str) -> Result<f64, String> {
    let value = match value {
        serde_json::Value::Number(value) => value
            .as_f64()
            .ok_or_else(|| format!("expected a {label} value"))?,
        serde_json::Value::String(value) if value == "-0" => -0.0,
        serde_json::Value::String(value) if value == "NaN" => f64::NAN,
        serde_json::Value::String(value) if value == "Infinity" => f64::INFINITY,
        serde_json::Value::String(value) if value == "-Infinity" => f64::NEG_INFINITY,
        _ => return Err(format!("expected a {label} value")),
    };
    Ok(value)
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct CompilerDiagnostic {
    pub stage: &'static str,
    pub code: u16,
    pub message: String,
    pub file: Option<String>,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub trace: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct CompilerFailure {
    pub diagnostics: Vec<CompilerDiagnostic>,
    pub source_files: Vec<String>,
    pub unresolved_source_files: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CompilationOutput<T> {
    pub output: T,
    pub source_files: Vec<String>,
    pub source_image: Option<SourceImage>,
}

struct LoadedProjectProgram {
    program: Program,
    source_files: Vec<String>,
    source_image: SourceImage,
}

impl CompilerFailure {
    fn without_sources(diagnostics: Vec<CompilerDiagnostic>) -> Self {
        Self {
            diagnostics,
            source_files: Vec::new(),
            unresolved_source_files: Vec::new(),
        }
    }

    fn with_sources(diagnostics: Vec<CompilerDiagnostic>, source_files: Vec<String>) -> Self {
        Self {
            diagnostics,
            source_files,
            unresolved_source_files: Vec::new(),
        }
    }

    fn with_source_manifest(
        diagnostics: Vec<CompilerDiagnostic>,
        source_files: Vec<String>,
        unresolved_source_files: Vec<String>,
    ) -> Self {
        Self {
            diagnostics,
            source_files,
            unresolved_source_files,
        }
    }
}

impl CompilerDiagnostic {
    fn configuration(message: impl Into<String>) -> Self {
        Self {
            stage: "configuration",
            code: DiagCode::Semantic as u16,
            message: message.into(),
            file: None,
            line: 0,
            column: 0,
            end_line: 0,
            end_column: 0,
            trace: Vec::new(),
        }
    }

    fn source(stage: &'static str, diagnostic: Diagnostic) -> Self {
        Self {
            stage,
            code: diagnostic.code as u16,
            message: diagnostic.message,
            file: diagnostic.file,
            line: diagnostic.line,
            column: diagnostic.column,
            end_line: diagnostic.end_line,
            end_column: diagnostic.end_column,
            trace: diagnostic.trace,
        }
    }
}

/// Compiles one in-memory Onda source file to validated, versioned MIR JSON.
///
/// Built-in `std/...` modules are embedded by `onda_frontend`, so this path
/// does not require filesystem access and is suitable for a browser compiler.
pub fn compile_source_to_mir_json(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<String, Vec<CompilerDiagnostic>> {
    compile_source_to_mir_json_with_manifest(source, sample_rate, block_size)
        .map(|compiled| compiled.output)
        .map_err(|failure| failure.diagnostics)
}

pub fn compile_source_to_mir_json_with_inputs(
    source: &str,
    sample_rate: f32,
    block_size: u32,
    inputs: &CompileInputs,
) -> Result<String, Vec<CompilerDiagnostic>> {
    lower_source_to_mir_with_manifest_and_inputs(source, sample_rate, block_size, inputs)
        .and_then(|compiled| {
            encode_mir_compilation(compiled, "mir-json", onda_mir::to_json_optimized)
        })
        .map(|compiled| compiled.output)
        .map_err(|failure| failure.diagnostics)
}

/// Compiles source to the compact MessagePack MIR transport used by browser
/// backends. JSON remains available for diagnostics and external tooling.
pub fn compile_source_to_mir_messagepack(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<Vec<u8>, Vec<CompilerDiagnostic>> {
    compile_source_to_mir_messagepack_with_manifest(source, sample_rate, block_size)
        .map(|compiled| compiled.output)
        .map_err(|failure| failure.diagnostics)
}

pub fn compile_source_to_mir_messagepack_with_inputs(
    source: &str,
    sample_rate: f32,
    block_size: u32,
    inputs: &CompileInputs,
) -> Result<Vec<u8>, Vec<CompilerDiagnostic>> {
    lower_source_to_mir_with_manifest_and_inputs(source, sample_rate, block_size, inputs)
        .and_then(|compiled| {
            encode_mir_compilation(
                compiled,
                "mir-messagepack",
                onda_mir::to_messagepack_optimized,
            )
        })
        .map(|compiled| compiled.output)
        .map_err(|failure| failure.diagnostics)
}

/// Compiles an in-memory multi-file project without consulting the host
/// filesystem. Paths are project-relative and imports/includes resolve only
/// against `sources` or the embedded standard library.
pub fn compile_project_sources_to_mir_json(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<String, Vec<CompilerDiagnostic>> {
    compile_project_sources_to_mir_json_with_manifest(entry_path, sources, sample_rate, block_size)
        .map(|compiled| compiled.output)
        .map_err(|failure| failure.diagnostics)
}

pub fn compile_project_sources_to_mir_json_with_inputs(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
    inputs: &CompileInputs,
) -> Result<String, Vec<CompilerDiagnostic>> {
    lower_project_sources_to_mir_with_manifest_and_inputs(
        entry_path,
        sources,
        sample_rate,
        block_size,
        inputs,
    )
    .and_then(|compiled| encode_mir_compilation(compiled, "mir-json", onda_mir::to_json_optimized))
    .map(|compiled| compiled.output)
    .map_err(|failure| failure.diagnostics)
}

pub fn compile_project_sources_to_mir_messagepack(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<Vec<u8>, Vec<CompilerDiagnostic>> {
    compile_project_sources_to_mir_messagepack_with_manifest(
        entry_path,
        sources,
        sample_rate,
        block_size,
    )
    .map(|compiled| compiled.output)
    .map_err(|failure| failure.diagnostics)
}

pub fn compile_project_sources_to_mir_messagepack_with_inputs(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
    inputs: &CompileInputs,
) -> Result<Vec<u8>, Vec<CompilerDiagnostic>> {
    lower_project_sources_to_mir_with_manifest_and_inputs(
        entry_path,
        sources,
        sample_rate,
        block_size,
        inputs,
    )
    .and_then(|compiled| {
        encode_mir_compilation(
            compiled,
            "mir-messagepack",
            onda_mir::to_messagepack_optimized,
        )
    })
    .map(|compiled| compiled.output)
    .map_err(|failure| failure.diagnostics)
}

/// Compiles an immutable, integrity-checked Onda project image without
/// consulting the host filesystem.
pub fn compile_project_image_to_mir_messagepack_with_manifest(
    image_bytes: &[u8],
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    compile_project_image_to_mir_messagepack_with_manifest_and_limits(
        image_bytes,
        sample_rate,
        block_size,
        ProjectLimits::default(),
    )
}

pub fn compile_project_image_to_mir_messagepack_with_inputs(
    image_bytes: &[u8],
    sample_rate: f32,
    block_size: u32,
    inputs: &CompileInputs,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    compile_project_image_to_mir_messagepack_with_manifest_and_limits_and_inputs(
        image_bytes,
        sample_rate,
        block_size,
        ProjectLimits::default(),
        inputs,
    )
}

fn compile_project_image_to_mir_messagepack_with_manifest_and_limits(
    image_bytes: &[u8],
    sample_rate: f32,
    block_size: u32,
    limits: ProjectLimits,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    compile_project_image_to_mir_messagepack_with_manifest_and_limits_and_inputs(
        image_bytes,
        sample_rate,
        block_size,
        limits,
        &CompileInputs::default(),
    )
}

fn compile_project_image_to_mir_messagepack_with_manifest_and_limits_and_inputs(
    image_bytes: &[u8],
    sample_rate: f32,
    block_size: u32,
    limits: ProjectLimits,
    inputs: &CompileInputs,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    compile_project_image_to_mir_messagepack_with_manifest_and_limits_and_request(
        image_bytes,
        sample_rate,
        block_size,
        limits,
        CompileInputRequest::Typed(inputs),
    )
}

fn compile_project_image_to_mir_messagepack_with_manifest_and_limits_and_request(
    image_bytes: &[u8],
    sample_rate: f32,
    block_size: u32,
    limits: ProjectLimits,
    input_request: CompileInputRequest<'_>,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    let config =
        compile_config(sample_rate, block_size).map_err(CompilerFailure::without_sources)?;
    let image = ProjectImage::deserialize(image_bytes, limits).map_err(|error| {
        CompilerFailure::without_sources(vec![CompilerDiagnostic::configuration(error.to_string())])
    })?;
    let source_image = image.sources().clone();
    let image_source_files = source_image
        .documents
        .iter()
        .map(|document| document.path.clone())
        .collect::<Vec<_>>();
    let loaded = source_image.replay(limits).map_err(|error| {
        CompilerFailure::with_sources(
            vec![CompilerDiagnostic::configuration(error.to_string())],
            image_source_files,
        )
    })?;
    let source_files = virtual_paths(Path::new(""), &loaded.sources.files);
    let lowered = lower_parsed_program(loaded.program, config, input_request)
        .map_err(|diagnostics| CompilerFailure::with_sources(diagnostics, source_files.clone()))?;
    let mut declarations = Vec::new();
    let grouped_ids = lowered
        .interface
        .buffer_arrays
        .iter()
        .flat_map(|array| array.first.index()..array.first.index() + array.len as usize)
        .collect::<std::collections::HashSet<_>>();
    let declaration =
        |name: String, buffer: &onda_mir::Buffer, array_len: usize, is_array: bool| {
            ProjectBufferDeclaration {
                name,
                element: match buffer.element {
                    onda_mir::ScalarType::F32 => BufferElement::F32,
                    onda_mir::ScalarType::F64 => BufferElement::F64,
                    onda_mir::ScalarType::I32 => BufferElement::I32,
                    onda_mir::ScalarType::I64 => BufferElement::I64,
                    onda_mir::ScalarType::Bool => BufferElement::Bool,
                },
                channels: match buffer.channels {
                    onda_mir::BufferChannels::Mono => ProjectBufferChannels::Mono,
                    onda_mir::BufferChannels::Static(channels) => {
                        ProjectBufferChannels::Static(channels)
                    }
                    onda_mir::BufferChannels::Dynamic => ProjectBufferChannels::Dynamic,
                },
                array_len,
                is_array,
            }
        };
    for (index, buffer) in lowered.interface.buffers.iter().enumerate() {
        if !grouped_ids.contains(&index) {
            declarations.push(declaration(buffer.name.clone(), buffer, 1, false));
        }
    }
    for array in &lowered.interface.buffer_arrays {
        let buffer = &lowered.interface.buffers[array.first.index()];
        declarations.push(declaration(
            array.name.clone(),
            buffer,
            array.len as usize,
            true,
        ));
    }
    image
        .validate_buffer_declarations(&declarations)
        .map_err(|error| {
            CompilerFailure::with_sources(
                vec![CompilerDiagnostic::configuration(error.to_string())],
                source_files.clone(),
            )
        })?;
    let output = onda_mir::to_messagepack_optimized(&lowered).map_err(|error| {
        CompilerFailure::with_sources(
            mir_encoding_error("mir-messagepack", error),
            source_files.clone(),
        )
    })?;
    Ok(CompilationOutput {
        output,
        source_files,
        source_image: Some(source_image),
    })
}

pub fn compile_source_to_mir_json_with_manifest(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<String>, CompilerFailure> {
    let compiled = lower_source_to_mir_with_manifest(source, sample_rate, block_size)?;
    encode_mir_compilation(compiled, "mir-json", onda_mir::to_json_optimized)
}

pub fn compile_source_to_mir_messagepack_with_manifest(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    let compiled = lower_source_to_mir_with_manifest(source, sample_rate, block_size)?;
    encode_mir_compilation(
        compiled,
        "mir-messagepack",
        onda_mir::to_messagepack_optimized,
    )
}

pub fn compile_project_sources_to_mir_json_with_manifest(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<String>, CompilerFailure> {
    let compiled =
        lower_project_sources_to_mir_with_manifest(entry_path, sources, sample_rate, block_size)?;
    encode_mir_compilation(compiled, "mir-json", onda_mir::to_json_optimized)
}

pub fn compile_project_sources_to_mir_messagepack_with_manifest(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<Vec<u8>>, CompilerFailure> {
    let compiled =
        lower_project_sources_to_mir_with_manifest(entry_path, sources, sample_rate, block_size)?;
    encode_mir_compilation(
        compiled,
        "mir-messagepack",
        onda_mir::to_messagepack_optimized,
    )
}

fn encode_mir_compilation<T, E>(
    compiled: CompilationOutput<onda_mir::OptimizedProgram>,
    stage: &'static str,
    encode: impl FnOnce(&onda_mir::OptimizedProgram) -> Result<T, E>,
) -> Result<CompilationOutput<T>, CompilerFailure>
where
    E: ToString,
{
    let output = encode(&compiled.output).map_err(|error| {
        CompilerFailure::with_sources(
            mir_encoding_error(stage, error),
            compiled.source_files.clone(),
        )
    })?;
    Ok(CompilationOutput {
        output,
        source_files: compiled.source_files,
        source_image: compiled.source_image,
    })
}

fn lower_source_to_mir_with_manifest(
    source: &str,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<onda_mir::OptimizedProgram>, CompilerFailure> {
    lower_source_to_mir_with_manifest_and_inputs(
        source,
        sample_rate,
        block_size,
        &CompileInputs::default(),
    )
}

fn lower_source_to_mir_with_manifest_and_inputs(
    source: &str,
    sample_rate: f32,
    block_size: u32,
    inputs: &CompileInputs,
) -> Result<CompilationOutput<onda_mir::OptimizedProgram>, CompilerFailure> {
    lower_source_to_mir_with_manifest_and_request(
        source,
        sample_rate,
        block_size,
        CompileInputRequest::Typed(inputs),
    )
}

fn lower_source_to_mir_with_manifest_and_request(
    source: &str,
    sample_rate: f32,
    block_size: u32,
    input_request: CompileInputRequest<'_>,
) -> Result<CompilationOutput<onda_mir::OptimizedProgram>, CompilerFailure> {
    let config =
        compile_config(sample_rate, block_size).map_err(CompilerFailure::without_sources)?;
    let parsed = parse_program(source).map_err(|diagnostics| {
        CompilerFailure::without_sources(
            diagnostics
                .into_iter()
                .map(|diagnostic| CompilerDiagnostic::source("parse", diagnostic))
                .collect::<Vec<_>>(),
        )
    })?;
    let output = lower_parsed_program(parsed, config, input_request)
        .map_err(CompilerFailure::without_sources)?;
    Ok(CompilationOutput {
        output,
        source_files: Vec::new(),
        source_image: None,
    })
}

fn lower_project_sources_to_mir_with_manifest(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
) -> Result<CompilationOutput<onda_mir::OptimizedProgram>, CompilerFailure> {
    lower_project_sources_to_mir_with_manifest_and_inputs(
        entry_path,
        sources,
        sample_rate,
        block_size,
        &CompileInputs::default(),
    )
}

fn lower_project_sources_to_mir_with_manifest_and_inputs(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
    inputs: &CompileInputs,
) -> Result<CompilationOutput<onda_mir::OptimizedProgram>, CompilerFailure> {
    lower_project_sources_to_mir_with_manifest_and_request(
        entry_path,
        sources,
        sample_rate,
        block_size,
        CompileInputRequest::Typed(inputs),
    )
}

fn lower_project_sources_to_mir_with_manifest_and_request(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
    input_request: CompileInputRequest<'_>,
) -> Result<CompilationOutput<onda_mir::OptimizedProgram>, CompilerFailure> {
    let config =
        compile_config(sample_rate, block_size).map_err(CompilerFailure::without_sources)?;
    let loaded = load_project_sources(entry_path, sources)?;
    let output =
        lower_parsed_program(loaded.program, config, input_request).map_err(|diagnostics| {
            CompilerFailure::with_sources(diagnostics, loaded.source_files.clone())
        })?;
    Ok(CompilationOutput {
        output,
        source_files: loaded.source_files,
        source_image: Some(loaded.source_image),
    })
}

fn load_project_sources(
    entry_path: &str,
    sources: &HashMap<String, String>,
) -> Result<LoadedProjectProgram, CompilerFailure> {
    // This is a logical namespace rather than a host filesystem path. Keeping
    // it relative avoids target-specific `Path::is_absolute` behavior on
    // `wasm32-unknown-unknown` while the virtual loader still confines every
    // lookup beneath the namespace.
    let root = PathBuf::from("onda-project");
    let mut overlays = HashMap::with_capacity(sources.len());
    for (path, source) in sources {
        let path = checked_project_path(path).map_err(CompilerFailure::without_sources)?;
        let full_path = root.join(path);
        if overlays.insert(full_path, source.clone()).is_some() {
            return Err(CompilerFailure::without_sources(vec![
                CompilerDiagnostic::configuration(
                    "project contains duplicate normalized source paths",
                ),
            ]));
        }
    }
    let entry_path =
        root.join(checked_project_path(entry_path).map_err(CompilerFailure::without_sources)?);
    if !overlays.contains_key(&entry_path) {
        return Err(CompilerFailure::without_sources(vec![
            CompilerDiagnostic::configuration(format!(
                "project entry '{entry_path}' is not present in the source map",
                entry_path = entry_path.display()
            )),
        ]));
    }
    let loaded =
        load_program_file_from_virtual_sources(&root, &entry_path, &overlays).map_err(|error| {
            let source_files = virtual_source_files(&root, &error.sources);
            let unresolved_source_files = virtual_paths(&root, &error.sources.unresolved_files);
            let diagnostics = error
                .diagnostics
                .into_iter()
                .map(|diagnostic| CompilerDiagnostic::source("parse", diagnostic))
                .collect::<Vec<_>>();
            CompilerFailure::with_source_manifest(
                diagnostics,
                source_files,
                unresolved_source_files,
            )
        })?;
    let source_files = virtual_source_files(&root, &loaded.sources);
    let source_image = SourceImage::from_portable_manifest(
        &entry_path,
        &root,
        &loaded.sources,
        ProjectLimits::default(),
    )
    .map_err(|error| {
        CompilerFailure::with_sources(
            vec![CompilerDiagnostic::configuration(error.to_string())],
            source_files.clone(),
        )
    })?;
    Ok(LoadedProjectProgram {
        program: loaded.program,
        source_files,
        source_image,
    })
}

fn virtual_source_files(root: &Path, manifest: &SourceManifest) -> Vec<String> {
    virtual_paths(root, &manifest.files)
}

fn virtual_paths(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.strip_prefix(root).unwrap_or(path))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

fn checked_project_path(path: &str) -> Result<PathBuf, Vec<CompilerDiagnostic>> {
    let portable_absolute = path.starts_with('/')
        || path.starts_with('\\')
        || matches!(path.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic());
    let path = Path::new(path);
    if path.as_os_str().is_empty() || portable_absolute || path.is_absolute() {
        return Err(vec![CompilerDiagnostic::configuration(
            "project source paths must be non-empty and relative",
        )]);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => normalized.push(component),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(vec![CompilerDiagnostic::configuration(format!(
                    "project source path '{}' escapes the virtual project root",
                    path.display()
                ))]);
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(vec![CompilerDiagnostic::configuration(
            "project source paths must identify a file",
        )]);
    }
    Ok(normalized)
}

fn lower_parsed_program(
    parsed: Program,
    config: onda_mir::CompileConfig,
    input_request: CompileInputRequest<'_>,
) -> Result<onda_mir::OptimizedProgram, Vec<CompilerDiagnostic>> {
    let inputs = input_request.resolve(&parsed)?;
    let typed = analyze_with_options_and_inputs(
        parsed,
        AnalysisOptions {
            sample_rate: config.sample_rate,
            block_size: config.block_size as usize,
        },
        inputs.as_ref(),
    )
    .map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| CompilerDiagnostic::source("semantic", diagnostic))
            .collect::<Vec<_>>()
    })?;
    lower_program_to_optimized_mir(&typed).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| CompilerDiagnostic {
                stage: "mir",
                code: DiagCode::Internal as u16,
                message: error.message,
                file: error.location.file(),
                line: error.location.line,
                column: error.location.column,
                end_line: error.location.end_line,
                end_column: error.location.end_column,
                trace: error.location.trace(),
            })
            .collect::<Vec<_>>()
    })
}

#[cfg(target_arch = "wasm32")]
fn inspect_parsed_compile_constants(
    parsed: Program,
    config: onda_mir::CompileConfig,
    input_request: CompileInputRequest<'_>,
) -> Result<Vec<CompileConstDescriptor>, Vec<CompilerDiagnostic>> {
    let inputs = input_request.resolve(&parsed)?;
    inspect_semantic_compile_constants(
        parsed,
        AnalysisOptions {
            sample_rate: config.sample_rate,
            block_size: config.block_size as usize,
        },
        inputs.as_ref(),
    )
    .map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| CompilerDiagnostic::source("semantic", diagnostic))
            .collect()
    })
}

#[cfg(target_arch = "wasm32")]
fn inspect_source_compile_constants_with_request(
    source: &str,
    sample_rate: f32,
    block_size: u32,
    input_request: CompileInputRequest<'_>,
) -> Result<Vec<CompileConstDescriptor>, CompilerFailure> {
    let config =
        compile_config(sample_rate, block_size).map_err(CompilerFailure::without_sources)?;
    let parsed = parse_program(source).map_err(|diagnostics| {
        CompilerFailure::without_sources(
            diagnostics
                .into_iter()
                .map(|diagnostic| CompilerDiagnostic::source("parse", diagnostic))
                .collect(),
        )
    })?;
    inspect_parsed_compile_constants(parsed, config, input_request)
        .map_err(CompilerFailure::without_sources)
}

#[cfg(target_arch = "wasm32")]
fn inspect_project_sources_compile_constants_with_request(
    entry_path: &str,
    sources: &HashMap<String, String>,
    sample_rate: f32,
    block_size: u32,
    input_request: CompileInputRequest<'_>,
) -> Result<Vec<CompileConstDescriptor>, CompilerFailure> {
    let config =
        compile_config(sample_rate, block_size).map_err(CompilerFailure::without_sources)?;
    let loaded = load_project_sources(entry_path, sources)?;
    inspect_parsed_compile_constants(loaded.program, config, input_request)
        .map_err(|diagnostics| CompilerFailure::with_sources(diagnostics, loaded.source_files))
}

#[cfg(target_arch = "wasm32")]
fn inspect_project_image_compile_constants_with_request(
    image_bytes: &[u8],
    sample_rate: f32,
    block_size: u32,
    limits: ProjectLimits,
    input_request: CompileInputRequest<'_>,
) -> Result<Vec<CompileConstDescriptor>, CompilerFailure> {
    let config =
        compile_config(sample_rate, block_size).map_err(CompilerFailure::without_sources)?;
    let image = ProjectImage::deserialize(image_bytes, limits).map_err(|error| {
        CompilerFailure::without_sources(vec![CompilerDiagnostic::configuration(error.to_string())])
    })?;
    let image_source_files = image
        .sources()
        .documents
        .iter()
        .map(|document| document.path.clone())
        .collect::<Vec<_>>();
    let loaded = image.sources().replay(limits).map_err(|error| {
        CompilerFailure::with_sources(
            vec![CompilerDiagnostic::configuration(error.to_string())],
            image_source_files,
        )
    })?;
    let source_files = virtual_paths(Path::new(""), &loaded.sources.files);
    inspect_parsed_compile_constants(loaded.program, config, input_request)
        .map_err(|diagnostics| CompilerFailure::with_sources(diagnostics, source_files))
}

#[cfg(target_arch = "wasm32")]
#[derive(Serialize)]
struct WebCompileConstDescriptor {
    name: String,
    element: &'static str,
    kind: &'static str,
    element_count: usize,
    values: Vec<serde_json::Value>,
}

#[cfg(target_arch = "wasm32")]
fn encode_web_compile_const_descriptors(
    descriptors: Vec<CompileConstDescriptor>,
) -> Result<String, wasm_bindgen::JsValue> {
    let descriptors = descriptors
        .into_iter()
        .map(|descriptor| {
            let kind = match descriptor.kind {
                CompileConstKind::Scalar => "scalar",
                CompileConstKind::FixedArray => "fixed-array",
                CompileConstKind::Array => "array",
            };
            let (element, values) = match descriptor.value {
                ConstValue::Scalar(value) => (
                    web_primitive_type(value.primitive_type()),
                    vec![web_compile_const_value(value)],
                ),
                ConstValue::Array {
                    elem_ty, values, ..
                } => {
                    let values = values.into_iter().map(web_compile_const_value).collect();
                    (web_primitive_type(elem_ty), values)
                }
            };
            WebCompileConstDescriptor {
                name: descriptor.name,
                element,
                kind,
                element_count: values.len(),
                values,
            }
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&descriptors)
        .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))
}

#[cfg(target_arch = "wasm32")]
fn web_primitive_type(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::Bool => "bool",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
    }
}

#[cfg(target_arch = "wasm32")]
fn web_compile_const_value(value: TypedConstValue) -> serde_json::Value {
    match value {
        TypedConstValue::Bool(value) => serde_json::Value::Bool(value),
        TypedConstValue::I32(value) => value.into(),
        TypedConstValue::I64(value) => value.to_string().into(),
        TypedConstValue::F32(value) => web_compile_float_value(value as f64),
        TypedConstValue::F64(value) => web_compile_float_value(value),
    }
}

#[cfg(target_arch = "wasm32")]
fn web_compile_float_value(value: f64) -> serde_json::Value {
    if value.is_nan() {
        "NaN".into()
    } else if value == f64::INFINITY {
        "Infinity".into()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".into()
    } else if value == 0.0 && value.is_sign_negative() {
        "-0".into()
    } else {
        serde_json::Value::from(value)
    }
}

fn compile_config(
    sample_rate: f32,
    block_size: u32,
) -> Result<onda_mir::CompileConfig, Vec<CompilerDiagnostic>> {
    onda_mir::CompileConfig::new(sample_rate, block_size)
        .map_err(|error| vec![CompilerDiagnostic::configuration(error.to_string())])
}

fn mir_encoding_error(stage: &'static str, error: impl ToString) -> Vec<CompilerDiagnostic> {
    vec![CompilerDiagnostic {
        stage,
        code: DiagCode::Internal as u16,
        message: error.to_string(),
        file: None,
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
        trace: Vec::new(),
    }]
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn mir_schema_version() -> u32 {
    MIR_SCHEMA_VERSION
}

/// Stateful `onda lsp` server for browser Worker transports.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct OndaLsp {
    session: onda_lsp::LspSession,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl OndaLsp {
    #[wasm_bindgen::prelude::wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            session: onda_lsp::LspSession::new(),
        }
    }

    pub fn set_analysis_options(
        &mut self,
        sample_rate: f32,
        block_size: u32,
    ) -> Result<(), wasm_bindgen::JsValue> {
        let config = onda_mir::CompileConfig::new(sample_rate, block_size)
            .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))?;
        self.session.set_analysis_options(AnalysisOptions {
            sample_rate: config.sample_rate,
            block_size: config.block_size as usize,
        });
        Ok(())
    }

    /// Accepts one JSON-RPC LSP message and returns a JSON array containing
    /// all responses and notifications emitted synchronously for it.
    pub fn handle_message(&mut self, message_json: &str) -> Result<String, wasm_bindgen::JsValue> {
        self.session
            .handle_message_json(message_json)
            .map_err(|error| wasm_bindgen::JsValue::from_str(&error))
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for OndaLsp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct FrontendMessagePackCompilation {
    mir: Vec<u8>,
    source_files_json: String,
    source_image_json: String,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl FrontendMessagePackCompilation {
    pub fn take_mir(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.mir)
    }

    pub fn source_files_json(&self) -> String {
        self.source_files_json.clone()
    }

    pub fn source_image_json(&self) -> String {
        self.source_image_json.clone()
    }
}

#[cfg(target_arch = "wasm32")]
fn frontend_messagepack_compilation(
    compiled: CompilationOutput<Vec<u8>>,
) -> FrontendMessagePackCompilation {
    FrontendMessagePackCompilation {
        mir: compiled.output,
        source_files_json: encode_source_files(&compiled.source_files),
        source_image_json: encode_source_image(compiled.source_image.as_ref()),
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct FrontendJsonCompilation {
    mir: String,
    source_files_json: String,
    source_image_json: String,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl FrontendJsonCompilation {
    pub fn take_mir(&mut self) -> String {
        std::mem::take(&mut self.mir)
    }

    pub fn source_files_json(&self) -> String {
        self.source_files_json.clone()
    }

    pub fn source_image_json(&self) -> String {
        self.source_image_json.clone()
    }
}

#[cfg(target_arch = "wasm32")]
fn frontend_json_compilation(compiled: CompilationOutput<String>) -> FrontendJsonCompilation {
    FrontendJsonCompilation {
        mir: compiled.output,
        source_files_json: encode_source_files(&compiled.source_files),
        source_image_json: encode_source_image(compiled.source_image.as_ref()),
    }
}

#[cfg(target_arch = "wasm32")]
fn encode_source_files(source_files: &[String]) -> String {
    serde_json::to_string(source_files).unwrap_or_else(|_| "[]".to_owned())
}

#[cfg(target_arch = "wasm32")]
fn encode_source_image(source_image: Option<&SourceImage>) -> String {
    serde_json::to_string(&source_image).unwrap_or_else(|_| "null".to_owned())
}

#[cfg(target_arch = "wasm32")]
fn project_js_error(error: impl ToString) -> wasm_bindgen::JsValue {
    wasm_bindgen::JsValue::from_str(&error.to_string())
}

#[cfg(target_arch = "wasm32")]
fn web_buffer_element(value: &str) -> Result<BufferElement, wasm_bindgen::JsValue> {
    match value {
        "bool" => Ok(BufferElement::Bool),
        "i32" => Ok(BufferElement::I32),
        "i64" => Ok(BufferElement::I64),
        "f32" => Ok(BufferElement::F32),
        "f64" => Ok(BufferElement::F64),
        _ => Err(project_js_error(format!(
            "unsupported Onda buffer element '{value}'"
        ))),
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn project_image_format_version() -> u32 {
    onda_project::ONDA_PROJECT_IMAGE_FORMAT_VERSION
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn buffer_asset_format_version() -> u32 {
    onda_project::ONDA_BUFFER_FORMAT_VERSION
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn current_stdlib_digest() -> String {
    onda_project::current_stdlib_digest()
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct WebProjectImageBuilder {
    sources: Option<SourceImage>,
    buffer_bindings: BTreeMap<String, AssetId>,
    assets: BTreeMap<AssetId, BufferAsset>,
    total_buffer_bytes: usize,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl WebProjectImageBuilder {
    #[wasm_bindgen::prelude::wasm_bindgen(constructor)]
    pub fn new(source_image_json: &str) -> Result<Self, wasm_bindgen::JsValue> {
        let sources: SourceImage =
            serde_json::from_str(source_image_json).map_err(project_js_error)?;
        sources
            .replay(web_project_limits())
            .map_err(project_js_error)?;
        Ok(Self {
            sources: Some(sources),
            buffer_bindings: BTreeMap::new(),
            assets: BTreeMap::new(),
            total_buffer_bytes: 0,
        })
    }

    pub fn add_buffer(
        &mut self,
        name: &str,
        ondabuffer_bytes: &[u8],
    ) -> Result<(), wasm_bindgen::JsValue> {
        if self.sources.is_none() {
            return Err(project_js_error(
                "project image builder has already been serialized",
            ));
        }
        if name.is_empty() {
            return Err(project_js_error("project buffer name must not be empty"));
        }
        if self.buffer_bindings.contains_key(name) {
            return Err(project_js_error(format!(
                "project buffer '{name}' was added more than once"
            )));
        }
        let limits = web_project_limits();
        if self.buffer_bindings.len() >= limits.max_buffer_bindings {
            return Err(project_js_error(format!(
                "project exceeds the {} buffer binding limit",
                limits.max_buffer_bindings
            )));
        }
        let validated = validate_ondabuffer(ondabuffer_bytes, limits).map_err(project_js_error)?;
        let id = AssetId::from_buffer_digest(validated.content_digest());
        if !self.assets.contains_key(&id) {
            let asset = validated
                .decode_with_remaining_asset_budget(limits, self.total_buffer_bytes)
                .map_err(project_js_error)?;
            let total_buffer_bytes = self
                .total_buffer_bytes
                .checked_add(asset.payload_bytes())
                .ok_or_else(|| project_js_error("project buffer byte total overflows"))?;
            if total_buffer_bytes > limits.max_total_asset_bytes {
                return Err(project_js_error(format!(
                    "project buffer payloads exceed the {} byte limit",
                    limits.max_total_asset_bytes
                )));
            }
            self.assets.insert(id.clone(), asset);
            self.total_buffer_bytes = total_buffer_bytes;
        }
        self.buffer_bindings.insert(name.to_owned(), id);
        Ok(())
    }

    pub fn serialize(&mut self) -> Result<Vec<u8>, wasm_bindgen::JsValue> {
        let sources = self
            .sources
            .take()
            .ok_or_else(|| project_js_error("project image builder may only be serialized once"))?;
        ProjectImage::new(
            sources,
            std::mem::take(&mut self.buffer_bindings),
            std::mem::take(&mut self.assets),
        )
        .and_then(serialize_web_project_image)
        .map_err(project_js_error)
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct WebMaterializedProjectBuilder {
    files: BTreeMap<String, Vec<u8>>,
    manifest_path: Option<String>,
    total_bytes: usize,
    serialized: bool,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl WebMaterializedProjectBuilder {
    #[wasm_bindgen::prelude::wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            manifest_path: None,
            total_bytes: 0,
            serialized: false,
        }
    }

    pub fn select_project(&mut self, manifest_path: &str) -> Result<(), wasm_bindgen::JsValue> {
        if self.serialized {
            return Err(project_js_error(
                "materialized project builder has already been serialized",
            ));
        }
        if self.manifest_path.is_some() {
            return Err(project_js_error(
                "materialized project builder already has a selected manifest",
            ));
        }
        self.manifest_path = Some(manifest_path.to_owned());
        Ok(())
    }

    pub fn add_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), wasm_bindgen::JsValue> {
        if self.serialized {
            return Err(project_js_error(
                "materialized project builder has already been serialized",
            ));
        }
        if self.files.contains_key(path) {
            return Err(project_js_error(format!(
                "project file '{path}' was added more than once"
            )));
        }
        let limits = web_project_limits();
        let max_files = limits.max_materialized_file_count();
        if self.files.len() >= max_files {
            return Err(project_js_error(format!(
                "project contains more than {max_files} files"
            )));
        }
        let max_file_bytes = limits.max_materialized_file_bytes();
        if bytes.len() > max_file_bytes {
            return Err(project_js_error(format!(
                "project file '{path}' exceeds the {max_file_bytes} byte browser limit"
            )));
        }
        let total_limit = limits.max_materialized_total_bytes();
        let total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| project_js_error("project file byte total overflows"))?;
        if total_bytes > total_limit {
            return Err(project_js_error(format!(
                "project files exceed the {total_limit} byte browser limit"
            )));
        }
        self.files.insert(path.to_owned(), bytes.to_vec());
        self.total_bytes = total_bytes;
        Ok(())
    }

    pub fn serialize(&mut self) -> Result<Vec<u8>, wasm_bindgen::JsValue> {
        if std::mem::replace(&mut self.serialized, true) {
            return Err(project_js_error(
                "materialized project builder may only be serialized once",
            ));
        }
        let files = std::mem::take(&mut self.files);
        let image = match self.manifest_path.as_deref() {
            Some(manifest_path) => ProjectImage::from_materialized_files_with_manifest(
                &files,
                manifest_path,
                web_project_limits(),
            ),
            None => ProjectImage::from_materialized_files(&files, web_project_limits()),
        };
        drop(files);
        image
            .and_then(serialize_web_project_image)
            .map_err(project_js_error)
    }
}

#[cfg(target_arch = "wasm32")]
impl Default for WebMaterializedProjectBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Serialize)]
struct WebProjectBufferInfo<'a> {
    name: &'a str,
    asset_id: &'a str,
    element: BufferElement,
    frames: u32,
    channels: u32,
    sample_rate: f32,
}

#[cfg(target_arch = "wasm32")]
#[derive(Serialize)]
struct WebProjectImageInfo<'a> {
    format_version: u32,
    content_digest: String,
    sources: &'a SourceImage,
    buffers: Vec<WebProjectBufferInfo<'a>>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn inspect_project_image(image_bytes: &[u8]) -> Result<String, wasm_bindgen::JsValue> {
    let image =
        ProjectImage::deserialize(image_bytes, web_project_limits()).map_err(project_js_error)?;
    let buffers = image
        .buffer_bindings()
        .iter()
        .map(|(name, id)| {
            let asset = image
                .assets()
                .get(id)
                .expect("validated project binding must resolve");
            WebProjectBufferInfo {
                name,
                asset_id: id.as_str(),
                element: asset.element(),
                frames: asset.frames,
                channels: asset.channels,
                sample_rate: asset.sample_rate,
            }
        })
        .collect();
    serde_json::to_string(&WebProjectImageInfo {
        format_version: onda_project::ONDA_PROJECT_IMAGE_FORMAT_VERSION,
        content_digest: image.content_digest_string(),
        sources: image.sources(),
        buffers,
    })
    .map_err(project_js_error)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct WebProjectMaterializationPlan {
    plan: MaterializationPlan,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl WebProjectMaterializationPlan {
    pub fn directories_json(&self) -> String {
        serde_json::to_string(&self.plan.directories).unwrap_or_else(|_| "[]".to_owned())
    }

    pub fn file_count(&self) -> usize {
        self.plan.files.len()
    }

    pub fn file_path(&self, index: usize) -> Option<String> {
        self.plan
            .files
            .get(index)
            .map(|file| file.relative_path.clone())
    }

    pub fn file_bytes(&self, index: usize) -> Option<Vec<u8>> {
        self.plan.files.get(index).map(|file| file.bytes.clone())
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn materialize_project_image(
    image_bytes: &[u8],
    asset_file_names_json: &str,
) -> Result<WebProjectMaterializationPlan, wasm_bindgen::JsValue> {
    let asset_file_names: BTreeMap<String, String> =
        serde_json::from_str(asset_file_names_json).map_err(project_js_error)?;
    ProjectImage::deserialize(image_bytes, web_project_limits())
        .and_then(|image| image.materialization_plan_with_asset_file_names(&asset_file_names))
        .map(|plan| WebProjectMaterializationPlan { plan })
        .map_err(project_js_error)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn encode_buffer_asset(
    element: &str,
    frames: u32,
    channels: u32,
    sample_rate: f32,
    canonical_payload: &[u8],
) -> Result<Vec<u8>, wasm_bindgen::JsValue> {
    let element = web_buffer_element(element)?;
    let limits = web_project_limits();
    if canonical_payload.len() > limits.max_asset_bytes {
        return Err(project_js_error(format!(
            "Onda buffer payload exceeds the {} byte browser limit",
            limits.max_asset_bytes
        )));
    }
    let samples = BufferSamples::from_canonical_le_bytes(element, canonical_payload)
        .map_err(project_js_error)?;
    let asset = BufferAsset {
        frames,
        channels,
        sample_rate,
        samples,
    };
    asset
        .validate(&limits)
        .and_then(|()| encode_ondabuffer(&asset))
        .map_err(project_js_error)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct WebDecodedBufferAsset {
    asset: BufferAsset,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl WebDecodedBufferAsset {
    pub fn element(&self) -> String {
        self.asset.element().to_string()
    }

    pub fn frames(&self) -> u32 {
        self.asset.frames
    }

    pub fn channels(&self) -> u32 {
        self.asset.channels
    }

    pub fn sample_rate(&self) -> f32 {
        self.asset.sample_rate
    }

    pub fn canonical_payload(&self) -> Vec<u8> {
        self.asset.canonical_payload()
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn decode_buffer_asset(
    ondabuffer_bytes: &[u8],
) -> Result<WebDecodedBufferAsset, wasm_bindgen::JsValue> {
    decode_ondabuffer(ondabuffer_bytes, web_project_limits())
        .map(|asset| WebDecodedBufferAsset { asset })
        .map_err(project_js_error)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn decode_buffer_file(
    bytes: &[u8],
    path: &str,
) -> Result<WebDecodedBufferAsset, wasm_bindgen::JsValue> {
    decode_buffer_bytes(bytes, Path::new(path), web_project_limits())
        .map(|asset| WebDecodedBufferAsset { asset })
        .map_err(project_js_error)
}

#[cfg(target_arch = "wasm32")]
fn web_project_limits() -> ProjectLimits {
    const MAX_DECODED_BUFFER_BYTES: usize = 16 * 1024 * 1024 * std::mem::size_of::<f32>();
    ProjectLimits {
        max_asset_bytes: MAX_DECODED_BUFFER_BYTES,
        max_total_asset_bytes: MAX_DECODED_BUFFER_BYTES,
        ..ProjectLimits::default()
    }
}

#[cfg(target_arch = "wasm32")]
fn serialize_web_project_image(image: ProjectImage) -> Result<Vec<u8>, onda_project::ProjectError> {
    image.serialize_with_limits(web_project_limits())
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn inspect_source_compile_constants(
    source: &str,
    sample_rate: f32,
    block_size: u32,
    constants_json: &str,
) -> Result<String, wasm_bindgen::JsValue> {
    inspect_source_compile_constants_with_request(
        source,
        sample_rate,
        block_size,
        CompileInputRequest::Json(constants_json),
    )
    .map_err(compiler_failure_js)
    .and_then(encode_web_compile_const_descriptors)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn inspect_source_workspace_compile_constants(
    entry_path: &str,
    sources_json: &str,
    sample_rate: f32,
    block_size: u32,
    constants_json: &str,
) -> Result<String, wasm_bindgen::JsValue> {
    let sources = decode_project_sources_json(sources_json)?;
    inspect_project_sources_compile_constants_with_request(
        entry_path,
        &sources,
        sample_rate,
        block_size,
        CompileInputRequest::Json(constants_json),
    )
    .map_err(compiler_failure_js)
    .and_then(encode_web_compile_const_descriptors)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn inspect_project_image_compile_constants(
    image_bytes: &[u8],
    sample_rate: f32,
    block_size: u32,
    constants_json: &str,
) -> Result<String, wasm_bindgen::JsValue> {
    inspect_project_image_compile_constants_with_request(
        image_bytes,
        sample_rate,
        block_size,
        web_project_limits(),
        CompileInputRequest::Json(constants_json),
    )
    .map_err(compiler_failure_js)
    .and_then(encode_web_compile_const_descriptors)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_to_mir_json(
    source: &str,
    sample_rate: f32,
    block_size: u32,
    constants_json: &str,
) -> Result<FrontendJsonCompilation, wasm_bindgen::JsValue> {
    lower_source_to_mir_with_manifest_and_request(
        source,
        sample_rate,
        block_size,
        CompileInputRequest::Json(constants_json),
    )
    .and_then(|compiled| encode_mir_compilation(compiled, "mir-json", onda_mir::to_json_optimized))
    .map(frontend_json_compilation)
    .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_to_mir_messagepack(
    source: &str,
    sample_rate: f32,
    block_size: u32,
    constants_json: &str,
) -> Result<FrontendMessagePackCompilation, wasm_bindgen::JsValue> {
    lower_source_to_mir_with_manifest_and_request(
        source,
        sample_rate,
        block_size,
        CompileInputRequest::Json(constants_json),
    )
    .and_then(|compiled| {
        encode_mir_compilation(
            compiled,
            "mir-messagepack",
            onda_mir::to_messagepack_optimized,
        )
    })
    .map(frontend_messagepack_compilation)
    .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_source_workspace_to_mir_json(
    entry_path: &str,
    sources_json: &str,
    sample_rate: f32,
    block_size: u32,
    constants_json: &str,
) -> Result<FrontendJsonCompilation, wasm_bindgen::JsValue> {
    let sources = decode_project_sources_json(sources_json)?;
    lower_project_sources_to_mir_with_manifest_and_request(
        entry_path,
        &sources,
        sample_rate,
        block_size,
        CompileInputRequest::Json(constants_json),
    )
    .and_then(|compiled| encode_mir_compilation(compiled, "mir-json", onda_mir::to_json_optimized))
    .map(frontend_json_compilation)
    .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_source_workspace_to_mir_messagepack(
    entry_path: &str,
    sources_json: &str,
    sample_rate: f32,
    block_size: u32,
    constants_json: &str,
) -> Result<FrontendMessagePackCompilation, wasm_bindgen::JsValue> {
    let sources = decode_project_sources_json(sources_json)?;
    lower_project_sources_to_mir_with_manifest_and_request(
        entry_path,
        &sources,
        sample_rate,
        block_size,
        CompileInputRequest::Json(constants_json),
    )
    .and_then(|compiled| {
        encode_mir_compilation(
            compiled,
            "mir-messagepack",
            onda_mir::to_messagepack_optimized,
        )
    })
    .map(frontend_messagepack_compilation)
    .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn compile_project_image_to_mir_messagepack(
    image_bytes: &[u8],
    sample_rate: f32,
    block_size: u32,
    constants_json: &str,
) -> Result<FrontendMessagePackCompilation, wasm_bindgen::JsValue> {
    compile_project_image_to_mir_messagepack_with_manifest_and_limits_and_request(
        image_bytes,
        sample_rate,
        block_size,
        web_project_limits(),
        CompileInputRequest::Json(constants_json),
    )
    .map(frontend_messagepack_compilation)
    .map_err(compiler_failure_js)
}

#[cfg(target_arch = "wasm32")]
fn decode_project_sources_json(
    sources_json: &str,
) -> Result<HashMap<String, String>, wasm_bindgen::JsValue> {
    serde_json::from_str(sources_json).map_err(|error| {
        compiler_failure_js(CompilerFailure::without_sources(vec![
            CompilerDiagnostic::configuration(format!("invalid project source map JSON: {error}")),
        ]))
    })
}

#[cfg(target_arch = "wasm32")]
fn compiler_failure_js(failure: CompilerFailure) -> wasm_bindgen::JsValue {
    let encoded = serde_json::to_string(&failure).unwrap_or_else(|_| {
        "{\"diagnostics\":[{\"stage\":\"internal\",\"message\":\"failed to encode compiler diagnostics\"}],\"source_files\":[],\"unresolved_source_files\":[]}".to_owned()
    });
    wasm_bindgen::JsValue::from_str(&encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_in_memory_source_to_valid_deterministic_mir() {
        let source = r#"
params:
  gain = 0.25
outs:
  out1
init:
  phase = 0.0
sample:
  phase = phase + gain
  out1 = phase
"#;
        let first = compile_source_to_mir_json(source, 48_000.0, 128)
            .expect("source should compile to MIR JSON");
        let second = compile_source_to_mir_json(source, 48_000.0, 128)
            .expect("source should compile deterministically");
        assert_eq!(first, second);
        let mir = unsafe { onda_mir::from_json_with_producer_proofs(&first) }
            .expect("JSON should decode as trusted producer MIR");
        assert_eq!(mir.config.sample_rate, 48_000.0);
        assert_eq!(mir.config.block_size, 128);

        let packed = compile_source_to_mir_messagepack(source, 48_000.0, 128)
            .expect("source should compile to MessagePack MIR");
        assert!(packed.len() < first.len());
        let packed_mir = unsafe { onda_mir::from_messagepack_with_producer_proofs(&packed) }
            .expect("MessagePack should decode as trusted producer MIR");
        assert_eq!(packed_mir.as_program(), mir.as_program());
    }

    #[test]
    fn typed_compile_inputs_drive_the_same_browser_compilation_pipeline() {
        let source = r#"
config const Selected: i32 = missing_default
sample:
  out1 = f32(Selected)
"#;
        let mut inputs = CompileInputs::default();
        inputs.constants.insert(
            "Selected".to_owned(),
            ConstValue::Scalar(TypedConstValue::I32(8)),
        );
        let packed = compile_source_to_mir_messagepack_with_inputs(source, 48_000.0, 128, &inputs)
            .expect("browser compilation should use the selected constant");
        unsafe { onda_mir::from_messagepack_with_producer_proofs(&packed) }
            .expect("compile-input result should be valid producer MIR");

        let json = compile_source_to_mir_json_with_inputs(source, 48_000.0, 128, &inputs)
            .expect("JSON compilation should accept the same inputs");
        unsafe { onda_mir::from_json_with_producer_proofs(&json) }
            .expect("JSON compile-input result should be valid producer MIR");

        let sources = HashMap::from([("main.onda".to_owned(), source.to_owned())]);
        let json = compile_project_sources_to_mir_json_with_inputs(
            "main.onda",
            &sources,
            48_000.0,
            128,
            &inputs,
        )
        .expect("workspace JSON compilation should accept the same inputs");
        unsafe { onda_mir::from_json_with_producer_proofs(&json) }
            .expect("workspace JSON compile-input result should be valid producer MIR");
    }

    #[test]
    fn compiles_embedded_standard_library_modules_without_a_filesystem() {
        let source = r#"
import std/osc

outs:
  out1
init:
  oscillator = std::osc::Sine()
sample:
  out1 = oscillator()
"#;
        let json = compile_source_to_mir_json(source, 48_000.0, 128)
            .expect("embedded standard library source should compile");
        unsafe { onda_mir::from_json_with_producer_proofs(&json) }
            .expect("standard-library result should be valid producer MIR");
    }

    #[test]
    fn compiles_a_multi_file_virtual_project_without_disk_files() {
        let sources = HashMap::from([
            (
                "main.onda".to_owned(),
                "import dsp\nouts 1\nsample:\n  out1 = DSP::value()\n".to_owned(),
            ),
            (
                "dsp.onda".to_owned(),
                "namespace DSP:\n  def value() -> f32:\n    return 0.75\n".to_owned(),
            ),
        ]);
        let json = compile_project_sources_to_mir_json("main.onda", &sources, 48_000.0, 128)
            .expect("virtual import should compile without filesystem access");
        unsafe { onda_mir::from_json_with_producer_proofs(&json) }
            .expect("virtual project result should be valid producer MIR");
    }

    #[test]
    fn project_compilation_returns_only_contributing_virtual_sources() {
        let sources = HashMap::from([
            (
                "main.onda".to_owned(),
                "include \"./shared.onda\"\nimport dsp/filter\nimport std/math\nouts 1\nsample:\n  out1 = DSP::value()\n"
                    .to_owned(),
            ),
            (
                "shared.onda".to_owned(),
                "const shared = 0.25\n".to_owned(),
            ),
            (
                "dsp/filter.onda".to_owned(),
                "namespace DSP:\n  def value() -> f32:\n    return 0.75\n".to_owned(),
            ),
            (
                "unused.onda".to_owned(),
                "const unused = 1.0\n".to_owned(),
            ),
        ]);
        let compiled_messagepack = compile_project_sources_to_mir_messagepack_with_manifest(
            "main.onda",
            &sources,
            48_000.0,
            128,
        )
        .expect("virtual project should compile");
        assert_eq!(
            compiled_messagepack.source_files,
            vec!["main.onda", "shared.onda", "dsp/filter.onda"]
        );

        let compiled_json =
            compile_project_sources_to_mir_json_with_manifest("main.onda", &sources, 48_000.0, 128)
                .expect("virtual project should compile to JSON");
        assert_eq!(
            compiled_json.source_files,
            vec!["main.onda", "shared.onda", "dsp/filter.onda"]
        );
        unsafe { onda_mir::from_json_with_producer_proofs(&compiled_json.output) }
            .expect("manifest-bearing JSON result should be valid producer MIR");
    }

    #[test]
    fn failed_project_compilation_returns_partial_source_manifest() {
        let sources = HashMap::from([
            (
                "main.onda".to_owned(),
                "import dsp\nouts 1\nsample:\n  out1 = 0.0\n".to_owned(),
            ),
            ("dsp.onda".to_owned(), "this is not valid onda\n".to_owned()),
        ]);
        let failure = compile_project_sources_to_mir_messagepack_with_manifest(
            "main.onda",
            &sources,
            48_000.0,
            128,
        )
        .expect_err("dependency should fail to parse");
        assert_eq!(failure.source_files, vec!["main.onda", "dsp.onda"]);
        assert!(failure.unresolved_source_files.is_empty());
        assert!(!failure.diagnostics.is_empty());
    }

    #[test]
    fn failed_project_compilation_returns_unresolved_source_candidates() {
        let sources = HashMap::from([("main.onda".to_owned(), "import dsp/filter\n".to_owned())]);
        let failure = compile_project_sources_to_mir_messagepack_with_manifest(
            "main.onda",
            &sources,
            48_000.0,
            128,
        )
        .expect_err("missing dependency should fail");
        assert_eq!(failure.source_files, vec!["main.onda"]);
        assert_eq!(
            failure.unresolved_source_files,
            vec!["dsp/filter.onda", "dsp/filter.on"]
        );
    }

    #[test]
    fn rejects_project_paths_that_escape_the_virtual_root() {
        let sources = HashMap::from([("../main.onda".to_owned(), String::new())]);
        let errors = compile_project_sources_to_mir_json("../main.onda", &sources, 48_000.0, 128)
            .expect_err("escaping project paths should fail");
        assert_eq!(errors[0].stage, "configuration");
    }

    #[test]
    fn rejects_nested_imports_and_includes_that_escape_the_virtual_root() {
        for source in [
            "include \"../outside.onda\"\n",
            "include \"/tmp/outside.onda\"\n",
            "import ../outside\n",
        ] {
            let sources = HashMap::from([("main.onda".to_owned(), source.to_owned())]);
            let errors = compile_project_sources_to_mir_json("main.onda", &sources, 48_000.0, 128)
                .expect_err("nested virtual path escape should fail");
            assert_eq!(errors[0].stage, "parse");
            assert!(errors[0].message.contains("escapes project root"));
        }
    }

    #[test]
    fn rejects_virtual_paths_that_escape_and_reenter_the_project_root() {
        let sources = HashMap::from([
            (
                "main.onda".to_owned(),
                "include \"../onda-project/lib.onda\"\n".to_owned(),
            ),
            ("lib.onda".to_owned(), "const captured = 1.0\n".to_owned()),
        ]);
        let errors = compile_project_sources_to_mir_json("main.onda", &sources, 48_000.0, 128)
            .expect_err("a virtual path must never cross the project root");
        assert_eq!(errors[0].stage, "parse");
        assert!(errors[0].message.contains("escapes project root"));
    }

    #[test]
    fn project_image_compilation_honors_host_asset_limits() {
        let sources = SourceImage {
            entry: "main.onda".to_owned(),
            stdlib_digest: onda_project::current_stdlib_digest(),
            documents: vec![onda_project::SourceDocument {
                path: "main.onda".to_owned(),
                contents: "outs 1\nsample:\n  out1 = 0.0\n".to_owned(),
            }],
            resolutions: Vec::new(),
        };
        let asset = onda_project::BufferAsset::new(
            2,
            1,
            48_000.0,
            onda_project::BufferSamples::F32(vec![0.0, 1.0]),
        )
        .expect("valid test asset");
        let image = ProjectImage::from_buffer_assets(
            sources,
            std::collections::BTreeMap::from([("sample".to_owned(), asset)]),
        )
        .and_then(|image| image.serialize())
        .expect("serialize test image");
        let limits = ProjectLimits {
            max_asset_bytes: 4,
            max_total_asset_bytes: 4,
            ..ProjectLimits::default()
        };

        let failure = compile_project_image_to_mir_messagepack_with_manifest_and_limits(
            &image, 48_000.0, 128, limits,
        )
        .expect_err("host asset limits must be enforced before compilation");
        assert!(failure.diagnostics[0].message.contains("byte limit"));
    }

    #[test]
    fn returns_structured_source_diagnostics() {
        let errors = compile_source_to_mir_json("sample:\n  out1 = missing\n", 48_000.0, 128)
            .expect_err("invalid source should fail");
        assert!(!errors.is_empty());
        assert!(errors
            .iter()
            .any(|diagnostic| diagnostic.stage == "semantic"));
        assert!(errors.iter().any(|diagnostic| diagnostic.line > 0));
    }

    #[test]
    fn rejects_invalid_host_configuration() {
        let errors = compile_source_to_mir_json("", f32::NAN, 0)
            .expect_err("invalid configuration should fail before parsing");
        assert_eq!(errors[0].stage, "configuration");

        let errors = compile_source_to_mir_json(
            "this source must not be parsed",
            48_000.0,
            i32::MAX as u32 + 1,
        )
        .expect_err("oversized blocks should fail before parsing");
        assert_eq!(errors[0].stage, "configuration");
        assert!(errors[0].message.contains("2147483647"));
    }
}
