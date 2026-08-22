use super::*;
use crate::internal_names::METHOD_RECEIVER_ARG;
use crate::proc_call_support::rewrite_proc_alias_call_sites_in_expr;
use crate::processor_lowering::{
    proc_local_bind_hidden_def_name, proc_local_nested_bind_hidden_def_name,
};
use onda_frontend::Span;

type ProcArrayAliases = HashMap<String, ProcArrayAliasInfo>;

#[derive(Debug, Clone)]
enum ProcNamedArgReceiver {
    Direct(String),
    Indexed {
        array_base: String,
        index: Expr,
        resolved_slot: Option<String>,
        access: IndexAccess,
    },
}

#[derive(Debug, Clone)]
struct ProcNamedArgCallTarget {
    display_name: String,
    receiver: ProcNamedArgReceiver,
    api: ProcApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcCallArgKind {
    Internal,
    Input,
    Param,
    PrivateParam,
    Ambiguous,
    Unknown,
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
    let mut params = Vec::<onda_frontend::FnParamDecl>::new();
    params.push(onda_frontend::FnParamDecl {
        loc: Default::default(),
        name: "idx".to_owned(),
        ty: None,
        ty_loc: Default::default(),
        default: None,
    });
    for i in 0..len {
        params.push(onda_frontend::FnParamDecl {
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
        is_const: false,
        type_params: Vec::new(),
        name: proc_read_helper_name(owner_proc, len, unsafe_mode),
        params,
        return_ty: None,
        return_ty_loc: Default::default(),
        body,
    }
}

pub(super) fn build_proc_write_helper(
    owner_proc: &str,
    slots: &[String],
    unsafe_mode: bool,
) -> FunctionDef {
    let params = vec![
        onda_frontend::FnParamDecl {
            loc: Default::default(),
            name: "self".to_owned(),
            ty: Some(FnParamType::Struct(owner_proc.to_owned())),
            ty_loc: Default::default(),
            default: None,
        },
        onda_frontend::FnParamDecl {
            loc: Default::default(),
            name: "idx".to_owned(),
            ty: None,
            ty_loc: Default::default(),
            default: None,
        },
        onda_frontend::FnParamDecl {
            loc: Default::default(),
            name: "value".to_owned(),
            ty: None,
            ty_loc: Default::default(),
            default: None,
        },
    ];

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

    if unsafe_mode {
        // Keep the unsafe OOB sentinel exclusively on the unmatched path.
        // A trailing dead `1 / 0` used to be harmless in the direct LLVM
        // backend because poison was unused, but MIR gives integer division
        // by-zero observable failure semantics.
        let mut unmatched = vec![Stmt::Expr {
            loc: Default::default(),
            expr: Expr::Binary {
                loc: Default::default(),
                op: BinaryOp::Div,
                lhs: Box::new(Expr::int(1)),
                rhs: Box::new(Expr::int(0)),
            },
        }];
        for (idx, slot) in slots.iter().enumerate().rev() {
            unmatched = vec![Stmt::If {
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
                else_branch: unmatched,
            }];
        }
        body.extend(unmatched);
    } else {
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
    }

    FunctionDef {
        loc: Default::default(),
        is_const: false,
        type_params: Vec::new(),
        name: proc_write_helper_name(owner_proc, slots, unsafe_mode),
        params,
        return_ty: None,
        return_ty_loc: Default::default(),
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
        access: IndexAccess,
    },
}

pub(super) fn take_proc_index_base_and_expr_mut(
    args: &mut Vec<CallArg>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<(String, Expr, IndexAccess)> {
    let access = if take_named_call_arg_expr(args, PROC_INDEX_UNCHECKED_ARG).is_some() {
        IndexAccess::Unchecked
    } else {
        IndexAccess::Clamp
    };
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
    Some((array_base, index_expr, access))
}

pub(super) fn resolve_proc_index_target_mut(
    args: &mut Vec<CallArg>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcIndexResolution> {
    let (array_base, index_expr, access) =
        take_proc_index_base_and_expr_mut(args, context, errors)?;
    let Some(slots) = proc_array_slots.get(&array_base) else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context}: unknown processor array '{array_base}'"),
        );
        return None;
    };
    if let Some(raw_idx) = try_constant_index_i64(&index_expr) {
        let slot_idx = resolve_proc_constant_slot_index(raw_idx, slots.len(), context, errors)?;
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
        access,
    })
}

pub(super) fn proc_index_base_name(args: &[CallArg]) -> Option<&str> {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some(PROC_INDEX_BASE_ARG))
        .or_else(|| args.first().filter(|arg| arg.name.is_none()))
        .and_then(|arg| match &arg.expr {
            Expr::Var { name, .. } => Some(name.as_str()),
            _ => None,
        })
}

type ProcArrayDispatchContext = (String, ProcApi, Vec<(String, ProcCallInstance)>);

pub(super) fn resolve_proc_array_dispatch_context(
    slots: &[String],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcArrayDispatchContext> {
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

fn canonicalize_indexed_proc_receiver_call(
    name: &mut String,
    args: &mut Vec<CallArg>,
    proc_array_slots: &HashMap<String, Vec<String>>,
) {
    if name.starts_with(PROC_INDEX_CALL_SENTINEL) {
        return;
    }
    if name.contains('.') || name.contains("::") {
        return;
    }
    let Some(CallArg {
        name: Some(receiver_marker),
        expr: Expr::Index { base, index, .. },
    }) = args.first()
    else {
        return;
    };
    if receiver_marker != METHOD_RECEIVER_ARG {
        return;
    }
    if resolve_proc_array_base_key(base, proc_array_slots).is_none() {
        return;
    }
    let base = base.clone();
    let index = index.as_ref().clone();
    args.remove(0);
    args.insert(
        0,
        CallArg {
            name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
            expr: index,
        },
    );
    args.insert(
        0,
        CallArg {
            name: Some(PROC_INDEX_BASE_ARG.to_owned()),
            expr: Expr::var(base),
        },
    );
    *name = format!("{PROC_INDEX_CALL_SENTINEL}.{name}");
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
        return;
    }

    let base_expr = named_call_arg_expr(args, PROC_INDEX_BASE_ARG).or_else(|| {
        args.first()
            .filter(|arg| arg.name.is_none())
            .map(|arg| &arg.expr)
    });
    let Some(Expr::Var { name: base, .. }) = base_expr else {
        return;
    };
    let Some(array_base) = resolve_proc_array_base_key(base, proc_array_slots) else {
        return;
    };
    if let Some(slots) = proc_array_slots.get(&array_base) {
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

fn validate_fixed_array_event_arg(
    expr: &Expr,
    len: usize,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::ArrayLiteral { values, .. } => {
            if values.len() != len {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "{context}: expected array argument with {len} elements, got {}",
                        values.len()
                    ),
                );
            }
        }
        Expr::Var { .. } => {}
        _ => {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("{context}: array argument requires an array literal or array symbol expression"),
            );
        }
    }
}

pub(super) fn select_proc_array_initializer_expr_for_slot(
    expr: &Expr,
    slot_idx: usize,
    slot_count: usize,
    context: &str,
    array_symbols: &HashSet<String>,
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
        Expr::Var { name: base, .. } if array_symbols.contains(base) => Expr::Index {
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
    args: &mut [CallArg],
    api: &ProcApi,
    slot_instances: &[(String, ProcCallInstance)],
    array_base: &str,
    index_expr: &Expr,
    access: IndexAccess,
    errors: &mut Vec<Diagnostic>,
) -> Vec<CallArg> {
    let expanded_inputs = expand_proc_call_args(args, api, &format!("{array_base}[...]"), errors);
    let dynamic_buffers =
        dynamic_proc_array_buffer_call_args(slot_instances, api, array_base, index_expr, errors);
    let mut rewritten =
        Vec::<CallArg>::with_capacity(1 + expanded_inputs.len() + dynamic_buffers.len());
    let selector = proc_index_selector_expr(array_base, index_expr, access);
    rewritten.push(CallArg {
        name: None,
        expr: selector,
    });
    rewritten.extend(expanded_inputs);
    rewritten.extend(dynamic_buffers);
    rewritten
}

pub(super) fn proc_index_selector_expr(
    array_base: &str,
    index_expr: &Expr,
    access: IndexAccess,
) -> Expr {
    indexed_read_expr(array_base, index_expr.clone(), access, Default::default())
}

fn proc_named_arg_temp_name(counter: &mut usize) -> String {
    let name = format!("__onda_proc_call_arg_tmp_{}", *counter);
    *counter += 1;
    name
}

fn proc_named_arg_result_temp_name(counter: &mut usize) -> String {
    let name = format!("__onda_proc_call_result_tmp_{}", *counter);
    *counter += 1;
    name
}

fn assign_temp_stmt(name: String, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var(name),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

fn proc_named_arg_is_internal(name: Option<&str>) -> bool {
    matches!(
        name,
        Some(PROC_INDEX_BASE_ARG)
            | Some(PROC_INDEX_EXPR_ARG)
            | Some(PROC_INDEX_UNCHECKED_ARG)
            | Some(PROC_FIELD_SENTINEL_ARG)
    )
}

fn proc_input_arg_names(api: &ProcApi) -> HashSet<String> {
    api.ins
        .iter()
        .flat_map(|port| std::iter::once(port.name.clone()).chain(port.slots.iter().cloned()))
        .collect()
}

fn proc_named_param_slots(
    api: &ProcApi,
    name: &str,
    include_private: bool,
) -> Option<Vec<ProcParamSlotSpec>> {
    if let Some(slot) = api.params.get(name) {
        if slot.private && !include_private {
            return None;
        }
        return Some(vec![slot.clone()]);
    }

    let prefix = format!("{name}[");
    let mut slots = api
        .params
        .values()
        .filter_map(|slot| {
            if slot.private && !include_private {
                return None;
            }
            let rest = slot.name.strip_prefix(&prefix)?;
            let idx = rest.strip_suffix(']')?.parse::<usize>().ok()?;
            Some((idx, slot.clone()))
        })
        .collect::<Vec<_>>();
    if slots.is_empty() {
        return None;
    }
    slots.sort_by_key(|(idx, _)| *idx);
    Some(slots.into_iter().map(|(_, slot)| slot).collect())
}

fn proc_param_slots_for_api<'a>(api: &'a ProcApi, name: &str) -> Vec<&'a ProcParamSlotSpec> {
    if let Some(slot) = api.params.get(name) {
        return vec![slot];
    }

    let prefix = format!("{name}[");
    let mut slots = api
        .params
        .values()
        .filter_map(|slot| {
            let rest = slot.name.strip_prefix(&prefix)?;
            let idx = rest.strip_suffix(']')?.parse::<usize>().ok()?;
            Some((idx, slot))
        })
        .collect::<Vec<_>>();
    slots.sort_by_key(|(idx, _)| *idx);
    slots.into_iter().map(|(_, slot)| slot).collect()
}

