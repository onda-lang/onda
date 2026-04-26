use super::*;

#[derive(Debug, Clone)]
struct NamespaceTemplateRecord {
    decl: NamespaceDecl,
    captured_artifacts: SemanticConstArtifacts,
    captured_template_consts: HashMap<String, Expr>,
}

#[derive(Debug, Clone)]
struct NamespaceAliasRecord {
    declared_ns: String,
    target: Vec<NamespaceRefSegment>,
}

#[derive(Debug, Default)]
struct NamespaceFlattenState {
    templates: HashMap<String, NamespaceTemplateRecord>,
    aliases: HashMap<String, NamespaceAliasRecord>,
    members: HashSet<String>,
    const_array_names: HashSet<String>,
    scalar_const_names: HashSet<String>,
    const_symbols: HashSet<String>,
    instantiations: HashMap<String, String>,
    next_instantiation_id: u64,
    artifacts: SemanticConstArtifacts,
}

pub(super) fn flatten_namespaces_for_semantics(
    program: &mut Program,
    options: AnalysisOptions,
) -> Result<(), Vec<Diagnostic>> {
    let mut state = NamespaceFlattenState::default();
    let mut errors = Vec::<Diagnostic>::new();
    let mut out = Vec::<Block>::new();
    let template_consts = HashMap::<String, Expr>::new();

    for block in std::mem::take(&mut program.blocks) {
        process_top_level_block(
            block,
            &template_consts,
            options,
            &mut state,
            &mut out,
            &mut errors,
        );
    }

    if errors.is_empty() {
        program.blocks = out;
        Ok(())
    } else {
        Err(errors)
    }
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

fn namespace_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}::{child}")
    }
}

fn namespace_of_symbol(name: &str) -> String {
    name.rsplit_once("::")
        .map(|(ns, _)| ns.to_owned())
        .unwrap_or_default()
}

fn split_namespace_parent_leaf(name: &str) -> (&str, &str) {
    if let Some((parent, leaf)) = name.rsplit_once("::") {
        (parent, leaf)
    } else {
        ("", name)
    }
}

fn looks_like_namespace_ref(name: &str) -> bool {
    name.contains("::") && !name.contains('.')
}

fn split_named_type_base_and_suffix(name: &str) -> (&str, &str) {
    if let Some(idx) = name.find('<') {
        (&name[..idx], &name[idx..])
    } else {
        (name, "")
    }
}

