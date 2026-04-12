use super::rewrite::{
    finalize_const_decl_expr, rewrite_block_namespace_refs, substitute_expr_with_env,
    validate_compile_time_expr,
};
use super::*;
use crate::ast::Span;

#[derive(Debug, Clone)]
pub(super) struct NamespaceTemplateDecl {
    loc: Span,
    name: String,
    params: Vec<NamespaceTemplateParam>,
    items: Vec<NamespaceDeclItem>,
    captured_consts: HashMap<String, Expr>,
}

#[derive(Debug, Clone)]
struct NamespaceTemplateParam {
    name: String,
    default: Expr,
}

#[derive(Debug, Clone)]
enum NamespaceDeclItem {
    Assert(AssertDecl),
    Const(ConstDecl),
    Struct(StructDef),
    Def(FunctionDef),
    Proc(ProcessorDef),
    Namespace(NamespaceTemplateDecl),
    Alias(NamespaceAliasLocalDecl),
}

#[derive(Debug, Clone)]
pub(super) struct NamespaceAliasLocalDecl {
    loc: Span,
    name: String,
    target: Vec<NamespaceRefSegment>,
}

#[derive(Debug, Clone)]
pub(super) struct NamespaceAliasDecl {
    declared_ns: String,
    target: Vec<NamespaceRefSegment>,
}

#[derive(Debug, Clone)]
struct NamespaceRefSegment {
    name: String,
    args: Option<Vec<NamespaceCallArg>>,
}

#[derive(Debug, Clone)]
struct NamespaceCallArg {
    name: Option<String>,
    expr: Expr,
}

fn expr_span(expr: &Expr) -> Span {
    expr.loc().span()
}

