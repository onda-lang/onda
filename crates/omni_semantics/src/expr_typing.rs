use std::collections::{HashMap, HashSet};

use omni_frontend::{BuiltinFn, CallArg, Diagnostic, Expr, PrimitiveType};

use crate::builtins::{
    builtin_constant_type, builtin_name, is_builtin_unsafe_data_fn, is_float_type,
    is_internal_buffer_2d_fn, parse_array_len_instance_base, parse_buffer_chans_instance_base,
    parse_unsafe_read_instance_base, parse_unsafe_write_instance_base,
};
use crate::decl_symbols::{
    declared_buffer_info, declared_symbol_scalar_type, has_declared_buffer_symbol_info,
    is_declared_data_array_symbol, DeclaredSymbolMap,
};
use crate::def_inference::{can_implicitly_assign, merge_numeric_types};
use crate::{
    resolve_struct_field_decl, split_field_path, LocalArrayAliasInfo, TypedFieldType,
    TypedStructField,
};

fn merge_integer_types_for_expr(
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
            errors.push(Diagnostic::semantic(
                format!(
                    "{context} requires integer operands (i32/i64), got {:?} and {:?}",
                    lhs, rhs
                ),
                0,
                0,
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
) -> bool {
    if local_array_aliases.contains_key(base)
        || is_declared_data_array_symbol(declared_symbols, base)
    {
        return true;
    }
    if let Some((root, field)) = split_simple_field_path(base) {
        if let Some(struct_name) = struct_instances.get(root) {
            if let Some(field_decl) = resolve_struct_field_decl(struct_name, field, struct_defs) {
                return matches!(field_decl.ty, TypedFieldType::Array(_));
            }
        }
    }
    false
}

fn is_buffer_receiver_symbol_for_builtin(base: &str, declared_symbols: &DeclaredSymbolMap) -> bool {
    has_declared_buffer_symbol_info(declared_symbols, base)
}

fn split_simple_field_path(name: &str) -> Option<(&str, &str)> {
    crate::split_root_field_path(name)
}

pub(crate) fn infer_scalar_expr_type(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
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
                            TypedFieldType::Struct => return None,
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
        Expr::ArrayCtor { .. } => None,
        Expr::Cast { to, expr } => {
            let _ = infer_scalar_expr_type(
                expr,
                state_scalars,
                declared_symbols,
                local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            )?;
            Some(*to)
        }
        Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr } => {
            let inner = infer_scalar_expr_type(
                expr,
                state_scalars,
                declared_symbols,
                local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            )?;
            merge_integer_types_for_expr(inner, inner, "bitwise not expression", errors)
        }
        Expr::Call { func, args } => {
            let arg_types = args
                .iter()
                .map(|arg| {
                    infer_scalar_expr_type(
                        arg,
                        state_scalars,
                        declared_symbols,
                        local_array_aliases,
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
            if let Some(base) = parse_unsafe_read_instance_base(name)
                .or_else(|| parse_unsafe_write_instance_base(name))
            {
                if !is_data_receiver_symbol_for_builtin(
                    base,
                    declared_symbols,
                    local_array_aliases,
                    struct_instances,
                    struct_defs,
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
            if is_internal_buffer_2d_fn(name) {
                if let Some(CallArg {
                    expr: Expr::Var(base),
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
                    expr: Expr::Var(base),
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
        Expr::Binary { op, lhs, rhs } => {
            let l = infer_scalar_expr_type(
                lhs,
                state_scalars,
                declared_symbols,
                local_array_aliases,
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
                declared_symbols,
                local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                struct_instances,
                struct_defs,
                errors,
            );
            if let (Some(l), Some(r)) = (l, r) {
                match op {
                    omni_frontend::BinaryOp::BitAnd
                    | omni_frontend::BinaryOp::BitOr
                    | omni_frontend::BinaryOp::BitXor
                    | omni_frontend::BinaryOp::ShiftLeft
                    | omni_frontend::BinaryOp::ShiftRight => {
                        merge_integer_types_for_expr(l, r, "bitwise expression", errors)
                    }
                    _ => merge_numeric_types(l, r, "binary expression", errors),
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
    let empty_local_data_aliases = HashMap::<String, LocalArrayAliasInfo>::new();
    infer_expr_type_for_semantics_with_local_data(
        expr,
        state_scalars,
        declared_symbols,
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
pub(crate) fn infer_expr_type_for_semantics_with_local_data(
    expr: &Expr,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    param_structs: Option<&HashMap<String, String>>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
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
        &declared_symbols,
        local_array_aliases,
        locals,
        input_names,
        output_names,
        param_names,
        struct_instance_ctx,
        struct_defs,
        errors,
    )
}

pub(crate) fn require_assignable_type(
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

pub(crate) fn require_numeric_type(
    ty: Option<PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
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

pub(crate) fn require_bool_type(
    ty: Option<PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
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
