use std::collections::{HashMap, HashSet};

use super::overloads::OverloadRewriteEnv;
use crate::*;

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
    /// Resolved generic def type parameter (e.g. T = f32).
    GenericType(PrimitiveType),
}

fn mono_def_name(base: &str, keys: &[MonoParamKey]) -> String {
    let mut suffix = String::new();
    for key in keys {
        match key {
            MonoParamKey::Passthrough => {}
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
            if let Expr::Var { name: var_name, .. } = arg_expr {
                if let Some(elem_ty) = env.array_elem_types.get(var_name) {
                    return Some(MonoParamKey::ResolvedArray(*elem_ty));
                }
            }
            // Default to f32 if we can't infer
            Some(MonoParamKey::ResolvedArray(PrimitiveType::F32))
        }
        Some(FnParamType::ArrayGeneric(_)) | Some(FnParamType::SizedArray { .. }) => {
            if let Expr::Var { name: var_name, .. } = arg_expr {
                if let Some(elem_ty) = env.array_elem_types.get(var_name) {
                    return Some(MonoParamKey::ResolvedArray(*elem_ty));
                }
            }
            Some(MonoParamKey::ResolvedArray(PrimitiveType::F32))
        }
        Some(FnParamType::BareBuffer) => {
            if let Expr::Var { name: var_name, .. } = arg_expr {
                if let Some((elem_ty, channels)) = env.buffer_types.get(var_name) {
                    return Some(MonoParamKey::ResolvedBuffer(*elem_ty, channels.clone()));
                }
            }
            // Default to f32 mono
            Some(MonoParamKey::ResolvedBuffer(
                PrimitiveType::F32,
                TypedBufferChannels::Mono,
            ))
        }
        _ => {
            // For untyped params, check if the arg is a tuple literal — if so,
            // monomorphize with the inferred tuple element types.
            if let Expr::Tuple { values, .. } = arg_expr {
                let elem_tys: Vec<PrimitiveType> = values
                    .iter()
                    .map(|v| infer_tuple_elem_type(v, env))
                    .collect();
                return Some(MonoParamKey::ResolvedTuple(elem_tys));
            }
            Some(MonoParamKey::Passthrough)
        }
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

fn generate_mono_def(
    original: &FunctionDef,
    original_sig: &FnSignature,
    keys: &[MonoParamKey],
    mono_name: &str,
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
        }

        // Clear type_params — the generated def is no longer generic.
        new_def.type_params.clear();
        new_sig.type_params.clear();
    }

    (new_def, new_sig)
}

/// Resolve generic def type parameter bindings from explicit type args or argument inference.
fn resolve_generic_def_type_bindings(
    sig: &FnSignature,
    type_args: &[CallTypeArg],
    args: &[CallArg],
    env: &OverloadRewriteEnv,
    fn_signatures: &HashMap<String, FnSignature>,
    generated_sigs: &HashMap<String, FnSignature>,
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
        // Infer from argument types: for each param typed as Struct("T"), ArrayGeneric("T"),
        // or Buffer(Generic("T")) where T is a type_param, infer T from the argument.
        let resolved_args = super::resolve_call_args(
            args,
            &sig.params,
            &sig.defaults,
            false,
            false,
            "generic def type inference",
            &mut Vec::new(),
        );
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
            if bindings.contains_key(&name) {
                continue; // Already inferred this type param
            }
            if let Some(Some(arg_expr)) = resolved_args.get(idx) {
                // For array/buffer params, infer from the arg's element type.
                let inferred = match param_ty {
                    Some(FnParamType::ArrayGeneric(_)) | Some(FnParamType::SizedArray { .. }) => {
                        if let Expr::Var { name: var_name, .. } = arg_expr {
                            env.array_elem_types.get(var_name).copied()
                        } else {
                            None
                        }
                    }
                    Some(FnParamType::Buffer(BufferType {
                        elem: BufferElemType::Generic(_),
                        ..
                    })) => {
                        if let Expr::Var { name: var_name, .. } = arg_expr {
                            env.buffer_types.get(var_name).map(|(prim, _)| *prim)
                        } else {
                            None
                        }
                    }
                    _ => infer_expr_primitive_type(arg_expr, env, fn_signatures, generated_sigs),
                };
                if let Some(prim) = inferred {
                    bindings.insert(name, prim);
                }
            }
        }
    }

    bindings
}

