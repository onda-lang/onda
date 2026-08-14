use std::collections::{HashMap, HashSet};

use super::call_types::{
    infer_array_arg_type as infer_call_array_arg_type, infer_buffer_arg_info,
    infer_scalar_expr_type, infer_struct_expr_type, infer_tuple_arg_types, join_branch_envs,
    resolved_buffer_channels, update_call_type_env_after_assign, CallArrayElemType,
    CallTypeContext, CallTypeEnv, StatementFlow,
};
use crate::*;
use onda_frontend::ast::{FnReturnScalarType, FnReturnType, Span};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum MonoParamKey {
    /// A concrete, non-generic parameter that needs no specialization.
    Passthrough,
    /// An untyped parameter whose supplied value has no concrete scalar or
    /// aggregate shape yet. It remains on the structural inference path.
    UnresolvedStructural,
    /// Resolved concrete struct name (e.g. "Voice.__gen__f32").
    ResolvedStruct(String),
    /// Resolved array element type.
    ResolvedArray(PrimitiveType),
    /// Resolved nominal array element type. Processor arrays retain their
    /// fixed capacity; data-struct arrays use a length-independent view ABI.
    ResolvedNominalArray(String, Option<usize>),
    /// Resolved buffer element type + channels.
    ResolvedBuffer(PrimitiveType, TypedBufferChannels),
    /// Resolved fixed buffer-collection element type + per-buffer channels.
    ResolvedBufferArray(PrimitiveType, TypedBufferChannels, usize),
    /// Resolved tuple element types (inferred from tuple literal arg).
    ResolvedTuple(Vec<PrimitiveType>),
    /// Resolved primitive type for an untyped scalar parameter.
    ResolvedScalar(PrimitiveType),
    /// Resolved generic def type parameter (e.g. T = f32).
    GenericType(PrimitiveType),
}

#[derive(Clone, Copy)]
pub(crate) struct MonoOwnerContext<'a> {
    pub(crate) type_params: &'a [String],
    pub(crate) proc_types: &'a HashSet<String>,
    pub(crate) return_type_env: &'a CallTypeEnv,
}

