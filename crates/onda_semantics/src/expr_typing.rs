use std::collections::{HashMap, HashSet};

use onda_frontend::{BuiltinFn, CallArg, Diagnostic, Expr, PrimitiveType};

use crate::builtins::{
    builtin_constant_type, builtin_name, is_builtin_buffer_2d_unsafe_fn, is_builtin_unsafe_data_fn,
    is_float_type, parse_array_len_instance_base, parse_buffer_chans_instance_base,
    parse_buffer_samplerate_instance_base, parse_unsafe_read2_instance_base,
    parse_unsafe_read_instance_base, parse_unsafe_write2_instance_base,
    parse_unsafe_write_instance_base,
};
use crate::decl_symbols::{
    declared_buffer_info, declared_symbol_scalar_type, has_declared_buffer_symbol_info,
    DeclaredSymbolMap,
};
use crate::def_semantics::{can_implicitly_assign, merge_numeric_types};
use crate::{
    is_builtin_array_like_receiver_with_resolver, resolve_struct_field_decl, split_field_path,
    LocalAliasTypes, LocalArrayAliasInfo, ProcNestedArrayState, TypedFieldType, TypedStructField,
};

/// Returns the appropriate type for a literal in an untyped assignment context.
/// Float literals default to F32, int literals fitting in i32 default to I32,
/// larger ints default to I64. This preserves backward-compatible inference for
/// untyped assignments like `x = 0.5` (F32) while typed literals elsewhere use
/// the full-precision F64/I64 types.
pub(crate) fn untyped_literal_type(expr: &Expr) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { value: v, .. } => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        _ => None,
    }
}

/// For untyped first-assignment inference, maps the inferred type of a pure
/// literal expression to its backward-compatible default (F64→F32, I64→I32).
pub(crate) fn effective_untyped_assignment_type(
    expr: &Expr,
    expr_ty: Option<PrimitiveType>,
) -> Option<PrimitiveType> {
    // Bare literals: use the classic F32/I32 defaults
    if let Some(lit_ty) = untyped_literal_type(expr) {
        return Some(lit_ty);
    }
    // Pure numeric expressions (e.g. 0.5 + 0.5, PI * 2.0): narrow F64→F32, I64→I32
    if is_pure_numeric_literal_expr(expr) {
        return expr_ty.map(|ty| match ty {
            PrimitiveType::F64 => PrimitiveType::F32,
            PrimitiveType::I64 => PrimitiveType::I32,
            other => other,
        });
    }
    expr_ty
}

/// Returns true if the expression is a "pure" numeric expression composed
/// entirely of numeric literals, unary ops on literals, and binary ops on
/// literals. Explicit casts (e.g. `i64(1)`) are NOT considered pure literals
/// — the user chose a specific type and implicit narrowing would discard that.
///
/// Used to allow implicit narrowing (F64→F32, I64→I32) at assignment sites
/// when the entire RHS is a compile-time numeric constant expression.
pub(crate) fn is_pure_numeric_literal_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } => true,
        Expr::UnaryBitNot { expr, .. } => is_pure_numeric_literal_expr(expr),
        Expr::Binary { lhs, rhs, .. } => {
            is_pure_numeric_literal_expr(lhs) && is_pure_numeric_literal_expr(rhs)
        }
        // Var references to builtin constants (PI, TWO_PI, SR, BS) are pure.
        Expr::Var { name, .. } => builtin_constant_type(name).is_some(),
        // Builtin function calls (abs, sin, cos, …) with all-pure-literal args.
        Expr::Call { args, .. } => args.iter().all(is_pure_numeric_literal_expr),
        _ => false,
    }
}

/// When one operand of a binary expression is a pure numeric literal expression
/// and the other is not, adapt the literal's inferred type to the non-literal's
/// type. This preserves backward-compatible expression types (e.g. `x_f32 + 0.5`
/// → F32, `acc + sin(0.0)` → F32) while keeping full precision internally.
fn adapt_binary_operand_types(
    lhs: &Expr,
    rhs: &Expr,
    lhs_ty: PrimitiveType,
    rhs_ty: PrimitiveType,
) -> (PrimitiveType, PrimitiveType) {
    let l_pure = is_pure_numeric_literal_expr(lhs);
    let r_pure = is_pure_numeric_literal_expr(rhs);
    match (l_pure, r_pure) {
        // One pure-literal, one non-literal: adapt literal to match the non-literal's type.
        // Only adapt within the numeric domain (don't coerce bools).
        (true, false) if lhs_ty != PrimitiveType::Bool && rhs_ty != PrimitiveType::Bool => {
            (rhs_ty, rhs_ty)
        }
        (false, true) if lhs_ty != PrimitiveType::Bool && rhs_ty != PrimitiveType::Bool => {
            (lhs_ty, lhs_ty)
        }
        // Both pure or neither: keep inferred types, let merge handle it.
        _ => (lhs_ty, rhs_ty),
    }
}

