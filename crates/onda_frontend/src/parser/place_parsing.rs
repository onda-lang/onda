use super::*;

enum PlaceSegment {
    Field(String),
    Indices(Vec<Expr>),
}

const INTERNAL_PLACE_PREFIX: &str = "__onda_place_";

pub(super) fn is_internal_place_name(name: &str) -> bool {
    name.starts_with(INTERNAL_PLACE_PREFIX)
}

pub(super) fn parse_stmt_expanded(pair: Pair<'_, Rule>) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::assign_stmt {
        return parse_stmt(pair).map(|statement| vec![statement]);
    }

    let loc = stmt_loc_from_pair(&pair);
    let Some(kind) = pair.clone().into_inner().next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing assignment statement",
        )]);
    };
    match kind.as_rule() {
        Rule::compound_assign_stmt => parse_compound_assignment(kind, loc),
        Rule::plain_assign_stmt => parse_plain_assignment(pair, kind, loc),
        _ => parse_assign_stmt(pair).map(|statement| vec![statement]),
    }
}

fn parse_compound_assignment(
    pair: Pair<'_, Rule>,
    loc: Span,
) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    let mut parts = pair.into_inner();
    let target = parts.next().ok_or_else(|| {
        vec![syntax_at_loc(
            loc.as_ref(),
            "missing compound assignment target",
        )]
    })?;
    let operator = parts.next().ok_or_else(|| {
        vec![syntax_at_loc(
            loc.as_ref(),
            "missing compound assignment operator",
        )]
    })?;
    let rhs = parts.next().ok_or_else(|| {
        vec![syntax_at_loc(
            loc.as_ref(),
            "missing compound assignment expression",
        )]
    })?;
    let op = parse_compound_op(&operator)?;
    let rhs = parse_expr(rhs)?;
    if target.as_rule() == Rule::deep_place_target {
        return lower_deep_place(target, rhs, Some(op), loc);
    }

    let target_loc = stmt_loc_from_pair(&target);
    let target = parse_assign_target(target)?;
    let mut statements = Vec::new();
    let mut ordinal = 0;
    finish_assignment(
        loc,
        target_loc,
        target,
        rhs,
        Some(op),
        &mut statements,
        &mut ordinal,
    )?;
    Ok(statements)
}

fn parse_plain_assignment(
    statement: Pair<'_, Rule>,
    pair: Pair<'_, Rule>,
    loc: Span,
) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    let mut parts = pair.into_inner();
    let Some(target) = parts.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing assignment target",
        )]);
    };
    if target.as_rule() != Rule::deep_place_target {
        return parse_assign_stmt(statement).map(|statement| vec![statement]);
    }
    let rhs = parts
        .next()
        .ok_or_else(|| vec![syntax_at_loc(loc.as_ref(), "missing assignment expression")])?;
    lower_deep_place(target, parse_expr(rhs)?, None, loc)
}

fn parse_deep_place(pair: Pair<'_, Rule>) -> Result<(String, Vec<PlaceSegment>), Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(base) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing assignment place base",
        )]);
    };
    let mut segments = Vec::new();
    for part in inner {
        match part.as_rule() {
            Rule::index_group => {
                segments.push(PlaceSegment::Indices(parse_index_groups(vec![part], loc)?));
            }
            Rule::place_field => {
                let Some(field) = part.into_inner().next() else {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "missing assignment place field",
                    )]);
                };
                segments.push(PlaceSegment::Field(field.as_str().to_owned()));
            }
            _ => {}
        }
    }
    Ok((pair_symbol_text(&base), segments))
}

fn parse_compound_op(pair: &Pair<'_, Rule>) -> Result<BinaryOp, Vec<Diagnostic>> {
    match pair.as_str() {
        "+=" => Ok(BinaryOp::Add),
        "-=" => Ok(BinaryOp::Sub),
        "*=" => Ok(BinaryOp::Mul),
        "/=" => Ok(BinaryOp::Div),
        "%=" => Ok(BinaryOp::Mod),
        "&=" => Ok(BinaryOp::BitAnd),
        "|=" => Ok(BinaryOp::BitOr),
        "^=" => Ok(BinaryOp::BitXor),
        "<<=" => Ok(BinaryOp::ShiftLeft),
        ">>=" => Ok(BinaryOp::ShiftRight),
        other => Err(vec![syntax_at_pair(
            pair,
            format!("unknown compound assignment operator '{other}'"),
        )]),
    }
}

