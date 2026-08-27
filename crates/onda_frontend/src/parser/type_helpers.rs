use super::*;
use crate::ast::{FnReturnScalarType, FnReturnType};

pub(super) fn parse_section_default_decl_type(
    pair: Pair<'_, Rule>,
    block_name: &str,
) -> Result<DeclType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::section_default_decl_type {
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
    if pair.as_rule() != Rule::section_default_buffer_type {
        return Err(vec![syntax_at_pair(
            &pair,
            format!("internal parser error: expected {block_name} section default buffer type"),
        )]);
    }
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            format!("missing {block_name} section default buffer type"),
        )]);
    };
    parse_buffer_inner(inner)
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

pub(super) struct ParsedBindingAttributes {
    pub(super) range: Option<(BuiltinFn, Expr, Expr)>,
}

pub(super) fn parse_binding_range_pair(
    pair: Pair<'_, Rule>,
) -> Result<ParsedBindingAttributes, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::binding_range {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected binding range",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut domain = None;
    let mut mode = None;
    let mut saw_named_or_mode = false;
    for item in pair.into_inner() {
        let Some(value) = item.into_inner().next() else {
            continue;
        };
        match value.as_rule() {
            Rule::expr | Rule::binding_range_spec => {
                if saw_named_or_mode {
                    return Err(vec![syntax_at_pair(
                        &value,
                        "positional binding range domain must precede named fields and mode",
                    )]);
                }
                let parsed = parse_binding_range_domain(value)?;
                if domain.replace(parsed).is_some() {
                    return Err(vec![syntax_at_loc(
                        loc.as_ref(),
                        "binding range count and range domains are mutually exclusive",
                    )]);
                }
            }
            Rule::binding_range_named_count | Rule::binding_range_named_range => {
                saw_named_or_mode = true;
                let field_loc = stmt_loc_from_pair(&value);
                let mut fields = value.into_inner();
                let Some(field) = fields.next() else {
                    return Err(vec![syntax_at_loc(
                        field_loc.as_ref(),
                        "missing binding range domain",
                    )]);
                };
                let parsed = match field.as_rule() {
                    Rule::expr => BindingRangeDomain::Count(parse_expr_inner(field)),
                    Rule::binding_range_spec => parse_binding_range_domain(field)?,
                    _ => unreachable!("binding range field returned an unexpected rule"),
                };
                if domain.replace(parsed).is_some() {
                    return Err(vec![syntax_at_loc(
                        field_loc.as_ref(),
                        "binding range count and range domains are mutually exclusive",
                    )]);
                }
            }
            Rule::binding_range_named_mode => {
                saw_named_or_mode = true;
                let mode_loc = stmt_loc_from_pair(&value);
                let mode_value = value
                    .into_inner()
                    .next()
                    .expect("named binding range mode has a value");
                if mode.replace(mode_value.as_str().to_owned()).is_some() {
                    return Err(vec![syntax_at_loc(
                        mode_loc.as_ref(),
                        "duplicate binding range field 'mode'",
                    )]);
                }
            }
            Rule::binding_range_mode => {
                saw_named_or_mode = true;
                if mode.replace(value.as_str().to_owned()).is_some() {
                    return Err(vec![syntax_at_pair(&value, "duplicate binding range mode")]);
                }
            }
            _ => unreachable!("binding range item grammar returned an unexpected rule"),
        }
    }
    let Some(domain) = domain else {
        if mode.is_some() {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                "binding range mode requires a count or range domain",
            )]);
        }
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "binding attributes require a count or range",
        )]);
    };
    let (begin, end, domain_kind) = match domain {
        BindingRangeDomain::Count(count) => (Expr::int(0), count, BindingRangeDomainKind::Count),
        BindingRangeDomain::Range {
            begin,
            end,
            inclusive,
        } => (
            begin,
            end,
            if inclusive {
                BindingRangeDomainKind::Inclusive
            } else {
                BindingRangeDomainKind::Exclusive
            },
        ),
    };
    let func = match (mode.as_deref().unwrap_or("clamp"), domain_kind) {
        ("clamp", BindingRangeDomainKind::Count) => BuiltinFn::BindingCountClamp,
        ("clamp", BindingRangeDomainKind::Exclusive) => BuiltinFn::BindingRangeClamp,
        ("clamp", BindingRangeDomainKind::Inclusive) => BuiltinFn::BindingRangeInclusiveClamp,
        ("wrap", BindingRangeDomainKind::Count) => BuiltinFn::BindingCountWrap,
        ("wrap", BindingRangeDomainKind::Exclusive) => BuiltinFn::BindingRangeWrap,
        ("wrap", BindingRangeDomainKind::Inclusive) => BuiltinFn::BindingRangeInclusiveWrap,
        (other, _) => {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                format!("unknown binding range mode '{other}'"),
            )]);
        }
    };
    Ok(ParsedBindingAttributes {
        range: Some((func, begin, end)),
    })
}

