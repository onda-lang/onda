use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use onda_frontend::{BuiltinFn, Diagnostic, Expr, PrimitiveType};

use crate::builtins::{
    builtin_constant_type, builtin_instance_method_return_type, builtin_name,
    is_builtin_buffer_write_function_name, is_float_type, is_internal_buffer_2d_fn,
    parse_array_len_instance_base, parse_buffer_bound_instance_base,
    parse_buffer_chans_instance_base, parse_buffer_samplerate_instance_base,
};
use crate::decl_symbols::{
    declared_buffer_info, declared_symbol_scalar_type, has_declared_buffer_symbol_info,
    DeclaredSymbolMap,
};
use crate::def_semantics::{can_implicitly_assign, merge_numeric_types};
use crate::expr_analysis::{has_lexical_root_binding, has_scalar_value_binding};
use crate::internal_names::PROC_INDEX_CALL_SENTINEL;
use crate::{
    is_builtin_array_like_receiver_with_resolver, resolve_flattened_struct_array_leaf_type,
    resolve_struct_field_decl, split_field_path, LocalAliasTypes, LocalArrayAliasInfo,
    ProcNestedArrayState, TypedFieldType, TypedStructField,
};

/// Returns the appropriate type for a literal in an untyped assignment context.
/// Float literals default to F32, int literals fitting in i32 default to I32,
/// and larger ints default to I64. Folded integer constants retain their type.
/// Typed literals elsewhere retain their
/// full-precision F64/I64 representation until a context selects a type.
pub(crate) fn untyped_literal_type(expr: &Expr) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int {
            const_ty: Some(ty), ..
        } => Some(*ty),
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
/// of their literal leaves or referenced constants requires I64.
pub(crate) fn effective_untyped_assignment_type(
    expr: &Expr,
    expr_ty: Option<PrimitiveType>,
    constants: &DeclaredSymbolMap,
) -> Option<PrimitiveType> {
    // Source literals use ordinary defaults; folded integer constants retain their type.
    if let Some(lit_ty) = untyped_literal_type(expr) {
        return Some(lit_ty);
    }
    // Pure numeric expressions (e.g. 0.5 + 0.5, PI * 2.0) use the ordinary
    // F32/I32 defaults. Preserve I64 for wide literals and I64 constants,
    // including folded constants, independently of when their values evaluate.
    if is_pure_numeric_literal_expr(expr, constants) {
        return expr_ty.map(|ty| match ty {
            PrimitiveType::F64 => PrimitiveType::F32,
            PrimitiveType::I64 if !requires_wide_integer_type(expr, constants) => {
                PrimitiveType::I32
            }
            other => other,
        });
    }
    expr_ty
}

/// Pure numerics need no symbol environment to select their ordinary defaults.
pub(crate) fn default_numeric_literal_type(expr: &Expr) -> Option<PrimitiveType> {
    let constants = DeclaredSymbolMap::new();
    if !is_pure_numeric_literal_expr(expr, &constants) {
        return None;
    }
    let inferred = if crate::builtins::can_eval_const_expr_exact_int(expr) {
        PrimitiveType::I64
    } else {
        PrimitiveType::F64
    };
    effective_untyped_assignment_type(expr, Some(inferred), &constants)
}

// Integer constants retain their width through both metadata and value folding.
fn requires_wide_integer_type(expr: &Expr, constants: &DeclaredSymbolMap) -> bool {
    expr.walk().any(|expr| match expr {
        Expr::Int {
            const_ty: Some(PrimitiveType::I64),
            ..
        } => true,
        Expr::Int { value, .. } => i32::try_from(*value).is_err(),
        Expr::Var { name, .. } => matches!(
            constants.get(name),
            Some(crate::decl_symbols::DeclaredSymbolInfo::Constant {
                ty: PrimitiveType::I64,
                ..
            })
        ),
        Expr::Index { base, .. } => matches!(
            constants.get(base),
            Some(crate::decl_symbols::DeclaredSymbolInfo::ConstArray {
                elem_ty: PrimitiveType::I64
            })
        ),
        _ => false,
    })
}

