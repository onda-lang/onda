//! Separate a distinguished first iteration without reordering any operation.
//! Predicate summaries follow value arguments through helpers; they guide
//! profitability, never confer independence or remove checks. The counted-loop
//! proof also folds direct first-index comparisons in the remainder. LLVM
//! decides whether that remainder can be vectorized after ordinary inlining.
use std::collections::{BTreeMap, BTreeSet};

use crate::*;

#[cfg(test)]
mod tests;

// Amortize the additional body over a substantial loop, as for distribution.
const MIN_ITERATIONS: i32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Origin {
    Parameter(usize),
    Induction,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Symbol {
    Index(Origin),
    Test(Origin, i32),
}

type Predicates = BTreeSet<(Origin, i32)>;

struct Fold {
    statement: usize,
    value: bool,
}

type SelectedLoops = BTreeMap<usize, Vec<Fold>>;

struct Scan<'a> {
    function: &'a Function,
    summaries: &'a [Predicates],
    predicates: Predicates,
    initial_index: Option<i32>,
    folds: Vec<Fold>,
    ordinal: usize,
}

impl<'a> Scan<'a> {
    fn new(
        function: &'a Function,
        summaries: &'a [Predicates],
        initial_index: Option<i32>,
    ) -> Self {
        Self {
            function,
            summaries,
            predicates: Predicates::new(),
            initial_index,
            folds: Vec::new(),
            ordinal: 0,
        }
    }

    fn value(locals: &[Option<Symbol>], value: Value) -> Option<Symbol> {
        match value {
            Value::Local(local) => locals[local.index()],
            _ => None,
        }
    }

    fn rvalue(&self, locals: &[Option<Symbol>], value: &Rvalue) -> Option<Symbol> {
        match value {
            Rvalue::Use(value)
            | Rvalue::NormalizeIndex {
                index: value,
                bounds: BoundsMode::Unchecked,
                ..
            } => Self::value(locals, *value),
            Rvalue::Load(Place { base, projections }) if projections.is_empty() => match base {
                PlaceBase::Local(local) => locals[local.index()],
                PlaceBase::Parameter(parameter)
                    if self.function.params[parameter.index()].mode == PassingMode::Value =>
                {
                    Some(Symbol::Index(Origin::Parameter(parameter.index())))
                }
                _ => None,
            },
            Rvalue::Compare {
                op: CompareOp::Equal | CompareOp::NotEqual,
                lhs,
                rhs,
            } => {
                let (value, constant) = match (lhs, rhs) {
                    (value, Value::Constant(ScalarValue::I32(constant)))
                    | (Value::Constant(ScalarValue::I32(constant)), value) => (value, constant),
                    _ => return None,
                };
                let Symbol::Index(origin) = Self::value(locals, *value)? else {
                    return None;
                };
                Some(Symbol::Test(origin, *constant))
            }
            _ => None,
        }
    }

    fn statements(&mut self, statements: &[Statement], locals: &mut [Option<Symbol>]) {
        for statement in statements {
            let ordinal = self.ordinal;
            self.ordinal += 1;
            match &statement.kind {
                StatementKind::Assign { destination, value } => {
                    let symbol = self.rvalue(locals, value);
                    if let Some(Symbol::Test(Origin::Induction, constant)) = symbol {
                        if self.initial_index == Some(constant) {
                            // The counted-loop proof, not a callee's profitability
                            // summary, establishes that this value cannot recur.
                            if let Rvalue::Compare { op, .. } = value {
                                self.folds.push(Fold {
                                    statement: ordinal,
                                    value: *op == CompareOp::NotEqual,
                                });
                            }
                        }
                    }
                    if let PlaceBase::Local(local) = destination.base {
                        locals[local.index()] = if destination.projections.is_empty() {
                            symbol
                        } else {
                            None
                        };
                    }
                }
                StatementKind::Call {
                    function,
                    args,
                    results,
                } => {
                    for &(origin, constant) in &self.summaries[function.index()] {
                        let Origin::Parameter(parameter) = origin else {
                            continue;
                        };
                        if let CallArgument::Value(value) = &args[parameter] {
                            if let Some(Symbol::Index(origin)) = Self::value(locals, *value) {
                                self.predicates.insert((origin, constant));
                            }
                        }
                    }
                    for result in results {
                        locals[result.index()] = None;
                    }
                    // References may overlap and mutate their source locals.
                    for argument in args {
                        if let CallArgument::Place(Place {
                            base: PlaceBase::Local(local),
                            ..
                        }) = argument
                        {
                            locals[local.index()] = None;
                        }
                    }
                }
                StatementKind::If {
                    condition,
                    then_block,
                    else_block,
                } => {
                    if let Some(Symbol::Test(origin, constant)) = Self::value(locals, *condition) {
                        self.predicates.insert((origin, constant));
                    }
                    let mut other = locals.to_vec();
                    self.statements(&then_block.statements, locals);
                    self.statements(&else_block.statements, &mut other);
                    for (a, b) in locals.iter_mut().zip(other) {
                        if *a != b {
                            *a = None;
                        }
                    }
                }
                // No loop-carried symbolic assumptions, including helper loops.
                StatementKind::Loop { .. } => locals.fill(None),
                _ => {}
            }
        }
    }
}

pub(super) struct Plan(Vec<SelectedLoops>);

