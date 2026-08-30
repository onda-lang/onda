use super::*;

#[derive(Default)]
struct ParsedNamedDecl {
    private: bool,
    ty: Option<DeclType>,
    ty_loc: Span,
    default: Option<Expr>,
    range: Option<DeclRange>,
    control: ParamControl,
    bind: Option<String>,
}

fn parse_port_decl_pair(
    item: Pair<'_, Rule>,
    default_ty: &Option<DeclType>,
    block_name: &str,
    allow_default_and_range: bool,
) -> Result<PortDecl, Vec<Diagnostic>> {
    let (loc, name, parsed) = parse_named_decl(
        item,
        "missing port identifier",
        "missing port declaration type",
    )?;
    if !allow_default_and_range {
        validate_decl_defaults_and_ranges(&parsed, block_name, loc.as_ref())?;
    }
    Ok(PortDecl {
        loc,
        name,
        output_timing: None,
        output_timing_loc: Span::ZERO,
        ty: parsed.ty.or_else(|| default_ty.clone()),
        ty_loc: parsed.ty_loc,
        default: parsed.default,
        range: parsed.range,
    })
}

fn parse_port_list_items(
    list_pair: Pair<'_, Rule>,
    default_ty: &Option<DeclType>,
    block_name: &str,
    allow_default_and_range: bool,
) -> Result<Vec<PortDecl>, Vec<Diagnostic>> {
    let mut ports = Vec::new();
    for item in list_pair.into_inner() {
        if item.as_rule() == Rule::port_decl {
            ports.push(parse_port_decl_pair(
                item,
                default_ty,
                block_name,
                allow_default_and_range,
            )?);
        }
    }
    Ok(ports)
}

fn parse_decl_type_item(
    pair: Pair<'_, Rule>,
    missing_type_message: &str,
) -> Result<(DeclType, Span), Vec<Diagnostic>> {
    let actual = if pair.as_rule() == Rule::decl_type {
        let loc = stmt_loc_from_pair(&pair);
        let mut decl_inner = pair.into_inner();
        decl_inner
            .next()
            .ok_or_else(|| vec![syntax_at_loc(loc.as_ref(), missing_type_message)])?
    } else {
        pair
    };
    let ty_loc = stmt_loc_from_pair(&actual);
    Ok((parse_decl_type(actual)?, ty_loc))
}

fn parse_named_decl(
    pair: Pair<'_, Rule>,
    missing_name_message: &str,
    missing_type_message: &str,
) -> Result<(Span, String, ParsedNamedDecl), Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut parsed = ParsedNamedDecl::default();
    let mut name: Option<String> = None;
    for item in pair.into_inner() {
        match item.as_rule() {
            Rule::param_private => parsed.private = true,
            Rule::ident if name.is_none() => {
                name = Some(item.as_str().to_owned());
            }
            Rule::decl_type
            | Rule::type_name
            | Rule::array_type
            | Rule::tuple_type
            | Rule::namespace_ref
            | Rule::qualified_ident => {
                let (ty, ty_loc) = parse_decl_type_item(item, missing_type_message)?;
                parsed.ty = Some(ty);
                parsed.ty_loc = ty_loc;
            }
            Rule::expr => parsed.default = Some(parse_expr_inner(item)),
            Rule::decl_range => parsed.range = Some(parse_decl_range_pair(item)?),
            Rule::param_domain => {
                let (range, control) = parse_param_domain_pair(item)?;
                parsed.range = Some(range);
                parsed.control = control;
            }
            Rule::param_bind => {
                if let Some(bind_pair) = item.into_inner().next() {
                    parsed.bind = Some(bind_pair.as_str().to_owned());
                }
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![syntax_at_loc(loc.as_ref(), missing_name_message)]);
    };
    Ok((loc, name, parsed))
}

fn validate_decl_defaults_and_ranges(
    parsed: &ParsedNamedDecl,
    block_name: &str,
    loc: impl Into<SourceLoc>,
) -> Result<(), Vec<Diagnostic>> {
    let loc = loc.into();
    if parsed.default.is_some() {
        return Err(vec![syntax_at_loc(
            loc,
            format!("{block_name} declarations do not support default values"),
        )]);
    }
    if parsed.range.is_some() {
        return Err(vec![syntax_at_loc(
            loc,
            format!("{block_name} declarations do not support ranges"),
        )]);
    }
    Ok(())
}

fn parse_section_count_inner(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    match pair.as_rule() {
        Rule::int_lit => Ok(Expr::int(parse_int(pair.as_str())? as i64).with_loc(loc)),
        Rule::path_ident | Rule::namespace_ref => {
            Ok(Expr::var(pair_symbol_text(&pair)).with_loc(loc))
        }
        Rule::section_count => {
            let mut inner = pair.into_inner();
            let Some(inner_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing section count expression",
                )]);
            };
            match inner_pair.as_rule() {
                Rule::expr => parse_expr(inner_pair),
                _ => parse_section_count_inner(inner_pair),
            }
        }
        Rule::expr => parse_expr(pair),
        _ => Err(vec![syntax_at_loc(
            loc.as_ref(),
            "section count must be an integer literal, constant name, or parenthesized expression",
        )]),
    }
}

