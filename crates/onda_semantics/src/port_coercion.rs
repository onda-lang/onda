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

fn eval_typed_int_const_expr(
    expr: &Expr,
    ty: PrimitiveType,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    let expr_diag = DiagCtx::new(expr.loc());

    let value = crate::const_scalar::eval_const_scalar(expr, Some(ty), options, context, errors)?;
    if let Some(value) = match value {
        TypedConstValue::I32(value) => Some(i64::from(value)),
        TypedConstValue::I64(value) => Some(value),
        TypedConstValue::Bool(value) => Some(i64::from(value)),
        _ => None,
    } {
        return match ty {
            PrimitiveType::I32 => {
                if let Ok(value) = i32::try_from(value) {
                    Some(TypedConstValue::I32(value))
                } else {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("{context} is out of range for {}", primitive_type_label(ty)),
                    );
                    None
                }
            }
            PrimitiveType::I64 => Some(TypedConstValue::I64(value)),
            _ => unreachable!("eval_typed_int_const_expr only supports integer targets"),
        };
    }

    let raw = value.to_f64();
    if !raw.is_finite() || raw.fract() != 0.0 {
        push_semantic(
            expr_diag,
            errors,
            format!("{context} must be an integer constant"),
        );
        return None;
    }
    if let Some((min, max)) = int_bounds_for_type(ty) {
        if raw < min || raw > max || (ty == PrimitiveType::I64 && raw == max) {
            push_semantic(
                expr_diag,
                errors,
                format!("{context} is out of range for {}", primitive_type_label(ty)),
            );
            return None;
        }
    }
    Some(coerce_const_default_to_typed(raw, ty))
}

pub(super) fn eval_typed_const_expr(
    expr: &Expr,
    ty: PrimitiveType,
    options: AnalysisOptions,
    context: &str,
    allow_non_finite: bool,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    let expr_diag = DiagCtx::new(expr.loc());
    match ty {
        PrimitiveType::F32 | PrimitiveType::F64 => {
            let value =
                crate::const_scalar::eval_const_scalar(expr, Some(ty), options, context, errors)?;
            let value = if ty == PrimitiveType::F32 {
                TypedConstValue::F32(value.to_f32())
            } else {
                TypedConstValue::F64(value.to_f64())
            };
            if !allow_non_finite && !value.to_f64().is_finite() {
                push_semantic(expr_diag, errors, format!("{context} must be finite"));
                return None;
            }
            Some(value)
        }
        PrimitiveType::I32 | PrimitiveType::I64 => {
            eval_typed_int_const_expr(expr, ty, options, context, errors)
        }
        PrimitiveType::Bool => Some(TypedConstValue::Bool(eval_const_bool_expr(
            expr, options, context, errors,
        )?)),
    }
}

pub(super) fn eval_decl_range_for_type(
    range: &DeclRange,
    ty: PrimitiveType,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedValueRange> {
    if ty == PrimitiveType::Bool {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context} range is not supported for bool"),
        );
        return None;
    }
    let min = if let Some(min_expr) = &range.min {
        eval_typed_const_expr(
            min_expr,
            ty,
            options,
            &format!("{context} range minimum"),
            false,
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
        errors,
    )?;
    if min.to_f64() > max.to_f64() {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context} range minimum is greater than range maximum"),
        );
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
    fn typed_scalar_expr(to: PrimitiveType, expr: Expr) -> Expr {
        Expr::Cast {
            loc: Default::default(),
            to,
            expr: Box::new(expr),
        }
    }

    match value {
        TypedConstValue::F32(v) => typed_scalar_expr(PrimitiveType::F32, Expr::number(v as f64)),
        TypedConstValue::F64(v) => Expr::number(v),
        TypedConstValue::I32(v) => typed_scalar_expr(PrimitiveType::I32, Expr::int(v as i64)),
        TypedConstValue::I64(value) => Expr::Int {
            loc: Default::default(),
            value,
            const_ty: Some(PrimitiveType::I64),
        },
        TypedConstValue::Bool(v) => Expr::bool(v),
    }
}

pub(super) fn cast_expr_to_primitive(expr: Expr, ty: PrimitiveType) -> Expr {
    Expr::Cast {
        loc: Default::default(),
        to: ty,
        expr: Box::new(expr),
    }
}