fn proc_param_field_is_private(api: &ProcApi, field: &str) -> bool {
    proc_param_slots_for_api(api, field)
        .into_iter()
        .any(|slot| slot.private)
}

fn proc_has_private_params(api: &ProcApi) -> bool {
    api.params.values().any(|slot| slot.private)
}

fn proc_call_arg_kind(api: &ProcApi, arg_name: Option<&str>) -> ProcCallArgKind {
    let Some(name) = arg_name else {
        return ProcCallArgKind::Input;
    };
    if proc_named_arg_is_internal(Some(name)) {
        return ProcCallArgKind::Internal;
    }
    let is_input = proc_input_arg_names(api).contains(name);
    let is_param = proc_named_param_slots(api, name, false).is_some();
    let is_private_param = !is_param && proc_named_param_slots(api, name, true).is_some();
    match (is_input, is_param, is_private_param) {
        (_, _, true) => ProcCallArgKind::PrivateParam,
        (true, true, false) => ProcCallArgKind::Ambiguous,
        (true, false, false) => ProcCallArgKind::Input,
        (false, true, false) => ProcCallArgKind::Param,
        (false, false, false) => ProcCallArgKind::Unknown,
    }
}

fn proc_named_arg_internal_indices(name: &str, args: &[CallArg]) -> HashSet<usize> {
    let is_indexed_call = name == PROC_INDEX_CALL_SENTINEL
        || name
            .strip_prefix(PROC_INDEX_CALL_SENTINEL)
            .is_some_and(|suffix| suffix.starts_with('.'))
        || name
            .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
            .is_some_and(|raw| raw == PROC_INDEX_CALL_SENTINEL);
    if !is_indexed_call {
        return HashSet::new();
    }

    let mut internal = HashSet::<usize>::new();
    let has_named_index_args = args.iter().any(|arg| {
        matches!(
            arg.name.as_deref(),
            Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG)
        )
    });
    if has_named_index_args {
        for (idx, arg) in args.iter().enumerate() {
            if proc_named_arg_is_internal(arg.name.as_deref()) {
                internal.insert(idx);
            }
        }
        return internal;
    }

    let mut positional_seen = 0usize;
    for (idx, arg) in args.iter().enumerate() {
        if proc_named_arg_is_internal(arg.name.as_deref()) {
            internal.insert(idx);
            continue;
        }
        if arg.name.is_none() && positional_seen < 2 {
            internal.insert(idx);
            positional_seen += 1;
        }
    }
    internal
}

fn proc_named_arg_assignment_stmt(
    receiver: &ProcNamedArgReceiver,
    slot_name: &str,
    expr: Expr,
) -> Stmt {
    if let ProcNamedArgReceiver::Indexed {
        array_base,
        index,
        resolved_slot: None,
        access: IndexAccess::Unchecked,
    } = receiver
    {
        return Stmt::Expr {
            loc: Default::default(),
            expr: Expr::UserCall {
                loc: Default::default(),
                name: WRITE_UNSAFE_FN.to_owned(),
                type_args: Vec::new(),
                args: vec![
                    CallArg {
                        name: None,
                        expr: Expr::var(format!("{array_base}.{slot_name}")),
                    },
                    CallArg {
                        name: None,
                        expr: index.clone(),
                    },
                    CallArg { name: None, expr },
                ],
            },
        };
    }
    let target = match receiver {
        ProcNamedArgReceiver::Direct(receiver) => {
            AssignTarget::Var(format!("{receiver}.{slot_name}"))
        }
        ProcNamedArgReceiver::Indexed {
            array_base,
            index,
            resolved_slot,
            ..
        } => resolved_slot.as_ref().map_or_else(
            || AssignTarget::Index {
                base: format!("{array_base}.{slot_name}"),
                index: index.clone(),
            },
            |slot| AssignTarget::Var(format!("{slot}.{slot_name}")),
        ),
    };
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target,
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

fn proc_named_arg_array_slot_expr(
    expr: &Expr,
    slot_idx: usize,
    slot_count: usize,
    call_display_name: &str,
    arg_name: &str,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    expand_expr_to_slots(
        expr,
        slot_count,
        &format!("processor call '{call_display_name}(...)' argument '{arg_name}'"),
        errors,
    )
    .get(slot_idx)
    .cloned()
    .unwrap_or_else(|| expr.clone())
}

fn proc_named_arg_call_target_from_parts(
    name: &str,
    args: &[CallArg],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ProcNamedArgCallTarget> {
    if let Some(instance) = proc_vars.get(name) {
        let api = proc_api.get(&instance.proc_name)?.clone();
        return Some(ProcNamedArgCallTarget {
            display_name: name.to_owned(),
            receiver: ProcNamedArgReceiver::Direct(name.to_owned()),
            api,
        });
    }

    if let Some(proc_var_raw) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
        if proc_var_raw != PROC_INDEX_CALL_SENTINEL {
            let instance = proc_vars.get(proc_var_raw)?;
            let api = proc_api.get(&instance.proc_name)?.clone();
            return Some(ProcNamedArgCallTarget {
                display_name: proc_var_raw.to_owned(),
                receiver: ProcNamedArgReceiver::Direct(proc_var_raw.to_owned()),
                api,
            });
        }
    }

    let is_indexed_call = name == PROC_INDEX_CALL_SENTINEL
        || name
            .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
            .is_some_and(|raw| raw == PROC_INDEX_CALL_SENTINEL);
    if !is_indexed_call {
        return None;
    }

    let (array_base, index_expr, access) =
        proc_index_base_and_expr_from_args(args, "processor indexed call", errors)?;
    let slots = proc_array_slots.get(&array_base)?;
    let (proc_name, api, _) = resolve_proc_array_dispatch_context(
        slots,
        proc_vars,
        proc_api,
        "processor indexed call",
        errors,
    )?;
    let resolved_slot = try_constant_index_i64(&index_expr).and_then(|raw_idx| {
        if raw_idx < 0 || raw_idx >= slots.len() as i64 {
            None
        } else {
            slots.get(raw_idx as usize).cloned()
        }
    });
    Some(ProcNamedArgCallTarget {
        display_name: format!("{array_base}[...]"),
        receiver: ProcNamedArgReceiver::Indexed {
            array_base,
            index: index_expr,
            resolved_slot,
            access,
        },
        api: proc_api.get(&proc_name).cloned().unwrap_or(api),
    })
}

fn proc_index_base_and_expr_from_args(
    args: &[CallArg],
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<(String, Expr, IndexAccess)> {
    let access = if args
        .iter()
        .any(|arg| arg.name.as_deref() == Some(PROC_INDEX_UNCHECKED_ARG))
    {
        IndexAccess::Unchecked
    } else {
        IndexAccess::Clamp
    };
    let named_base = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(PROC_INDEX_BASE_ARG))
        .map(|arg| arg.expr.clone());
    let named_index = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG))
        .map(|arg| arg.expr.clone());
    let (base_expr, index_expr) = if let (Some(base), Some(index)) = (named_base, named_index) {
        (base, index)
    } else {
        let mut positional = args.iter().filter(|arg| arg.name.is_none());
        let Some(base) = positional.next() else {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("{context}: missing processor array base/index"),
            );
            return None;
        };
        let Some(index) = positional.next() else {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!("{context}: missing processor array base/index"),
            );
            return None;
        };
        (base.expr.clone(), index.expr.clone())
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
    Some((array_base, index_expr, access))
}

