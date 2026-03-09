use super::*;

pub(super) fn parse_port_block(
    block_pair: Pair<'_, Rule>,
) -> Result<Vec<PortDecl>, Vec<Diagnostic>> {
    let (block_name, prefix) = match block_pair.as_rule() {
        Rule::ins_block => ("ins", "in"),
        Rule::outs_block => ("outs", "out"),
        _ => ("ports", "port"),
    };
    let allow_default_and_range = block_pair.as_rule() == Rule::ins_block;

    let mut ports = Vec::new();
    let mut has_list = false;
    let mut count_prefix: Option<usize> = None;
    let mut default_ty: Option<DeclType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_decl_type => {
                default_ty = Some(parse_section_default_decl_type(child, block_name)?);
            }
            Rule::int_lit => {
                if count_prefix.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        format!("{block_name} block count can only be specified once"),
                        0,
                        0,
                    )]);
                }
                let count_i = parse_int(child.as_str())?;
                if count_i <= 0 {
                    return Err(vec![Diagnostic::syntax(
                        format!("{block_name} count shorthand must be greater than zero"),
                        0,
                        0,
                    )]);
                }
                count_prefix = Some(count_i as usize);
            }
            Rule::port_list => {
                has_list = true;
                for item in child.into_inner() {
                    if item.as_rule() != Rule::port_decl {
                        continue;
                    }
                    let mut inner = item.into_inner();
                    let Some(name_pair) = inner.next() else {
                        return Err(vec![Diagnostic::syntax("missing port identifier", 0, 0)]);
                    };
                    let mut ty = None;
                    let mut default = None;
                    let mut range = None;
                    for inner_item in inner {
                        match inner_item.as_rule() {
                            Rule::decl_type
                            | Rule::type_name
                            | Rule::array_type
                            | Rule::namespace_ref
                            | Rule::qualified_ident => {
                                let actual = if inner_item.as_rule() == Rule::decl_type {
                                    let mut decl_inner = inner_item.into_inner();
                                    decl_inner.next().ok_or_else(|| {
                                        vec![Diagnostic::syntax(
                                            "missing port declaration type",
                                            0,
                                            0,
                                        )]
                                    })?
                                } else {
                                    inner_item
                                };
                                ty = Some(parse_decl_type(actual)?);
                            }
                            Rule::expr => default = Some(parse_expr_inner(inner_item)),
                            Rule::decl_range => range = Some(parse_decl_range_pair(inner_item)?),
                            _ => {}
                        }
                    }
                    if !allow_default_and_range {
                        if default.is_some() {
                            return Err(vec![Diagnostic::syntax(
                                format!("{block_name} declarations do not support default values"),
                                0,
                                0,
                            )]);
                        }
                        if range.is_some() {
                            return Err(vec![Diagnostic::syntax(
                                format!("{block_name} declarations do not support ranges"),
                                0,
                                0,
                            )]);
                        }
                    }
                    let ty = ty.or_else(|| default_ty.clone());
                    ports.push(PortDecl {
                        name: name_pair.as_str().to_owned(),
                        ty,
                        default,
                        range,
                    });
                }
            }
            _ => {}
        }
    }

    if has_list {
        if let Some(n) = count_prefix {
            if n != ports.len() {
                return Err(vec![Diagnostic::syntax(
                    format!(
                        "{block_name} block count prefix ({n}) does not match explicit declaration count ({})",
                        ports.len()
                    ),
                    0,
                    0,
                )]);
            }
        }
    } else if let Some(n) = count_prefix {
        for idx in 1..=n {
            ports.push(PortDecl {
                name: format!("{prefix}{idx}"),
                ty: default_ty.clone(),
                default: None,
                range: None,
            });
        }
    }

    Ok(ports)
}

