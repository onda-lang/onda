use super::*;
use crate::ast::{
    NamespaceAliasDecl, NamespaceCallArg, NamespaceDecl, NamespaceItem, NamespaceRefSegment,
    NamespaceTemplateParam, UseDecl,
};

pub(super) fn parse_namespace_decl_ast(
    block_pair: Pair<'_, Rule>,
) -> Result<NamespaceDecl, Vec<Diagnostic>> {
    if block_pair.as_rule() != Rule::namespace_block {
        return Err(vec![syntax_at_pair(
            &block_pair,
            "internal parser error: expected namespace block",
        )]);
    }
    let block_loc = stmt_loc_from_pair(&block_pair);
    let mut inner = block_pair.into_inner();
    let Some(head_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            block_loc.as_ref(),
            "missing namespace name",
        )]);
    };
    let (loc, name, params) = parse_namespace_decl_head(head_pair)?;
    let mut items = Vec::<NamespaceItem>::new();
    for item in inner {
        match item.as_rule() {
            Rule::assert_block => items.push(NamespaceItem::Assert(parse_assert_decl(item)?)),
            Rule::const_block => items.push(NamespaceItem::Const(parse_const_decl(item)?)),
            Rule::struct_block => items.push(NamespaceItem::Struct(parse_struct_block(item)?)),
            Rule::def_block => items.push(NamespaceItem::Def(parse_def_block(item)?)),
            Rule::proc_block => items.push(NamespaceItem::Proc(parse_proc_block(item)?)),
            Rule::namespace_block => {
                items.push(NamespaceItem::Namespace(parse_namespace_decl_ast(item)?));
            }
            Rule::namespace_alias_decl => {
                items.push(NamespaceItem::Alias(parse_namespace_alias_decl_ast(item)?));
            }
            Rule::use_decl => {
                items.push(NamespaceItem::Use(parse_use_decl_ast(item)?));
            }
            _ => {}
        }
    }
    Ok(NamespaceDecl {
        loc,
        name,
        params,
        items,
    })
}

fn parse_namespace_decl_head(
    pair: Pair<'_, Rule>,
) -> Result<(Span, String, Vec<NamespaceTemplateParam>), Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_decl_head {
        return Err(vec![syntax_at_pair(
            &pair,
            "missing namespace declaration head",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut name = None::<String>;
    let mut params = Vec::<NamespaceTemplateParam>::new();
    for item in pair.into_inner() {
        match item.as_rule() {
            Rule::namespace_name => name = Some(item.as_str().to_owned()),
            Rule::namespace_param_list => {
                params = parse_namespace_param_list(item)?;
            }
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing namespace name")]);
    };
    Ok((loc, name, params))
}

fn parse_namespace_param_list(
    pair: Pair<'_, Rule>,
) -> Result<Vec<NamespaceTemplateParam>, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_param_list {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected namespace parameter list",
        )]);
    }
    let mut out = Vec::<NamespaceTemplateParam>::new();
    let mut seen = HashSet::<String>::new();
    for item in pair.into_inner() {
        let item_loc = stmt_loc_from_pair(&item);
        if item.as_rule() != Rule::namespace_param_decl {
            continue;
        }
        let mut name = None::<String>;
        let mut default = None::<Expr>;
        for part in item.into_inner() {
            match part.as_rule() {
                Rule::ident => name = Some(part.as_str().to_owned()),
                Rule::expr => default = Some(parse_expr(part)?),
                _ => {}
            }
        }
        let Some(name) = name else {
            return Err(vec![syntax_at_loc(
                item_loc.as_ref(),
                "missing namespace template parameter name",
            )]);
        };
        if !seen.insert(name.clone()) {
            return Err(vec![syntax_at_loc(
                item_loc.as_ref(),
                format!("duplicate namespace template parameter '{name}'"),
            )]);
        }
        let Some(default) = default else {
            return Err(vec![syntax_at_loc(
                item_loc.as_ref(),
                format!("namespace template parameter '{name}' requires a default value"),
            )]);
        };
        out.push(NamespaceTemplateParam { name, default });
    }
    Ok(out)
}

