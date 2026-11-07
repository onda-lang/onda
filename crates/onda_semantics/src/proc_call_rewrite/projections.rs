use super::*;

fn proc_param_setter_name(proc_name: &str, field: &str) -> String {
    format!("{proc_name}.__onda_proc_set_param_{field}")
}

/// A setter keeps the store and hook together through repeated specialization
/// walks. The receiver precedes the value, preserving indexed-place evaluation order.
pub(crate) fn proc_param_setter_def(proc_name: &str, param: &ProcParamSlotSpec) -> FunctionDef {
    let mut store = assign_temp_stmt(format!("self.{}", param.name), Expr::var("value"));
    if let Stmt::Assign {
        param_write_lowered,
        ..
    } = &mut store
    {
        *param_write_lowered = true;
    }
    let body = vec![
        store,
        bind_hook_call_stmt(
            proc_local_bind_hidden_def_name(
                proc_name,
                param.bind.as_deref().expect("bound parameter"),
            ),
            Expr::var("self"),
        ),
    ];
    FunctionDef {
        loc: Default::default(),
        is_const: false,
        type_params: Vec::new(),
        name: proc_param_setter_name(proc_name, &param.name),
        params: vec![
            FnParamDecl {
                readonly: false,
                loc: Default::default(),
                name: "self".to_owned(),
                ty: Some(FnParamType::Struct(proc_name.to_owned())),
                ty_loc: Default::default(),
                default: None,
            },
            FnParamDecl {
                readonly: true,
                loc: Default::default(),
                name: "value".to_owned(),
                ty: Some(FnParamType::Primitive(param.ty)),
                ty_loc: Default::default(),
                default: None,
            },
        ],
        return_ty: None,
        return_ty_loc: Default::default(),
        body,
    }
}

fn projection_instance(receiver: &str, proc_name: &str, api: &ProcApi) -> ProcCallInstance {
    ProcCallInstance {
        proc_name: proc_name.to_owned(),
        buffer_args: api
            .buffers
            .iter()
            .enumerate()
            .map(|(index, _)| {
                Expr::var(crate::processor_views::bound_resource_member(
                    receiver, index,
                ))
            })
            .collect(),
        delegate_context_args: Vec::new(),
        routes_owner_delegates: false,
    }
}

fn parameter_projection<'a>(receiver: &str, env: &'a CallTypeEnv) -> Option<(String, &'a ProcApi)> {
    // Owner writes and constructor expansion have already been lowered. Only
    // external views discovered by lexical typing need this later rewrite.
    if receiver == "self"
        || receiver.starts_with("self.")
        || env.is_materialized_storage_view(receiver)
    {
        return None;
    }
    let shape = env.processor_view_shape(&Expr::var(receiver))?;
    let root = receiver.split('.').next().unwrap_or(receiver);
    if !shape.requires_environment() && !env.is_lexical_processor_binding(root) {
        return None;
    }
    let api = env.processor_apis.get(&shape.proc_name)?;
    Some((shape.proc_name, api))
}

pub(crate) fn projection_temp_counter(stmts: &[Stmt]) -> usize {
    let mut next = 0;
    for stmt in stmts {
        stmt.visit_statements(|stmt| {
            if let Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            } = stmt
            {
                if name.starts_with("__onda_proc_output_tuple_index_") {
                    if let Some(index) = name
                        .rsplit('_')
                        .next()
                        .and_then(|suffix| suffix.parse::<usize>().ok())
                    {
                        next = next.max(index.saturating_add(1));
                    }
                }
            }
        });
    }
    next
}

