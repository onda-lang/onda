use super::*;
use crate::{require_expr_assignable_type, ReturnType};

#[derive(Clone)]
struct ObservedReturn<'a> {
    expr: &'a Expr,
    ty: ReturnType,
}

fn infer_expr_type_for_def_return_inference_with_call_overrides(
    expr: &Expr,
    locals: &HashMap<String, PrimitiveType>,
    fn_return_types: &HashMap<String, PrimitiveType>,
    call_return_type_overrides: Option<&HashMap<String, PrimitiveType>>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { value: v, .. } => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::ArrayLiteral { .. }
        | Expr::ArrayCtor { .. }
        | Expr::Slice { .. }
        | Expr::Tuple { .. } => None,
        Expr::Var { name, .. } => builtin_constant_type(name).or_else(|| locals.get(name).copied()),
        Expr::Index { base, .. } => locals.get(base).copied().or(Some(PrimitiveType::F32)),
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Compare { .. } | Expr::Logical { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr, .. } => {
            let inner = infer_expr_type_for_def_return_inference_with_call_overrides(
                expr,
                locals,
                fn_return_types,
                call_return_type_overrides,
            )?;
            match inner {
                PrimitiveType::I32 | PrimitiveType::I64 => Some(inner),
                _ => None,
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let l = infer_expr_type_for_def_return_inference_with_call_overrides(
                lhs,
                locals,
                fn_return_types,
                call_return_type_overrides,
            )?;
            let r = infer_expr_type_for_def_return_inference_with_call_overrides(
                rhs,
                locals,
                fn_return_types,
                call_return_type_overrides,
            )?;
            match op {
                omni_frontend::BinaryOp::BitAnd
                | omni_frontend::BinaryOp::BitOr
                | omni_frontend::BinaryOp::BitXor
                | omni_frontend::BinaryOp::ShiftLeft
                | omni_frontend::BinaryOp::ShiftRight => match (l, r) {
                    (PrimitiveType::I64, PrimitiveType::I32)
                    | (PrimitiveType::I32, PrimitiveType::I64)
                    | (PrimitiveType::I64, PrimitiveType::I64) => Some(PrimitiveType::I64),
                    (PrimitiveType::I32, PrimitiveType::I32) => Some(PrimitiveType::I32),
                    _ => None,
                },
                _ => merge_inferred_return_types(l, r),
            }
        }
        Expr::Call { func, args, .. } => {
            let arg_tys = args
                .iter()
                .filter_map(|arg| {
                    infer_expr_type_for_def_return_inference_with_call_overrides(
                        arg,
                        locals,
                        fn_return_types,
                        call_return_type_overrides,
                    )
                })
                .collect::<Vec<_>>();
            if arg_tys.len() != args.len() {
                return None;
            }
            match func {
                BuiltinFn::Abs => arg_tys.first().copied(),
                BuiltinFn::Min | BuiltinFn::Max => {
                    let lhs = arg_tys.first().copied().unwrap_or(PrimitiveType::F32);
                    let rhs = arg_tys.get(1).copied().unwrap_or(PrimitiveType::F32);
                    merge_inferred_return_types(lhs, rhs)
                }
                BuiltinFn::Pow => Some(if arg_tys.iter().any(|t| *t == PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
                _ => Some(if arg_tys.iter().any(|t| *t == PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
            }
        }
        Expr::UserCall { name, args, .. } => {
            if let Some(base) = parse_array_len_instance_base(name) {
                if is_builtin_receiver_for_return_inference(base, locals) {
                    return Some(PrimitiveType::I32);
                }
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                if is_builtin_receiver_for_return_inference(base, locals) {
                    return Some(PrimitiveType::I32);
                }
            }
            if let Some(base) = parse_buffer_samplerate_instance_base(name) {
                if is_builtin_receiver_for_return_inference(base, locals) {
                    return Some(PrimitiveType::F32);
                }
            }
            if is_internal_buffer_2d_fn(name) {
                if let Some(CallArg {
                    expr: Expr::Var { name: base, .. },
                    ..
                }) = args.first()
                {
                    if let Some(ty) = locals.get(base).copied() {
                        return Some(ty);
                    }
                }
            }
            if let Some(overrides) = call_return_type_overrides {
                if let Some(ty) = overrides.get(name).copied() {
                    return Some(ty);
                }
            }
            fn_return_types
                .get(name)
                .copied()
                .or(Some(PrimitiveType::F32))
        }
    }
}

pub(crate) fn infer_expr_type_for_def_return_inference(
    expr: &Expr,
    locals: &HashMap<String, PrimitiveType>,
    fn_return_types: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    infer_expr_type_for_def_return_inference_with_call_overrides(
        expr,
        locals,
        fn_return_types,
        None,
    )
}

fn is_builtin_receiver_for_return_inference(
    base: &str,
    locals: &HashMap<String, PrimitiveType>,
) -> bool {
    if locals.contains_key(base) {
        return true;
    }
    let mut parts = base.split('.');
    let Some(root) = parts.next() else {
        return false;
    };
    let Some(_field) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    locals.contains_key(root)
}

fn infer_tuple_type_from_expr(
    expr: &Expr,
    locals: &HashMap<String, PrimitiveType>,
    fn_return_types: &HashMap<String, PrimitiveType>,
    full_return_types: &HashMap<String, ReturnType>,
    tuple_locals: &HashMap<String, Vec<PrimitiveType>>,
) -> Option<Vec<PrimitiveType>> {
    match expr {
        Expr::Tuple { values, .. } => Some(
            values
                .iter()
                .map(|v| {
                    infer_expr_type_for_def_return_inference(v, locals, fn_return_types)
                        .unwrap_or(PrimitiveType::F32)
                })
                .collect(),
        ),
        Expr::UserCall { name, .. } => match full_return_types.get(name.as_str()) {
            Some(ReturnType::Tuple(tys)) => Some(tys.clone()),
            _ => None,
        },
        Expr::Var { name, .. } => tuple_locals.get(name).cloned(),
        _ => None,
    }
}

fn infer_stmt_returns_for_def_return_inference<'a>(
    stmts: &'a [Stmt],
    locals: &mut HashMap<String, PrimitiveType>,
    fn_return_types: &HashMap<String, PrimitiveType>,
    full_return_types: &HashMap<String, ReturnType>,
    tuple_locals: &mut HashMap<String, Vec<PrimitiveType>>,
    out: &mut Vec<ObservedReturn<'a>>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } => {}
            Stmt::Assign {
                target: AssignTarget::Var(name),
                decl_ty,
                expr,
                ..
            } => {
                if split_simple_field_path(name).is_some() {
                    continue;
                }
                if matches!(expr, Expr::ArrayCtor { .. }) {
                    continue;
                }
                // Check if this assigns a tuple to a variable
                if let Some(tys) = infer_tuple_type_from_expr(
                    expr,
                    locals,
                    fn_return_types,
                    full_return_types,
                    tuple_locals,
                ) {
                    tuple_locals.insert(name.clone(), tys);
                    continue;
                }
                let inferred =
                    infer_expr_type_for_def_return_inference(expr, locals, fn_return_types);
                let target_ty = (*decl_ty)
                    .or_else(|| locals.get(name).copied())
                    .or(inferred)
                    .unwrap_or(PrimitiveType::F32);
                locals.entry(name.clone()).or_insert(target_ty);
            }
            Stmt::Assign {
                target: AssignTarget::Tuple(_),
                ..
            } => {}
            Stmt::Assign {
                target: AssignTarget::Index { .. },
                ..
            }
            | Stmt::Assign {
                target: AssignTarget::Slice { .. },
                ..
            } => {}
            Stmt::Expr { .. } => {}
            Stmt::Return { expr, .. } => {
                if let Some(elem_tys) = infer_tuple_type_from_expr(
                    expr,
                    locals,
                    fn_return_types,
                    full_return_types,
                    tuple_locals,
                ) {
                    out.push(ObservedReturn {
                        expr,
                        ty: ReturnType::Tuple(elem_tys),
                    });
                } else {
                    let ty =
                        infer_expr_type_for_def_return_inference(expr, locals, fn_return_types)
                            .unwrap_or(PrimitiveType::F32);
                    out.push(ObservedReturn {
                        expr,
                        ty: ReturnType::Scalar(ty),
                    });
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                let mut then_locals = locals.clone();
                let mut else_locals = locals.clone();
                let mut then_tuple_locals = tuple_locals.clone();
                let mut else_tuple_locals = tuple_locals.clone();
                infer_stmt_returns_for_def_return_inference(
                    then_branch,
                    &mut then_locals,
                    fn_return_types,
                    full_return_types,
                    &mut then_tuple_locals,
                    out,
                );
                infer_stmt_returns_for_def_return_inference(
                    else_branch,
                    &mut else_locals,
                    fn_return_types,
                    full_return_types,
                    &mut else_tuple_locals,
                    out,
                );
                let mut merged = locals.clone();
                for (name, then_ty) in &then_locals {
                    if let Some(else_ty) = else_locals.get(name) {
                        if then_ty == else_ty {
                            merged.insert(name.clone(), *then_ty);
                        }
                    }
                }
                *locals = merged;
                for (name, then_tys) in &then_tuple_locals {
                    if let Some(else_tys) = else_tuple_locals.get(name) {
                        if then_tys == else_tys {
                            tuple_locals.insert(name.clone(), then_tys.clone());
                        }
                    }
                }
            }
            Stmt::For { var, body, .. } => {
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone(), PrimitiveType::I32);
                let mut loop_tuple_locals = tuple_locals.clone();
                infer_stmt_returns_for_def_return_inference(
                    body,
                    &mut loop_locals,
                    fn_return_types,
                    full_return_types,
                    &mut loop_tuple_locals,
                    out,
                );
            }
            Stmt::While { body, .. } => {
                let mut loop_locals = locals.clone();
                let mut loop_tuple_locals = tuple_locals.clone();
                infer_stmt_returns_for_def_return_inference(
                    body,
                    &mut loop_locals,
                    fn_return_types,
                    full_return_types,
                    &mut loop_tuple_locals,
                    out,
                );
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
}

fn collect_def_return_observations<'a>(
    def: &'a FunctionDef,
    sig: &FnSignature,
    fn_return_types: &HashMap<String, PrimitiveType>,
    full_return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> Vec<ObservedReturn<'a>> {
    let mut locals = HashMap::<String, PrimitiveType>::new();
    let mut tuple_locals = HashMap::<String, Vec<PrimitiveType>>::new();
    for (idx, param) in sig.params.iter().enumerate() {
        match sig.param_types.get(idx).and_then(|ty| ty.as_ref()) {
            Some(FnParamType::Primitive(prim)) => {
                locals.insert(param.clone(), *prim);
            }
            Some(FnParamType::Struct(struct_name)) => {
                if let Some(fields) = struct_defs.get(struct_name) {
                    for field in fields {
                        if let TypedFieldType::Scalar(prim) = field.ty {
                            locals.insert(format!("{param}.{}", field.name), prim);
                        }
                    }
                }
                locals.insert(param.clone(), PrimitiveType::F32);
            }
            Some(FnParamType::Buffer(_)) | Some(FnParamType::BareBuffer) => {
                locals.insert(param.clone(), PrimitiveType::F32);
            }
            Some(FnParamType::Array(Some(prim))) => {
                locals.insert(param.clone(), *prim);
            }
            Some(FnParamType::ArrayGeneric(_)) | Some(FnParamType::SizedArray { .. }) => {
                locals.insert(param.clone(), PrimitiveType::F32);
            }
            Some(FnParamType::Tuple(elem_tys)) => {
                tuple_locals.insert(param.clone(), elem_tys.clone());
            }
            Some(FnParamType::Array(None)) | None => {
                locals.insert(param.clone(), PrimitiveType::F32);
            }
        }
    }

    let mut returns = Vec::<ObservedReturn>::new();
    infer_stmt_returns_for_def_return_inference(
        &def.body,
        &mut locals,
        fn_return_types,
        full_return_types,
        &mut tuple_locals,
        &mut returns,
    );
    returns
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
    fn_return_types: &HashMap<String, PrimitiveType>,
    full_return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> ReturnType {
    let returns =
        collect_def_return_observations(def, sig, fn_return_types, full_return_types, struct_defs);
    let mut it = returns.into_iter().map(|ret| ret.ty);
    let Some(mut out) = it.next() else {
        return ReturnType::Scalar(PrimitiveType::F32);
    };
    for ty in it {
        let Some(merged) = merge_return_types(out, ty) else {
            return ReturnType::Scalar(PrimitiveType::F32);
        };
        out = merged;
    }
    out
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
    full_return_types: &HashMap<String, ReturnType>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    let scalar_return_types = full_return_types
        .iter()
        .filter_map(|(name, ty)| match ty {
            ReturnType::Scalar(ty) => Some((name.clone(), *ty)),
            ReturnType::Tuple(_) => None,
        })
        .collect::<HashMap<_, _>>();

    for def in defs {
        let Some(sig) = fn_signatures.get(&def.name) else {
            continue;
        };
        let Some(expected) = full_return_types.get(&def.name) else {
            continue;
        };
        let observed_returns = collect_def_return_observations(
            def,
            sig,
            &scalar_return_types,
            full_return_types,
            struct_defs,
        );
        for observed in &observed_returns {
            validate_return_observation(&def.name, observed, expected, errors);
        }
    }
}

pub(crate) fn infer_def_return_types(
    defs: &[FunctionDef],
    fn_signatures: &HashMap<String, FnSignature>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> HashMap<String, ReturnType> {
    // Internal scalar-only map used for expression type inference (UserCall lookups).
    // Tuple-returning defs don't participate in scalar expression inference.
    let mut scalar_out = defs
        .iter()
        .map(|d| (d.name.clone(), PrimitiveType::F32))
        .collect::<HashMap<_, _>>();
    let mut out: HashMap<String, ReturnType> = defs
        .iter()
        .map(|d| (d.name.clone(), ReturnType::Scalar(PrimitiveType::F32)))
        .collect();
    for _ in 0..defs.len().saturating_add(1) {
        let mut changed = false;
        for def in defs {
            let Some(sig) = fn_signatures.get(&def.name) else {
                continue;
            };
            let inferred = infer_def_return_type(def, sig, &scalar_out, &out, struct_defs);
            if out.get(&def.name) != Some(&inferred) {
                // Update scalar map for expression inference
                if let ReturnType::Scalar(s) = &inferred {
                    scalar_out.insert(def.name.clone(), *s);
                }
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