enum BindingRangeDomain {
    Count(Expr),
    Range {
        begin: Expr,
        end: Expr,
        inclusive: bool,
    },
}

#[derive(Clone, Copy)]
enum BindingRangeDomainKind {
    Count,
    Exclusive,
    Inclusive,
}

fn parse_binding_range_domain(pair: Pair<'_, Rule>) -> Result<BindingRangeDomain, Vec<Diagnostic>> {
    if pair.as_rule() == Rule::expr {
        return Ok(BindingRangeDomain::Count(parse_expr_inner(pair)));
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut fields = pair.into_inner();
    let Some(begin) = fields.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "binding range is missing its lower bound",
        )]);
    };
    let Some(operator) = fields.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "binding range is missing its range operator",
        )]);
    };
    let Some(end) = fields.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "binding range is missing its upper bound",
        )]);
    };
    let inclusive = match operator.as_str() {
        ".." => false,
        "..=" => true,
        _ => unreachable!("binding range grammar restricts range operators"),
    };
    Ok(BindingRangeDomain::Range {
        begin: parse_expr_inner(begin),
        end: parse_expr_inner(end),
        inclusive,
    })
}

#[derive(Default)]
struct ParsedParamDomain {
    min: Option<Expr>,
    max: Option<Expr>,
    scale: Option<ParamScale>,
    curve: Option<Expr>,
    unit: Option<String>,
    step: Option<Expr>,
}

impl ParsedParamDomain {
    fn set_expr(
        &mut self,
        name: &str,
        value: Expr,
        pair: &Pair<'_, Rule>,
    ) -> Result<(), Vec<Diagnostic>> {
        let target = match name {
            "min" => &mut self.min,
            "max" => &mut self.max,
            "step" => &mut self.step,
            "curve" => &mut self.curve,
            _ => {
                return Err(vec![syntax_at_pair(
                    pair,
                    format!("unknown parameter domain field '{name}'"),
                )])
            }
        };
        if target.replace(value).is_some() {
            return Err(vec![syntax_at_pair(
                pair,
                format!("duplicate parameter domain field '{name}'"),
            )]);
        }
        Ok(())
    }

    fn set_scale(
        &mut self,
        value: ParamScale,
        pair: &Pair<'_, Rule>,
    ) -> Result<(), Vec<Diagnostic>> {
        if self.scale.replace(value).is_some() {
            return Err(vec![syntax_at_pair(
                pair,
                "duplicate parameter domain field 'scale'",
            )]);
        }
        Ok(())
    }

    fn set_unit(&mut self, value: String, pair: &Pair<'_, Rule>) -> Result<(), Vec<Diagnostic>> {
        if self.unit.replace(value).is_some() {
            return Err(vec![syntax_at_pair(
                pair,
                "duplicate parameter domain field 'unit'",
            )]);
        }
        Ok(())
    }
}

fn parse_param_scale(pair: &Pair<'_, Rule>) -> Result<ParamScale, Vec<Diagnostic>> {
    let value = pair.as_str().trim();
    ParamScale::from_name(value).ok_or_else(|| {
        vec![syntax_at_pair(
            pair,
            format!("unknown parameter scale '{value}'"),
        )]
    })
}

pub(super) fn parse_quoted_text(pair: &Pair<'_, Rule>) -> Result<String, Vec<Diagnostic>> {
    let raw = pair.as_str();
    let Some(inner) = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return Err(vec![syntax_at_pair(
            pair,
            "parameter unit must be a quoted string",
        )]);
    };
    let mut unit = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            unit.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(vec![syntax_at_pair(pair, "unterminated unit escape")]);
        };
        unit.push(match escaped {
            '"' => '"',
            '\\' => '\\',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            _ => {
                return Err(vec![syntax_at_pair(
                    pair,
                    format!("unsupported unit escape '\\{escaped}'"),
                )])
            }
        });
    }
    Ok(unit)
}

