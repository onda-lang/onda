use std::collections::{HashMap, HashSet};

use super::overloads::OverloadRewriteEnv;
use crate::*;
use onda_frontend::ast::{FnReturnScalarType, FnReturnType, Span};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum MonoParamKey {
    /// Non-generic param — keep as-is.
    Passthrough,
    /// Resolved concrete struct name (e.g. "Voice.__gen__f32").
    ResolvedStruct(String),
    /// Resolved array element type.
    ResolvedArray(PrimitiveType),
    /// Resolved buffer element type + channels.
    ResolvedBuffer(PrimitiveType, TypedBufferChannels),
    /// Resolved tuple element types (inferred from tuple literal arg).
    ResolvedTuple(Vec<PrimitiveType>),
    /// Resolved primitive type for an untyped scalar parameter.
    ResolvedScalar(PrimitiveType),
    /// Resolved generic def type parameter (e.g. T = f32).
    GenericType(PrimitiveType),
}

fn mono_def_name(base: &str, keys: &[MonoParamKey]) -> String {
    let mut suffix = String::new();
    for key in keys {
        match key {
            // Keep passthrough positions in the symbol. Omitting them makes
            // `[I32, passthrough]` and `[passthrough, I32]` collide even though
            // they describe different concrete signatures.
            MonoParamKey::Passthrough => suffix.push_str("__pass"),
            MonoParamKey::ResolvedStruct(s) => {
                suffix.push_str("__");
                suffix.push_str(&sanitize_symbol_component(s));
            }
            MonoParamKey::ResolvedArray(prim) => {
                suffix.push_str("__arr_");
                suffix.push_str(&format!("{prim:?}").to_lowercase());
            }
            MonoParamKey::ResolvedBuffer(prim, ch) => {
                suffix.push_str("__buf_");
                suffix.push_str(&format!("{prim:?}").to_lowercase());
                match ch {
                    TypedBufferChannels::Mono => {}
                    TypedBufferChannels::Static(n) => {
                        suffix.push_str(&format!("_{n}ch"));
                    }
                    TypedBufferChannels::Dynamic => suffix.push_str("_dyn"),
                }
            }
            MonoParamKey::ResolvedTuple(elem_tys) => {
                suffix.push_str("__tup");
                for ty in elem_tys {
                    suffix.push('_');
                    suffix.push_str(&format!("{ty:?}").to_lowercase());
                }
            }
            MonoParamKey::ResolvedScalar(prim) => {
                suffix.push_str("__scalar_");
                suffix.push_str(&format!("{prim:?}").to_lowercase());
            }
            MonoParamKey::GenericType(prim) => {
                suffix.push_str("__g_");
                suffix.push_str(&format!("{prim:?}").to_lowercase());
            }
        }
    }
    format!("{base}.__mono{suffix}")
}

fn infer_mono_arg_key(
    arg_expr: &Expr,
    param_ty: Option<&FnParamType>,
    env: &OverloadRewriteEnv,
    generic_templates: &HashSet<String>,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<MonoParamKey> {
    match param_ty {
        Some(FnParamType::Struct(struct_name)) if generic_templates.contains(struct_name) => {
            // Arg must be a variable whose concrete struct type is known.
            if let Expr::Var { name: var_name, .. } = arg_expr {
                if let Some(concrete) = env.struct_instances.get(var_name) {
                    // Check if this concrete name is a specialization of the template.
                    if concrete.starts_with(struct_name) || concrete.contains(".__gen__") {
                        return Some(MonoParamKey::ResolvedStruct(concrete.clone()));
                    }
                    // Could be a different struct that happens to match.
                    return Some(MonoParamKey::ResolvedStruct(concrete.clone()));
                }
            }
            None // Can't determine concrete struct type
        }
        Some(FnParamType::Array(None)) => {
            if let Some(elem_ty) =
                infer_array_arg_elem_type(arg_expr, env, return_types, struct_defs)
            {
                return Some(MonoParamKey::ResolvedArray(elem_ty));
            }
            // Default to f32 if we can't infer
            Some(MonoParamKey::ResolvedArray(PrimitiveType::F32))
        }
        Some(FnParamType::ArrayGeneric(_)) | Some(FnParamType::SizedArray { .. }) => {
            if let Some(elem_ty) =
                infer_array_arg_elem_type(arg_expr, env, return_types, struct_defs)
            {
                return Some(MonoParamKey::ResolvedArray(elem_ty));
            }
            Some(MonoParamKey::ResolvedArray(PrimitiveType::F32))
        }
        Some(FnParamType::BareBuffer) => {
            if let Some((elem_ty, channels)) = infer_buffer_arg_info(arg_expr, env) {
                return Some(MonoParamKey::ResolvedBuffer(elem_ty, channels));
            }
            // Default to f32 mono
            Some(MonoParamKey::ResolvedBuffer(
                PrimitiveType::F32,
                TypedBufferChannels::Mono,
            ))
        }
        None => {
            // Untyped parameters are structural duck types. Resolve resource
            // shapes before scalar shapes so one source def can be called with
            // arrays and buffers (including different element types) without a
            // later inference pass collapsing all call sites into one ABI.
            if let Some((elem_ty, channels)) = infer_buffer_arg_info(arg_expr, env) {
                return Some(MonoParamKey::ResolvedBuffer(elem_ty, channels));
            }
            if let Some(elem_ty) =
                infer_array_arg_elem_type(arg_expr, env, return_types, struct_defs)
            {
                return Some(MonoParamKey::ResolvedArray(elem_ty));
            }
            // Untyped scalar values are semantic polymorphism, not a backend
            // choice. Resolve primitive and tuple shapes at the call site so
            // every backend receives concrete function signatures.
            if let Expr::Tuple { values, .. } = arg_expr {
                let elem_tys: Vec<PrimitiveType> = values
                    .iter()
                    .map(|v| infer_tuple_elem_type(v, env))
                    .collect();
                return Some(MonoParamKey::ResolvedTuple(elem_tys));
            }
            if let Some(primitive) =
                infer_concrete_untyped_scalar_arg_type(arg_expr, env, return_types, struct_defs)
            {
                if primitive != PrimitiveType::F32 {
                    return Some(MonoParamKey::ResolvedScalar(primitive));
                }
            }
            Some(MonoParamKey::Passthrough)
        }
        _ => Some(MonoParamKey::Passthrough),
    }
}

/// Resolves the default scalar type of an argument to an untyped parameter.
/// Pure numeric expressions use the same default-narrowing rule as an untyped
/// assignment; explicit casts and non-literal expressions retain their
/// resolved type.
fn infer_concrete_untyped_scalar_arg_type(
    expr: &Expr,
    env: &OverloadRewriteEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } => untyped_literal_type(expr),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Var { name, .. } => builtin_constant_type(name)
            .or_else(|| lookup_scalar_symbol_type(name, env, struct_defs)),
        Expr::Index { base, .. } => lookup_slice_base_elem_type(base, env),
        Expr::UserCall { name, .. } => return_types.get(name).and_then(ReturnType::scalar),
        Expr::Binary { .. } | Expr::Call { .. } | Expr::UnaryBitNot { .. } => {
            let inferred = infer_expr_primitive_type(expr, env, return_types, struct_defs);
            if is_pure_numeric_literal_expr(expr) {
                effective_untyped_assignment_type(expr, inferred)
            } else {
                inferred
            }
        }
        Expr::Compare { .. } | Expr::Logical { .. } | Expr::UnaryNot { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::ArrayLiteral { .. }
        | Expr::Tuple { .. }
        | Expr::Slice { .. }
        | Expr::ArrayCtor { .. } => None,
    }
}

