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
        Some(FnParamType::ArrayGeneric(_)) => {
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
) -> (FunctionDef, FnSignature) {
    let mut new_def = original.clone();
    new_def.name = mono_name.to_owned();
    let mut new_sig = original_sig.clone();

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
                if let Some(param) = new_def.params.get_mut(idx) {
                    param.ty = Some(FnParamType::Array(Some(*elem_ty)));
                }
                if let Some(pt) = new_sig.param_types.get_mut(idx) {
                    *pt = Some(FnParamType::Array(Some(*elem_ty)));
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
        }
    }

    // Also need to desugar method calls in the mono body if we resolved struct params
    // This happens when the original def body calls methods on a generic struct param.
    // The method desugaring already happened on the original, so we just need the param
    // type to be correct for inference to work.

    (new_def, new_sig)
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
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { expr, .. } => {
            monomorphize_calls_in_expr(
                expr,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
            );
        }
        Stmt::Expr { expr, .. } => {
            monomorphize_calls_in_expr(
                expr,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
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
                generated_defs,
                generated_sigs,
                mono_cache,
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
                generated_defs,
                generated_sigs,
                mono_cache,
            );
            for s in then_branch.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }
            for s in else_branch.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            for s in body.iter_mut() {
                monomorphize_calls_in_stmt(
                    s,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
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
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            // Recurse into arg expressions first
            for arg in args.iter_mut() {
                monomorphize_calls_in_expr(
                    &mut arg.expr,
                    env,
                    mono_eligible,
                    fn_signatures,
                    original_defs,
                    generic_templates,
                    generated_defs,
                    generated_sigs,
                    mono_cache,
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
                        generate_mono_def(original, sig, &keys, &new_name, generic_templates);
                    generated_defs.push(gen_def);
                    generated_sigs.insert(new_name.clone(), gen_sig);
                }

                mono_cache.insert(cache_key, new_name.clone());
                new_name
            };

            *name = mono_name;
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
                generated_defs,
                generated_sigs,
                mono_cache,
            );
            monomorphize_calls_in_expr(
                rhs,
                env,
                mono_eligible,
                fn_signatures,
                original_defs,
                generic_templates,
                generated_defs,
                generated_sigs,
                mono_cache,
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
                    generated_defs,
                    generated_sigs,
                    mono_cache,
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
                generated_defs,
                generated_sigs,
                mono_cache,
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
                    generated_defs,
                    generated_sigs,
                    mono_cache,
                );
            }
        }
        _ => {}
    }
}