/// Reconstruct a `<...>` type-arg suffix from namespace call args
/// that turned out to be struct type arguments rather than namespace template args.
fn format_call_args_as_type_suffix(args: &[NamespaceCallArg]) -> String {
    let parts: Vec<String> = args
        .iter()
        .map(|a| match &a.expr {
            Expr::Var { name, .. } => name.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

fn rebase_relative_loc_to_use_site(loc: &mut SourceLoc, use_site_span: Span) {
    if use_site_span.is_zero() || loc.is_zero() {
        return;
    }

    let base_line = use_site_span.line as usize;
    let base_column = use_site_span.column as usize;
    let relative_line = loc.line;
    let relative_end_line = loc.end_line;
    let relative_end_column = loc.end_column;

    loc.line = base_line + relative_line.saturating_sub(1);
    if relative_line == 1 {
        loc.column = base_column + loc.column.saturating_sub(1);
    }
    loc.end_line = base_line + relative_end_line.saturating_sub(1);
    if relative_end_line == 1 {
        loc.end_column = base_column + relative_end_column.saturating_sub(1);
    }
}

fn rebase_namespace_expr_locs(expr: &mut Expr, use_site_span: Span) {
    let mut loc = expr.loc();
    rebase_relative_loc_to_use_site(&mut loc, use_site_span);
    expr.set_loc(loc);
    match expr {
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                rebase_namespace_expr_locs(value, use_site_span);
            }
        }
        Expr::Index { index, .. }
        | Expr::Cast { expr: index, .. }
        | Expr::UnaryNot { expr: index, .. }
        | Expr::UnaryBitNot { expr: index, .. } => {
            rebase_namespace_expr_locs(index, use_site_span);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                rebase_namespace_expr_locs(start, use_site_span);
            }
            if let Some(end) = end {
                rebase_namespace_expr_locs(end, use_site_span);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rebase_namespace_expr_locs(&mut spec.size, use_site_span);
            if let Some(init) = init {
                for value in init {
                    rebase_namespace_expr_locs(value, use_site_span);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rebase_namespace_expr_locs(lhs, use_site_span);
            rebase_namespace_expr_locs(rhs, use_site_span);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rebase_namespace_expr_locs(arg, use_site_span);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rebase_namespace_expr_locs(&mut arg.expr, use_site_span);
            }
        }
        Expr::Tuple { values, .. } => {
            for value in values {
                rebase_namespace_expr_locs(value, use_site_span);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn rebase_namespace_ref_expr_locs(segments: &mut [NamespaceRefSegment], use_site_span: Span) {
    if use_site_span.is_zero() {
        return;
    }

    for segment in segments {
        if let Some(args) = &mut segment.args {
            for arg in args {
                rebase_namespace_expr_locs(&mut arg.expr, use_site_span);
            }
        }
    }
}

pub(super) fn parse_namespace_decl(
    block_pair: Pair<'_, Rule>,
) -> Result<NamespaceTemplateDecl, Vec<Diagnostic>> {
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
    let mut items = Vec::<NamespaceDeclItem>::new();
    for item in inner {
        match item.as_rule() {
            Rule::assert_block => items.push(NamespaceDeclItem::Assert(parse_assert_decl(item)?)),
            Rule::const_block => items.push(NamespaceDeclItem::Const(parse_const_decl(item)?)),
            Rule::struct_block => items.push(NamespaceDeclItem::Struct(parse_struct_block(item)?)),
            Rule::def_block => items.push(NamespaceDeclItem::Def(parse_def_block(item)?)),
            Rule::proc_block => items.push(NamespaceDeclItem::Proc(parse_proc_block(item)?)),
            Rule::namespace_block => {
                items.push(NamespaceDeclItem::Namespace(parse_namespace_decl(item)?))
            }
            Rule::namespace_alias_decl => {
                items.push(NamespaceDeclItem::Alias(parse_namespace_alias_decl(item)?))
            }
            _ => {}
        }
    }
    Ok(NamespaceTemplateDecl {
        loc,
        name,
        params,
        items,
        captured_consts: HashMap::new(),
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
                format!("namespace template parameter '{name}' must define a default"),
            )]);
        };
        out.push(NamespaceTemplateParam { name, default });
    }
    Ok(out)
}

pub(super) fn parse_namespace_alias_decl(
    pair: Pair<'_, Rule>,
) -> Result<NamespaceAliasLocalDecl, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_alias_decl {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected namespace alias declaration",
        )]);
    }
    let pair_loc = stmt_loc_from_pair(&pair);
    let mut loc = Span::ZERO;
    let mut name = None::<String>;
    let mut target = None::<Vec<NamespaceRefSegment>>;
    for item in pair.into_inner() {
        match item.as_rule() {
            Rule::ident => {
                loc = stmt_loc_from_pair(&item);
                name = Some(item.as_str().to_owned());
            }
            Rule::namespace_any_ref | Rule::namespace_ref => {
                target = Some(parse_namespace_ref_pair(item)?)
            }
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![syntax_at_loc(
            pair_loc.as_ref(),
            "missing namespace alias name",
        )]);
    };
    let Some(target) = target else {
        return Err(vec![syntax_at_loc(
            pair_loc.as_ref(),
            "missing namespace alias target",
        )]);
    };
    Ok(NamespaceAliasLocalDecl { loc, name, target })
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
                    let mut parsed = Vec::<NamespaceCallArg>::new();
                    for arg in item.into_inner() {
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
                        let Some(arg_expr) = arg_expr else {
                            return Err(vec![syntax_at_loc(
                                arg_loc.as_ref(),
                                "missing namespace template argument expression",
                            )]);
                        };
                        parsed.push(NamespaceCallArg {
                            name: arg_name,
                            expr: arg_expr,
                        });
                    }
                    args = Some(parsed);
                }
                _ => {}
            }
        }
        let Some(name) = name else {
            return Err(vec![syntax_at_loc(
                seg_loc.as_ref(),
                "missing namespace path segment name",
            )]);
        };
        out.push(NamespaceRefSegment { name, args });
    }
    Ok(out)
}

