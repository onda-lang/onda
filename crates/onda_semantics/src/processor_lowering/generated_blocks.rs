use super::*;
use crate::internal_names::runtime_buffer_alias_selector_symbol;
use crate::proc_call_rewrite::lower_named_proc_param_calls_in_stmts;

#[derive(Debug, Clone)]
struct ManagedDynamicProcArray {
    proc_name: String,
    array_base: String,
    raw_slots: Vec<String>,
    slots: Vec<String>,
    active_field: String,
}

#[derive(Debug, Clone)]
enum PersistentBufferAliasSource {
    Direct(String),
    Collection {
        base: String,
        selector_state: String,
    },
}

#[derive(Debug, Clone)]
struct PersistentBufferAlias {
    name: String,
    source: PersistentBufferAliasSource,
}

fn proc_constructor_array_symbols(shape: &ProcLoweringShape) -> HashSet<String> {
    shape
        .field_array_slots
        .keys()
        .chain(shape.in_array_slots.keys())
        .chain(shape.state.data.keys())
        .cloned()
        .collect()
}

fn nested_wrapper_constructor_array_symbols(
    shape: &ProcLoweringShape,
    nested_path: &str,
) -> HashSet<String> {
    shape
        .field_array_slots
        .keys()
        .chain(shape.state.data.keys())
        .map(|name| format!("self.{}", nested_field_name(nested_path, name)))
        .collect()
}

fn persistent_proc_buffer_aliases(
    stmts: &[Stmt],
    buffer_specs: &[ProcBufferSpec],
) -> Vec<PersistentBufferAlias> {
    let direct_buffers = buffer_specs
        .iter()
        .filter(|buffer| !buffer.is_array)
        .map(|buffer| buffer.name.as_str())
        .collect::<HashSet<_>>();
    let buffer_collections = buffer_specs
        .iter()
        .filter(|buffer| buffer.is_array)
        .map(|buffer| buffer.name.as_str())
        .collect::<HashSet<_>>();
    let mut aliases = HashSet::<String>::new();
    let mut captures = Vec::<PersistentBufferAlias>::new();

    for stmt in stmts {
        let Stmt::Assign {
            target: AssignTarget::Var(name),
            expr,
            ..
        } = stmt
        else {
            continue;
        };
        let source = match expr {
            Expr::Var { name: source, .. }
                if direct_buffers.contains(source.as_str()) || aliases.contains(source) =>
            {
                PersistentBufferAliasSource::Direct(source.clone())
            }
            Expr::Index { base, .. } if buffer_collections.contains(base.as_str()) => {
                PersistentBufferAliasSource::Collection {
                    base: base.clone(),
                    selector_state: runtime_buffer_alias_selector_symbol(name),
                }
            }
            _ => continue,
        };
        aliases.insert(name.clone());
        captures.push(PersistentBufferAlias {
            name: name.clone(),
            source,
        });
    }
    captures
}

fn capture_persistent_buffer_alias_selectors(
    stmts: Vec<Stmt>,
    aliases: &[PersistentBufferAlias],
) -> Vec<Stmt> {
    let selector_states = aliases
        .iter()
        .filter_map(|alias| match &alias.source {
            PersistentBufferAliasSource::Collection { selector_state, .. } => {
                Some((alias.name.as_str(), selector_state.as_str()))
            }
            PersistentBufferAliasSource::Direct(_) => None,
        })
        .collect::<HashMap<_, _>>();
    let mut rewritten = Vec::with_capacity(stmts.len() + selector_states.len());

    for mut stmt in stmts {
        if let Stmt::Assign {
            loc,
            target: AssignTarget::Var(name),
            expr: Expr::Index { index, .. },
            ..
        } = &mut stmt
        {
            if let Some(selector_state) = selector_states.get(name.as_str()) {
                let selector = std::mem::replace(index.as_mut(), Expr::int(0));
                rewritten.push(Stmt::Assign {
                    loc: *loc,
                    target_loc: selector.loc().into(),
                    target: AssignTarget::Var((*selector_state).to_owned()),
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    typed_decl_ty_loc: Default::default(),
                    expr: selector,
                });
                **index = Expr::var((*selector_state).to_owned());
            }
        }
        rewritten.push(stmt);
    }
    rewritten
}

fn rebind_persistent_buffer_aliases(aliases: &[PersistentBufferAlias]) -> Vec<Stmt> {
    aliases
        .iter()
        .map(|alias| {
            let expr = match &alias.source {
                PersistentBufferAliasSource::Direct(source) => Expr::var(source.clone()),
                PersistentBufferAliasSource::Collection {
                    base,
                    selector_state,
                } => Expr::Index {
                    loc: Default::default(),
                    base: base.clone(),
                    index: Box::new(Expr::var(selector_state.clone())),
                },
            };
            Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var(alias.name.clone()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr,
            }
        })
        .collect()
}

fn proc_event_array_param_fn_ty(param_ty: ProcEventParamTypeSpec) -> Option<FnParamType> {
    match param_ty {
        ProcEventParamTypeSpec::FixedArray { elem_ty, len } => Some(FnParamType::SizedArray {
            elem: Some(elem_ty),
            generic_name: None,
            size: Expr::int(len as i64),
        }),
        ProcEventParamTypeSpec::Slice { elem_ty } => Some(FnParamType::Array(Some(elem_ty))),
        ProcEventParamTypeSpec::Scalar { .. } => None,
    }
}

fn proc_bind_hook_call_stmt(proc_name: &str, hook: &str, receiver: Expr) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name: proc_local_hidden_def_name(proc_name, hook),
            type_args: Vec::new(),
            args: vec![CallArg {
                name: None,
                expr: receiver,
            }],
        },
    }
}

fn nested_proc_bind_hook_call_stmt(owner_proc: &str, nested_path: &str, hook: &str) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name: proc_local_nested_bind_hidden_def_name(owner_proc, nested_path, hook),
            type_args: Vec::new(),
            args: vec![CallArg {
                name: None,
                expr: Expr::var("self"),
            }],
        },
    }
}

fn after_init_bind_hook_stmts_for_receiver(
    proc_name: &str,
    param_specs: &[ProcParamSpec],
    receiver: Expr,
) -> Vec<Stmt> {
    param_specs
        .iter()
        .filter_map(|param| {
            if param.slots.len() == 1 && param.slots[0].name == param.name {
                param.slots[0]
                    .bind
                    .as_ref()
                    .map(|hook| proc_bind_hook_call_stmt(proc_name, hook, receiver.clone()))
            } else {
                None
            }
        })
        .collect()
}

fn after_init_nested_bind_hook_stmts(
    owner_proc: &str,
    nested_path: &str,
    param_specs: &[ProcParamSpec],
) -> Vec<Stmt> {
    param_specs
        .iter()
        .filter_map(|spec| {
            if spec.slots.len() == 1 && spec.slots[0].name == spec.name {
                spec.slots[0]
                    .bind
                    .as_ref()
                    .map(|hook| nested_proc_bind_hook_call_stmt(owner_proc, nested_path, hook))
            } else {
                None
            }
        })
        .collect()
}

fn after_init_bind_hook_stmts(proc_name: &str, param_specs: &[ProcParamSpec]) -> Vec<Stmt> {
    after_init_bind_hook_stmts_for_receiver(proc_name, param_specs, Expr::var("self"))
}

const PINNED_INIT_BEGIN_MARKER: &str = "__onda_pinned_init_begin";
const PINNED_INIT_END_MARKER: &str = "__onda_pinned_init_end";

fn internal_marker_stmt(name: &str) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name: name.to_owned(),
            type_args: Vec::new(),
            args: Vec::new(),
        },
    }
}

pub(super) fn mark_pinned_initializer_stmt(stmt: Stmt) -> [Stmt; 3] {
    [
        internal_marker_stmt(PINNED_INIT_BEGIN_MARKER),
        stmt,
        internal_marker_stmt(PINNED_INIT_END_MARKER),
    ]
}

fn is_internal_marker(stmt: &Stmt, expected: &str) -> bool {
    matches!(
        stmt,
        Stmt::Expr {
            expr: Expr::UserCall { name, args, .. },
            ..
        } if name == expected && args.is_empty()
    )
}

pub(crate) fn is_pinned_initializer_marker(stmt: &Stmt) -> bool {
    is_internal_marker(stmt, PINNED_INIT_BEGIN_MARKER)
        || is_internal_marker(stmt, PINNED_INIT_END_MARKER)
}

/// Marks the declaration that introduced each pinned root. Later explicit
/// writes remain ordinary init code and therefore still run on re-init.
pub(crate) fn mark_pinned_initializers(init: &InitBlock) -> Vec<Stmt> {
    let mut pending = init.pinned_roots.iter().cloned().collect::<HashSet<_>>();
    let mut marked = Vec::with_capacity(init.body.len() + pending.len() * 2);
    for stmt in &init.body {
        let pinned = matches!(
            stmt,
            Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            } if pending.remove(name)
        );
        if pinned {
            marked.push(internal_marker_stmt(PINNED_INIT_BEGIN_MARKER));
        }
        marked.push(stmt.clone());
        if pinned {
            marked.push(internal_marker_stmt(PINNED_INIT_END_MARKER));
        }
    }
    marked
}

/// Converts marked initializer expansions into a single runtime guard. The
/// markers are inserted before processor/array constructors are expanded, so
/// every generated write belonging to the declaration is covered.
pub(crate) fn guard_pinned_initializers(stmts: &mut Vec<Stmt>, all_name: &str) {
    let source = std::mem::take(stmts);
    let mut source = source.into_iter();
    let mut lowered = Vec::new();
    while let Some(stmt) = source.next() {
        if !is_internal_marker(&stmt, PINNED_INIT_BEGIN_MARKER) {
            debug_assert!(!is_internal_marker(&stmt, PINNED_INIT_END_MARKER));
            lowered.push(stmt);
            continue;
        }

        let mut pinned_init = Vec::new();
        for stmt in source.by_ref() {
            if is_internal_marker(&stmt, PINNED_INIT_END_MARKER) {
                break;
            }
            pinned_init.push(stmt);
        }
        lowered.push(Stmt::If {
            loc: Default::default(),
            cond: Expr::var(all_name),
            then_branch: pinned_init,
            else_branch: Vec::new(),
        });
    }
    *stmts = lowered;
}

fn declaration_only_primitive_array_fill(stmt: &Stmt) -> Option<Stmt> {
    let Stmt::Assign {
        target: AssignTarget::Var(array_var),
        expr: Expr::ArrayCtor {
            spec, init: None, ..
        },
        ..
    } = stmt
    else {
        return None;
    };
    let ArrayElemType::Primitive(element) = spec.elem else {
        return None;
    };
    Some(Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Slice {
            base: array_var.clone(),
            selector: None,
            channel: None,
            start: None,
            end: None,
        },
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr: zero_expr(element),
    })
}

