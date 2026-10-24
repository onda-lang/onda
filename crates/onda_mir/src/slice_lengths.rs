//! Upper bounds on descriptor lengths, independent of payload mutability.
//! Bounds join every definition and call site. Descriptor references remain
//! conservative; writes through a slice cannot change its length.
use crate::*;

const UNKNOWN: u32 = i32::MAX as u32;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FunctionSliceLengths {
    parameters: Vec<Option<u32>>,
    locals: Vec<Option<u32>>,
    results: Vec<Option<u32>>,
}

impl FunctionSliceLengths {
    pub(crate) fn upper_bound(&self, value: Value) -> u32 {
        self.value(value).unwrap_or(UNKNOWN)
    }

    fn value(&self, value: Value) -> Option<u32> {
        match value {
            Value::Local(local) => self.locals[local.index()],
            Value::Constant(_) => Some(UNKNOWN),
        }
    }

    fn place(&self, program: &Program, function: &Function, place: &Place) -> Option<u32> {
        let Some(ty) = program.place_type(function, place) else {
            return Some(UNKNOWN);
        };
        match program.types[ty.index()] {
            Type::Scalar(_) => Some(1),
            Type::Array { len, .. } => Some(len.min(UNKNOWN)),
            Type::Slice { .. } if place.projections.is_empty() => match place.base {
                PlaceBase::Local(local) => self.locals[local.index()],
                PlaceBase::Parameter(parameter) => self.parameters[parameter.index()],
                _ => Some(UNKNOWN),
            },
            _ => Some(UNKNOWN),
        }
    }

    fn rvalue(&self, program: &Program, function: &Function, value: &Rvalue) -> Option<u32> {
        match value {
            Rvalue::Use(value) => self.value(*value),
            Rvalue::Load(place) => self.place(program, function, place),
            Rvalue::MakeSlice {
                source, start, len, ..
            } => {
                let bound = match source {
                    SliceSource::Place(place) => self.place(program, function, place),
                    SliceSource::ConstData(data) => Some(
                        program.const_data[data.index()]
                            .values
                            .len()
                            .min(UNKNOWN as usize) as u32,
                    ),
                    _ => Some(UNKNOWN),
                };
                let bound = match start {
                    Value::Constant(ScalarValue::I32(start)) => {
                        bound.map(|bound| bound.saturating_sub((*start).max(0) as u32))
                    }
                    _ => bound,
                };
                // Successful checked and producer-proved windows fit the
                // source. Clamped windows can only shorten this bound.
                match len {
                    Value::Constant(ScalarValue::I32(len)) => {
                        Some(bound.unwrap_or(UNKNOWN).min((*len).max(0) as u32))
                    }
                    _ => bound,
                }
            }
            _ => Some(UNKNOWN),
        }
    }
}

fn join(destination: &mut Option<u32>, source: Option<u32>) -> bool {
    let before = *destination;
    *destination = match (*destination, source) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    *destination != before
}

pub(crate) fn analyze(program: &Program) -> Vec<FunctionSliceLengths> {
    let mut lengths: Vec<_> = program
        .functions
        .iter()
        .map(|function| FunctionSliceLengths {
            parameters: function
                .params
                .iter()
                .map(|parameter| {
                    (function.kind != FunctionKind::User || parameter.mode != PassingMode::Value)
                        .then_some(UNKNOWN)
                })
                .collect(),
            locals: vec![None; function.locals.len()],
            results: vec![None; function.results.len()],
        })
        .collect();
    let graph = crate::call_graph::CallGraph::new(program);
    let mut pending = graph.worklist();
    while let Some(index) = pending.pop() {
        let function = &program.functions[index];
        let mut current = lengths[index].clone();
        let mut calls = Vec::new();
        scan(
            program,
            function,
            &function.body,
            &lengths,
            &mut current,
            &mut calls,
        );
        if current.locals != lengths[index].locals {
            pending.push(index);
        }
        if current.results != lengths[index].results {
            for &caller in &graph.callers[index] {
                pending.push(caller);
            }
        }
        lengths[index] = current;
        for (callee, parameter, bound) in calls {
            if join(&mut lengths[callee].parameters[parameter], bound) {
                pending.push(callee);
            }
        }
    }
    lengths
}

fn scan(
    program: &Program,
    function: &Function,
    block: &Block,
    lengths: &[FunctionSliceLengths],
    current: &mut FunctionSliceLengths,
    calls: &mut Vec<(usize, usize, Option<u32>)>,
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign {
                destination:
                    Place {
                        base: PlaceBase::Local(local),
                        projections,
                    },
                value,
            } if projections.is_empty()
                && matches!(
                    program.types[function.locals[local.index()].ty.index()],
                    Type::Slice { .. }
                ) =>
            {
                let bound = current.rvalue(program, function, value);
                join(&mut current.locals[local.index()], bound);
            }
            StatementKind::Call {
                function: callee,
                args,
                results,
            } => {
                for (index, argument) in args.iter().enumerate() {
                    let parameter = &program.functions[callee.index()].params[index];
                    if parameter.mode == PassingMode::Value
                        && matches!(program.types[parameter.ty.index()], Type::Slice { .. })
                    {
                        let bound = match argument {
                            CallArgument::Value(value) => current.value(*value),
                            _ => Some(UNKNOWN),
                        };
                        calls.push((callee.index(), index, bound));
                    }
                    if parameter.mode.is_writable_reference() {
                        if let CallArgument::Place(Place {
                            base: PlaceBase::Local(local),
                            ..
                        }) = argument
                        {
                            if matches!(
                                program.types[function.locals[local.index()].ty.index()],
                                Type::Slice { .. }
                            ) {
                                current.locals[local.index()] = Some(UNKNOWN);
                            }
                        }
                    }
                }
                for (index, local) in results.iter().enumerate() {
                    if matches!(
                        program.types[function.locals[local.index()].ty.index()],
                        Type::Slice { .. }
                    ) {
                        join(
                            &mut current.locals[local.index()],
                            lengths[callee.index()].results[index],
                        );
                    }
                }
            }
            StatementKind::Return { values } => {
                for (index, value) in values.iter().enumerate() {
                    if matches!(
                        program.types[function.results[index].index()],
                        Type::Slice { .. }
                    ) {
                        let bound = current.value(*value);
                        join(&mut current.results[index], bound);
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                scan(program, function, then_block, lengths, current, calls);
                scan(program, function, else_block, lengths, current, calls);
            }
            StatementKind::Loop { body } => scan(program, function, body, lengths, current, calls),
            _ => {}
        }
    }
}