pub(super) fn clamp_expr_to_range(expr: Expr, range: TypedValueRange) -> Expr {
    let min_expr = typed_const_expr(range.min);
    let max_expr = typed_const_expr(range.max);
    Expr::Call {
        loc: Default::default(),
        func: BuiltinFn::RangeClamp,
        args: vec![expr, min_expr, max_expr],
    }
}

fn expr_matches_typed_const(expr: &Expr, value: TypedConstValue) -> bool {
    match (expr, value) {
        (
            Expr::Cast {
                to: PrimitiveType::F32,
                expr,
                ..
            },
            TypedConstValue::F32(expected),
        ) => matches!(
            expr.as_ref(),
            Expr::Number { value, .. } if value.to_bits() == (expected as f64).to_bits()
        ),
        (Expr::Number { value, .. }, TypedConstValue::F64(expected)) => {
            value.to_bits() == expected.to_bits()
        }
        (
            Expr::Cast {
                to: PrimitiveType::I32,
                expr,
                ..
            },
            TypedConstValue::I32(expected),
        ) => matches!(
            expr.as_ref(),
            Expr::Int { value, .. } if *value == i64::from(expected)
        ),
        (Expr::Int { value, .. }, TypedConstValue::I64(expected)) => *value == expected,
        (Expr::Bool { value, .. }, TypedConstValue::Bool(expected)) => *value == expected,
        _ => false,
    }
}

pub(super) fn expr_is_clamped_to_range(
    expr: &Expr,
    range: TypedValueRange,
    ty: PrimitiveType,
) -> bool {
    let Expr::Cast {
        to, expr: clamped, ..
    } = expr
    else {
        return false;
    };
    if *to != ty {
        return false;
    }
    let Expr::Call {
        func: BuiltinFn::RangeClamp,
        args,
        ..
    } = clamped.as_ref()
    else {
        return false;
    };
    let [_, minimum, maximum] = args.as_slice() else {
        return false;
    };
    expr_matches_typed_const(minimum, range.min) && expr_matches_typed_const(maximum, range.max)
}

#[derive(Default)]
pub(super) struct TopLevelRangeClampUsage {
    pub(super) aliases: HashSet<String>,
    pub(super) dynamic_input_aliases: HashSet<String>,
    pub(super) dynamic_param_aliases: HashSet<String>,
}

pub(super) fn rewrite_top_level_range_clamps_in_expr(
    expr: &mut Expr,
    input_aliases: &HashMap<String, String>,
    param_aliases: &HashMap<String, String>,
    shadowed: &HashSet<String>,
    clamp_inputs: bool,
    clamp_params: bool,
    usage: &mut TopLevelRangeClampUsage,
) {
    expr.visit_mut(|expr| {
        match expr {
            Expr::Var { name, .. } => {
                if shadowed.contains(name) {
                    return false;
                }
                if clamp_inputs {
                    if let Some(alias) = input_aliases.get(name) {
                        usage.aliases.insert(alias.clone());
                        *expr = Expr::var(alias.clone());
                        return false;
                    }
                }
                if clamp_params {
                    if let Some(alias) = param_aliases.get(name) {
                        usage.aliases.insert(alias.clone());
                        *expr = Expr::var(alias.clone());
                    }
                }
            }
            Expr::Index { base, .. } => {
                if base == "ins" && clamp_inputs {
                    usage.aliases.extend(input_aliases.values().cloned());
                    usage
                        .dynamic_input_aliases
                        .extend(input_aliases.values().cloned());
                } else if matches!(base.as_str(), "params" | "kins") && clamp_params {
                    usage.aliases.extend(param_aliases.values().cloned());
                    usage
                        .dynamic_param_aliases
                        .extend(param_aliases.values().cloned());
                }
            }
            _ => {}
        }
        true
    });
}