pub(super) fn parse_params_block(
    block_pair: Pair<'_, Rule>,
) -> Result<Vec<ParamDecl>, Vec<Diagnostic>> {
    let mut params = Vec::new();
    let mut has_list = false;
    let mut count_prefix: Option<usize> = None;
    let mut default_ty: Option<DeclType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_decl_type => {
                default_ty = Some(parse_section_default_decl_type(child, "params")?);
            }
            Rule::int_lit => {
                if count_prefix.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "params block count can only be specified once",
                        0,
                        0,
                    )]);
                }
                let count_i = parse_int(child.as_str())?;
                if count_i <= 0 {
                    return Err(vec![Diagnostic::syntax(
                        "params count shorthand must be greater than zero",
                        0,
                        0,
                    )]);
                }
                count_prefix = Some(count_i as usize);
            }
            Rule::param_list => {
                has_list = true;
                for param_pair in child.into_inner() {
                    if param_pair.as_rule() != Rule::param_decl {
                        continue;
                    }
                    let mut inner = param_pair.into_inner();
                    let Some(name_pair) = inner.next() else {
                        return Err(vec![Diagnostic::syntax("missing param identifier", 0, 0)]);
                    };
                    let mut ty = None;
                    let mut default = None;
                    let mut range = None;
                    for item in inner {
                        match item.as_rule() {
                            Rule::decl_type
                            | Rule::type_name
                            | Rule::array_type
                            | Rule::namespace_ref
                            | Rule::qualified_ident => {
                                let actual = if item.as_rule() == Rule::decl_type {
                                    let mut decl_inner = item.into_inner();
                                    decl_inner.next().ok_or_else(|| {
                                        vec![Diagnostic::syntax(
                                            "missing param declaration type",
                                            0,
                                            0,
                                        )]
                                    })?
                                } else {
                                    item
                                };
                                ty = Some(parse_decl_type(actual)?);
                            }
                            Rule::expr => default = Some(parse_expr_inner(item)),
                            Rule::decl_range => range = Some(parse_decl_range_pair(item)?),
                            _ => {}
                        }
                    }
                    let ty = ty.or_else(|| default_ty.clone());
                    params.push(ParamDecl {
                        name: name_pair.as_str().to_owned(),
                        ty,
                        default,
                        range,
                    });
                }
            }
            _ => {}
        }
    }

    if has_list {
        if let Some(n) = count_prefix {
            if n != params.len() {
                return Err(vec![Diagnostic::syntax(
                    format!(
                        "params block count prefix ({n}) does not match explicit declaration count ({})",
                        params.len()
                    ),
                    0,
                    0,
                )]);
            }
        }
    } else if let Some(n) = count_prefix {
        for idx in 1..=n {
            params.push(ParamDecl {
                name: format!("param{idx}"),
                ty: default_ty.clone(),
                default: None,
                range: None,
            });
        }
    }

    Ok(params)
}

pub(super) fn parse_buffers_block(
    block_pair: Pair<'_, Rule>,
) -> Result<Vec<BufferDecl>, Vec<Diagnostic>> {
    let mut out = Vec::<BufferDecl>::new();
    let mut seen = HashSet::<String>::new();

    let mut has_list = false;
    let mut count_shorthand: Option<usize> = None;
    let mut default_ty: Option<BufferType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_elem_type => {
                default_ty = Some(parse_section_default_buffer_type(child, "buffers")?);
            }
            Rule::buffer_list => {
                has_list = true;
                for item in child.into_inner() {
                    if item.as_rule() != Rule::buffer_decl {
                        continue;
                    }
                    let mut inner = item.into_inner();
                    let Some(name_pair) = inner.next() else {
                        return Err(vec![Diagnostic::syntax("missing buffer identifier", 0, 0)]);
                    };
                    let name = name_pair.as_str().to_owned();
                    let ty = match inner.next() {
                        Some(ty_pair) => Some(parse_buffer_decl_type(ty_pair)?),
                        None => default_ty.clone(),
                    };
                    if !seen.insert(name.clone()) {
                        return Err(vec![Diagnostic::syntax(
                            format!("duplicate buffer declaration '{name}'"),
                            0,
                            0,
                        )]);
                    }
                    out.push(BufferDecl { name, ty });
                }
            }
            Rule::int_lit => {
                if has_list {
                    return Err(vec![Diagnostic::syntax(
                        "buffers block cannot mix explicit declarations and count shorthand",
                        0,
                        0,
                    )]);
                }
                let count_i = parse_int(child.as_str())?;
                if count_i <= 0 {
                    return Err(vec![Diagnostic::syntax(
                        "buffers count shorthand must be greater than zero",
                        0,
                        0,
                    )]);
                }
                count_shorthand = Some(count_i as usize);
            }
            _ => {}
        }
    }

    if let Some(n) = count_shorthand {
        for idx in 1..=n {
            out.push(BufferDecl {
                name: format!("buf{idx}"),
                ty: default_ty.clone(),
            });
        }
    }

    Ok(out)
}

