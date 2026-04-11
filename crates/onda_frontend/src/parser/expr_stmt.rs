use super::*;

pub(super) fn parse_stmt_list_pair(
    stmt_list_pair: Pair<'_, Rule>,
) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    let mut stmts = Vec::new();
    for stmt_pair in stmt_list_pair.into_inner() {
        stmts.push(parse_stmt(stmt_pair)?);
    }
    Ok(stmts)
}

pub(super) fn parse_exec_block(block_pair: Pair<'_, Rule>) -> Result<InitBlock, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&block_pair);
    let mut default_ty = None;
    let mut default_ty_loc = Span::ZERO;
    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_decl_type => {
                default_ty_loc = stmt_loc_from_pair(&child);
                default_ty = Some(parse_init_default_decl_type(child)?);
            }
            Rule::stmt_list => {
                return Ok(InitBlock {
                    loc,
                    default_ty,
                    default_ty_loc,
                    body: parse_stmt_list_pair(child)?,
                });
            }
            _ => {}
        }
    }
    Ok(InitBlock {
        loc,
        default_ty,
        default_ty_loc,
        body: Vec::new(),
    })
}

pub(super) fn parse_sample_block(
    block_pair: Pair<'_, Rule>,
) -> Result<SampleBlock, Vec<Diagnostic>> {
    let rule = block_pair.as_rule();
    if rule != Rule::sample_block && rule != Rule::sample_nested_block {
        return Err(vec![syntax_at_pair(
            &block_pair,
            "internal parser error: expected sample block",
        )]);
    }

    let loc = stmt_loc_from_pair(&block_pair);
    let mut oversample_factor = None;
    let mut body = Vec::new();
    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::sample_factor => {
                if oversample_factor.is_some() {
                    return Err(vec![syntax_at_pair(
                        &child,
                        "sample block oversampling factor can only be specified once",
                    )]);
                }
                oversample_factor = Some(parse_sample_factor(child)?);
            }
            Rule::stmt_list => {
                body = parse_stmt_list_pair(child)?;
            }
            Rule::stmt_block => {
                body = parse_stmt_block(child)?;
            }
            _ => {}
        }
    }

    Ok(SampleBlock {
        loc,
        oversample_factor,
        body,
    })
}

fn parse_sample_factor(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::sample_factor {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected sample factor",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(expr_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing sample oversampling factor expression",
        )]);
    };
    parse_expr(expr_pair)
}

pub(super) fn parse_block_exec_block(
    block_pair: Pair<'_, Rule>,
) -> Result<BlockExec, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&block_pair);
    let mut pre = Vec::new();
    let mut post = Vec::new();
    let mut nested_sample: Option<SampleBlock> = None;

    for child in block_pair.into_inner() {
        if child.as_rule() != Rule::block_exec_list {
            continue;
        }

        for item in child.into_inner() {
            if item.as_rule() == Rule::sample_nested_block {
                if nested_sample.is_some() {
                    return Err(vec![syntax_at_pair(
                        &item,
                        "duplicate nested sample block in block section",
                    )]);
                }
                nested_sample = Some(parse_sample_block(item)?);
                continue;
            }

            let stmt = parse_stmt(item)?;
            if nested_sample.is_some() {
                post.push(stmt);
            } else {
                pre.push(stmt);
            }
        }
    }

    Ok(BlockExec {
        loc,
        pre,
        sample: nested_sample,
        post,
    })
}

pub(super) fn parse_struct_method_decl(
    pair: Pair<'_, Rule>,
) -> Result<FunctionDef, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    if pair.as_rule() != Rule::struct_method_decl {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected struct method declaration",
        )]);
    }
    let mut name: Option<String> = None;
    let mut type_params = Vec::new();
    let mut params = Vec::new();
    let mut return_ty = None;
    let mut return_ty_loc = Span::ZERO;
    let mut body = None;
    for child in pair.into_inner() {
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
                        params.push(parse_fn_param_decl(item, "method")?);
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
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing method name")]);
    };
    let Some(body) = body else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing method body")]);
    };
    Ok(FunctionDef {
        loc,
        name,
        type_params,
        params,
        return_ty,
        return_ty_loc,
        body,
    })
}

pub(super) fn parse_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::const_decl => parse_const_stmt(pair),
        Rule::assign_stmt => parse_assign_stmt(pair),
        Rule::return_stmt => parse_return_stmt(pair),
        Rule::if_stmt => parse_if_stmt(pair),
        Rule::for_stmt => parse_for_stmt(pair),
        Rule::while_stmt => parse_while_stmt(pair),
        Rule::loop_stmt => parse_loop_stmt(pair),
        Rule::break_stmt => parse_break_stmt(pair),
        Rule::continue_stmt => parse_continue_stmt(pair),
        Rule::call_stmt => parse_call_stmt(pair),
        _ => Err(vec![syntax_at_pair(
            &pair,
            "unexpected statement kind in parser",
        )]),
    }
}