fn internal_place_name(loc: Span, ordinal: usize, kind: &str) -> String {
    let source: SourceLoc = loc.into();
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in source
        .file()
        .into_iter()
        .chain(source.trace())
        .flat_map(|part| part.into_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!(
        "{INTERNAL_PLACE_PREFIX}{kind}_{hash:016x}_{}_{}_{}",
        source.line, source.column, ordinal
    )
}

fn internal_assign(name: String, expr: Expr) -> Stmt {
    let loc = Span::from(expr.loc());
    Stmt::Assign {
        loc,
        target_loc: loc,
        target: AssignTarget::Var(name),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Span::ZERO,
        expr,
    }
}

fn finish_assignment(
    loc: Span,
    target_loc: Span,
    mut target: AssignTarget,
    rhs: Expr,
    op: Option<BinaryOp>,
    statements: &mut Vec<Stmt>,
    ordinal: &mut usize,
) -> Result<(), Vec<Diagnostic>> {
    let rhs = if let Some(op) = op {
        target.visit_selectors_mut(|selector| {
            if matches!(selector, Expr::Int { .. } | Expr::Number { .. }) {
                return;
            }
            let selector_loc = selector.loc();
            let name = internal_place_name(loc, *ordinal, "selector");
            *ordinal += 1;
            statements.push(internal_assign(name.clone(), selector.clone()));
            *selector = Expr::var(name).with_loc(selector_loc);
        });
        let lhs = target_read(&target, target_loc)?;
        Expr::Binary {
            loc,
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    } else {
        rhs
    };
    statements.push(Stmt::Assign {
        loc,
        target_loc,
        target,
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Span::ZERO,
        expr: rhs,
    });
    Ok(())
}

fn target_read(target: &AssignTarget, loc: Span) -> Result<Expr, Vec<Diagnostic>> {
    match target {
        AssignTarget::Var(name) => Ok(Expr::var(name.clone())),
        AssignTarget::Index { base, index } => Ok(Expr::Index {
            loc,
            base: base.clone(),
            index: Box::new(index.clone()),
        }),
        AssignTarget::Slice {
            base,
            selector,
            channel,
            start,
            end,
        } => Ok(Expr::Slice {
            loc,
            base: base.clone(),
            selector: selector.clone(),
            channel: channel.clone(),
            start: start.clone(),
            end: end.clone(),
        }),
        AssignTarget::IndexedMember {
            base,
            index,
            field,
            field_index,
        } => {
            let (name, args) = if let Some(field_index) = field_index {
                (
                    STRUCT_ARRAY_FIELD_INDEX_SENTINEL.to_owned(),
                    vec![
                        CallArg {
                            name: Some(SAFI_BASE_ARG.to_owned()),
                            expr: Expr::var(base.clone()),
                        },
                        CallArg {
                            name: Some(SAFI_IDX_ARG.to_owned()),
                            expr: index.clone(),
                        },
                        CallArg {
                            name: Some(SAFI_FIELD_ARG.to_owned()),
                            expr: Expr::var(field.clone()),
                        },
                        CallArg {
                            name: Some(SAFI_FIELD_IDX_ARG.to_owned()),
                            expr: field_index.as_ref().clone(),
                        },
                    ],
                )
            } else {
                (
                    format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"),
                    vec![
                        CallArg {
                            name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                            expr: Expr::var(base.clone()),
                        },
                        CallArg {
                            name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                            expr: index.clone(),
                        },
                        CallArg {
                            name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                            expr: Expr::var(field.clone()),
                        },
                    ],
                )
            };
            Ok(Expr::UserCall {
                loc,
                name,
                type_args: Vec::new(),
                args,
            })
        }
        AssignTarget::Tuple(_) => Err(vec![syntax_at_loc(
            loc.as_ref(),
            "compound assignment does not support tuple destinations",
        )]),
    }
}

fn lower_deep_place(
    target_pair: Pair<'_, Rule>,
    rhs: Expr,
    op: Option<BinaryOp>,
    loc: Span,
) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    let target_loc = stmt_loc_from_pair(&target_pair);
    let (mut base, segments) = parse_deep_place(target_pair)?;
    let mut statements = Vec::new();
    let mut ordinal = 0;
    match segments.as_slice() {
        [PlaceSegment::Indices(indices), PlaceSegment::Field(field)] if indices.len() == 1 => {
            finish_assignment(
                loc,
                target_loc,
                AssignTarget::IndexedMember {
                    base,
                    index: indices[0].clone(),
                    field: field.clone(),
                    field_index: None,
                },
                rhs,
                op,
                &mut statements,
                &mut ordinal,
            )?;
            return Ok(statements);
        }
        [PlaceSegment::Indices(indices), PlaceSegment::Field(field), PlaceSegment::Indices(field_indices)]
            if indices.len() == 1 && field_indices.len() == 1 =>
        {
            finish_assignment(
                loc,
                target_loc,
                AssignTarget::IndexedMember {
                    base,
                    index: indices[0].clone(),
                    field: field.clone(),
                    field_index: Some(Box::new(field_indices[0].clone())),
                },
                rhs,
                op,
                &mut statements,
                &mut ordinal,
            )?;
            return Ok(statements);
        }
        _ => {}
    }
    let mut cursor = 0;
    while cursor < segments.len() {
        match &segments[cursor] {
            PlaceSegment::Field(field) => {
                base.push('.');
                base.push_str(field);
                cursor += 1;
            }
            PlaceSegment::Indices(indices) => {
                let remaining_are_indices = segments[cursor..]
                    .iter()
                    .all(|segment| matches!(segment, PlaceSegment::Indices(_)));
                if remaining_are_indices {
                    return finish_indexed_assignment(
                        loc,
                        target_loc,
                        base,
                        &segments[cursor..],
                        rhs,
                        op,
                        statements,
                        ordinal,
                    );
                }
                if indices.len() != 1 {
                    return Err(vec![syntax_at_loc(
                        target_loc.as_ref(),
                        "an intermediate assignment-place index must have one coordinate",
                    )]);
                }
                let alias = internal_place_name(loc, ordinal, "element");
                ordinal += 1;
                statements.push(internal_assign(
                    alias.clone(),
                    Expr::Index {
                        loc: target_loc,
                        base,
                        index: Box::new(indices[0].clone()),
                    },
                ));
                base = alias;
                cursor += 1;
            }
        }
    }
    finish_assignment(
        loc,
        target_loc,
        AssignTarget::Var(base),
        rhs,
        op,
        &mut statements,
        &mut ordinal,
    )?;
    Ok(statements)
}

#[allow(clippy::too_many_arguments)]
fn finish_indexed_assignment(
    loc: Span,
    target_loc: Span,
    base: String,
    groups: &[PlaceSegment],
    rhs: Expr,
    op: Option<BinaryOp>,
    mut statements: Vec<Stmt>,
    mut ordinal: usize,
) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    let mut coordinates = groups
        .iter()
        .flat_map(|segment| match segment {
            PlaceSegment::Indices(values) => values.clone(),
            PlaceSegment::Field(_) => Vec::new(),
        })
        .collect::<Vec<_>>();
    if coordinates.len() == 1 {
        finish_assignment(
            loc,
            target_loc,
            AssignTarget::Index {
                base,
                index: coordinates.remove(0),
            },
            rhs,
            op,
            &mut statements,
            &mut ordinal,
        )?;
        return Ok(statements);
    }
    if !(2..=3).contains(&coordinates.len()) {
        return Err(vec![syntax_at_loc(
            target_loc.as_ref(),
            "buffer assignment requires one, two, or three coordinates",
        )]);
    }

    if op.is_some() {
        for coordinate in &mut coordinates {
            let name = internal_place_name(loc, ordinal, "selector");
            ordinal += 1;
            statements.push(internal_assign(name.clone(), coordinate.clone()));
            *coordinate = Expr::var(name);
        }
    }
    let (read_name, write_name) = buffer_access_names(groups.len(), coordinates.len());
    let value = if let Some(op) = op {
        Expr::Binary {
            loc,
            op,
            lhs: Box::new(Expr::UserCall {
                loc: target_loc,
                name: read_name.to_owned(),
                type_args: Vec::new(),
                args: std::iter::once(CallArg {
                    name: None,
                    expr: Expr::var(base.clone()),
                })
                .chain(
                    coordinates
                        .iter()
                        .cloned()
                        .map(|expr| CallArg { name: None, expr }),
                )
                .collect(),
            }),
            rhs: Box::new(rhs),
        }
    } else {
        rhs
    };
    let args = std::iter::once(CallArg {
        name: None,
        expr: Expr::var(base),
    })
    .chain(
        coordinates
            .into_iter()
            .map(|expr| CallArg { name: None, expr }),
    )
    .chain(std::iter::once(CallArg {
        name: None,
        expr: value,
    }))
    .collect();
    statements.push(Stmt::Expr {
        loc,
        expr: Expr::UserCall {
            loc: target_loc,
            name: write_name.to_owned(),
            type_args: Vec::new(),
            args,
        },
    });
    Ok(statements)
}

fn buffer_access_names(
    group_count: usize,
    coordinate_count: usize,
) -> (&'static str, &'static str) {
    if coordinate_count == 3 {
        (INTERNAL_BUFFER_READ3_FN, INTERNAL_BUFFER_WRITE3_FN)
    } else if group_count == 1 {
        (
            INTERNAL_BUFFER_READ_CHANNEL_FN,
            INTERNAL_BUFFER_WRITE_CHANNEL_FN,
        )
    } else {
        (INTERNAL_BUFFER_READ2_FN, INTERNAL_BUFFER_WRITE2_FN)
    }
}
