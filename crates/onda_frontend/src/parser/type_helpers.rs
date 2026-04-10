use super::*;

pub(super) fn parse_section_default_decl_type(
    pair: Pair<'_, Rule>,
    block_name: &str,
) -> Result<DeclType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::section_default_decl_type
        && pair.as_rule() != Rule::section_default_elem_type
    {
        return Err(vec![syntax_at_pair(
            &pair,
            format!("internal parser error: expected {block_name} section default type"),
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(actual) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            format!("missing {block_name} section default type"),
        )]);
    };
    parse_decl_type(actual)
}

pub(super) fn parse_init_default_decl_type(
    pair: Pair<'_, Rule>,
) -> Result<DeclType, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let ty = parse_section_default_decl_type(pair, "init")?;
    match ty {
        DeclType::Scalar(_) | DeclType::Generic(_) => Ok(ty),
        DeclType::Array { .. } | DeclType::ArrayGeneric { .. } | DeclType::Tuple(_) => {
            Err(vec![syntax_at_loc(
                loc.as_ref(),
                "init section default type must be a scalar primitive or generic type",
            )])
        }
    }
}

pub(super) fn parse_section_default_buffer_type(
    pair: Pair<'_, Rule>,
    block_name: &str,
) -> Result<BufferType, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let decl_ty = parse_section_default_decl_type(pair, block_name)?;
    let elem = match decl_ty {
        DeclType::Scalar(prim) => BufferElemType::Primitive(prim),
        DeclType::Generic(param) => BufferElemType::Generic(param),
        DeclType::Array { .. } | DeclType::ArrayGeneric { .. } | DeclType::Tuple(_) => {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                format!(
                    "{block_name} section default type must be primitive or generic element type"
                ),
            )])
        }
    };
    Ok(BufferType {
        elem,
        channels: BufferChannels::Mono,
    })
}

pub(super) fn parse_decl_range_pair(pair: Pair<'_, Rule>) -> Result<DeclRange, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::decl_range {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected declaration range",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(first_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing declaration range expression",
        )]);
    };
    let first = parse_expr_inner(first_pair);
    let second = inner.next().map(parse_expr_inner);
    if inner.next().is_some() {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "declaration range accepts at most two expressions",
        )]);
    }
    let (min, max) = match second {
        Some(max) => (Some(first), max),
        None => (None, first),
    };
    Ok(DeclRange { min, max })
}

pub(super) fn parse_int(text: &str) -> Result<i32, Vec<Diagnostic>> {
    text.parse::<i32>().map_err(|_| {
        vec![Diagnostic::syntax(
            format!("invalid integer literal '{text}'"),
            0,
            0,
        )]
    })
}

pub(super) fn parse_primitive_type(text: &str) -> Result<PrimitiveType, Diagnostic> {
    match text {
        "f32" => Ok(PrimitiveType::F32),
        "f64" => Ok(PrimitiveType::F64),
        "i32" => Ok(PrimitiveType::I32),
        "i64" => Ok(PrimitiveType::I64),
        "bool" => Ok(PrimitiveType::Bool),
        _ => Err(Diagnostic::syntax(
            format!("unsupported primitive type '{text}'"),
            0,
            0,
        )),
    }
}

pub(super) fn parse_decl_type(pair: Pair<'_, Rule>) -> Result<DeclType, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::type_name => Ok(DeclType::Scalar(
            parse_primitive_type(pair.as_str()).map_err(|d| vec![d])?,
        )),
        Rule::qualified_ident | Rule::namespace_ref | Rule::named_type => {
            Ok(DeclType::Generic(pair.as_str().trim().to_owned()))
        }
        Rule::array_type => {
            let loc = stmt_loc_from_pair(&pair);
            let mut inner = pair.into_inner();
            let Some(elem_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing array element type",
                )]);
            };
            let Some(size_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(loc.as_ref(), "missing array size")]);
            };
            match elem_pair.as_rule() {
                Rule::type_name => {
                    let elem = parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?;
                    Ok(DeclType::Array {
                        elem,
                        size: parse_expr_inner(size_pair),
                    })
                }
                Rule::qualified_ident | Rule::namespace_ref | Rule::named_type => {
                    Ok(DeclType::ArrayGeneric {
                        elem: elem_pair.as_str().trim().to_owned(),
                        size: parse_expr_inner(size_pair),
                    })
                }
                _ => Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "array declarations for ports/params require primitive or generic element type",
                )]),
            }
        }
        Rule::tuple_type => {
            let elems: Result<Vec<PrimitiveType>, Vec<Diagnostic>> = pair
                .into_inner()
                .filter(|p| p.as_rule() == Rule::type_name)
                .map(|p| parse_primitive_type(p.as_str()).map_err(|d| vec![d]))
                .collect();
            Ok(DeclType::Tuple(elems?))
        }
        _ => Err(vec![syntax_at_pair(&pair, "unsupported declaration type")]),
    }
}

