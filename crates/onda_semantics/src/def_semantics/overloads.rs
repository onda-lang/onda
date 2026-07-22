use std::collections::HashSet;

use crate::*;

#[derive(Debug, Clone)]
pub(crate) struct OverloadCandidate {
    internal_name: String,
    signature: FnSignature,
}

#[derive(Debug, Clone)]
enum OverloadArgShape {
    Scalar(PrimitiveType),
    Struct(String),
    Array,
    Buffer {
        elem_ty: PrimitiveType,
        channels: TypedBufferChannels,
    },
    Unknown,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct OverloadRewriteEnv {
    pub(crate) scalar_types: HashMap<String, PrimitiveType>,
    pub(crate) struct_instances: HashMap<String, String>,
    pub(crate) array_elem_types: HashMap<String, PrimitiveType>,
    pub(crate) buffer_types: HashMap<String, (PrimitiveType, TypedBufferChannels)>,
}

impl OverloadRewriteEnv {
    pub(crate) fn shadow_binding(&mut self, name: &str) {
        let child_prefix = format!("{name}.");
        self.scalar_types
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
        self.struct_instances
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
        self.array_elem_types
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
        self.buffer_types
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
    }
}

fn overload_internal_name(public_name: &str, ordinal: usize) -> String {
    format!(
        "__onda_ovl_{}_{}",
        sanitize_symbol_component(public_name),
        ordinal
    )
}

pub(crate) fn const_positive_usize_for_overload(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Int { value, .. } if *value > 0 => usize::try_from(*value).ok(),
        Expr::Number { value, .. } if *value > 0.0 && value.fract() == 0.0 => {
            usize::try_from(*value as i64).ok()
        }
        _ => None,
    }
}

fn primitive_type_for_number_literal(v: i64) -> PrimitiveType {
    if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
        PrimitiveType::I32
    } else {
        PrimitiveType::I64
    }
}

fn merge_numeric_types_no_diag(lhs: PrimitiveType, rhs: PrimitiveType) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (F64, I32)
        | (I32, F64)
        | (F64, I64)
        | (I64, F64)
        | (F64, F32)
        | (F32, F64)
        | (F64, F64) => Some(F64),
        (F32, I32) | (I32, F32) | (F32, F32) | (F32, I64) | (I64, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => None,
    }
}

fn infer_scalar_expr_type_for_overload(
    expr: &Expr,
    env: &OverloadRewriteEnv,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { value, .. } => Some(primitive_type_for_number_literal(*value)),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::Var { name, .. } => {
            if let Some(ty) = builtin_constant_type(name) {
                return Some(ty);
            }
            if let Some((base, field)) = split_simple_field_path(name) {
                let flat = format!("{base}.{field}");
                if let Some(ty) = env.scalar_types.get(&flat).copied() {
                    return Some(ty);
                }
            }
            env.scalar_types.get(name).copied()
        }
        Expr::Index { base, .. } => {
            if let Some(elem_ty) = env.array_elem_types.get(base).copied() {
                return Some(elem_ty);
            }
            if let Some((elem_ty, _)) = env.buffer_types.get(base) {
                return Some(*elem_ty);
            }
            if let Some((root, field)) = split_simple_field_path(base) {
                let flat = format!("{root}.{field}");
                if let Some(elem_ty) = env.array_elem_types.get(&flat).copied() {
                    return Some(elem_ty);
                }
            }
            None
        }
        Expr::Slice { .. } => None,
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr, .. } => {
            let inner_ty = infer_scalar_expr_type_for_overload(expr, env)?;
            match inner_ty {
                PrimitiveType::I32 | PrimitiveType::I64 => Some(inner_ty),
                _ => None,
            }
        }
        Expr::Call { args, .. } => {
            if args.is_empty() {
                return Some(PrimitiveType::F32);
            }
            let mut acc = PrimitiveType::F32;
            for arg in args {
                let arg_ty = infer_scalar_expr_type_for_overload(arg, env)?;
                let merged = merge_numeric_types_no_diag(acc, arg_ty)?;
                acc = merged;
            }
            Some(acc)
        }
        Expr::UserCall { .. } => None,
        Expr::Binary { op, lhs, rhs, .. } => {
            let lhs_ty = infer_scalar_expr_type_for_overload(lhs, env)?;
            let rhs_ty = infer_scalar_expr_type_for_overload(rhs, env)?;
            match op {
                BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight => match (lhs_ty, rhs_ty) {
                    (PrimitiveType::I64, PrimitiveType::I32)
                    | (PrimitiveType::I32, PrimitiveType::I64)
                    | (PrimitiveType::I64, PrimitiveType::I64) => Some(PrimitiveType::I64),
                    (PrimitiveType::I32, PrimitiveType::I32) => Some(PrimitiveType::I32),
                    _ => None,
                },
                _ => merge_numeric_types_no_diag(lhs_ty, rhs_ty),
            }
        }
        Expr::ArrayCtor { .. } | Expr::ArrayLiteral { .. } | Expr::Tuple { .. } => None,
    }
}