fn build_builtin_proc_init_event_parts<F>(
    receiver_ty: &str,
    param_specs: &[ProcParamSpec],
    mut target_for_slot: F,
    init_fn_name: String,
) -> (Vec<onda_frontend::FnParamDecl>, Vec<Stmt>)
where
    F: FnMut(&str) -> String,
{
    let mut event_params = Vec::<onda_frontend::FnParamDecl>::new();
    event_params.push(onda_frontend::FnParamDecl {
        loc: Default::default(),
        name: "self".to_owned(),
        ty: Some(FnParamType::Struct(receiver_ty.to_owned())),
        ty_loc: Default::default(),
        default: None,
    });

    let mut event_body = Vec::<Stmt>::new();
    for param in param_specs {
        if param.slots.len() == 1 && param.slots[0].name == param.name {
            let slot = &param.slots[0];
            event_params.push(onda_frontend::FnParamDecl {
                loc: Default::default(),
                name: param.name.clone(),
                ty: Some(FnParamType::Primitive(slot.ty)),
                ty_loc: Default::default(),
                default: None,
            });
            let mut value = Expr::var(param.name.clone());
            if let Some(range) = slot.range {
                value = cast_expr_to_primitive(clamp_expr_to_range(value, range), slot.ty);
            }
            event_body.push(Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var(target_for_slot(&slot.name)),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: value,
            });
            continue;
        }

        let elem_ty = param
            .slots
            .first()
            .map(|slot| slot.ty)
            .expect("lowered processor array param must have at least one slot");
        event_params.push(onda_frontend::FnParamDecl {
            loc: Default::default(),
            name: param.name.clone(),
            ty: Some(FnParamType::SizedArray {
                elem: Some(elem_ty),
                generic_name: None,
                size: Expr::int(param.slots.len() as i64),
            }),
            ty_loc: Default::default(),
            default: None,
        });
        for (idx, slot) in param.slots.iter().enumerate() {
            let mut value = Expr::Index {
                loc: Default::default(),
                base: param.name.clone(),
                index: Box::new(Expr::int(idx as i64)),
            };
            if let Some(range) = slot.range {
                value = cast_expr_to_primitive(clamp_expr_to_range(value, range), slot.ty);
            }
            event_body.push(Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var(target_for_slot(&slot.name)),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: value,
            });
        }
    }

    event_params.push(onda_frontend::FnParamDecl {
        loc: Default::default(),
        name: INIT_ALL_PARAM_NAME.to_owned(),
        ty: Some(FnParamType::Primitive(PrimitiveType::Bool)),
        ty_loc: Default::default(),
        default: Some(Expr::bool(false)),
    });

    event_body.push(Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name: init_fn_name,
            type_args: Vec::new(),
            args: vec![
                CallArg {
                    name: None,
                    expr: Expr::var("self"),
                },
                CallArg {
                    name: None,
                    expr: Expr::var(INIT_ALL_PARAM_NAME),
                },
            ],
        },
    });

    (event_params, event_body)
}