fn lookup_scalar_symbol_type(
    name: &str,
    env: &OverloadRewriteEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    if let Some(ty) = env.scalar_types.get(name).copied() {
        return Some(ty);
    }
    let (base, field) = split_simple_field_path(name)?;
    if let Some(ty) = env.scalar_types.get(&format!("{base}.{field}")).copied() {
        return Some(ty);
    }
    let struct_name = env.struct_instances.get(base)?;
    let field = resolve_struct_field_decl(struct_name, field, struct_defs)?;
    match field.ty {
        TypedFieldType::Scalar(ty) => Some(ty),
        TypedFieldType::Struct | TypedFieldType::Array(_) | TypedFieldType::Tuple(_) => None,
    }
}

fn lookup_array_symbol_elem_type(name: &str, env: &OverloadRewriteEnv) -> Option<PrimitiveType> {
    if let Some(elem_ty) = env.array_elem_types.get(name).copied() {
        return Some(elem_ty);
    }
    split_simple_field_path(name).and_then(|(root, field)| {
        let flat = format!("{root}.{field}");
        env.array_elem_types.get(&flat).copied()
    })
}

fn lookup_slice_base_elem_type(name: &str, env: &OverloadRewriteEnv) -> Option<PrimitiveType> {
    lookup_array_symbol_elem_type(name, env).or_else(|| {
        if let Some((elem_ty, _)) = env.buffer_types.get(name) {
            return Some(*elem_ty);
        }
        split_simple_field_path(name).and_then(|(root, field)| {
            let flat = format!("{root}.{field}");
            env.buffer_types.get(&flat).map(|(elem_ty, _)| *elem_ty)
        })
    })
}

fn infer_buffer_arg_info(
    expr: &Expr,
    env: &OverloadRewriteEnv,
) -> Option<(PrimitiveType, TypedBufferChannels)> {
    let name = match expr {
        Expr::Var { name, .. } => name,
        Expr::Index { base, .. } if env.buffer_arrays.contains(base) => base,
        _ => return None,
    };
    if let Some((elem_ty, channels)) = env.buffer_types.get(name) {
        return Some((*elem_ty, channels.clone()));
    }
    split_simple_field_path(name).and_then(|(root, field)| {
        let flat = format!("{root}.{field}");
        env.buffer_types
            .get(&flat)
            .map(|(elem_ty, channels)| (*elem_ty, channels.clone()))
    })
}

fn infer_array_arg_elem_type(
    expr: &Expr,
    env: &OverloadRewriteEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Var { name, .. } => lookup_array_symbol_elem_type(name, env),
        Expr::Slice { base, .. } => lookup_slice_base_elem_type(base, env),
        Expr::ArrayLiteral { values, .. } => values
            .first()
            .and_then(|value| infer_expr_primitive_type(value, env, return_types, struct_defs)),
        Expr::ArrayCtor { spec, .. } => match &spec.elem {
            ArrayElemType::Primitive(elem_ty) => Some(*elem_ty),
            ArrayElemType::Struct(_) => None,
        },
        _ => None,
    }
}

fn refresh_monomorphized_return_types(
    return_types: &mut HashMap<String, ReturnType>,
    original_defs: &[FunctionDef],
    generated_defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    generated_sigs: &HashMap<String, FnSignature>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    let mut combined_defs = Vec::with_capacity(original_defs.len() + generated_defs.len());
    combined_defs.extend_from_slice(original_defs);
    combined_defs.extend_from_slice(generated_defs);

    let mut combined_sigs = fn_signatures.clone();
    for (name, sig) in generated_sigs {
        combined_sigs.insert(name.clone(), sig.clone());
    }

    *return_types = infer_def_return_types(&combined_defs, &combined_sigs, struct_defs);
}

fn update_mono_env_after_assign(
    target: &AssignTarget,
    decl_ty: Option<PrimitiveType>,
    expr: &Expr,
    env: &mut OverloadRewriteEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    return_types: &HashMap<String, ReturnType>,
) {
    let AssignTarget::Var(name) = target else {
        return;
    };

    if let Some(declared) = decl_ty {
        env.scalar_types.insert(name.clone(), declared);
        env.array_elem_types.remove(name);
        env.struct_instances.remove(name);
        return;
    }

    if let Some(elem_ty) = infer_array_arg_elem_type(expr, env, return_types, struct_defs) {
        env.array_elem_types.insert(name.clone(), elem_ty);
        env.scalar_types.remove(name);
        env.struct_instances.remove(name);
        return;
    }

    if let Expr::UserCall { name: callee, .. } = expr {
        if struct_defs.contains_key(callee) {
            env.struct_instances.insert(name.clone(), callee.clone());
            env.scalar_types.remove(name);
            env.array_elem_types.remove(name);
            return;
        }
    }

    if let Expr::Var { name: src, .. } = expr {
        if let Some(struct_name) = env.struct_instances.get(src).cloned() {
            env.struct_instances.insert(name.clone(), struct_name);
            env.scalar_types.remove(name);
            env.array_elem_types.remove(name);
            return;
        }
    }

    if let Some(ty) = infer_expr_primitive_type(expr, env, return_types, struct_defs) {
        env.scalar_types.insert(name.clone(), ty);
        env.array_elem_types.remove(name);
        env.struct_instances.remove(name);
    }
}

/// Infer the primitive type of a tuple element expression for monomorphization.
fn infer_tuple_elem_type(expr: &Expr, env: &OverloadRewriteEnv) -> PrimitiveType {
    match expr {
        Expr::Int { .. } => PrimitiveType::I32,
        Expr::Bool { .. } => PrimitiveType::Bool,
        Expr::Number { .. } => PrimitiveType::F32,
        Expr::Cast { to, .. } => *to,
        Expr::Var { name, .. } => env
            .scalar_types
            .get(name)
            .copied()
            .unwrap_or(PrimitiveType::F32),
        _ => PrimitiveType::F32,
    }
}

