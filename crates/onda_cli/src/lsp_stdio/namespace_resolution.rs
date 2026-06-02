use onda_frontend::NamespaceRefSegment;

#[derive(Debug, Clone)]
pub(super) struct UseInfo {
    pub(super) namespace: String,
    pub(super) target: String,
    pub(super) alias: Option<String>,
    pub(super) public: bool,
    pub(super) file_key: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct NamespaceAliasInfo {
    pub(super) namespace: String,
    pub(super) target: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum AliasTargetPolicy {
    Always,
    Accepted,
}

pub(super) trait NamespaceResolutionContext {
    fn namespace_alias(&self, full_name: &str) -> Option<&NamespaceAliasInfo>;
    fn has_namespace(&self, namespace: &str) -> bool;
    fn has_symbol(&self, full_name: &str) -> bool;
    fn uses(&self) -> &[UseInfo];
    fn use_visible(&self, use_info: &UseInfo) -> bool;
}

pub(super) fn qualified_path_candidates<C, M, L>(
    ctx: &C,
    path: &str,
    current_namespace: &str,
    member_accepts: M,
    leaf_accepts: L,
    alias_target_policy: AliasTargetPolicy,
) -> Vec<String>
where
    C: NamespaceResolutionContext,
    M: Fn(&C, &str) -> bool,
    L: Fn(&C, &str) -> bool,
{
    let clean = strip_type_args_from_path(path);
    if clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = path_candidates_with_namespace_aliases(ctx, &clean, current_namespace);
    if let Some((head, tail)) = clean.split_once("::") {
        for use_info in visible_uses_in_namespace(ctx, current_namespace) {
            let target = resolve_use_target(ctx, use_info);
            match use_info.alias.as_deref() {
                Some(alias) if alias == head => {
                    if ctx.has_namespace(&target) {
                        for candidate in
                            namespace_alias_path_candidates(ctx, &namespace_join(&target, tail))
                        {
                            push_unique_candidate(&mut candidates, candidate);
                        }
                    }
                }
                None => {
                    let member = namespace_join(&target, &clean);
                    for candidate in namespace_alias_path_candidates(ctx, &member) {
                        if member_accepts(ctx, &candidate) {
                            push_unique_candidate(&mut candidates, candidate);
                        }
                    }
                }
                _ => {}
            }
        }
    } else {
        for use_info in visible_uses_in_namespace(ctx, current_namespace) {
            let target = resolve_use_target(ctx, use_info);
            match use_info.alias.as_deref() {
                Some(alias) if alias == clean => {
                    let accepted = match alias_target_policy {
                        AliasTargetPolicy::Always => true,
                        AliasTargetPolicy::Accepted => leaf_accepts(ctx, &target),
                    };
                    if accepted {
                        push_unique_candidate(&mut candidates, target);
                    }
                }
                None => {
                    if ctx.has_namespace(&target) {
                        let member = namespace_join(&target, &clean);
                        for candidate in namespace_alias_path_candidates(ctx, &member) {
                            if member_accepts(ctx, &candidate) {
                                push_unique_candidate(&mut candidates, candidate);
                            }
                        }
                    }
                    if target.rsplit("::").next() == Some(clean.as_str())
                        && leaf_accepts(ctx, &target)
                    {
                        push_unique_candidate(&mut candidates, target);
                    }
                }
                _ => {}
            }
        }
    }
    candidates
}

pub(super) fn path_candidates_with_namespace_aliases<C>(
    ctx: &C,
    path: &str,
    current_namespace: &str,
) -> Vec<String>
where
    C: NamespaceResolutionContext,
{
    let clean = strip_type_args_from_path(path);
    if clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::<String>::new();
    if let Some((head, tail)) = clean.split_once("::") {
        for candidate_namespace in namespace_candidates(current_namespace) {
            let head_candidate = namespace_join(&candidate_namespace, head);
            if let Some(alias) = ctx.namespace_alias(&head_candidate) {
                let alias_target = resolve_namespace_alias_target(ctx, alias);
                push_unique_candidate(&mut candidates, namespace_join(&alias_target, tail));
            }
            push_path_candidate_with_alias_expansion(
                ctx,
                &mut candidates,
                namespace_join(&head_candidate, tail),
            );
        }
    } else {
        for candidate_namespace in namespace_candidates(current_namespace) {
            let candidate = namespace_join(&candidate_namespace, &clean);
            if let Some(alias) = ctx.namespace_alias(&candidate) {
                push_unique_candidate(&mut candidates, resolve_namespace_alias_target(ctx, alias));
            }
            push_path_candidate_with_alias_expansion(ctx, &mut candidates, candidate);
        }
    }

    push_unique_candidate(&mut candidates, clean);
    candidates
}

pub(super) fn namespace_alias_path_candidates<C>(ctx: &C, path: &str) -> Vec<String>
where
    C: NamespaceResolutionContext,
{
    let mut candidates = Vec::new();
    push_path_candidate_with_alias_expansion(ctx, &mut candidates, path.to_owned());
    candidates
}

pub(super) fn expand_namespace_alias_path<C>(ctx: &C, path: &str) -> Option<String>
where
    C: NamespaceResolutionContext,
{
    let mut current = strip_type_args_from_path(path);
    let mut changed = false;
    for _ in 0..32 {
        let Some((target, suffix)) = longest_namespace_alias_prefix(ctx, &current) else {
            break;
        };
        let expanded = namespace_join(&target, &suffix);
        if expanded == current {
            break;
        }
        current = expanded;
        changed = true;
    }
    changed.then_some(current)
}

pub(super) fn resolve_use_target<C>(ctx: &C, use_info: &UseInfo) -> String
where
    C: NamespaceResolutionContext,
{
    for candidate in
        path_candidates_with_namespace_aliases(ctx, &use_info.target, &use_info.namespace)
    {
        if ctx.has_namespace(&candidate) || ctx.has_symbol(&candidate) {
            return candidate;
        }
    }
    strip_type_args_from_path(&use_info.target)
}

pub(super) fn namespace_segments_key(segments: &[NamespaceRefSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>()
        .join("::")
}

pub(super) fn namespace_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else if child.is_empty() {
        parent.to_owned()
    } else {
        format!("{parent}::{child}")
    }
}

pub(super) fn namespace_parent_of(name: &str) -> String {
    name.rsplit_once("::")
        .map(|(parent, _)| parent.to_owned())
        .unwrap_or_default()
}

pub(super) fn strip_type_args_from_path(path: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in path.trim().chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ' ' | '\t' | '\r' if depth > 0 => {}
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

pub(super) fn visible_uses_in_namespace<'a, C>(
    ctx: &'a C,
    current_namespace: &str,
) -> impl Iterator<Item = &'a UseInfo> + 'a
where
    C: NamespaceResolutionContext,
{
    let namespaces = namespace_candidates(current_namespace);
    ctx.uses().iter().filter(move |use_info| {
        ctx.use_visible(use_info)
            && namespaces
                .iter()
                .any(|namespace| namespace == &use_info.namespace)
    })
}

fn push_path_candidate_with_alias_expansion<C>(
    ctx: &C,
    candidates: &mut Vec<String>,
    candidate: String,
) where
    C: NamespaceResolutionContext,
{
    push_unique_candidate(candidates, candidate.clone());
    if let Some(expanded) = expand_namespace_alias_path(ctx, &candidate) {
        push_unique_candidate(candidates, expanded);
    }
}

fn longest_namespace_alias_prefix<C>(ctx: &C, path: &str) -> Option<(String, String)>
where
    C: NamespaceResolutionContext,
{
    let mut prefix = path;
    loop {
        if let Some(alias) = ctx.namespace_alias(prefix) {
            let suffix = path
                .strip_prefix(prefix)
                .unwrap_or_default()
                .strip_prefix("::")
                .unwrap_or_default();
            return Some((
                resolve_namespace_alias_target(ctx, alias),
                suffix.to_owned(),
            ));
        }
        let Some((parent, _)) = prefix.rsplit_once("::") else {
            return None;
        };
        prefix = parent;
    }
}

fn resolve_namespace_alias_target<C>(ctx: &C, alias: &NamespaceAliasInfo) -> String
where
    C: NamespaceResolutionContext,
{
    for candidate in namespace_ref_candidates(&alias.target, &alias.namespace) {
        if ctx.has_namespace(&candidate) || ctx.has_symbol(&candidate) {
            return candidate;
        }
    }
    strip_type_args_from_path(&alias.target)
}

fn namespace_ref_candidates(path: &str, current_namespace: &str) -> Vec<String> {
    let clean = strip_type_args_from_path(path);
    if clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::<String>::new();
    if let Some((head, tail)) = clean.split_once("::") {
        for candidate_namespace in namespace_candidates(current_namespace) {
            let head_candidate = namespace_join(&candidate_namespace, head);
            push_unique_candidate(&mut candidates, namespace_join(&head_candidate, tail));
        }
    } else {
        for candidate_namespace in namespace_candidates(current_namespace) {
            push_unique_candidate(
                &mut candidates,
                namespace_join(&candidate_namespace, &clean),
            );
        }
    }
    push_unique_candidate(&mut candidates, clean);
    candidates
}

fn namespace_candidates(current_namespace: &str) -> Vec<String> {
    if current_namespace.is_empty() {
        return vec![String::new()];
    }

    let mut out = Vec::<String>::new();
    let mut current = Some(current_namespace);
    while let Some(namespace) = current {
        out.push(namespace.to_owned());
        current = namespace_parent(namespace);
    }
    out.push(String::new());
    out
}

fn namespace_parent(namespace: &str) -> Option<&str> {
    namespace.rsplit_once("::").map(|(parent, _)| parent)
}

fn push_unique_candidate(candidates: &mut Vec<String>, candidate: String) {
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}