pub(super) fn parse_builtin_fn(name: &str) -> Option<BuiltinFn> {
    match name {
        "sin" => Some(BuiltinFn::Sin),
        "cos" => Some(BuiltinFn::Cos),
        "tan" => Some(BuiltinFn::Tan),
        "tanh" => Some(BuiltinFn::Tanh),
        "atan" => Some(BuiltinFn::Atan),
        "atan2" => Some(BuiltinFn::Atan2),
        "exp" => Some(BuiltinFn::Exp),
        "log" => Some(BuiltinFn::Log),
        "sqrt" => Some(BuiltinFn::Sqrt),
        "pow" => Some(BuiltinFn::Pow),
        "abs" | "fabs" => Some(BuiltinFn::Abs),
        "floor" => Some(BuiltinFn::Floor),
        "ceil" => Some(BuiltinFn::Ceil),
        "round" => Some(BuiltinFn::Round),
        "trunc" => Some(BuiltinFn::Trunc),
        "min" => Some(BuiltinFn::Min),
        "max" => Some(BuiltinFn::Max),
        "fma" => Some(BuiltinFn::Fma),
        _ => None,
    }
}

pub(super) fn parse_fn_param_type(pair: Pair<'_, Rule>) -> Result<FnParamType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::fn_param_type {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected function parameter type",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing function parameter type",
        )]);
    };
    let out = match inner.as_rule() {
        Rule::fn_typed_array_param => {
            let inner_type = inner.into_inner().next().unwrap();
            match inner_type.as_rule() {
                Rule::type_name => {
                    let prim = parse_primitive_type(inner_type.as_str()).map_err(|d| vec![d])?;
                    FnParamType::Array(Some(prim))
                }
                Rule::qualified_ident | Rule::namespace_ref => {
                    FnParamType::ArrayGeneric(inner_type.as_str().trim().to_owned())
                }
                _ => {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "unsupported typed array parameter element type",
                    )])
                }
            }
        }
        Rule::fn_sized_array_param => {
            let mut inner_pairs = inner.into_inner();
            let elem_pair = inner_pairs.next().unwrap();
            let size_pair = inner_pairs.next().unwrap();
            let size_expr = super::expr_stmt::parse_expr_inner(size_pair);
            match elem_pair.as_rule() {
                Rule::type_name => {
                    let prim = parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?;
                    FnParamType::SizedArray {
                        elem: Some(prim),
                        generic_name: None,
                        size: size_expr,
                    }
                }
                Rule::qualified_ident | Rule::namespace_ref => FnParamType::SizedArray {
                    elem: None,
                    generic_name: Some(elem_pair.as_str().trim().to_owned()),
                    size: size_expr,
                },
                _ => {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "unsupported sized array parameter element type",
                    )])
                }
            }
        }
        Rule::fn_untyped_array_param => FnParamType::Array(None),
        Rule::fn_bare_buffer => FnParamType::BareBuffer,
        Rule::fn_tuple_param => {
            let elems: Result<Vec<PrimitiveType>, Vec<Diagnostic>> = inner
                .into_inner()
                .filter(|p| p.as_rule() == Rule::type_name)
                .map(|p| parse_primitive_type(p.as_str()).map_err(|d| vec![d]))
                .collect();
            FnParamType::Tuple(elems?)
        }
        Rule::buffer_type => FnParamType::Buffer(parse_buffer_type(inner)?),
        Rule::type_name => {
            FnParamType::Primitive(parse_primitive_type(inner.as_str()).map_err(|d| vec![d])?)
        }
        Rule::qualified_ident | Rule::namespace_ref => {
            FnParamType::Struct(inner.as_str().trim().to_owned())
        }
        _ => {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                "unsupported function parameter type",
            )])
        }
    };
    Ok(out)
}

