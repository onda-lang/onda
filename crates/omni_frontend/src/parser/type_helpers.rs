use super::*;

pub(super) fn parse_section_default_decl_type(
    pair: Pair<'_, Rule>,
    block_name: &str,
) -> Result<DeclType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::section_default_elem_type {
        return Err(vec![Diagnostic::syntax(
            format!("internal parser error: expected {block_name} section default type"),
            0,
            0,
        )]);
    }
    let mut inner = pair.into_inner();
    let Some(actual) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            format!("missing {block_name} section default type"),
            0,
            0,
        )]);
    };
    parse_decl_type(actual)
}

pub(super) fn parse_init_default_decl_type(
    pair: Pair<'_, Rule>,
) -> Result<DeclType, Vec<Diagnostic>> {
    let ty = parse_section_default_decl_type(pair, "init")?;
    match ty {
        DeclType::Scalar(_) | DeclType::Generic(_) => Ok(ty),
        DeclType::Array { .. } | DeclType::ArrayGeneric { .. } => Err(vec![Diagnostic::syntax(
            "init section default type must be a scalar primitive or generic type",
            0,
            0,
        )]),
    }
}

pub(super) fn parse_section_default_buffer_type(
    pair: Pair<'_, Rule>,
    block_name: &str,
) -> Result<BufferType, Vec<Diagnostic>> {
    let decl_ty = parse_section_default_decl_type(pair, block_name)?;
    let elem = match decl_ty {
        DeclType::Scalar(prim) => BufferElemType::Primitive(prim),
        DeclType::Generic(param) => BufferElemType::Generic(param),
        DeclType::Array { .. } | DeclType::ArrayGeneric { .. } => {
            return Err(vec![Diagnostic::syntax(
                format!(
                    "{block_name} section default type must be primitive or generic element type"
                ),
                0,
                0,
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
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected declaration range",
            0,
            0,
        )]);
    }
    let mut inner = pair.into_inner();
    let Some(first_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            "missing declaration range expression",
            0,
            0,
        )]);
    };
    let first = parse_expr_inner(first_pair);
    let second = inner.next().map(parse_expr_inner);
    if inner.next().is_some() {
        return Err(vec![Diagnostic::syntax(
            "declaration range accepts at most two expressions",
            0,
            0,
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
        Rule::qualified_ident => Ok(DeclType::Generic(pair.as_str().to_owned())),
        Rule::array_type => {
            let mut inner = pair.into_inner();
            let Some(elem_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax("missing array element type", 0, 0)]);
            };
            let Some(size_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax("missing array size", 0, 0)]);
            };
            match elem_pair.as_rule() {
                Rule::type_name => {
                    let elem = parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?;
                    Ok(DeclType::Array {
                        elem,
                        size: parse_expr_inner(size_pair),
                    })
                }
                Rule::qualified_ident => Ok(DeclType::ArrayGeneric {
                    elem: elem_pair.as_str().to_owned(),
                    size: parse_expr_inner(size_pair),
                }),
                _ => Err(vec![Diagnostic::syntax(
                    "array declarations for ports/params require primitive or generic element type",
                    0,
                    0,
                )]),
            }
        }
        _ => Err(vec![Diagnostic::syntax(
            "unsupported declaration type",
            0,
            0,
        )]),
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
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected function parameter type",
            0,
            0,
        )]);
    }
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![Diagnostic::syntax(
            "missing function parameter type",
            0,
            0,
        )]);
    };
    let out = match inner.as_rule() {
        Rule::buffer_type => FnParamType::Buffer(parse_buffer_type(inner)?),
        Rule::type_name => {
            FnParamType::Primitive(parse_primitive_type(inner.as_str()).map_err(|d| vec![d])?)
        }
        Rule::qualified_ident => FnParamType::Struct(inner.as_str().to_owned()),
        _ => {
            return Err(vec![Diagnostic::syntax(
                "unsupported function parameter type",
                0,
                0,
            )])
        }
    };
    Ok(out)
}

pub(super) fn parse_event_param_type(
    pair: Pair<'_, Rule>,
) -> Result<EventParamType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::event_param_type {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected event parameter type",
            0,
            0,
        )]);
    }
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![Diagnostic::syntax(
            "missing event parameter type",
            0,
            0,
        )]);
    };
    match inner.as_rule() {
        Rule::type_name => Ok(EventParamType::Scalar(
            parse_primitive_type(inner.as_str()).map_err(|d| vec![d])?,
        )),
        Rule::array_type => {
            let mut array_inner = inner.into_inner();
            let Some(elem_pair) = array_inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing event array element type",
                    0,
                    0,
                )]);
            };
            let Some(size_pair) = array_inner.next() else {
                return Err(vec![Diagnostic::syntax("missing event array size", 0, 0)]);
            };
            match elem_pair.as_rule() {
                Rule::type_name => Ok(EventParamType::Array {
                    elem: parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?,
                    size: parse_expr_inner(size_pair),
                }),
                _ => Err(vec![Diagnostic::syntax(
                    "event array parameters require primitive element type",
                    0,
                    0,
                )]),
            }
        }
        _ => Err(vec![Diagnostic::syntax(
            "unsupported event parameter type",
            0,
            0,
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

pub(super) fn parse_array_type_spec(
    pair: Pair<'_, Rule>,
) -> Result<ArrayTypeSpec, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::array_type {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected array type",
            0,
            0,
        )]);
    }
    let mut inner = pair.into_inner();
    let Some(elem_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing array element type", 0, 0)]);
    };
    let Some(size_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing array size", 0, 0)]);
    };
    let elem = match elem_pair.as_rule() {
        Rule::type_name => parse_array_elem_type(elem_pair.as_str()),
        Rule::qualified_ident => ArrayElemType::Struct(elem_pair.as_str().to_owned()),
        _ => return Err(vec![Diagnostic::syntax("invalid array element type", 0, 0)]),
    };
    Ok(ArrayTypeSpec {
        elem,
        size: Box::new(parse_expr_inner(size_pair)),
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
            Rule::qualified_ident => {
                return Ok(FieldType::Generic(child.as_str().to_owned()));
            }
            Rule::array_type => {
                return Ok(FieldType::Array(parse_array_type_spec(child)?));
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
        Rule::index_target => {
            let mut inner = pair.into_inner();
            let Some(base_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing indexed assignment base",
                    0,
                    0,
                )]);
            };
            let Some(index_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing indexed assignment index",
                    0,
                    0,
                )]);
            };
            if inner.next().is_some() {
                return Err(vec![Diagnostic::syntax(
                    "nested indexed assignment targets must use parser rewrite path",
                    0,
                    0,
                )]);
            }
            Ok(AssignTarget::Index {
                base: base_pair.as_str().to_owned(),
                index: parse_expr(index_pair)?,
            })
        }
        Rule::assign_target => {
            let mut inner = pair.into_inner();
            let Some(inner_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax("missing assignment target", 0, 0)]);
            };
            parse_assign_target(inner_pair)
        }
        _ => Err(vec![Diagnostic::syntax(
            "unexpected assignment target",
            0,
            0,
        )]),
    }
}
