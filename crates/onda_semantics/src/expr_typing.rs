use std::collections::{HashMap, HashSet};

use onda_frontend::{BuiltinFn, Diagnostic, Expr, PrimitiveType};

use crate::builtins::{
    builtin_constant_type, builtin_name, is_builtin_buffer_write_function_name, is_float_type,
    is_internal_buffer_2d_fn, parse_array_len_instance_base, parse_buffer_chans_instance_base,
    parse_buffer_samplerate_instance_base, ARRAY_LEN_METHOD, BUFFER_CHANS_METHOD,
    BUFFER_SAMPLERATE_METHOD,
};
use crate::decl_symbols::{
    declared_buffer_info, declared_symbol_scalar_type, has_declared_buffer_symbol_info,
    DeclaredSymbolMap,
};
use crate::def_semantics::{can_implicitly_assign, merge_numeric_types};
use crate::internal_names::PROC_INDEX_CALL_SENTINEL;
use crate::{
    is_builtin_array_like_receiver_with_resolver, resolve_struct_field_decl, split_field_path,
    LocalAliasTypes, LocalArrayAliasInfo, ProcNestedArrayState, TypedFieldType, TypedStructField,
};

/// Returns the appropriate type for a literal in an untyped assignment context.
/// Float literals default to F32, int literals fitting in i32 default to I32,
/// and larger ints default to I64. Typed literals elsewhere retain their
/// full-precision F64/I64 representation until a context selects a type.
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
/// literal expression to its ordinary first-assignment default. Float
/// expressions default to F32. Integer literals are handled above with an
/// exact range check. Pure integer expressions default to I32 only when none
/// of their literal leaves already requires I64.
pub(crate) fn effective_untyped_assignment_type(
    expr: &Expr,
    expr_ty: Option<PrimitiveType>,
) -> Option<PrimitiveType> {
    // Bare literals use the ordinary F32/I32 defaults.
    if let Some(lit_ty) = untyped_literal_type(expr) {
        return Some(lit_ty);
    }
    // Pure numeric expressions (e.g. 0.5 + 0.5, PI * 2.0) use the ordinary
    // F32/I32 defaults. Preserve I64 when a literal leaf itself is outside the
    // i32 range; such a value must not be made narrow merely by wrapping it in
    // a larger constant expression.
    if is_pure_numeric_literal_expr(expr) {
        return expr_ty.map(|ty| match ty {
            PrimitiveType::F64 => PrimitiveType::F32,
            PrimitiveType::I64 if !contains_wide_integer_literal(expr) => PrimitiveType::I32,
            other => other,
        });
    }
    expr_ty
}

fn contains_wide_integer_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Int { value, .. } => *value < i32::MIN as i64 || *value > i32::MAX as i64,
        Expr::Binary { lhs, rhs, .. } => {
            contains_wide_integer_literal(lhs) || contains_wide_integer_literal(rhs)
        }
        Expr::UnaryBitNot { expr, .. } => contains_wide_integer_literal(expr),
        Expr::Call { args, .. } => args.iter().any(contains_wide_integer_literal),
        _ => false,
    }
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
/// type (for example, `x_f32 + 0.5` and `acc + sin(0.0)` stay F32) while keeping
/// full precision until that context is known.
pub(crate) fn adapt_binary_operand_types(
    lhs: &Expr,
    rhs: &Expr,
    lhs_ty: PrimitiveType,
    rhs_ty: PrimitiveType,
) -> (PrimitiveType, PrimitiveType) {
    let l_pure = is_pure_numeric_literal_expr(lhs);
    let r_pure = is_pure_numeric_literal_expr(rhs);
    match (l_pure, r_pure) {
        // One pure-literal, one non-literal: adapt literal to match the non-literal's type.
        // Never narrow a floating literal to an integer variable: `i / 10.0`
        // is a floating expression. Integer literals may still adapt to a
        // floating variable, and literals within one numeric family retain the
        // non-literal width.
        (true, false) if lhs_ty != PrimitiveType::Bool && rhs_ty != PrimitiveType::Bool => {
            if is_float_type(lhs_ty) && !is_float_type(rhs_ty) {
                (
                    effective_untyped_assignment_type(lhs, Some(lhs_ty)).unwrap_or(lhs_ty),
                    rhs_ty,
                )
            } else {
                (rhs_ty, rhs_ty)
            }
        }
        (false, true) if lhs_ty != PrimitiveType::Bool && rhs_ty != PrimitiveType::Bool => {
            if is_float_type(rhs_ty) && !is_float_type(lhs_ty) {
                (
                    lhs_ty,
                    effective_untyped_assignment_type(rhs, Some(rhs_ty)).unwrap_or(rhs_ty),
                )
            } else {
                (lhs_ty, lhs_ty)
            }
        }
        // Both pure or neither: keep inferred types, let merge handle it.
        _ => (lhs_ty, rhs_ty),
    }
}

