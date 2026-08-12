#![deny(clippy::all)]

mod aggregates;
mod audio_outputs;
mod calls;
mod control_flow;
mod expressions;
mod lowerer_core;
mod scheduling;
mod slices;
mod values;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use onda_frontend::{
    ArrayElemType, AssignTarget, BinaryOp as AstBinaryOp, BuiltinFn, CmpOp, Diagnostic, Expr,
    LogicalOp, ParamScale, PrimitiveType, SourceLoc, Stmt, INTERNAL_BUFFER_READ2_FN,
    INTERNAL_BUFFER_READ3_FN, INTERNAL_BUFFER_READ_CHANNEL_FN, INTERNAL_BUFFER_WRITE2_FN,
    INTERNAL_BUFFER_WRITE3_FN, INTERNAL_BUFFER_WRITE_CHANNEL_FN,
};
use onda_mir::{
    BinaryOp as MirBinaryOp, Block as MirBlock, BoundsMode, CallArgument, CompareOp,
    FunctionAttributes, FunctionId, FunctionOrigin, InlineHint, Intrinsic, LocalId, ParameterId,
    Place, PlaceBase, Projection, Rvalue, ScalarType, ScalarValue, SourceFile, SourceFileId,
    SourceSpan, Statement, StatementKind, Type as MirType, TypeId, UnaryOp, Value,
};

use crate::internal_names::{
    runtime_buffer_alias_selector_symbol, runtime_proc_array_active_symbol, PROC_INDEX_BASE_ARG,
    PROC_INDEX_BUFFER_SELECT_SENTINEL, PROC_INDEX_CALL_SENTINEL, PROC_INDEX_EXPR_ARG,
};
use crate::{
    adapt_binary_operand_types, adapt_numeric_argument_types, builtin_constant_type,
    builtin_constant_value_f64, can_assign_expr_to_type, can_eval_const_expr_exact_int,
    effective_untyped_assignment_type, eval_const_expr_i64_exact, merge_numeric_types,
    parse_array_len_instance_base, parse_buffer_chans_instance_base,
    parse_buffer_samplerate_instance_base, resolve_call_args_at, AggregateLayoutTable,
    AggregatePathComponent, AnalysisOptions, ProcSincStageStateFields, ProcStepOversampleMeta,
    ResolvedInterfaceSlot, ResolvedInterfaceView, ReturnType, TypedArrayInfo, TypedBufferChannels,
    TypedConstValue, TypedEvent, TypedEventParamDefault, TypedEventParamType, TypedFieldType,
    TypedFnParam, TypedFunction, TypedNestedProcArray, TypedParamControl, TypedProgram,
    TypedStructField, TypedValueRange,
};

const SINC_A1_COEFF: f64 = 0.039_151_597_734_460_045;
const SINC_A2_COEFF: f64 = 0.302_646_848_328_493_4;
const SINC_A3_COEFF: f64 = 0.674_615_918_546_963_9;
const SINC_B1_COEFF: f64 = 0.147_377_113_601_046_6;
const SINC_B2_COEFF: f64 = 0.482_468_542_769_700_14;
const SINC_B3_COEFF: f64 = 0.883_005_025_769_373_1;
const SINC_TAP_NAMES: [&str; 8] = ["a0", "a1", "a2", "a3", "b0", "b1", "b2", "b3"];
const MAX_STATIC_SINC_STAGE_ITERATIONS: usize = 2;

#[derive(Clone, Copy, Default)]
struct SliceSelection<'a> {
    selector: Option<&'a Expr>,
    channel: Option<&'a Expr>,
    start: Option<&'a Expr>,
    end: Option<&'a Expr>,
}

/// An error at the boundary between analyzed Onda code and MIR.
///
/// A successfully analyzed program should only produce these while a MIR
/// migration slice is incomplete or if semantic information was lost before
/// lowering. They are kept distinct from source diagnostics so callers cannot
/// accidentally present backend limitations as language errors.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MirLoweringError {
    pub message: String,
    pub location: SourceLoc,
}

impl MirLoweringError {
    fn new(message: impl Into<String>, location: SourceLoc) -> Self {
        Self {
            message: message.into(),
            location,
        }
    }
}

impl fmt::Display for MirLoweringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.location.is_zero() {
            write!(f, "MIR lowering: {}", self.message)
        } else {
            write!(
                f,
                "MIR lowering at {}:{}: {}",
                self.location.line, self.location.column, self.message
            )
        }
    }
}

impl std::error::Error for MirLoweringError {}