pub(super) fn parse_port_block(block_pair: Pair<'_, Rule>) -> Result<PortBlock, Vec<Diagnostic>> {
    let block_loc = stmt_loc_from_pair(&block_pair);
    let block_rule = block_pair.as_rule();
    let (block_name, prefix, output_timing, output_timing_loc) = match block_rule {
        Rule::ins_block => ("ins", "in", OutputTiming::Sample, Span::ZERO),
        Rule::outs_block => ("outs", "out", OutputTiming::Sample, Span::ZERO),
        Rule::kouts_block => ("kouts", "kout", OutputTiming::Block, block_loc),
        _ => ("ports", "port", OutputTiming::Sample, Span::ZERO),
    };
    let allow_default_and_range = block_rule == Rule::ins_block;

    let mut ports = Vec::new();
    let mut deferred_count: Option<Expr> = None;
    let mut default_ty: Option<DeclType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_decl_type => {
                default_ty = Some(parse_section_default_decl_type(child, block_name)?);
            }
            Rule::section_count => {
                if block_rule == Rule::outs_block && matches!(child.as_str(), "block" | "sample") {
                    return Err(vec![syntax_at_pair(
                        &child,
                        "outs does not accept rate words; use kouts for control-rate outputs",
                    )]);
                }
                if deferred_count.is_some() {
                    return Err(vec![syntax_at_pair(
                        &child,
                        format!("{block_name} block count can only be specified once"),
                    )]);
                }
                deferred_count = Some(parse_section_count_inner(child)?);
            }
            Rule::port_list => {
                ports.extend(parse_port_list_items(
                    child,
                    &default_ty,
                    block_name,
                    allow_default_and_range,
                )?);
            }
            _ => {}
        }
    }

    let deferred_default_ty = if deferred_count.is_some() {
        default_ty
    } else {
        None
    };

    Ok(PortBlock {
        loc: block_loc,
        decls: ports,
        deferred_count,
        deferred_default_ty,
        deferred_prefix: prefix.to_owned(),
        output_timing,
        output_timing_loc,
    })
}

pub(super) fn parse_params_block(
    block_pair: Pair<'_, Rule>,
) -> Result<ParamBlock, Vec<Diagnostic>> {
    let block_loc = stmt_loc_from_pair(&block_pair);
    let (block_name, prefix) = match block_pair.as_rule() {
        Rule::kins_block => ("kins", "kin"),
        _ => ("params", "param"),
    };
    let mut params = Vec::new();
    let mut deferred_count: Option<Expr> = None;
    let mut default_ty: Option<DeclType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_decl_type => {
                default_ty = Some(parse_section_default_decl_type(child, block_name)?);
            }
            Rule::section_count => {
                if deferred_count.is_some() {
                    return Err(vec![syntax_at_pair(
                        &child,
                        format!("{block_name} block count can only be specified once"),
                    )]);
                }
                deferred_count = Some(parse_section_count_inner(child)?);
            }
            Rule::param_list | Rule::proc_param_list => {
                for param_pair in child.into_inner() {
                    if !matches!(
                        param_pair.as_rule(),
                        Rule::param_decl | Rule::proc_param_decl
                    ) {
                        continue;
                    }
                    let (loc, name, parsed) = parse_named_decl(
                        param_pair,
                        "missing param identifier",
                        "missing param declaration type",
                    )?;
                    let ty = parsed.ty.or_else(|| default_ty.clone());
                    params.push(ParamDecl {
                        loc,
                        name,
                        private: parsed.private,
                        ty,
                        ty_loc: parsed.ty_loc,
                        default: parsed.default,
                        range: parsed.range,
                        control: parsed.control,
                        bind: parsed.bind,
                    });
                }
            }
            _ => {}
        }
    }

    let deferred_default_ty = if deferred_count.is_some() {
        default_ty
    } else {
        None
    };

    Ok(ParamBlock {
        loc: block_loc,
        decls: params,
        deferred_count,
        deferred_default_ty,
        deferred_prefix: prefix.to_owned(),
    })
}

pub(super) fn parse_buffers_block(
    block_pair: Pair<'_, Rule>,
) -> Result<BufferBlock, Vec<Diagnostic>> {
    let block_loc = stmt_loc_from_pair(&block_pair);
    let mut out = Vec::<BufferDecl>::new();
    let mut seen = HashSet::<String>::new();

    let mut deferred_count: Option<Expr> = None;
    let mut default_ty: Option<BufferType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_buffer_type => {
                default_ty = Some(parse_section_default_buffer_type(child, "buffers")?);
            }
            Rule::section_count => {
                if deferred_count.is_some() {
                    return Err(vec![syntax_at_pair(
                        &child,
                        "buffers block count can only be specified once",
                    )]);
                }
                deferred_count = Some(parse_section_count_inner(child)?);
            }
            Rule::buffer_list => {
                if deferred_count.is_some() {
                    return Err(vec![syntax_at_pair(
                        &child,
                        "buffers block cannot mix explicit declarations and count shorthand",
                    )]);
                }
                for item in child.into_inner() {
                    if item.as_rule() != Rule::buffer_decl {
                        continue;
                    }
                    let loc = stmt_loc_from_pair(&item);
                    let mut inner = item.into_inner();
                    let Some(name_pair) = inner.next() else {
                        return Err(vec![syntax_at_loc(
                            loc.as_ref(),
                            "missing buffer identifier",
                        )]);
                    };
                    let name = name_pair.as_str().to_owned();
                    let mut ty = default_ty.clone();
                    let mut ty_loc = Span::ZERO;
                    let mut array_size = None;
                    for declaration_part in inner {
                        match declaration_part.as_rule() {
                            Rule::buffer_decl_type => {
                                ty_loc = stmt_loc_from_pair(&declaration_part);
                                ty = Some(parse_buffer_decl_type(declaration_part)?);
                            }
                            Rule::buffer_count => {
                                array_size = Some(parse_buffer_count(declaration_part)?);
                            }
                            _ => {}
                        }
                    }
                    if !seen.insert(name.clone()) {
                        return Err(vec![syntax_at_loc(
                            loc.as_ref(),
                            format!("duplicate buffer declaration '{name}'"),
                        )]);
                    }
                    out.push(BufferDecl {
                        loc,
                        name,
                        ty,
                        ty_loc,
                        array_size,
                    });
                }
            }
            _ => {}
        }
    }

    let deferred_default_ty = if deferred_count.is_some() {
        default_ty
    } else {
        None
    };

    Ok(BufferBlock {
        loc: block_loc,
        decls: out,
        deferred_count,
        deferred_default_ty,
    })
}

