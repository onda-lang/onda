//! Check template constant declarations from metadata, without instantiation.
use super::*;

#[derive(Clone, Default)]
pub(super) struct TemplateConstScope {
    aliases: HashMap<String, String>,
    uses: UseScope,
    members: HashSet<String>,
    enums: HashMap<String, EnumType>,
}

/// Namespace arguments live in source name spellings rather than expression
/// children. Check those here; ordinary const checking owns all other names.
pub(super) fn validate_const_namespace_arguments(
    expr: &Expr,
    namespace: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    errors: &mut Vec<Diagnostic>,
) {
    let name = referenced_const_name(expr).or(match expr {
        Expr::UserCall { name, .. } => Some(name.as_str()),
        _ => None,
    });
    let Some(name) = name.filter(|name| looks_like_namespace_ref(name)) else {
        return;
    };
    let Ok(segments) = onda_frontend::parse_namespace_ref_text_ast(name) else {
        return;
    };
    validate_namespace_arguments(
        &segments,
        namespace,
        state,
        scope,
        "namespace template const",
        errors,
    );
}

pub(super) fn validate_namespace_arguments(
    segments: &[NamespaceRefSegment],
    namespace: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    for segment in segments {
        if let Some(args) = &segment.args {
            for arg in args {
                validate_template_static_expr_refs(
                    &arg.expr, namespace, state, scope, context, errors,
                );
            }
        }
    }
}

pub(super) fn template_member_paths(decl: &NamespaceDecl, namespace: &str) -> HashSet<String> {
    let mut members = HashSet::new();
    let mut pending = vec![(&decl.items, namespace.to_owned())];
    while let Some((items, namespace)) = pending.pop() {
        for item in items {
            let name = match item {
                NamespaceItem::Enum(def) => {
                    let owner = namespace_join(&namespace, &def.name);
                    members.extend(
                        def.members
                            .iter()
                            .map(|member| namespace_join(&owner, &member.name)),
                    );
                    &def.name
                }
                NamespaceItem::Const(decl) => &decl.name,
                NamespaceItem::Def(def) => &def.name,
                NamespaceItem::Struct(def) => &def.name,
                NamespaceItem::Proc(proc) => &proc.name,
                NamespaceItem::Alias(alias) => &alias.name,
                NamespaceItem::Namespace(child) => {
                    let child_ns = namespace_join(&namespace, &child.name);
                    pending.push((&child.items, child_ns.clone()));
                    members.insert(child_ns);
                    continue;
                }
                _ => continue,
            };
            members.insert(namespace_join(&namespace, name));
        }
    }
    members
}