#[allow(clippy::too_many_arguments)]
fn lower_named_proc_param_calls_in_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &ProcArrayAliases,
    prelude: &mut Vec<Stmt>,
    temp_counter: &mut usize,
    replace_call_with_temp: bool,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => lower_named_proc_param_calls_in_expr(
            index,
            proc_vars,
            proc_array_slots,
            proc_api,
            aliases,
            prelude,
            temp_counter,
            true,
            errors,
        ),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                lower_named_proc_param_calls_in_expr(
                    coordinate,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    prelude,
                    temp_counter,
                    true,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            lower_named_proc_param_calls_in_expr(
                &mut spec.size,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    lower_named_proc_param_calls_in_expr(
                        value,
                        proc_vars,
                        proc_array_slots,
                        proc_api,
                        aliases,
                        prelude,
                        temp_counter,
                        true,
                        errors,
                    );
                }
            }
        }
        Expr::Logical { .. } => {
            reject_named_proc_param_calls_in_expr(
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                "logical expressions",
                errors,
            );
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            lower_named_proc_param_calls_in_expr(
                lhs,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
            lower_named_proc_param_calls_in_expr(
                rhs,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                lower_named_proc_param_calls_in_expr(
                    arg,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    prelude,
                    temp_counter,
                    true,
                    errors,
                );
            }
        }
        Expr::UserCall { .. } => lower_named_proc_param_call_expr(
            expr,
            proc_vars,
            proc_array_slots,
            proc_api,
            aliases,
            prelude,
            temp_counter,
            replace_call_with_temp,
            errors,
        ),
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => lower_named_proc_param_calls_in_expr(
            inner,
            proc_vars,
            proc_array_slots,
            proc_api,
            aliases,
            prelude,
            temp_counter,
            true,
            errors,
        ),
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                lower_named_proc_param_calls_in_expr(
                    value,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    prelude,
                    temp_counter,
                    true,
                    errors,
                );
            }
        }
        Expr::Var { .. } | Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_named_proc_param_call_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &ProcArrayAliases,
    prelude: &mut Vec<Stmt>,
    temp_counter: &mut usize,
    replace_call_with_temp: bool,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_proc_alias_call_sites_in_expr(expr, aliases);

    let Expr::UserCall { name, args, .. } = expr else {
        return;
    };
    // Proc-array slots are known here, so classify receiver syntax before the
    // named-argument pass can interpret the internal receiver marker.
    canonicalize_indexed_proc_receiver_call(name, args, proc_array_slots);

    let Some(mut target) = proc_named_arg_call_target_from_parts(
        name,
        args,
        proc_vars,
        proc_array_slots,
        proc_api,
        errors,
    ) else {
        for arg in args {
            lower_named_proc_param_calls_in_expr(
                &mut arg.expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
        }
        return;
    };

    // Dynamic proc-array receivers feed the proc step/call itself, optional
    // block hooks, active-slot bookkeeping, and buffer selection. Snapshot the
    // source index before any of those rewrites so an effectful index expression
    // is evaluated exactly once regardless of how many ABI consumers it has.
    normalize_indexed_receiver_for_named_proc_args(
        &mut target,
        args,
        proc_vars,
        proc_array_slots,
        proc_api,
        aliases,
        prelude,
        temp_counter,
        errors,
    );

    let has_named_param_arg = args.iter().any(|arg| {
        matches!(
            proc_call_arg_kind(&target.api, arg.name.as_deref()),
            ProcCallArgKind::Param | ProcCallArgKind::PrivateParam | ProcCallArgKind::Ambiguous
        )
    });
    if !has_named_param_arg {
        for arg in args {
            lower_named_proc_param_calls_in_expr(
                &mut arg.expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
        }
        if replace_call_with_temp {
            let temp_name = proc_named_arg_result_temp_name(temp_counter);
            prelude.push(assign_temp_stmt(temp_name.clone(), expr.clone()));
            *expr = Expr::var(temp_name);
        }
        return;
    }

    let internal_arg_indices = proc_named_arg_internal_indices(name, args);
    let mut lowered_args = Vec::<CallArg>::with_capacity(args.len());
    let mut seen_param_args = HashSet::<String>::new();
    for (arg_idx, mut arg) in std::mem::take(args).into_iter().enumerate() {
        let arg_is_internal = internal_arg_indices.contains(&arg_idx)
            || proc_named_arg_is_internal(arg.name.as_deref());
        let arg_kind = if arg_is_internal {
            ProcCallArgKind::Internal
        } else {
            proc_call_arg_kind(&target.api, arg.name.as_deref())
        };
        if !arg_is_internal {
            lower_named_proc_param_calls_in_expr(
                &mut arg.expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                prelude,
                temp_counter,
                true,
                errors,
            );
        }

        match arg_kind {
            ProcCallArgKind::Param => {
                let arg_name = arg.name.clone().unwrap_or_default();
                if !seen_param_args.insert(arg_name.clone()) {
                    push_semantic(
                        DiagCtx::new(arg.expr.loc()),
                        errors,
                        format!(
                            "processor call '{}(...)': duplicate named argument '{}'",
                            target.display_name, arg_name
                        ),
                    );
                    continue;
                }
                let Some(slots) = proc_named_param_slots(&target.api, &arg_name, false) else {
                    continue;
                };
                for (slot_idx, slot) in slots.iter().enumerate() {
                    let mut value = proc_named_arg_array_slot_expr(
                        &arg.expr,
                        slot_idx,
                        slots.len(),
                        &target.display_name,
                        &arg_name,
                        errors,
                    );
                    if let Some(range) = slot.range {
                        value = cast_expr_to_primitive(clamp_expr_to_range(value, range), slot.ty);
                    }
                    prelude.push(proc_named_arg_assignment_stmt(
                        &target.receiver,
                        &slot.name,
                        value,
                    ));
                }
            }
            ProcCallArgKind::Ambiguous => {
                let arg_name = arg.name.clone().unwrap_or_default();
                push_semantic(
                    DiagCtx::new(arg.expr.loc()),
                    errors,
                    format!(
                        "processor call '{}(...)' named argument '{}' matches both an input and a param",
                        target.display_name, arg_name
                    ),
                );
                lowered_args.push(arg);
            }
            ProcCallArgKind::PrivateParam => {
                let arg_name = arg.name.clone().unwrap_or_default();
                push_semantic(
                    DiagCtx::new(arg.expr.loc()),
                    errors,
                    format!(
                        "processor call '{}(...)' named argument '{}' is private and cannot be used as a live param argument; pass it to the constructor or builtin init(...), or expose an event",
                        target.display_name, arg_name
                    ),
                );
                lowered_args.push(arg);
            }
            ProcCallArgKind::Input | ProcCallArgKind::Unknown => {
                let temp_name = proc_named_arg_temp_name(temp_counter);
                prelude.push(assign_temp_stmt(temp_name.clone(), arg.expr));
                arg.expr = Expr::var(temp_name);
                lowered_args.push(arg);
            }
            ProcCallArgKind::Internal => lowered_args.push(arg),
        }
    }
    *args = lowered_args;

    if replace_call_with_temp {
        let temp_name = proc_named_arg_result_temp_name(temp_counter);
        prelude.push(assign_temp_stmt(temp_name.clone(), expr.clone()));
        *expr = Expr::var(temp_name);
    }
}

#[allow(clippy::too_many_arguments)]
fn normalize_indexed_receiver_for_named_proc_args(
    target: &mut ProcNamedArgCallTarget,
    args: &mut [CallArg],
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &ProcArrayAliases,
    prelude: &mut Vec<Stmt>,
    temp_counter: &mut usize,
    errors: &mut Vec<Diagnostic>,
) {
    let ProcNamedArgReceiver::Indexed {
        index,
        resolved_slot,
        ..
    } = &mut target.receiver
    else {
        return;
    };
    if resolved_slot.is_some() {
        return;
    }

    lower_named_proc_param_calls_in_expr(
        index,
        proc_vars,
        proc_array_slots,
        proc_api,
        aliases,
        prelude,
        temp_counter,
        true,
        errors,
    );
    let temp_name = proc_named_arg_temp_name(temp_counter);
    prelude.push(assign_temp_stmt(temp_name.clone(), index.clone()));
    *index = Expr::var(temp_name.clone());
    for arg in &mut *args {
        if arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG) {
            arg.expr = Expr::var(temp_name.clone());
            return;
        }
    }
    let mut positional_seen = 0usize;
    for arg in &mut *args {
        if arg.name.is_none() {
            if positional_seen == 1 {
                arg.expr = Expr::var(temp_name.clone());
                return;
            }
            positional_seen += 1;
        }
    }
}

