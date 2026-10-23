//! Target-independent legality checks for counted and pointwise loops.
use std::collections::HashSet;

use crate::*;

mod independent;
pub use independent::independent_memory_loop;

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
pub fn pointwise_audio_loop(
    program: &Program,
    effects: &EffectAnalysis,
    function: &Function,
    block: &Block,
) -> Option<PointwiseAudioLoop> {
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
    let mut escaped = HashSet::new();
    let mut resets = 0;
    let mut steps = 0;
    let mut limits = 0;
    let mut pending = vec![&function.body];
    while let Some(block) = pending.pop() {
        for statement in &block.statements {
            if let Some((local, value)) = assigned_local(statement) {
                if let Rvalue::MakeSlice {
                    source:
                        SliceSource::Place(Place {
                            base: PlaceBase::Local(local),
                            ..
                        }),
                    ..
                } = value
                {
                    escaped.insert(*local);
                }
                if local == counted.index {
                    match value {
                        Rvalue::Use(Value::Constant(ScalarValue::I32(0))) => resets += 1,
                        Rvalue::Binary {
                            op: BinaryOp::Add,
                            lhs: Value::Local(lhs),
                            rhs: Value::Constant(ScalarValue::I32(1)),
                        } if *lhs == local => {
                            steps += 1;
                        }
                        _ => return None,
                    }
                }
                if local == limit {
                    if !matches!(value, Rvalue::Load(Place { base: PlaceBase::Parameter(parameter), projections }) if parameter.index() == PROCESS_FRAMES_PARAM_INDEX && projections.is_empty())
                    {
                        return None;
                    }
                    limits += 1;
                }
            }
            match &statement.kind {
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => pending.extend([then_block, else_block]),
                StatementKind::Loop { body } => pending.push(body),
                StatementKind::Call { results, .. }
                    if results.contains(&limit) || results.contains(&counted.index) =>
                {
                    return None
                }
                StatementKind::Call { args, .. } | StatementKind::PublishDelegate { args, .. } => {
                    for argument in args {
                        if let CallArgument::Place(place)
                        | CallArgument::ArrayWindow { array: place, .. } = argument
                        {
                            if let PlaceBase::Local(local) = place.base {
                                escaped.insert(local);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if resets != 1
        || steps != 1
        || limits != 1
        || escaped.contains(&counted.index)
        || escaped.contains(&limit)
    {
        return None;
    }
    let operations = &counted.body[1..];
    let mut written = HashSet::new();
    let mut record_write = |local: LocalId| {
        if local == counted.index || local == frame || local == limit || escaped.contains(&local) {
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
