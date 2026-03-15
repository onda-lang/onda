use super::*;
use crate::ReturnType;

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
        Expr::ArrayLiteral { .. } | Expr::ArrayCtor { .. } | Expr::Slice { .. } | Expr::Tuple { .. } => None,
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

fn infer_stmt_returns_for_def_return_inference(
    stmts: &[Stmt],
    locals: &mut HashMap<String, PrimitiveType>,
    fn_return_types: &HashMap<String, PrimitiveType>,
    out: &mut Vec<ReturnType>,
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
                expr,
                ..
            } => {
                // For tuple destructuring, try to infer element types from a
                // UserCall that returns a tuple (via fn_return_types lookup).
                // For now we skip detailed tracking — element types will be
                // resolved during body analysis.
                let _ = expr;
            }
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
                if let Expr::Tuple { values, .. } = expr {
                    // Tuple return: infer each element type
                    let elem_tys: Vec<PrimitiveType> = values
                        .iter()
                        .map(|v| {
                            infer_expr_type_for_def_return_inference(v, locals, fn_return_types)
                                .unwrap_or(PrimitiveType::F32)
                        })
                        .collect();
                    out.push(ReturnType::Tuple(elem_tys));
                } else {
                    let ty =
                        infer_expr_type_for_def_return_inference(expr, locals, fn_return_types)
                            .unwrap_or(PrimitiveType::F32);
                    out.push(ReturnType::Scalar(ty));
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                let mut then_locals = locals.clone();
                let mut else_locals = locals.clone();
                infer_stmt_returns_for_def_return_inference(
                    then_branch,
                    &mut then_locals,
                    fn_return_types,
                    out,
                );
                infer_stmt_returns_for_def_return_inference(
                    else_branch,
                    &mut else_locals,
                    fn_return_types,
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
            }
            Stmt::For { var, body, .. } => {
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone(), PrimitiveType::I32);
                infer_stmt_returns_for_def_return_inference(
                    body,
                    &mut loop_locals,
                    fn_return_types,
                    out,
                );
            }
            Stmt::While { body, .. } => {
                let mut loop_locals = locals.clone();
                infer_stmt_returns_for_def_return_inference(
                    body,
                    &mut loop_locals,
                    fn_return_types,
                    out,
                );
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
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
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> ReturnType {
    let mut locals = HashMap::<String, PrimitiveType>::new();
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
            Some(FnParamType::ArrayGeneric(_)) => {
                locals.insert(param.clone(), PrimitiveType::F32);
            }
            Some(FnParamType::Array(None)) | None => {
                locals.insert(param.clone(), PrimitiveType::F32);
            }
        }
    }

    let mut returns = Vec::<ReturnType>::new();
    infer_stmt_returns_for_def_return_inference(
        &def.body,
        &mut locals,
        fn_return_types,
        &mut returns,
    );
    let mut it = returns.into_iter();
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
            let inferred = infer_def_return_type(def, sig, &scalar_out, struct_defs);
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