pub(super) fn parse_events_block(
    block_pair: Pair<'_, Rule>,
) -> Result<EventBlock, Vec<Diagnostic>> {
    let block_loc = stmt_loc_from_pair(&block_pair);
    let mut events = Vec::<EventDef>::new();

    for child in block_pair.into_inner() {
        if child.as_rule() != Rule::event_list {
            continue;
        }
        for item in child.into_inner() {
            if item.as_rule() != Rule::event_decl {
                continue;
            }
            merge_event_defs(&mut events, vec![parse_event_decl(item)?])?;
        }
    }

    Ok(EventBlock {
        loc: block_loc,
        events,
    })
}

pub(super) fn parse_event_block(block_pair: Pair<'_, Rule>) -> Result<EventBlock, Vec<Diagnostic>> {
    if block_pair.as_rule() != Rule::event_block {
        return Err(vec![syntax_at_pair(
            &block_pair,
            "internal parser error: expected event block",
        )]);
    }

    let block_loc = stmt_loc_from_pair(&block_pair);
    let mut inner = block_pair.into_inner();
    let Some(event_decl) = inner.next() else {
        return Err(vec![syntax_at_loc(
            block_loc.as_ref(),
            "missing event declaration",
        )]);
    };

    Ok(EventBlock {
        loc: block_loc,
        events: vec![parse_event_decl(event_decl)?],
    })
}

pub(super) fn merge_event_defs(
    existing: &mut Vec<EventDef>,
    incoming: Vec<EventDef>,
) -> Result<(), Vec<Diagnostic>> {
    let mut seen = existing
        .iter()
        .map(|event| event.name.clone())
        .collect::<HashSet<_>>();
    for event in incoming {
        if !seen.insert(event.name.clone()) {
            return Err(vec![syntax_at_loc(
                event.loc.as_ref(),
                format!("duplicate event declaration '{}'", event.name),
            )]);
        }
        existing.push(event);
    }
    Ok(())
}

fn parse_event_decl(item: Pair<'_, Rule>) -> Result<EventDef, Vec<Diagnostic>> {
    let event_loc = stmt_loc_from_pair(&item);
    let mut name: Option<String> = None;
    let mut params = Vec::<EventParamDecl>::new();
    let mut body = None;
    for part in item.into_inner() {
        match part.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(part.as_str().to_owned());
                }
            }
            Rule::event_param_list => params = parse_event_params(part)?,
            Rule::stmt_block => {
                body = Some(parse_stmt_block(part)?);
            }
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![syntax_at_loc(
            event_loc.as_ref(),
            "missing event name",
        )]);
    };
    let Some(body) = body else {
        return Err(vec![syntax_at_loc(
            event_loc.as_ref(),
            format!("missing handler body for event '{name}'"),
        )]);
    };
    Ok(EventDef {
        loc: event_loc,
        name,
        params,
        body,
    })
}

pub(super) fn parse_delegates_block(
    block_pair: Pair<'_, Rule>,
) -> Result<DelegateBlock, Vec<Diagnostic>> {
    let block_loc = stmt_loc_from_pair(&block_pair);
    let mut delegates = Vec::new();
    for child in block_pair.into_inner() {
        if child.as_rule() != Rule::delegate_list {
            continue;
        }
        for item in child.into_inner() {
            if item.as_rule() == Rule::delegate_decl {
                merge_delegate_defs(&mut delegates, vec![parse_delegate_decl(item)?])?;
            }
        }
    }
    Ok(DelegateBlock {
        loc: block_loc,
        delegates,
    })
}

pub(super) fn parse_delegate_block(
    block_pair: Pair<'_, Rule>,
) -> Result<DelegateBlock, Vec<Diagnostic>> {
    let block_loc = stmt_loc_from_pair(&block_pair);
    let Some(decl) = block_pair.into_inner().next() else {
        return Err(vec![syntax_at_loc(
            block_loc.as_ref(),
            "missing delegate declaration",
        )]);
    };
    Ok(DelegateBlock {
        loc: block_loc,
        delegates: vec![parse_delegate_decl(decl)?],
    })
}