/// Returns true if the expression is a "pure" numeric expression composed
/// entirely of numeric literals, unary ops on literals, and binary ops on
/// literals. Explicit casts (e.g. `i64(1)`) are NOT considered pure literals
/// — the user chose a specific type and implicit narrowing would discard that.
///
/// Used to allow implicit narrowing (F64→F32, I64→I32) at assignment sites
/// when the entire RHS is a compile-time numeric constant expression.
pub(crate) fn is_pure_numeric_literal_expr(expr: &Expr, constants: &DeclaredSymbolMap) -> bool {
    let mut pending = vec![expr];
    while let Some(expr) = pending.pop() {
        match expr {
            Expr::Number { .. } | Expr::Int { .. } => {}
            Expr::UnaryBitNot { .. } | Expr::Binary { .. } | Expr::Call { .. } => {
                expr.children(&mut pending)
            }
            Expr::Var { name, .. }
                if builtin_constant_type(name).is_some()
                    || matches!(
                        constants.get(name),
                        Some(crate::decl_symbols::DeclaredSymbolInfo::Constant {
                            ty: PrimitiveType::F64 | PrimitiveType::I64,
                            ..
                        })
                    ) => {}
            Expr::Index { base, index, .. }
                if matches!(
                    constants.get(base),
                    Some(crate::decl_symbols::DeclaredSymbolInfo::ConstArray {
                        elem_ty: PrimitiveType::F64 | PrimitiveType::I64
                    })
                ) && crate::builtins::can_eval_const_expr_exact_int_with_symbols(
                    index, constants,
                ) => {}
            _ => return false,
        }
    }
    true
}

#[derive(Clone, Copy)]
pub(crate) enum ScalarConstKind {
    Literal(PrimitiveType),
    Concrete,
}

impl ScalarConstKind {
    pub(crate) fn literal_type(self) -> Option<PrimitiveType> {
        match self {
            Self::Literal(ty) => Some(ty),
            Self::Concrete => None,
        }
    }
}

/// Classify closed scalar expressions once, bottom-up. Literal arithmetic
/// waits for a destination or concrete peer to supply its width; other closed
/// trees preserve their own types. Lowering evaluates each maximal constant
/// subtree without rescanning nested trees.
pub(crate) fn scalar_const_kinds<'a>(
    expr: &'a Expr,
    descend: impl FnMut(&'a Expr, &mut Vec<&'a Expr>),
) -> HashMap<*const Expr, ScalarConstKind> {
    let mut constants = HashMap::new();
    expr.try_fold(
        descend,
        |node, children| -> Result<Option<ScalarConstKind>, std::convert::Infallible> {
            let closed = match node {
                Expr::Bool { .. } => true,
                Expr::Binary { .. }
                | Expr::Compare { .. }
                | Expr::Cast { .. }
                | Expr::UnaryNot { .. }
                | Expr::UnaryBitNot { .. }
                | Expr::Call { .. } => children.as_slice().iter().all(Option::is_some),
                _ => false,
            };
            let ty = match node {
                Expr::Int { .. } => Some(PrimitiveType::I64),
                Expr::Number { .. } => Some(PrimitiveType::F64),
                Expr::Var { name, .. } => builtin_constant_type(name),
                Expr::Binary { op, .. } => {
                    let left = children
                        .next()
                        .expect("left constant kind")
                        .and_then(ScalarConstKind::literal_type);
                    let right = children
                        .next()
                        .expect("right constant kind")
                        .and_then(ScalarConstKind::literal_type);
                    left.zip(right).and_then(|(left, right)| {
                        let ty = merge_numeric_types_without_diagnostics(left, right)?;
                        if matches!(
                            op,
                            onda_frontend::BinaryOp::BitAnd
                                | onda_frontend::BinaryOp::BitOr
                                | onda_frontend::BinaryOp::BitXor
                                | onda_frontend::BinaryOp::ShiftLeft
                                | onda_frontend::BinaryOp::ShiftRight
                        ) && !matches!(ty, PrimitiveType::I32 | PrimitiveType::I64)
                        {
                            None
                        } else {
                            Some(ty)
                        }
                    })
                }
                Expr::UnaryBitNot { .. } => children
                    .next()
                    .expect("constant operand kind")
                    .and_then(ScalarConstKind::literal_type)
                    .filter(|ty| matches!(ty, PrimitiveType::I32 | PrimitiveType::I64)),
                Expr::Call { func, .. } => children
                    .map(|kind| kind.and_then(ScalarConstKind::literal_type))
                    .collect::<Option<Vec<_>>>()
                    .and_then(|types| intrinsic_result_type(*func, &types)),
                _ => None,
            };
            let kind = ty
                .map(ScalarConstKind::Literal)
                .or_else(|| closed.then_some(ScalarConstKind::Concrete));
            if let Some(kind) = kind {
                constants.insert(node as *const Expr, kind);
            }
            Ok::<_, std::convert::Infallible>(kind)
        },
    )
    .unwrap();
    constants
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
    constants: &DeclaredSymbolMap,
) -> (PrimitiveType, PrimitiveType) {
    // Context cannot change operands that already agree. Avoid rescanning
    // their subtrees at every node of a long, uniformly typed expression.
    if lhs_ty == rhs_ty {
        return (lhs_ty, rhs_ty);
    }
    let l_pure = is_pure_numeric_literal_expr(lhs, constants);
    let r_pure = is_pure_numeric_literal_expr(rhs, constants);
    adapt_binary_types_from_purity(lhs_ty, rhs_ty, l_pure, r_pure)
}