/// Infer the primitive type of an expression for generic type inference.
fn infer_expr_primitive_type(
    expr: &Expr,
    env: &OverloadRewriteEnv,
    fn_signatures: &HashMap<String, FnSignature>,
    generated_sigs: &HashMap<String, FnSignature>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { .. } => Some(PrimitiveType::I32),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Var { name, .. } => env.scalar_types.get(name).copied(),
        Expr::UserCall { name, .. } => {
            // Look up the callee in generated or existing signatures to infer return type.
            let sig = generated_sigs
                .get(name.as_str())
                .or_else(|| fn_signatures.get(name.as_str()));
            if let Some(sig) = sig {
                if sig.type_params.is_empty() {
                    // Concrete function — heuristic: return type matches first primitive param type.
                    for pt in &sig.param_types {
                        if let Some(FnParamType::Primitive(prim)) = pt {
                            return Some(*prim);
                        }
                    }
                }
            }
            None
        }
        Expr::Binary { lhs, rhs, .. } => {
            // For binary ops, try to infer from either operand.
            infer_expr_primitive_type(lhs, env, fn_signatures, generated_sigs)
                .or_else(|| infer_expr_primitive_type(rhs, env, fn_signatures, generated_sigs))
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn monomorphize_calls_in_stmts(
    stmts: &mut Vec<Stmt>,
    env: &OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    _struct_defs: &HashMap<String, Vec<TypedStructField>>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
    errors: &mut Vec<Diagnostic>,
    enclosing_type_params: &[String],
) {
    for stmt in stmts.iter_mut() {
        monomorphize_calls_in_stmt(
            stmt,
            env,
            mono_eligible,
            fn_signatures,
            original_defs,
            generic_templates,
            generated_defs,
            generated_sigs,
            mono_cache,
            errors,
            enclosing_type_params,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn monomorphize_calls_in_stmt(
    stmt: &mut Stmt,
    env: &OverloadRewriteEnv,
    mono_eligible: &HashSet<String>,
    fn_signatures: &HashMap<String, FnSignature>,
    original_defs: &[FunctionDef],
    generic_templates: &HashSet<String>,
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
    errors: &mut Vec<Diagnostic>,
    enclosing_type_params: &[String],
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { expr, .. } => {
            monomorphize_calls_in_expr(
                expr, env, mono_eligible, fn_signatures, original_defs,
                generic_templates, generated_defs, generated_sigs, mono_cache,
                errors, enclosing_type_params,
            );
        }
        Stmt::Expr { expr, .. } => {
            monomorphize_calls_in_expr(
                expr, env, mono_eligible, fn_signatures, original_defs,
                generic_templates, generated_defs, generated_sigs, mono_cache,
                errors, enclosing_type_params,
            );
        }
        Stmt::Return { expr, .. } => {
            monomorphize_calls_in_expr(
                expr, env, mono_eligible, fn_signatures, original_defs,
                generic_templates, generated_defs, generated_sigs, mono_cache,
                errors, enclosing_type_params,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            monomorphize_calls_in_expr(
                cond, env, mono_eligible, fn_signatures, original_defs,
                generic_templates, generated_defs, generated_sigs, mono_cache,
                errors, enclosing_type_params,
            );
            for s in then_branch.iter_mut() {
                monomorphize_calls_in_stmt(
                    s, env, mono_eligible, fn_signatures, original_defs,
                    generic_templates, generated_defs, generated_sigs, mono_cache,
                    errors, enclosing_type_params,
                );
            }
            for s in else_branch.iter_mut() {
                monomorphize_calls_in_stmt(
                    s, env, mono_eligible, fn_signatures, original_defs,
                    generic_templates, generated_defs, generated_sigs, mono_cache,
                    errors, enclosing_type_params,
                );
            }
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            for s in body.iter_mut() {
                monomorphize_calls_in_stmt(
                    s, env, mono_eligible, fn_signatures, original_defs,
                    generic_templates, generated_defs, generated_sigs, mono_cache,
                    errors, enclosing_type_params,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
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
    generated_defs: &mut Vec<FunctionDef>,
    generated_sigs: &mut HashMap<String, FnSignature>,
    mono_cache: &mut HashMap<(String, Vec<MonoParamKey>), String>,
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
                    &mut arg.expr, env, mono_eligible, fn_signatures,
                    original_defs, generic_templates, generated_defs,
                    generated_sigs, mono_cache, errors, enclosing_type_params,
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
                    fn_signatures,
                    generated_sigs,
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
                    // buffer[T] / buffer[T[N]] — generic element type buffer param
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
                                        if let Expr::Var { name: var_name, .. } = arg {
                                            env.buffer_types.get(var_name).map(|(_, ch)| ch.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or(TypedBufferChannels::Mono);
                                keys.push(MonoParamKey::ResolvedBuffer(*prim, inferred_channels));
                                continue;
                            }
                            // Fall through to Phase 3 inference
                        }
                    }
                }

                let arg_expr = resolved_args.get(idx).and_then(|a| a.as_ref());
                if let Some(arg_expr) = arg_expr {
                    if let Some(key) =
                        infer_mono_arg_key(arg_expr, param_ty, env, generic_templates)
                    {
                        keys.push(key);
                    } else {
                        all_resolved = false;
                        break;
                    }
                } else {
                    // Use default — passthrough
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
                    let (gen_def, gen_sig) =
                        generate_mono_def(original, sig, &keys, &new_name, generic_templates, errors);
                    generated_defs.push(gen_def);
                    generated_sigs.insert(new_name.clone(), gen_sig);
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
                lhs, env, mono_eligible, fn_signatures, original_defs,
                generic_templates, generated_defs, generated_sigs, mono_cache,
                errors, enclosing_type_params,
            );
            monomorphize_calls_in_expr(
                rhs, env, mono_eligible, fn_signatures, original_defs,
                generic_templates, generated_defs, generated_sigs, mono_cache,
                errors, enclosing_type_params,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args.iter_mut() {
                monomorphize_calls_in_expr(
                    arg, env, mono_eligible, fn_signatures, original_defs,
                    generic_templates, generated_defs, generated_sigs, mono_cache,
                    errors, enclosing_type_params,
                );
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            monomorphize_calls_in_expr(
                inner, env, mono_eligible, fn_signatures, original_defs,
                generic_templates, generated_defs, generated_sigs, mono_cache,
                errors, enclosing_type_params,
            );
        }
        Expr::ArrayLiteral { values: elems, .. } | Expr::Tuple { values: elems, .. } => {
            for elem in elems.iter_mut() {
                monomorphize_calls_in_expr(
                    elem, env, mono_eligible, fn_signatures, original_defs,
                    generic_templates, generated_defs, generated_sigs, mono_cache,
                    errors, enclosing_type_params,
                );
            }
        }
        _ => {}
    }
}