pub(super) fn parse_const_decl(pair: Pair<'_, Rule>) -> Result<ConstDecl, Vec<Diagnostic>> {
    let pair = if pair.as_rule() == Rule::const_block {
        let loc = stmt_loc_from_pair(&pair);
        let mut inner = pair.into_inner();
        inner
            .next()
            .ok_or_else(|| vec![syntax_at_loc(loc.as_ref(), "missing const declaration")])?
    } else {
        pair
    };
    if pair.as_rule() != Rule::const_decl {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected const declaration",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut name = None::<String>;
    let mut ty = None::<PrimitiveType>;
    let mut expr = None::<Expr>;
    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::type_name => {
                ty = Some(parse_primitive_type(child.as_str()).map_err(|d| vec![d])?);
            }
            Rule::expr => {
                expr = Some(parse_expr_inner(child));
            }
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing const name")]);
    };
    let Some(expr) = expr else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            format!("missing initializer for const '{name}'"),
        )]);
    };
    Ok(ConstDecl {
        loc,
        name,
        ty,
        expr,
    })
}

fn parse_const_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let decl = parse_const_decl(pair)?;
    Ok(Stmt::Const {
        loc: loc.clone(),
        decl,
    })
}

pub(super) fn parse_assign_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(kind_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing assignment statement",
        )]);
    };

    match kind_pair.as_rule() {
        Rule::typed_assign_stmt => {
            let mut typed_inner = kind_pair.into_inner();
            let Some(name_pair) = typed_inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing typed assignment target",
                )]);
            };
            let Some(ty_pair) = typed_inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing typed assignment type",
                )]);
            };
            let ty_pair = if ty_pair.as_rule() == Rule::typed_decl_type {
                let mut inner = ty_pair.into_inner();
                let Some(actual) = inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing typed declaration type",
                    )]);
                };
                actual
            } else {
                ty_pair
            };
            let typed_decl_ty_loc = stmt_loc_from_pair(&ty_pair);
            let expr_pair = typed_inner.next();
            match ty_pair.as_rule() {
                Rule::type_name => {
                    let Some(expr_pair) = expr_pair else {
                        return Err(vec![syntax_at_loc(
                            loc.as_ref(),
                            "missing typed assignment expression",
                        )]);
                    };
                    let decl_ty = parse_primitive_type(ty_pair.as_str()).map_err(|d| vec![d])?;
                    Ok(Stmt::Assign {
                        loc: loc.clone(),
                        target_loc: stmt_loc_from_pair(&name_pair),
                        target: AssignTarget::Var(name_pair.as_str().to_owned()),
                        decl_ty: Some(decl_ty),
                        generic_decl_ty: None,
                        is_typed_decl: true,
                        typed_decl_ty_loc,
                        expr: parse_expr(expr_pair)?,
                    })
                }
                Rule::array_type => {
                    let spec = parse_array_type_spec(ty_pair)?;
                    let init = if let Some(expr_pair) = expr_pair {
                        let init_expr = parse_expr(expr_pair)?;
                        match init_expr {
                            Expr::ArrayLiteral { values, .. } => Some(values),
                            other => {
                                if matches!(spec.elem, ArrayElemType::Struct(_)) {
                                    Some(vec![other])
                                } else {
                                    return Err(vec![syntax_at_loc(
                                        loc.as_ref(),
                                        "array typed declaration initializer must be an array literal like [a, b, ...]",
                                    )]);
                                }
                            }
                        }
                    } else {
                        None
                    };
                    Ok(Stmt::Assign {
                        loc: loc.clone(),
                        target_loc: stmt_loc_from_pair(&name_pair),
                        target: AssignTarget::Var(name_pair.as_str().to_owned()),
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: true,
                        typed_decl_ty_loc,
                        expr: Expr::ArrayCtor {
                            loc: loc.clone(),
                            spec,
                            init,
                        },
                    })
                }
                Rule::named_type => {
                    let (decl_name, decl_type_args) = parse_named_type_ref(ty_pair)?;
                    let missing_decl_type_args = decl_type_args.is_empty();
                    let mut expr = if let Some(expr_pair) = expr_pair {
                        parse_expr(expr_pair)?
                    } else {
                        Expr::UserCall {
                            loc: loc.clone(),
                            name: decl_name.clone(),
                            type_args: Vec::new(),
                            args: Vec::new(),
                        }
                    };
                    if let Expr::UserCall {
                        name, type_args, ..
                    } = &mut expr
                    {
                        if *name == decl_name && type_args.is_empty() {
                            *type_args = decl_type_args;
                            return Ok(Stmt::Assign {
                                loc: loc.clone(),
                                target_loc: stmt_loc_from_pair(&name_pair),
                                target: AssignTarget::Var(name_pair.as_str().to_owned()),
                                decl_ty: None,
                                generic_decl_ty: None,
                                is_typed_decl: missing_decl_type_args,
                                typed_decl_ty_loc,
                                expr,
                            });
                        }
                    }
                    Ok(Stmt::Assign {
                        loc: loc.clone(),
                        target_loc: stmt_loc_from_pair(&name_pair),
                        target: AssignTarget::Var(name_pair.as_str().to_owned()),
                        decl_ty: None,
                        generic_decl_ty: Some(decl_name),
                        is_typed_decl: true,
                        typed_decl_ty_loc,
                        expr,
                    })
                }
                Rule::tuple_type => {
                    let elems: Result<Vec<PrimitiveType>, Vec<Diagnostic>> = ty_pair
                        .into_inner()
                        .filter(|p| p.as_rule() == Rule::type_name)
                        .map(|p| parse_primitive_type(p.as_str()).map_err(|d| vec![d]))
                        .collect();
                    let _tuple_ty = DeclType::Tuple(elems?);
                    let Some(expr_pair) = expr_pair else {
                        return Err(vec![syntax_at_loc(
                            loc.as_ref(),
                            "missing tuple typed assignment expression",
                        )]);
                    };
                    Ok(Stmt::Assign {
                        loc: loc.clone(),
                        target_loc: stmt_loc_from_pair(&name_pair),
                        target: AssignTarget::Var(name_pair.as_str().to_owned()),
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: true,
                        typed_decl_ty_loc,
                        expr: parse_expr(expr_pair)?,
                    })
                }
                _ => Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "unexpected typed declaration type",
                )]),
            }
        }
        Rule::plain_assign_stmt => {
            let mut plain_inner = kind_pair.into_inner();
            let Some(target_pair) = plain_inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing assignment target",
                )]);
            };
            let Some(expr_pair) = plain_inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing assignment expression",
                )]);
            };
            if target_pair.as_rule() == Rule::index_target {
                let mut target_inner = target_pair.clone().into_inner();
                let Some(base_pair) = target_inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing indexed assignment base",
                    )]);
                };
                let Some(first_index_pair) = target_inner.next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing indexed assignment index",
                    )]);
                };
                if let Some(second_index_pair) = target_inner.next() {
                    let value_expr = parse_expr(expr_pair)?;
                    return Ok(Stmt::Expr {
                        loc,
                        expr: Expr::UserCall {
                            loc: Span::ZERO,
                            name: BUFFER_WRITE2_INTERNAL_FN.to_owned(),
                            type_args: Vec::new(),
                            args: vec![
                                CallArg {
                                    name: None,
                                    expr: Expr::var(base_pair.as_str().to_owned()),
                                },
                                CallArg {
                                    name: None,
                                    expr: parse_expr(first_index_pair)?,
                                },
                                CallArg {
                                    name: None,
                                    expr: parse_expr(second_index_pair)?,
                                },
                                CallArg {
                                    name: None,
                                    expr: value_expr,
                                },
                            ],
                        },
                    });
                }
            }
            Ok(Stmt::Assign {
                loc,
                target_loc: stmt_loc_from_pair(&target_pair),
                target: parse_assign_target(target_pair)?,
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Span::ZERO,
                expr: parse_expr(expr_pair)?,
            })
        }
        _ => Err(vec![syntax_at_loc(
            loc.as_ref(),
            "unexpected assignment statement kind",
        )]),
    }
}

