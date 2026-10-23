//! Target-independent legality checks for counted and pointwise loops.
use std::cell::OnceCell;
use std::collections::HashSet;

use crate::*;

mod facts;
mod independent;
#[cfg(test)]
mod tests;

/// Reusable loop queries in one immutable function. Construct this once per
/// function to share structural and descriptor facts across its loop proofs.
pub struct LoopAnalysis<'a> {
    program: &'a Program,
    effects: &'a EffectAnalysis,
    memory: &'a MemoryAnalysis,
    ranges: &'a ProgramRangeAnalysis,
    function: FunctionId,
    facts: facts::FunctionFacts,
    memory_facts: OnceCell<independent::MemoryFacts>,
}

impl<'a> LoopAnalysis<'a> {
    pub fn new(
        program: &'a Program,
        effects: &'a EffectAnalysis,
        memory: &'a MemoryAnalysis,
        ranges: &'a ProgramRangeAnalysis,
        function: FunctionId,
    ) -> Self {
        Self {
            program,
            effects,
            memory,
            ranges,
            function,
            facts: facts::FunctionFacts::new(&program.functions[function.index()]),
            memory_facts: OnceCell::new(),
        }
    }

    /// Proves absence of cross-iteration memory dependences without relaxing
    /// scalar recurrences or floating-point order.
    pub fn independent_memory_loop(&self, body: &Block) -> bool {
        independent::prove(self, body).is_some()
    }

    pub fn pointwise_audio_loop(&self, body: &Block) -> Option<PointwiseAudioLoop> {
        pointwise_audio_loop(self, body)
    }

    /// Whether this local has exactly one zero reset and one unit increment,
    /// with no other writes or reference escapes relevant to a copy-loop hint.
    pub fn nonnegative_induction(&self, index: LocalId) -> bool {
        let local = &self.facts.locals[index.index()];
        local.unit_induction() && !local.induction_escaped
    }
}

pub struct CountedLoop<'a> {
    pub index: LocalId,
    pub limit: Value,
    pub body: &'a [Statement],
}

fn assigned_local(statement: &Statement) -> Option<(LocalId, &Rvalue)> {
    let StatementKind::Assign {
        destination:
            Place {
                base: PlaceBase::Local(local),
                projections,
            },
        value,
    } = &statement.kind
    else {
        return None;
    };
    projections.is_empty().then_some((*local, value))
}

pub fn counted_loop(block: &Block) -> Option<CountedLoop<'_>> {
    guarded_loop(block).or_else(|| nonempty_loop(block))
}

pub(crate) fn guarded_loop(block: &Block) -> Option<CountedLoop<'_>> {
    let statements = &block.statements;
    let (
        condition,
        Rvalue::Compare {
            op: CompareOp::Less,
            lhs: Value::Local(index),
            rhs: limit,
        },
    ) = assigned_local(statements.first()?)?
    else {
        return None;
    };
    let StatementKind::If {
        condition: Value::Local(test),
        then_block,
        else_block,
    } = &statements.get(1)?.kind
    else {
        return None;
    };
    if *test != condition
        || !matches!(
            else_block.statements.as_slice(),
            [Statement {
                kind: StatementKind::Break,
                ..
            }]
        )
    {
        return None;
    }
    let body = if statements.len() == 2 {
        then_block.statements.as_slice()
    } else if then_block.statements.is_empty() {
        &statements[2..]
    } else {
        return None;
    };
    let (updated, last) = assigned_local(body.last()?)?;
    let (step, increment, tail) = match last {
        Rvalue::Use(Value::Local(temporary)) if *temporary != *index => {
            let (step, increment) = assigned_local(body.get(body.len().checked_sub(2)?)?)?;
            if step != *temporary {
                return None;
            }
            (step, increment, 2)
        }
        _ => (updated, last, 1),
    };
    let Rvalue::Binary {
        op: BinaryOp::Add,
        lhs: Value::Local(before),
        rhs: Value::Constant(ScalarValue::I32(1)),
    } = increment
    else {
        return None;
    };
    if updated != *index || *before != *index || (tail == 1 && step != *index) {
        return None;
    }
    Some(CountedLoop {
        index: *index,
        limit: *limit,
        body: &body[..body.len() - tail],
    })
}

