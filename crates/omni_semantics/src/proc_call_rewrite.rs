use super::*;

pub(super) fn try_constant_index_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(v) => Some(*v),
        Expr::Number(v) => {
            let truncated = v.trunc();
            if (v - truncated).abs() <= 1e-6 {
                Some(truncated as i64)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(super) fn resolve_proc_constant_slot_index(
    idx: i64,
    len: usize,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if idx < 0 || idx >= len as i64 {
        errors.push(Diagnostic::semantic(
            format!(
                "{context}: index {idx} is out of range (expected 0..{})",
                len.saturating_sub(1)
            ),
            0,
            0,
        ));
        return None;
    }
    Some(idx as usize)
}

pub(super) fn proc_read_helper_name(owner_proc: &str, len: usize, unsafe_mode: bool) -> String {
    let mode = if unsafe_mode { "unsafe" } else { "clamp" };
    format!("{owner_proc}.__arr_read_{mode}_{len}")
}

pub(super) fn sanitize_symbol_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

pub(super) fn proc_write_helper_name(
    owner_proc: &str,
    slots: &[String],
    unsafe_mode: bool,
) -> String {
    let mode = if unsafe_mode { "unsafe" } else { "clamp" };
    let key = sanitize_symbol_component(&slots.join("__"));
    format!("{owner_proc}.__arr_write_{mode}_{key}")
}

pub(super) fn build_proc_read_helper(
    owner_proc: &str,
    len: usize,
    unsafe_mode: bool,
) -> FunctionDef {
    let mut params = Vec::<omni_frontend::FnParamDecl>::new();
    params.push(omni_frontend::FnParamDecl {
        name: "idx".to_owned(),
        ty: None,
        default: None,
    });
    for i in 0..len {
        params.push(omni_frontend::FnParamDecl {
            name: format!("s{i}"),
            ty: None,
            default: None,
        });
    }

    let mut body = Vec::<Stmt>::new();
    body.push(Stmt::Assign {
        loc: None,
        target: AssignTarget::Var("i".to_owned()),
        decl_ty: Some(PrimitiveType::I32),
        generic_decl_ty: None,
        is_typed_decl: true,
        expr: Expr::Cast {
            to: PrimitiveType::I32,
            expr: Box::new(Expr::Var("idx".to_owned())),
        },
    });

    if !unsafe_mode {
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Lt,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(0)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Int(0),
            }],
            else_branch: Vec::new(),
        });
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Ge,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(len as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Int((len.saturating_sub(1)) as i64),
            }],
            else_branch: Vec::new(),
        });
    }

    if len == 1 {
        body.push(Stmt::Return {
            loc: None,
            expr: Expr::Var("s0".to_owned()),
        });
    } else {
        for i in 0..len {
            body.push(Stmt::If {
                loc: None,
                cond: Expr::Compare {
                    op: CmpOp::Eq,
                    lhs: Box::new(Expr::Var("i".to_owned())),
                    rhs: Box::new(Expr::Int(i as i64)),
                },
                then_branch: vec![Stmt::Return {
                    loc: None,
                    expr: Expr::Var(format!("s{i}")),
                }],
                else_branch: Vec::new(),
            });
        }
        if unsafe_mode {
            body.push(Stmt::Expr {
                loc: None,
                expr: Expr::Binary {
                    op: BinaryOp::Div,
                    lhs: Box::new(Expr::Int(1)),
                    rhs: Box::new(Expr::Int(0)),
                },
            });
            body.push(Stmt::Return {
                loc: None,
                expr: Expr::Number(0.0),
            });
        } else {
            body.push(Stmt::Return {
                loc: None,
                expr: Expr::Var("s0".to_owned()),
            });
        }
    }

    FunctionDef {
        type_params: Vec::new(),
        name: proc_read_helper_name(owner_proc, len, unsafe_mode),
        params,
        body,
    }
}

pub(super) fn build_proc_write_helper(
    owner_proc: &str,
    slots: &[String],
    unsafe_mode: bool,
) -> FunctionDef {
    let mut params = Vec::<omni_frontend::FnParamDecl>::new();
    params.push(omni_frontend::FnParamDecl {
        name: "self".to_owned(),
        ty: Some(FnParamType::Struct(owner_proc.to_owned())),
        default: None,
    });
    params.push(omni_frontend::FnParamDecl {
        name: "idx".to_owned(),
        ty: None,
        default: None,
    });
    params.push(omni_frontend::FnParamDecl {
        name: "value".to_owned(),
        ty: None,
        default: None,
    });

    let len = slots.len();
    let mut body = Vec::<Stmt>::new();
    body.push(Stmt::Assign {
        loc: None,
        target: AssignTarget::Var("i".to_owned()),
        decl_ty: Some(PrimitiveType::I32),
        generic_decl_ty: None,
        is_typed_decl: true,
        expr: Expr::Cast {
            to: PrimitiveType::I32,
            expr: Box::new(Expr::Var("idx".to_owned())),
        },
    });

    if !unsafe_mode {
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Lt,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(0)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Int(0),
            }],
            else_branch: Vec::new(),
        });
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Ge,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(len as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Int((len.saturating_sub(1)) as i64),
            }],
            else_branch: Vec::new(),
        });
    }

    for (idx, slot) in slots.iter().enumerate() {
        body.push(Stmt::If {
            loc: None,
            cond: Expr::Compare {
                op: CmpOp::Eq,
                lhs: Box::new(Expr::Var("i".to_owned())),
                rhs: Box::new(Expr::Int(idx as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: None,
                target: AssignTarget::Var(format!("self.{slot}")),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: Expr::Var("value".to_owned()),
            }],
            else_branch: Vec::new(),
        });
    }
    if unsafe_mode {
        body.push(Stmt::Expr {
            loc: None,
            expr: Expr::Binary {
                op: BinaryOp::Div,
                lhs: Box::new(Expr::Int(1)),
                rhs: Box::new(Expr::Int(0)),
            },
        });
    }

    FunctionDef {
        type_params: Vec::new(),
        name: proc_write_helper_name(owner_proc, slots, unsafe_mode),
        params,
        body,
    }
}