fn format_call_args_as_type_suffix(args: &[NamespaceCallArg]) -> String {
    let parts = args
        .iter()
        .map(|arg| match &arg.expr {
            Expr::Var { name, .. } => name.clone(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>();
    format!("<{}>", parts.join(", "))
}

fn typed_const_value_key(value: TypedConstValue) -> String {
    match value {
        TypedConstValue::F32(v) => format!("f32({v})"),
        TypedConstValue::F64(v) => format!("f64({v})"),
        TypedConstValue::I32(v) => format!("i32({v})"),
        TypedConstValue::I64(v) => format!("i64({v})"),
        TypedConstValue::Bool(v) => format!("bool({v})"),
    }
}

fn register_member_for_item(
    item: &NamespaceItem,
    namespace: &str,
    state: &mut NamespaceFlattenState,
) {
    match item {
        NamespaceItem::Struct(s) => {
            state.members.insert(namespace_join(namespace, &s.name));
        }
        NamespaceItem::Def(d) => {
            state.members.insert(namespace_join(namespace, &d.name));
        }
        NamespaceItem::Proc(p) => {
            state.members.insert(namespace_join(namespace, &p.name));
        }
        NamespaceItem::Const(decl) => {
            let full_name = namespace_join(namespace, &decl.name);
            state.members.insert(full_name.clone());
            if is_const_array_decl(decl) {
                state.const_array_names.insert(full_name);
            } else {
                state.scalar_const_names.insert(full_name);
            }
        }
        NamespaceItem::Assert(_) | NamespaceItem::Namespace(_) | NamespaceItem::Alias(_) => {}
    }
}

fn register_const_def_artifact(
    def: &FunctionDef,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    if !state.const_symbols.insert(def.name.clone()) {
        errors.push(Diagnostic::semantic_span(
            format!("duplicate const symbol '{}'", def.name),
            def.loc,
        ));
        return;
    }
    state
        .artifacts
        .const_def_order
        .insert(def.name.clone(), state.artifacts.const_def_order.len());
    state
        .artifacts
        .const_defs
        .insert(def.name.clone(), def.clone());
}

fn register_const_decl_artifact(
    decl: &ConstDecl,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    if !state.const_symbols.insert(decl.name.clone()) {
        errors.push(Diagnostic::semantic_span(
            format!("duplicate const symbol '{}'", decl.name),
            decl.loc.as_ref(),
        ));
        return;
    }
    let force_const_array = is_const_array_decl(decl)
        || (decl.ty.is_none()
            && is_known_const_array_initializer(
                &decl.expr,
                &state.artifacts.const_values,
                &state.artifacts.const_defs,
            ));
    if force_const_array {
        if let Some(array) = coerce_const_array(
            decl,
            options,
            &state.artifacts.const_values,
            &state.artifacts.const_defs,
            &state.artifacts.const_def_order,
            errors,
        ) {
            record_const_array_artifact(&mut state.artifacts, array);
        }
    } else {
        let inferred_const_array = if decl.ty.is_none() {
            let mut probe_errors = Vec::new();
            coerce_const_array(
                decl,
                options,
                &state.artifacts.const_values,
                &state.artifacts.const_defs,
                &state.artifacts.const_def_order,
                &mut probe_errors,
            )
        } else {
            None
        };
        if let Some(array) = inferred_const_array {
            record_const_array_artifact(&mut state.artifacts, array);
        } else if let Some(value) = coerce_const_scalar(
            decl,
            options,
            &state.artifacts.const_values,
            &state.artifacts.const_defs,
            &state.artifacts.const_def_order,
            errors,
        ) {
            state
                .artifacts
                .const_values
                .insert(decl.name.clone(), ConstValue::Scalar(value));
        }
    }
}

fn register_artifact_for_block(
    block: &Block,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Def(def) if def.is_const => register_const_def_artifact(def, state, errors),
        Block::Const(decl) => register_const_decl_artifact(decl, options, state, errors),
        _ => {}
    }
}

fn merge_generated_namespace_artifacts(
    target: &mut SemanticConstArtifacts,
    emitted: SemanticConstArtifacts,
    namespace: &str,
) {
    let prefix = format!("{namespace}::");

    let mut generated_defs = emitted
        .const_defs
        .into_iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .collect::<Vec<_>>();
    generated_defs.sort_by_key(|(name, _)| {
        emitted
            .const_def_order
            .get(name)
            .copied()
            .unwrap_or(usize::MAX)
    });
    for (name, def) in generated_defs {
        target
            .const_def_order
            .insert(name.clone(), target.const_def_order.len());
        target.const_defs.insert(name, def);
    }

    for (name, value) in emitted.const_values {
        if name.starts_with(&prefix) && matches!(value, ConstValue::Scalar(_)) {
            target.const_values.insert(name, value);
        }
    }

    for array in emitted
        .const_arrays
        .into_iter()
        .filter(|array| array.name.starts_with(&prefix))
    {
        record_const_array_artifact(target, array);
    }
}

fn process_top_level_block(
    block: Block,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    out: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Namespace(decl) => {
            process_namespace_decl(decl, "", template_consts, options, state, out, errors);
        }
        Block::NamespaceAlias(alias) => {
            register_namespace_alias("", alias, state, errors);
        }
        mut block => {
            let current_ns = match &block {
                Block::Struct(s) => namespace_of_symbol(&s.name),
                Block::Def(d) => namespace_of_symbol(&d.name),
                Block::Proc(p) => namespace_of_symbol(&p.name),
                _ => String::new(),
            };
            let mut generated = Vec::<Block>::new();
            rewrite_block_namespace_refs(
                &mut block,
                &current_ns,
                template_consts,
                options,
                state,
                &mut generated,
                errors,
            );
            out.extend(generated);
            register_artifact_for_block(&block, options, state, errors);
            out.push(block);
        }
    }
}

fn process_namespace_decl(
    decl: NamespaceDecl,
    parent_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    out: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    let full_ns = namespace_join(parent_ns, &decl.name);
    if decl.params.is_empty() {
        emit_namespace_items(
            &decl.items,
            &full_ns,
            template_consts,
            options,
            state,
            out,
            errors,
        );
    } else {
        if state.templates.contains_key(&full_ns) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate namespace template '{full_ns}'"),
                decl.loc.as_ref(),
            ));
            return;
        }
        state.templates.insert(
            full_ns,
            NamespaceTemplateRecord {
                decl,
                captured_artifacts: state.artifacts.clone(),
                captured_template_consts: template_consts.clone(),
            },
        );
    }
}