pub(super) fn merge_delegate_defs(
    existing: &mut Vec<DelegateDef>,
    incoming: Vec<DelegateDef>,
) -> Result<(), Vec<Diagnostic>> {
    let mut seen = existing
        .iter()
        .map(|delegate| delegate.name.clone())
        .collect::<HashSet<_>>();
    for delegate in incoming {
        if !seen.insert(delegate.name.clone()) {
            return Err(vec![syntax_at_loc(
                delegate.loc.as_ref(),
                format!("duplicate delegate declaration '{}'", delegate.name),
            )]);
        }
        existing.push(delegate);
    }
    Ok(())
}

fn parse_delegate_decl(item: Pair<'_, Rule>) -> Result<DelegateDef, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&item);
    let mut name = None;
    let mut params = Vec::new();
    for part in item.into_inner() {
        match part.as_rule() {
            Rule::ident if name.is_none() => name = Some(part.as_str().to_owned()),
            Rule::event_param_list => params = parse_event_params(part)?,
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing delegate name")]);
    };
    Ok(DelegateDef { loc, name, params })
}

fn parse_event_params(list: Pair<'_, Rule>) -> Result<Vec<EventParamDecl>, Vec<Diagnostic>> {
    let mut params = Vec::new();
    for event_param in list.into_inner() {
        if event_param.as_rule() != Rule::event_param_decl {
            continue;
        }
        let param_loc = stmt_loc_from_pair(&event_param);
        let mut param_inner = event_param.into_inner();
        let Some(param_name_pair) = param_inner.next() else {
            return Err(vec![syntax_at_loc(
                param_loc.as_ref(),
                "missing event parameter name",
            )]);
        };
        let mut ty = EventParamType::Scalar(PrimitiveType::F32);
        let mut ty_loc = Span::ZERO;
        let mut default = None;
        for item in param_inner {
            match item.as_rule() {
                Rule::event_param_type => {
                    ty_loc = stmt_loc_from_pair(&item);
                    ty = parse_event_param_type(item)?;
                }
                Rule::expr => default = Some(parse_expr_inner(item)),
                _ => {}
            }
        }
        params.push(EventParamDecl {
            loc: param_loc,
            name: param_name_pair.as_str().to_owned(),
            ty,
            ty_loc,
            default,
        });
    }
    Ok(params)
}

pub(super) fn parse_when_block(block_pair: Pair<'_, Rule>) -> Result<WhenDef, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&block_pair);
    let mut target = None;
    let mut bindings = Vec::new();
    let mut body = None;
    for part in block_pair.into_inner() {
        match part.as_rule() {
            Rule::when_target => target = Some(parse_when_target(part)?),
            Rule::when_binding_list => {
                bindings = part
                    .into_inner()
                    .filter(|binding| binding.as_rule() == Rule::when_binding)
                    .filter_map(|binding| binding.into_inner().next())
                    .map(|name| WhenBinding {
                        loc: stmt_loc_from_pair(&name),
                        name: name.as_str().to_owned(),
                    })
                    .collect();
            }
            Rule::stmt_block => body = Some(parse_stmt_block(part)?),
            _ => {}
        }
    }
    Ok(WhenDef {
        loc,
        target: target.ok_or_else(|| {
            vec![syntax_at_loc(
                loc.as_ref(),
                "missing delegate target in when handler",
            )]
        })?,
        bindings,
        body: body.ok_or_else(|| vec![syntax_at_loc(loc.as_ref(), "missing when handler body")])?,
    })
}

fn parse_when_target(pair: Pair<'_, Rule>) -> Result<WhenTarget, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let Some(target) = pair.into_inner().next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing delegate target")]);
    };
    match target.as_rule() {
        Rule::path_ident => {
            let mut path = target
                .as_str()
                .split('.')
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let delegate = path.pop().expect("path_ident is nonempty");
            Ok(WhenTarget {
                loc,
                receiver: path,
                index: None,
                delegate,
            })
        }
        Rule::when_indexed_target => {
            let mut receiver = None;
            let mut index = None;
            let mut delegate = None;
            for part in target.into_inner() {
                match part.as_rule() {
                    Rule::ident if receiver.is_none() => receiver = Some(part.as_str().to_owned()),
                    Rule::ident => delegate = Some(part.as_str().to_owned()),
                    Rule::expr => index = Some(parse_expr_inner(part)),
                    _ => {}
                }
            }
            Ok(WhenTarget {
                loc,
                receiver: vec![receiver.ok_or_else(|| {
                    vec![syntax_at_loc(loc.as_ref(), "missing indexed when receiver")]
                })?],
                index,
                delegate: delegate.ok_or_else(|| {
                    vec![syntax_at_loc(loc.as_ref(), "missing indexed when delegate")]
                })?,
            })
        }
        _ => Err(vec![syntax_at_loc(loc.as_ref(), "invalid when target")]),
    }
}

pub(super) fn parse_tasks_block(block_pair: Pair<'_, Rule>) -> Result<TaskBlock, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&block_pair);
    let mut tasks = Vec::new();
    for child in block_pair.into_inner() {
        if child.as_rule() != Rule::task_list {
            continue;
        }
        for item in child.into_inner() {
            if item.as_rule() == Rule::task_decl {
                merge_task_defs(&mut tasks, vec![parse_task_decl(item)?])?;
            }
        }
    }
    Ok(TaskBlock { loc, tasks })
}