fn take_named_call_arg_expr(args: &mut Vec<CallArg>, arg_name: &str) -> Option<Expr> {
    let pos = args
        .iter()
        .position(|arg| arg.name.as_deref().map(|n| n == arg_name).unwrap_or(false))?;
    Some(args.remove(pos).expr)
}

fn named_call_arg_expr<'a>(args: &'a [CallArg], arg_name: &str) -> Option<&'a Expr> {
    args.iter()
        .find(|arg| arg.name.as_deref().map(|n| n == arg_name).unwrap_or(false))
        .map(|arg| &arg.expr)
}

fn resolve_proc_array_slot_by_index(
    array_base: &str,
    index_expr: &Expr,
    proc_array_slots: &HashMap<String, Vec<String>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let Some(slots) = proc_array_slots.get(array_base) else {
        errors.push(Diagnostic::semantic(
            format!("{context}: unknown processor array '{array_base}'"),
            0,
            0,
        ));
        return None;
    };
    let Some(raw_idx) = try_constant_index_i64(index_expr) else {
        errors.push(Diagnostic::semantic(
            format!("{context}: processor array index must be an integer literal"),
            0,
            0,
        ));
        return None;
    };
    let Some(slot_idx) = resolve_proc_constant_slot_index(raw_idx, slots.len(), context, errors)
    else {
        return None;
    };
    slots.get(slot_idx).cloned()
}

pub(super) fn extract_proc_index_slot_mut(
    args: &mut Vec<CallArg>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let Some(base_expr) = take_named_call_arg_expr(args, PROC_INDEX_BASE_ARG) else {
        errors.push(Diagnostic::semantic(
            format!("{context}: missing processor array base"),
            0,
            0,
        ));
        return None;
    };
    let Some(index_expr) = take_named_call_arg_expr(args, PROC_INDEX_EXPR_ARG) else {
        errors.push(Diagnostic::semantic(
            format!("{context}: missing processor array index"),
            0,
            0,
        ));
        return None;
    };
    let Expr::Var(array_base) = base_expr else {
        errors.push(Diagnostic::semantic(
            format!("{context}: processor array base must be a compile-time identifier"),
            0,
            0,
        ));
        return None;
    };
    resolve_proc_array_slot_by_index(&array_base, &index_expr, proc_array_slots, context, errors)
}

fn extract_proc_index_slot(
    args: &[CallArg],
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Option<String> {
    let Expr::Var(array_base) = named_call_arg_expr(args, PROC_INDEX_BASE_ARG)? else {
        return None;
    };
    let index_expr = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG)?;
    let slots = proc_array_slots.get(array_base)?;
    let raw_idx = try_constant_index_i64(index_expr)?;
    let slot_idx = usize::try_from(raw_idx).ok()?;
    slots.get(slot_idx).cloned()
}

fn collect_proc_array_call_targets(
    args: &[CallArg],
    proc_array_slots: &HashMap<String, Vec<String>>,
    out: &mut HashSet<String>,
) {
    if let Some(slot) = extract_proc_index_slot(args, proc_array_slots) {
        out.insert(slot);
        return;
    }
    let Some(Expr::Var(array_base)) = named_call_arg_expr(args, PROC_INDEX_BASE_ARG) else {
        return;
    };
    if let Some(slots) = proc_array_slots.get(array_base) {
        out.extend(slots.iter().cloned());
    }
}

pub(super) fn expand_expr_to_slots(
    expr: &Expr,
    slot_count: usize,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Expr> {
    if slot_count == 0 {
        return Vec::new();
    }
    if slot_count == 1 {
        return vec![expr.clone()];
    }
    match expr {
        Expr::ArrayLiteral(values) => {
            if values.len() != slot_count {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context}: expected array argument with {slot_count} elements, got {}",
                        values.len()
                    ),
                    0,
                    0,
                ));
            }
            (0..slot_count)
                .map(|i| values.get(i).cloned().unwrap_or(Expr::Number(0.0)))
                .collect()
        }
        Expr::Var(base) => (0..slot_count)
            .map(|i| Expr::Index {
                base: base.clone(),
                index: Box::new(Expr::Int(i as i64)),
            })
            .collect(),
        _ => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: array argument requires an array literal or array symbol expression"
                ),
                0,
                0,
            ));
            vec![expr.clone(); slot_count]
        }
    }
}

pub(super) fn select_proc_array_initializer_expr_for_slot(
    expr: &Expr,
    slot_idx: usize,
    slot_count: usize,
    context: &str,
    allow_symbol_index: bool,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    if slot_count <= 1 {
        return expr.clone();
    }
    match expr {
        Expr::ArrayLiteral(values) => {
            if values.len() != slot_count {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context}: expected array argument with {slot_count} elements, got {}",
                        values.len()
                    ),
                    0,
                    0,
                ));
            }
            values
                .get(slot_idx)
                .cloned()
                .or_else(|| values.last().cloned())
                .unwrap_or(Expr::Number(0.0))
        }
        Expr::Var(base) if allow_symbol_index => Expr::Index {
            base: base.clone(),
            index: Box::new(Expr::Int(slot_idx as i64)),
        },
        _ => expr.clone(),
    }
}