pub(super) fn parse_return_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(expr_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing return expression",
        )]);
    };
    let expr = parse_expr(expr_pair)?;
    Ok(Stmt::Return { loc, expr })
}

pub(super) fn parse_if_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    fn parse_if_cond_pair(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
        let loc = stmt_loc_from_pair(&pair);
        match pair.as_rule() {
            Rule::if_cond => {
                let mut inner = pair.into_inner();
                let Some(expr_pair) = inner.next() else {
                    return Err(vec![syntax_at_loc(loc.as_ref(), "missing if condition")]);
                };
                parse_expr(expr_pair)
            }
            Rule::expr => parse_expr(pair),
            _ => Err(vec![syntax_at_loc(
                loc.as_ref(),
                "internal parser error: expected if condition",
            )]),
        }
    }

    let if_loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(cond_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(if_loc.as_ref(), "missing if condition")]);
    };
    let cond = parse_if_cond_pair(cond_pair)?;

    let Some(then_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            if_loc.as_ref(),
            "missing if then block",
        )]);
    };
    let then_branch = parse_stmt_block(then_pair)?;

    let mut elifs = Vec::<(Expr, Vec<Stmt>, Span)>::new();
    let mut explicit_else: Option<Vec<Stmt>> = None;
    for item in inner {
        match item.as_rule() {
            Rule::elif_clause => {
                let elif_loc = stmt_loc_from_pair(&item);
                let mut elif_inner = item.into_inner();
                let Some(elif_cond_pair) = elif_inner.next() else {
                    return Err(vec![syntax_at_loc(
                        elif_loc.as_ref(),
                        "missing elif condition",
                    )]);
                };
                let Some(elif_then_pair) = elif_inner.next() else {
                    return Err(vec![syntax_at_loc(elif_loc.as_ref(), "missing elif block")]);
                };
                elifs.push((
                    parse_if_cond_pair(elif_cond_pair)?,
                    parse_stmt_block(elif_then_pair)?,
                    elif_loc,
                ));
            }
            Rule::stmt_block => {
                if explicit_else.is_some() {
                    return Err(vec![syntax_at_pair(
                        &item,
                        "duplicate else block in if statement",
                    )]);
                }
                explicit_else = Some(parse_stmt_block(item)?);
            }
            _ => {
                return Err(vec![syntax_at_pair(
                    &item,
                    "unexpected token in if statement",
                )]);
            }
        }
    }
    let mut else_branch = explicit_else.unwrap_or_default();
    for (elif_cond, elif_then, elif_loc) in elifs.into_iter().rev() {
        else_branch = vec![Stmt::If {
            loc: elif_loc,
            cond: elif_cond,
            then_branch: elif_then,
            else_branch,
        }];
    }

    Ok(Stmt::If {
        loc: if_loc,
        cond,
        then_branch,
        else_branch,
    })
}