pub(super) fn parse_task_block(block_pair: Pair<'_, Rule>) -> Result<TaskBlock, Vec<Diagnostic>> {
    let block_loc = stmt_loc_from_pair(&block_pair);
    let Some(task_decl) = block_pair
        .into_inner()
        .find(|part| part.as_rule() == Rule::task_decl)
    else {
        return Err(vec![syntax_at_loc(
            block_loc.as_ref(),
            "missing task declaration",
        )]);
    };
    Ok(TaskBlock {
        loc: block_loc,
        tasks: vec![parse_task_decl(task_decl)?],
    })
}

pub(super) fn merge_task_defs(
    existing: &mut Vec<TaskDef>,
    incoming: Vec<TaskDef>,
) -> Result<(), Vec<Diagnostic>> {
    let mut seen = existing
        .iter()
        .map(|task| task.name.clone())
        .collect::<HashSet<_>>();
    for task in incoming {
        if !seen.insert(task.name.clone()) {
            return Err(vec![syntax_at_loc(
                task.loc.as_ref(),
                format!("duplicate task declaration '{}'", task.name),
            )]);
        }
        existing.push(task);
    }
    Ok(())
}

fn parse_task_decl(item: Pair<'_, Rule>) -> Result<TaskDef, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&item);
    let mut name = None;
    let mut body = None;
    for part in item.into_inner() {
        match part.as_rule() {
            Rule::ident if name.is_none() => name = Some(part.as_str().to_owned()),
            Rule::stmt_block => body = Some(parse_stmt_block(part)?),
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing task name")]);
    };
    let Some(body) = body else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            format!("missing body for task '{name}'"),
        )]);
    };
    Ok(TaskDef { loc, name, body })
}

pub(super) fn parse_graph_block(block_pair: Pair<'_, Rule>) -> Result<GraphBlock, Vec<Diagnostic>> {
    if block_pair.as_rule() != Rule::graph_block {
        return Err(vec![syntax_at_pair(
            &block_pair,
            "internal parser error: expected graph block",
        )]);
    }

    let loc = stmt_loc_from_pair(&block_pair);
    let mut edges = Vec::<GraphEdge>::new();
    for child in block_pair.into_inner() {
        if child.as_rule() != Rule::graph_edge_list {
            continue;
        }
        for edge_pair in child.into_inner() {
            if edge_pair.as_rule() != Rule::graph_edge {
                continue;
            }
            edges.push(parse_graph_edge(edge_pair)?);
        }
    }

    Ok(GraphBlock { loc, edges })
}

fn parse_graph_edge(edge_pair: Pair<'_, Rule>) -> Result<GraphEdge, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&edge_pair);
    let mut rate = None::<GraphRate>;
    let mut source = None::<Expr>;
    let mut delay = None::<Expr>;
    let mut dests = None::<Vec<GraphEndpoint>>;

    for child in edge_pair.into_inner() {
        match child.as_rule() {
            Rule::graph_rate => {
                if rate.is_some() {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "graph edge rate can only be specified once",
                    )]);
                }
                rate = Some(parse_graph_rate(child)?);
            }
            Rule::graph_send_edge => {
                let mut inner = child.into_inner();
                let Some(source_pair) = inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing graph edge source expression",
                    )]);
                };
                source = Some(parse_expr(source_pair)?);
                let Some(arrow_pair) = inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing graph edge arrow",
                    )]);
                };
                delay = parse_graph_edge_delay(arrow_pair)?;
                let Some(dest_pair) = inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing graph edge destination endpoint",
                    )]);
                };
                dests = Some(parse_graph_edge_targets(dest_pair)?);
            }
            Rule::graph_recv_edge => {
                let mut inner = child.into_inner();
                let Some(dest_pair) = inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing graph edge destination endpoint",
                    )]);
                };
                dests = Some(parse_graph_edge_targets(dest_pair)?);
                let Some(arrow_pair) = inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing graph edge arrow",
                    )]);
                };
                delay = parse_graph_edge_delay(arrow_pair)?;
                let Some(source_pair) = inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing graph edge source expression",
                    )]);
                };
                source = Some(parse_expr(source_pair)?);
            }
            _ => {}
        }
    }

    let Some(source) = source else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing graph edge source expression",
        )]);
    };
    let Some(dests) = dests else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing graph edge destination endpoint",
        )]);
    };

    Ok(GraphEdge {
        loc,
        rate,
        source,
        delay,
        dests,
    })
}

fn parse_graph_edge_delay(arrow_pair: Pair<'_, Rule>) -> Result<Option<Expr>, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&arrow_pair);
    let mut delay = None::<Expr>;
    for part in arrow_pair.into_inner() {
        if part.as_rule() == Rule::graph_delay {
            let mut delay_inner = part.into_inner();
            let Some(expr_pair) = delay_inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing graph edge delay expression",
                )]);
            };
            delay = Some(parse_expr(expr_pair)?);
        }
    }
    Ok(delay)
}

fn parse_graph_rate(pair: Pair<'_, Rule>) -> Result<GraphRate, Vec<Diagnostic>> {
    match pair.as_str() {
        "@block" => Ok(GraphRate::Block),
        "@sample" => Ok(GraphRate::Sample),
        _ => Err(vec![syntax_at_pair(
            &pair,
            "unknown graph edge rate annotation",
        )]),
    }
}

