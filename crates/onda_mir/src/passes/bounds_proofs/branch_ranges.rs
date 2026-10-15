//! Integer facts valid at a particular point in structured control flow.
//!
//! Comparisons describe the current values of their operands, not historical
//! values. Writes invalidate dependent comparisons; indirect writes discard
//! all path facts. Loop headers use widened inductive intervals, never facts
//! justified only by the first iteration.

mod loops;
mod relations;

use std::collections::HashMap;
use std::rc::Rc;

use relations::Relations;

use crate::{
    CompareOp, Function, FunctionRangeAnalysis, IntegerRange, LocalId, PlaceBase, Program, Rvalue,
    ScalarType, ScalarValue, StatementKind, Value,
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Comparison {
    op: CompareOp,
    lhs: Value,
    rhs: Value,
}

#[derive(Clone)]
pub(super) struct BranchRanges {
    baseline: Vec<Option<IntegerRange>>,
    current: Vec<Option<IntegerRange>>,
    comparisons: HashMap<LocalId, Comparison>,
    relations: Relations,
    slice_writes: Rc<[Option<Vec<LocalId>>]>,
}

impl BranchRanges {
    pub(super) fn new(
        program: &Program,
        function: &Function,
        ranges: &FunctionRangeAnalysis,
    ) -> Self {
        let baseline = function
            .locals
            .iter()
            .enumerate()
            .map(|(index, local)| {
                ranges.local(LocalId::new(index as u32)).or_else(|| {
                    match program.types[local.ty.index()] {
                        crate::Type::Scalar(ScalarType::I32) => {
                            IntegerRange::new(ScalarType::I32, i32::MIN.into(), i32::MAX.into())
                        }
                        crate::Type::Scalar(ScalarType::I64) => {
                            IntegerRange::new(ScalarType::I64, i64::MIN, i64::MAX)
                        }
                        _ => None,
                    }
                })
            })
            .collect::<Vec<_>>();
        Self {
            current: baseline.clone(),
            baseline,
            comparisons: HashMap::new(),
            relations: Relations::default(),
            slice_writes: crate::analysis::slice_local_writes(program, function).into(),
        }
    }

    pub(super) fn value(&self, value: Value) -> Option<IntegerRange> {
        match value {
            Value::Constant(ScalarValue::I32(value)) => {
                IntegerRange::new(ScalarType::I32, value.into(), value.into())
            }
            Value::Constant(ScalarValue::I64(value)) => {
                IntegerRange::new(ScalarType::I64, value, value)
            }
            Value::Local(local) => self.current[local.index()],
            _ => None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.current.clone_from(&self.baseline);
        self.comparisons.clear();
        self.relations.clear();
    }

    fn invalidate(&mut self, local: LocalId) {
        self.current[local.index()] = self.baseline[local.index()];
        self.relations.invalidate(local);
        self.comparisons.retain(|result, comparison| {
            *result != local
                && comparison.lhs != Value::Local(local)
                && comparison.rhs != Value::Local(local)
        });
    }

    pub(super) fn observe(&mut self, statement: &StatementKind) {
        match statement {
            StatementKind::Assign { destination, value } => {
                let PlaceBase::Local(local) = destination.base else {
                    return;
                };
                let assigned_range = match value {
                    Rvalue::Use(value) | Rvalue::Cast { value, .. } => self.value(*value),
                    Rvalue::Load(crate::Place {
                        base: PlaceBase::Local(source),
                        projections,
                    }) if projections.is_empty() => self.current[source.index()],
                    Rvalue::Binary { op, lhs, rhs } => self
                        .value(*lhs)
                        .zip(self.value(*rhs))
                        .and_then(|(lhs, rhs)| crate::analysis::range_of_binary(*op, lhs, rhs)),
                    _ => None,
                };
                let comparison = match value {
                    Rvalue::Compare { op, lhs, rhs }
                        if self.value(*lhs).is_some() && self.value(*rhs).is_some() =>
                    {
                        Some(Comparison {
                            op: *op,
                            lhs: *lhs,
                            rhs: *rhs,
                        })
                    }
                    Rvalue::Use(Value::Local(source)) => self.comparisons.get(source).copied(),
                    _ => None,
                };
                let preserves_integer_value = assigned_range
                    .zip(self.baseline[local.index()])
                    .is_some_and(|(source, target)| {
                        target.contains(source.min()) && target.contains(source.max())
                    });
                let copied_from = match value {
                    Rvalue::Use(Value::Local(source)) => Some(*source),
                    Rvalue::Load(crate::Place {
                        base: PlaceBase::Local(source),
                        projections,
                    }) if projections.is_empty() => Some(*source),
                    Rvalue::Cast {
                        value: Value::Local(source),
                        ..
                    } if preserves_integer_value => Some(*source),
                    _ => None,
                };
                let copied_relations = copied_from
                    .filter(|_| self.baseline[local.index()].is_none() || preserves_integer_value)
                    .map(|source| self.relations.copied(source, local));
                self.invalidate(local);
                if destination.projections.is_empty() {
                    if let (Some(source), Some(target)) =
                        (assigned_range, self.baseline[local.index()])
                    {
                        // Propagate only value-preserving integer assignments.
                        // Staying within the destination's whole-function range
                        // also respects any declared normalization invariant.
                        if target.contains(source.min()) && target.contains(source.max()) {
                            self.current[local.index()] =
                                IntegerRange::new(target.scalar(), source.min(), source.max());
                        }
                    }
                    if let Some(copied) = copied_relations {
                        self.relations.extend(copied);
                    }
                    if let Rvalue::SliceLen(Value::Local(slice)) = value {
                        self.relations.length(local, *slice);
                    }
                    if let Some(comparison) = comparison {
                        self.comparisons.insert(local, comparison);
                    }
                }
            }
            StatementKind::Call { .. } => self.reset(),
            StatementKind::SliceStore { slice, .. } => self.invalidate_slice_write(*slice),
            StatementKind::SliceFill { destination, .. } => {
                self.invalidate_slice_write(*destination)
            }
            StatementKind::SliceCopy { copies, .. } => {
                for copy in copies {
                    self.invalidate_slice_write(copy.destination);
                }
            }
            // Structured control flow is handled by the bounds-proof walker.
            _ => {}
        }
    }

    pub(super) fn assume(&mut self, condition: Value, truth: bool) {
        let Value::Local(local) = condition else {
            return;
        };
        let Some(mut comparison) = self.comparisons.get(&local).copied() else {
            return;
        };
        if !truth {
            comparison.op = match comparison.op {
                CompareOp::Equal => CompareOp::NotEqual,
                CompareOp::NotEqual => CompareOp::Equal,
                CompareOp::Less => CompareOp::GreaterEqual,
                CompareOp::LessEqual => CompareOp::Greater,
                CompareOp::Greater => CompareOp::LessEqual,
                CompareOp::GreaterEqual => CompareOp::Less,
            };
        }
        let Comparison {
            mut op,
            mut lhs,
            mut rhs,
        } = comparison;
        if matches!(op, CompareOp::Greater | CompareOp::GreaterEqual) {
            std::mem::swap(&mut lhs, &mut rhs);
            op = if op == CompareOp::Greater {
                CompareOp::Less
            } else {
                CompareOp::LessEqual
            };
        }
        let (Some(left), Some(right)) = (self.value(lhs), self.value(rhs)) else {
            return;
        };
        if left.scalar() != right.scalar() {
            return;
        }
        if op == CompareOp::Less {
            if let (Value::Local(lhs), Value::Local(rhs)) = (lhs, rhs) {
                self.relations.less_than(lhs, rhs);
            }
        }
        match op {
            CompareOp::Less | CompareOp::LessEqual => {
                let strict = i64::from(op == CompareOp::Less);
                if let Some(max) = right.max().checked_sub(strict) {
                    self.narrow(lhs, left.min(), max);
                }
                if let Some(min) = left.min().checked_add(strict) {
                    self.narrow(rhs, min, right.max());
                }
            }
            CompareOp::Equal => {
                self.narrow(lhs, right.min(), right.max());
                self.narrow(rhs, left.min(), left.max());
            }
            CompareOp::NotEqual => {
                self.exclude_endpoint(lhs, right);
                self.exclude_endpoint(rhs, left);
            }
            _ => unreachable!("greater comparisons were reversed"),
        }
    }

    pub(super) fn slice_index_in_bounds(&self, slice: Value, index: Value) -> bool {
        let Value::Local(slice) = slice else {
            return false;
        };
        let Some(range) = self.value(index) else {
            return false;
        };
        range.is_nonnegative() && self.relations.lengths(slice).any(|length| {
            self.current[length.index()].is_some_and(|length| range.max() < length.min())
                || matches!(index, Value::Local(index) if self.relations.is_less_than(index, length))
        })
    }

    fn invalidate_slice_write(&mut self, slice: Value) {
        let writes = Rc::clone(&self.slice_writes);
        let known = match slice {
            Value::Local(slice) => writes.get(slice.index()).and_then(Option::as_ref),
            _ => None,
        };
        if let Some(locals) = known {
            for &local in locals {
                self.invalidate(local);
            }
        } else {
            self.reset();
        }
    }

    fn narrow(&mut self, value: Value, min: i64, max: i64) {
        if let Value::Local(local) = value {
            if let Some(current) = self.current[local.index()] {
                // An empty intersection describes an unreachable branch. It
                // need not be represented to remove checks on reachable paths.
                if let Some(range) = IntegerRange::new(
                    current.scalar(),
                    current.min().max(min),
                    current.max().min(max),
                ) {
                    self.current[local.index()] = Some(range);
                }
            }
        }
    }

    fn exclude_endpoint(&mut self, value: Value, excluded: IntegerRange) {
        if excluded.min() != excluded.max() {
            return;
        }
        let Some(range) = self.value(value) else {
            return;
        };
        if range.min() == excluded.min() {
            if let Some(min) = range.min().checked_add(1) {
                self.narrow(value, min, range.max());
            }
        } else if range.max() == excluded.max() {
            if let Some(max) = range.max().checked_sub(1) {
                self.narrow(value, range.min(), max);
            }
        }
    }

    pub(super) fn join(&mut self, other: &Self) {
        self.relations.join(&other.relations);
        for (left, right) in self.current.iter_mut().zip(&other.current) {
            *left = match (*left, *right) {
                (Some(left), Some(right)) => IntegerRange::new(
                    left.scalar(),
                    left.min().min(right.min()),
                    left.max().max(right.max()),
                ),
                _ => None,
            };
        }
        self.comparisons
            .retain(|local, comparison| other.comparisons.get(local) == Some(comparison));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparisons_preserve_every_feasible_integer_pair_in_both_branches() {
        for scalar in [ScalarType::I32, ScalarType::I64] {
            let (min, max) = if scalar == ScalarType::I32 {
                (i64::from(i32::MIN), i64::from(i32::MAX))
            } else {
                (i64::MIN, i64::MAX)
            };
            let intervals = [(min, min + 1), (-3, 3), (max - 1, max)];
            for (lo, hi) in intervals {
                for (rlo, rhi) in intervals {
                    for op in [
                        CompareOp::Equal,
                        CompareOp::NotEqual,
                        CompareOp::Less,
                        CompareOp::LessEqual,
                        CompareOp::Greater,
                        CompareOp::GreaterEqual,
                    ] {
                        for truth in [false, true] {
                            let ranges = vec![
                                IntegerRange::new(scalar, lo, hi),
                                IntegerRange::new(scalar, rlo, rhi),
                                None,
                            ];
                            let mut facts = BranchRanges {
                                baseline: ranges.clone(),
                                current: ranges,
                                relations: Default::default(),
                                slice_writes: Default::default(),
                                comparisons: HashMap::from([(
                                    LocalId::new(2),
                                    Comparison {
                                        op,
                                        lhs: Value::Local(LocalId::new(0)),
                                        rhs: Value::Local(LocalId::new(1)),
                                    },
                                )]),
                            };
                            facts.assume(Value::Local(LocalId::new(2)), truth);
                            for x in lo..=hi {
                                for y in rlo..=rhi {
                                    let matches = match op {
                                        CompareOp::Equal => x == y,
                                        CompareOp::NotEqual => x != y,
                                        CompareOp::Less => x < y,
                                        CompareOp::LessEqual => x <= y,
                                        CompareOp::Greater => x > y,
                                        CompareOp::GreaterEqual => x >= y,
                                    };
                                    if matches == truth {
                                        assert!(
                                            facts.current[0].unwrap().contains(x),
                                            "{scalar:?} {x} {op:?} {y}, branch {truth}"
                                        );
                                        assert!(facts.current[1].unwrap().contains(y));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