pub(super) fn validate_template_consts(
    decl: &NamespaceDecl,
    namespace: &str,
    state: &mut NamespaceFlattenState,
    parent: Option<(&ConstCheck, &TemplateConstScope)>,
    template_consts: &HashMap<String, Expr>,
    errors: &mut Vec<Diagnostic>,
) -> (ConstCheck, SemanticConstArtifacts, TemplateConstScope) {
    let mut check = parent
        .map(|(check, _)| check.clone())
        .unwrap_or_else(ConstCheck::empty);
    let mut captured = SemanticConstArtifacts::default();
    let mut scope = parent.map(|(_, scope)| scope.clone()).unwrap_or_default();
    scope.members.extend(template_member_paths(decl, namespace));
    for name in template_consts.keys() {
        check.scalar_constant(name, PrimitiveType::I32, false);
        scope.members.insert(name.clone());
    }
    for param in &decl.params {
        capture_references::visit_expression(&param.default, &mut |name, loc| {
            bind_name(
                name,
                loc,
                namespace_parent(namespace).unwrap_or(""),
                state,
                &scope,
                &mut check,
                &mut captured,
                errors,
            );
        });
        check.scalar_constant(&param.name, PrimitiveType::I32, false);
        check.alias(&namespace_join(namespace, &param.name), &param.name);
        scope.members.insert(param.name.clone());
        scope.members.insert(namespace_join(namespace, &param.name));
    }
    for item in &decl.items {
        capture_references::visit_item(item, &mut |name, loc| {
            bind_name(
                name,
                loc,
                namespace,
                state,
                &scope,
                &mut check,
                &mut captured,
                errors,
            );
        });
        match item {
            NamespaceItem::Enum(def) => {
                let Some(ty) = enums::allocate_type(def, state, errors) else {
                    continue;
                };
                scope.enums.insert(namespace_join(namespace, &def.name), ty);
                for member in &def.members {
                    let name = namespace_join(&def.name, &member.name);
                    check.scalar_constant(&name, PrimitiveType::Enum(ty), false);
                    check.alias(&namespace_join(namespace, &name), &name);
                }
            }
            NamespaceItem::Const(decl) => {
                let mut decl = decl.clone();
                let resolve = |name: &str| resolve_enum(name, namespace, state, &scope);
                if let Some(ty) = &mut decl.ty {
                    let elem = match ty {
                        ConstType::Scalar(ty)
                        | ConstType::Array { elem: ty, .. }
                        | ConstType::Slice { elem: ty } => ty,
                    };
                    enums::scalar_ref(elem, resolve);
                }
                if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                    enums::rewrite_const_expr(size, resolve, errors);
                }
                enums::rewrite_const_expr(&mut decl.expr, resolve, errors);
                check.constant(&decl, errors);
                check.alias(&namespace_join(namespace, &decl.name), &decl.name);
            }
            NamespaceItem::Def(def) if def.is_const => {
                let mut def = def.clone();
                enums::rewrite_const_function(
                    &mut def,
                    |name| resolve_enum(name, namespace, state, &scope),
                    errors,
                );
                let def = &def;
                validate_const_def_declaration(def, errors);
                let resolve_size = |expr: &Expr, context: &str, errors: &mut Vec<Diagnostic>| {
                    Some(check.dimension(expr, context, errors))
                };
                let result = const_def_return_type(def, resolve_size, errors);
                let params = const_def_param_signature(def, resolve_size, errors);
                if let (Some(result), Some(params)) = (result, params) {
                    // Defaults are checked before parameter bindings. The body
                    // gets a separate scope, so locals never escape a def.
                    check.template_function(def, result);
                    check.alias(&namespace_join(namespace, &def.name), &def.name);
                    let mut body_check = check.definition_scope(def);
                    body_check.definition(def, &params, result, errors);
                }
            }
            NamespaceItem::Namespace(child) => {
                let child_ns = namespace_join(namespace, &child.name);
                let (child_check, child_capture, child_scope) = validate_template_consts(
                    child,
                    &child_ns,
                    state,
                    Some((&check, &scope)),
                    template_consts,
                    errors,
                );
                let prefix = format!("{child_ns}::");
                scope.enums.extend(
                    child_scope
                        .enums
                        .into_iter()
                        .filter(|(name, _)| name.starts_with(&prefix)),
                );
                check.import_namespace(&child_ns, &child_check);
                captured.import_all(&child_capture);
            }
            NamespaceItem::Alias(alias) => {
                scope
                    .aliases
                    .insert(alias.name.clone(), namespace_segments_key(&alias.target));
            }
            NamespaceItem::Use(use_decl) => {
                let target = template_reference_target(
                    &namespace_segments_key(&use_decl.target),
                    &scope.aliases,
                );
                let Some(target) = namespace_ref_candidates(&target, namespace, state)
                    .into_iter()
                    .find(|candidate| {
                        metadata_contains(candidate, &check, state, &scope)
                            || scope.members.contains(candidate)
                            || template_or_member_path_exists(candidate, state)
                            || has_visible_namespace_prefix(candidate, state)
                    })
                else {
                    continue; // The namespace-reference checker diagnoses unknown use targets.
                };
                if metadata_contains(&target, &check, state, &scope) {
                    let name = use_decl
                        .alias
                        .as_deref()
                        .unwrap_or_else(|| split_namespace_parent_leaf(&target).1);
                    scope.uses.symbol(name, target.clone());
                } else if let Some(alias) = &use_decl.alias {
                    scope.aliases.insert(alias.clone(), target);
                } else {
                    scope.uses.namespace(target);
                }
            }
            _ => {}
        }
    }
    (check, captured, scope)
}

