use super::*;

pub(super) fn namespace_parent(ns: &str) -> Option<&str> {
    ns.rsplit_once("::").map(|(parent, _)| parent)
}

pub(super) fn namespace_candidates(current_ns: &str) -> Vec<String> {
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

pub(super) fn namespace_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}::{child}")
    }
}

pub(super) fn namespace_of_symbol(name: &str) -> String {
    name.rsplit_once("::")
        .map(|(ns, _)| ns.to_owned())
        .unwrap_or_default()
}

pub(super) fn split_namespace_parent_leaf(name: &str) -> (&str, &str) {
    if let Some((parent, leaf)) = name.rsplit_once("::") {
        (parent, leaf)
    } else {
        ("", name)
    }
}

pub(super) fn looks_like_namespace_ref(name: &str) -> bool {
    name.contains("::") && !name.contains('.')
}

pub(super) fn split_named_type_base_and_suffix(name: &str) -> (&str, &str) {
    if !name.ends_with('>') {
        return (name, "");
    }

    let mut depth = 0usize;
    for (idx, ch) in name.char_indices().rev() {
        match ch {
            '>' => depth += 1,
            '<' => {
                if depth == 0 {
                    return (name, "");
                }
                depth -= 1;
                if depth == 0 {
                    let base = name[..idx].trim_end();
                    let suffix = name[idx..].trim_start();
                    if base.is_empty() {
                        return (name, "");
                    }
                    return (base, suffix);
                }
            }
            _ => {}
        }
    }

    (name, "")
}

pub(super) fn format_call_args_as_type_suffix(args: &[NamespaceCallArg]) -> String {
    let parts = args
        .iter()
        .map(|arg| match &arg.expr {
            Expr::Var { name, .. } => name.clone(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>();
    format!("<{}>", parts.join(", "))
}

pub(super) fn namespace_segments_key(segments: &[NamespaceRefSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join("::")
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

#[derive(Debug, Clone, Default)]
pub(super) struct RewriteNameScope {
    pub(super) names: HashSet<String>,
}

impl RewriteNameScope {
    pub(super) fn from_names(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            names: names.into_iter().collect(),
        }
    }

    pub(super) fn contains_value_name(&self, name: &str) -> bool {
        if name.is_empty() || name.contains("::") || name.contains('.') {
            return false;
        }
        let (base, _) = split_named_type_base_and_suffix(name);
        self.names.contains(base)
    }

    pub(super) fn insert_plain(&mut self, name: impl Into<String>) {
        let name = name.into();
        if !name.is_empty() && !name.contains("::") && !name.contains('.') {
            self.names.insert(name);
        }
    }

    pub(super) fn extend(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            self.insert_plain(name);
        }
    }
}