pub(super) fn process_namespace_decl(
    decl: NamespaceTemplateDecl,
    parent_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    out: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let full_ns = namespace_join(parent_ns, &decl.name);
    if decl.params.is_empty() {
        emit_namespace_items(&decl.items, &full_ns, const_env, state, out)
    } else {
        if state.namespace_templates.contains_key(&full_ns) {
            return Err(vec![Diagnostic::semantic_span(
                format!("duplicate namespace template '{full_ns}'"),
                decl.loc.as_ref(),
            )]);
        }
        state.namespace_templates.insert(full_ns, decl);
        Ok(())
    }
}

fn emit_namespace_items(
    items: &[NamespaceDeclItem],
    ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    out: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for item in items {
        match item {
            NamespaceDeclItem::Struct(s) => {
                state.namespace_members.insert(namespace_join(ns, &s.name));
            }
            NamespaceDeclItem::Def(d) => {
                state.namespace_members.insert(namespace_join(ns, &d.name));
            }
            NamespaceDeclItem::Proc(p) => {
                state.namespace_members.insert(namespace_join(ns, &p.name));
            }
            NamespaceDeclItem::Assert(_)
            | NamespaceDeclItem::Const(_)
            | NamespaceDeclItem::Namespace(_)
            | NamespaceDeclItem::Alias(_) => {}
        }
    }

    let mut local_consts = const_env.clone();
    let mut local_const_names = HashSet::<String>::new();
    for item in items {
        match item {
            NamespaceDeclItem::Assert(assert_decl) => {
                let mut block = Block::Assert(assert_decl.clone());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(&mut block, ns, &local_consts, state, &mut generated)?;
                out.push(block);
                out.extend(generated);
            }
            NamespaceDeclItem::Const(decl) => {
                if !local_const_names.insert(decl.name.clone()) {
                    return Err(vec![Diagnostic::semantic_span(
                        format!("duplicate constant '{}' in namespace '{}'", decl.name, ns),
                        decl.loc.as_ref(),
                    )]);
                }
                let mut decl = decl.clone();
                let mut generated = Vec::<Block>::new();
                let value =
                    finalize_const_decl_expr(&mut decl, ns, &local_consts, state, &mut generated)?;
                out.extend(generated);
                state
                    .namespace_const_values
                    .insert(namespace_join(ns, &decl.name), value.clone());
                local_consts.insert(decl.name.clone(), value);
            }
            NamespaceDeclItem::Struct(s) => {
                let mut s = s.clone();
                s.name = namespace_join(ns, &s.name);
                let mut block = Block::Struct(s);
                let ns2 = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns2,
                    &local_consts,
                    state,
                    &mut generated,
                )?;
                out.push(block);
                out.extend(generated);
            }
            NamespaceDeclItem::Def(d) => {
                let mut d = d.clone();
                d.name = namespace_join(ns, &d.name);
                let mut block = Block::Def(d);
                let ns2 = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns2,
                    &local_consts,
                    state,
                    &mut generated,
                )?;
                out.push(block);
                out.extend(generated);
            }
            NamespaceDeclItem::Proc(p) => {
                let mut p = p.clone();
                p.name = namespace_join(ns, &p.name);
                let mut block = Block::Proc(p);
                let ns2 = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns2,
                    &local_consts,
                    state,
                    &mut generated,
                )?;
                out.push(block);
                out.extend(generated);
            }
            NamespaceDeclItem::Namespace(nested) => {
                let mut nested = nested.clone();
                if nested.params.is_empty() {
                    let full_nested = namespace_join(ns, &nested.name);
                    emit_instantiated_namespace_items(
                        &nested.items,
                        &full_nested,
                        &local_consts,
                        state,
                        out,
                    )?;
                } else {
                    let full_nested = namespace_join(ns, &nested.name);
                    let mut captured = nested.captured_consts.clone();
                    for (k, v) in &local_consts {
                        captured.insert(k.clone(), v.clone());
                    }
                    nested.captured_consts = captured;
                    state.namespace_templates.insert(full_nested, nested);
                }
            }
            NamespaceDeclItem::Alias(alias) => {
                let mut alias = alias.clone();
                substitute_namespace_ref_consts(&mut alias.target, &local_consts);
                register_namespace_alias(ns, alias, state)?;
            }
        }
    }
    Ok(())
}