pub(super) fn parse_call_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(call_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing call expression")]);
    };
    let expr = parse_primary_expr(call_pair);
    Ok(Stmt::Expr { loc, expr })
}

pub(super) fn parse_for_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(var_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing for loop variable",
        )]);
    };
    let Some(next_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing for loop start")]);
    };
    let (step, start_pair) = if next_pair.as_rule() == Rule::for_step {
        let mut step_inner = next_pair.into_inner();
        let Some(step_expr_pair) = step_inner.next() else {
            return Err(vec![syntax_at_loc(loc.as_ref(), "missing for loop step")]);
        };
        let Some(start_pair) = inner.next() else {
            return Err(vec![syntax_at_loc(loc.as_ref(), "missing for loop start")]);
        };
        (Some(parse_expr(step_expr_pair)?), start_pair)
    } else {
        (None, next_pair)
    };
    let Some(op_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing for loop range operator",
        )]);
    };
    let Some(end_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing for loop end")]);
    };
    let Some(body_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing for loop body")]);
    };

    let end_inclusive = match op_pair.as_rule() {
        Rule::for_range_op => match op_pair.as_str() {
            ".." => false,
            "..=" => true,
            _ => {
                return Err(vec![syntax_at_pair(
                    &op_pair,
                    "invalid for loop range operator",
                )]);
            }
        },
        _ => {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                "missing for loop range operator",
            )]);
        }
    };
    let start = parse_for_bound(start_pair)?;
    let end = parse_for_bound(end_pair)?;
    let body = parse_stmt_block(body_pair)?;

    Ok(Stmt::For {
        loc,
        var: var_pair.as_str().to_owned(),
        step,
        start,
        end,
        end_inclusive,
        body,
    })
}

pub(super) fn parse_loop_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(count_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing loop count")]);
    };
    let Some(body_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing loop body")]);
    };

    let count = parse_for_bound(count_pair)?;
    let body = parse_stmt_block(body_pair)?;
    Ok(Stmt::For {
        loc,
        var: "_".to_owned(),
        step: None,
        start: Expr::int(0),
        end: count,
        end_inclusive: false,
        body,
    })
}

pub(super) fn parse_while_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(cond_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing while condition")]);
    };
    let Some(body_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing while loop body")]);
    };

    let cond = parse_expr(cond_pair)?;
    let body = parse_stmt_block(body_pair)?;
    Ok(Stmt::While { loc, cond, body })
}

pub(super) fn parse_break_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    Ok(Stmt::Break { loc })
}

pub(super) fn parse_continue_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    Ok(Stmt::Continue { loc })
}

pub(super) fn parse_for_bound(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    match pair.as_rule() {
        Rule::int_lit => Ok(Expr::int(parse_int(pair.as_str())? as i64).with_loc(loc)),
        Rule::path_ident | Rule::namespace_ref => {
            Ok(Expr::var(pair.as_str().to_owned()).with_loc(loc))
        }
        Rule::for_bound => {
            let mut inner = pair.into_inner();
            let Some(inner_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing for/loop bound expression",
                )]);
            };
            match inner_pair.as_rule() {
                Rule::expr => parse_expr(inner_pair),
                _ => parse_for_bound(inner_pair),
            }
        }
        Rule::expr => parse_expr(pair),
        _ => Err(vec![syntax_at_loc(
            loc.as_ref(),
            "for/loop bound must be an integer literal, variable path, or parenthesized expression",
        )]),
    }
}

pub(super) fn parse_stmt_block(pair: Pair<'_, Rule>) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::stmt_block {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected statement block",
        )]);
    }

    let mut stmts = Vec::new();
    for child in pair.into_inner() {
        if child.as_rule() != Rule::stmt_list {
            continue;
        }
        for stmt_pair in child.into_inner() {
            stmts.push(parse_stmt(stmt_pair)?);
        }
    }
    Ok(stmts)
}

pub(super) fn parse_expr(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::expr && pair.as_rule() != Rule::graph_expr {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected expression pair",
        )]);
    }
    Ok(parse_expr_inner(pair))
}