fn reject_named_proc_param_calls_in_expr(
    expr: &Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            if let Some(target) = proc_named_arg_call_target_from_parts(
                name,
                args,
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            ) {
                let has_named_param_arg = args.iter().any(|arg| {
                    matches!(
                        proc_call_arg_kind(&target.api, arg.name.as_deref()),
                        ProcCallArgKind::Param
                            | ProcCallArgKind::PrivateParam
                            | ProcCallArgKind::Ambiguous
                    )
                });
                if has_named_param_arg {
                    push_semantic(
                        DiagCtx::new(expr.loc()),
                        errors,
                        format!(
                            "processor named param arguments are not supported in {context}; assign the param explicitly before the proc call"
                        ),
                    );
                }
            }
            for arg in args {
                reject_named_proc_param_calls_in_expr(
                    &arg.expr,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    context,
                    errors,
                );
            }
        }
        Expr::Index { index, .. } => reject_named_proc_param_calls_in_expr(
            index,
            proc_vars,
            proc_array_slots,
            proc_api,
            context,
            errors,
        ),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                reject_named_proc_param_calls_in_expr(
                    coordinate,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    context,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            reject_named_proc_param_calls_in_expr(
                &spec.size,
                proc_vars,
                proc_array_slots,
                proc_api,
                context,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    reject_named_proc_param_calls_in_expr(
                        value,
                        proc_vars,
                        proc_array_slots,
                        proc_api,
                        context,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            reject_named_proc_param_calls_in_expr(
                lhs,
                proc_vars,
                proc_array_slots,
                proc_api,
                context,
                errors,
            );
            reject_named_proc_param_calls_in_expr(
                rhs,
                proc_vars,
                proc_array_slots,
                proc_api,
                context,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                reject_named_proc_param_calls_in_expr(
                    arg,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    context,
                    errors,
                );
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => reject_named_proc_param_calls_in_expr(
            inner,
            proc_vars,
            proc_array_slots,
            proc_api,
            context,
            errors,
        ),
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                reject_named_proc_param_calls_in_expr(
                    value,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    context,
                    errors,
                );
            }
        }
        Expr::Var { .. } | Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_named_proc_param_calls_in_stmt(
    stmt: &mut Stmt,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &mut ProcArrayAliases,
    temp_counter: &mut usize,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Stmt> {
    let mut prelude = Vec::<Stmt>::new();
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            rewrite_proc_alias_calls_in_expr(expr, aliases);
            lower_named_proc_param_calls_in_expr(
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
            update_proc_array_aliases_from_assignment(target, expr, proc_array_slots, aliases);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_proc_alias_calls_in_expr(expr, aliases);
            lower_named_proc_param_calls_in_expr(
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_proc_alias_calls_in_expr(cond, aliases);
            lower_named_proc_param_calls_in_expr(
                cond,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
            let mut then_aliases = aliases.clone();
            lower_named_proc_param_calls_in_stmts_with_aliases(
                then_branch,
                proc_vars,
                proc_array_slots,
                proc_api,
                &mut then_aliases,
                temp_counter,
                errors,
            );
            let mut else_aliases = aliases.clone();
            lower_named_proc_param_calls_in_stmts_with_aliases(
                else_branch,
                proc_vars,
                proc_array_slots,
                proc_api,
                &mut else_aliases,
                temp_counter,
                errors,
            );
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
            lower_named_proc_param_calls_in_expr(
                start,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
            lower_named_proc_param_calls_in_expr(
                end,
                proc_vars,
                proc_array_slots,
                proc_api,
                aliases,
                &mut prelude,
                temp_counter,
                false,
                errors,
            );
            if let Some(step_expr) = step {
                rewrite_proc_alias_calls_in_expr(step_expr, aliases);
                lower_named_proc_param_calls_in_expr(
                    step_expr,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    aliases,
                    &mut prelude,
                    temp_counter,
                    false,
                    errors,
                );
            }
            let mut body_aliases = aliases.clone();
            lower_named_proc_param_calls_in_stmts_with_aliases(
                body,
                proc_vars,
                proc_array_slots,
                proc_api,
                &mut body_aliases,
                temp_counter,
                errors,
            );
        }
        Stmt::While { cond, body, .. } => {
            rewrite_proc_alias_calls_in_expr(cond, aliases);
            reject_named_proc_param_calls_in_expr(
                cond,
                proc_vars,
                proc_array_slots,
                proc_api,
                "while conditions",
                errors,
            );
            let mut body_aliases = aliases.clone();
            lower_named_proc_param_calls_in_stmts_with_aliases(
                body,
                proc_vars,
                proc_array_slots,
                proc_api,
                &mut body_aliases,
                temp_counter,
                errors,
            );
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
    prelude
}

fn update_proc_array_aliases_from_assignment(
    target: &AssignTarget,
    expr: &Expr,
    proc_array_slots: &HashMap<String, Vec<String>>,
    aliases: &mut ProcArrayAliases,
) {
    let AssignTarget::Var(name) = target else {
        return;
    };
    if let Some(alias) = proc_array_alias_from_index_expr(expr, proc_array_slots) {
        aliases.insert(name.clone(), alias);
    } else {
        aliases.remove(name);
    }
}

fn proc_array_alias_from_index_expr(
    expr: &Expr,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Option<ProcArrayAliasInfo> {
    let source = indexed_read_source(expr)?;
    let array_base = resolve_proc_array_base_key(source.base, proc_array_slots)?;
    Some(ProcArrayAliasInfo {
        array_base,
        index_expr: source.index.clone(),
        access: source.access,
    })
}

#[allow(clippy::too_many_arguments)]
fn lower_named_proc_param_calls_in_stmts_with_aliases(
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    aliases: &mut ProcArrayAliases,
    temp_counter: &mut usize,
    errors: &mut Vec<Diagnostic>,
) {
    let mut rewritten = Vec::<Stmt>::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        let prelude = lower_named_proc_param_calls_in_stmt(
            &mut stmt,
            proc_vars,
            proc_array_slots,
            proc_api,
            aliases,
            temp_counter,
            errors,
        );
        rewritten.extend(prelude);
        rewritten.push(stmt);
    }
    *stmts = rewritten;
}

pub(crate) fn lower_named_proc_param_calls_in_stmts(
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut aliases = ProcArrayAliases::new();
    let mut temp_counter = 0usize;
    lower_named_proc_param_calls_in_stmts_with_aliases(
        stmts,
        proc_vars,
        proc_array_slots,
        proc_api,
        &mut aliases,
        &mut temp_counter,
        errors,
    );
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
        match param.ty {
            ProcEventParamTypeSpec::FixedArray { len, .. } => {
                validate_fixed_array_event_arg(
                    &resolved_expr,
                    len,
                    &format!(
                        "processor event call '{call_display_name}(...)' argument '{}'",
                        param.name
                    ),
                    errors,
                );
                expanded.push(CallArg {
                    name: None,
                    expr: resolved_expr,
                });
                continue;
            }
            ProcEventParamTypeSpec::Slice { .. } => {
                expanded.push(CallArg {
                    name: None,
                    expr: resolved_expr,
                });
                continue;
            }
            ProcEventParamTypeSpec::Scalar { .. } => {}
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

type ExpandedProcPortSpecs = (
    Vec<String>,
    HashMap<String, PrimitiveType>,
    Vec<ProcPortSpec>,
    HashMap<String, Vec<String>>,
);

pub(super) fn expand_proc_port_specs(
    proc_name: &str,
    ports: &[PortDecl],
    kind: &str,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ExpandedProcPortSpecs {
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
                let slot_defaults = slots
                    .iter()
                    .map(|slot| defaults.get(slot).copied().map(typed_const_expr))
                    .collect::<Vec<_>>();
                array_slots.insert(port.name.clone(), slots.clone());
                port_specs.push(ProcPortSpec {
                    name: port.name.clone(),
                    slots,
                    defaults: slot_defaults,
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
                        private: param.private,
                        ty,
                        default: Some(typed_const_expr(default)),
                        range,
                        bind: param.bind.clone(),
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
                        private: param.private,
                        ty: PrimitiveType::F32,
                        default: Some(typed_const_expr(default)),
                        range,
                        bind: param.bind.clone(),
                    }],
                });
            }
            Some(DeclType::ArrayGeneric { elem, size }) => {
                if let Some(bind) = &param.bind {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' uses bind hook '=> {bind}', but binds are only supported on primitive scalar params",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
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
                        private: param.private,
                        ty: PrimitiveType::F32,
                        default: slot_defaults.get(idx).cloned().unwrap_or(None),
                        range: None,
                        bind: None,
                    });
                }
                field_array_slots.insert(param.name.clone(), slot_names);
                specs.push(ProcParamSpec {
                    name: param.name.clone(),
                    slots,
                });
            }
            Some(DeclType::Tuple(_)) => {
                if let Some(bind) = &param.bind {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' uses bind hook '=> {bind}', but binds are only supported on primitive scalar params",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
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
                if let Some(bind) = &param.bind {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "processor '{proc_name}' param '{}' uses bind hook '=> {bind}', but binds are only supported on primitive scalar params",
                            param.name
                        ),
                        param.loc.as_ref(),
                    ));
                }
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
                        private: param.private,
                        ty: *elem,
                        default: slot_defaults
                            .get(idx)
                            .cloned()
                            .unwrap_or(Some(Expr::number(0.0))),
                        range: None,
                        bind: None,
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
    let buffer = onda_frontend::BufferType {
        elem: BufferElemType::Primitive(spec.elem_ty),
        channels,
    };
    if spec.is_array {
        FnParamType::BufferArray {
            buffer,
            len: spec.array_len,
        }
    } else {
        FnParamType::Buffer(buffer)
    }
}

pub(super) fn rewrite_proc_calls_in_expr(
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let expr_loc = expr.loc();
    let expr_diag = DiagCtx::new(expr_loc);
    match expr {
        Expr::Index { base, index, .. } => {
            rewrite_proc_calls_in_expr(index, proc_vars, proc_array_slots, proc_api, errors);
            if reject_private_proc_param_read_path(
                base,
                expr_loc.into(),
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            ) {
                return;
            }
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_proc_calls_in_expr(
                    coordinate,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
            if reject_private_proc_param_read_path(
                base,
                expr_loc.into(),
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            ) {
                return;
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
            canonicalize_indexed_proc_receiver_call(name, args, proc_array_slots);
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
                        access,
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
                        if api.outputs.names.len() != 1 {
                            push_semantic(
                                expr_diag,
                                errors,
                                format!(
                                    "processor call '{}[...]' has {} outputs; use '{}[...]().<endpoint>'/{} or call as statement then read fields",
                                    array_base,
                                    api.outputs.names.len(),
                                    array_base,
                                    proc_output_alias_label(api.outputs.timing)
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
                            access,
                            errors,
                        );
                        *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                        *args = rewritten;
                        return;
                    }
                }
            }

            if let Some(proc_var_raw) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
                let mut dynamic_index = None::<(String, Expr, Vec<String>, IndexAccess)>;
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
                            access,
                        } => {
                            dynamic_index = Some((array_base, index_expr, slots, access));
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

                if let Some((array_base, index_expr, slots, access)) = dynamic_index {
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
                    if proc_param_field_is_private(&api, &field_name) {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "processor '{proc_name}' param '{field_name}' is private and cannot be read through '{array_base}.{field_name}'"
                            ),
                        );
                        return;
                    }
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
                        access,
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
                    if param_slot.private {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "processor '{proc_name}' param '{field_name}' is private and cannot be read through '{proc_var}.{field_name}'"
                            ),
                        );
                        return;
                    }
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
                if api.outputs.names.len() != 1 {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!(
                            "processor call '{}(...)' has {} outputs; use '{}(...).<endpoint>'/{} or call as statement then read fields",
                            name,
                            api.outputs.names.len(),
                            name,
                            proc_output_alias_label(api.outputs.timing)
                        ),
                    );
                    return;
                }
                let mut rewritten = Vec::<CallArg>::with_capacity(args.len() + 1);
                rewritten.push(CallArg {
                    name: None,
                    expr: proc_instance_self_expr(name, proc_array_slots),
                });
                let expanded_args = expand_proc_call_args(args, api, name, errors);
                rewritten.extend(expanded_args);
                let expanded_buffers = expand_proc_buffer_call_args(instance, api, name, errors);
                rewritten.extend(expanded_buffers);
                *name = format!("{proc_name}{PROC_CALL_OUT_FN_PREFIX}0");
                *args = rewritten;
                return;
            }

            if let Some((base_raw, event_name)) = split_dot_path(name) {
                let mut dynamic_index = None::<(String, Expr, Vec<String>, IndexAccess)>;
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
                            access,
                        } => {
                            dynamic_index = Some((array_base, index_expr, slots, access));
                            String::new()
                        }
                    }
                } else {
                    base_raw.to_owned()
                };

                if let Some((array_base, _, slots, _)) = &dynamic_index {
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

                if let Some((array_base, index_expr, slots, _access)) = dynamic_index {
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
                }
            }
        }
        Expr::Var { name, .. } => {
            if reject_private_proc_param_read_path(
                name,
                expr_loc.into(),
                proc_vars,
                proc_array_slots,
                proc_api,
                errors,
            ) {
                return;
            }
            normalize_proc_output_alias_path(name, proc_vars, proc_api);
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn proc_output_alias_prefix(timing: OutputTiming) -> &'static str {
    match timing {
        OutputTiming::Sample => "out",
        OutputTiming::Block => "kout",
    }
}