pub(super) fn parse_namespace_alias_decl_ast(
    pair: Pair<'_, Rule>,
) -> Result<NamespaceAliasDecl, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_alias_decl {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected namespace alias declaration",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut name = None::<String>;
    let mut target = None::<Vec<NamespaceRefSegment>>;
    for part in pair.into_inner() {
        match part.as_rule() {
            Rule::ident => name = Some(part.as_str().to_owned()),
            Rule::namespace_any_ref | Rule::namespace_ref => {
                target = Some(parse_namespace_ref_pair(part)?);
            }
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing namespace alias name",
        )]);
    };
    let Some(target) = target else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing namespace alias target",
        )]);
    };
    Ok(NamespaceAliasDecl { loc, name, target })
}

pub(super) fn parse_use_decl_ast(pair: Pair<'_, Rule>) -> Result<UseDecl, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::use_decl {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected use declaration",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut target = None::<Vec<NamespaceRefSegment>>;
    let mut alias = None::<String>;
    let mut public = false;
    for part in pair.into_inner() {
        match part.as_rule() {
            Rule::use_pub => public = true,
            Rule::namespace_any_ref | Rule::namespace_ref => {
                target = Some(parse_namespace_ref_pair(part)?);
            }
            Rule::ident => alias = Some(part.as_str().to_owned()),
            _ => {}
        }
    }
    let Some(target) = target else {
        return Err(vec![syntax_at_loc(loc.as_ref(), "missing use target")]);
    };
    Ok(UseDecl {
        loc,
        target,
        alias,
        public,
    })
}

fn parse_namespace_ref_pair(
    pair: Pair<'_, Rule>,
) -> Result<Vec<NamespaceRefSegment>, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_ref && pair.as_rule() != Rule::namespace_any_ref {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected namespace reference",
        )]);
    }
    let mut out = Vec::<NamespaceRefSegment>::new();
    for seg in pair.into_inner() {
        let seg_loc = stmt_loc_from_pair(&seg);
        if seg.as_rule() != Rule::namespace_ref_segment {
            continue;
        }
        let mut name = None::<String>;
        let mut args = None::<Vec<NamespaceCallArg>>;
        for item in seg.into_inner() {
            match item.as_rule() {
                Rule::ident => name = Some(item.as_str().to_owned()),
                Rule::namespace_call_arg_list => {
                    args = Some(parse_namespace_call_arg_list(item)?);
                }
                _ => {}
            }
        }
        let Some(name) = name else {
            return Err(vec![syntax_at_loc(
                seg_loc.as_ref(),
                "missing namespace segment name",
            )]);
        };
        out.push(NamespaceRefSegment { name, args });
    }
    Ok(out)
}

fn parse_namespace_call_arg_list(
    pair: Pair<'_, Rule>,
) -> Result<Vec<NamespaceCallArg>, Vec<Diagnostic>> {
    let mut parsed = Vec::<NamespaceCallArg>::new();
    for arg in pair.into_inner() {
        let arg_loc = stmt_loc_from_pair(&arg);
        if arg.as_rule() != Rule::namespace_call_arg {
            continue;
        }
        let mut arg_name = None::<String>;
        let mut arg_expr = None::<Expr>;
        for part in arg.into_inner() {
            match part.as_rule() {
                Rule::ident => arg_name = Some(part.as_str().to_owned()),
                Rule::expr => arg_expr = Some(parse_expr(part)?),
                _ => {}
            }
        }
        let Some(expr) = arg_expr else {
            return Err(vec![syntax_at_loc(
                arg_loc.as_ref(),
                "missing namespace template argument expression",
            )]);
        };
        parsed.push(NamespaceCallArg {
            name: arg_name,
            expr,
        });
    }
    Ok(parsed)
}

fn parse_namespace_ref_text(text: &str) -> Result<Vec<NamespaceRefSegment>, Vec<Diagnostic>> {
    let mut parsed = OndaParser::parse(Rule::namespace_ref, text.trim())
        .map_err(|err| vec![diag_from_pest_error(err)])?;
    let pair = parsed
        .next()
        .ok_or_else(|| vec![Diagnostic::syntax("missing namespace reference", 1, 1)])?;
    let out = parse_namespace_ref_pair(pair)?;
    if out.len() < 2 {
        return Err(vec![Diagnostic::syntax(
            "namespace reference must contain at least one '::'",
            1,
            1,
        )]);
    }
    Ok(out)
}

pub fn parse_namespace_ref_text_ast(
    text: &str,
) -> Result<Vec<NamespaceRefSegment>, Vec<Diagnostic>> {
    parse_namespace_ref_text(text)
}