/// The nonempty form tests its final index before incrementing. Restrict this
/// view to limits representable as an exclusive i32 bound.
fn nonempty_loop(block: &Block) -> Option<CountedLoop<'_>> {
    let statements = &block.statements;
    let tail = statements.len().checked_sub(2)?;
    let (
        condition,
        Rvalue::Compare {
            op: CompareOp::Equal,
            lhs: Value::Local(index),
            rhs: Value::Constant(ScalarValue::I32(last)),
        },
    ) = assigned_local(&statements[tail])?
    else {
        return None;
    };
    let StatementKind::If {
        condition: Value::Local(test),
        then_block,
        else_block,
    } = &statements[tail + 1].kind
    else {
        return None;
    };
    if *test != condition
        || !matches!(
            then_block.statements.as_slice(),
            [Statement {
                kind: StatementKind::Break,
                ..
            }]
        )
        || else_block.statements.len() != 1
    {
        return None;
    }
    let (
        updated,
        Rvalue::Binary {
            op: BinaryOp::Add,
            lhs: Value::Local(before),
            rhs: Value::Constant(ScalarValue::I32(1)),
        },
    ) = assigned_local(&else_block.statements[0])?
    else {
        return None;
    };
    if updated != *index || *before != *index {
        return None;
    }
    Some(CountedLoop {
        index: *index,
        limit: Value::Constant(ScalarValue::I32(last.checked_add(1)?)),
        body: &statements[..tail],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioPort {
    Input(InputId),
    Output(OutputId),
}

pub struct PointwiseAudioLoop {
    pub scalar: ScalarType,
    pub ports: Vec<AudioPort>,
}

/// Audio accesses use the same frame coordinate and all other reads are
/// invariant. A backend may execute iterations independently when every pair
/// involving a write is either disjoint or exactly equal, preserving operation
/// order within each iteration. Partial overlap still needs a fallback.
fn pointwise_audio_loop(analysis: &LoopAnalysis<'_>, block: &Block) -> Option<PointwiseAudioLoop> {
    let program = analysis.program;
    let effects = analysis.effects;
    let function = &program.functions[analysis.function.index()];
    if function.kind != FunctionKind::Process {
        return None;
    }
    let counted = counted_loop(block)?;
    let Value::Local(limit) = counted.limit else {
        return None;
    };
    let (
        frame,
        Rvalue::ProcessFrame {
            offset: Value::Local(index),
        },
    ) = assigned_local(counted.body.first()?)?
    else {
        return None;
    };
    if *index != counted.index {
        return None;
    }
    let induction = &analysis.facts.locals[counted.index.index()];
    if !induction.unit_induction()
        || induction.escaped
        || !analysis.facts.locals[limit.index()].process_limit()
    {
        return None;
    }
    let operations = &counted.body[1..];
    let mut written = HashSet::new();
    let mut record_write = |local: LocalId| {
        if local == counted.index
            || local == frame
            || local == limit
            || analysis.facts.locals[local.index()].escaped
        {
            return None;
        }
        if !matches!(
            program.types[function.locals[local.index()].ty.index()],
            Type::Scalar(ScalarType::F32 | ScalarType::F64)
        ) {
            return None;
        }
        written.insert(local);
        Some(())
    };
    for statement in operations {
        if let Some((local, _)) = assigned_local(statement) {
            record_write(local)?;
        } else if let StatementKind::Call { results, .. } = &statement.kind {
            for &local in results {
                record_write(local)?;
            }
        } else if !matches!(statement.kind, StatementKind::OutputStore { .. }) {
            return None;
        }
    }
    let mut defined = HashSet::new();
    let mut ports = Vec::new();
    let mut scalar = None;
    let mut add_port = |port, ty| {
        let Type::Scalar(ty @ (ScalarType::F32 | ScalarType::F64)) = program.types[ty] else {
            return false;
        };
        if scalar.is_some_and(|old| old != ty) {
            return false;
        }
        scalar = Some(ty);
        if !ports.contains(&port) {
            ports.push(port);
        }
        true
    };
    for statement in operations {
        let valid = |value: Value| match value {
            Value::Constant(_) => true,
            Value::Local(local) => !written.contains(&local) || defined.contains(&local),
        };
        if let StatementKind::Call {
            function: callee,
            args,
            results,
        } = &statement.kind
        {
            let summary = effects.function(*callee);
            // Helpers may hide arithmetic and control flow, but cannot observe
            // another iteration, fail, or prevent its completion. LLVM can
            // inline these calls after this shared legality proof.
            if !summary.is_memory_free()
                || summary.may_fail
                || summary.may_not_return
                || !program.functions[callee.index()]
                    .params
                    .iter()
                    .all(|parameter| {
                        parameter.mode == PassingMode::Value
                            && matches!(program.types[parameter.ty.index()], Type::Scalar(_))
                    })
                || !args
                    .iter()
                    .all(|argument| matches!(argument, CallArgument::Value(value) if valid(*value)))
            {
                return None;
            }
            defined.extend(results.iter().copied());
            continue;
        }
        if let StatementKind::OutputStore {
            output,
            element: None,
            frame: Value::Local(at),
            value,
            ..
        } = &statement.kind
        {
            if *at != frame
                || !valid(*value)
                || !add_port(
                    AudioPort::Output(*output),
                    program.interface.outputs[output.index()].ty.index(),
                )
            {
                return None;
            }
            continue;
        }
        let (local, value) = assigned_local(statement)?;
        let legal = match value {
            Rvalue::Use(value) => valid(*value),
            Rvalue::Unary { operand, .. } => valid(*operand),
            Rvalue::Binary { lhs, rhs, .. } | Rvalue::Compare { lhs, rhs, .. } => {
                valid(*lhs) && valid(*rhs)
            }
            Rvalue::Cast { value, .. } => valid(*value),
            Rvalue::Intrinsic { intrinsic, args } => {
                !matches!(intrinsic, Intrinsic::RangeClamp | Intrinsic::RangeWrap)
                    && args.iter().all(|value| valid(*value))
            }
            Rvalue::Load(Place {
                base: PlaceBase::State(_) | PlaceBase::Param(_),
                projections,
            }) => projections.is_empty(),
            Rvalue::InputLoad {
                input,
                element: None,
                frame: Value::Local(at),
                ..
            } => {
                *at == frame
                    && add_port(
                        AudioPort::Input(*input),
                        program.interface.inputs[input.index()].ty.index(),
                    )
            }
            _ => false,
        };
        if !legal {
            return None;
        }
        defined.insert(local);
    }
    if !ports
        .iter()
        .any(|port| matches!(port, AudioPort::Output(_)))
    {
        return None;
    }
    Some(PointwiseAudioLoop {
        scalar: scalar?,
        ports,
    })
}
