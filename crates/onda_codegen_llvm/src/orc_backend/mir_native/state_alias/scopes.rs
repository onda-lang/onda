//! Cover disjoint origin pairs with shared scope domains. A balanced group of
//! distinct objects needs O(n log n) operands rather than n full complements.
//! Overlapping unions are refined until every original disjointness fact is
//! represented; no analysis budget or coarse alias fallback discards facts.
use std::collections::BTreeSet;
use std::ops::Range;

use super::Origins;
use onda_mir::StateId;

#[derive(Default)]
pub(super) struct Lists {
    pub alias: Vec<usize>,
    pub noalias: Vec<usize>,
}

pub(super) struct Plan {
    pub domains: usize,
    pub lists: Vec<Lists>,
}

struct Group {
    members: Range<usize>,
    states: BTreeSet<StateId>,
    common: BTreeSet<StateId>,
    only_states: bool,
    has_only_states: bool,
    children: Option<[usize; 2]>,
}

impl Group {
    fn build(origins: &[Origins], members: Range<usize>, groups: &mut Vec<Self>) -> usize {
        let group = if members.len() == 1 {
            let origin = &origins[members.start];
            let states: BTreeSet<_> = origin.states.iter().copied().collect();
            Self {
                members,
                common: states.clone(),
                states,
                only_states: origin.only_states,
                has_only_states: origin.only_states,
                children: None,
            }
        } else {
            let middle = members.start + members.len() / 2;
            let left = Self::build(origins, members.start..middle, groups);
            let right = Self::build(origins, middle..members.end, groups);
            let (a, b) = (&groups[left], &groups[right]);
            Self {
                members,
                states: a.states.union(&b.states).copied().collect(),
                common: a.common.intersection(&b.common).copied().collect(),
                only_states: a.only_states && b.only_states,
                has_only_states: a.has_only_states || b.has_only_states,
                children: Some([left, right]),
            }
        };
        let index = groups.len();
        groups.push(group);
        index
    }
}

impl Plan {
    pub fn new(origins: &[Origins]) -> Self {
        let mut plan = Self {
            domains: 0,
            lists: (0..origins.len()).map(|_| Lists::default()).collect(),
        };
        if !origins.is_empty() {
            let mut groups = Vec::new();
            let root = Group::build(origins, 0..origins.len(), &mut groups);
            plan.within(&groups, root);
        }
        plan
    }

    fn within(&mut self, groups: &[Group], index: usize) {
        if let Some([left, right]) = groups[index].children {
            self.within(groups, left);
            self.within(groups, right);
            self.between(groups, left, right);
        }
    }

    fn between(&mut self, groups: &[Group], left: usize, right: usize) {
        let (a, b) = (&groups[left], &groups[right]);
        if (a.only_states || b.only_states) && a.states.is_disjoint(&b.states) {
            // One domain covers every cross-group pair. Accesses outside these
            // two groups carry neither scope, so they gain no alias promises.
            let scope = 2 * self.domains;
            self.domains += 1;
            for (members, alias, noalias) in [
                (a.members.clone(), scope, scope + 1),
                (b.members.clone(), scope + 1, scope),
            ] {
                for index in members {
                    self.lists[index].alias.push(alias);
                    self.lists[index].noalias.push(noalias);
                }
            }
            return;
        }
        if !a.common.is_disjoint(&b.common) || (!a.has_only_states && !b.has_only_states) {
            // All pairs share a state, or can share non-state storage.
            return;
        }
        if a.members.len() >= b.members.len() {
            if let Some(children) = a.children {
                for child in children {
                    self.between(groups, child, right);
                }
            }
        } else if let Some(children) = b.children {
            for child in children {
                self.between(groups, left, child);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origins(states: impl IntoIterator<Item = usize>, only_states: bool) -> Origins {
        Origins {
            states: states
                .into_iter()
                .map(|state| StateId::new(state as u32))
                .collect(),
            only_states,
        }
    }

    fn separates(plan: &Plan, origins: &[Origins], a: usize, b: usize) -> bool {
        [(a, b), (b, a)].into_iter().any(|(alias, noalias)| {
            origins[alias].only_states
                && plan.lists[alias].alias.iter().any(|scope| {
                    // LLVM requires a nonempty alias set in one domain to
                    // be a subset of the other access's noalias set there.
                    plan.lists[alias]
                        .alias
                        .iter()
                        .filter(|candidate| *candidate / 2 == scope / 2)
                        .all(|candidate| plan.lists[noalias].noalias.contains(candidate))
                })
        })
    }

    #[test]
    fn preserves_every_disjoint_union_without_separating_overlapping_origins() {
        let mut sets = Vec::new();
        for bits in 0..32 {
            for only_states in [false, true] {
                if bits != 0 || !only_states {
                    sets.push(origins(
                        (0..5).filter(|bit| bits & (1 << bit) != 0),
                        only_states,
                    ));
                }
            }
        }
        sets.sort();
        let plan = Plan::new(&sets);
        for a in 0..sets.len() {
            for b in 0..sets.len() {
                let expected = (sets[a].only_states || sets[b].only_states)
                    && sets[a]
                        .states
                        .iter()
                        .all(|state| !sets[b].states.contains(state));
                assert_eq!(
                    separates(&plan, &sets, a, b),
                    expected,
                    "{:?} / {:?}",
                    sets[a],
                    sets[b]
                );
            }
        }
    }

    #[test]
    fn separate_objects_and_small_unions_share_compact_scope_domains() {
        for count in [128_usize, 256, 512, 1024] {
            for width in [1, 2] {
                let sets: Vec<_> = (0..count)
                    .map(|state| origins(state..state + width, true))
                    .collect();
                let plan = Plan::new(&sets);
                let operands = 6 * plan.domains
                    + plan
                        .lists
                        .iter()
                        .map(|list| list.alias.len() + list.noalias.len())
                        .sum::<usize>();
                assert!(
                    operands < 8 * count * count.ilog2() as usize,
                    "{count}, {width}: {operands}"
                );
            }
        }
    }
}