pub(super) fn parse_expr_inner(pair: Pair<'_, Rule>) -> Expr {
    let pratt = PrattParser::new()
        .op(Op::prefix(Rule::prefix))
        .op(Op::infix(Rule::or_op, Assoc::Left))
        .op(Op::infix(Rule::and_op, Assoc::Left))
        .op(Op::infix(Rule::bit_or_op, Assoc::Left))
        .op(Op::infix(Rule::bit_xor_op, Assoc::Left))
        .op(Op::infix(Rule::bit_and_op, Assoc::Left))
        .op(Op::infix(Rule::cmp_op, Assoc::Left))
        .op(Op::infix(Rule::shift_op, Assoc::Left))
        .op(Op::infix(Rule::add_op, Assoc::Left))
        .op(Op::infix(Rule::mul_op, Assoc::Left));

    pratt
        .map_primary(parse_primary_expr)
        .map_prefix(|op, rhs| {
            let op_loc = stmt_loc_from_pair(&op);
            let loc = SourceLoc::spanning(op_loc.as_ref(), rhs.loc());
            match op.as_str() {
                "-" => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::Sub,
                    lhs: Box::new(Expr::number(0.0)),
                    rhs: Box::new(rhs),
                },
                "!" => Expr::UnaryNot {
                    loc: loc.into(),
                    expr: Box::new(rhs),
                },
                "~" => Expr::UnaryBitNot {
                    loc: loc.into(),
                    expr: Box::new(rhs),
                },
                _ => unreachable!("unknown prefix operator"),
            }
        })
        .map_infix(|lhs, op, rhs| {
            let loc = SourceLoc::spanning(lhs.loc(), rhs.loc());
            match (op.as_rule(), op.as_str()) {
                (Rule::or_op, "||") => Expr::Logical {
                    loc: loc.into(),
                    op: LogicalOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::and_op, "&&") => Expr::Logical {
                    loc: loc.into(),
                    op: LogicalOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::bit_or_op, "|") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::BitOr,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::bit_xor_op, "^") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::BitXor,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::bit_and_op, "&") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::BitAnd,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::cmp_op, "==") => Expr::Compare {
                    loc: loc.into(),
                    op: CmpOp::Eq,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::cmp_op, "!=") => Expr::Compare {
                    loc: loc.into(),
                    op: CmpOp::Ne,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::cmp_op, "<") => Expr::Compare {
                    loc: loc.into(),
                    op: CmpOp::Lt,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::cmp_op, "<=") => Expr::Compare {
                    loc: loc.into(),
                    op: CmpOp::Le,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::cmp_op, ">") => Expr::Compare {
                    loc: loc.into(),
                    op: CmpOp::Gt,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::cmp_op, ">=") => Expr::Compare {
                    loc: loc.into(),
                    op: CmpOp::Ge,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::shift_op, "<<") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::ShiftLeft,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::shift_op, ">>") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::ShiftRight,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::add_op, "+") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::Add,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::add_op, "-") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::Sub,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::mul_op, "*") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::Mul,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::mul_op, "/") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::Div,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                (Rule::mul_op, "%") => Expr::Binary {
                    loc: loc.into(),
                    op: BinaryOp::Mod,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                _ => unreachable!("unknown infix operator"),
            }
        })
        .parse(pair.into_inner())
}

