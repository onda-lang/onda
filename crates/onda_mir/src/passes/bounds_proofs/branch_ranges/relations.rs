//! Relations between current local values. These complement intervals without
//! assuming descriptor lengths are constant or solving arbitrary inequalities.

use std::collections::HashSet;

use crate::LocalId;

#[derive(Clone, Default)]
pub(super) struct Relations {
    lengths: HashSet<(LocalId, LocalId)>,
    less_than: HashSet<(LocalId, LocalId)>,
}

impl Relations {
    pub(super) fn clear(&mut self) {
        self.lengths.clear();
        self.less_than.clear();
    }

    pub(super) fn invalidate(&mut self, local: LocalId) {
        self.lengths.retain(|&(a, b)| a != local && b != local);
        self.less_than.retain(|&(a, b)| a != local && b != local);
    }

    pub(super) fn length(&mut self, length: LocalId, slice: LocalId) {
        self.lengths.insert((length, slice));
    }

    pub(super) fn less_than(&mut self, lhs: LocalId, rhs: LocalId) {
        self.less_than.insert((lhs, rhs));
    }

    pub(super) fn lengths(&self, slice: LocalId) -> impl Iterator<Item = LocalId> + '_ {
        self.lengths
            .iter()
            .filter_map(move |&(length, owner)| (owner == slice).then_some(length))
    }

    pub(super) fn is_less_than(&self, lhs: LocalId, rhs: LocalId) -> bool {
        self.less_than.contains(&(lhs, rhs))
    }

    pub(super) fn copied(&self, source: LocalId, destination: LocalId) -> Self {
        let copy = |pairs: &HashSet<(LocalId, LocalId)>| {
            pairs
                .iter()
                .filter_map(|&(a, b)| {
                    // A copy cannot establish facts involving the old destination.
                    if source != destination && (a == destination || b == destination) {
                        return None;
                    }
                    (a == source || b == source).then_some((
                        if a == source { destination } else { a },
                        if b == source { destination } else { b },
                    ))
                })
                .collect()
        };
        Self {
            lengths: copy(&self.lengths),
            less_than: copy(&self.less_than),
        }
    }

    pub(super) fn extend(&mut self, other: Self) {
        self.lengths.extend(other.lengths);
        self.less_than.extend(other.less_than);
    }

    pub(super) fn join(&mut self, other: &Self) {
        self.lengths.retain(|fact| other.lengths.contains(fact));
        self.less_than.retain(|fact| other.less_than.contains(fact));
    }

    /// Fewer constraints describe a larger set of possible states.
    pub(super) fn covers(&self, other: &Self) -> bool {
        self.lengths.is_subset(&other.lengths) && self.less_than.is_subset(&other.less_than)
    }
}