pub(super) fn register_namespace_alias(
    parent_ns: &str,
    alias: NamespaceAliasLocalDecl,
    state: &mut LoadState,
) -> Result<(), Vec<Diagnostic>> {
    let full_name = namespace_join(parent_ns, &alias.name);
    if state.namespace_aliases.contains_key(&full_name) {
        return Err(vec![Diagnostic::semantic_span(
            format!("duplicate namespace alias '{full_name}'"),
            alias.loc.as_ref(),
        )]);
    }
    state.namespace_aliases.insert(
        full_name.clone(),
        NamespaceAliasDecl {
            declared_ns: parent_ns.to_owned(),
            target: alias.target,
        },
    );
    Ok(())
}

fn namespace_parent(ns: &str) -> Option<&str> {
    ns.rsplit_once("::").map(|(parent, _)| parent)
}

fn namespace_candidates(current_ns: &str) -> Vec<String> {
    if current_ns.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::<String>::new();
    let mut cur = Some(current_ns);
    while let Some(ns) = cur {
        out.push(ns.to_owned());
        cur = namespace_parent(ns);
    }
    out.push(String::new());
    out
}

pub(super) fn namespace_of_symbol(name: &str) -> String {
    name.rsplit_once("::")
        .map(|(ns, _)| ns.to_owned())
        .unwrap_or_default()
}

fn namespace_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}::{child}")
    }
}

fn split_namespace_parent_leaf(name: &str) -> (&str, &str) {
    if let Some((parent, leaf)) = name.rsplit_once("::") {
        (parent, leaf)
    } else {
        ("", name)
    }
}

pub(super) fn looks_like_namespace_ref(name: &str) -> bool {
    name.contains("::") && !name.contains('.')
}

fn parse_namespace_ref_text(text: &str) -> Result<Vec<NamespaceRefSegment>, Vec<Diagnostic>> {
    let mut parsed = OndaParser::parse(Rule::namespace_ref, text)
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

fn expr_key(expr: &Expr) -> String {
    match expr {
        Expr::Number { value, .. } => format!("f32({value})"),
        Expr::Int { value, .. } => format!("i64({value})"),
        Expr::Bool { value, .. } => format!("bool({value})"),
        Expr::Var { name, .. } => format!("var({name})"),
        Expr::ArrayLiteral { values, .. } => format!(
            "[{}]",
            values.iter().map(expr_key).collect::<Vec<_>>().join(",")
        ),
        Expr::Index { base, index, .. } => format!("idx({base},{})", expr_key(index)),
        Expr::Slice {
            base, start, end, ..
        } => format!(
            "slice({base},{},{})",
            start
                .as_ref()
                .map(|expr| expr_key(expr))
                .unwrap_or_default(),
            end.as_ref().map(|expr| expr_key(expr)).unwrap_or_default()
        ),
        Expr::ArrayCtor { spec, init, .. } => {
            let elem = match &spec.elem {
                ArrayElemType::Primitive(p) => format!("{p:?}"),
                ArrayElemType::Struct(s) => s.clone(),
            };
            let init_key = init
                .as_ref()
                .map(|v| v.iter().map(expr_key).collect::<Vec<_>>().join(","))
                .unwrap_or_default();
            format!("arr({elem};{};{init_key})", expr_key(&spec.size))
        }
        Expr::Compare { op, lhs, rhs, .. } => {
            format!("cmp({op:?},{},{})", expr_key(lhs), expr_key(rhs))
        }
        Expr::Call { func, args, .. } => format!(
            "call({func:?},[{}])",
            args.iter().map(expr_key).collect::<Vec<_>>().join(",")
        ),
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            let type_key = type_args
                .iter()
                .map(|a| match a {
                    CallTypeArg::Primitive(p) => format!("p:{p:?}"),
                    CallTypeArg::Generic(g) => format!("g:{g}"),
                })
                .collect::<Vec<_>>()
                .join(",");
            let arg_key = args
                .iter()
                .map(|a| {
                    let n = a.name.clone().unwrap_or_default();
                    format!("{n}={}", expr_key(&a.expr))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("ucall({name};[{type_key}];[{arg_key}])")
        }
        Expr::Cast { to, expr, .. } => format!("cast({to:?},{})", expr_key(expr)),
        Expr::UnaryNot { expr, .. } => format!("not({})", expr_key(expr)),
        Expr::UnaryBitNot { expr, .. } => format!("bitnot({})", expr_key(expr)),
        Expr::Logical { op, lhs, rhs, .. } => {
            format!("log({op:?},{},{})", expr_key(lhs), expr_key(rhs))
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            format!("bin({op:?},{},{})", expr_key(lhs), expr_key(rhs))
        }
        Expr::Tuple { values, .. } => format!(
            "tuple({})",
            values.iter().map(expr_key).collect::<Vec<_>>().join(",")
        ),
    }
}

fn resolve_visible_alias(
    alias_leaf: &str,
    current_ns: &str,
    state: &LoadState,
) -> Option<NamespaceAliasDecl> {
    for ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&ns, alias_leaf);
        if let Some(alias) = state.namespace_aliases.get(&candidate) {
            return Some(alias.clone());
        }
    }
    None
}