/// Adapts pure numeric arguments to the concrete width selected by the
/// non-literal arguments of the same builtin call.
///
/// This is the n-ary counterpart of [`adapt_binary_operand_types`]. It keeps
/// calls such as `max(x_f32, 0.0)` and `fma(x_f32, 1.0, 0.0)` at `f32` while
/// retaining the normal numeric merge when several concrete operands disagree.
pub(crate) fn adapt_numeric_argument_types(
    args: &[Expr],
    arg_types: &[PrimitiveType],
) -> Vec<PrimitiveType> {
    if args.len() != arg_types.len() {
        return arg_types.to_vec();
    }

    let mut concrete_ty = None;
    for (arg, ty) in args.iter().zip(arg_types.iter().copied()) {
        if is_pure_numeric_literal_expr(arg) {
            continue;
        }
        concrete_ty = Some(match concrete_ty {
            Some(current) => {
                let Some(merged) = merge_numeric_types_without_diagnostics(current, ty) else {
                    return arg_types.to_vec();
                };
                merged
            }
            None if ty != PrimitiveType::Bool => ty,
            None => return arg_types.to_vec(),
        });
    }

    let Some(concrete_ty) = concrete_ty else {
        return arg_types.to_vec();
    };

    args.iter()
        .zip(arg_types.iter().copied())
        .map(|(arg, ty)| {
            if !is_pure_numeric_literal_expr(arg)
                || ty == PrimitiveType::Bool
                || concrete_ty == PrimitiveType::Bool
            {
                return ty;
            }
            if is_float_type(ty) && !is_float_type(concrete_ty) {
                effective_untyped_assignment_type(arg, Some(ty)).unwrap_or(ty)
            } else {
                concrete_ty
            }
        })
        .collect()
}

pub(crate) fn merge_numeric_types_without_diagnostics(
    lhs: PrimitiveType,
    rhs: PrimitiveType,
) -> Option<PrimitiveType> {
    use PrimitiveType::{Bool, F32, F64, I32, I64};
    match (lhs, rhs) {
        (Bool, _) | (_, Bool) => None,
        (F64, _) | (_, F64) => Some(F64),
        (F32, _) | (_, F32) => Some(F32),
        (I64, _) | (_, I64) => Some(I64),
        (I32, I32) => Some(I32),
    }
}