pub(super) fn parse_primary_expr(pair: Pair<'_, Rule>) -> Expr {
    let loc = stmt_loc_from_pair(&pair);
    match pair.as_rule() {
        Rule::number => {
            let text = pair.as_str();
            if text.contains('.') {
                Expr::number(
                    text.parse::<f64>()
                        .expect("pest number rule produced invalid float literal"),
                )
                .with_loc(loc)
            } else {
                Expr::int(
                    text.parse::<i64>()
                        .expect("pest number rule produced invalid int literal"),
                )
                .with_loc(loc)
            }
        }
        Rule::bool_lit => Expr::bool(pair.as_str() == "true").with_loc(loc),
        Rule::array_lit => Expr::array_literal(
            pair.into_inner()
                .filter(|p| p.as_rule() == Rule::expr || p.as_rule() == Rule::graph_expr)
                .map(parse_expr_inner)
                .collect(),
        )
        .with_loc(loc),
        Rule::ident | Rule::path_ident | Rule::namespace_ref => {
            Expr::var(pair.as_str().to_owned()).with_loc(loc)
        }
        Rule::indexed_member_expr => {
            let mut inner = pair.into_inner();
            let base = inner
                .next()
                .expect("indexed_member_expr rule must include base path")
                .as_str()
                .to_owned();
            let index_pair = inner
                .next()
                .expect("indexed_member_expr rule must include index expression");
            let field_pair = inner
                .next()
                .expect("indexed_member_expr rule must include field identifier");
            let field_index_pair = inner.next();
            if let Some(field_index_pair) = field_index_pair {
                // base[idx].field[fidx] — struct array field index
                Expr::UserCall {
                    loc,
                    name: STRUCT_ARRAY_FIELD_INDEX_SENTINEL.to_owned(),
                    type_args: Vec::new(),
                    args: vec![
                        CallArg {
                            name: Some(SAFI_BASE_ARG.to_owned()),
                            expr: Expr::var(base),
                        },
                        CallArg {
                            name: Some(SAFI_IDX_ARG.to_owned()),
                            expr: parse_expr_inner(index_pair),
                        },
                        CallArg {
                            name: Some(SAFI_FIELD_ARG.to_owned()),
                            expr: Expr::var(field_pair.as_str().to_owned())
                                .with_loc(stmt_loc_from_pair(&field_pair)),
                        },
                        CallArg {
                            name: Some(SAFI_FIELD_IDX_ARG.to_owned()),
                            expr: parse_expr_inner(field_index_pair),
                        },
                    ],
                }
            } else {
                // base[idx].field — proc array field access
                Expr::UserCall {
                    loc,
                    name: format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"),
                    type_args: Vec::new(),
                    args: vec![
                        CallArg {
                            name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                            expr: Expr::var(base),
                        },
                        CallArg {
                            name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                            expr: parse_expr_inner(index_pair),
                        },
                        CallArg {
                            name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                            expr: Expr::var(field_pair.as_str().to_owned())
                                .with_loc(stmt_loc_from_pair(&field_pair)),
                        },
                    ],
                }
            }
        }
        Rule::graph_nested_indexed_member_expr => {
            let mut inner = pair.into_inner();
            let base = inner
                .next()
                .expect("graph_nested_indexed_member_expr rule must include base path")
                .as_str()
                .to_owned();
            let proc_index_pair = inner
                .next()
                .expect("graph_nested_indexed_member_expr rule must include proc index");
            let field_pair = inner
                .next()
                .expect("graph_nested_indexed_member_expr rule must include field identifier");
            let field_index_pair = inner
                .next()
                .expect("graph_nested_indexed_member_expr rule must include field index");
            Expr::UserCall {
                loc,
                name: GRAPH_PROC_ARRAY_FIELD_INDEX_SENTINEL.to_owned(),
                type_args: Vec::new(),
                args: vec![
                    CallArg {
                        name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                        expr: Expr::var(base),
                    },
                    CallArg {
                        name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                        expr: parse_expr_inner(proc_index_pair),
                    },
                    CallArg {
                        name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                        expr: Expr::var(field_pair.as_str().to_owned())
                            .with_loc(stmt_loc_from_pair(&field_pair)),
                    },
                    CallArg {
                        name: Some(GRAPH_PROC_FIELD_INDEX_EXPR_ARG.to_owned()),
                        expr: parse_expr_inner(field_index_pair),
                    },
                ],
            }
        }
        Rule::index_expr => {
            let mut inner = pair.into_inner();
            let base = inner
                .next()
                .expect("index_expr rule must include base path")
                .as_str()
                .to_owned();
            let idx_pair = inner
                .next()
                .expect("index_expr rule must include index expression");
            let idx_first = parse_expr_inner(idx_pair);
            if let Some(idx_second_pair) = inner.next() {
                Expr::UserCall {
                    loc,
                    name: BUFFER_READ2_INTERNAL_FN.to_owned(),
                    type_args: Vec::new(),
                    args: vec![
                        CallArg {
                            name: None,
                            expr: Expr::var(base),
                        },
                        CallArg {
                            name: None,
                            expr: idx_first,
                        },
                        CallArg {
                            name: None,
                            expr: parse_expr_inner(idx_second_pair),
                        },
                    ],
                }
            } else {
                Expr::Index {
                    loc,
                    base,
                    index: Box::new(idx_first),
                }
            }
        }
        Rule::slice_expr => {
            let mut inner = pair.into_inner();
            let base = inner
                .next()
                .expect("slice_expr rule must include base path")
                .as_str()
                .to_owned();
            let mut start = None::<Box<Expr>>;
            let mut end = None::<Box<Expr>>;
            for bound in inner {
                match bound.as_rule() {
                    Rule::slice_start => {
                        let expr = bound
                            .into_inner()
                            .next()
                            .expect("slice_start must contain expr");
                        start = Some(Box::new(parse_expr_inner(expr)));
                    }
                    Rule::slice_end => {
                        let expr = bound
                            .into_inner()
                            .next()
                            .expect("slice_end must contain expr");
                        end = Some(Box::new(parse_expr_inner(expr)));
                    }
                    _ => {}
                }
            }
            Expr::Slice {
                loc,
                base,
                start,
                end,
            }
        }
        Rule::call_field_expr => {
            let mut inner = pair.into_inner();
            let call_pair = inner
                .next()
                .expect("call_field_expr rule must include call expression");
            let field_pair = inner
                .next()
                .expect("call_field_expr rule must include field identifier");
            let (name, type_args, mut args) = parse_call_expr_parts(call_pair);
            args.push(CallArg {
                name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                expr: Expr::var(field_pair.as_str().to_owned())
                    .with_loc(stmt_loc_from_pair(&field_pair)),
            });
            Expr::UserCall {
                loc,
                name: format!("{PROC_FIELD_SENTINEL_PREFIX}{name}"),
                type_args,
                args,
            }
        }
        Rule::call_expr => {
            let (name, type_args, mut args) = parse_call_expr_parts(pair);

            if type_args.is_empty() {
                if let Ok(to_ty) = parse_primitive_type(&name) {
                    if to_ty != PrimitiveType::Bool && args.len() == 1 && args[0].name.is_none() {
                        return Expr::Cast {
                            loc,
                            to: to_ty,
                            expr: Box::new(args.remove(0).expr),
                        };
                    }
                }
            }

            if type_args.is_empty() {
                if let Some(func) = parse_builtin_fn(&name) {
                    if args.iter().all(|a| a.name.is_none()) {
                        return Expr::Call {
                            loc,
                            func,
                            args: args.into_iter().map(|a| a.expr).collect(),
                        };
                    }
                }
            }

            Expr::UserCall {
                loc,
                name,
                type_args,
                args,
            }
        }
        Rule::tuple_expr => {
            let values: Vec<Expr> = pair
                .into_inner()
                .filter(|p| p.as_rule() == Rule::expr)
                .map(parse_expr_inner)
                .collect();
            Expr::Tuple { loc, values }
        }
        Rule::expr | Rule::graph_expr => parse_expr_inner(pair),
        _ => unreachable!("unexpected primary expression token"),
    }
}