pub(super) fn resolve_namespace_symbol_name(
    name: &str,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    use_site_span: Span,
) -> Result<String, Vec<Diagnostic>> {
    if !looks_like_namespace_ref(name) {
        return Ok(name.to_owned());
    }
    let mut segments = parse_namespace_ref_text(name)?;
    rebase_namespace_ref_expr_locs(&mut segments, use_site_span);
    resolve_namespace_segments_internal(
        &segments,
        current_ns,
        const_env,
        state,
        generated,
        use_site_span,
        0,
    )
}

fn split_named_type_base_and_suffix(name: &str) -> (&str, &str) {
    if let Some(idx) = name.find('<') {
        (&name[..idx], &name[idx..])
    } else {
        (name, "")
    }
}

pub(super) fn rewrite_named_type_ref_name(
    name: &mut String,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    use_site_loc: impl Into<SourceLoc>,
) -> Result<(), Vec<Diagnostic>> {
    let use_site_span = use_site_loc.into().span();

    if let Some(qualified) = qualify_local_namespace_member_name(name, current_ns, state) {
        *name = qualified;
        return Ok(());
    }

    let (base, suffix) = split_named_type_base_and_suffix(name);
    if !suffix.is_empty() {
        if let Some(qualified) = qualify_local_namespace_member_name(base, current_ns, state) {
            *name = format!("{qualified}{suffix}");
            return Ok(());
        }
        if looks_like_namespace_ref(base) {
            let resolved = resolve_namespace_symbol_name(
                base,
                current_ns,
                const_env,
                state,
                generated,
                use_site_span,
            )?;
            *name = format!("{resolved}{suffix}");
            return Ok(());
        }
    }

    if looks_like_namespace_ref(name) {
        *name = resolve_namespace_symbol_name(
            name,
            current_ns,
            const_env,
            state,
            generated,
            use_site_span,
        )?;
    }
    Ok(())
}

pub(super) fn qualify_local_namespace_member_name(
    name: &str,
    current_ns: &str,
    state: &LoadState,
) -> Option<String> {
    if name.is_empty() || name.contains("::") || name.contains('.') {
        return None;
    }
    let (base, suffix) = split_named_type_base_and_suffix(name);
    for ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&ns, base);
        if state.namespace_members.contains(&candidate) {
            return Some(format!("{candidate}{suffix}"));
        }
    }
    None
}