fn infer_array_elem_type_for_overload(
    expr: &Expr,
    env: &OverloadRewriteEnv,
) -> Option<PrimitiveType> {
    match expr {
        Expr::ArrayLiteral { values, .. } => {
            let first = values.first()?;
            infer_scalar_expr_type_for_overload(first, env)
        }
        Expr::ArrayCtor { spec, .. } => match &spec.elem {
            ArrayElemType::Primitive(elem) => Some(*elem),
            ArrayElemType::Struct(_) => None,
        },
        _ => None,
    }
}

fn infer_overload_arg_shape(expr: &Expr, env: &OverloadRewriteEnv) -> OverloadArgShape {
    match expr {
        Expr::Var { name, .. } => {
            if let Some(struct_name) = env.struct_instances.get(name) {
                return OverloadArgShape::Struct(struct_name.clone());
            }
            if env.array_elem_types.contains_key(name) {
                return OverloadArgShape::Array;
            }
            if let Some((elem_ty, channels)) = env.buffer_types.get(name) {
                return OverloadArgShape::Buffer {
                    elem_ty: *elem_ty,
                    channels: channels.clone(),
                };
            }
            if let Some(ty) = infer_scalar_expr_type_for_overload(expr, env) {
                return OverloadArgShape::Scalar(ty);
            }
            OverloadArgShape::Unknown
        }
        Expr::Index { base, .. } => {
            if let Some((elem_ty, _)) = env.buffer_types.get(base) {
                return OverloadArgShape::Scalar(*elem_ty);
            }
            if let Some(elem_ty) = env.array_elem_types.get(base).copied() {
                return OverloadArgShape::Scalar(elem_ty);
            }
            if let Some((root, field)) = split_simple_field_path(base) {
                let flat = format!("{root}.{field}");
                if let Some(elem_ty) = env.array_elem_types.get(&flat).copied() {
                    return OverloadArgShape::Scalar(elem_ty);
                }
            }
            OverloadArgShape::Unknown
        }
        Expr::Slice { base, .. } => {
            if env.array_elem_types.contains_key(base) || env.buffer_types.contains_key(base) {
                OverloadArgShape::Array
            } else {
                OverloadArgShape::Unknown
            }
        }
        _ => {
            if let Some(ty) = infer_scalar_expr_type_for_overload(expr, env) {
                OverloadArgShape::Scalar(ty)
            } else {
                OverloadArgShape::Unknown
            }
        }
    }
}