/// Generated definitions are implementation details, so diagnostics originating
/// in them should point at the user expression that requested the specialization.
/// Rebasing the complete cloned body also carries that origin through nested
/// monomorphization (for example `readL(i32)` -> `calcIdx(i32)`).
fn rebase_generated_expr(expr: &mut Expr, origin: Span) {
    match expr {
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rebase_generated_expr(value, origin);
            }
        }
        Expr::Index { index, .. } => rebase_generated_expr(index, origin),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for value in [selector, channel, start, end].into_iter().flatten() {
                rebase_generated_expr(value, origin);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rebase_generated_expr(&mut spec.size, origin);
            if let Some(values) = init {
                for value in values {
                    rebase_generated_expr(value, origin);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rebase_generated_expr(lhs, origin);
            rebase_generated_expr(rhs, origin);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rebase_generated_expr(arg, origin);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rebase_generated_expr(&mut arg.expr, origin);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            rebase_generated_expr(expr, origin)
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
    expr.set_loc(origin);
}

fn rebase_generated_target(target: &mut AssignTarget, origin: Span) {
    match target {
        AssignTarget::Index { index, .. } => rebase_generated_expr(index, origin),
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for value in [selector, channel, start, end].into_iter().flatten() {
                rebase_generated_expr(value, origin);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

fn rebase_generated_stmt(stmt: &mut Stmt, origin: Span) {
    match stmt {
        Stmt::Const { loc, decl } => {
            *loc = origin;
            decl.loc = origin;
            rebase_generated_expr(&mut decl.expr, origin);
        }
        Stmt::Assign {
            loc,
            target_loc,
            target,
            typed_decl_ty_loc,
            expr,
            ..
        } => {
            *loc = origin;
            *target_loc = origin;
            *typed_decl_ty_loc = origin;
            rebase_generated_target(target, origin);
            rebase_generated_expr(expr, origin);
        }
        Stmt::Expr { loc, expr } | Stmt::Return { loc, expr } => {
            *loc = origin;
            rebase_generated_expr(expr, origin);
        }
        Stmt::If {
            loc,
            cond,
            then_branch,
            else_branch,
        } => {
            *loc = origin;
            rebase_generated_expr(cond, origin);
            for nested in then_branch.iter_mut().chain(else_branch) {
                rebase_generated_stmt(nested, origin);
            }
        }
        Stmt::For {
            loc,
            step,
            start,
            end,
            body,
            ..
        } => {
            *loc = origin;
            if let Some(step) = step {
                rebase_generated_expr(step, origin);
            }
            rebase_generated_expr(start, origin);
            rebase_generated_expr(end, origin);
            for nested in body {
                rebase_generated_stmt(nested, origin);
            }
        }
        Stmt::While { loc, cond, body } => {
            *loc = origin;
            rebase_generated_expr(cond, origin);
            for nested in body {
                rebase_generated_stmt(nested, origin);
            }
        }
        Stmt::Break { loc } | Stmt::Continue { loc } => *loc = origin,
    }
}

fn generate_mono_def(
    original: &FunctionDef,
    original_sig: &FnSignature,
    keys: &[MonoParamKey],
    mono_name: &str,
    origin: Span,
    _generic_templates: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) -> (FunctionDef, FnSignature) {
    let mut new_def = original.clone();
    new_def.name = mono_name.to_owned();
    let mut new_sig = original_sig.clone();

    // Build type bindings from GenericType keys for body rewriting.
    let mut type_bindings = HashMap::<String, PrimitiveType>::new();
    let has_generic_type_params = !original.type_params.is_empty();

    for (idx, key) in keys.iter().enumerate() {
        match key {
            MonoParamKey::Passthrough => {}
            MonoParamKey::ResolvedStruct(concrete_name) => {
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Struct(concrete_name.clone()));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Struct(concrete_name.clone()));
                }
            }
            MonoParamKey::ResolvedArray(elem_ty) => {
                // If original param is SizedArray, preserve the size.
                let original_param_ty = original.params.get(idx).and_then(|p| p.ty.as_ref());
                let new_ty = if let Some(FnParamType::SizedArray { size, .. }) = original_param_ty {
                    FnParamType::SizedArray {
                        elem: Some(*elem_ty),
                        generic_name: None,
                        size: size.clone(),
                    }
                } else {
                    FnParamType::Array(Some(*elem_ty))
                };
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(new_ty.clone());
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(new_ty);
                }
            }
            MonoParamKey::ResolvedBuffer(elem_ty, channels) => {
                let buf_ty = BufferType {
                    elem: BufferElemType::Primitive(*elem_ty),
                    channels: match channels {
                        TypedBufferChannels::Mono => BufferChannels::Mono,
                        TypedBufferChannels::Dynamic => BufferChannels::Dynamic,
                        TypedBufferChannels::Static(n) => {
                            BufferChannels::Static(Expr::int(*n as i64))
                        }
                    },
                };
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Buffer(buf_ty.clone()));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Buffer(buf_ty));
                }
            }
            MonoParamKey::ResolvedTuple(elem_tys) => {
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Tuple(elem_tys.clone()));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Tuple(elem_tys.clone()));
                }
            }
            MonoParamKey::ResolvedScalar(prim) => {
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Primitive(*prim));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Primitive(*prim));
                }
            }
            MonoParamKey::GenericType(prim) => {
                // Map the type param at this position to its concrete type.
                // GenericType keys are stored in order matching the def's type_params,
                // but they're indexed by *param* position in the keys array.
                // We need to find which type param this param references.
                if let Some(param) = new_def.params.get_mut(idx) {
                    // The param type is Struct("T") where "T" is a type param name —
                    // replace with Primitive.
                    param.ty = Some(FnParamType::Primitive(*prim));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Primitive(*prim));
                }
            }
        }
    }

    if has_generic_type_params {
        // Build type_bindings from GenericType keys.
        // Keys at param positions (0..params.len()) correspond to value params;
        // any extra GenericType keys appended after that are for type params not
        // covered by any value param (e.g. `def zero<T>()` with no params).

        // Pass 1: keys at param positions — extract type bindings from GenericType,
        // ResolvedArray (for ArrayGeneric params), and ResolvedBuffer (for Buffer(Generic) params).
        for (idx, key) in keys.iter().enumerate().take(original.params.len()) {
            if let Some(original_param) = original.params.get(idx) {
                match (key, &original_param.ty) {
                    (MonoParamKey::GenericType(prim), Some(FnParamType::Struct(ref name)))
                        if original.type_params.contains(name) =>
                    {
                        type_bindings.insert(name.clone(), *prim);
                    }
                    (
                        MonoParamKey::ResolvedArray(prim),
                        Some(FnParamType::ArrayGeneric(ref name)),
                    ) if original.type_params.contains(name) => {
                        type_bindings.insert(name.clone(), *prim);
                    }
                    (
                        MonoParamKey::ResolvedArray(prim),
                        Some(FnParamType::SizedArray {
                            generic_name: Some(ref name),
                            ..
                        }),
                    ) if original.type_params.contains(name) => {
                        type_bindings.insert(name.clone(), *prim);
                    }
                    (
                        MonoParamKey::ResolvedBuffer(prim, _),
                        Some(FnParamType::Buffer(BufferType {
                            elem: BufferElemType::Generic(ref name),
                            ..
                        })),
                    ) if original.type_params.contains(name) => {
                        type_bindings.insert(name.clone(), *prim);
                    }
                    _ => {}
                }
            }
        }

        // Pass 2: extra keys for type params not covered by value params
        let mut extra_idx = original.params.len();
        for tp in &original.type_params {
            if !type_bindings.contains_key(tp) {
                if let Some(MonoParamKey::GenericType(prim)) = keys.get(extra_idx) {
                    type_bindings.insert(tp.clone(), *prim);
                    extra_idx += 1;
                }
            }
        }

        // Rewrite the body: T(expr) → cast, T declarations, etc.
        if !type_bindings.is_empty() {
            let context = format!("generic def '{}'", original.name);
            for stmt in &mut new_def.body {
                crate::generic_specialization::substitute_call_type_args_with_bindings_stmt(
                    stmt,
                    &type_bindings,
                    &context,
                    errors,
                );
            }
            // Also rewrite param default expressions
            for param in &mut new_def.params {
                if let Some(default) = &mut param.default {
                    crate::generic_specialization::substitute_call_type_args_with_bindings_expr(
                        default,
                        &type_bindings,
                        &context,
                        errors,
                    );
                }
            }
            if let Some(return_ty) = &mut new_def.return_ty {
                *return_ty = match return_ty {
                    FnReturnType::Scalar(scalar) => FnReturnType::Scalar(match scalar {
                        FnReturnScalarType::Primitive(prim) => FnReturnScalarType::Primitive(*prim),
                        FnReturnScalarType::Named(name) => match type_bindings.get(name).copied() {
                            Some(bound) => FnReturnScalarType::Primitive(bound),
                            None => FnReturnScalarType::Named(name.clone()),
                        },
                    }),
                    FnReturnType::Array { elem, size } => FnReturnType::Array {
                        elem: *elem,
                        size: size.clone(),
                    },
                    FnReturnType::Tuple(elems) => FnReturnType::Tuple(
                        elems
                            .iter()
                            .map(|elem| match elem {
                                FnReturnScalarType::Primitive(prim) => {
                                    FnReturnScalarType::Primitive(*prim)
                                }
                                FnReturnScalarType::Named(name) => {
                                    match type_bindings.get(name).copied() {
                                        Some(bound) => FnReturnScalarType::Primitive(bound),
                                        None => FnReturnScalarType::Named(name.clone()),
                                    }
                                }
                            })
                            .collect(),
                    ),
                };
            }
        }

        // Clear type_params — the generated def is no longer generic.
        new_def.type_params.clear();
        new_sig.type_params.clear();
    }

    new_def.loc = origin;
    for param in &mut new_def.params {
        param.loc = origin;
        param.ty_loc = origin;
        if let Some(default) = &mut param.default {
            rebase_generated_expr(default, origin);
        }
    }
    if let Some(FnReturnType::Array { size, .. }) = &mut new_def.return_ty {
        rebase_generated_expr(size, origin);
    }
    for stmt in &mut new_def.body {
        rebase_generated_stmt(stmt, origin);
    }

    (new_def, new_sig)
}