fn parse_graph_endpoint(pair: Pair<'_, Rule>) -> Result<GraphEndpoint, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(first) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing graph destination endpoint",
        )]);
    };

    match first.as_rule() {
        Rule::path_ident => {
            let path = pair_symbol_text(&first);
            if let Some((proc, field)) = path.rsplit_once('.') {
                Ok(GraphEndpoint::ProcField {
                    loc,
                    proc: proc.to_owned(),
                    field: field.to_owned(),
                })
            } else {
                Ok(GraphEndpoint::Symbol { loc, name: path })
            }
        }
        Rule::indexed_graph_endpoint => {
            let mut endpoint_inner = first.into_inner();
            let proc = endpoint_inner
                .next()
                .ok_or_else(|| {
                    vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing graph proc endpoint base",
                    )]
                })?
                .as_str()
                .to_owned();
            let index = parse_expr(endpoint_inner.next().ok_or_else(|| {
                vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing graph proc endpoint index",
                )]
            })?)?;
            let field = endpoint_inner
                .next()
                .ok_or_else(|| {
                    vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing graph proc endpoint field",
                    )]
                })?
                .as_str()
                .to_owned();
            Ok(GraphEndpoint::ProcIndexedField {
                loc,
                proc,
                index,
                field,
            })
        }
        _ => Err(vec![syntax_at_loc(
            loc.as_ref(),
            "invalid graph destination endpoint",
        )]),
    }
}

fn parse_graph_edge_targets(pair: Pair<'_, Rule>) -> Result<Vec<GraphEndpoint>, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::graph_edge_targets => {
            let loc = stmt_loc_from_pair(&pair);
            let mut inner = pair.into_inner();
            let Some(targets) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing graph edge destination endpoint",
                )]);
            };
            parse_graph_edge_targets(targets)
        }
        Rule::graph_endpoint_set => {
            let mut out = Vec::new();
            for part in pair.into_inner() {
                if part.as_rule() == Rule::graph_endpoint {
                    out.push(parse_graph_endpoint(part)?);
                }
            }
            Ok(out)
        }
        Rule::graph_endpoint => Ok(vec![parse_graph_endpoint(pair)?]),
        _ => Err(vec![syntax_at_pair(
            &pair,
            "invalid graph destination endpoint list",
        )]),
    }
}

