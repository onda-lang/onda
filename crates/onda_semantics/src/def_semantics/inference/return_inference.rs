use super::super::call_types::{
    infer_scalar_expr_type, infer_tuple_arg_types, join_branch_envs,
    update_call_type_env_after_assign, CallTypeContext, CallTypeEnv, StatementFlow,
};
use super::*;
use crate::{effective_untyped_assignment_type, require_expr_assignable_type, ReturnType};
use onda_frontend::{
    ast::{FnReturnScalarType, FnReturnType},
    SourceLoc,
};

#[derive(Clone)]
struct ObservedReturn<'a> {
    expr: &'a Expr,
    ty: ReturnType,
}

fn try_resolve_declared_return_type(def: &FunctionDef) -> Option<ReturnType> {
    fn resolve_scalar(
        ty: &FnReturnScalarType,
        type_params: &[String],
        strict: bool,
        def_name: &str,
        loc: SourceLoc,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<PrimitiveType> {
        match ty {
            FnReturnScalarType::Primitive(prim) => Some(*prim),
            FnReturnScalarType::Named(name) if type_params.contains(name) => None,
            FnReturnScalarType::Named(name) => {
                if strict {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "function '{def_name}' return type '{}' is not supported; return annotations only support primitive scalars, generic primitive type parameters, and tuples of those",
                            name
                        ),
                        loc,
                    ));
                }
                None
            }
        }
    }

    fn resolve_inner(
        def: &FunctionDef,
        strict: bool,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<ReturnType> {
        let return_ty = def.return_ty.as_ref()?;
        let loc = def.return_ty_loc.or(def.loc).into();
        match return_ty {
            FnReturnType::Scalar(scalar) => {
                let prim =
                    resolve_scalar(scalar, &def.type_params, strict, &def.name, loc, errors)?;
                Some(ReturnType::Scalar(prim))
            }
            FnReturnType::Array { .. } => {
                if strict {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "function '{}' array return types are only supported for const defs",
                            def.name
                        ),
                        loc,
                    ));
                }
                None
            }
            FnReturnType::Tuple(elems) => {
                let mut resolved = Vec::with_capacity(elems.len());
                for elem in elems {
                    let prim =
                        resolve_scalar(elem, &def.type_params, strict, &def.name, loc, errors)?;
                    resolved.push(prim);
                }
                Some(ReturnType::Tuple(resolved))
            }
        }
    }

    resolve_inner(def, false, &mut Vec::new())
}

fn validate_declared_return_type(
    def: &FunctionDef,
    display_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<ReturnType> {
    fn resolve_scalar(
        ty: &FnReturnScalarType,
        type_params: &[String],
        def_name: &str,
        loc: SourceLoc,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<PrimitiveType> {
        match ty {
            FnReturnScalarType::Primitive(prim) => Some(*prim),
            FnReturnScalarType::Named(name) if type_params.contains(name) => None,
            FnReturnScalarType::Named(name) => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "function '{def_name}' return type '{}' is not supported; return annotations only support primitive scalars, generic primitive type parameters, and tuples of those",
                        name
                    ),
                    loc,
                ));
                None
            }
        }
    }

    let return_ty = def.return_ty.as_ref()?;
    let loc = def.return_ty_loc.or(def.loc).into();
    match return_ty {
        FnReturnType::Scalar(scalar) => {
            let prim = resolve_scalar(scalar, &def.type_params, display_name, loc, errors)?;
            Some(ReturnType::Scalar(prim))
        }
        FnReturnType::Array { .. } => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "function '{}' array return types are only supported for const defs",
                    display_name
                ),
                loc,
            ));
            None
        }
        FnReturnType::Tuple(elems) => {
            let mut resolved = Vec::with_capacity(elems.len());
            for elem in elems {
                let prim = resolve_scalar(elem, &def.type_params, display_name, loc, errors)?;
                resolved.push(prim);
            }
            Some(ReturnType::Tuple(resolved))
        }
    }
}

fn infer_return_scalar_type(
    expr: &Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
    require_known_calls: bool,
) -> Option<PrimitiveType> {
    let inferred = infer_scalar_expr_type(expr, env, context);
    effective_untyped_assignment_type(expr, inferred)
        .or(inferred)
        .or_else(|| (!require_known_calls).then_some(PrimitiveType::F32))
}

fn infer_return_tuple_type(
    expr: &Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
    require_known_calls: bool,
) -> Option<Vec<PrimitiveType>> {
    if let Some(types) = infer_tuple_arg_types(expr, env, context) {
        return Some(types);
    }
    if require_known_calls {
        return None;
    }
    match expr {
        Expr::Tuple { values, .. } => Some(vec![PrimitiveType::F32; values.len()]),
        _ => None,
    }
}