fn resolve_enum(
    name: &str,
    namespace: &str,
    state: &NamespaceFlattenState,
    scope: &TemplateConstScope,
) -> Option<PrimitiveType> {
    let target = template_reference_target(name, &scope.aliases);
    let candidates = namespace_ref_candidates(&target, namespace, state);
    let lookup = |name: &str| {
        scope
            .enums
            .get(name)
            .or_else(|| state.enums.get(name))
            .copied()
    };
    let exists = |name: &str| lookup(name).is_some();
    let mut imports = Vec::new();
    scope.uses.collect_candidates(&target, exists, &mut imports);
    collect_visible_use_candidates(
        &target,
        namespace,
        SourceLoc::default(),
        state,
        exists,
        &mut imports,
    );
    if !imports.is_empty() {
        imports.extend(candidates.iter().find(|name| exists(name)).cloned());
        imports.sort();
        imports.dedup();
        if imports.len() != 1 {
            return None;
        }
        return lookup(&imports[0]).map(PrimitiveType::Enum);
    }
    candidates
        .iter()
        .find_map(|name| lookup(name))
        .map(PrimitiveType::Enum)
}

#[allow(clippy::too_many_arguments)]
fn bind_name(
    name: &str,
    loc: SourceLoc,
    namespace: &str,
    state: &NamespaceFlattenState,
    scope: &TemplateConstScope,
    check: &mut ConstCheck,
    captured: &mut SemanticConstArtifacts,
    errors: &mut Vec<Diagnostic>,
) {
    // Namespace parameters already have lexical bindings and shadow imports.
    if !name.contains("::") && scope.members.contains(name) {
        return;
    }
    let target = template_reference_target(name, &scope.aliases);
    let mut candidates = namespace_ref_candidates(&target, namespace, state);
    let mut imports = Vec::new();
    let exists = |name: &str| metadata_contains(name, check, state, scope);
    scope.uses.collect_candidates(&target, exists, &mut imports);
    collect_visible_use_candidates(&target, namespace, loc.clone(), state, exists, &mut imports);
    if !imports.is_empty() {
        let local = candidates
            .iter()
            .find(|candidate| exists(candidate))
            .cloned();
        imports.extend(local);
        imports.sort();
        imports.dedup();
        if imports.len() > 1 {
            errors.push(Diagnostic::semantic_span(format!("ambiguous unqualified symbol '{name}' from explicit use declarations; qualify the reference as one of: {}", imports.join(", ")), loc));
            return;
        }
        candidates = imports;
    }
    for candidate in candidates {
        if state.artifacts.contains(&candidate) {
            captured.import(&candidate, &state.artifacts);
            check.import_artifact(&candidate, &state.artifacts);
        }
        if check.alias(name, &candidate) {
            return;
        }
        if let Some(metadata) = template_const_metadata(&candidate, state) {
            check.import(name, metadata, &candidate);
            return;
        }
    }
}

fn metadata_contains(
    name: &str,
    check: &ConstCheck,
    state: &NamespaceFlattenState,
    scope: &TemplateConstScope,
) -> bool {
    // Source declarations own canonical metadata; alternate spellings in the
    // checker are lookup bindings and never become additional candidates.
    state.artifacts.contains(name)
        || (check.contains(name) && scope.members.contains(name))
        || template_const_metadata(name, state).is_some()
}

pub(super) fn template_const_metadata<'a>(
    name: &str,
    state: &'a NamespaceFlattenState,
) -> Option<&'a ConstCheck> {
    namespace_candidates(name).into_iter().find_map(|prefix| {
        state
            .templates
            .get(&prefix)
            .map(|template| &template.const_metadata)
            .filter(|metadata| metadata.contains(name))
    })
}