pub(super) fn parse_proc_block(
    block_pair: Pair<'_, Rule>,
) -> Result<ProcessorDef, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&block_pair);
    let mut name: Option<String> = None;
    let mut type_params = Vec::new();
    let mut consts = Vec::new();
    let mut ins = Vec::new();
    let mut ins_deferred_count: Option<Expr> = None;
    let mut ins_deferred_default_ty: Option<DeclType> = None;
    let mut outs = Vec::new();
    let mut outs_deferred_count: Option<Expr> = None;
    let mut outs_deferred_default_ty: Option<DeclType> = None;
    let mut seen_output_block = false;
    let mut outs_timing = OutputTiming::Sample;
    let mut outs_timing_loc = Span::ZERO;
    let mut params = Vec::new();
    let mut params_deferred_count: Option<Expr> = None;
    let mut params_deferred_default_ty: Option<DeclType> = None;
    let mut events = Vec::<EventDef>::new();
    let mut delegates = Vec::<DelegateDef>::new();
    let mut whens = Vec::<WhenDef>::new();
    let mut tasks = Vec::<TaskDef>::new();
    let mut buffers = Vec::new();
    let mut buffers_deferred_count: Option<Expr> = None;
    let mut buffers_deferred_default_ty: Option<BufferType> = None;
    let mut init: Option<InitBlock> = None;
    let mut block_exec: Option<BlockExec> = None;
    let mut sample: Option<SampleBlock> = None;
    let mut graph: Option<GraphBlock> = None;
    let mut local_defs = Vec::new();

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::generic_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::ident {
                        type_params.push(item.as_str().to_owned());
                    }
                }
            }
            Rule::const_block => {
                let child_loc = stmt_loc_from_pair(&child);
                let mut inner = child.into_inner();
                let decl = inner.next().ok_or_else(|| {
                    vec![syntax_at_loc(
                        child_loc.as_ref(),
                        "missing const declaration",
                    )]
                })?;
                consts.push(parse_const_decl(decl)?);
            }
            Rule::ins_block => {
                if !ins.is_empty() || ins_deferred_count.is_some() {
                    return Err(vec![syntax_at_pair(&child, "duplicate proc ins block")]);
                }
                let pb = parse_port_block(child)?;
                ins = pb.decls;
                ins_deferred_count = pb.deferred_count;
                ins_deferred_default_ty = pb.deferred_default_ty;
            }
            Rule::outs_block | Rule::kouts_block => {
                if seen_output_block {
                    return Err(vec![syntax_at_pair(&child, "duplicate proc output block")]);
                }
                seen_output_block = true;
                let pb = parse_port_block(child)?;
                outs = pb.decls;
                outs_deferred_count = pb.deferred_count;
                outs_deferred_default_ty = pb.deferred_default_ty;
                outs_timing = pb.output_timing;
                outs_timing_loc = pb.output_timing_loc;
            }
            Rule::params_block | Rule::proc_params_block => {
                if !params.is_empty() || params_deferred_count.is_some() {
                    return Err(vec![syntax_at_pair(&child, "duplicate proc params block")]);
                }
                let pb = parse_params_block(child)?;
                params = pb.decls;
                params_deferred_count = pb.deferred_count;
                params_deferred_default_ty = pb.deferred_default_ty;
            }
            Rule::events_block => {
                let block = parse_events_block(child)?;
                merge_event_defs(&mut events, block.events)?;
            }
            Rule::event_block => {
                let block = parse_event_block(child)?;
                merge_event_defs(&mut events, block.events)?;
            }
            Rule::delegates_block => {
                let block = parse_delegates_block(child)?;
                merge_delegate_defs(&mut delegates, block.delegates)?;
            }
            Rule::delegate_block => {
                let block = parse_delegate_block(child)?;
                merge_delegate_defs(&mut delegates, block.delegates)?;
            }
            Rule::when_block => whens.push(parse_when_block(child)?),
            Rule::tasks_block => {
                let parsed = parse_tasks_block(child)?;
                merge_task_defs(&mut tasks, parsed.tasks)?;
            }
            Rule::task_block => {
                let parsed = parse_task_block(child)?;
                merge_task_defs(&mut tasks, parsed.tasks)?;
            }
            Rule::buffers_block => {
                if !buffers.is_empty() || buffers_deferred_count.is_some() {
                    return Err(vec![syntax_at_pair(&child, "duplicate proc buffers block")]);
                }
                let bb = parse_buffers_block(child)?;
                buffers = bb.decls;
                buffers_deferred_count = bb.deferred_count;
                buffers_deferred_default_ty = bb.deferred_default_ty;
            }
            Rule::init_block => {
                if init.is_some() {
                    return Err(vec![syntax_at_pair(&child, "duplicate proc init block")]);
                }
                init = Some(parse_exec_block(child)?);
            }
            Rule::block_exec_block => {
                if block_exec.is_some() {
                    return Err(vec![syntax_at_pair(&child, "duplicate proc block section")]);
                }
                block_exec = Some(parse_block_exec_block(child)?);
            }
            Rule::sample_block => {
                if sample.is_some() {
                    return Err(vec![syntax_at_pair(&child, "duplicate proc sample block")]);
                }
                sample = Some(parse_sample_block(child)?);
            }
            Rule::graph_block => {
                if graph.is_some() {
                    return Err(vec![syntax_at_pair(&child, "duplicate proc graph block")]);
                }
                graph = Some(parse_graph_block(child)?);
            }
            Rule::def_block => {
                let def = parse_def_block(child)?;
                if def.is_const {
                    return Err(vec![syntax_at_loc(
                        def.loc.as_ref(),
                        "const defs are only supported at top-level and namespace scope",
                    )]);
                }
                local_defs.push(def);
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing proc name")]);
    };

    let mut block_pre = Vec::new();
    let mut block_post = Vec::new();
    let mut has_block_block = false;
    if let Some(exec) = block_exec {
        has_block_block = true;
        if sample.is_some() {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                "proc sample block cannot be declared both directly and inside block section",
            )]);
        }
        block_pre = exec.pre;
        block_post = exec.post;
        sample = exec.sample;
    }

    if graph.is_some() && (sample.is_some() || has_block_block) {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "proc graph block cannot be declared with sample or block",
        )]);
    }

    let has_sample_block = sample.is_some();
    let has_graph_block = graph.is_some();
    let (sample_oversample_factor, sample_body) = if let Some(sample_block) = sample {
        (sample_block.oversample_factor, sample_block.body)
    } else {
        (None, Vec::new())
    };

    Ok(ProcessorDef {
        loc,
        name,
        type_params,
        consts,
        ins,
        ins_deferred_count,
        ins_deferred_default_ty,
        outs,
        outs_deferred_count,
        outs_deferred_default_ty,
        outs_timing,
        outs_timing_loc,
        params,
        params_deferred_count,
        params_deferred_default_ty,
        events,
        delegates,
        whens,
        tasks,
        buffers,
        buffers_deferred_count,
        buffers_deferred_default_ty,
        has_init_block: init.is_some(),
        has_block_block,
        has_sample_block,
        has_graph_block,
        sample_oversample_factor,
        init: init.unwrap_or(InitBlock {
            loc: Span::ZERO,
            default_ty: None,
            default_ty_loc: Span::ZERO,
            pinned_roots: Vec::new(),
            compiler_scratch_roots: Vec::new(),
            body: Vec::new(),
        }),
        block_pre,
        sample: sample_body,
        block_post,
        graph,
        local_defs,
    })
}