fn has_visible_namespace_prefix(prefix: &str, state: &LoadState) -> bool {
    let nested_prefix = format!("{prefix}::");

    state.namespace_templates.contains_key(prefix)
        || state.namespace_aliases.contains_key(prefix)
        || state
            .namespace_members
            .iter()
            .any(|name| name.starts_with(&nested_prefix))
        || state
            .namespace_const_values
            .keys()
            .any(|name| name.starts_with(&nested_prefix))
        || state
            .namespace_templates
            .keys()
            .any(|name| name.starts_with(&nested_prefix))
        || state
            .namespace_aliases
            .keys()
            .any(|name| name.starts_with(&nested_prefix))
}

fn resolve_namespace_segments_internal(
    segments: &[NamespaceRefSegment],
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    use_site_span: Span,
    depth: usize,
) -> Result<String, Vec<Diagnostic>> {
    if depth > 64 {
        return Err(vec![Diagnostic::semantic_span(
            "namespace alias/template resolution exceeded recursion depth",
            use_site_span,
        )]);
    }
    if segments.is_empty() {
        return Err(vec![Diagnostic::semantic_span(
            "empty namespace reference",
            use_site_span,
        )]);
    }

    let mut idx = 0usize;
    let mut path = String::new();
    let empty_call_args = Vec::<NamespaceCallArg>::new();

    if segments[0].args.is_none() {
        if let Some(alias) = resolve_visible_alias(&segments[0].name, current_ns, state) {
            path = resolve_namespace_segments_internal(
                &alias.target,
                &alias.declared_ns,
                const_env,
                state,
                generated,
                use_site_span,
                depth + 1,
            )?;
            idx = 1;
        }
    }

    if idx == 0 {
        if let Some(args) = &segments[0].args {
            let mut resolved = None::<String>;
            for candidate_ns in namespace_candidates(current_ns) {
                let candidate = namespace_join(&candidate_ns, &segments[0].name);
                if state.namespace_templates.contains_key(&candidate) {
                    resolved = Some(instantiate_namespace_template(
                        &candidate,
                        args,
                        const_env,
                        state,
                        generated,
                        use_site_span,
                    )?);
                    break;
                }
            }
            let Some(found) = resolved else {
                return Err(vec![Diagnostic::semantic_span(
                    format!("unknown namespace template '{}'", segments[0].name),
                    use_site_span,
                )]);
            };
            path = found;
        } else {
            let mut resolved = None::<String>;
            for candidate_ns in namespace_candidates(current_ns) {
                let candidate = namespace_join(&candidate_ns, &segments[0].name);
                if state.namespace_templates.contains_key(&candidate) {
                    resolved = Some(instantiate_namespace_template(
                        &candidate,
                        &empty_call_args,
                        const_env,
                        state,
                        generated,
                        use_site_span,
                    )?);
                    break;
                }
                if has_visible_namespace_prefix(&candidate, state) {
                    resolved = Some(candidate);
                    break;
                }
            }
            path = resolved.unwrap_or_else(|| segments[0].name.clone());
        }
        idx = 1;
    }

    for seg in &segments[idx..] {
        let candidate = namespace_join(&path, &seg.name);
        if let Some(args) = &seg.args {
            if state.namespace_templates.contains_key(&candidate) {
                path = instantiate_namespace_template(
                    &candidate,
                    args,
                    const_env,
                    state,
                    generated,
                    use_site_span,
                )?;
            } else {
                // Not a namespace template — preserve args as struct type arg suffix
                // so generic struct specialization can process them later.
                let suffix = format_call_args_as_type_suffix(args);
                path = format!("{candidate}{suffix}");
            }
        } else if state.namespace_templates.contains_key(&candidate) {
            path = instantiate_namespace_template(
                &candidate,
                &empty_call_args,
                const_env,
                state,
                generated,
                use_site_span,
            )?;
        } else {
            path = candidate;
        }
    }
    Ok(path)
}