pub(super) fn parse_events_block(
    block_pair: Pair<'_, Rule>,
) -> Result<Vec<EventDef>, Vec<Diagnostic>> {
    let mut events = Vec::<EventDef>::new();
    let mut seen = HashSet::<String>::new();

    for child in block_pair.into_inner() {
        if child.as_rule() != Rule::event_list {
            continue;
        }
        for item in child.into_inner() {
            if item.as_rule() != Rule::event_decl {
                continue;
            }
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
                    Rule::event_param_list => {
                        for event_param in part.into_inner() {
                            if event_param.as_rule() != Rule::event_param_decl {
                                continue;
                            }
                            let mut param_inner = event_param.into_inner();
                            let Some(param_name_pair) = param_inner.next() else {
                                return Err(vec![Diagnostic::syntax(
                                    "missing event parameter name",
                                    0,
                                    0,
                                )]);
                            };
                            let Some(param_ty_pair) = param_inner.next() else {
                                params.push(EventParamDecl {
                                    name: param_name_pair.as_str().to_owned(),
                                    ty: EventParamType::Scalar(PrimitiveType::F32),
                                });
                                continue;
                            };
                            params.push(EventParamDecl {
                                name: param_name_pair.as_str().to_owned(),
                                ty: parse_event_param_type(param_ty_pair)?,
                            });
                        }
                    }
                    Rule::stmt_block => {
                        body = Some(parse_stmt_block(part)?);
                    }
                    _ => {}
                }
            }
            let Some(name) = name else {
                return Err(vec![Diagnostic::syntax("missing event name", 0, 0)]);
            };
            if !seen.insert(name.clone()) {
                return Err(vec![Diagnostic::syntax(
                    format!("duplicate event declaration '{name}'"),
                    0,
                    0,
                )]);
            }
            let Some(body) = body else {
                return Err(vec![Diagnostic::syntax(
                    format!("missing handler body for event '{name}'"),
                    0,
                    0,
                )]);
            };
            events.push(EventDef { name, params, body });
        }
    }

    Ok(events)
}

pub(super) fn parse_graph_block(block_pair: Pair<'_, Rule>) -> Result<GraphBlock, Vec<Diagnostic>> {
    if block_pair.as_rule() != Rule::graph_block {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected graph block",
            0,
            0,
        )]);
    }

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

    Ok(GraphBlock { edges })
}

fn parse_graph_edge(edge_pair: Pair<'_, Rule>) -> Result<GraphEdge, Vec<Diagnostic>> {
    let mut rate = None::<GraphRate>;
    let mut source = None::<Expr>;
    let mut delay = None::<Expr>;
    let mut dests = None::<Vec<GraphEndpoint>>;

    for child in edge_pair.into_inner() {
        match child.as_rule() {
            Rule::graph_rate => {
                if rate.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "graph edge rate can only be specified once",
                        0,
                        0,
                    )]);
                }
                rate = Some(parse_graph_rate(child)?);
            }
            Rule::graph_send_edge => {
                let mut inner = child.into_inner();
                let Some(source_pair) = inner.next() else {
                    return Err(vec![Diagnostic::syntax(
                        "missing graph edge source expression",
                        0,
                        0,
                    )]);
                };
                source = Some(parse_expr(source_pair)?);
                let Some(arrow_pair) = inner.next() else {
                    return Err(vec![Diagnostic::syntax("missing graph edge arrow", 0, 0)]);
                };
                delay = parse_graph_edge_delay(arrow_pair)?;
                let Some(dest_pair) = inner.next() else {
                    return Err(vec![Diagnostic::syntax(
                        "missing graph edge destination endpoint",
                        0,
                        0,
                    )]);
                };
                dests = Some(parse_graph_edge_targets(dest_pair)?);
            }
            Rule::graph_recv_edge => {
                let mut inner = child.into_inner();
                let Some(dest_pair) = inner.next() else {
                    return Err(vec![Diagnostic::syntax(
                        "missing graph edge destination endpoint",
                        0,
                        0,
                    )]);
                };
                dests = Some(parse_graph_edge_targets(dest_pair)?);
                let Some(arrow_pair) = inner.next() else {
                    return Err(vec![Diagnostic::syntax("missing graph edge arrow", 0, 0)]);
                };
                delay = parse_graph_edge_delay(arrow_pair)?;
                let Some(source_pair) = inner.next() else {
                    return Err(vec![Diagnostic::syntax(
                        "missing graph edge source expression",
                        0,
                        0,
                    )]);
                };
                source = Some(parse_expr(source_pair)?);
            }
            _ => {}
        }
    }

    let Some(source) = source else {
        return Err(vec![Diagnostic::syntax(
            "missing graph edge source expression",
            0,
            0,
        )]);
    };
    let Some(dests) = dests else {
        return Err(vec![Diagnostic::syntax(
            "missing graph edge destination endpoint",
            0,
            0,
        )]);
    };

    Ok(GraphEdge {
        rate,
        source,
        delay,
        dests,
    })
}