pub(super) fn expand_proc_call_args(
    call_args: &[CallArg],
    api: &ProcApi,
    call_display_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    let param_names = api.ins.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let param_defaults = api
        .ins
        .iter()
        .map(|p| {
            if p.slots.len() == 1 {
                p.defaults.first().cloned().flatten()
            } else if p.defaults.iter().all(|d| d.is_some()) {
                Some(Expr::ArrayLiteral(
                    p.defaults
                        .iter()
                        .filter_map(|d| d.clone())
                        .collect::<Vec<_>>(),
                ))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        call_args,
        &param_names,
        &param_defaults,
        false,
        false,
        &format!("processor call '{call_display_name}(...)'"),
        errors,
    );
    let mut expanded = Vec::<CallArg>::new();
    for (idx, port) in api.ins.iter().enumerate() {
        let slot_exprs = match resolved.get(idx).and_then(|a| *a) {
            Some(arg_expr) => expand_expr_to_slots(
                arg_expr,
                port.slots.len(),
                &format!(
                    "processor call '{call_display_name}(...)' argument '{}'",
                    port.name
                ),
                errors,
            ),
            None => {
                if port.defaults.iter().all(|d| d.is_some()) {
                    port.defaults
                        .iter()
                        .filter_map(|d| d.clone())
                        .collect::<Vec<_>>()
                } else {
                    continue;
                }
            }
        };
        for (slot_idx, expr) in slot_exprs.into_iter().enumerate() {
            let expr = if let Some(range) = port.ranges.get(slot_idx).and_then(|r| *r) {
                clamp_expr_to_range(expr, range)
            } else {
                expr
            };
            expanded.push(CallArg { name: None, expr });
        }
    }
    expanded
}

pub(super) fn expand_proc_buffer_call_args(
    instance: &ProcCallInstance,
    api: &ProcApi,
    call_display_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    if api.buffers.is_empty() {
        return Vec::new();
    }
    if instance.buffer_args.len() != api.buffers.len() {
        errors.push(Diagnostic::semantic(
            format!(
                "processor call '{call_display_name}(...)' is missing bound buffer arguments (expected {}, got {})",
                api.buffers.len(),
                instance.buffer_args.len()
            ),
            0,
            0,
        ));
        return Vec::new();
    }
    instance
        .buffer_args
        .iter()
        .cloned()
        .map(|expr| CallArg { name: None, expr })
        .collect::<Vec<_>>()
}

pub(super) fn expand_proc_event_call_args(
    call_args: &[CallArg],
    event: &ProcEventSpec,
    call_display_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    let param_names = event
        .params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let param_defaults = vec![None; param_names.len()];
    let resolved = resolve_call_args(
        call_args,
        &param_names,
        &param_defaults,
        false,
        false,
        &format!("processor event call '{call_display_name}(...)'"),
        errors,
    );
    let mut expanded = Vec::<CallArg>::new();
    for (idx, param) in event.params.iter().enumerate() {
        let slot_count = param.slots.len();
        let slot_exprs = match resolved.get(idx).and_then(|a| *a) {
            Some(arg_expr) => expand_expr_to_slots(
                arg_expr,
                slot_count,
                &format!(
                    "processor event call '{call_display_name}(...)' argument '{}'",
                    param.name
                ),
                errors,
            ),
            None => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor event call '{call_display_name}(...)' is missing required argument '{}'",
                        param.name
                    ),
                    0,
                    0,
                ));
                continue;
            }
        };
        for expr in slot_exprs {
            expanded.push(CallArg { name: None, expr });
        }
    }
    expanded
}