fn rewrite_stmt_for_managed_dynamic_proc_block_hooks(
    mut stmt: Stmt,
    managed_arrays: &HashMap<String, ManagedDynamicProcArray>,
    proc_api: &HashMap<String, ProcApi>,
    used_arrays: &mut HashSet<String>,
    temp_counter: &mut usize,
) -> Vec<Stmt> {
    fn temp_name(temp_counter: &mut usize, purpose: &str) -> String {
        let id = *temp_counter;
        *temp_counter += 1;
        format!("__onda_dynamic_proc_{purpose}_{id}")
    }

    fn assign_temp(name: String, expr: Expr, ty: Option<PrimitiveType>) -> Stmt {
        Stmt::Assign {
            loc: Default::default(),
            target_loc: Default::default(),
            target: AssignTarget::Var(name),
            decl_ty: ty,
            generic_decl_ty: None,
            is_typed_decl: ty.is_some(),
            typed_decl_ty_loc: Default::default(),
            expr,
        }
    }

    fn collect_guards_from_expr(
        expr: &mut Expr,
        managed_arrays: &HashMap<String, ManagedDynamicProcArray>,
        proc_api: &HashMap<String, ProcApi>,
        used_arrays: &mut HashSet<String>,
        guards: &mut Vec<Stmt>,
        temp_counter: &mut usize,
    ) {
        match expr {
            Expr::Index { index, .. } => {
                collect_guards_from_expr(
                    index,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    guards,
                    temp_counter,
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
                    collect_guards_from_expr(
                        coordinate,
                        managed_arrays,
                        proc_api,
                        used_arrays,
                        guards,
                        temp_counter,
                    );
                }
            }
            Expr::ArrayCtor { spec, init, .. } => {
                collect_guards_from_expr(
                    &mut spec.size,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    guards,
                    temp_counter,
                );
                if let Some(values) = init {
                    for value in values {
                        collect_guards_from_expr(
                            value,
                            managed_arrays,
                            proc_api,
                            used_arrays,
                            guards,
                            temp_counter,
                        );
                    }
                }
            }
            Expr::Logical { op, lhs, rhs, .. } => {
                collect_guards_from_expr(
                    lhs,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    guards,
                    temp_counter,
                );
                let mut rhs_guards = Vec::new();
                collect_guards_from_expr(
                    rhs,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    &mut rhs_guards,
                    temp_counter,
                );
                if !rhs_guards.is_empty() {
                    // Keep hook execution behind the language's short-circuit
                    // boundary while still producing one expression result.
                    let result = temp_name(temp_counter, "condition");
                    guards.push(assign_temp(
                        result.clone(),
                        lhs.as_ref().clone(),
                        Some(PrimitiveType::Bool),
                    ));
                    let branch_cond = match op {
                        LogicalOp::And => Expr::var(result.clone()),
                        LogicalOp::Or => Expr::UnaryNot {
                            loc: Default::default(),
                            expr: Box::new(Expr::var(result.clone())),
                        },
                    };
                    rhs_guards.push(assign_temp(result.clone(), rhs.as_ref().clone(), None));
                    guards.push(Stmt::If {
                        loc: Default::default(),
                        cond: branch_cond,
                        then_branch: rhs_guards,
                        else_branch: Vec::new(),
                    });
                    *expr = Expr::var(result);
                }
            }
            Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
                collect_guards_from_expr(
                    lhs,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    guards,
                    temp_counter,
                );
                collect_guards_from_expr(
                    rhs,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    guards,
                    temp_counter,
                );
            }
            Expr::Call { args, .. } => {
                for arg in args.iter_mut() {
                    collect_guards_from_expr(
                        arg,
                        managed_arrays,
                        proc_api,
                        used_arrays,
                        guards,
                        temp_counter,
                    );
                }
            }
            Expr::UserCall { name, args, .. } => {
                for arg in args.iter_mut() {
                    collect_guards_from_expr(
                        &mut arg.expr,
                        managed_arrays,
                        proc_api,
                        used_arrays,
                        guards,
                        temp_counter,
                    );
                }
                let proc_name = if let Some(step_proc) = name.strip_suffix(PROC_STEP_FN_SUFFIX) {
                    Some(step_proc)
                } else if let Some((call_proc, out_idx_raw)) =
                    name.rsplit_once(PROC_CALL_OUT_FN_PREFIX)
                {
                    if out_idx_raw.parse::<usize>().is_ok() {
                        Some(call_proc)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let Some(proc_name) = proc_name else {
                    return;
                };
                let Some(api) = proc_api.get(proc_name) else {
                    return;
                };
                if !proc_needs_block_hooks(api) {
                    return;
                }
                let Some(CallArg {
                    expr: Expr::Index { base, index, .. },
                    ..
                }) = args.first_mut()
                else {
                    return;
                };
                if matches!(index.as_ref(), Expr::Int { .. }) {
                    return;
                }
                let array_base = base.clone();
                let Some(managed) = managed_arrays.get(&array_base) else {
                    return;
                };
                if managed.proc_name != proc_name {
                    return;
                }
                used_arrays.insert(array_base.clone());
                // The selector participates in the hook and the original call;
                // cache it so source evaluation still happens exactly once.
                let selector = temp_name(temp_counter, "selector");
                guards.push(assign_temp(selector.clone(), index.as_ref().clone(), None));
                **index = Expr::var(selector.clone());
                let input_slots = api.ins.iter().map(|port| port.slots.len()).sum::<usize>();
                let buffer_start = 1 + input_slots;
                let mut pre_args = Vec::<CallArg>::new();
                pre_args.push(CallArg {
                    name: None,
                    expr: Expr::Index {
                        loc: Default::default(),
                        base: array_base,
                        index: Box::new(Expr::var(selector.clone())),
                    },
                });
                pre_args.extend(args.iter().skip(buffer_start).cloned());
                guards.push(Stmt::If {
                    loc: Default::default(),
                    cond: Expr::UnaryNot {
                        loc: Default::default(),
                        expr: Box::new(Expr::Index {
                            loc: Default::default(),
                            base: format!("self.{}", managed.active_field),
                            index: Box::new(Expr::var(selector.clone())),
                        }),
                    },
                    then_branch: vec![
                        Stmt::Expr {
                            loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: format!("{proc_name}{PROC_BLOCK_PRE_FN_SUFFIX}"),
                                type_args: Vec::new(),
                                args: pre_args,
                            },
                        },
                        Stmt::Assign {
                            loc: Default::default(),
                            target_loc: Default::default(),
                            target: AssignTarget::Index {
                                base: format!("self.{}", managed.active_field),
                                index: Expr::var(selector),
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            typed_decl_ty_loc: Default::default(),
                            expr: Expr::bool(true),
                        },
                    ],
                    else_branch: Vec::new(),
                });
            }
            Expr::Cast { expr: inner, .. }
            | Expr::UnaryNot { expr: inner, .. }
            | Expr::UnaryBitNot { expr: inner, .. } => {
                collect_guards_from_expr(
                    inner,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    guards,
                    temp_counter,
                );
            }
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                for value in values {
                    collect_guards_from_expr(
                        value,
                        managed_arrays,
                        proc_api,
                        used_arrays,
                        guards,
                        temp_counter,
                    );
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }
    }

    let mut guards = Vec::<Stmt>::new();
    match &mut stmt {
        Stmt::Expr { expr, .. } | Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } => {
            collect_guards_from_expr(
                expr,
                managed_arrays,
                proc_api,
                used_arrays,
                &mut guards,
                temp_counter,
            );
        }
        _ => {}
    }
    if !guards.is_empty() {
        let mut rewritten = guards;
        rewritten.push(stmt);
        return rewritten;
    }

    match stmt {
        Stmt::Const { .. } => Vec::new(),
        Stmt::If {
            loc,
            mut cond,
            then_branch,
            else_branch,
        } => {
            let mut cond_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &mut cond,
                managed_arrays,
                proc_api,
                used_arrays,
                &mut cond_guards,
                temp_counter,
            );
            let mut new_then = Vec::<Stmt>::new();
            for nested in then_branch {
                new_then.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    nested,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    temp_counter,
                ));
            }
            let mut new_else = Vec::<Stmt>::new();
            for nested in else_branch {
                new_else.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    nested,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    temp_counter,
                ));
            }
            cond_guards.push(Stmt::If {
                loc,
                cond,
                then_branch: new_then,
                else_branch: new_else,
            });
            cond_guards
        }
        Stmt::For {
            loc,
            var,
            var_ty,
            mut step,
            mut start,
            mut end,
            end_inclusive,
            body,
        } => {
            let mut range_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &mut start,
                managed_arrays,
                proc_api,
                used_arrays,
                &mut range_guards,
                temp_counter,
            );
            collect_guards_from_expr(
                &mut end,
                managed_arrays,
                proc_api,
                used_arrays,
                &mut range_guards,
                temp_counter,
            );
            if let Some(step_expr) = &mut step {
                collect_guards_from_expr(
                    step_expr,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    &mut range_guards,
                    temp_counter,
                );
            }
            let mut new_body = Vec::<Stmt>::new();
            for nested in body {
                new_body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    nested,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    temp_counter,
                ));
            }
            range_guards.push(Stmt::For {
                loc,
                var,
                var_ty,
                step,
                start,
                end,
                end_inclusive,
                body: new_body,
            });
            range_guards
        }
        Stmt::While {
            loc,
            mut cond,
            body,
        } => {
            let mut cond_guards = Vec::<Stmt>::new();
            collect_guards_from_expr(
                &mut cond,
                managed_arrays,
                proc_api,
                used_arrays,
                &mut cond_guards,
                temp_counter,
            );
            let mut new_body = Vec::<Stmt>::new();
            for nested in body {
                new_body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    nested,
                    managed_arrays,
                    proc_api,
                    used_arrays,
                    temp_counter,
                ));
            }
            if cond_guards.is_empty() {
                vec![Stmt::While {
                    loc,
                    cond,
                    body: new_body,
                }]
            } else {
                // Condition hooks belong to each evaluation, including the one
                // that terminates the loop.
                cond_guards.push(Stmt::If {
                    loc: Default::default(),
                    cond: Expr::UnaryNot {
                        loc: Default::default(),
                        expr: Box::new(cond),
                    },
                    then_branch: vec![Stmt::Break {
                        loc: Default::default(),
                    }],
                    else_branch: Vec::new(),
                });
                cond_guards.extend(new_body);
                vec![Stmt::While {
                    loc,
                    cond: Expr::bool(true),
                    body: cond_guards,
                }]
            }
        }
        _ => vec![stmt],
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_nested_wrapper_defs(
    proc: &ProcessorDef,
    shape: &ProcLoweringShape,
    proc_defs_by_name: &HashMap<String, ProcessorDef>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    struct_defs_by_name: &HashMap<String, StructDef>,
    proc_api: &HashMap<String, ProcApi>,
    nested_instances: &HashMap<String, ProcCallInstance>,
    ins_names: &HashSet<String>,
    managed_active_fields: &mut HashMap<String, usize>,
    def_sample_oversample_factors: &mut HashMap<String, usize>,
    proc_step_oversample_meta: &mut HashMap<String, ProcStepOversampleMeta>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Block> {
    let mut nested_defs = Vec::<Block>::new();
    let mut nested_paths = Vec::<(String, String)>::new();
    collect_nested_proc_instances(shape, None, lowering_shapes, &mut nested_paths);
    for (nested_path, callee_proc_name) in nested_paths {
        let Some(callee_proc) = proc_defs_by_name.get(&callee_proc_name) else {
            continue;
        };
        let Some(callee_shape) = lowering_shapes.get(&callee_proc_name).cloned() else {
            continue;
        };
        let callee_api = proc_api.get(&callee_proc_name);
        let callee_sample_oversample_factor = callee_api
            .map(|api| api.sample_oversample_factor)
            .unwrap_or(1)
            .max(1);
        let proc_symbols = proc_api.keys().cloned().collect::<HashSet<_>>();
        let proc_ns = namespace_of_symbol(&callee_proc_name);
        let mut callee_ins_names = callee_shape.ins.iter().cloned().collect::<HashSet<_>>();
        for port in &callee_shape.in_ports {
            callee_ins_names.insert(port.name.clone());
        }
        for buffer in &callee_shape.buffer_specs {
            callee_ins_names.insert(buffer.name.clone());
        }
        let mut callee_nested_instances = callee_shape
            .state
            .nested_procs
            .iter()
            .map(|(name, state)| {
                (
                    name.clone(),
                    ProcCallInstance {
                        proc_name: state.proc_name.clone(),
                        buffer_args: Vec::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let mut nested_hook_instances = nested_instances.clone();
        nested_hook_instances.insert(
            nested_path.clone(),
            ProcCallInstance {
                proc_name: callee_proc_name.clone(),
                buffer_args: Vec::new(),
            },
        );
        for (name, state) in &callee_shape.state.nested_procs {
            nested_hook_instances.insert(
                nested_field_name(&nested_path, name),
                ProcCallInstance {
                    proc_name: state.proc_name.clone(),
                    buffer_args: Vec::new(),
                },
            );
        }

        let mut nested_init_body = Vec::<Stmt>::new();
        let mut constructor_setup_indices = HashSet::<usize>::new();
        let constructor_array_symbols =
            nested_wrapper_constructor_array_symbols(&callee_shape, &nested_path);
        let mut callee_init_stmts = mark_pinned_initializers(&callee_proc.init);
        lower_named_proc_param_calls_in_stmts(
            &mut callee_init_stmts,
            &callee_nested_instances,
            &callee_shape.nested_proc_array_slots,
            proc_api,
            errors,
        );
        for stmt in &callee_init_stmts {
            if let Stmt::Assign {
                target: AssignTarget::Var(array_var),
                expr: expr @ Expr::ArrayCtor { init, .. },
                ..
            } = stmt
            {
                if let Some(slot_names) = callee_shape.nested_proc_array_slots.get(array_var) {
                    let Some(array_state) = callee_shape.state.nested_proc_arrays.get(array_var)
                    else {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' processor-array '{}' is missing state metadata",
                                callee_proc_name, array_var
                            ),
                        );
                        continue;
                    };

                    if let Some(values) = init {
                        if values.len() != slot_names.len() && values.len() != 1 {
                            with_expr_diag_context(expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "processor '{}.{}' initializer expects {} constructor entries (or a single broadcast constructor), got {}",
                                        callee_proc_name,
                                        array_var,
                                        slot_names.len(),
                                        values.len()
                                    ),
                                );
                            });
                        }
                    }

                    for (slot_idx, slot_name) in slot_names.iter().enumerate() {
                        let mut slot_ctor_args = Vec::<CallArg>::new();
                        let mut proc_array_slot = None;
                        if let Some(values) = init {
                            let value = if values.len() == 1 {
                                proc_array_slot = Some((slot_idx, slot_names.len()));
                                values.first()
                            } else {
                                values.get(slot_idx)
                            };
                            if let Some(value) = value {
                                if let Expr::UserCall {
                                    name: ctor_name,
                                    type_args,
                                    args,
                                    ..
                                } = value
                                {
                                    let resolved_ctor = if ctor_name.contains("::") {
                                        if proc_symbols.contains(ctor_name) {
                                            Some(ctor_name.clone())
                                        } else {
                                            None
                                        }
                                    } else {
                                        resolve_unqualified_symbol_name(
                                            ctor_name,
                                            &proc_ns,
                                            &proc_symbols,
                                        )
                                    };
                                    if let Some(resolved_ctor) = resolved_ctor {
                                        if resolved_ctor != array_state.proc_name {
                                            with_expr_diag_context(value, |expr_diag| {
                                                push_semantic(
                                                    expr_diag,
                                                    errors,
                                                    format!(
                                                        "processor '{}.{}' initializer entry {} uses constructor '{}' but '{}' is required",
                                                        callee_proc_name,
                                                        array_var,
                                                        slot_idx,
                                                        resolved_ctor,
                                                        array_state.proc_name
                                                    ),
                                                );
                                            });
                                        }
                                    } else {
                                        with_expr_diag_context(value, |expr_diag| {
                                            push_semantic(
                                                expr_diag,
                                                errors,
                                                format!(
                                                    "processor '{}.{}' initializer entry {} references unknown processor constructor '{}'",
                                                    callee_proc_name, array_var, slot_idx, ctor_name
                                                ),
                                            );
                                        });
                                    }
                                    if !type_args.is_empty() {
                                        with_expr_diag_context(value, |expr_diag| {
                                            push_semantic(
                                                expr_diag,
                                                errors,
                                                format!(
                                                    "processor '{}' is not generic and cannot take type arguments",
                                                    ctor_name
                                                ),
                                            );
                                        });
                                    }
                                    slot_ctor_args = args.clone();
                                } else {
                                    with_expr_diag_context(value, |expr_diag| {
                                        push_semantic(
                                            expr_diag,
                                            errors,
                                            format!(
                                                "processor '{}.{}' initializer entry {} must be a processor constructor call",
                                                callee_proc_name, array_var, slot_idx
                                            ),
                                        );
                                    });
                                }
                            }
                        }

                        let Some(slot_state) = callee_shape.state.nested_procs.get(slot_name)
                        else {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' processor-array slot '{}' is missing nested processor state",
                                    callee_proc_name, slot_name
                                ),
                            );
                            continue;
                        };
                        let Some(nested_callee_shape) = lowering_shapes.get(&slot_state.proc_name)
                        else {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' nested state '{}' references unknown processor '{}'",
                                    callee_proc_name, slot_name, slot_state.proc_name
                                ),
                            );
                            continue;
                        };
                        let lowered_slot_ctor_args = slot_ctor_args
                            .iter()
                            .map(|arg| CallArg {
                                name: arg.name.clone(),
                                expr: lower_callee_expr_for_nested_wrapper(
                                    &arg.expr,
                                    &proc.name,
                                    &nested_path,
                                    &callee_shape,
                                    &callee_nested_instances,
                                    &callee_shape.field_array_slots,
                                    &callee_shape.in_array_slots,
                                    &callee_shape.nested_proc_array_slots,
                                    proc_api,
                                    errors,
                                ),
                            })
                            .collect::<Vec<_>>();
                        let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                            &proc.name,
                            &nested_field_name(&nested_path, slot_name),
                            &slot_state.proc_name,
                            &lowered_slot_ctor_args,
                            &nested_callee_shape.param_specs,
                            &nested_callee_shape.buffer_specs,
                            proc_array_slot,
                            &constructor_array_symbols,
                            errors,
                        );
                        if let Some(instance) = callee_nested_instances.get_mut(slot_name) {
                            instance.buffer_args = bound_buffers;
                        }
                        for expanded_stmt in expanded {
                            if let Some(rewritten) = rewrite_owner_proc_stmt(
                                expanded_stmt,
                                &proc.name,
                                &shape.field_names,
                                &shape.array_field_names,
                                ins_names,
                                &shape.field_array_slots,
                                &shape.in_array_slots,
                                &shape.nested_proc_array_slots,
                                &shape.nested_fields,
                                nested_instances,
                                proc_api,
                                errors,
                            ) {
                                constructor_setup_indices.insert(nested_init_body.len());
                                nested_init_body.push(rewritten);
                            }
                        }
                    }
                    continue;
                }
            }
            if let Some(fill_stmt) = declaration_only_primitive_array_fill(stmt) {
                if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                    &fill_stmt,
                    &proc.name,
                    &callee_proc_name,
                    &nested_path,
                    &callee_shape,
                    &callee_nested_instances,
                    &callee_ins_names,
                    &callee_shape.field_array_slots,
                    &callee_shape.in_array_slots,
                    &callee_shape.nested_proc_array_slots,
                    proc_api,
                    errors,
                ) {
                    nested_init_body.push(rewritten);
                }
                continue;
            }
            if let Stmt::Assign {
                target: AssignTarget::Var(var),
                expr:
                    Expr::UserCall {
                        name: ctor_name,
                        type_args,
                        args,
                        ..
                    },
                ..
            } = stmt
            {
                if let Some(nested_state) = callee_shape.state.nested_procs.get(var) {
                    if !type_args.is_empty() {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' is not generic and cannot take type arguments",
                                ctor_name
                            ),
                        );
                    }
                    if nested_state.proc_name != *ctor_name {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor state symbol '{}' has conflicting processor types '{}' and '{}'",
                                var, nested_state.proc_name, ctor_name
                            ),
                        );
                        continue;
                    }
                    let Some(nested_callee_shape) = lowering_shapes.get(&nested_state.proc_name)
                    else {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' nested state '{}' references unknown processor '{}'",
                                callee_proc_name, var, nested_state.proc_name
                            ),
                        );
                        continue;
                    };
                    let lowered_ctor_args = args
                        .iter()
                        .map(|arg| CallArg {
                            name: arg.name.clone(),
                            expr: lower_callee_expr_for_nested_wrapper(
                                &arg.expr,
                                &proc.name,
                                &nested_path,
                                &callee_shape,
                                &callee_nested_instances,
                                &callee_shape.field_array_slots,
                                &callee_shape.in_array_slots,
                                &callee_shape.nested_proc_array_slots,
                                proc_api,
                                errors,
                            ),
                        })
                        .collect::<Vec<_>>();
                    let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                        &proc.name,
                        &nested_field_name(&nested_path, var),
                        ctor_name,
                        &lowered_ctor_args,
                        &nested_callee_shape.param_specs,
                        &nested_callee_shape.buffer_specs,
                        None,
                        &constructor_array_symbols,
                        errors,
                    );
                    if let Some(instance) = callee_nested_instances.get_mut(var) {
                        instance.buffer_args = bound_buffers;
                    }
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
                            &shape.nested_fields,
                            nested_instances,
                            proc_api,
                            errors,
                        ) {
                            constructor_setup_indices.insert(nested_init_body.len());
                            nested_init_body.push(rewritten);
                        }
                    }
                    continue;
                }
                if let Some(state_struct) = callee_shape.state.struct_instances.get(var) {
                    if state_struct.struct_name != *ctor_name {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor state symbol '{}' has conflicting struct types '{}' and '{}'",
                                var, state_struct.struct_name, ctor_name
                            ),
                        );
                        continue;
                    }
                    let Some(struct_def) = resolve_proc_state_struct_def(
                        &callee_proc_name,
                        var,
                        state_struct,
                        struct_defs_by_name,
                        errors,
                    ) else {
                        continue;
                    };
                    if !type_args.is_empty() {
                        let Some(resolved_type_args) = resolve_explicit_call_type_args(
                            type_args,
                            &format!("processor state constructor '{} = {}(...)'", var, ctor_name),
                            DiagCtx::new(stmt.loc()),
                            errors,
                        ) else {
                            continue;
                        };
                        if resolved_type_args.as_slice() != state_struct.type_args.as_slice() {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor state constructor '{} = {}(...)' uses type arguments inconsistent with the declared state specialization",
                                    var, ctor_name
                                ),
                            );
                        }
                    }
                    let expanded = expand_nested_struct_ctor_assign(
                        &nested_field_name(&nested_path, var),
                        ctor_name,
                        &args
                            .iter()
                            .map(|arg| CallArg {
                                name: arg.name.clone(),
                                expr: lower_callee_expr_for_nested_wrapper(
                                    &arg.expr,
                                    &proc.name,
                                    &nested_path,
                                    &callee_shape,
                                    &callee_nested_instances,
                                    &callee_shape.field_array_slots,
                                    &callee_shape.in_array_slots,
                                    &callee_shape.nested_proc_array_slots,
                                    proc_api,
                                    errors,
                                ),
                            })
                            .collect::<Vec<_>>(),
                        &struct_def,
                        struct_defs_by_name,
                        errors,
                    );
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
                            &shape.nested_fields,
                            nested_instances,
                            proc_api,
                            errors,
                        ) {
                            nested_init_body.push(rewritten);
                        }
                    }
                    continue;
                }
            }
            if let Some(rewritten) = lower_callee_stmt_for_nested_wrapper(
                stmt,
                &proc.name,
                &callee_proc_name,
                &nested_path,
                &callee_shape,
                &callee_nested_instances,
                &callee_ins_names,
                &callee_shape.field_array_slots,
                &callee_shape.in_array_slots,
                &callee_shape.nested_proc_array_slots,
                proc_api,
                errors,
            ) {
                nested_init_body.push(rewritten);
            }
        }
        inject_bound_proc_param_hooks_in_stmts_skipping_top_level(
            Some(&proc.name),
            &mut nested_init_body,
            &nested_hook_instances,
            &shape.nested_proc_array_slots,
            proc_api,
            errors,
            &constructor_setup_indices,
        );
        guard_pinned_initializers(&mut nested_init_body, INIT_ALL_PARAM_NAME);
        nested_init_body.extend(after_init_nested_bind_hook_stmts(
            &proc.name,
            &nested_path,
            &callee_shape.param_specs,
        ));

        def_sample_oversample_factors.insert(
            nested_init_fn_name(&proc.name, &nested_path),
            callee_sample_oversample_factor,
        );
        nested_defs.push(Block::Def(FunctionDef {
            loc: Default::default(),
            is_const: false,
            type_params: Vec::new(),
            name: nested_init_fn_name(&proc.name, &nested_path),
            params: vec![
                onda_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: "self".to_owned(),
                    ty: Some(FnParamType::Struct(proc.name.clone())),
                    ty_loc: Default::default(),
                    default: None,
                },
                onda_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: INIT_ALL_PARAM_NAME.to_owned(),
                    ty: Some(FnParamType::Primitive(PrimitiveType::Bool)),
                    ty_loc: Default::default(),
                    default: None,
                },
            ],
            return_ty: None,
            return_ty_loc: Default::default(),
            body: nested_init_body,
        }));

        let nested_managed_dynamic_arrays = callee_shape
            .nested_proc_array_slots
            .iter()
            .filter_map(|(array_base, slots)| {
                let first_slot = slots.first()?;
                let instance = callee_nested_instances.get(first_slot)?;
                let api = proc_api.get(&instance.proc_name)?;
                if !proc_needs_block_hooks(api) {
                    return None;
                }
                let prefixed_base = nested_field_name(&nested_path, array_base);
                let prefixed_slots = slots
                    .iter()
                    .map(|slot| nested_field_name(&nested_path, slot))
                    .collect::<Vec<_>>();
                let active_field = callee_shape
                    .nested_proc_array_active_fields
                    .get(array_base)
                    .map(|field| nested_field_name(&nested_path, field))?;
                Some((
                    prefixed_base.clone(),
                    ManagedDynamicProcArray {
                        proc_name: instance.proc_name.clone(),
                        array_base: prefixed_base.clone(),
                        raw_slots: slots.clone(),
                        slots: prefixed_slots,
                        active_field,
                    },
                ))
            })
            .collect::<HashMap<_, _>>();
        let mut used_nested_managed_dynamic_arrays = HashSet::<String>::new();
        let mut dynamic_hook_temp_counter = 0;
        let mut nested_step_body = Vec::<Stmt>::new();
        for local_def in unique_proc_local_defs(callee_proc) {
            let mut body = Vec::<Stmt>::new();
            for rewritten in lower_callee_stmts_for_nested_wrapper(
                local_def.body.clone(),
                &proc.name,
                &callee_proc_name,
                &nested_path,
                &callee_shape,
                &callee_nested_instances,
                &callee_ins_names,
                &callee_shape.field_array_slots,
                &callee_shape.in_array_slots,
                &callee_shape.nested_proc_array_slots,
                proc_api,
                errors,
            ) {
                body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    rewritten,
                    &nested_managed_dynamic_arrays,
                    proc_api,
                    &mut used_nested_managed_dynamic_arrays,
                    &mut dynamic_hook_temp_counter,
                ));
            }
            inject_bound_proc_param_hooks_in_stmts(
                Some(&proc.name),
                &mut body,
                &nested_hook_instances,
                &shape.nested_proc_array_slots,
                proc_api,
                errors,
            );
            nested_defs.push(Block::Def(nested_wrapper_proc_local_hidden_def(
                &proc.name,
                &nested_path,
                &local_def,
                body,
            )));
        }
        let nested_step_source = if callee_proc.outs_timing == OutputTiming::Block {
            callee_proc.block_pre.clone()
        } else {
            callee_proc.sample.clone()
        };
        for rewritten in lower_callee_stmts_for_nested_wrapper(
            nested_step_source,
            &proc.name,
            &callee_proc_name,
            &nested_path,
            &callee_shape,
            &callee_nested_instances,
            &callee_ins_names,
            &callee_shape.field_array_slots,
            &callee_shape.in_array_slots,
            &callee_shape.nested_proc_array_slots,
            proc_api,
            errors,
        ) {
            nested_step_body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                rewritten,
                &nested_managed_dynamic_arrays,
                proc_api,
                &mut used_nested_managed_dynamic_arrays,
                &mut dynamic_hook_temp_counter,
            ));
        }
        inject_bound_proc_param_hooks_in_stmts(
            Some(&proc.name),
            &mut nested_step_body,
            &nested_hook_instances,
            &shape.nested_proc_array_slots,
            proc_api,
            errors,
        );
        let mut nested_step_params = Vec::<onda_frontend::FnParamDecl>::new();
        nested_step_params.push(onda_frontend::FnParamDecl {
            loc: Default::default(),
            name: "self".to_owned(),
            ty: Some(FnParamType::Struct(proc.name.clone())),
            ty_loc: Default::default(),
            default: None,
        });
        for in_name in &callee_shape.ins {
            nested_step_params.push(onda_frontend::FnParamDecl {
                loc: Default::default(),
                name: in_name.clone(),
                ty: None,
                ty_loc: Default::default(),
                default: None,
            });
        }
        for buffer in &callee_shape.buffer_specs {
            nested_step_params.push(onda_frontend::FnParamDecl {
                loc: Default::default(),
                name: buffer.name.clone(),
                ty: Some(proc_buffer_fn_param_type(buffer)),
                ty_loc: Default::default(),
                default: None,
            });
        }
        let nested_step_name = nested_step_fn_name(&proc.name, &nested_path);
        if callee_sample_oversample_factor > 1 {
            let stage_count = proc_os_sinc_stage_count(callee_sample_oversample_factor);
            let mut input_state_fields = HashMap::<String, ProcInputOversampleStateFields>::new();
            for in_name in &callee_shape.ins {
                let in_ty = *callee_shape
                    .in_types
                    .get(in_name)
                    .unwrap_or(&PrimitiveType::F32);
                let up_stages = if matches!(in_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    (0..stage_count)
                        .map(|stage| ProcSincStageStateFields {
                            a0: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "a0"),
                            ),
                            a1: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "a1"),
                            ),
                            a2: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "a2"),
                            ),
                            a3: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "a3"),
                            ),
                            b0: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "b0"),
                            ),
                            b1: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "b1"),
                            ),
                            b2: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "b2"),
                            ),
                            b3: nested_field_name(
                                &nested_path,
                                &proc_os_up_stage_tap_field_name(in_name, stage, "b3"),
                            ),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                input_state_fields.insert(
                    in_name.clone(),
                    ProcInputOversampleStateFields { up_stages },
                );
            }
            let mut output_state_fields = HashMap::<String, ProcOutputOversampleStateFields>::new();
            for out_name in &callee_shape.outs {
                let out_ty = *callee_shape
                    .out_types
                    .get(out_name)
                    .unwrap_or(&PrimitiveType::F32);
                let output_field = nested_field_name(&nested_path, out_name);
                let down_stages = if matches!(out_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    (0..stage_count)
                        .map(|stage| ProcSincStageStateFields {
                            a0: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "a0"),
                            ),
                            a1: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "a1"),
                            ),
                            a2: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "a2"),
                            ),
                            a3: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "a3"),
                            ),
                            b0: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "b0"),
                            ),
                            b1: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "b1"),
                            ),
                            b2: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "b2"),
                            ),
                            b3: nested_field_name(
                                &nested_path,
                                &proc_os_down_stage_tap_field_name(out_name, stage, "b3"),
                            ),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                output_state_fields.insert(
                    output_field,
                    ProcOutputOversampleStateFields { down_stages },
                );
            }
            proc_step_oversample_meta.insert(
                nested_step_name.clone(),
                ProcStepOversampleMeta {
                    input_state_fields,
                    output_state_fields,
                },
            );
        }
        def_sample_oversample_factors.insert(
            nested_step_name.clone(),
            callee_sample_oversample_factor.max(1),
        );
        nested_defs.push(Block::Def(FunctionDef {
            loc: Default::default(),
            is_const: false,
            type_params: Vec::new(),
            name: nested_step_name.clone(),
            params: nested_step_params.clone(),
            return_ty: None,
            return_ty_loc: Default::default(),
            body: nested_step_body,
        }));

        if let Some(callee_api) = callee_api {
            for event in &callee_proc.events {
                let Some(event_spec) = callee_api.events.get(&event.name) else {
                    push_semantic(
                        DiagCtx::new(event.loc),
                        errors,
                        format!(
                            "processor '{}' nested event '{}' is missing lowered metadata",
                            callee_proc_name, event.name
                        ),
                    );
                    continue;
                };
                let (nested_event_params, nested_event_body) =
                    if is_builtin_proc_init_event_name(&event.name) {
                        build_builtin_proc_init_event_parts(
                            &proc.name,
                            &callee_shape.param_specs,
                            |slot| nested_field_name(&nested_path, slot),
                            nested_init_fn_name(&proc.name, &nested_path),
                        )
                    } else {
                        let mut nested_event_params = Vec::<onda_frontend::FnParamDecl>::new();
                        nested_event_params.push(onda_frontend::FnParamDecl {
                            loc: Default::default(),
                            name: "self".to_owned(),
                            ty: Some(FnParamType::Struct(proc.name.clone())),
                            ty_loc: Default::default(),
                            default: None,
                        });
                        let mut callee_event_ins_names = callee_ins_names.clone();
                        let mut callee_event_in_array_slots = HashMap::<String, Vec<String>>::new();
                        for param in &event_spec.params {
                            callee_event_ins_names.insert(param.name.clone());
                            if let Some(param_ty) = proc_event_array_param_fn_ty(param.ty) {
                                nested_event_params.push(onda_frontend::FnParamDecl {
                                    loc: Default::default(),
                                    name: param.name.clone(),
                                    ty: Some(param_ty),
                                    ty_loc: Default::default(),
                                    default: None,
                                });
                                continue;
                            }
                            let mut slot_names = Vec::<String>::new();
                            for slot in &param.slots {
                                slot_names.push(slot.name.clone());
                                callee_event_ins_names.insert(slot.name.clone());
                                nested_event_params.push(onda_frontend::FnParamDecl {
                                    loc: Default::default(),
                                    name: slot.name.clone(),
                                    ty: Some(FnParamType::Primitive(slot.ty)),
                                    ty_loc: Default::default(),
                                    default: None,
                                });
                            }
                            if slot_names.len() > 1 {
                                callee_event_in_array_slots.insert(param.name.clone(), slot_names);
                            }
                        }
                        let mut nested_event_body = lower_callee_stmts_for_nested_wrapper(
                            event.body.clone(),
                            &proc.name,
                            &callee_proc_name,
                            &nested_path,
                            &callee_shape,
                            &callee_nested_instances,
                            &callee_event_ins_names,
                            &callee_shape.field_array_slots,
                            &callee_event_in_array_slots,
                            &callee_shape.nested_proc_array_slots,
                            proc_api,
                            errors,
                        );
                        inject_bound_proc_param_hooks_in_nested_event_stmts(
                            &proc.name,
                            &nested_path,
                            &mut nested_event_body,
                            &nested_hook_instances,
                            &shape.nested_proc_array_slots,
                            proc_api,
                            errors,
                        );
                        (nested_event_params, nested_event_body)
                    };
                let nested_event_name = nested_event_fn_name(&proc.name, &nested_path, &event.name);
                def_sample_oversample_factors
                    .insert(nested_event_name.clone(), callee_sample_oversample_factor);
                nested_defs.push(Block::Def(FunctionDef {
                    loc: Default::default(),
                    is_const: false,
                    type_params: Vec::new(),
                    name: nested_event_name,
                    params: nested_event_params,
                    return_ty: None,
                    return_ty_loc: Default::default(),
                    body: nested_event_body,
                }));
            }
        }

        let callee_has_effective_block = proc_api
            .get(&callee_proc_name)
            .map(proc_needs_block_hooks)
            .unwrap_or(
                callee_proc.has_block_block && callee_proc.outs_timing == OutputTiming::Sample,
            );
        if callee_has_effective_block {
            let mut nested_block_params = Vec::<onda_frontend::FnParamDecl>::new();
            nested_block_params.push(onda_frontend::FnParamDecl {
                loc: Default::default(),
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                ty_loc: Default::default(),
                default: None,
            });
            for buffer in &callee_shape.buffer_specs {
                nested_block_params.push(onda_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: buffer.name.clone(),
                    ty: Some(proc_buffer_fn_param_type(buffer)),
                    ty_loc: Default::default(),
                    default: None,
                });
            }
            let mut nested_block_pre_body = Vec::<Stmt>::new();
            nested_block_pre_body.extend(lower_callee_stmts_for_nested_wrapper(
                callee_proc.block_pre.clone(),
                &proc.name,
                &callee_proc_name,
                &nested_path,
                &callee_shape,
                &callee_nested_instances,
                &callee_ins_names,
                &callee_shape.field_array_slots,
                &callee_shape.in_array_slots,
                &callee_shape.nested_proc_array_slots,
                proc_api,
                errors,
            ));
            let mut called_callee_nested = collect_called_proc_instances_in_stmts(
                &callee_proc.sample,
                &callee_nested_instances,
                &callee_shape.nested_proc_array_slots,
            );
            for array_base in &used_nested_managed_dynamic_arrays {
                if let Some(managed) = nested_managed_dynamic_arrays.get(array_base) {
                    for slot in &managed.raw_slots {
                        called_callee_nested.remove(slot);
                    }
                }
            }
            let mut callee_nested_vars =
                callee_nested_instances.keys().cloned().collect::<Vec<_>>();
            callee_nested_vars.sort();
            let mut nested_managed_arrays =
                nested_managed_dynamic_arrays.values().collect::<Vec<_>>();
            nested_managed_arrays.sort_by(|a, b| a.active_field.cmp(&b.active_field));
            for managed in &nested_managed_arrays {
                managed_active_fields
                    .entry(managed.active_field.clone())
                    .or_insert(managed.slots.len());
                for slot_idx in 0..managed.slots.len() {
                    nested_block_pre_body.push(Stmt::Assign {
                        loc: Default::default(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: format!("self.{}", managed.active_field),
                            index: Expr::int(slot_idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        typed_decl_ty_loc: Default::default(),
                        expr: Expr::bool(false),
                    });
                }
            }
            for nested_var in &callee_nested_vars {
                if !called_callee_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = callee_nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !proc_needs_block_hooks(api) {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::var("self"),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                nested_block_pre_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
                        name: nested_block_pre_fn_name(
                            &proc.name,
                            &nested_field_name(&nested_path, nested_var),
                        ),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            inject_bound_proc_param_hooks_in_stmts(
                Some(&proc.name),
                &mut nested_block_pre_body,
                &nested_hook_instances,
                &shape.nested_proc_array_slots,
                proc_api,
                errors,
            );
            let nested_block_pre_name = nested_block_pre_fn_name(&proc.name, &nested_path);
            def_sample_oversample_factors.insert(
                nested_block_pre_name.clone(),
                callee_sample_oversample_factor,
            );
            nested_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                is_const: false,
                type_params: Vec::new(),
                name: nested_block_pre_name,
                params: nested_block_params.clone(),
                return_ty: None,
                return_ty_loc: Default::default(),
                body: nested_block_pre_body,
            }));

            let mut nested_block_post_body = Vec::<Stmt>::new();
            for nested_var in &callee_nested_vars {
                if !called_callee_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = callee_nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !proc_needs_block_hooks(api) {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::var("self"),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                nested_block_post_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
                        name: nested_block_post_fn_name(
                            &proc.name,
                            &nested_field_name(&nested_path, nested_var),
                        ),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            nested_block_post_body.extend(lower_callee_stmts_for_nested_wrapper(
                callee_proc.block_post.clone(),
                &proc.name,
                &callee_proc_name,
                &nested_path,
                &callee_shape,
                &callee_nested_instances,
                &callee_ins_names,
                &callee_shape.field_array_slots,
                &callee_shape.in_array_slots,
                &callee_shape.nested_proc_array_slots,
                proc_api,
                errors,
            ));
            for managed in &nested_managed_arrays {
                for (slot_idx, slot_name) in managed.slots.iter().enumerate() {
                    let raw_slot_name = managed.raw_slots.get(slot_idx);
                    let Some(raw_slot_name) = raw_slot_name else {
                        continue;
                    };
                    let Some(instance) = callee_nested_instances.get(raw_slot_name) else {
                        continue;
                    };
                    let Some(api) = proc_api.get(&instance.proc_name) else {
                        continue;
                    };
                    let mut call_args = vec![CallArg {
                        name: None,
                        expr: Expr::Index {
                            loc: Default::default(),
                            base: managed.array_base.clone(),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                    }];
                    call_args.extend(expand_proc_buffer_call_args(
                        instance,
                        api,
                        raw_slot_name,
                        errors,
                    ));
                    nested_block_post_body.push(Stmt::If {
                        loc: Default::default(),
                        cond: Expr::Index {
                            loc: Default::default(),
                            base: format!("self.{}", managed.active_field),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                        then_branch: vec![Stmt::Expr {
                            loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: nested_block_post_fn_name(&proc.name, slot_name),
                                type_args: Vec::new(),
                                args: call_args,
                            },
                        }],
                        else_branch: Vec::new(),
                    });
                }
            }
            inject_bound_proc_param_hooks_in_stmts(
                Some(&proc.name),
                &mut nested_block_post_body,
                &nested_hook_instances,
                &shape.nested_proc_array_slots,
                proc_api,
                errors,
            );
            let nested_block_post_name = nested_block_post_fn_name(&proc.name, &nested_path);
            def_sample_oversample_factors.insert(
                nested_block_post_name.clone(),
                callee_sample_oversample_factor,
            );
            nested_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                is_const: false,
                type_params: Vec::new(),
                name: nested_block_post_name,
                params: nested_block_params,
                return_ty: None,
                return_ty_loc: Default::default(),
                body: nested_block_post_body,
            }));
        }

        for (idx, out_name) in callee_shape.outs.iter().enumerate() {
            let mut call_args = vec![CallArg {
                name: None,
                expr: Expr::var("self"),
            }];
            for in_name in &callee_shape.ins {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::var(in_name.clone()),
                });
            }
            for buffer in &callee_shape.buffer_specs {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::var(buffer.name.clone()),
                });
            }
            nested_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                is_const: false,
                type_params: Vec::new(),
                name: nested_call_out_fn_name(&proc.name, &nested_path, idx),
                params: nested_step_params.clone(),
                return_ty: None,
                return_ty_loc: Default::default(),
                body: vec![
                    Stmt::Expr {
                        loc: Default::default(),
                        expr: Expr::UserCall {
                            loc: Default::default(),
                            name: nested_step_name.clone(),
                            type_args: Vec::new(),
                            args: call_args,
                        },
                    },
                    Stmt::Return {
                        loc: Default::default(),
                        expr: Expr::var(format!(
                            "self.{}",
                            nested_field_name(&nested_path, out_name)
                        )),
                    },
                ],
            }));
        }
    }
    nested_defs
}

