//! Bounded interval iteration for structured loop headers. Widening gives a
//! conservative induction invariant; guards refine it again inside the body.
//! No bounds rewrite is allowed until the backedges satisfy that invariant.

use super::BranchRanges;
use crate::{Block, IntegerRange, PlaceBase, StatementKind};

impl BranchRanges {
    /// Keep only incoming facts independent of all writes the loop may execute.
    /// This covers every exit, including breaks before a later assignment.
    pub(in super::super) fn forget_loop_writes(&mut self, body: &Block) {
        for statement in &body.statements {
            match &statement.kind {
                StatementKind::Assign { destination, .. } => {
                    if let PlaceBase::Local(local) = destination.base {
                        self.invalidate(local);
                    }
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    self.forget_loop_writes(then_block);
                    self.forget_loop_writes(else_block);
                }
                StatementKind::Loop { body } => self.forget_loop_writes(body),
                // observe discards facts for calls and indirect writes.
                statement => self.observe(statement),
            }
        }
    }

    pub(in super::super) fn loop_header(&self, body: &Block) -> Self {
        let mut header = self.clone();
        // A predicate computed before the loop need not describe its operands
        // on later iterations, even if their interval happens to be unchanged.
        header.comparisons.clear();
        for _ in 0..8 {
            let mut end = header.clone();
            let mut backedges = None;
            if transfer(&mut end, body, &mut backedges) {
                merge(&mut backedges, end);
            }
            let mut next = self.clone();
            if let Some(backedges) = backedges {
                next.join(&backedges);
            }
            if header.covers(&next) {
                return header;
            }
            header.widen(&next);
        }
        // Compilation cost is bounded independently of trip count and the
        // length of dependencies between locals. Baseline facts are always safe.
        header.reset();
        header
    }

    fn covers(&self, other: &Self) -> bool {
        self.relations.covers(&other.relations)
            && self
                .current
                .iter()
                .zip(&other.current)
                .all(|(a, b)| match (a, b) {
                    (None, _) => true,
                    (Some(a), Some(b)) => a.min() <= b.min() && a.max() >= b.max(),
                    (Some(_), None) => false,
                })
    }

    fn widen(&mut self, next: &Self) {
        self.relations.join(&next.relations);
        for ((current, next), baseline) in self
            .current
            .iter_mut()
            .zip(&next.current)
            .zip(&self.baseline)
        {
            *current = match (*current, *next, *baseline) {
                (Some(current), Some(next), Some(baseline)) => IntegerRange::new(
                    baseline.scalar(),
                    if next.min() < current.min() {
                        baseline.min()
                    } else {
                        current.min()
                    },
                    if next.max() > current.max() {
                        baseline.max()
                    } else {
                        current.max()
                    },
                ),
                _ => *baseline,
            };
        }
    }
}

fn merge(destination: &mut Option<BranchRanges>, incoming: BranchRanges) {
    if let Some(destination) = destination {
        destination.join(&incoming);
    } else {
        *destination = Some(incoming);
    }
}

