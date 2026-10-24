//! Storage identities behind slices and reference arguments. Origins are
//! unions across all definitions and call sites: different views of one state
//! array retain the same identity, including views forwarded through helpers.
//! Contiguous views retain possible logical element ranges across definitions
//! and call sites. Physical sizes and alignment remain backend decisions.
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

use crate::*;

// Bound both storage and fixed-point height when loop-carried descriptors
// repeatedly shift a view. Exceeding the budget loses ranges, never identity.
const MAX_RANGES_PER_STATE: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StorageOrigins {
    states: BTreeMap<StateId, Option<BTreeSet<ElementRange>>>,
    non_state: bool,
    unknown: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ElementRange {
    start: u32,
    len: u32,
}

/// A contiguous range in an owned state's top-level array elements. A scalar
/// or struct state is one element. Nested projections retain identity only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateRegion {
    pub state: StateId,
    pub start: u32,
    pub len: u32,
}

impl StorageOrigins {
    fn unknown() -> Self {
        Self {
            unknown: true,
            ..Self::default()
        }
    }
    fn non_state() -> Self {
        Self {
            non_state: true,
            ..Self::default()
        }
    }
    fn state(program: &Program, state: StateId) -> Self {
        let len = match program.types[program.state[state.index()].ty.index()] {
            Type::Array { len, .. } => len,
            _ => 1,
        };
        Self {
            states: BTreeMap::from([(
                state,
                Some(BTreeSet::from([ElementRange { start: 0, len }])),
            )]),
            ..Self::default()
        }
    }