fn mono_def_name(base: &str, keys: &[MonoParamKey]) -> String {
    let mut suffix = String::new();
    for key in keys {
        match key {
            // Keep passthrough positions in the symbol. Omitting them makes
            // `[I32, passthrough]` and `[passthrough, I32]` collide even though
            // they describe different concrete signatures.
            MonoParamKey::Passthrough => suffix.push_str("__pass"),
            MonoParamKey::UnresolvedStructural => suffix.push_str("__open"),
            MonoParamKey::ResolvedStruct(s) => {
                suffix.push_str("__");
                suffix.push_str(&crate::internal_names::encode_internal_symbol_component(s));
            }
            MonoParamKey::ResolvedArray(prim) => {
                suffix.push_str("__arr_");
                suffix.push_str(&format!("{prim:?}").to_lowercase());
            }
            MonoParamKey::ResolvedNominalArray(name, len) => {
                suffix.push_str("__arr_nom_");
                suffix.push_str(&crate::internal_names::encode_internal_symbol_component(
                    name,
                ));
                match len {
                    Some(len) => suffix.push_str(&format!("_{len}")),
                    None => suffix.push_str("_view"),
                }
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
            MonoParamKey::ResolvedBufferArray(prim, ch, len) => {
                suffix.push_str("__bufs_");
                suffix.push_str(&format!("{prim:?}").to_lowercase());
                match ch {
                    TypedBufferChannels::Mono => {}
                    TypedBufferChannels::Static(n) => suffix.push_str(&format!("_{n}ch")),
                    TypedBufferChannels::Dynamic => suffix.push_str("_dyn"),
                }
                suffix.push_str(&format!("_{len}items"));
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
    // The generated member begins with the language-reserved `__onda_`
    // prefix, so no legal source declaration can shadow a specialization.
    format!("{base}.__onda_mono{suffix}")
}

fn infer_mono_arg_key(
    arg_expr: &Expr,
    param_ty: Option<&FnParamType>,
    env: &CallTypeEnv,
    generic_templates: &HashSet<String>,
    proc_types: &HashSet<String>,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<MonoParamKey> {
    match param_ty {
        Some(FnParamType::Struct(struct_name)) if generic_templates.contains(struct_name) => {
            infer_struct_expr_type(
                arg_expr,
                env,
                CallTypeContext {
                    return_types,
                    struct_defs,
                },
            )
            .map(MonoParamKey::ResolvedStruct)
        }
        Some(FnParamType::Array(None)) => {
            infer_array_arg_key(arg_expr, env, proc_types, return_types, struct_defs)
        }
        Some(FnParamType::ArrayGeneric(name)) if proc_types.contains(name) => {
            infer_array_arg_key(arg_expr, env, proc_types, return_types, struct_defs)
        }
        Some(FnParamType::BareBuffer) => infer_buffer_arg_info(arg_expr, env)
            .map(|(elem_ty, channels)| MonoParamKey::ResolvedBuffer(elem_ty, channels)),
        None => {
            // Untyped parameters are structural duck types. Resolve resource
            // shapes before scalar shapes so one source def can be called with
            // arrays and buffers (including different element types) without a
            // later inference pass collapsing all call sites into one ABI.
            if let Some(struct_name) = infer_struct_expr_type(
                arg_expr,
                env,
                CallTypeContext {
                    return_types,
                    struct_defs,
                },
            ) {
                return Some(MonoParamKey::ResolvedStruct(struct_name));
            }
            if let Expr::Var { name, .. } = arg_expr {
                if let Some(len) = env.buffer_array_lens.get(name).copied() {
                    if let Some((elem_ty, channels)) = infer_buffer_arg_info(arg_expr, env) {
                        return Some(MonoParamKey::ResolvedBufferArray(elem_ty, channels, len));
                    }
                }
            }
            if let Some((elem_ty, channels)) = infer_buffer_arg_info(arg_expr, env) {
                return Some(MonoParamKey::ResolvedBuffer(elem_ty, channels));
            }
            if let Some(key) =
                infer_array_arg_key(arg_expr, env, proc_types, return_types, struct_defs)
            {
                return Some(key);
            }
            // Untyped scalar values are semantic polymorphism, not a backend
            // choice. Resolve primitive and tuple shapes at the call site so
            // every backend receives concrete function signatures.
            if let Some(elem_tys) = infer_tuple_arg_types(
                arg_expr,
                env,
                CallTypeContext {
                    return_types,
                    struct_defs,
                },
            ) {
                return Some(MonoParamKey::ResolvedTuple(elem_tys));
            }
            // Aggregate syntax carries a shape even when one of its dependent
            // element types is not concrete yet. Treat that as deferred, not
            // as the legacy shape-free f32 passthrough.
            if matches!(
                arg_expr,
                Expr::ArrayLiteral { .. }
                    | Expr::Tuple { .. }
                    | Expr::Slice { .. }
                    | Expr::ArrayCtor { .. }
            ) {
                return None;
            }
            if let Some(primitive) =
                infer_concrete_untyped_scalar_arg_type(arg_expr, env, return_types, struct_defs)
            {
                return Some(MonoParamKey::ResolvedScalar(primitive));
            }
            Some(MonoParamKey::UnresolvedStructural)
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
    env: &CallTypeEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    let inferred = infer_expr_primitive_type(expr, env, return_types, struct_defs);
    if is_pure_numeric_literal_expr(expr) {
        effective_untyped_assignment_type(expr, inferred)
    } else {
        inferred
    }
}

fn infer_array_arg_type(
    expr: &Expr,
    env: &CallTypeEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<super::call_types::CallArrayType> {
    infer_call_array_arg_type(
        expr,
        env,
        CallTypeContext {
            return_types,
            struct_defs,
        },
    )
}

fn infer_array_arg_elem_type(
    expr: &Expr,
    env: &CallTypeEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    infer_array_arg_type(expr, env, return_types, struct_defs)?.primitive_elem()
}

fn infer_array_arg_key(
    expr: &Expr,
    env: &CallTypeEnv,
    proc_types: &HashSet<String>,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<MonoParamKey> {
    let array_ty = infer_array_arg_type(expr, env, return_types, struct_defs)?;
    match array_ty.elem {
        CallArrayElemType::Primitive(elem) => Some(MonoParamKey::ResolvedArray(elem)),
        CallArrayElemType::Nominal(name) if proc_types.contains(&name) => array_ty
            .len
            .map(|len| MonoParamKey::ResolvedNominalArray(name, Some(len))),
        CallArrayElemType::Nominal(name) => Some(MonoParamKey::ResolvedNominalArray(name, None)),
    }
}

fn source_buffer_channels(channels: &TypedBufferChannels) -> BufferChannels {
    match channels {
        TypedBufferChannels::Mono => BufferChannels::Mono,
        TypedBufferChannels::Dynamic => BufferChannels::Dynamic,
        TypedBufferChannels::Static(channels) => {
            BufferChannels::Static(Expr::int(*channels as i64))
        }
    }
}

pub(crate) fn refresh_monomorphized_return_types(
    return_types: &mut HashMap<String, ReturnType>,
    original_defs: &[FunctionDef],
    generated_defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    generated_sigs: &HashMap<String, FnSignature>,
    env_seed: &CallTypeEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> bool {
    let mut combined_defs = Vec::with_capacity(original_defs.len() + generated_defs.len());
    combined_defs.extend_from_slice(original_defs);
    combined_defs.extend_from_slice(generated_defs);

    let mut combined_sigs = fn_signatures.clone();
    for (name, sig) in generated_sigs {
        combined_sigs.insert(name.clone(), sig.clone());
    }

    // Strict return inference publishes only results whose complete expression
    // dependencies are known. Keep dependent templates in the input so a
    // result that does not depend on their open parameters (for example
    // `def len(value): return 1`) remains available to nested call inference.
    // A result that actually reads an open parameter or unresolved call is
    // withheld by `infer_known_def_return_types` itself.
    let refreshed =
        infer_known_def_return_types(&combined_defs, &combined_sigs, env_seed, struct_defs);
    if *return_types == refreshed {
        return false;
    }
    *return_types = refreshed;
    true
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
    errors: &mut Vec<Diagnostic>,
) -> (FunctionDef, FnSignature) {
    let mut new_def = original.clone();
    new_def.name = mono_name.to_owned();
    let mut new_sig = original_sig.clone();
    new_sig.requires_call_specialization = false;

    // Build type bindings from GenericType keys for body rewriting.
    let mut type_bindings = HashMap::<String, PrimitiveType>::new();
    let has_generic_type_params = !original.type_params.is_empty();

    for (idx, key) in keys.iter().enumerate() {
        match key {
            MonoParamKey::Passthrough | MonoParamKey::UnresolvedStructural => {}
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
            MonoParamKey::ResolvedNominalArray(name, resolved_len) => {
                let original_param_ty = original.params.get(idx).and_then(|p| p.ty.as_ref());
                let new_ty = if let Some(FnParamType::SizedArray { size, .. }) = original_param_ty {
                    FnParamType::SizedArray {
                        elem: None,
                        generic_name: Some(name.clone()),
                        size: size.clone(),
                    }
                } else if let Some(len) = resolved_len {
                    FnParamType::SizedArray {
                        elem: None,
                        generic_name: Some(name.clone()),
                        size: Expr::int(*len as i64),
                    }
                } else {
                    FnParamType::ArrayGeneric(name.clone())
                };
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(new_ty.clone());
                }
                if let Some(param_ty) = new_sig.param_types.get_mut(idx) {
                    *param_ty = Some(new_ty);
                }
            }
            MonoParamKey::ResolvedBuffer(elem_ty, channels) => {
                let declared_channels = match original.params.get(idx).and_then(|p| p.ty.as_ref()) {
                    Some(FnParamType::Buffer(buffer)) => Some(buffer.channels.clone()),
                    _ => None,
                };
                let buf_ty = BufferType {
                    elem: BufferElemType::Primitive(*elem_ty),
                    channels: declared_channels.unwrap_or_else(|| source_buffer_channels(channels)),
                };
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Buffer(buf_ty.clone()));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Buffer(buf_ty));
                }
            }
            MonoParamKey::ResolvedBufferArray(elem_ty, channels, resolved_len) => {
                let (declared_channels, len) =
                    match original.params.get(idx).and_then(|p| p.ty.as_ref()) {
                        Some(FnParamType::BufferArray { buffer, len }) => {
                            (buffer.channels.clone(), *len)
                        }
                        None => (source_buffer_channels(channels), *resolved_len),
                        _ => continue,
                    };
                let new_ty = FnParamType::BufferArray {
                    buffer: BufferType {
                        elem: BufferElemType::Primitive(*elem_ty),
                        // Specialization resolves only the generic element;
                        // the collection's declared channel contract is exact.
                        channels: declared_channels,
                    },
                    len,
                };
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(new_ty.clone());
                }
                if let Some(param_ty) = new_sig.param_types.get_mut(idx) {
                    *param_ty = Some(new_ty);
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
                    (
                        MonoParamKey::ResolvedBufferArray(prim, _, _),
                        Some(FnParamType::BufferArray {
                            buffer:
                                BufferType {
                                    elem: BufferElemType::Generic(ref name),
                                    ..
                                },
                            ..
                        }),
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
    new_sig.sync_defaults_from_def(&new_def);

    (new_def, new_sig)
}

/// Resolve generic def type parameter bindings from explicit type args or argument inference.
fn resolve_generic_def_type_bindings(
    sig: &FnSignature,
    type_args: &[CallTypeArg],
    args: &[CallArg],
    env: &CallTypeEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    call_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<HashMap<String, PrimitiveType>> {
    let mut bindings = HashMap::new();

    if !type_args.is_empty() {
        if type_args
            .iter()
            .any(|type_arg| matches!(type_arg, CallTypeArg::Primitive(PrimitiveType::Bool)))
        {
            return None;
        }
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
        let mut has_dependent_argument = false;
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
                Some(FnParamType::BufferArray {
                    buffer:
                        BufferType {
                            elem: BufferElemType::Generic(ref name),
                            ..
                        },
                    ..
                }) if sig.type_params.contains(name) => Some(name.clone()),
                _ => None,
            };
            let Some(name) = type_param_name else {
                continue;
            };
            let supplied_arg = resolved_args.get(idx).copied().flatten();
            let arg_expr = supplied_arg.or_else(|| sig.defaults.get(idx).and_then(Option::as_ref));
            if let Some(arg_expr) = arg_expr {
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
                    Some(FnParamType::BufferArray {
                        buffer:
                            BufferType {
                                elem: BufferElemType::Generic(_),
                                ..
                            },
                        ..
                    }) => (
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
                } else if supplied_arg.is_some() || !expr_references_type_param(arg_expr, &name) {
                    // The argument exists, but its semantic type depends on an
                    // enclosing specialization, or the omitted default depends
                    // on a return type that has not converged yet. Defer this
                    // call instead of confusing either unknown with an
                    // argument-free generic parameter. A default that actually
                    // references this type parameter is self-contextual and
                    // intentionally contributes no constraint, allowing the
                    // parameter's ordinary f32 default to break the cycle.
                    has_dependent_argument = true;
                }
            }
        }

        if has_dependent_argument {
            return None;
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
                    let diagnostic = Diagnostic::semantic_span(
                        format!(
                            "generic function '{call_name}' type parameter '{type_param}' has incompatible exact argument types {} and {}",
                            exact_target.name(),
                            actual.name()
                        ),
                        expr.loc(),
                    );
                    if !errors.contains(&diagnostic) {
                        errors.push(diagnostic);
                    }
                    return None;
                }
                exact_target
            } else {
                let contextual_type = |actual, expr| {
                    effective_untyped_assignment_type(expr, Some(actual)).unwrap_or(actual)
                };
                let mut inferred = contextual_type(type_constraints[0].0, type_constraints[0].2);
                for (next, _, expr) in type_constraints.iter().skip(1) {
                    let next = contextual_type(*next, expr);
                    let Some(merged) = merge_inferred_return_types(inferred, next) else {
                        let diagnostic = Diagnostic::semantic_span(
                            format!(
                                "generic function '{call_name}' type parameter '{type_param}' has incompatible argument types {} and {}",
                                inferred.name(),
                                next.name()
                            ),
                            expr.loc(),
                        );
                        if !errors.contains(&diagnostic) {
                            errors.push(diagnostic);
                        }
                        return None;
                    };
                    inferred = merged;
                }
                inferred
            };

            if !target.is_numeric() {
                let diagnostic = Diagnostic::semantic_span(
                    format!(
                        "generic function '{call_name}' type parameter '{type_param}' inferred as bool, but generic type arguments must be numeric (f32, f64, i32, or i64)"
                    ),
                    type_constraints[0].2.loc(),
                );
                if !errors.contains(&diagnostic) {
                    errors.push(diagnostic);
                }
                return None;
            }

            for (actual, exact, expr) in type_constraints {
                let compatible = if *exact {
                    *actual == target
                } else {
                    can_assign_expr_to_type(expr, *actual, target)
                };
                if !compatible {
                    let diagnostic = Diagnostic::semantic_span(
                        format!(
                            "generic function '{call_name}' type parameter '{type_param}' resolves to {}, but argument has type {} and cannot be implicitly converted",
                            target.name(),
                            actual.name()
                        ),
                        expr.loc(),
                    );
                    if !errors.contains(&diagnostic) {
                        errors.push(diagnostic);
                    }
                    return None;
                }
            }
            bindings.insert(type_param.clone(), target);
        }
    }

    Some(bindings)
}

/// Returns whether an expression's type is explicitly contextualized by
/// `type_param`. Such a default cannot independently infer that same type
/// parameter: `T(1)` and `identity<T>(1)` acquire their type from `T`, rather
/// than supplying a constraint for it.
fn expr_references_type_param(expr: &Expr, type_param: &str) -> bool {
    match expr {
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => values
            .iter()
            .any(|value| expr_references_type_param(value, type_param)),
        Expr::Index { index, .. } => expr_references_type_param(index, type_param),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => [selector, channel, start, end]
            .into_iter()
            .flatten()
            .any(|value| expr_references_type_param(value, type_param)),
        Expr::ArrayCtor { spec, init, .. } => {
            matches!(&spec.elem, ArrayElemType::Struct(name) if name == type_param)
                || expr_references_type_param(&spec.size, type_param)
                || init.as_ref().is_some_and(|values| {
                    values
                        .iter()
                        .any(|value| expr_references_type_param(value, type_param))
                })
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            expr_references_type_param(lhs, type_param)
                || expr_references_type_param(rhs, type_param)
        }
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| expr_references_type_param(arg, type_param)),
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            name == type_param
                || type_args
                    .iter()
                    .any(|arg| matches!(arg, CallTypeArg::Generic(name) if name == type_param))
                || args
                    .iter()
                    .any(|arg| expr_references_type_param(&arg.expr, type_param))
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            expr_references_type_param(expr, type_param)
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => false,
    }
}

/// Infer the primitive type of an expression for generic type inference.
fn infer_expr_primitive_type(
    expr: &Expr,
    env: &CallTypeEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<PrimitiveType> {
    infer_scalar_expr_type(
        expr,
        env,
        CallTypeContext {
            return_types,
            struct_defs,
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn monomorphize_calls_in_stmts(
    stmts: &mut [Stmt],
    env: &CallTypeEnv,
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
    owner: MonoOwnerContext<'_>,
) -> CallTypeEnv {
    let mut local_env = env.clone();
    monomorphize_calls_in_stmt_list_impl(
        stmts,
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
        owner,
    );
    local_env
}

/// Monomorphizes every runtime expression owned by a function. Parameter
/// defaults are call sites just like body expressions, but their lexical
/// environment is built incrementally in declaration order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn monomorphize_calls_in_function(
    def: &mut FunctionDef,
    env: &CallTypeEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    proc_types: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
    return_types: &mut HashMap<String, ReturnType>,
    errors: &mut Vec<Diagnostic>,
) {
    let type_params = def.type_params.clone();
    let owner = MonoOwnerContext {
        type_params: &type_params,
        proc_types,
        return_type_env: env,
    };
    let mut local_env = env.clone();
    local_env.set_owner_type_params(&type_params);

    for param in &mut def.params {
        local_env.bind_function_param(param, &type_params);
        if let Some(default) = &mut param.default {
            monomorphize_calls_in_expr(
                default,
                &local_env,
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
                owner,
            );
        }
    }

    monomorphize_calls_in_stmts(
        &mut def.body,
        &local_env,
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
        owner,
    );
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_stmt(
    stmt: &mut Stmt,
    env: &mut CallTypeEnv,
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
    owner: MonoOwnerContext<'_>,
) -> StatementFlow {
    match stmt {
        Stmt::Const { .. } => StatementFlow::Continues,
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
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
                owner,
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
                owner,
            );
            update_call_type_env_after_assign(
                target,
                *decl_ty,
                generic_decl_ty.as_deref(),
                expr,
                env,
                CallTypeContext {
                    return_types,
                    struct_defs,
                },
            );
            StatementFlow::Continues
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
                owner,
            );
            StatementFlow::Continues
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
                owner,
            );
            StatementFlow::Terminates
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
                owner,
            );
            let mut then_env = env.clone();
            let then_flow = monomorphize_calls_in_stmt_list_impl(
                then_branch,
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
                owner,
            );
            let mut else_env = env.clone();
            let else_flow = monomorphize_calls_in_stmt_list_impl(
                else_branch,
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
                owner,
            );
            let (joined, flow) = join_branch_envs(then_env, then_flow, else_env, else_flow);
            *env = joined;
            flow
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
                owner,
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
                owner,
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
                    owner,
                );
            }
            let mut body_env = env.clone();
            // Runtime loop induction variables are always i32. Keep that
            // resolved semantic fact available while specializing calls in the
            // body; otherwise generated helpers called with `i` silently fall
            // back to their contextual f32 template.
            body_env.shadow_binding(var);
            body_env
                .scalar_types
                .insert(var.clone(), PrimitiveType::I32);
            monomorphize_calls_in_stmt_list_impl(
                body,
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
                owner,
            );
            StatementFlow::Continues
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
                owner,
            );
            let mut body_env = env.clone();
            monomorphize_calls_in_stmt_list_impl(
                body,
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
                owner,
            );
            StatementFlow::Continues
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => StatementFlow::Terminates,
    }
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_stmt_list_impl(
    stmts: &mut [Stmt],
    env: &mut CallTypeEnv,
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
    owner: MonoOwnerContext<'_>,
) -> StatementFlow {
    for stmt in stmts {
        let flow = monomorphize_calls_in_stmt(
            stmt,
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
            owner,
        );
        if flow == StatementFlow::Terminates {
            return flow;
        }
    }
    StatementFlow::Continues
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_assign_target(
    target: &mut AssignTarget,
    env: &CallTypeEnv,
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
    owner: MonoOwnerContext<'_>,
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
                    owner,
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
            owner,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_expr(
    expr: &mut Expr,
    env: &CallTypeEnv,
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
    owner: MonoOwnerContext<'_>,
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
                    owner,
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
                        if owner.type_params.contains(param) {
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
                let call_display_name = sig.display_name.as_deref().unwrap_or(name);
                let Some(mut bindings) = resolve_generic_def_type_bindings(
                    sig,
                    type_args,
                    args,
                    env,
                    return_types,
                    struct_defs,
                    call_display_name,
                    errors,
                ) else {
                    return;
                };
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
                    if let Some(FnParamType::Buffer(buffer)) = param_ty {
                        if let BufferElemType::Generic(s) = &buffer.elem {
                            if sig.type_params.contains(s) {
                                if let Some(prim) = resolved_type_bindings.get(s) {
                                    keys.push(MonoParamKey::ResolvedBuffer(
                                        *prim,
                                        resolved_buffer_channels(buffer),
                                    ));
                                    continue;
                                }
                                // Fall through to Phase 3 inference
                            }
                        }
                    }
                    // buffer<T> {N} — generic fixed buffer collection.
                    if let Some(FnParamType::BufferArray { buffer, len }) = param_ty {
                        if let BufferElemType::Generic(s) = &buffer.elem {
                            if sig.type_params.contains(s) {
                                if let Some(prim) = resolved_type_bindings.get(s) {
                                    keys.push(MonoParamKey::ResolvedBufferArray(
                                        *prim,
                                        resolved_buffer_channels(buffer),
                                        *len,
                                    ));
                                    continue;
                                }
                            }
                        }
                    }
                }

                // Defaults are effective call arguments. They must contribute
                // exactly the same specialization shape as an explicitly
                // supplied expression; otherwise the rewritten call and the
                // generated function signature disagree at the MIR boundary.
                let arg_expr = resolved_args
                    .get(idx)
                    .copied()
                    .flatten()
                    .or_else(|| sig.defaults.get(idx).and_then(Option::as_ref));
                if let Some(arg_expr) = arg_expr {
                    if let Some(key) = infer_mono_arg_key(
                        arg_expr,
                        param_ty,
                        env,
                        generic_templates,
                        owner.proc_types,
                        return_types,
                        struct_defs,
                    ) {
                        keys.push(key);
                    } else {
                        all_resolved = false;
                        break;
                    }
                } else {
                    // A genuinely missing value cannot contribute a call-site
                    // shape. Preserve that distinction for incomplete calls so
                    // it cannot suppress another parameter's specialization.
                    keys.push(if param_ty.is_none() {
                        MonoParamKey::UnresolvedStructural
                    } else {
                        MonoParamKey::Passthrough
                    });
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
                        Some(FnParamType::BufferArray {
                            buffer:
                                BufferType {
                                    elem: BufferElemType::Generic(ref s),
                                    ..
                                },
                            ..
                        }) if sig.type_params.contains(s) => {
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

            let has_unresolved_structural = keys
                .iter()
                .any(|key| matches!(key, MonoParamKey::UnresolvedStructural));

            // A deferred call must retain its source callee name so the next
            // fixed-point iteration can reconsider every argument after nested
            // return types become concrete. Rewriting it to an `__open`
            // specialization would make the call ineligible for refinement.
            if has_unresolved_structural {
                return;
            }

            if !all_resolved {
                return;
            }

            // Check if all keys are passthrough (nothing to monomorphize)
            if keys
                .iter()
                .all(|key| matches!(key, MonoParamKey::Passthrough))
            {
                return;
            }

            let cache_key = (name.clone(), keys.clone());
            let mono_name = if let Some(cached) = mono_cache.get(&cache_key) {
                cached.clone()
            } else {
                let new_name = mono_def_name(name, &keys);

                // Specializations must always clone an immutable source
                // template. A body already rewritten for an open or different
                // call context is not a valid template for another ABI.
                let Some(original) = original_defs.iter().find(|d| d.name == *name) else {
                    return;
                };
                let (gen_def, gen_sig) =
                    generate_mono_def(original, sig, &keys, &new_name, *loc, errors);
                generated_defs.push(gen_def);
                generated_sigs.insert(new_name.clone(), gen_sig);
                refresh_monomorphized_return_types(
                    return_types,
                    original_defs,
                    generated_defs,
                    fn_signatures,
                    generated_sigs,
                    owner.return_type_env,
                    struct_defs,
                );

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
                owner,
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
                owner,
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
                    owner,
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
                owner,
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
                    owner,
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
                owner,
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
                    owner,
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
                owner,
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
                        owner,
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
                    .starts_with("Core.__arr_read_clamp_3.__onda_mono__scalar_i32")
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
            [
                Some(PrimitiveType::I32),
                Some(PrimitiveType::F32),
                Some(PrimitiveType::F32),
                Some(PrimitiveType::F32),
            ]
        );

        let clamp = typed
            .defs
            .iter()
            .find(|def| {
                def.name == "clamp_idx.__onda_mono__scalar_i32__scalar_i32"
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
            .find(|def| def.name == "left.__onda_mono__scalar_i32__scalar_f32")
            .expect("missing first-parameter specialization");
        let second = typed
            .defs
            .iter()
            .find(|def| def.name == "left.__onda_mono__scalar_f32__scalar_i32")
            .expect("missing second-parameter specialization");
        assert_eq!(
            scalar_param_types(first),
            [Some(PrimitiveType::I32), Some(PrimitiveType::F32)]
        );
        assert_eq!(
            scalar_param_types(second),
            [Some(PrimitiveType::F32), Some(PrimitiveType::I32)]
        );
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
        let specialized_name = "pick.__onda_mono__scalar_i32";
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
            ("f32", PrimitiveType::F32),
            ("f64", PrimitiveType::F64),
            ("bool", PrimitiveType::Bool),
        ];
        for (suffix, primitive) in expected {
            let leaf_name = format!("leaf.__onda_mono__scalar_{suffix}");
            let middle_name = format!("middle.__onda_mono__scalar_{suffix}");
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
            .find(|def| def.name == "wrapper.__onda_mono__scalar_i32")
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
            ("identity.__onda_mono__scalar_i32", PrimitiveType::I32),
            ("identity.__onda_mono__scalar_f32", PrimitiveType::F32),
            ("identity.__onda_mono__scalar_i64", PrimitiveType::I64),
            ("identity.__onda_mono__scalar_f64", PrimitiveType::F64),
        ] {
            let specialization = typed
                .defs
                .iter()
                .find(|def| def.name == name)
                .unwrap_or_else(|| panic!("missing literal specialization '{name}'"));
            assert_eq!(scalar_param_types(specialization), [Some(primitive)]);
            assert_eq!(specialization.return_ty, ReturnType::Scalar(primitive));
        }
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
            .any(|def| def.name == "wrapper.__onda_mono__scalar_i32"));
        assert!(typed
            .defs
            .iter()
            .any(|def| def.name == "inner.__onda_mono__scalar_i32"));
        crate::lower_program_to_optimized_mir(&typed)
            .expect("nested integer specializations should lower to MIR");
    }

    #[test]
    fn typed_parameters_do_not_suppress_f32_specialization_of_untyped_parameters() {
        let source = r#"
def classify(x: i32) -> f32:
  return 1.0

def classify(x: f32) -> f32:
  return 2.0

def relay(x, count: i32):
  return classify(x) + f32(count - count)

sample:
  out1 = relay(1.0, 1)
"#;
        let parsed = parse_program(source).expect("mixed parameter source should parse");
        let typed = crate::analyze(parsed)
            .expect("the typed parameter must not suppress f32 specialization");

        let relay = typed
            .defs
            .iter()
            .find(|def| def.name == "relay.__onda_mono__scalar_f32__pass")
            .expect("missing concrete f32 relay specialization");
        assert_eq!(
            scalar_param_types(relay),
            [Some(PrimitiveType::F32), Some(PrimitiveType::I32)]
        );
        let [Stmt::Return {
            expr: Expr::Binary { lhs, .. },
            ..
        }] = relay.body.as_slice()
        else {
            panic!("expected relay to return a binary expression");
        };
        assert!(matches!(
            lhs.as_ref(),
            Expr::UserCall { name, .. }
                if name.starts_with("__onda_ovl_classify") && name.ends_with("_2")
        ));
        crate::lower_program_to_optimized_mir(&typed)
            .expect("the mixed-signature specialization should lower to MIR");
    }

    #[test]
    fn nested_generated_returns_reach_a_monomorphization_fixed_point() {
        let source = r#"
def identity<T>(x: T):
  return x

def relay<T>(x: T):
  return identity(x)

def integer_only<T>(x: T) -> T:
  return ~x

sample:
  out1 = f32(integer_only(relay(1)))
"#;
        let parsed = parse_program(source).expect("nested generic source should parse");
        let typed = crate::analyze(parsed)
            .expect("generated return types should drive enclosing specialization");

        for expected in [
            "identity.__onda_mono__g_i32",
            "relay.__onda_mono__g_i32",
            "integer_only.__onda_mono__g_i32",
        ] {
            let specialization = typed
                .defs
                .iter()
                .find(|def| def.name == expected)
                .unwrap_or_else(|| panic!("missing specialization '{expected}'"));
            assert_eq!(
                specialization.return_ty,
                ReturnType::Scalar(PrimitiveType::I32)
            );
        }
        crate::lower_program_to_optimized_mir(&typed)
            .expect("the converged generic chain should lower to MIR");
    }

    #[test]
    fn dependent_nested_returns_do_not_freeze_partial_specializations() {
        let source = r#"
def identity<T>(value: T) -> T:
  return value

def relay<T>(value: T):
  return identity(value)

def choose(first, second):
  return second

sample:
  result = choose(1, relay(1))
  out1 = f32(result)
"#;
        let parsed = parse_program(source).expect("dependent call source should parse");
        let typed =
            crate::analyze(parsed).expect("the outer call should wait for the nested return type");

        let expected = "choose.__onda_mono__scalar_i32__scalar_i32";
        assert!(
            typed.defs.iter().any(|def| def.name == expected),
            "missing fully concrete outer specialization"
        );
        assert!(
            typed
                .defs
                .iter()
                .all(|def| def.name != "choose.__onda_mono__scalar_i32__open"),
            "a deferred call was frozen as a partially open specialization"
        );
        crate::lower_program_to_optimized_mir(&typed)
            .expect("the refined outer specialization should lower to MIR");
    }

    #[test]
    fn tuple_struct_fields_supply_call_site_element_types() {
        let source = r#"
struct Holder:
  values: (i32, f32) = (1, 2.0)

def first(holder: Holder):
  return holder.values[0]

def integer_only<T>(x: T) -> T:
  return ~x

init:
  holder = Holder()

sample:
  direct = integer_only(holder.values[0])
  nested = integer_only(first(holder))
  out1 = f32(direct + nested)
"#;
        let parsed = parse_program(source).expect("tuple field source should parse");
        let typed = crate::analyze(parsed)
            .expect("tuple field elements should provide concrete call types");

        let specialization = typed
            .defs
            .iter()
            .find(|def| def.name == "integer_only.__onda_mono__g_i32")
            .expect("missing tuple-field-driven i32 specialization");
        assert_eq!(
            scalar_param_types(specialization),
            [Some(PrimitiveType::I32)]
        );
        crate::lower_program_to_optimized_mir(&typed)
            .expect("tuple-field-driven calls should lower to MIR");
    }

    #[test]
    fn omitted_defaults_select_the_same_specialization_as_explicit_arguments() {
        let source = r#"
def implicit_identity(value = 1):
  return value

def generic_identity<T>(value: T = 1) -> T:
  return value

def contextual_default<T>(value: T = T(1)) -> T:
  return value

sample:
  implicit = implicit_identity()
  generic = generic_identity()
  contextual = contextual_default()
  out1 = f32(implicit + generic) + contextual
"#;
        let parsed = parse_program(source).expect("default specialization source should parse");
        let typed = crate::analyze(parsed)
            .expect("omitted defaults should provide concrete specialization types");

        for expected in [
            "implicit_identity.__onda_mono__scalar_i32",
            "generic_identity.__onda_mono__g_i32",
            "contextual_default.__onda_mono__g_f32",
        ] {
            assert!(
                typed.defs.iter().any(|def| def.name == expected),
                "missing default-driven specialization '{expected}'"
            );
        }
        crate::lower_program_to_optimized_mir(&typed)
            .expect("default-driven specializations should lower to MIR");
    }

    #[test]
    fn omitted_dependent_defaults_wait_for_concrete_return_types() {
        let source = r#"
def identity<T>(value: T) -> T:
  return value

def produce():
  return identity(i32(1))

def consume<T>(value: T = produce()) -> T:
  return value

def need_i32(value: i32):
  return value

sample:
  omitted = consume()
  explicit = consume(produce())
  out1 = f32(need_i32(omitted + explicit))
"#;
        let parsed = parse_program(source).expect("dependent default source should parse");
        let typed = crate::analyze(parsed)
            .expect("an omitted dependent default should wait for its return type");

        assert!(
            typed
                .defs
                .iter()
                .any(|def| def.name == "consume.__onda_mono__g_i32"),
            "missing return-type-driven i32 specialization"
        );
        assert!(
            typed
                .defs
                .iter()
                .all(|def| def.name != "consume.__onda_mono__g_f32"),
            "the unresolved default was prematurely specialized as f32"
        );
        crate::lower_program_to_optimized_mir(&typed)
            .expect("dependent-default specialization should lower to MIR");
    }

    #[test]
    fn deferred_specializations_clone_pristine_source_templates() {
        let source = r#"
def helper<T>(value, marker: T):
  return value

def wrapper(value):
  return helper(value, 1)

def identity<T>(value: T):
  return value

def relay<T>(value: T):
  return identity(value)

sample:
  out1 = f32(wrapper(relay(1)))
"#;
        let parsed = parse_program(source).expect("deferred specialization source should parse");
        let typed = crate::analyze(parsed)
            .expect("deferred specializations should use pristine source templates");

        let wrapper = typed
            .defs
            .iter()
            .find(|def| def.name == "wrapper.__onda_mono__scalar_i32")
            .expect("missing concrete wrapper specialization");
        let (callee, _) = returned_call(wrapper);
        assert_eq!(callee, "helper.__onda_mono__scalar_i32__g_i32");
        assert!(
            typed.defs.iter().any(|def| {
                def.name == "helper.__onda_mono__scalar_i32__g_i32"
                    && scalar_param_types(def)
                        == [Some(PrimitiveType::I32), Some(PrimitiveType::I32)]
            }),
            "missing fully concrete nested helper specialization"
        );
        crate::lower_program_to_optimized_mir(&typed)
            .expect("pristine-template specialization should lower to MIR");
    }
}