impl Plan {
    pub fn new(program: &Program) -> Self {
        let mut summaries = vec![Predicates::new(); program.functions.len()];
        for index in crate::call_graph::CallGraph::new(program).postorder {
            let function = &program.functions[index];
            if function.attributes.inline == InlineHint::Never {
                continue;
            }
            let mut scan = Scan::new(function, &summaries, None);
            scan.statements(
                &function.body.statements,
                &mut vec![None; function.locals.len()],
            );
            summaries[index] = scan.predicates;
        }
        Self(
            program
                .functions
                .iter()
                .map(|function| {
                    let mut selected = SelectedLoops::new();
                    collect(function, &function.body, &summaries, &mut 0, &mut selected);
                    selected
                })
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(BTreeMap::is_empty)
    }

    pub fn apply(&self, program: &mut Program, stats: &mut super::PassStats) {
        for (function, selected) in program.functions.iter_mut().zip(&self.0) {
            if !selected.is_empty() {
                peel(&mut function.body, selected, &mut 0, stats);
            }
        }
    }
}

fn plan_loop(
    function: &Function,
    previous: Option<&Statement>,
    body: &Block,
    summaries: &[Predicates],
) -> Option<Vec<Fold>> {
    let counted = crate::loops::nonempty_loop(body)?;
    let Some(Statement {
        kind:
            StatementKind::Assign {
                destination:
                    Place {
                        base: PlaceBase::Local(index),
                        projections,
                    },
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(start))),
            },
        ..
    }) = previous
    else {
        return None;
    };
    let Value::Constant(ScalarValue::I32(limit)) = counted.limit else {
        return None;
    };
    if *index != counted.index
        || !projections.is_empty()
        || i64::from(limit) - i64::from(*start) < i64::from(MIN_ITERATIONS)
        || !unchanged_induction(counted.body, *index)
    {
        return None;
    }
    let mut locals = vec![None; function.locals.len()];
    locals[index.index()] = Some(Symbol::Index(Origin::Induction));
    let mut scan = Scan::new(function, summaries, Some(*start));
    scan.statements(counted.body, &mut locals);
    scan.predicates
        .contains(&(Origin::Induction, *start))
        .then_some(scan.folds)
}

fn unchanged_induction(statements: &[Statement], index: LocalId) -> bool {
    statements.iter().all(|statement| {
        match &statement.kind {
            StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Return { .. }
            | StatementKind::Loop { .. } => false,
            StatementKind::Assign { destination, .. } => {
                destination.base != PlaceBase::Local(index)
            }
            StatementKind::Call { args, results, .. } => {
                !results.contains(&index)
                    && !args.iter().any(|arg| {
                        matches!(arg, CallArgument::Place(place) if place.base == PlaceBase::Local(index))
                    })
            }
            StatementKind::If { then_block, else_block, .. } => {
                unchanged_induction(&then_block.statements, index)
                    && unchanged_induction(&else_block.statements, index)
            }
            _ => true,
        }
    })
}

fn collect(
    function: &Function,
    block: &Block,
    summaries: &[Predicates],
    ordinal: &mut usize,
    selected: &mut SelectedLoops,
) {
    for (index, statement) in block.statements.iter().enumerate() {
        match &statement.kind {
            StatementKind::Loop { body } => {
                let id = *ordinal;
                *ordinal += 1;
                if let Some(folds) = plan_loop(
                    function,
                    index.checked_sub(1).map(|i| &block.statements[i]),
                    body,
                    summaries,
                ) {
                    selected.insert(id, folds);
                }
                collect(function, body, summaries, ordinal, selected);
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect(function, then_block, summaries, ordinal, selected);
                collect(function, else_block, summaries, ordinal, selected);
            }
            _ => {}
        }
    }
}

fn peel(
    block: &mut Block,
    selected: &SelectedLoops,
    ordinal: &mut usize,
    stats: &mut super::PassStats,
) {
    let mut output = Vec::with_capacity(block.statements.len());
    for mut statement in std::mem::take(&mut block.statements) {
        match &mut statement.kind {
            StatementKind::Loop { body } => {
                let id = *ordinal;
                *ordinal += 1;
                peel(body, selected, ordinal, stats);
                if let Some(folds) = selected.get(&id) {
                    // The final-index test is false on the first iteration.
                    // Keep its assignment and exact increment, so all locals
                    // have the same values when entering the remaining loop.
                    let (last, prefix) = body.statements.split_last().expect("nonempty loop");
                    let StatementKind::If { else_block, .. } = &last.kind else {
                        unreachable!()
                    };
                    output.extend_from_slice(prefix);
                    output.extend_from_slice(&else_block.statements);
                    // Copy first, then fold only within the remaining loop.
                    fold_remainder(body, &mut folds.iter().peekable(), &mut 0, stats);
                    stats.peeled_loops += 1;
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                peel(then_block, selected, ordinal, stats);
                peel(else_block, selected, ordinal, stats);
            }
            _ => {}
        }
        output.push(statement);
    }
    block.statements = output;
}

// Fold positions are in statement preorder, including both sides of branches.
// The scan starts with only the proven induction value: loop-carried locals,
// overwritten copies, and copies passed by reference supply no assumptions.
fn fold_remainder(
    block: &mut Block,
    folds: &mut std::iter::Peekable<std::slice::Iter<'_, Fold>>,
    ordinal: &mut usize,
    stats: &mut super::PassStats,
) {
    for statement in &mut block.statements {
        let id = *ordinal;
        *ordinal += 1;
        if folds.peek().is_some_and(|fold| fold.statement == id) {
            let fold = folds.next().unwrap();
            let StatementKind::Assign { value, .. } = &mut statement.kind else {
                unreachable!()
            };
            *value = Rvalue::Use(Value::Constant(ScalarValue::Bool(fold.value)));
            stats.folded_rvalues += 1;
        }
        if let StatementKind::If {
            then_block,
            else_block,
            ..
        } = &mut statement.kind
        {
            fold_remainder(then_block, folds, ordinal, stats);
            fold_remainder(else_block, folds, ordinal, stats);
        }
    }
}
