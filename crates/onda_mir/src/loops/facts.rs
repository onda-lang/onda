//! Function-wide loop facts, collected once while the MIR remains immutable.
use std::collections::HashMap;

use crate::*;

#[derive(Default)]
pub(super) struct LocalFacts {
    writes: usize,
    resets: usize,
    steps: usize,
    frame_limits: usize,
    call_result: bool,
    pub escaped: bool,
    pub induction_escaped: bool,
}

impl LocalFacts {
    pub fn unit_induction(&self) -> bool {
        self.writes == 2 && self.resets == 1 && self.steps == 1 && !self.call_result
    }

    pub fn process_limit(&self) -> bool {
        self.writes == 1 && self.frame_limits == 1 && !self.call_result && !self.escaped
    }
}

pub(super) struct FunctionFacts {
    pub locals: Vec<LocalFacts>,
    resets: HashMap<*const Block, i32>,
    #[cfg(test)]
    pub visits: usize,
}

impl FunctionFacts {
    pub fn new(function: &Function) -> Self {
        let mut facts = Self {
            locals: (0..function.locals.len())
                .map(|_| LocalFacts::default())
                .collect(),
            resets: HashMap::new(),
            #[cfg(test)]
            visits: 0,
        };
        let mut pending = vec![&function.body];
        while let Some(block) = pending.pop() {
            for (position, statement) in block.statements.iter().enumerate() {
                #[cfg(test)]
                {
                    facts.visits += 1;
                }
                if let Some((id, value)) = super::assigned_local(statement) {
                    let local = &mut facts.locals[id.index()];
                    local.writes += 1;
                    match value {
                        Rvalue::Use(Value::Constant(ScalarValue::I32(0))) => local.resets += 1,
                        Rvalue::Binary {
                            op: BinaryOp::Add,
                            lhs: Value::Local(before),
                            rhs: Value::Constant(ScalarValue::I32(1)),
                        } if id == *before => local.steps += 1,
                        Rvalue::Load(Place {
                            base: PlaceBase::Parameter(parameter),
                            projections,
                        }) if parameter.index() == PROCESS_FRAMES_PARAM_INDEX
                            && projections.is_empty() =>
                        {
                            local.frame_limits += 1
                        }
                        _ => {}
                    }
                }
                match &statement.kind {
                    StatementKind::Assign {
                        value:
                            Rvalue::MakeSlice {
                                source:
                                    SliceSource::Place(Place {
                                        base: PlaceBase::Local(local),
                                        ..
                                    }),
                                ..
                            },
                        ..
                    } => {
                        facts.locals[local.index()].induction_escaped = true;
                        if super::assigned_local(statement).is_some() {
                            facts.locals[local.index()].escaped = true;
                        }
                    }
                    StatementKind::Call { results, args, .. } => {
                        for local in results {
                            facts.locals[local.index()].call_result = true;
                        }
                        for argument in args {
                            if let CallArgument::Place(Place {
                                base: PlaceBase::Local(local),
                                ..
                            }) = argument
                            {
                                facts.locals[local.index()].induction_escaped = true;
                            }
                        }
                        facts.escape_arguments(args);
                    }
                    StatementKind::PublishDelegate { args, .. } => facts.escape_arguments(args),
                    StatementKind::Loop { body } => {
                        if let Some(start) = super::counted_loop(body)
                            .and_then(|counted| reset_before(block, position, counted.index))
                        {
                            facts.resets.insert(body, start);
                        }
                        pending.push(body);
                    }
                    StatementKind::If {
                        then_block,
                        else_block,
                        ..
                    } => pending.extend([then_block, else_block]),
                    _ => {}
                }
            }
        }
        facts
    }

    fn escape_arguments(&mut self, args: &[CallArgument]) {
        for argument in args {
            if let CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } =
                argument
            {
                if let PlaceBase::Local(local) = place.base {
                    self.locals[local.index()].escaped = true;
                }
            }
        }
    }

    pub fn preceding_reset(&self, body: &Block) -> Option<i32> {
        self.resets.get(&std::ptr::from_ref(body)).copied()
    }
}

fn reset_before(block: &Block, position: usize, index: LocalId) -> Option<i32> {
    let previous = block.statements.get(position.checked_sub(1)?)?;
    let (local, Rvalue::Use(Value::Constant(ScalarValue::I32(start)))) =
        super::assigned_local(previous)?
    else {
        return None;
    };
    (local == index).then_some(*start)
}