pub(crate) fn intrinsic_result_type(
    function: BuiltinFn,
    adapted_arg_types: &[PrimitiveType],
) -> Option<PrimitiveType> {
    match function {
        BuiltinFn::Abs => adapted_arg_types
            .first()
            .copied()
            .filter(|ty| *ty != PrimitiveType::Bool),
        BuiltinFn::Min | BuiltinFn::Max => merge_numeric_types_without_diagnostics(
            *adapted_arg_types.first()?,
            *adapted_arg_types.get(1)?,
        ),
        BuiltinFn::RangeClamp
        | BuiltinFn::BindingCountClamp
        | BuiltinFn::BindingRangeClamp
        | BuiltinFn::BindingRangeInclusiveClamp
        | BuiltinFn::RangeWrap
        | BuiltinFn::BindingCountWrap
        | BuiltinFn::BindingRangeWrap
        | BuiltinFn::BindingRangeInclusiveWrap => {
            let value = *adapted_arg_types.first()?;
            if matches!(
                function,
                BuiltinFn::RangeWrap
                    | BuiltinFn::BindingCountWrap
                    | BuiltinFn::BindingRangeWrap
                    | BuiltinFn::BindingRangeInclusiveWrap
            ) && !matches!(value, PrimitiveType::I32 | PrimitiveType::I64)
            {
                return None;
            }
            adapted_arg_types
                .iter()
                .copied()
                .skip(1)
                .try_fold(value, merge_numeric_types_without_diagnostics)
        }
        BuiltinFn::Pow
        | BuiltinFn::Sin
        | BuiltinFn::Cos
        | BuiltinFn::Tan
        | BuiltinFn::Tanh
        | BuiltinFn::Atan
        | BuiltinFn::Atan2
        | BuiltinFn::Exp
        | BuiltinFn::Log
        | BuiltinFn::Sqrt
        | BuiltinFn::Floor
        | BuiltinFn::Ceil
        | BuiltinFn::Round
        | BuiltinFn::Trunc
        | BuiltinFn::Fma => Some(if adapted_arg_types.contains(&PrimitiveType::F64) {
            PrimitiveType::F64
        } else {
            PrimitiveType::F32
        }),
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
            let lexical_root = name.split('.').next().unwrap_or(name);
            if locals.contains(lexical_root) {
                return (name == lexical_root).then_some(PrimitiveType::I32);
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
                if let Some(ty) = declared_symbol_scalar_type(declared_symbols, field) {
                    return Some(ty);
                }
                None
            } else if let Some(ty) = state_scalars.get(name).copied() {
                Some(ty)
            } else if let Some(ty) = local_aliases.get(name).copied() {
                Some(ty)
            } else if input_names.contains(name)
                || output_names.contains(name)
                || param_names.contains(name)
            {
                Some(
                    declared_symbol_scalar_type(declared_symbols, name)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else {
                None
            }
        }
        Expr::Index { base, index, .. } => {
            if locals.contains(base.split('.').next().unwrap_or(base)) {
                return None;
            }
            if let Expr::Int { value, .. } = index.as_ref() {
                if let Some(ty) = state_scalars
                    .get(&format!("{base}.__{value}"))
                    .or_else(|| state_scalars.get(&format!("{base}[{value}]")))
                    .or_else(|| local_aliases.get(&format!("{base}[{value}]")))
                    .copied()
                {
                    return Some(ty);
                }
            }
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
                        match &field_decl.ty {
                            TypedFieldType::Array(_) => {
                                if let Some(elem_ty) = field_decl.array_elem_ty {
                                    return Some(elem_ty);
                                }
                            }
                            TypedFieldType::Tuple(elem_types) => {
                                let Expr::Int { value, .. } = index.as_ref() else {
                                    return None;
                                };
                                return usize::try_from(*value)
                                    .ok()
                                    .and_then(|index| elem_types.get(index).copied());
                            }
                            TypedFieldType::Scalar(_) | TypedFieldType::Struct => {}
                        }
                    }
                }
                // Proc-lowered state fields are often addressed as `self.field[...]` while
                // declared element metadata is keyed by bare field name.
                if let Some(ty) = declared_symbol_scalar_type(declared_symbols, field) {
                    return Some(ty);
                }
                if let Some((ty, _)) = declared_buffer_info(declared_symbols, field) {
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
            let arg_types = adapt_numeric_argument_types(args, &arg_types);

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
                BuiltinFn::RangeClamp
                | BuiltinFn::BindingCountClamp
                | BuiltinFn::BindingRangeClamp
                | BuiltinFn::BindingRangeInclusiveClamp
                | BuiltinFn::RangeWrap
                | BuiltinFn::BindingCountWrap
                | BuiltinFn::BindingRangeWrap
                | BuiltinFn::BindingRangeInclusiveWrap => {
                    let mut merged = arg_types.first().copied().unwrap_or(PrimitiveType::F32);
                    for rhs in arg_types.iter().copied().skip(1) {
                        merged = merge_numeric_types(
                            merged,
                            rhs,
                            "compiler-generated integer range normalization",
                            errors,
                        )?;
                    }
                    if matches!(
                        func,
                        BuiltinFn::RangeWrap
                            | BuiltinFn::BindingCountWrap
                            | BuiltinFn::BindingRangeWrap
                            | BuiltinFn::BindingRangeInclusiveWrap
                    ) && !matches!(merged, PrimitiveType::I32 | PrimitiveType::I64)
                    {
                        errors.push(Diagnostic::semantic_span(
                            "wrapped binding ranges require i32 or i64 operands",
                            expr.loc(),
                        ));
                        None
                    } else {
                        Some(merged)
                    }
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
                    Some(if arg_types.contains(&PrimitiveType::F64) {
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
                    Some(if arg_types.contains(&PrimitiveType::F64) {
                        PrimitiveType::F64
                    } else {
                        PrimitiveType::F32
                    })
                }
            }
        }
        Expr::UserCall { name, args, .. } => {
            if let Some(method) = name
                .strip_prefix(PROC_INDEX_CALL_SENTINEL)
                .and_then(|suffix| suffix.strip_prefix('.'))
            {
                if matches!(method, ARRAY_LEN_METHOD | BUFFER_CHANS_METHOD) {
                    return Some(PrimitiveType::I32);
                }
                if method == BUFFER_SAMPLERATE_METHOD {
                    return Some(PrimitiveType::F32);
                }
            }
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
            if is_internal_buffer_2d_fn(name) {
                if is_builtin_buffer_write_function_name(name) {
                    return None;
                }
                if let Some(first) = args.first() {
                    let base = match &first.expr {
                        Expr::Var { name: base, .. } | Expr::Index { base, .. } => base,
                        _ => return Some(PrimitiveType::F32),
                    };
                    if let Some((ty, _)) = declared_buffer_info(declared_symbols, base) {
                        return Some(ty);
                    }
                    if let Some(ty) = declared_symbol_scalar_type(declared_symbols, base) {
                        return Some(ty);
                    }
                    if let Some(alias) = local_array_aliases.get(base) {
                        return Some(alias.elem_ty);
                    }
                    let surface_names = match base.as_str() {
                        "ins" => Some(input_names),
                        "outs" | "kouts" => Some(output_names),
                        "params" | "kins" => Some(param_names),
                        _ => None,
                    };
                    if let Some(surface_names) = surface_names {
                        return Some(
                            surface_names
                                .iter()
                                .find_map(|name| {
                                    declared_symbol_scalar_type(declared_symbols, name)
                                })
                                .unwrap_or(PrimitiveType::F32),
                        );
                    }
                }
                return Some(PrimitiveType::F32);
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
        declared_symbols,
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
        if !can_assign_expr_to_type(expr, src, dst) {
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

/// Returns whether semantic analysis may adapt `expr` from `src` to `dst`
/// without an explicit source cast.
///
/// Concrete runtime values follow the ordinary widening relation. Pure
/// numeric literal expressions additionally adapt once at their contextual
/// boundary, retaining their wide compile-time representation until then.
pub(crate) fn can_assign_expr_to_type(expr: &Expr, src: PrimitiveType, dst: PrimitiveType) -> bool {
    if src == dst || can_implicitly_assign(src, dst) {
        return true;
    }
    if !is_pure_numeric_literal_expr(expr) {
        return false;
    }
    matches!(
        (src, dst),
        // Same-category contextual literal narrowing.
        (PrimitiveType::F64, PrimitiveType::F32)
            | (PrimitiveType::I64, PrimitiveType::I32)
            // Integer literal to floating context.
            | (PrimitiveType::I64, PrimitiveType::F32)
    )
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