pub(super) fn expand_proc_port_specs(
    proc_name: &str,
    ports: &[PortDecl],
    kind: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (
    Vec<String>,
    HashMap<String, PrimitiveType>,
    Vec<ProcPortSpec>,
    HashMap<String, Vec<String>>,
) {
    let (flat, flat_types, arrays, defaults, ranges) = expand_port_decls(
        ports,
        &format!("processor '{proc_name}' {kind}"),
        options,
        errors,
    );
    let mut port_specs = Vec::<ProcPortSpec>::new();
    let mut array_slots = HashMap::<String, Vec<String>>::new();
    for port in ports {
        match port.ty.as_ref() {
            Some(DeclType::Array { .. }) | Some(DeclType::ArrayGeneric { .. }) => {
                let len = arrays.get(&port.name).map(|i| i.len).unwrap_or(0);
                let slots = (0..len)
                    .map(|idx| format!("{}[{idx}]", port.name))
                    .collect::<Vec<_>>();
                array_slots.insert(port.name.clone(), slots.clone());
                port_specs.push(ProcPortSpec {
                    name: port.name.clone(),
                    slots,
                    defaults: vec![None; len],
                    ranges: vec![None; len],
                });
            }
            _ => {
                let default = if port.default.is_some() {
                    defaults.get(&port.name).copied().map(typed_const_expr)
                } else {
                    None
                };
                port_specs.push(ProcPortSpec {
                    name: port.name.clone(),
                    slots: vec![port.name.clone()],
                    defaults: vec![default],
                    ranges: vec![ranges.get(&port.name).copied()],
                });
            }
        }
    }
    (flat, flat_types, port_specs, array_slots)
}

pub(super) fn expand_proc_param_specs(
    proc_name: &str,
    params: &[ParamDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<ProcParamSpec>, HashMap<String, Vec<String>>) {
    let mut specs = Vec::<ProcParamSpec>::new();
    let mut field_array_slots = HashMap::<String, Vec<String>>::new();

    for param in params {
        match param.ty.as_ref() {
            None | Some(DeclType::Scalar(_)) => {
                let ty = match param.ty.as_ref() {
                    Some(DeclType::Scalar(ty)) => *ty,
                    _ => PrimitiveType::F32,
                };
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        ty,
                        options,
                        &format!("processor '{proc_name}' param '{}' default", param.name),
                        is_float_type(ty),
                        matches!(ty, PrimitiveType::I32 | PrimitiveType::I64),
                        errors,
                    )
                    .unwrap_or_else(|| coerce_const_default_to_typed(0.0, ty)),
                    None => coerce_const_default_to_typed(0.0, ty),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        ty,
                        options,
                        &format!("processor '{proc_name}' param '{}'", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcParamSlotSpec {
                        name: param.name.clone(),
                        ty,
                        default: Some(typed_const_expr(default)),
                        range,
                    }],
                });
            }
            Some(DeclType::Generic(param_ty)) => {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{proc_name}' param '{}' uses unresolved generic type '{}'",
                        param.name, param_ty
                    ),
                    0,
                    0,
                ));
                let raw_default = match &param.default {
                    Some(expr) => eval_typed_const_expr(
                        expr,
                        PrimitiveType::F32,
                        options,
                        &format!("processor '{proc_name}' param '{}' default", param.name),
                        true,
                        false,
                        errors,
                    )
                    .unwrap_or(TypedConstValue::F32(0.0)),
                    None => TypedConstValue::F32(0.0),
                };
                let range = param.range.as_ref().and_then(|r| {
                    eval_decl_range_for_type(
                        r,
                        PrimitiveType::F32,
                        options,
                        &format!("processor '{proc_name}' param '{}'", param.name),
                        errors,
                    )
                });
                let default = range
                    .map(|r| clamp_typed_const_to_range(raw_default, r))
                    .unwrap_or(raw_default);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots: vec![ProcParamSlotSpec {
                        name: param.name.clone(),
                        ty: PrimitiveType::F32,
                        default: Some(typed_const_expr(default)),
                        range,
                    }],
                });
            }
            Some(DeclType::ArrayGeneric { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{proc_name}' param '{}' range is not supported for array declarations",
                            param.name
                        ),
                        0,
                        0,
                    ));
                }
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor '{proc_name}' param '{}' uses unresolved generic array element type '{}'",
                        param.name, elem
                    ),
                    0,
                    0,
                ));
                let size_context =
                    format!("processor '{proc_name}' param '{}' array size", param.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                let mut slot_defaults = Vec::<Option<Expr>>::with_capacity(len);
                match &param.default {
                    None => {
                        for _ in 0..len {
                            slot_defaults.push(Some(Expr::Number(0.0)));
                        }
                    }
                    Some(Expr::ArrayLiteral(values)) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor '{proc_name}' param '{}' default expects {len} elements, got {}",
                                    param.name,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
                        }
                        for idx in 0..len {
                            slot_defaults.push(values.get(idx).cloned());
                        }
                    }
                    Some(expr) => {
                        for _ in 0..len {
                            slot_defaults.push(Some(expr.clone()));
                        }
                    }
                }
                let mut slots = Vec::<ProcParamSlotSpec>::with_capacity(len);
                let mut slot_names = Vec::<String>::with_capacity(len);
                for idx in 0..len {
                    let slot_name = format!("{}[{idx}]", param.name);
                    slot_names.push(slot_name.clone());
                    slots.push(ProcParamSlotSpec {
                        name: slot_name,
                        ty: PrimitiveType::F32,
                        default: slot_defaults.get(idx).cloned().unwrap_or(None),
                        range: None,
                    });
                }
                field_array_slots.insert(param.name.clone(), slot_names);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots,
                });
            }
            Some(DeclType::Array { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor '{proc_name}' param '{}' range is not supported for array declarations",
                            param.name
                        ),
                        0,
                        0,
                    ));
                }
                let size_context =
                    format!("processor '{proc_name}' param '{}' array size", param.name);
                let Some(len) = eval_data_size_expr(size, options, &size_context, errors) else {
                    continue;
                };
                let mut slot_defaults = Vec::<Option<Expr>>::with_capacity(len);
                match &param.default {
                    None => {
                        for _ in 0..len {
                            slot_defaults.push(Some(Expr::Number(0.0)));
                        }
                    }
                    Some(Expr::ArrayLiteral(values)) => {
                        if values.len() != len {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor '{proc_name}' param '{}' default expects {len} elements, got {}",
                                    param.name,
                                    values.len()
                                ),
                                0,
                                0,
                            ));
                        }
                        for idx in 0..len {
                            slot_defaults
                                .push(values.get(idx).cloned().or(Some(Expr::Number(0.0))));
                        }
                    }
                    Some(expr) => {
                        for _ in 0..len {
                            slot_defaults.push(Some(expr.clone()));
                        }
                    }
                }
                let mut slots = Vec::<ProcParamSlotSpec>::with_capacity(len);
                let mut slot_names = Vec::<String>::with_capacity(len);
                for idx in 0..len {
                    let slot_name = format!("{}[{idx}]", param.name);
                    slot_names.push(slot_name.clone());
                    slots.push(ProcParamSlotSpec {
                        name: slot_name,
                        ty: *elem,
                        default: slot_defaults
                            .get(idx)
                            .cloned()
                            .unwrap_or(Some(Expr::Number(0.0))),
                        range: None,
                    });
                }
                field_array_slots.insert(param.name.clone(), slot_names);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots,
                });
            }
        }
    }

    (specs, field_array_slots)
}