    /// None means an unknown or as-yet unobserved origin. An empty iterator
    /// means storage known to be separate from the processor's state region.
    pub fn states(&self) -> Option<impl Iterator<Item = StateId> + '_> {
        (!self.unknown && (self.non_state || !self.states.is_empty()))
            .then(|| self.states.keys().copied())
    }

    /// Known origins consisting entirely of owned state objects.
    pub fn only_states(&self) -> Option<impl Iterator<Item = StateId> + '_> {
        if self.non_state || self.unknown || self.states.is_empty() {
            None
        } else {
            Some(self.states.keys().copied())
        }
    }

    /// Every possible contiguous view, or None if any origin or range is
    /// unresolved. One state object may have several alternative ranges;
    /// callers must establish their property for every returned view.
    pub fn state_regions(&self) -> Option<impl Iterator<Item = StateRegion> + '_> {
        let _ = self.only_states()?;
        self.states.values().all(Option::is_some).then(|| {
            self.states.iter().flat_map(|(&state, ranges)| {
                ranges
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(move |range| StateRegion {
                        state,
                        start: range.start,
                        len: range.len,
                    })
            })
        })
    }

    fn forget_ranges(mut self) -> Self {
        self.states.values_mut().for_each(|range| *range = None);
        self
    }

    fn window(mut self, start: Value, len: Value, bounds: BoundsMode, fixed: bool) -> Self {
        let integer = |value| match value {
            Value::Constant(ScalarValue::I32(value)) => Some(i64::from(value)),
            Value::Constant(ScalarValue::I64(value)) => Some(value),
            _ => None,
        };
        for ranges in self.states.values_mut() {
            *ranges = ranges.as_ref().and_then(|ranges| {
                ranges
                    .iter()
                    .map(|source| {
                        let (mut start, mut len) = (integer(start)?, integer(len)?);
                        let available = i64::from(source.len);
                        if fixed && (len < 0 || len > available) {
                            return None;
                        }
                        if bounds == BoundsMode::Clamp {
                            start = start.clamp(0, available - if fixed { len } else { 0 });
                            len = len.clamp(0, available - start);
                        }
                        if start < 0 || len < 0 || start > available || len > available - start {
                            return None;
                        }
                        Some(ElementRange {
                            start: source.start.checked_add(u32::try_from(start).ok()?)?,
                            len: u32::try_from(len).ok()?,
                        })
                    })
                    .collect()
            });
        }
        self
    }

    fn merge(&mut self, other: &Self) -> bool {
        let mut changed = (other.non_state && !self.non_state) || (other.unknown && !self.unknown);
        for (&state, ranges) in &other.states {
            match self.states.entry(state) {
                Entry::Vacant(entry) => {
                    entry.insert(ranges.clone());
                    changed = true;
                }
                Entry::Occupied(mut entry) => {
                    if let Some(known) = entry.get_mut() {
                        if let Some(incoming) = ranges {
                            let before = known.len();
                            known.extend(incoming);
                            changed |= known.len() != before;
                            if known.len() > MAX_RANGES_PER_STATE {
                                entry.insert(None);
                            }
                        } else {
                            entry.insert(None);
                            changed = true;
                        }
                    }
                }
            }
        }
        self.non_state |= other.non_state;
        self.unknown |= other.unknown;
        changed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FunctionOrigins {
    parameters: Vec<StorageOrigins>,
    locals: Vec<StorageOrigins>,
    results: Vec<StorageOrigins>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryAnalysis {
    functions: Vec<FunctionOrigins>,
    pub function_visits: usize,
}

impl MemoryAnalysis {
    pub fn slice(&self, function: FunctionId, value: Value) -> Option<&StorageOrigins> {
        let Value::Local(local) = value else {
            return None;
        };
        self.functions
            .get(function.index())?
            .locals
            .get(local.index())
    }

    pub fn parameter(&self, function: FunctionId, parameter: ParameterId) -> &StorageOrigins {
        &self.functions[function.index()].parameters[parameter.index()]
    }

    pub fn place(&self, program: &Program, function: FunctionId, place: &Place) -> StorageOrigins {
        place_origins(
            program,
            &program.functions[function.index()],
            place,
            &self.functions[function.index()],
        )
    }
}

pub fn analyze_memory(program: &Program) -> MemoryAnalysis {
    let graph = crate::call_graph::CallGraph::new(program);
    let mut analysis = MemoryAnalysis {
        functions: program
            .functions
            .iter()
            .map(|function| FunctionOrigins {
                parameters: vec![
                    if function.kind == FunctionKind::User {
                        StorageOrigins::default()
                    } else {
                        StorageOrigins::unknown()
                    };
                    function.params.len()
                ],
                locals: vec![StorageOrigins::default(); function.locals.len()],
                results: vec![StorageOrigins::default(); function.results.len()],
            })
            .collect(),
        function_visits: 0,
    };
    let mut work = graph.worklist();
    while let Some(index) = work.pop() {
        analysis.function_visits += 1;
        let function = &program.functions[index];
        let mut current = analysis.functions[index].clone();
        let mut calls = Vec::new();
        scan(
            program,
            function,
            &function.body,
            &analysis,
            &mut current,
            &mut calls,
        );
        let previous = &analysis.functions[index];
        if current.locals != previous.locals {
            work.push(index);
        }
        if current.results != previous.results {
            for &caller in &graph.callers[index] {
                work.push(caller);
            }
        }
        analysis.functions[index] = current;
        for (callee, parameter, origins) in calls {
            if analysis.functions[callee].parameters[parameter].merge(&origins) {
                work.push(callee);
            }
        }
    }
    analysis
}

fn value_origins(value: Value, current: &FunctionOrigins) -> StorageOrigins {
    match value {
        Value::Local(local) => current.locals[local.index()].clone(),
        _ => StorageOrigins::unknown(),
    }
}

fn place_origins(
    program: &Program,
    function: &Function,
    place: &Place,
    current: &FunctionOrigins,
) -> StorageOrigins {
    let origins = match place.base {
        PlaceBase::State(state) => StorageOrigins::state(program, state),
        PlaceBase::Parameter(parameter) => current.parameters[parameter.index()].clone(),
        PlaceBase::Local(_) | PlaceBase::Param(_) | PlaceBase::EventParam(_) => {
            StorageOrigins::non_state()
        }
    };
    if place.projections.is_empty() {
        return origins;
    }
    let base = Place {
        base: place.base,
        projections: Vec::new(),
    };
    if let [Projection::Index { index, bounds }] = place.projections.as_slice() {
        if program.place_type(function, &base).is_some_and(|ty| {
            matches!(program.types[ty.index()], Type::Array { element, .. } if !matches!(program.types[element.index()], Type::Array { .. }))
        }) {
            return origins.window(*index, Value::Constant(ScalarValue::I32(1)), *bounds, true);
        }
    }
    origins.forget_ranges()
}

fn slice_place_origins(
    program: &Program,
    function: &Function,
    place: &Place,
    current: &FunctionOrigins,
) -> StorageOrigins {
    if program
        .place_type(function, place)
        .is_some_and(|ty| matches!(program.types[ty.index()], Type::Slice { .. }))
    {
        match place.base {
            PlaceBase::Local(local) if place.projections.is_empty() => {
                current.locals[local.index()].clone()
            }
            PlaceBase::Parameter(parameter) if place.projections.is_empty() => {
                current.parameters[parameter.index()].clone()
            }
            _ => StorageOrigins::unknown(),
        }
    } else {
        place_origins(program, function, place, current)
    }
}

fn rvalue_origins(
    program: &Program,
    function: &Function,
    value: &Rvalue,
    current: &FunctionOrigins,
) -> StorageOrigins {
    match value {
        Rvalue::Use(value) => value_origins(*value, current),
        Rvalue::Load(place) => slice_place_origins(program, function, place, current),
        Rvalue::MakeSlice {
            source: SliceSource::Place(place),
            start,
            len,
            bounds,
            ..
        } => slice_place_origins(program, function, place, current)
            .window(*start, *len, *bounds, false),
        // External buffers, constant data, and stack allocations cannot alias
        // owned state under the MIR/processor ABI storage contract.
        Rvalue::MakeSlice { .. } => StorageOrigins::non_state(),
        _ => StorageOrigins::unknown(),
    }
}

fn argument_origins(
    program: &Program,
    function: &Function,
    argument: &CallArgument,
    parameter: &FunctionParam,
    current: &FunctionOrigins,
) -> StorageOrigins {
    let required_len = match program.types[parameter.ty.index()] {
        Type::Array { len, .. } => len,
        _ => 1,
    };
    let length = Value::Constant(ScalarValue::I64(i64::from(required_len)));
    match argument {
        CallArgument::Value(value) => value_origins(*value, current),
        CallArgument::SliceElement {
            slice,
            index,
            bounds,
        } => value_origins(*slice, current).window(*index, length, *bounds, true),
        CallArgument::SliceWindow {
            slice,
            start,
            bounds,
        } => value_origins(*slice, current).window(*start, length, *bounds, true),
        CallArgument::Place(place) => place_origins(program, function, place, current),
        CallArgument::ArrayWindow {
            array,
            start,
            bounds,
        } => place_origins(program, function, array, current).window(*start, length, *bounds, true),
        _ => StorageOrigins::non_state(),
    }
}

fn scan(
    program: &Program,
    function: &Function,
    block: &Block,
    analysis: &MemoryAnalysis,
    current: &mut FunctionOrigins,
    calls: &mut Vec<(usize, usize, StorageOrigins)>,
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
                let origins = rvalue_origins(program, function, value, current);
                current.locals[local.index()].merge(&origins);
            }
            StatementKind::Call {
                function: callee,
                args,
                results,
            } => {
                for (index, argument) in args.iter().enumerate() {
                    let parameter = &program.functions[callee.index()].params[index];
                    if parameter.mode != PassingMode::Value
                        || matches!(program.types[parameter.ty.index()], Type::Slice { .. })
                    {
                        // References to a descriptor address the descriptor,
                        // not its payload. Do not conflate those two origins.
                        let origins = if parameter.mode != PassingMode::Value
                            && matches!(program.types[parameter.ty.index()], Type::Slice { .. })
                        {
                            StorageOrigins::unknown()
                        } else {
                            argument_origins(program, function, argument, parameter, current)
                        };
                        calls.push((callee.index(), index, origins));
                    }
                    // A writable reference to descriptor storage can replace
                    // its pointer. Element writes only change the payload.
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
                                current.locals[local.index()].merge(&StorageOrigins::unknown());
                            }
                        }
                    }
                }
                for (index, local) in results.iter().enumerate() {
                    if matches!(
                        program.types[function.locals[local.index()].ty.index()],
                        Type::Slice { .. }
                    ) {
                        current.locals[local.index()]
                            .merge(&analysis.functions[callee.index()].results[index]);
                    }
                }
            }
            StatementKind::Return { values } => {
                for (index, value) in values.iter().enumerate() {
                    if matches!(
                        program.types[function.results[index].index()],
                        Type::Slice { .. }
                    ) {
                        let origins = value_origins(*value, current);
                        current.results[index].merge(&origins);
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                scan(program, function, then_block, analysis, current, calls);
                scan(program, function, else_block, analysis, current, calls);
            }
            StatementKind::Loop { body } => scan(program, function, body, analysis, current, calls),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