/// Low-level transactional helper for complete-program lowering and focused
/// MIR tests. It lowers the reachable closure of specialized user functions
/// whose storage uses scalars, scalar tuples, and primitive slices into `mir`.
///
/// Production callers should use [`lower_program_to_optimized_mir`], which
/// returns a complete program with its validation and optimization proof.
/// This helper supports scalar value
/// parameters, primitive slice parameters, scalarized tuple parameters/results,
/// resolved direct calls (including named/default arguments), tuple locals/destructuring/indexing,
/// scalar locals, casts, arithmetic, comparisons, intrinsics, short-circuit
/// logic, branches, `while`, directional `for`, loop control, no-result
/// functions, data structs, structure-of-slices data-struct arrays, and the
/// normalized proc-array `(len, active slots, flattened state leaves)` ABI.
///
/// The operation is transactional: on error, types, immutable constant data,
/// source files, and functions in `mir` are left unchanged.
fn lower_scalar_user_functions_to_mir(
    program: &TypedProgram,
    mir: &mut onda_mir::Program,
) -> Result<Vec<FunctionId>, Vec<MirLoweringError>> {
    let function_base = mir.functions.len();
    let mut function_indices = HashMap::<String, usize>::new();
    let mut errors = Vec::new();

    for (index, function) in program.defs.iter().enumerate() {
        if function_indices
            .insert(function.name.clone(), index)
            .is_some()
        {
            errors.push(MirLoweringError::new(
                format!("duplicate specialized function name '{}'", function.name),
                function_location(function),
            ));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let contexts = discover_function_contexts(program, mir.config, &function_indices);
    for (index, function) in program.defs.iter().enumerate() {
        if !contexts.contains_key(&index) {
            continue;
        }
        if !function.type_params.is_empty() {
            errors.push(MirLoweringError::new(
                format!(
                    "function '{}' still has unresolved type parameters",
                    function.name
                ),
                function_location(function),
            ));
        }
        for (param_index, kind) in function.param_kinds.iter().enumerate() {
            if !matches!(
                kind,
                TypedFnParam::Scalar { .. }
                    | TypedFnParam::Array { .. }
                    | TypedFnParam::Tuple { .. }
                    | TypedFnParam::Buffer { .. }
                    | TypedFnParam::BufferArray { .. }
                    | TypedFnParam::Struct { .. }
                    | TypedFnParam::StructArray { .. }
                    | TypedFnParam::ProcArray { .. }
            ) {
                let param = function
                    .params
                    .get(param_index)
                    .map(String::as_str)
                    .unwrap_or("<missing>");
                errors.push(MirLoweringError::new(
                    format!(
                        "function '{}' parameter '{}' is not a resolved scalar, scalar tuple, primitive slice, buffer, data struct, data-struct array, or proc array",
                        function.name, param
                    ),
                    function_location(function),
                ));
            }
        }
        if function.params.len() != function.param_kinds.len() {
            errors.push(MirLoweringError::new(
                format!(
                    "function '{}' has {} parameter names but {} resolved parameter types",
                    function.name,
                    function.params.len(),
                    function.param_kinds.len()
                ),
                function_location(function),
            ));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let mut specializations = Vec::<(usize, CompileContext)>::new();
    for index in 0..program.defs.len() {
        let Some(mut function_contexts) = contexts.get(&index).cloned() else {
            continue;
        };
        function_contexts.sort_by_key(|context| (context.sample_rate_bits, context.block_size));
        specializations.extend(
            function_contexts
                .into_iter()
                .map(|context| (index, context)),
        );
    }

    let mut function_ids = HashMap::<FunctionKey, FunctionId>::new();
    for (offset, (function_index, context)) in specializations.iter().copied().enumerate() {
        function_ids.insert(
            FunctionKey {
                function_index,
                context,
            },
            FunctionId::new((function_base + offset) as u32),
        );
    }

    let mut types = mir.types.clone();
    let mut const_data = mir.const_data.clone();
    let mut const_arrays = HashMap::new();
    for array in &program.const_arrays {
        let len = u32::try_from(array.len).map_err(|_| {
            vec![MirLoweringError::new(
                format!("constant array '{}' length does not fit u32", array.name),
                SourceLoc::ZERO,
            )]
        })?;
        let values = array
            .values
            .iter()
            .copied()
            .map(mir_scalar)
            .collect::<Vec<_>>();
        let index = if let Some(index) = const_data
            .iter()
            .position(|candidate| candidate.name == array.name)
        {
            let existing = &const_data[index];
            if existing.element != scalar_type(array.elem_ty)
                || !scalar_values_exact_equal(&existing.values, &values)
            {
                return Err(vec![MirLoweringError::new(
                    format!(
                        "constant data '{}' already exists in MIR with different contents",
                        array.name
                    ),
                    SourceLoc::ZERO,
                )]);
            }
            index
        } else {
            let index = const_data.len();
            const_data.push(onda_mir::ConstData {
                name: array.name.clone(),
                element: scalar_type(array.elem_ty),
                values,
            });
            index
        };
        const_arrays.insert(
            array.name.clone(),
            (onda_mir::ConstDataId::new(index as u32), array.elem_ty, len),
        );
    }
    let mut source_files = mir.source_files.clone();
    let structs = program
        .structs
        .iter()
        .map(|structure| (structure.name.clone(), structure.fields.clone()))
        .collect::<HashMap<_, _>>();
    let mut functions = Vec::with_capacity(specializations.len());
    for (function_index, context) in &specializations {
        let function = &program.defs[*function_index];
        let function_context_count = contexts.get(function_index).map(Vec::len).unwrap_or(1);
        let emitted_name = if function_context_count > 1 {
            format!(
                "{}.__ctx_sr_{:08x}_bs_{:08x}",
                function.name, context.sample_rate_bits, context.block_size
            )
        } else {
            function.name.clone()
        };
        let lowerer = FunctionLowerer::new(
            function,
            &program.defs,
            &function_ids,
            &function_indices,
            &program.def_sample_oversample_factors,
            &program.proc_instance_oversample_factors,
            program.proc_step_oversample_meta.get(&function.name),
            &structs,
            &program.aggregate_layouts,
            &program.nested_proc_arrays,
            &const_arrays,
            mir.config,
            context.config(),
            emitted_name,
            &mut types,
            &mut source_files,
        );
        match lowerer.lower() {
            Ok(lowered) => functions.push(lowered),
            Err(error) => return Err(vec![error]),
        }
    }

    let ids = (0..specializations.len())
        .map(|index| FunctionId::new((function_base + index) as u32))
        .collect::<Vec<_>>();
    mir.types = types;
    mir.const_data = const_data;
    mir.source_files = source_files;
    mir.functions.extend(functions);
    Ok(ids)
}

/// Lowers an analyzed Onda program into the canonical optimized MIR consumed
/// by every backend.
///
/// The accepted executable boundary includes scalar and fixed primitive-array
/// interfaces/state/locals, primitive slices, constant data, external buffers,
/// scalar/fixed-array/dynamic-slice events, `init`, block/sample processing,
/// user functions with scalar, tuple, slice, and buffer parameters, and
/// canonical top-level/per-processor oversampling schedules. Structs, struct
/// arrays, and processor arrays reach this boundary only after semantic
/// normalization has flattened them into portable scalar/array/slice shapes.
pub fn lower_program_to_optimized_mir(
    program: &TypedProgram,
) -> Result<onda_mir::OptimizedProgram, Vec<MirLoweringError>> {
    let raw = lower_program_to_raw_mir(program)?;
    // SAFETY: MIR lowering owns the proof for every unchecked access it emits.
    // Array extents come from semantic types, process frames are validator-
    // tracked, and slice/buffer loops establish their bounds before emission.
    let validated = unsafe { onda_mir::validate_owned_with_producer_proofs(raw) }
        .map_err(mir_validation_errors)?;
    let (optimized, _) = onda_mir::optimize(validated).map_err(mir_validation_errors)?;
    Ok(optimized)
}

fn lower_program_to_raw_mir(
    program: &TypedProgram,
) -> Result<onda_mir::Program, Vec<MirLoweringError>> {
    let mut errors = mir_program_boundary_errors(program);
    let config = onda_mir::CompileConfig::from_usize(
        program.analysis_options.sample_rate,
        program.analysis_options.block_size,
    )
    .unwrap_or_else(|error| {
        errors.push(MirLoweringError::new(
            format!("invalid compile configuration: {error}"),
            SourceLoc::ZERO,
        ));
        onda_mir::CompileConfig::new(48_000.0, 1).expect("fallback config is valid")
    });
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut mir = onda_mir::Program::new(config, FunctionId::new(0), FunctionId::new(1));
    mir.functions.push(empty_entry_function(
        "onda_init",
        onda_mir::FunctionKind::Init,
    ));
    mir.functions.push(empty_entry_function(
        "onda_process",
        onda_mir::FunctionKind::Process,
    ));

    let mut globals = RuntimeGlobals::default();
    populate_interface(program, &mut mir, &mut globals)?;
    populate_state(program, &mut mir, &mut globals)?;
    populate_runtime_interface_views(program, &mut globals)?;
    populate_constant_data(program, &mut mir, &mut globals)?;

    lower_scalar_user_functions_to_mir(program, &mut mir)?;
    let (function_indices, function_ids) = runtime_function_ids(program, config, 2);

    let init_function = synthetic_runtime_function("onda_init", program.init.clone());
    let mut init = FunctionLowerer::new_runtime(
        &init_function,
        &program.defs,
        &function_ids,
        &function_indices,
        &program.def_sample_oversample_factors,
        &program.proc_instance_oversample_factors,
        config,
        config,
        "onda_init".to_owned(),
        &globals,
        &mut mir.types,
        &mut mir.source_files,
    )
    .with_prezeroed_init_state()
    .lower()
    .map_err(|error| vec![error])?;
    init.kind = onda_mir::FunctionKind::Init;

    let process_function = synthetic_runtime_function("onda_process", Vec::new());
    let process = FunctionLowerer::new_runtime(
        &process_function,
        &program.defs,
        &function_ids,
        &function_indices,
        &program.def_sample_oversample_factors,
        &program.proc_instance_oversample_factors,
        config,
        config,
        "onda_process".to_owned(),
        &globals,
        &mut mir.types,
        &mut mir.source_files,
    )
    .lower_process(
        &program.block_pre,
        &program.sample,
        &program.block_post,
        config.block_size,
        program.sample_oversample_factor,
    )
    .map_err(|error| vec![error])?;

    mir.functions[0] = init;
    mir.functions[1] = process;
    lower_events(
        program,
        &mut mir,
        &globals,
        &function_indices,
        &function_ids,
    )?;
    normalize_mir_source_paths(&mut mir);
    Ok(mir)
}

fn mir_validation_errors(
    validation_errors: Vec<onda_mir::ValidationError>,
) -> Vec<MirLoweringError> {
    validation_errors
        .into_iter()
        .map(|error| MirLoweringError::new(error.to_string(), SourceLoc::ZERO))
        .collect()
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct SourcePathParts {
    absolute: bool,
    volume: Option<String>,
    opaque: Option<String>,
    segments: Vec<String>,
    stdlib_anchor: Option<usize>,
}

fn normalize_mir_source_paths(program: &mut onda_mir::Program) {
    if program.source_files.is_empty() {
        return;
    }

    let parts = program
        .source_files
        .iter()
        .map(|source| split_source_path(&source.path))
        .collect::<Vec<_>>();
    let common_user_directory = common_absolute_user_source_directory(&parts);
    let normalized = parts
        .iter()
        .map(|path| stable_source_path(path, &common_user_directory))
        .collect::<Vec<_>>();

    let mut remap = Vec::with_capacity(normalized.len());
    let mut identities = HashMap::<SourcePathParts, SourceFileId>::new();
    let mut used_paths = HashSet::<String>::new();
    let mut source_files = Vec::new();
    for (identity, preferred_path) in parts.into_iter().zip(normalized) {
        let id = if let Some(id) = identities.get(&identity).copied() {
            id
        } else {
            let id = SourceFileId::new(source_files.len() as u32);
            let path = unique_source_path(preferred_path, &mut used_paths);
            identities.insert(identity, id);
            source_files.push(SourceFile { path });
            id
        };
        remap.push(id);
    }

    for function in &mut program.functions {
        remap_source_span(&mut function.source, &remap);
        remap_block_source_spans(&mut function.body, &remap);
    }
    program.source_files = source_files;
}

fn unique_source_path(preferred: String, used_paths: &mut HashSet<String>) -> String {
    if used_paths.insert(preferred.clone()) {
        return preferred;
    }
    for disambiguator in 2_u64.. {
        let candidate = format!("{preferred}~{disambiguator}");
        if used_paths.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("the source-path disambiguator space is unbounded")
}

fn split_source_path(path: &str) -> SourcePathParts {
    let replaced = path.replace('\\', "/");
    let path = replaced.strip_prefix("//?/").unwrap_or(&replaced);
    if path.contains("://") || (path.starts_with('<') && path.ends_with('>')) {
        return SourcePathParts {
            absolute: false,
            volume: None,
            opaque: Some(path.to_owned()),
            segments: Vec::new(),
            stdlib_anchor: None,
        };
    }

    let drive_absolute =
        path.as_bytes().get(1) == Some(&b':') && path.as_bytes().get(2) == Some(&b'/');
    let absolute = path.starts_with('/') || drive_absolute;
    let volume = drive_absolute.then(|| path[..2].to_ascii_lowercase());
    let path = if drive_absolute { &path[3..] } else { path };
    let mut segments = Vec::<String>::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|segment| segment != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..".to_owned());
                }
            }
            component => segments.push(component.to_owned()),
        }
    }
    let stdlib_anchor = segments
        .windows(2)
        .position(|window| window[0] == "stdlib" && window[1] == "std");
    SourcePathParts {
        absolute,
        volume,
        opaque: None,
        segments,
        stdlib_anchor,
    }
}

fn common_absolute_user_source_directory(paths: &[SourcePathParts]) -> Vec<String> {
    let mut paths = paths
        .iter()
        .filter(|path| path.absolute && path.stdlib_anchor.is_none() && !path.segments.is_empty())
        .map(|path| {
            (
                path.volume.as_deref(),
                &path.segments[..path.segments.len() - 1],
            )
        });
    let Some((volume, first)) = paths.next() else {
        return Vec::new();
    };
    let mut common = first.to_vec();
    for (directory_volume, directory) in paths {
        if directory_volume != volume {
            return Vec::new();
        }
        let shared = common
            .iter()
            .zip(directory.iter())
            .take_while(|(lhs, rhs)| lhs == rhs)
            .count();
        common.truncate(shared);
        if common.is_empty() {
            break;
        }
    }
    common
}

fn stable_source_path(path: &SourcePathParts, common_user_directory: &[String]) -> String {
    if let Some(opaque) = &path.opaque {
        return opaque.clone();
    }
    if let Some(anchor) = path.stdlib_anchor {
        return path.segments[anchor..].join("/");
    }
    if !path.absolute {
        return nonempty_source_path(path.segments.join("/"));
    }
    if path.segments.starts_with(common_user_directory) && !common_user_directory.is_empty() {
        return nonempty_source_path(path.segments[common_user_directory.len()..].join("/"));
    }

    let stable_anchor = path.segments.iter().position(|segment| {
        matches!(
            segment.as_str(),
            "src" | "lib" | "modules" | "examples" | "tests"
        )
    });
    let suffix = stable_anchor
        .map(|anchor| &path.segments[anchor..])
        .unwrap_or_else(|| {
            let start = path.segments.len().saturating_sub(2);
            &path.segments[start..]
        });
    nonempty_source_path(format!("external/{}", suffix.join("/")))
}

fn nonempty_source_path(path: String) -> String {
    if path.is_empty() {
        "<unknown>".to_owned()
    } else {
        path
    }
}

fn remap_source_span(source: &mut SourceSpan, remap: &[SourceFileId]) {
    let Some(file) = source.file else {
        return;
    };
    source.file = remap.get(file.index()).copied();
}

fn remap_block_source_spans(block: &mut MirBlock, remap: &[SourceFileId]) {
    for statement in &mut block.statements {
        remap_source_span(&mut statement.source, remap);
        match &mut statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                remap_block_source_spans(then_block, remap);
                remap_block_source_spans(else_block, remap);
            }
            StatementKind::Loop { body } => remap_block_source_spans(body, remap),
            _ => {}
        }
    }
}

fn mir_program_boundary_errors(program: &TypedProgram) -> Vec<MirLoweringError> {
    let mut errors = Vec::new();
    validate_flat_surface_arrays(
        "input",
        &program.ins,
        &program.in_types,
        &program.in_arrays,
        &mut errors,
    );
    validate_flat_surface_arrays(
        "output",
        &program.outs,
        &program.out_types,
        &program.out_arrays,
        &mut errors,
    );
    validate_flat_surface_arrays(
        "control output",
        &program.control_outs,
        &program.control_out_types,
        &program.control_out_arrays,
        &mut errors,
    );
    let param_names = program
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    validate_flat_surface_arrays(
        "parameter",
        &param_names,
        &program.param_types,
        &program.param_arrays,
        &mut errors,
    );
    for array in &program.array_vars {
        if array.len == 0 || array.len > i32::MAX as usize {
            errors.push(MirLoweringError::new(
                format!(
                    "state array '{}' length must be between 1 and i32::MAX for MIR indexing",
                    array.name
                ),
                SourceLoc::ZERO,
            ));
        }
    }
    for array in &program.const_arrays {
        if array.len == 0 || array.len > i32::MAX as usize {
            errors.push(MirLoweringError::new(
                format!(
                    "constant array '{}' length must be between 1 and i32::MAX for MIR indexing",
                    array.name
                ),
                SourceLoc::ZERO,
            ));
        }
        if array.values.len() != array.len {
            errors.push(MirLoweringError::new(
                format!(
                    "constant array '{}' declares {} elements but contains {} values",
                    array.name,
                    array.len,
                    array.values.len()
                ),
                SourceLoc::ZERO,
            ));
        }
        if array
            .values
            .iter()
            .any(|value| mir_scalar(*value).ty() != scalar_type(array.elem_ty))
        {
            errors.push(MirLoweringError::new(
                format!(
                    "constant array '{}' contains a value that does not match element type {}",
                    array.name,
                    array.elem_ty.name()
                ),
                SourceLoc::ZERO,
            ));
        }
    }
    for buffer in &program.buffers {
        if let TypedBufferChannels::Static(channels) = &buffer.channels {
            let maximum = crate::builtins::max_buffer_static_channels(buffer.elem_ty);
            if *channels == 0 || *channels > maximum {
                errors.push(MirLoweringError::new(
                    format!(
                        "buffer '{}' static channel count must be between 1 and {maximum} for {} elements",
                        buffer.name,
                        buffer.elem_ty.name(),
                    ),
                    SourceLoc::ZERO,
                ));
            }
        }
    }
    for event in &program.events {
        for param in &event.params {
            match (&param.ty, &param.default) {
                (TypedEventParamType::Scalar(ty), Some(TypedEventParamDefault::Scalar(value)))
                    if mir_scalar(*value).ty() != scalar_type(*ty) =>
                {
                    errors.push(MirLoweringError::new(
                        format!(
                            "event '{}' parameter '{}' default does not match type {}",
                            event.name,
                            param.name,
                            ty.name()
                        ),
                        event_location(event),
                    ));
                }
                (TypedEventParamType::Scalar(_), Some(TypedEventParamDefault::Array(_))) => {
                    errors.push(MirLoweringError::new(
                        format!(
                            "event '{}' scalar parameter '{}' has an aggregate default",
                            event.name, param.name
                        ),
                        event_location(event),
                    ));
                }
                (
                    TypedEventParamType::Array { elem, len },
                    Some(TypedEventParamDefault::Array(values)),
                ) => {
                    if *len == 0 || *len > i32::MAX as usize {
                        errors.push(MirLoweringError::new(
                            format!(
                                "event '{}' array parameter '{}' length must be between 1 and i32::MAX",
                                event.name, param.name
                            ),
                            event_location(event),
                        ));
                    }
                    if values.len() != *len
                        || values
                            .iter()
                            .any(|value| mir_scalar(*value).ty() != scalar_type(*elem))
                    {
                        errors.push(MirLoweringError::new(
                            format!(
                                "event '{}' array parameter '{}' default does not match {}[{}]",
                                event.name,
                                param.name,
                                elem.name(),
                                len
                            ),
                            event_location(event),
                        ));
                    }
                }
                (TypedEventParamType::Array { len, .. }, default) => {
                    if *len == 0 || *len > i32::MAX as usize {
                        errors.push(MirLoweringError::new(
                            format!(
                                "event '{}' array parameter '{}' length must be between 1 and i32::MAX",
                                event.name, param.name
                            ),
                            event_location(event),
                        ));
                    }
                    if matches!(default, Some(TypedEventParamDefault::Scalar(_))) {
                        errors.push(MirLoweringError::new(
                            format!(
                                "event '{}' array parameter '{}' has a scalar default",
                                event.name, param.name
                            ),
                            event_location(event),
                        ));
                    }
                }
                (TypedEventParamType::Slice { .. }, Some(_)) => {
                    errors.push(MirLoweringError::new(
                        format!(
                            "event '{}' slice parameter '{}' cannot have a default",
                            event.name, param.name
                        ),
                        event_location(event),
                    ));
                }
                (TypedEventParamType::Slice { .. }, None) => {}
                (TypedEventParamType::Scalar(_), _) => {}
            }
        }
    }
    let top_level_factor = program.sample_oversample_factor.max(1);
    if !top_level_factor.is_power_of_two() {
        errors.push(MirLoweringError::new(
            format!("top-level oversampling factor {top_level_factor} is not a power of two"),
            SourceLoc::ZERO,
        ));
    }
    let mut proc_steps = program.proc_step_oversample_meta.keys().collect::<Vec<_>>();
    proc_steps.sort();
    for name in proc_steps {
        let factor = program
            .def_sample_oversample_factors
            .get(name)
            .copied()
            .unwrap_or(1);
        if factor <= 1 || !factor.is_power_of_two() {
            errors.push(MirLoweringError::new(
                format!(
                    "processor step '{name}' has oversampling filter metadata but invalid factor {factor}"
                ),
                SourceLoc::ZERO,
            ));
        }
    }
    if program.state_vars.len() != program.state_types.len() {
        errors.push(MirLoweringError::new(
            format!(
                "semantic state table has {} names but {} scalar types",
                program.state_vars.len(),
                program.state_types.len()
            ),
            SourceLoc::ZERO,
        ));
    }
    errors
}