fn infer_stmt_returns_for_def_return_inference<'a>(
    stmts: &'a [Stmt],
    env: &mut CallTypeEnv,
    context: CallTypeContext<'_>,
    out: &mut Vec<ObservedReturn<'a>>,
    require_known_calls: bool,
    complete: &mut bool,
) -> StatementFlow {
    for stmt in stmts {
        let flow = match stmt {
            Stmt::Const { .. } => StatementFlow::Continues,
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                expr,
                ..
            } => {
                update_call_type_env_after_assign(
                    target,
                    *decl_ty,
                    generic_decl_ty.as_deref(),
                    expr,
                    env,
                    context,
                );
                if !require_known_calls {
                    match target {
                        AssignTarget::Var(name)
                            if !name.contains('.') && !env.has_binding(name) =>
                        {
                            let ty = infer_return_scalar_type(expr, env, context, false)
                                .unwrap_or(PrimitiveType::F32);
                            env.scalar_types.insert(name.clone(), ty);
                        }
                        AssignTarget::Tuple(names) => {
                            for name in names {
                                if !env.has_binding(name) {
                                    env.scalar_types.insert(name.clone(), PrimitiveType::F32);
                                }
                            }
                        }
                        AssignTarget::Var(_)
                        | AssignTarget::Index { .. }
                        | AssignTarget::Slice { .. } => {}
                    }
                }
                StatementFlow::Continues
            }
            Stmt::Expr { .. } => StatementFlow::Continues,
            Stmt::Return { expr, .. } => {
                if let Some(elem_tys) =
                    infer_return_tuple_type(expr, env, context, require_known_calls)
                {
                    out.push(ObservedReturn {
                        expr,
                        ty: ReturnType::Tuple(elem_tys),
                    });
                } else {
                    let ty = infer_return_scalar_type(expr, env, context, require_known_calls);
                    if let Some(ty) = ty {
                        out.push(ObservedReturn {
                            expr,
                            ty: ReturnType::Scalar(ty),
                        });
                    } else {
                        *complete = false;
                    }
                }
                StatementFlow::Terminates
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                let mut then_env = env.clone();
                let mut else_env = env.clone();
                let then_flow = infer_stmt_returns_for_def_return_inference(
                    then_branch,
                    &mut then_env,
                    context,
                    out,
                    require_known_calls,
                    complete,
                );
                let else_flow = infer_stmt_returns_for_def_return_inference(
                    else_branch,
                    &mut else_env,
                    context,
                    out,
                    require_known_calls,
                    complete,
                );
                let (joined, flow) = join_branch_envs(then_env, then_flow, else_env, else_flow);
                *env = joined;
                flow
            }
            Stmt::For { var, body, .. } => {
                let mut loop_env = env.clone();
                loop_env.shadow_binding(var);
                loop_env
                    .scalar_types
                    .insert(var.clone(), PrimitiveType::I32);
                infer_stmt_returns_for_def_return_inference(
                    body,
                    &mut loop_env,
                    context,
                    out,
                    require_known_calls,
                    complete,
                );
                StatementFlow::Continues
            }
            Stmt::While { body, .. } => {
                let mut loop_env = env.clone();
                infer_stmt_returns_for_def_return_inference(
                    body,
                    &mut loop_env,
                    context,
                    out,
                    require_known_calls,
                    complete,
                );
                StatementFlow::Continues
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => StatementFlow::Terminates,
        };
        if flow == StatementFlow::Terminates {
            return flow;
        }
    }
    StatementFlow::Continues
}

fn collect_def_return_observations<'a>(
    def: &'a FunctionDef,
    sig: &FnSignature,
    env_seed: &CallTypeEnv,
    full_return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    require_known_calls: bool,
) -> (Vec<ObservedReturn<'a>>, bool) {
    let mut env = env_seed.clone();
    env.set_owner_type_params(&sig.type_params);
    for (index, param) in sig.params.iter().enumerate() {
        env.bind_function_param_type(
            param,
            sig.param_types.get(index).and_then(Option::as_ref),
            &sig.type_params,
        );
    }

    let mut returns = Vec::<ObservedReturn>::new();
    let mut complete = true;
    infer_stmt_returns_for_def_return_inference(
        &def.body,
        &mut env,
        CallTypeContext {
            return_types: full_return_types,
            struct_defs,
        },
        &mut returns,
        require_known_calls,
        &mut complete,
    );
    (returns, complete)
}