pub(super) fn parse_struct_block(block_pair: Pair<'_, Rule>) -> Result<StructDef, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&block_pair);
    let mut name: Option<String> = None;
    let mut type_params = Vec::new();
    let mut fields = Vec::new();
    let mut methods = Vec::new();

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::generic_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::ident {
                        type_params.push(item.as_str().to_owned());
                    }
                }
            }
            Rule::field_list => {
                for item in child.into_inner() {
                    if item.as_rule() != Rule::field_decl {
                        continue;
                    }
                    let field_loc = stmt_loc_from_pair(&item);
                    let mut decl_inner = item.into_inner();
                    let Some(field_name) = decl_inner.next() else {
                        return Err(vec![syntax_at_loc(
                            field_loc.as_ref(),
                            "missing struct field name",
                        )]);
                    };
                    let mut parsed_ty = None::<FieldType>;
                    let mut ty_loc = Span::ZERO;
                    let mut default = None;
                    let mut range = None;
                    for part in decl_inner {
                        match part.as_rule() {
                            Rule::field_type => {
                                ty_loc = stmt_loc_from_pair(&part);
                                parsed_ty = Some(parse_field_type(part)?);
                            }
                            Rule::expr => {
                                default = Some(parse_expr_inner(part));
                            }
                            Rule::binding_range => {
                                range = parse_binding_range_pair(part)?.range;
                            }
                            _ => {}
                        }
                    }
                    let ty = if let Some(explicit) = parsed_ty {
                        explicit
                    } else if let Some(ref default_expr) = default {
                        FieldType::Scalar(infer_struct_field_scalar_type_from_default(default_expr))
                    } else {
                        FieldType::Scalar(PrimitiveType::F32)
                    };
                    if range.is_some()
                        && !matches!(
                            ty,
                            FieldType::Scalar(PrimitiveType::I32 | PrimitiveType::I64)
                        )
                    {
                        return Err(vec![syntax_at_loc(
                            field_loc.as_ref(),
                            "binding ranges require an i32 or i64 struct field",
                        )]);
                    }
                    if let Some((func, lower, upper)) = range {
                        let Some(value) = default.take() else {
                            return Err(vec![syntax_at_loc(
                                field_loc.as_ref(),
                                "ranged struct fields require a default expression",
                            )]);
                        };
                        default = Some(Expr::Call {
                            loc: field_loc,
                            func,
                            args: vec![value, lower, upper],
                        });
                    }
                    fields.push(StructField {
                        loc: field_loc,
                        name: field_name.as_str().to_owned(),
                        ty,
                        ty_loc,
                        default,
                    });
                }
            }
            Rule::struct_method_list => {
                for item in child.into_inner() {
                    if item.as_rule() != Rule::struct_method_decl {
                        continue;
                    }
                    methods.push(parse_struct_method_decl(item)?);
                }
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing struct name")]);
    };
    Ok(StructDef {
        loc,
        name,
        type_params,
        fields,
        methods,
    })
}

pub(super) fn infer_struct_field_scalar_type_from_default(expr: &Expr) -> PrimitiveType {
    infer_expr_primitive_type(expr).unwrap_or(PrimitiveType::F32)
}

pub(super) fn infer_expr_primitive_type(expr: &Expr) -> Option<PrimitiveType> {
    match expr {
        Expr::Int { value, .. } => Some(if i32::try_from(*value).is_ok() {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Compare { .. } | Expr::Logical { .. } | Expr::UnaryNot { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr, .. } => {
            let inner = infer_expr_primitive_type(expr)?;
            merge_inferred_integer_type(inner, inner)
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let left = infer_expr_primitive_type(lhs)?;
            let right = infer_expr_primitive_type(rhs)?;
            match op {
                BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight => merge_inferred_integer_type(left, right),
                _ => merge_inferred_numeric_type(left, right),
            }
        }
        _ => None,
    }
}

pub(super) fn merge_inferred_numeric_type(
    a: PrimitiveType,
    b: PrimitiveType,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (a, b) {
        (Bool, _) | (_, Bool) => None,
        (F64, _) | (_, F64) => Some(F64),
        (F32, _) | (_, F32) => Some(F32),
        (I64, _) | (_, I64) => Some(I64),
        (I32, I32) => Some(I32),
    }
}

pub(super) fn merge_inferred_integer_type(
    a: PrimitiveType,
    b: PrimitiveType,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (a, b) {
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => None,
    }
}

pub(super) fn parse_def_block(block_pair: Pair<'_, Rule>) -> Result<FunctionDef, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&block_pair);
    let is_const = block_pair.as_str().trim_start().starts_with("const");
    let mut name: Option<String> = None;
    let mut type_params = Vec::new();
    let mut params = Vec::new();
    let mut return_ty = None;
    let mut return_ty_loc = Span::ZERO;
    let mut body = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::generic_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::ident {
                        type_params.push(item.as_str().to_owned());
                    }
                }
            }
            Rule::fn_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::fn_param_decl {
                        params.push(parse_fn_param_decl(item, "function")?);
                    }
                }
            }
            Rule::fn_return_type => {
                return_ty_loc = stmt_loc_from_pair(&child);
                return_ty = Some(parse_fn_return_type(child)?);
            }
            Rule::stmt_block => {
                body = Some(parse_stmt_block(child)?);
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing function name")]);
    };
    let Some(body) = body else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing function body")]);
    };

    Ok(FunctionDef {
        loc,
        name,
        is_const,
        type_params,
        params,
        return_ty,
        return_ty_loc,
        body,
    })
}

pub(super) fn parse_fn_param_decl(
    pair: Pair<'_, Rule>,
    context: &str,
) -> Result<FnParamDecl, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(name_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            format!("missing {context} parameter name"),
        )]);
    };

    let mut ty = None;
    let mut ty_loc = Span::ZERO;
    let mut default = None;
    for item in inner {
        match item.as_rule() {
            Rule::fn_param_type => {
                ty_loc = stmt_loc_from_pair(&item);
                ty = Some(parse_fn_param_type(item)?);
            }
            Rule::expr => {
                default = Some(parse_expr_inner(item));
            }
            _ => {}
        }
    }

    Ok(FnParamDecl {
        loc,
        name: name_pair.as_str().to_owned(),
        ty,
        ty_loc,
        default,
    })
}