pub(super) fn proc_output_alias_label(timing: OutputTiming) -> &'static str {
    match timing {
        OutputTiming::Sample => "outN",
        OutputTiming::Block => "koutN",
    }
}

fn parse_proc_output_alias_index(field: &str, timing: OutputTiming) -> Option<usize> {
    parse_numbered_port_index(field, proc_output_alias_prefix(timing)).map(|idx| idx - 1)
}

pub(super) fn resolve_proc_output_field_index(
    api: &ProcApi,
    field_name: &str,
    call_display_name: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if let Some(idx) = api.outputs.names.iter().position(|out| out == field_name) {
        return Some(idx);
    }
    if let Some(idx) = parse_proc_output_alias_index(field_name, api.outputs.timing) {
        if idx < api.outputs.names.len() {
            return Some(idx);
        }
        push_semantic(
            diag,
            errors,
            format!(
                "processor output alias '{}' is out of range (outputs: {})",
                field_name,
                api.outputs.names.len()
            ),
        );
        return None;
    }
    push_semantic(
        diag,
        errors,
        format!(
            "unknown processor output '{}' for '{}'; expected one of [{}] or {}",
            field_name,
            call_display_name,
            api.outputs.names.join(", "),
            proc_output_alias_label(api.outputs.timing)
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
    if api.outputs.names.iter().any(|out_name| out_name == field) {
        return;
    }
    let Some(out_idx) = parse_proc_output_alias_index(field, api.outputs.timing) else {
        return;
    };
    let Some(actual_out_name) = api.outputs.names.get(out_idx) else {
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
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                normalize_proc_output_aliases_in_expr(coordinate, proc_vars, proc_api);
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
        AssignTarget::Slice {
            base,
            selector,
            channel,
            start,
            end,
        } => {
            normalize_proc_output_alias_path(base, proc_vars, proc_api);
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                normalize_proc_output_aliases_in_expr(coordinate, proc_vars, proc_api);
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

fn split_receiver_field(name: &str) -> Option<(&str, &str)> {
    let (base, field) = name.rsplit_once('.')?;
    if base.is_empty() || field.is_empty() {
        return None;
    }
    Some((base, field))
}

fn proc_api_for_receiver<'a>(
    receiver: &str,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a ProcApi)> {
    if let Some(instance) = proc_vars.get(receiver) {
        let api = proc_api.get(&instance.proc_name)?;
        return Some((instance.proc_name.as_str(), api));
    }
    if let Some(stripped) = receiver.strip_prefix("self.") {
        if let Some(instance) = proc_vars.get(stripped) {
            let api = proc_api.get(&instance.proc_name)?;
            return Some((instance.proc_name.as_str(), api));
        }
    }

    let array_base = resolve_proc_array_base_key(receiver, proc_array_slots)?;
    let slots = proc_array_slots.get(&array_base)?;
    let first_slot = slots.first()?;
    let instance = proc_vars.get(first_slot)?;
    let api = proc_api.get(&instance.proc_name)?;
    Some((instance.proc_name.as_str(), api))
}

fn proc_param_slot_for_receiver<'a>(
    receiver: &str,
    field: &str,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a ProcParamSlotSpec)> {
    let (proc_name, api) = proc_api_for_receiver(receiver, proc_vars, proc_array_slots, proc_api)?;
    let slot = api.params.get(field)?;
    Some((proc_name, slot))
}

fn proc_private_param_field_for_receiver<'a>(
    receiver: &str,
    field: &str,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a ProcApi)> {
    let (proc_name, api) = proc_api_for_receiver(receiver, proc_vars, proc_array_slots, proc_api)?;
    if proc_param_field_is_private(api, field) {
        Some((proc_name, api))
    } else {
        None
    }
}

fn proc_array_receiver_expr_base(receiver: &str, array_base: String) -> String {
    if receiver.starts_with("self.") && !array_base.starts_with("self.") {
        format!("self.{array_base}")
    } else {
        array_base
    }
}

fn bound_proc_receiver_expr(
    receiver: &str,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Expr {
    let raw_receiver = receiver.strip_prefix("self.").unwrap_or(receiver);
    if let Some((array_base, slot_idx)) = find_proc_array_slot(raw_receiver, proc_array_slots) {
        return Expr::Index {
            loc: Default::default(),
            base: proc_array_receiver_expr_base(receiver, array_base),
            index: Box::new(Expr::int(slot_idx as i64)),
        };
    }
    Expr::var(receiver.to_owned())
}

fn bound_proc_indexed_receiver_expr(
    receiver: &str,
    index: &Expr,
    proc_array_slots: &HashMap<String, Vec<String>>,
) -> Expr {
    let receiver_base = resolve_proc_array_base_key(receiver, proc_array_slots)
        .unwrap_or_else(|| receiver.to_owned());
    Expr::Index {
        loc: Default::default(),
        base: proc_array_receiver_expr_base(receiver, receiver_base),
        index: Box::new(index.clone()),
    }
}

fn bind_hook_call_stmt(name: String, receiver: Expr) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name,
            type_args: Vec::new(),
            args: vec![CallArg {
                name: None,
                expr: receiver,
            }],
        },
    }
}

fn nested_bind_hook_call_stmt(owner_proc: &str, nested_path: &str, hook: &str) -> Stmt {
    bind_hook_call_stmt(
        proc_local_nested_bind_hidden_def_name(owner_proc, nested_path, hook),
        Expr::var("self"),
    )
}

fn nested_bind_hook_index_cond(index: &Expr, op: CmpOp, value: i64) -> Expr {
    Expr::Compare {
        loc: Default::default(),
        op,
        lhs: Box::new(Expr::Cast {
            loc: Default::default(),
            to: PrimitiveType::I32,
            expr: Box::new(index.clone()),
        }),
        rhs: Box::new(Expr::int(value)),
    }
}

fn nested_proc_array_bind_hook_dispatch_stmts(
    owner_proc: &str,
    slots: &[String],
    index: &Expr,
    hook: &str,
) -> Vec<Stmt> {
    let Some((first, rest)) = slots.split_first() else {
        return Vec::new();
    };
    if rest.is_empty() {
        return vec![nested_bind_hook_call_stmt(owner_proc, first, hook)];
    }

    let last_idx = slots.len() - 1;
    let mut else_branch = Vec::<Stmt>::new();
    for idx in (1..last_idx).rev() {
        else_branch = vec![Stmt::If {
            loc: Default::default(),
            cond: nested_bind_hook_index_cond(index, CmpOp::Eq, idx as i64),
            then_branch: vec![nested_bind_hook_call_stmt(owner_proc, &slots[idx], hook)],
            else_branch,
        }];
    }
    else_branch = vec![Stmt::If {
        loc: Default::default(),
        cond: nested_bind_hook_index_cond(index, CmpOp::Ge, last_idx as i64),
        then_branch: vec![nested_bind_hook_call_stmt(
            owner_proc,
            &slots[last_idx],
            hook,
        )],
        else_branch,
    }];

    vec![Stmt::If {
        loc: Default::default(),
        cond: nested_bind_hook_index_cond(index, CmpOp::Le, 0),
        then_branch: vec![nested_bind_hook_call_stmt(owner_proc, first, hook)],
        else_branch,
    }]
}

fn nested_bound_proc_param_hook_stmts(
    owner_proc: Option<&str>,
    receiver: &str,
    index: Option<&Expr>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    hook: &str,
) -> Option<Vec<Stmt>> {
    let owner_proc = owner_proc?;
    let nested_path = receiver.strip_prefix("self.")?;
    if nested_path.is_empty() {
        return None;
    }

    if let Some(index) = index {
        if let Some(array_base) = resolve_proc_array_base_key(receiver, proc_array_slots) {
            let slots = proc_array_slots.get(&array_base)?;
            if slots.is_empty() {
                return None;
            }
            if let Some(raw_idx) = try_constant_index_i64(index) {
                let clamped = raw_idx.clamp(0, slots.len().saturating_sub(1) as i64) as usize;
                return Some(vec![nested_bind_hook_call_stmt(
                    owner_proc,
                    &slots[clamped],
                    hook,
                )]);
            }
            return Some(nested_proc_array_bind_hook_dispatch_stmts(
                owner_proc, slots, index, hook,
            ));
        }
    }

    Some(vec![nested_bind_hook_call_stmt(
        owner_proc,
        nested_path,
        hook,
    )])
}

fn flattened_nested_proc_field<'a>(
    path: &'a str,
    proc_vars: &HashMap<String, ProcCallInstance>,
) -> Option<(String, &'a str)> {
    let path = path.strip_prefix("self.")?;
    let mut nested_paths = proc_vars
        .keys()
        .filter(|name| name.as_str() != "self")
        .map(|name| name.as_str())
        .collect::<Vec<_>>();
    nested_paths.sort_by_key(|name| std::cmp::Reverse(name.len()));
    for nested_path in nested_paths {
        let prefix = format!("{nested_path}__");
        if let Some(field) = path.strip_prefix(&prefix) {
            if !field.is_empty() {
                return Some((nested_path.to_owned(), field));
            }
        }
    }
    None
}

fn proc_api_for_nested_path<'a>(
    nested_path: &str,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a ProcApi)> {
    if let Some(instance) = proc_vars.get(nested_path) {
        let api = proc_api.get(&instance.proc_name)?;
        return Some((instance.proc_name.as_str(), api));
    }
    let array_base = resolve_proc_array_base_key(nested_path, proc_array_slots)?;
    let slots = proc_array_slots.get(&array_base)?;
    let first_slot = slots.first()?;
    let instance = proc_vars.get(first_slot)?;
    let api = proc_api.get(&instance.proc_name)?;
    Some((instance.proc_name.as_str(), api))
}