fn merge_integer_types_for_expr(
    expr: &Expr,
    lhs: PrimitiveType,
    rhs: PrimitiveType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "{context} requires integer operands (i32/i64), got {:?} and {:?}",
                    lhs, rhs
                ),
                expr.loc(),
            ));
            None
        }
    }
}

fn is_data_receiver_symbol_for_builtin(
    base: &str,
    declared_symbols: &DeclaredSymbolMap,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
) -> bool {
    local_array_aliases.contains_key(base)
        || is_builtin_array_like_receiver_with_resolver(
            base,
            declared_symbols,
            struct_defs,
            proc_array_roots,
            |root| struct_instances.get(root).map(String::as_str),
        )
}

fn is_buffer_receiver_symbol_for_builtin(base: &str, declared_symbols: &DeclaredSymbolMap) -> bool {
    has_declared_buffer_symbol_info(declared_symbols, base)
}

fn infer_scalar_expr_type_with_proc_arrays(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    local_aliases: &LocalAliasTypes,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    locals: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F64),
        Expr::Int { .. } => Some(PrimitiveType::I64),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::ArrayLiteral { .. } => None,
        Expr::Tuple { .. } => None,
        Expr::Var { name, .. } => {
            if let Some(ty) = builtin_constant_type(name) {
                return Some(ty);
            }
            if let Some((base, field)) = split_field_path(name, errors) {
                let flat = format!("{base}.{field}");
                if let Some(ty) = state_scalars.get(&flat).copied() {
                    return Some(ty);
                }
                if let Some(ty) = declared_symbol_scalar_type(declared_symbols, &flat) {
                    return Some(ty);
                }
                if let Some(struct_name) = struct_instances.get(base) {
                    if let Some(field_decl) =
                        resolve_struct_field_decl(struct_name, field, struct_defs)
                    {
                        return Some(match field_decl.ty {
                            TypedFieldType::Scalar(prim) => prim,
                            TypedFieldType::Struct | TypedFieldType::Tuple(_) => return None,
                            TypedFieldType::Array(_) => PrimitiveType::F32,
                        });
                    }
                }
                if let Some(ty) = declared_symbol_scalar_type(declared_symbols, &field) {
                    return Some(ty);
                }
                None
            } else if let Some(ty) = state_scalars.get(name).copied() {
                Some(ty)
            } else if let Some(ty) = local_aliases.get(name).copied() {
                Some(ty)
            } else if locals.contains(name) {
                Some(PrimitiveType::I32)
            } else if input_names.contains(name) {
                Some(
                    declared_symbol_scalar_type(declared_symbols, name)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else if output_names.contains(name) {
                Some(
                    declared_symbol_scalar_type(declared_symbols, name)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else if param_names.contains(name) {
                Some(
                    declared_symbol_scalar_type(declared_symbols, name)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else {
                None
            }
        }
        Expr::Index { base, .. } => {
            // Port index access: ins[i], outs[i], kouts[i], params[i], kins[i]
            // These are validated upstream; here we just return the uniform type.
            // The fallback at the end of this arm returns F32 which covers the common case,
            // but for completeness we check input/output/param types explicitly.
            if base == "ins" {
                let ty = input_names
                    .iter()
                    .find_map(|n| declared_symbol_scalar_type(declared_symbols, n))
                    .unwrap_or(PrimitiveType::F32);
                return Some(ty);
            }
            if base == "outs" || base == "kouts" {
                return Some(PrimitiveType::F32);
            }
            if base == "params" || base == "kins" {
                let ty = param_names
                    .iter()
                    .find_map(|n| declared_symbol_scalar_type(declared_symbols, n))
                    .unwrap_or(PrimitiveType::F32);
                return Some(ty);
            }
            if let Some(alias) = local_array_aliases.get(base) {
                if alias.elem_struct.is_none() {
                    return Some(alias.elem_ty);
                }
            }
            if let Some((root, field)) = split_field_path(base, errors) {
                if let Some(struct_name) = struct_instances.get(root) {
                    if let Some(field_decl) =
                        resolve_struct_field_decl(struct_name, field, struct_defs)
                    {
                        if let TypedFieldType::Array(_) = field_decl.ty {
                            if let Some(elem_ty) = field_decl.array_elem_ty {
                                return Some(elem_ty);
                            }
                        }
                    }
                }
                // Proc-lowered state fields are often addressed as `self.field[...]` while
                // declared element metadata is keyed by bare field name.
                if let Some(ty) = declared_symbol_scalar_type(declared_symbols, &field) {
                    return Some(ty);
                }
                if let Some((ty, _)) = declared_buffer_info(declared_symbols, &field) {
                    return Some(ty);
                }
            }
            if let Some(ty) = declared_symbol_scalar_type(declared_symbols, base) {
                return Some(ty);
            }
            if let Some((ty, _)) = declared_buffer_info(declared_symbols, base) {
                return Some(ty);
            }
            Some(PrimitiveType::F32)
        }
        Expr::Slice { .. } => None,
        Expr::ArrayCtor { .. } => None,
        Expr::Cast { to, expr, .. } => {
            let _ = infer_scalar_expr_type_with_proc_arrays(
                expr,
                state_scalars,
                declared_symbols,
                local_aliases,
                local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                proc_array_roots,
                errors,
            )?;
            Some(*to)
        }
        Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr, .. } => {
            let inner = infer_scalar_expr_type_with_proc_arrays(
                expr,
                state_scalars,
                declared_symbols,
                local_aliases,
                local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                proc_array_roots,
                errors,
            )?;
            merge_integer_types_for_expr(expr, inner, inner, "bitwise not expression", errors)
        }
        Expr::Call { func, args, .. } => {
            let arg_types = args
                .iter()
                .map(|arg| {
                    infer_scalar_expr_type_with_proc_arrays(
                        arg,
                        state_scalars,
                        declared_symbols,
                        local_aliases,
                        local_array_aliases,
                        locals,
                        input_names,
                        output_names,
                        param_names,
                        struct_instances,
                        struct_defs,
                        proc_array_roots,
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
                        errors.push(Diagnostic::semantic_span(
                            "builtin 'abs' requires numeric argument (bool is not supported)",
                            expr.loc(),
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
                            errors.push(Diagnostic::semantic_span(
                                "builtin 'pow' requires numeric arguments (bool is not supported)",
                                expr.loc(),
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
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "builtin '{}' requires float arguments (f32/f64), got {:?}",
                                    builtin_name(*func),
                                    ty
                                ),
                                expr.loc(),
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
            if let Some(ty) = declared_symbol_scalar_type(declared_symbols, name) {
                return Some(ty);
            }
            if let Some(base) = parse_array_len_instance_base(name) {
                if is_data_receiver_symbol_for_builtin(
                    base,
                    declared_symbols,
                    local_array_aliases,
                    struct_instances,
                    struct_defs,
                    proc_array_roots,
                ) || is_buffer_receiver_symbol_for_builtin(base, declared_symbols)
                {
                    return Some(PrimitiveType::I32);
                }
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                if is_buffer_receiver_symbol_for_builtin(base, declared_symbols) {
                    return Some(PrimitiveType::I32);
                }
            }
            if let Some(base) = parse_buffer_samplerate_instance_base(name) {
                if is_buffer_receiver_symbol_for_builtin(base, declared_symbols) {
                    return Some(PrimitiveType::F32);
                }
            }
            if let Some(base) = parse_unsafe_read_instance_base(name)
                .or_else(|| parse_unsafe_write_instance_base(name))
            {
                if !is_data_receiver_symbol_for_builtin(
                    base,
                    declared_symbols,
                    local_array_aliases,
                    struct_instances,
                    struct_defs,
                    proc_array_roots,
                ) && !is_buffer_receiver_symbol_for_builtin(base, declared_symbols)
                {
                    return Some(PrimitiveType::F32);
                }
                if let Some(alias) = local_array_aliases.get(base) {
                    if alias.elem_struct.is_none() {
                        return Some(alias.elem_ty);
                    }
                }
                if let Some(ty) = declared_symbol_scalar_type(declared_symbols, base) {
                    return Some(ty);
                }
                if let Some((ty, _)) = declared_buffer_info(declared_symbols, base) {
                    return Some(ty);
                }
                return Some(PrimitiveType::F32);
            }
            if let Some(base) = parse_unsafe_read2_instance_base(name)
                .or_else(|| parse_unsafe_write2_instance_base(name))
            {
                if let Some((ty, _)) = declared_buffer_info(declared_symbols, base) {
                    return Some(ty);
                }
                return Some(PrimitiveType::F32);
            }
            if is_builtin_buffer_2d_unsafe_fn(name) {
                if let Some(CallArg {
                    expr: Expr::Var { name: base, .. },
                    ..
                }) = args.first()
                {
                    if let Some((ty, _)) = declared_buffer_info(declared_symbols, base) {
                        return Some(ty);
                    }
                }
                return Some(PrimitiveType::F32);
            }
            if is_builtin_unsafe_data_fn(name) {
                if let Some(CallArg {
                    expr: Expr::Var { name: base, .. },
                    ..
                }) = args.first()
                {
                    if let Some(alias) = local_array_aliases.get(base) {
                        if alias.elem_struct.is_none() {
                            return Some(alias.elem_ty);
                        }
                    }
                }
            }
            Some(PrimitiveType::F32)
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let l = infer_scalar_expr_type_with_proc_arrays(
                lhs,
                state_scalars,
                declared_symbols,
                local_aliases,
                local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                proc_array_roots,
                errors,
            );
            let r = infer_scalar_expr_type_with_proc_arrays(
                rhs,
                state_scalars,
                declared_symbols,
                local_aliases,
                local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                proc_array_roots,
                errors,
            );
            if let (Some(l), Some(r)) = (l, r) {
                // Adapt literal types to the non-literal operand's type so that
                // e.g. `x_f32 + 0.5` stays F32 rather than widening to F64.
                let (el, er) = adapt_binary_operand_types(lhs, rhs, l, r);
                match op {
                    onda_frontend::BinaryOp::BitAnd
                    | onda_frontend::BinaryOp::BitOr
                    | onda_frontend::BinaryOp::BitXor
                    | onda_frontend::BinaryOp::ShiftLeft
                    | onda_frontend::BinaryOp::ShiftRight => {
                        merge_integer_types_for_expr(expr, el, er, "bitwise expression", errors)
                    }
                    _ => merge_numeric_types(el, er, "binary expression", errors),
                }
            } else {
                None
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn infer_expr_type_for_semantics(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    param_structs: Option<&HashMap<String, String>>,
    locals: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    let empty_proc_array_roots = HashMap::<String, ProcNestedArrayState>::new();
    infer_expr_type_for_semantics_with_proc_arrays(
        expr,
        state_scalars,
        declared_symbols,
        param_structs,
        locals,
        input_names,
        output_names,
        param_names,
        struct_instances,
        struct_defs,
        &empty_proc_array_roots,
        errors,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn infer_expr_type_for_semantics_with_proc_arrays(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    param_structs: Option<&HashMap<String, String>>,
    locals: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    let empty_local_aliases = LocalAliasTypes::new();
    let empty_local_data_aliases = HashMap::<String, LocalArrayAliasInfo>::new();
    infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
        expr,
        state_scalars,
        declared_symbols,
        param_structs,
        &empty_local_aliases,
        &empty_local_data_aliases,
        locals,
        input_names,
        output_names,
        param_names,
        struct_instances,
        struct_defs,
        proc_array_roots,
        errors,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    param_structs: Option<&HashMap<String, String>>,
    local_aliases: &LocalAliasTypes,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    locals: &HashSet<String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
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

    infer_scalar_expr_type_with_proc_arrays(
        expr,
        state_scalars,
        &declared_symbols,
        local_aliases,
        local_array_aliases,
        locals,
        input_names,
        output_names,
        param_names,
        struct_instance_ctx,
        struct_defs,
        proc_array_roots,
        errors,
    )
}

pub(crate) fn require_expr_assignable_type(
    expr: &Expr,
    src: Option<PrimitiveType>,
    dst: PrimitiveType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(src) = src {
        if src != dst && !can_implicitly_assign(src, dst) {
            // Pure numeric literal expressions (literals, builtin consts, and
            // arithmetic on them) may narrow at the assignment site so that
            // full precision is retained internally while the language surface
            // still defaults to F32/I32.
            if is_pure_numeric_literal_expr(expr) {
                match (src, dst) {
                    // Same-category narrowing for literals:
                    (PrimitiveType::F64, PrimitiveType::F32)
                    | (PrimitiveType::I64, PrimitiveType::I32)
                    // Int literal to float (e.g. `x: f32 = 5`):
                    | (PrimitiveType::I64, PrimitiveType::F32) => return,
                    _ => {}
                }
            }
            errors.push(Diagnostic::semantic_span(
                format!(
                    "{context} type mismatch: cannot assign {:?} to {:?}",
                    src, dst
                ),
                expr.loc(),
            ));
        }
    }
}

pub(crate) fn require_expr_numeric_type(
    expr: &Expr,
    ty: Option<PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(ty) = ty {
        if !matches!(
            ty,
            PrimitiveType::F32 | PrimitiveType::F64 | PrimitiveType::I32 | PrimitiveType::I64
        ) {
            errors.push(Diagnostic::semantic_span(
                format!("{context} requires numeric type, got {:?}", ty),
                expr.loc(),
            ));
        }
    }
}

pub(crate) fn require_expr_bool_type(
    expr: &Expr,
    ty: Option<PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(ty) = ty {
        if ty != PrimitiveType::Bool {
            errors.push(Diagnostic::semantic_span(
                format!("{context} requires bool type, got {:?}", ty),
                expr.loc(),
            ));
        }
    }
}