/// The constant evaluator carries purity alongside values to avoid repeatedly
/// walking operand trees. Both paths use this same literal adaptation rule.
pub(crate) fn adapt_binary_types_from_purity(
    lhs_ty: PrimitiveType,
    rhs_ty: PrimitiveType,
    l_pure: bool,
    r_pure: bool,
) -> (PrimitiveType, PrimitiveType) {
    match (l_pure, r_pure) {
        // One pure-literal, one non-literal: adapt literal to match the non-literal's type.
        // Never narrow a floating literal to an integer variable: `i / 10.0`
        // is a floating expression. Integer literals may still adapt to a
        // floating variable, and literals within one numeric family retain the
        // non-literal width.
        (true, false) if lhs_ty != PrimitiveType::Bool && rhs_ty != PrimitiveType::Bool => {
            if is_float_type(lhs_ty) && !is_float_type(rhs_ty) {
                (PrimitiveType::F32, rhs_ty)
            } else {
                (rhs_ty, rhs_ty)
            }
        }
        (false, true) if lhs_ty != PrimitiveType::Bool && rhs_ty != PrimitiveType::Bool => {
            if is_float_type(rhs_ty) && !is_float_type(lhs_ty) {
                (lhs_ty, PrimitiveType::F32)
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
    constants: &DeclaredSymbolMap,
) -> Vec<PrimitiveType> {
    if args.len() != arg_types.len() {
        return arg_types.to_vec();
    }
    let pure = args
        .iter()
        .map(|arg| is_pure_numeric_literal_expr(arg, constants))
        .collect::<Vec<_>>();
    adapt_numeric_argument_types_from_purity(arg_types, &pure)
}

/// Constant evaluation carries literal purity with each value instead of
/// repeatedly walking the expression. Keep its builtin adaptation identical.
pub(crate) fn adapt_numeric_argument_types_from_purity(
    arg_types: &[PrimitiveType],
    pure: &[bool],
) -> Vec<PrimitiveType> {
    debug_assert_eq!(arg_types.len(), pure.len());
    let mut concrete_ty = None;
    for (&is_pure, ty) in pure.iter().zip(arg_types.iter().copied()) {
        if is_pure {
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

    pure.iter()
        .zip(arg_types.iter().copied())
        .map(|(&is_pure, ty)| {
            if !is_pure || ty == PrimitiveType::Bool || concrete_ty == PrimitiveType::Bool {
                return ty;
            }
            if is_float_type(ty) && !is_float_type(concrete_ty) {
                PrimitiveType::F32
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
    check_children: bool,
) -> Option<PrimitiveType> {
    let mut infer_node =
        |expr: &Expr, children: &mut std::vec::Drain<'_, Option<PrimitiveType>>| {
            match expr {
                Expr::Number { .. } => Some(PrimitiveType::F64),
                Expr::Int { .. } => Some(PrimitiveType::I64),
                Expr::Bool { .. } => Some(PrimitiveType::Bool),
                Expr::ArrayLiteral { .. } => None,
                Expr::Tuple { .. } => None,
                Expr::Var { name, .. } => {
                    if declared_symbols.unresolved_types.contains(name) {
                        return None;
                    }
                    if let Some(ty) = builtin_constant_type(name) {
                        return Some(ty);
                    }
                    let lexical_root = name.split('.').next().unwrap_or(name);
                    if locals.contains(lexical_root) {
                        return (name == lexical_root).then(|| {
                            local_aliases
                                .get(name)
                                .copied()
                                .unwrap_or(PrimitiveType::I32)
                        });
                    }
                    if let Some((base, field)) = split_field_path(name, errors) {
                        let flat = format!("{base}.{field}");
                        if has_scalar_value_binding(base, locals, local_aliases) {
                            return None;
                        }
                        if let Some(struct_name) = struct_instances.get(base) {
                            if let Some(field_decl) =
                                resolve_struct_field_decl(struct_name, field, struct_defs)
                            {
                                return Some(match field_decl.ty {
                                    TypedFieldType::Scalar(prim) => prim,
                                    TypedFieldType::Struct | TypedFieldType::Tuple(_) => {
                                        return None
                                    }
                                    TypedFieldType::Array(_) => PrimitiveType::F32,
                                });
                            }
                        }
                        if let Some(ty) = local_aliases.get(&flat).copied() {
                            return Some(ty);
                        }
                        if let Some(ty) = state_scalars.get(&flat).copied() {
                            return Some(ty);
                        }
                        if let Some(ty) = declared_symbol_scalar_type(declared_symbols, &flat) {
                            return Some(ty);
                        }
                        if let Some(ty) = declared_symbol_scalar_type(declared_symbols, field) {
                            return Some(ty);
                        }
                        None
                    } else if let Some(ty) = local_aliases.get(name).copied() {
                        Some(ty)
                    } else if let Some(ty) = state_scalars.get(name).copied() {
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
                        match declared_symbols.get(name) {
                            Some(crate::decl_symbols::DeclaredSymbolInfo::Constant {
                                ty, ..
                            }) => Some(*ty),
                            _ => None,
                        }
                    }
                }
                Expr::Index { base, index, .. } => {
                    if declared_symbols.unresolved_types.contains(base) {
                        return None;
                    }
                    let lexical_root = base.split('.').next().unwrap_or(base);
                    if has_scalar_value_binding(lexical_root, locals, local_aliases) {
                        return None;
                    }
                    if let Some(alias) = local_array_aliases.get(base) {
                        if alias.elem_struct.is_none() {
                            return Some(alias.elem_ty);
                        }
                    }
                    let tuple_binding = local_aliases.contains_key(&format!("{base}[0]"))
                        || state_scalars.contains_key(&format!("{base}.__0"))
                        || state_scalars.contains_key(&format!("{base}[0]"));
                    if let Some(value) = tuple_binding
                        .then(|| declared_symbols.constant_integer(index, errors))
                        .flatten()
                    {
                        if let Some(ty) = local_aliases
                            .get(&format!("{base}[{value}]"))
                            .or_else(|| state_scalars.get(&format!("{base}.__{value}")))
                            .or_else(|| state_scalars.get(&format!("{base}[{value}]")))
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
                                        let value =
                                            declared_symbols.constant_integer(index, errors)?;
                                        return usize::try_from(value)
                                            .ok()
                                            .and_then(|index| elem_types.get(index).copied());
                                    }
                                    TypedFieldType::Scalar(_) | TypedFieldType::Struct => {}
                                }
                            }
                            if let Some(ty) = resolve_flattened_struct_array_leaf_type(
                                struct_name,
                                field,
                                struct_defs,
                            ) {
                                return Some(ty);
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
                Expr::Cast { to, .. } => {
                    let _ = children.next().expect("inferred child")?;
                    Some(*to)
                }
                Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
                    Some(PrimitiveType::Bool)
                }
                Expr::UnaryBitNot { expr, .. } => {
                    let inner = children.next().expect("inferred child")?;
                    merge_integer_types_for_expr(
                        expr,
                        inner,
                        inner,
                        "bitwise not expression",
                        errors,
                    )
                }
                Expr::Call { func, args, .. } => {
                    let arg_types = args
                        .iter()
                        .map(|_arg| children.next().expect("inferred child"))
                        .collect::<Vec<_>>();
                    if arg_types.iter().any(|t| t.is_none()) {
                        return None;
                    }
                    let arg_types = arg_types.into_iter().flatten().collect::<Vec<_>>();
                    let arg_types =
                        adapt_numeric_argument_types(args, &arg_types, declared_symbols);

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
                                expr.loc(),
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
                            let mut merged =
                                arg_types.first().copied().unwrap_or(PrimitiveType::F32);
                            for rhs in arg_types.iter().copied().skip(1) {
                                merged = merge_numeric_types(
                                    merged,
                                    rhs,
                                    "compiler-generated integer range normalization",
                                    expr.loc(),
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
                    if declared_symbols.unresolved_types.contains(name) {
                        return None;
                    }
                    if name == crate::proc_state_rewrite::STRUCT_ARRAY_FIELD_INDEX_SENTINEL {
                        let (base, _, field, field_index) =
                            crate::array_structs::extract_safi_args(args)?;
                        let struct_name = local_array_aliases.get(&base)?.elem_struct.as_ref()?;
                        return crate::resolve_indexed_struct_field_scalar_type(
                            struct_name,
                            &field,
                            &field_index,
                            struct_defs,
                        );
                    }
                    if let Some(method) = name
                        .strip_prefix(PROC_INDEX_CALL_SENTINEL)
                        .and_then(|suffix| suffix.strip_prefix('.'))
                    {
                        if let Some(ty) = builtin_instance_method_return_type(method) {
                            return Some(ty);
                        }
                    }
                    if let Some((receiver, _)) = name.rsplit_once('.') {
                        let root = receiver.split('.').next().unwrap_or(receiver);
                        if has_scalar_value_binding(root, locals, local_aliases) {
                            return None;
                        }
                    }
                    if let Some(ty) = declared_symbol_scalar_type(declared_symbols, name) {
                        return Some(ty);
                    }
                    if let Some((receiver, method)) = name.rsplit_once('.') {
                        if let Some(struct_name) = struct_instances.get(receiver) {
                            let resolved_name = format!("{struct_name}.{method}");
                            if let Some(ty) =
                                declared_symbol_scalar_type(declared_symbols, &resolved_name)
                            {
                                return Some(ty);
                            }
                        }
                    }
                    if let Some(base) = parse_array_len_instance_base(name) {
                        let root = base.split('.').next().unwrap_or(base);
                        if !has_scalar_value_binding(root, locals, local_aliases)
                            && (is_data_receiver_symbol_for_builtin(
                                base,
                                declared_symbols,
                                local_array_aliases,
                                struct_instances,
                                struct_defs,
                                proc_array_roots,
                            ) || (!has_lexical_root_binding(
                                base,
                                locals,
                                local_aliases,
                                local_array_aliases,
                                struct_instances,
                            ) && is_buffer_receiver_symbol_for_builtin(
                                base,
                                declared_symbols,
                            )))
                        {
                            return Some(PrimitiveType::I32);
                        }
                    }
                    if let Some(base) = parse_buffer_chans_instance_base(name) {
                        if !has_lexical_root_binding(
                            base,
                            locals,
                            local_aliases,
                            local_array_aliases,
                            struct_instances,
                        ) && is_buffer_receiver_symbol_for_builtin(base, declared_symbols)
                        {
                            return Some(PrimitiveType::I32);
                        }
                    }
                    if let Some(base) = parse_buffer_bound_instance_base(name) {
                        if !has_lexical_root_binding(
                            base,
                            locals,
                            local_aliases,
                            local_array_aliases,
                            struct_instances,
                        ) && is_buffer_receiver_symbol_for_builtin(base, declared_symbols)
                        {
                            return Some(PrimitiveType::Bool);
                        }
                    }
                    if let Some(base) = parse_buffer_samplerate_instance_base(name) {
                        if !has_lexical_root_binding(
                            base,
                            locals,
                            local_aliases,
                            local_array_aliases,
                            struct_instances,
                        ) && is_buffer_receiver_symbol_for_builtin(base, declared_symbols)
                        {
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
                    let l = children.next().expect("inferred child");
                    let r = children.next().expect("inferred child");
                    if let (Some(l), Some(r)) = (l, r) {
                        // Adapt literal types to the non-literal operand's type so that
                        // e.g. `x_f32 + 0.5` stays F32 rather than widening to F64.
                        let (el, er) = adapt_binary_operand_types(lhs, rhs, l, r, declared_symbols);
                        match op {
                            onda_frontend::BinaryOp::BitAnd
                            | onda_frontend::BinaryOp::BitOr
                            | onda_frontend::BinaryOp::BitXor
                            | onda_frontend::BinaryOp::ShiftLeft
                            | onda_frontend::BinaryOp::ShiftRight => merge_integer_types_for_expr(
                                expr,
                                el,
                                er,
                                "bitwise expression",
                                errors,
                            ),
                            _ => {
                                merge_numeric_types(el, er, "binary expression", expr.loc(), errors)
                            }
                        }
                    } else {
                        None
                    }
                }
            }
        };
    expr.try_fold(
        |expr, children| {
            // Validation checks every subexpression. Metadata queries only need
            // operands that determine the result; signatures and boolean result
            // types do not require traversing their arguments again.
            if check_children
                || matches!(
                    expr,
                    Expr::Cast { .. }
                        | Expr::UnaryBitNot { .. }
                        | Expr::Binary { .. }
                        | Expr::Call { .. }
                )
            {
                expr.children(children);
            }
        },
        |expr, children| Ok::<_, std::convert::Infallible>(infer_node(expr, children)),
    )
    .unwrap()
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
    let struct_instances = scalar_struct_instances(param_structs, struct_instances);
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
        &struct_instances,
        struct_defs,
        proc_array_roots,
        errors,
        true,
    )
}

fn scalar_struct_instances<'a>(
    params: Option<&'a HashMap<String, String>>,
    instances: &'a HashMap<String, String>,
) -> Cow<'a, HashMap<String, String>> {
    match params {
        None => Cow::Borrowed(instances),
        Some(params) if params.is_empty() => Cow::Borrowed(instances),
        Some(params) if instances.is_empty() => Cow::Borrowed(params),
        Some(params) => {
            let mut merged = instances.clone();
            merged.extend(params.iter().map(|(name, ty)| (name.clone(), ty.clone())));
            Cow::Owned(merged)
        }
    }
}

/// Infer a scalar type without evaluating values. With diagnostics, also check
/// subexpressions whose types are not needed to determine the parent's type.
pub(crate) fn infer_scalar_expr_type(
    expr: &Expr,
    env: crate::expr_analysis::ExprEnv<'_>,
    errors: Option<&mut Vec<Diagnostic>>,
) -> Option<PrimitiveType> {
    let check_children = errors.is_some();
    let mut discarded = Vec::new();
    let struct_instances = scalar_struct_instances(Some(env.param_structs), env.struct_instances);
    infer_scalar_expr_type_with_proc_arrays(
        expr,
        env.state_scalars,
        env.declared_symbols,
        env.local_aliases,
        env.local_array_aliases,
        env.locals,
        env.input_names,
        env.output_names,
        env.param_names,
        &struct_instances,
        env.struct_defs,
        env.proc_array_roots,
        errors.unwrap_or(&mut discarded),
        check_children,
    )
}

pub(crate) fn require_expr_assignable_type(
    expr: &Expr,
    src: Option<PrimitiveType>,
    dst: PrimitiveType,
    context: &str,
    errors: &mut Vec<Diagnostic>,
    constants: &DeclaredSymbolMap,
) {
    if let Some(src) = src {
        if !can_assign_expr_to_type(expr, src, dst, constants) {
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
pub(crate) fn can_assign_expr_to_type(
    expr: &Expr,
    src: PrimitiveType,
    dst: PrimitiveType,
    constants: &DeclaredSymbolMap,
) -> bool {
    if src == dst || can_implicitly_assign(src, dst) {
        return true;
    }
    // Const conversion checks the computed value's range in the interpreter.
    // Checking its numeric family does not require computing that value.
    if constants.constant_context {
        return src != PrimitiveType::Bool
            && dst != PrimitiveType::Bool
            && (is_float_type(dst) || !is_float_type(src));
    }
    if !is_pure_numeric_literal_expr(expr, constants) {
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
