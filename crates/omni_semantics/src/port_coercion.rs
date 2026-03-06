use super::*;

pub(super) fn coerce_const_default_to_typed(
    raw_default: f64,
    ty: PrimitiveType,
) -> TypedConstValue {
    match ty {
        PrimitiveType::F32 => TypedConstValue::F32(raw_default as f32),
        PrimitiveType::F64 => TypedConstValue::F64(raw_default),
        PrimitiveType::I32 => TypedConstValue::I32(raw_default as i32),
        PrimitiveType::I64 => TypedConstValue::I64(raw_default as i64),
        PrimitiveType::Bool => TypedConstValue::Bool(raw_default != 0.0),
    }
}

pub(super) fn int_bounds_for_type(ty: PrimitiveType) -> Option<(f64, f64)> {
    match ty {
        PrimitiveType::I32 => Some((i32::MIN as f64, i32::MAX as f64)),
        PrimitiveType::I64 => Some((i64::MIN as f64, i64::MAX as f64)),
        _ => None,
    }
}

pub(super) fn primitive_type_label(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

pub(super) fn typed_min_for_type(ty: PrimitiveType) -> TypedConstValue {
    match ty {
        PrimitiveType::F32 => TypedConstValue::F32(f32::MIN),
        PrimitiveType::F64 => TypedConstValue::F64(f64::MIN),
        PrimitiveType::I32 => TypedConstValue::I32(i32::MIN),
        PrimitiveType::I64 => TypedConstValue::I64(i64::MIN),
        PrimitiveType::Bool => TypedConstValue::Bool(false),
    }
}

pub(super) fn eval_typed_const_expr(
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

pub(super) fn eval_decl_range_for_type(
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

pub(super) fn clamp_typed_const_to_range(
    value: TypedConstValue,
    range: TypedValueRange,
) -> TypedConstValue {
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

pub(super) fn typed_const_expr(value: TypedConstValue) -> Expr {
    match value {
        TypedConstValue::F32(v) => Expr::Number(v),
        TypedConstValue::F64(v) => Expr::Number(v as f32),
        TypedConstValue::I32(v) => Expr::Int(v as i64),
        TypedConstValue::I64(v) => Expr::Int(v),
        TypedConstValue::Bool(v) => Expr::Bool(v),
    }
}

pub(super) fn clamp_expr_to_range(expr: Expr, range: TypedValueRange) -> Expr {
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

pub(super) fn rewrite_top_level_range_clamps_in_expr(
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
        Expr::ArrayCtor { spec, init } => {
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
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner }
        | Expr::UnaryBitNot { expr: inner } => {
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

pub(super) fn rewrite_top_level_range_clamps_in_stmt(
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
            start,
            end,
            step,
            body,
            ..
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
            if let Some(step_expr) = step {
                rewrite_top_level_range_clamps_in_expr(
                    step_expr,
                    input_aliases,
                    param_aliases,
                    clamp_inputs,
                    clamp_params,
                );
            }
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
        Stmt::While { cond, body, .. } => {
            rewrite_top_level_range_clamps_in_expr(
                cond,
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
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn build_top_level_range_hoist_assign(
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

pub(super) fn expand_port_decls(
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

#[derive(Debug, Clone)]
pub(crate) struct FnSignature {
    pub(crate) params: Vec<String>,
    pub(crate) defaults: Vec<Option<Expr>>,
    pub(crate) param_types: Vec<Option<FnParamType>>,
    pub(crate) type_params: Vec<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct ExprEnv<'a> {
    pub(crate) known_scalars: &'a HashSet<String>,
    pub(crate) locals: &'a HashSet<String>,
    pub(crate) outputs: &'a HashSet<String>,
    pub(crate) array_vars: &'a HashMap<String, usize>,
    pub(crate) param_structs: &'a HashMap<String, String>,
    pub(crate) struct_instances: &'a HashMap<String, String>,
    pub(crate) struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub(crate) fn_signatures: &'a HashMap<String, FnSignature>,
    pub(crate) allow_array_ctor: bool,
    pub(crate) scope: ScopeKind,
}