fn validate_flat_surface_arrays(
    kind: &str,
    names: &[String],
    types: &HashMap<String, PrimitiveType>,
    arrays: &HashMap<String, TypedArrayInfo>,
    errors: &mut Vec<MirLoweringError>,
) {
    let mut claimed = vec![false; names.len()];
    for (name, info) in arrays {
        if info.len == 0 || info.len > i32::MAX as usize {
            errors.push(MirLoweringError::new(
                format!(
                    "{kind} array '{name}' length must be between 1 and i32::MAX for MIR indexing"
                ),
                SourceLoc::ZERO,
            ));
            continue;
        }
        let Some(end) = info.offset.checked_add(info.len) else {
            errors.push(MirLoweringError::new(
                format!("{kind} array '{name}' flattened range overflows usize"),
                SourceLoc::ZERO,
            ));
            continue;
        };
        if end > names.len() {
            errors.push(MirLoweringError::new(
                format!(
                    "{kind} array '{name}' flattened range {}..{end} exceeds {} slots",
                    info.offset,
                    names.len()
                ),
                SourceLoc::ZERO,
            ));
            continue;
        }
        for index in 0..info.len {
            let slot = info.offset + index;
            let expected_name = format!("{name}[{index}]");
            if names[slot] != expected_name {
                errors.push(MirLoweringError::new(
                    format!(
                        "{kind} array '{name}' expected flattened slot '{expected_name}' at offset {slot}, got '{}'",
                        names[slot]
                    ),
                    SourceLoc::ZERO,
                ));
            }
            if types.get(&names[slot]).copied() != Some(info.elem_ty) {
                errors.push(MirLoweringError::new(
                    format!(
                        "{kind} array '{name}' slot '{expected_name}' does not have element type {}",
                        info.elem_ty.name()
                    ),
                    SourceLoc::ZERO,
                ));
            }
            if std::mem::replace(&mut claimed[slot], true) {
                errors.push(MirLoweringError::new(
                    format!("{kind} array '{name}' overlaps another flattened array"),
                    SourceLoc::ZERO,
                ));
            }
        }
    }
}