fn parse_param_unit(pair: &Pair<'_, Rule>) -> Result<String, Vec<Diagnostic>> {
    parse_quoted_text(pair)
}

fn parse_named_param_domain_item(
    pair: Pair<'_, Rule>,
    parsed: &mut ParsedParamDomain,
) -> Result<(), Vec<Diagnostic>> {
    let item_loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.clone().into_inner();
    let Some(name_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            item_loc.as_ref(),
            "missing parameter domain field name",
        )]);
    };
    let name = name_pair.as_str();
    let Some(value_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            item_loc.as_ref(),
            format!("missing value for parameter domain field '{name}'"),
        )]);
    };
    match name {
        "min" | "max" | "step" | "curve" if value_pair.as_rule() == Rule::expr => {
            parsed.set_expr(name, parse_expr_inner(value_pair), &pair)
        }
        "scale" if value_pair.as_rule() == Rule::expr => {
            parsed.set_scale(parse_param_scale(&value_pair)?, &pair)
        }
        "unit" if value_pair.as_rule() == Rule::param_unit => {
            parsed.set_unit(parse_param_unit(&value_pair)?, &pair)
        }
        "min" | "max" | "step" | "curve" => Err(vec![syntax_at_pair(
            &pair,
            format!("parameter domain field '{name}' requires a constant expression"),
        )]),
        "scale" => Err(vec![syntax_at_pair(
            &pair,
            format!(
                "parameter domain field 'scale' must be {}",
                PARAM_SCALES
                    .iter()
                    .map(|(_, name)| format!("'{name}'"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
        )]),
        "unit" => Err(vec![syntax_at_pair(
            &pair,
            "parameter domain field 'unit' must be a quoted string",
        )]),
        _ => Err(vec![syntax_at_pair(
            &pair,
            format!("unknown parameter domain field '{name}'"),
        )]),
    }
}

pub(super) fn parse_param_domain_pair(
    pair: Pair<'_, Rule>,
) -> Result<(DeclRange, ParamControl), Vec<Diagnostic>> {
    if pair.as_rule() != Rule::param_domain {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected parameter domain",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut positional = Vec::new();
    let mut named = Vec::new();
    let mut saw_named = false;
    for item in pair.into_inner() {
        let Some(value) = item.into_inner().next() else {
            continue;
        };
        if value.as_rule() == Rule::param_domain_named {
            saw_named = true;
            named.push(value);
        } else {
            if saw_named {
                return Err(vec![syntax_at_pair(
                    &value,
                    "positional parameter domain fields must precede named fields",
                )]);
            }
            positional.push(value);
        }
    }
    if positional.len() > PARAM_DOMAIN_POSITIONAL_FIELDS.len() {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            format!(
                "parameter domain accepts at most {} positional fields",
                PARAM_DOMAIN_POSITIONAL_FIELDS.len()
            ),
        )]);
    }

    let mut parsed = ParsedParamDomain::default();
    for item in &named {
        parse_named_param_domain_item(item.clone(), &mut parsed)?;
    }

    let positional_len = positional.len();
    let single_is_max = positional_len == 1 && parsed.max.is_none();
    for (index, item) in positional.into_iter().enumerate() {
        let field = if index == 0 && single_is_max {
            "max"
        } else {
            PARAM_DOMAIN_POSITIONAL_FIELDS[index]
        };
        match (field, item.as_rule()) {
            ("min" | "max" | "step", Rule::expr) => {
                parsed.set_expr(field, parse_expr_inner(item.clone()), &item)?;
            }
            ("scale", Rule::expr) => {
                parsed.set_scale(parse_param_scale(&item)?, &item)?;
            }
            ("unit", Rule::param_unit) => {
                parsed.set_unit(parse_param_unit(&item)?, &item)?;
            }
            _ => {
                return Err(vec![syntax_at_pair(
                    &item,
                    format!("invalid positional parameter domain field '{field}'"),
                )])
            }
        }
    }

    let Some(max) = parsed.max else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "parameter domain requires a 'max' value",
        )]);
    };
    Ok((
        DeclRange {
            min: parsed.min,
            max,
        },
        ParamControl {
            scale: parsed.scale.unwrap_or_default(),
            curve: parsed.curve,
            unit: parsed.unit,
            step: parsed.step,
        },
    ))
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
    PrimitiveType::from_name(text)
        .ok_or_else(|| Diagnostic::syntax(format!("unsupported primitive type '{text}'"), 0, 0))
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
    BuiltinFn::from_name(name)
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