fn proc_param_slot_for_nested_path<'a>(
    nested_path: &str,
    field: &str,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a ProcParamSlotSpec)> {
    let (proc_name, api) =
        proc_api_for_nested_path(nested_path, proc_vars, proc_array_slots, proc_api)?;
    let slot = api.params.get(field)?;
    Some((proc_name, slot))
}

fn proc_private_param_field_for_nested_path<'a>(
    nested_path: &str,
    field: &str,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a ProcApi)> {
    let (proc_name, api) =
        proc_api_for_nested_path(nested_path, proc_vars, proc_array_slots, proc_api)?;
    if proc_param_field_is_private(api, field) {
        Some((proc_name, api))
    } else {
        None
    }
}

fn bound_proc_param_hook_stmts_for_flattened_nested_target(
    owner_proc: Option<&str>,
    nested_path: &str,
    field: &str,
    index: Option<&Expr>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
) -> Vec<Stmt> {
    let Some(owner_proc) = owner_proc else {
        return Vec::new();
    };
    let Some((_proc_name, param_slot)) =
        proc_param_slot_for_nested_path(nested_path, field, proc_vars, proc_array_slots, proc_api)
    else {
        return Vec::new();
    };
    let Some(hook) = param_slot.bind.as_ref() else {
        return Vec::new();
    };
    if let Some(index) = index {
        if let Some(array_base) = resolve_proc_array_base_key(nested_path, proc_array_slots) {
            let Some(slots) = proc_array_slots.get(&array_base) else {
                return Vec::new();
            };
            if slots.is_empty() {
                return Vec::new();
            }
            if let Some(raw_idx) = try_constant_index_i64(index) {
                let clamped = raw_idx.clamp(0, slots.len().saturating_sub(1) as i64) as usize;
                return vec![nested_bind_hook_call_stmt(
                    owner_proc,
                    &slots[clamped],
                    hook,
                )];
            }
            return nested_proc_array_bind_hook_dispatch_stmts(owner_proc, slots, index, hook);
        }
    }
    vec![nested_bind_hook_call_stmt(owner_proc, nested_path, hook)]
}

fn maybe_clamp_flattened_nested_proc_param_assignment_expr(
    target: &AssignTarget,
    expr: &mut Expr,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
) {
    let Some((nested_path, field)) = (match target {
        AssignTarget::Var(name) => flattened_nested_proc_field(name, proc_vars),
        AssignTarget::Index { base, .. } => flattened_nested_proc_field(base, proc_vars),
        AssignTarget::Slice { .. } | AssignTarget::Tuple(_) => None,
    }) else {
        return;
    };
    let Some((_proc_name, param_slot)) =
        proc_param_slot_for_nested_path(&nested_path, field, proc_vars, proc_array_slots, proc_api)
    else {
        return;
    };
    if let Some(range) = param_slot
        .range
        .filter(|range| !expr_is_clamped_to_range(expr, *range, param_slot.ty))
    {
        let original = std::mem::replace(expr, Expr::number(0.0));
        *expr = cast_expr_to_primitive(clamp_expr_to_range(original, range), param_slot.ty);
    }
}

fn dynamic_params_assignment_target<'a>(
    target: &AssignTarget,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a ProcApi, String)> {
    let base = match target {
        AssignTarget::Index { base, .. } | AssignTarget::Slice { base, .. } => base,
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => return None,
    };

    if let Some((nested_path, field)) = flattened_nested_proc_field(base, proc_vars) {
        if field == "params" {
            let (proc_name, api) =
                proc_api_for_nested_path(&nested_path, proc_vars, proc_array_slots, proc_api)?;
            return Some((proc_name, api, format!("{nested_path}.params")));
        }
    }

    let (receiver, field) = split_receiver_field(base)?;
    if field != "params" {
        return None;
    }
    let (proc_name, api) = proc_api_for_receiver(receiver, proc_vars, proc_array_slots, proc_api)?;
    Some((proc_name, api, base.clone()))
}

fn private_proc_param_assignment_target<'a>(
    target: &AssignTarget,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<(&'a str, String, String)> {
    let (base, field) = match target {
        AssignTarget::Var(name) => {
            if let Some((nested_path, field)) = flattened_nested_proc_field(name, proc_vars) {
                if let Some((proc_name, _api)) = proc_private_param_field_for_nested_path(
                    &nested_path,
                    field,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                ) {
                    return Some((proc_name, nested_path, field.to_owned()));
                }
            }
            split_receiver_field(name)?
        }
        AssignTarget::Index { base, .. } | AssignTarget::Slice { base, .. } => {
            if let Some((nested_path, field)) = flattened_nested_proc_field(base, proc_vars) {
                if let Some((proc_name, _api)) = proc_private_param_field_for_nested_path(
                    &nested_path,
                    field,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                ) {
                    return Some((proc_name, nested_path, field.to_owned()));
                }
            }
            split_receiver_field(base)?
        }
        AssignTarget::Tuple(_) => return None,
    };
    if base == "self" {
        return None;
    }
    let (proc_name, _api) =
        proc_private_param_field_for_receiver(base, field, proc_vars, proc_array_slots, proc_api)?;
    Some((proc_name, base.to_owned(), field.to_owned()))
}

fn reject_private_proc_param_assignment(
    target: &AssignTarget,
    target_loc: Span,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    authorized_receiver: Option<&str>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some((proc_name, receiver, field)) =
        private_proc_param_assignment_target(target, proc_vars, proc_array_slots, proc_api)
    else {
        return;
    };
    if authorized_receiver == Some(receiver.as_str()) {
        return;
    }
    push_semantic(
        DiagCtx::new(target_loc),
        errors,
        format!(
            "processor '{proc_name}' param '{field}' is private and cannot be assigned through '{receiver}.{field}'; pass it to the constructor or builtin init(...), or expose an event"
        ),
    );
}

enum PrivateParamReadAccess<'a> {
    Field {
        proc_name: &'a str,
        receiver: String,
        field: String,
    },
    DynamicParams {
        proc_name: &'a str,
        display: String,
    },
}

fn private_proc_param_read_access<'a>(
    path: &str,
    proc_vars: &'a HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &'a HashMap<String, ProcApi>,
) -> Option<PrivateParamReadAccess<'a>> {
    if let Some((nested_path, field)) = flattened_nested_proc_field(path, proc_vars) {
        let (proc_name, api) =
            proc_api_for_nested_path(&nested_path, proc_vars, proc_array_slots, proc_api)?;
        if field == "params" && proc_has_private_params(api) {
            return Some(PrivateParamReadAccess::DynamicParams {
                proc_name,
                display: format!("{nested_path}.params"),
            });
        }
        if proc_param_field_is_private(api, field) {
            return Some(PrivateParamReadAccess::Field {
                proc_name,
                receiver: nested_path,
                field: field.to_owned(),
            });
        }
    }

    let (receiver, field) = split_receiver_field(path)?;
    if receiver == "self" {
        return None;
    }
    let (proc_name, api) = proc_api_for_receiver(receiver, proc_vars, proc_array_slots, proc_api)?;
    if field == "params" && proc_has_private_params(api) {
        return Some(PrivateParamReadAccess::DynamicParams {
            proc_name,
            display: path.to_owned(),
        });
    }
    if proc_param_field_is_private(api, field) {
        return Some(PrivateParamReadAccess::Field {
            proc_name,
            receiver: receiver.to_owned(),
            field: field.to_owned(),
        });
    }
    None
}

fn reject_private_proc_param_read_path(
    path: &str,
    loc: Span,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some(access) = private_proc_param_read_access(path, proc_vars, proc_array_slots, proc_api)
    else {
        return false;
    };
    match access {
        PrivateParamReadAccess::Field {
            proc_name,
            receiver,
            field,
        } => push_semantic(
            DiagCtx::new(loc),
            errors,
            format!(
                "processor '{proc_name}' param '{field}' is private and cannot be read through '{receiver}.{field}'"
            ),
        ),
        PrivateParamReadAccess::DynamicParams { proc_name, display } => push_semantic(
            DiagCtx::new(loc),
            errors,
            format!(
                "processor '{proc_name}' has private params, so dynamic param access through '{display}[...]' is not supported"
            ),
        ),
    }
    true
}

fn reject_bound_proc_dynamic_params_assignment(
    target: &AssignTarget,
    target_loc: Span,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some((proc_name, api, display)) =
        dynamic_params_assignment_target(target, proc_vars, proc_array_slots, proc_api)
    else {
        return;
    };
    if proc_has_private_params(api) {
        push_semantic(
            DiagCtx::new(target_loc),
            errors,
            format!(
                "processor '{proc_name}' has private params, so assignment through dynamic '{display}[...]' is not supported; assign a live named param or expose an event"
            ),
        );
        return;
    }
    if !api.has_bound_params {
        return;
    }
    push_semantic(
        DiagCtx::new(target_loc),
        errors,
        format!(
            "processor '{proc_name}' has bound params, so assignment through dynamic '{display}[...]' is not supported; assign the named param instead"
        ),
    );
}