pub(super) fn parse_call_expr_parts(
    pair: Pair<'_, Rule>,
) -> (String, Vec<CallTypeArg>, Vec<CallArg>) {
    assert!(
        pair.as_rule() == Rule::call_expr,
        "parse_call_expr_parts expects call_expr pair"
    );
    let mut inner = pair.into_inner();
    let target_pair = inner
        .next()
        .expect("call_expr rule must include call target");
    let (name, mut args, mut type_args) = parse_call_target(target_pair);
    for item in inner {
        match item.as_rule() {
            Rule::generic_type_arg_list => {
                for ty in item.into_inner() {
                    let mut push_arg = |pair: Pair<'_, Rule>| match pair.as_rule() {
                        Rule::type_name => {
                            type_args.push(CallTypeArg::Primitive(
                                parse_primitive_type(pair.as_str()).expect(
                                    "generic_type_arg_list type_name should parse to primitive",
                                ),
                            ));
                        }
                        Rule::ident => {
                            type_args.push(CallTypeArg::Generic(pair.as_str().to_owned()));
                        }
                        _ => {}
                    };
                    match ty.as_rule() {
                        Rule::generic_type_arg => {
                            let mut inner = ty.into_inner();
                            if let Some(inner_pair) = inner.next() {
                                push_arg(inner_pair);
                            }
                        }
                        Rule::type_name | Rule::ident => push_arg(ty),
                        _ => {}
                    }
                }
            }
            Rule::arg_list => {
                for arg_pair in item.into_inner() {
                    if arg_pair.as_rule() == Rule::call_arg {
                        let mut arg_inner = arg_pair.into_inner();
                        let Some(first) = arg_inner.next() else {
                            continue;
                        };
                        let (arg_name, expr_pair) = match (first.as_rule(), arg_inner.next()) {
                            (Rule::ident, Some(expr_pair)) => {
                                (Some(first.as_str().to_owned()), expr_pair)
                            }
                            (Rule::expr, None) => (None, first),
                            _ => continue,
                        };
                        args.push(CallArg {
                            name: arg_name,
                            expr: parse_expr_inner(expr_pair),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    (name, type_args, args)
}

fn parse_call_target(pair: Pair<'_, Rule>) -> (String, Vec<CallArg>, Vec<CallTypeArg>) {
    match pair.as_rule() {
        Rule::path_ident => (pair.as_str().to_owned(), Vec::new(), Vec::new()),
        Rule::namespace_ref => parse_namespace_call_target(pair),
        Rule::call_index_member_target => {
            let (name, args) = parse_call_index_member_target(pair);
            (name, args, Vec::new())
        }
        Rule::call_index_target => {
            let (name, args) = parse_call_index_target(pair);
            (name, args, Vec::new())
        }
        Rule::call_target => {
            let mut inner = pair.into_inner();
            let Some(actual) = inner.next() else {
                return ("".to_owned(), Vec::new(), Vec::new());
            };
            parse_call_target(actual)
        }
        _ => ("".to_owned(), Vec::new(), Vec::new()),
    }
}

fn parse_namespace_call_target(pair: Pair<'_, Rule>) -> (String, Vec<CallArg>, Vec<CallTypeArg>) {
    let raw = pair.as_str().to_owned();
    let mut segment_args = Vec::<Vec<(Option<String>, Expr)>>::new();

    for seg in pair.into_inner() {
        if seg.as_rule() != Rule::namespace_ref_segment {
            continue;
        }
        let mut seg_args = Vec::<(Option<String>, Expr)>::new();
        for item in seg.into_inner() {
            if item.as_rule() != Rule::namespace_call_arg_list {
                continue;
            }
            for arg in item.into_inner() {
                if arg.as_rule() != Rule::namespace_call_arg {
                    continue;
                }
                let mut arg_name = None::<String>;
                let mut arg_expr = None::<Expr>;
                for part in arg.into_inner() {
                    match part.as_rule() {
                        Rule::ident => arg_name = Some(part.as_str().to_owned()),
                        Rule::expr => arg_expr = Some(parse_expr_inner(part)),
                        _ => {}
                    }
                }
                if let Some(expr) = arg_expr {
                    seg_args.push((arg_name, expr));
                }
            }
        }
        segment_args.push(seg_args);
    }

    let Some(last_args) = segment_args.last() else {
        return (raw, Vec::new(), Vec::new());
    };
    if last_args.is_empty() {
        return (raw, Vec::new(), Vec::new());
    }

    let mut type_args = Vec::<CallTypeArg>::new();
    for (name, expr) in last_args {
        if name.is_some() {
            return (raw, Vec::new(), Vec::new());
        }
        let Expr::Var { name: sym, .. } = expr else {
            return (raw, Vec::new(), Vec::new());
        };
        if let Ok(prim) = parse_primitive_type(sym) {
            type_args.push(CallTypeArg::Primitive(prim));
            continue;
        }
        if is_simple_ident(sym) {
            type_args.push(CallTypeArg::Generic(sym.clone()));
            continue;
        }
        return (raw, Vec::new(), Vec::new());
    }

    let Some(name_without_last_args) = strip_trailing_angle_group(&raw) else {
        return (raw, Vec::new(), Vec::new());
    };
    (name_without_last_args, Vec::new(), type_args)
}

fn is_simple_ident(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn strip_trailing_angle_group(text: &str) -> Option<String> {
    if !text.ends_with('>') {
        return None;
    }
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices().rev() {
        match ch {
            '>' => depth += 1,
            '<' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(text[..idx].to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_call_index_target(pair: Pair<'_, Rule>) -> (String, Vec<CallArg>) {
    let mut inner = pair.into_inner();
    let Some(base_pair) = inner.next() else {
        return (PROC_INDEX_CALL_SENTINEL.to_owned(), Vec::new());
    };
    let Some(index_pair) = inner.next() else {
        return (PROC_INDEX_CALL_SENTINEL.to_owned(), Vec::new());
    };
    let base = base_pair.as_str().to_owned();
    (
        PROC_INDEX_CALL_SENTINEL.to_owned(),
        vec![
            CallArg {
                name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                expr: Expr::var(base),
            },
            CallArg {
                name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                expr: parse_expr_inner(index_pair),
            },
        ],
    )
}

fn parse_call_index_member_target(pair: Pair<'_, Rule>) -> (String, Vec<CallArg>) {
    let mut inner = pair.into_inner();
    let Some(base_pair) = inner.next() else {
        return (PROC_INDEX_CALL_SENTINEL.to_owned(), Vec::new());
    };
    let Some(index_pair) = inner.next() else {
        return (PROC_INDEX_CALL_SENTINEL.to_owned(), Vec::new());
    };
    let Some(member_pair) = inner.next() else {
        return (PROC_INDEX_CALL_SENTINEL.to_owned(), Vec::new());
    };
    let base = base_pair.as_str().to_owned();
    let member = member_pair.as_str();
    (
        format!("{PROC_INDEX_CALL_SENTINEL}.{member}"),
        vec![
            CallArg {
                name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                expr: Expr::var(base),
            },
            CallArg {
                name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                expr: parse_expr_inner(index_pair),
            },
        ],
    )
}

fn parse_named_type_ref(
    pair: Pair<'_, Rule>,
) -> Result<(String, Vec<CallTypeArg>), Vec<Diagnostic>> {
    if pair.as_rule() != Rule::named_type {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected named type reference",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut name = None::<String>;
    let mut type_args = Vec::<CallTypeArg>::new();
    for item in pair.into_inner() {
        match item.as_rule() {
            Rule::namespace_ref => {
                let (ns_name, _ns_call_args, ns_type_args) = parse_namespace_call_target(item);
                if name.is_none() {
                    name = Some(ns_name);
                    type_args = ns_type_args;
                }
            }
            Rule::qualified_ident | Rule::ident => {
                if name.is_none() {
                    name = Some(item.as_str().to_owned());
                }
            }
            Rule::generic_type_arg_list => {
                for ty in item.into_inner() {
                    let push_arg = |arg_pair: Pair<'_, Rule>,
                                    out: &mut Vec<CallTypeArg>|
                     -> Result<(), Vec<Diagnostic>> {
                        match arg_pair.as_rule() {
                            Rule::type_name => out.push(CallTypeArg::Primitive(
                                parse_primitive_type(arg_pair.as_str()).map_err(|d| vec![d])?,
                            )),
                            Rule::ident => {
                                out.push(CallTypeArg::Generic(arg_pair.as_str().to_owned()))
                            }
                            _ => {}
                        }
                        Ok(())
                    };
                    match ty.as_rule() {
                        Rule::generic_type_arg => {
                            if let Some(inner) = ty.into_inner().next() {
                                push_arg(inner, &mut type_args)?;
                            }
                        }
                        Rule::type_name | Rule::ident => {
                            push_arg(ty, &mut type_args)?;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing named type reference",
        )]);
    };
    Ok((name, type_args))
}