pub(super) fn parse_fn_return_type(pair: Pair<'_, Rule>) -> Result<FnReturnType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::fn_return_type {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected function return type",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing function return type",
        )]);
    };

    fn parse_scalar(pair: Pair<'_, Rule>) -> Result<FnReturnScalarType, Vec<Diagnostic>> {
        match pair.as_rule() {
            Rule::type_name => Ok(FnReturnScalarType::Primitive(
                parse_primitive_type(pair.as_str()).map_err(|d| vec![d])?,
            )),
            Rule::qualified_ident | Rule::namespace_ref => {
                Ok(FnReturnScalarType::Named(pair.as_str().trim().to_owned()))
            }
            _ => Err(vec![syntax_at_pair(
                &pair,
                "unsupported function return scalar type",
            )]),
        }
    }

    match inner.as_rule() {
        Rule::fn_return_scalar_type => {
            let Some(scalar_pair) = inner.into_inner().next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing function return scalar type",
                )]);
            };
            parse_scalar(scalar_pair).map(FnReturnType::Scalar)
        }
        Rule::fn_return_array_type => {
            let mut inner = inner.into_inner();
            let Some(elem_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing function return array element type",
                )]);
            };
            let Some(size_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing function return array size",
                )]);
            };
            Ok(FnReturnType::Array {
                elem: parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?,
                size: parse_expr(size_pair)?,
            })
        }
        Rule::fn_return_tuple_type => {
            let elems: Result<Vec<FnReturnScalarType>, Vec<Diagnostic>> = inner
                .into_inner()
                .filter(|p| p.as_rule() == Rule::fn_return_scalar_type)
                .map(|p| {
                    let scalar_pair = p.into_inner().next().ok_or_else(|| {
                        vec![syntax_at_loc(
                            loc.as_ref(),
                            "missing function return tuple element type",
                        )]
                    })?;
                    parse_scalar(scalar_pair)
                })
                .collect();
            Ok(FnReturnType::Tuple(elems?))
        }
        _ => Err(vec![syntax_at_loc(
            loc.as_ref(),
            "unsupported function return type",
        )]),
    }
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
        Rule::qualified_ident | Rule::namespace_ref | Rule::named_type => {
            Ok(EventParamType::GenericScalar {
                name: inner.as_str().trim().to_owned(),
            })
        }
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
                Rule::qualified_ident | Rule::namespace_ref | Rule::named_type => {
                    Ok(EventParamType::GenericArray {
                        elem: elem_pair.as_str().trim().to_owned(),
                        size: parse_expr_inner(size_pair),
                    })
                }
                _ => Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "event array parameters require primitive or generic primitive element type",
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

pub(super) fn parse_buffer_count(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    if pair.as_rule() != Rule::buffer_count {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected buffer count",
        )]);
    }
    let Some(value) = pair.into_inner().next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing buffer count expression",
        )]);
    };
    let value = if value.as_rule() == Rule::buffer_count_named {
        value.into_inner().next().ok_or_else(|| {
            vec![syntax_at_loc(
                loc.as_ref(),
                "missing named buffer count expression",
            )]
        })?
    } else {
        value
    };
    parse_expr(value)
}

pub(super) fn parse_index_groups(
    groups: Vec<Pair<'_, Rule>>,
    loc: Span,
) -> Result<Vec<Expr>, Vec<Diagnostic>> {
    if groups.is_empty() || groups.len() > 2 {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "indexing expects one or two bracket groups",
        )]);
    }
    let group_count = groups.len();
    let mut indices = Vec::new();
    for (group_index, group) in groups.into_iter().enumerate() {
        if group.as_rule() != Rule::index_group {
            return Err(vec![syntax_at_pair(
                &group,
                "internal parser error: expected index group",
            )]);
        }
        let Some(args) = group.into_inner().next() else {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                "missing index expression",
            )]);
        };
        let parsed = args
            .into_inner()
            .filter(|pair| pair.as_rule() == Rule::expr)
            .map(parse_expr)
            .collect::<Result<Vec<_>, _>>()?;
        if parsed.is_empty()
            || parsed.len() > 2
            || (group_count == 2 && group_index == 0 && parsed.len() != 1)
        {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                "invalid indexed access shape",
            )]);
        }
        indices.extend(parsed);
    }
    if indices.len() > 3 {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "indexed access supports at most resource, channel, and frame coordinates",
        )]);
    }
    Ok(indices)
}