type GeneratedProcBlocks = (
    Vec<Block>,
    Vec<Block>,
    HashMap<String, usize>,
    HashMap<String, ProcStepOversampleMeta>,
);

pub(super) fn generate_lowered_proc_blocks(
    proc_order: &[String],
    proc_defs_by_name: &HashMap<String, ProcessorDef>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    struct_defs_by_name: &HashMap<String, StructDef>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> GeneratedProcBlocks {
    let mut generated_structs = Vec::<Block>::new();
    let mut generated_defs = Vec::<Block>::new();
    let mut def_sample_oversample_factors = HashMap::<String, usize>::new();
    let mut proc_step_oversample_meta = HashMap::<String, ProcStepOversampleMeta>::new();
    for proc_name in proc_order {
        let Some(proc) = proc_defs_by_name.get(proc_name) else {
            continue;
        };
        let Some(shape) = lowering_shapes.get(proc_name).cloned() else {
            continue;
        };
        let proc_sample_oversample_factor = proc_api
            .get(&proc.name)
            .map(|api| api.sample_oversample_factor)
            .unwrap_or(1)
            .max(1);

        let mut nested_vars = shape.state.nested_procs.keys().cloned().collect::<Vec<_>>();
        nested_vars.sort();
        let mut nested_instances = HashMap::<String, ProcCallInstance>::new();
        for nested_var in &nested_vars {
            let Some(nested_state) = shape.state.nested_procs.get(nested_var) else {
                continue;
            };
            if !lowering_shapes.contains_key(&nested_state.proc_name) {
                push_semantic(
                    DiagCtx::default(),
                    errors,
                    format!(
                        "processor '{}' nested state '{}' references unknown processor '{}'",
                        proc.name, nested_var, nested_state.proc_name
                    ),
                );
                continue;
            }
            nested_instances.insert(
                nested_var.clone(),
                ProcCallInstance {
                    proc_name: nested_state.proc_name.clone(),
                    buffer_args: Vec::new(),
                },
            );
        }
        let mut hook_proc_instances = nested_instances.clone();
        hook_proc_instances.insert(
            "self".to_owned(),
            ProcCallInstance {
                proc_name: proc.name.clone(),
                buffer_args: Vec::new(),
            },
        );

        let struct_idx = generated_structs.len();
        generated_structs.push(Block::Struct(onda_frontend::StructDef {
            loc: Default::default(),
            name: proc.name.clone(),
            type_params: Vec::new(),
            fields: shape.fields.clone(),
            methods: Vec::new(),
        }));

        let mut read_lens = shape
            .in_array_slots
            .values()
            .chain(shape.field_array_slots.values())
            .map(|slots| slots.len())
            .filter(|len| *len > 1)
            .collect::<Vec<_>>();
        if let Some(api) = proc_api.get(&proc.name) {
            for event in api.events.values() {
                for param in &event.params {
                    let len = param.slots.len();
                    if len > 1 {
                        read_lens.push(len);
                    }
                }
            }
        }
        read_lens.sort_unstable();
        read_lens.dedup();
        for len in read_lens {
            generated_defs.push(Block::Def(build_proc_read_helper(&proc.name, len, false)));
            generated_defs.push(Block::Def(build_proc_read_helper(&proc.name, len, true)));
        }

        let mut generated_write_helpers = HashSet::<String>::new();
        let mut write_slots = shape
            .field_array_slots
            .values()
            .cloned()
            .collect::<Vec<Vec<String>>>();
        write_slots.sort();
        write_slots.dedup();
        for slots in write_slots {
            let clamp_name = proc_write_helper_name(&proc.name, &slots, false);
            if generated_write_helpers.insert(clamp_name) {
                let mut helper = build_proc_write_helper(&proc.name, &slots, false);
                inject_bound_proc_param_hooks_in_stmts(
                    Some(&proc.name),
                    &mut helper.body,
                    &hook_proc_instances,
                    &shape.nested_proc_array_slots,
                    proc_api,
                    errors,
                );
                generated_defs.push(Block::Def(helper));
            }
            let unsafe_name = proc_write_helper_name(&proc.name, &slots, true);
            if generated_write_helpers.insert(unsafe_name) {
                let mut helper = build_proc_write_helper(&proc.name, &slots, true);
                inject_bound_proc_param_hooks_in_stmts(
                    Some(&proc.name),
                    &mut helper.body,
                    &hook_proc_instances,
                    &shape.nested_proc_array_slots,
                    proc_api,
                    errors,
                );
                generated_defs.push(Block::Def(helper));
            }
        }

        let mut ins_names = shape.ins.iter().cloned().collect::<HashSet<_>>();
        for port in &shape.in_ports {
            ins_names.insert(port.name.clone());
        }
        for buffer in &shape.buffer_specs {
            ins_names.insert(buffer.name.clone());
        }

        let mut init_body = Vec::<Stmt>::new();
        let mut constructor_setup_indices = HashSet::<usize>::new();
        let constructor_array_symbols = proc_constructor_array_symbols(&shape);
        let proc_symbols = proc_api.keys().cloned().collect::<HashSet<_>>();
        let proc_ns = namespace_of_symbol(&proc.name);
        let mut proc_init_stmts = mark_pinned_initializers(&proc.init);
        lower_named_proc_param_calls_in_stmts(
            &mut proc_init_stmts,
            &nested_instances,
            &shape.nested_proc_array_slots,
            proc_api,
            errors,
        );
        for stmt in &proc_init_stmts {
            if let Stmt::Assign {
                target: AssignTarget::Var(array_var),
                expr: expr @ Expr::ArrayCtor { init, .. },
                ..
            } = stmt
            {
                if let Some(slot_names) = shape.nested_proc_array_slots.get(array_var) {
                    let Some(array_state) = shape.state.nested_proc_arrays.get(array_var) else {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' processor-array '{}' is missing state metadata",
                                proc.name, array_var
                            ),
                        );
                        continue;
                    };

                    if let Some(values) = init {
                        if values.len() != slot_names.len() && values.len() != 1 {
                            with_expr_diag_context(expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "processor '{}.{}' initializer expects {} constructor entries (or a single broadcast constructor), got {}",
                                        proc.name,
                                        array_var,
                                        slot_names.len(),
                                        values.len()
                                    ),
                                );
                            });
                        }
                    }

                    for (slot_idx, slot_name) in slot_names.iter().enumerate() {
                        let mut slot_ctor_args = Vec::<CallArg>::new();
                        let mut proc_array_slot = None;
                        if let Some(values) = init {
                            let value = if values.len() == 1 {
                                proc_array_slot = Some((slot_idx, slot_names.len()));
                                values.first()
                            } else {
                                values.get(slot_idx)
                            };
                            if let Some(value) = value {
                                if let Expr::UserCall {
                                    name: ctor_name,
                                    type_args,
                                    args,
                                    ..
                                } = value
                                {
                                    let resolved_ctor = if ctor_name.contains("::") {
                                        if proc_symbols.contains(ctor_name) {
                                            Some(ctor_name.clone())
                                        } else {
                                            None
                                        }
                                    } else {
                                        resolve_unqualified_symbol_name(
                                            ctor_name,
                                            &proc_ns,
                                            &proc_symbols,
                                        )
                                    };
                                    if let Some(resolved_ctor) = resolved_ctor {
                                        if resolved_ctor != array_state.proc_name {
                                            with_expr_diag_context(value, |expr_diag| {
                                                push_semantic(
                                                    expr_diag,
                                                    errors,
                                                    format!(
                                                        "processor '{}.{}' initializer entry {} uses constructor '{}' but '{}' is required",
                                                        proc.name,
                                                        array_var,
                                                        slot_idx,
                                                        resolved_ctor,
                                                        array_state.proc_name
                                                    ),
                                                );
                                            });
                                        }
                                    } else {
                                        with_expr_diag_context(value, |expr_diag| {
                                            push_semantic(
                                                expr_diag,
                                                errors,
                                                format!(
                                                    "processor '{}.{}' initializer entry {} references unknown processor constructor '{}'",
                                                    proc.name, array_var, slot_idx, ctor_name
                                                ),
                                            );
                                        });
                                    }
                                    if !type_args.is_empty() {
                                        with_expr_diag_context(value, |expr_diag| {
                                            push_semantic(
                                                expr_diag,
                                                errors,
                                                format!(
                                                    "processor '{}' is not generic and cannot take type arguments",
                                                    ctor_name
                                                ),
                                            );
                                        });
                                    }
                                    slot_ctor_args = args.clone();
                                } else {
                                    with_expr_diag_context(value, |expr_diag| {
                                        push_semantic(
                                            expr_diag,
                                            errors,
                                            format!(
                                                "processor '{}.{}' initializer entry {} must be a processor constructor call",
                                                proc.name, array_var, slot_idx
                                            ),
                                        );
                                    });
                                }
                            }
                        }

                        let Some(slot_state) = shape.state.nested_procs.get(slot_name) else {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' processor-array slot '{}' is missing nested processor state",
                                    proc.name, slot_name
                                ),
                            );
                            continue;
                        };
                        let Some(callee_shape) = lowering_shapes.get(&slot_state.proc_name) else {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor '{}' nested state '{}' references unknown processor '{}'",
                                    proc.name, slot_name, slot_state.proc_name
                                ),
                            );
                            continue;
                        };
                        let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                            &proc.name,
                            slot_name,
                            &slot_state.proc_name,
                            &slot_ctor_args,
                            &callee_shape.param_specs,
                            &callee_shape.buffer_specs,
                            proc_array_slot,
                            &constructor_array_symbols,
                            errors,
                        );
                        if let Some(instance) = nested_instances.get_mut(slot_name) {
                            instance.buffer_args = bound_buffers;
                        }
                        for expanded_stmt in expanded {
                            if let Some(rewritten) = rewrite_owner_proc_stmt(
                                expanded_stmt,
                                &proc.name,
                                &shape.field_names,
                                &shape.array_field_names,
                                &ins_names,
                                &shape.field_array_slots,
                                &shape.in_array_slots,
                                &shape.nested_proc_array_slots,
                                &shape.nested_fields,
                                &nested_instances,
                                proc_api,
                                errors,
                            ) {
                                constructor_setup_indices.insert(init_body.len());
                                init_body.push(rewritten);
                            }
                        }
                    }
                    continue;
                }
                if let Some(values) = init {
                    let mut decl_stmt = stmt.clone();
                    if let Stmt::Assign {
                        expr: Expr::ArrayCtor { init, .. },
                        ..
                    } = &mut decl_stmt
                    {
                        *init = None;
                    }
                    if let Some(rewritten) = rewrite_owner_proc_stmt(
                        decl_stmt,
                        &proc.name,
                        &shape.field_names,
                        &shape.array_field_names,
                        &ins_names,
                        &shape.field_array_slots,
                        &shape.in_array_slots,
                        &shape.nested_proc_array_slots,
                        &shape.nested_fields,
                        &nested_instances,
                        proc_api,
                        errors,
                    ) {
                        init_body.push(rewritten);
                    }
                    for (idx, value) in values.iter().cloned().enumerate() {
                        let write_stmt = Stmt::Assign {
                            loc: Default::default(),
                            target_loc: Default::default(),
                            target: AssignTarget::Index {
                                base: array_var.clone(),
                                index: Expr::int(idx as i64),
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            typed_decl_ty_loc: Default::default(),
                            expr: value,
                        };
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            write_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            proc_api,
                            errors,
                        ) {
                            init_body.push(rewritten);
                        }
                    }
                    continue;
                }
            }
            if let Some(fill_stmt) = declaration_only_primitive_array_fill(stmt) {
                if let Some(rewritten) = rewrite_owner_proc_stmt(
                    fill_stmt,
                    &proc.name,
                    &shape.field_names,
                    &shape.array_field_names,
                    &ins_names,
                    &shape.field_array_slots,
                    &shape.in_array_slots,
                    &shape.nested_proc_array_slots,
                    &shape.nested_fields,
                    &nested_instances,
                    proc_api,
                    errors,
                ) {
                    init_body.push(rewritten);
                }
                continue;
            }
            if let Stmt::Assign {
                target: AssignTarget::Var(array_var),
                expr: Expr::ArrayLiteral { values, .. },
                ..
            } = stmt
            {
                for (idx, value) in values.iter().cloned().enumerate() {
                    let write_stmt = Stmt::Assign {
                        loc: Default::default(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: array_var.clone(),
                            index: Expr::int(idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        typed_decl_ty_loc: Default::default(),
                        expr: value,
                    };
                    if let Some(rewritten) = rewrite_owner_proc_stmt(
                        write_stmt,
                        &proc.name,
                        &shape.field_names,
                        &shape.array_field_names,
                        &ins_names,
                        &shape.field_array_slots,
                        &shape.in_array_slots,
                        &shape.nested_proc_array_slots,
                        &shape.nested_fields,
                        &nested_instances,
                        proc_api,
                        errors,
                    ) {
                        init_body.push(rewritten);
                    }
                }
                continue;
            }
            if let Stmt::Assign {
                target: AssignTarget::Var(var),
                expr:
                    Expr::UserCall {
                        name: ctor_name,
                        type_args,
                        args,
                        ..
                    },
                ..
            } = stmt
            {
                if let Some(nested_state) = shape.state.nested_procs.get(var) {
                    if !type_args.is_empty() {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' is not generic and cannot take type arguments",
                                ctor_name
                            ),
                        );
                    }
                    if nested_state.proc_name != *ctor_name {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor state symbol '{}' has conflicting processor types '{}' and '{}'",
                                var, nested_state.proc_name, ctor_name
                            ),
                        );
                        continue;
                    }
                    let Some(callee_shape) = lowering_shapes.get(&nested_state.proc_name) else {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor '{}' nested state '{}' references unknown processor '{}'",
                                proc.name, var, nested_state.proc_name
                            ),
                        );
                        continue;
                    };
                    let (expanded, bound_buffers) = expand_nested_proc_ctor_assign(
                        &proc.name,
                        var,
                        ctor_name,
                        args,
                        &callee_shape.param_specs,
                        &callee_shape.buffer_specs,
                        None,
                        &constructor_array_symbols,
                        errors,
                    );
                    if let Some(instance) = nested_instances.get_mut(var) {
                        instance.buffer_args = bound_buffers;
                    }
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            proc_api,
                            errors,
                        ) {
                            constructor_setup_indices.insert(init_body.len());
                            init_body.push(rewritten);
                        }
                    }
                    continue;
                }
                if let Some(state_struct) = shape.state.struct_instances.get(var) {
                    if state_struct.struct_name != *ctor_name {
                        push_semantic(
                            DiagCtx::new(stmt.loc()),
                            errors,
                            format!(
                                "processor state symbol '{}' has conflicting struct types '{}' and '{}'",
                                var, state_struct.struct_name, ctor_name
                            ),
                        );
                        continue;
                    }
                    let Some(struct_def) = resolve_proc_state_struct_def(
                        &proc.name,
                        var,
                        state_struct,
                        struct_defs_by_name,
                        errors,
                    ) else {
                        continue;
                    };
                    if !type_args.is_empty() {
                        let Some(resolved_type_args) = resolve_explicit_call_type_args(
                            type_args,
                            &format!("processor state constructor '{} = {}(...)'", var, ctor_name),
                            DiagCtx::new(stmt.loc()),
                            errors,
                        ) else {
                            continue;
                        };
                        if resolved_type_args.as_slice() != state_struct.type_args.as_slice() {
                            push_semantic(
                                DiagCtx::new(stmt.loc()),
                                errors,
                                format!(
                                    "processor state constructor '{} = {}(...)' uses type arguments inconsistent with the declared state specialization",
                                    var, ctor_name
                                ),
                            );
                        }
                    }
                    let expanded = expand_nested_struct_ctor_assign(
                        var,
                        ctor_name,
                        args,
                        &struct_def,
                        struct_defs_by_name,
                        errors,
                    );
                    for expanded_stmt in expanded {
                        if let Some(rewritten) = rewrite_owner_proc_stmt(
                            expanded_stmt,
                            &proc.name,
                            &shape.field_names,
                            &shape.array_field_names,
                            &ins_names,
                            &shape.field_array_slots,
                            &shape.in_array_slots,
                            &shape.nested_proc_array_slots,
                            &shape.nested_fields,
                            &nested_instances,
                            proc_api,
                            errors,
                        ) {
                            init_body.push(rewritten);
                        }
                    }
                    continue;
                }
            }
            if let Some(rewritten) = rewrite_owner_proc_stmt(
                stmt.clone(),
                &proc.name,
                &shape.field_names,
                &shape.array_field_names,
                &ins_names,
                &shape.field_array_slots,
                &shape.in_array_slots,
                &shape.nested_proc_array_slots,
                &shape.nested_fields,
                &nested_instances,
                proc_api,
                errors,
            ) {
                init_body.push(rewritten);
            }
        }

        inject_bound_proc_param_hooks_in_stmts_skipping_top_level(
            Some(&proc.name),
            &mut init_body,
            &hook_proc_instances,
            &shape.nested_proc_array_slots,
            proc_api,
            errors,
            &constructor_setup_indices,
        );
        guard_pinned_initializers(&mut init_body, INIT_ALL_PARAM_NAME);
        init_body.extend(after_init_bind_hook_stmts(&proc.name, &shape.param_specs));
        let init_fn_name = format!("{}{}", proc.name, PROC_INIT_FN_SUFFIX);
        def_sample_oversample_factors.insert(init_fn_name.clone(), proc_sample_oversample_factor);
        generated_defs.push(Block::Def(FunctionDef {
            loc: Default::default(),
            is_const: false,
            type_params: Vec::new(),
            name: init_fn_name,
            params: vec![
                onda_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: "self".to_owned(),
                    ty: Some(FnParamType::Struct(proc.name.clone())),
                    ty_loc: Default::default(),
                    default: None,
                },
                onda_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: INIT_ALL_PARAM_NAME.to_owned(),
                    ty: Some(FnParamType::Primitive(PrimitiveType::Bool)),
                    ty_loc: Default::default(),
                    default: None,
                },
            ],
            return_ty: None,
            return_ty_loc: Default::default(),
            body: init_body,
        }));

        let mut nested_managed_active_fields = HashMap::<String, usize>::new();
        generated_defs.extend(generate_nested_wrapper_defs(
            proc,
            &shape,
            proc_defs_by_name,
            lowering_shapes,
            struct_defs_by_name,
            proc_api,
            &nested_instances,
            &ins_names,
            &mut nested_managed_active_fields,
            &mut def_sample_oversample_factors,
            &mut proc_step_oversample_meta,
            errors,
        ));
        if let Some(owner_api) = proc_api.get(&proc.name) {
            for event in &proc.events {
                let Some(event_spec) = owner_api.events.get(&event.name) else {
                    push_semantic(
                        DiagCtx::new(event.loc),
                        errors,
                        format!(
                            "processor '{}' event '{}' is missing lowered event metadata",
                            proc.name, event.name
                        ),
                    );
                    continue;
                };
                let (event_params, event_body) = if is_builtin_proc_init_event_name(&event.name) {
                    build_builtin_proc_init_event_parts(
                        &proc.name,
                        &shape.param_specs,
                        |slot| format!("self.{slot}"),
                        format!("{}{}", proc.name, PROC_INIT_FN_SUFFIX),
                    )
                } else {
                    let mut event_params = Vec::<onda_frontend::FnParamDecl>::new();
                    event_params.push(onda_frontend::FnParamDecl {
                        loc: Default::default(),
                        name: "self".to_owned(),
                        ty: Some(FnParamType::Struct(proc.name.clone())),
                        ty_loc: Default::default(),
                        default: None,
                    });
                    let mut event_ins_names = ins_names.clone();
                    let mut event_in_array_slots = HashMap::<String, Vec<String>>::new();
                    for param in &event_spec.params {
                        event_ins_names.insert(param.name.clone());
                        if let Some(param_ty) = proc_event_array_param_fn_ty(param.ty) {
                            event_params.push(onda_frontend::FnParamDecl {
                                loc: Default::default(),
                                name: param.name.clone(),
                                ty: Some(param_ty),
                                ty_loc: Default::default(),
                                default: None,
                            });
                            continue;
                        }
                        let mut slot_names = Vec::<String>::new();
                        for slot in &param.slots {
                            slot_names.push(slot.name.clone());
                            event_ins_names.insert(slot.name.clone());
                            event_params.push(onda_frontend::FnParamDecl {
                                loc: Default::default(),
                                name: slot.name.clone(),
                                ty: Some(FnParamType::Primitive(slot.ty)),
                                ty_loc: Default::default(),
                                default: None,
                            });
                        }
                        if slot_names.len() > 1 {
                            event_in_array_slots.insert(param.name.clone(), slot_names);
                        }
                    }
                    let mut event_body = rewrite_owner_proc_stmts(
                        event.body.clone(),
                        &proc.name,
                        &shape.field_names,
                        &shape.array_field_names,
                        &event_ins_names,
                        &shape.field_array_slots,
                        &event_in_array_slots,
                        &shape.nested_proc_array_slots,
                        &shape.nested_fields,
                        &nested_instances,
                        proc_api,
                        errors,
                    );
                    inject_bound_proc_param_hooks_in_stmts(
                        Some(&proc.name),
                        &mut event_body,
                        &hook_proc_instances,
                        &shape.nested_proc_array_slots,
                        proc_api,
                        errors,
                    );
                    (event_params, event_body)
                };
                let event_fn_name = format!("{}{}{}", proc.name, PROC_EVENT_FN_PREFIX, event.name);
                def_sample_oversample_factors
                    .insert(event_fn_name.clone(), proc_sample_oversample_factor);
                generated_defs.push(Block::Def(FunctionDef {
                    loc: Default::default(),
                    is_const: false,
                    type_params: Vec::new(),
                    name: event_fn_name,
                    params: event_params,
                    return_ty: None,
                    return_ty_loc: Default::default(),
                    body: event_body,
                }));
            }
        }

        let managed_dynamic_arrays = shape
            .nested_proc_array_slots
            .iter()
            .filter_map(|(array_base, slots)| {
                let first_slot = slots.first()?;
                let instance = nested_instances.get(first_slot)?;
                let api = proc_api.get(&instance.proc_name)?;
                if !proc_needs_block_hooks(api) {
                    return None;
                }
                let active_field = shape
                    .nested_proc_array_active_fields
                    .get(array_base)
                    .cloned()?;
                Some((
                    array_base.clone(),
                    ManagedDynamicProcArray {
                        proc_name: instance.proc_name.clone(),
                        array_base: array_base.clone(),
                        raw_slots: slots.clone(),
                        slots: slots.clone(),
                        active_field,
                    },
                ))
            })
            .collect::<HashMap<_, _>>();

        let mut used_managed_dynamic_arrays = HashSet::<String>::new();
        let mut dynamic_hook_temp_counter = 0;
        let mut step_body = Vec::<Stmt>::new();
        for local_def in unique_proc_local_defs(proc) {
            let mut body = Vec::<Stmt>::new();
            for rewritten in rewrite_owner_proc_stmts(
                local_def.body.clone(),
                &proc.name,
                &shape.field_names,
                &shape.array_field_names,
                &ins_names,
                &shape.field_array_slots,
                &shape.in_array_slots,
                &shape.nested_proc_array_slots,
                &shape.nested_fields,
                &nested_instances,
                proc_api,
                errors,
            ) {
                body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                    rewritten,
                    &managed_dynamic_arrays,
                    proc_api,
                    &mut used_managed_dynamic_arrays,
                    &mut dynamic_hook_temp_counter,
                ));
            }
            inject_bound_proc_param_hooks_in_stmts(
                Some(&proc.name),
                &mut body,
                &hook_proc_instances,
                &shape.nested_proc_array_slots,
                proc_api,
                errors,
            );
            let local_fn_name = proc_local_hidden_def_name(&proc.name, &local_def.name);
            def_sample_oversample_factors
                .insert(local_fn_name.clone(), proc_sample_oversample_factor);
            generated_defs.push(Block::Def(owner_proc_local_hidden_def(
                &proc.name, &local_def, body,
            )));
        }
        let persistent_buffer_aliases =
            persistent_proc_buffer_aliases(&proc.block_pre, &shape.buffer_specs);
        let proc_step_source = if proc.outs_timing == OutputTiming::Block {
            proc.block_pre.clone()
        } else {
            let mut body = rebind_persistent_buffer_aliases(&persistent_buffer_aliases);
            body.extend(proc.sample.clone());
            body
        };
        for rewritten in rewrite_owner_proc_stmts(
            proc_step_source,
            &proc.name,
            &shape.field_names,
            &shape.array_field_names,
            &ins_names,
            &shape.field_array_slots,
            &shape.in_array_slots,
            &shape.nested_proc_array_slots,
            &shape.nested_fields,
            &nested_instances,
            proc_api,
            errors,
        ) {
            step_body.extend(rewrite_stmt_for_managed_dynamic_proc_block_hooks(
                rewritten,
                &managed_dynamic_arrays,
                proc_api,
                &mut used_managed_dynamic_arrays,
                &mut dynamic_hook_temp_counter,
            ));
        }
        inject_bound_proc_param_hooks_in_stmts(
            Some(&proc.name),
            &mut step_body,
            &hook_proc_instances,
            &shape.nested_proc_array_slots,
            proc_api,
            errors,
        );

        let proc_has_effective_block = proc_api
            .get(&proc.name)
            .map(proc_needs_block_hooks)
            .unwrap_or(proc.has_block_block);
        if proc_has_effective_block {
            let mut block_params = Vec::<onda_frontend::FnParamDecl>::new();
            block_params.push(onda_frontend::FnParamDecl {
                loc: Default::default(),
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc.name.clone())),
                ty_loc: Default::default(),
                default: None,
            });
            for buffer in &shape.buffer_specs {
                block_params.push(onda_frontend::FnParamDecl {
                    loc: Default::default(),
                    name: buffer.name.clone(),
                    ty: Some(proc_buffer_fn_param_type(buffer)),
                    ty_loc: Default::default(),
                    default: None,
                });
            }
            let mut block_pre_body = Vec::<Stmt>::new();
            block_pre_body.extend(rewrite_owner_proc_stmts(
                capture_persistent_buffer_alias_selectors(
                    proc.block_pre.clone(),
                    &persistent_buffer_aliases,
                ),
                &proc.name,
                &shape.field_names,
                &shape.array_field_names,
                &ins_names,
                &shape.field_array_slots,
                &shape.in_array_slots,
                &shape.nested_proc_array_slots,
                &shape.nested_fields,
                &nested_instances,
                proc_api,
                errors,
            ));
            let mut called_nested = collect_called_proc_instances_in_stmts(
                &proc.sample,
                &hook_proc_instances,
                &shape.nested_proc_array_slots,
            );
            for array_base in &used_managed_dynamic_arrays {
                if let Some(managed) = managed_dynamic_arrays.get(array_base) {
                    for slot in &managed.raw_slots {
                        called_nested.remove(slot);
                    }
                }
            }
            let mut nested_vars = nested_instances.keys().cloned().collect::<Vec<_>>();
            nested_vars.sort();
            let mut managed_arrays = managed_dynamic_arrays.values().collect::<Vec<_>>();
            managed_arrays.sort_by(|a, b| a.active_field.cmp(&b.active_field));
            for managed in &managed_arrays {
                for slot_idx in 0..managed.slots.len() {
                    block_pre_body.push(Stmt::Assign {
                        loc: Default::default(),
                        target_loc: Default::default(),
                        target: AssignTarget::Index {
                            base: format!("self.{}", managed.active_field),
                            index: Expr::int(slot_idx as i64),
                        },
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: false,
                        typed_decl_ty_loc: Default::default(),
                        expr: Expr::bool(false),
                    });
                }
            }
            for nested_var in &nested_vars {
                if !called_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !proc_needs_block_hooks(api) {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::var("self"),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                block_pre_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
                        name: nested_block_pre_fn_name(&proc.name, nested_var),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            inject_bound_proc_param_hooks_in_stmts(
                Some(&proc.name),
                &mut block_pre_body,
                &hook_proc_instances,
                &shape.nested_proc_array_slots,
                proc_api,
                errors,
            );
            let block_pre_fn_name = format!("{}{}", proc.name, PROC_BLOCK_PRE_FN_SUFFIX);
            def_sample_oversample_factors
                .insert(block_pre_fn_name.clone(), proc_sample_oversample_factor);
            generated_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                is_const: false,
                type_params: Vec::new(),
                name: block_pre_fn_name,
                params: block_params.clone(),
                return_ty: None,
                return_ty_loc: Default::default(),
                body: block_pre_body,
            }));

            let mut block_post_body = Vec::<Stmt>::new();
            for nested_var in &nested_vars {
                if !called_nested.contains(nested_var) {
                    continue;
                }
                let Some(instance) = nested_instances.get(nested_var) else {
                    continue;
                };
                let Some(api) = proc_api.get(&instance.proc_name) else {
                    continue;
                };
                if !proc_needs_block_hooks(api) {
                    continue;
                }
                let mut call_args = vec![CallArg {
                    name: None,
                    expr: Expr::var("self"),
                }];
                call_args.extend(expand_proc_buffer_call_args(
                    instance, api, nested_var, errors,
                ));
                block_post_body.push(Stmt::Expr {
                    loc: Default::default(),
                    expr: Expr::UserCall {
                        loc: Default::default(),
                        name: nested_block_post_fn_name(&proc.name, nested_var),
                        type_args: Vec::new(),
                        args: call_args,
                    },
                });
            }
            let mut block_post_source =
                rebind_persistent_buffer_aliases(&persistent_buffer_aliases);
            block_post_source.extend(proc.block_post.clone());
            block_post_body.extend(rewrite_owner_proc_stmts(
                block_post_source,
                &proc.name,
                &shape.field_names,
                &shape.array_field_names,
                &ins_names,
                &shape.field_array_slots,
                &shape.in_array_slots,
                &shape.nested_proc_array_slots,
                &shape.nested_fields,
                &nested_instances,
                proc_api,
                errors,
            ));
            for managed in &managed_arrays {
                for (slot_idx, slot_name) in managed.slots.iter().enumerate() {
                    let Some(instance) = nested_instances.get(slot_name) else {
                        continue;
                    };
                    let Some(api) = proc_api.get(&instance.proc_name) else {
                        continue;
                    };
                    let mut call_args = vec![CallArg {
                        name: None,
                        expr: Expr::Index {
                            loc: Default::default(),
                            base: managed.array_base.clone(),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                    }];
                    call_args.extend(expand_proc_buffer_call_args(
                        instance, api, slot_name, errors,
                    ));
                    block_post_body.push(Stmt::If {
                        loc: Default::default(),
                        cond: Expr::Index {
                            loc: Default::default(),
                            base: format!("self.{}", managed.active_field),
                            index: Box::new(Expr::int(slot_idx as i64)),
                        },
                        then_branch: vec![Stmt::Expr {
                            loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: format!(
                                    "{}{}",
                                    instance.proc_name, PROC_BLOCK_POST_FN_SUFFIX
                                ),
                                type_args: Vec::new(),
                                args: call_args,
                            },
                        }],
                        else_branch: Vec::new(),
                    });
                }
            }
            inject_bound_proc_param_hooks_in_stmts(
                Some(&proc.name),
                &mut block_post_body,
                &hook_proc_instances,
                &shape.nested_proc_array_slots,
                proc_api,
                errors,
            );
            let block_post_fn_name = format!("{}{}", proc.name, PROC_BLOCK_POST_FN_SUFFIX);
            def_sample_oversample_factors
                .insert(block_post_fn_name.clone(), proc_sample_oversample_factor);
            generated_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                is_const: false,
                type_params: Vec::new(),
                name: block_post_fn_name,
                params: block_params,
                return_ty: None,
                return_ty_loc: Default::default(),
                body: block_post_body,
            }));
        }
        let mut step_params = Vec::<onda_frontend::FnParamDecl>::new();
        step_params.push(onda_frontend::FnParamDecl {
            loc: Default::default(),
            name: "self".to_owned(),
            ty: Some(FnParamType::Struct(proc.name.clone())),
            ty_loc: Default::default(),
            default: None,
        });
        for in_name in &shape.ins {
            let in_ty = *shape.in_types.get(in_name).unwrap_or(&PrimitiveType::F32);
            step_params.push(onda_frontend::FnParamDecl {
                loc: Default::default(),
                name: in_name.clone(),
                ty: Some(FnParamType::Primitive(in_ty)),
                ty_loc: Default::default(),
                default: None,
            });
        }
        for buffer in &shape.buffer_specs {
            step_params.push(onda_frontend::FnParamDecl {
                loc: Default::default(),
                name: buffer.name.clone(),
                ty: Some(proc_buffer_fn_param_type(buffer)),
                ty_loc: Default::default(),
                default: None,
            });
        }
        let step_fn_name = format!("{}{}", proc.name, PROC_STEP_FN_SUFFIX);
        def_sample_oversample_factors.insert(step_fn_name.clone(), proc_sample_oversample_factor);
        if proc_sample_oversample_factor > 1 {
            let stage_count = proc_os_sinc_stage_count(proc_sample_oversample_factor);
            let mut input_state_fields = HashMap::<String, ProcInputOversampleStateFields>::new();
            for in_name in &shape.ins {
                let in_ty = *shape.in_types.get(in_name).unwrap_or(&PrimitiveType::F32);
                let up_stages = if matches!(in_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    (0..stage_count)
                        .map(|stage| ProcSincStageStateFields {
                            a0: proc_os_up_stage_tap_field_name(in_name, stage, "a0"),
                            a1: proc_os_up_stage_tap_field_name(in_name, stage, "a1"),
                            a2: proc_os_up_stage_tap_field_name(in_name, stage, "a2"),
                            a3: proc_os_up_stage_tap_field_name(in_name, stage, "a3"),
                            b0: proc_os_up_stage_tap_field_name(in_name, stage, "b0"),
                            b1: proc_os_up_stage_tap_field_name(in_name, stage, "b1"),
                            b2: proc_os_up_stage_tap_field_name(in_name, stage, "b2"),
                            b3: proc_os_up_stage_tap_field_name(in_name, stage, "b3"),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                input_state_fields.insert(
                    in_name.clone(),
                    ProcInputOversampleStateFields { up_stages },
                );
            }
            let mut output_state_fields = HashMap::<String, ProcOutputOversampleStateFields>::new();
            for out_name in &shape.outs {
                let out_ty = *shape.out_types.get(out_name).unwrap_or(&PrimitiveType::F32);
                let down_stages = if matches!(out_ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    (0..stage_count)
                        .map(|stage| ProcSincStageStateFields {
                            a0: proc_os_down_stage_tap_field_name(out_name, stage, "a0"),
                            a1: proc_os_down_stage_tap_field_name(out_name, stage, "a1"),
                            a2: proc_os_down_stage_tap_field_name(out_name, stage, "a2"),
                            a3: proc_os_down_stage_tap_field_name(out_name, stage, "a3"),
                            b0: proc_os_down_stage_tap_field_name(out_name, stage, "b0"),
                            b1: proc_os_down_stage_tap_field_name(out_name, stage, "b1"),
                            b2: proc_os_down_stage_tap_field_name(out_name, stage, "b2"),
                            b3: proc_os_down_stage_tap_field_name(out_name, stage, "b3"),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                output_state_fields.insert(
                    out_name.clone(),
                    ProcOutputOversampleStateFields { down_stages },
                );
            }
            proc_step_oversample_meta.insert(
                step_fn_name.clone(),
                ProcStepOversampleMeta {
                    input_state_fields,
                    output_state_fields,
                },
            );
        }
        generated_defs.push(Block::Def(FunctionDef {
            loc: Default::default(),
            is_const: false,
            type_params: Vec::new(),
            name: step_fn_name.clone(),
            params: step_params.clone(),
            return_ty: None,
            return_ty_loc: Default::default(),
            body: step_body,
        }));

        if !managed_dynamic_arrays.is_empty() || !nested_managed_active_fields.is_empty() {
            if let Some(Block::Struct(def)) = generated_structs.get_mut(struct_idx) {
                for managed in managed_dynamic_arrays.values() {
                    if def.fields.iter().any(|f| f.name == managed.active_field) {
                        continue;
                    }
                    def.fields.push(StructField {
                        loc: Default::default(),
                        name: managed.active_field.clone(),
                        ty: FieldType::Array(onda_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(PrimitiveType::Bool),
                            size: Box::new(Expr::int(managed.slots.len() as i64)),
                        }),
                        ty_loc: Default::default(),
                        default: None,
                    });
                }
                for (field_name, len) in &nested_managed_active_fields {
                    if def.fields.iter().any(|f| f.name == *field_name) {
                        continue;
                    }
                    def.fields.push(StructField {
                        loc: Default::default(),
                        name: field_name.clone(),
                        ty: FieldType::Array(onda_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(PrimitiveType::Bool),
                            size: Box::new(Expr::int(*len as i64)),
                        }),
                        ty_loc: Default::default(),
                        default: None,
                    });
                }
            }
        }

        for (idx, out_name) in shape.outs.iter().enumerate() {
            let out_ty = shape
                .out_types
                .get(out_name)
                .copied()
                .unwrap_or(PrimitiveType::F32);
            let mut call_args = vec![CallArg {
                name: None,
                expr: Expr::var("self"),
            }];
            for in_name in &shape.ins {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::var(in_name.clone()),
                });
            }
            for buffer in &shape.buffer_specs {
                call_args.push(CallArg {
                    name: None,
                    expr: Expr::var(buffer.name.clone()),
                });
            }
            let mut call_out_body = vec![Stmt::Expr {
                loc: Default::default(),
                expr: Expr::UserCall {
                    loc: Default::default(),
                    name: step_fn_name.clone(),
                    type_args: Vec::new(),
                    args: call_args,
                },
            }];
            if shape
                .field_names
                .contains(crate::task_lowering::task_available_field())
            {
                call_out_body.push(Stmt::If {
                    loc: Default::default(),
                    cond: Expr::UnaryNot {
                        loc: Default::default(),
                        expr: Box::new(Expr::var(format!(
                            "self.{}",
                            crate::task_lowering::task_available_field()
                        ))),
                    },
                    then_branch: vec![Stmt::Return {
                        loc: Default::default(),
                        expr: zero_expr(out_ty),
                    }],
                    else_branch: Vec::new(),
                });
            }
            call_out_body.push(Stmt::Return {
                loc: Default::default(),
                expr: Expr::var(format!("self.{out_name}")),
            });
            generated_defs.push(Block::Def(FunctionDef {
                loc: Default::default(),
                is_const: false,
                type_params: Vec::new(),
                name: format!("{}{}{}", proc.name, PROC_CALL_OUT_FN_PREFIX, idx),
                params: step_params.clone(),
                return_ty: None,
                return_ty_loc: Default::default(),
                body: call_out_body,
            }));
        }
    }

    (
        generated_structs,
        generated_defs,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
    )
}