fn emit_namespace_items(
    items: &[NamespaceItem],
    namespace: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    out: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for item in items {
        register_member_for_item(item, namespace, state);
    }

    let mut local_const_names = HashSet::<String>::new();
    for item in items {
        match item {
            NamespaceItem::Assert(assert_decl) => {
                let mut block = Block::Assert(assert_decl.clone());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    namespace,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                out.push(block);
            }
            NamespaceItem::Const(decl) => {
                if !local_const_names.insert(decl.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "duplicate constant '{}' in namespace '{}'",
                            decl.name, namespace
                        ),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                let mut decl = decl.clone();
                decl.name = namespace_join(namespace, &decl.name);
                let mut block = Block::Const(decl);
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    namespace,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                register_artifact_for_block(&block, options, state, errors);
                out.push(block);
            }
            NamespaceItem::Struct(s) => {
                let mut s = s.clone();
                s.name = namespace_join(namespace, &s.name);
                let mut block = Block::Struct(s);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                out.push(block);
            }
            NamespaceItem::Def(d) => {
                let mut d = d.clone();
                d.name = namespace_join(namespace, &d.name);
                let mut block = Block::Def(d);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                register_artifact_for_block(&block, options, state, errors);
                out.push(block);
            }
            NamespaceItem::Proc(p) => {
                let mut p = p.clone();
                p.name = namespace_join(namespace, &p.name);
                let mut block = Block::Proc(p);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                out.push(block);
            }
            NamespaceItem::Namespace(nested) => {
                process_namespace_decl(
                    nested.clone(),
                    namespace,
                    template_consts,
                    options,
                    state,
                    out,
                    errors,
                );
            }
            NamespaceItem::Alias(alias) => {
                let mut alias = alias.clone();
                rewrite_namespace_ref_args(
                    &mut alias.target,
                    namespace,
                    template_consts,
                    options,
                    state,
                    out,
                    errors,
                );
                register_namespace_alias(namespace, alias, state, errors);
            }
        }
    }
}

fn block_decl_name(block: &Block) -> Option<&str> {
    match block {
        Block::Const(c) => Some(c.name.as_str()),
        Block::Struct(s) => Some(s.name.as_str()),
        Block::Def(d) => Some(d.name.as_str()),
        Block::Proc(p) => Some(p.name.as_str()),
        Block::Namespace(ns) => Some(ns.name.as_str()),
        Block::NamespaceAlias(alias) => Some(alias.name.as_str()),
        _ => None,
    }
}

fn register_namespace_alias(
    parent_ns: &str,
    alias: NamespaceAliasDecl,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    let full_name = namespace_join(parent_ns, &alias.name);
    if state.aliases.contains_key(&full_name) {
        errors.push(Diagnostic::semantic_span(
            format!("duplicate namespace alias '{full_name}'"),
            alias.loc.as_ref(),
        ));
        return;
    }
    state.aliases.insert(
        full_name,
        NamespaceAliasRecord {
            declared_ns: parent_ns.to_owned(),
            target: alias.target,
        },
    );
}

fn resolve_visible_alias(
    alias_leaf: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
) -> Option<NamespaceAliasRecord> {
    for ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&ns, alias_leaf);
        if let Some(alias) = state.aliases.get(&candidate) {
            return Some(alias.clone());
        }
    }
    None
}

fn has_visible_namespace_prefix(prefix: &str, state: &NamespaceFlattenState) -> bool {
    let nested_prefix = format!("{prefix}::");
    state.templates.contains_key(prefix)
        || state.aliases.contains_key(prefix)
        || state
            .members
            .iter()
            .any(|name| name.starts_with(&nested_prefix))
        || state
            .templates
            .keys()
            .any(|name| name.starts_with(&nested_prefix))
        || state
            .aliases
            .keys()
            .any(|name| name.starts_with(&nested_prefix))
}

fn qualify_local_namespace_member_name(
    name: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
) -> Option<String> {
    if name.is_empty() || name.contains("::") || name.contains('.') {
        return None;
    }
    let (base, suffix) = split_named_type_base_and_suffix(name);
    for ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&ns, base);
        if state.members.contains(&candidate) {
            return Some(format!("{candidate}{suffix}"));
        }
    }
    None
}