pub(super) fn rewrite_top_level_range_clamps_in_stmt(
    stmt: &mut Stmt,
    input_aliases: &HashMap<String, String>,
    param_aliases: &HashMap<String, String>,
    shadowed: &HashSet<String>,
    clamp_inputs: bool,
    clamp_params: bool,
    usage: &mut TopLevelRangeClampUsage,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            target.visit_selectors_mut(|index| {
                rewrite_top_level_range_clamps_in_expr(
                    index,
                    input_aliases,
                    param_aliases,
                    shadowed,
                    clamp_inputs,
                    clamp_params,
                    usage,
                );
            });
            rewrite_top_level_range_clamps_in_expr(
                expr,
                input_aliases,
                param_aliases,
                shadowed,
                clamp_inputs,
                clamp_params,
                usage,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_top_level_range_clamps_in_expr(
                expr,
                input_aliases,
                param_aliases,
                shadowed,
                clamp_inputs,
                clamp_params,
                usage,
            );
        }
        Stmt::Print { values, .. } => {
            for value in values {
                rewrite_top_level_range_clamps_in_expr(
                    value,
                    input_aliases,
                    param_aliases,
                    shadowed,
                    clamp_inputs,
                    clamp_params,
                    usage,
                );
            }
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
                shadowed,
                clamp_inputs,
                clamp_params,
                usage,
            );
            for nested in then_branch {
                rewrite_top_level_range_clamps_in_stmt(
                    nested,
                    input_aliases,
                    param_aliases,
                    shadowed,
                    clamp_inputs,
                    clamp_params,
                    usage,
                );
            }
            for nested in else_branch {
                rewrite_top_level_range_clamps_in_stmt(
                    nested,
                    input_aliases,
                    param_aliases,
                    shadowed,
                    clamp_inputs,
                    clamp_params,
                    usage,
                );
            }
        }
        Stmt::For {
            var,
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
                shadowed,
                clamp_inputs,
                clamp_params,
                usage,
            );
            rewrite_top_level_range_clamps_in_expr(
                end,
                input_aliases,
                param_aliases,
                shadowed,
                clamp_inputs,
                clamp_params,
                usage,
            );
            if let Some(step_expr) = step {
                rewrite_top_level_range_clamps_in_expr(
                    step_expr,
                    input_aliases,
                    param_aliases,
                    shadowed,
                    clamp_inputs,
                    clamp_params,
                    usage,
                );
            }
            let mut body_shadowed = shadowed.clone();
            body_shadowed.insert(var.clone());
            for nested in body {
                rewrite_top_level_range_clamps_in_stmt(
                    nested,
                    input_aliases,
                    param_aliases,
                    &body_shadowed,
                    clamp_inputs,
                    clamp_params,
                    usage,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_top_level_range_clamps_in_expr(
                cond,
                input_aliases,
                param_aliases,
                shadowed,
                clamp_inputs,
                clamp_params,
                usage,
            );
            for nested in body {
                rewrite_top_level_range_clamps_in_stmt(
                    nested,
                    input_aliases,
                    param_aliases,
                    shadowed,
                    clamp_inputs,
                    clamp_params,
                    usage,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn build_top_level_range_hoist_assign(
    alias_name: String,
    source_name: &str,
    range: TypedValueRange,
) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var(alias_name),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr: clamp_expr_to_range(Expr::var(source_name), range),
    }
}

pub(super) fn build_top_level_range_clamp_entry(
    names: &[String],
    ranges: &HashMap<String, TypedValueRange>,
    mut make_alias: impl FnMut(&str) -> String,
) -> (HashMap<String, String>, Vec<(String, Stmt)>) {
    let mut aliases = HashMap::new();
    let mut hoists = Vec::new();
    for name in names {
        let range = *ranges
            .get(name)
            .expect("range-clamp entry names must come from its range map");
        let alias = make_alias(name);
        aliases.insert(name.clone(), alias.clone());
        hoists.push((
            alias.clone(),
            build_top_level_range_hoist_assign(alias, name, range),
        ));
    }
    (aliases, hoists)
}

pub(super) fn used_top_level_range_clamp_hoists(
    hoists: Vec<(String, Stmt)>,
    used_aliases: &HashSet<String>,
) -> Vec<Stmt> {
    hoists
        .into_iter()
        .filter_map(|(alias, stmt)| used_aliases.contains(&alias).then_some(stmt))
        .collect()
}

type ExpandedPortDecls = (
    Vec<String>,
    HashMap<String, PrimitiveType>,
    HashMap<String, TypedArrayInfo>,
    HashMap<String, TypedConstValue>,
    HashMap<String, TypedValueRange>,
);

pub(super) fn expand_port_decls(
    ports: &[PortDecl],
    kind: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ExpandedPortDecls {
    let mut flat = Vec::new();
    let mut types = HashMap::new();
    let mut arrays = HashMap::new();
    let mut defaults = HashMap::new();
    let mut ranges = HashMap::new();

    for port in ports {
        let port_loc = port.loc.as_ref();
        match port.ty.as_ref() {
            Some(DeclType::Slice(_)) => {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    "ports and parameters require fixed value shapes",
                );
            }
            None | Some(DeclType::Scalar(_)) => {
                let ty = match port.ty.as_ref() {
                    Some(DeclType::Scalar(t)) => *t,
                    _ => PrimitiveType::F32,
                };
                let raw_default = match &port.default {
                    Some(expr) => with_loc_diag_context(port_loc, |_diag| {
                        eval_typed_const_expr(
                            expr,
                            ty,
                            options,
                            &format!("{kind} '{}' default", port.name),
                            is_float_type(ty),
                            errors,
                        )
                    })
                    .unwrap_or_else(|| coerce_const_default_to_typed(0.0, ty)),
                    None => coerce_const_default_to_typed(0.0, ty),
                };
                let mut default = raw_default;
                let range = with_loc_diag_context(port_loc, |_diag| {
                    port.range.as_ref().and_then(|r| {
                        eval_decl_range_for_type(
                            r,
                            ty,
                            options,
                            &format!("{kind} '{}'", port.name),
                            errors,
                        )
                    })
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
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "{kind} '{}' uses unresolved generic type '{}'",
                        port.name, param
                    ),
                    port_loc,
                ));
                flat.push(port.name.clone());
                types.insert(port.name.clone(), PrimitiveType::F32);
                defaults.insert(
                    port.name.clone(),
                    coerce_const_default_to_typed(0.0, PrimitiveType::F32),
                );
            }
            Some(DeclType::ArrayGeneric { elem, size }) => {
                if port.range.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{kind} '{}' range is not supported for array declarations",
                            port.name
                        ),
                        port_loc,
                    ));
                }
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "{kind} '{}' uses unresolved generic array element type '{}'",
                        port.name, elem
                    ),
                    port_loc,
                ));
                let size_context = format!("{kind} '{}' array size", port.name);
                let Some(len) = with_loc_diag_context(port_loc, |_diag| {
                    eval_data_size_expr(size, options, &size_context, errors)
                }) else {
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
                let defaults_for_slots = match &port.default {
                    None => vec![coerce_const_default_to_typed(0.0, PrimitiveType::F32); len],
                    Some(Expr::ArrayLiteral { values, .. }) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "{kind} '{}' default expects {len} elements, got {}",
                                    port.name,
                                    values.len()
                                ),
                                port_loc,
                            ));
                        }
                        let mut coerced = Vec::with_capacity(len);
                        for idx in 0..len {
                            let value = values.get(idx).and_then(|expr| {
                                with_loc_diag_context(port_loc, |_diag| {
                                    eval_typed_const_expr(
                                        expr,
                                        PrimitiveType::F32,
                                        options,
                                        &format!("{kind} '{}' default element [{idx}]", port.name),
                                        true,
                                        errors,
                                    )
                                })
                            });
                            coerced.push(value.unwrap_or_else(|| {
                                coerce_const_default_to_typed(0.0, PrimitiveType::F32)
                            }));
                        }
                        coerced
                    }
                    Some(expr) => {
                        let value = with_loc_diag_context(port_loc, |_diag| {
                            eval_typed_const_expr(
                                expr,
                                PrimitiveType::F32,
                                options,
                                &format!("{kind} '{}' default", port.name),
                                true,
                                errors,
                            )
                        })
                        .unwrap_or_else(|| coerce_const_default_to_typed(0.0, PrimitiveType::F32));
                        vec![value; len]
                    }
                };
                for (idx, default) in defaults_for_slots.into_iter().enumerate() {
                    let slot_name = format!("{}[{idx}]", port.name);
                    flat.push(slot_name.clone());
                    types.insert(slot_name.clone(), PrimitiveType::F32);
                    defaults.insert(slot_name, default);
                }
            }
            Some(DeclType::Tuple(_)) => {
                errors.push(Diagnostic::semantic_span(
                    format!("{kind} '{}' uses unsupported tuple type", port.name),
                    port_loc,
                ));
                continue;
            }
            Some(DeclType::Array { elem, size }) => {
                if port.range.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{kind} '{}' range is not supported for array declarations",
                            port.name
                        ),
                        port_loc,
                    ));
                }
                let size_context = format!("{kind} '{}' array size", port.name);
                let Some(len) = with_loc_diag_context(port_loc, |_diag| {
                    eval_data_size_expr(size, options, &size_context, errors)
                }) else {
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
                let defaults_for_slots = match &port.default {
                    None => vec![coerce_const_default_to_typed(0.0, *elem); len],
                    Some(Expr::ArrayLiteral { values, .. }) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "{kind} '{}' default expects {len} elements, got {}",
                                    port.name,
                                    values.len()
                                ),
                                port_loc,
                            ));
                        }
                        let mut coerced = Vec::with_capacity(len);
                        for idx in 0..len {
                            let value = values.get(idx).and_then(|expr| {
                                with_loc_diag_context(port_loc, |_diag| {
                                    eval_typed_const_expr(
                                        expr,
                                        *elem,
                                        options,
                                        &format!("{kind} '{}' default element [{idx}]", port.name),
                                        is_float_type(*elem),
                                        errors,
                                    )
                                })
                            });
                            coerced.push(
                                value.unwrap_or_else(|| coerce_const_default_to_typed(0.0, *elem)),
                            );
                        }
                        coerced
                    }
                    Some(expr) => {
                        let value = with_loc_diag_context(port_loc, |_diag| {
                            eval_typed_const_expr(
                                expr,
                                *elem,
                                options,
                                &format!("{kind} '{}' default", port.name),
                                is_float_type(*elem),
                                errors,
                            )
                        })
                        .unwrap_or_else(|| coerce_const_default_to_typed(0.0, *elem));
                        vec![value; len]
                    }
                };
                for (idx, default) in defaults_for_slots.into_iter().enumerate() {
                    let slot_name = format!("{}[{idx}]", port.name);
                    flat.push(slot_name.clone());
                    types.insert(slot_name.clone(), *elem);
                    defaults.insert(slot_name, default);
                }
            }
        }
    }

    (flat, types, arrays, defaults, ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_integer_literals_preserve_exact_values_and_checked_boundaries() {
        for (expr, ty, expected) in [
            (
                Expr::int(i64::MAX),
                PrimitiveType::I64,
                Some(TypedConstValue::I64(i64::MAX)),
            ),
            (Expr::int(i64::from(i32::MAX) + 1), PrimitiveType::I32, None),
            (Expr::number(i64::MAX as f64), PrimitiveType::I64, None),
            (
                Expr::number(i64::MIN as f64),
                PrimitiveType::I64,
                Some(TypedConstValue::I64(i64::MIN)),
            ),
            (
                Expr::number((i64::MAX as f64) - 1024.0),
                PrimitiveType::I64,
                Some(TypedConstValue::I64(i64::MAX - 1023)),
            ),
        ] {
            let mut errors = Vec::new();
            assert_eq!(
                eval_typed_int_const_expr(
                    &expr,
                    ty,
                    AnalysisOptions::default(),
                    "integer literal",
                    &mut errors
                ),
                expected
            );
            if expected.is_some() {
                assert!(errors.is_empty(), "{errors:?}");
            } else {
                assert_eq!(errors.len(), 1);
                assert!(errors[0].message.contains("out of range"), "{errors:?}");
            }
        }
    }

    #[test]
    fn eval_typed_const_expr_preserves_f64_literal_precision() {
        let expr = Expr::Cast {
            loc: Default::default(),
            to: PrimitiveType::F64,
            expr: Box::new(Expr::Binary {
                loc: Default::default(),
                op: BinaryOp::Add,
                lhs: Box::new(Expr::number(0.1)),
                rhs: Box::new(Expr::number(0.2)),
            }),
        };
        let mut errors = Vec::new();
        let value = eval_typed_const_expr(
            &expr,
            PrimitiveType::F64,
            AnalysisOptions::default(),
            "f64 const",
            true,
            &mut errors,
        );
        let expected = 0.1_f64 + 0.2_f64;
        let widened_f32 = 0.1_f32 as f64 + 0.2_f32 as f64;

        match value {
            Some(TypedConstValue::F64(v)) => {
                assert!(
                    (v - expected).abs() < 1e-18,
                    "expected f64 precision, got {v:?}"
                );
                assert!(
                    (v - widened_f32).abs() > 1e-9,
                    "expected f64 const eval to differ from widened f32 path, got {v:?}"
                );
            }
            other => panic!("expected f64 typed const value, got {other:?}"),
        }
        assert!(
            errors.is_empty(),
            "expected f64 typed const eval to succeed, got {errors:?}"
        );
    }

    #[test]
    fn eval_typed_const_expr_preserves_large_i64_values() {
        let expr = Expr::Cast {
            loc: Default::default(),
            to: PrimitiveType::I64,
            expr: Box::new(Expr::Binary {
                loc: Default::default(),
                op: BinaryOp::Add,
                lhs: Box::new(Expr::int(9_007_199_254_740_992)),
                rhs: Box::new(Expr::int(1)),
            }),
        };
        let mut errors = Vec::new();
        let value = eval_typed_const_expr(
            &expr,
            PrimitiveType::I64,
            AnalysisOptions::default(),
            "i64 const",
            false,
            &mut errors,
        );

        assert_eq!(value, Some(TypedConstValue::I64(9_007_199_254_740_993)));
        assert!(
            errors.is_empty(),
            "expected i64 typed const eval to succeed, got {errors:?}"
        );
    }

    #[test]
    fn implicit_integer_to_float_conversion_rounds_directly_to_destination() {
        for integer in [4_611_686_293_305_294_849_i64, -4_611_686_293_305_294_849] {
            let expr = typed_const_expr(TypedConstValue::I64(integer));
            for ty in [PrimitiveType::F32, PrimitiveType::F64] {
                let mut errors = Vec::new();
                let value = eval_typed_const_expr(
                    &expr,
                    ty,
                    AnalysisOptions::default(),
                    "float const",
                    false,
                    &mut errors,
                );
                let expected = if ty == PrimitiveType::F32 {
                    TypedConstValue::F32(integer as f32)
                } else {
                    TypedConstValue::F64(integer as f64)
                };
                assert_eq!(value, Some(expected));
                assert!(errors.is_empty(), "{errors:?}");
            }
        }
    }

    #[test]
    fn finite_float_requirements_apply_after_destination_conversion() {
        let expr = typed_const_expr(TypedConstValue::F64(f64::MAX));
        let mut errors = Vec::new();
        assert_eq!(
            eval_typed_const_expr(
                &expr,
                PrimitiveType::F32,
                AnalysisOptions::default(),
                "float const",
                false,
                &mut errors
            ),
            None
        );
        assert!(
            errors.iter().any(|error| error.message.contains("finite")),
            "{errors:?}"
        );
        errors.clear();
        assert_eq!(
            eval_typed_const_expr(
                &expr,
                PrimitiveType::F32,
                AnalysisOptions::default(),
                "float const",
                true,
                &mut errors
            ),
            Some(TypedConstValue::F32(f32::INFINITY))
        );
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn typed_const_expr_preserves_nondefault_numeric_types() {
        match typed_const_expr(TypedConstValue::F32(0.5)) {
            Expr::Cast { to, expr, .. } => {
                assert_eq!(to, PrimitiveType::F32);
                assert!(matches!(
                    expr.as_ref(),
                    Expr::Number { value, .. } if (*value - 0.5).abs() < f64::EPSILON
                ));
            }
            other => panic!("expected f32 typed const expr cast, got {other:?}"),
        }

        match typed_const_expr(TypedConstValue::F64(1.25)) {
            Expr::Number { value, .. } => {
                assert!(
                    (value - 1.25).abs() < f64::EPSILON,
                    "expected f64 literal, got {value:?}"
                );
            }
            other => panic!("expected f64 typed const expr number literal, got {other:?}"),
        }

        match typed_const_expr(TypedConstValue::I32(7)) {
            Expr::Cast { to, expr, .. } => {
                assert_eq!(to, PrimitiveType::I32);
                assert!(matches!(expr.as_ref(), Expr::Int { value: 7, .. }));
            }
            other => panic!("expected i32 typed const expr cast, got {other:?}"),
        }

        match typed_const_expr(TypedConstValue::I64(7)) {
            Expr::Int {
                value: 7,
                const_ty: Some(PrimitiveType::I64),
                ..
            } => {}
            other => panic!("expected i64 typed const expr int literal, got {other:?}"),
        }
    }
}
