//! Shared dependency order for interprocedural analyses. Validated programs
//! are acyclic; the worklist also converges for monotone diagnostic analyses
//! of recursive, unvalidated programs.
use crate::{Program, StatementKind};
use std::collections::VecDeque;

pub(crate) struct CallGraph {
    pub callees: Vec<Vec<usize>>,
    pub callers: Vec<Vec<usize>>,
    pub postorder: Vec<usize>,
}

impl CallGraph {
    pub fn new(program: &Program) -> Self {
        let mut callees = vec![Vec::new(); program.functions.len()];
        let mut callers = callees.clone();
        for (caller, function) in program.functions.iter().enumerate() {
            let mut blocks = vec![&function.body];
            while let Some(block) = blocks.pop() {
                for statement in &block.statements {
                    match &statement.kind {
                        StatementKind::Call { function, .. } => {
                            callees[caller].push(function.index())
                        }
                        StatementKind::If {
                            then_block,
                            else_block,
                            ..
                        } => blocks.extend([then_block, else_block]),
                        StatementKind::Loop { body } => blocks.push(body),
                        _ => {}
                    }
                }
            }
            callees[caller].sort_unstable();
            callees[caller].dedup();
            for &callee in &callees[caller] {
                callers[callee].push(caller);
            }
        }
        let mut postorder = Vec::with_capacity(callees.len());
        let mut seen = vec![false; callees.len()];
        for root in 0..callees.len() {
            let mut pending = vec![(root, false)];
            while let Some((node, finish)) = pending.pop() {
                if finish {
                    postorder.push(node);
                    continue;
                }
                if std::mem::replace(&mut seen[node], true) {
                    continue;
                }
                pending.push((node, true));
                pending.extend(callees[node].iter().rev().map(|&callee| (callee, false)));
            }
        }
        Self {
            callees,
            callers,
            postorder,
        }
    }

    pub fn worklist(&self) -> Worklist {
        Worklist {
            pending: self.postorder.iter().copied().collect(),
            queued: vec![true; self.callees.len()],
        }
    }
}

pub(crate) struct Worklist {
    pending: VecDeque<usize>,
    queued: Vec<bool>,
}

impl Worklist {
    pub fn pop(&mut self) -> Option<usize> {
        let next = self.pending.pop_front()?;
        self.queued[next] = false;
        Some(next)
    }

    pub fn push(&mut self, function: usize) {
        if !std::mem::replace(&mut self.queued[function], true) {
            self.pending.push_back(function);
        }
    }
}