pub(crate) fn expand_processor_projection_assignment(
    stmt: &mut Stmt,
    env: &CallTypeEnv,
    temp_counter: &mut usize,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<Stmt>> {
    if let Stmt::Assign {
        target: AssignTarget::Tuple(_),
        expr,
        ..
    } = stmt
    {
        let (instances, arrays) = projection_call_bindings(expr, env);
        if instances.is_empty() {
            return None;
        }
        let mut expanded = vec![stmt.clone()];
        expand_proc_output_tuple_assignments_with_aliases(
            &mut expanded,
            &instances,
            &arrays,
            &env.processor_apis,
            &mut ProcArrayAliases::new(),
            temp_counter,
            errors,
        );
        return (expanded.len() != 1 || expanded.first() != Some(stmt)).then_some(expanded);
    }
    let Stmt::Assign {
        loc,
        target,
        target_loc,
        param_write_lowered,
        expr,
        ..
    } = stmt
    else {
        return None;
    };
    if *param_write_lowered {
        return None;
    }
    let (receiver, field) = match &*target {
        AssignTarget::Var(path)
        | AssignTarget::Index { base: path, .. }
        | AssignTarget::Slice { base: path, .. } => split_receiver_field(path)?,
        AssignTarget::IndexedMember { base, field, .. } => (base.as_str(), field.as_str()),
        AssignTarget::Tuple(_) => return None,
    };
    let (proc_name, api) = parameter_projection(receiver, env)?;
    let instances = HashMap::from([(
        receiver.to_owned(),
        projection_instance(receiver, &proc_name, api),
    )]);
    let arrays = ProcArrayDeclarations::new();
    reject_private_proc_param_assignment(
        target,
        target_loc.clone(),
        &instances,
        &arrays,
        &env.processor_apis,
        None,
        errors,
    );
    reject_bound_proc_dynamic_params_assignment(
        target,
        target_loc.clone(),
        &instances,
        &arrays,
        &env.processor_apis,
        errors,
    );
    if let AssignTarget::Index { index, .. } = &*target {
        if api.params.contains_key(&format!("{field}[0]")) {
            let slots = (0..)
                .map(|index| format!("{field}[{index}]"))
                .take_while(|slot| api.params.contains_key(slot))
                .collect::<Vec<_>>();
            *stmt = Stmt::Expr {
                loc: loc.clone(),
                expr: Expr::UserCall {
                    loc: loc.clone(),
                    name: proc_write_helper_name(&proc_name, &slots, false),
                    type_args: Vec::new(),
                    args: vec![
                        CallArg {
                            name: None,
                            expr: Expr::var(receiver),
                        },
                        CallArg {
                            name: None,
                            expr: index.clone(),
                        },
                        CallArg {
                            name: None,
                            expr: std::mem::replace(expr, Expr::int(0)),
                        },
                    ],
                },
            };
            return None;
        }
    }
    maybe_clamp_proc_param_assignment_expr(target, expr, &instances, &arrays, &env.processor_apis);
    let param = api
        .params
        .get(field)
        .filter(|param| !param.private && param.bind.is_some())?;
    let receiver_expr = match &*target {
        AssignTarget::Var(_) => Expr::var(receiver),
        AssignTarget::IndexedMember {
            index,
            field_index: None,
            ..
        } => bound_proc_indexed_receiver_expr(receiver, index, &arrays),
        AssignTarget::Index { index, .. }
            if env
                .processor_view_shape(&Expr::var(receiver))
                .is_some_and(|shape| shape.is_array()) =>
        {
            bound_proc_indexed_receiver_expr(receiver, index, &arrays)
        }
        _ => return None,
    };
    let value = std::mem::replace(expr, Expr::int(0));
    *stmt = Stmt::Expr {
        loc: loc.clone(),
        expr: Expr::UserCall {
            loc: loc.clone(),
            name: proc_param_setter_name(&proc_name, &param.name),
            type_args: Vec::new(),
            args: vec![
                CallArg {
                    name: None,
                    expr: receiver_expr,
                },
                CallArg {
                    name: None,
                    expr: value,
                },
            ],
        },
    };
    None
}

/// Lexical type information also exposes processor projections beneath selected
/// owners. Feed those receivers through the ordinary processor call resolver.
pub(crate) fn rewrite_processor_projection_calls(
    expression: &mut Expr,
    env: &CallTypeEnv,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_processor_projection_call_sites(expression, env, false, errors);
}

pub(crate) fn rewrite_processor_projection_statement(
    expression: &mut Expr,
    env: &CallTypeEnv,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_processor_projection_call_sites(expression, env, true, errors);
}

fn rewrite_processor_projection_call_sites(
    expression: &mut Expr,
    env: &CallTypeEnv,
    statement: bool,
    errors: &mut Vec<Diagnostic>,
) {
    let (instances, arrays) = projection_call_bindings(expression, env);
    if !instances.is_empty() {
        if statement {
            let mut stmt = Stmt::Expr {
                loc: expression.loc().span(),
                expr: std::mem::replace(expression, Expr::bool(false)),
            };
            rewrite_proc_calls_in_stmt(&mut stmt, &instances, &arrays, &env.processor_apis, errors);
            let Stmt::Expr { expr, .. } = stmt else {
                unreachable!("processor statement rewriting preserves its form")
            };
            *expression = expr;
        } else {
            rewrite_proc_calls_in_expr(
                expression,
                &instances,
                &arrays,
                &env.processor_apis,
                errors,
            );
        }
    }
}

fn projection_call_bindings(
    expression: &mut Expr,
    env: &CallTypeEnv,
) -> (HashMap<String, ProcCallInstance>, ProcArrayDeclarations) {
    expression.visit_mut_postorder(|node| {
        if let Expr::UserCall { name, args, .. } = node {
            canonicalize_indexed_proc_receiver_call(name, args, |base| {
                env.processor_view_shape(&Expr::var(base))
                    .is_some_and(|shape| shape.is_array())
            });
        }
    });
    let mut instances = HashMap::new();
    let mut arrays = ProcArrayDeclarations::new();
    for expression in expression.walk() {
        let Expr::UserCall { name, args, .. } = expression else {
            continue;
        };
        let indexed = proc_index_base_name(args);
        let receiver = indexed.unwrap_or(name);
        let resolved = env
            .processor_view_shape(&Expr::var(receiver))
            .map(|shape| (receiver, shape))
            .or_else(|| {
                let (receiver, _) = name.rsplit_once('.')?;
                env.processor_view_shape(&Expr::var(receiver))
                    .map(|shape| (receiver, shape))
            });
        let Some((receiver, shape)) = resolved else {
            continue;
        };
        if env.is_materialized_storage_view(receiver) {
            continue;
        }
        if !shape.requires_environment() && !env.is_lexical_processor_binding(receiver) {
            continue;
        }
        let Some(api) = env.processor_apis.get(&shape.proc_name) else {
            continue;
        };
        if name.starts_with(PROC_FIELD_SENTINEL_PREFIX) {
            if let Some(Expr::Var { name: field, .. }) = args
                .iter()
                .find(|arg| arg.name.as_deref() == Some(PROC_FIELD_SENTINEL_ARG))
                .map(|arg| &arg.expr)
            {
                if !api.outputs.names.contains(field)
                    && parse_proc_output_alias_index(field, api.outputs.timing).is_none()
                    && !api.params.contains_key(field)
                {
                    continue;
                }
            }
        }
        let binding = if shape.is_array() {
            if indexed.is_none() {
                continue;
            }
            let declaration = if let Some(len) = shape.array_len() {
                ProcArrayDeclaration::new(receiver.to_owned(), shape.proc_name.clone(), len, true)
            } else {
                ProcArrayDeclaration::borrowed(receiver, shape.proc_name.clone())
            };
            let binding = declaration
                .first_binding()
                .expect("processor array has a family binding")
                .clone();
            arrays.insert(receiver.to_owned(), declaration);
            binding
        } else {
            if indexed.is_some() {
                continue;
            }
            receiver.to_owned()
        };
        instances.insert(
            binding,
            projection_instance(receiver, &shape.proc_name, api),
        );
    }
    for node in expression.walk() {
        let path = match node {
            Expr::Var { name, .. }
            | Expr::Index { base: name, .. }
            | Expr::Slice { base: name, .. } => name,
            _ => continue,
        };
        let Some((receiver, _)) = split_receiver_field(path) else {
            continue;
        };
        if let Some((proc_name, api)) = parameter_projection(receiver, env) {
            instances
                .entry(receiver.to_owned())
                .or_insert_with(|| projection_instance(receiver, &proc_name, api));
        }
    }
    (instances, arrays)
}