/// Resolve generic def type parameter bindings from explicit type args or argument inference.
fn resolve_generic_def_type_bindings(
    sig: &FnSignature,
    type_args: &[CallTypeArg],
    args: &[CallArg],
    env: &OverloadRewriteEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    call_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, PrimitiveType> {
    let mut bindings = HashMap::new();

    if !type_args.is_empty() {
        // Explicit type args: map type_params[i] -> type_args[i]
        for (i, tp) in sig.type_params.iter().enumerate() {
            if let Some(CallTypeArg::Primitive(prim)) = type_args.get(i) {
                bindings.insert(tp.clone(), *prim);
            }
            // CallTypeArg::Generic would mean a forwarded generic — not applicable here.
        }
    } else {
        // Infer from every occurrence of a type parameter. Scalar occurrences
        // contribute widening-compatible constraints; array and buffer element
        // occurrences are exact because runtime aggregate elements are not
        // implicitly converted at a call boundary.
        let resolved_args = super::resolve_call_args(
            args,
            &sig.params,
            &sig.defaults,
            false,
            false,
            "generic def type inference",
            &mut Vec::new(),
        );
        let mut constraints = HashMap::<String, Vec<(PrimitiveType, bool, &Expr)>>::new();
        for (idx, param_ty) in sig.param_types.iter().enumerate() {
            let type_param_name = match param_ty {
                Some(FnParamType::Struct(ref name)) if sig.type_params.contains(name) => {
                    Some(name.clone())
                }
                Some(FnParamType::ArrayGeneric(ref name)) if sig.type_params.contains(name) => {
                    Some(name.clone())
                }
                Some(FnParamType::SizedArray {
                    generic_name: Some(ref name),
                    ..
                }) if sig.type_params.contains(name) => Some(name.clone()),
                Some(FnParamType::Buffer(BufferType {
                    elem: BufferElemType::Generic(ref name),
                    ..
                })) if sig.type_params.contains(name) => Some(name.clone()),
                _ => None,
            };
            let Some(name) = type_param_name else {
                continue;
            };
            if let Some(Some(arg_expr)) = resolved_args.get(idx) {
                // For array/buffer params, infer from the arg's element type.
                let (inferred, exact) = match param_ty {
                    Some(FnParamType::ArrayGeneric(_)) | Some(FnParamType::SizedArray { .. }) => (
                        infer_array_arg_elem_type(arg_expr, env, return_types, struct_defs),
                        true,
                    ),
                    Some(FnParamType::Buffer(BufferType {
                        elem: BufferElemType::Generic(_),
                        ..
                    })) => (
                        infer_buffer_arg_info(arg_expr, env).map(|(prim, _)| prim),
                        true,
                    ),
                    _ => (
                        infer_expr_primitive_type(arg_expr, env, return_types, struct_defs),
                        false,
                    ),
                };
                if let Some(prim) = inferred {
                    constraints
                        .entry(name)
                        .or_default()
                        .push((prim, exact, arg_expr));
                }
            }
        }

        for type_param in &sig.type_params {
            let Some(type_constraints) = constraints.get(type_param) else {
                continue;
            };
            let exact_target = type_constraints
                .iter()
                .find_map(|(ty, exact, _)| exact.then_some(*ty));
            let target = if let Some(exact_target) = exact_target {
                if let Some((actual, _, expr)) = type_constraints
                    .iter()
                    .find(|(ty, exact, _)| *exact && *ty != exact_target)
                {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "generic function '{call_name}' type parameter '{type_param}' has incompatible exact argument types {} and {}",
                            exact_target.name(),
                            actual.name()
                        ),
                        expr.loc(),
                    ));
                }
                exact_target
            } else {
                let mut inferred = type_constraints[0].0;
                for (next, _, expr) in type_constraints.iter().skip(1) {
                    let Some(merged) = merge_monomorphized_numeric_types(inferred, *next) else {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "generic function '{call_name}' type parameter '{type_param}' has incompatible argument types {} and {}",
                                inferred.name(),
                                next.name()
                            ),
                            expr.loc(),
                        ));
                        continue;
                    };
                    inferred = merged;
                }
                inferred
            };

            for (actual, exact, expr) in type_constraints {
                let compatible = if *exact {
                    *actual == target
                } else {
                    can_assign_expr_to_type(expr, *actual, target)
                };
                if !compatible {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "generic function '{call_name}' type parameter '{type_param}' resolves to {}, but argument has type {} and cannot be implicitly converted",
                            target.name(),
                            actual.name()
                        ),
                        expr.loc(),
                    ));
                }
            }
            bindings.insert(type_param.clone(), target);
        }
    }

    bindings
}