pub(super) fn proc_buffer_fn_param_type(spec: &ProcBufferSpec) -> FnParamType {
    let channels = match spec.channels {
        TypedBufferChannels::Mono => BufferChannels::Mono,
        TypedBufferChannels::Dynamic => BufferChannels::Dynamic,
        TypedBufferChannels::Static(ch) => BufferChannels::Static(Expr::Int(ch as i64)),
    };
    FnParamType::Buffer(omni_frontend::BufferType {
        elem: BufferElemType::Primitive(spec.elem_ty),
        channels,
    })
}

pub(super) fn rewrite_proc_calls_in_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { base, index } => {
            rewrite_proc_calls_in_expr(index, proc_vars, proc_array_slots, proc_api, errors);
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
        }
        Expr::DataCtor { spec, init } => {
            rewrite_proc_calls_in_expr(
                &mut spec.size,
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_proc_calls_in_expr(
                        value,
                        proc_vars,
                        proc_array_slots,
                        proc_api,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_proc_calls_in_expr(lhs, proc_vars, proc_array_slots, proc_api, errors);
            rewrite_proc_calls_in_expr(rhs, proc_vars, proc_array_slots, proc_api, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_proc_calls_in_expr(arg, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_proc_calls_in_expr(inner, proc_vars, proc_array_slots, proc_api, errors);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_proc_calls_in_expr(value, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                rewrite_proc_calls_in_expr(
                    &mut arg.expr,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }

            if *name == PROC_INDEX_CALL_SENTINEL {
                let Some(resolved_slot) = extract_proc_index_slot_mut(
                    args,
                    proc_array_slots,
                    "processor indexed call",
                    errors,
                ) else {
                    return;
                };
                *name = resolved_slot;
            }

            if let Some(proc_var_raw) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
                let proc_var = if proc_var_raw == PROC_INDEX_CALL_SENTINEL {
                    let Some(resolved_slot) = extract_proc_index_slot_mut(
                        args,
                        proc_array_slots,
                        "processor indexed field call",
                        errors,
                    ) else {
                        return;
                    };
                    resolved_slot
                } else {
                    proc_var_raw.to_owned()
                };
                let Some(instance) = proc_vars.get(proc_var.as_str()) else {
                    errors.push(Diagnostic::semantic(
                        format!("processor call target '{}' is not an instance", proc_var),
                        0,
                        0,
                    ));
                    return;
                };
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    errors.push(Diagnostic::semantic(
                        format!("unknown processor type '{proc_name}'"),
                        0,
                        0,
                    ));
                    return;
                };
                let field_pos = args.iter().position(|a| {
                    a.name
                        .as_ref()
                        .map(|s| s == PROC_FIELD_SENTINEL_ARG)
                        .unwrap_or(false)
                });
                let Some(field_pos) = field_pos else {
                    errors.push(Diagnostic::semantic(
                        "processor call field selection is missing endpoint name",
                        0,
                        0,
                    ));
                    return;
                };
                let field_arg = args.remove(field_pos);
                let Expr::Var(field_name) = field_arg.expr else {
                    errors.push(Diagnostic::semantic(
                        "processor call field selection must be a compile-time endpoint identifier",
                        0,
                        0,
                    ));
                    return;
                };
                let out_idx = if let Some(idx) = api.outs.iter().position(|out| out == &field_name)
                {
                    idx
                } else if let Some(idx) = parse_proc_output_alias_index(&field_name) {
                    if idx >= api.outs.len() {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "processor output alias '{}' is out of range (outs: {})",
                                field_name,
                                api.outs.len()
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    idx
                } else {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "unknown processor output '{}' for '{}'; expected one of [{}] or outN",
                            field_name,
                            proc_var,
                            api.outs.join(", ")
                        ),
                        0,
                        0,
                    ));
                    return;
                };
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: Expr::Var(proc_var.to_owned()),
                });
                let expanded_args = expand_proc_call_args(args, api, proc_var.as_str(), errors);
                rewritten.extend(expanded_args);
                let expanded_buffers =
                    expand_proc_buffer_call_args(instance, api, proc_var.as_str(), errors);
                rewritten.extend(expanded_buffers);
                *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}{out_idx}");
                *args = rewritten;
                return;
            }

            if let Some(instance) = proc_vars.get(name) {
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    errors.push(Diagnostic::semantic(
                        format!("unknown processor type '{proc_name}'"),
                        0,
                        0,
                    ));
                    return;
                };
                if api.outs.len() != 1 {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "processor call '{}(...)' has {} outputs; use '{}(...).<endpoint>'/outN or call as statement then read fields",
                            name,
                            api.outs.len(),
                            name
                        ),
                        0,
                        0,
                    ));
                    return;
                }
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: Expr::Var(name.clone()),
                });
                let expanded_args = expand_proc_call_args(args, &api, name, errors);
                rewritten.extend(expanded_args);
                let expanded_buffers = expand_proc_buffer_call_args(instance, &api, name, errors);
                rewritten.extend(expanded_buffers);
                *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                *args = rewritten;
                return;
            }

            if let Some((base, event_name)) = split_dot_path(name) {
                if let Some(instance) = proc_vars.get(base) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown processor type '{proc_name}'"),
                            0,
                            0,
                        ));
                        return;
                    };
                    let Some(event_spec) = api.events.get(event_name) else {
                        let mut known_events = api.events.keys().cloned().collect::<Vec<_>>();
                        known_events.sort();
                        errors.push(Diagnostic::semantic(
                            format!(
                                "unknown processor event '{}.{}'; expected one of [{}]",
                                base,
                                event_name,
                                known_events.join(", ")
                            ),
                            0,
                            0,
                        ));
                        return;
                    };
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Var(base.to_owned()),
                    });
                    let expanded = expand_proc_event_call_args(args, event_spec, name, errors);
                    rewritten.extend(expanded);
                    *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                    *args = rewritten;
                }
            }
        }
        Expr::Var(name) => {
            normalize_proc_output_alias_path(name, proc_vars, proc_api);
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

pub(super) fn split_dot_path(name: &str) -> Option<(&str, &str)> {
    let (base, field) = name.split_once('.')?;
    if base.is_empty() || field.is_empty() {
        return None;
    }
    if field.contains('.') {
        return None;
    }
    Some((base, field))
}

fn parse_proc_output_alias_index(field: &str) -> Option<usize> {
    let raw = field.strip_prefix("out")?;
    if raw.is_empty() {
        return None;
    }
    let ordinal = raw.parse::<usize>().ok()?;
    if ordinal == 0 {
        return None;
    }
    Some(ordinal - 1)
}

pub(super) fn normalize_proc_output_alias_path(
    path: &mut String,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) {
    let Some((base, field)) = split_dot_path(path.as_str()) else {
        return;
    };
    let Some(instance) = proc_vars.get(base) else {
        return;
    };
    let Some(api) = proc_api.get(&instance.proc_name) else {
        return;
    };
    if api.outs.iter().any(|out_name| out_name == field) {
        return;
    }
    let Some(out_idx) = parse_proc_output_alias_index(field) else {
        return;
    };
    let Some(actual_out_name) = api.outs.get(out_idx) else {
        return;
    };
    *path = format!("{base}.{actual_out_name}");
}

pub(super) fn normalize_proc_output_aliases_in_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) {
    match expr {
        Expr::Var(name) => normalize_proc_output_alias_path(name, proc_vars, proc_api),
        Expr::Index { base, index } => {
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
            normalize_proc_output_aliases_in_expr(index, proc_vars, proc_api);
        }
        Expr::DataCtor { spec, init } => {
            normalize_proc_output_aliases_in_expr(&mut spec.size, proc_vars, proc_api);
            if let Some(values) = init {
                for value in values {
                    normalize_proc_output_aliases_in_expr(value, proc_vars, proc_api);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            normalize_proc_output_aliases_in_expr(lhs, proc_vars, proc_api);
            normalize_proc_output_aliases_in_expr(rhs, proc_vars, proc_api);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                normalize_proc_output_aliases_in_expr(arg, proc_vars, proc_api);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                normalize_proc_output_aliases_in_expr(&mut arg.expr, proc_vars, proc_api);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            normalize_proc_output_aliases_in_expr(inner, proc_vars, proc_api);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                normalize_proc_output_aliases_in_expr(value, proc_vars, proc_api);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

pub(super) fn normalize_proc_output_aliases_in_assign_target(
    target: &mut AssignTarget,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) {
    match target {
        AssignTarget::Var(name) => normalize_proc_output_alias_path(name, proc_vars, proc_api),
        AssignTarget::Index { base, index } => {
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
            normalize_proc_output_aliases_in_expr(index, proc_vars, proc_api);
        }
    }
}

pub(super) fn normalize_proc_output_aliases_in_stmt(
    stmt: &mut Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            normalize_proc_output_aliases_in_assign_target(target, proc_vars, proc_api);
            normalize_proc_output_aliases_in_expr(expr, proc_vars, proc_api);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            normalize_proc_output_aliases_in_expr(expr, proc_vars, proc_api);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            normalize_proc_output_aliases_in_expr(cond, proc_vars, proc_api);
            for nested in then_branch {
                normalize_proc_output_aliases_in_stmt(nested, proc_vars, proc_api);
            }
            for nested in else_branch {
                normalize_proc_output_aliases_in_stmt(nested, proc_vars, proc_api);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            normalize_proc_output_aliases_in_expr(start, proc_vars, proc_api);
            normalize_proc_output_aliases_in_expr(end, proc_vars, proc_api);
            if let Some(step_expr) = step {
                normalize_proc_output_aliases_in_expr(step_expr, proc_vars, proc_api);
            }
            for nested in body {
                normalize_proc_output_aliases_in_stmt(nested, proc_vars, proc_api);
            }
        }
        Stmt::While { cond, body, .. } => {
            normalize_proc_output_aliases_in_expr(cond, proc_vars, proc_api);
            for nested in body {
                normalize_proc_output_aliases_in_stmt(nested, proc_vars, proc_api);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn maybe_clamp_proc_param_assignment_expr(
    target: &AssignTarget,
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) {
    let Some((base, field)) = (match target {
        AssignTarget::Var(name) => split_dot_path(name),
        AssignTarget::Index { base, .. } => split_dot_path(base),
    }) else {
        return;
    };
    let Some(instance) = proc_vars.get(base) else {
        return;
    };
    let Some(api) = proc_api.get(&instance.proc_name) else {
        return;
    };
    let Some(param_slot) = api.params.get(field) else {
        return;
    };
    if let Some(range) = param_slot.range {
        let original = std::mem::replace(expr, Expr::Number(0.0));
        *expr = clamp_expr_to_range(original, range);
    }
}

pub(super) fn rewrite_proc_calls_in_stmt(
    stmt: &mut Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Assign { target, expr, .. } => {
            normalize_proc_output_aliases_in_assign_target(target, proc_vars, proc_api);
            rewrite_proc_calls_in_expr(expr, proc_vars, proc_array_slots, proc_api, errors);
            maybe_clamp_proc_param_assignment_expr(target, expr, proc_vars, proc_api);
        }
        Stmt::Expr { expr, .. } => {
            let mut handled_proc_stmt_call = false;
            if let Expr::UserCall { name, args, .. } = expr {
                for arg in args.iter_mut() {
                    rewrite_proc_calls_in_expr(
                        &mut arg.expr,
                        proc_vars,
                        proc_array_slots,
                        proc_api,
                        errors,
                    );
                }
                if *name == PROC_INDEX_CALL_SENTINEL {
                    let Some(resolved_slot) = extract_proc_index_slot_mut(
                        args,
                        proc_array_slots,
                        "processor indexed statement call",
                        errors,
                    ) else {
                        return;
                    };
                    *name = resolved_slot;
                }
                if let Some(instance) = proc_vars.get(name) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown processor type '{proc_name}'"),
                            0,
                            0,
                        ));
                        return;
                    };
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Var(name.clone()),
                    });
                    let expanded_args = expand_proc_call_args(args, api, name, errors);
                    rewritten.extend(expanded_args);
                    let expanded_buffers =
                        expand_proc_buffer_call_args(instance, api, name, errors);
                    rewritten.extend(expanded_buffers);
                    *name = format!("{proc_name}{PROC_STEP_FN_SUFFIX}");
                    *args = rewritten;
                    handled_proc_stmt_call = true;
                }
            }
            if !handled_proc_stmt_call {
                rewrite_proc_calls_in_expr(expr, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Stmt::Return { expr, .. } => {
            rewrite_proc_calls_in_expr(expr, proc_vars, proc_array_slots, proc_api, errors)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_proc_calls_in_expr(cond, proc_vars, proc_array_slots, proc_api, errors);
            for s in then_branch {
                rewrite_proc_calls_in_stmt(s, proc_vars, proc_array_slots, proc_api, errors);
            }
            for s in else_branch {
                rewrite_proc_calls_in_stmt(s, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_proc_calls_in_expr(start, proc_vars, proc_array_slots, proc_api, errors);
            rewrite_proc_calls_in_expr(end, proc_vars, proc_array_slots, proc_api, errors);
            if let Some(step_expr) = step {
                rewrite_proc_calls_in_expr(
                    step_expr,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            for s in body {
                rewrite_proc_calls_in_stmt(s, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_proc_calls_in_expr(cond, proc_vars, proc_array_slots, proc_api, errors);
            for s in body {
                rewrite_proc_calls_in_stmt(s, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(super) fn rewrite_proc_calls_in_stmts(
    stmts: &mut [Stmt],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        rewrite_proc_calls_in_stmt(stmt, proc_vars, proc_array_slots, proc_api, errors);
    }
}

pub(super) fn collect_called_proc_instances_in_expr(
    expr: &Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    out: &mut HashSet<String>,
) {
    match expr {
        Expr::Index { index, .. } => {
            collect_called_proc_instances_in_expr(index, proc_vars, proc_array_slots, out)
        }
        Expr::DataCtor { spec, init } => {
            collect_called_proc_instances_in_expr(&spec.size, proc_vars, proc_array_slots, out);
            if let Some(values) = init {
                for value in values {
                    collect_called_proc_instances_in_expr(value, proc_vars, proc_array_slots, out);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_called_proc_instances_in_expr(lhs, proc_vars, proc_array_slots, out);
            collect_called_proc_instances_in_expr(rhs, proc_vars, proc_array_slots, out);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_called_proc_instances_in_expr(arg, proc_vars, proc_array_slots, out);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            collect_called_proc_instances_in_expr(inner, proc_vars, proc_array_slots, out);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                collect_called_proc_instances_in_expr(value, proc_vars, proc_array_slots, out);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                collect_called_proc_instances_in_expr(&arg.expr, proc_vars, proc_array_slots, out);
            }
            if let Some(proc_var) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
                if proc_var == PROC_INDEX_CALL_SENTINEL {
                    collect_proc_array_call_targets(args, proc_array_slots, out);
                } else if proc_vars.contains_key(proc_var) {
                    out.insert(proc_var.to_owned());
                }
            } else if name == PROC_INDEX_CALL_SENTINEL {
                collect_proc_array_call_targets(args, proc_array_slots, out);
            } else if proc_vars.contains_key(name) {
                out.insert(name.clone());
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

pub(super) fn collect_called_proc_instances_in_stmt(
    stmt: &Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    out: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } | Stmt::Expr { expr, .. } => {
            collect_called_proc_instances_in_expr(expr, proc_vars, proc_array_slots, out);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_called_proc_instances_in_expr(cond, proc_vars, proc_array_slots, out);
            for nested in then_branch {
                collect_called_proc_instances_in_stmt(nested, proc_vars, proc_array_slots, out);
            }
            for nested in else_branch {
                collect_called_proc_instances_in_stmt(nested, proc_vars, proc_array_slots, out);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            collect_called_proc_instances_in_expr(start, proc_vars, proc_array_slots, out);
            collect_called_proc_instances_in_expr(end, proc_vars, proc_array_slots, out);
            if let Some(step_expr) = step {
                collect_called_proc_instances_in_expr(step_expr, proc_vars, proc_array_slots, out);
            }
            for nested in body {
                collect_called_proc_instances_in_stmt(nested, proc_vars, proc_array_slots, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            collect_called_proc_instances_in_expr(cond, proc_vars, proc_array_slots, out);
            for nested in body {
                collect_called_proc_instances_in_stmt(nested, proc_vars, proc_array_slots, out);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn collect_called_proc_instances_in_stmts(
    stmts: &[Stmt],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> HashSet<String> {
    let mut out = HashSet::<String>::new();
    for stmt in stmts {
        collect_called_proc_instances_in_stmt(stmt, proc_vars, proc_array_slots, &mut out);
    }
    out
}

pub(super) fn desugar_expr_instance_method_calls(
    expr: &mut Expr,
    struct_instances: &HashMap<String, String>,
) {
    match expr {
        Expr::Index { index, .. } => desugar_expr_instance_method_calls(index, struct_instances),
        Expr::DataCtor { spec, init } => {
            desugar_expr_instance_method_calls(&mut spec.size, struct_instances);
            if let Some(values) = init {
                for value in values {
                    desugar_expr_instance_method_calls(value, struct_instances);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            desugar_expr_instance_method_calls(lhs, struct_instances);
            desugar_expr_instance_method_calls(rhs, struct_instances);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                desugar_expr_instance_method_calls(arg, struct_instances);
            }
        }
        Expr::Cast { expr: arg, .. } | Expr::UnaryNot { expr: arg } => {
            desugar_expr_instance_method_calls(arg, struct_instances)
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                desugar_expr_instance_method_calls(value, struct_instances);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                desugar_expr_instance_method_calls(&mut arg.expr, struct_instances);
            }
            if let Some((base, method)) = split_simple_field_path(name) {
                if let Some(struct_name) = struct_instances.get(base) {
                    let base_name = base.to_owned();
                    let method_name = method.to_owned();
                    *name = format!("{}.{}", struct_name, method_name);
                    args.insert(
                        0,
                        CallArg {
                            name: None,
                            expr: Expr::Var(base_name),
                        },
                    );
                }
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

pub(super) fn desugar_init_instance_method_calls(
    stmt: &mut Stmt,
    struct_instances: &mut HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Var(name) = target {
                if let Expr::UserCall {
                    name: struct_name,
                    type_args,
                    ..
                } = expr
                {
                    if type_args.is_empty() && struct_defs.contains_key(struct_name) {
                        struct_instances.insert(name.clone(), struct_name.clone());
                    }
                }
            }
            if let AssignTarget::Index { index, .. } = target {
                desugar_expr_instance_method_calls(index, struct_instances);
            }
            desugar_expr_instance_method_calls(expr, struct_instances);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            desugar_expr_instance_method_calls(expr, struct_instances);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            desugar_expr_instance_method_calls(cond, struct_instances);
            for nested in then_branch.iter_mut() {
                desugar_init_instance_method_calls(nested, struct_instances, struct_defs);
            }
            for nested in else_branch.iter_mut() {
                desugar_init_instance_method_calls(nested, struct_instances, struct_defs);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            desugar_expr_instance_method_calls(start, struct_instances);
            desugar_expr_instance_method_calls(end, struct_instances);
            if let Some(step_expr) = step {
                desugar_expr_instance_method_calls(step_expr, struct_instances);
            }
            for nested in body.iter_mut() {
                desugar_init_instance_method_calls(nested, struct_instances, struct_defs);
            }
        }
        Stmt::While { cond, body, .. } => {
            desugar_expr_instance_method_calls(cond, struct_instances);
            for nested in body.iter_mut() {
                desugar_init_instance_method_calls(nested, struct_instances, struct_defs);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn desugar_sample_instance_method_calls(
    stmt: &mut Stmt,
    struct_instances: &HashMap<String, String>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                desugar_expr_instance_method_calls(index, struct_instances);
            }
            desugar_expr_instance_method_calls(expr, struct_instances);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            desugar_expr_instance_method_calls(expr, struct_instances);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            desugar_expr_instance_method_calls(cond, struct_instances);
            for nested in then_branch.iter_mut() {
                desugar_sample_instance_method_calls(nested, struct_instances);
            }
            for nested in else_branch.iter_mut() {
                desugar_sample_instance_method_calls(nested, struct_instances);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            desugar_expr_instance_method_calls(start, struct_instances);
            desugar_expr_instance_method_calls(end, struct_instances);
            if let Some(step_expr) = step {
                desugar_expr_instance_method_calls(step_expr, struct_instances);
            }
            for nested in body.iter_mut() {
                desugar_sample_instance_method_calls(nested, struct_instances);
            }
        }
        Stmt::While { cond, body, .. } => {
            desugar_expr_instance_method_calls(cond, struct_instances);
            for nested in body.iter_mut() {
                desugar_sample_instance_method_calls(nested, struct_instances);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}
