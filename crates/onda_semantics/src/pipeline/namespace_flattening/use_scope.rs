//! Explicit-use candidates, shared by concrete and metadata-only lookup.
use super::*;

pub(super) fn collect_visible_use_candidates(
    name: &str,
    namespace: &str,
    loc: SourceLoc,
    state: &NamespaceFlattenState,
    exists: impl Fn(&str) -> bool,
    candidates: &mut Vec<String>,
) {
    let file = loc.file().unwrap_or_default();
    for ns in namespace_candidates(namespace) {
        if let Some(scope) = state.public_uses.get(&ns) {
            scope.collect_candidates(name, &exists, candidates);
        }
        if let Some(scope) = state.private_uses.get(&(ns, file.clone())) {
            scope.collect_candidates(name, &exists, candidates);
        }
    }
}

impl UseScope {
    pub(super) fn symbol(&mut self, name: &str, target: String) {
        self.symbols
            .entry(name.to_owned())
            .or_default()
            .push(UseBinding { target });
    }

    pub(super) fn namespace(&mut self, target: String) {
        if !self
            .namespaces
            .iter()
            .any(|binding| binding.target == target)
        {
            self.namespaces.push(NamespaceUseBinding { target });
        }
    }

    pub(super) fn collect_candidates(
        &self,
        name: &str,
        exists: impl Fn(&str) -> bool,
        candidates: &mut Vec<String>,
    ) {
        let (head, tail) = name.split_once("::").unwrap_or((name, ""));
        if let Some(bindings) = self.symbols.get(head) {
            for binding in bindings {
                let target = if tail.is_empty() {
                    binding.target.clone()
                } else {
                    namespace_join(&binding.target, tail)
                };
                if exists(&target) {
                    candidates.push(target);
                }
            }
        }
        for binding in &self.namespaces {
            let target = namespace_join(&binding.target, name);
            if exists(&target) {
                candidates.push(target);
            }
        }
    }
}