fn resolve_namespace_symbol_name(
    name: &str,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_span: Span,
) -> Option<String> {
    if !looks_like_namespace_ref(name) {
        return Some(name.to_owned());
    }
    let segments = match onda_frontend::parse_namespace_ref_text_ast(name) {
        Ok(segments) => segments,
        Err(mut diags) => {
            errors.append(&mut diags);
            return None;
        }
    };
    resolve_namespace_segments_internal(
        &segments,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        use_site_span,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_namespace_segments_internal(
    segments: &[NamespaceRefSegment],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_span: Span,
    depth: usize,
) -> Option<String> {
    if depth > 64 {
        errors.push(Diagnostic::semantic_span(
            "namespace alias/template resolution exceeded recursion depth",
            use_site_span,
        ));
        return None;
    }
    if segments.is_empty() {
        errors.push(Diagnostic::semantic_span(
            "empty namespace reference",
            use_site_span,
        ));
        return None;
    }

    let mut idx = 0usize;
    let mut path = String::new();
    let empty_call_args = Vec::<NamespaceCallArg>::new();

    if segments[0].args.is_none() {
        if let Some(alias) = resolve_visible_alias(&segments[0].name, current_ns, state) {
            path = resolve_namespace_segments_internal(
                &alias.target,
                &alias.declared_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
                depth + 1,
            )?;
            idx = 1;
        }
    }

    if idx == 0 {
        if let Some(args) = &segments[0].args {
            let mut resolved = None::<String>;
            let before_errors = errors.len();
            for candidate_ns in namespace_candidates(current_ns) {
                let candidate = namespace_join(&candidate_ns, &segments[0].name);
                if state.templates.contains_key(&candidate) {
                    resolved = instantiate_namespace_template(
                        &candidate,
                        args,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        use_site_span,
                    );
                    break;
                }
            }
            let Some(found) = resolved else {
                if errors.len() > before_errors {
                    return None;
                }
                errors.push(Diagnostic::semantic_span(
                    format!("unknown namespace template '{}'", segments[0].name),
                    use_site_span,
                ));
                return None;
            };
            path = found;
        } else {
            let mut resolved = None::<String>;
            for candidate_ns in namespace_candidates(current_ns) {
                let candidate = namespace_join(&candidate_ns, &segments[0].name);
                if state.templates.contains_key(&candidate) {
                    resolved = instantiate_namespace_template(
                        &candidate,
                        &empty_call_args,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        use_site_span,
                    );
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
            if state.templates.contains_key(&candidate) {
                path = instantiate_namespace_template(
                    &candidate,
                    args,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_span,
                )?;
            } else {
                let suffix = format_call_args_as_type_suffix(args);
                path = format!("{candidate}{suffix}");
            }
        } else if state.templates.contains_key(&candidate) {
            path = instantiate_namespace_template(
                &candidate,
                &empty_call_args,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
            )?;
        } else {
            path = candidate;
        }
    }
    Some(path)
}

#[allow(clippy::too_many_arguments)]
fn instantiate_namespace_template(
    full_template_name: &str,
    call_args: &[NamespaceCallArg],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_span: Span,
) -> Option<String> {
    let template = match state.templates.get(full_template_name).cloned() {
        Some(template) => template,
        None => {
            errors.push(Diagnostic::semantic_span(
                format!("unknown namespace template '{full_template_name}'"),
                use_site_span,
            ));
            return None;
        }
    };

    let mut named = HashMap::<String, (Expr, Span)>::new();
    let mut positional = Vec::<Expr>::new();
    for arg in call_args {
        let mut expr = arg.expr.clone();
        rewrite_expr(
            &mut expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
        let arg_span = expr.loc().span();
        if let Some(name) = &arg.name {
            if named.insert(name.clone(), (expr, arg_span)).is_some() {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "namespace template '{}' argument '{}' specified more than once",
                        full_template_name, name
                    ),
                    if arg_span.is_zero() {
                        use_site_span
                    } else {
                        arg_span
                    },
                ));
                return None;
            }
        } else {
            positional.push(expr);
        }
    }

    let mut effective_template_consts = template.captured_template_consts.clone();
    let template_parent_ns = namespace_parent(full_template_name).unwrap_or("");
    let mut param_values = Vec::<(String, TypedConstValue, Span)>::new();
    let mut pos_idx = 0usize;
    for param in &template.decl.params {
        let (mut value_expr, value_span, use_captured_artifacts) =
            if let Some((expr, span)) = named.remove(&param.name) {
                (expr, span, false)
            } else if let Some(pos_expr) = positional.get(pos_idx) {
                pos_idx += 1;
                (pos_expr.clone(), pos_expr.loc().span(), false)
            } else {
                let mut default = param.default.clone();
                rewrite_expr(
                    &mut default,
                    template_parent_ns,
                    &effective_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
                let span = default.loc().span();
                (default, span, true)
            };
        if !use_captured_artifacts {
            rewrite_expr(
                &mut value_expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        let eval_artifacts = if use_captured_artifacts {
            &template.captured_artifacts
        } else {
            &state.artifacts
        };
        let value = eval_namespace_template_arg(
            &value_expr,
            eval_artifacts,
            options,
            &format!(
                "namespace template '{}' argument '{}'",
                full_template_name, param.name
            ),
            errors,
        )?;
        effective_template_consts.insert(
            param.name.clone(),
            typed_const_expr_with_loc(value, value_expr.loc()),
        );
        param_values.push((param.name.clone(), value, value_span));
    }

    if pos_idx < positional.len() {
        let extra_arg_span = positional
            .get(pos_idx)
            .map(|expr| expr.loc().span())
            .filter(|span| !span.is_zero())
            .unwrap_or(use_site_span);
        errors.push(Diagnostic::semantic_span(
            format!(
                "namespace template '{}' received too many positional arguments",
                full_template_name
            ),
            extra_arg_span,
        ));
        return None;
    }
    if !named.is_empty() {
        let mut unknown = named.keys().cloned().collect::<Vec<_>>();
        unknown.sort();
        let unknown_span = unknown
            .iter()
            .filter_map(|name| named.get(name).map(|(_, span)| *span))
            .find(|span| !span.is_zero())
            .unwrap_or(use_site_span);
        errors.push(Diagnostic::semantic_span(
            format!(
                "namespace template '{}' received unknown named arguments: {}",
                full_template_name,
                unknown.join(", ")
            ),
            unknown_span,
        ));
        return None;
    }

    let key = {
        let values = param_values
            .iter()
            .map(|(_, value, _)| typed_const_value_key(*value))
            .collect::<Vec<_>>()
            .join(",");
        format!("{full_template_name}[{values}]")
    };
    if let Some(existing) = state.instantiations.get(&key) {
        return Some(existing.clone());
    }

    let (parent, leaf) = split_namespace_parent_leaf(full_template_name);
    let concrete_leaf = format!("{leaf}__nsinst{}", state.next_instantiation_id);
    state.next_instantiation_id += 1;
    let concrete_ns = namespace_join(parent, &concrete_leaf);
    state.instantiations.insert(key, concrete_ns.clone());

    let saved_artifacts =
        std::mem::replace(&mut state.artifacts, template.captured_artifacts.clone());
    emit_namespace_items(
        &template.decl.items,
        &concrete_ns,
        &effective_template_consts,
        options,
        state,
        generated,
        errors,
    );
    let emitted_artifacts = std::mem::replace(&mut state.artifacts, saved_artifacts);
    merge_generated_namespace_artifacts(&mut state.artifacts, emitted_artifacts, &concrete_ns);

    Some(concrete_ns)
}

fn eval_namespace_template_arg(
    expr: &Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    eval_const_scalar_expr_with_defs(
        expr,
        PrimitiveType::I32,
        &locals,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        context,
        &mut Vec::new(),
        errors,
    )
}

#[allow(clippy::too_many_arguments)]
fn rewrite_namespace_ref_args(
    segments: &mut [NamespaceRefSegment],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for seg in segments {
        if let Some(args) = &mut seg.args {
            for arg in args {
                rewrite_expr(
                    &mut arg.expr,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
    }
}

fn rewrite_named_type_ref_name(
    name: &mut String,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_loc: impl Into<SourceLoc>,
) {
    let use_site_span = use_site_loc.into().span();
    if let Some(qualified) = qualify_local_namespace_member_name(name, current_ns, state) {
        *name = qualified;
        return;
    }

    let (base, suffix) = split_named_type_base_and_suffix(name);
    if !suffix.is_empty() {
        if let Some(qualified) = qualify_local_namespace_member_name(base, current_ns, state) {
            *name = format!("{qualified}{suffix}");
            return;
        }
        if looks_like_namespace_ref(base) {
            if let Some(resolved) = resolve_namespace_symbol_name(
                base,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
            ) {
                *name = format!("{resolved}{suffix}");
            }
            return;
        }
    }

    if looks_like_namespace_ref(name) {
        if let Some(resolved) = resolve_namespace_symbol_name(
            name,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            use_site_span,
        ) {
            *name = resolved;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn qualify_or_resolve_symbol_name(
    name: &str,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) -> Option<String> {
    if let Some(qualified) = qualify_local_namespace_member_name(name, current_ns, state) {
        return Some(qualified);
    }
    if looks_like_namespace_ref(name) {
        return resolve_namespace_symbol_name(
            name,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc.into().span(),
        );
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn qualify_instance_method_call_name(
    name: &mut String,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let Some((base, method)) = name.rsplit_once('.') else {
        return;
    };
    if let Some(resolved) = qualify_or_resolve_symbol_name(
        base,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        loc,
    ) {
        *name = format!("{resolved}.{method}");
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_block_namespace_refs(
    block: &mut Block,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(port_block) | Block::Outs(port_block) => {
            rewrite_deferred_port_count(
                port_block,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_port_decls(
                &mut port_block.decls,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Params(param_block) => {
            rewrite_deferred_param_count(
                param_block,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_param_decls(
                &mut param_block.decls,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Const(decl) => {
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                rewrite_expr(
                    size,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_expr(
                &mut decl.expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Buffers(buffer_block) => {
            rewrite_deferred_buffer_count(
                buffer_block,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            for decl in &mut buffer_block.decls {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(
                        ty,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        decl.ty_loc.as_ref().or(decl.loc.as_ref()),
                    );
                }
            }
        }
        Block::Assert(assert_decl) => {
            rewrite_expr(
                &mut assert_decl.expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Events(events) => {
            for event in events {
                rewrite_event_def(
                    event,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
        Block::Struct(s) => {
            for field in &mut s.fields {
                rewrite_field_type(
                    &mut field.ty,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    field.ty_loc.as_ref().or(field.loc.as_ref()),
                );
                if let Some(default) = &mut field.default {
                    rewrite_expr(
                        default,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                    );
                }
            }
            for method in &mut s.methods {
                rewrite_function_def(
                    method,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
        Block::Def(d) => rewrite_function_def(
            d,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        ),
        Block::Proc(p) => {
            let proc_template_consts = template_consts.clone();
            for decl in &mut p.consts {
                if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                    rewrite_expr(
                        size,
                        current_ns,
                        &proc_template_consts,
                        options,
                        state,
                        generated,
                        errors,
                    );
                }
                rewrite_expr(
                    &mut decl.expr,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_deferred_proc_port_count(
                &mut p.ins_deferred_count,
                &mut p.ins_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_deferred_proc_port_count(
                &mut p.outs_deferred_count,
                &mut p.outs_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_deferred_proc_param_count(
                &mut p.params_deferred_count,
                &mut p.params_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_deferred_proc_buffer_count(
                &mut p.buffers_deferred_count,
                &mut p.buffers_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_port_decls(
                &mut p.ins,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_port_decls(
                &mut p.outs,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_param_decls(
                &mut p.params,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            for event in &mut p.events {
                rewrite_event_def(
                    event,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            for decl in &mut p.buffers {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(
                        ty,
                        current_ns,
                        &proc_template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        decl.ty_loc.as_ref().or(decl.loc.as_ref()),
                    );
                }
            }
            if let Some(default_ty) = &mut p.init.default_ty {
                rewrite_decl_type(
                    default_ty,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    p.init.default_ty_loc.as_ref().or(p.init.loc.as_ref()),
                );
            }
            rewrite_stmts(
                &mut p.init.body,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_stmts(
                &mut p.block_pre,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            if let Some(os) = &mut p.sample_oversample_factor {
                rewrite_expr(
                    os,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_stmts(
                &mut p.sample,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_stmts(
                &mut p.block_post,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            if let Some(graph) = &mut p.graph {
                rewrite_graph_block(
                    graph,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            for def in &mut p.local_defs {
                rewrite_function_def(
                    def,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
        Block::Init(init) => {
            if let Some(default_ty) = &mut init.default_ty {
                rewrite_decl_type(
                    default_ty,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    init.default_ty_loc.as_ref().or(init.loc.as_ref()),
                );
            }
            rewrite_stmts(
                &mut init.body,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Block(exec) => {
            rewrite_stmts(
                &mut exec.pre,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            if let Some(sample) = &mut exec.sample {
                if let Some(os) = &mut sample.oversample_factor {
                    rewrite_expr(
                        os,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                    );
                }
                rewrite_stmts(
                    &mut sample.body,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_stmts(
                &mut exec.post,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Sample(sample) => {
            if let Some(os) = &mut sample.oversample_factor {
                rewrite_expr(
                    os,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_stmts(
                &mut sample.body,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Graph(graph) => {
            rewrite_graph_block(
                graph,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Namespace(_) | Block::NamespaceAlias(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_port_count(
    port_block: &mut PortBlock,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = &mut port_block.deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = &mut port_block.deferred_default_ty {
        rewrite_decl_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            port_block.loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_param_count(
    param_block: &mut ParamBlock,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = &mut param_block.deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = &mut param_block.deferred_default_ty {
        rewrite_decl_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            param_block.loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_buffer_count(
    buffer_block: &mut BufferBlock,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = &mut buffer_block.deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = &mut buffer_block.deferred_default_ty {
        rewrite_buffer_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            buffer_block.loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_proc_port_count(
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    loc: &Span,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = deferred_default_ty {
        rewrite_decl_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_proc_param_count(
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    loc: &Span,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_deferred_proc_port_count(
        deferred_count,
        deferred_default_ty,
        loc,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
    );
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_proc_buffer_count(
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<BufferType>,
    loc: &Span,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = deferred_default_ty {
        rewrite_buffer_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_port_decls(
    decls: &mut [PortDecl],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for decl in decls {
        rewrite_decl_type_default_range(
            &mut decl.ty,
            &mut decl.default,
            &mut decl.range,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            decl.ty_loc.as_ref().or(decl.loc.as_ref()),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_param_decls(
    decls: &mut [ParamDecl],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for decl in decls {
        rewrite_decl_type_default_range(
            &mut decl.ty,
            &mut decl.default,
            &mut decl.range,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            decl.ty_loc.as_ref().or(decl.loc.as_ref()),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_decl_type_default_range(
    ty: &mut Option<DeclType>,
    default: &mut Option<Expr>,
    range: &mut Option<DeclRange>,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    if let Some(ty) = ty {
        rewrite_decl_type(
            ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc,
        );
    }
    if let Some(default) = default {
        rewrite_expr(
            default,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            rewrite_expr(
                min,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        rewrite_expr(
            &mut range.max,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_graph_block(
    graph: &mut GraphBlock,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &mut graph.edges {
        rewrite_expr(
            &mut edge.source,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
        if let Some(delay) = &mut edge.delay {
            rewrite_expr(
                delay,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                rewrite_expr(
                    index,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_function_def(
    def: &mut FunctionDef,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        if let Some(ty) = &mut param.ty {
            rewrite_fn_param_type(
                ty,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                param.ty_loc.as_ref().or(param.loc.as_ref()),
            );
        }
        if let Some(default) = &mut param.default {
            rewrite_expr(
                default,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
    }
    if let Some(return_ty) = &mut def.return_ty {
        rewrite_fn_return_type(
            return_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            def.return_ty_loc.as_ref().or(def.loc.as_ref()),
        );
    }
    rewrite_stmts(
        &mut def.body,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
    );
}

#[allow(clippy::too_many_arguments)]
fn rewrite_event_def(
    event: &mut EventDef,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut event.params {
        match &mut param.ty {
            EventParamType::Array { size, .. } => {
                rewrite_expr(
                    size,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            EventParamType::GenericSlice { elem } => {
                rewrite_named_type_ref_name(
                    elem,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    param.ty_loc.as_ref().or(param.loc.as_ref()),
                );
            }
            EventParamType::Scalar(_) | EventParamType::Slice { .. } => {}
        }
        if let Some(default) = &mut param.default {
            rewrite_expr(
                default,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
    }
    rewrite_stmts(
        &mut event.body,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
    );
}

#[allow(clippy::too_many_arguments)]
fn rewrite_decl_type(
    ty: &mut DeclType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        DeclType::Generic(name) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        DeclType::ArrayGeneric { elem, size } => {
            rewrite_named_type_ref_name(
                elem,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        DeclType::Array { size, .. } => {
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        DeclType::Scalar(_) | DeclType::Tuple(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_field_type(
    ty: &mut FieldType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FieldType::Generic(name) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FieldType::Array(spec) => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc,
                );
            }
            rewrite_expr(
                &mut spec.size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FieldType::Scalar(_) | FieldType::Tuple(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_fn_param_type(
    ty: &mut FnParamType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FnParamType::Struct(name) | FnParamType::ArrayGeneric(name) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnParamType::SizedArray {
            generic_name, size, ..
        } => {
            if let Some(name) = generic_name {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc,
                );
            }
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FnParamType::Buffer(buffer_ty) => {
            rewrite_buffer_type(
                buffer_ty,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnParamType::Primitive(_)
        | FnParamType::Array(_)
        | FnParamType::BareBuffer
        | FnParamType::Tuple(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_fn_return_type(
    ty: &mut FnReturnType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FnReturnType::Scalar(FnReturnScalarType::Named(name)) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnReturnType::Tuple(elems) => {
            for elem in elems {
                if let FnReturnScalarType::Named(name) = elem {
                    rewrite_named_type_ref_name(
                        name,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        loc,
                    );
                }
            }
        }
        FnReturnType::Array { size, .. } => {
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FnReturnType::Scalar(FnReturnScalarType::Primitive(_)) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_buffer_type(
    ty: &mut BufferType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    if let BufferElemType::Generic(name) = &mut ty.elem {
        rewrite_named_type_ref_name(
            name,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc,
        );
    }
    if let BufferChannels::Static(expr) = &mut ty.channels {
        rewrite_expr(
            expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_stmts(
    stmts: &mut Vec<Stmt>,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        rewrite_stmt(
            stmt,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_stmt(
    stmt: &mut Stmt,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                rewrite_expr(
                    size,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_expr(
                &mut decl.expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Stmt::Assign {
            target,
            generic_decl_ty,
            typed_decl_ty_loc,
            target_loc,
            expr,
            ..
        } => {
            match target {
                AssignTarget::Var(name) => {
                    if looks_like_namespace_ref(name) {
                        if let Some(resolved) = resolve_namespace_symbol_name(
                            name,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                            target_loc
                                .as_ref()
                                .map(SourceLoc::from)
                                .unwrap_or_default()
                                .span(),
                        ) {
                            *name = resolved;
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if looks_like_namespace_ref(base) {
                        if let Some(resolved) = resolve_namespace_symbol_name(
                            base,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                            target_loc
                                .as_ref()
                                .map(SourceLoc::from)
                                .unwrap_or_default()
                                .span(),
                        ) {
                            *base = resolved;
                        }
                    }
                    rewrite_expr(
                        index,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                    );
                }
                AssignTarget::Slice { base, start, end } => {
                    if looks_like_namespace_ref(base) {
                        if let Some(resolved) = resolve_namespace_symbol_name(
                            base,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                            target_loc
                                .as_ref()
                                .map(SourceLoc::from)
                                .unwrap_or_default()
                                .span(),
                        ) {
                            *base = resolved;
                        }
                    }
                    if let Some(start) = start {
                        rewrite_expr(
                            start,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                        );
                    }
                    if let Some(end) = end {
                        rewrite_expr(
                            end,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                        );
                    }
                }
                AssignTarget::Tuple(_) => {}
            }
            if let Some(name) = generic_decl_ty {
                if looks_like_namespace_ref(name) {
                    if let Some(resolved) = resolve_namespace_symbol_name(
                        name,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        typed_decl_ty_loc
                            .as_ref()
                            .or(target_loc.as_ref())
                            .map(SourceLoc::from)
                            .unwrap_or_default()
                            .span(),
                    ) {
                        *name = resolved;
                    }
                }
            }
            rewrite_expr(
                expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_expr(
                expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr(
                cond,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_stmts(
                then_branch,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_stmts(
                else_branch,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                rewrite_expr(
                    step,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_expr(
                start,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_expr(
                end,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_stmts(
                body,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Stmt::While { cond, body, .. } => {
            rewrite_expr(
                cond,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_stmts(
                body,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_expr(
    expr: &mut Expr,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    let use_site_loc = expr.loc();
    if let Expr::Var { name, .. } = expr {
        if let Some(value) = template_consts.get(name).cloned() {
            *expr = value.with_loc(use_site_loc);
            return;
        }
    }

    match expr {
        Expr::Var { name, .. } => {
            if let Some(qualified) = qualify_local_namespace_member_name(name, current_ns, state) {
                *name = qualified;
            } else if looks_like_namespace_ref(name) {
                if let Some(resolved) = resolve_namespace_symbol_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_loc.span(),
                ) {
                    *name = resolved;
                }
            }
        }
        Expr::Index { base, index, .. } => {
            if let Some(qualified) = qualify_local_namespace_member_name(base, current_ns, state) {
                *base = qualified;
            } else if looks_like_namespace_ref(base) {
                if let Some(resolved) = resolve_namespace_symbol_name(
                    base,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_loc.span(),
                ) {
                    *base = resolved;
                }
            }
            rewrite_expr(
                index,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            if let Some(qualified) = qualify_local_namespace_member_name(base, current_ns, state) {
                *base = qualified;
            } else if looks_like_namespace_ref(base) {
                if let Some(resolved) = resolve_namespace_symbol_name(
                    base,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_loc.span(),
                ) {
                    *base = resolved;
                }
            }
            if let Some(start) = start {
                rewrite_expr(
                    start,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            if let Some(end) = end {
                rewrite_expr(
                    end,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { loc, spec, init } => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc.as_ref(),
                );
            }
            rewrite_expr(
                &mut spec.size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_expr(
                        value,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_expr(
                lhs,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_expr(
                rhs,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_expr(
                    arg,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            qualify_instance_method_call_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_loc,
            );
            if let Some(qualified) = qualify_local_namespace_member_name(name, current_ns, state) {
                *name = qualified;
            } else if looks_like_namespace_ref(name) {
                if let Some(resolved) = resolve_namespace_symbol_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_loc.span(),
                ) {
                    *name = resolved;
                }
            }
            for arg in args {
                rewrite_expr(
                    &mut arg.expr,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            rewrite_expr(
                expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_expr(
                    value,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}