fn merge_return_types(a: ReturnType, b: ReturnType) -> Option<ReturnType> {
    match (&a, &b) {
        (ReturnType::Scalar(s1), ReturnType::Scalar(s2)) => {
            merge_inferred_return_types(*s1, *s2).map(ReturnType::Scalar)
        }
        (ReturnType::Tuple(t1), ReturnType::Tuple(t2)) if t1.len() == t2.len() => {
            let merged: Option<Vec<PrimitiveType>> = t1
                .iter()
                .zip(t2.iter())
                .map(|(a, b)| merge_inferred_return_types(*a, *b))
                .collect();
            merged.map(ReturnType::Tuple)
        }
        _ => None, // scalar/tuple mismatch or different tuple lengths
    }
}

fn infer_def_return_type(
    def: &FunctionDef,
    sig: &FnSignature,
    env_seed: &CallTypeEnv,
    full_return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    require_known_calls: bool,
) -> Option<ReturnType> {
    if let Some(declared) = try_resolve_declared_return_type(def) {
        return Some(declared);
    }
    let (returns, complete) = collect_def_return_observations(
        def,
        sig,
        env_seed,
        full_return_types,
        struct_defs,
        require_known_calls,
    );
    if require_known_calls && !complete {
        return None;
    }
    let mut it = returns.into_iter().map(|ret| ret.ty);
    let Some(mut out) = it.next() else {
        return (!require_known_calls).then_some(ReturnType::Scalar(PrimitiveType::F32));
    };
    for ty in it {
        let Some(merged) = merge_return_types(out, ty) else {
            return (!require_known_calls).then_some(ReturnType::Scalar(PrimitiveType::F32));
        };
        out = merged;
    }
    Some(out)
}

