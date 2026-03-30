use super::*;
use crate::proc_call_support::rewrite_proc_alias_call_sites_in_expr;

type ProcArrayAliases = HashMap<String, ProcArrayAliasInfo>;

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

pub(super) fn try_constant_index_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int { value: v, .. } => Some(*v),
        Expr::Number { value: v, .. } => {
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
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "{context}: index {idx} is out of range (expected 0..{})",
                len.saturating_sub(1)
            ),
        );
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
        loc: Default::default(),
        name: "idx".to_owned(),
        ty: None,
        ty_loc: Default::default(),
        default: None,
    });
    for i in 0..len {
        params.push(omni_frontend::FnParamDecl {
            loc: Default::default(),
            name: format!("s{i}"),
            ty: None,
            ty_loc: Default::default(),
            default: None,
        });
    }

    let mut body = Vec::<Stmt>::new();
    body.push(Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var("i".to_owned()),
        decl_ty: Some(PrimitiveType::I32),
        generic_decl_ty: None,
        is_typed_decl: true,
        typed_decl_ty_loc: Default::default(),
        expr: Expr::Cast {
            loc: Default::default(),
            to: PrimitiveType::I32,
            expr: Box::new(Expr::var("idx")),
        },
    });

    if !unsafe_mode {
        body.push(Stmt::If {
            loc: Default::default(),
            cond: Expr::Compare {
                loc: Default::default(),
                op: CmpOp::Lt,
                lhs: Box::new(Expr::var("i")),
                rhs: Box::new(Expr::int(0)),
            },
            then_branch: vec![Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::int(0),
            }],
            else_branch: Vec::new(),
        });
        body.push(Stmt::If {
            loc: Default::default(),
            cond: Expr::Compare {
                loc: Default::default(),
                op: CmpOp::Ge,
                lhs: Box::new(Expr::var("i")),
                rhs: Box::new(Expr::int(len as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::int((len.saturating_sub(1)) as i64),
            }],
            else_branch: Vec::new(),
        });
    }

    if len == 1 {
        body.push(Stmt::Return {
            loc: Default::default(),
            expr: Expr::var("s0"),
        });
    } else {
        for i in 0..len {
            body.push(Stmt::If {
                loc: Default::default(),
                cond: Expr::Compare {
                    loc: Default::default(),
                    op: CmpOp::Eq,
                    lhs: Box::new(Expr::var("i")),
                    rhs: Box::new(Expr::int(i as i64)),
                },
                then_branch: vec![Stmt::Return {
                    loc: Default::default(),
                    expr: Expr::var(format!("s{i}")),
                }],
                else_branch: Vec::new(),
            });
        }
        if unsafe_mode {
            body.push(Stmt::Expr {
                loc: Default::default(),
                expr: Expr::Binary {
                    loc: Default::default(),
                    op: BinaryOp::Div,
                    lhs: Box::new(Expr::int(1)),
                    rhs: Box::new(Expr::int(0)),
                },
            });
            body.push(Stmt::Return {
                loc: Default::default(),
                expr: Expr::number(0.0),
            });
        } else {
            body.push(Stmt::Return {
                loc: Default::default(),
                expr: Expr::var("s0"),
            });
        }
    }

    FunctionDef {
        loc: Default::default(),
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
        loc: Default::default(),
        name: "self".to_owned(),
        ty: Some(FnParamType::Struct(owner_proc.to_owned())),
        ty_loc: Default::default(),
        default: None,
    });
    params.push(omni_frontend::FnParamDecl {
        loc: Default::default(),
        name: "idx".to_owned(),
        ty: None,
        ty_loc: Default::default(),
        default: None,
    });
    params.push(omni_frontend::FnParamDecl {
        loc: Default::default(),
        name: "value".to_owned(),
        ty: None,
        ty_loc: Default::default(),
        default: None,
    });

    let len = slots.len();
    let mut body = Vec::<Stmt>::new();
    body.push(Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var("i".to_owned()),
        decl_ty: Some(PrimitiveType::I32),
        generic_decl_ty: None,
        is_typed_decl: true,
        typed_decl_ty_loc: Default::default(),
        expr: Expr::Cast {
            loc: Default::default(),
            to: PrimitiveType::I32,
            expr: Box::new(Expr::var("idx")),
        },
    });

    if !unsafe_mode {
        body.push(Stmt::If {
            loc: Default::default(),
            cond: Expr::Compare {
                loc: Default::default(),
                op: CmpOp::Lt,
                lhs: Box::new(Expr::var("i")),
                rhs: Box::new(Expr::int(0)),
            },
            then_branch: vec![Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::int(0),
            }],
            else_branch: Vec::new(),
        });
        body.push(Stmt::If {
            loc: Default::default(),
            cond: Expr::Compare {
                loc: Default::default(),
                op: CmpOp::Ge,
                lhs: Box::new(Expr::var("i")),
                rhs: Box::new(Expr::int(len as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var("i".to_owned()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::int((len.saturating_sub(1)) as i64),
            }],
            else_branch: Vec::new(),
        });
    }

    for (idx, slot) in slots.iter().enumerate() {
        body.push(Stmt::If {
            loc: Default::default(),
            cond: Expr::Compare {
                loc: Default::default(),
                op: CmpOp::Eq,
                lhs: Box::new(Expr::var("i")),
                rhs: Box::new(Expr::int(idx as i64)),
            },
            then_branch: vec![Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var(format!("self.{slot}")),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::var("value"),
            }],
            else_branch: Vec::new(),
        });
    }
    if unsafe_mode {
        body.push(Stmt::Expr {
            loc: Default::default(),
            expr: Expr::Binary {
                loc: Default::default(),
                op: BinaryOp::Div,
                lhs: Box::new(Expr::int(1)),
                rhs: Box::new(Expr::int(0)),
            },
        });
    }

    FunctionDef {
        loc: Default::default(),
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

pub(super) enum ProcIndexResolution {
    Slot(String),
    Dynamic {
        array_base: String,
        index_expr: Expr,
        slots: Vec<String>,
    },
}

pub(super) fn take_proc_index_base_and_expr_mut(
    args: &mut Vec<CallArg>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<(String, Expr)> {
    let maybe_named_base = take_named_call_arg_expr(args, PROC_INDEX_BASE_ARG);
    let maybe_named_index = take_named_call_arg_expr(args, PROC_INDEX_EXPR_ARG);
    let (base_expr, index_expr) =
        if let (Some(base_expr), Some(index_expr)) = (maybe_named_base, maybe_named_index) {
            (base_expr, index_expr)
        } else if args.len() >= 2 && args[0].name.is_none() && args[1].name.is_none() {
            let base_expr = args.remove(0).expr;
            let index_expr = args.remove(0).expr;
            (base_expr, index_expr)
        } else {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("{context}: missing processor array base/index"),
            );
            return None;
        };
    let Expr::Var {
        name: array_base, ..
    } = base_expr
    else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context}: processor array base must be a compile-time identifier"),
        );
        return None;
    };
    Some((array_base, index_expr))
}