fn parse_graph_edge_delay(arrow_pair: Pair<'_, Rule>) -> Result<Option<Expr>, Vec<Diagnostic>> {
    let mut delay = None::<Expr>;
    for part in arrow_pair.into_inner() {
        if part.as_rule() == Rule::graph_delay {
            let mut delay_inner = part.into_inner();
            let Some(expr_pair) = delay_inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing graph edge delay expression",
                    0,
                    0,
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
        _ => Err(vec![Diagnostic::syntax(
            "unknown graph edge rate annotation",
            0,
            0,
        )]),
    }
}

fn parse_graph_endpoint(pair: Pair<'_, Rule>) -> Result<GraphEndpoint, Vec<Diagnostic>> {
    let mut inner = pair.into_inner();
    let Some(first) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            "missing graph destination endpoint",
            0,
            0,
        )]);
    };

    match first.as_rule() {
        Rule::path_ident => {
            let path = first.as_str().to_owned();
            if let Some((proc, field)) = path.rsplit_once('.') {
                Ok(GraphEndpoint::ProcField {
                    proc: proc.to_owned(),
                    field: field.to_owned(),
                })
            } else {
                Ok(GraphEndpoint::Symbol(path))
            }
        }
        Rule::indexed_graph_endpoint => {
            let mut endpoint_inner = first.into_inner();
            let proc = endpoint_inner
                .next()
                .ok_or_else(|| vec![Diagnostic::syntax("missing graph proc endpoint base", 0, 0)])?
                .as_str()
                .to_owned();
            let index = parse_expr(endpoint_inner.next().ok_or_else(|| {
                vec![Diagnostic::syntax(
                    "missing graph proc endpoint index",
                    0,
                    0,
                )]
            })?)?;
            let field = endpoint_inner
                .next()
                .ok_or_else(|| {
                    vec![Diagnostic::syntax(
                        "missing graph proc endpoint field",
                        0,
                        0,
                    )]
                })?
                .as_str()
                .to_owned();
            Ok(GraphEndpoint::ProcIndexedField { proc, index, field })
        }
        _ => Err(vec![Diagnostic::syntax(
            "invalid graph destination endpoint",
            0,
            0,
        )]),
    }
}

fn parse_graph_edge_targets(pair: Pair<'_, Rule>) -> Result<Vec<GraphEndpoint>, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::graph_edge_targets => {
            let mut inner = pair.into_inner();
            let Some(targets) = inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing graph edge destination endpoint",
                    0,
                    0,
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
        _ => Err(vec![Diagnostic::syntax(
            "invalid graph destination endpoint list",
            0,
            0,
        )]),
    }
}