fn parse_slice_range(
    pair: Pair<'_, Rule>,
    loc: Span,
) -> Result<(Option<Expr>, Option<Expr>), Vec<Diagnostic>> {
    if pair.as_rule() != Rule::slice_range {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected slice range",
        )]);
    }
    let mut start = None;
    let mut end = None;
    for bound in pair.into_inner() {
        let rule = bound.as_rule();
        let Some(expression) = bound.into_inner().next() else {
            return Err(vec![syntax_at_loc(loc.as_ref(), "missing slice bound")]);
        };
        match rule {
            Rule::slice_start => start = Some(parse_expr(expression)?),
            Rule::slice_end => end = Some(parse_expr(expression)?),
            _ => {}
        }
    }
    Ok((start, end))
}

pub(super) struct ParsedSliceParts {
    pub selector: Option<Box<Expr>>,
    pub channel: Option<Box<Expr>>,
    pub start: Option<Box<Expr>>,
    pub end: Option<Box<Expr>>,
}

pub(super) fn parse_slice_parts(
    parts: Vec<Pair<'_, Rule>>,
    loc: Span,
) -> Result<ParsedSliceParts, Vec<Diagnostic>> {
    let (selector, slice_group) = match parts.as_slice() {
        [slice_group] if slice_group.as_rule() == Rule::slice_group => (None, slice_group.clone()),
        [selector_group, slice_group]
            if selector_group.as_rule() == Rule::index_group
                && slice_group.as_rule() == Rule::slice_group =>
        {
            let selectors = parse_index_groups(vec![selector_group.clone()], loc)?;
            if selectors.len() != 1 {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "buffer collection selection expects exactly one index",
                )]);
            }
            (selectors.into_iter().next(), slice_group.clone())
        }
        _ => {
            return Err(vec![syntax_at_loc(
                loc.as_ref(),
                "invalid slice access shape",
            )]);
        }
    };

    let Some(contents) = slice_group.into_inner().next() else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing slice range")]);
    };
    let (channel, range) = match contents.as_rule() {
        Rule::slice_range => (None, contents),
        Rule::buffer_slice_args => {
            let mut inner = contents.into_inner();
            let channel = inner
                .next()
                .ok_or_else(|| vec![syntax_at_loc(loc.as_ref(), "missing buffer slice channel")])?;
            let range = inner
                .next()
                .ok_or_else(|| vec![syntax_at_loc(loc.as_ref(), "missing buffer slice range")])?;
            (Some(parse_expr(channel)?), range)
        }
        _ => {
            return Err(vec![syntax_at_loc(loc.as_ref(), "invalid slice contents")]);
        }
    };
    let (start, end) = parse_slice_range(range, loc)?;
    Ok(ParsedSliceParts {
        selector: selector.map(Box::new),
        channel: channel.map(Box::new),
        start: start.map(Box::new),
        end: end.map(Box::new),
    })
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
        && matches!(&size, Expr::Var { name, .. } if PrimitiveType::is_name(name))
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
        Rule::path_ident => Ok(AssignTarget::Var(pair_symbol_text(&pair))),
        Rule::slice_target => {
            let loc = stmt_loc_from_pair(&pair);
            let mut inner = pair.into_inner();
            let Some(base_pair) = inner.next() else {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "missing sliced assignment base",
                )]);
            };
            let ParsedSliceParts {
                selector,
                channel,
                start,
                end,
            } = parse_slice_parts(inner.collect(), loc)?;
            Ok(AssignTarget::Slice {
                base: base_pair.as_str().to_owned(),
                selector,
                channel,
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
            let indices = parse_index_groups(inner.collect(), loc)?;
            if indices.len() != 1 {
                return Err(vec![syntax_at_loc(
                    loc.as_ref(),
                    "nested indexed assignment targets must use parser rewrite path",
                )]);
            }
            Ok(AssignTarget::Index {
                base: base_pair.as_str().to_owned(),
                index: indices.into_iter().next().expect("one index was checked"),
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