pub(super) fn parse_event_param_type(
    pair: Pair<'_, Rule>,
) -> Result<EventParamType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::event_param_type {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected event parameter type",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing event parameter type",
        )]);
    };
    match inner.as_rule() {
        Rule::type_name => Ok(EventParamType::Scalar(
            parse_primitive_type(inner.as_str()).map_err(|d| vec![d])?,
        )),
        Rule::fn_typed_array_param => {
            let Some(elem_pair) = inner.into_inner().next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing event slice element type",
                )]);
            };
            match elem_pair.as_rule() {
                Rule::type_name => Ok(EventParamType::Slice {
                    elem: parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?,
                }),
                Rule::qualified_ident | Rule::namespace_ref => Ok(EventParamType::GenericSlice {
                    elem: elem_pair.as_str().trim().to_owned(),
                }),
                _ => Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "event slice parameters require primitive or generic primitive element type",
                )]),
            }
        }
        Rule::array_type => {
            let mut array_inner = inner.into_inner();
            let Some(elem_pair) = array_inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing event array element type",
                )]);
            };
            let Some(size_pair) = array_inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing event array size",
                )]);
            };
            match elem_pair.as_rule() {
                Rule::type_name => Ok(EventParamType::Array {
                    elem: parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?,
                    size: parse_expr_inner(size_pair),
                }),
                _ => Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "event array parameters require primitive element type",
                )]),
            }
        }
        _ => Err(vec![syntax_at_loc(
            loc.as_ref(),
            "unsupported event parameter type",
        )]),
    }
}

pub(super) fn parse_buffer_type(pair: Pair<'_, Rule>) -> Result<BufferType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::buffer_type {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected buffer type",
            0,
            0,
        )]);
    }
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![Diagnostic::syntax(
            "missing buffer type contents",
            0,
            0,
        )]);
    };
    if inner.as_rule() != Rule::buffer_inner {
        return Err(vec![Diagnostic::syntax(
            "invalid buffer type contents",
            0,
            0,
        )]);
    }
    parse_buffer_inner(inner)
}

pub(super) fn parse_buffer_decl_type(pair: Pair<'_, Rule>) -> Result<BufferType, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::buffer_decl_type => {
            let mut inner = pair.into_inner();
            let Some(actual) = inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing buffers declaration type",
                    0,
                    0,
                )]);
            };
            parse_buffer_decl_type(actual)
        }
        Rule::buffer_type => parse_buffer_type(pair),
        Rule::buffer_inner => parse_buffer_inner(pair),
        _ => Err(vec![Diagnostic::syntax(
            "invalid buffers declaration type",
            0,
            0,
        )]),
    }
}

pub(super) fn parse_buffer_inner(
    inner_pair: Pair<'_, Rule>,
) -> Result<BufferType, Vec<Diagnostic>> {
    // Rule::buffer_inner shape:
    //   (type_name|qualified_ident)
    //   (type_name|qualified_ident) "[" "]"
    //   (type_name|qualified_ident) "[" expr "]"
    let text = inner_pair.as_str().trim().to_owned();
    let mut iter = inner_pair.into_inner();
    let Some(elem_pair) = iter.next() else {
        return Err(vec![Diagnostic::syntax(
            "missing buffer element type",
            0,
            0,
        )]);
    };
    let elem = match elem_pair.as_rule() {
        Rule::type_name => BufferElemType::Primitive(
            parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?,
        ),
        Rule::qualified_ident => BufferElemType::Generic(elem_pair.as_str().to_owned()),
        _ => {
            return Err(vec![Diagnostic::syntax(
                "missing or invalid buffer element type",
                0,
                0,
            )]);
        }
    };
    let size_expr = iter.next().map(parse_expr_inner);
    let has_brackets = text.contains('[');
    let channels = if !has_brackets {
        BufferChannels::Mono
    } else if let Some(expr) = size_expr {
        BufferChannels::Static(expr)
    } else {
        BufferChannels::Dynamic
    };
    Ok(BufferType { elem, channels })
}
pub(super) fn parse_array_elem_type(text: &str) -> ArrayElemType {
    match parse_primitive_type(text) {
        Ok(prim) => ArrayElemType::Primitive(prim),
        Err(_) => ArrayElemType::Struct(text.to_owned()),
    }
}

fn is_primitive_type_name(text: &str) -> bool {
    matches!(text, "f32" | "f64" | "i32" | "i64" | "bool")
}

pub(super) fn parse_array_type_spec(
    pair: Pair<'_, Rule>,
) -> Result<ArrayTypeSpec, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::array_type {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected array type",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(elem_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing array element type",
        )]);
    };
    let Some(size_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing array size")]);
    };
    let elem = match elem_pair.as_rule() {
        Rule::type_name => parse_array_elem_type(elem_pair.as_str()),
        Rule::qualified_ident | Rule::namespace_ref | Rule::named_type => {
            ArrayElemType::Struct(elem_pair.as_str().trim().to_owned())
        }
        _ => {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                "invalid array element type",
            )])
        }
    };
    let size = parse_expr_inner(size_pair);
    if matches!(elem, ArrayElemType::Struct(_))
        && matches!(&size, Expr::Var { name, .. } if is_primitive_type_name(name))
    {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "generic type arguments must use '<...>'; bracket syntax is reserved for array sizes",
        )]);
    }
    Ok(ArrayTypeSpec {
        elem,
        size: Box::new(size),
    })
}