pub(super) fn resolve_proc_index_target_mut(
    args: &mut Vec<CallArg>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcIndexResolution> {
    let (array_base, index_expr) = take_proc_index_base_and_expr_mut(args, context, errors)?;
    let Some(slots) = proc_array_slots.get(&array_base) else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context}: unknown processor array '{array_base}'"),
        );
        return None;
    };
    if let Some(raw_idx) = try_constant_index_i64(&index_expr) {
        let Some(slot_idx) =
            resolve_proc_constant_slot_index(raw_idx, slots.len(), context, errors)
        else {
            return None;
        };
        let Some(slot_name) = slots.get(slot_idx) else {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("{context}: resolved processor array slot index is out of range"),
            );
            return None;
        };
        return Some(ProcIndexResolution::Slot(slot_name.clone()));
    }
    Some(ProcIndexResolution::Dynamic {
        array_base,
        index_expr,
        slots: slots.clone(),
    })
}

pub(super) fn resolve_proc_array_dispatch_context(
    slots: &[String],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<(String, ProcApi, Vec<(String, ProcCallInstance)>)> {
    let Some(first_slot) = slots.first() else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context}: processor array has no slots"),
        );
        return None;
    };
    let Some(first_instance) = proc_vars.get(first_slot) else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "{context}: processor array slot '{}' is not an instance",
                first_slot
            ),
        );
        return None;
    };
    let proc_name = first_instance.proc_name.clone();
    let Some(api) = proc_api.get(&proc_name) else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("unknown processor type '{proc_name}'"),
        );
        return None;
    };
    let mut instances = Vec::<(String, ProcCallInstance)>::with_capacity(slots.len());
    for slot in slots {
        let Some(instance) = proc_vars.get(slot) else {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "{context}: processor array slot '{}' is not an instance",
                    slot
                ),
            );
            return None;
        };
        if instance.proc_name != proc_name {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "{context}: processor array mixes processor types ('{}' vs '{}')",
                    proc_name, instance.proc_name
                ),
            );
            return None;
        }
        instances.push((slot.clone(), instance.clone()));
    }
    Some((proc_name, api.clone(), instances))
}

pub(super) fn find_proc_array_slot(
    instance_name: &str,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Option<(String, usize)> {
    for (base, slots) in proc_array_slots {
        if let Some(idx) = slots.iter().position(|slot| slot == instance_name) {
            return Some((base.clone(), idx));
        }
    }
    None
}

fn resolve_proc_array_base_key(
    base: &str,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Option<String> {
    if proc_array_slots.contains_key(base) {
        return Some(base.to_owned());
    }
    let self_base = format!("self.{base}");
    if proc_array_slots.contains_key(&self_base) {
        return Some(self_base);
    }
    if let Some(stripped) = base.strip_prefix("self.") {
        if proc_array_slots.contains_key(stripped) {
            return Some(stripped.to_owned());
        }
    }
    let suffix_base = base.strip_prefix("self.").unwrap_or(base);
    let suffix = format!(".{suffix_base}");
    let mut matches = proc_array_slots
        .keys()
        .filter(|k| k.ends_with(&suffix))
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return matches.pop();
    }
    None
}

fn can_resolve_proc_index_base(
    args: &[CallArg],
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> bool {
    let base_expr = named_call_arg_expr(args, PROC_INDEX_BASE_ARG).or_else(|| {
        args.first()
            .filter(|arg| arg.name.is_none())
            .map(|arg| &arg.expr)
    });
    let Some(Expr::Var { name: base, .. }) = base_expr else {
        return false;
    };
    resolve_proc_array_base_key(base, proc_array_slots).is_some()
}

pub(super) fn proc_instance_self_expr(
    instance_name: &str,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Expr {
    if let Some((base, idx)) = find_proc_array_slot(instance_name, proc_array_slots) {
        Expr::Index {
            loc: Default::default(),
            base,
            index: Box::new(Expr::int(idx as i64)),
        }
    } else {
        Expr::var(instance_name.to_owned())
    }
}

fn extract_proc_index_slot(
    args: &[CallArg],
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Option<String> {
    let (base_expr, index_expr) = if let (Some(base), Some(index)) = (
        named_call_arg_expr(args, PROC_INDEX_BASE_ARG),
        named_call_arg_expr(args, PROC_INDEX_EXPR_ARG),
    ) {
        (base, index)
    } else {
        let base = args
            .first()
            .filter(|arg| arg.name.is_none())
            .map(|arg| &arg.expr)?;
        let index = args
            .get(1)
            .filter(|arg| arg.name.is_none())
            .map(|arg| &arg.expr)?;
        (base, index)
    };
    let Expr::Var {
        name: array_base, ..
    } = base_expr
    else {
        return None;
    };
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
        Expr::ArrayLiteral { values, .. } => {
            if values.len() != slot_count {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "{context}: expected array argument with {slot_count} elements, got {}",
                        values.len()
                    ),
                );
            }
            (0..slot_count)
                .map(|i| values.get(i).cloned().unwrap_or(Expr::number(0.0)))
                .collect()
        }
        Expr::Var { name: base, .. } => (0..slot_count)
            .map(|i| Expr::Index {
                loc: Default::default(),
                base: base.clone(),
                index: Box::new(Expr::int(i as i64)),
            })
            .collect(),
        _ => {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "{context}: array argument requires an array literal or array symbol expression"
                ),
            );
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
        Expr::ArrayLiteral { values, .. } => {
            if values.len() != slot_count {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "{context}: expected array argument with {slot_count} elements, got {}",
                        values.len()
                    ),
                );
            }
            values
                .get(slot_idx)
                .cloned()
                .or_else(|| values.last().cloned())
                .unwrap_or(Expr::number(0.0))
        }
        Expr::Var { name: base, .. } if allow_symbol_index => Expr::Index {
            loc: Default::default(),
            base: base.clone(),
            index: Box::new(Expr::int(slot_idx as i64)),
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
                Some(Expr::array_literal(
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
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "processor call '{call_display_name}(...)' is missing bound buffer arguments (expected {}, got {})",
                api.buffers.len(),
                instance.buffer_args.len()
            ),
        );
        return Vec::new();
    }
    instance
        .buffer_args
        .iter()
        .cloned()
        .map(|expr| CallArg { name: None, expr })
        .collect::<Vec<_>>()
}

