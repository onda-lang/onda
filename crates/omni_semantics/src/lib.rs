use std::collections::{HashMap, HashSet};

use omni_frontend::{
    inject_auto_std_math, with_diagnostic_location, AssignTarget, BinaryOp, Block, BlockExec,
    BlockKind, BufferChannels, BufferDecl, BufferElemType, BufferType, BuiltinFn, CallArg,
    CallTypeArg, CmpOp, DataElemType, DeclRange, DeclType, Diagnostic, Expr, FieldType,
    FnParamType, FunctionDef, ParamDecl, PortDecl, PrimitiveType, ProcessorDef, Program, Stmt,
    StructDef, StructField,
};

#[derive(Debug, Clone)]
pub struct TypedProgram {
    pub ins: Vec<String>,
    pub outs: Vec<String>,
    pub in_types: HashMap<String, PrimitiveType>,
    pub out_types: HashMap<String, PrimitiveType>,
    pub param_types: HashMap<String, PrimitiveType>,
    pub in_defaults: HashMap<String, TypedConstValue>,
    pub in_ranges: HashMap<String, TypedValueRange>,
    pub in_arrays: HashMap<String, TypedArrayInfo>,
    pub out_arrays: HashMap<String, TypedArrayInfo>,
    pub param_arrays: HashMap<String, TypedArrayInfo>,
    pub params: Vec<TypedParam>,
    pub buffers: Vec<TypedBufferDecl>,
    pub structs: Vec<TypedStruct>,
    pub defs: Vec<TypedFunction>,
    pub init: Vec<Stmt>,
    pub block_pre: Vec<Stmt>,
    pub sample: Vec<Stmt>,
    pub block_post: Vec<Stmt>,
    pub state_vars: Vec<String>,
    pub state_types: Vec<PrimitiveType>,
    pub data_vars: Vec<TypedDataVar>,
    pub data_struct_roots: Vec<TypedDataStructRoot>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TypedBufferChannels {
    Mono,
    Static(usize),
    Dynamic,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TypedBufferDecl {
    pub name: String,
    pub elem_ty: PrimitiveType,
    pub channels: TypedBufferChannels,
}

#[derive(Debug, Clone)]
pub struct TypedStruct {
    pub name: String,
    pub fields: Vec<TypedStructField>,
}

#[derive(Debug, Clone)]
pub struct TypedStructField {
    pub name: String,
    pub ty: TypedFieldType,
    pub default: Option<Expr>,
    pub data_elem_ty: Option<PrimitiveType>,
    pub data_elem_struct: Option<String>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum TypedFieldType {
    Scalar(PrimitiveType),
    Data(usize),
}

#[derive(Debug, Clone)]
pub struct TypedFunction {
    pub name: String,
    pub method_of: Option<String>,
    pub type_params: Vec<String>,
    pub params: Vec<String>,
    pub param_defaults: Vec<Option<Expr>>,
    pub param_kinds: Vec<TypedFnParam>,
    pub return_ty: PrimitiveType,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TypedFnParam {
    Scalar,
    Struct {
        struct_name: String,
    },
    Buffer {
        elem_ty: PrimitiveType,
        channels: TypedBufferChannels,
    },
}

#[derive(Debug, Clone)]
pub struct TypedParam {
    pub name: String,
    pub ty: PrimitiveType,
    pub default: TypedConstValue,
    pub range: Option<TypedValueRange>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypedValueRange {
    pub min: TypedConstValue,
    pub max: TypedConstValue,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TypedConstValue {
    F32(f32),
    F64(f64),
    I32(i32),
    I64(i64),
    Bool(bool),
}

impl TypedConstValue {
    pub fn to_f32(self) -> f32 {
        match self {
            Self::F32(v) => v,
            Self::F64(v) => v as f32,
            Self::I32(v) => v as f32,
            Self::I64(v) => v as f32,
            Self::Bool(v) => {
                if v {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    pub fn to_f64(self) -> f64 {
        match self {
            Self::F32(v) => v as f64,
            Self::F64(v) => v,
            Self::I32(v) => v as f64,
            Self::I64(v) => v as f64,
            Self::Bool(v) => {
                if v {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TypedArrayInfo {
    pub elem_ty: PrimitiveType,
    pub len: usize,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct TypedDataVar {
    pub name: String,
    pub len: usize,
    pub elem_ty: PrimitiveType,
}

#[derive(Debug, Clone)]
pub struct TypedDataStructRoot {
    pub name: String,
    pub struct_name: String,
    pub len: usize,
}

#[derive(Debug, Clone)]
struct DataStructRootInfo {
    struct_name: String,
    len: usize,
}

#[derive(Debug, Clone)]
struct LocalDataAliasInfo {
    len: usize,
    elem_ty: PrimitiveType,
    elem_struct: Option<String>,
    writable: bool,
}

type LocalAliasTypes = HashMap<String, PrimitiveType>;

#[derive(Debug, Clone)]
enum StructDataLayoutKind {
    Scalar(PrimitiveType),
    Data {
        len: usize,
        elem_ty: Option<PrimitiveType>,
        elem_struct: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct StructDataLayoutField {
    name: String,
    kind: StructDataLayoutKind,
}

#[derive(Debug, Clone, Copy)]
pub struct AnalysisOptions {
    pub sample_rate: f32,
    pub block_size: usize,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            block_size: 512,
        }
    }
}

impl TypedProgram {
    pub fn param_default(&self, name: &str) -> Option<f32> {
        self.params
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.default.to_f32())
    }
}

#[derive(Debug, Clone, Copy)]
enum ScopeKind {
    Init,
    Sample,
    Def,
}

fn with_stmt_diag_context<T>(stmt: &Stmt, f: impl FnOnce() -> T) -> T {
    with_diagnostic_location(stmt.loc(), f)
}

fn with_stmt_diag_context_mut<T>(stmt: &mut Stmt, f: impl FnOnce(&mut Stmt) -> T) -> T {
    let loc = stmt.loc().cloned();
    with_diagnostic_location(loc.as_ref(), || f(stmt))
}

const PROC_INDEX_SENTINEL_PREFIX: &str = "__omni_proc_index__";
const PROC_INDEX_SENTINEL_ARG: &str = "__proc_index";
const PROC_INIT_FN_SUFFIX: &str = ".__proc_init";
const PROC_BLOCK_PRE_FN_SUFFIX: &str = ".__proc_block_pre";
const PROC_BLOCK_POST_FN_SUFFIX: &str = ".__proc_block_post";
const PROC_STEP_FN_SUFFIX: &str = ".__proc_step";
const PROC_CALL_OUT_FN_PREFIX: &str = ".__proc_call_out";
const DECLARED_INPUT_TYPE_PREFIX: &str = "__omni_decl_input_ty__";
const DECLARED_OUTPUT_TYPE_PREFIX: &str = "__omni_decl_output_ty__";
const DECLARED_PARAM_TYPE_PREFIX: &str = "__omni_decl_param_ty__";
const DECLARED_DATA_ELEM_TYPE_PREFIX: &str = "__omni_decl_data_elem_ty__";
const DECLARED_BUFFER_ELEM_TYPE_PREFIX: &str = "__omni_decl_buffer_elem_ty__";
const DECLARED_FUNCTION_RETURN_TYPE_PREFIX: &str = "__omni_decl_fn_ret_ty__";
const DECLARED_BUFFER_MULTICHANNEL_PREFIX: &str = "__omni_decl_buffer_multich__";
const DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX: &str = "__omni_decl_buffer_dynch__";
const DECLARED_BUFFER_STATIC_CHANNELS_PREFIX: &str = "__omni_decl_buffer_stch__";
const DECLARED_BUFFER_ELEM_F32_PREFIX: &str = "__omni_decl_buffer_elem_f32__";
const DECLARED_BUFFER_ELEM_F64_PREFIX: &str = "__omni_decl_buffer_elem_f64__";
const DECLARED_BUFFER_ELEM_I32_PREFIX: &str = "__omni_decl_buffer_elem_i32__";
const DECLARED_BUFFER_ELEM_I64_PREFIX: &str = "__omni_decl_buffer_elem_i64__";
const DECLARED_BUFFER_ELEM_BOOL_PREFIX: &str = "__omni_decl_buffer_elem_bool__";
const INTERNAL_BUFFER_READ2_FN: &str = "__omni_buffer_read2";
const INTERNAL_BUFFER_WRITE2_FN: &str = "__omni_buffer_write2";

fn declared_type_key(prefix: &str, name: &str) -> String {
    format!("{prefix}{name}")
}

fn set_declared_symbol_types(
    state_scalars: &mut HashMap<String, PrimitiveType>,
    names: &HashSet<String>,
    types: &HashMap<String, PrimitiveType>,
    key_prefix: &str,
) {
    for name in names {
        let ty = *types.get(name).unwrap_or(&PrimitiveType::F32);
        state_scalars.insert(declared_type_key(key_prefix, name), ty);
    }
}

fn get_declared_symbol_type(
    state_scalars: &HashMap<String, PrimitiveType>,
    name: &str,
    key_prefix: &str,
) -> Option<PrimitiveType> {
    state_scalars
        .get(&declared_type_key(key_prefix, name))
        .copied()
}

fn has_declared_buffer_symbol(known_scalars: &HashSet<String>, name: &str) -> bool {
    known_scalars.contains(&declared_type_key(DECLARED_BUFFER_ELEM_TYPE_PREFIX, name))
}

fn is_declared_multichannel_buffer_symbol(known_scalars: &HashSet<String>, name: &str) -> bool {
    known_scalars.contains(&declared_type_key(
        DECLARED_BUFFER_MULTICHANNEL_PREFIX,
        name,
    ))
}

fn buffer_elem_decl_prefix(elem_ty: PrimitiveType) -> &'static str {
    match elem_ty {
        PrimitiveType::F32 => DECLARED_BUFFER_ELEM_F32_PREFIX,
        PrimitiveType::F64 => DECLARED_BUFFER_ELEM_F64_PREFIX,
        PrimitiveType::I32 => DECLARED_BUFFER_ELEM_I32_PREFIX,
        PrimitiveType::I64 => DECLARED_BUFFER_ELEM_I64_PREFIX,
        PrimitiveType::Bool => DECLARED_BUFFER_ELEM_BOOL_PREFIX,
    }
}

fn declared_buffer_static_channels_key(name: &str, channels: usize) -> String {
    format!("{DECLARED_BUFFER_STATIC_CHANNELS_PREFIX}{name}__{channels}")
}

fn has_declared_buffer_elem_type(
    known_scalars: &HashSet<String>,
    name: &str,
    elem_ty: PrimitiveType,
) -> bool {
    known_scalars.contains(&declared_type_key(buffer_elem_decl_prefix(elem_ty), name))
}

fn has_declared_dynamic_buffer_channels(known_scalars: &HashSet<String>, name: &str) -> bool {
    known_scalars.contains(&declared_type_key(
        DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX,
        name,
    ))
}

fn declared_static_buffer_channels(known_scalars: &HashSet<String>, name: &str) -> Option<usize> {
    let prefix = format!("{DECLARED_BUFFER_STATIC_CHANNELS_PREFIX}{name}__");
    for symbol in known_scalars {
        if let Some(ch) = symbol.strip_prefix(&prefix) {
            if let Ok(parsed) = ch.parse::<usize>() {
                return Some(parsed);
            }
        }
    }
    None
}

fn coerce_const_default_to_typed(raw_default: f64, ty: PrimitiveType) -> TypedConstValue {
    match ty {
        PrimitiveType::F32 => TypedConstValue::F32(raw_default as f32),
        PrimitiveType::F64 => TypedConstValue::F64(raw_default),
        PrimitiveType::I32 => TypedConstValue::I32(raw_default as i32),
        PrimitiveType::I64 => TypedConstValue::I64(raw_default as i64),
        PrimitiveType::Bool => TypedConstValue::Bool(raw_default != 0.0),
    }
}

fn int_bounds_for_type(ty: PrimitiveType) -> Option<(f64, f64)> {
    match ty {
        PrimitiveType::I32 => Some((i32::MIN as f64, i32::MAX as f64)),
        PrimitiveType::I64 => Some((i64::MIN as f64, i64::MAX as f64)),
        _ => None,
    }
}

fn primitive_type_label(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

fn typed_min_for_type(ty: PrimitiveType) -> TypedConstValue {
    match ty {
        PrimitiveType::F32 => TypedConstValue::F32(f32::MIN),
        PrimitiveType::F64 => TypedConstValue::F64(f64::MIN),
        PrimitiveType::I32 => TypedConstValue::I32(i32::MIN),
        PrimitiveType::I64 => TypedConstValue::I64(i64::MIN),
        PrimitiveType::Bool => TypedConstValue::Bool(false),
    }
}

fn eval_typed_const_expr(
    expr: &Expr,
    ty: PrimitiveType,
    options: AnalysisOptions,
    context: &str,
    allow_non_finite: bool,
    require_integral: bool,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    let raw = eval_const_expr_f64(expr, options, context, errors)?;
    if !allow_non_finite && !raw.is_finite() {
        errors.push(Diagnostic::semantic(
            format!("{context} must be finite"),
            0,
            0,
        ));
        return None;
    }
    if require_integral && (!raw.is_finite() || raw.fract() != 0.0) {
        errors.push(Diagnostic::semantic(
            format!("{context} must be an integer constant"),
            0,
            0,
        ));
        return None;
    }
    if let Some((min, max)) = int_bounds_for_type(ty) {
        if !raw.is_finite() || raw < min || raw > max {
            errors.push(Diagnostic::semantic(
                format!("{context} is out of range for {}", primitive_type_label(ty)),
                0,
                0,
            ));
            return None;
        }
    }
    Some(coerce_const_default_to_typed(raw, ty))
}

fn eval_decl_range_for_type(
    range: &DeclRange,
    ty: PrimitiveType,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedValueRange> {
    if ty == PrimitiveType::Bool {
        errors.push(Diagnostic::semantic(
            format!("{context} range is not supported for bool"),
            0,
            0,
        ));
        return None;
    }
    let require_integral = matches!(ty, PrimitiveType::I32 | PrimitiveType::I64);
    let min = if let Some(min_expr) = &range.min {
        eval_typed_const_expr(
            min_expr,
            ty,
            options,
            &format!("{context} range minimum"),
            false,
            require_integral,
            errors,
        )?
    } else {
        typed_min_for_type(ty)
    };
    let max = eval_typed_const_expr(
        &range.max,
        ty,
        options,
        &format!("{context} range maximum"),
        false,
        require_integral,
        errors,
    )?;
    if min.to_f64() > max.to_f64() {
        errors.push(Diagnostic::semantic(
            format!("{context} range minimum is greater than range maximum"),
            0,
            0,
        ));
        return None;
    }
    Some(TypedValueRange { min, max })
}

fn clamp_typed_const_to_range(value: TypedConstValue, range: TypedValueRange) -> TypedConstValue {
    match (value, range.min, range.max) {
        (TypedConstValue::F32(v), TypedConstValue::F32(min), TypedConstValue::F32(max)) => {
            if v.is_nan() {
                TypedConstValue::F32(min)
            } else if !v.is_finite() {
                TypedConstValue::F32(if v.is_sign_negative() { min } else { max })
            } else if v < min {
                TypedConstValue::F32(min)
            } else if v > max {
                TypedConstValue::F32(max)
            } else {
                TypedConstValue::F32(v)
            }
        }
        (TypedConstValue::F64(v), TypedConstValue::F64(min), TypedConstValue::F64(max)) => {
            if v.is_nan() {
                TypedConstValue::F64(min)
            } else if !v.is_finite() {
                TypedConstValue::F64(if v.is_sign_negative() { min } else { max })
            } else if v < min {
                TypedConstValue::F64(min)
            } else if v > max {
                TypedConstValue::F64(max)
            } else {
                TypedConstValue::F64(v)
            }
        }
        (TypedConstValue::I32(v), TypedConstValue::I32(min), TypedConstValue::I32(max)) => {
            TypedConstValue::I32(v.clamp(min, max))
        }
        (TypedConstValue::I64(v), TypedConstValue::I64(min), TypedConstValue::I64(max)) => {
            TypedConstValue::I64(v.clamp(min, max))
        }
        (other, _, _) => other,
    }
}

fn typed_const_expr(value: TypedConstValue) -> Expr {
    match value {
        TypedConstValue::F32(v) => Expr::Number(v),
        TypedConstValue::F64(v) => Expr::Number(v as f32),
        TypedConstValue::I32(v) => Expr::Int(v as i64),
        TypedConstValue::I64(v) => Expr::Int(v),
        TypedConstValue::Bool(v) => Expr::Bool(v),
    }
}

fn clamp_expr_to_range(expr: Expr, range: TypedValueRange) -> Expr {
    let min_expr = typed_const_expr(range.min);
    let max_expr = typed_const_expr(range.max);
    Expr::Call {
        func: BuiltinFn::Max,
        args: vec![
            Expr::Call {
                func: BuiltinFn::Min,
                args: vec![expr, max_expr],
            },
            min_expr,
        ],
    }
}

fn rewrite_top_level_range_clamps_in_expr(
    expr: &mut Expr,
    input_aliases: &HashMap<String, String>,
    param_aliases: &HashMap<String, String>,
    clamp_inputs: bool,
    clamp_params: bool,
) {
    match expr {
        Expr::Var(name) => {
            if clamp_inputs {
                if let Some(alias) = input_aliases.get(name) {
                    *expr = Expr::Var(alias.clone());
                    return;
                }
            }
            if clamp_params {
                if let Some(alias) = param_aliases.get(name) {
                    *expr = Expr::Var(alias.clone());
                }
            }
        }
        Expr::Index { index, .. } => {
            rewrite_top_level_range_clamps_in_expr(
                index,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
        }
        Expr::DataCtor { spec, init } => {
            rewrite_top_level_range_clamps_in_expr(
                &mut spec.size,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_top_level_range_clamps_in_expr(
                        value,
                        input_aliases,
                        param_aliases,
                        clamp_inputs,
                        clamp_params,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_top_level_range_clamps_in_expr(
                lhs,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
            rewrite_top_level_range_clamps_in_expr(
                rhs,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_top_level_range_clamps_in_expr(
                    arg,
                    input_aliases,
                    param_aliases,
                    clamp_inputs,
                    clamp_params,
                );
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_top_level_range_clamps_in_expr(
                inner,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_top_level_range_clamps_in_expr(
                    value,
                    input_aliases,
                    param_aliases,
                    clamp_inputs,
                    clamp_params,
                );
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_top_level_range_clamps_in_expr(
                    &mut arg.expr,
                    input_aliases,
                    param_aliases,
                    clamp_inputs,
                    clamp_params,
                );
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

fn rewrite_top_level_range_clamps_in_stmt(
    stmt: &mut Stmt,
    input_aliases: &HashMap<String, String>,
    param_aliases: &HashMap<String, String>,
    clamp_inputs: bool,
    clamp_params: bool,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                rewrite_top_level_range_clamps_in_expr(
                    index,
                    input_aliases,
                    param_aliases,
                    clamp_inputs,
                    clamp_params,
                );
            }
            rewrite_top_level_range_clamps_in_expr(
                expr,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_top_level_range_clamps_in_expr(
                expr,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_top_level_range_clamps_in_expr(
                cond,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
            for nested in then_branch {
                rewrite_top_level_range_clamps_in_stmt(
                    nested,
                    input_aliases,
                    param_aliases,
                    clamp_inputs,
                    clamp_params,
                );
            }
            for nested in else_branch {
                rewrite_top_level_range_clamps_in_stmt(
                    nested,
                    input_aliases,
                    param_aliases,
                    clamp_inputs,
                    clamp_params,
                );
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            rewrite_top_level_range_clamps_in_expr(
                start,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
            rewrite_top_level_range_clamps_in_expr(
                end,
                input_aliases,
                param_aliases,
                clamp_inputs,
                clamp_params,
            );
            for nested in body {
                rewrite_top_level_range_clamps_in_stmt(
                    nested,
                    input_aliases,
                    param_aliases,
                    clamp_inputs,
                    clamp_params,
                );
            }
        }
    }
}

fn build_top_level_range_hoist_assign(
    alias_name: String,
    source_name: &str,
    _ty: PrimitiveType,
    range: TypedValueRange,
) -> Stmt {
    Stmt::Assign {
        loc: None,
        target: AssignTarget::Var(alias_name),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        expr: clamp_expr_to_range(Expr::Var(source_name.to_owned()), range),
    }
}

fn expand_port_decls(
    ports: &[PortDecl],
    kind: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (
    Vec<String>,
    HashMap<String, PrimitiveType>,
    HashMap<String, TypedArrayInfo>,
    HashMap<String, TypedConstValue>,
    HashMap<String, TypedValueRange>,
) {
    let mut flat = Vec::new();
    let mut types = HashMap::new();
    let mut arrays = HashMap::new();
    let mut defaults = HashMap::new();
    let mut ranges = HashMap::new();

    for port in ports {
        match port.ty.as_ref() {
            None | Some(DeclType::Scalar(_)) => {
                let ty = match port.ty.as_ref() {
                    Some(DeclType::Scalar(t)) => *t,
                    _ => PrimitiveType::F32,
                };
                let raw_default = match &port.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        ty,
                        options,
                        &format!("{kind} '{}' default", port.name),
                        is_float_type(ty),
                        matches!(ty, PrimitiveType::I32 | PrimitiveType::I64),
                        errors,
                    )
                    .unwrap_or_else(|| coerce_const_default_to_typed(0.0, ty)),
                    None => coerce_const_default_to_typed(0.0, ty),
                };
                let mut default = raw_default;
                let range = port.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        ty,
                        options,
                        &format!("{kind} '{}'", port.name),
                        errors,
                    )
                });
                if let Some(r) = range {
                    default = clamp_typed_const_to_range(raw_default, r);
                    ranges.insert(port.name.clone(), r);
                }
                flat.push(port.name.clone());
                types.insert(port.name.clone(), ty);
                defaults.insert(port.name.clone(), default);
            }
            Some(DeclType::Generic(param)) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{kind} '{}' uses unresolved generic type '{}'",
                        port.name, param
                    ),
                    0,
                    0,
                ));
                flat.push(port.name.clone());
                types.insert(port.name.clone(), PrimitiveType::F32);
                defaults.insert(
                    port.name.clone(),
                    coerce_const_default_to_typed(0.0, PrimitiveType::F32),
                );
            }
            Some(DeclType::ArrayGeneric { elem, size }) => {
                if port.default.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{kind} '{}' default is not supported for array declarations",
                            port.name
                        ),
                        0,
                        0,
                    ));
                }
                if port.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{kind} '{}' range is not supported for array declarations",
                            port.name
                        ),
                        0,
                        0,
                    ));
                }
                errors.push(Diagnostic::semantic(
                    format!(
                        "{kind} '{}' uses unresolved generic array element type '{}'",
                        port.name, elem
                    ),
                    0,
                    0,
                ));
                let size_context = format!("{kind} '{}' array size", port.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                let offset = flat.len();
                arrays.insert(
                    port.name.clone(),
                    TypedArrayInfo {
                        elem_ty: PrimitiveType::F32,
                        len,
                        offset,
                    },
                );
                for idx in 0..len {
                    let slot_name = format!("{}[{idx}]", port.name);
                    flat.push(slot_name.clone());
                    types.insert(slot_name, PrimitiveType::F32);
                }
            }
            Some(DeclType::Array { elem, size }) => {
                if port.default.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{kind} '{}' default is not supported for array declarations",
                            port.name
                        ),
                        0,
                        0,
                    ));
                }
                if port.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{kind} '{}' range is not supported for array declarations",
                            port.name
                        ),
                        0,
                        0,
                    ));
                }
                let size_context = format!("{kind} '{}' array size", port.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                let offset = flat.len();
                arrays.insert(
                    port.name.clone(),
                    TypedArrayInfo {
                        elem_ty: *elem,
                        len,
                        offset,
                    },
                );
                for idx in 0..len {
                    let slot_name = format!("{}[{idx}]", port.name);
                    flat.push(slot_name.clone());
                    types.insert(slot_name, *elem);
                }
            }
        }
    }

    (flat, types, arrays, defaults, ranges)
}

fn is_builtin_constant_name(name: &str) -> bool {
    matches!(
        name,
        "PI" | "TWO_PI" | "TWOPI" | "SAMPLE_RATE" | "SR" | "BLOCK_SIZE"
    )
}

fn is_builtin_function_name(name: &str) -> bool {
    matches!(
        name,
        "sin"
            | "cos"
            | "tan"
            | "tanh"
            | "atan"
            | "atan2"
            | "exp"
            | "log"
            | "sqrt"
            | "pow"
            | "abs"
            | "fabs"
            | "floor"
            | "ceil"
            | "round"
            | "trunc"
            | "min"
            | "max"
            | "fma"
            | "unsafe_read"
            | "unsafe_write"
    )
}

fn is_builtin_unsafe_data_fn(name: &str) -> bool {
    matches!(name, "unsafe_read" | "unsafe_write")
}

fn is_internal_buffer_2d_fn(name: &str) -> bool {
    matches!(name, INTERNAL_BUFFER_READ2_FN | INTERNAL_BUFFER_WRITE2_FN)
}

fn split_instance_method_path(name: &str) -> Option<(&str, &str)> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method.is_empty() {
        return None;
    }
    Some((base, method))
}

fn parse_data_len_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == "len" {
        Some(base)
    } else {
        None
    }
}

fn parse_buffer_chans_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == "chans" {
        Some(base)
    } else {
        None
    }
}

fn builtin_arity(func: BuiltinFn) -> usize {
    match func {
        BuiltinFn::Sin
        | BuiltinFn::Cos
        | BuiltinFn::Tan
        | BuiltinFn::Tanh
        | BuiltinFn::Atan
        | BuiltinFn::Exp
        | BuiltinFn::Log
        | BuiltinFn::Sqrt
        | BuiltinFn::Abs
        | BuiltinFn::Floor
        | BuiltinFn::Ceil
        | BuiltinFn::Round
        | BuiltinFn::Trunc => 1,
        BuiltinFn::Pow | BuiltinFn::Atan2 | BuiltinFn::Min | BuiltinFn::Max => 2,
        BuiltinFn::Fma => 3,
    }
}

fn builtin_name(func: BuiltinFn) -> &'static str {
    match func {
        BuiltinFn::Sin => "sin",
        BuiltinFn::Cos => "cos",
        BuiltinFn::Tan => "tan",
        BuiltinFn::Tanh => "tanh",
        BuiltinFn::Atan => "atan",
        BuiltinFn::Atan2 => "atan2",
        BuiltinFn::Exp => "exp",
        BuiltinFn::Log => "log",
        BuiltinFn::Sqrt => "sqrt",
        BuiltinFn::Pow => "pow",
        BuiltinFn::Abs => "abs",
        BuiltinFn::Floor => "floor",
        BuiltinFn::Ceil => "ceil",
        BuiltinFn::Round => "round",
        BuiltinFn::Trunc => "trunc",
        BuiltinFn::Min => "min",
        BuiltinFn::Max => "max",
        BuiltinFn::Fma => "fma",
    }
}

fn is_float_type(ty: PrimitiveType) -> bool {
    matches!(ty, PrimitiveType::F32 | PrimitiveType::F64)
}

fn builtin_constant_value_f64(name: &str, options: AnalysisOptions) -> Option<f64> {
    match name {
        "PI" => Some(std::f32::consts::PI as f64),
        "TWO_PI" | "TWOPI" => Some((2.0 * std::f32::consts::PI) as f64),
        "SAMPLE_RATE" | "SR" => Some(options.sample_rate as f64),
        "BLOCK_SIZE" => Some(options.block_size as f64),
        _ => None,
    }
}

fn eval_const_expr_f64(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<f64> {
    match expr {
        Expr::Number(v) => Some(*v as f64),
        Expr::Int(v) => Some(*v as f64),
        Expr::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
        Expr::Var(name) => {
            if let Some(value) = builtin_constant_value_f64(name, options) {
                Some(value)
            } else {
                errors.push(Diagnostic::semantic(
                    format!("{context} uses non-constant symbol '{name}'"),
                    0,
                    0,
                ));
                None
            }
        }
        Expr::Cast { to, expr } => {
            let value = eval_const_expr_f64(expr, options, context, errors)?;
            Some(match to {
                PrimitiveType::F32 | PrimitiveType::F64 => value,
                PrimitiveType::I32 => (value as i32) as f64,
                PrimitiveType::I64 => (value as i64) as f64,
                PrimitiveType::Bool => {
                    if value != 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            })
        }
        Expr::UnaryNot { expr } => {
            let value = eval_const_expr_f64(expr, options, context, errors)?;
            Some(if value == 0.0 { 1.0 } else { 0.0 })
        }
        Expr::Logical { op, lhs, rhs } => {
            let lhs_value = eval_const_expr_f64(lhs, options, context, errors)?;
            match op {
                omni_frontend::LogicalOp::And => {
                    if lhs_value == 0.0 {
                        Some(0.0)
                    } else {
                        let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
                        Some(if rhs_value != 0.0 { 1.0 } else { 0.0 })
                    }
                }
                omni_frontend::LogicalOp::Or => {
                    if lhs_value != 0.0 {
                        Some(1.0)
                    } else {
                        let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
                        Some(if rhs_value != 0.0 { 1.0 } else { 0.0 })
                    }
                }
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let lhs_value = eval_const_expr_f64(lhs, options, context, errors)?;
            let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
            Some(match op {
                omni_frontend::BinaryOp::Add => lhs_value + rhs_value,
                omni_frontend::BinaryOp::Sub => lhs_value - rhs_value,
                omni_frontend::BinaryOp::Mul => lhs_value * rhs_value,
                omni_frontend::BinaryOp::Div => lhs_value / rhs_value,
                omni_frontend::BinaryOp::Mod => lhs_value % rhs_value,
            })
        }
        Expr::Compare { op, lhs, rhs } => {
            let lhs_value = eval_const_expr_f64(lhs, options, context, errors)?;
            let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
            let pred = match op {
                CmpOp::Eq => lhs_value == rhs_value,
                CmpOp::Ne => lhs_value != rhs_value,
                CmpOp::Lt => lhs_value < rhs_value,
                CmpOp::Le => lhs_value <= rhs_value,
                CmpOp::Gt => lhs_value > rhs_value,
                CmpOp::Ge => lhs_value >= rhs_value,
            };
            Some(if pred { 1.0 } else { 0.0 })
        }
        _ => {
            errors.push(Diagnostic::semantic(
                format!("{context} must be a compile-time constant expression"),
                0,
                0,
            ));
            None
        }
    }
}

fn eval_data_size_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let value = eval_const_expr_f64(expr, options, context, errors)?;
    if !value.is_finite() {
        errors.push(Diagnostic::semantic(
            format!("{context} must evaluate to a finite numeric value"),
            0,
            0,
        ));
        return None;
    }

    let truncated = value.trunc();
    if (value - truncated).abs() > 1e-6 {
        errors.push(Diagnostic::semantic(
            format!("{context} must evaluate to an integer value"),
            0,
            0,
        ));
        return None;
    }
    if truncated <= 0.0 {
        errors.push(Diagnostic::semantic(
            format!("{context} must be greater than zero"),
            0,
            0,
        ));
        return None;
    }
    if truncated > usize::MAX as f64 {
        errors.push(Diagnostic::semantic(
            format!("{context} exceeds supported range"),
            0,
            0,
        ));
        return None;
    }

    Some(truncated as usize)
}

fn validate_data_struct_layout(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<StructDataLayoutField>> {
    let mut stack = Vec::<String>::new();
    validate_data_struct_layout_inner(struct_name, struct_defs, context, errors, &mut stack)
}

fn validate_data_struct_layout_inner(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
    stack: &mut Vec<String>,
) -> Option<Vec<StructDataLayoutField>> {
    if stack.iter().any(|s| s == struct_name) {
        let mut cycle = stack.join(" -> ");
        if !cycle.is_empty() {
            cycle.push_str(" -> ");
        }
        cycle.push_str(struct_name);
        errors.push(Diagnostic::semantic(
            format!("{context} contains recursive Data[Struct, N] cycle: {cycle}"),
            0,
            0,
        ));
        return None;
    }
    let fields = struct_defs.get(struct_name).cloned();
    let Some(fields) = fields else {
        errors.push(Diagnostic::semantic(
            format!("{context} references unknown struct '{struct_name}'"),
            0,
            0,
        ));
        return None;
    };

    stack.push(struct_name.to_owned());
    let mut layout = Vec::new();
    for field in fields {
        match field.ty {
            TypedFieldType::Scalar(prim) => layout.push(StructDataLayoutField {
                name: field.name,
                kind: StructDataLayoutKind::Scalar(prim),
            }),
            TypedFieldType::Data(len) => {
                if let Some(elem_struct) = &field.data_elem_struct {
                    let nested_context = format!(
                        "{context} nested Data field '{}.{}'",
                        struct_name, field.name
                    );
                    if validate_data_struct_layout_inner(
                        elem_struct,
                        struct_defs,
                        &nested_context,
                        errors,
                        stack,
                    )
                    .is_none()
                    {
                        stack.pop();
                        return None;
                    }
                }
                layout.push(StructDataLayoutField {
                    name: field.name,
                    kind: StructDataLayoutKind::Data {
                        len,
                        elem_ty: field.data_elem_ty,
                        elem_struct: field.data_elem_struct.clone(),
                    },
                });
            }
        }
    }
    stack.pop();
    Some(layout)
}

fn register_data_struct_root(
    base: &str,
    struct_name: &str,
    len: usize,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &mut HashMap<String, usize>,
    state_data_struct_roots: &mut HashMap<String, DataStructRootInfo>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if validate_data_struct_layout(struct_name, struct_defs, context, errors).is_none() {
        return false;
    }
    let mut stack = Vec::<String>::new();
    register_data_struct_root_inner(
        base,
        struct_name,
        len,
        struct_defs,
        context,
        state_scalars,
        state_data,
        state_data_struct_roots,
        errors,
        &mut stack,
    )
}

#[allow(clippy::too_many_arguments)]
fn register_data_struct_root_inner(
    base: &str,
    struct_name: &str,
    len: usize,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &mut HashMap<String, usize>,
    state_data_struct_roots: &mut HashMap<String, DataStructRootInfo>,
    errors: &mut Vec<Diagnostic>,
    stack: &mut Vec<String>,
) -> bool {
    if stack.iter().any(|s| s == struct_name) {
        let mut cycle = stack.join(" -> ");
        if !cycle.is_empty() {
            cycle.push_str(" -> ");
        }
        cycle.push_str(struct_name);
        errors.push(Diagnostic::semantic(
            format!("{context} contains recursive Data[Struct, N] cycle: {cycle}"),
            0,
            0,
        ));
        return false;
    }
    let Some(fields) = struct_defs.get(struct_name).cloned() else {
        errors.push(Diagnostic::semantic(
            format!("{context} references unknown struct '{struct_name}'"),
            0,
            0,
        ));
        return false;
    };
    state_data_struct_roots
        .entry(base.to_owned())
        .or_insert(DataStructRootInfo {
            struct_name: struct_name.to_owned(),
            len,
        });

    stack.push(struct_name.to_owned());
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                state_scalars.insert(
                    declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                    prim,
                );
                state_data.entry(flat).or_insert(len);
            }
            TypedFieldType::Data(field_len) => {
                let nested_len = len.saturating_mul(field_len);
                if let Some(elem_struct) = &field.data_elem_struct {
                    let nested_context = format!(
                        "{context} nested Data field '{}.{}'",
                        struct_name, field.name
                    );
                    if !register_data_struct_root_inner(
                        &flat,
                        elem_struct,
                        nested_len,
                        struct_defs,
                        &nested_context,
                        state_scalars,
                        state_data,
                        state_data_struct_roots,
                        errors,
                        stack,
                    ) {
                        stack.pop();
                        return false;
                    }
                } else {
                    let elem_ty = field.data_elem_ty.unwrap_or(PrimitiveType::F32);
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        elem_ty,
                    );
                    state_data.entry(flat).or_insert(nested_len);
                }
            }
        }
    }
    stack.pop();
    true
}

fn add_struct_element_alias_bindings(
    alias_name: &str,
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_data_aliases: &mut HashMap<String, LocalDataAliasInfo>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some(layout) = validate_data_struct_layout(struct_name, struct_defs, context, errors)
    else {
        return false;
    };
    for field in layout {
        match field.kind {
            StructDataLayoutKind::Scalar(prim) => {
                let alias = format!("{alias_name}.{}", field.name);
                local_aliases.insert(alias.clone(), prim);
                known_scalars.insert(alias);
            }
            StructDataLayoutKind::Data {
                len,
                elem_ty,
                elem_struct,
            } => {
                local_data_aliases.insert(
                    format!("{alias_name}.{}", field.name),
                    LocalDataAliasInfo {
                        len,
                        elem_ty: elem_ty.unwrap_or(PrimitiveType::F32),
                        elem_struct,
                        writable: true,
                    },
                );
            }
        }
    }
    true
}

#[derive(Debug, Clone)]
struct FnSignature {
    params: Vec<String>,
    defaults: Vec<Option<Expr>>,
    param_types: Vec<Option<FnParamType>>,
    type_params: Vec<String>,
}

#[derive(Clone, Copy)]
struct ExprEnv<'a> {
    known_scalars: &'a HashSet<String>,
    locals: &'a HashSet<String>,
    outputs: &'a HashSet<String>,
    data_vars: &'a HashMap<String, usize>,
    param_structs: &'a HashMap<String, String>,
    struct_instances: &'a HashMap<String, String>,
    struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &'a HashMap<String, FnSignature>,
    allow_data_ctor: bool,
    scope: ScopeKind,
}

fn parent_namespace(ns: &str) -> Option<&str> {
    ns.rsplit_once("::").map(|(parent, _)| parent)
}

fn namespace_of_symbol(name: &str) -> String {
    let base = name.split('.').next().unwrap_or(name);
    base.rsplit_once("::")
        .map(|(ns, _)| ns.to_owned())
        .unwrap_or_default()
}

fn namespace_candidates(current_ns: &str) -> Vec<String> {
    if current_ns.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut cur = Some(current_ns);
    while let Some(ns) = cur {
        out.push(ns.to_owned());
        cur = parent_namespace(ns);
    }
    out.push(String::new());
    out
}

fn join_namespace(ns: &str, leaf: &str) -> String {
    if ns.is_empty() {
        leaf.to_owned()
    } else {
        format!("{ns}::{leaf}")
    }
}

fn collect_declared_namespaces(symbols: &HashSet<String>) -> HashSet<String> {
    let mut out = HashSet::new();
    for symbol in symbols {
        let mut ns = namespace_of_symbol(symbol);
        while !ns.is_empty() {
            out.insert(ns.clone());
            ns = parent_namespace(&ns).unwrap_or_default().to_owned();
        }
    }
    out
}

fn primitive_sig_code_for_specialization(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

fn specialized_struct_name(base: &str, type_args: &[PrimitiveType]) -> String {
    if type_args.is_empty() {
        return base.to_owned();
    }
    let sig = type_args
        .iter()
        .map(|t| primitive_sig_code_for_specialization(*t))
        .collect::<Vec<_>>()
        .join("_");
    format!("{base}.__gen__{sig}")
}

fn resolve_explicit_call_type_args(
    type_args: &[CallTypeArg],
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut resolved = Vec::<PrimitiveType>::with_capacity(type_args.len());
    for arg in type_args {
        match arg {
            CallTypeArg::Primitive(ty) => resolved.push(*ty),
            CallTypeArg::Generic(name) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context}: generic type argument '{}' is not allowed here; expected concrete primitive type",
                        name
                    ),
                    0,
                    0,
                ));
                return None;
            }
        }
    }
    Some(resolved)
}

fn substitute_call_type_args_with_bindings_expr(
    expr: &mut Expr,
    bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => {
            substitute_call_type_args_with_bindings_expr(index, bindings, context, errors);
        }
        Expr::DataCtor { spec, init } => {
            substitute_call_type_args_with_bindings_expr(&mut spec.size, bindings, context, errors);
            if let Some(values) = init {
                for value in values {
                    substitute_call_type_args_with_bindings_expr(value, bindings, context, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            substitute_call_type_args_with_bindings_expr(lhs, bindings, context, errors);
            substitute_call_type_args_with_bindings_expr(rhs, bindings, context, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                substitute_call_type_args_with_bindings_expr(arg, bindings, context, errors);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            substitute_call_type_args_with_bindings_expr(inner, bindings, context, errors);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                substitute_call_type_args_with_bindings_expr(value, bindings, context, errors);
            }
        }
        Expr::UserCall {
            type_args, args, ..
        } => {
            for arg in args {
                substitute_call_type_args_with_bindings_expr(
                    &mut arg.expr,
                    bindings,
                    context,
                    errors,
                );
            }
            for type_arg in type_args.iter_mut() {
                if let CallTypeArg::Generic(param) = type_arg {
                    let Some(bound) = bindings.get(param).copied() else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{context}: unknown generic type argument '{}'; not declared in current generic owner",
                                param
                            ),
                            0,
                            0,
                        ));
                        continue;
                    };
                    *type_arg = CallTypeArg::Primitive(bound);
                }
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn substitute_call_type_args_with_bindings_stmt(
    stmt: &mut Stmt,
    bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                substitute_call_type_args_with_bindings_expr(index, bindings, context, errors);
            }
            substitute_call_type_args_with_bindings_expr(expr, bindings, context, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            substitute_call_type_args_with_bindings_expr(expr, bindings, context, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            substitute_call_type_args_with_bindings_expr(cond, bindings, context, errors);
            for nested in then_branch {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
            for nested in else_branch {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            substitute_call_type_args_with_bindings_expr(start, bindings, context, errors);
            substitute_call_type_args_with_bindings_expr(end, bindings, context, errors);
            for nested in body {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
        }
    });
}

fn specialize_generic_struct_template(
    template: &StructDef,
    type_args: &[PrimitiveType],
    errors: &mut Vec<Diagnostic>,
) -> Option<StructDef> {
    if type_args.len() != template.type_params.len() {
        errors.push(Diagnostic::semantic(
            format!(
                "struct '{}' expects {} type arguments, got {}",
                template.name,
                template.type_params.len(),
                type_args.len()
            ),
            0,
            0,
        ));
        return None;
    }

    let mut type_bindings = HashMap::<String, PrimitiveType>::new();
    for (param, ty) in template.type_params.iter().zip(type_args.iter()) {
        type_bindings.insert(param.clone(), *ty);
    }

    let specialize_fn_param_type = |ty: &FnParamType| -> FnParamType {
        match ty {
            FnParamType::Primitive(prim) => FnParamType::Primitive(*prim),
            FnParamType::Struct(name) => match type_bindings.get(name).copied() {
                Some(bound) => FnParamType::Primitive(bound),
                None => FnParamType::Struct(name.clone()),
            },
            FnParamType::Buffer(buffer_ty) => {
                let elem = match &buffer_ty.elem {
                    BufferElemType::Primitive(prim) => BufferElemType::Primitive(*prim),
                    BufferElemType::Generic(param) => match type_bindings.get(param).copied() {
                        Some(bound) => BufferElemType::Primitive(bound),
                        None => BufferElemType::Generic(param.clone()),
                    },
                };
                FnParamType::Buffer(BufferType {
                    elem,
                    channels: buffer_ty.channels.clone(),
                })
            }
        }
    };

    let mut fields = Vec::<StructField>::new();
    for field in &template.fields {
        let mut default = field.default.clone();
        if let Some(expr) = &mut default {
            rewrite_generic_data_ctor_expr_types(expr, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                expr,
                &type_bindings,
                &format!("struct '{}'", template.name),
                errors,
            );
        }
        let specialized_ty = match &field.ty {
            FieldType::Scalar(prim) => FieldType::Scalar(*prim),
            FieldType::Generic(param) => {
                let bound = match type_bindings.get(param).copied() {
                    Some(bound) => bound,
                    None => {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "struct '{}.{}' references unknown generic type parameter '{}'",
                                template.name, field.name, param
                            ),
                            0,
                            0,
                        ));
                        PrimitiveType::F32
                    }
                };
                FieldType::Scalar(bound)
            }
            FieldType::Data(spec) => {
                let elem = match &spec.elem {
                    DataElemType::Primitive(prim) => DataElemType::Primitive(*prim),
                    DataElemType::Struct(elem) => match type_bindings.get(elem).copied() {
                        Some(bound) => DataElemType::Primitive(bound),
                        None => DataElemType::Struct(elem.clone()),
                    },
                };
                FieldType::Data(omni_frontend::DataTypeSpec {
                    elem,
                    size: spec.size.clone(),
                })
            }
        };
        fields.push(StructField {
            name: field.name.clone(),
            ty: specialized_ty,
            default,
        });
    }
    let mut methods = template.methods.clone();
    for method in &mut methods {
        for param in &mut method.params {
            if let Some(ty) = &param.ty {
                param.ty = Some(specialize_fn_param_type(ty));
            }
            if let Some(default) = &mut param.default {
                rewrite_generic_data_ctor_expr_types(default, &type_bindings);
                substitute_call_type_args_with_bindings_expr(
                    default,
                    &type_bindings,
                    &format!(
                        "struct '{}.{}' parameter default",
                        template.name, method.name
                    ),
                    errors,
                );
            }
        }
        for stmt in &mut method.body {
            rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &format!("struct '{}.{}' method body", template.name, method.name),
                errors,
            );
        }
    }

    Some(StructDef {
        name: specialized_struct_name(&template.name, type_args),
        type_params: Vec::new(),
        fields,
        methods,
    })
}

#[derive(Debug, Clone, Default)]
struct GenericInferenceLocals {
    scalar_types: HashMap<String, PrimitiveType>,
    array_elem_types: HashMap<String, PrimitiveType>,
}

fn add_decl_type_to_generic_inference_locals(
    name: &str,
    ty: Option<&DeclType>,
    locals: &mut GenericInferenceLocals,
) {
    match ty {
        Some(DeclType::Scalar(prim)) => {
            locals.scalar_types.entry(name.to_owned()).or_insert(*prim);
        }
        Some(DeclType::Array { elem, .. }) => {
            locals
                .array_elem_types
                .entry(name.to_owned())
                .or_insert(*elem);
        }
        Some(DeclType::Generic(_)) | Some(DeclType::ArrayGeneric { .. }) => {}
        None => {
            locals
                .scalar_types
                .entry(name.to_owned())
                .or_insert(PrimitiveType::F32);
        }
    }
}

fn generic_inference_seed_for_processor(proc: &ProcessorDef) -> GenericInferenceLocals {
    let mut locals = GenericInferenceLocals::default();
    for input in &proc.ins {
        add_decl_type_to_generic_inference_locals(&input.name, input.ty.as_ref(), &mut locals);
    }
    for output in &proc.outs {
        add_decl_type_to_generic_inference_locals(&output.name, output.ty.as_ref(), &mut locals);
    }
    for param in &proc.params {
        add_decl_type_to_generic_inference_locals(&param.name, param.ty.as_ref(), &mut locals);
    }
    locals
}

fn generic_inference_seed_for_top_level(blocks: &[Block]) -> GenericInferenceLocals {
    let mut locals = GenericInferenceLocals::default();
    for block in blocks {
        match block {
            Block::Ins(ports) | Block::Outs(ports) => {
                for port in ports {
                    add_decl_type_to_generic_inference_locals(
                        &port.name,
                        port.ty.as_ref(),
                        &mut locals,
                    );
                }
            }
            Block::Params(params) => {
                for param in params {
                    add_decl_type_to_generic_inference_locals(
                        &param.name,
                        param.ty.as_ref(),
                        &mut locals,
                    );
                }
            }
            _ => {}
        }
    }
    locals
}

fn update_generic_inference_locals_from_assign(
    target: &AssignTarget,
    decl_ty: Option<PrimitiveType>,
    expr: &Expr,
    locals: &mut GenericInferenceLocals,
) {
    match target {
        AssignTarget::Var(name) => {
            if locals.scalar_types.contains_key(name) || locals.array_elem_types.contains_key(name)
            {
                return;
            }
            if let Some(declared) = decl_ty {
                locals.scalar_types.insert(name.clone(), declared);
                return;
            }
            if let Some(elem_ty) = infer_array_elem_type_for_generic_binding(
                expr,
                &locals.scalar_types,
                &locals.array_elem_types,
            ) {
                locals.array_elem_types.insert(name.clone(), elem_ty);
                return;
            }
            if let Some(scalar_ty) = infer_scalar_type_for_generic_binding(
                expr,
                &locals.scalar_types,
                &locals.array_elem_types,
            ) {
                locals.scalar_types.insert(name.clone(), scalar_ty);
            }
        }
        AssignTarget::Index { base, .. } => {
            if locals.array_elem_types.contains_key(base) {
                return;
            }
            if let Some(elem_ty) = infer_scalar_type_for_generic_binding(
                expr,
                &locals.scalar_types,
                &locals.array_elem_types,
            ) {
                locals.array_elem_types.insert(base.clone(), elem_ty);
            }
        }
    }
}

fn rewrite_generic_struct_ctor_expr(
    expr: &mut Expr,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
) {
    match expr {
        Expr::Index { index, .. } => {
            rewrite_generic_struct_ctor_expr(index, templates, generated, errors, locals);
        }
        Expr::DataCtor { spec, init } => {
            rewrite_generic_struct_ctor_expr(&mut spec.size, templates, generated, errors, locals);
            if let Some(values) = init {
                for value in values {
                    rewrite_generic_struct_ctor_expr(value, templates, generated, errors, locals);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_generic_struct_ctor_expr(lhs, templates, generated, errors, locals);
            rewrite_generic_struct_ctor_expr(rhs, templates, generated, errors, locals);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_generic_struct_ctor_expr(arg, templates, generated, errors, locals);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_generic_struct_ctor_expr(inner, templates, generated, errors, locals);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_generic_struct_ctor_expr(value, templates, generated, errors, locals);
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            for arg in args.iter_mut() {
                rewrite_generic_struct_ctor_expr(
                    &mut arg.expr,
                    templates,
                    generated,
                    errors,
                    locals,
                );
            }
            if let Some(template) = templates.get(name) {
                let type_args_to_use = if type_args.is_empty() {
                    infer_generic_struct_ctor_type_args(
                        template,
                        args,
                        &locals.scalar_types,
                        &locals.array_elem_types,
                        errors,
                    )
                } else {
                    resolve_explicit_call_type_args(
                        type_args,
                        &format!("struct constructor '{}'", name),
                        errors,
                    )
                };
                let Some(type_args_to_use) = type_args_to_use else {
                    return;
                };
                let Some(specialized) =
                    specialize_generic_struct_template(template, &type_args_to_use, errors)
                else {
                    return;
                };
                let specialized_name = specialized.name.clone();
                generated
                    .entry(specialized_name.clone())
                    .or_insert(specialized);
                *name = specialized_name;
                type_args.clear();
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn rewrite_generic_struct_ctor_stmt(
    stmt: &mut Stmt,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            expr,
            ..
        } => {
            if let AssignTarget::Index { index, .. } = target {
                rewrite_generic_struct_ctor_expr(index, templates, generated, errors, locals);
            }
            rewrite_generic_struct_ctor_expr(expr, templates, generated, errors, locals);
            update_generic_inference_locals_from_assign(target, *decl_ty, expr, locals);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_generic_struct_ctor_expr(expr, templates, generated, errors, locals);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_generic_struct_ctor_expr(cond, templates, generated, errors, locals);
            let mut then_locals = locals.clone();
            for nested in then_branch {
                rewrite_generic_struct_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut then_locals,
                );
            }
            let mut else_locals = locals.clone();
            for nested in else_branch {
                rewrite_generic_struct_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut else_locals,
                );
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            rewrite_generic_struct_ctor_expr(start, templates, generated, errors, locals);
            rewrite_generic_struct_ctor_expr(end, templates, generated, errors, locals);
            let mut body_locals = locals.clone();
            for nested in body {
                rewrite_generic_struct_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut body_locals,
                );
            }
        }
    });
}

fn rewrite_generic_struct_ctor_stmt_list(
    stmts: &mut [Stmt],
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut locals = GenericInferenceLocals::default();
    for stmt in stmts {
        rewrite_generic_struct_ctor_stmt(stmt, templates, generated, errors, &mut locals);
    }
}

fn infer_scalar_type_for_generic_binding(
    expr: &Expr,
    scalar_locals: &HashMap<String, PrimitiveType>,
    array_elem_locals: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    let mut locals = scalar_locals.clone();
    for (name, ty) in array_elem_locals {
        locals.entry(name.clone()).or_insert(*ty);
    }
    infer_expr_type_for_def_return_inference(expr, &locals, &HashMap::new())
}

fn infer_array_elem_type_for_generic_binding(
    expr: &Expr,
    scalar_locals: &HashMap<String, PrimitiveType>,
    array_elem_locals: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::ArrayLiteral(values) => {
            let mut acc = None::<PrimitiveType>;
            for value in values {
                let ty =
                    infer_scalar_type_for_generic_binding(value, scalar_locals, array_elem_locals)?;
                acc = Some(match acc {
                    Some(existing) => merge_inferred_return_types(existing, ty)?,
                    None => ty,
                });
            }
            acc
        }
        Expr::Var(name) => array_elem_locals.get(name).copied(),
        Expr::DataCtor { spec, .. } => match &spec.elem {
            DataElemType::Primitive(ty) => Some(*ty),
            DataElemType::Struct(_) => None,
        },
        _ => None,
    }
}

fn bind_inferred_generic_type(
    bindings: &mut HashMap<String, PrimitiveType>,
    type_param: &str,
    inferred: PrimitiveType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(existing) = bindings.get(type_param).copied() {
        if existing == inferred {
            return;
        }
        if let Some(merged) = merge_inferred_return_types(existing, inferred) {
            bindings.insert(type_param.to_owned(), merged);
        } else {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: conflicting inferred types {:?} and {:?} for generic parameter '{}'",
                    existing, inferred, type_param
                ),
                0,
                0,
            ));
        }
    } else {
        bindings.insert(type_param.to_owned(), inferred);
    }
}

fn finalize_inferred_generic_type_args(
    owner_name: &str,
    owner_kind: &str,
    type_params: &[String],
    bindings: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut out = Vec::<PrimitiveType>::with_capacity(type_params.len());
    for param in type_params {
        let Some(bound) = bindings.get(param).copied() else {
            errors.push(Diagnostic::semantic(
                format!(
                    "cannot infer generic type parameter '{}' for {} '{}' constructor; provide explicit type arguments",
                    param, owner_kind, owner_name
                ),
                0,
                0,
            ));
            return None;
        };
        out.push(bound);
    }
    Some(out)
}

fn infer_generic_struct_ctor_type_args(
    template: &StructDef,
    args: &[CallArg],
    scalar_locals: &HashMap<String, PrimitiveType>,
    array_elem_locals: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let scalar_fields = template
        .fields
        .iter()
        .filter(|f| !matches!(f.ty, FieldType::Data(_)))
        .collect::<Vec<_>>();
    let param_names = scalar_fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<Vec<_>>();
    let defaults = scalar_fields
        .iter()
        .map(|f| f.default.clone().or(Some(Expr::Number(0.0))))
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        args,
        &param_names,
        &defaults,
        false,
        false,
        &format!("struct constructor '{}'", template.name),
        errors,
    );

    let mut bindings = HashMap::<String, PrimitiveType>::new();
    for (idx, field) in scalar_fields.iter().enumerate() {
        let Some(expr) = resolved.get(idx).and_then(|arg| *arg).or_else(|| {
            defaults
                .get(idx)
                .and_then(|default_expr| default_expr.as_ref())
        }) else {
            continue;
        };
        if let FieldType::Generic(type_param) = &field.ty {
            if let Some(inferred) =
                infer_scalar_type_for_generic_binding(expr, scalar_locals, array_elem_locals)
            {
                bind_inferred_generic_type(
                    &mut bindings,
                    type_param,
                    inferred,
                    &format!("struct constructor '{}'", template.name),
                    errors,
                );
            }
        }
    }
    finalize_inferred_generic_type_args(
        &template.name,
        "struct",
        &template.type_params,
        &bindings,
        errors,
    )
}

fn infer_generic_proc_ctor_type_args(
    template: &ProcessorDef,
    args: &[CallArg],
    scalar_locals: &HashMap<String, PrimitiveType>,
    array_elem_locals: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut ctor_param_names = template
        .params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    ctor_param_names.extend(template.buffers.iter().map(|b| b.name.clone()));
    let mut ctor_defaults = template
        .params
        .iter()
        .map(|p| p.default.clone())
        .collect::<Vec<_>>();
    ctor_defaults.extend((0..template.buffers.len()).map(|_| None));

    let resolved = resolve_call_args(
        args,
        &ctor_param_names,
        &ctor_defaults,
        false,
        false,
        &format!("processor constructor '{}'", template.name),
        errors,
    );

    let mut bindings = HashMap::<String, PrimitiveType>::new();
    for (idx, param) in template.params.iter().enumerate() {
        let Some(expr) = resolved.get(idx).and_then(|arg| *arg).or_else(|| {
            ctor_defaults
                .get(idx)
                .and_then(|default_expr| default_expr.as_ref())
        }) else {
            continue;
        };
        if let Some(param_ty) = &param.ty {
            match param_ty {
                DeclType::Generic(type_param) => {
                    if let Some(inferred) = infer_scalar_type_for_generic_binding(
                        expr,
                        scalar_locals,
                        array_elem_locals,
                    ) {
                        bind_inferred_generic_type(
                            &mut bindings,
                            type_param,
                            inferred,
                            &format!("processor constructor '{}'", template.name),
                            errors,
                        );
                    }
                }
                DeclType::ArrayGeneric { elem, .. } => {
                    if let Some(inferred) = infer_array_elem_type_for_generic_binding(
                        expr,
                        scalar_locals,
                        array_elem_locals,
                    ) {
                        bind_inferred_generic_type(
                            &mut bindings,
                            elem,
                            inferred,
                            &format!("processor constructor '{}'", template.name),
                            errors,
                        );
                    }
                }
                DeclType::Scalar(_) | DeclType::Array { .. } => {}
            }
        }
    }
    finalize_inferred_generic_type_args(
        &template.name,
        "processor",
        &template.type_params,
        &bindings,
        errors,
    )
}

fn finalize_generated_generic_struct_specializations(
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut processed = HashSet::<String>::new();
    loop {
        let names = generated.keys().cloned().collect::<Vec<_>>();
        let mut progressed = false;
        for name in names {
            if processed.contains(&name) {
                continue;
            }
            let Some(mut spec) = generated.remove(&name) else {
                continue;
            };
            for field in &mut spec.fields {
                if let Some(default) = &mut field.default {
                    let mut locals = GenericInferenceLocals::default();
                    rewrite_generic_struct_ctor_expr(
                        default,
                        templates,
                        generated,
                        errors,
                        &mut locals,
                    );
                }
            }
            for method in &mut spec.methods {
                rewrite_generic_struct_ctor_stmt_list(
                    &mut method.body,
                    templates,
                    generated,
                    errors,
                );
            }
            generated.insert(name.clone(), spec);
            processed.insert(name);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
}

fn specialize_generic_proc_decl_type(
    ty: &DeclType,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    symbol_kind: &str,
    symbol_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> DeclType {
    match ty {
        DeclType::Scalar(prim) => DeclType::Scalar(*prim),
        DeclType::Generic(param) => match type_bindings.get(param).copied() {
            Some(bound) => DeclType::Scalar(bound),
            None => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' {} '{}' references unknown generic type parameter '{}'",
                        proc_name, symbol_kind, symbol_name, param
                    ),
                    0,
                    0,
                ));
                DeclType::Scalar(PrimitiveType::F32)
            }
        },
        DeclType::ArrayGeneric { elem, size } => match type_bindings.get(elem).copied() {
            Some(bound) => DeclType::Array {
                elem: bound,
                size: size.clone(),
            },
            None => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' {} '{}' references unknown generic array element type '{}'",
                        proc_name, symbol_kind, symbol_name, elem
                    ),
                    0,
                    0,
                ));
                DeclType::Array {
                    elem: PrimitiveType::F32,
                    size: size.clone(),
                }
            }
        },
        DeclType::Array { elem, size } => DeclType::Array {
            elem: *elem,
            size: size.clone(),
        },
    }
}

fn specialize_generic_proc_buffer_type(
    ty: &BufferType,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    buffer_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> BufferType {
    let elem = match &ty.elem {
        BufferElemType::Primitive(prim) => BufferElemType::Primitive(*prim),
        BufferElemType::Generic(param) => match type_bindings.get(param).copied() {
            Some(bound) => BufferElemType::Primitive(bound),
            None => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' buffer '{}' references unknown generic element type '{}'",
                        proc_name, buffer_name, param
                    ),
                    0,
                    0,
                ));
                BufferElemType::Primitive(PrimitiveType::F32)
            }
        },
    };
    BufferType {
        elem,
        channels: ty.channels.clone(),
    }
}

fn rewrite_generic_data_ctor_expr_types(
    expr: &mut Expr,
    type_bindings: &HashMap<String, PrimitiveType>,
) {
    match expr {
        Expr::Index { index, .. } => {
            rewrite_generic_data_ctor_expr_types(index, type_bindings);
        }
        Expr::DataCtor { spec, init } => {
            if let DataElemType::Struct(param) = &spec.elem {
                if let Some(bound) = type_bindings.get(param).copied() {
                    spec.elem = DataElemType::Primitive(bound);
                }
            }
            rewrite_generic_data_ctor_expr_types(&mut spec.size, type_bindings);
            if let Some(values) = init {
                for value in values {
                    rewrite_generic_data_ctor_expr_types(value, type_bindings);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_generic_data_ctor_expr_types(lhs, type_bindings);
            rewrite_generic_data_ctor_expr_types(rhs, type_bindings);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_generic_data_ctor_expr_types(arg, type_bindings);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_generic_data_ctor_expr_types(&mut arg.expr, type_bindings);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_generic_data_ctor_expr_types(inner, type_bindings);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_generic_data_ctor_expr_types(value, type_bindings);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn rewrite_generic_data_ctor_stmt_types(
    stmt: &mut Stmt,
    type_bindings: &HashMap<String, PrimitiveType>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                rewrite_generic_data_ctor_expr_types(index, type_bindings);
            }
            rewrite_generic_data_ctor_expr_types(expr, type_bindings);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_generic_data_ctor_expr_types(expr, type_bindings);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_generic_data_ctor_expr_types(cond, type_bindings);
            for nested in then_branch {
                rewrite_generic_data_ctor_stmt_types(nested, type_bindings);
            }
            for nested in else_branch {
                rewrite_generic_data_ctor_stmt_types(nested, type_bindings);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            rewrite_generic_data_ctor_expr_types(start, type_bindings);
            rewrite_generic_data_ctor_expr_types(end, type_bindings);
            for nested in body {
                rewrite_generic_data_ctor_stmt_types(nested, type_bindings);
            }
        }
    });
}

fn specialize_generic_init_typed_decls(
    stmt: &mut Stmt,
    type_bindings: &HashMap<String, PrimitiveType>,
    proc_name: &str,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            ..
        } => {
            let Some(param) = generic_decl_ty.clone() else {
                return;
            };
            let AssignTarget::Var(name) = target else {
                errors.push(Diagnostic::semantic(
                    "typed declaration is only supported for plain scalar variables",
                    0,
                    0,
                ));
                *generic_decl_ty = None;
                return;
            };
            if decl_ty.is_some() {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' init declaration '{}: {}' cannot combine primitive and generic type annotations",
                        proc_name, name, param
                    ),
                    0,
                    0,
                ));
                *generic_decl_ty = None;
                return;
            }
            match type_bindings.get(&param).copied() {
                Some(bound) => {
                    *decl_ty = Some(bound);
                    *generic_decl_ty = None;
                    *is_typed_decl = true;
                }
                None => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{}' init declaration '{}: {}' references unknown generic type parameter '{}'",
                            proc_name, name, param, param
                        ),
                        0,
                        0,
                    ));
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                specialize_generic_init_typed_decls(nested, type_bindings, proc_name, errors);
            }
            for nested in else_branch {
                specialize_generic_init_typed_decls(nested, type_bindings, proc_name, errors);
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                specialize_generic_init_typed_decls(nested, type_bindings, proc_name, errors);
            }
        }
        Stmt::Expr { .. } | Stmt::Return { .. } => {}
    });
}

fn expand_inline_data_ctor_initializers(stmts: &mut Vec<Stmt>) {
    let mut expanded = Vec::<Stmt>::new();
    for mut stmt in std::mem::take(stmts) {
        match &mut stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                expand_inline_data_ctor_initializers(then_branch);
                expand_inline_data_ctor_initializers(else_branch);
            }
            Stmt::For { body, .. } => {
                expand_inline_data_ctor_initializers(body);
            }
            _ => {}
        }

        let mut index_writes = Vec::<Stmt>::new();
        if let Stmt::Assign {
            loc,
            target: AssignTarget::Var(base),
            expr: Expr::DataCtor { init, .. },
            ..
        } = &mut stmt
        {
            if let Some(values) = init.take() {
                for (idx, value) in values.into_iter().enumerate() {
                    index_writes.push(Stmt::Assign {
                        loc: loc.clone(),
                        target: AssignTarget::Index {
                            base: base.clone(),
                            index: Expr::Int(idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        expr: value,
                    });
                }
            }
        }

        expanded.push(stmt);
        expanded.extend(index_writes);
    }
    *stmts = expanded;
}

fn specialize_generic_proc_template(
    template: &ProcessorDef,
    type_args: &[PrimitiveType],
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcessorDef> {
    if type_args.len() != template.type_params.len() {
        errors.push(Diagnostic::semantic(
            format!(
                "processor '{}' expects {} type arguments, got {}",
                template.name,
                template.type_params.len(),
                type_args.len()
            ),
            0,
            0,
        ));
        return None;
    }

    let mut type_bindings = HashMap::<String, PrimitiveType>::new();
    for (param, ty) in template.type_params.iter().zip(type_args.iter()) {
        type_bindings.insert(param.clone(), *ty);
    }

    let mut ins = template
        .ins
        .iter()
        .map(|decl| PortDecl {
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "input",
                    &decl.name,
                    errors,
                )
            }),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    let mut outs = template
        .outs
        .iter()
        .map(|decl| PortDecl {
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "output",
                    &decl.name,
                    errors,
                )
            }),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    let mut params = template
        .params
        .iter()
        .map(|decl| ParamDecl {
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_decl_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    "param",
                    &decl.name,
                    errors,
                )
            }),
            default: decl.default.clone(),
            range: decl.range.clone(),
        })
        .collect::<Vec<_>>();
    for input in &mut ins {
        if let Some(default) = &mut input.default {
            rewrite_generic_data_ctor_expr_types(default, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                default,
                &type_bindings,
                &format!("processor '{}' input default", template.name),
                errors,
            );
        }
        if let Some(range) = &mut input.range {
            if let Some(min) = &mut range.min {
                rewrite_generic_data_ctor_expr_types(min, &type_bindings);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' input range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_data_ctor_expr_types(&mut range.max, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                &mut range.max,
                &type_bindings,
                &format!("processor '{}' input range maximum", template.name),
                errors,
            );
        }
    }
    for output in &mut outs {
        if let Some(range) = &mut output.range {
            if let Some(min) = &mut range.min {
                rewrite_generic_data_ctor_expr_types(min, &type_bindings);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' output range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_data_ctor_expr_types(&mut range.max, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                &mut range.max,
                &type_bindings,
                &format!("processor '{}' output range maximum", template.name),
                errors,
            );
        }
    }
    for param in &mut params {
        if let Some(default) = &mut param.default {
            rewrite_generic_data_ctor_expr_types(default, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                default,
                &type_bindings,
                &format!("processor '{}' parameter default", template.name),
                errors,
            );
        }
        if let Some(range) = &mut param.range {
            if let Some(min) = &mut range.min {
                rewrite_generic_data_ctor_expr_types(min, &type_bindings);
                substitute_call_type_args_with_bindings_expr(
                    min,
                    &type_bindings,
                    &format!("processor '{}' parameter range minimum", template.name),
                    errors,
                );
            }
            rewrite_generic_data_ctor_expr_types(&mut range.max, &type_bindings);
            substitute_call_type_args_with_bindings_expr(
                &mut range.max,
                &type_bindings,
                &format!("processor '{}' parameter range maximum", template.name),
                errors,
            );
        }
    }
    let buffers = template
        .buffers
        .iter()
        .map(|decl| BufferDecl {
            name: decl.name.clone(),
            ty: decl.ty.as_ref().map(|ty| {
                specialize_generic_proc_buffer_type(
                    ty,
                    &type_bindings,
                    &template.name,
                    &decl.name,
                    errors,
                )
            }),
        })
        .collect::<Vec<_>>();
    let mut init = template.init.clone();
    let mut block_pre = template.block_pre.clone();
    let mut sample = template.sample.clone();
    let mut block_post = template.block_post.clone();
    for stmt in &mut init {
        specialize_generic_init_typed_decls(stmt, &type_bindings, &template.name, errors);
    }
    for stmt in &mut init {
        rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' init", template.name),
            errors,
        );
    }
    for stmt in &mut block_pre {
        rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' block-pre", template.name),
            errors,
        );
    }
    for stmt in &mut sample {
        rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' sample", template.name),
            errors,
        );
    }
    for stmt in &mut block_post {
        rewrite_generic_data_ctor_stmt_types(stmt, &type_bindings);
        substitute_call_type_args_with_bindings_stmt(
            stmt,
            &type_bindings,
            &format!("processor '{}' block-post", template.name),
            errors,
        );
    }
    expand_inline_data_ctor_initializers(&mut init);
    expand_inline_data_ctor_initializers(&mut block_pre);
    expand_inline_data_ctor_initializers(&mut sample);
    expand_inline_data_ctor_initializers(&mut block_post);

    Some(ProcessorDef {
        name: specialized_struct_name(&template.name, type_args),
        type_params: Vec::new(),
        ins,
        outs,
        params,
        buffers,
        has_init_block: template.has_init_block,
        has_block_block: template.has_block_block,
        has_sample_block: template.has_sample_block,
        init,
        block_pre,
        sample,
        block_post,
    })
}

fn resolve_generic_proc_template_name(
    name: &str,
    current_ns: &str,
    templates: &HashMap<String, ProcessorDef>,
) -> Option<String> {
    if templates.contains_key(name) {
        return Some(name.to_owned());
    }
    if name.contains("::") {
        return None;
    }
    let symbols = templates.keys().cloned().collect::<HashSet<_>>();
    resolve_unqualified_symbol_name(name, current_ns, &symbols)
}

fn rewrite_generic_proc_ctor_expr(
    expr: &mut Expr,
    templates: &HashMap<String, ProcessorDef>,
    generated: &mut HashMap<String, ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
    current_ns: &str,
) {
    match expr {
        Expr::Index { index, .. } => {
            rewrite_generic_proc_ctor_expr(index, templates, generated, errors, locals, current_ns);
        }
        Expr::DataCtor { spec, init } => {
            rewrite_generic_proc_ctor_expr(
                &mut spec.size,
                templates,
                generated,
                errors,
                locals,
                current_ns,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_generic_proc_ctor_expr(
                        value, templates, generated, errors, locals, current_ns,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_generic_proc_ctor_expr(lhs, templates, generated, errors, locals, current_ns);
            rewrite_generic_proc_ctor_expr(rhs, templates, generated, errors, locals, current_ns);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_generic_proc_ctor_expr(
                    arg, templates, generated, errors, locals, current_ns,
                );
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_generic_proc_ctor_expr(inner, templates, generated, errors, locals, current_ns);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_generic_proc_ctor_expr(
                    value, templates, generated, errors, locals, current_ns,
                );
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            for arg in args.iter_mut() {
                rewrite_generic_proc_ctor_expr(
                    &mut arg.expr,
                    templates,
                    generated,
                    errors,
                    locals,
                    current_ns,
                );
            }
            let resolved_name = if templates.contains_key(name) {
                Some(name.clone())
            } else if name.contains("::") {
                None
            } else {
                resolve_generic_proc_template_name(name, current_ns, templates)
            };
            if let Some(resolved_name) = resolved_name {
                if *name != resolved_name {
                    *name = resolved_name.clone();
                }
            }
            if let Some(template) = templates.get(name) {
                let type_args_to_use = if type_args.is_empty() {
                    infer_generic_proc_ctor_type_args(
                        template,
                        args,
                        &locals.scalar_types,
                        &locals.array_elem_types,
                        errors,
                    )
                } else {
                    resolve_explicit_call_type_args(
                        type_args,
                        &format!("processor constructor '{}'", name),
                        errors,
                    )
                };
                let Some(type_args_to_use) = type_args_to_use else {
                    return;
                };
                let Some(specialized) =
                    specialize_generic_proc_template(template, &type_args_to_use, errors)
                else {
                    return;
                };
                let specialized_name = specialized.name.clone();
                generated
                    .entry(specialized_name.clone())
                    .or_insert(specialized);
                *name = specialized_name;
                type_args.clear();
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn rewrite_generic_proc_ctor_stmt(
    stmt: &mut Stmt,
    templates: &HashMap<String, ProcessorDef>,
    generated: &mut HashMap<String, ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
    current_ns: &str,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            expr,
            ..
        } => {
            if let AssignTarget::Index { index, .. } = target {
                rewrite_generic_proc_ctor_expr(
                    index, templates, generated, errors, locals, current_ns,
                );
            }
            rewrite_generic_proc_ctor_expr(expr, templates, generated, errors, locals, current_ns);
            update_generic_inference_locals_from_assign(target, *decl_ty, expr, locals);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_generic_proc_ctor_expr(expr, templates, generated, errors, locals, current_ns);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_generic_proc_ctor_expr(cond, templates, generated, errors, locals, current_ns);
            let mut then_locals = locals.clone();
            for nested in then_branch {
                rewrite_generic_proc_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut then_locals,
                    current_ns,
                );
            }
            let mut else_locals = locals.clone();
            for nested in else_branch {
                rewrite_generic_proc_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut else_locals,
                    current_ns,
                );
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            rewrite_generic_proc_ctor_expr(start, templates, generated, errors, locals, current_ns);
            rewrite_generic_proc_ctor_expr(end, templates, generated, errors, locals, current_ns);
            let mut body_locals = locals.clone();
            for nested in body {
                rewrite_generic_proc_ctor_stmt(
                    nested,
                    templates,
                    generated,
                    errors,
                    &mut body_locals,
                    current_ns,
                );
            }
        }
    });
}

fn rewrite_generic_proc_ctor_stmt_list(
    stmts: &mut [Stmt],
    templates: &HashMap<String, ProcessorDef>,
    generated: &mut HashMap<String, ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
    seed_locals: &GenericInferenceLocals,
    current_ns: &str,
) {
    let mut locals = seed_locals.clone();
    for stmt in stmts {
        rewrite_generic_proc_ctor_stmt(stmt, templates, generated, errors, &mut locals, current_ns);
    }
}

fn finalize_generated_generic_proc_specializations(
    templates: &HashMap<String, ProcessorDef>,
    generated: &mut HashMap<String, ProcessorDef>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut processed = HashSet::<String>::new();
    loop {
        let names = generated.keys().cloned().collect::<Vec<_>>();
        let mut progressed = false;
        for name in names {
            if processed.contains(&name) {
                continue;
            }
            let Some(mut spec) = generated.remove(&name) else {
                continue;
            };
            let spec_ns = namespace_of_symbol(&spec.name);
            let spec_seed = generic_inference_seed_for_processor(&spec);
            rewrite_generic_proc_ctor_stmt_list(
                &mut spec.init,
                templates,
                generated,
                errors,
                &spec_seed,
                &spec_ns,
            );
            rewrite_generic_proc_ctor_stmt_list(
                &mut spec.block_pre,
                templates,
                generated,
                errors,
                &spec_seed,
                &spec_ns,
            );
            rewrite_generic_proc_ctor_stmt_list(
                &mut spec.sample,
                templates,
                generated,
                errors,
                &spec_seed,
                &spec_ns,
            );
            rewrite_generic_proc_ctor_stmt_list(
                &mut spec.block_post,
                templates,
                generated,
                errors,
                &spec_seed,
                &spec_ns,
            );
            generated.insert(name.clone(), spec);
            processed.insert(name);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
}

fn resolve_qualified_symbol_name(
    name: &str,
    symbols: &HashSet<String>,
    namespaces: &HashSet<String>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    if symbols.contains(name) {
        return Some(name.to_owned());
    }
    if let Some((ns, symbol)) = name.rsplit_once("::") {
        if !namespaces.contains(ns) {
            errors.push(Diagnostic::semantic(
                format!("{context}: unknown namespace '{ns}' in symbol '{name}'"),
                0,
                0,
            ));
        } else {
            errors.push(Diagnostic::semantic(
                format!("{context}: unknown symbol '{symbol}' in namespace '{ns}'"),
                0,
                0,
            ));
        }
    } else {
        errors.push(Diagnostic::semantic(
            format!("{context}: unknown symbol '{name}'"),
            0,
            0,
        ));
    }
    None
}

fn resolve_unqualified_symbol_name(
    name: &str,
    current_ns: &str,
    symbols: &HashSet<String>,
) -> Option<String> {
    for ns in namespace_candidates(current_ns) {
        let candidate = join_namespace(&ns, name);
        if symbols.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn qualify_struct_type_name(
    ty_name: &mut String,
    current_ns: &str,
    struct_symbols: &HashSet<String>,
    struct_namespaces: &HashSet<String>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if ty_name.contains("::") {
        if let Some(resolved) = resolve_qualified_symbol_name(
            ty_name,
            struct_symbols,
            struct_namespaces,
            context,
            errors,
        ) {
            *ty_name = resolved;
        }
        return;
    }
    if let Some(resolved) = resolve_unqualified_symbol_name(ty_name, current_ns, struct_symbols) {
        *ty_name = resolved;
    }
}

fn qualify_expr_namespaced_symbols(
    expr: &mut Expr,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
    callable_namespaces: &HashSet<String>,
    struct_symbols: &HashSet<String>,
    struct_namespaces: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
    context: &str,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                qualify_expr_namespaced_symbols(
                    &mut arg.expr,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
            if is_builtin_function_name(name)
                || is_internal_buffer_2d_fn(name)
                || name.contains('.')
            {
                return;
            }
            if name.contains("::") {
                if let Some(resolved) = resolve_qualified_symbol_name(
                    name,
                    callable_symbols,
                    callable_namespaces,
                    context,
                    errors,
                ) {
                    *name = resolved;
                }
                return;
            }
            if let Some(resolved) =
                resolve_unqualified_symbol_name(name, current_ns, callable_symbols)
            {
                *name = resolved;
            }
        }
        Expr::DataCtor { spec, init } => {
            if let DataElemType::Struct(name) = &mut spec.elem {
                qualify_struct_type_name(
                    name,
                    current_ns,
                    struct_symbols,
                    struct_namespaces,
                    context,
                    errors,
                );
            }
            qualify_expr_namespaced_symbols(
                &mut spec.size,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            if let Some(values) = init {
                for value in values {
                    qualify_expr_namespaced_symbols(
                        value,
                        current_ns,
                        callable_symbols,
                        callable_namespaces,
                        struct_symbols,
                        struct_namespaces,
                        errors,
                        context,
                    );
                }
            }
        }
        Expr::Index { index, .. } => qualify_expr_namespaced_symbols(
            index,
            current_ns,
            callable_symbols,
            callable_namespaces,
            struct_symbols,
            struct_namespaces,
            errors,
            context,
        ),
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            qualify_expr_namespaced_symbols(
                lhs,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            qualify_expr_namespaced_symbols(
                rhs,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                qualify_expr_namespaced_symbols(
                    arg,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
        }
        Expr::Cast { expr: arg, .. } | Expr::UnaryNot { expr: arg } => {
            qualify_expr_namespaced_symbols(
                arg,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                qualify_expr_namespaced_symbols(
                    value,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn qualify_stmt_namespaced_symbols(
    stmt: &mut Stmt,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
    callable_namespaces: &HashSet<String>,
    struct_symbols: &HashSet<String>,
    struct_namespaces: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
    context: &str,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                qualify_expr_namespaced_symbols(
                    index,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
            qualify_expr_namespaced_symbols(
                expr,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => qualify_expr_namespaced_symbols(
            expr,
            current_ns,
            callable_symbols,
            callable_namespaces,
            struct_symbols,
            struct_namespaces,
            errors,
            context,
        ),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            qualify_expr_namespaced_symbols(
                cond,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            for nested in then_branch {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
            for nested in else_branch {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            qualify_expr_namespaced_symbols(
                start,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            qualify_expr_namespaced_symbols(
                end,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            for nested in body {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
        }
    });
}

#[derive(Debug, Clone)]
struct ProcApi {
    ins: Vec<ProcPortSpec>,
    params: HashMap<String, ProcParamSlotSpec>,
    outs: Vec<String>,
    buffers: Vec<ProcBufferSpec>,
    has_block: bool,
}

#[derive(Debug, Clone)]
struct ProcPortSpec {
    name: String,
    slots: Vec<String>,
    defaults: Vec<Option<Expr>>,
    ranges: Vec<Option<TypedValueRange>>,
}

#[derive(Debug, Clone)]
struct ProcParamSlotSpec {
    name: String,
    ty: PrimitiveType,
    default: Option<Expr>,
    range: Option<TypedValueRange>,
}

#[derive(Debug, Clone)]
struct ProcParamSpec {
    name: String,
    slots: Vec<ProcParamSlotSpec>,
}

#[derive(Debug, Clone)]
struct ProcBufferSpec {
    name: String,
    elem_ty: PrimitiveType,
    channels: TypedBufferChannels,
}

#[derive(Debug, Clone)]
struct ProcCallInstance {
    proc_name: String,
    buffer_args: Vec<Expr>,
}

#[derive(Default, Debug, Clone)]
struct ProcStateFields {
    scalars: HashMap<String, PrimitiveType>,
    data: HashMap<String, omni_frontend::DataTypeSpec>,
    nested_procs: HashMap<String, ProcNestedState>,
    struct_instances: HashMap<String, ProcStructState>,
}

#[derive(Debug, Clone)]
struct ProcNestedState {
    proc_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcStructState {
    struct_name: String,
    type_args: Vec<PrimitiveType>,
}

fn is_plain_symbol(name: &str) -> bool {
    !name.contains('.')
}

fn record_proc_state_scalar_assignment(
    name: &str,
    decl_ty: Option<PrimitiveType>,
    expr: &Expr,
    out: &mut ProcStateFields,
    state_type_hints: &HashMap<String, PrimitiveType>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_primary_output_types: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) {
    if out.data.contains_key(name)
        || out.struct_instances.contains_key(name)
        || out.nested_procs.contains_key(name)
    {
        errors.push(Diagnostic::semantic(
            format!("processor state symbol '{name}' is used as both scalar and non-scalar value"),
            0,
            0,
        ));
    }
    let existing = out.scalars.get(name).copied();
    let mut inference_scalars = state_type_hints.clone();
    for (state_name, state_ty) in &out.scalars {
        inference_scalars.insert(state_name.clone(), *state_ty);
    }
    for (instance, nested) in &out.nested_procs {
        if let Some(ty) = proc_primary_output_types.get(&nested.proc_name).copied() {
            inference_scalars.insert(
                declared_type_key(DECLARED_FUNCTION_RETURN_TYPE_PREFIX, instance),
                ty,
            );
        }
    }
    let struct_instances = out
        .struct_instances
        .iter()
        .map(|(instance_name, state)| (instance_name.clone(), state.struct_name.clone()))
        .collect::<HashMap<_, _>>();
    let ty = match (existing, decl_ty) {
        (Some(existing_ty), Some(declared_ty)) => {
            if existing_ty != declared_ty {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor state symbol '{name}' has conflicting types {:?} and {:?}",
                        existing_ty, declared_ty
                    ),
                    0,
                    0,
                ));
            }
            existing_ty
        }
        (Some(existing_ty), None) => existing_ty,
        (None, Some(declared_ty)) => declared_ty,
        (None, None) => infer_proc_state_scalar_type(
            expr,
            &inference_scalars,
            input_names,
            output_names,
            param_names,
            &struct_instances,
            struct_defs,
            errors,
        )
        .unwrap_or(PrimitiveType::F32),
    };
    if let Some(existing_ty) = existing {
        if existing_ty != ty {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor state symbol '{name}' has conflicting types {:?} and {:?}",
                    existing_ty, ty
                ),
                0,
                0,
            ));
        }
    } else {
        out.scalars.insert(name.to_owned(), ty);
    }
}

fn collect_proc_state_fields(
    stmt: &Stmt,
    reserved: &HashSet<String>,
    current_ns: &str,
    proc_symbols: &HashSet<String>,
    state_type_hints: &HashMap<String, PrimitiveType>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    typed_struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_primary_output_types: &HashMap<String, PrimitiveType>,
    struct_symbols: &HashSet<String>,
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    ctor_symbols: &HashSet<String>,
    in_init_scope: bool,
    out: &mut ProcStateFields,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            is_typed_decl: _,
            expr,
            ..
        } => {
            if let AssignTarget::Var(name) = target {
                if is_plain_symbol(name)
                    && !reserved.contains(name)
                    && !is_builtin_constant_name(name)
                {
                    match expr {
                        Expr::DataCtor { spec, .. } => {
                            if out.scalars.contains_key(name)
                                || out.struct_instances.contains_key(name)
                            {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "processor state symbol '{name}' is used as both Data and non-Data value"
                                    ),
                                    0,
                                    0,
                                ));
                            }
                            if out.nested_procs.contains_key(name) {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "processor state symbol '{name}' is used as both Data and processor instance"
                                    ),
                                    0,
                                    0,
                                ));
                            }
                            out.data.entry(name.clone()).or_insert_with(|| spec.clone());
                        }
                        Expr::UserCall {
                            name: ctor,
                            type_args,
                            args,
                        } => {
                            let mut handled_as_constructor = false;
                            let resolved_proc_ctor = if ctor.contains("::") {
                                if proc_symbols.contains(ctor) {
                                    Some(ctor.clone())
                                } else {
                                    None
                                }
                            } else {
                                resolve_unqualified_symbol_name(ctor, current_ns, proc_symbols)
                            };
                            if let Some(proc_ctor) = resolved_proc_ctor {
                                handled_as_constructor = true;
                                if !type_args.is_empty() {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "processor '{}' is not generic and cannot take type arguments",
                                            proc_ctor
                                        ),
                                        0,
                                        0,
                                    ));
                                } else if let Some(existing) = out.nested_procs.get(name) {
                                    if existing.proc_name != proc_ctor {
                                        errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor state symbol '{name}' has conflicting processor types '{}' and '{}'",
                                                existing.proc_name, proc_ctor
                                            ),
                                            0,
                                            0,
                                        ));
                                    }
                                } else {
                                    if !in_init_scope {
                                        errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor state constructor '{name} = {proc_ctor}(...)' is only allowed in processor init block"
                                            ),
                                            0,
                                            0,
                                        ));
                                    }
                                    if out.scalars.contains_key(name)
                                        || out.data.contains_key(name)
                                        || out.struct_instances.contains_key(name)
                                    {
                                        errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor state symbol '{name}' is used as both processor instance and non-processor value"
                                            ),
                                            0,
                                            0,
                                        ));
                                    } else {
                                        out.nested_procs.insert(
                                            name.clone(),
                                            ProcNestedState {
                                                proc_name: proc_ctor,
                                            },
                                        );
                                    }
                                }
                            } else {
                                let resolved_struct_ctor = if ctor.contains("::") {
                                    if struct_symbols.contains(ctor) {
                                        Some(ctor.clone())
                                    } else {
                                        None
                                    }
                                } else {
                                    resolve_unqualified_symbol_name(
                                        ctor,
                                        current_ns,
                                        struct_symbols,
                                    )
                                };
                                if let Some(struct_ctor) = resolved_struct_ctor {
                                    handled_as_constructor = true;
                                    if !in_init_scope {
                                        errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor state constructor '{name} = {struct_ctor}(...)' is only allowed in processor init block"
                                            ),
                                            0,
                                            0,
                                        ));
                                    }
                                    let resolved_type_args = match struct_defs.get(&struct_ctor) {
                                        Some(struct_template) => {
                                            if type_args.is_empty() {
                                                if !struct_template.type_params.is_empty() {
                                                    infer_generic_struct_ctor_type_args(
                                                        struct_template,
                                                        args,
                                                        &out.scalars,
                                                        &HashMap::new(),
                                                        errors,
                                                    )
                                                } else {
                                                    Some(Vec::new())
                                                }
                                            } else if struct_template.type_params.is_empty() {
                                                errors.push(Diagnostic::semantic(
                                                    format!(
                                                        "struct '{}' is not generic and cannot take type arguments",
                                                        struct_ctor
                                                    ),
                                                    0,
                                                    0,
                                                ));
                                                None
                                            } else if type_args.len()
                                                != struct_template.type_params.len()
                                            {
                                                errors.push(Diagnostic::semantic(
                                                    format!(
                                                        "struct '{}' expects {} type arguments, got {}",
                                                        struct_ctor,
                                                        struct_template.type_params.len(),
                                                        type_args.len()
                                                    ),
                                                    0,
                                                    0,
                                                ));
                                                None
                                            } else {
                                                resolve_explicit_call_type_args(
                                                    type_args,
                                                    &format!(
                                                        "struct constructor '{}'",
                                                        struct_ctor
                                                    ),
                                                    errors,
                                                )
                                            }
                                        }
                                        None => {
                                            errors.push(Diagnostic::semantic(
                                                format!("unknown struct '{}'", struct_ctor),
                                                0,
                                                0,
                                            ));
                                            None
                                        }
                                    };
                                    if let Some(resolved_type_args) = resolved_type_args {
                                        if out.scalars.contains_key(name)
                                            || out.data.contains_key(name)
                                            || out.nested_procs.contains_key(name)
                                        {
                                            errors.push(Diagnostic::semantic(
                                                format!(
                                                    "processor state symbol '{name}' is used as both struct instance and non-struct value"
                                                ),
                                                0,
                                                0,
                                            ));
                                        } else if let Some(existing) =
                                            out.struct_instances.get(name)
                                        {
                                            let current = ProcStructState {
                                                struct_name: struct_ctor.clone(),
                                                type_args: resolved_type_args.clone(),
                                            };
                                            if existing != &current {
                                                errors.push(Diagnostic::semantic(
                                                    format!(
                                                        "processor state symbol '{name}' has conflicting struct constructor specializations"
                                                    ),
                                                    0,
                                                    0,
                                                ));
                                            }
                                        } else {
                                            out.struct_instances.insert(
                                                name.clone(),
                                                ProcStructState {
                                                    struct_name: struct_ctor,
                                                    type_args: resolved_type_args,
                                                },
                                            );
                                        }
                                    }
                                } else if ctor_symbols.contains(ctor) {
                                    handled_as_constructor = true;
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "processor state constructor '{name} = {ctor}(...)' is only supported for known struct or processor constructors"
                                        ),
                                        0,
                                        0,
                                    ));
                                }
                            }
                            if !handled_as_constructor {
                                record_proc_state_scalar_assignment(
                                    name,
                                    *decl_ty,
                                    expr,
                                    out,
                                    state_type_hints,
                                    input_names,
                                    output_names,
                                    param_names,
                                    typed_struct_defs,
                                    proc_primary_output_types,
                                    errors,
                                );
                            }
                        }
                        _ => {
                            record_proc_state_scalar_assignment(
                                name,
                                *decl_ty,
                                expr,
                                out,
                                state_type_hints,
                                input_names,
                                output_names,
                                param_names,
                                typed_struct_defs,
                                proc_primary_output_types,
                                errors,
                            );
                        }
                    }
                }
            }
            collect_proc_state_expr_fields(expr, reserved, ctor_symbols, out, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_proc_state_expr_fields(expr, reserved, ctor_symbols, out, errors)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_proc_state_expr_fields(cond, reserved, ctor_symbols, out, errors);
            for s in then_branch {
                collect_proc_state_fields(
                    s,
                    reserved,
                    current_ns,
                    proc_symbols,
                    state_type_hints,
                    input_names,
                    output_names,
                    param_names,
                    typed_struct_defs,
                    proc_primary_output_types,
                    struct_symbols,
                    struct_defs,
                    ctor_symbols,
                    in_init_scope,
                    out,
                    errors,
                );
            }
            for s in else_branch {
                collect_proc_state_fields(
                    s,
                    reserved,
                    current_ns,
                    proc_symbols,
                    state_type_hints,
                    input_names,
                    output_names,
                    param_names,
                    typed_struct_defs,
                    proc_primary_output_types,
                    struct_symbols,
                    struct_defs,
                    ctor_symbols,
                    in_init_scope,
                    out,
                    errors,
                );
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            collect_proc_state_expr_fields(start, reserved, ctor_symbols, out, errors);
            collect_proc_state_expr_fields(end, reserved, ctor_symbols, out, errors);
            for s in body {
                collect_proc_state_fields(
                    s,
                    reserved,
                    current_ns,
                    proc_symbols,
                    state_type_hints,
                    input_names,
                    output_names,
                    param_names,
                    typed_struct_defs,
                    proc_primary_output_types,
                    struct_symbols,
                    struct_defs,
                    ctor_symbols,
                    in_init_scope,
                    out,
                    errors,
                );
            }
        }
    }
}

fn infer_proc_state_scalar_type(
    expr: &Expr,
    known_scalars: &HashMap<String, PrimitiveType>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    let locals = HashSet::<String>::new();
    let local_data_aliases = HashMap::<String, LocalDataAliasInfo>::new();
    infer_scalar_expr_type(
        expr,
        known_scalars,
        &local_data_aliases,
        &locals,
        input_names,
        output_names,
        param_names,
        struct_instances,
        struct_defs,
        errors,
    )
}

fn collect_proc_state_expr_fields(
    expr: &Expr,
    reserved: &HashSet<String>,
    ctor_symbols: &HashSet<String>,
    out: &mut ProcStateFields,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => {
            collect_proc_state_expr_fields(index, reserved, ctor_symbols, out, errors);
        }
        Expr::DataCtor { spec, .. } => {
            collect_proc_state_expr_fields(&spec.size, reserved, ctor_symbols, out, errors);
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_proc_state_expr_fields(lhs, reserved, ctor_symbols, out, errors);
            collect_proc_state_expr_fields(rhs, reserved, ctor_symbols, out, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_proc_state_expr_fields(arg, reserved, ctor_symbols, out, errors);
            }
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                collect_proc_state_expr_fields(value, reserved, ctor_symbols, out, errors);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            collect_proc_state_expr_fields(inner, reserved, ctor_symbols, out, errors);
        }
        Expr::UserCall { name, args, .. } => {
            if name.starts_with(PROC_INDEX_SENTINEL_PREFIX) {
                // Processor instance validation is handled after proc desugaring/rewrite.
            }
            for arg in args {
                collect_proc_state_expr_fields(&arg.expr, reserved, ctor_symbols, out, errors);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn rewrite_proc_expr_symbols(
    expr: &mut Expr,
    owner_proc: &str,
    field_names: &HashSet<String>,
    field_array_slots: &HashMap<String, Vec<String>>,
    in_array_slots: &HashMap<String, Vec<String>>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Var(name) => {
            if field_names.contains(name) && is_plain_symbol(name) {
                *name = format!("self.{name}");
            }
        }
        Expr::Index { base, index } => {
            rewrite_proc_expr_symbols(
                index,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
            if let Some(slots) = field_array_slots.get(base.as_str()) {
                if slots.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!("processor array field '{base}' has zero slots"),
                        0,
                        0,
                    ));
                    return;
                }
                if let Some(raw_idx) = try_constant_index_i64(index) {
                    let Some(slot_idx) = resolve_proc_constant_slot_index(
                        raw_idx,
                        slots.len(),
                        &format!("processor array field '{base}'"),
                        errors,
                    ) else {
                        return;
                    };
                    if let Some(slot_name) = slots.get(slot_idx) {
                        *expr = Expr::Var(format!("self.{slot_name}"));
                    }
                } else if slots.len() == 1 {
                    if let Some(slot_name) = slots.first() {
                        *expr = Expr::Var(format!("self.{slot_name}"));
                    }
                } else {
                    let mut args = Vec::<CallArg>::new();
                    args.push(CallArg {
                        name: None,
                        expr: *index.clone(),
                    });
                    for slot in slots {
                        args.push(CallArg {
                            name: None,
                            expr: Expr::Var(format!("self.{slot}")),
                        });
                    }
                    *expr = Expr::UserCall {
                        name: proc_read_helper_name(owner_proc, slots.len(), false),
                        type_args: Vec::new(),
                        args,
                    };
                }
                return;
            }
            if let Some(slots) = in_array_slots.get(base.as_str()) {
                if slots.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!("processor input array '{base}' has zero slots"),
                        0,
                        0,
                    ));
                    return;
                }
                if let Some(raw_idx) = try_constant_index_i64(index) {
                    let Some(slot_idx) = resolve_proc_constant_slot_index(
                        raw_idx,
                        slots.len(),
                        &format!("processor input array '{base}'"),
                        errors,
                    ) else {
                        return;
                    };
                    if let Some(slot_name) = slots.get(slot_idx) {
                        *expr = Expr::Var(slot_name.clone());
                    }
                } else if slots.len() == 1 {
                    if let Some(slot_name) = slots.first() {
                        *expr = Expr::Var(slot_name.clone());
                    }
                } else {
                    let mut args = Vec::<CallArg>::new();
                    args.push(CallArg {
                        name: None,
                        expr: *index.clone(),
                    });
                    for slot in slots {
                        args.push(CallArg {
                            name: None,
                            expr: Expr::Var(slot.clone()),
                        });
                    }
                    *expr = Expr::UserCall {
                        name: proc_read_helper_name(owner_proc, slots.len(), false),
                        type_args: Vec::new(),
                        args,
                    };
                }
                return;
            }
            if field_names.contains(base) && is_plain_symbol(base) {
                *base = format!("self.{base}");
            }
        }
        Expr::DataCtor { spec, init } => {
            rewrite_proc_expr_symbols(
                &mut spec.size,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_proc_expr_symbols(
                        value,
                        owner_proc,
                        field_names,
                        field_array_slots,
                        in_array_slots,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_proc_expr_symbols(
                lhs,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
            rewrite_proc_expr_symbols(
                rhs,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_proc_expr_symbols(
                    arg,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_proc_expr_symbols(
                    &mut arg.expr,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
            }
            if let Expr::UserCall { name, args, .. } = expr {
                if let Some(base) = parse_data_len_instance_base(name) {
                    if field_names.contains(base) && is_plain_symbol(base) {
                        *name = format!("self.{base}.len");
                    }
                } else if let Some(base) = parse_buffer_chans_instance_base(name) {
                    if field_names.contains(base) && is_plain_symbol(base) {
                        *name = format!("self.{base}.chans");
                    }
                }
                if name == "unsafe_read" && args.len() == 2 {
                    if let Expr::Var(base) = &args[0].expr {
                        if let Some(slots) = field_array_slots.get(base.as_str()) {
                            if slots.is_empty() {
                                errors.push(Diagnostic::semantic(
                                    format!("processor array field '{base}' has zero slots"),
                                    0,
                                    0,
                                ));
                                return;
                            }
                            let idx_expr = args[1].expr.clone();
                            if let Some(raw_idx) = try_constant_index_i64(&idx_expr) {
                                if raw_idx >= 0 && raw_idx < slots.len() as i64 {
                                    let slot_idx = raw_idx as usize;
                                    if let Some(slot_name) = slots.get(slot_idx) {
                                        *expr = Expr::Var(format!("self.{slot_name}"));
                                    }
                                } else {
                                    let mut call_args = Vec::<CallArg>::new();
                                    call_args.push(CallArg {
                                        name: None,
                                        expr: idx_expr,
                                    });
                                    for slot in slots {
                                        call_args.push(CallArg {
                                            name: None,
                                            expr: Expr::Var(format!("self.{slot}")),
                                        });
                                    }
                                    *expr = Expr::UserCall {
                                        name: proc_read_helper_name(owner_proc, slots.len(), true),
                                        type_args: Vec::new(),
                                        args: call_args,
                                    };
                                }
                            } else if slots.len() == 1 {
                                if let Some(slot_name) = slots.first() {
                                    *expr = Expr::Var(format!("self.{slot_name}"));
                                }
                            } else {
                                let mut call_args = Vec::<CallArg>::new();
                                call_args.push(CallArg {
                                    name: None,
                                    expr: idx_expr,
                                });
                                for slot in slots {
                                    call_args.push(CallArg {
                                        name: None,
                                        expr: Expr::Var(format!("self.{slot}")),
                                    });
                                }
                                *expr = Expr::UserCall {
                                    name: proc_read_helper_name(owner_proc, slots.len(), true),
                                    type_args: Vec::new(),
                                    args: call_args,
                                };
                            }
                            return;
                        }
                        if let Some(slots) = in_array_slots.get(base.as_str()) {
                            if slots.is_empty() {
                                errors.push(Diagnostic::semantic(
                                    format!("processor input array '{base}' has zero slots"),
                                    0,
                                    0,
                                ));
                                return;
                            }
                            let idx_expr = args[1].expr.clone();
                            if let Some(raw_idx) = try_constant_index_i64(&idx_expr) {
                                if raw_idx >= 0 && raw_idx < slots.len() as i64 {
                                    let slot_idx = raw_idx as usize;
                                    if let Some(slot_name) = slots.get(slot_idx) {
                                        *expr = Expr::Var(slot_name.clone());
                                    }
                                } else {
                                    let mut call_args = Vec::<CallArg>::new();
                                    call_args.push(CallArg {
                                        name: None,
                                        expr: idx_expr,
                                    });
                                    for slot in slots {
                                        call_args.push(CallArg {
                                            name: None,
                                            expr: Expr::Var(slot.clone()),
                                        });
                                    }
                                    *expr = Expr::UserCall {
                                        name: proc_read_helper_name(owner_proc, slots.len(), true),
                                        type_args: Vec::new(),
                                        args: call_args,
                                    };
                                }
                            } else if slots.len() == 1 {
                                if let Some(slot_name) = slots.first() {
                                    *expr = Expr::Var(slot_name.clone());
                                }
                            } else {
                                let mut call_args = Vec::<CallArg>::new();
                                call_args.push(CallArg {
                                    name: None,
                                    expr: idx_expr,
                                });
                                for slot in slots {
                                    call_args.push(CallArg {
                                        name: None,
                                        expr: Expr::Var(slot.clone()),
                                    });
                                }
                                *expr = Expr::UserCall {
                                    name: proc_read_helper_name(owner_proc, slots.len(), true),
                                    type_args: Vec::new(),
                                    args: call_args,
                                };
                            }
                            return;
                        }
                    }
                }
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_proc_expr_symbols(
                inner,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_proc_expr_symbols(
                    value,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

fn rewrite_proc_stmt_symbols(
    stmt: &Stmt,
    owner_proc: &str,
    field_names: &HashSet<String>,
    data_fields: &HashSet<String>,
    ins_names: &HashSet<String>,
    field_array_slots: &HashMap<String, Vec<String>>,
    in_array_slots: &HashMap<String, Vec<String>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Stmt> {
    with_stmt_diag_context(stmt, || {
        let source_loc = stmt.loc().cloned();
        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => {
                let mut expr_rewritten = expr.clone();
                rewrite_proc_expr_symbols(
                    &mut expr_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                match target {
                    AssignTarget::Var(name) => {
                        if ins_names.contains(name) {
                            errors.push(Diagnostic::semantic(
                                format!("cannot assign to processor input '{name}'"),
                                0,
                                0,
                            ));
                            return Some(Stmt::Assign {
                                loc: source_loc.clone(),
                                target: AssignTarget::Var(name.clone()),
                                decl_ty: *decl_ty,
                                generic_decl_ty: generic_decl_ty.clone(),
                                is_typed_decl: *is_typed_decl,
                                expr: expr_rewritten,
                            });
                        }
                        if field_names.contains(name) && is_plain_symbol(name) {
                            if matches!(expr, Expr::DataCtor { .. }) && data_fields.contains(name) {
                                return None;
                            }
                            return Some(Stmt::Assign {
                                loc: source_loc.clone(),
                                target: AssignTarget::Var(format!("self.{name}")),
                                decl_ty: None,
                                generic_decl_ty: None,
                                is_typed_decl: false,
                                expr: expr_rewritten,
                            });
                        }
                        Some(Stmt::Assign {
                            loc: source_loc.clone(),
                            target: AssignTarget::Var(name.clone()),
                            decl_ty: *decl_ty,
                            generic_decl_ty: generic_decl_ty.clone(),
                            is_typed_decl: *is_typed_decl,
                            expr: expr_rewritten,
                        })
                    }
                    AssignTarget::Index { base, index } => {
                        let mut idx_rewritten = index.clone();
                        rewrite_proc_expr_symbols(
                            &mut idx_rewritten,
                            owner_proc,
                            field_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        );
                        if let Some(slots) = in_array_slots.get(base) {
                            errors.push(Diagnostic::semantic(
                                format!("cannot assign to processor input '{base}'"),
                                0,
                                0,
                            ));
                            if let Some(raw_idx) = try_constant_index_i64(&idx_rewritten) {
                                if let Some(slot_idx) = resolve_proc_constant_slot_index(
                                    raw_idx,
                                    slots.len(),
                                    &format!("processor input array assignment '{base}[...]'"),
                                    errors,
                                ) {
                                    if let Some(slot_name) = slots.get(slot_idx) {
                                        return Some(Stmt::Assign {
                                            loc: None,
                                            target: AssignTarget::Var(slot_name.clone()),
                                            decl_ty: *decl_ty,
                                            generic_decl_ty: generic_decl_ty.clone(),
                                            is_typed_decl: *is_typed_decl,
                                            expr: expr_rewritten,
                                        });
                                    }
                                }
                            }
                        }
                        if let Some(slots) = field_array_slots.get(base) {
                            if let Some(raw_idx) = try_constant_index_i64(&idx_rewritten) {
                                let Some(slot_idx) = resolve_proc_constant_slot_index(
                                    raw_idx,
                                    slots.len(),
                                    &format!("processor array field assignment '{base}[...]'"),
                                    errors,
                                ) else {
                                    return Some(Stmt::Assign {
                                        loc: source_loc.clone(),
                                        target: AssignTarget::Index {
                                            base: base.clone(),
                                            index: idx_rewritten,
                                        },
                                        decl_ty: *decl_ty,
                                        generic_decl_ty: generic_decl_ty.clone(),
                                        is_typed_decl: *is_typed_decl,
                                        expr: expr_rewritten,
                                    });
                                };
                                if let Some(slot_name) = slots.get(slot_idx) {
                                    return Some(Stmt::Assign {
                                        loc: source_loc.clone(),
                                        target: AssignTarget::Var(format!("self.{slot_name}")),
                                        decl_ty: None,
                                        generic_decl_ty: None,
                                        is_typed_decl: false,
                                        expr: expr_rewritten,
                                    });
                                }
                            } else {
                                return Some(Stmt::Expr {
                                    loc: source_loc.clone(),
                                    expr: Expr::UserCall {
                                        name: proc_write_helper_name(owner_proc, slots, false),
                                        type_args: Vec::new(),
                                        args: vec![
                                            CallArg {
                                                name: None,
                                                expr: Expr::Var("self".to_owned()),
                                            },
                                            CallArg {
                                                name: None,
                                                expr: idx_rewritten,
                                            },
                                            CallArg {
                                                name: None,
                                                expr: expr_rewritten,
                                            },
                                        ],
                                    },
                                });
                            }
                        }
                        let target_base = if field_names.contains(base) && is_plain_symbol(base) {
                            format!("self.{base}")
                        } else {
                            base.clone()
                        };
                        Some(Stmt::Assign {
                            loc: source_loc.clone(),
                            target: AssignTarget::Index {
                                base: target_base,
                                index: idx_rewritten,
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            expr: expr_rewritten,
                        })
                    }
                }
            }
            Stmt::Expr { expr, .. } => {
                let mut expr_rewritten = expr.clone();
                rewrite_proc_expr_symbols(
                    &mut expr_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                if let Expr::UserCall { name, args, .. } = &expr_rewritten {
                    if name == "unsafe_write" && args.len() == 3 {
                        if let Expr::Var(base) = &args[0].expr {
                            if let Some(slots) = in_array_slots.get(base) {
                                errors.push(Diagnostic::semantic(
                                    format!("cannot assign to processor input '{base}'"),
                                    0,
                                    0,
                                ));
                                if let Some(raw_idx) = try_constant_index_i64(&args[1].expr) {
                                    if raw_idx >= 0 && raw_idx < slots.len() as i64 {
                                        let slot_idx = raw_idx as usize;
                                        if let Some(slot_name) = slots.get(slot_idx) {
                                            return Some(Stmt::Assign {
                                                loc: source_loc.clone(),
                                                target: AssignTarget::Var(slot_name.clone()),
                                                decl_ty: None,
                                                generic_decl_ty: None,
                                                is_typed_decl: false,
                                                expr: args[2].expr.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                            if let Some(slots) = field_array_slots.get(base) {
                                if let Some(raw_idx) = try_constant_index_i64(&args[1].expr) {
                                    if raw_idx >= 0 && raw_idx < slots.len() as i64 {
                                        let slot_idx = raw_idx as usize;
                                        if let Some(slot_name) = slots.get(slot_idx) {
                                            return Some(Stmt::Assign {
                                                loc: source_loc.clone(),
                                                target: AssignTarget::Var(format!(
                                                    "self.{slot_name}"
                                                )),
                                                decl_ty: None,
                                                generic_decl_ty: None,
                                                is_typed_decl: false,
                                                expr: args[2].expr.clone(),
                                            });
                                        }
                                    }
                                }
                                return Some(Stmt::Expr {
                                    loc: source_loc.clone(),
                                    expr: Expr::UserCall {
                                        name: proc_write_helper_name(owner_proc, slots, true),
                                        type_args: Vec::new(),
                                        args: vec![
                                            CallArg {
                                                name: None,
                                                expr: Expr::Var("self".to_owned()),
                                            },
                                            CallArg {
                                                name: None,
                                                expr: args[1].expr.clone(),
                                            },
                                            CallArg {
                                                name: None,
                                                expr: args[2].expr.clone(),
                                            },
                                        ],
                                    },
                                });
                            }
                        }
                    }
                }
                Some(Stmt::Expr {
                    loc: source_loc.clone(),
                    expr: expr_rewritten,
                })
            }
            Stmt::Return { expr, .. } => {
                let mut expr_rewritten = expr.clone();
                rewrite_proc_expr_symbols(
                    &mut expr_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                Some(Stmt::Return {
                    loc: source_loc.clone(),
                    expr: expr_rewritten,
                })
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let mut cond_rewritten = cond.clone();
                rewrite_proc_expr_symbols(
                    &mut cond_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                let then_branch = then_branch
                    .iter()
                    .filter_map(|s| {
                        rewrite_proc_stmt_symbols(
                            s,
                            owner_proc,
                            field_names,
                            data_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                let else_branch = else_branch
                    .iter()
                    .filter_map(|s| {
                        rewrite_proc_stmt_symbols(
                            s,
                            owner_proc,
                            field_names,
                            data_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(Stmt::If {
                    loc: source_loc.clone(),
                    cond: cond_rewritten,
                    then_branch,
                    else_branch,
                })
            }
            Stmt::For {
                loc: _stmt_loc,
                var,
                start,
                end,
                body,
                ..
            } => {
                let mut start_rewritten = start.clone();
                let mut end_rewritten = end.clone();
                rewrite_proc_expr_symbols(
                    &mut start_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                rewrite_proc_expr_symbols(
                    &mut end_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                let body = body
                    .iter()
                    .filter_map(|s| {
                        rewrite_proc_stmt_symbols(
                            s,
                            owner_proc,
                            field_names,
                            data_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(Stmt::For {
                    loc: source_loc,
                    var: var.clone(),
                    start: start_rewritten,
                    end: end_rewritten,
                    body,
                })
            }
        }
    })
}

fn try_constant_proc_out_index(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Int(v) => usize::try_from(*v).ok(),
        Expr::Number(v) => {
            let truncated = v.trunc();
            if (v - truncated).abs() <= 1e-6 && truncated >= 0.0 {
                usize::try_from(truncated as i64).ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn try_constant_index_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(v) => Some(*v),
        Expr::Number(v) => {
            let truncated = v.trunc();
            if (v - truncated).abs() <= 1e-6 {
                Some(truncated as i64)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn resolve_proc_constant_slot_index(
    idx: i64,
    len: usize,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if idx < 0 || idx >= len as i64 {
        errors.push(Diagnostic::semantic(
            format!(
                "{context}: index {idx} is out of range (expected 0..{})",
                len.saturating_sub(1)
            ),
            0,
            0,
        ));
        return None;
    }
    Some(idx as usize)
}

fn proc_read_helper_name(owner_proc: &str, len: usize, unsafe_mode: bool) -> String {
    let mode = if unsafe_mode { "unsafe" } else { "clamp" };
    format!("{owner_proc}.__arr_read_{mode}_{len}")
}

fn sanitize_symbol_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn proc_write_helper_name(owner_proc: &str, slots: &[String], unsafe_mode: bool) -> String {
    let mode = if unsafe_mode { "unsafe" } else { "clamp" };
    let key = sanitize_symbol_component(&slots.join("__"));
    format!("{owner_proc}.__arr_write_{mode}_{key}")
}

fn build_proc_read_helper(owner_proc: &str, len: usize, unsafe_mode: bool) -> FunctionDef {
    let mut params = Vec::<omni_frontend::FnParamDecl>::new();
    params.push(omni_frontend::FnParamDecl {
        name: "idx".to_owned(),
        ty: None,
        default: None,
    });
    for i in 0..len {
        params.push(omni_frontend::FnParamDecl {
            name: format!("s{i}"),
            ty: None,
            default: None,
        });
    }

    let mut body = Vec::<Stmt>::new();
    body.push(Stmt::Assign {
        loc: None,
        target: AssignTarget::Var("i".to_owned()),
        decl_ty: Some(PrimitiveType::I32),
        generic_decl_ty: None,
        is_typed_decl: true,
        expr: Expr::Cast {
            to: PrimitiveType::I32,
            expr: Box::new(Expr::Var("idx".to_owned())),
        },
    });

    if !unsafe_mode {
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Lt,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(0)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Int(0),
            }],
            else_branch: Vec::new(),
        });
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Ge,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(len as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Int((len.saturating_sub(1)) as i64),
            }],
            else_branch: Vec::new(),
        });
    }

    if len == 1 {
        body.push(Stmt::Return {
            loc: None,
            expr: Expr::Var("s0".to_owned()),
        });
    } else {
        for i in 0..len {
            body.push(Stmt::If {
                loc: None,
                cond: Expr::Compare {
                    op: CmpOp::Eq,
                    lhs: Box::new(Expr::Var("i".to_owned())),
                    rhs: Box::new(Expr::Int(i as i64)),
                },
                then_branch: vec![Stmt::Return {
                    loc: None,
                    expr: Expr::Var(format!("s{i}")),
                }],
                else_branch: Vec::new(),
            });
        }
        if unsafe_mode {
            body.push(Stmt::Expr {
                loc: None,
                expr: Expr::Binary {
                    op: BinaryOp::Div,
                    lhs: Box::new(Expr::Int(1)),
                    rhs: Box::new(Expr::Int(0)),
                },
            });
            body.push(Stmt::Return {
                loc: None,
                expr: Expr::Number(0.0),
            });
        } else {
            body.push(Stmt::Return {
                loc: None,
                expr: Expr::Var("s0".to_owned()),
            });
        }
    }

    FunctionDef {
        type_params: Vec::new(),
        name: proc_read_helper_name(owner_proc, len, unsafe_mode),
        params,
        body,
    }
}

fn build_proc_write_helper(owner_proc: &str, slots: &[String], unsafe_mode: bool) -> FunctionDef {
    let mut params = Vec::<omni_frontend::FnParamDecl>::new();
    params.push(omni_frontend::FnParamDecl {
        name: "self".to_owned(),
        ty: Some(FnParamType::Struct(owner_proc.to_owned())),
        default: None,
    });
    params.push(omni_frontend::FnParamDecl {
        name: "idx".to_owned(),
        ty: None,
        default: None,
    });
    params.push(omni_frontend::FnParamDecl {
        name: "value".to_owned(),
        ty: None,
        default: None,
    });

    let len = slots.len();
    let mut body = Vec::<Stmt>::new();
    body.push(Stmt::Assign {
        loc: None,
        target: AssignTarget::Var("i".to_owned()),
        decl_ty: Some(PrimitiveType::I32),
        generic_decl_ty: None,
        is_typed_decl: true,
        expr: Expr::Cast {
            to: PrimitiveType::I32,
            expr: Box::new(Expr::Var("idx".to_owned())),
        },
    });

    if !unsafe_mode {
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Lt,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(0)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Int(0),
            }],
            else_branch: Vec::new(),
        });
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Ge,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(len as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Int((len.saturating_sub(1)) as i64),
            }],
            else_branch: Vec::new(),
        });
    }

    for (idx, slot) in slots.iter().enumerate() {
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Eq,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(idx as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var(format!("self.{slot}")),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Var("value".to_owned()),
            }],
            else_branch: Vec::new(),
        });
    }
    if unsafe_mode {
        body.push(Stmt::Expr {
            loc: None,
            expr: Expr::Binary {
                op: BinaryOp::Div,
                lhs: Box::new(Expr::Int(1)),
                rhs: Box::new(Expr::Int(0)),
            },
        });
    }

    FunctionDef {
        type_params: Vec::new(),
        name: proc_write_helper_name(owner_proc, slots, unsafe_mode),
        params,
        body,
    }
}

fn expand_expr_to_slots(
    expr: &Expr,
    slot_count: usize,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Expr> {
    if slot_count == 0 {
        return Vec::new();
    }
    if slot_count == 1 {
        return vec![expr.clone()];
    }
    match expr {
        Expr::ArrayLiteral(values) => {
            if values.len() != slot_count {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context}: expected array argument with {slot_count} elements, got {}",
                        values.len()
                    ),
                    0,
                    0,
                ));
            }
            (0..slot_count)
                .map(|i| values.get(i).cloned().unwrap_or(Expr::Number(0.0)))
                .collect()
        }
        Expr::Var(base) => (0..slot_count)
            .map(|i| Expr::Index {
                base: base.clone(),
                index: Box::new(Expr::Int(i as i64)),
            })
            .collect(),
        _ => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: array argument requires an array literal or array symbol expression"
                ),
                0,
                0,
            ));
            vec![expr.clone(); slot_count]
        }
    }
}

fn expand_proc_call_args(
    call_args: &[CallArg],
    api: &ProcApi,
    call_display_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    let param_names = api.ins.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let param_defaults = api
        .ins
        .iter()
        .map(|p| {
            if p.slots.len() == 1 {
                p.defaults.first().cloned().flatten()
            } else if p.defaults.iter().all(|d| d.is_some()) {
                Some(Expr::ArrayLiteral(
                    p.defaults
                        .iter()
                        .filter_map(|d| d.clone())
                        .collect::<Vec<_>>(),
                ))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        call_args,
        &param_names,
        &param_defaults,
        false,
        false,
        &format!("processor call '{call_display_name}(...)'"),
        errors,
    );
    let mut expanded = Vec::<CallArg>::new();
    for (idx, port) in api.ins.iter().enumerate() {
        let slot_exprs = match resolved.get(idx).and_then(|a| *a) {
            Some(arg_expr) => expand_expr_to_slots(
                arg_expr,
                port.slots.len(),
                &format!(
                    "processor call '{call_display_name}(...)' argument '{}'",
                    port.name
                ),
                errors,
            ),
            None => {
                if port.defaults.iter().all(|d| d.is_some()) {
                    port.defaults
                        .iter()
                        .filter_map(|d| d.clone())
                        .collect::<Vec<_>>()
                } else {
                    continue;
                }
            }
        };
        for (slot_idx, expr) in slot_exprs.into_iter().enumerate() {
            let expr = if let Some(range) = port.ranges.get(slot_idx).and_then(|r| *r) {
                clamp_expr_to_range(expr, range)
            } else {
                expr
            };
            expanded.push(CallArg { name: None, expr });
        }
    }
    expanded
}

fn expand_proc_buffer_call_args(
    instance: &ProcCallInstance,
    api: &ProcApi,
    call_display_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    if api.buffers.is_empty() {
        return Vec::new();
    }
    if instance.buffer_args.len() != api.buffers.len() {
        errors.push(Diagnostic::semantic(
            format!(
                "processor call '{call_display_name}(...)' is missing bound buffer arguments (expected {}, got {})",
                api.buffers.len(),
                instance.buffer_args.len()
            ),
            0,
            0,
        ));
        return Vec::new();
    }
    instance
        .buffer_args
        .iter()
        .cloned()
        .map(|expr| CallArg { name: None, expr })
        .collect::<Vec<_>>()
}

fn expand_proc_port_specs(
    proc_name: &str,
    ports: &[PortDecl],
    kind: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (
    Vec<String>,
    HashMap<String, PrimitiveType>,
    Vec<ProcPortSpec>,
    HashMap<String, Vec<String>>,
) {
    let (flat, flat_types, arrays, defaults, ranges) = expand_port_decls(
        ports,
        &format!("processor '{proc_name}' {kind}"),
        options,
        errors,
    );
    let mut port_specs = Vec::<ProcPortSpec>::new();
    let mut array_slots = HashMap::<String, Vec<String>>::new();
    for port in ports {
        match port.ty.as_ref() {
            Some(DeclType::Array { .. }) | Some(DeclType::ArrayGeneric { .. }) => {
                let len = arrays.get(&port.name).map(|i| i.len).unwrap_or(0);
                let slots = (0..len)
                    .map(|idx| format!("{}[{idx}]", port.name))
                    .collect::<Vec<_>>();
                array_slots.insert(port.name.clone(), slots.clone());
                port_specs.push(ProcPortSpec {
                    name: port.name.clone(),
                    slots,
                    defaults: vec![None; len],
                    ranges: vec![None; len],
                });
            }
            _ => {
                let default = if port.default.is_some() {
                    defaults.get(&port.name).copied().map(typed_const_expr)
                } else {
                    None
                };
                port_specs.push(ProcPortSpec {
                    name: port.name.clone(),
                    slots: vec![port.name.clone()],
                    defaults: vec![default],
                    ranges: vec![ranges.get(&port.name).copied()],
                });
            }
        }
    }
    (flat, flat_types, port_specs, array_slots)
}

fn expand_proc_param_specs(
    proc_name: &str,
    params: &[ParamDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<ProcParamSpec>, HashMap<String, Vec<String>>) {
    let mut specs = Vec::<ProcParamSpec>::new();
    let mut field_array_slots = HashMap::<String, Vec<String>>::new();

    for param in params {
        match param.ty.as_ref() {
            None | Some(DeclType::Scalar(_)) => {
                let ty = match param.ty.as_ref() {
                    Some(DeclType::Scalar(ty)) => *ty,
                    _ => PrimitiveType::F32,
                };
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        ty,
                        options,
                        &format!("processor '{proc_name}' param '{}' default", param.name),
                        is_float_type(ty),
                        matches!(ty, PrimitiveType::I32 | PrimitiveType::I64),
                        errors,
                    )
                    .unwrap_or_else(|| coerce_const_default_to_typed(0.0, ty)),
                    None => coerce_const_default_to_typed(0.0, ty),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        ty,
                        options,
                        &format!("processor '{proc_name}' param '{}'", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcParamSlotSpec {
                        name: param.name.clone(),
                        ty,
                        default: Some(typed_const_expr(default)),
                        range,
                    }],
                });
            }
            Some(DeclType::Generic(param_ty)) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{proc_name}' param '{}' uses unresolved generic type '{}'",
                        param.name, param_ty
                    ),
                    0,
                    0,
                ));
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        PrimitiveType::F32,
                        options,
                        &format!("processor '{proc_name}' param '{}' default", param.name),
                        true,
                        false,
                        errors,
                    )
                    .unwrap_or(TypedConstValue::F32(0.0)),
                    None => TypedConstValue::F32(0.0),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        PrimitiveType::F32,
                        options,
                        &format!("processor '{proc_name}' param '{}'", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcParamSlotSpec {
                        name: param.name.clone(),
                        ty: PrimitiveType::F32,
                        default: Some(typed_const_expr(default)),
                        range,
                    }],
                });
            }
            Some(DeclType::ArrayGeneric { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{proc_name}' param '{}' range is not supported for array declarations",
                            param.name
                        ),
                        0,
                        0,
                    ));
                }
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{proc_name}' param '{}' uses unresolved generic array element type '{}'",
                        param.name, elem
                    ),
                    0,
                    0,
                ));
                let size_context =
                    format!("processor '{proc_name}' param '{}' array size", param.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                let mut slot_defaults = Vec::<Option<Expr>>::with_capacity(len);
                match &param.default {
                    None => {
                        for _ in 0..len {
                            slot_defaults.push(Some(Expr::Number(0.0)));
                        }
                    }
                    Some(Expr::ArrayLiteral(values)) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor '{proc_name}' param '{}' default expects {len} elements, got {}",
                                    param.name,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
                        }
                        for idx in 0..len {
                            slot_defaults.push(values.get(idx).cloned());
                        }
                    }
                    Some(expr) => {
                        for _ in 0..len {
                            slot_defaults.push(Some(expr.clone()));
                        }
                    }
                }
                let mut slots = Vec::<ProcParamSlotSpec>::with_capacity(len);
                let mut slot_names = Vec::<String>::with_capacity(len);
                for idx in 0..len {
                    let slot_name = format!("{}[{idx}]", param.name);
                    slot_names.push(slot_name.clone());
                    slots.push(ProcParamSlotSpec {
                        name: slot_name,
                        ty: PrimitiveType::F32,
                        default: slot_defaults.get(idx).cloned().unwrap_or(None),
                        range: None,
                    });
                }
                field_array_slots.insert(param.name.clone(), slot_names);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots,
                });
            }
            Some(DeclType::Array { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{proc_name}' param '{}' range is not supported for array declarations",
                            param.name
                        ),
                        0,
                        0,
                    ));
                }
                let size_context =
                    format!("processor '{proc_name}' param '{}' array size", param.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                let mut slot_defaults = Vec::<Option<Expr>>::with_capacity(len);
                match &param.default {
                    None => {
                        for _ in 0..len {
                            slot_defaults.push(Some(Expr::Number(0.0)));
                        }
                    }
                    Some(Expr::ArrayLiteral(values)) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor '{proc_name}' param '{}' default expects {len} elements, got {}",
                                    param.name,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
                        }
                        for idx in 0..len {
                            slot_defaults
                                .push(values.get(idx).cloned().or(Some(Expr::Number(0.0))));
                        }
                    }
                    Some(expr) => {
                        for _ in 0..len {
                            slot_defaults.push(Some(expr.clone()));
                        }
                    }
                }
                let mut slots = Vec::<ProcParamSlotSpec>::with_capacity(len);
                let mut slot_names = Vec::<String>::with_capacity(len);
                for idx in 0..len {
                    let slot_name = format!("{}[{idx}]", param.name);
                    slot_names.push(slot_name.clone());
                    slots.push(ProcParamSlotSpec {
                        name: slot_name,
                        ty: *elem,
                        default: slot_defaults
                            .get(idx)
                            .cloned()
                            .unwrap_or(Some(Expr::Number(0.0))),
                        range: None,
                    });
                }
                field_array_slots.insert(param.name.clone(), slot_names);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots,
                });
            }
        }
    }

    (specs, field_array_slots)
}

fn proc_buffer_fn_param_type(spec: &ProcBufferSpec) -> FnParamType {
    let channels = match spec.channels {
        TypedBufferChannels::Mono => BufferChannels::Mono,
        TypedBufferChannels::Dynamic => BufferChannels::Dynamic,
        TypedBufferChannels::Static(ch) => BufferChannels::Static(Expr::Int(ch as i64)),
    };
    FnParamType::Buffer(omni_frontend::BufferType {
        elem: BufferElemType::Primitive(spec.elem_ty),
        channels,
    })
}

fn rewrite_proc_calls_in_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => {
            rewrite_proc_calls_in_expr(index, proc_vars, proc_api, errors);
        }
        Expr::DataCtor { spec, init } => {
            rewrite_proc_calls_in_expr(&mut spec.size, proc_vars, proc_api, errors);
            if let Some(values) = init {
                for value in values {
                    rewrite_proc_calls_in_expr(value, proc_vars, proc_api, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_proc_calls_in_expr(lhs, proc_vars, proc_api, errors);
            rewrite_proc_calls_in_expr(rhs, proc_vars, proc_api, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_proc_calls_in_expr(arg, proc_vars, proc_api, errors);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_proc_calls_in_expr(inner, proc_vars, proc_api, errors);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_proc_calls_in_expr(value, proc_vars, proc_api, errors);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                rewrite_proc_calls_in_expr(&mut arg.expr, proc_vars, proc_api, errors);
            }

            if let Some(proc_var) = name.strip_prefix(PROC_INDEX_SENTINEL_PREFIX) {
                let Some(instance) = proc_vars.get(proc_var) else {
                    errors.push(Diagnostic::semantic(
                        format!("processor call target '{proc_var}' is not an instance"),
                        0,
                        0,
                    ));
                    return;
                };
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    errors.push(Diagnostic::semantic(
                        format!("unknown processor type '{proc_name}'"),
                        0,
                        0,
                    ));
                    return;
                };
                let index_pos = args.iter().position(|a| {
                    a.name
                        .as_ref()
                        .map(|s| s == PROC_INDEX_SENTINEL_ARG)
                        .unwrap_or(false)
                });
                let Some(index_pos) = index_pos else {
                    errors.push(Diagnostic::semantic(
                        "processor indexed call is missing index expression",
                        0,
                        0,
                    ));
                    return;
                };
                let index_arg = args.remove(index_pos);
                let Some(out_idx) = try_constant_proc_out_index(&index_arg.expr) else {
                    errors.push(Diagnostic::semantic(
                        "processor indexed call requires a compile-time integer index",
                        0,
                        0,
                    ));
                    return;
                };
                if out_idx >= api.outs.len() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor output index {out_idx} is out of range (outs: {})",
                            api.outs.len()
                        ),
                        0,
                        0,
                    ));
                    return;
                }
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: Expr::Var(proc_var.to_owned()),
                });
                let expanded_args = expand_proc_call_args(args, api, proc_var, errors);
                rewritten.extend(expanded_args);
                let expanded_buffers =
                    expand_proc_buffer_call_args(instance, api, proc_var, errors);
                rewritten.extend(expanded_buffers);
                *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}{out_idx}");
                *args = rewritten;
                return;
            }

            if let Some(instance) = proc_vars.get(name) {
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    errors.push(Diagnostic::semantic(
                        format!("unknown processor type '{proc_name}'"),
                        0,
                        0,
                    ));
                    return;
                };
                if api.outs.len() != 1 {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor call '{}(...)' has {} outputs; use '{}(...)[index]' or call as statement then read fields",
                            name,
                            api.outs.len(),
                            name
                        ),
                        0,
                        0,
                    ));
                    return;
                }
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: Expr::Var(name.clone()),
                });
                let expanded_args = expand_proc_call_args(args, &api, name, errors);
                rewritten.extend(expanded_args);
                let expanded_buffers = expand_proc_buffer_call_args(instance, &api, name, errors);
                rewritten.extend(expanded_buffers);
                *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                *args = rewritten;
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn split_dot_path(name: &str) -> Option<(&str, &str)> {
    let (base, field) = name.split_once('.')?;
    if base.is_empty() || field.is_empty() {
        return None;
    }
    if field.contains('.') {
        return None;
    }
    Some((base, field))
}

fn maybe_clamp_proc_param_assignment_expr(
    target: &AssignTarget,
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) {
    let Some((base, field)) = (match target {
        AssignTarget::Var(name) => split_dot_path(name),
        AssignTarget::Index { base, .. } => split_dot_path(base),
    }) else {
        return;
    };
    let Some(instance) = proc_vars.get(base) else {
        return;
    };
    let Some(api) = proc_api.get(&instance.proc_name) else {
        return;
    };
    let Some(param_slot) = api.params.get(field) else {
        return;
    };
    if let Some(range) = param_slot.range {
        let original = std::mem::replace(expr, Expr::Number(0.0));
        *expr = clamp_expr_to_range(original, range);
    }
}

fn rewrite_proc_calls_in_stmt(
    stmt: &mut Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { target, expr, .. } => {
            rewrite_proc_calls_in_expr(expr, proc_vars, proc_api, errors);
            maybe_clamp_proc_param_assignment_expr(target, expr, proc_vars, proc_api);
        }
        Stmt::Expr { expr, .. } => {
            let mut handled_proc_stmt_call = false;
            if let Expr::UserCall { name, args, .. } = expr {
                for arg in args.iter_mut() {
                    rewrite_proc_calls_in_expr(&mut arg.expr, proc_vars, proc_api, errors);
                }
                if let Some(instance) = proc_vars.get(name) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown processor type '{proc_name}'"),
                            0,
                            0,
                        ));
                        return;
                    };
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Var(name.clone()),
                    });
                    let expanded_args = expand_proc_call_args(args, api, name, errors);
                    rewritten.extend(expanded_args);
                    let expanded_buffers =
                        expand_proc_buffer_call_args(instance, api, name, errors);
                    rewritten.extend(expanded_buffers);
                    *name = format!("{proc_name}{PROC_STEP_FN_SUFFIX}");
                    *args = rewritten;
                    handled_proc_stmt_call = true;
                }
            }
            if !handled_proc_stmt_call {
                rewrite_proc_calls_in_expr(expr, proc_vars, proc_api, errors);
            }
        }
        Stmt::Return { expr, .. } => rewrite_proc_calls_in_expr(expr, proc_vars, proc_api, errors),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_proc_calls_in_expr(cond, proc_vars, proc_api, errors);
            for s in then_branch {
                rewrite_proc_calls_in_stmt(s, proc_vars, proc_api, errors);
            }
            for s in else_branch {
                rewrite_proc_calls_in_stmt(s, proc_vars, proc_api, errors);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            rewrite_proc_calls_in_expr(start, proc_vars, proc_api, errors);
            rewrite_proc_calls_in_expr(end, proc_vars, proc_api, errors);
            for s in body {
                rewrite_proc_calls_in_stmt(s, proc_vars, proc_api, errors);
            }
        }
    });
}

fn rewrite_proc_calls_in_stmts(
    stmts: &mut [Stmt],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        rewrite_proc_calls_in_stmt(stmt, proc_vars, proc_api, errors);
    }
}

fn collect_called_proc_instances_in_expr(
    expr: &Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    out: &mut HashSet<String>,
) {
    match expr {
        Expr::Index { index, .. } => collect_called_proc_instances_in_expr(index, proc_vars, out),
        Expr::DataCtor { spec, init } => {
            collect_called_proc_instances_in_expr(&spec.size, proc_vars, out);
            if let Some(values) = init {
                for value in values {
                    collect_called_proc_instances_in_expr(value, proc_vars, out);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_called_proc_instances_in_expr(lhs, proc_vars, out);
            collect_called_proc_instances_in_expr(rhs, proc_vars, out);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_called_proc_instances_in_expr(arg, proc_vars, out);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            collect_called_proc_instances_in_expr(inner, proc_vars, out);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                collect_called_proc_instances_in_expr(value, proc_vars, out);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                collect_called_proc_instances_in_expr(&arg.expr, proc_vars, out);
            }
            if let Some(proc_var) = name.strip_prefix(PROC_INDEX_SENTINEL_PREFIX) {
                if proc_vars.contains_key(proc_var) {
                    out.insert(proc_var.to_owned());
                }
            } else if proc_vars.contains_key(name) {
                out.insert(name.clone());
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn collect_called_proc_instances_in_stmt(
    stmt: &Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    out: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } | Stmt::Expr { expr, .. } => {
            collect_called_proc_instances_in_expr(expr, proc_vars, out);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_called_proc_instances_in_expr(cond, proc_vars, out);
            for nested in then_branch {
                collect_called_proc_instances_in_stmt(nested, proc_vars, out);
            }
            for nested in else_branch {
                collect_called_proc_instances_in_stmt(nested, proc_vars, out);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            collect_called_proc_instances_in_expr(start, proc_vars, out);
            collect_called_proc_instances_in_expr(end, proc_vars, out);
            for nested in body {
                collect_called_proc_instances_in_stmt(nested, proc_vars, out);
            }
        }
    }
}

fn collect_called_proc_instances_in_stmts(
    stmts: &[Stmt],
    proc_vars: &HashMap<String, ProcCallInstance>,
) -> HashSet<String> {
    let mut out = HashSet::<String>::new();
    for stmt in stmts {
        collect_called_proc_instances_in_stmt(stmt, proc_vars, &mut out);
    }
    out
}

fn compute_effective_proc_block_flags(
    proc_order: &[String],
    proc_defs_by_name: &HashMap<String, omni_frontend::ProcessorDef>,
    base_shapes: &HashMap<String, ProcBaseShape>,
) -> HashMap<String, bool> {
    fn visit(
        proc_name: &str,
        proc_defs_by_name: &HashMap<String, omni_frontend::ProcessorDef>,
        base_shapes: &HashMap<String, ProcBaseShape>,
        cache: &mut HashMap<String, bool>,
        visiting: &mut HashSet<String>,
    ) -> bool {
        if let Some(value) = cache.get(proc_name) {
            return *value;
        }
        if !visiting.insert(proc_name.to_owned()) {
            return false;
        }
        let mut has_block = proc_defs_by_name
            .get(proc_name)
            .map(|p| p.has_block_block)
            .unwrap_or(false);
        if !has_block {
            if let (Some(proc_def), Some(shape)) =
                (proc_defs_by_name.get(proc_name), base_shapes.get(proc_name))
            {
                let nested_instances = shape
                    .state
                    .nested_procs
                    .iter()
                    .map(|(name, state)| {
                        (
                            name.clone(),
                            ProcCallInstance {
                                proc_name: state.proc_name.clone(),
                                buffer_args: Vec::new(),
                            },
                        )
                    })
                    .collect::<HashMap<_, _>>();
                let called_nested =
                    collect_called_proc_instances_in_stmts(&proc_def.sample, &nested_instances);
                for nested_var in called_nested {
                    let Some(instance) = nested_instances.get(&nested_var) else {
                        continue;
                    };
                    if visit(
                        &instance.proc_name,
                        proc_defs_by_name,
                        base_shapes,
                        cache,
                        visiting,
                    ) {
                        has_block = true;
                        break;
                    }
                }
            }
        }
        visiting.remove(proc_name);
        cache.insert(proc_name.to_owned(), has_block);
        has_block
    }

    let mut cache = HashMap::<String, bool>::new();
    let mut visiting = HashSet::<String>::new();
    for proc_name in proc_order {
        let _ = visit(
            proc_name,
            proc_defs_by_name,
            base_shapes,
            &mut cache,
            &mut visiting,
        );
    }
    cache
}

#[derive(Debug, Clone)]
struct ProcBaseShape {
    ins: Vec<String>,
    outs: Vec<String>,
    in_ports: Vec<ProcPortSpec>,
    param_specs: Vec<ProcParamSpec>,
    buffer_specs: Vec<ProcBufferSpec>,
    in_types: HashMap<String, PrimitiveType>,
    in_array_slots: HashMap<String, Vec<String>>,
    field_array_slots: HashMap<String, Vec<String>>,
    state: ProcStateFields,
    instance_fields: HashMap<String, HashSet<String>>,
    fields: Vec<StructField>,
    field_names: HashSet<String>,
    data_field_names: HashSet<String>,
}

#[derive(Debug, Clone)]
struct ProcLoweringShape {
    ins: Vec<String>,
    outs: Vec<String>,
    in_ports: Vec<ProcPortSpec>,
    param_specs: Vec<ProcParamSpec>,
    buffer_specs: Vec<ProcBufferSpec>,
    in_types: HashMap<String, PrimitiveType>,
    in_array_slots: HashMap<String, Vec<String>>,
    field_array_slots: HashMap<String, Vec<String>>,
    state: ProcStateFields,
    fields: Vec<StructField>,
    field_names: HashSet<String>,
    data_field_names: HashSet<String>,
    nested_fields: HashMap<String, HashSet<String>>,
}

fn infer_primary_output_type_from_processor(proc: &ProcessorDef) -> PrimitiveType {
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let out_ports = normalize_numbered_port_decls(&proc.outs, "out", inferred_io.max_out);
    let Some(first_out) = out_ports.first() else {
        return PrimitiveType::F32;
    };
    match first_out.ty.as_ref() {
        Some(DeclType::Scalar(ty)) => *ty,
        Some(DeclType::Array { elem, .. }) => *elem,
        Some(DeclType::Generic(_)) | Some(DeclType::ArrayGeneric { .. }) | None => {
            PrimitiveType::F32
        }
    }
}

fn struct_defs_for_scalar_expr_inference(
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
) -> HashMap<String, Vec<TypedStructField>> {
    struct_defs
        .iter()
        .map(|(name, def)| {
            let fields = def
                .fields
                .iter()
                .map(|field| {
                    let (ty, data_elem_ty, data_elem_struct) = match &field.ty {
                        FieldType::Scalar(prim) => (TypedFieldType::Scalar(*prim), None, None),
                        FieldType::Generic(_) => {
                            // Generic struct fields are unresolved in this prepass; default like other unresolved scalar contexts.
                            (TypedFieldType::Scalar(PrimitiveType::F32), None, None)
                        }
                        FieldType::Data(spec) => {
                            let (elem_ty, elem_struct) = match &spec.elem {
                                DataElemType::Primitive(prim) => (Some(*prim), None),
                                DataElemType::Struct(struct_name) => {
                                    (None, Some(struct_name.clone()))
                                }
                            };
                            (TypedFieldType::Data(0), elem_ty, elem_struct)
                        }
                    };
                    TypedStructField {
                        name: field.name.clone(),
                        ty,
                        default: field.default.clone(),
                        data_elem_ty,
                        data_elem_struct,
                    }
                })
                .collect::<Vec<_>>();
            (name.clone(), fields)
        })
        .collect::<HashMap<_, _>>()
}

fn compute_proc_shape(
    proc: &omni_frontend::ProcessorDef,
    options: AnalysisOptions,
    proc_symbols: &HashSet<String>,
    proc_primary_output_types: &HashMap<String, PrimitiveType>,
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    ctor_symbols: &HashSet<String>,
    fn_return_types: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> ProcBaseShape {
    let struct_symbols = struct_defs.keys().cloned().collect::<HashSet<_>>();
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let ins_ports = normalize_numbered_port_decls(&proc.ins, "in", inferred_io.max_in);
    let out_ports = normalize_numbered_port_decls(&proc.outs, "out", inferred_io.max_out);
    let (ins, in_types, in_ports, in_array_slots) =
        expand_proc_port_specs(&proc.name, &ins_ports, "input", options, errors);
    let (outs, out_types, _out_ports, out_array_slots) =
        expand_proc_port_specs(&proc.name, &out_ports, "output", options, errors);
    let (param_specs, mut field_array_slots) =
        expand_proc_param_specs(&proc.name, &proc.params, options, errors);
    let buffer_specs = coerce_buffers(&proc.buffers, options, errors)
        .into_iter()
        .map(|b| ProcBufferSpec {
            name: b.name,
            elem_ty: b.elem_ty,
            channels: b.channels,
        })
        .collect::<Vec<_>>();
    for (name, slots) in out_array_slots {
        field_array_slots.insert(name, slots);
    }

    let typed_struct_defs = struct_defs_for_scalar_expr_inference(struct_defs);
    let mut typed_param_names = HashSet::<String>::new();
    for spec in &param_specs {
        typed_param_names.insert(spec.name.clone());
        for slot in &spec.slots {
            typed_param_names.insert(slot.name.clone());
        }
    }
    let mut param_names = typed_param_names.clone();
    for buffer in &buffer_specs {
        if param_names.contains(&buffer.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}' buffer '{}' conflicts with param name",
                    proc.name, buffer.name
                ),
                0,
                0,
            ));
        }
        param_names.insert(buffer.name.clone());
    }
    let mut ins_names = ins.iter().cloned().collect::<HashSet<_>>();
    for spec in &in_ports {
        ins_names.insert(spec.name.clone());
    }
    let mut out_names = outs.iter().cloned().collect::<HashSet<_>>();
    for port in &out_ports {
        out_names.insert(port.name.clone());
    }
    let mut reserved = HashSet::<String>::new();
    reserved.extend(param_names.iter().cloned());
    reserved.extend(ins_names.iter().cloned());
    reserved.extend(out_names.iter().cloned());

    let mut state_type_hints = HashMap::<String, PrimitiveType>::new();
    set_declared_symbol_types(
        &mut state_type_hints,
        &ins_names,
        &in_types,
        DECLARED_INPUT_TYPE_PREFIX,
    );
    set_declared_symbol_types(
        &mut state_type_hints,
        &out_names,
        &out_types,
        DECLARED_OUTPUT_TYPE_PREFIX,
    );
    let param_slot_types = param_specs
        .iter()
        .flat_map(|spec| spec.slots.iter())
        .map(|slot| (slot.name.clone(), slot.ty))
        .collect::<HashMap<_, _>>();
    set_declared_symbol_types(
        &mut state_type_hints,
        &typed_param_names,
        &param_slot_types,
        DECLARED_PARAM_TYPE_PREFIX,
    );
    for (fn_name, fn_ty) in fn_return_types {
        state_type_hints.insert(
            declared_type_key(DECLARED_FUNCTION_RETURN_TYPE_PREFIX, fn_name),
            *fn_ty,
        );
    }

    let mut state = ProcStateFields::default();
    let proc_ns = namespace_of_symbol(&proc.name);
    for stmt in &proc.init {
        collect_proc_state_fields(
            stmt,
            &reserved,
            &proc_ns,
            proc_symbols,
            &state_type_hints,
            &ins_names,
            &out_names,
            &typed_param_names,
            &typed_struct_defs,
            proc_primary_output_types,
            &struct_symbols,
            struct_defs,
            ctor_symbols,
            true,
            &mut state,
            errors,
        );
    }
    for stmt in &proc.sample {
        collect_proc_state_fields(
            stmt,
            &reserved,
            &proc_ns,
            proc_symbols,
            &state_type_hints,
            &ins_names,
            &out_names,
            &typed_param_names,
            &typed_struct_defs,
            proc_primary_output_types,
            &struct_symbols,
            struct_defs,
            ctor_symbols,
            false,
            &mut state,
            errors,
        );
    }
    for stmt in &proc.block_pre {
        collect_proc_state_fields(
            stmt,
            &reserved,
            &proc_ns,
            proc_symbols,
            &state_type_hints,
            &ins_names,
            &out_names,
            &typed_param_names,
            &typed_struct_defs,
            proc_primary_output_types,
            &struct_symbols,
            struct_defs,
            ctor_symbols,
            false,
            &mut state,
            errors,
        );
    }
    for stmt in &proc.block_post {
        collect_proc_state_fields(
            stmt,
            &reserved,
            &proc_ns,
            proc_symbols,
            &state_type_hints,
            &ins_names,
            &out_names,
            &typed_param_names,
            &typed_struct_defs,
            proc_primary_output_types,
            &struct_symbols,
            struct_defs,
            ctor_symbols,
            false,
            &mut state,
            errors,
        );
    }

    let mut state_scalar_names = state.scalars.keys().cloned().collect::<Vec<_>>();
    state_scalar_names.sort();
    let mut state_data_names = state.data.keys().cloned().collect::<Vec<_>>();
    state_data_names.sort();
    let mut struct_instance_names = state.struct_instances.keys().cloned().collect::<Vec<_>>();
    struct_instance_names.sort();

    let mut fields = Vec::<StructField>::new();
    let mut instance_fields = HashMap::<String, HashSet<String>>::new();
    for spec in &param_specs {
        for slot in &spec.slots {
            fields.push(StructField {
                name: slot.name.clone(),
                ty: FieldType::Scalar(slot.ty),
                default: slot.default.clone(),
            });
        }
    }
    for out_name in &outs {
        let out_ty = *out_types.get(out_name).unwrap_or(&PrimitiveType::F32);
        fields.push(StructField {
            name: out_name.clone(),
            ty: FieldType::Scalar(out_ty),
            default: None,
        });
    }
    for name in &state_scalar_names {
        if reserved.contains(name) {
            continue;
        }
        fields.push(StructField {
            name: name.clone(),
            ty: FieldType::Scalar(*state.scalars.get(name).unwrap_or(&PrimitiveType::F32)),
            default: None,
        });
    }
    for name in &state_data_names {
        if reserved.contains(name) {
            continue;
        }
        if let Some(spec) = state.data.get(name) {
            let _ = eval_data_size_expr(
                &spec.size,
                options,
                &format!("processor '{}.{}' Data size", proc.name, name),
                errors,
            );
            fields.push(StructField {
                name: name.clone(),
                ty: FieldType::Data(spec.clone()),
                default: None,
            });
        }
    }
    for instance in &struct_instance_names {
        if reserved.contains(instance) {
            continue;
        }
        let Some(state_struct) = state.struct_instances.get(instance) else {
            continue;
        };
        let Some(struct_def) =
            resolve_proc_state_struct_def(&proc.name, instance, state_struct, struct_defs, errors)
        else {
            continue;
        };
        let mut member_names = HashSet::<String>::new();
        for field in &struct_def.fields {
            let flat_name = nested_field_name(instance, &field.name);
            member_names.insert(field.name.clone());
            match &field.ty {
                FieldType::Scalar(prim) => {
                    fields.push(StructField {
                        name: flat_name,
                        ty: FieldType::Scalar(*prim),
                        default: None,
                    });
                }
                FieldType::Data(spec) => {
                    let _ = eval_data_size_expr(
                        &spec.size,
                        options,
                        &format!(
                            "processor '{}.{}' struct field '{}' Data size",
                            proc.name, instance, field.name
                        ),
                        errors,
                    );
                    fields.push(StructField {
                        name: flat_name,
                        ty: FieldType::Data(spec.clone()),
                        default: None,
                    });
                }
                FieldType::Generic(param) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{}' state symbol '{}' field '{}' uses unresolved generic type parameter '{}'",
                            proc.name, instance, field.name, param
                        ),
                        0,
                        0,
                    ));
                }
            }
        }
        instance_fields.insert(instance.clone(), member_names);
    }

    let field_names = fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<HashSet<_>>();
    let data_field_names = fields
        .iter()
        .filter_map(|f| match f.ty {
            FieldType::Data(_) => Some(f.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();

    ProcBaseShape {
        ins,
        outs,
        in_ports,
        param_specs,
        buffer_specs,
        in_types,
        in_array_slots,
        field_array_slots,
        state,
        instance_fields,
        fields,
        field_names,
        data_field_names,
    }
}

fn resolve_proc_state_struct_def(
    proc_name: &str,
    instance: &str,
    state_struct: &ProcStructState,
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    errors: &mut Vec<Diagnostic>,
) -> Option<omni_frontend::StructDef> {
    let Some(struct_template) = struct_defs.get(&state_struct.struct_name) else {
        errors.push(Diagnostic::semantic(
            format!(
                "processor '{}' state symbol '{}' references unknown struct '{}'",
                proc_name, instance, state_struct.struct_name
            ),
            0,
            0,
        ));
        return None;
    };

    if state_struct.type_args.is_empty() {
        if !struct_template.type_params.is_empty() {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}' state symbol '{}' uses generic struct '{}' without type arguments",
                    proc_name, instance, state_struct.struct_name
                ),
                0,
                0,
            ));
            return None;
        }
        return Some(struct_template.clone());
    }

    let Some(specialized) =
        specialize_generic_struct_template(struct_template, &state_struct.type_args, errors)
    else {
        return None;
    };
    Some(specialized)
}

fn build_proc_lowering_shape(
    proc_name: &str,
    base_shapes: &HashMap<String, ProcBaseShape>,
    cache: &mut HashMap<String, ProcLoweringShape>,
    visiting: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcLoweringShape> {
    if let Some(cached) = cache.get(proc_name) {
        return Some(cached.clone());
    }

    if let Some(idx) = visiting.iter().position(|n| n == proc_name) {
        let mut cycle = visiting[idx..].to_vec();
        cycle.push(proc_name.to_owned());
        errors.push(Diagnostic::semantic(
            format!(
                "processor nested-state cycle detected: {}",
                cycle.join(" -> ")
            ),
            0,
            0,
        ));
        return None;
    }

    let Some(base) = base_shapes.get(proc_name).cloned() else {
        errors.push(Diagnostic::semantic(
            format!("unknown processor '{proc_name}'"),
            0,
            0,
        ));
        return None;
    };

    visiting.push(proc_name.to_owned());

    let mut fields = base.fields.clone();
    let mut field_names = base.field_names.clone();
    let mut data_field_names = base.data_field_names.clone();
    let mut field_array_slots = base.field_array_slots.clone();
    let mut nested_fields = base.instance_fields.clone();

    let mut nested_vars = base.state.nested_procs.keys().cloned().collect::<Vec<_>>();
    nested_vars.sort();

    for nested_var in nested_vars {
        let Some(nested_state) = base.state.nested_procs.get(&nested_var) else {
            continue;
        };

        let Some(callee_shape) = build_proc_lowering_shape(
            &nested_state.proc_name,
            base_shapes,
            cache,
            visiting,
            errors,
        ) else {
            continue;
        };

        nested_fields.insert(nested_var.clone(), callee_shape.field_names.clone());
        for (array_base, slots) in &callee_shape.field_array_slots {
            let prefixed_base = nested_field_name(&nested_var, array_base);
            let prefixed_slots = slots
                .iter()
                .map(|slot| nested_field_name(&nested_var, slot))
                .collect::<Vec<_>>();
            field_array_slots.insert(prefixed_base, prefixed_slots);
        }

        let mut nested_callee_fields = callee_shape.fields.clone();
        nested_callee_fields.sort_by(|a, b| a.name.cmp(&b.name));
        for mut nested_field in nested_callee_fields {
            let flat_name = nested_field_name(&nested_var, &nested_field.name);
            if field_names.contains(&flat_name) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' nested field '{}' conflicts with existing field '{}'",
                        proc_name, nested_field.name, flat_name
                    ),
                    0,
                    0,
                ));
                continue;
            }
            nested_field.name = flat_name.clone();
            if matches!(nested_field.ty, FieldType::Data(_)) {
                data_field_names.insert(flat_name.clone());
            }
            field_names.insert(flat_name);
            fields.push(nested_field);
        }
    }

    let _ = visiting.pop();

    let resolved = ProcLoweringShape {
        ins: base.ins,
        outs: base.outs,
        in_ports: base.in_ports,
        param_specs: base.param_specs,
        buffer_specs: base.buffer_specs,
        in_types: base.in_types,
        in_array_slots: base.in_array_slots,
        field_array_slots,
        state: base.state,
        fields,
        field_names,
        data_field_names,
        nested_fields,
    };
    cache.insert(proc_name.to_owned(), resolved.clone());
    Some(resolved)
}

fn nested_field_name(var: &str, field: &str) -> String {
    format!("{var}__{field}")
}

fn nested_init_fn_name(owner_proc: &str, nested_var: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_init")
}

fn nested_step_fn_name(owner_proc: &str, nested_var: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_step")
}

fn nested_block_pre_fn_name(owner_proc: &str, nested_var: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_block_pre")
}

fn nested_block_post_fn_name(owner_proc: &str, nested_var: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_block_post")
}

fn nested_call_out_fn_name(owner_proc: &str, nested_var: &str, out_idx: usize) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_call_out{out_idx}")
}

fn rewrite_nested_field_paths_in_expr(
    expr: &mut Expr,
    nested_fields: &HashMap<String, HashSet<String>>,
) {
    match expr {
        Expr::Var(name) => {
            if let Some((base, field)) = split_simple_field_path(name) {
                if let Some(fields) = nested_fields.get(base) {
                    if fields.contains(field) {
                        *name = format!("self.{}", nested_field_name(base, field));
                    }
                }
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if let Some(fields) = nested_fields.get(root) {
                    if fields.contains(field) {
                        *base = format!("self.{}", nested_field_name(root, field));
                    }
                }
            }
            rewrite_nested_field_paths_in_expr(index, nested_fields);
        }
        Expr::DataCtor { spec, init } => {
            rewrite_nested_field_paths_in_expr(&mut spec.size, nested_fields);
            if let Some(values) = init {
                for value in values {
                    rewrite_nested_field_paths_in_expr(value, nested_fields);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_nested_field_paths_in_expr(lhs, nested_fields);
            rewrite_nested_field_paths_in_expr(rhs, nested_fields);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_nested_field_paths_in_expr(arg, nested_fields);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_nested_field_paths_in_expr(&mut arg.expr, nested_fields);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_nested_field_paths_in_expr(inner, nested_fields)
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_nested_field_paths_in_expr(value, nested_fields);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

fn rewrite_nested_field_paths_in_stmt(
    stmt: &mut Stmt,
    nested_fields: &HashMap<String, HashSet<String>>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some((base, field)) = split_simple_field_path(name) {
                        if let Some(fields) = nested_fields.get(base) {
                            if fields.contains(field) {
                                *name = format!("self.{}", nested_field_name(base, field));
                            }
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(fields) = nested_fields.get(root) {
                            if fields.contains(field) {
                                *base = format!("self.{}", nested_field_name(root, field));
                            }
                        }
                    }
                    rewrite_nested_field_paths_in_expr(index, nested_fields);
                }
            }
            rewrite_nested_field_paths_in_expr(expr, nested_fields);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_nested_field_paths_in_expr(expr, nested_fields)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_nested_field_paths_in_expr(cond, nested_fields);
            for s in then_branch {
                rewrite_nested_field_paths_in_stmt(s, nested_fields);
            }
            for s in else_branch {
                rewrite_nested_field_paths_in_stmt(s, nested_fields);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            rewrite_nested_field_paths_in_expr(start, nested_fields);
            rewrite_nested_field_paths_in_expr(end, nested_fields);
            for s in body {
                rewrite_nested_field_paths_in_stmt(s, nested_fields);
            }
        }
    }
}

fn remap_nested_symbols_in_expr(expr: &mut Expr, remap: &HashMap<String, String>) {
    match expr {
        Expr::Var(name) => {
            if let Some((base, field)) = split_simple_field_path(name) {
                if let Some(mapped) = remap.get(base) {
                    *name = format!("{mapped}.{field}");
                }
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if let Some(mapped) = remap.get(root) {
                    *base = format!("{mapped}.{field}");
                }
            }
            remap_nested_symbols_in_expr(index, remap);
        }
        Expr::DataCtor { spec, init } => {
            remap_nested_symbols_in_expr(&mut spec.size, remap);
            if let Some(values) = init {
                for value in values {
                    remap_nested_symbols_in_expr(value, remap);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            remap_nested_symbols_in_expr(lhs, remap);
            remap_nested_symbols_in_expr(rhs, remap);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                remap_nested_symbols_in_expr(arg, remap);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                remap_nested_symbols_in_expr(&mut arg.expr, remap);
            }
            if let Some(mapped) = remap.get(name) {
                *name = mapped.clone();
            } else if let Some(raw) = name.strip_prefix(PROC_INDEX_SENTINEL_PREFIX) {
                if let Some(mapped) = remap.get(raw) {
                    *name = format!("{PROC_INDEX_SENTINEL_PREFIX}{mapped}");
                }
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            remap_nested_symbols_in_expr(inner, remap)
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                remap_nested_symbols_in_expr(value, remap);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

fn remap_nested_symbols_in_stmt(stmt: &mut Stmt, remap: &HashMap<String, String>) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some((base, field)) = split_simple_field_path(name) {
                        if let Some(mapped) = remap.get(base) {
                            *name = format!("{mapped}.{field}");
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(mapped) = remap.get(root) {
                            *base = format!("{mapped}.{field}");
                        }
                    }
                    remap_nested_symbols_in_expr(index, remap);
                }
            }
            remap_nested_symbols_in_expr(expr, remap);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            remap_nested_symbols_in_expr(expr, remap)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            remap_nested_symbols_in_expr(cond, remap);
            for s in then_branch {
                remap_nested_symbols_in_stmt(s, remap);
            }
            for s in else_branch {
                remap_nested_symbols_in_stmt(s, remap);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            remap_nested_symbols_in_expr(start, remap);
            remap_nested_symbols_in_expr(end, remap);
            for s in body {
                remap_nested_symbols_in_stmt(s, remap);
            }
        }
    }
}

fn prefix_self_fields_in_expr(expr: &mut Expr, prefix: &str, nested_field_names: &HashSet<String>) {
    match expr {
        Expr::Var(name) => {
            if let Some((base, field)) = split_simple_field_path(name) {
                if base == "self" && nested_field_names.contains(field) {
                    *name = format!("self.{}", nested_field_name(prefix, field));
                }
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if root == "self" && nested_field_names.contains(field) {
                    *base = format!("self.{}", nested_field_name(prefix, field));
                }
            }
            prefix_self_fields_in_expr(index, prefix, nested_field_names);
        }
        Expr::DataCtor { spec, init } => {
            prefix_self_fields_in_expr(&mut spec.size, prefix, nested_field_names);
            if let Some(values) = init {
                for value in values {
                    prefix_self_fields_in_expr(value, prefix, nested_field_names);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            prefix_self_fields_in_expr(lhs, prefix, nested_field_names);
            prefix_self_fields_in_expr(rhs, prefix, nested_field_names);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                prefix_self_fields_in_expr(arg, prefix, nested_field_names);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                prefix_self_fields_in_expr(&mut arg.expr, prefix, nested_field_names);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            prefix_self_fields_in_expr(inner, prefix, nested_field_names)
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                prefix_self_fields_in_expr(value, prefix, nested_field_names);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

fn prefix_self_fields_in_stmt(stmt: &mut Stmt, prefix: &str, nested_field_names: &HashSet<String>) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some((base, field)) = split_simple_field_path(name) {
                        if base == "self" && nested_field_names.contains(field) {
                            *name = format!("self.{}", nested_field_name(prefix, field));
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if root == "self" && nested_field_names.contains(field) {
                            *base = format!("self.{}", nested_field_name(prefix, field));
                        }
                    }
                    prefix_self_fields_in_expr(index, prefix, nested_field_names);
                }
            }
            prefix_self_fields_in_expr(expr, prefix, nested_field_names);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            prefix_self_fields_in_expr(expr, prefix, nested_field_names)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            prefix_self_fields_in_expr(cond, prefix, nested_field_names);
            for s in then_branch {
                prefix_self_fields_in_stmt(s, prefix, nested_field_names);
            }
            for s in else_branch {
                prefix_self_fields_in_stmt(s, prefix, nested_field_names);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            prefix_self_fields_in_expr(start, prefix, nested_field_names);
            prefix_self_fields_in_expr(end, prefix, nested_field_names);
            for s in body {
                prefix_self_fields_in_stmt(s, prefix, nested_field_names);
            }
        }
    }
}

fn rewrite_nested_proc_calls_in_expr(
    expr: &mut Expr,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => {
            rewrite_nested_proc_calls_in_expr(index, owner_proc, nested_instances, proc_api, errors)
        }
        Expr::DataCtor { spec, init } => {
            rewrite_nested_proc_calls_in_expr(
                &mut spec.size,
                owner_proc,
                nested_instances,
                proc_api,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_nested_proc_calls_in_expr(
                        value,
                        owner_proc,
                        nested_instances,
                        proc_api,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_nested_proc_calls_in_expr(lhs, owner_proc, nested_instances, proc_api, errors);
            rewrite_nested_proc_calls_in_expr(rhs, owner_proc, nested_instances, proc_api, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_nested_proc_calls_in_expr(
                    arg,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    errors,
                );
            }
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_nested_proc_calls_in_expr(
                    value,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    errors,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                rewrite_nested_proc_calls_in_expr(
                    &mut arg.expr,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    errors,
                );
            }
            if let Some(var) = name.strip_prefix(PROC_INDEX_SENTINEL_PREFIX) {
                if let Some(instance) = nested_instances.get(var) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown processor type '{proc_name}'"),
                            0,
                            0,
                        ));
                        return;
                    };
                    let index_pos = args.iter().position(|a| {
                        a.name
                            .as_ref()
                            .map(|n| n == PROC_INDEX_SENTINEL_ARG)
                            .unwrap_or(false)
                    });
                    let Some(index_pos) = index_pos else {
                        errors.push(Diagnostic::semantic(
                            "processor indexed call is missing index expression",
                            0,
                            0,
                        ));
                        return;
                    };
                    let idx_arg = args.remove(index_pos);
                    let Some(out_idx) = try_constant_proc_out_index(&idx_arg.expr) else {
                        errors.push(Diagnostic::semantic(
                            "processor indexed call requires a compile-time integer index",
                            0,
                            0,
                        ));
                        return;
                    };
                    if out_idx >= api.outs.len() {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor output index {out_idx} is out of range (outs: {})",
                                api.outs.len()
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Var("self".to_owned()),
                    });
                    let expanded_args = expand_proc_call_args(args, api, var, errors);
                    rewritten.extend(expanded_args);
                    let expanded_buffers = expand_proc_buffer_call_args(instance, api, var, errors);
                    rewritten.extend(expanded_buffers);
                    *name = nested_call_out_fn_name(owner_proc, var, out_idx);
                    *args = rewritten;
                }
                return;
            }
            if let Some(instance) = nested_instances.get(name) {
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    errors.push(Diagnostic::semantic(
                        format!("unknown processor type '{proc_name}'"),
                        0,
                        0,
                    ));
                    return;
                };
                if api.outs.len() != 1 {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor call '{}(...)' has {} outputs; use '{}(...)[index]'",
                            name,
                            api.outs.len(),
                            name
                        ),
                        0,
                        0,
                    ));
                    return;
                }
                let nested_var = name.clone();
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: Expr::Var("self".to_owned()),
                });
                let expanded_args = expand_proc_call_args(args, api, name, errors);
                rewritten.extend(expanded_args);
                let expanded_buffers =
                    expand_proc_buffer_call_args(instance, api, &nested_var, errors);
                rewritten.extend(expanded_buffers);
                *name = nested_call_out_fn_name(owner_proc, &nested_var, 0);
                *args = rewritten;
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_nested_proc_calls_in_expr(inner, owner_proc, nested_instances, proc_api, errors)
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn rewrite_nested_proc_calls_in_stmt(
    stmt: &mut Stmt,
    owner_proc: &str,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { expr, .. } => {
            rewrite_nested_proc_calls_in_expr(expr, owner_proc, nested_instances, proc_api, errors)
        }
        Stmt::Expr { expr, .. } => {
            rewrite_nested_proc_calls_in_expr(expr, owner_proc, nested_instances, proc_api, errors);
            if let Expr::UserCall { name, args, .. } = expr {
                if let Some(instance) = nested_instances.get(name) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown processor type '{proc_name}'"),
                            0,
                            0,
                        ));
                        return;
                    };
                    let nested_var = name.clone();
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Var("self".to_owned()),
                    });
                    let expanded_args = expand_proc_call_args(args, api, &nested_var, errors);
                    rewritten.extend(expanded_args);
                    let expanded_buffers =
                        expand_proc_buffer_call_args(instance, api, &nested_var, errors);
                    rewritten.extend(expanded_buffers);
                    *name = nested_step_fn_name(owner_proc, &nested_var);
                    *args = rewritten;
                }
            }
        }
        Stmt::Return { expr, .. } => {
            rewrite_nested_proc_calls_in_expr(expr, owner_proc, nested_instances, proc_api, errors)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_nested_proc_calls_in_expr(cond, owner_proc, nested_instances, proc_api, errors);
            for s in then_branch {
                rewrite_nested_proc_calls_in_stmt(
                    s,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    errors,
                );
            }
            for s in else_branch {
                rewrite_nested_proc_calls_in_stmt(
                    s,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    errors,
                );
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            rewrite_nested_proc_calls_in_expr(
                start,
                owner_proc,
                nested_instances,
                proc_api,
                errors,
            );
            rewrite_nested_proc_calls_in_expr(end, owner_proc, nested_instances, proc_api, errors);
            for s in body {
                rewrite_nested_proc_calls_in_stmt(
                    s,
                    owner_proc,
                    nested_instances,
                    proc_api,
                    errors,
                );
            }
        }
    });
}

fn rewrite_owner_proc_stmt(
    mut stmt: Stmt,
    owner_proc: &str,
    field_names: &HashSet<String>,
    data_field_names: &HashSet<String>,
    ins_names: &HashSet<String>,
    field_array_slots: &HashMap<String, Vec<String>>,
    in_array_slots: &HashMap<String, Vec<String>>,
    nested_fields: &HashMap<String, HashSet<String>>,
    nested_instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Stmt> {
    rewrite_nested_field_paths_in_stmt(&mut stmt, nested_fields);
    rewrite_nested_proc_calls_in_stmt(&mut stmt, owner_proc, nested_instances, proc_api, errors);
    rewrite_proc_stmt_symbols(
        &stmt,
        owner_proc,
        field_names,
        data_field_names,
        ins_names,
        field_array_slots,
        in_array_slots,
        errors,
    )
}

fn expand_nested_struct_ctor_assign(
    instance_var: &str,
    ctor_name: &str,
    ctor_args: &[CallArg],
    struct_def: &omni_frontend::StructDef,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Stmt> {
    if !struct_def.type_params.is_empty() {
        errors.push(Diagnostic::semantic(
            format!(
                "processor state constructor '{instance_var} = {ctor_name}(...)' does not support generic struct templates"
            ),
            0,
            0,
        ));
        return Vec::new();
    }

    let scalar_fields = struct_def
        .fields
        .iter()
        .filter(|f| matches!(f.ty, FieldType::Scalar(_)))
        .collect::<Vec<_>>();
    let scalar_param_names = scalar_fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<Vec<_>>();
    let scalar_defaults = scalar_fields
        .iter()
        .map(|f| f.default.clone().or(Some(Expr::Number(0.0))))
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        ctor_args,
        &scalar_param_names,
        &scalar_defaults,
        false,
        false,
        &format!("processor state struct constructor '{instance_var} = {ctor_name}(...)'"),
        errors,
    );

    let mut out = Vec::<Stmt>::new();
    let mut scalar_idx = 0usize;
    for field in &struct_def.fields {
        let flat = nested_field_name(instance_var, &field.name);
        match &field.ty {
            FieldType::Scalar(_) => {
                let value = resolved
                    .get(scalar_idx)
                    .copied()
                    .flatten()
                    .cloned()
                    .or_else(|| scalar_defaults.get(scalar_idx).cloned().flatten())
                    .unwrap_or(Expr::Number(0.0));
                out.push(Stmt::Assign {
                    loc: None,
                    target: AssignTarget::Var(flat),
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    expr: value,
                });
                scalar_idx += 1;
            }
            FieldType::Data(_) => {
                if let Some(default) = &field.default {
                    out.push(Stmt::Assign {
                        loc: None,
                        target: AssignTarget::Var(flat),
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        expr: default.clone(),
                    });
                }
            }
            FieldType::Generic(param) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor state struct field '{}.{}' uses unresolved generic parameter '{}'",
                        instance_var, field.name, param
                    ),
                    0,
                    0,
                ));
            }
        }
    }
    out
}

fn expand_nested_proc_ctor_assign(
    owner_proc: &str,
    nested_var: &str,
    ctor_name: &str,
    ctor_args: &[CallArg],
    callee_param_specs: &[ProcParamSpec],
    callee_buffer_specs: &[ProcBufferSpec],
    errors: &mut Vec<Diagnostic>,
) -> (Vec<Stmt>, Vec<Expr>) {
    let mut param_names = callee_param_specs
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let mut param_defaults = callee_param_specs
        .iter()
        .map(|p| {
            if p.slots.iter().all(|s| s.default.is_some()) {
                Some(Expr::Number(0.0))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for buffer in callee_buffer_specs {
        param_names.push(buffer.name.clone());
        param_defaults.push(None);
    }
    let resolved = resolve_call_args(
        ctor_args,
        &param_names,
        &param_defaults,
        false,
        true,
        &format!(
            "processor constructor '{}(...)' for nested state '{}'",
            ctor_name, nested_var
        ),
        errors,
    );

    let mut out = Vec::<Stmt>::new();
    let mut bound_buffers = Vec::<Expr>::new();
    for (idx, param) in callee_param_specs.iter().enumerate() {
        let values = match resolved.get(idx).copied().flatten() {
            Some(expr) => expand_expr_to_slots(
                expr,
                param.slots.len(),
                &format!(
                    "processor constructor '{}(...)' argument '{}'",
                    ctor_name, param.name
                ),
                errors,
            ),
            None => param
                .slots
                .iter()
                .map(|slot| slot.default.clone().unwrap_or(Expr::Number(0.0)))
                .collect::<Vec<_>>(),
        };
        for (slot_idx, slot) in param.slots.iter().enumerate() {
            let value = values
                .get(slot_idx)
                .cloned()
                .unwrap_or_else(|| slot.default.clone().unwrap_or(Expr::Number(0.0)));
            let value = if let Some(range) = slot.range {
                clamp_expr_to_range(value, range)
            } else {
                value
            };
            out.push(Stmt::Assign {
                loc: None,
                target: AssignTarget::Var(nested_field_name(nested_var, &slot.name)),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: value,
            });
        }
    }
    for (buffer_idx, buffer_spec) in callee_buffer_specs.iter().enumerate() {
        let resolved_idx = callee_param_specs.len() + buffer_idx;
        let Some(expr) = resolved.get(resolved_idx).copied().flatten().cloned() else {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor constructor '{ctor_name}(...)' for nested state '{nested_var}' is missing required buffer argument '{}'",
                    buffer_spec.name
                ),
                0,
                0,
            ));
            continue;
        };
        if !matches!(expr, Expr::Var(_)) {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor constructor '{ctor_name}(...)' buffer argument '{}' for nested state '{nested_var}' must be a buffer symbol",
                    buffer_spec.name
                ),
                0,
                0,
            ));
        }
        bound_buffers.push(expr);
    }
    out.push(Stmt::Expr {
        loc: None,
        expr: Expr::UserCall {
            name: nested_init_fn_name(owner_proc, nested_var),
            type_args: Vec::new(),
            args: vec![CallArg {
                name: None,
                expr: Expr::Var("self".to_owned()),
            }],
        },
    });
    (out, bound_buffers)
}

fn expand_proc_instance_ctor_assign(
    instance_var: &str,
    ctor_name: &str,
    ctor_args: &[CallArg],
    param_specs: &[ProcParamSpec],
    buffer_specs: &[ProcBufferSpec],
    errors: &mut Vec<Diagnostic>,
) -> (Vec<Stmt>, Vec<Expr>) {
    let mut param_names = param_specs
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let mut param_defaults = param_specs
        .iter()
        .map(|p| {
            if p.slots.iter().all(|s| s.default.is_some()) {
                Some(Expr::Number(0.0))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for buffer in buffer_specs {
        param_names.push(buffer.name.clone());
        param_defaults.push(None);
    }
    let resolved = resolve_call_args(
        ctor_args,
        &param_names,
        &param_defaults,
        false,
        true,
        &format!("processor constructor '{ctor_name}(...)' for instance '{instance_var}'"),
        errors,
    );

    let mut out = Vec::<Stmt>::new();
    let mut bound_buffers = Vec::<Expr>::new();
    for (idx, param) in param_specs.iter().enumerate() {
        let values = match resolved.get(idx).copied().flatten() {
            Some(expr) => expand_expr_to_slots(
                expr,
                param.slots.len(),
                &format!(
                    "processor constructor '{}(...)' argument '{}'",
                    ctor_name, param.name
                ),
                errors,
            ),
            None => param
                .slots
                .iter()
                .map(|slot| slot.default.clone().unwrap_or(Expr::Number(0.0)))
                .collect::<Vec<_>>(),
        };
        for (slot_idx, slot) in param.slots.iter().enumerate() {
            let value = values
                .get(slot_idx)
                .cloned()
                .unwrap_or_else(|| slot.default.clone().unwrap_or(Expr::Number(0.0)));
            let value = if let Some(range) = slot.range {
                clamp_expr_to_range(value, range)
            } else {
                value
            };
            out.push(Stmt::Assign {
                loc: None,
                target: AssignTarget::Var(format!("{instance_var}.{}", slot.name)),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: value,
            });
        }
    }
    for (buffer_idx, buffer_spec) in buffer_specs.iter().enumerate() {
        let resolved_idx = param_specs.len() + buffer_idx;
        let Some(expr) = resolved.get(resolved_idx).copied().flatten().cloned() else {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor constructor '{ctor_name}(...)' for instance '{instance_var}' is missing required buffer argument '{}'",
                    buffer_spec.name
                ),
                0,
                0,
            ));
            continue;
        };
        if !matches!(expr, Expr::Var(_)) {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor constructor '{ctor_name}(...)' buffer argument '{}' must be a buffer symbol",
                    buffer_spec.name
                ),
                0,
                0,
            ));
        }
        bound_buffers.push(expr);
    }
    (out, bound_buffers)
}

fn collect_nested_proc_instances(
    shape: &ProcLoweringShape,
    path_prefix: Option<&str>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    out: &mut Vec<(String, String)>,
) {
    let mut nested_vars = shape.state.nested_procs.keys().cloned().collect::<Vec<_>>();
    nested_vars.sort();
    for nested_var in nested_vars {
        let Some(nested_state) = shape.state.nested_procs.get(&nested_var) else {
            continue;
        };
        let full_path = if let Some(prefix) = path_prefix {
            nested_field_name(prefix, &nested_var)
        } else {
            nested_var.clone()
        };
        out.push((full_path.clone(), nested_state.proc_name.clone()));
        if let Some(child_shape) = lowering_shapes.get(&nested_state.proc_name) {
            collect_nested_proc_instances(child_shape, Some(&full_path), lowering_shapes, out);
        }
    }
}

fn lower_callee_stmt_for_nested_wrapper(
    stmt: &Stmt,
    owner_proc: &str,
    nested_path: &str,
    callee_shape: &ProcLoweringShape,
    callee_nested_instances: &HashMap<String, ProcCallInstance>,
    callee_ins_names: &HashSet<String>,
    callee_field_array_slots: &HashMap<String, Vec<String>>,
    callee_in_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Stmt> {
    with_stmt_diag_context(stmt, || {
        let mut stmt = stmt.clone();
        let remap = callee_shape
            .state
            .nested_procs
            .keys()
            .map(|name| (name.clone(), nested_field_name(nested_path, name)))
            .collect::<HashMap<_, _>>();
        remap_nested_symbols_in_stmt(&mut stmt, &remap);

        let nested_fields = callee_shape
            .nested_fields
            .iter()
            .map(|(name, fields)| (nested_field_name(nested_path, name), fields.clone()))
            .collect::<HashMap<_, _>>();
        let nested_instances = callee_nested_instances
            .iter()
            .map(|(name, instance)| {
                let mapped_name = nested_field_name(nested_path, name);
                let mut mapped_instance = instance.clone();
                for expr in &mut mapped_instance.buffer_args {
                    remap_nested_symbols_in_expr(expr, &remap);
                }
                (mapped_name, mapped_instance)
            })
            .collect::<HashMap<_, _>>();
        rewrite_nested_field_paths_in_stmt(&mut stmt, &nested_fields);
        rewrite_nested_proc_calls_in_stmt(
            &mut stmt,
            owner_proc,
            &nested_instances,
            proc_api,
            errors,
        );

        let mapped_field_array_slots = callee_field_array_slots
            .iter()
            .map(|(base, slots)| {
                (
                    base.clone(),
                    slots
                        .iter()
                        .map(|slot| nested_field_name(nested_path, slot))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();

        let lowered = rewrite_proc_stmt_symbols(
            &stmt,
            owner_proc,
            &callee_shape.field_names,
            &callee_shape.data_field_names,
            callee_ins_names,
            &mapped_field_array_slots,
            callee_in_array_slots,
            errors,
        )?;
        let mut lowered = lowered;
        prefix_self_fields_in_stmt(&mut lowered, nested_path, &callee_shape.field_names);
        Some(lowered)
    })
}

fn desugar_processors(
    mut program: Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Program {
    let initial_proc_defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Proc(p) => Some(p.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if initial_proc_defs.is_empty() {
        return program;
    }

    let mut generic_proc_templates = HashMap::<String, ProcessorDef>::new();
    for p in &initial_proc_defs {
        if p.type_params.is_empty() {
            continue;
        }
        if generic_proc_templates.contains_key(&p.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate generic processor '{}'", p.name),
                0,
                0,
            ));
            continue;
        }
        let mut seen_tp = HashSet::<String>::new();
        for tp in &p.type_params {
            if !seen_tp.insert(tp.clone()) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "duplicate generic type parameter '{}' in processor '{}'",
                        tp, p.name
                    ),
                    0,
                    0,
                ));
            }
        }
        generic_proc_templates.insert(p.name.clone(), p.clone());
    }

    if !generic_proc_templates.is_empty() {
        let mut generated_specializations = HashMap::<String, ProcessorDef>::new();
        let top_level_seed = generic_inference_seed_for_top_level(&program.blocks);
        let empty_seed = GenericInferenceLocals::default();
        for block in &mut program.blocks {
            match block {
                Block::Struct(s) => {
                    let struct_ns = namespace_of_symbol(&s.name);
                    for field in &mut s.fields {
                        if let Some(default) = &mut field.default {
                            let mut locals = GenericInferenceLocals::default();
                            rewrite_generic_proc_ctor_expr(
                                default,
                                &generic_proc_templates,
                                &mut generated_specializations,
                                errors,
                                &mut locals,
                                &struct_ns,
                            );
                        }
                    }
                    for method in &mut s.methods {
                        rewrite_generic_proc_ctor_stmt_list(
                            &mut method.body,
                            &generic_proc_templates,
                            &mut generated_specializations,
                            errors,
                            &empty_seed,
                            &struct_ns,
                        );
                    }
                }
                Block::Def(d) => {
                    let def_ns = namespace_of_symbol(&d.name);
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut d.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &empty_seed,
                        &def_ns,
                    );
                }
                Block::Proc(p) => {
                    if !p.type_params.is_empty() {
                        continue;
                    }
                    let proc_ns = namespace_of_symbol(&p.name);
                    let proc_seed = generic_inference_seed_for_processor(p);
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut p.init,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut p.block_pre,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut p.sample,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut p.block_post,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                }
                Block::Init(stmts) => {
                    rewrite_generic_proc_ctor_stmt_list(
                        stmts,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &top_level_seed,
                        "",
                    );
                }
                Block::Block(exec) => {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut exec.pre,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &top_level_seed,
                        "",
                    );
                    if let Some(sample) = &mut exec.sample {
                        rewrite_generic_proc_ctor_stmt_list(
                            sample,
                            &generic_proc_templates,
                            &mut generated_specializations,
                            errors,
                            &top_level_seed,
                            "",
                        );
                    }
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut exec.post,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &top_level_seed,
                        "",
                    );
                }
                Block::Sample(stmts) => {
                    rewrite_generic_proc_ctor_stmt_list(
                        stmts,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &top_level_seed,
                        "",
                    );
                }
                _ => {}
            }
        }
        finalize_generated_generic_proc_specializations(
            &generic_proc_templates,
            &mut generated_specializations,
            errors,
        );

        program
            .blocks
            .retain(|b| !matches!(b, Block::Proc(p) if !p.type_params.is_empty()));
        let mut generated = generated_specializations.into_values().collect::<Vec<_>>();
        generated.sort_by(|a, b| a.name.cmp(&b.name));
        for proc in generated {
            program.blocks.push(Block::Proc(proc));
        }
    }

    let proc_defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Proc(p) => Some(p.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if proc_defs.is_empty() {
        return program;
    }

    let struct_symbols = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let struct_defs_by_name = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some((s.name.clone(), s.clone())),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let proc_symbols = proc_defs
        .iter()
        .map(|p| p.name.clone())
        .collect::<HashSet<_>>();
    let ctor_symbols = struct_symbols
        .iter()
        .cloned()
        .chain(proc_symbols.iter().cloned())
        .collect::<HashSet<_>>();
    let proc_defs_by_name = proc_defs
        .iter()
        .map(|p| (p.name.clone(), p.clone()))
        .collect::<HashMap<_, _>>();
    let proc_primary_output_types = proc_defs
        .iter()
        .map(|p| (p.name.clone(), infer_primary_output_type_from_processor(p)))
        .collect::<HashMap<_, _>>();
    let pre_desugar_defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(d) => Some(d.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut pre_desugar_fn_signatures = HashMap::<String, FnSignature>::new();
    for def in &pre_desugar_defs {
        pre_desugar_fn_signatures
            .entry(def.name.clone())
            .or_insert_with(|| FnSignature {
                params: def.params.iter().map(|p| p.name.clone()).collect(),
                defaults: def.params.iter().map(|p| p.default.clone()).collect(),
                param_types: def.params.iter().map(|p| p.ty.clone()).collect(),
                type_params: def.type_params.clone(),
            });
    }
    let pre_desugar_def_return_types = infer_def_return_types(
        &pre_desugar_defs,
        &pre_desugar_fn_signatures,
        &HashMap::new(),
    );

    let mut base_shapes = HashMap::<String, ProcBaseShape>::new();
    let mut proc_api = HashMap::<String, ProcApi>::new();
    let mut proc_order = Vec::<String>::new();
    for proc in &proc_defs {
        if !proc.has_sample_block {
            errors.push(Diagnostic::semantic(
                format!("processor '{}' must declare sample block", proc.name),
                0,
                0,
            ));
        }
        if base_shapes.contains_key(&proc.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate processor '{}'", proc.name),
                0,
                0,
            ));
            continue;
        }
        let shape = compute_proc_shape(
            proc,
            options,
            &proc_symbols,
            &proc_primary_output_types,
            &struct_defs_by_name,
            &ctor_symbols,
            &pre_desugar_def_return_types,
            errors,
        );
        if shape.outs.is_empty() {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor '{}' must declare outs block or assign to outN in sample",
                    proc.name
                ),
                0,
                0,
            ));
            continue;
        }
        proc_api.insert(
            proc.name.clone(),
            ProcApi {
                ins: shape.in_ports.clone(),
                params: shape
                    .param_specs
                    .iter()
                    .flat_map(|spec| spec.slots.iter().cloned())
                    .map(|slot| (slot.name.clone(), slot))
                    .collect::<HashMap<_, _>>(),
                outs: shape.outs.clone(),
                buffers: shape.buffer_specs.clone(),
                has_block: proc.has_block_block,
            },
        );
        base_shapes.insert(proc.name.clone(), shape);
        proc_order.push(proc.name.clone());
    }

    let mut lowering_shapes = HashMap::<String, ProcLoweringShape>::new();
    for proc_name in &proc_order {
        let mut visiting = Vec::<String>::new();
        let _ = build_proc_lowering_shape(
            proc_name,
            &base_shapes,
            &mut lowering_shapes,
            &mut visiting,
            errors,
        );
    }

    let effective_proc_blocks =
        compute_effective_proc_block_flags(&proc_order, &proc_defs_by_name, &base_shapes);
    for (proc_name, api) in &mut proc_api {
        api.has_block = *effective_proc_blocks
            .get(proc_name)
            .unwrap_or(&api.has_block);
    }

    let mut generated_structs = Vec::<Block>::new();
    let mut generated_defs = Vec::<Block>::new();
    for proc_name in &proc_order {
        let Some(proc) = proc_defs_by_name.get(proc_name) else {
            continue;
        };
        let Some(shape) = lowering_shapes.get(proc_name).cloned() else {
            continue;
        };

        let mut nested_vars = shape.state.nested_procs.keys().cloned().collect::<Vec<_>>();
        nested_vars.sort();
        let mut nested_instances = HashMap::<String, ProcCallInstance>::new();
        for nested_var in &nested_vars {
            let Some(nested_state) = shape.state.nested_procs.get(nested_var) else {
                continue;
            };
            if !lowering_shapes.contains_key(&nested_state.proc_name) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{}' nested state '{}' references unknown processor '{}'",
                        proc.name, nested_var, nested_state.proc_name
                    ),
                    0,
                    0,
                ));
                continue;
            }
            nested_instances.insert(
                nested_var.clone(),
                ProcCallInstance {
                    proc_name: nested_state.proc_name.clone(),
                    buffer_args: Vec::new(),
                },
            );
        }

        generated_structs.push(Block::Struct(omni_frontend::StructDef {
            name: proc.name.clone(),
            type_params: Vec::new(),
            fields: shape.fields.clone(),
            methods: Vec::new(),
        }));

        let mut read_lens = shape
            .in_array_slots
            .values()
            .chain(shape.field_array_slots.values())
            .map(|slots| slots.len())
            .filter(|len| *len > 1)
            .collect::<Vec<_>>();
        read_lens.sort_unstable();
        read_lens.dedup();
        for len in read_lens {
            generated_defs.push(Block::Def(build_proc_read_helper(&proc.name, len, false)));
            generated_defs.push(Block::Def(build_proc_read_helper(&proc.name, len, true)));
        }

        let mut generated_write_helpers = HashSet::<String>::new();
        let mut write_slots = shape
            .field_array_slots
            .values()
            .cloned()
            .collect::<Vec<Vec<String>>>();
        write_slots.sort();
        write_slots.dedup();
        for slots in write_slots {
            let clamp_name = proc_write_helper_name(&proc.name, &slots, false);
            if generated_write_helpers.insert(clamp_name) {
                generated_defs.push(Block::Def(build_proc_write_helper(
                    &proc.name, &slots, false,
                )));
            }
            let unsafe_name = proc_write_helper_name(&proc.name, &slots, true);
            if generated_write_helpers.insert(unsafe_name) {
                generated_defs.push(Block::Def(build_proc_write_helper(
                    &proc.name, &slots, true,
                )));
            }
        }

        let mut ins_names = shape.ins.iter().cloned().collect::<HashSet<_>>();
        for port in &shape.in_ports {
            ins_names.insert(port.name.clone());
        }
        for buffer in &shape.buffer_specs {
            ins_names.insert(buffer.name.clone());
        }

        let mut init_body = Vec::<Stmt>::new();
        for stmt in &proc.init {
            if let Stmt::Assign {
                target: AssignTarget::Var(var),
                expr:
                    Expr::UserCall {
                        name: ctor_name,
                        type_args,
                        args,
                        ..
                    },
                ..
            } = stmt
            {
                if let Some(nested_state) = shape.state.nested_procs.get(var) {
                    if !type_args.is_empty() {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor '{}' is not generic and cannot take type arguments",
                                ctor_name
                            ),
                            0,
                            0,
                        ));
                    }
                    if nested_state.proc_name != *ctor_name {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor state symbol '{}' has conflicting processor types '{}' and '{}'",
                                var, nested_state.proc_name, ctor_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    }
                    let Some(callee_shape) = lowering_shapes.get(&nested_state.proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor '{}' nested state '{}' references unknown processor '{}'",
                                proc.name, var, nested_state.proc_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    };
                    let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                        &proc.name,
                        var,
                        ctor_name,
                        args,
                        &callee_shape.param_specs,
                        &callee_shape.buffer_specs,
                        errors,
                    );
                    if let Some(instance) = nested_instances.get_mut(var) {
                        instance.buffer_args = bound_buffers;
                    }
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.data_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            &proc_api,
                            errors,
                        ) {
                            init_body.push(rewritten);
                        }
                    }
                    continue;
                }
                if let Some(state_struct) = shape.state.struct_instances.get(var) {
                    if state_struct.struct_name != *ctor_name {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor state symbol '{}' has conflicting struct types '{}' and '{}'",
                                var, state_struct.struct_name, ctor_name
                            ),
                            0,
                            0,
                        ));
                        continue;
                    }
                    let Some(struct_def) = resolve_proc_state_struct_def(
                        &proc.name,
                        var,
                        state_struct,
                        &struct_defs_by_name,
                        errors,
                    ) else {
                        continue;
                    };
                    if !type_args.is_empty() {
                        let Some(resolved_type_args) = resolve_explicit_call_type_args(
                            type_args,
                            &format!("processor state constructor '{} = {}(...)'", var, ctor_name),
                            errors,
                        ) else {
                            continue;
                        };
                        if resolved_type_args.as_slice() != state_struct.type_args.as_slice() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor state constructor '{} = {}(...)' uses type arguments inconsistent with the declared state specialization",
                                    var, ctor_name
                                ),
                                0,
                                0,
                            ));
                        }
                    }
                    let expanded =
                        expand_nested_struct_ctor_assign(var, ctor_name, args, &struct_def, errors);
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.data_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            &proc_api,
                            errors,
                        ) {
                            init_body.push(rewritten);
                        }
                    }
                    continue;
                }
            }
            if let Some(rewritten) = rewrite_owner_proc_stmt(
                stmt.clone(),
                &proc.name,
                &shape.field_names,
                &shape.data_field_names,
                &ins_names,
                &shape.field_array_slots,
                &shape.in_array_slots,
                &shape.nested_fields,
                &nested_instances,
                &proc_api,
                errors,
            ) {
                init_body.push(rewritten);
            }
        }

        generated_defs.push(Block::Def(FunctionDef {
            type_params: Vec::new(),
            name: format!("{}{}", proc.name, PROC_INIT_FN_SUFFIX),
            params: vec![omni_frontend::FnParamDecl {
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                default: None,
            }],
            body: init_body,
        }));

        let mut nested_paths = Vec::<(String, String)>::new();
        collect_nested_proc_instances(&shape, None, &lowering_shapes, &mut nested_paths);
        for (nested_path, callee_proc_name) in nested_paths {
            let Some(callee_proc) = proc_defs_by_name.get(&callee_proc_name) else {
                continue;
            };
            let Some(callee_shape) = lowering_shapes.get(&callee_proc_name).cloned() else {
                continue;
            };
            let mut callee_ins_names = callee_shape.ins.iter().cloned().collect::<HashSet<_>>();
            for port in &callee_shape.in_ports {
                callee_ins_names.insert(port.name.clone());
            }
            for buffer in &callee_shape.buffer_specs {
                callee_ins_names.insert(buffer.name.clone());
            }
            let mut callee_nested_instances = callee_shape
                .state
                .nested_procs
                .iter()
                .map(|(name, state)| {
                    (
                        name.clone(),
                        ProcCallInstance {
                            proc_name: state.proc_name.clone(),
                            buffer_args: Vec::new(),
                        },
                    )
                })
                .collect::<HashMap<_, _>>();

            let mut nested_init_body = Vec::<Stmt>::new();
            for stmt in &callee_proc.init {
                if let Stmt::Assign {
                    target: AssignTarget::Var(var),
                    expr:
                        Expr::UserCall {
                            name: ctor_name,
                            type_args,
                            args,
                            ..
                        },
                    ..
                } = stmt
                {
                    if let Some(nested_state) = callee_shape.state.nested_procs.get(var) {
                        if !type_args.is_empty() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor '{}' is not generic and cannot take type arguments",
                                    ctor_name
                                ),
                                0,
                                0,
                            ));
                        }
                        if nested_state.proc_name != *ctor_name {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor state symbol '{}' has conflicting processor types '{}' and '{}'",
                                    var, nested_state.proc_name, ctor_name
                                ),
                                0,
                                0,
                            ));
                            continue;
                        }
                        let Some(nested_callee_shape) =
                            lowering_shapes.get(&nested_state.proc_name)
                        else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor '{}' nested state '{}' references unknown processor '{}'",
                                    callee_proc_name, var, nested_state.proc_name
                                ),
                                0,
                                0,
                            ));
                            continue;
                        };
                        let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                            &proc.name,
                            &nested_field_name(&nested_path, var),
                            ctor_name,
                            args,
                            &nested_callee_shape.param_specs,
                            &nested_callee_shape.buffer_specs,
                            errors,
                        );
                        if let Some(instance) = callee_nested_instances.get_mut(var) {
                            instance.buffer_args = bound_buffers;
                        }
                        for expanded_stmt in expanded {
                            if let Some(rewritten) = rewrite_owner_proc_stmt(
                                expanded_stmt,
                                &proc.name,
                                &shape.field_names,
                                &shape.data_field_names,
                                &ins_names,
                                &shape.field_array_slots,
                                &shape.in_array_slots,
                                &shape.nested_fields,
                                &nested_instances,
                                &proc_api,
                                errors,
                            ) {
                                nested_init_body.push(rewritten);
                            }
                        }
                        continue;
                    }
                    if let Some(state_struct) = callee_shape.state.struct_instances.get(var) {
                        if state_struct.struct_name != *ctor_name {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor state symbol '{}' has conflicting struct types '{}' and '{}'",
                                    var, state_struct.struct_name, ctor_name
                                ),
                                0,
                                0,
                            ));
                            continue;
                        }
                        let Some(struct_def) = resolve_proc_state_struct_def(
                            &callee_proc_name,
                            var,
                            state_struct,
                            &struct_defs_by_name,
                            errors,
                        ) else {
                            continue;
                        };
                        if !type_args.is_empty() {
                            let Some(resolved_type_args) = resolve_explicit_call_type_args(
                                type_args,
                                &format!(
                                    "processor state constructor '{} = {}(...)'",
                                    var, ctor_name
                                ),
                                errors,
                            ) else {
                                continue;
                            };
                            if resolved_type_args.as_slice() != state_struct.type_args.as_slice() {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "processor state constructor '{} = {}(...)' uses type arguments inconsistent with the declared state specialization",
                                        var, ctor_name
                                    ),
                                    0,
                                    0,
                                ));
                            }
                        }
                        let expanded = expand_nested_struct_ctor_assign(
                            &nested_field_name(&nested_path, var),
                            ctor_name,
                            args,
                            &struct_def,
                            errors,
                        );
                        for expanded_stmt in expanded {
                            if let Some(rewritten) = rewrite_owner_proc_stmt(
                                expanded_stmt,
                                &proc.name,
                                &shape.field_names,
                                &shape.data_field_names,
                                &ins_names,
                                &shape.field_array_slots,
                                &shape.in_array_slots,
                                &shape.nested_fields,
                                &nested_instances,
                                &proc_api,
                                errors,
                            ) {
                                nested_init_body.push(rewritten);
                            }
                        }
                        continue;
                    }
                }
                if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                    stmt,
                    &proc.name,
                    &nested_path,
                    &callee_shape,
                    &callee_nested_instances,
                    &callee_ins_names,
                    &callee_shape.field_array_slots,
                    &callee_shape.in_array_slots,
                    &proc_api,
                    errors,
                ) {
                    nested_init_body.push(rewritten);
                }
            }

            generated_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: nested_init_fn_name(&proc.name, &nested_path),
                params: vec![omni_frontend::FnParamDecl {
                    name: "self".to_owned(),
                    ty: Some(FnParamType::Struct(proc.name.clone())),
                    default: None,
                }],
                body: nested_init_body,
            }));

            let nested_step_body = callee_proc
                .sample
                .iter()
                .filter_map(|stmt| {
                    lower_callee_stmt_for_nested_wrapper(
                        stmt,
                        &proc.name,
                        &nested_path,
                        &callee_shape,
                        &callee_nested_instances,
                        &callee_ins_names,
                        &callee_shape.field_array_slots,
                        &callee_shape.in_array_slots,
                        &proc_api,
                        errors,
                    )
                })
                .collect::<Vec<_>>();
            let mut nested_step_params = Vec::<omni_frontend::FnParamDecl>::new();
            nested_step_params.push(omni_frontend::FnParamDecl {
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                default: None,
            });
            for in_name in &callee_shape.ins {
                nested_step_params.push(omni_frontend::FnParamDecl {
                    name: in_name.clone(),
                    ty: None,
                    default: None,
                });
            }
            for buffer in &callee_shape.buffer_specs {
                nested_step_params.push(omni_frontend::FnParamDecl {
                    name: buffer.name.clone(),
                    ty: Some(proc_buffer_fn_param_type(buffer)),
                    default: None,
                });
            }
            generated_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: nested_step_fn_name(&proc.name, &nested_path),
                params: nested_step_params.clone(),
                body: nested_step_body,
            }));

            let callee_has_effective_block = proc_api
                .get(&callee_proc_name)
                .map(|api| api.has_block)
                .unwrap_or(callee_proc.has_block_block);
            if callee_has_effective_block {
                let mut nested_block_params = Vec::<omni_frontend::FnParamDecl>::new();
                nested_block_params.push(omni_frontend::FnParamDecl {
                    name: "self".to_owned(),
                    ty: Some(FnParamType::Struct(proc.name.clone())),
                    default: None,
                });
                for buffer in &callee_shape.buffer_specs {
                    nested_block_params.push(omni_frontend::FnParamDecl {
                        name: buffer.name.clone(),
                        ty: Some(proc_buffer_fn_param_type(buffer)),
                        default: None,
                    });
                }
                let mut nested_block_pre_body = Vec::<Stmt>::new();
                for stmt in &callee_proc.block_pre {
                    if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                        stmt,
                        &proc.name,
                        &nested_path,
                        &callee_shape,
                        &callee_nested_instances,
                        &callee_ins_names,
                        &callee_shape.field_array_slots,
                        &callee_shape.in_array_slots,
                        &proc_api,
                        errors,
                    ) {
                        nested_block_pre_body.push(rewritten);
                    }
                }
                let called_callee_nested = collect_called_proc_instances_in_stmts(
                    &callee_proc.sample,
                    &callee_nested_instances,
                );
                let mut callee_nested_vars =
                    callee_nested_instances.keys().cloned().collect::<Vec<_>>();
                callee_nested_vars.sort();
                for nested_var in &callee_nested_vars {
                    if !called_callee_nested.contains(nested_var) {
                        continue;
                    }
                    let Some(instance) = callee_nested_instances.get(nested_var) else {
                        continue;
                    };
                    let Some(api) = proc_api.get(&instance.proc_name) else {
                        continue;
                    };
                    if !api.has_block {
                        continue;
                    }
                    let mut call_args = vec![CallArg {
                        name: None,
                        expr: Expr::Var("self".to_owned()),
                    }];
                    call_args.extend(expand_proc_buffer_call_args(
                        instance, api, nested_var, errors,
                    ));
                    nested_block_pre_body.push(Stmt::Expr {
                        loc: None,
                        expr: Expr::UserCall {
                            name: nested_block_pre_fn_name(
                                &proc.name,
                                &nested_field_name(&nested_path, nested_var),
                            ),
                            type_args: Vec::new(),
                            args: call_args,
                        },
                    });
                }
                generated_defs.push(Block::Def(FunctionDef {
                    type_params: Vec::new(),
                    name: nested_block_pre_fn_name(&proc.name, &nested_path),
                    params: nested_block_params.clone(),
                    body: nested_block_pre_body,
                }));

                let mut nested_block_post_body = Vec::<Stmt>::new();
                for nested_var in &callee_nested_vars {
                    if !called_callee_nested.contains(nested_var) {
                        continue;
                    }
                    let Some(instance) = callee_nested_instances.get(nested_var) else {
                        continue;
                    };
                    let Some(api) = proc_api.get(&instance.proc_name) else {
                        continue;
                    };
                    if !api.has_block {
                        continue;
                    }
                    let mut call_args = vec![CallArg {
                        name: None,
                        expr: Expr::Var("self".to_owned()),
                    }];
                    call_args.extend(expand_proc_buffer_call_args(
                        instance, api, nested_var, errors,
                    ));
                    nested_block_post_body.push(Stmt::Expr {
                        loc: None,
                        expr: Expr::UserCall {
                            name: nested_block_post_fn_name(
                                &proc.name,
                                &nested_field_name(&nested_path, nested_var),
                            ),
                            type_args: Vec::new(),
                            args: call_args,
                        },
                    });
                }
                for stmt in &callee_proc.block_post {
                    if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                        stmt,
                        &proc.name,
                        &nested_path,
                        &callee_shape,
                        &callee_nested_instances,
                        &callee_ins_names,
                        &callee_shape.field_array_slots,
                        &callee_shape.in_array_slots,
                        &proc_api,
                        errors,
                    ) {
                        nested_block_post_body.push(rewritten);
                    }
                }
                generated_defs.push(Block::Def(FunctionDef {
                    type_params: Vec::new(),
                    name: nested_block_post_fn_name(&proc.name, &nested_path),
                    params: nested_block_params,
                    body: nested_block_post_body,
                }));
            }

            for (idx, out_name) in callee_shape.outs.iter().enumerate() {
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::Var("self".to_owned()),
                }];
                for in_name in &callee_shape.ins {
                    call_args.push(CallArg {
                        name: None,
                        expr: Expr::Var(in_name.clone()),
                    });
                }
                for buffer in &callee_shape.buffer_specs {
                    call_args.push(CallArg {
                        name: None,
                        expr: Expr::Var(buffer.name.clone()),
                    });
                }
                generated_defs.push(Block::Def(FunctionDef {
                    type_params: Vec::new(),
                    name: nested_call_out_fn_name(&proc.name, &nested_path, idx),
                    params: nested_step_params.clone(),
                    body: vec![
                        Stmt::Expr {
                            loc: None,
                            expr: Expr::UserCall {
                                name: nested_step_fn_name(&proc.name, &nested_path),
                                type_args: Vec::new(),
                                args: call_args,
                            },
                        },
                        Stmt::Return {
                            loc: None,
                            expr: Expr::Var(format!(
                                "self.{}",
                                nested_field_name(&nested_path, out_name)
                            )),
                        },
                    ],
                }));
            }
        }

        let proc_has_effective_block = proc_api
            .get(&proc.name)
            .map(|api| api.has_block)
            .unwrap_or(proc.has_block_block);
        if proc_has_effective_block {
            let mut block_params = Vec::<omni_frontend::FnParamDecl>::new();
            block_params.push(omni_frontend::FnParamDecl {
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                default: None,
            });
            for buffer in &shape.buffer_specs {
                block_params.push(omni_frontend::FnParamDecl {
                    name: buffer.name.clone(),
                    ty: Some(proc_buffer_fn_param_type(buffer)),
                    default: None,
                });
            }
            let mut block_pre_body = Vec::<Stmt>::new();
            for stmt in &proc.block_pre {
                if let Some(rewritten) = rewrite_owner_proc_stmt(
                    stmt.clone(),
                    &proc.name,
                    &shape.field_names,
                    &shape.data_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                ) {
                    block_pre_body.push(rewritten);
                }
            }
            let called_nested =
                collect_called_proc_instances_in_stmts(&proc.sample, &nested_instances);
            let mut nested_vars = nested_instances.keys().cloned().collect::<Vec<_>>();
            nested_vars.sort();
            for nested_var in &nested_vars {
                if !called_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !api.has_block {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::Var("self".to_owned()),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                block_pre_body.push(Stmt::Expr {
                    loc: None,
                    expr: Expr::UserCall {
                        name: nested_block_pre_fn_name(&proc.name, nested_var),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            generated_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: format!("{}{}", proc.name, PROC_BLOCK_PRE_FN_SUFFIX),
                params: block_params.clone(),
                body: block_pre_body,
            }));

            let mut block_post_body = Vec::<Stmt>::new();
            for nested_var in &nested_vars {
                if !called_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !api.has_block {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::Var("self".to_owned()),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                block_post_body.push(Stmt::Expr {
                    loc: None,
                    expr: Expr::UserCall {
                        name: nested_block_post_fn_name(&proc.name, nested_var),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            for stmt in &proc.block_post {
                if let Some(rewritten) = rewrite_owner_proc_stmt(
                    stmt.clone(),
                    &proc.name,
                    &shape.field_names,
                    &shape.data_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                ) {
                    block_post_body.push(rewritten);
                }
            }
            generated_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: format!("{}{}", proc.name, PROC_BLOCK_POST_FN_SUFFIX),
                params: block_params,
                body: block_post_body,
            }));
        }

        let step_body = proc
            .sample
            .iter()
            .filter_map(|stmt| {
                rewrite_owner_proc_stmt(
                    stmt.clone(),
                    &proc.name,
                    &shape.field_names,
                    &shape.data_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    &proc_api,
                    errors,
                )
            })
            .collect::<Vec<_>>();
        let mut step_params = Vec::<omni_frontend::FnParamDecl>::new();
        step_params.push(omni_frontend::FnParamDecl {
            name: "self".to_owned(),
            ty: Some(FnParamType::Struct(proc.name.clone())),
            default: None,
        });
        for in_name in &shape.ins {
            let in_ty = *shape.in_types.get(in_name).unwrap_or(&PrimitiveType::F32);
            step_params.push(omni_frontend::FnParamDecl {
                name: in_name.clone(),
                ty: Some(FnParamType::Primitive(in_ty)),
                default: None,
            });
        }
        for buffer in &shape.buffer_specs {
            step_params.push(omni_frontend::FnParamDecl {
                name: buffer.name.clone(),
                ty: Some(proc_buffer_fn_param_type(buffer)),
                default: None,
            });
        }
        generated_defs.push(Block::Def(FunctionDef {
            type_params: Vec::new(),
            name: format!("{}{}", proc.name, PROC_STEP_FN_SUFFIX),
            params: step_params.clone(),
            body: step_body,
        }));

        for (idx, out_name) in shape.outs.iter().enumerate() {
            let mut call_args = vec![CallArg {
                name: None,
                expr: Expr::Var("self".to_owned()),
            }];
            for in_name in &shape.ins {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::Var(in_name.clone()),
                });
            }
            for buffer in &shape.buffer_specs {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::Var(buffer.name.clone()),
                });
            }
            generated_defs.push(Block::Def(FunctionDef {
                type_params: Vec::new(),
                name: format!("{}{}{}", proc.name, PROC_CALL_OUT_FN_PREFIX, idx),
                params: step_params.clone(),
                body: vec![
                    Stmt::Expr {
                        loc: None,
                        expr: Expr::UserCall {
                            name: format!("{}{}", proc.name, PROC_STEP_FN_SUFFIX),
                            type_args: Vec::new(),
                            args: call_args,
                        },
                    },
                    Stmt::Return {
                        loc: None,
                        expr: Expr::Var(format!("self.{out_name}")),
                    },
                ],
            }));
        }
    }

    program.blocks.retain(|b| !matches!(b, Block::Proc(_)));
    program.blocks.extend(generated_structs);
    program.blocks.extend(generated_defs);

    let mut global_proc_instances = HashMap::<String, ProcCallInstance>::new();
    if let Some(Block::Init(init)) = program
        .blocks
        .iter_mut()
        .find(|b| b.kind() == BlockKind::Init)
    {
        let mut rewritten_init = Vec::<Stmt>::new();
        for mut stmt in init.clone() {
            rewrite_proc_calls_in_stmt(&mut stmt, &global_proc_instances, &proc_api, errors);
            if let Stmt::Assign {
                target: AssignTarget::Var(var),
                expr:
                    Expr::UserCall {
                        name: ctor_name,
                        type_args: ctor_type_args,
                        args: ctor_args,
                        ..
                    },
                ..
            } = &stmt
            {
                if proc_api.contains_key(ctor_name) {
                    if !ctor_type_args.is_empty() {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor '{}' is not generic and cannot take type arguments",
                                ctor_name
                            ),
                            0,
                            0,
                        ));
                    }
                    let mut ctor_stmt = stmt.clone();
                    if let Stmt::Assign {
                        expr:
                            Expr::UserCall {
                                type_args, args, ..
                            },
                        ..
                    } = &mut ctor_stmt
                    {
                        type_args.clear();
                        args.clear();
                    }
                    rewritten_init.push(ctor_stmt);
                    if let Some(shape) = lowering_shapes.get(ctor_name) {
                        let (ctor_assigns, buffer_args) = expand_proc_instance_ctor_assign(
                            var,
                            ctor_name,
                            ctor_args,
                            &shape.param_specs,
                            &shape.buffer_specs,
                            errors,
                        );
                        global_proc_instances.insert(
                            var.clone(),
                            ProcCallInstance {
                                proc_name: ctor_name.clone(),
                                buffer_args: buffer_args.clone(),
                            },
                        );
                        rewritten_init.extend(ctor_assigns);
                        rewritten_init.push(Stmt::Expr {
                            loc: None,
                            expr: Expr::UserCall {
                                name: format!("{ctor_name}{PROC_INIT_FN_SUFFIX}"),
                                type_args: Vec::new(),
                                args: vec![CallArg {
                                    name: None,
                                    expr: Expr::Var(var.clone()),
                                }],
                            },
                        });
                    } else {
                        rewritten_init.push(Stmt::Expr {
                            loc: None,
                            expr: Expr::UserCall {
                                name: format!("{ctor_name}{PROC_INIT_FN_SUFFIX}"),
                                type_args: Vec::new(),
                                args: vec![CallArg {
                                    name: None,
                                    expr: Expr::Var(var.clone()),
                                }],
                            },
                        });
                    }
                    continue;
                }
            }
            rewritten_init.push(stmt);
        }
        *init = rewritten_init;
    }

    let mut called_proc_instances = HashSet::<String>::new();
    for block in &program.blocks {
        match block {
            Block::Block(exec) => {
                if let Some(sample) = &exec.sample {
                    called_proc_instances.extend(collect_called_proc_instances_in_stmts(
                        sample,
                        &global_proc_instances,
                    ));
                }
            }
            Block::Sample(stmts) => {
                called_proc_instances.extend(collect_called_proc_instances_in_stmts(
                    stmts,
                    &global_proc_instances,
                ));
            }
            _ => {}
        }
    }
    if !called_proc_instances.is_empty() {
        let mut called_order = called_proc_instances.into_iter().collect::<Vec<_>>();
        called_order.sort();
        let mut injected_block_pre = Vec::<Stmt>::new();
        let mut injected_block_post = Vec::<Stmt>::new();
        for instance_name in called_order {
            let Some(instance) = global_proc_instances.get(&instance_name) else {
                continue;
            };
            let Some(api) = proc_api.get(&instance.proc_name) else {
                errors.push(Diagnostic::semantic(
                    format!("unknown processor type '{}'", instance.proc_name),
                    0,
                    0,
                ));
                continue;
            };
            if !api.has_block {
                continue;
            }
            let mut pre_args = Vec::<CallArg>::new();
            pre_args.push(CallArg {
                name: None,
                expr: Expr::Var(instance_name.clone()),
            });
            pre_args.extend(expand_proc_buffer_call_args(
                instance,
                api,
                &instance_name,
                errors,
            ));
            injected_block_pre.push(Stmt::Expr {
                loc: None,
                expr: Expr::UserCall {
                    name: format!("{}{}", instance.proc_name, PROC_BLOCK_PRE_FN_SUFFIX),
                    type_args: Vec::new(),
                    args: pre_args,
                },
            });

            let mut post_args = Vec::<CallArg>::new();
            post_args.push(CallArg {
                name: None,
                expr: Expr::Var(instance_name.clone()),
            });
            post_args.extend(expand_proc_buffer_call_args(
                instance,
                api,
                &instance_name,
                errors,
            ));
            injected_block_post.push(Stmt::Expr {
                loc: None,
                expr: Expr::UserCall {
                    name: format!("{}{}", instance.proc_name, PROC_BLOCK_POST_FN_SUFFIX),
                    type_args: Vec::new(),
                    args: post_args,
                },
            });
        }

        if !injected_block_pre.is_empty() || !injected_block_post.is_empty() {
            if let Some(block_idx) = program
                .blocks
                .iter()
                .position(|b| matches!(b, Block::Block(_)))
            {
                if let Block::Block(exec) = &mut program.blocks[block_idx] {
                    let mut pre = injected_block_pre;
                    pre.append(&mut exec.pre);
                    exec.pre = pre;
                    exec.post.extend(injected_block_post);
                }
            } else if let Some(sample_idx) = program
                .blocks
                .iter()
                .position(|b| matches!(b, Block::Sample(_)))
            {
                let sample_body = match program.blocks.remove(sample_idx) {
                    Block::Sample(stmts) => stmts,
                    _ => Vec::new(),
                };
                program.blocks.insert(
                    sample_idx,
                    Block::Block(BlockExec {
                        pre: injected_block_pre,
                        sample: Some(sample_body),
                        post: injected_block_post,
                    }),
                );
            }
        }
    }

    for block in &mut program.blocks {
        match block {
            Block::Block(exec) => {
                rewrite_proc_calls_in_stmts(
                    &mut exec.pre,
                    &global_proc_instances,
                    &proc_api,
                    errors,
                );
                if let Some(sample) = &mut exec.sample {
                    rewrite_proc_calls_in_stmts(sample, &global_proc_instances, &proc_api, errors);
                }
                rewrite_proc_calls_in_stmts(
                    &mut exec.post,
                    &global_proc_instances,
                    &proc_api,
                    errors,
                );
            }
            Block::Sample(stmts) => {
                rewrite_proc_calls_in_stmts(stmts, &global_proc_instances, &proc_api, errors);
            }
            Block::Def(def) => {
                let mut proc_vars = HashMap::<String, ProcCallInstance>::new();
                for param in &def.params {
                    if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                        if proc_api.contains_key(struct_name) {
                            proc_vars.insert(
                                param.name.clone(),
                                ProcCallInstance {
                                    proc_name: struct_name.clone(),
                                    buffer_args: Vec::new(),
                                },
                            );
                        }
                    }
                }
                rewrite_proc_calls_in_stmts(&mut def.body, &proc_vars, &proc_api, errors);
            }
            _ => {}
        }
    }

    program
}

pub fn analyze(program: Program) -> Result<TypedProgram, Vec<Diagnostic>> {
    analyze_with_options(program, AnalysisOptions::default())
}

pub fn analyze_with_options(
    program: Program,
    options: AnalysisOptions,
) -> Result<TypedProgram, Vec<Diagnostic>> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'sample_rate' must be finite and greater than zero",
        )]);
    }
    if options.block_size == 0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'block_size' must be greater than zero",
        )]);
    }

    let mut program = program;
    inject_auto_std_math(&mut program)?;

    let mut errors = Vec::new();
    let program = desugar_processors(program, options, &mut errors);

    let mut seen_singleton = HashSet::new();
    for block in &program.blocks {
        let kind = block.kind();
        if matches!(kind, BlockKind::Def | BlockKind::Struct | BlockKind::Proc) {
            continue;
        }
        if !seen_singleton.insert(kind) {
            errors.push(Diagnostic::semantic(
                format!("duplicate block '{:?}'", kind).to_lowercase(),
                0,
                0,
            ));
        }
    }

    let raw_ins = match program.block(BlockKind::Ins) {
        Some(Block::Ins(v)) => v.clone(),
        _ => Vec::new(),
    };
    let raw_outs = match program.block(BlockKind::Outs) {
        Some(Block::Outs(v)) => v.clone(),
        _ => Vec::new(),
    };
    let params = match program.block(BlockKind::Params) {
        Some(Block::Params(v)) => v.clone(),
        _ => Vec::new(),
    };
    let buffers = match program.block(BlockKind::Buffers) {
        Some(Block::Buffers(v)) => v.clone(),
        _ => Vec::new(),
    };
    let mut struct_defs_raw = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(def) => Some(def.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut init = match program.block(BlockKind::Init) {
        Some(Block::Init(v)) => v.clone(),
        _ => Vec::new(),
    };
    let mut block_pre = Vec::new();
    let mut block_post = Vec::new();
    let mut nested_block_sample = None;
    if let Some(Block::Block(exec)) = program.block(BlockKind::Block) {
        block_pre = exec.pre.clone();
        block_post = exec.post.clone();
        nested_block_sample = exec.sample.clone();
        if nested_block_sample.is_none() {
            errors.push(Diagnostic::semantic(
                "block section must include nested 'sample' block",
                0,
                0,
            ));
        }
    }
    let top_sample = match program.block(BlockKind::Sample) {
        Some(Block::Sample(v)) => Some(v.clone()),
        _ => None,
    };

    let mut sample = match (nested_block_sample, top_sample) {
        (Some(_), Some(_)) => {
            errors.push(Diagnostic::semantic(
                "sample block cannot be declared both at top-level and inside block",
                0,
                0,
            ));
            Vec::new()
        }
        (Some(v), None) => v,
        (None, Some(v)) => v,
        (None, None) => Vec::new(),
    };

    if sample.is_empty() {
        errors.push(Diagnostic::semantic(
            "missing required 'sample' block",
            0,
            0,
        ));
    }

    {
        let struct_symbols = struct_defs_raw
            .iter()
            .map(|s| s.name.clone())
            .collect::<HashSet<_>>();
        let mut callable_symbols = defs.iter().map(|d| d.name.clone()).collect::<HashSet<_>>();
        for p in program.blocks.iter().filter_map(|b| match b {
            Block::Proc(proc_def) => Some(proc_def),
            _ => None,
        }) {
            callable_symbols.insert(p.name.clone());
        }
        for s in &struct_defs_raw {
            callable_symbols.insert(s.name.clone());
            for method in &s.methods {
                callable_symbols.insert(format!("{}.{}", s.name, method.name));
            }
        }
        let struct_namespaces = collect_declared_namespaces(&struct_symbols);
        let callable_namespaces = collect_declared_namespaces(&callable_symbols);

        for s in &mut struct_defs_raw {
            let struct_ns = namespace_of_symbol(&s.name);
            for field in &mut s.fields {
                if let FieldType::Data(spec) = &mut field.ty {
                    if let DataElemType::Struct(name) = &mut spec.elem {
                        qualify_struct_type_name(
                            name,
                            &struct_ns,
                            &struct_symbols,
                            &struct_namespaces,
                            &format!("struct '{}.{}' Data element type", s.name, field.name),
                            &mut errors,
                        );
                    }
                }
                if let Some(default) = &mut field.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("struct '{}.{}' default", s.name, field.name),
                    );
                }
            }
            for method in &mut s.methods {
                for param in &mut method.params {
                    if let Some(FnParamType::Struct(name)) = &mut param.ty {
                        qualify_struct_type_name(
                            name,
                            &struct_ns,
                            &struct_symbols,
                            &struct_namespaces,
                            &format!(
                                "method '{}.{}' parameter '{}'",
                                s.name, method.name, param.name
                            ),
                            &mut errors,
                        );
                    }
                    if let Some(default) = &mut param.default {
                        qualify_expr_namespaced_symbols(
                            default,
                            &struct_ns,
                            &callable_symbols,
                            &callable_namespaces,
                            &struct_symbols,
                            &struct_namespaces,
                            &mut errors,
                            &format!(
                                "method '{}.{}' parameter '{}' default",
                                s.name, method.name, param.name
                            ),
                        );
                    }
                }
                for stmt in &mut method.body {
                    qualify_stmt_namespaced_symbols(
                        stmt,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("method '{}.{}' body", s.name, method.name),
                    );
                }
            }
        }

        for def in &mut defs {
            let def_ns = namespace_of_symbol(&def.name);
            for param in &mut def.params {
                if let Some(FnParamType::Struct(name)) = &mut param.ty {
                    qualify_struct_type_name(
                        name,
                        &def_ns,
                        &struct_symbols,
                        &struct_namespaces,
                        &format!("function '{}' parameter '{}'", def.name, param.name),
                        &mut errors,
                    );
                }
                if let Some(default) = &mut param.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &def_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("function '{}' parameter '{}' default", def.name, param.name),
                    );
                }
            }
            for stmt in &mut def.body {
                qualify_stmt_namespaced_symbols(
                    stmt,
                    &def_ns,
                    &callable_symbols,
                    &callable_namespaces,
                    &struct_symbols,
                    &struct_namespaces,
                    &mut errors,
                    &format!("function '{}' body", def.name),
                );
            }
        }

        for stmt in &mut init {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "init",
            );
        }
        for stmt in &mut block_pre {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "block pre",
            );
        }
        for stmt in &mut block_post {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "block post",
            );
        }
        for stmt in &mut sample {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "sample",
            );
        }
    }

    {
        let mut concrete_structs = Vec::<StructDef>::new();
        let mut generic_templates = HashMap::<String, StructDef>::new();
        for s in struct_defs_raw.drain(..) {
            if s.type_params.is_empty() {
                concrete_structs.push(s);
                continue;
            }
            if generic_templates.contains_key(&s.name) {
                errors.push(Diagnostic::semantic(
                    format!("duplicate generic struct '{}'", s.name),
                    0,
                    0,
                ));
                continue;
            }
            let mut seen = HashSet::new();
            for tp in &s.type_params {
                if !seen.insert(tp.clone()) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "duplicate generic type parameter '{}' in struct '{}'",
                            tp, s.name
                        ),
                        0,
                        0,
                    ));
                }
            }
            generic_templates.insert(s.name.clone(), s);
        }

        let mut generated_specializations = HashMap::<String, StructDef>::new();
        for s in &mut concrete_structs {
            for field in &mut s.fields {
                if let Some(default) = &mut field.default {
                    let mut locals = GenericInferenceLocals::default();
                    rewrite_generic_struct_ctor_expr(
                        default,
                        &generic_templates,
                        &mut generated_specializations,
                        &mut errors,
                        &mut locals,
                    );
                }
            }
            for method in &mut s.methods {
                rewrite_generic_struct_ctor_stmt_list(
                    &mut method.body,
                    &generic_templates,
                    &mut generated_specializations,
                    &mut errors,
                );
            }
        }
        for def in &mut defs {
            rewrite_generic_struct_ctor_stmt_list(
                &mut def.body,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
        }
        rewrite_generic_struct_ctor_stmt_list(
            &mut init,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut block_pre,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut block_post,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut sample,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );

        finalize_generated_generic_struct_specializations(
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );

        struct_defs_raw = concrete_structs;
        let mut generated = generated_specializations.into_values().collect::<Vec<_>>();
        generated.sort_by(|a, b| a.name.cmp(&b.name));
        struct_defs_raw.extend(generated);
    }

    check_local_port_duplicates(&raw_ins, "input", &mut errors);
    check_local_port_duplicates(&raw_outs, "output", &mut errors);

    let inferred_io = infer_numbered_io_from_sample(&sample);
    let ins_ports = normalize_numbered_port_decls(&raw_ins, "in", inferred_io.max_in);
    let outs_ports = normalize_numbered_port_decls(&raw_outs, "out", inferred_io.max_out);
    let input_declared_names = ins_ports.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let output_declared_names = outs_ports
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let (ins, in_types, in_arrays, in_defaults, in_ranges) =
        expand_port_decls(&ins_ports, "input", options, &mut errors);
    let (outs, out_types, out_arrays, _out_defaults, _out_ranges) =
        expand_port_decls(&outs_ports, "output", options, &mut errors);

    let (typed_params, param_arrays) = coerce_params(&params, options, &mut errors);
    let typed_buffers = coerce_buffers(&buffers, options, &mut errors);
    let param_types = typed_params
        .iter()
        .map(|p| (p.name.clone(), p.ty))
        .collect::<HashMap<_, _>>();
    let param_ranges = typed_params
        .iter()
        .filter_map(|p| p.range.map(|r| (p.name.clone(), r)))
        .collect::<HashMap<_, _>>();
    let mut occupied_temp_names = HashSet::<String>::new();
    occupied_temp_names.extend(input_declared_names.iter().cloned());
    occupied_temp_names.extend(output_declared_names.iter().cloned());
    occupied_temp_names.extend(params.iter().map(|p| p.name.clone()));
    occupied_temp_names.extend(typed_buffers.iter().map(|b| b.name.clone()));

    let mut make_unique_temp = |base: String| -> String {
        if occupied_temp_names.insert(base.clone()) {
            return base;
        }
        let mut idx = 1usize;
        loop {
            let candidate = format!("{base}_{idx}");
            if occupied_temp_names.insert(candidate.clone()) {
                return candidate;
            }
            idx += 1;
        }
    };

    let mut input_aliases = HashMap::<String, String>::new();
    let mut input_hoists = Vec::<Stmt>::new();
    let mut input_names = in_ranges.keys().cloned().collect::<Vec<_>>();
    input_names.sort();
    for name in input_names {
        let Some(range) = in_ranges.get(&name).copied() else {
            continue;
        };
        let ty = *in_types.get(&name).unwrap_or(&PrimitiveType::F32);
        let alias = make_unique_temp(format!(
            "__omni_clamped_in__{}",
            sanitize_symbol_component(&name)
        ));
        input_aliases.insert(name.clone(), alias.clone());
        input_hoists.push(build_top_level_range_hoist_assign(alias, &name, ty, range));
    }

    let mut param_aliases = HashMap::<String, String>::new();
    let mut param_hoists = Vec::<Stmt>::new();
    let mut param_names_sorted = param_ranges.keys().cloned().collect::<Vec<_>>();
    param_names_sorted.sort();
    for name in param_names_sorted {
        let Some(range) = param_ranges.get(&name).copied() else {
            continue;
        };
        let ty = *param_types.get(&name).unwrap_or(&PrimitiveType::F32);
        let alias = make_unique_temp(format!(
            "__omni_clamped_param__{}",
            sanitize_symbol_component(&name)
        ));
        param_aliases.insert(name.clone(), alias.clone());
        param_hoists.push(build_top_level_range_hoist_assign(alias, &name, ty, range));
    }

    for stmt in &mut block_pre {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, false, true);
    }
    for stmt in &mut sample {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, true, true);
    }
    for stmt in &mut block_post {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, false, true);
    }

    if !param_hoists.is_empty() {
        let mut rewritten = param_hoists;
        rewritten.append(&mut block_pre);
        block_pre = rewritten;
    }
    if !input_hoists.is_empty() {
        let mut rewritten = input_hoists;
        rewritten.append(&mut sample);
        sample = rewritten;
    }

    let mut all_declared = HashSet::new();
    check_unique_set(
        &input_declared_names,
        "input",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &output_declared_names,
        "output",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        "param",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &typed_buffers
            .iter()
            .map(|b| b.name.clone())
            .collect::<Vec<_>>(),
        "buffer",
        &mut all_declared,
        &mut errors,
    );

    let mut struct_defs = HashMap::new();
    let mut typed_structs = Vec::new();
    let mut method_self_struct = HashMap::<String, String>::new();
    for s in &struct_defs_raw {
        if is_builtin_constant_name(&s.name) {
            errors.push(Diagnostic::semantic(
                format!("struct name '{}' is reserved as a builtin constant", s.name),
                0,
                0,
            ));
            continue;
        }
        if all_declared.contains(&s.name) {
            errors.push(Diagnostic::semantic(
                format!("struct name '{}' conflicts with existing symbol", s.name),
                0,
                0,
            ));
            continue;
        }
        if struct_defs.contains_key(&s.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate struct '{}'", s.name),
                0,
                0,
            ));
            continue;
        }
        let typed_fields = coerce_struct_fields(&s.name, &s.fields, options, &mut errors);
        struct_defs.insert(s.name.clone(), typed_fields.clone());
        typed_structs.push(TypedStruct {
            name: s.name.clone(),
            fields: typed_fields,
        });
        all_declared.insert(s.name.clone());

        let mut local_method_names = HashSet::new();
        for method in &s.methods {
            if !local_method_names.insert(method.name.clone()) {
                errors.push(Diagnostic::semantic(
                    format!("duplicate method '{}.{}'", s.name, method.name),
                    0,
                    0,
                ));
                continue;
            }
            if method.params.first().map(|p| p.name.as_str()) != Some("self") {
                errors.push(Diagnostic::semantic(
                    format!(
                        "method '{}.{}' must declare 'self' as first parameter",
                        s.name, method.name
                    ),
                    0,
                    0,
                ));
            }
            let fq_name = format!("{}.{}", s.name, method.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                method_self_struct.insert(fq_name.clone(), s.name.clone());
            }
            defs.push(FunctionDef {
                type_params: Vec::new(),
                name: fq_name,
                params: method.params.clone(),
                body: method.body.clone(),
            });
        }
    }

    for (struct_name, fields) in &struct_defs {
        for field in fields {
            if let Some(elem_struct) = &field.data_elem_struct {
                let context = format!("field '{}.{}' Data element", struct_name, field.name);
                let _ =
                    validate_data_struct_layout(elem_struct, &struct_defs, &context, &mut errors);
            }
        }
    }

    let mut desugar_struct_instances = HashMap::<String, String>::new();
    for stmt in &mut init {
        desugar_init_instance_method_calls(stmt, &mut desugar_struct_instances, &struct_defs);
    }
    for stmt in &mut block_pre {
        desugar_sample_instance_method_calls(stmt, &desugar_struct_instances);
    }
    for stmt in &mut block_post {
        desugar_sample_instance_method_calls(stmt, &desugar_struct_instances);
    }
    for stmt in &mut sample {
        desugar_sample_instance_method_calls(stmt, &desugar_struct_instances);
    }
    let mut fn_signatures: HashMap<String, FnSignature> = HashMap::new();
    for def in &defs {
        if is_builtin_constant_name(&def.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "function name '{}' is reserved as a builtin constant",
                    def.name
                ),
                0,
                0,
            ));
            continue;
        }
        if is_builtin_function_name(&def.name) {
            errors.push(Diagnostic::semantic(
                format!("cannot redefine builtin function '{}'", def.name),
                0,
                0,
            ));
            continue;
        }
        if struct_defs.contains_key(&def.name) {
            errors.push(Diagnostic::semantic(
                format!("function name '{}' conflicts with struct name", def.name),
                0,
                0,
            ));
            continue;
        }
        if all_declared.contains(&def.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "function name '{}' conflicts with existing symbol",
                    def.name
                ),
                0,
                0,
            ));
            continue;
        }
        if fn_signatures.contains_key(&def.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate function '{}'", def.name),
                0,
                0,
            ));
            continue;
        }
        fn_signatures.insert(
            def.name.clone(),
            FnSignature {
                params: def.params.iter().map(|p| p.name.clone()).collect(),
                defaults: def.params.iter().map(|p| p.default.clone()).collect(),
                param_types: def.params.iter().map(|p| p.ty.clone()).collect(),
                type_params: def.type_params.clone(),
            },
        );
        all_declared.insert(def.name.clone());

        if !def.type_params.is_empty() {
            errors.push(Diagnostic::semantic(
                format!(
                    "function '{}' does not support generic type parameters; use typed/untyped parameters and call-site monomorphization",
                    def.name
                ),
                0,
                0,
            ));
        }
        let mut local_params = HashSet::new();
        for p in &def.params {
            if is_builtin_constant_name(&p.name) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "function parameter '{}' in '{}' is reserved as a builtin constant",
                        p.name, def.name
                    ),
                    0,
                    0,
                ));
            }
            if !local_params.insert(p.name.clone()) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "duplicate function parameter '{}' in '{}'",
                        p.name, def.name
                    ),
                    0,
                    0,
                ));
            }
            if let Some(default) = &p.default {
                if matches!(p.ty, Some(FnParamType::Buffer(_))) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function parameter '{}.{}' is a buffer and cannot have a default value",
                            def.name, p.name
                        ),
                        0,
                        0,
                    ));
                }
                validate_default_expr(
                    default,
                    &mut errors,
                    &format!("function parameter '{}.{}'", def.name, p.name),
                );
            }
        }
    }

    let input_names: HashSet<String> = ins.iter().cloned().collect();
    let output_names: HashSet<String> = outs.iter().cloned().collect();
    let param_names: HashSet<String> = typed_params.iter().map(|p| p.name.clone()).collect();
    let def_return_types = infer_def_return_types(&defs, &fn_signatures, &struct_defs);

    let mut state_scalars = HashMap::<String, PrimitiveType>::new();
    set_declared_symbol_types(
        &mut state_scalars,
        &input_names,
        &in_types,
        DECLARED_INPUT_TYPE_PREFIX,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &output_names,
        &out_types,
        DECLARED_OUTPUT_TYPE_PREFIX,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &param_names,
        &param_types,
        DECLARED_PARAM_TYPE_PREFIX,
    );
    for (fn_name, ret_ty) in &def_return_types {
        state_scalars.insert(
            declared_type_key(DECLARED_FUNCTION_RETURN_TYPE_PREFIX, fn_name),
            *ret_ty,
        );
    }
    for buffer in &typed_buffers {
        state_scalars.insert(
            declared_type_key(DECLARED_BUFFER_ELEM_TYPE_PREFIX, &buffer.name),
            buffer.elem_ty,
        );
        state_scalars.insert(
            declared_type_key(buffer_elem_decl_prefix(buffer.elem_ty), &buffer.name),
            PrimitiveType::Bool,
        );
        let is_multi = match buffer.channels {
            TypedBufferChannels::Mono => false,
            TypedBufferChannels::Static(ch) => ch > 1,
            TypedBufferChannels::Dynamic => true,
        };
        match buffer.channels {
            TypedBufferChannels::Dynamic => {
                state_scalars.insert(
                    declared_type_key(DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX, &buffer.name),
                    PrimitiveType::Bool,
                );
            }
            TypedBufferChannels::Static(ch) if ch > 1 => {
                state_scalars.insert(
                    declared_buffer_static_channels_key(&buffer.name, ch),
                    PrimitiveType::Bool,
                );
            }
            _ => {}
        }
        if is_multi {
            state_scalars.insert(
                declared_type_key(DECLARED_BUFFER_MULTICHANNEL_PREFIX, &buffer.name),
                PrimitiveType::Bool,
            );
        }
    }
    let mut state_data = HashMap::new();
    let mut state_data_struct_roots = HashMap::<String, DataStructRootInfo>::new();
    let mut struct_instances = HashMap::new();
    let mut init_known_scalars = param_names.clone();
    init_known_scalars.extend(state_scalars.keys().cloned());
    let init_locals = HashSet::new();
    let mut init_local_aliases = LocalAliasTypes::new();
    let mut init_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut init_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &out_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &param_arrays, false);

    for stmt in &init {
        analyze_init_stmt(
            stmt,
            &mut init_known_scalars,
            &mut init_local_aliases,
            &mut init_local_data_aliases,
            &init_locals,
            &mut state_scalars,
            &mut state_data,
            &mut state_data_struct_roots,
            &mut struct_instances,
            &input_names,
            &output_names,
            &param_names,
            &struct_defs,
            &fn_signatures,
            options,
            &mut errors,
        );
    }

    register_block_assigned_scalars_as_state(
        block_pre.iter().chain(block_post.iter()),
        &mut state_scalars,
        &state_data,
        &state_data_struct_roots,
        &struct_instances,
        &input_names,
        &output_names,
        &param_names,
        &struct_defs,
        &fn_signatures,
    );

    let mut block_known_scalars = param_names.clone();
    block_known_scalars.extend(state_scalars.keys().cloned());
    let block_locals = HashSet::new();
    let mut block_local_aliases = LocalAliasTypes::new();
    let mut block_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut block_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut block_local_data_aliases, &out_arrays, true);
    seed_top_level_array_aliases(&mut block_local_data_aliases, &param_arrays, false);
    let empty_inputs = HashSet::new();
    let empty_outputs = HashSet::new();
    let block_forbidden_assigns = output_names.clone();

    for stmt in block_pre.iter().chain(block_post.iter()) {
        analyze_sample_stmt(
            stmt,
            &mut block_known_scalars,
            &mut block_local_aliases,
            &mut block_local_data_aliases,
            &block_locals,
            &state_scalars,
            &state_data,
            &state_data_struct_roots,
            &struct_instances,
            &empty_inputs,
            &empty_outputs,
            &block_forbidden_assigns,
            &param_names,
            &struct_defs,
            &fn_signatures,
            options,
            &mut errors,
        );
    }

    register_sample_typed_scalar_decls_as_state(
        sample.iter(),
        &mut state_scalars,
        &state_data,
        &state_data_struct_roots,
        &struct_instances,
        &input_names,
        &output_names,
        &param_names,
    );

    let mut sample_known_scalars = param_names.clone();
    sample_known_scalars.extend(input_names.clone());
    sample_known_scalars.extend(state_scalars.keys().cloned());
    let sample_locals = HashSet::new();
    let mut sample_local_aliases = LocalAliasTypes::new();
    let mut sample_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &out_arrays, true);
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &param_arrays, false);
    let sample_forbidden_assigns = HashSet::new();

    for stmt in &sample {
        analyze_sample_stmt(
            stmt,
            &mut sample_known_scalars,
            &mut sample_local_aliases,
            &mut sample_local_data_aliases,
            &sample_locals,
            &state_scalars,
            &state_data,
            &state_data_struct_roots,
            &struct_instances,
            &input_names,
            &output_names,
            &sample_forbidden_assigns,
            &param_names,
            &struct_defs,
            &fn_signatures,
            options,
            &mut errors,
        );
    }

    let mut block_exec = block_pre.clone();
    block_exec.extend(block_post.clone());

    let (inferred_def_params, synthesized_struct_defs) = infer_def_param_kinds(
        &defs,
        &init,
        &block_exec,
        &sample,
        &struct_instances,
        &typed_buffers
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    vec![InferredBufferParam {
                        elem_ty: b.elem_ty,
                        channels: b.channels.clone(),
                    }],
                )
            })
            .collect::<HashMap<_, _>>(),
        &fn_signatures,
        &method_self_struct,
        &struct_defs,
        options,
        &mut errors,
    );

    let mut def_struct_defs = struct_defs.clone();
    for (name, fields) in &synthesized_struct_defs {
        def_struct_defs.insert(name.clone(), fields.clone());
    }

    for def in &defs {
        let mut fn_known = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<HashSet<_>>();
        let mut def_state_scalars = state_scalars.clone();
        let fn_locals = HashSet::new();
        let mut fn_local_aliases = LocalAliasTypes::new();
        let mut fn_local_data_aliases = HashMap::new();
        let param_names_vec = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();
        let param_structs = inferred_def_params
            .get(&def.name)
            .map(|k| param_struct_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_buffers = inferred_def_params
            .get(&def.name)
            .map(|k| param_buffer_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        for (param_name, (elem_ty, channels)) in &param_buffers {
            let elem_key = declared_type_key(DECLARED_BUFFER_ELEM_TYPE_PREFIX, param_name);
            let typed_key = declared_type_key(buffer_elem_decl_prefix(*elem_ty), param_name);
            def_state_scalars.insert(elem_key.clone(), *elem_ty);
            def_state_scalars.insert(typed_key.clone(), PrimitiveType::Bool);
            fn_known.insert(elem_key);
            fn_known.insert(typed_key);
            match channels {
                TypedBufferChannels::Mono => {}
                TypedBufferChannels::Static(ch) => {
                    if *ch > 1 {
                        let key =
                            declared_type_key(DECLARED_BUFFER_MULTICHANNEL_PREFIX, param_name);
                        let st_key = declared_buffer_static_channels_key(param_name, *ch);
                        def_state_scalars.insert(key.clone(), PrimitiveType::Bool);
                        def_state_scalars.insert(st_key.clone(), PrimitiveType::Bool);
                        fn_known.insert(key);
                        fn_known.insert(st_key);
                    }
                }
                TypedBufferChannels::Dynamic => {
                    let key = declared_type_key(DECLARED_BUFFER_MULTICHANNEL_PREFIX, param_name);
                    let dyn_key =
                        declared_type_key(DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX, param_name);
                    def_state_scalars.insert(key.clone(), PrimitiveType::Bool);
                    def_state_scalars.insert(dyn_key.clone(), PrimitiveType::Bool);
                    fn_known.insert(key);
                    fn_known.insert(dyn_key);
                }
            }
        }
        for stmt in &def.body {
            analyze_def_stmt(
                stmt,
                &mut fn_known,
                &mut fn_local_aliases,
                &mut fn_local_data_aliases,
                &fn_locals,
                &param_structs,
                &def_state_scalars,
                &input_names,
                &output_names,
                &param_names,
                &def_struct_defs,
                &fn_signatures,
                options,
                &mut errors,
            );
        }
    }

    if errors.is_empty() {
        let mut sorted_state = state_scalars
            .keys()
            .filter(|name| {
                !name.starts_with(DECLARED_INPUT_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_OUTPUT_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_PARAM_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_DATA_ELEM_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_TYPE_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_MULTICHANNEL_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_STATIC_CHANNELS_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_F32_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_F64_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_I32_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_I64_PREFIX)
                    && !name.starts_with(DECLARED_BUFFER_ELEM_BOOL_PREFIX)
                    && !name.starts_with(DECLARED_FUNCTION_RETURN_TYPE_PREFIX)
            })
            .cloned()
            .collect::<Vec<_>>();
        sorted_state.sort();
        let state_types = sorted_state
            .iter()
            .map(|name| {
                state_scalars
                    .get(name)
                    .copied()
                    .unwrap_or(PrimitiveType::F32)
            })
            .collect::<Vec<_>>();

        let mut typed_data = state_data
            .into_iter()
            .map(|(name, len)| {
                let elem_ty =
                    get_declared_symbol_type(&state_scalars, &name, DECLARED_DATA_ELEM_TYPE_PREFIX)
                        .unwrap_or(PrimitiveType::F32);
                TypedDataVar { name, len, elem_ty }
            })
            .collect::<Vec<_>>();
        typed_data.sort_by(|a, b| a.name.cmp(&b.name));
        let mut typed_data_roots = state_data_struct_roots
            .into_iter()
            .map(|(name, info)| TypedDataStructRoot {
                name,
                struct_name: info.struct_name,
                len: info.len,
            })
            .collect::<Vec<_>>();
        typed_data_roots.sort_by(|a, b| a.name.cmp(&b.name));

        let mut synth_names = synthesized_struct_defs.keys().cloned().collect::<Vec<_>>();
        synth_names.sort();
        for name in synth_names {
            if let Some(fields) = synthesized_struct_defs.get(&name) {
                typed_structs.push(TypedStruct {
                    name,
                    fields: fields.clone(),
                });
            }
        }

        let typed_defs = defs
            .into_iter()
            .map(|d| {
                let param_kinds = inferred_def_params
                    .get(&d.name)
                    .cloned()
                    .unwrap_or_else(|| vec![TypedFnParam::Scalar; d.params.len()]);
                TypedFunction {
                    method_of: method_self_struct.get(&d.name).cloned(),
                    type_params: d.type_params.clone(),
                    param_defaults: d.params.iter().map(|p| p.default.clone()).collect(),
                    param_kinds,
                    return_ty: def_return_types
                        .get(&d.name)
                        .copied()
                        .unwrap_or(PrimitiveType::F32),
                    name: d.name,
                    params: d.params.into_iter().map(|p| p.name).collect(),
                    body: d.body,
                }
            })
            .collect::<Vec<_>>();

        Ok(TypedProgram {
            ins,
            outs,
            in_types,
            out_types,
            param_types,
            in_defaults,
            in_ranges,
            in_arrays,
            out_arrays,
            param_arrays,
            params: typed_params,
            buffers: typed_buffers,
            structs: typed_structs,
            defs: typed_defs,
            init,
            block_pre,
            sample,
            block_post,
            state_vars: sorted_state,
            state_types,
            data_vars: typed_data,
            data_struct_roots: typed_data_roots,
        })
    } else {
        Err(errors)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StructFieldUsage {
    Scalar,
    Data,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct InferredFnParam {
    saw_scalar: bool,
    saw_structs: HashSet<String>,
    saw_buffers: Vec<InferredBufferParam>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InferredBufferParam {
    elem_ty: PrimitiveType,
    channels: TypedBufferChannels,
}

fn infer_def_param_kinds(
    defs: &[FunctionDef],
    init: &[Stmt],
    block_stmts: &[Stmt],
    sample: &[Stmt],
    struct_instances: &HashMap<String, String>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    method_self_struct: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (
    HashMap<String, Vec<TypedFnParam>>,
    HashMap<String, Vec<TypedStructField>>,
) {
    let declared_struct_params =
        collect_declared_struct_param_types(defs, method_self_struct, struct_defs, errors);
    let declared_buffer_params = collect_declared_buffer_param_types(defs, options, errors);
    let field_usage = collect_def_param_field_usage(defs, errors);

    let mut kinds = HashMap::new();
    for def in defs {
        kinds.insert(
            def.name.clone(),
            vec![InferredFnParam::default(); def.params.len()],
        );
    }

    for def in defs {
        if let Some(explicit) = declared_struct_params.get(&def.name) {
            if let Some(kinds_for_def) = kinds.get_mut(&def.name) {
                for (idx, explicit_struct) in explicit.iter().enumerate() {
                    if let (Some(struct_name), Some(dst)) =
                        (explicit_struct.as_ref(), kinds_for_def.get_mut(idx))
                    {
                        dst.saw_structs.insert(struct_name.clone());
                    }
                }
            }
        }
    }

    for stmt in init {
        infer_stmt_calls(
            stmt,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }
    for stmt in block_stmts {
        infer_stmt_calls(
            stmt,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }
    for stmt in sample {
        infer_stmt_calls(
            stmt,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            &mut kinds,
            errors,
        );
    }

    // Propagate inferred def parameter kinds through def-to-def calls.
    for _ in 0..defs.len().saturating_add(1) {
        let snapshot = kinds.clone();
        for def in defs {
            let mut local_struct_instances = HashMap::<String, String>::new();
            let mut local_buffer_bindings = HashMap::<String, Vec<InferredBufferParam>>::new();

            if let Some(explicit_structs) = declared_struct_params.get(&def.name) {
                for (idx, explicit) in explicit_structs.iter().enumerate() {
                    if let (Some(struct_name), Some(param)) =
                        (explicit.as_ref(), def.params.get(idx))
                    {
                        local_struct_instances.insert(param.name.clone(), struct_name.clone());
                    }
                }
            }

            if let Some(explicit_buffers) = declared_buffer_params.get(&def.name) {
                for (idx, explicit) in explicit_buffers.iter().enumerate() {
                    if let (Some((elem_ty, channels)), Some(param)) =
                        (explicit.as_ref(), def.params.get(idx))
                    {
                        local_buffer_bindings.insert(
                            param.name.clone(),
                            vec![InferredBufferParam {
                                elem_ty: *elem_ty,
                                channels: channels.clone(),
                            }],
                        );
                    }
                }
            }

            if let Some(inferred_for_def) = kinds.get(&def.name) {
                for (idx, inferred_kind) in inferred_for_def.iter().enumerate() {
                    let Some(param) = def.params.get(idx) else {
                        continue;
                    };
                    if local_buffer_bindings.contains_key(&param.name) {
                        continue;
                    }
                    if !inferred_kind.saw_buffers.is_empty() {
                        local_buffer_bindings
                            .insert(param.name.clone(), inferred_kind.saw_buffers.clone());
                    }
                }
            }

            for stmt in &def.body {
                infer_stmt_calls(
                    stmt,
                    &local_struct_instances,
                    &local_buffer_bindings,
                    fn_signatures,
                    &mut kinds,
                    errors,
                );
            }
        }
        if kinds == snapshot {
            break;
        }
    }

    let mut out = HashMap::new();
    let mut synthesized = HashMap::new();

    for def in defs {
        let mut typed = Vec::with_capacity(def.params.len());
        let inferred = kinds.get(&def.name);
        let explicit = declared_struct_params
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| vec![None; def.params.len()]);
        let explicit_buffers = declared_buffer_params
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| vec![None; def.params.len()]);
        let usage = field_usage
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| vec![HashMap::new(); def.params.len()]);

        for idx in 0..def.params.len() {
            let inferred_kind = inferred
                .and_then(|v| v.get(idx))
                .cloned()
                .unwrap_or_default();
            let explicit_struct = explicit.get(idx).and_then(|s| s.as_ref());
            let explicit_buffer = explicit_buffers.get(idx).and_then(|s| s.as_ref());
            let param_name = def
                .params
                .get(idx)
                .map(|p| p.name.as_str())
                .unwrap_or("<param>");
            let usage_for_param = usage.get(idx).cloned().unwrap_or_default();

            if let Some((elem_ty, channels)) = explicit_buffer {
                if inferred_kind.saw_scalar {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as scalar",
                            def.name,
                            param_name,
                            format_buffer_type_name(*elem_ty, channels)
                        ),
                        0,
                        0,
                    ));
                }
                if !inferred_kind.saw_structs.is_empty() || !usage_for_param.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as struct",
                            def.name,
                            param_name,
                            format_buffer_type_name(*elem_ty, channels)
                        ),
                        0,
                        0,
                    ));
                }
                typed.push(TypedFnParam::Buffer {
                    elem_ty: *elem_ty,
                    channels: channels.clone(),
                });
                continue;
            }

            if !inferred_kind.saw_buffers.is_empty() {
                if inferred_kind.saw_scalar {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is used both as scalar and buffer",
                            def.name, param_name
                        ),
                        0,
                        0,
                    ));
                }
                if !inferred_kind.saw_structs.is_empty() || !usage_for_param.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is used both as struct and buffer",
                            def.name, param_name
                        ),
                        0,
                        0,
                    ));
                }
                let inferred_buffer = infer_untyped_buffer_from_observations(
                    &def.name,
                    param_name,
                    &inferred_kind,
                    true,
                    errors,
                )
                .unwrap_or(InferredBufferParam {
                    elem_ty: PrimitiveType::F32,
                    channels: TypedBufferChannels::Mono,
                });
                typed.push(TypedFnParam::Buffer {
                    elem_ty: inferred_buffer.elem_ty,
                    channels: inferred_buffer.channels,
                });
                continue;
            }

            if let Some(struct_name) = explicit_struct {
                if inferred_kind.saw_scalar {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is explicitly '{}' but is also used as scalar",
                            def.name, param_name, struct_name
                        ),
                        0,
                        0,
                    ));
                }
                for observed in &inferred_kind.saw_structs {
                    if observed != struct_name {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "function '{}' parameter '{}' is explicitly '{}' but is called with '{}'",
                                def.name, param_name, struct_name, observed
                            ),
                            0,
                            0,
                        ));
                    }
                }
                typed.push(TypedFnParam::Struct {
                    struct_name: struct_name.clone(),
                });
                continue;
            }

            if !inferred_kind.saw_structs.is_empty() || !usage_for_param.is_empty() {
                if inferred_kind.saw_scalar {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' is used both as scalar and struct",
                            def.name, param_name
                        ),
                        0,
                        0,
                    ));
                }

                let synthetic_name = synthetic_struct_param_name(&def.name, idx, param_name);
                let fields = build_structural_param_fields(
                    &def.name,
                    param_name,
                    &usage_for_param,
                    &inferred_kind.saw_structs,
                    struct_defs,
                    errors,
                );
                synthesized.insert(synthetic_name.clone(), fields);
                typed.push(TypedFnParam::Struct {
                    struct_name: synthetic_name,
                });
            } else {
                typed.push(TypedFnParam::Scalar);
            }
        }

        out.insert(def.name.clone(), typed);
    }

    (out, synthesized)
}

fn collect_declared_struct_param_types(
    defs: &[FunctionDef],
    method_self_struct: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<Option<String>>> {
    let mut out = HashMap::new();
    for def in defs {
        let mut param_structs = vec![None; def.params.len()];
        for (idx, param) in def.params.iter().enumerate() {
            if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                if !struct_defs.contains_key(struct_name) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' references unknown struct '{}'",
                            def.name, param.name, struct_name
                        ),
                        0,
                        0,
                    ));
                } else {
                    param_structs[idx] = Some(struct_name.clone());
                }
            }
        }

        if let Some(method_struct) = method_self_struct.get(&def.name) {
            if !param_structs.is_empty() {
                if let Some(existing) = param_structs[0].as_ref() {
                    if existing != method_struct {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "method '{}' self parameter is '{}' but annotation declares '{}'",
                                def.name, method_struct, existing
                            ),
                            0,
                            0,
                        ));
                    }
                }
                param_structs[0] = Some(method_struct.clone());
            }
        }

        out.insert(def.name.clone(), param_structs);
    }
    out
}

fn collect_declared_buffer_param_types(
    defs: &[FunctionDef],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<Option<(PrimitiveType, TypedBufferChannels)>>> {
    let mut out = HashMap::new();
    for def in defs {
        let mut param_buffers = vec![None; def.params.len()];
        for (idx, param) in def.params.iter().enumerate() {
            if let Some(FnParamType::Buffer(buffer_ty)) = &param.ty {
                let channels = match &buffer_ty.channels {
                    BufferChannels::Mono => TypedBufferChannels::Mono,
                    BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                    BufferChannels::Static(expr) => {
                        let context = format!(
                            "function '{}' parameter '{}' buffer channels",
                            def.name, param.name
                        );
                        let Some(channels) = eval_data_size_expr(expr, options, &context, errors)
                        else {
                            continue;
                        };
                        if channels == 1 {
                            TypedBufferChannels::Mono
                        } else {
                            TypedBufferChannels::Static(channels)
                        }
                    }
                };
                let elem_ty = match buffer_ty.elem {
                    BufferElemType::Primitive(ty) => ty,
                    BufferElemType::Generic(ref param_ty) => {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "function '{}' parameter '{}' uses unresolved generic buffer element type '{}'",
                                def.name, param.name, param_ty
                            ),
                            0,
                            0,
                        ));
                        PrimitiveType::F32
                    }
                };
                param_buffers[idx] = Some((elem_ty, channels));
            }
        }
        out.insert(def.name.clone(), param_buffers);
    }
    out
}

fn format_buffer_type_name(elem_ty: PrimitiveType, channels: &TypedBufferChannels) -> String {
    let elem = match elem_ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    };
    match channels {
        TypedBufferChannels::Mono => format!("buffer[{elem}]"),
        TypedBufferChannels::Static(ch) => format!("buffer[{elem}[{ch}]]"),
        TypedBufferChannels::Dynamic => format!("buffer[{elem}[]]"),
    }
}

fn collect_def_param_field_usage(
    defs: &[FunctionDef],
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<HashMap<String, StructFieldUsage>>> {
    let mut out = HashMap::new();
    for def in defs {
        let mut by_param = vec![HashMap::new(); def.params.len()];
        let param_index = def
            .params
            .iter()
            .enumerate()
            .map(|(idx, p)| (p.name.clone(), idx))
            .collect::<HashMap<_, _>>();
        for stmt in &def.body {
            collect_stmt_field_usage(stmt, &def.name, &param_index, &mut by_param, errors);
        }
        out.insert(def.name.clone(), by_param);
    }
    out
}

fn collect_stmt_field_usage(
    stmt: &Stmt,
    fn_name: &str,
    param_index: &HashMap<String, usize>,
    usage: &mut [HashMap<String, StructFieldUsage>],
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some((base, field)) = split_simple_field_path(name) {
                        if let Some(param_idx) = param_index.get(base).copied() {
                            mark_param_field_usage(
                                usage,
                                param_idx,
                                field,
                                StructFieldUsage::Scalar,
                                fn_name,
                                base,
                                errors,
                            );
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(param_idx) = param_index.get(root).copied() {
                            mark_param_field_usage(
                                usage,
                                param_idx,
                                field,
                                StructFieldUsage::Data,
                                fn_name,
                                root,
                                errors,
                            );
                        }
                    }
                    collect_expr_field_usage(index, fn_name, param_index, usage, errors);
                }
            }
            collect_expr_field_usage(expr, fn_name, param_index, usage, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_expr_field_usage(expr, fn_name, param_index, usage, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_expr_field_usage(cond, fn_name, param_index, usage, errors);
            for nested in then_branch {
                collect_stmt_field_usage(nested, fn_name, param_index, usage, errors);
            }
            for nested in else_branch {
                collect_stmt_field_usage(nested, fn_name, param_index, usage, errors);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            collect_expr_field_usage(start, fn_name, param_index, usage, errors);
            collect_expr_field_usage(end, fn_name, param_index, usage, errors);
            for nested in body {
                collect_stmt_field_usage(nested, fn_name, param_index, usage, errors);
            }
        }
    }
}

fn collect_expr_field_usage(
    expr: &Expr,
    fn_name: &str,
    param_index: &HashMap<String, usize>,
    usage: &mut [HashMap<String, StructFieldUsage>],
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::DataCtor { .. } => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                collect_expr_field_usage(value, fn_name, param_index, usage, errors);
            }
        }
        Expr::Var(name) => {
            if let Some((base, field)) = split_simple_field_path(name) {
                if let Some(param_idx) = param_index.get(base).copied() {
                    mark_param_field_usage(
                        usage,
                        param_idx,
                        field,
                        StructFieldUsage::Scalar,
                        fn_name,
                        base,
                        errors,
                    );
                }
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if let Some(param_idx) = param_index.get(root).copied() {
                    mark_param_field_usage(
                        usage,
                        param_idx,
                        field,
                        StructFieldUsage::Data,
                        fn_name,
                        root,
                        errors,
                    );
                }
            }
            collect_expr_field_usage(index, fn_name, param_index, usage, errors);
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            collect_expr_field_usage(lhs, fn_name, param_index, usage, errors);
            collect_expr_field_usage(rhs, fn_name, param_index, usage, errors);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
            collect_expr_field_usage(expr, fn_name, param_index, usage, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_field_usage(arg, fn_name, param_index, usage, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            if let Expr::UserCall { name, .. } = expr {
                if let Some(base) = parse_data_len_instance_base(name) {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(param_idx) = param_index.get(root).copied() {
                            mark_param_field_usage(
                                usage,
                                param_idx,
                                field,
                                StructFieldUsage::Data,
                                fn_name,
                                root,
                                errors,
                            );
                        }
                    }
                }
            }
            for arg in args {
                collect_expr_field_usage(&arg.expr, fn_name, param_index, usage, errors);
            }
        }
    }
}

fn mark_param_field_usage(
    usage: &mut [HashMap<String, StructFieldUsage>],
    param_idx: usize,
    field: &str,
    kind: StructFieldUsage,
    fn_name: &str,
    param_name: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(map) = usage.get_mut(param_idx) else {
        return;
    };
    if let Some(existing) = map.get(field).copied() {
        if existing != kind {
            errors.push(Diagnostic::semantic(
                format!(
                    "function '{}' parameter '{}' uses field '{}' both as scalar and Data",
                    fn_name, param_name, field
                ),
                0,
                0,
            ));
        }
        return;
    }
    map.insert(field.to_owned(), kind);
}

fn split_simple_field_path(name: &str) -> Option<(&str, &str)> {
    let mut parts = name.split('.');
    let first = parts.next()?;
    let second = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((first, second))
}

fn synthetic_struct_param_name(def_name: &str, idx: usize, param_name: &str) -> String {
    fn sanitize(input: &str) -> String {
        input
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    }

    format!(
        "__omni_struct_any_{}_{}_{}",
        sanitize(def_name),
        idx,
        sanitize(param_name)
    )
}

fn build_structural_param_fields(
    fn_name: &str,
    param_name: &str,
    usage: &HashMap<String, StructFieldUsage>,
    observed_structs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedStructField> {
    let mut field_names = usage.keys().cloned().collect::<Vec<_>>();
    field_names.sort();

    let mut observed = observed_structs.iter().cloned().collect::<Vec<_>>();
    observed.sort();

    let mut out = Vec::with_capacity(field_names.len());

    for field_name in field_names {
        let required_kind = usage
            .get(&field_name)
            .copied()
            .unwrap_or(StructFieldUsage::Scalar);
        let mut resolved_ty: Option<TypedFieldType> = None;
        let mut resolved_data_elem_ty: Option<Option<PrimitiveType>> = None;
        let mut resolved_data_elem_struct: Option<Option<String>> = None;

        for struct_name in &observed {
            let Some(fields) = struct_defs.get(struct_name) else {
                continue;
            };
            let Some(found) = fields.iter().find(|f| f.name == field_name) else {
                errors.push(Diagnostic::semantic(
                    format!(
                        "function '{}' parameter '{}' requires field '{}' but struct '{}' does not define it",
                        fn_name, param_name, field_name, struct_name
                    ),
                    0,
                    0,
                ));
                continue;
            };

            let (candidate, candidate_data_elem_ty, candidate_data_elem_struct) = match (
                required_kind,
                found.ty,
            ) {
                (StructFieldUsage::Scalar, TypedFieldType::Scalar(prim)) => {
                    (TypedFieldType::Scalar(prim), None, None)
                }
                (StructFieldUsage::Scalar, TypedFieldType::Data(_)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' uses '{}.{}' as scalar but struct '{}' defines it as Data",
                            fn_name, param_name, param_name, field_name, struct_name
                        ),
                        0,
                        0,
                    ));
                    continue;
                }
                (StructFieldUsage::Data, TypedFieldType::Data(len)) => (
                    TypedFieldType::Data(len),
                    found.data_elem_ty,
                    found.data_elem_struct.clone(),
                ),
                (StructFieldUsage::Data, TypedFieldType::Scalar(_)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' uses '{}.{}' as Data but struct '{}' defines it as scalar",
                            fn_name, param_name, param_name, field_name, struct_name
                        ),
                        0,
                        0,
                    ));
                    continue;
                }
            };

            if let Some(existing) = resolved_ty {
                let existing_data_elem_ty = resolved_data_elem_ty.flatten();
                let existing_data_elem_struct = resolved_data_elem_struct.clone().unwrap_or(None);
                if existing != candidate
                    || existing_data_elem_ty != candidate_data_elem_ty
                    || existing_data_elem_struct != candidate_data_elem_struct
                {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' parameter '{}' field '{}' resolves to incompatible types across structs",
                            fn_name, param_name, field_name
                        ),
                        0,
                        0,
                    ));
                }
            } else {
                resolved_ty = Some(candidate);
                resolved_data_elem_ty = Some(candidate_data_elem_ty);
                resolved_data_elem_struct = Some(candidate_data_elem_struct);
            }
        }

        let ty = if let Some(resolved) = resolved_ty {
            resolved
        } else {
            match required_kind {
                StructFieldUsage::Scalar => TypedFieldType::Scalar(PrimitiveType::F32),
                StructFieldUsage::Data => TypedFieldType::Data(1),
            }
        };
        let data_elem_ty = resolved_data_elem_ty.flatten();
        let data_elem_struct = resolved_data_elem_struct.unwrap_or(None);

        out.push(TypedStructField {
            name: field_name,
            ty,
            default: None,
            data_elem_ty,
            data_elem_struct,
        });
    }

    out
}

fn param_struct_map_from_kinds(
    param_names: &[String],
    kinds: &[TypedFnParam],
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (name, kind) in param_names.iter().zip(kinds.iter()) {
        if let TypedFnParam::Struct { struct_name } = kind {
            out.insert(name.clone(), struct_name.clone());
        }
    }
    out
}

fn param_buffer_map_from_kinds(
    param_names: &[String],
    kinds: &[TypedFnParam],
) -> HashMap<String, (PrimitiveType, TypedBufferChannels)> {
    let mut out = HashMap::new();
    for (name, kind) in param_names.iter().zip(kinds.iter()) {
        if let TypedFnParam::Buffer { elem_ty, channels } = kind {
            out.insert(name.clone(), (*elem_ty, channels.clone()));
        }
    }
    out
}

fn merge_inferred_buffer_channels(
    lhs: &TypedBufferChannels,
    rhs: &TypedBufferChannels,
) -> TypedBufferChannels {
    use TypedBufferChannels::{Dynamic, Mono, Static};
    match (lhs, rhs) {
        (Mono, Mono) => Mono,
        (Static(a), Static(b)) if a == b => {
            if *a == 1 {
                Mono
            } else {
                Static(*a)
            }
        }
        (Mono, Static(1)) | (Static(1), Mono) => Mono,
        (Dynamic, _) | (_, Dynamic) => Dynamic,
        _ => Dynamic,
    }
}

fn infer_untyped_buffer_from_observations(
    _fn_name: &str,
    _param_name: &str,
    inferred: &InferredFnParam,
    _report_errors: bool,
    _errors: &mut Vec<Diagnostic>,
) -> Option<InferredBufferParam> {
    if inferred.saw_buffers.is_empty() {
        return None;
    }
    let first = inferred.saw_buffers[0].clone();
    let mut merged_channels = first.channels.clone();
    let mut merged_elem = first.elem_ty;
    for seen in inferred.saw_buffers.iter().skip(1) {
        merged_elem = match (merged_elem, seen.elem_ty) {
            (PrimitiveType::F32, PrimitiveType::F64) | (PrimitiveType::F64, PrimitiveType::F32) => {
                PrimitiveType::F64
            }
            (lhs, rhs) if lhs == rhs => lhs,
            (lhs, _) => lhs,
        };
        merged_channels = merge_inferred_buffer_channels(&merged_channels, &seen.channels);
    }
    Some(InferredBufferParam {
        elem_ty: merged_elem,
        channels: merged_channels,
    })
}

fn infer_stmt_calls(
    stmt: &Stmt,
    struct_instances: &HashMap<String, String>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                infer_expr_calls(
                    index,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            infer_expr_calls(
                expr,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Stmt::Expr { expr, .. } => infer_expr_calls(
            expr,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            kinds,
            errors,
        ),
        Stmt::Return { expr, .. } => infer_expr_calls(
            expr,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            kinds,
            errors,
        ),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            infer_expr_calls(
                cond,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            for nested in then_branch {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            for nested in else_branch {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            infer_expr_calls(
                start,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                end,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            for nested in body {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
    });
}

fn infer_expr_calls(
    expr: &Expr,
    struct_instances: &HashMap<String, String>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::DataCtor { .. } | Expr::Var(_) => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                infer_expr_calls(
                    value,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Expr::Index { index, .. } => {
            infer_expr_calls(
                index,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            infer_expr_calls(
                lhs,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                rhs,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
            infer_expr_calls(
                expr,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Logical { lhs, rhs, .. } => {
            infer_expr_calls(
                lhs,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                rhs,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                infer_expr_calls(
                    arg,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            if let Some(sig) = fn_signatures.get(name) {
                let resolved = resolve_call_args(
                    args,
                    &sig.params,
                    &sig.defaults,
                    false,
                    false,
                    &format!("function '{name}' call"),
                    errors,
                );
                if let Some(param_kinds) = kinds.get_mut(name) {
                    for (idx, arg) in resolved.into_iter().enumerate() {
                        if let Some(arg) = arg {
                            if let Some(slot) = param_kinds.get_mut(idx) {
                                match arg {
                                    Expr::Var(v) => {
                                        if let Some(struct_name) = struct_instances.get(v) {
                                            slot.saw_structs.insert(struct_name.clone());
                                        } else if let Some(buffer_infos) = buffer_bindings.get(v) {
                                            for buffer_info in buffer_infos {
                                                if !slot.saw_buffers.iter().any(|seen| {
                                                    seen.elem_ty == buffer_info.elem_ty
                                                        && seen.channels == buffer_info.channels
                                                }) {
                                                    slot.saw_buffers.push(buffer_info.clone());
                                                }
                                            }
                                        } else {
                                            slot.saw_scalar = true;
                                        }
                                    }
                                    _ => slot.saw_scalar = true,
                                }
                            }
                        }
                    }
                }
            }
            for arg in args {
                infer_expr_calls(
                    &arg.expr,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
    }
}

fn resolve_call_args<'a>(
    args: &'a [CallArg],
    param_names: &[String],
    param_defaults: &[Option<Expr>],
    forbid_self_named: bool,
    named_only: bool,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Option<&'a Expr>> {
    let mut resolved: Vec<Option<&Expr>> = vec![None; param_names.len()];
    let mut next_pos = 0usize;
    let mut seen_named = HashSet::new();
    let mut saw_named = false;

    for arg in args {
        if let Some(name) = &arg.name {
            saw_named = true;
            if forbid_self_named && name == "self" {
                errors.push(Diagnostic::semantic(
                    format!("{context}: 'self' cannot be passed as a named argument"),
                    0,
                    0,
                ));
                continue;
            }
            if !seen_named.insert(name.clone()) {
                errors.push(Diagnostic::semantic(
                    format!("{context}: duplicate named argument '{name}'"),
                    0,
                    0,
                ));
                continue;
            }
            let Some(idx) = param_names.iter().position(|p| p == name) else {
                errors.push(Diagnostic::semantic(
                    format!("{context}: unknown named argument '{name}'"),
                    0,
                    0,
                ));
                continue;
            };
            if resolved[idx].is_some() {
                errors.push(Diagnostic::semantic(
                    format!("{context}: argument '{name}' provided multiple times"),
                    0,
                    0,
                ));
                continue;
            }
            resolved[idx] = Some(&arg.expr);
        } else {
            if named_only {
                errors.push(Diagnostic::semantic(
                    format!("{context}: positional arguments are not allowed; use named arguments"),
                    0,
                    0,
                ));
                continue;
            }
            if saw_named {
                errors.push(Diagnostic::semantic(
                    format!("{context}: positional arguments must come before named arguments"),
                    0,
                    0,
                ));
                continue;
            }
            while next_pos < resolved.len() && resolved[next_pos].is_some() {
                next_pos += 1;
            }
            if next_pos >= resolved.len() {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context}: too many positional arguments (expected at most {})",
                        param_names.len()
                    ),
                    0,
                    0,
                ));
                continue;
            }
            resolved[next_pos] = Some(&arg.expr);
            next_pos += 1;
        }
    }

    for idx in 0..resolved.len() {
        let has_default = matches!(param_defaults.get(idx), Some(Some(_)));
        if resolved[idx].is_none() && !has_default {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: missing required argument '{}'",
                    param_names[idx]
                ),
                0,
                0,
            ));
        }
    }

    resolved
}

fn validate_default_expr(expr: &Expr, errors: &mut Vec<Diagnostic>, context: &str) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                validate_default_expr(value, errors, context);
            }
            errors.push(Diagnostic::semantic(
                "array literals are only allowed in typed array declarations and parameter defaults",
                0,
                0,
            ));
        }
        Expr::Var(name) => {
            if !is_builtin_constant_name(name) {
                errors.push(Diagnostic::semantic(
                    format!("{context} default expression uses non-constant symbol '{name}'"),
                    0,
                    0,
                ));
            }
        }
        Expr::Binary { lhs, rhs, .. } | Expr::Compare { lhs, rhs, .. } => {
            validate_default_expr(lhs, errors, context);
            validate_default_expr(rhs, errors, context);
        }
        _ => {
            errors.push(Diagnostic::semantic(
                format!("{context} default expression must be constant"),
                0,
                0,
            ));
        }
    }
}

fn can_implicitly_assign(src: PrimitiveType, dst: PrimitiveType) -> bool {
    if src == dst {
        return true;
    }
    matches!(
        (src, dst),
        (PrimitiveType::I32, PrimitiveType::I64)
            | (PrimitiveType::I32, PrimitiveType::F32)
            | (PrimitiveType::I32, PrimitiveType::F64)
            | (PrimitiveType::I64, PrimitiveType::F64)
            | (PrimitiveType::F32, PrimitiveType::F64)
    )
}

fn merge_numeric_types(
    lhs: PrimitiveType,
    rhs: PrimitiveType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (F64, I32)
        | (I32, F64)
        | (F64, I64)
        | (I64, F64)
        | (F64, F32)
        | (F32, F64)
        | (F64, F64) => Some(F64),
        (F32, I32) | (I32, F32) | (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context} requires numeric operands, got {:?} and {:?}",
                    lhs, rhs
                ),
                0,
                0,
            ));
            None
        }
    }
}

fn merge_inferred_return_types(lhs: PrimitiveType, rhs: PrimitiveType) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (a, b) if a == b => Some(a),
        (Bool, _) | (_, Bool) => None,
        (F64, _) | (_, F64) => Some(F64),
        (F32, I64) | (I64, F32) => Some(F64),
        (F32, I32) | (I32, F32) => Some(F32),
        (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
    }
}

fn infer_expr_type_for_def_return_inference_with_call_overrides(
    expr: &Expr,
    locals: &HashMap<String, PrimitiveType>,
    fn_return_types: &HashMap<String, PrimitiveType>,
    call_return_type_overrides: Option<&HashMap<String, PrimitiveType>>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Int(v) => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::ArrayLiteral(_) | Expr::DataCtor { .. } => None,
        Expr::Var(name) => {
            if is_builtin_constant_name(name) {
                Some(PrimitiveType::F32)
            } else {
                locals.get(name).copied()
            }
        }
        Expr::Index { base, .. } => locals.get(base).copied().or(Some(PrimitiveType::F32)),
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Compare { .. } | Expr::Logical { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::Binary { lhs, rhs, .. } => {
            let l = infer_expr_type_for_def_return_inference_with_call_overrides(
                lhs,
                locals,
                fn_return_types,
                call_return_type_overrides,
            )?;
            let r = infer_expr_type_for_def_return_inference_with_call_overrides(
                rhs,
                locals,
                fn_return_types,
                call_return_type_overrides,
            )?;
            merge_inferred_return_types(l, r)
        }
        Expr::Call { func, args } => {
            let arg_tys = args
                .iter()
                .filter_map(|arg| {
                    infer_expr_type_for_def_return_inference_with_call_overrides(
                        arg,
                        locals,
                        fn_return_types,
                        call_return_type_overrides,
                    )
                })
                .collect::<Vec<_>>();
            if arg_tys.len() != args.len() {
                return None;
            }
            match func {
                BuiltinFn::Abs => arg_tys.first().copied(),
                BuiltinFn::Min | BuiltinFn::Max => {
                    let lhs = arg_tys.first().copied().unwrap_or(PrimitiveType::F32);
                    let rhs = arg_tys.get(1).copied().unwrap_or(PrimitiveType::F32);
                    merge_inferred_return_types(lhs, rhs)
                }
                BuiltinFn::Pow => Some(if arg_tys.iter().any(|t| *t == PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
                _ => Some(if arg_tys.iter().any(|t| *t == PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
            }
        }
        Expr::UserCall { name, args, .. } => {
            if parse_data_len_instance_base(name).is_some()
                || parse_buffer_chans_instance_base(name).is_some()
            {
                return Some(PrimitiveType::I32);
            }
            if is_internal_buffer_2d_fn(name) {
                if let Some(CallArg {
                    expr: Expr::Var(base),
                    ..
                }) = args.first()
                {
                    if let Some(ty) = locals.get(base).copied() {
                        return Some(ty);
                    }
                }
            }
            if let Some(overrides) = call_return_type_overrides {
                if let Some(ty) = overrides.get(name).copied() {
                    return Some(ty);
                }
            }
            fn_return_types
                .get(name)
                .copied()
                .or(Some(PrimitiveType::F32))
        }
    }
}

fn infer_expr_type_for_def_return_inference(
    expr: &Expr,
    locals: &HashMap<String, PrimitiveType>,
    fn_return_types: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    infer_expr_type_for_def_return_inference_with_call_overrides(
        expr,
        locals,
        fn_return_types,
        None,
    )
}

fn infer_stmt_returns_for_def_return_inference(
    stmts: &[Stmt],
    locals: &mut HashMap<String, PrimitiveType>,
    fn_return_types: &HashMap<String, PrimitiveType>,
    out: &mut Vec<PrimitiveType>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                decl_ty,
                expr,
                ..
            } => {
                if split_simple_field_path(name).is_some() {
                    continue;
                }
                if matches!(expr, Expr::DataCtor { .. }) {
                    continue;
                }
                let inferred =
                    infer_expr_type_for_def_return_inference(expr, locals, fn_return_types);
                let target_ty = (*decl_ty)
                    .or_else(|| locals.get(name).copied())
                    .or(inferred)
                    .unwrap_or(PrimitiveType::F32);
                locals.entry(name.clone()).or_insert(target_ty);
            }
            Stmt::Assign {
                target: AssignTarget::Index { .. },
                ..
            } => {}
            Stmt::Expr { .. } => {}
            Stmt::Return { expr, .. } => {
                let ty = infer_expr_type_for_def_return_inference(expr, locals, fn_return_types)
                    .unwrap_or(PrimitiveType::F32);
                out.push(ty);
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                let mut then_locals = locals.clone();
                let mut else_locals = locals.clone();
                infer_stmt_returns_for_def_return_inference(
                    then_branch,
                    &mut then_locals,
                    fn_return_types,
                    out,
                );
                infer_stmt_returns_for_def_return_inference(
                    else_branch,
                    &mut else_locals,
                    fn_return_types,
                    out,
                );
                let mut merged = locals.clone();
                for (name, then_ty) in &then_locals {
                    if let Some(else_ty) = else_locals.get(name) {
                        if then_ty == else_ty {
                            merged.insert(name.clone(), *then_ty);
                        }
                    }
                }
                *locals = merged;
            }
            Stmt::For { var, body, .. } => {
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone(), PrimitiveType::I32);
                infer_stmt_returns_for_def_return_inference(
                    body,
                    &mut loop_locals,
                    fn_return_types,
                    out,
                );
            }
        }
    }
}

fn infer_def_return_type(
    def: &FunctionDef,
    sig: &FnSignature,
    fn_return_types: &HashMap<String, PrimitiveType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> PrimitiveType {
    let mut locals = HashMap::<String, PrimitiveType>::new();
    for (idx, param) in sig.params.iter().enumerate() {
        match sig.param_types.get(idx).and_then(|ty| ty.as_ref()) {
            Some(FnParamType::Primitive(prim)) => {
                locals.insert(param.clone(), *prim);
            }
            Some(FnParamType::Struct(struct_name)) => {
                if let Some(fields) = struct_defs.get(struct_name) {
                    for field in fields {
                        if let TypedFieldType::Scalar(prim) = field.ty {
                            locals.insert(format!("{param}.{}", field.name), prim);
                        }
                    }
                }
                // Preserve previous fallback for direct uses of the struct parameter symbol.
                locals.insert(param.clone(), PrimitiveType::F32);
            }
            Some(FnParamType::Buffer(_)) => {
                locals.insert(param.clone(), PrimitiveType::F32);
            }
            None => {
                locals.insert(param.clone(), PrimitiveType::F32);
            }
        }
    }

    let mut returns = Vec::<PrimitiveType>::new();
    infer_stmt_returns_for_def_return_inference(
        &def.body,
        &mut locals,
        fn_return_types,
        &mut returns,
    );
    let mut it = returns.into_iter();
    let Some(mut out) = it.next() else {
        return PrimitiveType::F32;
    };
    for ty in it {
        let Some(merged) = merge_inferred_return_types(out, ty) else {
            return PrimitiveType::F32;
        };
        out = merged;
    }
    out
}

fn infer_def_return_types(
    defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> HashMap<String, PrimitiveType> {
    let mut out = defs
        .iter()
        .map(|d| (d.name.clone(), PrimitiveType::F32))
        .collect::<HashMap<_, _>>();
    for _ in 0..defs.len().saturating_add(1) {
        let mut changed = false;
        for def in defs {
            let Some(sig) = fn_signatures.get(&def.name) else {
                continue;
            };
            let inferred = infer_def_return_type(def, sig, &out, struct_defs);
            if out.get(&def.name).copied() != Some(inferred) {
                out.insert(def.name.clone(), inferred);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn infer_scalar_expr_type(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    local_data_aliases: &HashMap<String, LocalDataAliasInfo>,
    locals: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Int(v) => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::ArrayLiteral(_) => None,
        Expr::Var(name) => {
            if is_builtin_constant_name(name) {
                return Some(PrimitiveType::F32);
            }
            if let Some((base, field)) = split_field_path(name, errors) {
                let flat = format!("{base}.{field}");
                if let Some(ty) = state_scalars.get(&flat).copied() {
                    return Some(ty);
                }
                if let Some(struct_name) = struct_instances.get(base) {
                    if let Some(fields) = struct_defs.get(struct_name) {
                        if let Some(field_decl) = fields.iter().find(|f| f.name == field) {
                            return Some(match field_decl.ty {
                                TypedFieldType::Scalar(prim) => prim,
                                TypedFieldType::Data(_) => PrimitiveType::F32,
                            });
                        }
                    }
                }
                None
            } else if let Some(ty) = state_scalars.get(name).copied() {
                Some(ty)
            } else if locals.contains(name) {
                Some(PrimitiveType::I32)
            } else if input_names.contains(name) {
                Some(
                    get_declared_symbol_type(state_scalars, name, DECLARED_INPUT_TYPE_PREFIX)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else if output_names.contains(name) {
                Some(
                    get_declared_symbol_type(state_scalars, name, DECLARED_OUTPUT_TYPE_PREFIX)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else if param_names.contains(name) {
                Some(
                    get_declared_symbol_type(state_scalars, name, DECLARED_PARAM_TYPE_PREFIX)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else {
                None
            }
        }
        Expr::Index { base, .. } => {
            if let Some(alias) = local_data_aliases.get(base) {
                if alias.elem_struct.is_none() {
                    return Some(alias.elem_ty);
                }
            }
            if let Some((root, field)) = split_field_path(base, errors) {
                if let Some(struct_name) = struct_instances.get(root) {
                    if let Some(fields) = struct_defs.get(struct_name) {
                        if let Some(field_decl) = fields.iter().find(|f| f.name == field) {
                            if let TypedFieldType::Data(_) = field_decl.ty {
                                if let Some(elem_ty) = field_decl.data_elem_ty {
                                    return Some(elem_ty);
                                }
                            }
                        }
                    }
                }
                // Proc-lowered state fields are often addressed as `self.field[...]` while
                // declared element metadata is keyed by bare field name.
                if let Some(ty) = get_declared_symbol_type(
                    state_scalars,
                    &field,
                    DECLARED_DATA_ELEM_TYPE_PREFIX,
                ) {
                    return Some(ty);
                }
                if let Some(ty) = get_declared_symbol_type(
                    state_scalars,
                    &field,
                    DECLARED_BUFFER_ELEM_TYPE_PREFIX,
                ) {
                    return Some(ty);
                }
            }
            if let Some(ty) =
                get_declared_symbol_type(state_scalars, base, DECLARED_DATA_ELEM_TYPE_PREFIX)
            {
                return Some(ty);
            }
            if let Some(ty) =
                get_declared_symbol_type(state_scalars, base, DECLARED_BUFFER_ELEM_TYPE_PREFIX)
            {
                return Some(ty);
            }
            Some(PrimitiveType::F32)
        }
        Expr::DataCtor { .. } => None,
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::Call { func, args } => {
            let arg_types = args
                .iter()
                .map(|arg| {
                    infer_scalar_expr_type(
                        arg,
                        state_scalars,
                        local_data_aliases,
                        locals,
                        input_names,
                        output_names,
                        param_names,
                        struct_instances,
                        struct_defs,
                        errors,
                    )
                })
                .collect::<Vec<_>>();
            if arg_types.iter().any(|t| t.is_none()) {
                return None;
            }
            let arg_types = arg_types.into_iter().flatten().collect::<Vec<_>>();

            match func {
                BuiltinFn::Abs => {
                    let ty = arg_types.first().copied().unwrap_or(PrimitiveType::F32);
                    if ty == PrimitiveType::Bool {
                        errors.push(Diagnostic::semantic(
                            "builtin 'abs' requires numeric argument (bool is not supported)",
                            0,
                            0,
                        ));
                        None
                    } else {
                        Some(ty)
                    }
                }
                BuiltinFn::Min | BuiltinFn::Max => {
                    let lhs = arg_types.first().copied().unwrap_or(PrimitiveType::F32);
                    let rhs = arg_types.get(1).copied().unwrap_or(PrimitiveType::F32);
                    merge_numeric_types(
                        lhs,
                        rhs,
                        &format!("builtin '{}'", builtin_name(*func)),
                        errors,
                    )
                }
                BuiltinFn::Pow => {
                    for ty in &arg_types {
                        if *ty == PrimitiveType::Bool {
                            errors.push(Diagnostic::semantic(
                                "builtin 'pow' requires numeric arguments (bool is not supported)",
                                0,
                                0,
                            ));
                            return None;
                        }
                    }
                    Some(if arg_types.iter().any(|t| *t == PrimitiveType::F64) {
                        PrimitiveType::F64
                    } else {
                        PrimitiveType::F32
                    })
                }
                _ => {
                    for ty in &arg_types {
                        if !is_float_type(*ty) {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "builtin '{}' requires float arguments (f32/f64), got {:?}",
                                    builtin_name(*func),
                                    ty
                                ),
                                0,
                                0,
                            ));
                            return None;
                        }
                    }
                    Some(if arg_types.iter().any(|t| *t == PrimitiveType::F64) {
                        PrimitiveType::F64
                    } else {
                        PrimitiveType::F32
                    })
                }
            }
        }
        Expr::UserCall { name, args, .. } => {
            if parse_data_len_instance_base(name).is_some()
                || parse_buffer_chans_instance_base(name).is_some()
            {
                return Some(PrimitiveType::I32);
            }
            if is_internal_buffer_2d_fn(name) {
                if let Some(CallArg {
                    expr: Expr::Var(base),
                    ..
                }) = args.first()
                {
                    if let Some(ty) = get_declared_symbol_type(
                        state_scalars,
                        base,
                        DECLARED_BUFFER_ELEM_TYPE_PREFIX,
                    ) {
                        return Some(ty);
                    }
                }
                return Some(PrimitiveType::F32);
            }
            if is_builtin_unsafe_data_fn(name) {
                if let Some(CallArg {
                    expr: Expr::Var(base),
                    ..
                }) = args.first()
                {
                    if let Some(alias) = local_data_aliases.get(base) {
                        if alias.elem_struct.is_none() {
                            return Some(alias.elem_ty);
                        }
                    }
                }
            }
            get_declared_symbol_type(state_scalars, name, DECLARED_FUNCTION_RETURN_TYPE_PREFIX)
                .or(Some(PrimitiveType::F32))
        }
        Expr::Binary { lhs, rhs, .. } => {
            let l = infer_scalar_expr_type(
                lhs,
                state_scalars,
                local_data_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            let r = infer_scalar_expr_type(
                rhs,
                state_scalars,
                local_data_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            if let (Some(l), Some(r)) = (l, r) {
                merge_numeric_types(l, r, "binary expression", errors)
            } else {
                None
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn infer_expr_type_for_semantics(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    param_structs: Option<&HashMap<String, String>>,
    locals: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    let empty_local_data_aliases = HashMap::<String, LocalDataAliasInfo>::new();
    infer_expr_type_for_semantics_with_local_data(
        expr,
        state_scalars,
        param_structs,
        &empty_local_data_aliases,
        locals,
        input_names,
        output_names,
        param_names,
        struct_instances,
        struct_defs,
        errors,
    )
}

#[allow(clippy::too_many_arguments)]
fn infer_expr_type_for_semantics_with_local_data(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    param_structs: Option<&HashMap<String, String>>,
    local_data_aliases: &HashMap<String, LocalDataAliasInfo>,
    locals: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    let merged_struct_instances;
    let struct_instance_ctx = if let Some(param_structs) = param_structs {
        if struct_instances.is_empty() {
            param_structs
        } else if param_structs.is_empty() {
            struct_instances
        } else {
            merged_struct_instances = {
                let mut merged = struct_instances.clone();
                for (name, struct_name) in param_structs {
                    merged
                        .entry(name.clone())
                        .or_insert_with(|| struct_name.clone());
                }
                merged
            };
            &merged_struct_instances
        }
    } else {
        struct_instances
    };

    infer_scalar_expr_type(
        expr,
        state_scalars,
        local_data_aliases,
        locals,
        input_names,
        output_names,
        param_names,
        struct_instance_ctx,
        struct_defs,
        errors,
    )
}

fn require_assignable_type(
    src: Option<PrimitiveType>,
    dst: PrimitiveType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(src) = src {
        if src != dst && !can_implicitly_assign(src, dst) {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context} type mismatch: cannot assign {:?} to {:?}",
                    src, dst
                ),
                0,
                0,
            ));
        }
    }
}

fn require_numeric_type(ty: Option<PrimitiveType>, context: &str, errors: &mut Vec<Diagnostic>) {
    if let Some(ty) = ty {
        if !matches!(
            ty,
            PrimitiveType::F32 | PrimitiveType::F64 | PrimitiveType::I32 | PrimitiveType::I64
        ) {
            errors.push(Diagnostic::semantic(
                format!("{context} requires numeric type, got {:?}", ty),
                0,
                0,
            ));
        }
    }
}

fn require_bool_type(ty: Option<PrimitiveType>, context: &str, errors: &mut Vec<Diagnostic>) {
    if let Some(ty) = ty {
        if ty != PrimitiveType::Bool {
            errors.push(Diagnostic::semantic(
                format!("{context} requires bool type, got {:?}", ty),
                0,
                0,
            ));
        }
    }
}
fn desugar_expr_instance_method_calls(expr: &mut Expr, struct_instances: &HashMap<String, String>) {
    match expr {
        Expr::Index { index, .. } => desugar_expr_instance_method_calls(index, struct_instances),
        Expr::DataCtor { spec, init } => {
            desugar_expr_instance_method_calls(&mut spec.size, struct_instances);
            if let Some(values) = init {
                for value in values {
                    desugar_expr_instance_method_calls(value, struct_instances);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            desugar_expr_instance_method_calls(lhs, struct_instances);
            desugar_expr_instance_method_calls(rhs, struct_instances);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                desugar_expr_instance_method_calls(arg, struct_instances);
            }
        }
        Expr::Cast { expr: arg, .. } | Expr::UnaryNot { expr: arg } => {
            desugar_expr_instance_method_calls(arg, struct_instances)
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                desugar_expr_instance_method_calls(value, struct_instances);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                desugar_expr_instance_method_calls(&mut arg.expr, struct_instances);
            }
            if let Some((base, method)) = split_simple_field_path(name) {
                if let Some(struct_name) = struct_instances.get(base) {
                    let base_name = base.to_owned();
                    let method_name = method.to_owned();
                    *name = format!("{}.{}", struct_name, method_name);
                    args.insert(
                        0,
                        CallArg {
                            name: None,
                            expr: Expr::Var(base_name),
                        },
                    );
                }
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

fn desugar_init_instance_method_calls(
    stmt: &mut Stmt,
    struct_instances: &mut HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Var(name) = target {
                if let Expr::UserCall {
                    name: struct_name,
                    type_args,
                    ..
                } = expr
                {
                    if type_args.is_empty() && struct_defs.contains_key(struct_name) {
                        struct_instances.insert(name.clone(), struct_name.clone());
                    }
                }
            }
            if let AssignTarget::Index { index, .. } = target {
                desugar_expr_instance_method_calls(index, struct_instances);
            }
            desugar_expr_instance_method_calls(expr, struct_instances);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            desugar_expr_instance_method_calls(expr, struct_instances);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            desugar_expr_instance_method_calls(cond, struct_instances);
            for nested in then_branch.iter_mut() {
                desugar_init_instance_method_calls(nested, struct_instances, struct_defs);
            }
            for nested in else_branch.iter_mut() {
                desugar_init_instance_method_calls(nested, struct_instances, struct_defs);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            desugar_expr_instance_method_calls(start, struct_instances);
            desugar_expr_instance_method_calls(end, struct_instances);
            for nested in body.iter_mut() {
                desugar_init_instance_method_calls(nested, struct_instances, struct_defs);
            }
        }
    }
}

fn desugar_sample_instance_method_calls(
    stmt: &mut Stmt,
    struct_instances: &HashMap<String, String>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                desugar_expr_instance_method_calls(index, struct_instances);
            }
            desugar_expr_instance_method_calls(expr, struct_instances);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            desugar_expr_instance_method_calls(expr, struct_instances);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            desugar_expr_instance_method_calls(cond, struct_instances);
            for nested in then_branch.iter_mut() {
                desugar_sample_instance_method_calls(nested, struct_instances);
            }
            for nested in else_branch.iter_mut() {
                desugar_sample_instance_method_calls(nested, struct_instances);
            }
        }
        Stmt::For {
            start, end, body, ..
        } => {
            desugar_expr_instance_method_calls(start, struct_instances);
            desugar_expr_instance_method_calls(end, struct_instances);
            for nested in body.iter_mut() {
                desugar_sample_instance_method_calls(nested, struct_instances);
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn analyze_init_stmt(
    stmt: &Stmt,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_data_aliases: &mut HashMap<String, LocalDataAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &mut HashMap<String, usize>,
    state_data_struct_roots: &mut HashMap<String, DataStructRootInfo>,
    struct_instances: &mut HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || {
        let data_vars = merged_data_vars_for_sample(state_data, local_data_aliases);
        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl: _,
                expr,
                ..
            } => analyze_assign_init(
                target,
                decl_ty,
                generic_decl_ty,
                expr,
                known_scalars,
                local_aliases,
                local_data_aliases,
                locals,
                state_scalars,
                state_data,
                state_data_struct_roots,
                struct_instances,
                input_names,
                output_names,
                param_names,
                struct_defs,
                fn_signatures,
                options,
                errors,
            ),
            Stmt::Expr { expr, .. } => {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                let _ = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
            }
            Stmt::Return { .. } => errors.push(Diagnostic::semantic(
                "return is only allowed inside def blocks",
                0,
                0,
            )),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                validate_expr(
                    cond,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                let cond_ty = infer_expr_type_for_semantics_with_local_data(
                    cond,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "if condition", errors);

                let mut then_known = known_scalars.clone();
                let mut then_aliases = local_aliases.clone();
                let mut then_data_aliases = local_data_aliases.clone();
                let mut then_scalars = state_scalars.clone();
                let mut then_data = state_data.clone();
                let mut then_data_struct_roots = state_data_struct_roots.clone();
                let mut then_structs = struct_instances.clone();
                for nested in then_branch {
                    analyze_init_stmt(
                        nested,
                        &mut then_known,
                        &mut then_aliases,
                        &mut then_data_aliases,
                        locals,
                        &mut then_scalars,
                        &mut then_data,
                        &mut then_data_struct_roots,
                        &mut then_structs,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }

                let mut else_known = known_scalars.clone();
                let mut else_aliases = local_aliases.clone();
                let mut else_data_aliases = local_data_aliases.clone();
                let mut else_scalars = state_scalars.clone();
                let mut else_data = state_data.clone();
                let mut else_data_struct_roots = state_data_struct_roots.clone();
                let mut else_structs = struct_instances.clone();
                for nested in else_branch {
                    analyze_init_stmt(
                        nested,
                        &mut else_known,
                        &mut else_aliases,
                        &mut else_data_aliases,
                        locals,
                        &mut else_scalars,
                        &mut else_data,
                        &mut else_data_struct_roots,
                        &mut else_structs,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }

                state_scalars.extend(then_scalars);
                state_scalars.extend(else_scalars);
                for (k, v) in then_data {
                    state_data.entry(k).or_insert(v);
                }
                for (k, v) in then_data_struct_roots {
                    state_data_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in else_data {
                    state_data.entry(k).or_insert(v);
                }
                for (k, v) in else_data_struct_roots {
                    state_data_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in then_structs {
                    struct_instances.entry(k).or_insert(v);
                }
                for (k, v) in else_structs {
                    struct_instances.entry(k).or_insert(v);
                }
                known_scalars.extend(state_scalars.keys().cloned());
                local_aliases.extend(then_aliases);
                local_aliases.extend(else_aliases);
                for (k, v) in then_data_aliases {
                    local_data_aliases.entry(k).or_insert(v);
                }
                for (k, v) in else_data_aliases {
                    local_data_aliases.entry(k).or_insert(v);
                }
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                ..
            } => {
                validate_expr(
                    start,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                validate_expr(
                    end,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                let start_ty = infer_expr_type_for_semantics_with_local_data(
                    start,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_numeric_type(start_ty, "for loop start bound", errors);
                let end_ty = infer_expr_type_for_semantics_with_local_data(
                    end,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_numeric_type(end_ty, "for loop end bound", errors);
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_known = known_scalars.clone();
                let mut loop_aliases = local_aliases.clone();
                let mut loop_data_aliases = local_data_aliases.clone();
                let mut loop_scalars = state_scalars.clone();
                let mut loop_data = state_data.clone();
                let mut loop_data_struct_roots = state_data_struct_roots.clone();
                let mut loop_structs = struct_instances.clone();
                for nested in body {
                    analyze_init_stmt(
                        nested,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        &loop_locals,
                        &mut loop_scalars,
                        &mut loop_data,
                        &mut loop_data_struct_roots,
                        &mut loop_structs,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }
                state_scalars.extend(loop_scalars);
                for (k, v) in loop_data {
                    state_data.entry(k).or_insert(v);
                }
                for (k, v) in loop_data_struct_roots {
                    state_data_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in loop_structs {
                    struct_instances.entry(k).or_insert(v);
                }
                known_scalars.extend(state_scalars.keys().cloned());
                *local_aliases = loop_aliases;
                *local_data_aliases = loop_data_aliases;
            }
        }
    });
}
#[allow(clippy::too_many_arguments)]
fn analyze_assign_init(
    target: &AssignTarget,
    decl_ty: &Option<PrimitiveType>,
    generic_decl_ty: &Option<String>,
    expr: &Expr,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_data_aliases: &mut HashMap<String, LocalDataAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &mut HashMap<String, usize>,
    state_data_struct_roots: &mut HashMap<String, DataStructRootInfo>,
    struct_instances: &mut HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let data_vars = merged_data_vars_for_sample(state_data, local_data_aliases);
    match target {
        AssignTarget::Index { base, index } => {
            if state_data_struct_roots.contains_key(base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed assignment target '{base}[...]' is Data[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                    ),
                    0,
                    0,
                ));
                return;
            }
            if let Some(alias) = local_data_aliases.get(base) {
                if !alias.writable {
                    errors.push(Diagnostic::semantic(
                        format!("cannot assign to immutable Data alias '{base}'"),
                        0,
                        0,
                    ));
                    return;
                }
                if alias.elem_struct.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "indexed assignment target '{base}[...]' is Data[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                        ),
                        0,
                        0,
                    ));
                    return;
                }
            }
            if decl_ty.is_some() || generic_decl_ty.is_some() {
                errors.push(Diagnostic::semantic(
                    "typed declaration is only supported for plain scalar variables",
                    0,
                    0,
                ));
            }
            if !state_data.contains_key(base)
                && !local_data_aliases.contains_key(base)
                && !has_declared_buffer_symbol(known_scalars, base)
            {
                errors.push(Diagnostic::semantic(
                    format!("indexed assignment target '{base}[...]' is not a Data/buffer symbol"),
                    0,
                    0,
                ));
            } else if is_declared_multichannel_buffer_symbol(known_scalars, base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                    0,
                    0,
                ));
            }
            validate_expr(
                index,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    data_vars: &data_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_data_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    data_vars: &data_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_data_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );
            let idx_ty = infer_expr_type_for_semantics_with_local_data(
                index,
                state_scalars,
                None,
                local_data_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            require_numeric_type(idx_ty, "Data index expression", errors);
            let expr_ty = infer_expr_type_for_semantics_with_local_data(
                expr,
                state_scalars,
                None,
                local_data_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            let expected_ty = local_data_aliases
                .get(base)
                .map(|a| a.elem_ty)
                .or_else(|| {
                    get_declared_symbol_type(state_scalars, base, DECLARED_DATA_ELEM_TYPE_PREFIX)
                })
                .or_else(|| {
                    get_declared_symbol_type(state_scalars, base, DECLARED_BUFFER_ELEM_TYPE_PREFIX)
                })
                .unwrap_or(PrimitiveType::F32);
            require_assignable_type(expr_ty, expected_ty, "Data/buffer write", errors);
        }
        AssignTarget::Var(name) => {
            let declared_ty = if let Some(declared) = *decl_ty {
                Some(declared)
            } else if let Some(param) = generic_decl_ty {
                errors.push(Diagnostic::semantic(
                    format!(
                        "generic typed declaration for '{name}: {param}' is only supported in init blocks of specialized generic processors"
                    ),
                    0,
                    0,
                ));
                None
            } else {
                None
            };
            if locals.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to loop variable '{name}'"),
                    0,
                    0,
                ));
            }
            if is_builtin_constant_name(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to builtin constant '{name}'"),
                    0,
                    0,
                ));
            }
            if input_names.contains(name)
                || output_names.contains(name)
                || param_names.contains(name)
            {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to '{name}' in init block"),
                    0,
                    0,
                ));
            }

            if local_aliases.contains_key(name) {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                let expr_ty = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_assignable_type(
                    expr_ty,
                    *local_aliases.get(name).unwrap_or(&PrimitiveType::F32),
                    &format!("alias assignment to '{name}'"),
                    errors,
                );
                known_scalars.insert(name.clone());
                return;
            }
            if local_data_aliases.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("Data alias '{name}' must be written using '{name}[index] = value'"),
                    0,
                    0,
                ));
                return;
            }

            if let Some((base, field)) = split_field_path(name, errors) {
                analyze_struct_field_init_assign(
                    base,
                    field,
                    expr,
                    known_scalars,
                    locals,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    output_names,
                    struct_defs,
                    fn_signatures,
                    options,
                    errors,
                );
                return;
            }

            if let Expr::UserCall {
                name: struct_name,
                type_args,
                args,
                ..
            } = expr
            {
                if struct_defs.contains_key(struct_name) {
                    if !type_args.is_empty() {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "struct '{}' is not generic and cannot take type arguments",
                                struct_name
                            ),
                            0,
                            0,
                        ));
                    }
                    if declared_ty.is_some() {
                        errors.push(Diagnostic::semantic(
                            "typed declaration cannot be used with struct constructor assignment",
                            0,
                            0,
                        ));
                        return;
                    }
                    analyze_struct_ctor_init_assign(
                        name,
                        struct_name,
                        args,
                        known_scalars,
                        locals,
                        state_scalars,
                        state_data,
                        state_data_struct_roots,
                        struct_instances,
                        output_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                    return;
                }
            }

            if let Expr::DataCtor { spec, init } = expr {
                if declared_ty.is_some() {
                    errors.push(Diagnostic::semantic(
                        "typed declaration cannot be used with Data[...] constructor assignment",
                        0,
                        0,
                    ));
                    return;
                }
                let context = format!("Data constructor for symbol '{name}'");
                let size_context = format!("Data constructor size for symbol '{name}'");
                if init.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "Data constructor for symbol '{name}' does not support inline array initializers"
                        ),
                        0,
                        0,
                    ));
                }
                let Some(size_value) =
                    eval_data_size_expr(&spec.size, options, &size_context, errors)
                else {
                    return;
                };
                if state_data.contains_key(name) || state_data_struct_roots.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!("Data symbol '{name}' can only be initialized once"),
                        0,
                        0,
                    ));
                    return;
                }
                if state_scalars.contains_key(name) || struct_instances.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!("symbol '{name}' already used with a different state type"),
                        0,
                        0,
                    ));
                    return;
                }
                match &spec.elem {
                    DataElemType::Primitive(elem_ty) => {
                        state_scalars.insert(
                            declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name),
                            *elem_ty,
                        );
                        state_data.insert(name.clone(), size_value);
                    }
                    DataElemType::Struct(struct_name) => {
                        if !register_data_struct_root(
                            name,
                            struct_name,
                            size_value,
                            struct_defs,
                            &context,
                            state_scalars,
                            state_data,
                            state_data_struct_roots,
                            errors,
                        ) {
                            return;
                        }
                    }
                }
                return;
            }

            if !state_data.contains_key(name)
                && !state_data_struct_roots.contains_key(name)
                && !state_scalars.contains_key(name)
                && !struct_instances.contains_key(name)
                && !input_names.contains(name)
                && !output_names.contains(name)
                && !param_names.contains(name)
                && !local_aliases.contains_key(name)
                && !local_data_aliases.contains_key(name)
            {
                if let Expr::Index { base, index } = expr {
                    let mut is_scalar_data_base = state_data.contains_key(base);
                    let mut data_struct_elem_struct = state_data_struct_roots
                        .get(base)
                        .map(|r| r.struct_name.clone());
                    if let Some(alias) = local_data_aliases.get(base) {
                        if let Some(elem_struct) = &alias.elem_struct {
                            data_struct_elem_struct = Some(elem_struct.clone());
                        } else {
                            is_scalar_data_base = true;
                        }
                    }
                    if !is_scalar_data_base && data_struct_elem_struct.is_none() {
                        if let Some((root, field)) = split_field_path(base, errors) {
                            if let Some(struct_name) = struct_instances.get(root) {
                                if let Some(fields) = struct_defs.get(struct_name) {
                                    if let Some(field_decl) =
                                        fields.iter().find(|f| f.name == field)
                                    {
                                        if matches!(field_decl.ty, TypedFieldType::Data(_)) {
                                            if let Some(elem_struct) = &field_decl.data_elem_struct
                                            {
                                                data_struct_elem_struct = Some(elem_struct.clone());
                                            } else {
                                                is_scalar_data_base = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if is_scalar_data_base || data_struct_elem_struct.is_some() {
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: output_names,
                                data_vars: &data_vars,
                                param_structs: &HashMap::new(),
                                struct_instances,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Init,
                            },
                            errors,
                        );
                        let idx_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instances,
                            struct_defs,
                            errors,
                        );
                        require_numeric_type(idx_ty, "Data index expression", errors);
                        if is_scalar_data_base {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "local alias binding '{name} = {base}[...]' is not supported for primitive arrays; use direct indexed access"
                                ),
                                0,
                                0,
                            ));
                        } else if let Some(struct_name) = data_struct_elem_struct {
                            if !add_struct_element_alias_bindings(
                                name,
                                &struct_name,
                                struct_defs,
                                known_scalars,
                                local_aliases,
                                local_data_aliases,
                                &format!("Data alias '{name}' from '{base}[...]'"),
                                errors,
                            ) {
                                return;
                            }
                        }
                        return;
                    }
                }
            }

            if state_data.contains_key(name) || state_data_struct_roots.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign scalar expression to Data symbol '{name}'"),
                    0,
                    0,
                ));
            }
            if struct_instances.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign scalar expression to struct instance '{name}'"),
                    0,
                    0,
                ));
            }
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    data_vars: &data_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_data_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );

            let expr_ty = infer_expr_type_for_semantics_with_local_data(
                expr,
                state_scalars,
                None,
                local_data_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            let target_ty = match (declared_ty, state_scalars.get(name).copied()) {
                (Some(declared), Some(existing)) if declared != existing => {
                    errors.push(Diagnostic::semantic(format!("typed declaration for '{name}' conflicts with existing state type {:?}", existing), 0, 0));
                    existing
                }
                (Some(declared), _) => declared,
                (None, Some(existing)) => existing,
                (None, None) => expr_ty.unwrap_or(PrimitiveType::F32),
            };
            require_assignable_type(
                expr_ty,
                target_ty,
                &format!("init assignment to '{name}'"),
                errors,
            );
            state_scalars.insert(name.clone(), target_ty);
            known_scalars.insert(name.clone());
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn analyze_struct_ctor_init_assign(
    target: &str,
    struct_name: &str,
    args: &[CallArg],
    known_scalars: &mut HashSet<String>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &mut HashMap<String, usize>,
    state_data_struct_roots: &mut HashMap<String, DataStructRootInfo>,
    struct_instances: &mut HashMap<String, String>,
    outputs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    _options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if struct_instances.contains_key(target) {
        errors.push(Diagnostic::semantic(
            format!("struct instance '{target}' can only be initialized once"),
            0,
            0,
        ));
        return;
    }
    if state_scalars.contains_key(target)
        || state_data.contains_key(target)
        || state_data_struct_roots.contains_key(target)
    {
        errors.push(Diagnostic::semantic(
            format!("symbol '{target}' already used with a different state type"),
            0,
            0,
        ));
        return;
    }

    let Some(fields) = struct_defs.get(struct_name) else {
        errors.push(Diagnostic::semantic(
            format!("unknown struct '{struct_name}'"),
            0,
            0,
        ));
        return;
    };

    let scalar_param_names = fields
        .iter()
        .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
        .map(|f| f.name.clone())
        .collect::<Vec<_>>();
    let scalar_defaults = fields
        .iter()
        .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
        .map(|f| f.default.clone().or(Some(Expr::Number(0.0))))
        .collect::<Vec<_>>();

    let resolved = resolve_call_args(
        args,
        &scalar_param_names,
        &scalar_defaults,
        false,
        false,
        &format!("struct constructor '{struct_name}'"),
        errors,
    );

    let mut scalar_idx = 0usize;
    for field in fields {
        let flat = format!("{target}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                if let Some(arg) = resolved[scalar_idx] {
                    validate_expr(
                        arg,
                        ExprEnv {
                            known_scalars,
                            locals,
                            outputs,
                            data_vars: state_data,
                            param_structs: &HashMap::new(),
                            struct_instances,
                            struct_defs,
                            fn_signatures,
                            allow_data_ctor: false,
                            scope: ScopeKind::Init,
                        },
                        errors,
                    );
                    let arg_ty = infer_expr_type_for_semantics(
                        arg,
                        state_scalars,
                        None,
                        locals,
                        &HashSet::new(),
                        outputs,
                        &HashSet::new(),
                        struct_instances,
                        struct_defs,
                        errors,
                    );
                    require_assignable_type(
                        arg_ty,
                        prim,
                        &format!("struct constructor field '{flat}'"),
                        errors,
                    );
                }
                scalar_idx += 1;
                state_scalars.insert(flat.clone(), prim);
                known_scalars.insert(flat);
            }
            TypedFieldType::Data(len) => {
                if let Some(elem_struct) = &field.data_elem_struct {
                    let context =
                        format!("struct constructor field '{flat}' Data element '{elem_struct}'");
                    if !register_data_struct_root(
                        &flat,
                        elem_struct,
                        len,
                        struct_defs,
                        &context,
                        state_scalars,
                        state_data,
                        state_data_struct_roots,
                        errors,
                    ) {
                        continue;
                    }
                } else {
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        field.data_elem_ty.unwrap_or(PrimitiveType::F32),
                    );
                    state_data.entry(flat).or_insert(len);
                }
            }
        }
    }

    struct_instances.insert(target.to_owned(), struct_name.to_owned());
}

#[allow(clippy::too_many_arguments)]
fn analyze_struct_field_init_assign(
    base: &str,
    field: &str,
    expr: &Expr,
    known_scalars: &mut HashSet<String>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &mut HashMap<String, usize>,
    state_data_struct_roots: &mut HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    outputs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(struct_name) = struct_instances.get(base) else {
        errors.push(Diagnostic::semantic(
            format!("unknown struct instance '{base}'"),
            0,
            0,
        ));
        return;
    };
    let Some(fields) = struct_defs.get(struct_name) else {
        errors.push(Diagnostic::semantic(
            format!("unknown struct type '{}'", struct_name),
            0,
            0,
        ));
        return;
    };
    let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
        errors.push(Diagnostic::semantic(
            format!("struct '{}' has no field '{}'", struct_name, field),
            0,
            0,
        ));
        return;
    };

    let flat = format!("{base}.{field}");
    match field_decl.ty {
        TypedFieldType::Scalar(prim) => {
            if matches!(expr, Expr::DataCtor { .. }) {
                errors.push(Diagnostic::semantic(
                    format!("field '{flat}' is scalar and cannot be assigned Data[...]"),
                    0,
                    0,
                ));
                return;
            }
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs,
                    data_vars: state_data,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_data_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );
            let expr_ty = infer_expr_type_for_semantics(
                expr,
                state_scalars,
                None,
                locals,
                &HashSet::new(),
                outputs,
                &HashSet::new(),
                struct_instances,
                struct_defs,
                errors,
            );
            require_assignable_type(
                expr_ty,
                prim,
                &format!("struct field init '{flat}'"),
                errors,
            );
            state_scalars.insert(flat.clone(), prim);
            known_scalars.insert(flat);
        }
        TypedFieldType::Data(expected_len) => {
            let Expr::DataCtor { spec, .. } = expr else {
                errors.push(Diagnostic::semantic(
                    format!("field '{flat}' requires Data[{expected_len}] initialization"),
                    0,
                    0,
                ));
                return;
            };
            let context = format!("Data constructor for '{flat}'");
            let size_context = format!("Data constructor size for '{flat}'");
            let Some(actual_len) = eval_data_size_expr(&spec.size, options, &size_context, errors)
            else {
                return;
            };
            if actual_len != expected_len {
                errors.push(Diagnostic::semantic(
                    format!(
                        "field '{flat}' requires Data[{expected_len}] but got Data[{actual_len}]"
                    ),
                    0,
                    0,
                ));
                return;
            }
            match (&field_decl.data_elem_struct, &spec.elem) {
                (None, DataElemType::Primitive(elem_ty)) => {
                    let expected_elem_ty = field_decl.data_elem_ty.unwrap_or(PrimitiveType::F32);
                    if expected_elem_ty != *elem_ty {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "field '{flat}' expects Data[{:?}, N] but constructor uses Data[{:?}, N]",
                                expected_elem_ty, elem_ty
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        expected_elem_ty,
                    );
                    state_data.entry(flat).or_insert(expected_len);
                }
                (None, DataElemType::Struct(name)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects primitive Data but constructor uses struct element type '{name}'"
                        ),
                        0,
                        0,
                    ));
                }
                (Some(expected_struct), DataElemType::Struct(actual_struct))
                    if expected_struct == actual_struct =>
                {
                    if !register_data_struct_root(
                        &flat,
                        expected_struct,
                        expected_len,
                        struct_defs,
                        &context,
                        state_scalars,
                        state_data,
                        state_data_struct_roots,
                        errors,
                    ) {
                        return;
                    }
                }
                (Some(expected_struct), DataElemType::Struct(actual_struct)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects Data[{expected_struct}, N] but constructor uses Data[{actual_struct}, N]"
                        ),
                        0,
                        0,
                    ));
                }
                (Some(expected_struct), DataElemType::Primitive(other)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects Data[{expected_struct}, N] but constructor uses primitive element type {:?}",
                            other
                        ),
                        0,
                        0,
                    ));
                }
            }
        }
    }
}

fn merged_data_vars_for_sample(
    state_data: &HashMap<String, usize>,
    local_data_aliases: &HashMap<String, LocalDataAliasInfo>,
) -> HashMap<String, usize> {
    let mut merged = state_data.clone();
    for (name, alias) in local_data_aliases {
        if alias.elem_struct.is_none() {
            merged.insert(name.clone(), alias.len);
        }
    }
    merged
}

fn seed_top_level_array_aliases(
    aliases: &mut HashMap<String, LocalDataAliasInfo>,
    arrays: &HashMap<String, TypedArrayInfo>,
    writable: bool,
) {
    for (name, info) in arrays {
        aliases.insert(
            name.clone(),
            LocalDataAliasInfo {
                len: info.len,
                elem_ty: info.elem_ty,
                elem_struct: None,
                writable,
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn analyze_sample_stmt(
    stmt: &Stmt,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_data_aliases: &mut HashMap<String, LocalDataAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    forbidden_assign_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || {
        let data_vars = merged_data_vars_for_sample(state_data, local_data_aliases);
        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => {
                analyze_assign_sample(
                    target,
                    decl_ty,
                    generic_decl_ty,
                    *is_typed_decl,
                    expr,
                    known_scalars,
                    local_aliases,
                    local_data_aliases,
                    locals,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    forbidden_assign_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
                    options,
                    errors,
                );
            }
            Stmt::Expr { expr, .. } => {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                let _ = infer_expr_type_for_semantics(
                    expr,
                    state_scalars,
                    None,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
            }
            Stmt::Return { .. } => {
                errors.push(Diagnostic::semantic(
                    "return is only allowed inside def blocks",
                    0,
                    0,
                ));
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                validate_expr(
                    cond,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                let cond_ty = infer_expr_type_for_semantics(
                    cond,
                    state_scalars,
                    None,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "if condition", errors);
                let mut then_known = known_scalars.clone();
                let mut then_aliases = local_aliases.clone();
                let mut then_data_aliases = local_data_aliases.clone();
                for nested in then_branch {
                    analyze_sample_stmt(
                        nested,
                        &mut then_known,
                        &mut then_aliases,
                        &mut then_data_aliases,
                        locals,
                        state_scalars,
                        state_data,
                        state_data_struct_roots,
                        struct_instances,
                        input_names,
                        output_names,
                        forbidden_assign_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }
                let mut else_known = known_scalars.clone();
                let mut else_aliases = local_aliases.clone();
                let mut else_data_aliases = local_data_aliases.clone();
                for nested in else_branch {
                    analyze_sample_stmt(
                        nested,
                        &mut else_known,
                        &mut else_aliases,
                        &mut else_data_aliases,
                        locals,
                        state_scalars,
                        state_data,
                        state_data_struct_roots,
                        struct_instances,
                        input_names,
                        output_names,
                        forbidden_assign_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                ..
            } => {
                validate_expr(
                    start,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                validate_expr(
                    end,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                let start_ty = infer_expr_type_for_semantics_with_local_data(
                    start,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_numeric_type(start_ty, "for loop start bound", errors);
                let end_ty = infer_expr_type_for_semantics_with_local_data(
                    end,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_numeric_type(end_ty, "for loop end bound", errors);
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_known = known_scalars.clone();
                let mut loop_aliases = local_aliases.clone();
                let mut loop_data_aliases = local_data_aliases.clone();
                for nested in body {
                    analyze_sample_stmt(
                        nested,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        &loop_locals,
                        state_scalars,
                        state_data,
                        state_data_struct_roots,
                        struct_instances,
                        input_names,
                        output_names,
                        forbidden_assign_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn analyze_assign_sample(
    target: &AssignTarget,
    decl_ty: &Option<PrimitiveType>,
    generic_decl_ty: &Option<String>,
    is_typed_decl: bool,
    expr: &Expr,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_data_aliases: &mut HashMap<String, LocalDataAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    forbidden_assign_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let data_vars = merged_data_vars_for_sample(state_data, local_data_aliases);
    match target {
        AssignTarget::Index { base, index } => {
            if decl_ty.is_some() || generic_decl_ty.is_some() || is_typed_decl {
                errors.push(Diagnostic::semantic(
                    "typed declaration is only supported for plain scalar variables",
                    0,
                    0,
                ));
            }
            if state_data_struct_roots.contains_key(base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed assignment target '{base}[...]' is Data[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                    ),
                    0,
                    0,
                ));
                return;
            }
            if let Some(alias) = local_data_aliases.get(base) {
                if !alias.writable {
                    errors.push(Diagnostic::semantic(
                        format!("cannot assign to immutable Data alias '{base}'"),
                        0,
                        0,
                    ));
                    return;
                }
                if alias.elem_struct.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "indexed assignment target '{base}[...]' is Data[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                        ),
                        0,
                        0,
                    ));
                    return;
                }
            }
            if !state_data.contains_key(base)
                && !local_data_aliases.contains_key(base)
                && !has_declared_buffer_symbol(known_scalars, base)
            {
                errors.push(Diagnostic::semantic(
                    format!("indexed assignment target '{base}[...]' is not a Data/buffer symbol"),
                    0,
                    0,
                ));
            } else if is_declared_multichannel_buffer_symbol(known_scalars, base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                    0,
                    0,
                ));
            }
            validate_expr(
                index,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    data_vars: &data_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_data_ctor: false,
                    scope: ScopeKind::Sample,
                },
                errors,
            );
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    data_vars: &data_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_data_ctor: false,
                    scope: ScopeKind::Sample,
                },
                errors,
            );
            let index_ty = infer_expr_type_for_semantics_with_local_data(
                index,
                state_scalars,
                None,
                local_data_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            require_numeric_type(index_ty, "Data index expression", errors);
            let expr_ty = infer_expr_type_for_semantics_with_local_data(
                expr,
                state_scalars,
                None,
                local_data_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            let expected_ty = local_data_aliases
                .get(base)
                .map(|a| a.elem_ty)
                .or_else(|| {
                    get_declared_symbol_type(state_scalars, base, DECLARED_DATA_ELEM_TYPE_PREFIX)
                })
                .or_else(|| {
                    get_declared_symbol_type(state_scalars, base, DECLARED_BUFFER_ELEM_TYPE_PREFIX)
                })
                .unwrap_or(PrimitiveType::F32);
            require_assignable_type(expr_ty, expected_ty, "Data/buffer write", errors);
        }
        AssignTarget::Var(name) => {
            if let Some(param) = generic_decl_ty {
                errors.push(Diagnostic::semantic(
                    format!(
                        "generic typed declaration for '{name}: {param}' is only supported in init blocks"
                    ),
                    0,
                    0,
                ));
            }
            if locals.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to loop variable '{name}'"),
                    0,
                    0,
                ));
            }
            if is_builtin_constant_name(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to builtin constant '{name}'"),
                    0,
                    0,
                ));
            }
            if forbidden_assign_names.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to output symbol '{name}' in block"),
                    0,
                    0,
                ));
            }
            if let Expr::DataCtor { spec, init } = expr {
                if is_typed_decl {
                    if decl_ty.is_some() {
                        errors.push(Diagnostic::semantic(
                            "typed declaration cannot combine scalar type annotation with Data constructor",
                            0,
                            0,
                        ));
                        return;
                    }
                    if split_field_path(name, errors).is_some() {
                        errors.push(Diagnostic::semantic(
                            "typed array declaration target must be a plain variable name",
                            0,
                            0,
                        ));
                        return;
                    }
                    if known_scalars.contains(name)
                        || local_aliases.contains_key(name)
                        || local_data_aliases.contains_key(name)
                        || state_scalars.contains_key(name)
                        || state_data.contains_key(name)
                        || state_data_struct_roots.contains_key(name)
                        || struct_instances.contains_key(name)
                        || input_names.contains(name)
                        || output_names.contains(name)
                        || param_names.contains(name)
                    {
                        errors.push(Diagnostic::semantic(
                            format!("typed array declaration for '{name}' conflicts with existing symbol"),
                            0,
                            0,
                        ));
                        return;
                    }
                    let size_context =
                        format!("typed array declaration size for symbol '{name}' in sample");
                    let Some(size_value) =
                        eval_data_size_expr(&spec.size, options, &size_context, errors)
                    else {
                        return;
                    };
                    match &spec.elem {
                        DataElemType::Primitive(elem_ty) => {
                            local_data_aliases.insert(
                                name.clone(),
                                LocalDataAliasInfo {
                                    len: size_value,
                                    elem_ty: *elem_ty,
                                    elem_struct: None,
                                    writable: true,
                                },
                            );
                            if let Some(values) = init {
                                if values.len() != size_value {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "typed array declaration '{name}' initializer expects {size_value} elements, got {}",
                                            values.len()
                                        ),
                                        0,
                                        0,
                                    ));
                                }
                                for (idx, value) in values.iter().take(size_value).enumerate() {
                                    validate_expr(
                                        value,
                                        ExprEnv {
                                            known_scalars,
                                            locals,
                                            outputs: output_names,
                                            data_vars: &data_vars,
                                            param_structs: &HashMap::new(),
                                            struct_instances,
                                            struct_defs,
                                            fn_signatures,
                                            allow_data_ctor: false,
                                            scope: ScopeKind::Sample,
                                        },
                                        errors,
                                    );
                                    let value_ty = infer_expr_type_for_semantics_with_local_data(
                                        value,
                                        state_scalars,
                                        None,
                                        local_data_aliases,
                                        locals,
                                        input_names,
                                        output_names,
                                        param_names,
                                        struct_instances,
                                        struct_defs,
                                        errors,
                                    );
                                    require_assignable_type(
                                        value_ty,
                                        *elem_ty,
                                        &format!(
                                            "typed array initializer assignment to '{name}[{idx}]'"
                                        ),
                                        errors,
                                    );
                                }
                            }
                        }
                        DataElemType::Struct(struct_name) => {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "typed array declaration '{name}: {struct_name}[N]' is not yet supported in sample/block"
                                ),
                                0,
                                0,
                            ));
                        }
                    }
                    return;
                }
            }

            if local_aliases.contains_key(name) {
                if matches!(expr, Expr::DataCtor { .. }) {
                    errors.push(Diagnostic::semantic(
                        "Data[...] construction is only allowed in init",
                        0,
                        0,
                    ));
                }
                if let Expr::UserCall { name: ctor, .. } = expr {
                    if struct_defs.contains_key(ctor) {
                        errors.push(Diagnostic::semantic(
                            "struct construction is only allowed in init",
                            0,
                            0,
                        ));
                    }
                }
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        data_vars: &data_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                let expr_ty = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_assignable_type(
                    expr_ty,
                    *local_aliases.get(name).unwrap_or(&PrimitiveType::F32),
                    &format!("alias assignment to '{name}'"),
                    errors,
                );
                known_scalars.insert(name.clone());
                return;
            }
            if local_data_aliases.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("Data alias '{name}' must be written using '{name}[index] = value'"),
                    0,
                    0,
                ));
                return;
            }

            if let Some((base, field)) = split_field_path(name, errors) {
                let Some(struct_name) = struct_instances.get(base) else {
                    errors.push(Diagnostic::semantic(
                        format!("unknown struct instance '{base}'"),
                        0,
                        0,
                    ));
                    return;
                };
                let Some(fields) = struct_defs.get(struct_name) else {
                    return;
                };
                let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                    errors.push(Diagnostic::semantic(
                        format!("struct '{}' has no field '{}'", struct_name, field),
                        0,
                        0,
                    ));
                    return;
                };
                let flat = format!("{base}.{field}");
                match field_decl.ty {
                    TypedFieldType::Scalar(prim) => {
                        if !state_scalars.contains_key(&flat) {
                            errors.push(Diagnostic::semantic(
                                format!("struct field '{flat}' must be initialized in init"),
                                0,
                                0,
                            ));
                        }
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: output_names,
                                data_vars: &data_vars,
                                param_structs: &HashMap::new(),
                                struct_instances,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Sample,
                            },
                            errors,
                        );
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
                            expr,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instances,
                            struct_defs,
                            errors,
                        );
                        require_assignable_type(
                            expr_ty,
                            prim,
                            &format!("sample assignment to '{flat}'"),
                            errors,
                        );
                    }
                    TypedFieldType::Data(_) => {
                        errors.push(Diagnostic::semantic(
                            format!("Data field '{flat}' must be accessed with index syntax"),
                            0,
                            0,
                        ));
                    }
                }
                return;
            }

            if !output_names.contains(name)
                && !state_scalars.contains_key(name)
                && !state_data.contains_key(name)
                && !local_data_aliases.contains_key(name)
                && !state_data_struct_roots.contains_key(name)
                && !struct_instances.contains_key(name)
                && !input_names.contains(name)
                && !param_names.contains(name)
            {
                if let Expr::Index { base, index } = expr {
                    let mut is_scalar_data_base = state_data.contains_key(base);
                    let mut data_struct_elem_struct = state_data_struct_roots
                        .get(base)
                        .map(|r| r.struct_name.clone());
                    if let Some(alias) = local_data_aliases.get(base) {
                        if let Some(elem_struct) = &alias.elem_struct {
                            data_struct_elem_struct = Some(elem_struct.clone());
                        } else {
                            is_scalar_data_base = true;
                        }
                    }
                    if !is_scalar_data_base && data_struct_elem_struct.is_none() {
                        if let Some((root, field)) = split_field_path(base, errors) {
                            if let Some(struct_name) = struct_instances.get(root) {
                                if let Some(fields) = struct_defs.get(struct_name) {
                                    if let Some(field_decl) =
                                        fields.iter().find(|f| f.name == field)
                                    {
                                        if matches!(field_decl.ty, TypedFieldType::Data(_)) {
                                            if let Some(elem_struct) = &field_decl.data_elem_struct
                                            {
                                                data_struct_elem_struct = Some(elem_struct.clone());
                                            } else {
                                                is_scalar_data_base = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if is_scalar_data_base || data_struct_elem_struct.is_some() {
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: output_names,
                                data_vars: &data_vars,
                                param_structs: &HashMap::new(),
                                struct_instances,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Sample,
                            },
                            errors,
                        );
                        let idx_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instances,
                            struct_defs,
                            errors,
                        );
                        require_numeric_type(idx_ty, "Data index expression", errors);
                        if is_scalar_data_base {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "local alias binding '{name} = {base}[...]' is not supported for primitive arrays; use direct indexed access"
                                ),
                                0,
                                0,
                            ));
                        } else if let Some(struct_name) = data_struct_elem_struct {
                            if !add_struct_element_alias_bindings(
                                name,
                                &struct_name,
                                struct_defs,
                                known_scalars,
                                local_aliases,
                                local_data_aliases,
                                &format!("Data alias '{name}' from '{base}[...]'"),
                                errors,
                            ) {
                                return;
                            }
                        }
                        return;
                    }
                }
            }

            if input_names.contains(name) || param_names.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to immutable symbol '{name}' in sample block"),
                    0,
                    0,
                ));
            }
            if struct_instances.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("struct instance '{name}' cannot be assigned in sample"),
                    0,
                    0,
                ));
            }
            if matches!(expr, Expr::DataCtor { .. }) {
                errors.push(Diagnostic::semantic(
                    "Data[...] construction is only allowed in init",
                    0,
                    0,
                ));
            }
            if let Expr::UserCall { name: ctor, .. } = expr {
                if struct_defs.contains_key(ctor) {
                    errors.push(Diagnostic::semantic(
                        "struct construction is only allowed in init",
                        0,
                        0,
                    ));
                }
            }
            if state_data.contains_key(name) || state_data_struct_roots.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("Data symbol '{name}' must be written using '{name}[index] = value'"),
                    0,
                    0,
                ));
            }
            if local_data_aliases.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("Data alias '{name}' must be written using '{name}[index] = value'"),
                    0,
                    0,
                ));
            }
            if let Some(declared_ty) = *decl_ty {
                if output_names.contains(name) || local_aliases.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "typed declaration for '{name}' is only allowed on first assignment"
                        ),
                        0,
                        0,
                    ));
                } else if let Some(existing_ty) = state_scalars.get(name).copied() {
                    if existing_ty != declared_ty {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "typed declaration for '{name}' conflicts with existing state type {:?}",
                                existing_ty
                            ),
                            0,
                            0,
                        ));
                    }
                }
            }

            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    data_vars: &data_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_data_ctor: false,
                    scope: ScopeKind::Sample,
                },
                errors,
            );
            let expr_ty = infer_expr_type_for_semantics_with_local_data(
                expr,
                state_scalars,
                None,
                local_data_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            let can_track_local = !output_names.contains(name)
                && !state_scalars.contains_key(name)
                && !state_data.contains_key(name)
                && !state_data_struct_roots.contains_key(name)
                && !struct_instances.contains_key(name)
                && !input_names.contains(name)
                && !param_names.contains(name)
                && !local_data_aliases.contains_key(name)
                && !locals.contains(name)
                && !is_builtin_constant_name(name);
            let target_ty = if output_names.contains(name) {
                Some(
                    get_declared_symbol_type(state_scalars, name, DECLARED_OUTPUT_TYPE_PREFIX)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else if let Some(existing) = state_scalars.get(name).copied() {
                Some(existing)
            } else if let Some(existing) = local_aliases.get(name).copied() {
                Some(existing)
            } else if let Some(declared) = *decl_ty {
                Some(declared)
            } else {
                Some(expr_ty.unwrap_or(PrimitiveType::F32))
            };
            if let Some(target_ty) = target_ty {
                require_assignable_type(
                    expr_ty,
                    target_ty,
                    &format!("sample assignment to '{name}'"),
                    errors,
                );
                if can_track_local {
                    local_aliases.entry(name.clone()).or_insert(target_ty);
                }
            }

            if output_names.contains(name) || can_track_local {
                known_scalars.insert(name.clone());
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn analyze_def_stmt(
    stmt: &Stmt,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_data_aliases: &mut HashMap<String, LocalDataAliasInfo>,
    locals: &HashSet<String>,
    param_structs: &HashMap<String, String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || {
        let empty_data = HashMap::<String, usize>::new();
        // In def analysis, struct-typed parameters (for example `self`) should be
        // visible to expression type inference, including indexed Data field reads.
        let struct_instance_ctx = param_structs;
        let empty_outputs = HashSet::<String>::new();
        let data_vars = merged_data_vars_for_sample(&empty_data, local_data_aliases);

        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => match target {
                AssignTarget::Var(name) => {
                    if let Some(param) = generic_decl_ty {
                        errors.push(Diagnostic::semantic(
                        format!(
                            "generic typed declaration for '{name}: {param}' is only supported in init blocks"
                        ),
                        0,
                        0,
                    ));
                    }
                    let declared_ty = *decl_ty;
                    if let Expr::DataCtor { spec, init } = expr {
                        if *is_typed_decl {
                            if declared_ty.is_some() {
                                errors.push(Diagnostic::semantic(
                                    "typed declaration cannot combine scalar type annotation with Data constructor",
                                    0,
                                    0,
                                ));
                                return;
                            }
                            if split_field_path(name, errors).is_some() {
                                errors.push(Diagnostic::semantic(
                                    "typed array declaration target must be a plain variable name",
                                    0,
                                    0,
                                ));
                                return;
                            }
                            if known_scalars.contains(name)
                                || local_data_aliases.contains_key(name)
                                || input_names.contains(name)
                                || output_names.contains(name)
                                || param_names.contains(name)
                                || state_scalars.contains_key(name)
                            {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "typed array declaration for '{name}' conflicts with existing symbol"
                                    ),
                                    0,
                                    0,
                                ));
                                return;
                            }
                            let size_context =
                                format!("typed array declaration size for symbol '{name}' in def");
                            let Some(size_value) =
                                eval_data_size_expr(&spec.size, options, &size_context, errors)
                            else {
                                return;
                            };
                            match &spec.elem {
                                DataElemType::Primitive(elem_ty) => {
                                    local_data_aliases.insert(
                                        name.clone(),
                                        LocalDataAliasInfo {
                                            len: size_value,
                                            elem_ty: *elem_ty,
                                            elem_struct: None,
                                            writable: true,
                                        },
                                    );
                                    if let Some(values) = init {
                                        if values.len() != size_value {
                                            errors.push(Diagnostic::semantic(
                                            format!(
                                                "typed array declaration '{name}' initializer expects {size_value} elements, got {}",
                                                values.len()
                                            ),
                                            0,
                                            0,
                                        ));
                                        }
                                        for (idx, value) in
                                            values.iter().take(size_value).enumerate()
                                        {
                                            validate_expr(
                                                value,
                                                ExprEnv {
                                                    known_scalars,
                                                    locals,
                                                    outputs: &empty_outputs,
                                                    data_vars: &data_vars,
                                                    param_structs,
                                                    struct_instances: struct_instance_ctx,
                                                    struct_defs,
                                                    fn_signatures,
                                                    allow_data_ctor: false,
                                                    scope: ScopeKind::Def,
                                                },
                                                errors,
                                            );
                                            let value_ty =
                                                infer_expr_type_for_semantics_with_local_data(
                                                    value,
                                                    state_scalars,
                                                    None,
                                                    local_data_aliases,
                                                    locals,
                                                    input_names,
                                                    output_names,
                                                    param_names,
                                                    struct_instance_ctx,
                                                    struct_defs,
                                                    errors,
                                                );
                                            require_assignable_type(
                                            value_ty,
                                            *elem_ty,
                                            &format!(
                                                "typed array initializer assignment to '{name}[{idx}]'"
                                            ),
                                            errors,
                                        );
                                        }
                                    }
                                }
                                DataElemType::Struct(struct_name) => {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "typed array declaration '{name}: {struct_name}[N]' is not yet supported in def blocks"
                                        ),
                                        0,
                                        0,
                                    ));
                                }
                            }
                            return;
                        } else {
                            errors.push(Diagnostic::semantic(
                                "Data[...] construction is only allowed in init or typed array declarations",
                                0,
                                0,
                            ));
                            return;
                        }
                    }
                    if local_aliases.contains_key(name) {
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                data_vars: &data_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
                            expr,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            errors,
                        );
                        require_assignable_type(
                            expr_ty,
                            *local_aliases.get(name).unwrap_or(&PrimitiveType::F32),
                            &format!("alias assignment to '{name}'"),
                            errors,
                        );
                        known_scalars.insert(name.clone());
                        return;
                    }
                    if local_data_aliases.contains_key(name) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "Data alias '{name}' must be written using '{name}[index] = value'"
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    if let Some((base, field)) = split_field_path(name, errors) {
                        if declared_ty.is_some() {
                            errors.push(Diagnostic::semantic(
                                "typed declaration is only supported for plain scalar variables",
                                0,
                                0,
                            ));
                        }
                        let Some(struct_name) = param_structs.get(base) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "invalid assignment target '{name}' in def block; only struct parameters can be assigned via fields"
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(fields) = struct_defs.get(struct_name) else {
                            errors.push(Diagnostic::semantic(
                                format!("unknown struct type '{}'", struct_name),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    base, struct_name, field
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        match field_decl.ty {
                            TypedFieldType::Scalar(prim) => {
                                validate_expr(
                                    expr,
                                    ExprEnv {
                                        known_scalars,
                                        locals,
                                        outputs: &empty_outputs,
                                        data_vars: &data_vars,
                                        param_structs,
                                        struct_instances: struct_instance_ctx,
                                        struct_defs,
                                        fn_signatures,
                                        allow_data_ctor: false,
                                        scope: ScopeKind::Def,
                                    },
                                    errors,
                                );
                                let expr_ty = infer_expr_type_for_semantics_with_local_data(
                                    expr,
                                    state_scalars,
                                    None,
                                    local_data_aliases,
                                    locals,
                                    input_names,
                                    output_names,
                                    param_names,
                                    struct_instance_ctx,
                                    struct_defs,
                                    errors,
                                );
                                require_assignable_type(
                                    expr_ty,
                                    prim,
                                    &format!("def assignment to '{}.{}'", base, field),
                                    errors,
                                );
                            }
                            TypedFieldType::Data(_) => {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "Data field '{}.{}' must be assigned via index syntax",
                                        base, field
                                    ),
                                    0,
                                    0,
                                ));
                            }
                        }
                        return;
                    }
                    if !known_scalars.contains(name)
                        && !local_aliases.contains_key(name)
                        && !local_data_aliases.contains_key(name)
                        && !input_names.contains(name)
                        && !output_names.contains(name)
                        && !param_names.contains(name)
                        && !state_scalars.contains_key(name)
                    {
                        if let Expr::Index { base, index } = expr {
                            let mut is_scalar_data_base = false;
                            let mut data_struct_elem_struct: Option<String> = None;

                            if let Some(alias) = local_data_aliases.get(base) {
                                if let Some(elem_struct) = &alias.elem_struct {
                                    data_struct_elem_struct = Some(elem_struct.clone());
                                } else {
                                    is_scalar_data_base = true;
                                }
                            }

                            if !is_scalar_data_base && data_struct_elem_struct.is_none() {
                                if let Some((root, field)) = split_field_path(base, errors) {
                                    if let Some(struct_name) = param_structs.get(root) {
                                        if let Some(fields) = struct_defs.get(struct_name) {
                                            if let Some(field_decl) =
                                                fields.iter().find(|f| f.name == field)
                                            {
                                                if matches!(field_decl.ty, TypedFieldType::Data(_))
                                                {
                                                    if let Some(elem_struct) =
                                                        &field_decl.data_elem_struct
                                                    {
                                                        data_struct_elem_struct =
                                                            Some(elem_struct.clone());
                                                    } else {
                                                        is_scalar_data_base = true;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if is_scalar_data_base || data_struct_elem_struct.is_some() {
                                validate_expr(
                                    index,
                                    ExprEnv {
                                        known_scalars,
                                        locals,
                                        outputs: &empty_outputs,
                                        data_vars: &data_vars,
                                        param_structs,
                                        struct_instances: struct_instance_ctx,
                                        struct_defs,
                                        fn_signatures,
                                        allow_data_ctor: false,
                                        scope: ScopeKind::Def,
                                    },
                                    errors,
                                );
                                let idx_ty = infer_expr_type_for_semantics_with_local_data(
                                    index,
                                    state_scalars,
                                    None,
                                    local_data_aliases,
                                    locals,
                                    input_names,
                                    output_names,
                                    param_names,
                                    struct_instance_ctx,
                                    struct_defs,
                                    errors,
                                );
                                require_numeric_type(idx_ty, "Data index expression", errors);
                                if is_scalar_data_base {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "local alias binding '{name} = {base}[...]' is not supported for primitive arrays; use direct indexed access"
                                        ),
                                        0,
                                        0,
                                    ));
                                } else if let Some(struct_name) = data_struct_elem_struct {
                                    if !add_struct_element_alias_bindings(
                                        name,
                                        &struct_name,
                                        struct_defs,
                                        known_scalars,
                                        local_aliases,
                                        local_data_aliases,
                                        &format!("Data alias '{name}' from '{base}[...]'"),
                                        errors,
                                    ) {
                                        return;
                                    }
                                }
                                return;
                            }
                        }
                    }
                    if known_scalars.contains(name) && declared_ty.is_some() {
                        errors.push(Diagnostic::semantic(
                            format!(
                            "typed declaration for '{name}' is only allowed on first assignment"
                        ),
                            0,
                            0,
                        ));
                    }
                    if local_data_aliases.contains_key(name) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "Data alias '{name}' must be written using '{name}[index] = value'"
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    if is_builtin_constant_name(name) {
                        errors.push(Diagnostic::semantic(
                            format!("cannot assign to builtin constant '{name}'"),
                            0,
                            0,
                        ));
                    }
                    if locals.contains(name) {
                        errors.push(Diagnostic::semantic(
                            format!("cannot assign to loop variable '{name}'"),
                            0,
                            0,
                        ));
                    }
                    if input_names.contains(name)
                        || output_names.contains(name)
                        || param_names.contains(name)
                        || state_scalars.contains_key(name)
                    {
                        errors.push(Diagnostic::semantic(
                            format!("cannot assign to global symbol '{name}' inside def"),
                            0,
                            0,
                        ));
                    }
                    validate_expr(
                        expr,
                        ExprEnv {
                            known_scalars,
                            locals,
                            outputs: &empty_outputs,
                            data_vars: &data_vars,
                            param_structs,
                            struct_instances: struct_instance_ctx,
                            struct_defs,
                            fn_signatures,
                            allow_data_ctor: false,
                            scope: ScopeKind::Def,
                        },
                        errors,
                    );
                    let expr_ty = infer_expr_type_for_semantics_with_local_data(
                        expr,
                        state_scalars,
                        None,
                        local_data_aliases,
                        locals,
                        input_names,
                        output_names,
                        param_names,
                        struct_instance_ctx,
                        struct_defs,
                        errors,
                    );
                    let can_track_local = !input_names.contains(name)
                        && !output_names.contains(name)
                        && !param_names.contains(name)
                        && !state_scalars.contains_key(name);
                    let target_ty = if let Some(declared) = declared_ty {
                        declared
                    } else if let Some(existing) = local_aliases.get(name).copied() {
                        existing
                    } else {
                        expr_ty.unwrap_or(PrimitiveType::F32)
                    };
                    require_assignable_type(
                        expr_ty,
                        target_ty,
                        &format!("def assignment to '{name}'"),
                        errors,
                    );
                    if can_track_local {
                        local_aliases.entry(name.clone()).or_insert(target_ty);
                    }
                    known_scalars.insert(name.clone());
                }
                AssignTarget::Index { base, index } => {
                    if decl_ty.is_some() || generic_decl_ty.is_some() || *is_typed_decl {
                        errors.push(Diagnostic::semantic(
                            "typed declaration is only supported for plain scalar variables",
                            0,
                            0,
                        ));
                    }
                    if let Some(alias) = local_data_aliases.get(base) {
                        if !alias.writable {
                            errors.push(Diagnostic::semantic(
                                format!("cannot assign to immutable Data alias '{base}'"),
                                0,
                                0,
                            ));
                            return;
                        }
                        if alias.elem_struct.is_some() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "indexed assignment target '{base}[...]' has struct elements; assign fields through an alias"
                                ),
                                0,
                                0,
                            ));
                            return;
                        }
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                data_vars: &data_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                data_vars: &data_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            errors,
                        );
                        require_numeric_type(index_ty, "Data index expression", errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
                            expr,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            errors,
                        );
                        require_assignable_type(
                            expr_ty,
                            alias.elem_ty,
                            "Data/buffer write",
                            errors,
                        );
                        return;
                    }
                    if has_declared_buffer_symbol(known_scalars, base) {
                        if is_declared_multichannel_buffer_symbol(known_scalars, base) {
                            errors.push(Diagnostic::semantic(
                            format!(
                                "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                            ),
                            0,
                            0,
                        ));
                        }
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                data_vars: &data_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                data_vars: &data_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            errors,
                        );
                        require_numeric_type(index_ty, "Data index expression", errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
                            expr,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            errors,
                        );
                        let expected_ty = get_declared_symbol_type(
                            state_scalars,
                            base,
                            DECLARED_BUFFER_ELEM_TYPE_PREFIX,
                        )
                        .unwrap_or(PrimitiveType::F32);
                        require_assignable_type(expr_ty, expected_ty, "Data/buffer write", errors);
                        return;
                    }
                    if let Some((root, field)) = split_field_path(base, errors) {
                        let Some(struct_name) = param_structs.get(root) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                "indexed assignment target '{base}[...]' is invalid in def block"
                            ),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(fields) = struct_defs.get(struct_name) else {
                            errors.push(Diagnostic::semantic(
                                format!("unknown struct type '{}'", struct_name),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    root, struct_name, field
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        if !matches!(field_decl.ty, TypedFieldType::Data(_)) {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "field '{}.{}' is not Data and cannot be indexed",
                                    root, field
                                ),
                                0,
                                0,
                            ));
                        }
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                data_vars: &data_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                data_vars: &data_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_data_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            errors,
                        );
                        require_numeric_type(index_ty, "Data index expression", errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
                            expr,
                            state_scalars,
                            None,
                            local_data_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            errors,
                        );
                        let expected_elem_ty =
                            field_decl.data_elem_ty.unwrap_or(PrimitiveType::F32);
                        require_assignable_type(
                            expr_ty,
                            expected_elem_ty,
                            "Data/buffer write",
                            errors,
                        );
                        return;
                    }
                    errors.push(Diagnostic::semantic(
                        "indexed assignments in def are only allowed for local typed arrays or Data fields on struct parameters (for example 'tmp[i] = x' or 'self.buf[i] = x')",
                        0,
                        0,
                    ));
                }
            },
            Stmt::Expr { expr, .. } => {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        data_vars: &data_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                let _ = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
            }
            Stmt::Return { expr, .. } => {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        data_vars: &data_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                let _ = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                validate_expr(
                    cond,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        data_vars: &data_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                let cond_ty = infer_expr_type_for_semantics_with_local_data(
                    cond,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "if condition", errors);
                let mut then_known = known_scalars.clone();
                let mut then_aliases = local_aliases.clone();
                let mut then_data_aliases = local_data_aliases.clone();
                for nested in then_branch {
                    analyze_def_stmt(
                        nested,
                        &mut then_known,
                        &mut then_aliases,
                        &mut then_data_aliases,
                        locals,
                        param_structs,
                        state_scalars,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }
                let mut else_known = known_scalars.clone();
                let mut else_aliases = local_aliases.clone();
                let mut else_data_aliases = local_data_aliases.clone();
                for nested in else_branch {
                    analyze_def_stmt(
                        nested,
                        &mut else_known,
                        &mut else_aliases,
                        &mut else_data_aliases,
                        locals,
                        param_structs,
                        state_scalars,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }
                let mut merged = known_scalars.clone();
                for name in &then_known {
                    if else_known.contains(name) {
                        merged.insert(name.clone());
                    }
                }
                *known_scalars = merged;
                *local_aliases = then_aliases;
                local_aliases.extend(else_aliases);
                local_aliases.retain(|name, _| known_scalars.contains(name));
                *local_data_aliases = then_data_aliases;
                for (k, v) in else_data_aliases {
                    local_data_aliases.entry(k).or_insert(v);
                }
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                ..
            } => {
                validate_expr(
                    start,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        data_vars: &data_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                validate_expr(
                    end,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        data_vars: &data_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_data_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                let start_ty = infer_expr_type_for_semantics_with_local_data(
                    start,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
                require_numeric_type(start_ty, "for loop start bound", errors);
                let end_ty = infer_expr_type_for_semantics_with_local_data(
                    end,
                    state_scalars,
                    None,
                    local_data_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
                require_numeric_type(end_ty, "for loop end bound", errors);
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_known = known_scalars.clone();
                let mut loop_aliases = local_aliases.clone();
                let mut loop_data_aliases = local_data_aliases.clone();
                for nested in body {
                    analyze_def_stmt(
                        nested,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        &loop_locals,
                        param_structs,
                        state_scalars,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                }
                loop_aliases.retain(|name, _| known_scalars.contains(name));
                *local_aliases = loop_aliases;
                *local_data_aliases = loop_data_aliases;
            }
        }
    });
}
fn validate_expr(expr: &Expr, env: ExprEnv<'_>, errors: &mut Vec<Diagnostic>) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                validate_expr(value, env, errors);
            }
            errors.push(Diagnostic::semantic(
                "array literals are only allowed in typed array declarations and parameter defaults",
                0,
                0,
            ));
        }
        Expr::Var(name) => {
            if is_builtin_constant_name(name) {
                return;
            }
            if let Some((base, field)) = split_field_path(name, errors) {
                if let Some(struct_name) = env.param_structs.get(base) {
                    let Some(fields) = env.struct_defs.get(struct_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown struct type '{}'", struct_name),
                            0,
                            0,
                        ));
                        return;
                    };
                    let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "struct parameter '{}' (type '{}') has no field '{}'",
                                base, struct_name, field
                            ),
                            0,
                            0,
                        ));
                        return;
                    };
                    if matches!(field_decl.ty, TypedFieldType::Data(_)) {
                        errors.push(Diagnostic::semantic(
                            format!("Data field '{}.{}' must be indexed", base, field),
                            0,
                            0,
                        ));
                    }
                    return;
                }

                let flat = format!("{base}.{field}");
                if env.data_vars.contains_key(&flat) {
                    errors.push(Diagnostic::semantic(
                        format!("Data symbol '{flat}' must be indexed"),
                        0,
                        0,
                    ));
                    return;
                }
                if !env.known_scalars.contains(&flat)
                    && !env.locals.contains(&flat)
                    && !env.outputs.contains(&flat)
                {
                    errors.push(Diagnostic::semantic(
                        format!("unknown symbol '{flat}' in expression"),
                        0,
                        0,
                    ));
                }
                return;
            }

            if env.param_structs.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("struct parameter '{}' must be accessed via fields", name),
                    0,
                    0,
                ));
                return;
            }
            if env.struct_instances.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("struct instance '{name}' must be accessed via fields"),
                    0,
                    0,
                ));
                return;
            }
            if env.data_vars.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("Data symbol '{name}' must be indexed"),
                    0,
                    0,
                ));
                return;
            }
            if has_declared_buffer_symbol(env.known_scalars, name) {
                errors.push(Diagnostic::semantic(
                    format!("buffer symbol '{name}' must be indexed"),
                    0,
                    0,
                ));
                return;
            }
            if !env.known_scalars.contains(name)
                && !env.locals.contains(name)
                && !env.outputs.contains(name)
            {
                errors.push(Diagnostic::semantic(
                    format!("unknown symbol '{name}' in expression"),
                    0,
                    0,
                ));
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_field_path(base, errors) {
                if let Some(struct_name) = env.param_structs.get(root) {
                    let Some(fields) = env.struct_defs.get(struct_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown struct type '{}'", struct_name),
                            0,
                            0,
                        ));
                        return;
                    };
                    let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "struct parameter '{}' (type '{}') has no field '{}'",
                                root, struct_name, field
                            ),
                            0,
                            0,
                        ));
                        return;
                    };
                    if !matches!(field_decl.ty, TypedFieldType::Data(_)) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "field '{}.{}' is not Data and cannot be indexed",
                                root, field
                            ),
                            0,
                            0,
                        ));
                    }
                    validate_expr(index, env, errors);
                    return;
                }
            }
            if !env.data_vars.contains_key(base)
                && !has_declared_buffer_symbol(env.known_scalars, base)
            {
                errors.push(Diagnostic::semantic(
                    format!("indexed expression '{base}[...]' is not a Data/buffer symbol"),
                    0,
                    0,
                ));
            } else if is_declared_multichannel_buffer_symbol(env.known_scalars, base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed expression '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                    0,
                    0,
                ));
            }
            validate_expr(index, env, errors);
        }
        Expr::DataCtor { init, .. } => {
            if !env.allow_data_ctor {
                errors.push(Diagnostic::semantic(
                    "Data[...] constructor is only allowed in init assignments",
                    0,
                    0,
                ));
            }
            if let Some(values) = init {
                for value in values {
                    validate_expr(value, env, errors);
                }
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
            validate_expr(expr, env, errors);
        }
        Expr::Logical { lhs, rhs, .. } => {
            validate_expr(lhs, env, errors);
            validate_expr(rhs, env, errors);
        }
        Expr::Compare { lhs, rhs, .. } => {
            validate_expr(lhs, env, errors);
            validate_expr(rhs, env, errors);
        }
        Expr::Call { func, args } => {
            for arg in args {
                validate_expr(arg, env, errors);
            }
            let expected = builtin_arity(*func);
            if args.len() != expected {
                errors.push(Diagnostic::semantic(
                    format!(
                        "builtin '{}' expects {expected} positional arguments, got {}",
                        builtin_name(*func),
                        args.len()
                    ),
                    0,
                    0,
                ));
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            if is_builtin_unsafe_data_fn(name) {
                validate_unsafe_data_builtin_call(name, args, env, errors);
                return;
            }
            if is_internal_buffer_2d_fn(name) {
                validate_internal_buffer_2d_call(name, args, env, errors);
                return;
            }
            if let Some(base) = parse_data_len_instance_base(name) {
                validate_data_len_builtin_call(name, base, args, env, errors);
                return;
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                validate_buffer_chans_builtin_call(name, base, args, env, errors);
                return;
            }
            if let Some(sig) = env.fn_signatures.get(name) {
                if sig.type_params.is_empty() {
                    if !type_args.is_empty() {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "function '{}' is not generic and cannot take type arguments",
                                name
                            ),
                            0,
                            0,
                        ));
                    }
                } else if !type_args.is_empty() && type_args.len() != sig.type_params.len() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' expects {} type arguments, got {}",
                            name,
                            sig.type_params.len(),
                            type_args.len()
                        ),
                        0,
                        0,
                    ));
                }

                let forbid_self_named = sig.params.first().map(String::as_str) == Some("self");
                let resolved = resolve_call_args(
                    args,
                    &sig.params,
                    &sig.defaults,
                    forbid_self_named,
                    false,
                    &format!("function '{name}' call"),
                    errors,
                );
                for (idx, arg) in resolved.into_iter().enumerate() {
                    if let Some(arg) = arg {
                        let param_ty = sig.param_types.get(idx).and_then(|t| t.as_ref());
                        if let Some(FnParamType::Buffer(buffer_ty)) = param_ty {
                            validate_buffer_param_call_arg(
                                name,
                                idx,
                                &sig.params,
                                buffer_ty,
                                arg,
                                env,
                                errors,
                            );
                            continue;
                        }
                        if param_ty.is_none() {
                            if let Expr::Var(v) = arg {
                                if has_declared_buffer_symbol(env.known_scalars, v) {
                                    continue;
                                }
                            }
                        }
                        if let Expr::Var(v) = arg {
                            if env.struct_instances.contains_key(v)
                                || env.param_structs.contains_key(v)
                            {
                                continue;
                            }
                        }
                        validate_expr(arg, env, errors);
                    } else if let Some(default) = sig.defaults.get(idx).and_then(|d| d.as_ref()) {
                        validate_default_expr(
                            default,
                            errors,
                            &format!("function '{name}' default '{}'", sig.params[idx]),
                        );
                    }
                }
                return;
            }

            if env.struct_defs.contains_key(name) {
                let scope_name = match env.scope {
                    ScopeKind::Init => "init",
                    ScopeKind::Sample => "sample",
                    ScopeKind::Def => "def",
                };
                errors.push(Diagnostic::semantic(
                    format!(
                        "struct constructors are only allowed as direct init assignments; found '{}' call in {scope_name}",
                        name
                    ),
                    0,
                    0,
                ));
                for arg in args {
                    validate_expr(&arg.expr, env, errors);
                }
                return;
            }

            errors.push(Diagnostic::semantic(
                format!("unknown function '{name}' in expression"),
                0,
                0,
            ));
        }
        Expr::Binary { lhs, rhs, .. } => {
            validate_expr(lhs, env, errors);
            validate_expr(rhs, env, errors);
        }
    }
}

fn validate_data_len_builtin_call(
    name: &str,
    base: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if !args.is_empty() {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin method '{}' expects 0 arguments, got {}",
                name,
                args.len()
            ),
            0,
            0,
        ));
    }
    for arg in args {
        if arg.name.is_some() {
            errors.push(Diagnostic::semantic(
                format!("builtin method '{}' does not support named arguments", name),
                0,
                0,
            ));
        }
    }

    let before = errors.len();
    let is_data_symbol = if env.data_vars.contains_key(base) {
        true
    } else if has_declared_buffer_symbol(env.known_scalars, base) {
        true
    } else if let Some((root, field)) = split_field_path(base, errors) {
        let struct_name = env
            .param_structs
            .get(root)
            .or_else(|| env.struct_instances.get(root));
        if let Some(struct_name) = struct_name {
            if let Some(fields) = env.struct_defs.get(struct_name) {
                if let Some(field_decl) = fields.iter().find(|f| f.name == field) {
                    match field_decl.ty {
                        TypedFieldType::Data(_) => true,
                        TypedFieldType::Scalar(_) => {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "builtin method '{}' requires a Data symbol, but '{}.{}' is scalar",
                                    name, root, field
                                ),
                                0,
                                0,
                            ));
                            false
                        }
                    }
                } else {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "struct instance '{}' (type '{}') has no field '{}'",
                            root, struct_name, field
                        ),
                        0,
                        0,
                    ));
                    false
                }
            } else {
                errors.push(Diagnostic::semantic(
                    format!("unknown struct type '{}'", struct_name),
                    0,
                    0,
                ));
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if !is_data_symbol && errors.len() == before {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin method '{}' requires a Data or buffer symbol receiver, got '{}'",
                name, base
            ),
            0,
            0,
        ));
    }
}

fn validate_buffer_chans_builtin_call(
    name: &str,
    base: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if !args.is_empty() {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin method '{}' expects 0 arguments, got {}",
                name,
                args.len()
            ),
            0,
            0,
        ));
    }
    for arg in args {
        if arg.name.is_some() {
            errors.push(Diagnostic::semantic(
                format!("builtin method '{}' does not support named arguments", name),
                0,
                0,
            ));
        }
    }
    if !has_declared_buffer_symbol(env.known_scalars, base) {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin method '{}' requires a buffer symbol receiver, got '{}'",
                name, base
            ),
            0,
            0,
        ));
    }
}

fn validate_buffer_param_call_arg(
    fn_name: &str,
    param_idx: usize,
    param_names: &[String],
    expected: &BufferType,
    arg: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let context = if let Some(param_name) = param_names.get(param_idx) {
        format!("function '{fn_name}' parameter '{param_name}'")
    } else {
        format!("function '{fn_name}' parameter #{param_idx}")
    };
    let Expr::Var(symbol) = arg else {
        errors.push(Diagnostic::semantic(
            format!("{context} expects a buffer symbol argument"),
            0,
            0,
        ));
        validate_expr(arg, env, errors);
        return;
    };
    if !has_declared_buffer_symbol(env.known_scalars, symbol) {
        errors.push(Diagnostic::semantic(
            format!("{context} expects a buffer symbol argument, got '{symbol}'"),
            0,
            0,
        ));
        return;
    }
    let expected_elem = match expected.elem {
        BufferElemType::Primitive(ty) => ty,
        BufferElemType::Generic(ref param_ty) => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context} uses unresolved generic buffer element type '{}'",
                    param_ty
                ),
                0,
                0,
            ));
            PrimitiveType::F32
        }
    };
    if !has_declared_buffer_elem_type(env.known_scalars, symbol, expected_elem) {
        errors.push(Diagnostic::semantic(
            format!(
                "{context} expects element type {:?}, but buffer '{}' has a different element type",
                expected_elem, symbol
            ),
            0,
            0,
        ));
    }
    match &expected.channels {
        BufferChannels::Mono => {
            if is_declared_multichannel_buffer_symbol(env.known_scalars, symbol) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context} expects mono buffer, but '{}' is multichannel",
                        symbol
                    ),
                    0,
                    0,
                ));
            }
        }
        BufferChannels::Static(expr) => {
            let requested_channels = const_positive_usize(expr);
            if let Some(channels) = requested_channels {
                if channels <= 1 {
                    if is_declared_multichannel_buffer_symbol(env.known_scalars, symbol) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{context} expects mono/static-1 buffer, but '{}' is multichannel",
                                symbol
                            ),
                            0,
                            0,
                        ));
                    }
                    return;
                }
            }
            if !is_declared_multichannel_buffer_symbol(env.known_scalars, symbol) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context} expects multichannel buffer, but '{}' is mono",
                        symbol
                    ),
                    0,
                    0,
                ));
                return;
            }
            if let Some(channels) = requested_channels {
                if has_declared_dynamic_buffer_channels(env.known_scalars, symbol) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{context} expects static {channels} channels, but '{}' is dynamic",
                            symbol
                        ),
                        0,
                        0,
                    ));
                    return;
                }
                if let Some(actual) = declared_static_buffer_channels(env.known_scalars, symbol) {
                    if actual != channels {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{context} expects {channels} channels, but '{}' has {actual}",
                                symbol
                            ),
                            0,
                            0,
                        ));
                    }
                }
            }
        }
        BufferChannels::Dynamic => {
            if !is_declared_multichannel_buffer_symbol(env.known_scalars, symbol) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context} expects multichannel dynamic buffer, but '{}' is mono",
                        symbol
                    ),
                    0,
                    0,
                ));
            }
        }
    }
}

fn const_positive_usize(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Int(v) if *v > 0 => usize::try_from(*v).ok(),
        Expr::Number(v) if *v > 0.0 && v.fract() == 0.0 => usize::try_from(*v as i64).ok(),
        _ => None,
    }
}

fn validate_internal_buffer_2d_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expected_arity = if name == INTERNAL_BUFFER_READ2_FN {
        3
    } else {
        4
    };
    if args.len() != expected_arity {
        errors.push(Diagnostic::semantic(
            format!(
                "internal builtin '{}' expects {} positional arguments, got {}",
                name,
                expected_arity,
                args.len()
            ),
            0,
            0,
        ));
    }
    for arg in args {
        if arg.name.is_some() {
            errors.push(Diagnostic::semantic(
                format!(
                    "internal builtin '{}' does not support named arguments",
                    name
                ),
                0,
                0,
            ));
        }
    }
    if let Some(first) = args.first() {
        match &first.expr {
            Expr::Var(base) => {
                if !has_declared_buffer_symbol(env.known_scalars, base) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "internal builtin '{}' first argument must be a declared buffer symbol, got '{}'",
                            name, base
                        ),
                        0,
                        0,
                    ));
                } else if !is_declared_multichannel_buffer_symbol(env.known_scalars, base) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "internal builtin '{}' requires multichannel buffer indexing form, but '{}' is mono",
                            name, base
                        ),
                        0,
                        0,
                    ));
                }
            }
            other => {
                validate_expr(other, env, errors);
                errors.push(Diagnostic::semantic(
                    format!(
                        "internal builtin '{}' first argument must be a declared buffer symbol variable",
                        name
                    ),
                    0,
                    0,
                ));
            }
        }
    }
    if let Some(ch_arg) = args.get(1) {
        validate_expr(&ch_arg.expr, env, errors);
    }
    if let Some(sample_arg) = args.get(2) {
        validate_expr(&sample_arg.expr, env, errors);
    }
    if name == INTERNAL_BUFFER_WRITE2_FN {
        if let Some(value_arg) = args.get(3) {
            validate_expr(&value_arg.expr, env, errors);
        }
    }
}

fn validate_unsafe_data_builtin_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expected_arity = if name == "unsafe_read" { 2 } else { 3 };
    if args.len() != expected_arity {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin '{}' expects {} positional arguments, got {}",
                name,
                expected_arity,
                args.len()
            ),
            0,
            0,
        ));
    }

    for arg in args {
        if arg.name.is_some() {
            errors.push(Diagnostic::semantic(
                format!("builtin '{}' does not support named arguments", name),
                0,
                0,
            ));
        }
    }

    if let Some(first_arg) = args.first() {
        match &first_arg.expr {
            Expr::Var(base) => {
                let mut is_valid_primitive_data = false;

                if let Some((root, field)) = split_field_path(base, errors) {
                    if let Some(struct_name) = env.param_structs.get(root) {
                        let Some(fields) = env.struct_defs.get(struct_name) else {
                            errors.push(Diagnostic::semantic(
                                format!("unknown struct type '{}'", struct_name),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    root, struct_name, field
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        match field_decl.ty {
                            TypedFieldType::Data(_) => {
                                if field_decl.data_elem_struct.is_some() {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "builtin '{}' does not support Data[Struct, N] symbol '{}'",
                                            name, base
                                        ),
                                        0,
                                        0,
                                    ));
                                } else {
                                    is_valid_primitive_data = true;
                                }
                            }
                            TypedFieldType::Scalar(_) => {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "builtin '{}' expects a Data symbol as first argument, but '{}.{}' is scalar",
                                        name, root, field
                                    ),
                                    0,
                                    0,
                                ));
                            }
                        }
                    } else if env.data_vars.contains_key(base) {
                        is_valid_primitive_data = true;
                    }
                } else if env.data_vars.contains_key(base)
                    || has_declared_buffer_symbol(env.known_scalars, base)
                {
                    is_valid_primitive_data = true;
                }

                if !is_valid_primitive_data {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "builtin '{}' expects a primitive Data or buffer symbol as first argument, got '{}'",
                            name, base
                        ),
                        0,
                        0,
                    ));
                }
            }
            other => {
                validate_expr(other, env, errors);
                errors.push(Diagnostic::semantic(
                    format!(
                        "builtin '{}' first argument must be a Data symbol variable",
                        name
                    ),
                    0,
                    0,
                ));
            }
        }
    }

    if let Some(index_arg) = args.get(1) {
        validate_expr(&index_arg.expr, env, errors);
    }
    if name == "unsafe_write" {
        if let Some(value_arg) = args.get(2) {
            validate_expr(&value_arg.expr, env, errors);
        }
    }
}

fn coerce_struct_fields(
    struct_name: &str,
    fields: &[StructField],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedStructField> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for field in fields {
        if !seen.insert(field.name.clone()) {
            errors.push(Diagnostic::semantic(
                format!(
                    "duplicate field '{}' in struct '{}'",
                    field.name, struct_name
                ),
                0,
                0,
            ));
            continue;
        }
        let (ty, default, data_elem_ty, data_elem_struct) = match &field.ty {
            FieldType::Scalar(prim) => {
                if let Some(expr) = &field.default {
                    validate_default_expr(
                        expr,
                        errors,
                        &format!("struct field '{}.{}'", struct_name, field.name),
                    );
                }
                (
                    TypedFieldType::Scalar(*prim),
                    field.default.clone(),
                    None,
                    None,
                )
            }
            FieldType::Generic(param) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "field '{}.{}' uses unresolved generic type '{}'",
                        struct_name, field.name, param
                    ),
                    0,
                    0,
                ));
                (
                    TypedFieldType::Scalar(PrimitiveType::F32),
                    field.default.clone(),
                    None,
                    None,
                )
            }
            FieldType::Data(spec) => {
                let size_context = format!("field '{}.{}' Data size", struct_name, field.name);
                let size =
                    eval_data_size_expr(&spec.size, options, &size_context, errors).unwrap_or(1);
                if field.default.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "Data field '{}.{}' cannot have a default expression",
                            struct_name, field.name
                        ),
                        0,
                        0,
                    ));
                }
                let (elem_ty, elem_struct) = match &spec.elem {
                    DataElemType::Primitive(prim) => (Some(*prim), None),
                    DataElemType::Struct(name) => (None, Some(name.clone())),
                };
                (TypedFieldType::Data(size), None, elem_ty, elem_struct)
            }
        };
        out.push(TypedStructField {
            name: field.name.clone(),
            ty,
            default,
            data_elem_ty,
            data_elem_struct,
        });
    }
    out
}
fn split_field_path<'a>(name: &'a str, errors: &mut Vec<Diagnostic>) -> Option<(&'a str, &'a str)> {
    let mut parts = name.split('.');
    let first = parts.next()?;
    let second = parts.next();
    let third = parts.next();
    match (second, third) {
        (None, None) => None,
        (Some(f), None) => Some((first, f)),
        _ => {
            errors.push(Diagnostic::semantic(
                format!("unsupported nested path '{name}'"),
                0,
                0,
            ));
            None
        }
    }
}

fn coerce_params(
    params: &[ParamDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<TypedParam>, HashMap<String, TypedArrayInfo>) {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut arrays = HashMap::new();
    for param in params {
        if is_builtin_constant_name(&param.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "param name '{}' is reserved as a builtin constant",
                    param.name
                ),
                0,
                0,
            ));
            continue;
        }
        if !seen.insert(param.name.as_str()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate param '{}'", param.name),
                0,
                0,
            ));
            continue;
        }
        match param.ty.as_ref() {
            None | Some(DeclType::Scalar(_)) => {
                let ty = match param.ty.as_ref() {
                    Some(DeclType::Scalar(ty)) => *ty,
                    _ => PrimitiveType::F32,
                };
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        ty,
                        options,
                        &format!("param '{}.{}' default", "<top-level>", param.name),
                        is_float_type(ty),
                        matches!(ty, PrimitiveType::I32 | PrimitiveType::I64),
                        errors,
                    )
                    .unwrap_or_else(|| coerce_const_default_to_typed(0.0, ty)),
                    None => coerce_const_default_to_typed(0.0, ty),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        ty,
                        options,
                        &format!("param '{}.{}'", "<top-level>", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                out.push(TypedParam {
                    name: param.name.clone(),
                    ty,
                    default,
                    range,
                });
            }
            Some(DeclType::Generic(param_ty)) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "param '{}.{}' uses unresolved generic type '{}'",
                        "<top-level>", param.name, param_ty
                    ),
                    0,
                    0,
                ));
                let ty = PrimitiveType::F32;
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        ty,
                        options,
                        &format!("param '{}.{}' default", "<top-level>", param.name),
                        true,
                        false,
                        errors,
                    )
                    .unwrap_or(TypedConstValue::F32(0.0)),
                    None => TypedConstValue::F32(0.0),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        ty,
                        options,
                        &format!("param '{}.{}'", "<top-level>", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                out.push(TypedParam {
                    name: param.name.clone(),
                    ty,
                    default,
                    range,
                });
            }
            Some(DeclType::ArrayGeneric { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "param '{}.{}' range is not supported for array declarations",
                            "<top-level>", param.name
                        ),
                        0,
                        0,
                    ));
                }
                errors.push(Diagnostic::semantic(
                    format!(
                        "param '{}.{}' uses unresolved generic array element type '{}'",
                        "<top-level>", param.name, elem
                    ),
                    0,
                    0,
                ));
                let size_context = format!("param '{}.{}' array size", "<top-level>", param.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                arrays.insert(
                    param.name.clone(),
                    TypedArrayInfo {
                        elem_ty: PrimitiveType::F32,
                        len,
                        offset: out.len(),
                    },
                );

                let defaults = match &param.default {
                    None => vec![coerce_const_default_to_typed(0.0, PrimitiveType::F32); len],
                    Some(Expr::ArrayLiteral(values)) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "param '{}.{}' default expects {len} elements, got {}",
                                    "<top-level>",
                                    param.name,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
                        }
                        let mut defaults = Vec::with_capacity(len);
                        for idx in 0..len {
                            let value = values
                                .get(idx)
                                .and_then(|expr| {
                                    eval_typed_const_expr(
                                        expr,
                                        PrimitiveType::F32,
                                        options,
                                        &format!(
                                            "param '{}.{}' default element [{idx}]",
                                            "<top-level>", param.name
                                        ),
                                        true,
                                        false,
                                        errors,
                                    )
                                })
                                .unwrap_or(TypedConstValue::F32(0.0));
                            defaults.push(value);
                        }
                        defaults
                    }
                    Some(expr) => {
                        let value = eval_typed_const_expr(
                            expr,
                            PrimitiveType::F32,
                            options,
                            &format!("param '{}.{}' default", "<top-level>", param.name),
                            true,
                            false,
                            errors,
                        )
                        .unwrap_or(TypedConstValue::F32(0.0));
                        vec![value; len]
                    }
                };

                for (idx, default) in defaults.into_iter().enumerate() {
                    out.push(TypedParam {
                        name: format!("{}[{idx}]", param.name),
                        ty: PrimitiveType::F32,
                        default,
                        range: None,
                    });
                }
            }
            Some(DeclType::Array { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "param '{}.{}' range is not supported for array declarations",
                            "<top-level>", param.name
                        ),
                        0,
                        0,
                    ));
                }
                let size_context = format!("param '{}.{}' array size", "<top-level>", param.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                arrays.insert(
                    param.name.clone(),
                    TypedArrayInfo {
                        elem_ty: *elem,
                        len,
                        offset: out.len(),
                    },
                );

                let defaults = match &param.default {
                    None => vec![coerce_const_default_to_typed(0.0, *elem); len],
                    Some(Expr::ArrayLiteral(values)) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "param '{}.{}' default expects {len} elements, got {}",
                                    "<top-level>",
                                    param.name,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
                        }
                        let mut defaults = Vec::with_capacity(len);
                        for idx in 0..len {
                            let value = values.get(idx).and_then(|expr| {
                                eval_typed_const_expr(
                                    expr,
                                    *elem,
                                    options,
                                    &format!(
                                        "param '{}.{}' default element {idx}",
                                        "<top-level>", param.name
                                    ),
                                    is_float_type(*elem),
                                    matches!(*elem, PrimitiveType::I32 | PrimitiveType::I64),
                                    errors,
                                )
                            });
                            defaults.push(
                                value.unwrap_or_else(|| coerce_const_default_to_typed(0.0, *elem)),
                            );
                        }
                        defaults
                    }
                    Some(expr) => {
                        let value = eval_typed_const_expr(
                            expr,
                            *elem,
                            options,
                            &format!("param '{}.{}' default", "<top-level>", param.name),
                            is_float_type(*elem),
                            matches!(*elem, PrimitiveType::I32 | PrimitiveType::I64),
                            errors,
                        )
                        .unwrap_or_else(|| coerce_const_default_to_typed(0.0, *elem));
                        vec![value; len]
                    }
                };

                for (idx, default) in defaults.into_iter().enumerate() {
                    out.push(TypedParam {
                        name: format!("{}[{idx}]", param.name),
                        ty: *elem,
                        default,
                        range: None,
                    });
                }
            }
        }
    }
    (out, arrays)
}

fn coerce_buffers(
    buffers: &[BufferDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<TypedBufferDecl> {
    let mut seen = HashSet::new();
    let mut out = Vec::<TypedBufferDecl>::new();

    for buffer in buffers {
        if !seen.insert(buffer.name.as_str()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate buffer '{}'", buffer.name),
                0,
                0,
            ));
            continue;
        }
        if is_builtin_constant_name(&buffer.name) {
            errors.push(Diagnostic::semantic(
                format!(
                    "buffer name '{}' is reserved as a builtin constant",
                    buffer.name
                ),
                0,
                0,
            ));
            continue;
        }

        let (elem_ty, channels) = match &buffer.ty {
            None => (PrimitiveType::F32, TypedBufferChannels::Mono),
            Some(spec) => {
                let elem_ty = match spec.elem {
                    BufferElemType::Primitive(ty) => ty,
                    BufferElemType::Generic(ref param_ty) => {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "buffer '{}' uses unresolved generic element type '{}'",
                                buffer.name, param_ty
                            ),
                            0,
                            0,
                        ));
                        PrimitiveType::F32
                    }
                };
                let channels = match &spec.channels {
                    BufferChannels::Mono => TypedBufferChannels::Mono,
                    BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                    BufferChannels::Static(expr) => {
                        let ctx = format!("buffer '{}' static channel count", buffer.name);
                        let Some(ch) = eval_data_size_expr(expr, options, &ctx, errors) else {
                            continue;
                        };
                        TypedBufferChannels::Static(ch)
                    }
                };
                (elem_ty, channels)
            }
        };
        out.push(TypedBufferDecl {
            name: buffer.name.clone(),
            elem_ty,
            channels,
        });
    }

    out
}

fn check_unique_set(
    names: &[String],
    kind: &str,
    all_declared: &mut HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut local = HashSet::new();
    for name in names {
        if is_builtin_constant_name(name) {
            errors.push(Diagnostic::semantic(
                format!("{kind} name '{name}' is reserved as a builtin constant"),
                0,
                0,
            ));
            continue;
        }
        if !local.insert(name.clone()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate {kind} '{name}'"),
                0,
                0,
            ));
            continue;
        }
        if !all_declared.insert(name.clone()) {
            errors.push(Diagnostic::semantic(
                format!("symbol '{name}' declared multiple times across blocks"),
                0,
                0,
            ));
        }
    }
}

#[derive(Default)]
struct IoInference {
    max_in: usize,
    max_out: usize,
}

fn infer_numbered_io_from_sample(sample: &[Stmt]) -> IoInference {
    let mut out = IoInference::default();
    for stmt in sample {
        infer_io_from_stmt(stmt, &mut out);
    }
    out
}

fn infer_io_from_stmt(stmt: &Stmt, acc: &mut IoInference) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    acc.max_out = acc
                        .max_out
                        .max(parse_numbered_port_index(name, "out").unwrap_or(0));
                }
                AssignTarget::Index { base, index } => {
                    acc.max_out = acc
                        .max_out
                        .max(parse_numbered_port_index(base, "out").unwrap_or(0));
                    infer_io_from_expr(index, acc);
                }
            }
            infer_io_from_expr(expr, acc);
        }
        Stmt::Expr { expr, .. } => infer_io_from_expr(expr, acc),
        Stmt::Return { expr, .. } => infer_io_from_expr(expr, acc),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            infer_io_from_expr(cond, acc);
            for nested in then_branch {
                infer_io_from_stmt(nested, acc);
            }
            for nested in else_branch {
                infer_io_from_stmt(nested, acc);
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                infer_io_from_stmt(nested, acc);
            }
        }
    }
}

fn infer_io_from_expr(expr: &Expr, acc: &mut IoInference) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::DataCtor { .. } => {}
        Expr::Var(name) => {
            acc.max_in = acc
                .max_in
                .max(parse_numbered_port_index(name, "in").unwrap_or(0));
            acc.max_out = acc
                .max_out
                .max(parse_numbered_port_index(name, "out").unwrap_or(0));
        }
        Expr::Index { base, index } => {
            acc.max_in = acc
                .max_in
                .max(parse_numbered_port_index(base, "in").unwrap_or(0));
            acc.max_out = acc
                .max_out
                .max(parse_numbered_port_index(base, "out").unwrap_or(0));
            infer_io_from_expr(index, acc);
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            infer_io_from_expr(lhs, acc);
            infer_io_from_expr(rhs, acc);
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => infer_io_from_expr(expr, acc),
        Expr::Logical { lhs, rhs, .. } => {
            infer_io_from_expr(lhs, acc);
            infer_io_from_expr(rhs, acc);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                infer_io_from_expr(arg, acc);
            }
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                infer_io_from_expr(value, acc);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                infer_io_from_expr(&arg.expr, acc);
            }
        }
    }
}

fn parse_numbered_port_index(name: &str, prefix: &str) -> Option<usize> {
    let tail = name.strip_prefix(prefix)?;
    if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let idx = tail.parse::<usize>().ok()?;
    if idx == 0 {
        return None;
    }
    Some(idx)
}

fn register_block_assigned_scalars_as_state<'a>(
    stmts: impl Iterator<Item = &'a Stmt>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
) {
    for stmt in stmts {
        register_block_stmt_assigned_scalars_as_state(
            stmt,
            state_scalars,
            state_data,
            state_data_struct_roots,
            struct_instances,
            input_names,
            output_names,
            param_names,
            struct_defs,
            fn_signatures,
        );
    }
}

fn register_sample_typed_scalar_decls_as_state<'a>(
    stmts: impl Iterator<Item = &'a Stmt>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
) {
    for stmt in stmts {
        register_sample_stmt_typed_scalar_decls_as_state(
            stmt,
            state_scalars,
            state_data,
            state_data_struct_roots,
            struct_instances,
            input_names,
            output_names,
            param_names,
        );
    }
}

fn register_sample_stmt_typed_scalar_decls_as_state(
    stmt: &Stmt,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
) {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            expr,
            ..
        } => {
            if !*is_typed_decl {
                return;
            }
            if let AssignTarget::Var(name) = target {
                if split_simple_field_path(name).is_none()
                    && !is_builtin_constant_name(name)
                    && !input_names.contains(name)
                    && !output_names.contains(name)
                    && !param_names.contains(name)
                    && !state_scalars.contains_key(name)
                    && !state_data.contains_key(name)
                    && !state_data_struct_roots.contains_key(name)
                    && !struct_instances.contains_key(name)
                    && !matches!(expr, Expr::DataCtor { .. })
                    && generic_decl_ty.is_none()
                {
                    state_scalars.insert(name.clone(), decl_ty.unwrap_or(PrimitiveType::F32));
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                register_sample_stmt_typed_scalar_decls_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                );
            }
            for nested in else_branch {
                register_sample_stmt_typed_scalar_decls_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                );
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                register_sample_stmt_typed_scalar_decls_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                );
            }
        }
        Stmt::Expr { .. } | Stmt::Return { .. } => {}
    }
}

fn register_block_stmt_assigned_scalars_as_state(
    stmt: &Stmt,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_data: &HashMap<String, usize>,
    state_data_struct_roots: &HashMap<String, DataStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
) {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl: _,
            expr,
            ..
        } => {
            if let AssignTarget::Var(name) = target {
                if split_simple_field_path(name).is_none()
                    && !is_builtin_constant_name(name)
                    && !input_names.contains(name)
                    && !output_names.contains(name)
                    && !param_names.contains(name)
                    && !state_scalars.contains_key(name)
                    && !state_data.contains_key(name)
                    && !state_data_struct_roots.contains_key(name)
                    && !struct_instances.contains_key(name)
                    && !matches!(expr, Expr::DataCtor { .. })
                    && generic_decl_ty.is_none()
                {
                    let inferred_ty = {
                        let mut infer_errors = Vec::<Diagnostic>::new();
                        let empty_locals = HashSet::<String>::new();
                        infer_expr_type_for_semantics(
                            expr,
                            state_scalars,
                            None,
                            &empty_locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instances,
                            struct_defs,
                            &mut infer_errors,
                        )
                        .unwrap_or(PrimitiveType::F32)
                    };
                    state_scalars.insert(name.clone(), decl_ty.unwrap_or(inferred_ty));
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                register_block_stmt_assigned_scalars_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
                );
            }
            for nested in else_branch {
                register_block_stmt_assigned_scalars_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
                );
            }
        }
        Stmt::For { body, .. } => {
            for nested in body {
                register_block_stmt_assigned_scalars_as_state(
                    nested,
                    state_scalars,
                    state_data,
                    state_data_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
                );
            }
        }
        Stmt::Expr { .. } | Stmt::Return { .. } => {}
    }
}

fn normalize_numbered_ports(explicit: &[String], prefix: &str, inferred_max: usize) -> Vec<String> {
    let explicit_max = explicit
        .iter()
        .filter_map(|name| parse_numbered_port_index(name, prefix))
        .max()
        .unwrap_or(0);
    let max_idx = explicit_max.max(inferred_max);

    let mut out = Vec::new();
    if max_idx > 0 {
        for idx in 1..=max_idx {
            out.push(format!("{prefix}{idx}"));
        }
    }

    for name in explicit {
        if parse_numbered_port_index(name, prefix).is_none() && !out.contains(name) {
            out.push(name.clone());
        }
    }

    out
}

fn normalize_numbered_port_decls(
    explicit: &[PortDecl],
    prefix: &str,
    inferred_max: usize,
) -> Vec<PortDecl> {
    let explicit_names = explicit.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let ordered_names = normalize_numbered_ports(&explicit_names, prefix, inferred_max);
    let explicit_map = explicit
        .iter()
        .map(|p| (p.name.clone(), p.clone()))
        .collect::<HashMap<_, _>>();
    ordered_names
        .into_iter()
        .map(|name| {
            explicit_map.get(&name).cloned().unwrap_or(PortDecl {
                name,
                ty: None,
                default: None,
                range: None,
            })
        })
        .collect()
}

fn check_local_port_duplicates(ports: &[PortDecl], kind: &str, errors: &mut Vec<Diagnostic>) {
    let names = ports.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    check_local_duplicates(&names, kind, errors);
}

fn check_local_duplicates(names: &[String], kind: &str, errors: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name.clone()) {
            errors.push(Diagnostic::semantic(
                format!("duplicate {kind} '{name}'"),
                0,
                0,
            ));
        }
    }
}