fn score_buffer_match(
    expected: &BufferType,
    arg_elem_ty: PrimitiveType,
    arg_channels: &TypedBufferChannels,
) -> Option<i32> {
    let expected_elem = match expected.elem {
        BufferElemType::Primitive(ty) => ty,
        BufferElemType::Generic(_) => return Some(3),
    };
    if expected_elem != arg_elem_ty {
        return None;
    }
    match &expected.channels {
        BufferChannels::Mono => match arg_channels {
            TypedBufferChannels::Mono => Some(0),
            TypedBufferChannels::Static(ch) if *ch == 1 => Some(0),
            _ => None,
        },
        BufferChannels::Dynamic => match arg_channels {
            TypedBufferChannels::Mono => None,
            TypedBufferChannels::Static(ch) if *ch > 1 => Some(1),
            TypedBufferChannels::Dynamic => Some(0),
            _ => None,
        },
        BufferChannels::Static(expr) => {
            let expected_ch = const_positive_usize_for_overload(expr);
            match expected_ch {
                Some(ch) if ch <= 1 => match arg_channels {
                    TypedBufferChannels::Mono => Some(0),
                    TypedBufferChannels::Static(actual) if *actual == 1 => Some(0),
                    _ => None,
                },
                Some(ch) => match arg_channels {
                    TypedBufferChannels::Static(actual) if *actual == ch => Some(0),
                    TypedBufferChannels::Dynamic => Some(1),
                    _ => None,
                },
                None => match arg_channels {
                    TypedBufferChannels::Mono => None,
                    TypedBufferChannels::Static(ch) if *ch > 1 => Some(1),
                    TypedBufferChannels::Dynamic => Some(1),
                    _ => None,
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Def monomorphization — generic struct, untyped array [], bare buffer params
// ---------------------------------------------------------------------------

fn score_overload_param_match(
    arg_shape: &OverloadArgShape,
    param_ty: Option<&FnParamType>,
    def_type_params: &[String],
) -> Option<i32> {
    match param_ty {
        Some(FnParamType::Primitive(expected)) => match arg_shape {
            OverloadArgShape::Scalar(src) => {
                if *src == *expected {
                    Some(0)
                } else if can_implicitly_assign(*src, *expected) {
                    Some(1)
                } else {
                    None
                }
            }
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::Struct(expected_struct))
            if !def_type_params.is_empty() && def_type_params.contains(expected_struct) =>
        {
            // Generic type parameter — matches any scalar arg.
            // Score 2: between concrete (0) and untyped (3).
            match arg_shape {
                OverloadArgShape::Scalar(_) | OverloadArgShape::Unknown => Some(2),
                _ => None,
            }
        }
        Some(FnParamType::Struct(expected_struct)) => match arg_shape {
            OverloadArgShape::Struct(actual_struct) if actual_struct == expected_struct => Some(0),
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::Buffer(expected_buffer)) => match arg_shape {
            OverloadArgShape::Buffer { elem_ty, channels } => {
                score_buffer_match(expected_buffer, *elem_ty, channels)
            }
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::Array(Some(_expected_elem))) => match arg_shape {
            OverloadArgShape::Array => Some(0),
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::ArrayGeneric(_)) | Some(FnParamType::SizedArray { .. }) => {
            match arg_shape {
                OverloadArgShape::Array => Some(0),
                OverloadArgShape::Unknown => Some(2),
                _ => None,
            }
        }
        Some(FnParamType::Array(None)) => match arg_shape {
            OverloadArgShape::Array => Some(1),
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::BareBuffer) => match arg_shape {
            OverloadArgShape::Buffer { .. } => Some(1),
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        Some(FnParamType::Tuple(_)) => match arg_shape {
            OverloadArgShape::Unknown => Some(2),
            _ => None,
        },
        None => Some(3),
    }
}

fn format_fn_param_for_overload(name: &str, ty: Option<&FnParamType>, has_default: bool) -> String {
    let typed = match ty {
        Some(FnParamType::Primitive(prim)) => format!("{name}: {prim:?}").to_lowercase(),
        Some(FnParamType::Struct(struct_name)) => format!("{name}: {struct_name}"),
        Some(FnParamType::Buffer(buffer_ty)) => format!("{name}: {:?}", buffer_ty),
        Some(FnParamType::Array(Some(prim))) => format!("{name}: {prim:?}[]").to_lowercase(),
        Some(FnParamType::ArrayGeneric(param)) => format!("{name}: {param}[]"),
        Some(FnParamType::SizedArray {
            elem,
            generic_name,
            size,
        }) => {
            let type_str = if let Some(prim) = elem {
                format!("{prim:?}").to_lowercase()
            } else if let Some(g) = generic_name {
                g.clone()
            } else {
                "?".to_owned()
            };
            format!("{name}: {type_str}[{size:?}]")
        }
        Some(FnParamType::Array(None)) => format!("{name}: []"),
        Some(FnParamType::BareBuffer) => format!("{name}: buffer"),
        Some(FnParamType::Tuple(elems)) => {
            let inner = elems
                .iter()
                .map(|p| format!("{p:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{name}: ({inner})")
        }
        None => name.to_owned(),
    };
    if has_default {
        format!("{typed} = ...")
    } else {
        typed
    }
}

fn format_overload_signature(name: &str, signature: &FnSignature) -> String {
    let params = signature
        .params
        .iter()
        .enumerate()
        .map(|(idx, param_name)| {
            format_fn_param_for_overload(
                param_name,
                signature.param_types.get(idx).and_then(|p| p.as_ref()),
                signature
                    .defaults
                    .get(idx)
                    .and_then(|d| d.as_ref())
                    .is_some(),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({params})")
}

fn resolve_overloaded_call_name(
    public_name: &str,
    args: &[CallArg],
    env: &OverloadRewriteEnv,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let candidates = overloads.get(public_name)?;
    if candidates.len() == 1 {
        return Some(candidates[0].internal_name.clone());
    }

    let mut scored = Vec::<(i32, usize)>::new();
    for (cand_idx, cand) in candidates.iter().enumerate() {
        let mut bind_errors = Vec::new();
        let resolved = super::resolve_call_args(
            args,
            &cand.signature.params,
            &cand.signature.defaults,
            false,
            false,
            &format!("function '{public_name}' call"),
            &mut bind_errors,
        );
        if !bind_errors.is_empty() {
            continue;
        }

        let mut total_score = 0_i32;
        let mut viable = true;
        for (param_idx, arg_expr) in resolved.into_iter().enumerate() {
            if let Some(arg_expr) = arg_expr {
                let arg_shape = infer_overload_arg_shape(arg_expr, env);
                let param_ty = cand
                    .signature
                    .param_types
                    .get(param_idx)
                    .and_then(|t| t.as_ref());
                let Some(score) =
                    score_overload_param_match(&arg_shape, param_ty, &cand.signature.type_params)
                else {
                    viable = false;
                    break;
                };
                total_score += score;
            } else {
                // Slight preference for overloads requiring fewer defaulted params.
                total_score += 1;
            }
        }
        if viable {
            scored.push((total_score, cand_idx));
        }
    }

    if scored.is_empty() {
        let overload_list = candidates
            .iter()
            .map(|c| format_overload_signature(public_name, &c.signature))
            .collect::<Vec<_>>()
            .join(", ");
        push_semantic(
            diag,
            errors,
            format!(
                "no matching overload for function '{}' (candidates: {})",
                public_name, overload_list
            ),
        );
        return Some(candidates[0].internal_name.clone());
    }

    let best_score = scored
        .iter()
        .map(|(score, _)| *score)
        .min()
        .unwrap_or(i32::MAX);
    let best = scored
        .into_iter()
        .filter(|(score, _)| *score == best_score)
        .map(|(_, idx)| idx)
        .collect::<Vec<_>>();
    if best.len() > 1 {
        let overload_list = best
            .iter()
            .filter_map(|idx| candidates.get(*idx))
            .map(|c| format_overload_signature(public_name, &c.signature))
            .collect::<Vec<_>>()
            .join(", ");
        push_semantic(
            diag,
            errors,
            format!(
                "ambiguous overload for function '{}'; matching candidates: {}",
                public_name, overload_list
            ),
        );
    }

    best.first()
        .and_then(|idx| candidates.get(*idx))
        .map(|cand| cand.internal_name.clone())
}

fn update_overload_env_after_assign(
    target: &AssignTarget,
    decl_ty: Option<PrimitiveType>,
    expr: &Expr,
    env: &mut OverloadRewriteEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    let AssignTarget::Var(name) = target else {
        return;
    };

    if let Some(declared) = decl_ty {
        env.scalar_types.insert(name.clone(), declared);
        env.struct_instances.remove(name);
        return;
    }

    if let Some(elem_ty) = infer_array_elem_type_for_overload(expr, env) {
        env.array_elem_types.insert(name.clone(), elem_ty);
        env.scalar_types.remove(name);
        env.struct_instances.remove(name);
        return;
    }

    if let Expr::UserCall { name: callee, .. } = expr {
        if struct_defs.contains_key(callee) {
            env.struct_instances.insert(name.clone(), callee.clone());
            env.scalar_types.remove(name);
            return;
        }
    }

    if let Expr::Var { name: src, .. } = expr {
        if let Some(struct_name) = env.struct_instances.get(src).cloned() {
            env.struct_instances.insert(name.clone(), struct_name);
            env.scalar_types.remove(name);
            return;
        }
    }

    if let Some(ty) = infer_scalar_expr_type_for_overload(expr, env) {
        env.scalar_types.insert(name.clone(), ty);
        env.struct_instances.remove(name);
    }
}

pub(crate) fn rewrite_overloaded_calls_in_expr(
    expr: &mut Expr,
    env: &OverloadRewriteEnv,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    errors: &mut Vec<Diagnostic>,
) {
    with_expr_diag_context_mut(expr, |diag, expr| match expr {
        Expr::Index { index, .. } => {
            rewrite_overloaded_calls_in_expr(index, env, overloads, errors);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                rewrite_overloaded_calls_in_expr(start, env, overloads, errors);
            }
            if let Some(end) = end {
                rewrite_overloaded_calls_in_expr(end, env, overloads, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_overloaded_calls_in_expr(&mut spec.size, env, overloads, errors);
            if let Some(values) = init {
                for value in values {
                    rewrite_overloaded_calls_in_expr(value, env, overloads, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_overloaded_calls_in_expr(lhs, env, overloads, errors);
            rewrite_overloaded_calls_in_expr(rhs, env, overloads, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_overloaded_calls_in_expr(arg, env, overloads, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            rewrite_overloaded_calls_in_expr(expr, env, overloads, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_overloaded_calls_in_expr(value, env, overloads, errors);
            }
        }
        Expr::UserCall {
            name,
            type_args: _,
            args,
            ..
        } => {
            for arg in args.iter_mut() {
                rewrite_overloaded_calls_in_expr(&mut arg.expr, env, overloads, errors);
            }
            if let Some(resolved_name) =
                resolve_overloaded_call_name(name, args, env, overloads, diag, errors)
            {
                *name = resolved_name;
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    })
}

pub(crate) fn rewrite_overloaded_calls_in_stmt_list(
    stmts: &mut [Stmt],
    env: &mut OverloadRewriteEnv,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        with_stmt_diag_context_mut(stmt, |_diag, stmt| match stmt {
            Stmt::Const { .. } => {}
            Stmt::Assign {
                target,
                decl_ty,
                expr,
                ..
            } => {
                if let AssignTarget::Index { index, .. } = target {
                    rewrite_overloaded_calls_in_expr(index, env, overloads, errors);
                }
                rewrite_overloaded_calls_in_expr(expr, env, overloads, errors);
                update_overload_env_after_assign(target, *decl_ty, expr, env, struct_defs);
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                rewrite_overloaded_calls_in_expr(expr, env, overloads, errors);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_overloaded_calls_in_expr(cond, env, overloads, errors);
                let mut then_env = env.clone();
                rewrite_overloaded_calls_in_stmt_list(
                    then_branch,
                    &mut then_env,
                    overloads,
                    struct_defs,
                    errors,
                );
                let mut else_env = env.clone();
                rewrite_overloaded_calls_in_stmt_list(
                    else_branch,
                    &mut else_env,
                    overloads,
                    struct_defs,
                    errors,
                );
                env.scalar_types = then_env
                    .scalar_types
                    .iter()
                    .filter_map(|(name, ty)| {
                        else_env
                            .scalar_types
                            .get(name)
                            .copied()
                            .filter(|other| other == ty)
                            .map(|_| (name.clone(), *ty))
                    })
                    .collect::<HashMap<_, _>>();
                env.struct_instances = then_env
                    .struct_instances
                    .iter()
                    .filter_map(|(name, ty)| {
                        else_env
                            .struct_instances
                            .get(name)
                            .filter(|other| *other == ty)
                            .map(|_| (name.clone(), ty.clone()))
                    })
                    .collect::<HashMap<_, _>>();
                env.array_elem_types = then_env
                    .array_elem_types
                    .iter()
                    .filter_map(|(name, ty)| {
                        else_env
                            .array_elem_types
                            .get(name)
                            .copied()
                            .filter(|other| other == ty)
                            .map(|_| (name.clone(), *ty))
                    })
                    .collect::<HashMap<_, _>>();
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                rewrite_overloaded_calls_in_expr(start, env, overloads, errors);
                rewrite_overloaded_calls_in_expr(end, env, overloads, errors);
                if let Some(step_expr) = step {
                    rewrite_overloaded_calls_in_expr(step_expr, env, overloads, errors);
                }
                let mut body_env = env.clone();
                rewrite_overloaded_calls_in_stmt_list(
                    body,
                    &mut body_env,
                    overloads,
                    struct_defs,
                    errors,
                );
            }
            Stmt::While { cond, body, .. } => {
                rewrite_overloaded_calls_in_expr(cond, env, overloads, errors);
                let mut body_env = env.clone();
                rewrite_overloaded_calls_in_stmt_list(
                    body,
                    &mut body_env,
                    overloads,
                    struct_defs,
                    errors,
                );
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        });
    }
}

pub(crate) fn prepare_function_overloads(
    defs: &mut [FunctionDef],
) -> (
    HashMap<String, Vec<OverloadCandidate>>,
    HashMap<String, String>,
) {
    let mut by_public_top_level = HashMap::<String, Vec<usize>>::new();
    for (idx, def) in defs.iter().enumerate() {
        by_public_top_level
            .entry(def.name.clone())
            .or_default()
            .push(idx);
    }

    let mut internal_to_public = HashMap::<String, String>::new();
    for (public_name, indices) in &by_public_top_level {
        if indices.len() <= 1 {
            continue;
        }
        for (ordinal, idx) in indices.iter().enumerate() {
            let internal_name = overload_internal_name(public_name, ordinal + 1);
            defs[*idx].name = internal_name.clone();
            internal_to_public.insert(internal_name, public_name.clone());
        }
    }

    let mut overloads = HashMap::<String, Vec<OverloadCandidate>>::new();
    for def in defs.iter() {
        let public_name = internal_to_public
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| def.name.clone());
        internal_to_public
            .entry(def.name.clone())
            .or_insert_with(|| public_name.clone());
        overloads
            .entry(public_name)
            .or_default()
            .push(OverloadCandidate {
                internal_name: def.name.clone(),
                signature: FnSignature {
                    params: def.params.iter().map(|p| p.name.clone()).collect(),
                    defaults: def.params.iter().map(|p| p.default.clone()).collect(),
                    param_types: def.params.iter().map(|p| p.ty.clone()).collect(),
                    type_params: def.type_params.clone(),
                    readonly_array_params: HashSet::new(),
                },
            });
    }

    (overloads, internal_to_public)
}