pub(super) fn parse_proc_block(
    block_pair: Pair<'_, Rule>,
) -> Result<ProcessorDef, Vec<Diagnostic>> {
    let mut name: Option<String> = None;
    let mut type_params = Vec::new();
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    let mut params = Vec::new();
    let mut events: Option<Vec<EventDef>> = None;
    let mut buffers = Vec::new();
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
            Rule::ins_block => {
                if !ins.is_empty() {
                    return Err(vec![Diagnostic::syntax("duplicate proc ins block", 0, 0)]);
                }
                ins = parse_port_block(child)?;
            }
            Rule::outs_block => {
                if !outs.is_empty() {
                    return Err(vec![Diagnostic::syntax("duplicate proc outs block", 0, 0)]);
                }
                outs = parse_port_block(child)?;
            }
            Rule::params_block => {
                if !params.is_empty() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc params block",
                        0,
                        0,
                    )]);
                }
                params = parse_params_block(child)?;
            }
            Rule::events_block => {
                if events.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc events block",
                        0,
                        0,
                    )]);
                }
                events = Some(parse_events_block(child)?);
            }
            Rule::buffers_block => {
                if !buffers.is_empty() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc buffers block",
                        0,
                        0,
                    )]);
                }
                buffers = parse_buffers_block(child)?;
            }
            Rule::init_block => {
                if init.is_some() {
                    return Err(vec![Diagnostic::syntax("duplicate proc init block", 0, 0)]);
                }
                init = Some(parse_exec_block(child)?);
            }
            Rule::block_exec_block => {
                if block_exec.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc block section",
                        0,
                        0,
                    )]);
                }
                block_exec = Some(parse_block_exec_block(child)?);
            }
            Rule::sample_block => {
                if sample.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc sample block",
                        0,
                        0,
                    )]);
                }
                sample = Some(parse_sample_block(child)?);
            }
            Rule::graph_block => {
                if graph.is_some() {
                    return Err(vec![Diagnostic::syntax("duplicate proc graph block", 0, 0)]);
                }
                graph = Some(parse_graph_block(child)?);
            }
            Rule::def_block => {
                local_defs.push(parse_def_block(child)?);
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![Diagnostic::syntax("missing proc name", 0, 0)]);
    };

    let mut block_pre = Vec::new();
    let mut block_post = Vec::new();
    let mut has_block_block = false;
    if let Some(exec) = block_exec {
        has_block_block = true;
        if sample.is_some() {
            return Err(vec![Diagnostic::syntax(
                "proc sample block cannot be declared both directly and inside block section",
                0,
                0,
            )]);
        }
        let Some(nested_sample) = exec.sample else {
            return Err(vec![Diagnostic::syntax(
                "proc block section must include nested 'sample' block",
                0,
                0,
            )]);
        };
        block_pre = exec.pre;
        block_post = exec.post;
        sample = Some(nested_sample);
    }

    if graph.is_some() && (sample.is_some() || has_block_block) {
        return Err(vec![Diagnostic::syntax(
            "proc graph block cannot be declared with sample or block",
            0,
            0,
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
        name,
        type_params,
        ins,
        outs,
        params,
        events: events.unwrap_or_default(),
        buffers,
        has_init_block: init.is_some(),
        has_block_block,
        has_sample_block,
        has_graph_block,
        sample_oversample_factor,
        init: init.unwrap_or(InitBlock {
            default_ty: None,
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
                    let mut decl_inner = item.into_inner();
                    let Some(field_name) = decl_inner.next() else {
                        return Err(vec![Diagnostic::syntax("missing struct field name", 0, 0)]);
                    };
                    let mut parsed_ty = None::<FieldType>;
                    let mut default = None;
                    for part in decl_inner {
                        match part.as_rule() {
                            Rule::field_type => {
                                parsed_ty = Some(parse_field_type(part)?);
                            }
                            Rule::expr => {
                                default = Some(parse_expr_inner(part));
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
                    fields.push(StructField {
                        name: field_name.as_str().to_owned(),
                        ty,
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
        return Err(vec![Diagnostic::syntax("missing struct name", 0, 0)]);
    };
    Ok(StructDef {
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
        Expr::Int(_) => Some(PrimitiveType::I32),
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Compare { .. } | Expr::Logical { .. } | Expr::UnaryNot { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr } => {
            let inner = infer_expr_primitive_type(expr)?;
            merge_inferred_integer_type(inner, inner)
        }
        Expr::Binary { op, lhs, rhs } => {
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
    let mut name: Option<String> = None;
    let mut params = Vec::new();
    let mut body = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::fn_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::fn_param_decl {
                        params.push(parse_fn_param_decl(item, "function")?);
                    }
                }
            }
            Rule::stmt_block => {
                body = Some(parse_stmt_block(child)?);
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![Diagnostic::syntax("missing function name", 0, 0)]);
    };
    let Some(body) = body else {
        return Err(vec![Diagnostic::syntax("missing function body", 0, 0)]);
    };

    Ok(FunctionDef {
        name,
        type_params: Vec::new(),
        params,
        body,
    })
}

pub(super) fn parse_fn_param_decl(
    pair: Pair<'_, Rule>,
    context: &str,
) -> Result<FnParamDecl, Vec<Diagnostic>> {
    let mut inner = pair.into_inner();
    let Some(name_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            format!("missing {context} parameter name"),
            0,
            0,
        )]);
    };

    let mut ty = None;
    let mut default = None;
    for item in inner {
        match item.as_rule() {
            Rule::fn_param_type => {
                ty = Some(parse_fn_param_type(item)?);
            }
            Rule::expr => {
                default = Some(parse_expr_inner(item));
            }
            _ => {}
        }
    }

    Ok(FnParamDecl {
        name: name_pair.as_str().to_owned(),
        ty,
        default,
    })
}