/// Returns whether execution can reach the end of this block. Continues join
/// the enclosing loop's backedges; breaks and returns cannot reach its header.
fn transfer(facts: &mut BranchRanges, block: &Block, backedges: &mut Option<BranchRanges>) -> bool {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                let mut yes = facts.clone();
                let mut no = facts.clone();
                yes.assume(*condition, true);
                no.assume(*condition, false);
                let yes_flows = transfer(&mut yes, then_block, backedges);
                let no_flows = transfer(&mut no, else_block, backedges);
                *facts = match (yes_flows, no_flows) {
                    (true, true) => {
                        yes.join(&no);
                        yes
                    }
                    (true, false) => yes,
                    (false, true) => no,
                    (false, false) => return false,
                };
            }
            // Avoid recursively iterating nested loops during the enclosing
            // fixed point. Their writes are covered by whole-function facts.
            // The bounds walker analyzes each nested header independently.
            StatementKind::Loop { body } => facts.forget_loop_writes(body),
            StatementKind::Continue => {
                merge(backedges, facts.clone());
                return false;
            }
            StatementKind::Break | StatementKind::Return { .. } => return false,
            statement => facts.observe(statement),
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BinaryOp, CompareOp, LocalId, Place, Rvalue, ScalarType, ScalarValue, SourceSpan,
        Statement, Value,
    };

    #[test]
    fn iteration_budget_falls_back_to_baseline_for_long_dependencies() {
        let baseline = vec![IntegerRange::new(ScalarType::I32, 0, 3); 20];
        let facts = BranchRanges {
            baseline: baseline.clone(),
            current: vec![IntegerRange::new(ScalarType::I32, 0, 0); 20],
            comparisons: Default::default(),
            relations: Default::default(),
            slice_writes: Default::default(),
        };
        // Reverse copies propagate the expanded interval only one link per
        // backedge. This deliberately exceeds the iteration budget.
        let body = Block {
            statements: (0..20)
                .rev()
                .map(|index| Statement {
                    source: SourceSpan::UNKNOWN,
                    kind: StatementKind::Assign {
                        destination: Place::local(LocalId::new(index)),
                        value: Rvalue::Use(if index == 0 {
                            Value::Constant(ScalarValue::I32(1))
                        } else {
                            Value::Local(LocalId::new(index - 1))
                        }),
                    },
                })
                .collect(),
        };
        assert_eq!(facts.loop_header(&body).current, baseline);
    }

    #[test]
    fn widened_headers_contain_concrete_wrapping_iterations_at_both_integer_widths() {
        for scalar in [ScalarType::I32, ScalarType::I64] {
            let (min, max) = if scalar == ScalarType::I32 {
                (i64::from(i32::MIN), i64::from(i32::MAX))
            } else {
                (i64::MIN, i64::MAX)
            };
            let constant = |v| {
                Value::Constant(if scalar == ScalarType::I32 {
                    ScalarValue::I32(v as i32)
                } else {
                    ScalarValue::I64(v)
                })
            };
            let local = |index| Value::Local(LocalId::new(index));
            let statement = |kind| Statement {
                kind,
                source: SourceSpan::UNKNOWN,
            };
            let assign = |index, value| {
                statement(StatementKind::Assign {
                    destination: Place::local(LocalId::new(index)),
                    value,
                })
            };
            for start in [min, min + 1, -3, 0, 3, max - 1, max] {
                for limit in [min, -4, 0, 4, max] {
                    for step in [min, -3, -1, 1, 3, max] {
                        for op in [CompareOp::Less, CompareOp::GreaterEqual] {
                            let body = Block {
                                statements: vec![
                                    assign(
                                        1,
                                        Rvalue::Compare {
                                            op,
                                            lhs: local(0),
                                            rhs: constant(limit),
                                        },
                                    ),
                                    statement(StatementKind::If {
                                        condition: local(1),
                                        then_block: Block::default(),
                                        else_block: Block {
                                            statements: vec![statement(StatementKind::Break)],
                                        },
                                    }),
                                    assign(
                                        0,
                                        Rvalue::Binary {
                                            op: BinaryOp::Add,
                                            lhs: local(0),
                                            rhs: constant(step),
                                        },
                                    ),
                                ],
                            };
                            let entry = BranchRanges {
                                baseline: vec![IntegerRange::new(scalar, min, max), None],
                                current: vec![IntegerRange::new(scalar, start, start), None],
                                comparisons: Default::default(),
                                relations: Default::default(),
                                slice_writes: Default::default(),
                            };
                            let header = entry.loop_header(&body).value(local(0)).unwrap();
                            let mut value = start;
                            for _ in 0..32 {
                                assert!(header.contains(value), "{scalar:?}: {start}, {op:?} {limit}, step {step}, header {header:?}, value {value}");
                                if !(if op == CompareOp::Less {
                                    value < limit
                                } else {
                                    value >= limit
                                }) {
                                    break;
                                }
                                value = if scalar == ScalarType::I32 {
                                    i64::from((value as i32).wrapping_add(step as i32))
                                } else {
                                    value.wrapping_add(step)
                                };
                            }
                        }
                    }
                }
            }
        }
    }
}