/// Infer the primitive type of an expression for generic type inference.
fn infer_expr_primitive_type(
    expr: &Expr,
    env: &OverloadRewriteEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { .. } => untyped_literal_type(expr),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Var { name, .. } => builtin_constant_type(name)
            .or_else(|| lookup_scalar_symbol_type(name, env, struct_defs)),
        Expr::Index { base, .. } => lookup_slice_base_elem_type(base, env),
        Expr::UserCall { name, .. } => {
            if let Some(return_ty) = return_types.get(name.as_str()) {
                return return_ty.scalar();
            }
            None
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let lhs_ty = infer_expr_primitive_type(lhs, env, return_types, struct_defs)?;
            let rhs_ty = infer_expr_primitive_type(rhs, env, return_types, struct_defs)?;
            let (lhs_ty, rhs_ty) = adapt_binary_operand_types(lhs, rhs, lhs_ty, rhs_ty);
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
                _ => merge_monomorphized_numeric_types(lhs_ty, rhs_ty),
            }
        }
        Expr::Compare { .. } | Expr::Logical { .. } | Expr::UnaryNot { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr, .. } => {
            let ty = infer_expr_primitive_type(expr, env, return_types, struct_defs)?;
            matches!(ty, PrimitiveType::I32 | PrimitiveType::I64).then_some(ty)
        }
        Expr::Call { func, args, .. } => {
            let arg_types = args
                .iter()
                .map(|arg| infer_expr_primitive_type(arg, env, return_types, struct_defs))
                .collect::<Option<Vec<_>>>()?;
            let arg_types = adapt_numeric_argument_types(args, &arg_types);
            match func {
                BuiltinFn::Abs => arg_types.first().copied(),
                BuiltinFn::Min | BuiltinFn::Max => {
                    merge_monomorphized_numeric_types(*arg_types.first()?, *arg_types.get(1)?)
                }
                BuiltinFn::RangeClamp => {
                    let value = *arg_types.first()?;
                    arg_types
                        .iter()
                        .copied()
                        .skip(1)
                        .try_fold(value, merge_monomorphized_numeric_types)
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
                | BuiltinFn::Fma => Some(if arg_types.contains(&PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
            }
        }
        Expr::ArrayLiteral { .. }
        | Expr::Tuple { .. }
        | Expr::Slice { .. }
        | Expr::ArrayCtor { .. } => None,
    }
}

fn merge_monomorphized_numeric_types(
    lhs: PrimitiveType,
    rhs: PrimitiveType,
) -> Option<PrimitiveType> {
    use PrimitiveType::{Bool, F32, F64, I32, I64};
    match (lhs, rhs) {
        (a, b) if a == b => Some(a),
        (Bool, _) | (_, Bool) => None,
        (F64, _) | (_, F64) => Some(F64),
        (F32, I64) | (I64, F32) => Some(F64),
        (F32, I32) | (I32, F32) | (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn monomorphize_calls_in_stmts(
    stmts: &mut [Stmt],
    env: &OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
    return_types: &mut HashMap<String, ReturnType>,
    errors: &mut Vec<Diagnostic>,
    enclosing_type_params: &[String],
) -> OverloadRewriteEnv {
    let mut local_env = env.clone();
    for stmt in stmts.iter_mut() {
        monomorphize_calls_in_stmt(
            stmt,
            &mut local_env,
            mono_eligible,
            fn_signatures,
            original_defs,
            generic_templates,
            struct_defs,
            generated_defs,
            generated_sigs,
            mono_cache,
            return_types,
            errors,
            enclosing_type_params,
        );
    }
    local_env
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_stmt(
    stmt: &mut Stmt,
    env: &mut OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
    return_types: &mut HashMap<String, ReturnType>,
    errors: &mut Vec<Diagnostic>,
    enclosing_type_params: &[String],
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target,
            decl_ty,
            expr,
            ..
        } => {
            monomorphize_calls_in_assign_target(
                target,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
            monomorphize_calls_in_expr(
                expr,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
            update_mono_env_after_assign(target, *decl_ty, expr, env, struct_defs, return_types);
        }
        Stmt::Expr { expr, .. } => {
            monomorphize_calls_in_expr(
                expr,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
        }
        Stmt::Return { expr, .. } => {
            monomorphize_calls_in_expr(
                expr,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            monomorphize_calls_in_expr(
                cond,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
            let mut then_env = env.clone();
            for s in then_branch.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    &mut then_env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
            let mut else_env = env.clone();
            for s in else_branch.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    &mut else_env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
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
            var,
            step,
            start,
            end,
            body,
            ..
        } => {
            monomorphize_calls_in_expr(
                start,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
            monomorphize_calls_in_expr(
                end,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
            if let Some(step) = step {
                monomorphize_calls_in_expr(
                    step,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
            let mut body_env = env.clone();
            // Runtime loop induction variables are always i32. Keep that
            // resolved semantic fact available while specializing calls in the
            // body; otherwise generated helpers called with `i` silently fall
            // back to their contextual f32 template.
            body_env
                .scalar_types
                .insert(var.clone(), PrimitiveType::I32);
            for s in body.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    &mut body_env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            monomorphize_calls_in_expr(
                cond,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
            let mut body_env = env.clone();
            for s in body.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    &mut body_env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_assign_target(
    target: &mut AssignTarget,
    env: &OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
    return_types: &mut HashMap<String, ReturnType>,
    errors: &mut Vec<Diagnostic>,
    enclosing_type_params: &[String],
) {
    let coordinates = match target {
        AssignTarget::Index { index, .. } => std::slice::from_mut(index),
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                monomorphize_calls_in_expr(
                    coordinate,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
            return;
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => return,
    };
    for coordinate in coordinates {
        monomorphize_calls_in_expr(
            coordinate,
            env,
            mono_eligible,
            fn_signatures,
            original_defs,
            generic_templates,
            struct_defs,
            generated_defs,
            generated_sigs,
            mono_cache,
            return_types,
            errors,
            enclosing_type_params,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_expr(
    expr: &mut Expr,
    env: &OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
    return_types: &mut HashMap<String, ReturnType>,
    errors: &mut Vec<Diagnostic>,
    enclosing_type_params: &[String],
) {
    match expr {
        Expr::UserCall {
            loc,
            name,
            type_args,
            args,
        } => {
            // Recurse into arg expressions first
            for arg in args.iter_mut() {
                monomorphize_calls_in_expr(
                    &mut arg.expr,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }

            if !mono_eligible.contains(name.as_str()) {
                // Also allow mono for calls with tuple literal args to untyped-param defs
                let has_tuple_arg = args.iter().any(|a| matches!(a.expr, Expr::Tuple { .. }));
                if !has_tuple_arg {
                    return;
                }
            }

            let Some(sig) = fn_signatures.get(name.as_str()) else {
                return;
            };
            // For generic defs, resolve type param bindings from explicit type args
            // or infer from argument types.  Unresolved params default to f32,
            // consistent with struct/proc generic defaults.
            //
            // When explicit type args contain unresolved generics (CallTypeArg::Generic),
            // check whether each generic name is a type param of the enclosing generic
            // def.  If so, skip monomorphization — the call will be monomorphized later
            // when the enclosing def is specialized.  If a generic name is NOT a type
            // param of the enclosing def, it's an error (e.g. `foo<U>()` where U is
            // undefined).
            let has_def_type_params = !sig.type_params.is_empty();
            if !type_args.is_empty() {
                let mut has_forwarded = false;
                for ta in type_args.iter() {
                    if let CallTypeArg::Generic(param) = ta {
                        if enclosing_type_params.contains(param) {
                            has_forwarded = true;
                        } else {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "unresolved generic type argument '{}' in call to '{}'; \
                                     expected a concrete type (f32, f64, i32, ...) or a type \
                                     parameter declared by the enclosing generic def",
                                    param, name
                                ),
                                *loc,
                            ));
                        }
                    }
                }
                if has_forwarded {
                    return;
                }
            }
            let resolved_type_bindings: HashMap<String, PrimitiveType> = if has_def_type_params {
                let mut bindings = resolve_generic_def_type_bindings(
                    sig,
                    type_args,
                    args,
                    env,
                    return_types,
                    struct_defs,
                    name,
                    errors,
                );
                for tp in &sig.type_params {
                    bindings.entry(tp.clone()).or_insert(PrimitiveType::F32);
                }
                bindings
            } else {
                HashMap::new()
            };

            // Build monomorphization key from each argument
            let resolved_args = super::resolve_call_args(
                args,
                &sig.params,
                &sig.defaults,
                false,
                false,
                &format!("mono call '{name}'"),
                &mut Vec::new(),
            );

            let mut keys = Vec::with_capacity(sig.params.len());
            let mut all_resolved = true;
            for (idx, _param_name) in sig.params.iter().enumerate() {
                let param_ty = sig.param_types.get(idx).and_then(|t| t.as_ref());

                // Check if this param references a def type param.
                if has_def_type_params {
                    if let Some(FnParamType::Struct(ref s)) = param_ty {
                        if sig.type_params.contains(s) {
                            if let Some(prim) = resolved_type_bindings.get(s) {
                                keys.push(MonoParamKey::GenericType(*prim));
                                continue;
                            }
                            all_resolved = false;
                            break;
                        }
                    }
                    // T[] — generic element type array param
                    if let Some(FnParamType::ArrayGeneric(ref s)) = param_ty {
                        if sig.type_params.contains(s) {
                            if let Some(prim) = resolved_type_bindings.get(s) {
                                keys.push(MonoParamKey::ResolvedArray(*prim));
                                continue;
                            }
                            // Fall through to Phase 3 inference
                        }
                    }
                    // T[N] — generic element type sized array param
                    if let Some(FnParamType::SizedArray {
                        generic_name: Some(ref s),
                        ..
                    }) = param_ty
                    {
                        if sig.type_params.contains(s) {
                            if let Some(prim) = resolved_type_bindings.get(s) {
                                keys.push(MonoParamKey::ResolvedArray(*prim));
                                continue;
                            }
                            // Fall through to Phase 3 inference
                        }
                    }
                    // buffer<T> / buffer<T[N]> — generic element type buffer param
                    if let Some(FnParamType::Buffer(BufferType {
                        elem: BufferElemType::Generic(ref s),
                        channels: ref _declared_channels,
                    })) = param_ty
                    {
                        if sig.type_params.contains(s) {
                            if let Some(prim) = resolved_type_bindings.get(s) {
                                // Infer channels from the argument
                                let inferred_channels = resolved_args
                                    .get(idx)
                                    .and_then(|a| a.as_ref())
                                    .and_then(|arg| {
                                        infer_buffer_arg_info(arg, env).map(|(_, ch)| ch)
                                    })
                                    .unwrap_or(TypedBufferChannels::Mono);
                                keys.push(MonoParamKey::ResolvedBuffer(*prim, inferred_channels));
                                continue;
                            }
                            // Fall through to Phase 3 inference
                        }
                    }
                }

                let arg_expr = resolved_args.get(idx).copied().flatten();
                if let Some(arg_expr) = arg_expr {
                    if let Some(key) = infer_mono_arg_key(
                        arg_expr,
                        param_ty,
                        env,
                        generic_templates,
                        return_types,
                        struct_defs,
                    ) {
                        keys.push(key);
                    } else {
                        all_resolved = false;
                        break;
                    }
                } else {
                    // A genuinely shape-free parameter remains on the
                    // non-specialized structural inference path.
                    keys.push(MonoParamKey::Passthrough);
                }
            }

            // Add GenericType keys for type params not covered by any value param.
            if has_def_type_params && all_resolved {
                let mut covered = HashSet::new();
                for (idx, _) in sig.params.iter().enumerate() {
                    let pt = sig.param_types.get(idx).and_then(|t| t.as_ref());
                    match pt {
                        Some(FnParamType::Struct(ref s)) if sig.type_params.contains(s) => {
                            covered.insert(s.clone());
                        }
                        Some(FnParamType::ArrayGeneric(ref s)) if sig.type_params.contains(s) => {
                            covered.insert(s.clone());
                        }
                        Some(FnParamType::SizedArray {
                            generic_name: Some(ref s),
                            ..
                        }) if sig.type_params.contains(s) => {
                            covered.insert(s.clone());
                        }
                        Some(FnParamType::Buffer(BufferType {
                            elem: BufferElemType::Generic(ref s),
                            ..
                        })) if sig.type_params.contains(s) => {
                            covered.insert(s.clone());
                        }
                        _ => {}
                    }
                }
                for tp in &sig.type_params {
                    if !covered.contains(tp) {
                        if let Some(prim) = resolved_type_bindings.get(tp) {
                            keys.push(MonoParamKey::GenericType(*prim));
                        } else {
                            all_resolved = false;
                            break;
                        }
                    }
                }
            }

            if !all_resolved {
                return;
            }

            // Check if all keys are passthrough (nothing to monomorphize)
            if keys.iter().all(|k| matches!(k, MonoParamKey::Passthrough)) {
                return;
            }

            let cache_key = (name.clone(), keys.clone());
            let mono_name = if let Some(cached) = mono_cache.get(&cache_key) {
                cached.clone()
            } else {
                let new_name = mono_def_name(name, &keys);

                // Find original def
                let original = original_defs
                    .iter()
                    .find(|d| d.name == *name)
                    .or_else(|| generated_defs.iter().find(|d| d.name == *name));

                if let Some(original) = original {
                    let (gen_def, gen_sig) = generate_mono_def(
                        original,
                        sig,
                        &keys,
                        &new_name,
                        *loc,
                        generic_templates,
                        errors,
                    );
                    generated_defs.push(gen_def);
                    generated_sigs.insert(new_name.clone(), gen_sig);
                    refresh_monomorphized_return_types(
                        return_types,
                        original_defs,
                        generated_defs,
                        fn_signatures,
                        generated_sigs,
                        struct_defs,
                    );
                }

                mono_cache.insert(cache_key, new_name.clone());
                new_name
            };

            *name = mono_name;
            // Clear type_args on the rewritten call — the mono copy is concrete.
            type_args.clear();
        }
        Expr::Binary { lhs, rhs, .. }
        | Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            monomorphize_calls_in_expr(
                lhs,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
            monomorphize_calls_in_expr(
                rhs,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args.iter_mut() {
                monomorphize_calls_in_expr(
                    arg,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            monomorphize_calls_in_expr(
                inner,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
        }
        Expr::ArrayLiteral { values: elems, .. } | Expr::Tuple { values: elems, .. } => {
            for elem in elems.iter_mut() {
                monomorphize_calls_in_expr(
                    elem,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
        }
        Expr::Index { index, .. } => {
            monomorphize_calls_in_expr(
                index,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                monomorphize_calls_in_expr(
                    coordinate,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    struct_defs,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                    return_types,
                    errors,
                    enclosing_type_params,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            monomorphize_calls_in_expr(
                &mut spec.size,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                struct_defs,
                generated_defs,
                generated_sigs,
                mono_cache,
                return_types,
                errors,
                enclosing_type_params,
            );
            if let Some(values) = init {
                for value in values {
                    monomorphize_calls_in_expr(
                        value,
                        env,
                        mono_eligible,
                        fn_signatures,
                        original_defs,
                        generic_templates,
                        struct_defs,
                        generated_defs,
                        generated_sigs,
                        mono_cache,
                        return_types,
                        errors,
                        enclosing_type_params,
                    );
                }
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onda_frontend::parse_program;

    fn scalar_param_types(def: &TypedFunction) -> Vec<Option<PrimitiveType>> {
        def.param_kinds
            .iter()
            .map(|param| match param {
                TypedFnParam::Scalar { ty } => *ty,
                other => panic!("expected scalar parameter, got {other:?}"),
            })
            .collect()
    }

    fn returned_call(def: &TypedFunction) -> (&str, &[CallArg]) {
        let [Stmt::Return {
            expr: Expr::UserCall { name, args, .. },
            ..
        }] = def.body.as_slice()
        else {
            panic!("expected '{}' to contain one returned call", def.name);
        };
        (name, args)
    }

    #[test]
    fn loop_indices_and_proc_fields_specialize_generated_calls() {
        let source = r#"
def clamp_idx(i, max_i):
  if i < 0:
    return 0
  if i > max_i:
    return max_i
  return i

proc Core:
  ins 3
  params:
    selected: i32 = 0
  outs 1

  init:
    sum = 0.0

  sample:
    sum = 0.0
    for i in 0..3:
      sum = sum + ins[i]
    clamped = clamp_idx(selected, 3 - 1)
    out1 = sum + f32(clamped)

init:
  core = Core()

sample:
  out1 = core(0.1, 0.2, 0.3)
"#;
        let parsed = parse_program(source).expect("generated-helper regression should parse");
        let typed = crate::analyze(parsed).expect("generated-helper regression should analyze");

        let read_helper = typed
            .defs
            .iter()
            .find(|def| {
                def.name
                    .starts_with("Core.__arr_read_clamp_3.__mono__scalar_i32")
                    && scalar_param_types(def).first() == Some(&Some(PrimitiveType::I32))
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing i32 loop-index helper specialization: {:?}",
                    typed
                        .defs
                        .iter()
                        .filter(|def| def.name.contains("__arr_read_clamp_3"))
                        .map(|def| (&def.name, &def.param_kinds))
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(
            scalar_param_types(read_helper),
            [Some(PrimitiveType::I32), None, None, None,]
        );

        let clamp = typed
            .defs
            .iter()
            .find(|def| {
                def.name == "clamp_idx.__mono__scalar_i32__scalar_i32"
                    && scalar_param_types(def)
                        == [Some(PrimitiveType::I32), Some(PrimitiveType::I32)]
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing concrete proc-field/integer-expression specialization: {:?}",
                    typed
                        .defs
                        .iter()
                        .filter(|def| def.name.starts_with("clamp_idx"))
                        .map(|def| (&def.name, &def.param_kinds))
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(clamp.return_ty, ReturnType::Scalar(PrimitiveType::I32));
    }

    #[test]
    fn sparse_scalar_specializations_have_distinct_symbols() {
        let source = r#"
def left(a, b):
  return a

sample:
  i: i32 = 1
  first = left(i, 0.0)
  second = left(0.0, i)
  out1 = f32(first) + second
"#;
        let parsed = parse_program(source).expect("sparse specialization source should parse");
        let typed = crate::analyze(parsed).expect("sparse specializations should analyze");

        let first = typed
            .defs
            .iter()
            .find(|def| def.name == "left.__mono__scalar_i32__pass")
            .expect("missing first-parameter specialization");
        let second = typed
            .defs
            .iter()
            .find(|def| def.name == "left.__mono__pass__scalar_i32")
            .expect("missing second-parameter specialization");
        assert_eq!(scalar_param_types(first), [Some(PrimitiveType::I32), None]);
        assert_eq!(scalar_param_types(second), [None, Some(PrimitiveType::I32)]);
    }

    #[test]
    fn assignment_target_coordinates_are_monomorphized_and_retained() {
        let source = r#"
def pick(value):
  return value

params:
  selected: i32 = 1

outs 2

sample:
  values = [0.0, 0.0]
  values[pick(selected):] = 0.0
  outs[pick(selected)] = values[0]
"#;
        let parsed = parse_program(source).expect("assignment target source should parse");
        let typed = crate::analyze(parsed).expect("assignment target call should analyze");
        let specialized_name = "pick.__mono__scalar_i32";
        assert!(
            typed.defs.iter().any(|def| def.name == specialized_name),
            "the specialization used only by the assignment target must remain reachable"
        );
        let target_calls = typed
            .sample
            .iter()
            .filter_map(|stmt| {
                let Stmt::Assign { target, .. } = stmt else {
                    return None;
                };
                let coordinate = match target {
                    AssignTarget::Index { index, .. } => index,
                    AssignTarget::Slice {
                        start: Some(start), ..
                    } => start,
                    _ => return None,
                };
                match coordinate {
                    Expr::UserCall { name, .. } => Some(name.as_str()),
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(target_calls, [specialized_name, specialized_name]);
        crate::lower_program_to_optimized_mir(&typed)
            .expect("assignment target specialization should lower to MIR");
    }

    #[test]
    fn scalar_specializations_preserve_every_concrete_argument_type_transitively() {
        let source = r#"
def leaf(x):
  return x

def middle(x):
  return leaf(x)

params:
  value_i32: i32
  value_i64: i64
  value_f32: f32
  value_f64: f64

sample:
  result_i32 = middle(value_i32)
  result_i64 = middle(value_i64)
  result_f32 = middle(value_f32)
  result_f64 = middle(value_f64)
  total = f32(result_i32) + f32(result_i64) + result_f32 + f32(result_f64)
  if middle(value_i32 > 0):
    total = total + 1.0
  out1 = total
"#;
        let parsed = parse_program(source).expect("scalar specialization matrix should parse");
        let typed = crate::analyze(parsed).expect("scalar specialization matrix should analyze");

        let expected = [
            ("i32", PrimitiveType::I32),
            ("i64", PrimitiveType::I64),
            ("f64", PrimitiveType::F64),
            ("bool", PrimitiveType::Bool),
        ];
        for (suffix, primitive) in expected {
            let leaf_name = format!("leaf.__mono__scalar_{suffix}");
            let middle_name = format!("middle.__mono__scalar_{suffix}");
            let leaf = typed
                .defs
                .iter()
                .find(|def| def.name == leaf_name)
                .unwrap_or_else(|| panic!("missing specialization '{leaf_name}'"));
            let middle = typed
                .defs
                .iter()
                .find(|def| def.name == middle_name)
                .unwrap_or_else(|| panic!("missing specialization '{middle_name}'"));

            assert_eq!(scalar_param_types(leaf), [Some(primitive)]);
            assert_eq!(scalar_param_types(middle), [Some(primitive)]);
            assert_eq!(leaf.return_ty, ReturnType::Scalar(primitive));
            assert_eq!(middle.return_ty, ReturnType::Scalar(primitive));

            let (callee, args) = returned_call(middle);
            assert_eq!(callee, leaf_name);
            assert!(
                matches!(args, [CallArg { expr: Expr::Var { name, .. }, .. }] if name == "x"),
                "specialization '{middle_name}' inserted an unexpected argument adaptation: {args:?}"
            );
        }

        let default_leaf = typed
            .defs
            .iter()
            .find(|def| def.name == "leaf")
            .expect("missing canonical f32 leaf specialization");
        let default_middle = typed
            .defs
            .iter()
            .find(|def| def.name == "middle")
            .expect("missing canonical f32 middle specialization");
        assert_eq!(scalar_param_types(default_leaf), [None]);
        assert_eq!(scalar_param_types(default_middle), [None]);
        assert_eq!(
            default_leaf.return_ty,
            ReturnType::Scalar(PrimitiveType::F32)
        );
        assert_eq!(
            default_middle.return_ty,
            ReturnType::Scalar(PrimitiveType::F32)
        );

        crate::lower_program_to_optimized_mir(&typed)
            .expect("every scalar specialization should lower to MIR");
    }

    #[test]
    fn typed_float_boundaries_widen_without_changing_untyped_specialization_types() {
        let source = r#"
def accept_float(x: f32):
  return x

def wrapper(x):
  return accept_float(x)

params:
  value: i32

sample:
  out1 = wrapper(value)
"#;
        let parsed = parse_program(source).expect("typed boundary source should parse");
        let typed = crate::analyze(parsed).expect("i32-to-f32 widening should analyze");
        let wrapper = typed
            .defs
            .iter()
            .find(|def| def.name == "wrapper.__mono__scalar_i32")
            .expect("missing i32 wrapper specialization");

        assert_eq!(scalar_param_types(wrapper), [Some(PrimitiveType::I32)]);
        assert_eq!(wrapper.return_ty, ReturnType::Scalar(PrimitiveType::F32));
        let (callee, args) = returned_call(wrapper);
        assert_eq!(callee, "accept_float");
        assert!(matches!(
            args,
            [CallArg {
                expr: Expr::Var { name, .. },
                ..
            }] if name == "x"
        ));

        crate::lower_program_to_optimized_mir(&typed)
            .expect("legal typed-boundary widening should lower to MIR");
    }

    #[test]
    fn numeric_literals_select_their_source_language_specialization_types() {
        let source = r#"
def identity(x):
  return x

sample:
  value_i32 = identity(1)
  value_f32 = identity(1.0)
  value_i64 = identity(2147483648)
  value_f64 = identity(f64(1))
  out1 = f32(value_i32) + value_f32 + f32(value_i64) + f32(value_f64)
"#;
        let parsed = parse_program(source).expect("literal specialization source should parse");
        let typed = crate::analyze(parsed).expect("literal specializations should analyze");

        for (name, primitive) in [
            ("identity.__mono__scalar_i32", PrimitiveType::I32),
            ("identity.__mono__scalar_i64", PrimitiveType::I64),
            ("identity.__mono__scalar_f64", PrimitiveType::F64),
        ] {
            let specialization = typed
                .defs
                .iter()
                .find(|def| def.name == name)
                .unwrap_or_else(|| panic!("missing literal specialization '{name}'"));
            assert_eq!(scalar_param_types(specialization), [Some(primitive)]);
            assert_eq!(specialization.return_ty, ReturnType::Scalar(primitive));
        }
        let default_f32 = typed
            .defs
            .iter()
            .find(|def| def.name == "identity")
            .expect("missing canonical f32 specialization");
        assert_eq!(scalar_param_types(default_f32), [None]);
        assert_eq!(
            default_f32.return_ty,
            ReturnType::Scalar(PrimitiveType::F32)
        );

        crate::lower_program_to_optimized_mir(&typed)
            .expect("literal specializations should lower to MIR");
    }

    #[test]
    fn invalid_concrete_specialization_bodies_fail_during_semantic_analysis() {
        let source = r#"
def floor_value(x):
  return floor(x)

def wrapper(x):
  return floor_value(x)

params:
  value: i32

sample:
  out1 = f32(wrapper(value))
"#;
        let parsed = parse_program(source).expect("invalid specialization source should parse");
        let errors = crate::analyze(parsed)
            .expect_err("an i32 specialization using floor must fail semantically");
        assert!(
            errors.iter().any(|error| {
                error
                    .message
                    .contains("while checking specialization of 'floor_value'")
                    && error.message.contains("got I32")
            }),
            "missing concrete specialization diagnostic: {errors:?}"
        );
    }

    #[test]
    fn integer_arguments_specialize_untyped_callees_without_forced_float_coercion() {
        let source = r#"
def inner(freq):
  return log(freq / 440.0)

def wrapper(freq):
  return inner(freq)

sample:
  out1 = wrapper(440)
"#;
        let parsed = parse_program(source).expect("float-constraint source should parse");
        let typed = crate::analyze(parsed).expect("integer specializations should analyze");

        assert!(typed
            .defs
            .iter()
            .any(|def| def.name == "wrapper.__mono__scalar_i32"));
        assert!(typed
            .defs
            .iter()
            .any(|def| def.name == "inner.__mono__scalar_i32"));
        crate::lower_program_to_optimized_mir(&typed)
            .expect("nested integer specializations should lower to MIR");
    }
}