fn populate_interface(
    program: &TypedProgram,
    mir: &mut onda_mir::Program,
    globals: &mut RuntimeGlobals,
) -> Result<(), Vec<MirLoweringError>> {
    let mut errors = Vec::new();
    let mut index = 0;
    while index < program.ins.len() {
        if let Some((name, info)) = array_at_offset(&program.in_arrays, index) {
            let id = onda_mir::InputId::new(mir.interface.inputs.len() as u32);
            let len = info.len as u32;
            let type_id = intern_array_type(&mut mir.types, info.elem_ty, len);
            let mut defaults = Vec::with_capacity(info.len);
            for slot in &program.ins[index..index + info.len] {
                let Some(default) = program.in_defaults.get(slot).copied() else {
                    errors.push(MirLoweringError::new(
                        format!("input array '{name}' slot '{slot}' has no default"),
                        SourceLoc::ZERO,
                    ));
                    continue;
                };
                if program.in_ranges.contains_key(slot) {
                    errors.push(MirLoweringError::new(
                        format!("input array '{name}' slot '{slot}' unexpectedly has a range"),
                        SourceLoc::ZERO,
                    ));
                }
                defaults.push(mir_constant(default));
            }
            let default = (defaults.len() == info.len)
                .then_some(onda_mir::ConstantValue::Aggregate(defaults));
            mir.interface.inputs.push(onda_mir::Input {
                name: name.clone(),
                ty: type_id,
                default,
                range: None,
            });
            globals
                .input_arrays
                .insert(name.clone(), (id, info.elem_ty, len));
            index += info.len;
            continue;
        }
        let name = &program.ins[index];
        let Some(ty) = program.in_types.get(name).copied() else {
            errors.push(missing_interface_type("input", name));
            index += 1;
            continue;
        };
        let id = onda_mir::InputId::new(mir.interface.inputs.len() as u32);
        let type_id = intern_scalar_type(&mut mir.types, ty);
        mir.interface.inputs.push(onda_mir::Input {
            name: name.clone(),
            ty: type_id,
            default: program.in_defaults.get(name).copied().map(mir_constant),
            range: program.in_ranges.get(name).copied().map(mir_range),
        });
        globals.inputs.insert(name.clone(), (id, ty));
        index += 1;
    }
    index = 0;
    while index < program.outs.len() {
        if let Some((name, info)) = array_at_offset(&program.out_arrays, index) {
            let id = onda_mir::OutputId::new(mir.interface.outputs.len() as u32);
            let len = info.len as u32;
            let type_id = intern_array_type(&mut mir.types, info.elem_ty, len);
            mir.interface.outputs.push(onda_mir::Output {
                name: name.clone(),
                ty: type_id,
            });
            globals
                .output_arrays
                .insert(name.clone(), (id, info.elem_ty, len));
            index += info.len;
            continue;
        }
        let name = &program.outs[index];
        let Some(ty) = program.out_types.get(name).copied() else {
            errors.push(missing_interface_type("output", name));
            index += 1;
            continue;
        };
        let id = onda_mir::OutputId::new(mir.interface.outputs.len() as u32);
        let type_id = intern_scalar_type(&mut mir.types, ty);
        mir.interface.outputs.push(onda_mir::Output {
            name: name.clone(),
            ty: type_id,
        });
        globals.outputs.insert(name.clone(), (id, ty));
        index += 1;
    }
    index = 0;
    while index < program.control_outs.len() {
        if let Some((name, info)) = array_at_offset(&program.control_out_arrays, index) {
            let id = onda_mir::ControlOutputId::new(mir.interface.control_outputs.len() as u32);
            let len = info.len as u32;
            let type_id = intern_array_type(&mut mir.types, info.elem_ty, len);
            let mirror = onda_mir::StateId::new(mir.state.len() as u32);
            mir.state.push(onda_mir::StateSlot {
                name: name.clone(),
                ty: type_id,
                persistence: onda_mir::StatePersistence::ControlMirror,
            });
            mir.interface.control_outputs.push(onda_mir::ControlOutput {
                name: name.clone(),
                ty: type_id,
                mirror,
            });
            globals
                .state_arrays
                .insert(name.clone(), (mirror, info.elem_ty, len));
            globals
                .control_output_arrays
                .insert(name.clone(), (id, info.elem_ty, len));
            index += info.len;
            continue;
        }
        let name = &program.control_outs[index];
        let Some(ty) = program.control_out_types.get(name).copied() else {
            errors.push(missing_interface_type("control output", name));
            index += 1;
            continue;
        };
        let id = onda_mir::ControlOutputId::new(mir.interface.control_outputs.len() as u32);
        let type_id = intern_scalar_type(&mut mir.types, ty);
        let mirror = onda_mir::StateId::new(mir.state.len() as u32);
        mir.state.push(onda_mir::StateSlot {
            name: name.clone(),
            ty: type_id,
            persistence: onda_mir::StatePersistence::ControlMirror,
        });
        mir.interface.control_outputs.push(onda_mir::ControlOutput {
            name: name.clone(),
            ty: type_id,
            mirror,
        });
        globals.states.insert(name.clone(), (mirror, ty));
        globals.control_outputs.insert(name.clone(), (id, ty));
        index += 1;
    }
    index = 0;
    while index < program.params.len() {
        if let Some((name, info)) = array_at_offset(&program.param_arrays, index) {
            let id = onda_mir::ParamId::new(mir.interface.params.len() as u32);
            let len = info.len as u32;
            let type_id = intern_array_type(&mut mir.types, info.elem_ty, len);
            let defaults = program.params[index..index + info.len]
                .iter()
                .map(|param| mir_constant(param.default))
                .collect::<Vec<_>>();
            if program.params[index..index + info.len]
                .iter()
                .any(|param| param.range.is_some())
            {
                errors.push(MirLoweringError::new(
                    format!("parameter array '{name}' unexpectedly has a scalar range"),
                    SourceLoc::ZERO,
                ));
            }
            mir.interface.params.push(onda_mir::Param {
                name: name.clone(),
                ty: type_id,
                default: onda_mir::ConstantValue::Aggregate(defaults),
                range: None,
                control: onda_mir::ParamControl::default(),
            });
            globals
                .param_arrays
                .insert(name.clone(), (id, info.elem_ty, len));
            index += info.len;
            continue;
        }
        let param = &program.params[index];
        let id = onda_mir::ParamId::new(mir.interface.params.len() as u32);
        let type_id = intern_scalar_type(&mut mir.types, param.ty);
        mir.interface.params.push(onda_mir::Param {
            name: param.name.clone(),
            ty: type_id,
            default: mir_constant(param.default),
            range: param.range.map(mir_range),
            control: mir_param_control(&param.control),
        });
        globals.params.insert(param.name.clone(), (id, param.ty));
        index += 1;
    }
    for buffer in &program.buffers {
        let channels = match &buffer.channels {
            TypedBufferChannels::Mono => onda_mir::BufferChannels::Mono,
            TypedBufferChannels::Static(channels) => {
                let Ok(channels) = u32::try_from(*channels) else {
                    errors.push(MirLoweringError::new(
                        format!("buffer '{}' channel count does not fit u32", buffer.name),
                        SourceLoc::ZERO,
                    ));
                    continue;
                };
                onda_mir::BufferChannels::Static(channels)
            }
            TypedBufferChannels::Dynamic => onda_mir::BufferChannels::Dynamic,
        };
        let Ok(array_len) = u32::try_from(buffer.array_len) else {
            errors.push(MirLoweringError::new(
                format!("buffer array '{}' length does not fit u32", buffer.name),
                SourceLoc::ZERO,
            ));
            continue;
        };
        let first = onda_mir::BufferId::new(mir.interface.buffers.len() as u32);
        for index in 0..array_len {
            mir.interface.buffers.push(onda_mir::Buffer {
                name: if buffer.is_array {
                    format!("{}[{index}]", buffer.name)
                } else {
                    buffer.name.clone()
                },
                element: scalar_type(buffer.elem_ty),
                channels,
                access: onda_mir::AccessMode::ReadWrite,
            });
        }
        if !buffer.is_array {
            globals
                .buffers
                .insert(buffer.name.clone(), (first, buffer.elem_ty));
        } else {
            mir.interface.buffer_arrays.push(onda_mir::BufferArray {
                name: buffer.name.clone(),
                first,
                len: array_len,
            });
            globals
                .buffer_arrays
                .insert(buffer.name.clone(), (first, buffer.elem_ty, array_len));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn populate_runtime_interface_views(
    program: &TypedProgram,
    globals: &mut RuntimeGlobals,
) -> Result<(), Vec<MirLoweringError>> {
    let mut errors = Vec::new();
    for (kind, view) in [
        (
            DynamicInterfaceKind::Inputs,
            program.interface_views.inputs.as_ref(),
        ),
        (
            DynamicInterfaceKind::AudioOutputs,
            program.interface_views.audio_outputs.as_ref(),
        ),
        (
            DynamicInterfaceKind::ControlOutputs,
            program.interface_views.control_outputs.as_ref(),
        ),
        (
            DynamicInterfaceKind::Params,
            program.interface_views.params.as_ref(),
        ),
    ] {
        let Some(view) = view else {
            continue;
        };
        match resolve_runtime_interface_view(program, globals, kind, view) {
            Ok(view) => {
                globals.interface_views.insert(kind, view);
            }
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn resolve_runtime_interface_view(
    program: &TypedProgram,
    globals: &RuntimeGlobals,
    kind: DynamicInterfaceKind,
    view: &ResolvedInterfaceView,
) -> Result<RuntimeInterfaceView, MirLoweringError> {
    let kind_name = match kind {
        DynamicInterfaceKind::Inputs => "input",
        DynamicInterfaceKind::AudioOutputs => "audio output",
        DynamicInterfaceKind::ControlOutputs => "control output",
        DynamicInterfaceKind::Params => "parameter",
    };
    if view.slots.is_empty() || view.slots.len() > i32::MAX as usize {
        return Err(MirLoweringError::new(
            format!("resolved {kind_name} interface must contain between 1 and i32::MAX slots"),
            SourceLoc::ZERO,
        ));
    }
    let mut endpoints = Vec::with_capacity(view.slots.len());
    for (expected_id, slot) in view.slots.iter().enumerate() {
        if slot.id.index() != expected_id {
            return Err(MirLoweringError::new(
                format!(
                    "resolved {kind_name} interface slot '{}' has ID {}, expected {expected_id}",
                    slot.root,
                    slot.id.raw()
                ),
                SourceLoc::ZERO,
            ));
        }
        let (endpoint, actual_type) =
            resolve_runtime_interface_endpoint(program, globals, kind, slot)?;
        if actual_type != view.element_type {
            return Err(MirLoweringError::new(
                format!(
                    "resolved {kind_name} interface slot '{}' has type {}, expected {}",
                    slot.root,
                    actual_type.name(),
                    view.element_type.name()
                ),
                SourceLoc::ZERO,
            ));
        }
        endpoints.push(endpoint);
    }
    Ok(RuntimeInterfaceView {
        element_type: view.element_type,
        slots: endpoints,
    })
}

fn resolve_runtime_interface_endpoint(
    program: &TypedProgram,
    globals: &RuntimeGlobals,
    kind: DynamicInterfaceKind,
    slot: &ResolvedInterfaceSlot,
) -> Result<(RuntimeInterfaceEndpoint, PrimitiveType), MirLoweringError> {
    let missing = || {
        MirLoweringError::new(
            format!(
                "resolved dynamic interface slot '{}' does not name a concrete MIR endpoint",
                slot.root
            ),
            SourceLoc::ZERO,
        )
    };
    let checked_element = |element: usize, len: u32| {
        let element = u32::try_from(element).map_err(|_| missing())?;
        (element < len).then_some(element).ok_or_else(missing)
    };

    match (kind, slot.element) {
        (DynamicInterfaceKind::Inputs, None) => {
            let (input, ty) = globals
                .inputs
                .get(&slot.root)
                .copied()
                .ok_or_else(missing)?;
            let clamped = program.dynamic_input_range_aliases.get(&slot.root).cloned();
            Ok((
                RuntimeInterfaceEndpoint::Input {
                    input,
                    element: None,
                    clamped,
                },
                ty,
            ))
        }
        (DynamicInterfaceKind::Inputs, Some(element)) => {
            let (input, ty, len) = globals
                .input_arrays
                .get(&slot.root)
                .copied()
                .ok_or_else(missing)?;
            Ok((
                RuntimeInterfaceEndpoint::Input {
                    input,
                    element: Some(checked_element(element, len)?),
                    clamped: None,
                },
                ty,
            ))
        }
        (DynamicInterfaceKind::AudioOutputs, None) => {
            let (output, ty) = globals
                .outputs
                .get(&slot.root)
                .copied()
                .ok_or_else(missing)?;
            Ok((
                RuntimeInterfaceEndpoint::AudioOutput {
                    output,
                    element: None,
                },
                ty,
            ))
        }
        (DynamicInterfaceKind::AudioOutputs, Some(element)) => {
            let (output, ty, len) = globals
                .output_arrays
                .get(&slot.root)
                .copied()
                .ok_or_else(missing)?;
            Ok((
                RuntimeInterfaceEndpoint::AudioOutput {
                    output,
                    element: Some(checked_element(element, len)?),
                },
                ty,
            ))
        }
        (DynamicInterfaceKind::ControlOutputs, None) => {
            let (output, ty) = globals
                .control_outputs
                .get(&slot.root)
                .copied()
                .ok_or_else(missing)?;
            Ok((
                RuntimeInterfaceEndpoint::ControlOutput {
                    output,
                    element: None,
                },
                ty,
            ))
        }
        (DynamicInterfaceKind::ControlOutputs, Some(element)) => {
            let (output, ty, len) = globals
                .control_output_arrays
                .get(&slot.root)
                .copied()
                .ok_or_else(missing)?;
            let element = checked_element(element, len)?;
            Ok((
                RuntimeInterfaceEndpoint::ControlOutput {
                    output,
                    element: Some(element),
                },
                ty,
            ))
        }
        (DynamicInterfaceKind::Params, None) => {
            let (param, ty) = globals
                .params
                .get(&slot.root)
                .copied()
                .ok_or_else(missing)?;
            let clamped = program
                .dynamic_param_range_aliases
                .get(&slot.root)
                .map(|alias| {
                    globals
                        .states
                        .get(alias)
                        .map(|(state, _)| *state)
                        .ok_or_else(missing)
                })
                .transpose()?;
            Ok((
                RuntimeInterfaceEndpoint::Param {
                    param,
                    element: None,
                    clamped,
                },
                ty,
            ))
        }
        (DynamicInterfaceKind::Params, Some(element)) => {
            let (param, ty, len) = globals
                .param_arrays
                .get(&slot.root)
                .copied()
                .ok_or_else(missing)?;
            Ok((
                RuntimeInterfaceEndpoint::Param {
                    param,
                    element: Some(checked_element(element, len)?),
                    clamped: None,
                },
                ty,
            ))
        }
    }
}

fn array_at_offset(
    arrays: &HashMap<String, TypedArrayInfo>,
    offset: usize,
) -> Option<(&String, &TypedArrayInfo)> {
    arrays.iter().find(|(_, info)| info.offset == offset)
}

fn collect_runtime_struct_roots(
    statements: &[Stmt],
    layouts: &AggregateLayoutTable,
    roots: &mut HashMap<String, String>,
) {
    for statement in statements {
        match statement {
            Stmt::Assign {
                target: AssignTarget::Var(target),
                expr: Expr::UserCall { name, .. },
                ..
            } if layouts.layout_for_struct(name).is_some() => {
                roots.insert(target.clone(), name.clone());
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_runtime_struct_roots(then_branch, layouts, roots);
                collect_runtime_struct_roots(else_branch, layouts, roots);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                collect_runtime_struct_roots(body, layouts, roots);
            }
            _ => {}
        }
    }
}

fn populate_state(
    program: &TypedProgram,
    mir: &mut onda_mir::Program,
    globals: &mut RuntimeGlobals,
) -> Result<(), Vec<MirLoweringError>> {
    let proc_array_types = program
        .defs
        .iter()
        .flat_map(|function| function.param_kinds.iter())
        .filter_map(|kind| match kind {
            TypedFnParam::ProcArray { proc_name, .. } => Some(proc_name.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let proc_array_active_slots = program
        .array_struct_roots
        .iter()
        .filter(|root| proc_array_types.contains(root.struct_name.as_str()))
        .map(|root| (runtime_proc_array_active_symbol(&root.name), root.len))
        .collect::<HashMap<_, _>>();
    let all_internal_active_names = program
        .array_struct_roots
        .iter()
        .map(|root| runtime_proc_array_active_symbol(&root.name))
        .collect::<HashSet<_>>();
    globals.structs.extend(
        program
            .structs
            .iter()
            .map(|structure| (structure.name.clone(), structure.fields.clone())),
    );
    globals.aggregate_layouts = program.aggregate_layouts.clone();
    globals.nested_proc_arrays = program.nested_proc_arrays.clone();
    collect_runtime_struct_roots(
        &program.init,
        &globals.aggregate_layouts,
        &mut globals.struct_roots,
    );
    for root in &program.array_struct_roots {
        let len = u32::try_from(root.len).map_err(|_| {
            vec![MirLoweringError::new(
                format!(
                    "array-of-struct state '{}' length does not fit u32",
                    root.name
                ),
                SourceLoc::ZERO,
            )]
        })?;
        globals
            .array_struct_roots
            .insert(root.name.clone(), (root.struct_name.clone(), len));
    }
    if program.state_vars.len() != program.state_types.len() {
        return Err(vec![MirLoweringError::new(
            "semantic state names and types are inconsistent",
            SourceLoc::ZERO,
        )]);
    }
    for (name, ty) in program
        .state_vars
        .iter()
        .zip(program.state_types.iter().copied())
    {
        if let Some((state, existing_ty)) = globals.states.get(name).copied() {
            if existing_ty != ty
                || mir.state[state.index()].persistence != onda_mir::StatePersistence::ControlMirror
            {
                return Err(vec![MirLoweringError::new(
                    format!(
                        "control output mirror state '{name}' has type {}, expected {}",
                        existing_ty.name(),
                        ty.name()
                    ),
                    SourceLoc::ZERO,
                )]);
            }
            continue;
        }
        let id = onda_mir::StateId::new(mir.state.len() as u32);
        let type_id = intern_scalar_type(&mut mir.types, ty);
        mir.state.push(onda_mir::StateSlot {
            name: name.clone(),
            ty: type_id,
            persistence: onda_mir::StatePersistence::Snapshot,
        });
        globals.states.insert(name.clone(), (id, ty));
    }
    for (name, types) in &program.state_tuples {
        let mut components = Vec::with_capacity(types.len());
        for (index, expected_ty) in types.iter().copied().enumerate() {
            let flat_name = format!("{name}.__{index}");
            let Some((state, actual_ty)) = globals.states.get(&flat_name).copied() else {
                return Err(vec![MirLoweringError::new(
                    format!(
                        "tuple state '{name}' component {index} has no flattened scalar state slot"
                    ),
                    SourceLoc::ZERO,
                )]);
            };
            if actual_ty != expected_ty {
                return Err(vec![MirLoweringError::new(
                    format!(
                        "tuple state '{name}' component {index} type mismatch: expected {}, got {}",
                        expected_ty.name(),
                        actual_ty.name()
                    ),
                    SourceLoc::ZERO,
                )]);
            }
            components.push((state, actual_ty));
        }
        globals.state_tuples.insert(name.clone(), components);
    }
    for array in &program.array_vars {
        let len = u32::try_from(array.len).map_err(|_| {
            vec![MirLoweringError::new(
                format!("state array '{}' length does not fit u32", array.name),
                SourceLoc::ZERO,
            )]
        })?;
        if let Some((state, element, existing_len)) = globals.state_arrays.get(&array.name).copied()
        {
            if element != array.elem_ty
                || existing_len != len
                || mir.state[state.index()].persistence != onda_mir::StatePersistence::ControlMirror
            {
                return Err(vec![MirLoweringError::new(
                    format!(
                        "control output mirror state '{}' does not match {}[{len}]",
                        array.name,
                        array.elem_ty.name()
                    ),
                    SourceLoc::ZERO,
                )]);
            }
            continue;
        }
        let id = onda_mir::StateId::new(mir.state.len() as u32);
        let type_id = intern_array_type(&mut mir.types, array.elem_ty, len);
        let persistence = if all_internal_active_names.contains(&array.name) {
            onda_mir::StatePersistence::InstanceScratch
        } else {
            onda_mir::StatePersistence::Snapshot
        };
        mir.state.push(onda_mir::StateSlot {
            name: array.name.clone(),
            ty: type_id,
            persistence,
        });
        globals
            .state_arrays
            .insert(array.name.clone(), (id, array.elem_ty, len));
    }
    for (active_name, expected_len) in proc_array_active_slots {
        let expected_len = u32::try_from(expected_len).map_err(|_| {
            vec![MirLoweringError::new(
                format!("proc-array active state '{active_name}' length does not fit u32"),
                SourceLoc::ZERO,
            )]
        })?;
        if let Some((_, element, actual_len)) = globals.state_arrays.get(&active_name).copied() {
            if element != PrimitiveType::Bool || actual_len != expected_len {
                return Err(vec![MirLoweringError::new(
                    format!("proc-array active state '{active_name}' must be bool[{expected_len}]"),
                    SourceLoc::ZERO,
                )]);
            }
            continue;
        }
        let id = onda_mir::StateId::new(mir.state.len() as u32);
        let type_id = intern_array_type(&mut mir.types, PrimitiveType::Bool, expected_len);
        mir.state.push(onda_mir::StateSlot {
            name: active_name.clone(),
            ty: type_id,
            persistence: onda_mir::StatePersistence::InstanceScratch,
        });
        globals
            .state_arrays
            .insert(active_name, (id, PrimitiveType::Bool, expected_len));
    }
    populate_top_level_oversampling_state(program, mir, globals);
    Ok(())
}

fn populate_top_level_oversampling_state(
    program: &TypedProgram,
    mir: &mut onda_mir::Program,
    globals: &mut RuntimeGlobals,
) {
    let factor = program.sample_oversample_factor.max(1);
    if factor <= 1 {
        return;
    }
    let stage_count = factor.trailing_zeros() as usize;

    let mut input_names = globals.inputs.keys().cloned().collect::<Vec<_>>();
    input_names.sort();
    for name in input_names {
        let (_, ty) = globals.inputs[&name];
        if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
            let stages = append_mir_sinc_stages(mir, "input", &name, ty, stage_count);
            globals.top_level_oversampling.inputs.insert(name, stages);
        }
    }
    let mut input_arrays = globals
        .input_arrays
        .iter()
        .map(|(name, (_, ty, len))| (name.clone(), *ty, *len))
        .collect::<Vec<_>>();
    input_arrays.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
    for (name, ty, len) in input_arrays {
        if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
            for element in 0..len {
                let surface = format!("{name}[{element}]");
                let stages = append_mir_sinc_stages(mir, "input", &surface, ty, stage_count);
                globals
                    .top_level_oversampling
                    .inputs
                    .insert(surface, stages);
            }
        }
    }

    let mut output_names = globals.outputs.keys().cloned().collect::<Vec<_>>();
    output_names.sort();
    for name in output_names {
        let (_, ty) = globals.outputs[&name];
        if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
            let stages = append_mir_sinc_stages(mir, "output", &name, ty, stage_count);
            globals.top_level_oversampling.outputs.insert(name, stages);
        }
    }
    let mut output_arrays = globals
        .output_arrays
        .iter()
        .map(|(name, (_, ty, len))| (name.clone(), *ty, *len))
        .collect::<Vec<_>>();
    output_arrays.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
    for (name, ty, len) in output_arrays {
        if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
            for element in 0..len {
                let surface = format!("{name}[{element}]");
                let stages = append_mir_sinc_stages(mir, "output", &surface, ty, stage_count);
                globals
                    .top_level_oversampling
                    .outputs
                    .insert(surface, stages);
            }
        }
    }
}

fn append_mir_sinc_stages(
    mir: &mut onda_mir::Program,
    direction: &str,
    surface: &str,
    ty: PrimitiveType,
    stage_count: usize,
) -> Vec<MirSincStageState> {
    (0..stage_count)
        .map(|stage| {
            let taps = std::array::from_fn(|tap| {
                let id = onda_mir::StateId::new(mir.state.len() as u32);
                mir.state.push(onda_mir::StateSlot {
                    name: format!(
                        "$oversample.{direction}.{surface}.stage{stage}.{}",
                        SINC_TAP_NAMES[tap]
                    ),
                    ty: intern_scalar_type(&mut mir.types, ty),
                    persistence: onda_mir::StatePersistence::Snapshot,
                });
                id
            });
            MirSincStageState { ty, taps }
        })
        .collect()
}

fn populate_constant_data(
    program: &TypedProgram,
    mir: &mut onda_mir::Program,
    globals: &mut RuntimeGlobals,
) -> Result<(), Vec<MirLoweringError>> {
    for array in &program.const_arrays {
        let len = u32::try_from(array.len).map_err(|_| {
            vec![MirLoweringError::new(
                format!("constant array '{}' length does not fit u32", array.name),
                SourceLoc::ZERO,
            )]
        })?;
        let id = onda_mir::ConstDataId::new(mir.const_data.len() as u32);
        mir.const_data.push(onda_mir::ConstData {
            name: array.name.clone(),
            element: scalar_type(array.elem_ty),
            values: array.values.iter().copied().map(mir_scalar).collect(),
        });
        globals
            .const_arrays
            .insert(array.name.clone(), (id, array.elem_ty, len));
    }
    Ok(())
}

fn lower_events(
    program: &TypedProgram,
    mir: &mut onda_mir::Program,
    globals: &RuntimeGlobals,
    function_indices: &HashMap<String, usize>,
    function_ids: &HashMap<FunctionKey, FunctionId>,
) -> Result<(), Vec<MirLoweringError>> {
    for event in &program.events {
        let event_id = onda_mir::EventId::new(mir.interface.events.len() as u32);
        let handler = FunctionId::new(mir.functions.len() as u32);
        let mut params = Vec::with_capacity(event.params.len());
        for param in &event.params {
            let (type_id, default) = match &param.ty {
                TypedEventParamType::Scalar(ty) => {
                    let default = match &param.default {
                        Some(TypedEventParamDefault::Scalar(value)) => Some(mir_constant(*value)),
                        Some(TypedEventParamDefault::Array(_)) => {
                            return Err(vec![MirLoweringError::new(
                                format!(
                                    "event '{}' scalar parameter '{}' has an aggregate default",
                                    event.name, param.name
                                ),
                                event_location(event),
                            )]);
                        }
                        None => None,
                    };
                    (intern_scalar_type(&mut mir.types, *ty), default)
                }
                TypedEventParamType::Array { elem, len } => {
                    let len = u32::try_from(*len).map_err(|_| {
                        vec![MirLoweringError::new(
                            format!(
                                "event '{}' array parameter '{}' length does not fit u32",
                                event.name, param.name
                            ),
                            event_location(event),
                        )]
                    })?;
                    let default = match &param.default {
                        Some(TypedEventParamDefault::Array(values)) => {
                            Some(onda_mir::ConstantValue::Aggregate(
                                values.iter().copied().map(mir_constant).collect(),
                            ))
                        }
                        Some(TypedEventParamDefault::Scalar(_)) => {
                            return Err(vec![MirLoweringError::new(
                                format!(
                                    "event '{}' array parameter '{}' has a scalar default",
                                    event.name, param.name
                                ),
                                event_location(event),
                            )]);
                        }
                        None => None,
                    };
                    (intern_array_type(&mut mir.types, *elem, len), default)
                }
                TypedEventParamType::Slice { elem } => {
                    if param.default.is_some() {
                        return Err(vec![MirLoweringError::new(
                            format!(
                                "event '{}' slice parameter '{}' cannot have a default",
                                event.name, param.name
                            ),
                            event_location(event),
                        )]);
                    }
                    (
                        intern_slice_type(&mut mir.types, *elem, onda_mir::AccessMode::ReadOnly),
                        None,
                    )
                }
            };
            params.push(onda_mir::EventParam {
                name: param.name.clone(),
                ty: type_id,
                default,
            });
        }
        mir.interface.events.push(onda_mir::Event {
            name: event.name.clone(),
            params,
            handler,
        });

        let function_name = format!("onda_event::{}", event.name);
        let synthetic = synthetic_runtime_function(&function_name, event.body.clone());
        let mut lowerer = FunctionLowerer::new_runtime(
            &synthetic,
            &program.defs,
            function_ids,
            function_indices,
            &program.def_sample_oversample_factors,
            &program.proc_instance_oversample_factors,
            mir.config,
            mir.config,
            function_name,
            globals,
            &mut mir.types,
            &mut mir.source_files,
        );
        lowerer
            .bind_event_params(event)
            .map_err(|error| vec![error])?;
        let mut lowered = lowerer.lower().map_err(|error| vec![error])?;
        lowered.kind = onda_mir::FunctionKind::Event(event_id);
        mir.functions.push(lowered);
    }
    Ok(())
}

fn runtime_function_ids(
    program: &TypedProgram,
    config: onda_mir::CompileConfig,
    function_base: usize,
) -> (HashMap<String, usize>, HashMap<FunctionKey, FunctionId>) {
    let function_indices = program
        .defs
        .iter()
        .enumerate()
        .map(|(index, function)| (function.name.clone(), index))
        .collect::<HashMap<_, _>>();
    let contexts = discover_function_contexts(program, config, &function_indices);
    let mut function_ids = HashMap::new();
    let mut offset = 0;
    for index in 0..program.defs.len() {
        let Some(mut function_contexts) = contexts.get(&index).cloned() else {
            continue;
        };
        function_contexts.sort_by_key(|context| (context.sample_rate_bits, context.block_size));
        for context in function_contexts {
            function_ids.insert(
                FunctionKey {
                    function_index: index,
                    context,
                },
                FunctionId::new((function_base + offset) as u32),
            );
            offset += 1;
        }
    }
    (function_indices, function_ids)
}

fn synthetic_runtime_function(name: &str, body: Vec<Stmt>) -> TypedFunction {
    TypedFunction {
        name: name.to_owned(),
        method_of: None,
        type_params: Vec::new(),
        params: Vec::new(),
        param_defaults: Vec::new(),
        param_kinds: Vec::new(),
        readonly_array_params: std::collections::HashSet::new(),
        return_ty: ReturnType::Scalar(PrimitiveType::F32),
        returns_value: false,
        local_scalar_types: HashMap::new(),
        body,
    }
}

fn empty_entry_function(name: &str, kind: onda_mir::FunctionKind) -> onda_mir::Function {
    onda_mir::Function {
        name: name.to_owned(),
        kind,
        attributes: compiler_generated_function_attributes(),
        params: Vec::new(),
        results: Vec::new(),
        locals: Vec::new(),
        body: MirBlock::default(),
        source: SourceSpan::UNKNOWN,
    }
}

fn compiler_generated_function_attributes() -> FunctionAttributes {
    FunctionAttributes {
        origin: FunctionOrigin::CompilerGenerated,
        inline: InlineHint::Always,
    }
}

fn source_function_attributes(name: &str) -> FunctionAttributes {
    if crate::internal_names::is_compiler_generated_function_name(name) {
        compiler_generated_function_attributes()
    } else {
        FunctionAttributes::default()
    }
}

fn missing_interface_type(kind: &str, name: &str) -> MirLoweringError {
    MirLoweringError::new(
        format!("semantic {kind} '{name}' has no resolved scalar type"),
        SourceLoc::ZERO,
    )
}

fn call_resource_base(
    name: &str,
    args: &[onda_frontend::CallArg],
    location: SourceLoc,
) -> Result<String, MirLoweringError> {
    let Some(first) = args.first() else {
        return Err(MirLoweringError::new(
            format!("resource builtin '{name}' is missing its base argument"),
            location,
        ));
    };
    let Expr::Var { name: base, .. } = &first.expr else {
        return Err(MirLoweringError::new(
            format!("resource builtin '{name}' base is not a direct resource symbol"),
            first.expr.loc(),
        ));
    };
    Ok(base.clone())
}

fn mir_scalar(value: TypedConstValue) -> ScalarValue {
    match value {
        TypedConstValue::F32(value) => ScalarValue::F32(value),
        TypedConstValue::F64(value) => ScalarValue::F64(value),
        TypedConstValue::I32(value) => ScalarValue::I32(value),
        TypedConstValue::I64(value) => ScalarValue::I64(value),
        TypedConstValue::Bool(value) => ScalarValue::Bool(value),
    }
}

fn scalar_values_exact_equal(lhs: &[ScalarValue], rhs: &[ScalarValue]) -> bool {
    lhs.len() == rhs.len()
        && lhs.iter().zip(rhs).all(|(lhs, rhs)| match (lhs, rhs) {
            (ScalarValue::F32(lhs), ScalarValue::F32(rhs)) => lhs.to_bits() == rhs.to_bits(),
            (ScalarValue::F64(lhs), ScalarValue::F64(rhs)) => lhs.to_bits() == rhs.to_bits(),
            (ScalarValue::I32(lhs), ScalarValue::I32(rhs)) => lhs == rhs,
            (ScalarValue::I64(lhs), ScalarValue::I64(rhs)) => lhs == rhs,
            (ScalarValue::Bool(lhs), ScalarValue::Bool(rhs)) => lhs == rhs,
            _ => false,
        })
}

fn mir_constant(value: TypedConstValue) -> onda_mir::ConstantValue {
    onda_mir::ConstantValue::Scalar(mir_scalar(value))
}

fn mir_range(range: TypedValueRange) -> onda_mir::ValueRange {
    onda_mir::ValueRange {
        min: mir_scalar(range.min),
        max: mir_scalar(range.max),
    }
}

fn mir_param_control(control: &TypedParamControl) -> onda_mir::ParamControl {
    onda_mir::ParamControl {
        scale: match control.scale {
            ParamScale::Linear => onda_mir::ParamScale::Linear,
            ParamScale::Log => onda_mir::ParamScale::Log,
        },
        curve: control.curve,
        unit: control.unit.clone(),
        step: control.step.map(mir_scalar),
        step_count: control.step_count,
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct CompileContext {
    sample_rate_bits: u32,
    block_size: u32,
}

impl CompileContext {
    fn from_config(config: onda_mir::CompileConfig) -> Self {
        Self {
            sample_rate_bits: config.sample_rate.to_bits(),
            block_size: config.block_size,
        }
    }

    fn config(self) -> onda_mir::CompileConfig {
        onda_mir::CompileConfig {
            sample_rate: f32::from_bits(self.sample_rate_bits),
            block_size: self.block_size,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum DynamicInterfaceKind {
    Inputs,
    AudioOutputs,
    ControlOutputs,
    Params,
}

#[derive(Debug, Clone)]
enum RuntimeInterfaceEndpoint {
    Input {
        input: onda_mir::InputId,
        element: Option<u32>,
        clamped: Option<String>,
    },
    AudioOutput {
        output: onda_mir::OutputId,
        element: Option<u32>,
    },
    ControlOutput {
        output: onda_mir::ControlOutputId,
        element: Option<u32>,
    },
    Param {
        param: onda_mir::ParamId,
        element: Option<u32>,
        clamped: Option<onda_mir::StateId>,
    },
}

#[derive(Debug, Clone)]
struct RuntimeInterfaceView {
    element_type: PrimitiveType,
    slots: Vec<RuntimeInterfaceEndpoint>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct FunctionKey {
    function_index: usize,
    context: CompileContext,
}

#[derive(Debug, Default)]
struct RuntimeGlobals {
    states: HashMap<String, (onda_mir::StateId, PrimitiveType)>,
    state_tuples: HashMap<String, Vec<(onda_mir::StateId, PrimitiveType)>>,
    state_arrays: HashMap<String, (onda_mir::StateId, PrimitiveType, u32)>,
    array_struct_roots: HashMap<String, (String, u32)>,
    const_arrays: HashMap<String, (onda_mir::ConstDataId, PrimitiveType, u32)>,
    inputs: HashMap<String, (onda_mir::InputId, PrimitiveType)>,
    input_arrays: HashMap<String, (onda_mir::InputId, PrimitiveType, u32)>,
    outputs: HashMap<String, (onda_mir::OutputId, PrimitiveType)>,
    output_arrays: HashMap<String, (onda_mir::OutputId, PrimitiveType, u32)>,
    control_outputs: HashMap<String, (onda_mir::ControlOutputId, PrimitiveType)>,
    control_output_arrays: HashMap<String, (onda_mir::ControlOutputId, PrimitiveType, u32)>,
    params: HashMap<String, (onda_mir::ParamId, PrimitiveType)>,
    param_arrays: HashMap<String, (onda_mir::ParamId, PrimitiveType, u32)>,
    buffers: HashMap<String, (onda_mir::BufferId, PrimitiveType)>,
    buffer_arrays: HashMap<String, (onda_mir::BufferId, PrimitiveType, u32)>,
    interface_views: HashMap<DynamicInterfaceKind, RuntimeInterfaceView>,
    structs: HashMap<String, Vec<TypedStructField>>,
    aggregate_layouts: AggregateLayoutTable,
    nested_proc_arrays: Vec<TypedNestedProcArray>,
    struct_roots: HashMap<String, String>,
    top_level_oversampling: TopLevelOversamplingState,
}

#[derive(Debug, Default)]
struct TopLevelOversamplingState {
    inputs: HashMap<String, Vec<MirSincStageState>>,
    outputs: HashMap<String, Vec<MirSincStageState>>,
}

#[derive(Debug, Clone)]
struct DiscoveredCall {
    name: String,
    receiver: Option<Expr>,
}

#[derive(Debug, Clone)]
struct MirSincStageState {
    ty: PrimitiveType,
    taps: [onda_mir::StateId; 8],
}

fn discover_function_contexts(
    program: &TypedProgram,
    host_config: onda_mir::CompileConfig,
    function_indices: &HashMap<String, usize>,
) -> HashMap<usize, Vec<CompileContext>> {
    let host_context = CompileContext::from_config(host_config);
    let sample_context = CompileContext::from_config(onda_mir::CompileConfig {
        sample_rate: host_config.sample_rate * program.sample_oversample_factor.max(1) as f32,
        block_size: host_config.block_size,
    });
    let mut contexts = HashMap::<usize, Vec<CompileContext>>::new();
    let mut queue = VecDeque::<(usize, CompileContext)>::new();

    let mut host_calls = Vec::new();
    collect_calls_in_statements(&program.init, &mut host_calls);
    collect_calls_in_statements(&program.block_pre, &mut host_calls);
    collect_calls_in_statements(&program.block_post, &mut host_calls);
    for event in &program.events {
        collect_calls_in_statements(&event.body, &mut host_calls);
    }
    for call in host_calls {
        record_function_context(
            &call,
            host_context,
            program,
            host_config,
            function_indices,
            &mut contexts,
            &mut queue,
        );
    }

    let mut sample_calls = Vec::new();
    collect_calls_in_statements(&program.sample, &mut sample_calls);
    for call in sample_calls {
        record_function_context(
            &call,
            sample_context,
            program,
            host_config,
            function_indices,
            &mut contexts,
            &mut queue,
        );
    }

    drain_context_queue(
        program,
        host_config,
        function_indices,
        &mut contexts,
        &mut queue,
    );

    contexts
}

fn drain_context_queue(
    program: &TypedProgram,
    host_config: onda_mir::CompileConfig,
    function_indices: &HashMap<String, usize>,
    contexts: &mut HashMap<usize, Vec<CompileContext>>,
    queue: &mut VecDeque<(usize, CompileContext)>,
) {
    while let Some((function_index, context)) = queue.pop_front() {
        let function = &program.defs[function_index];
        let mut calls = Vec::new();
        collect_calls_in_statements(&function.body, &mut calls);
        // Defaults execute in the caller's compilation context. Scanning every
        // default may create an unused specialization, but never changes
        // executable behavior and keeps discovery independent of call order.
        for default in function.param_defaults.iter().flatten() {
            collect_calls_in_expr(default, &mut calls);
        }
        for call in calls {
            record_function_context(
                &call,
                context,
                program,
                host_config,
                function_indices,
                contexts,
                queue,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record_function_context(
    call: &DiscoveredCall,
    caller_context: CompileContext,
    program: &TypedProgram,
    host_config: onda_mir::CompileConfig,
    function_indices: &HashMap<String, usize>,
    contexts: &mut HashMap<usize, Vec<CompileContext>>,
    queue: &mut VecDeque<(usize, CompileContext)>,
) {
    let Some(function_index) = function_indices.get(&call.name).copied() else {
        return;
    };
    let context = effective_call_context(
        &call.name,
        call.receiver.as_ref(),
        caller_context,
        host_config,
        &program.def_sample_oversample_factors,
        &program.proc_instance_oversample_factors,
    );
    let entries = contexts.entry(function_index).or_default();
    if !entries.contains(&context) {
        entries.push(context);
        queue.push_back((function_index, context));
    }
}

fn effective_function_context(
    name: &str,
    caller_context: CompileContext,
    host_config: onda_mir::CompileConfig,
    oversample_factors: &HashMap<String, usize>,
) -> CompileContext {
    let Some(factor) = oversample_factors.get(name).copied() else {
        return caller_context;
    };
    if factor <= 1 {
        return caller_context;
    }
    CompileContext::from_config(onda_mir::CompileConfig {
        sample_rate: host_config.sample_rate * factor as f32,
        block_size: host_config.block_size,
    })
}

fn proc_instance_oversample_key_for_expr(
    expression: &Expr,
    factors: &HashMap<String, usize>,
) -> Option<String> {
    match expression {
        Expr::Var { name, .. } => factors.contains_key(name).then(|| name.clone()),
        Expr::Index { base, index, .. } => {
            if let Expr::Int { value, .. } = index.as_ref() {
                let slot = format!("{base}[{value}]");
                if factors.contains_key(&slot) {
                    return Some(slot);
                }
            }
            factors.contains_key(base).then(|| base.clone())
        }
        _ => None,
    }
}

fn effective_call_context(
    name: &str,
    receiver: Option<&Expr>,
    caller_context: CompileContext,
    host_config: onda_mir::CompileConfig,
    oversample_factors: &HashMap<String, usize>,
    proc_instance_oversample_factors: &HashMap<String, usize>,
) -> CompileContext {
    let instance_context = if name.contains(".__onda_proc_") {
        receiver
            .and_then(|expression| {
                proc_instance_oversample_key_for_expr(expression, proc_instance_oversample_factors)
            })
            .and_then(|key| proc_instance_oversample_factors.get(&key).copied())
            .filter(|factor| *factor > 1)
            .map(|factor| {
                CompileContext::from_config(onda_mir::CompileConfig {
                    sample_rate: host_config.sample_rate * factor as f32,
                    block_size: host_config.block_size,
                })
            })
            .unwrap_or(caller_context)
    } else {
        caller_context
    };
    effective_function_context(name, instance_context, host_config, oversample_factors)
}

fn collect_calls_in_statements(statements: &[Stmt], calls: &mut Vec<DiscoveredCall>) {
    for statement in statements {
        match statement {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Assign { target, expr, .. } => {
                match target {
                    AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
                    AssignTarget::Index { index, .. } => collect_calls_in_expr(index, calls),
                    AssignTarget::Slice {
                        selector,
                        channel,
                        start,
                        end,
                        ..
                    } => {
                        for coordinate in [selector, channel, start, end].into_iter().flatten() {
                            collect_calls_in_expr(coordinate, calls);
                        }
                    }
                }
                collect_calls_in_expr(expr, calls);
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_calls_in_expr(expr, calls);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_calls_in_expr(cond, calls);
                collect_calls_in_statements(then_branch, calls);
                collect_calls_in_statements(else_branch, calls);
            }
            Stmt::For {
                step,
                start,
                end,
                body,
                ..
            } => {
                if let Some(step) = step {
                    collect_calls_in_expr(step, calls);
                }
                collect_calls_in_expr(start, calls);
                collect_calls_in_expr(end, calls);
                collect_calls_in_statements(body, calls);
            }
            Stmt::While { cond, body, .. } => {
                collect_calls_in_expr(cond, calls);
                collect_calls_in_statements(body, calls);
            }
        }
    }
}

fn collect_calls_in_expr(expression: &Expr, calls: &mut Vec<DiscoveredCall>) {
    match expression {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_calls_in_expr(value, calls);
            }
        }
        Expr::Index { index, .. } => collect_calls_in_expr(index, calls),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_calls_in_expr(coordinate, calls);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_calls_in_expr(&spec.size, calls);
            if let Some(values) = init {
                for value in values {
                    collect_calls_in_expr(value, calls);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_calls_in_expr(lhs, calls);
            collect_calls_in_expr(rhs, calls);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_calls_in_expr(arg, calls);
            }
        }
        Expr::UserCall { name, args, .. } => {
            calls.push(DiscoveredCall {
                name: name.clone(),
                receiver: args.first().map(|arg| arg.expr.clone()),
            });
            for arg in args {
                collect_calls_in_expr(&arg.expr, calls);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_calls_in_expr(expr, calls)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LoweredValue {
    value: Value,
    ty: PrimitiveType,
}

#[derive(Debug, Clone)]
struct OversampledInputRuntime {
    ty: PrimitiveType,
    raw: LocalId,
    values: Option<LocalId>,
    current: Option<Place>,
}

#[derive(Debug, Clone)]
struct OversampledOutputRuntime {
    ty: PrimitiveType,
    destination: OversampledOutputDestination,
    values: LocalId,
    down_stages: Vec<[Place; 8]>,
}

#[derive(Debug, Clone)]
enum OversampledOutputDestination {
    Place(Place),
    Interface {
        output: onda_mir::OutputId,
        element: Option<u32>,
        current: Place,
    },
}

#[derive(Debug, Clone, Copy)]
struct LoweredSlice {
    value: Value,
    element: PrimitiveType,
    access: onda_mir::AccessMode,
}

#[derive(Debug, Clone)]
enum LoweredStructArrayFieldBase {
    State(onda_mir::StateId),
    Slice(LocalId),
}

#[derive(Debug, Clone)]
struct LoweredStructArrayField {
    base: LoweredStructArrayFieldBase,
    width: u32,
}

#[derive(Debug, Clone)]
struct LoweredStructArrayElement {
    index: LocalId,
    fields: Vec<LoweredStructArrayField>,
}

#[derive(Debug, Clone)]
enum LoweredIndexedStructArgument {
    Direct(LoweredStructArrayElement),
    Dispatch {
        index: LocalId,
        alternatives: Vec<Vec<CallArgument>>,
    },
}

/// One source argument after its observable expression evaluation has
/// completed, but before its value/reference pieces are reordered into the
/// callee's ABI parameter order.
#[derive(Debug, Clone)]
enum PreparedCallArgument {
    Scalar(LoweredValue),
    Array(LoweredSlice),
    Tuple(Vec<LoweredValue>),
    IndexedStruct(LoweredIndexedStructArgument),
    /// Direct resource/aggregate references contain no evaluable subexpression.
    /// ABI loads and slice-descriptor construction may happen later, but cannot
    /// invoke user code or mutate language-visible storage.
    DirectReference(Expr),
}

#[derive(Debug, Clone, Copy)]
enum BufferArrayCallSource {
    Interface(onda_mir::BufferId),
    Parameters(ParameterId),
}

#[derive(Debug, Clone)]
struct PendingCallDispatch {
    index: LocalId,
    argument_start: usize,
    argument_len: usize,
    alternatives: Vec<Vec<CallArgument>>,
    slot_arguments: Vec<(usize, Vec<CallArgument>)>,
}

#[derive(Debug, Clone)]
struct NestedProcElementAlias {
    struct_name: String,
    index: LocalId,
    alternatives: Vec<Vec<CallArgument>>,
}

#[derive(Debug, Clone)]
struct EmbeddedStructArrayShape {
    path: String,
    struct_name: String,
    len: u32,
    fields: Vec<EmbeddedStructArrayFieldShape>,
}

#[derive(Debug, Clone)]
struct EmbeddedStructArrayFieldShape {
    outer_name: String,
    inner_name: String,
    element: PrimitiveType,
    width: u32,
}

#[derive(Debug, Clone)]
struct PendingEmbeddedStructArrayView {
    name: String,
    struct_name: String,
    len: u32,
    fields: Vec<PendingEmbeddedStructArrayField>,
}

#[derive(Debug, Clone)]
struct PendingEmbeddedStructArrayField {
    inner_name: String,
    parameter: ParameterId,
    total_len: u32,
    element: PrimitiveType,
}

#[derive(Debug, Clone)]
enum StructFieldShape {
    Scalar {
        name: String,
        ty: PrimitiveType,
    },
    Array {
        name: String,
        element: PrimitiveType,
        len: u32,
    },
}

#[derive(Debug, Clone)]
enum StructFieldReference {
    Scalar {
        name: String,
        parameter: ParameterId,
        ty: PrimitiveType,
    },
    Array {
        name: String,
        parameter: ParameterId,
        element: PrimitiveType,
        len: u32,
    },
}

#[derive(Debug, Clone, Copy)]
enum StructArrayLength {
    Dynamic(ParameterId),
    Fixed(u32),
}

#[derive(Debug, Clone)]
enum Binding {
    ReferenceParameter(ParameterId, PrimitiveType),
    EventParameter(onda_mir::EventParamId, PrimitiveType),
    EventArrayParameter(onda_mir::EventParamId, PrimitiveType, u32),
    BufferParameter(ParameterId, PrimitiveType),
    BufferParameterArray(ParameterId, PrimitiveType, u32),
    BufferAlias(BufferBindingReference, PrimitiveType),
    Local(LocalId, PrimitiveType),
    Array(LocalId, PrimitiveType, u32),
    ArrayParameter(ParameterId, PrimitiveType, u32),
    Slice(LocalId, PrimitiveType, onda_mir::AccessMode),
    Tuple(Vec<(LocalId, PrimitiveType)>),
    TupleReferenceParameter(Vec<(ParameterId, PrimitiveType)>),
    TupleSliceElementAlias(Vec<(LocalId, PrimitiveType, LocalId)>),
    SliceElementAlias {
        slice: LocalId,
        element: PrimitiveType,
        index: LocalId,
    },
    StructParameter {
        struct_name: String,
        fields: Vec<StructFieldReference>,
    },
    StructArrayParameter {
        struct_name: String,
        length: StructArrayLength,
        fields: Vec<(String, LocalId, PrimitiveType)>,
    },
    ProcArrayParameter {
        proc_name: String,
        fixed_len: u32,
        length: ParameterId,
        active: LocalId,
        fields: Vec<(String, LocalId, PrimitiveType)>,
    },
    StructArrayElementAlias {
        struct_name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BufferBindingReference {
    Interface(onda_mir::BufferRef),
    Parameter(onda_mir::BufferParamRef),
    InterfaceStateArray {
        first: onda_mir::BufferId,
        len: u32,
        selector: onda_mir::StateId,
    },
    ParameterStateArray {
        span: ParameterId,
        selector: onda_mir::StateId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MaterializedBufferReference {
    Interface(onda_mir::BufferRef),
    Parameter(onda_mir::BufferParamRef),
}

#[derive(Debug, Clone, Copy)]
enum ContinueMode {
    None,
    Plain,
    For {
        index: LocalId,
        step: Value,
        source: SourceLoc,
    },
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct PrezeroedStateRegion {
    state: onda_mir::StateId,
    path: Vec<PrezeroedStateProjection>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PrezeroedStateProjection {
    Field(u32),
    Index(u32),
}

struct FunctionLowerer<'a> {
    function: &'a TypedFunction,
    functions: &'a [TypedFunction],
    function_ids: &'a HashMap<FunctionKey, FunctionId>,
    function_indices: &'a HashMap<String, usize>,
    oversample_factors: &'a HashMap<String, usize>,
    proc_instance_oversample_factors: &'a HashMap<String, usize>,
    proc_step_oversample_meta: Option<&'a ProcStepOversampleMeta>,
    structs: &'a HashMap<String, Vec<TypedStructField>>,
    aggregate_layouts: &'a AggregateLayoutTable,
    nested_proc_arrays: &'a [TypedNestedProcArray],
    const_arrays: &'a HashMap<String, (onda_mir::ConstDataId, PrimitiveType, u32)>,
    host_config: onda_mir::CompileConfig,
    config: onda_mir::CompileConfig,
    emitted_name: String,
    types: &'a mut Vec<MirType>,
    source_files: &'a mut Vec<SourceFile>,
    runtime_globals: Option<&'a RuntimeGlobals>,
    current_frame: Option<Value>,
    oversampled_inputs: HashMap<String, (LocalId, PrimitiveType)>,
    audio_output_caches: HashMap<String, (LocalId, PrimitiveType)>,
    oversampled_input_endpoints: HashMap<onda_mir::InputId, (LocalId, PrimitiveType)>,
    audio_output_endpoint_caches: HashMap<onda_mir::OutputId, (LocalId, PrimitiveType)>,
    oversampled_input_arrays: HashMap<onda_mir::InputId, (LocalId, PrimitiveType, u32)>,
    audio_output_array_caches: HashMap<onda_mir::OutputId, (LocalId, PrimitiveType, u32)>,
    params: Vec<onda_mir::FunctionParam>,
    results: Vec<TypeId>,
    locals: Vec<onda_mir::Local>,
    bindings: HashMap<String, Binding>,
    nested_proc_aliases: HashMap<String, NestedProcElementAlias>,
    event_slice_parameters: Vec<(String, onda_mir::EventParamId, PrimitiveType)>,
    /// Exact state regions that are no longer known to contain their
    /// backend-provided all-bits-zero init image. `Some(empty)` is the
    /// straight-line prefix of the init entry; `None` disables the proof
    /// outside init or after an alias/control-flow barrier.
    prezeroed_init_state_dirty: Option<Vec<PrezeroedStateRegion>>,
}

fn function_location(function: &TypedFunction) -> SourceLoc {
    function
        .body
        .first()
        .map(Stmt::loc)
        .unwrap_or(SourceLoc::ZERO)
}

fn event_location(event: &TypedEvent) -> SourceLoc {
    event.body.first().map(Stmt::loc).unwrap_or(SourceLoc::ZERO)
}

fn scalar_type(ty: PrimitiveType) -> ScalarType {
    match ty {
        PrimitiveType::F32 => ScalarType::F32,
        PrimitiveType::F64 => ScalarType::F64,
        PrimitiveType::I32 => ScalarType::I32,
        PrimitiveType::I64 => ScalarType::I64,
        PrimitiveType::Bool => ScalarType::Bool,
    }
}

fn float_constant(ty: PrimitiveType, value: f64) -> Value {
    match ty {
        PrimitiveType::F32 => Value::Constant(ScalarValue::F32(value as f32)),
        PrimitiveType::F64 => Value::Constant(ScalarValue::F64(value)),
        _ => unreachable!("sinc filters are only defined for floating-point surfaces"),
    }
}

fn zero_value(ty: PrimitiveType) -> Value {
    Value::Constant(match ty {
        PrimitiveType::F32 => ScalarValue::F32(0.0),
        PrimitiveType::F64 => ScalarValue::F64(0.0),
        PrimitiveType::I32 => ScalarValue::I32(0),
        PrimitiveType::I64 => ScalarValue::I64(0),
        PrimitiveType::Bool => ScalarValue::Bool(false),
    })
}

fn prezeroed_state_region(place: &Place) -> Option<(PrezeroedStateRegion, bool)> {
    let PlaceBase::State(state) = place.base else {
        return None;
    };
    let mut path = Vec::with_capacity(place.projections.len());
    for projection in &place.projections {
        match projection {
            Projection::Field(field) => {
                path.push(PrezeroedStateProjection::Field(field.raw()));
            }
            Projection::Index { index, .. } => {
                let Value::Constant(ScalarValue::I32(index)) = index else {
                    return Some((
                        PrezeroedStateRegion {
                            state,
                            path: Vec::new(),
                        },
                        false,
                    ));
                };
                let Ok(index) = u32::try_from(*index) else {
                    return Some((
                        PrezeroedStateRegion {
                            state,
                            path: Vec::new(),
                        },
                        false,
                    ));
                };
                path.push(PrezeroedStateProjection::Index(index));
            }
        }
    }
    Some((PrezeroedStateRegion { state, path }, true))
}

fn prezeroed_state_path_is_prefix(
    prefix: &[PrezeroedStateProjection],
    path: &[PrezeroedStateProjection],
) -> bool {
    prefix.len() <= path.len() && prefix.iter().zip(path).all(|(lhs, rhs)| lhs == rhs)
}

fn prezeroed_state_regions_overlap(lhs: &PrezeroedStateRegion, rhs: &PrezeroedStateRegion) -> bool {
    lhs.state == rhs.state
        && (prezeroed_state_path_is_prefix(&lhs.path, &rhs.path)
            || prezeroed_state_path_is_prefix(&rhs.path, &lhs.path))
}

fn mark_prezeroed_state_dirty(dirty: &mut Vec<PrezeroedStateRegion>, region: PrezeroedStateRegion) {
    if dirty.iter().any(|existing| {
        existing.state == region.state
            && prezeroed_state_path_is_prefix(&existing.path, &region.path)
    }) {
        return;
    }
    dirty.retain(|existing| {
        existing.state != region.state
            || !prezeroed_state_path_is_prefix(&region.path, &existing.path)
    });
    dirty.push(region);
}

fn clear_prezeroed_state_region(
    dirty: &mut Vec<PrezeroedStateRegion>,
    region: &PrezeroedStateRegion,
) {
    dirty.retain(|existing| {
        existing.state != region.state
            || !prezeroed_state_path_is_prefix(&region.path, &existing.path)
    });
}

fn scalar_value_is_all_bits_zero(value: Value) -> bool {
    match value {
        Value::Constant(ScalarValue::F32(value)) => value.to_bits() == 0,
        Value::Constant(ScalarValue::F64(value)) => value.to_bits() == 0,
        Value::Constant(ScalarValue::I32(value)) => value == 0,
        Value::Constant(ScalarValue::I64(value)) => value == 0,
        Value::Constant(ScalarValue::Bool(value)) => !value,
        Value::Local(_) => false,
    }
}

fn mir_sinc_stage_places(stage: &MirSincStageState) -> [Place; 8] {
    debug_assert!(matches!(stage.ty, PrimitiveType::F32 | PrimitiveType::F64));
    stage.taps.map(|state| Place {
        base: PlaceBase::State(state),
        projections: Vec::new(),
    })
}

fn intern_scalar_type(types: &mut Vec<MirType>, ty: PrimitiveType) -> TypeId {
    let scalar = scalar_type(ty);
    if let Some(index) = types
        .iter()
        .position(|candidate| *candidate == MirType::Scalar(scalar))
    {
        return TypeId::new(index as u32);
    }
    let id = TypeId::new(types.len() as u32);
    types.push(MirType::Scalar(scalar));
    id
}

fn intern_array_type(types: &mut Vec<MirType>, element: PrimitiveType, len: u32) -> TypeId {
    let element = intern_scalar_type(types, element);
    let array = MirType::Array { element, len };
    if let Some(index) = types.iter().position(|candidate| *candidate == array) {
        return TypeId::new(index as u32);
    }
    let id = TypeId::new(types.len() as u32);
    types.push(array);
    id
}

fn intern_slice_type(
    types: &mut Vec<MirType>,
    element: PrimitiveType,
    access: onda_mir::AccessMode,
) -> TypeId {
    let slice = MirType::Slice {
        element: scalar_type(element),
        access,
    };
    if let Some(index) = types.iter().position(|candidate| *candidate == slice) {
        return TypeId::new(index as u32);
    }
    let id = TypeId::new(types.len() as u32);
    types.push(slice);
    id
}

fn intern_buffer_type(
    types: &mut Vec<MirType>,
    element: PrimitiveType,
    channels: onda_mir::BufferChannels,
    access: onda_mir::AccessMode,
) -> TypeId {
    let buffer = MirType::Buffer {
        element: scalar_type(element),
        channels,
        access,
    };
    if let Some(index) = types.iter().position(|candidate| *candidate == buffer) {
        return TypeId::new(index as u32);
    }
    let id = TypeId::new(types.len() as u32);
    types.push(buffer);
    id
}

fn intern_buffer_span_type(
    types: &mut Vec<MirType>,
    element: PrimitiveType,
    channels: onda_mir::BufferChannels,
    access: onda_mir::AccessMode,
    len: u32,
) -> TypeId {
    let span = MirType::BufferSpan {
        element: scalar_type(element),
        channels,
        access,
        len,
    };
    if let Some(index) = types.iter().position(|candidate| *candidate == span) {
        return TypeId::new(index as u32);
    }
    let id = TypeId::new(types.len() as u32);
    types.push(span);
    id
}

fn mir_buffer_channels(channels: &TypedBufferChannels) -> Option<onda_mir::BufferChannels> {
    match channels {
        TypedBufferChannels::Mono => Some(onda_mir::BufferChannels::Mono),
        TypedBufferChannels::Static(channels) => u32::try_from(*channels)
            .ok()
            .map(onda_mir::BufferChannels::Static),
        TypedBufferChannels::Dynamic => Some(onda_mir::BufferChannels::Dynamic),
    }
}

fn zero_scalar(ty: PrimitiveType) -> ScalarValue {
    match ty {
        PrimitiveType::F32 => ScalarValue::F32(0.0),
        PrimitiveType::F64 => ScalarValue::F64(0.0),
        PrimitiveType::I32 => ScalarValue::I32(0),
        PrimitiveType::I64 => ScalarValue::I64(0),
        PrimitiveType::Bool => ScalarValue::Bool(false),
    }
}

fn zero_expr(ty: PrimitiveType) -> Expr {
    match ty {
        PrimitiveType::Bool => Expr::bool(false),
        PrimitiveType::I32 | PrimitiveType::I64 => Expr::int(0),
        PrimitiveType::F32 | PrimitiveType::F64 => Expr::number(0.0),
    }
}

fn scalar_from_f64(value: f64, ty: PrimitiveType) -> ScalarValue {
    match ty {
        PrimitiveType::F32 => ScalarValue::F32(value as f32),
        PrimitiveType::F64 => ScalarValue::F64(value),
        PrimitiveType::I32 => ScalarValue::I32(value as i32),
        PrimitiveType::I64 => ScalarValue::I64(value as i64),
        PrimitiveType::Bool => ScalarValue::Bool(value != 0.0),
    }
}

fn cast_scalar_constant(value: ScalarValue, to: PrimitiveType) -> Option<ScalarValue> {
    use PrimitiveType::{F32, F64, I32, I64};
    use ScalarValue::{
        Bool as BoolValue, F32 as F32Value, F64 as F64Value, I32 as I32Value, I64 as I64Value,
    };

    match (value, to) {
        (BoolValue(_), _) | (_, PrimitiveType::Bool) => None,
        (F32Value(value), F32) => Some(F32Value(value)),
        (F32Value(value), F64) => Some(F64Value(value as f64)),
        (F32Value(value), I32) => Some(I32Value(value as i32)),
        (F32Value(value), I64) => Some(I64Value(value as i64)),
        (F64Value(value), F32) => Some(F32Value(value as f32)),
        (F64Value(value), F64) => Some(F64Value(value)),
        (F64Value(value), I32) => Some(I32Value(value as i32)),
        (F64Value(value), I64) => Some(I64Value(value as i64)),
        (I32Value(value), F32) => Some(F32Value(value as f32)),
        (I32Value(value), F64) => Some(F64Value(value as f64)),
        (I32Value(value), I32) => Some(I32Value(value)),
        (I32Value(value), I64) => Some(I64Value(value as i64)),
        (I64Value(value), F32) => Some(F32Value(value as f32)),
        (I64Value(value), F64) => Some(F64Value(value as f64)),
        (I64Value(value), I32) => Some(I32Value(value as i32)),
        (I64Value(value), I64) => Some(I64Value(value)),
    }
}

fn merge_integer_types(lhs: PrimitiveType, rhs: PrimitiveType) -> Option<PrimitiveType> {
    match (lhs, rhs) {
        (PrimitiveType::I32, PrimitiveType::I32) => Some(PrimitiveType::I32),
        (PrimitiveType::I32, PrimitiveType::I64)
        | (PrimitiveType::I64, PrimitiveType::I32)
        | (PrimitiveType::I64, PrimitiveType::I64) => Some(PrimitiveType::I64),
        _ => None,
    }
}

fn map_binary(op: AstBinaryOp) -> MirBinaryOp {
    match op {
        AstBinaryOp::Add => MirBinaryOp::Add,
        AstBinaryOp::Sub => MirBinaryOp::Subtract,
        AstBinaryOp::Mul => MirBinaryOp::Multiply,
        AstBinaryOp::Div => MirBinaryOp::Divide,
        AstBinaryOp::Mod => MirBinaryOp::Remainder,
        AstBinaryOp::BitAnd => MirBinaryOp::BitAnd,
        AstBinaryOp::BitOr => MirBinaryOp::BitOr,
        AstBinaryOp::BitXor => MirBinaryOp::BitXor,
        AstBinaryOp::ShiftLeft => MirBinaryOp::ShiftLeft,
        AstBinaryOp::ShiftRight => MirBinaryOp::ShiftRight,
    }
}

fn map_compare(op: CmpOp) -> CompareOp {
    match op {
        CmpOp::Eq => CompareOp::Equal,
        CmpOp::Ne => CompareOp::NotEqual,
        CmpOp::Lt => CompareOp::Less,
        CmpOp::Le => CompareOp::LessEqual,
        CmpOp::Gt => CompareOp::Greater,
        CmpOp::Ge => CompareOp::GreaterEqual,
    }
}

fn map_intrinsic(function: BuiltinFn) -> Intrinsic {
    match function {
        BuiltinFn::Sin => Intrinsic::Sin,
        BuiltinFn::Cos => Intrinsic::Cos,
        BuiltinFn::Tan => Intrinsic::Tan,
        BuiltinFn::Tanh => Intrinsic::Tanh,
        BuiltinFn::Atan => Intrinsic::Atan,
        BuiltinFn::Atan2 => Intrinsic::Atan2,
        BuiltinFn::Exp => Intrinsic::Exp,
        BuiltinFn::Log => Intrinsic::Log,
        BuiltinFn::Sqrt => Intrinsic::Sqrt,
        BuiltinFn::Pow => Intrinsic::Pow,
        BuiltinFn::Abs => Intrinsic::Abs,
        BuiltinFn::Floor => Intrinsic::Floor,
        BuiltinFn::Ceil => Intrinsic::Ceil,
        BuiltinFn::Round => Intrinsic::Round,
        BuiltinFn::Trunc => Intrinsic::Trunc,
        BuiltinFn::Min => Intrinsic::Min,
        BuiltinFn::Max => Intrinsic::Max,
        BuiltinFn::Fma => Intrinsic::Fma,
        BuiltinFn::RangeClamp => Intrinsic::RangeClamp,
    }
}

#[cfg(test)]
mod tests;