/// Infers one definition against an already-established strict return-type
/// environment. Appending a specialization cannot invalidate entries in that
/// environment, so callers can publish its result without rebuilding the
/// whole-program fixed point.
pub(crate) fn infer_known_def_return_type(
    def: &FunctionDef,
    sig: &FnSignature,
    env_seed: &CallTypeEnv,
    return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Option<ReturnType> {
    infer_def_return_type(def, sig, env_seed, return_types, struct_defs, true)
}

fn format_primitive_type(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

fn format_return_type(ty: &ReturnType) -> String {
    match ty {
        ReturnType::Scalar(ty) => format_primitive_type(*ty).to_owned(),
        ReturnType::Tuple(elem_tys) => {
            let elems = elem_tys
                .iter()
                .map(|ty| format_primitive_type(*ty))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({elems})")
        }
    }
}

fn return_type_is_assignable(src: &ReturnType, dst: &ReturnType) -> bool {
    match (src, dst) {
        (ReturnType::Scalar(src), ReturnType::Scalar(dst)) => {
            *src == *dst || can_implicitly_assign(*src, *dst)
        }
        (ReturnType::Tuple(src), ReturnType::Tuple(dst)) if src.len() == dst.len() => src
            .iter()
            .zip(dst.iter())
            .all(|(src, dst)| *src == *dst || can_implicitly_assign(*src, *dst)),
        _ => false,
    }
}

/// Returns `true` when execution entering `statements` cannot reach the end of
/// the block without returning a value.
///
/// Loops are deliberately conservative here: `for` and `while` may execute
/// zero times (or may terminate via `break`), so a return nested in a loop does
/// not make the enclosing function total. A return after the loop can still
/// make the function total in the usual sequential way.
fn statements_must_return_value(statements: &[Stmt]) -> bool {
    for statement in statements {
        match statement {
            Stmt::Return { .. } => return true,
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } if statements_must_return_value(then_branch)
                && statements_must_return_value(else_branch) =>
            {
                return true;
            }
            Stmt::Const { .. }
            | Stmt::Assign { .. }
            | Stmt::Expr { .. }
            | Stmt::If { .. }
            | Stmt::For { .. }
            | Stmt::While { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => {}
        }
    }
    false
}

fn statements_contain_return(statements: &[Stmt]) -> bool {
    statements.iter().any(|statement| match statement {
        Stmt::Return { .. } => true,
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => statements_contain_return(then_branch) || statements_contain_return(else_branch),
        Stmt::For { body, .. } | Stmt::While { body, .. } => statements_contain_return(body),
        Stmt::Const { .. }
        | Stmt::Assign { .. }
        | Stmt::Expr { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => false,
    })
}

/// Enforces the source-language result contract before monomorphization can
/// discard unused generic templates. A function is result-bearing when it has
/// an explicit return annotation or contains a value return. Result-bearing
/// functions must return a value on every structurally reachable fallthrough
/// path; functions with no return and no annotation remain ordinary no-result
/// functions.
pub(crate) fn validate_def_return_control_flow(
    defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    for def in defs {
        let returns_value = def.return_ty.is_some() || statements_contain_return(&def.body);
        if returns_value && !statements_must_return_value(&def.body) {
            let display_name = fn_signatures
                .get(&def.name)
                .and_then(|signature| signature.display_name.as_deref())
                .unwrap_or(&def.name);
            errors.push(Diagnostic::semantic_span(
                format!(
                    "function '{}' returns a value, but not all reachable paths return a value",
                    display_name
                ),
                def.loc,
            ));
        }
    }
}

fn validate_return_observation(
    def_name: &str,
    observed: &ObservedReturn<'_>,
    expected: &ReturnType,
    errors: &mut Vec<Diagnostic>,
) {
    match (expected, &observed.ty, observed.expr) {
        (ReturnType::Scalar(expected_ty), ReturnType::Scalar(src_ty), expr) => {
            require_expr_assignable_type(
                expr,
                Some(*src_ty),
                *expected_ty,
                &format!("return in function '{def_name}'"),
                errors,
            );
        }
        (
            ReturnType::Tuple(expected_tys),
            ReturnType::Tuple(src_tys),
            Expr::Tuple { values, .. },
        ) if src_tys.len() == expected_tys.len() && values.len() == expected_tys.len() => {
            for ((value, src_ty), expected_ty) in
                values.iter().zip(src_tys.iter()).zip(expected_tys.iter())
            {
                require_expr_assignable_type(
                    value,
                    Some(*src_ty),
                    *expected_ty,
                    &format!("return in function '{def_name}'"),
                    errors,
                );
            }
        }
        _ if return_type_is_assignable(&observed.ty, expected) => {}
        _ => errors.push(Diagnostic::semantic_span(
            format!(
                "return in function '{def_name}' type mismatch: cannot assign {} to {}",
                format_return_type(&observed.ty),
                format_return_type(expected)
            ),
            observed.expr.loc(),
        )),
    }
}

pub(crate) fn validate_def_return_types(
    defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    env_seed: &CallTypeEnv,
    full_return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    for def in defs {
        let Some(sig) = fn_signatures.get(&def.name) else {
            continue;
        };
        let display_name = sig.display_name.as_deref().unwrap_or(&def.name);
        let expected = if def.return_ty.is_some() {
            let Some(expected) = validate_declared_return_type(def, display_name, errors) else {
                continue;
            };
            expected
        } else {
            let Some(expected) = full_return_types.get(&def.name).cloned() else {
                continue;
            };
            expected
        };
        let (observed_returns, _) = collect_def_return_observations(
            def,
            sig,
            env_seed,
            full_return_types,
            struct_defs,
            false,
        );
        for observed in &observed_returns {
            validate_return_observation(display_name, observed, &expected, errors);
        }
    }
}

fn infer_def_return_types_impl(
    defs: &[FunctionDef],
    generated_defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    generated_signatures: &HashMap<String, FnSignature>,
    env_seed: &CallTypeEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    require_known_calls: bool,
) -> HashMap<String, ReturnType> {
    let all_defs = || defs.iter().chain(generated_defs);
    let mut out = if require_known_calls {
        all_defs()
            .filter_map(|def| {
                try_resolve_declared_return_type(def).map(|ty| (def.name.clone(), ty))
            })
            .collect::<HashMap<_, _>>()
    } else {
        all_defs()
            .map(|def| (def.name.clone(), ReturnType::Scalar(PrimitiveType::F32)))
            .collect::<HashMap<_, _>>()
    };
    for _ in 0..defs
        .len()
        .saturating_add(generated_defs.len())
        .saturating_add(1)
    {
        let mut changed = false;
        for def in all_defs() {
            let Some(sig) = generated_signatures
                .get(&def.name)
                .or_else(|| fn_signatures.get(&def.name))
            else {
                continue;
            };
            let Some(inferred) =
                infer_def_return_type(def, sig, env_seed, &out, struct_defs, require_known_calls)
            else {
                continue;
            };
            if out.get(&def.name) != Some(&inferred) {
                out.insert(def.name.clone(), inferred);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

pub(crate) fn infer_def_return_types(
    defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    env_seed: &CallTypeEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> HashMap<String, ReturnType> {
    infer_def_return_types_impl(
        defs,
        &[],
        fn_signatures,
        &HashMap::new(),
        env_seed,
        struct_defs,
        false,
    )
}

/// Infers only return types whose complete expression dependencies are known.
/// This is the call-rewrite contract: an unresolved call must defer its caller
/// instead of silently publishing the ordinary untyped f32 fallback.
pub(crate) fn infer_known_def_return_types(
    defs: &[FunctionDef],
    generated_defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    generated_signatures: &HashMap<String, FnSignature>,
    env_seed: &CallTypeEnv,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> HashMap<String, ReturnType> {
    infer_def_return_types_impl(
        defs,
        generated_defs,
        fn_signatures,
        generated_signatures,
        env_seed,
        struct_defs,
        true,
    )
}