fn uniform_proc_array_buffer_call_args(
    slot_instances: &[(String, ProcCallInstance)],
    api: &ProcApi,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<CallArg>> {
    if slot_instances.is_empty() {
        return Some(Vec::new());
    }

    let (first_slot_name, first_instance) = slot_instances.first()?;
    let first = expand_proc_buffer_call_args(first_instance, api, first_slot_name, errors);
    let first_exprs = first.iter().map(|arg| arg.expr.clone()).collect::<Vec<_>>();

    for (slot_name, instance) in slot_instances.iter().skip(1) {
        let args = expand_proc_buffer_call_args(instance, api, slot_name, errors);
        let exprs = args.into_iter().map(|arg| arg.expr).collect::<Vec<_>>();
        if exprs != first_exprs {
            return None;
        }
    }

    Some(first)
}

pub(super) fn dynamic_proc_array_buffer_call_args(
    slot_instances: &[(String, ProcCallInstance)],
    api: &ProcApi,
    array_base: &str,
    index_expr: &Expr,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    if api.buffers.is_empty() {
        return Vec::new();
    }
    if slot_instances.is_empty() {
        return Vec::new();
    }

    if let Some(shared) = uniform_proc_array_buffer_call_args(slot_instances, api, errors) {
        return shared;
    }

    let expanded_per_slot = slot_instances
        .iter()
        .map(|(slot_name, instance)| expand_proc_buffer_call_args(instance, api, slot_name, errors))
        .collect::<Vec<_>>();

    let mut out = Vec::<CallArg>::with_capacity(api.buffers.len());
    for buf_idx in 0..api.buffers.len() {
        let mut selector_args = Vec::<CallArg>::with_capacity(2 + expanded_per_slot.len());
        selector_args.push(CallArg {
            name: Some(PROC_INDEX_BASE_ARG.to_owned()),
            expr: Expr::var(array_base.to_owned()),
        });
        selector_args.push(CallArg {
            name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
            expr: index_expr.clone(),
        });
        for slot_args in &expanded_per_slot {
            let slot_expr = slot_args
                .get(buf_idx)
                .map(|arg| arg.expr.clone())
                .unwrap_or(Expr::number(0.0));
            selector_args.push(CallArg {
                name: None,
                expr: slot_expr,
            });
        }
        out.push(CallArg {
            name: None,
            expr: Expr::UserCall {
                loc: Default::default(),
                name: PROC_INDEX_BUFFER_SELECT_SENTINEL.to_owned(),
                type_args: Vec::new(),
                args: selector_args,
            },
        });
    }
    out
}

pub(super) fn build_dynamic_proc_array_dispatch_args(
    args: &mut Vec<CallArg>,
    api: &ProcApi,
    slot_instances: &[(String, ProcCallInstance)],
    array_base: &str,
    index_expr: &Expr,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    let expanded_inputs = expand_proc_call_args(args, api, &format!("{array_base}[...]"), errors);
    let dynamic_buffers =
        dynamic_proc_array_buffer_call_args(slot_instances, api, array_base, index_expr, errors);
    let mut rewritten =
        Vec::<CallArg>::with_capacity(1 + expanded_inputs.len() + dynamic_buffers.len());
    rewritten.push(CallArg {
        name: None,
        expr: Expr::Index {
            loc: Default::default(),
            base: array_base.to_owned(),
            index: Box::new(index_expr.clone()),
        },
    });
    rewritten.extend(expanded_inputs);
    rewritten.extend(dynamic_buffers);
    rewritten
}

pub(super) fn expand_proc_event_call_args(
    call_args: &[CallArg],
    event: &ProcEventSpec,
    call_display_name: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    let param_names = event
        .params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let binding_defaults = param_names
        .iter()
        .map(|_| Some(Expr::number(0.0)))
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        call_args,
        &param_names,
        &binding_defaults,
        false,
        false,
        &format!("processor event call '{call_display_name}(...)'"),
        errors,
    );
    let mut expanded = Vec::<CallArg>::new();
    for (idx, param) in event.params.iter().enumerate() {
        let resolved_expr = match resolved.get(idx).and_then(|a| *a) {
            Some(arg_expr) => arg_expr.clone(),
            None if param.default.is_some() => {
                param.default.clone().unwrap_or_else(|| Expr::number(0.0))
            }
            None => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "processor event call '{call_display_name}(...)' is missing required argument '{}'",
                        param.name
                    ),
                );
                continue;
            }
        };
        if param.fixed_array_elem_ty.is_some() || param.slice_elem_ty.is_some() {
            expanded.push(CallArg {
                name: None,
                expr: resolved_expr,
            });
            continue;
        }
        let slot_exprs = expand_expr_to_slots(
            &resolved_expr,
            param.slots.len(),
            &format!(
                "processor event call '{call_display_name}(...)' argument '{}'",
                param.name
            ),
            errors,
        );
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
                    None => param
                        .default
                        .as_ref()
                        .and_then(|expr| {
                            with_expr_diag_context(expr, |_diag| {
                                let expr_ty = infer_const_expr_type(
                                    expr,
                                    options,
                                    &format!(
                                        "processor '{proc_name}' param '{}' default",
                                        param.name
                                    ),
                                    errors,
                                );
                                effective_untyped_assignment_type(expr, expr_ty)
                            })
                        })
                        .unwrap_or(PrimitiveType::F32),
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
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "processor '{proc_name}' param '{}' uses unresolved generic type '{}'",
                        param.name, param_ty
                    ),
                    param.ty_loc.or(param.loc),
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
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' range is not supported for array declarations",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "processor '{proc_name}' param '{}' uses unresolved generic array element type '{}'",
                        param.name, elem
                    ),
                    param.loc.as_ref(),
                ));
                let size_context =
                    format!("processor '{proc_name}' param '{}' array size", param.name);
                let Some(len) = with_expr_diag_context(size, |_diag| {
                    eval_data_size_expr(size, options, &size_context, errors)
                }) else {
                    continue;
                };
                let mut slot_defaults = Vec::<Option<Expr>>::with_capacity(len);
                match &param.default {
                    None => {
                        for _ in 0..len {
                            slot_defaults.push(Some(Expr::number(0.0)));
                        }
                    }
                    Some(default_expr @ Expr::ArrayLiteral { values, .. }) => {
                        if values.len() != len {
                            with_expr_diag_context(default_expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "processor '{proc_name}' param '{}' default expects {len} elements, got {}",
                                        param.name,
                                        values.len()
                                    ),
                                );
                            });
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
            Some(DeclType::Tuple(_)) => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "processor '{proc_name}' param '{}' tuple type is not supported",
                        param.name
                    ),
                    param.ty_loc.or(param.loc),
                ));
                continue;
            }
            Some(DeclType::Array { elem, size }) => {
                if param.range.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' range is not supported for array declarations",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
                let size_context =
                    format!("processor '{proc_name}' param '{}' array size", param.name);
                let Some(len) = with_expr_diag_context(size, |_diag| {
                    eval_data_size_expr(size, options, &size_context, errors)
                }) else {
                    continue;
                };
                let mut slot_defaults = Vec::<Option<Expr>>::with_capacity(len);
                match &param.default {
                    None => {
                        for _ in 0..len {
                            slot_defaults.push(Some(Expr::number(0.0)));
                        }
                    }
                    Some(default_expr @ Expr::ArrayLiteral { values, .. }) => {
                        if values.len() != len {
                            with_expr_diag_context(default_expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "processor '{proc_name}' param '{}' default expects {len} elements, got {}",
                                        param.name,
                                        values.len()
                                    ),
                                );
                            });
                        }
                        for idx in 0..len {
                            slot_defaults
                                .push(values.get(idx).cloned().or(Some(Expr::number(0.0))));
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
                            .unwrap_or(Some(Expr::number(0.0))),
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
        TypedBufferChannels::Static(ch) => BufferChannels::Static(Expr::int(ch as i64)),
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
    let expr_diag = DiagCtx::new(expr.loc());
    match expr {
        Expr::Index { base, index, .. } => {
            rewrite_proc_calls_in_expr(index, proc_vars, proc_array_slots, proc_api, errors);
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            if let Some(start) = start {
                rewrite_proc_calls_in_expr(start, proc_vars, proc_array_slots, proc_api, errors);
            }
            if let Some(end) = end {
                rewrite_proc_calls_in_expr(end, proc_vars, proc_array_slots, proc_api, errors);
            }
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
        }
        Expr::ArrayCtor { spec, init, .. } => {
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
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            rewrite_proc_calls_in_expr(inner, proc_vars, proc_array_slots, proc_api, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
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
                if !can_resolve_proc_index_base(args, proc_array_slots) {
                    return;
                }
                let Some(index_target) = resolve_proc_index_target_mut(
                    args,
                    proc_array_slots,
                    "processor indexed call",
                    errors,
                ) else {
                    return;
                };
                match index_target {
                    ProcIndexResolution::Slot(resolved_slot) => {
                        *name = resolved_slot;
                    }
                    ProcIndexResolution::Dynamic {
                        array_base,
                        index_expr,
                        slots,
                    } => {
                        let Some((proc_name, api, slot_instances)) =
                            resolve_proc_array_dispatch_context(
                                &slots,
                                proc_vars,
                                proc_api,
                                "processor indexed call",
                                errors,
                            )
                        else {
                            return;
                        };
                        if api.outs.len() != 1 {
                            push_semantic(
                                expr_diag,
                                errors,
                                format!(
                                    "processor call '{}[...]' has {} outputs; use '{}[...]().<endpoint>'/outN or call as statement then read fields",
                                    array_base,
                                    api.outs.len(),
                                    array_base
                                ),
                            );
                            return;
                        }
                        let rewritten = build_dynamic_proc_array_dispatch_args(
                            args,
                            &api,
                            &slot_instances,
                            &array_base,
                            &index_expr,
                            errors,
                        );
                        *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                        *args = rewritten;
                        return;
                    }
                }
            }

            if let Some(proc_var_raw) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
                let mut dynamic_index = None::<(String, Expr, Vec<String>)>;
                let proc_var = if proc_var_raw == PROC_INDEX_CALL_SENTINEL {
                    if !can_resolve_proc_index_base(args, proc_array_slots) {
                        return;
                    }
                    let Some(index_target) = resolve_proc_index_target_mut(
                        args,
                        proc_array_slots,
                        "processor indexed field call",
                        errors,
                    ) else {
                        return;
                    };
                    match index_target {
                        ProcIndexResolution::Slot(resolved_slot) => resolved_slot,
                        ProcIndexResolution::Dynamic {
                            array_base,
                            index_expr,
                            slots,
                        } => {
                            dynamic_index = Some((array_base, index_expr, slots));
                            String::new()
                        }
                    }
                } else {
                    proc_var_raw.to_owned()
                };
                let field_pos = args.iter().position(|a| {
                    a.name
                        .as_ref()
                        .map(|s| s == PROC_FIELD_SENTINEL_ARG)
                        .unwrap_or(false)
                });
                let Some(field_pos) = field_pos else {
                    push_semantic(
                        expr_diag,
                        errors,
                        "processor call field selection is missing endpoint name",
                    );
                    return;
                };
                let field_arg = args.remove(field_pos);
                let Expr::Var {
                    name: field_name, ..
                } = field_arg.expr
                else {
                    push_semantic(
                        expr_diag,
                        errors,
                        "processor call field selection must be a compile-time endpoint identifier",
                    );
                    return;
                };

                if let Some((array_base, index_expr, slots)) = dynamic_index {
                    let Some((proc_name, api, slot_instances)) =
                        resolve_proc_array_dispatch_context(
                            &slots,
                            proc_vars,
                            proc_api,
                            "processor indexed field call",
                            errors,
                        )
                    else {
                        return;
                    };
                    let Some(out_idx) = resolve_proc_output_field_index(
                        &api,
                        &field_name,
                        &array_base,
                        expr_diag,
                        errors,
                    ) else {
                        return;
                    };
                    let rewritten = build_dynamic_proc_array_dispatch_args(
                        args,
                        &api,
                        &slot_instances,
                        &array_base,
                        &index_expr,
                        errors,
                    );
                    *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}{out_idx}");
                    *args = rewritten;
                    return;
                }

                let Some(instance) = proc_vars.get(proc_var.as_str()) else {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("processor call target '{}' is not an instance", proc_var),
                    );
                    return;
                };
                let proc_name = instance.proc_name.clone();
                let Some(api) = proc_api.get(&proc_name) else {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("unknown processor type '{proc_name}'"),
                    );
                    return;
                };
                if let Some(param_slot) = api.params.get(&field_name) {
                    *expr = Expr::var(format!("{proc_var}.{}", param_slot.name));
                    return;
                }
                let Some(out_idx) = resolve_proc_output_field_index(
                    api,
                    &field_name,
                    proc_var.as_str(),
                    expr_diag,
                    errors,
                ) else {
                    return;
                };
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: proc_instance_self_expr(&proc_var, proc_array_slots),
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
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("unknown processor type '{proc_name}'"),
                    );
                    return;
                };
                if api.outs.len() != 1 {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!(
                            "processor call '{}(...)' has {} outputs; use '{}(...).<endpoint>'/outN or call as statement then read fields",
                            name,
                            api.outs.len(),
                            name
                        ),
                    );
                    return;
                }
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: proc_instance_self_expr(name, proc_array_slots),
                });
                let expanded_args = expand_proc_call_args(args, &api, name, errors);
                rewritten.extend(expanded_args);
                let expanded_buffers = expand_proc_buffer_call_args(instance, &api, name, errors);
                rewritten.extend(expanded_buffers);
                *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                *args = rewritten;
                return;
            }

            if let Some((base_raw, event_name)) = split_dot_path(name) {
                let mut dynamic_index = None::<(String, Expr, Vec<String>)>;
                let base = if base_raw == PROC_INDEX_CALL_SENTINEL {
                    if !can_resolve_proc_index_base(args, proc_array_slots) {
                        return;
                    }
                    let Some(index_target) = resolve_proc_index_target_mut(
                        args,
                        proc_array_slots,
                        "processor indexed event call",
                        errors,
                    ) else {
                        return;
                    };
                    match index_target {
                        ProcIndexResolution::Slot(resolved_slot) => resolved_slot,
                        ProcIndexResolution::Dynamic {
                            array_base,
                            index_expr,
                            slots,
                        } => {
                            dynamic_index = Some((array_base, index_expr, slots));
                            String::new()
                        }
                    }
                } else {
                    base_raw.to_owned()
                };

                if let Some((array_base, _, slots)) = &dynamic_index {
                    let Some((_proc_name, api, _slot_instances)) =
                        resolve_proc_array_dispatch_context(
                            slots,
                            proc_vars,
                            proc_api,
                            "processor indexed event call",
                            errors,
                        )
                    else {
                        return;
                    };
                    if api.events.contains_key(event_name) {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "processor event call '{}[...].{}(...)' is statement-only",
                                array_base, event_name
                            ),
                        );
                        return;
                    }
                }

                if let Some(instance) = proc_vars.get(base.as_str()) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!("unknown processor type '{proc_name}'"),
                        );
                        return;
                    };
                    if api.events.contains_key(event_name) {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "processor event call '{}.{}(...)' is statement-only",
                                base, event_name
                            ),
                        );
                        return;
                    }
                }

                if let Some((array_base, index_expr, slots)) = dynamic_index {
                    let Some((proc_name, api, _slot_instances)) =
                        resolve_proc_array_dispatch_context(
                            &slots,
                            proc_vars,
                            proc_api,
                            "processor indexed event call",
                            errors,
                        )
                    else {
                        return;
                    };
                    let Some(event_spec) = api.events.get(event_name) else {
                        let mut known_events = api.events.keys().cloned().collect::<Vec<_>>();
                        known_events.sort();
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "unknown processor event '{}.{}'; expected one of [{}]",
                                array_base,
                                event_name,
                                known_events.join(", ")
                            ),
                        );
                        return;
                    };
                    let expanded = expand_proc_event_call_args(
                        args,
                        event_spec,
                        &format!("{array_base}[...].{event_name}"),
                        expr_diag,
                        errors,
                    );
                    let mut rewritten = Vec::<CallArg>::with_capacity(1 + expanded.len());
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Index {
                            loc: Default::default(),
                            base: array_base.clone(),
                            index: Box::new(index_expr.clone()),
                        },
                    });
                    rewritten.extend(expanded);
                    *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                    *args = rewritten;
                    return;
                }

                if let Some(instance) = proc_vars.get(base.as_str()) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!("unknown processor type '{proc_name}'"),
                        );
                        return;
                    };
                    let Some(event_spec) = api.events.get(event_name) else {
                        let mut known_events = api.events.keys().cloned().collect::<Vec<_>>();
                        known_events.sort();
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "unknown processor event '{}.{}'; expected one of [{}]",
                                base,
                                event_name,
                                known_events.join(", ")
                            ),
                        );
                        return;
                    };
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    rewritten.push(CallArg {
                        name: None,
                        expr: proc_instance_self_expr(&base, proc_array_slots),
                    });
                    let expanded = expand_proc_event_call_args(
                        args,
                        event_spec,
                        &format!("{base}.{event_name}"),
                        expr_diag,
                        errors,
                    );
                    rewritten.extend(expanded);
                    *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                    *args = rewritten;
                    return;
                }
            }
        }
        Expr::Var { name, .. } => {
            normalize_proc_output_alias_path(name, proc_vars, proc_api);
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
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

pub(super) fn resolve_proc_output_field_index(
    api: &ProcApi,
    field_name: &str,
    call_display_name: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if let Some(idx) = api.outs.iter().position(|out| out == field_name) {
        return Some(idx);
    }
    if let Some(idx) = parse_proc_output_alias_index(field_name) {
        if idx < api.outs.len() {
            return Some(idx);
        }
        push_semantic(
            diag,
            errors,
            format!(
                "processor output alias '{}' is out of range (outs: {})",
                field_name,
                api.outs.len()
            ),
        );
        return None;
    }
    push_semantic(
        diag,
        errors,
        format!(
            "unknown processor output '{}' for '{}'; expected one of [{}] or outN",
            field_name,
            call_display_name,
            api.outs.join(", ")
        ),
    );
    None
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
        Expr::Var { name, .. } => normalize_proc_output_alias_path(name, proc_vars, proc_api),
        Expr::Index { base, index, .. } => {
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
            normalize_proc_output_aliases_in_expr(index, proc_vars, proc_api);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
            if let Some(start) = start {
                normalize_proc_output_aliases_in_expr(start, proc_vars, proc_api);
            }
            if let Some(end) = end {
                normalize_proc_output_aliases_in_expr(end, proc_vars, proc_api);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
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
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            normalize_proc_output_aliases_in_expr(inner, proc_vars, proc_api);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                normalize_proc_output_aliases_in_expr(value, proc_vars, proc_api);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
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
        AssignTarget::Slice { base, start, end } => {
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
            if let Some(start) = start {
                normalize_proc_output_aliases_in_expr(start, proc_vars, proc_api);
            }
            if let Some(end) = end {
                normalize_proc_output_aliases_in_expr(end, proc_vars, proc_api);
            }
        }
        AssignTarget::Tuple(names) => {
            for name in names {
                normalize_proc_output_alias_path(name, proc_vars, proc_api);
            }
        }
    }
}

fn normalize_proc_array_slot_assign_target(
    target: &mut AssignTarget,
    proc_array_slots: &HashMap<String, Vec<String>>,
) {
    let AssignTarget::Var(name) = target else {
        return;
    };
    let Some((base, field)) = split_dot_path(name) else {
        return;
    };
    let Some((array_base, slot_idx)) = find_proc_array_slot(base, proc_array_slots) else {
        return;
    };
    *target = AssignTarget::Index {
        base: format!("{array_base}.{field}"),
        index: Expr::int(slot_idx as i64),
    };
}

pub(super) fn normalize_proc_output_aliases_in_stmt(
    stmt: &mut Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
) {
    match stmt {
        Stmt::Const { .. } => {}
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
        AssignTarget::Slice { base, .. } => split_dot_path(base),
        AssignTarget::Tuple(_) => None,
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
        let original = std::mem::replace(expr, Expr::number(0.0));
        *expr = clamp_expr_to_range(original, range);
    }
}

fn rewrite_proc_calls_in_stmt_with_aliases(
    stmt: &mut Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &mut ProcArrayAliases,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |diag, stmt| match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Var(name) = target {
                if let Expr::Index { base, index, .. } = expr {
                    let resolved_base = resolve_proc_array_base_key(base, proc_array_slots)
                        .unwrap_or_else(|| base.clone());
                    aliases.insert(
                        name.clone(),
                        ProcArrayAliasInfo {
                            array_base: resolved_base,
                            index_expr: index.as_ref().clone(),
                        },
                    );
                } else {
                    aliases.remove(name);
                }
            }
            normalize_proc_array_slot_assign_target(target, proc_array_slots);
            rewrite_proc_alias_calls_in_expr(expr, aliases);
            normalize_proc_output_aliases_in_assign_target(target, proc_vars, proc_api);
            rewrite_proc_calls_in_expr(expr, proc_vars, proc_array_slots, proc_api, errors);
            maybe_clamp_proc_param_assignment_expr(target, expr, proc_vars, proc_api);
        }
        Stmt::Expr { expr, .. } => {
            rewrite_proc_alias_calls_in_expr(expr, aliases);
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
                    if !can_resolve_proc_index_base(args, proc_array_slots) {
                        return;
                    }
                    let Some(index_target) = resolve_proc_index_target_mut(
                        args,
                        proc_array_slots,
                        "processor indexed statement call",
                        errors,
                    ) else {
                        return;
                    };
                    match index_target {
                        ProcIndexResolution::Slot(resolved_slot) => {
                            *name = resolved_slot;
                        }
                        ProcIndexResolution::Dynamic {
                            array_base,
                            index_expr,
                            slots,
                        } => {
                            let Some((proc_name, api, slot_instances)) =
                                resolve_proc_array_dispatch_context(
                                    &slots,
                                    proc_vars,
                                    proc_api,
                                    "processor indexed statement call",
                                    errors,
                                )
                            else {
                                return;
                            };
                            let rewritten = build_dynamic_proc_array_dispatch_args(
                                args,
                                &api,
                                &slot_instances,
                                &array_base,
                                &index_expr,
                                errors,
                            );
                            *name = format!("{proc_name}{PROC_STEP_FN_SUFFIX}");
                            *args = rewritten;
                            return;
                        }
                    }
                }
                if let Some(instance) = proc_vars.get(name) {
                    let proc_name = instance.proc_name.clone();
                    let Some(api) = proc_api.get(&proc_name) else {
                        push_semantic(
                            diag,
                            errors,
                            format!("unknown processor type '{proc_name}'"),
                        );
                        return;
                    };
                    let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                    rewritten.push(CallArg {
                        name: None,
                        expr: proc_instance_self_expr(name, proc_array_slots),
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
                if !handled_proc_stmt_call {
                    if let Some((base_raw, event_name)) = split_dot_path(name) {
                        let mut dynamic_index = None::<(String, Expr, Vec<String>)>;
                        let base = if base_raw == PROC_INDEX_CALL_SENTINEL {
                            if !can_resolve_proc_index_base(args, proc_array_slots) {
                                return;
                            }
                            let Some(index_target) = resolve_proc_index_target_mut(
                                args,
                                proc_array_slots,
                                "processor indexed event statement call",
                                errors,
                            ) else {
                                return;
                            };
                            match index_target {
                                ProcIndexResolution::Slot(resolved_slot) => resolved_slot,
                                ProcIndexResolution::Dynamic {
                                    array_base,
                                    index_expr,
                                    slots,
                                } => {
                                    dynamic_index = Some((array_base, index_expr, slots));
                                    String::new()
                                }
                            }
                        } else {
                            base_raw.to_owned()
                        };

                        if let Some((array_base, index_expr, slots)) = dynamic_index {
                            let Some((proc_name, api, _slot_instances)) =
                                resolve_proc_array_dispatch_context(
                                    &slots,
                                    proc_vars,
                                    proc_api,
                                    "processor indexed event statement call",
                                    errors,
                                )
                            else {
                                return;
                            };
                            let Some(event_spec) = api.events.get(event_name) else {
                                let mut known_events =
                                    api.events.keys().cloned().collect::<Vec<_>>();
                                known_events.sort();
                                push_semantic(
                                    diag,
                                    errors,
                                    format!(
                                        "unknown processor event '{}.{}'; expected one of [{}]",
                                        array_base,
                                        event_name,
                                        known_events.join(", ")
                                    ),
                                );
                                return;
                            };
                            let expanded = expand_proc_event_call_args(
                                args,
                                event_spec,
                                &format!("{array_base}[...].{event_name}"),
                                diag,
                                errors,
                            );
                            let mut rewritten = Vec::<CallArg>::with_capacity(1 + expanded.len());
                            rewritten.push(CallArg {
                                name: None,
                                expr: Expr::Index {
                                    loc: Default::default(),
                                    base: array_base.clone(),
                                    index: Box::new(index_expr.clone()),
                                },
                            });
                            rewritten.extend(expanded);
                            *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                            *args = rewritten;
                            handled_proc_stmt_call = true;
                        } else if let Some(instance) = proc_vars.get(base.as_str()) {
                            let proc_name = instance.proc_name.clone();
                            let Some(api) = proc_api.get(&proc_name) else {
                                push_semantic(
                                    diag,
                                    errors,
                                    format!("unknown processor type '{proc_name}'"),
                                );
                                return;
                            };
                            let Some(event_spec) = api.events.get(event_name) else {
                                let mut known_events =
                                    api.events.keys().cloned().collect::<Vec<_>>();
                                known_events.sort();
                                push_semantic(
                                    diag,
                                    errors,
                                    format!(
                                        "unknown processor event '{}.{}'; expected one of [{}]",
                                        base,
                                        event_name,
                                        known_events.join(", ")
                                    ),
                                );
                                return;
                            };
                            let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                            rewritten.push(CallArg {
                                name: None,
                                expr: proc_instance_self_expr(&base, proc_array_slots),
                            });
                            let expanded = expand_proc_event_call_args(
                                args,
                                event_spec,
                                &format!("{base}.{event_name}"),
                                diag,
                                errors,
                            );
                            rewritten.extend(expanded);
                            *name = format!("{proc_name}{PROC_EVENT_FN_PREFIX}{event_name}");
                            *args = rewritten;
                            handled_proc_stmt_call = true;
                        }
                    }
                }
            }
            if !handled_proc_stmt_call {
                rewrite_proc_calls_in_expr(expr, proc_vars, proc_array_slots, proc_api, errors);
            }
        }
        Stmt::Return { expr, .. } => {
            rewrite_proc_alias_calls_in_expr(expr, aliases);
            rewrite_proc_calls_in_expr(expr, proc_vars, proc_array_slots, proc_api, errors)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_proc_alias_calls_in_expr(cond, aliases);
            rewrite_proc_calls_in_expr(cond, proc_vars, proc_array_slots, proc_api, errors);
            let mut then_aliases = aliases.clone();
            for s in then_branch {
                rewrite_proc_calls_in_stmt_with_aliases(
                    s,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    &mut then_aliases,
                    errors,
                );
            }
            let mut else_aliases = aliases.clone();
            for s in else_branch {
                rewrite_proc_calls_in_stmt_with_aliases(
                    s,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    &mut else_aliases,
                    errors,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_proc_alias_calls_in_expr(start, aliases);
            rewrite_proc_alias_calls_in_expr(end, aliases);
            rewrite_proc_calls_in_expr(start, proc_vars, proc_array_slots, proc_api, errors);
            rewrite_proc_calls_in_expr(end, proc_vars, proc_array_slots, proc_api, errors);
            if let Some(step_expr) = step {
                rewrite_proc_alias_calls_in_expr(step_expr, aliases);
                rewrite_proc_calls_in_expr(
                    step_expr,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            let mut body_aliases = aliases.clone();
            for s in body {
                rewrite_proc_calls_in_stmt_with_aliases(
                    s,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    &mut body_aliases,
                    errors,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_proc_alias_calls_in_expr(cond, aliases);
            rewrite_proc_calls_in_expr(cond, proc_vars, proc_array_slots, proc_api, errors);
            let mut body_aliases = aliases.clone();
            for s in body {
                rewrite_proc_calls_in_stmt_with_aliases(
                    s,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    &mut body_aliases,
                    errors,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(super) fn rewrite_proc_calls_in_stmt(
    stmt: &mut Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut aliases = ProcArrayAliases::new();
    rewrite_proc_calls_in_stmt_with_aliases(
        stmt,
        proc_vars,
        proc_array_slots,
        proc_api,
        &mut aliases,
        errors,
    );
}

pub(super) fn rewrite_proc_calls_in_stmts(
    stmts: &mut [Stmt],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut aliases = ProcArrayAliases::new();
    for stmt in stmts {
        rewrite_proc_calls_in_stmt_with_aliases(
            stmt,
            proc_vars,
            proc_array_slots,
            proc_api,
            &mut aliases,
            errors,
        );
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
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_called_proc_instances_in_expr(start, proc_vars, proc_array_slots, out);
            }
            if let Some(end) = end {
                collect_called_proc_instances_in_expr(end, proc_vars, proc_array_slots, out);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
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
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            collect_called_proc_instances_in_expr(inner, proc_vars, proc_array_slots, out);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
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
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn collect_called_proc_instances_in_stmt(
    stmt: &Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    aliases: &mut ProcArrayAliases,
    out: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            let mut expr_for_collect = expr.clone();
            rewrite_proc_alias_call_sites_in_expr(&mut expr_for_collect, aliases);
            collect_called_proc_instances_in_expr(
                &expr_for_collect,
                proc_vars,
                proc_array_slots,
                out,
            );
            if let AssignTarget::Var(name) = target {
                if let Expr::Index { base, index, .. } = expr {
                    let resolved_base = resolve_proc_array_base_key(base, proc_array_slots)
                        .unwrap_or_else(|| base.clone());
                    aliases.insert(
                        name.clone(),
                        ProcArrayAliasInfo {
                            array_base: resolved_base,
                            index_expr: index.as_ref().clone(),
                        },
                    );
                } else {
                    aliases.remove(name);
                }
            }
        }
        Stmt::Return { expr, .. } | Stmt::Expr { expr, .. } => {
            let mut expr_for_collect = expr.clone();
            rewrite_proc_alias_call_sites_in_expr(&mut expr_for_collect, aliases);
            collect_called_proc_instances_in_expr(
                &expr_for_collect,
                proc_vars,
                proc_array_slots,
                out,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let mut cond_for_collect = cond.clone();
            rewrite_proc_alias_call_sites_in_expr(&mut cond_for_collect, aliases);
            collect_called_proc_instances_in_expr(
                &cond_for_collect,
                proc_vars,
                proc_array_slots,
                out,
            );
            let mut then_aliases = aliases.clone();
            for nested in then_branch {
                collect_called_proc_instances_in_stmt(
                    nested,
                    proc_vars,
                    proc_array_slots,
                    &mut then_aliases,
                    out,
                );
            }
            let mut else_aliases = aliases.clone();
            for nested in else_branch {
                collect_called_proc_instances_in_stmt(
                    nested,
                    proc_vars,
                    proc_array_slots,
                    &mut else_aliases,
                    out,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            let mut start_for_collect = start.clone();
            rewrite_proc_alias_call_sites_in_expr(&mut start_for_collect, aliases);
            collect_called_proc_instances_in_expr(
                &start_for_collect,
                proc_vars,
                proc_array_slots,
                out,
            );
            let mut end_for_collect = end.clone();
            rewrite_proc_alias_call_sites_in_expr(&mut end_for_collect, aliases);
            collect_called_proc_instances_in_expr(
                &end_for_collect,
                proc_vars,
                proc_array_slots,
                out,
            );
            if let Some(step_expr) = step {
                let mut step_for_collect = step_expr.clone();
                rewrite_proc_alias_call_sites_in_expr(&mut step_for_collect, aliases);
                collect_called_proc_instances_in_expr(
                    &step_for_collect,
                    proc_vars,
                    proc_array_slots,
                    out,
                );
            }
            let mut body_aliases = aliases.clone();
            for nested in body {
                collect_called_proc_instances_in_stmt(
                    nested,
                    proc_vars,
                    proc_array_slots,
                    &mut body_aliases,
                    out,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            let mut cond_for_collect = cond.clone();
            rewrite_proc_alias_call_sites_in_expr(&mut cond_for_collect, aliases);
            collect_called_proc_instances_in_expr(
                &cond_for_collect,
                proc_vars,
                proc_array_slots,
                out,
            );
            let mut body_aliases = aliases.clone();
            for nested in body {
                collect_called_proc_instances_in_stmt(
                    nested,
                    proc_vars,
                    proc_array_slots,
                    &mut body_aliases,
                    out,
                );
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
    let mut aliases = ProcArrayAliases::new();
    for stmt in stmts {
        collect_called_proc_instances_in_stmt(
            stmt,
            proc_vars,
            proc_array_slots,
            &mut aliases,
            &mut out,
        );
    }
    out
}

pub(super) fn desugar_expr_instance_method_calls(
    expr: &mut Expr,
    struct_instances: &HashMap<String, String>,
    struct_array_roots: &HashMap<String, String>,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
) {
    fn extract_indexed_receiver(args: &[CallArg]) -> Option<(&str, Expr)> {
        let mut base = None::<&str>;
        let mut index = None::<Expr>;
        for arg in args {
            match arg.name.as_deref() {
                Some(PROC_INDEX_BASE_ARG) => {
                    if let Expr::Var { name, .. } = &arg.expr {
                        base = Some(name.as_str());
                    }
                }
                Some(PROC_INDEX_EXPR_ARG) => {
                    index = Some(arg.expr.clone());
                }
                _ => {}
            }
        }
        if base.is_none() || index.is_none() {
            if args.len() >= 2 {
                if let Expr::Var { name, .. } = &args[0].expr {
                    base = Some(name.as_str());
                    index = Some(args[1].expr.clone());
                }
            }
        }
        Some((base?, index?))
    }

    match expr {
        Expr::Index { index, .. } => desugar_expr_instance_method_calls(
            index,
            struct_instances,
            struct_array_roots,
            current_ns,
            callable_symbols,
        ),
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                desugar_expr_instance_method_calls(
                    start,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
            if let Some(end) = end {
                desugar_expr_instance_method_calls(
                    end,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            desugar_expr_instance_method_calls(
                &mut spec.size,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            if let Some(values) = init {
                for value in values {
                    desugar_expr_instance_method_calls(
                        value,
                        struct_instances,
                        struct_array_roots,
                        current_ns,
                        callable_symbols,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            desugar_expr_instance_method_calls(
                lhs,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            desugar_expr_instance_method_calls(
                rhs,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                desugar_expr_instance_method_calls(
                    arg,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Expr::Cast { expr: arg, .. }
        | Expr::UnaryNot { expr: arg, .. }
        | Expr::UnaryBitNot { expr: arg, .. } => desugar_expr_instance_method_calls(
            arg,
            struct_instances,
            struct_array_roots,
            current_ns,
            callable_symbols,
        ),
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                desugar_expr_instance_method_calls(
                    value,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                desugar_expr_instance_method_calls(
                    &mut arg.expr,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
            if let Some(method) = name.strip_prefix(&format!("{PROC_INDEX_CALL_SENTINEL}.")) {
                if let Some((base, index_expr)) = extract_indexed_receiver(args) {
                    let base = base.to_owned();
                    if let Some(struct_name) = struct_array_roots.get(base.as_str()) {
                        let resolved_method = format!("{}.{}", struct_name, method);
                        if callable_symbols.contains(&resolved_method) {
                            *name = resolved_method;
                            args.retain(|arg| {
                                !matches!(
                                    arg.name.as_deref(),
                                    Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG)
                                )
                            });
                            args.insert(
                                0,
                                CallArg {
                                    name: None,
                                    expr: Expr::Index {
                                        loc: Default::default(),
                                        base,
                                        index: Box::new(index_expr),
                                    },
                                },
                            );
                            return;
                        }
                    }
                }
            }
            if let Some((base, method)) = split_receiver_method_path(name) {
                if let Some(struct_name) = struct_instances.get(base) {
                    let base_name = base.to_owned();
                    let method_name = method.to_owned();
                    *name = format!("{}.{}", struct_name, method_name);
                    args.insert(
                        0,
                        CallArg {
                            name: None,
                            expr: Expr::var(base_name),
                        },
                    );
                } else if !base.contains("::")
                    && !method.is_empty()
                    && !method.contains('.')
                    && !is_builtin_instance_method_name(method)
                {
                    let resolved_method = if method.contains("::") {
                        callable_symbols.get(method).cloned()
                    } else {
                        resolve_unqualified_symbol_name(method, current_ns, callable_symbols)
                    };
                    if let Some(resolved_method) = resolved_method {
                        let receiver = base.to_owned();
                        *name = resolved_method;
                        args.insert(
                            0,
                            CallArg {
                                name: None,
                                expr: Expr::var(receiver),
                            },
                        );
                    }
                }
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn is_builtin_instance_method_name(method: &str) -> bool {
    matches!(method, "len" | "chans" | "unsafe_read" | "unsafe_write")
}

pub(super) fn desugar_init_instance_method_calls(
    stmt: &mut Stmt,
    struct_instances: &mut HashMap<String, String>,
    struct_array_roots: &mut HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            let expr_diag = DiagCtx::new(expr.loc());
            if let AssignTarget::Var(name) = target {
                if let Expr::UserCall {
                    name: struct_name,
                    type_args,
                    ..
                } = expr
                {
                    if type_args.is_empty() && struct_defs.contains_key(struct_name) {
                        register_struct_instance_and_array_roots(
                            name,
                            struct_name,
                            struct_defs,
                            struct_instances,
                            struct_array_roots,
                        );
                    } else if !type_args.is_empty() && struct_defs.contains_key(struct_name) {
                        let mut local_errors = Vec::new();
                        if resolve_explicit_call_type_args(
                            type_args,
                            &format!("proc init struct constructor '{}'", struct_name),
                            expr_diag,
                            &mut local_errors,
                        )
                        .is_some()
                        {
                            register_struct_instance_and_array_roots(
                                name,
                                struct_name,
                                struct_defs,
                                struct_instances,
                                struct_array_roots,
                            );
                        }
                    }
                }
                if let Expr::ArrayCtor { spec, .. } = expr {
                    if let ArrayElemType::Struct(struct_name) = &spec.elem {
                        register_struct_array_roots(
                            name,
                            struct_name,
                            struct_defs,
                            struct_array_roots,
                        );
                    }
                }
            }
            if let AssignTarget::Index { index, .. } = target {
                desugar_expr_instance_method_calls(
                    index,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
            desugar_expr_instance_method_calls(
                expr,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            desugar_expr_instance_method_calls(
                expr,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            desugar_expr_instance_method_calls(
                cond,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            for nested in then_branch.iter_mut() {
                desugar_init_instance_method_calls(
                    nested,
                    struct_instances,
                    struct_array_roots,
                    struct_defs,
                    current_ns,
                    callable_symbols,
                );
            }
            for nested in else_branch.iter_mut() {
                desugar_init_instance_method_calls(
                    nested,
                    struct_instances,
                    struct_array_roots,
                    struct_defs,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            desugar_expr_instance_method_calls(
                start,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            desugar_expr_instance_method_calls(
                end,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            if let Some(step_expr) = step {
                desugar_expr_instance_method_calls(
                    step_expr,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
            for nested in body.iter_mut() {
                desugar_init_instance_method_calls(
                    nested,
                    struct_instances,
                    struct_array_roots,
                    struct_defs,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            desugar_expr_instance_method_calls(
                cond,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            for nested in body.iter_mut() {
                desugar_init_instance_method_calls(
                    nested,
                    struct_instances,
                    struct_array_roots,
                    struct_defs,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn desugar_sample_instance_method_calls(
    stmt: &mut Stmt,
    struct_instances: &HashMap<String, String>,
    struct_array_roots: &HashMap<String, String>,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                desugar_expr_instance_method_calls(
                    index,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
            desugar_expr_instance_method_calls(
                expr,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            desugar_expr_instance_method_calls(
                expr,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            desugar_expr_instance_method_calls(
                cond,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            for nested in then_branch.iter_mut() {
                desugar_sample_instance_method_calls(
                    nested,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
            for nested in else_branch.iter_mut() {
                desugar_sample_instance_method_calls(
                    nested,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            desugar_expr_instance_method_calls(
                start,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            desugar_expr_instance_method_calls(
                end,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            if let Some(step_expr) = step {
                desugar_expr_instance_method_calls(
                    step_expr,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
            for nested in body.iter_mut() {
                desugar_sample_instance_method_calls(
                    nested,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            desugar_expr_instance_method_calls(
                cond,
                struct_instances,
                struct_array_roots,
                current_ns,
                callable_symbols,
            );
            for nested in body.iter_mut() {
                desugar_sample_instance_method_calls(
                    nested,
                    struct_instances,
                    struct_array_roots,
                    current_ns,
                    callable_symbols,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn desugar_processor_instance_method_calls(
    proc: &mut ProcessorDef,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    callable_symbols: &HashSet<String>,
) {
    let current_ns = namespace_of_symbol(&proc.name);
    let mut struct_instances = HashMap::<String, String>::new();
    let mut struct_array_roots = HashMap::<String, String>::new();

    for stmt in &mut proc.init {
        desugar_init_instance_method_calls(
            stmt,
            &mut struct_instances,
            &mut struct_array_roots,
            struct_defs,
            &current_ns,
            callable_symbols,
        );
    }

    for stmt in &mut proc.block_pre {
        desugar_sample_instance_method_calls(
            stmt,
            &struct_instances,
            &struct_array_roots,
            &current_ns,
            callable_symbols,
        );
    }
    for stmt in &mut proc.block_post {
        desugar_sample_instance_method_calls(
            stmt,
            &struct_instances,
            &struct_array_roots,
            &current_ns,
            callable_symbols,
        );
    }
    for stmt in &mut proc.sample {
        desugar_sample_instance_method_calls(
            stmt,
            &struct_instances,
            &struct_array_roots,
            &current_ns,
            callable_symbols,
        );
    }
    for event in &mut proc.events {
        for stmt in &mut event.body {
            desugar_sample_instance_method_calls(
                stmt,
                &struct_instances,
                &struct_array_roots,
                &current_ns,
                callable_symbols,
            );
        }
    }
    for def in &mut proc.local_defs {
        for stmt in &mut def.body {
            desugar_sample_instance_method_calls(
                stmt,
                &struct_instances,
                &struct_array_roots,
                &current_ns,
                callable_symbols,
            );
        }
    }
}