fn instantiate_namespace_template(
    full_template_name: &str,
    call_args: &[NamespaceCallArg],
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    use_site_span: Span,
) -> Result<String, Vec<Diagnostic>> {
    let template = state
        .namespace_templates
        .get(full_template_name)
        .cloned()
        .ok_or_else(|| {
            vec![Diagnostic::semantic_span(
                format!("unknown namespace template '{full_template_name}'"),
                use_site_span,
            )]
        })?;

    let mut effective_consts = template.captured_consts.clone();
    for (k, v) in const_env {
        effective_consts
            .entry(k.clone())
            .or_insert_with(|| v.clone());
    }

    let mut named = HashMap::<String, Expr>::new();
    let mut named_spans = Vec::<(String, Span)>::new();
    let mut positional = Vec::<Expr>::new();
    for arg in call_args {
        let expr = substitute_expr_with_env(&arg.expr, &effective_consts);
        let arg_span = expr_span(&arg.expr);
        if let Some(name) = &arg.name {
            if named.insert(name.clone(), expr).is_some() {
                return Err(vec![Diagnostic::semantic_span(
                    format!(
                        "namespace template '{}' argument '{}' specified more than once",
                        full_template_name, name
                    ),
                    if arg_span.is_zero() {
                        use_site_span
                    } else {
                        arg_span
                    },
                )]);
            }
            named_spans.push((name.clone(), arg_span));
        } else {
            positional.push(expr);
        }
    }

    let mut pos_idx = 0usize;
    for param in &template.params {
        let value = if let Some(named_expr) = named.remove(&param.name) {
            named_expr
        } else if let Some(pos_expr) = positional.get(pos_idx) {
            pos_idx += 1;
            pos_expr.clone()
        } else {
            substitute_expr_with_env(&param.default, &effective_consts)
        };
        validate_compile_time_expr(
            &value,
            &effective_consts,
            &format!(
                "namespace template '{}' argument '{}'",
                full_template_name, param.name
            ),
        )?;
        let casted = Expr::Cast {
            loc: value.loc().into(),
            to: PrimitiveType::I32,
            expr: Box::new(value),
        };
        effective_consts.insert(param.name.clone(), casted);
    }

    if pos_idx < positional.len() {
        let extra_arg_span = positional
            .get(pos_idx)
            .map(expr_span)
            .filter(|span| !span.is_zero())
            .unwrap_or(use_site_span);
        return Err(vec![Diagnostic::semantic_span(
            format!(
                "namespace template '{}' received too many positional arguments",
                full_template_name
            ),
            extra_arg_span,
        )]);
    }
    if !named.is_empty() {
        let unknown_entries = named_spans
            .iter()
            .filter(|(name, _)| named.contains_key(name))
            .collect::<Vec<_>>();
        let mut unknown = named.keys().cloned().collect::<Vec<_>>();
        unknown.sort();
        let unknown_span = unknown_entries
            .iter()
            .map(|(_, span)| *span)
            .find(|span| !span.is_zero())
            .unwrap_or(use_site_span);
        return Err(vec![Diagnostic::semantic_span(
            format!(
                "namespace template '{}' received unknown named arguments: {}",
                full_template_name,
                unknown.join(", ")
            ),
            unknown_span,
        )]);
    }

    let key = {
        let values = template
            .params
            .iter()
            .map(|p| {
                effective_consts
                    .get(&p.name)
                    .map(expr_key)
                    .unwrap_or_else(|| "missing".to_owned())
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{full_template_name}[{values}]")
    };
    if let Some(existing) = state.namespace_instantiations.get(&key) {
        return Ok(existing.clone());
    }

    let (parent, leaf) = split_namespace_parent_leaf(full_template_name);
    let concrete_leaf = format!("{leaf}__nsinst{}", state.next_namespace_instantiation_id);
    state.next_namespace_instantiation_id += 1;
    let concrete_ns = namespace_join(parent, &concrete_leaf);
    state
        .namespace_instantiations
        .insert(key, concrete_ns.clone());

    emit_instantiated_namespace_items(
        &template.items,
        &concrete_ns,
        &effective_consts,
        state,
        generated,
    )?;

    Ok(concrete_ns)
}

fn emit_instantiated_namespace_items(
    items: &[NamespaceDeclItem],
    namespace: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for item in items {
        match item {
            NamespaceDeclItem::Struct(s) => {
                state
                    .namespace_members
                    .insert(namespace_join(namespace, &s.name));
            }
            NamespaceDeclItem::Def(d) => {
                state
                    .namespace_members
                    .insert(namespace_join(namespace, &d.name));
            }
            NamespaceDeclItem::Proc(p) => {
                state
                    .namespace_members
                    .insert(namespace_join(namespace, &p.name));
            }
            NamespaceDeclItem::Assert(_)
            | NamespaceDeclItem::Const(_)
            | NamespaceDeclItem::Namespace(_)
            | NamespaceDeclItem::Alias(_) => {}
        }
    }

    let mut local_consts = const_env.clone();
    let mut local_const_names = HashSet::<String>::new();
    for item in items {
        match item {
            NamespaceDeclItem::Assert(assert_decl) => {
                let mut block = Block::Assert(assert_decl.clone());
                rewrite_block_namespace_refs(
                    &mut block,
                    namespace,
                    &local_consts,
                    state,
                    generated,
                )?;
                generated.push(block);
            }
            NamespaceDeclItem::Const(decl) => {
                if !local_const_names.insert(decl.name.clone()) {
                    return Err(vec![Diagnostic::semantic_span(
                        format!(
                            "duplicate constant '{}' in namespace '{}'",
                            decl.name, namespace
                        ),
                        decl.loc.as_ref(),
                    )]);
                }
                let mut decl = decl.clone();
                let value = finalize_const_decl_expr(
                    &mut decl,
                    namespace,
                    &local_consts,
                    state,
                    generated,
                )?;
                state
                    .namespace_const_values
                    .insert(namespace_join(namespace, &decl.name), value.clone());
                local_consts.insert(decl.name.clone(), value);
            }
            NamespaceDeclItem::Struct(s) => {
                let mut s = s.clone();
                s.name = namespace_join(namespace, &s.name);
                let mut block = Block::Struct(s);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                rewrite_block_namespace_refs(&mut block, &ns, &local_consts, state, generated)?;
                generated.push(block);
            }
            NamespaceDeclItem::Def(d) => {
                let mut d = d.clone();
                d.name = namespace_join(namespace, &d.name);
                let mut block = Block::Def(d);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                rewrite_block_namespace_refs(&mut block, &ns, &local_consts, state, generated)?;
                generated.push(block);
            }
            NamespaceDeclItem::Proc(p) => {
                let mut p = p.clone();
                p.name = namespace_join(namespace, &p.name);
                let mut block = Block::Proc(p);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                rewrite_block_namespace_refs(&mut block, &ns, &local_consts, state, generated)?;
                generated.push(block);
            }
            NamespaceDeclItem::Namespace(nested) => {
                let mut nested = nested.clone();
                let full_nested = namespace_join(namespace, &nested.name);
                if nested.params.is_empty() {
                    emit_instantiated_namespace_items(
                        &nested.items,
                        &full_nested,
                        &local_consts,
                        state,
                        generated,
                    )?;
                } else {
                    let mut captured = nested.captured_consts.clone();
                    for (k, v) in &local_consts {
                        captured.insert(k.clone(), v.clone());
                    }
                    nested.captured_consts = captured;
                    state.namespace_templates.insert(full_nested, nested);
                }
            }
            NamespaceDeclItem::Alias(alias) => {
                let mut alias = alias.clone();
                substitute_namespace_ref_consts(&mut alias.target, &local_consts);
                register_namespace_alias(namespace, alias, state)?;
            }
        }
    }
    Ok(())
}

fn substitute_namespace_ref_consts(
    segments: &mut [NamespaceRefSegment],
    const_env: &HashMap<String, Expr>,
) {
    for seg in segments {
        if let Some(args) = &mut seg.args {
            for arg in args {
                arg.expr = substitute_expr_with_env(&arg.expr, const_env);
            }
        }
    }
}