fn bound_proc_param_hook_stmts_for_target(
    owner_proc: Option<&str>,
    target: &AssignTarget,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
) -> Vec<Stmt> {
    match target {
        AssignTarget::Var(name) => {
            if let Some((nested_path, field)) = flattened_nested_proc_field(name, proc_vars) {
                return bound_proc_param_hook_stmts_for_flattened_nested_target(
                    owner_proc,
                    &nested_path,
                    field,
                    None,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                );
            }
        }
        AssignTarget::Index { base, index } => {
            if let Some((nested_path, field)) = flattened_nested_proc_field(base, proc_vars) {
                return bound_proc_param_hook_stmts_for_flattened_nested_target(
                    owner_proc,
                    &nested_path,
                    field,
                    Some(index),
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                );
            }
        }
        AssignTarget::Slice { .. } | AssignTarget::Tuple(_) => {}
    }

    let (receiver, field, index, receiver_expr) = match target {
        AssignTarget::Var(name) => {
            let Some((receiver, field)) = split_receiver_field(name) else {
                return Vec::new();
            };
            (
                receiver,
                field,
                None,
                bound_proc_receiver_expr(receiver, proc_array_slots),
            )
        }
        AssignTarget::Index { base, index } => {
            let Some((receiver, field)) = split_receiver_field(base) else {
                return Vec::new();
            };
            (
                receiver,
                field,
                Some(index),
                bound_proc_indexed_receiver_expr(receiver, index, proc_array_slots),
            )
        }
        AssignTarget::Slice { .. } | AssignTarget::Tuple(_) => return Vec::new(),
    };
    let Some((proc_name, param_slot)) =
        proc_param_slot_for_receiver(receiver, field, proc_vars, proc_array_slots, proc_api)
    else {
        return Vec::new();
    };
    let Some(hook) = param_slot.bind.as_ref() else {
        return Vec::new();
    };
    if let Some(stmts) =
        nested_bound_proc_param_hook_stmts(owner_proc, receiver, index, proc_array_slots, hook)
    {
        return stmts;
    }
    vec![bind_hook_call_stmt(
        proc_local_bind_hidden_def_name(proc_name, hook),
        receiver_expr,
    )]
}

fn rewrite_proc_alias_assign_target(
    target: &mut AssignTarget,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) {
    match target {
        AssignTarget::Var(name) => {
            let Some((base, field)) = split_dot_path(name) else {
                return;
            };
            let Some(alias) = aliases.get(base) else {
                return;
            };
            *target = AssignTarget::Index {
                base: format!("{}.{}", alias.array_base, field),
                index: alias.index_expr.clone(),
            };
        }
        AssignTarget::Index { base, .. } | AssignTarget::Slice { base, .. } => {
            let Some((receiver, field)) = split_dot_path(base) else {
                return;
            };
            let Some(alias) = aliases.get(receiver) else {
                return;
            };
            *base = format!("{}.{}", alias.array_base, field);
        }
        AssignTarget::Tuple(_) => {}
    }
}

fn bound_hook_temp_name(counter: &mut usize, kind: &str) -> String {
    let name = format!("__onda_bound_hook_{kind}_tmp_{}", *counter);
    *counter += 1;
    name
}

fn normalize_indexed_bound_hook_assignment(stmt: &mut Stmt, temp_counter: &mut usize) -> Vec<Stmt> {
    let Stmt::Assign { target, expr, .. } = stmt else {
        return Vec::new();
    };
    let AssignTarget::Index { index, .. } = target else {
        return Vec::new();
    };
    if try_constant_index_i64(index).is_some() {
        return Vec::new();
    }

    let value_tmp = bound_hook_temp_name(temp_counter, "value");
    let original_expr = std::mem::replace(expr, Expr::var(value_tmp.clone()));
    let index_tmp = bound_hook_temp_name(temp_counter, "index");
    let original_index = std::mem::replace(index, Expr::var(index_tmp.clone()));

    vec![
        assign_temp_stmt(value_tmp, original_expr),
        assign_temp_stmt(index_tmp, original_index),
    ]
}

fn inject_bound_proc_param_hooks_in_stmts_inner(
    stmts: &mut Vec<Stmt>,
    owner_proc: Option<&str>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    authorized_private_receiver: Option<&str>,
    errors: &mut Vec<Diagnostic>,
    skip_top_level_indices: Option<&HashSet<usize>>,
    temp_counter: &mut usize,
) {
    let mut rewritten = Vec::<Stmt>::with_capacity(stmts.len());
    for (idx, mut stmt) in std::mem::take(stmts).into_iter().enumerate() {
        match &mut stmt {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                inject_bound_proc_param_hooks_in_stmts_inner(
                    then_branch,
                    owner_proc,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    authorized_private_receiver,
                    errors,
                    None,
                    temp_counter,
                );
                inject_bound_proc_param_hooks_in_stmts_inner(
                    else_branch,
                    owner_proc,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    authorized_private_receiver,
                    errors,
                    None,
                    temp_counter,
                );
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                inject_bound_proc_param_hooks_in_stmts_inner(
                    body,
                    owner_proc,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    authorized_private_receiver,
                    errors,
                    None,
                    temp_counter,
                );
            }
            Stmt::Const { .. }
            | Stmt::Assign { .. }
            | Stmt::Expr { .. }
            | Stmt::Return { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => {}
        }
        let skip_hook = skip_top_level_indices
            .map(|indices| indices.contains(&idx))
            .unwrap_or(false);
        if let Stmt::Assign {
            target, target_loc, ..
        } = &stmt
        {
            if !skip_hook {
                reject_private_proc_param_assignment(
                    target,
                    *target_loc,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    authorized_private_receiver,
                    errors,
                );
                reject_bound_proc_dynamic_params_assignment(
                    target,
                    *target_loc,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                    errors,
                );
            }
        }
        if let Stmt::Assign { target, expr, .. } = &mut stmt {
            maybe_clamp_proc_param_assignment_expr(
                target,
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
            );
            maybe_clamp_flattened_nested_proc_param_assignment_expr(
                target,
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
            );
        }
        let mut hook_stmts = if !skip_hook {
            if let Stmt::Assign { target, .. } = &stmt {
                bound_proc_param_hook_stmts_for_target(
                    owner_proc,
                    target,
                    proc_vars,
                    proc_array_slots,
                    proc_api,
                )
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        if !hook_stmts.is_empty() {
            let prelude = normalize_indexed_bound_hook_assignment(&mut stmt, temp_counter);
            if !prelude.is_empty() {
                if let Stmt::Assign { target, .. } = &stmt {
                    hook_stmts = bound_proc_param_hook_stmts_for_target(
                        owner_proc,
                        target,
                        proc_vars,
                        proc_array_slots,
                        proc_api,
                    );
                }
                rewritten.extend(prelude);
            }
        }
        rewritten.push(stmt);
        rewritten.extend(hook_stmts);
    }
    *stmts = rewritten;
}

pub(super) fn inject_bound_proc_param_hooks_in_stmts(
    owner_proc: Option<&str>,
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut temp_counter = 0usize;
    inject_bound_proc_param_hooks_in_stmts_inner(
        stmts,
        owner_proc,
        proc_vars,
        proc_array_slots,
        proc_api,
        None,
        errors,
        None,
        &mut temp_counter,
    );
}

pub(super) fn inject_bound_proc_param_hooks_in_stmts_skipping_top_level(
    owner_proc: Option<&str>,
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
    skip_top_level_indices: &HashSet<usize>,
) {
    let mut temp_counter = 0usize;
    inject_bound_proc_param_hooks_in_stmts_inner(
        stmts,
        owner_proc,
        proc_vars,
        proc_array_slots,
        proc_api,
        None,
        errors,
        Some(skip_top_level_indices),
        &mut temp_counter,
    );
}

pub(super) fn inject_bound_proc_param_hooks_in_nested_event_stmts(
    owner_proc: &str,
    nested_receiver: &str,
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut temp_counter = 0usize;
    inject_bound_proc_param_hooks_in_stmts_inner(
        stmts,
        Some(owner_proc),
        proc_vars,
        proc_array_slots,
        proc_api,
        Some(nested_receiver),
        errors,
        None,
        &mut temp_counter,
    );
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
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
) {
    let Some((base, field)) = (match target {
        AssignTarget::Var(name) => split_receiver_field(name),
        AssignTarget::Index { base, .. } => split_receiver_field(base),
        AssignTarget::Slice { base, .. } => split_dot_path(base),
        AssignTarget::Tuple(_) => None,
    }) else {
        return;
    };
    let Some((_proc_name, param_slot)) =
        proc_param_slot_for_receiver(base, field, proc_vars, proc_array_slots, proc_api)
    else {
        return;
    };
    if let Some(range) = param_slot
        .range
        .filter(|range| !expr_is_clamped_to_range(expr, *range, param_slot.ty))
    {
        let original = std::mem::replace(expr, Expr::number(0.0));
        *expr = cast_expr_to_primitive(clamp_expr_to_range(original, range), param_slot.ty);
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
                if let Some(alias) = proc_array_alias_from_index_expr(expr, proc_array_slots) {
                    aliases.insert(name.clone(), alias);
                } else {
                    aliases.remove(name);
                }
            }
            rewrite_proc_alias_assign_target(target, aliases);
            normalize_proc_array_slot_assign_target(target, proc_array_slots);
            rewrite_proc_alias_calls_in_expr(expr, aliases);
            normalize_proc_output_aliases_in_assign_target(target, proc_vars, proc_api);
            rewrite_proc_calls_in_expr(expr, proc_vars, proc_array_slots, proc_api, errors);
            maybe_clamp_proc_param_assignment_expr(
                target,
                expr,
                proc_vars,
                proc_array_slots,
                proc_api,
            );
        }
        Stmt::Expr { expr, .. } => {
            rewrite_proc_alias_calls_in_expr(expr, aliases);
            let mut handled_proc_stmt_call = false;
            if let Expr::UserCall { name, args, .. } = expr {
                canonicalize_indexed_proc_receiver_call(name, args, proc_array_slots);
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
                            access,
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
                                access,
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
                        let mut dynamic_index = None::<(String, Expr, Vec<String>, IndexAccess)>;
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
                                    access,
                                } => {
                                    dynamic_index = Some((array_base, index_expr, slots, access));
                                    String::new()
                                }
                            }
                        } else {
                            base_raw.to_owned()
                        };

                        if let Some((array_base, index_expr, slots, access)) = dynamic_index {
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
                                expr: proc_index_selector_expr(&array_base, &index_expr, access),
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
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_proc_calls_in_stmts_without_hooks(stmts, proc_vars, proc_array_slots, proc_api, errors);
    inject_bound_proc_param_hooks_in_stmts(
        None,
        stmts,
        proc_vars,
        proc_array_slots,
        proc_api,
        errors,
    );
}

/// Processor-array parameters use a structure-of-arrays ABI. Rewrite readable
/// parameter fields to that ABI before the ordinary processor-call rewriter,
/// which otherwise resolves fixed indices to top-level instance slot names.
pub(super) fn rewrite_proc_array_param_field_reads(
    stmts: &mut [Stmt],
    proc_arrays: &HashMap<String, ProcNestedArrayState>,
    proc_api: &HashMap<String, ProcApi>,
) {
    fn rewrite_expr(
        expr: &mut Expr,
        proc_arrays: &HashMap<String, ProcNestedArrayState>,
        proc_api: &HashMap<String, ProcApi>,
    ) {
        match expr {
            Expr::Index { index, .. } => rewrite_expr(index, proc_arrays, proc_api),
            Expr::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => {
                for nested in [selector, channel, start, end].into_iter().flatten() {
                    rewrite_expr(nested, proc_arrays, proc_api);
                }
            }
            Expr::ArrayCtor { spec, init, .. } => {
                rewrite_expr(&mut spec.size, proc_arrays, proc_api);
                if let Some(values) = init {
                    for value in values {
                        rewrite_expr(value, proc_arrays, proc_api);
                    }
                }
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                rewrite_expr(lhs, proc_arrays, proc_api);
                rewrite_expr(rhs, proc_arrays, proc_api);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    rewrite_expr(arg, proc_arrays, proc_api);
                }
            }
            Expr::Cast { expr, .. }
            | Expr::UnaryNot { expr, .. }
            | Expr::UnaryBitNot { expr, .. } => rewrite_expr(expr, proc_arrays, proc_api),
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                for value in values {
                    rewrite_expr(value, proc_arrays, proc_api);
                }
            }
            Expr::UserCall { args, .. } => {
                for arg in &mut *args {
                    rewrite_expr(&mut arg.expr, proc_arrays, proc_api);
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }

        let Expr::UserCall {
            loc, name, args, ..
        } = expr
        else {
            return;
        };
        if name != &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}") {
            return;
        }
        let Some(Expr::Var { name: base, .. }) = named_call_arg_expr(args, PROC_INDEX_BASE_ARG)
        else {
            return;
        };
        let Some(array) = proc_arrays.get(base) else {
            return;
        };
        let Some(Expr::Var { name: field, .. }) =
            named_call_arg_expr(args, PROC_FIELD_SENTINEL_ARG)
        else {
            return;
        };
        let Some(param) = proc_api
            .get(&array.proc_name)
            .and_then(|api| api.params.get(field))
            .filter(|param| !param.private)
        else {
            return;
        };
        let Some(index) = named_call_arg_expr(args, PROC_INDEX_EXPR_ARG).cloned() else {
            return;
        };
        *expr = Expr::Index {
            loc: *loc,
            base: format!("{base}.{}", param.name),
            index: Box::new(index),
        };
    }

    fn rewrite_stmt(
        stmt: &mut Stmt,
        proc_arrays: &HashMap<String, ProcNestedArrayState>,
        proc_api: &HashMap<String, ProcApi>,
    ) {
        match stmt {
            Stmt::Assign { target, expr, .. } => {
                match target {
                    AssignTarget::Index { index, .. } => rewrite_expr(index, proc_arrays, proc_api),
                    AssignTarget::Slice {
                        selector,
                        channel,
                        start,
                        end,
                        ..
                    } => {
                        for nested in [selector, channel, start, end].into_iter().flatten() {
                            rewrite_expr(nested, proc_arrays, proc_api);
                        }
                    }
                    AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
                }
                rewrite_expr(expr, proc_arrays, proc_api);
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                rewrite_expr(expr, proc_arrays, proc_api)
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_expr(cond, proc_arrays, proc_api);
                for nested in then_branch.iter_mut().chain(else_branch) {
                    rewrite_stmt(nested, proc_arrays, proc_api);
                }
            }
            Stmt::For {
                step,
                start,
                end,
                body,
                ..
            } => {
                if let Some(step) = step {
                    rewrite_expr(step, proc_arrays, proc_api);
                }
                rewrite_expr(start, proc_arrays, proc_api);
                rewrite_expr(end, proc_arrays, proc_api);
                for nested in body {
                    rewrite_stmt(nested, proc_arrays, proc_api);
                }
            }
            Stmt::While { cond, body, .. } => {
                rewrite_expr(cond, proc_arrays, proc_api);
                for nested in body {
                    rewrite_stmt(nested, proc_arrays, proc_api);
                }
            }
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }

    for stmt in stmts {
        rewrite_stmt(stmt, proc_arrays, proc_api);
    }
}