pub(super) fn parse_field_type(pair: Pair<'_, Rule>) -> Result<FieldType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::field_type {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected field type",
            0,
            0,
        )]);
    }
    let text = pair.as_str().trim().to_owned();

    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::type_name => {
                let ty = parse_primitive_type(child.as_str()).map_err(|d| vec![d])?;
                return Ok(FieldType::Scalar(ty));
            }
            Rule::qualified_ident | Rule::namespace_ref | Rule::named_type => {
                return Ok(FieldType::Generic(child.as_str().trim().to_owned()));
            }
            Rule::array_type => {
                return Ok(FieldType::Array(parse_array_type_spec(child)?));
            }
            Rule::tuple_type => {
                let elems: Result<Vec<PrimitiveType>, Vec<Diagnostic>> = child
                    .into_inner()
                    .filter(|p| p.as_rule() == Rule::type_name)
                    .map(|p| parse_primitive_type(p.as_str()).map_err(|d| vec![d]))
                    .collect();
                return Ok(FieldType::Tuple(elems?));
            }
            _ => {}
        }
    }

    Err(vec![Diagnostic::syntax(
        format!("unsupported struct field type '{text}'"),
        0,
        0,
    )])
}

pub(super) fn parse_assign_target(pair: Pair<'_, Rule>) -> Result<AssignTarget, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::path_ident => Ok(AssignTarget::Var(pair.as_str().to_owned())),
        Rule::slice_target => {
            let loc = stmt_loc_from_pair(&pair);
            let mut inner = pair.into_inner();
            let Some(base_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing sliced assignment base",
                )]);
            };
            let mut start = None::<Expr>;
            let mut end = None::<Expr>;
            for bound in inner {
                match bound.as_rule() {
                    Rule::slice_start => {
                        let expr = bound.into_inner().next().ok_or_else(|| {
                            vec![syntax_at_loc(
                                loc.as_ref(),
                                "missing sliced assignment start bound",
                            )]
                        })?;
                        start = Some(parse_expr(expr)?);
                    }
                    Rule::slice_end => {
                        let expr = bound.into_inner().next().ok_or_else(|| {
                            vec![syntax_at_loc(
                                loc.as_ref(),
                                "missing sliced assignment end bound",
                            )]
                        })?;
                        end = Some(parse_expr(expr)?);
                    }
                    _ => {}
                }
            }
            Ok(AssignTarget::Slice {
                base: base_pair.as_str().to_owned(),
                start,
                end,
            })
        }
        Rule::index_target => {
            let loc = stmt_loc_from_pair(&pair);
            let mut inner = pair.into_inner();
            let Some(base_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing indexed assignment base",
                )]);
            };
            let Some(index_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing indexed assignment index",
                )]);
            };
            if inner.next().is_some() {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "nested indexed assignment targets must use parser rewrite path",
                )]);
            }
            Ok(AssignTarget::Index {
                base: base_pair.as_str().to_owned(),
                index: parse_expr(index_pair)?,
            })
        }
        Rule::indexed_member_target => {
            let loc = stmt_loc_from_pair(&pair);
            let mut inner = pair.into_inner();
            let Some(base_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing indexed member assignment base",
                )]);
            };
            let Some(index_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing indexed member assignment index",
                )]);
            };
            let Some(field_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing indexed member assignment field",
                )]);
            };
            Ok(AssignTarget::Index {
                base: format!("{}.{}", base_pair.as_str(), field_pair.as_str()),
                index: parse_expr(index_pair)?,
            })
        }
        Rule::tuple_target => {
            let targets: Vec<String> = pair
                .into_inner()
                .filter(|p| p.as_rule() == Rule::ident)
                .map(|p| p.as_str().to_owned())
                .collect();
            Ok(AssignTarget::Tuple(targets))
        }
        Rule::assign_target => {
            let loc = stmt_loc_from_pair(&pair);
            let mut inner = pair.into_inner();
            let Some(inner_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing assignment target",
                )]);
            };
            parse_assign_target(inner_pair)
        }
        _ => Err(vec![syntax_at_pair(&pair, "unexpected assignment target")]),
    }
}