pub(super) fn rewrite_proc_calls_in_stmts_without_hooks(
    stmts: &mut Vec<Stmt>,
    proc_vars: &HashMap<String, ProcCallInstance>,
    proc_array_slots: &HashMap<String, Vec<String>>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) {
    lower_named_proc_param_calls_in_stmts(stmts, proc_vars, proc_array_slots, proc_api, errors);
    let mut aliases = ProcArrayAliases::new();
    for stmt in &mut *stmts {
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
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_called_proc_instances_in_expr(coordinate, proc_vars, proc_array_slots, out);
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
                if let Some(alias) = proc_array_alias_from_index_expr(expr, proc_array_slots) {
                    aliases.insert(name.clone(), alias);
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
    fn extract_indexed_receiver(args: &[CallArg]) -> Option<(&str, Expr, IndexAccess)> {
        let mut base = None::<&str>;
        let mut index = None::<Expr>;
        let mut access = IndexAccess::Clamp;
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
                Some(PROC_INDEX_UNCHECKED_ARG) => access = IndexAccess::Unchecked,
                _ => {}
            }
        }
        if (base.is_none() || index.is_none()) && args.len() >= 2 {
            if let Expr::Var { name, .. } = &args[0].expr {
                base = Some(name.as_str());
                index = Some(args[1].expr.clone());
            }
        }
        Some((base?, index?, access))
    }

    match expr {
        Expr::Index { index, .. } => desugar_expr_instance_method_calls(
            index,
            struct_instances,
            struct_array_roots,
            current_ns,
            callable_symbols,
        ),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                desugar_expr_instance_method_calls(
                    coordinate,
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
            if let Some(CallArg {
                name: receiver_name,
                expr: Expr::Index { base, .. },
            }) = args.first()
            {
                if receiver_name.as_deref() == Some(METHOD_RECEIVER_ARG) {
                    if let Some(struct_name) = struct_array_roots.get(base) {
                        let resolved_method = format!("{struct_name}.{name}");
                        if callable_symbols.contains(&resolved_method) {
                            args[0].name = None;
                            *name = resolved_method;
                            return;
                        }
                    }
                }
            }
            if let Some(CallArg {
                name: receiver_name,
                expr: Expr::Var { name: receiver, .. },
            }) = args.first()
            {
                if receiver_name.as_deref() == Some(METHOD_RECEIVER_ARG) {
                    if let Some(struct_name) = struct_instances.get(receiver) {
                        let resolved_method = format!("{struct_name}.{name}");
                        if callable_symbols.contains(&resolved_method) {
                            args[0].name = None;
                            *name = resolved_method;
                            return;
                        }
                    }
                }
            }
            if is_unsafe_index_method_name(name) {
                if let Some(receiver) = args
                    .first_mut()
                    .filter(|arg| arg.name.as_deref() == Some(METHOD_RECEIVER_ARG))
                {
                    receiver.name = None;
                    return;
                }
            }
            if is_builtin_instance_method_name(name) {
                let indexed_receiver = args.first().and_then(|arg| match &arg.expr {
                    Expr::Index { base, index, .. }
                        if arg.name.as_deref() == Some(METHOD_RECEIVER_ARG) =>
                    {
                        Some((base.clone(), index.as_ref().clone()))
                    }
                    _ => None,
                });
                if let Some((base, index)) = indexed_receiver {
                    args.remove(0);
                    args.insert(
                        0,
                        CallArg {
                            name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                            expr: index,
                        },
                    );
                    args.insert(
                        0,
                        CallArg {
                            name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                            expr: Expr::var(base),
                        },
                    );
                    *name = format!("{PROC_INDEX_CALL_SENTINEL}.{name}");
                    return;
                }
            }
            if let Some(receiver) = args
                .first_mut()
                .filter(|arg| arg.name.as_deref() == Some(METHOD_RECEIVER_ARG))
            {
                let resolved_name = if callable_symbols.contains(name) {
                    Some(name.clone())
                } else {
                    resolve_unqualified_symbol_name(name, current_ns, callable_symbols)
                };
                if let Some(resolved_name) = resolved_name {
                    receiver.name = None;
                    *name = resolved_name;
                }
            }
            if let Some(method) = name.strip_prefix(&format!("{PROC_INDEX_CALL_SENTINEL}.")) {
                if let Some((base, index_expr, access)) = extract_indexed_receiver(args) {
                    let base = base.to_owned();
                    if let Some(struct_name) = struct_array_roots.get(base.as_str()) {
                        let resolved_method = format!("{}.{}", struct_name, method);
                        if callable_symbols.contains(&resolved_method) {
                            *name = resolved_method;
                            args.retain(|arg| {
                                !matches!(
                                    arg.name.as_deref(),
                                    Some(PROC_INDEX_BASE_ARG)
                                        | Some(PROC_INDEX_EXPR_ARG)
                                        | Some(PROC_INDEX_UNCHECKED_ARG)
                                )
                            });
                            args.insert(
                                0,
                                CallArg {
                                    name: None,
                                    expr: indexed_read_expr(
                                        base,
                                        index_expr,
                                        access,
                                        Default::default(),
                                    ),
                                },
                            );
                            return;
                        }
                    }
                }
            }
            if callable_symbols.contains(name) {
                return;
            }
            if let Some((base, method)) = split_receiver_method_path(name) {
                if is_unsafe_index_method_name(method) {
                    let receiver = base.to_owned();
                    *name = method.to_owned();
                    args.insert(
                        0,
                        CallArg {
                            name: None,
                            expr: Expr::var(receiver),
                        },
                    );
                    return;
                }
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

pub(crate) fn desugar_init_instance_method_calls(
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
