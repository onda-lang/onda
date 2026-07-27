//! Backend-neutral semantic facts derived from validated MIR.
//!
//! These analyses deliberately describe logical effects rather than a target
//! ABI.  Optimizers and backends can therefore make the same decisions about
//! calls without reverse-engineering pointer provenance from lowered code.

use crate::{
    BinaryOp, Block, BoundsMode, CallArgument, FunctionId, FunctionKind, LocalId, ParameterId,
    Place, PlaceBase, Program, Projection, Rvalue, ScalarType, ScalarValue, SliceSource,
    StatementKind, Value,
};

/// Logical memory domains visible to a MIR function.
///
/// Local scalar and aggregate storage is intentionally absent: it cannot be
/// observed by a caller. `INDIRECT` covers memory reached through a slice or
/// buffer descriptor whose concrete source is not recoverable from the local
/// MIR value alone.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct MemoryRegionSet(u16);

impl MemoryRegionSet {
    pub const STATE: Self = Self(1 << 0);
    pub const PARAMS: Self = Self(1 << 1);
    pub const INPUTS: Self = Self(1 << 2);
    pub const OUTPUTS: Self = Self(1 << 3);
    pub const CONTROL_OUTPUTS: Self = Self(1 << 4);
    pub const BUFFERS: Self = Self(1 << 5);
    pub const CONST_DATA: Self = Self(1 << 6);
    pub const EVENT_PAYLOAD: Self = Self(1 << 7);
    pub const ARGUMENTS: Self = Self(1 << 8);
    pub const INDIRECT: Self = Self(1 << 9);

    pub const CALLER_VISIBLE: Self = Self(
        Self::STATE.0
            | Self::PARAMS.0
            | Self::INPUTS.0
            | Self::OUTPUTS.0
            | Self::CONTROL_OUTPUTS.0
            | Self::BUFFERS.0
            | Self::CONST_DATA.0
            | Self::EVENT_PAYLOAD.0
            | Self::ARGUMENTS.0
            | Self::INDIRECT.0,
    );

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    fn insert(&mut self, other: Self) -> bool {
        let previous = self.0;
        self.0 |= other.0;
        self.0 != previous
    }
}

/// Access performed through one logical function parameter.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ReferenceEffects {
    pub reads: bool,
    pub writes: bool,
}

impl ReferenceEffects {
    pub const fn is_unused(self) -> bool {
        !self.reads && !self.writes
    }

    fn merge(&mut self, other: Self) -> bool {
        let previous = *self;
        self.reads |= other.reads;
        self.writes |= other.writes;
        *self != previous
    }
}

/// Transitive effects of one MIR function.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct FunctionEffects {
    pub reads: MemoryRegionSet,
    pub writes: MemoryRegionSet,
    pub parameters: Vec<ReferenceEffects>,
    /// The function may execute an explicit trap or a checked operation.
    pub may_trap: bool,
    /// The function contains a loop, or calls a function that may not return.
    /// MIR loops are structured but are not required to have a static trip
    /// count, so this is intentionally conservative.
    pub may_not_return: bool,
}

impl FunctionEffects {
    pub fn is_memory_free(&self) -> bool {
        self.reads.is_empty() && self.writes.is_empty()
    }

    pub fn is_read_only(&self) -> bool {
        self.writes.is_empty()
    }

    pub fn has_observable_writes(&self) -> bool {
        self.writes.intersects(MemoryRegionSet::CALLER_VISIBLE)
    }
}

/// Whole-program, call-transitive MIR effects indexed by [`FunctionId`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EffectAnalysis {
    functions: Vec<FunctionEffects>,
}

impl EffectAnalysis {
    pub fn function(&self, function: FunctionId) -> &FunctionEffects {
        &self.functions[function.index()]
    }

    pub fn functions(&self) -> &[FunctionEffects] {
        &self.functions
    }
}

#[derive(Debug, Clone)]
struct CallSite {
    callee: FunctionId,
    args: Vec<CallArgument>,
}

/// Computes direct and call-transitive effects for every MIR function.
///
/// Validation rejects recursive call graphs. The implementation nevertheless
/// uses a monotonic fixed point so the analysis remains independent of
/// function-table ordering and remains finite if presented with unvalidated
/// input by a diagnostic caller.
pub fn analyze_effects(program: &Program) -> EffectAnalysis {
    let mut functions = Vec::with_capacity(program.functions.len());
    let mut calls = Vec::with_capacity(program.functions.len());
    for function in &program.functions {
        let parameters = function
            .params
            .iter()
            .map(|parameter| {
                if matches!(program.types[parameter.ty.index()], crate::Type::Scalar(_)) {
                    return ReferenceEffects::default();
                }
                // Aggregate references may be converted to slice descriptors
                // before their storage is accessed. Until the analysis tracks
                // descriptor provenance, the declared passing mode is the
                // sound contract for those indirect accesses.
                match parameter.mode {
                    crate::PassingMode::Value => ReferenceEffects::default(),
                    crate::PassingMode::ReadOnlyReference => ReferenceEffects {
                        reads: true,
                        writes: false,
                    },
                    crate::PassingMode::ReadWriteReference => ReferenceEffects {
                        reads: true,
                        writes: true,
                    },
                }
            })
            .collect::<Vec<_>>();
        let mut effects = FunctionEffects {
            parameters,
            ..FunctionEffects::default()
        };
        if effects.parameters.iter().any(|effect| effect.reads) {
            effects.reads.insert(MemoryRegionSet::ARGUMENTS);
        }
        if effects.parameters.iter().any(|effect| effect.writes) {
            effects.writes.insert(MemoryRegionSet::ARGUMENTS);
        }
        let mut function_calls = Vec::new();
        scan_block(&function.body, &mut effects, &mut function_calls);
        normalize_value_parameter_effects(function, &mut effects);
        functions.push(effects);
        calls.push(function_calls);
    }

    loop {
        let snapshot = functions.clone();
        let mut changed = false;
        for (caller_index, call_sites) in calls.iter().enumerate() {
            for call in call_sites {
                let callee = &snapshot[call.callee.index()];
                changed |= functions[caller_index]
                    .reads
                    .insert(callee.reads.without(MemoryRegionSet::ARGUMENTS));
                changed |= functions[caller_index]
                    .writes
                    .insert(callee.writes.without(MemoryRegionSet::ARGUMENTS));
                if callee.may_trap && !functions[caller_index].may_trap {
                    functions[caller_index].may_trap = true;
                    changed = true;
                }
                if callee.may_not_return && !functions[caller_index].may_not_return {
                    functions[caller_index].may_not_return = true;
                    changed = true;
                }

                for (parameter_index, parameter_effects) in
                    callee.parameters.iter().copied().enumerate()
                {
                    if parameter_effects.is_unused() {
                        continue;
                    }
                    let Some(argument) = call.args.get(parameter_index) else {
                        continue;
                    };
                    changed |= merge_argument_effects(
                        &mut functions[caller_index],
                        &program.functions[caller_index],
                        argument,
                        parameter_effects,
                    );
                }
            }
        }
        if !changed {
            break;
        }
    }

    EffectAnalysis { functions }
}

fn normalize_value_parameter_effects(function: &crate::Function, effects: &mut FunctionEffects) {
    for (parameter, parameter_effects) in function.params.iter().zip(&mut effects.parameters) {
        if parameter.mode == crate::PassingMode::Value {
            *parameter_effects = ReferenceEffects::default();
        }
    }
    if !effects.parameters.iter().any(|effect| effect.reads) {
        effects.reads = effects.reads.without(MemoryRegionSet::ARGUMENTS);
    }
    if !effects.parameters.iter().any(|effect| effect.writes) {
        effects.writes = effects.writes.without(MemoryRegionSet::ARGUMENTS);
    }
}

/// Inclusive integer interval with an explicit MIR scalar width.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct IntegerRange {
    scalar: ScalarType,
    min: i64,
    max: i64,
}

impl IntegerRange {
    pub fn new(scalar: ScalarType, min: i64, max: i64) -> Option<Self> {
        let (type_min, type_max) = integer_type_bounds(scalar)?;
        (min <= max && min >= type_min && max <= type_max).then_some(Self { scalar, min, max })
    }

    pub const fn scalar(self) -> ScalarType {
        self.scalar
    }

    pub const fn min(self) -> i64 {
        self.min
    }

    pub const fn max(self) -> i64 {
        self.max
    }

    pub const fn contains(self, value: i64) -> bool {
        value >= self.min && value <= self.max
    }

    pub const fn is_nonnegative(self) -> bool {
        self.min >= 0
    }

    fn full(scalar: ScalarType) -> Option<Self> {
        let (min, max) = integer_type_bounds(scalar)?;
        Some(Self { scalar, min, max })
    }

    fn singleton(value: ScalarValue) -> Option<Self> {
        match value {
            ScalarValue::I32(value) => {
                Self::new(ScalarType::I32, i64::from(value), i64::from(value))
            }
            ScalarValue::I64(value) => Self::new(ScalarType::I64, value, value),
            _ => None,
        }
    }

    fn join(self, other: Self) -> Option<Self> {
        (self.scalar == other.scalar).then_some(Self {
            scalar: self.scalar,
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        })
    }
}

/// Conservative integer ranges for one function. A local range contains all
/// values assigned to that local along every analyzed structured path.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FunctionRangeAnalysis {
    parameters: Vec<Option<IntegerRange>>,
    locals: Vec<Option<IntegerRange>>,
}

impl FunctionRangeAnalysis {
    pub fn parameter(&self, parameter: ParameterId) -> Option<IntegerRange> {
        self.parameters.get(parameter.index()).copied().flatten()
    }

    pub fn local(&self, local: LocalId) -> Option<IntegerRange> {
        self.locals.get(local.index()).copied().flatten()
    }

    pub fn locals(&self) -> &[Option<IntegerRange>] {
        &self.locals
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RangeSummary {
    seen: bool,
    range: Option<IntegerRange>,
}

/// Infers target-independent integer ranges from constants, declared
/// interface ranges, the segmented-process contract, and structured scalar
/// operations. Wrapping operations widen to the complete scalar range unless
/// the mathematical result is proven not to overflow.
pub fn analyze_integer_ranges(program: &Program, function: FunctionId) -> FunctionRangeAnalysis {
    let function = &program.functions[function.index()];
    let parameters = function_parameter_ranges(program, function);
    let mut environment = vec![None; function.locals.len()];
    let mut summary = vec![RangeSummary::default(); function.locals.len()];
    analyze_range_block(
        program,
        function,
        &function.body,
        &parameters,
        &mut environment,
        &mut summary,
    );
    FunctionRangeAnalysis {
        parameters,
        locals: summary.into_iter().map(|summary| summary.range).collect(),
    }
}

fn function_parameter_ranges(
    program: &Program,
    function: &crate::Function,
) -> Vec<Option<IntegerRange>> {
    let mut ranges = vec![None; function.params.len()];
    if function.kind == FunctionKind::Process && ranges.len() >= crate::PROCESS_PARAM_COUNT {
        let maximum = i64::from(program.config.block_size);
        ranges[crate::PROCESS_START_FRAME_PARAM_INDEX] =
            IntegerRange::new(ScalarType::I32, 0, maximum);
        ranges[crate::PROCESS_FRAMES_PARAM_INDEX] = IntegerRange::new(ScalarType::I32, 0, maximum);
        ranges[crate::PROCESS_FLAGS_PARAM_INDEX] =
            IntegerRange::new(ScalarType::I32, 0, i64::from(crate::PROCESS_FULL_BLOCK));
    }
    ranges
}

fn analyze_range_block(
    program: &Program,
    function: &crate::Function,
    block: &Block,
    parameters: &[Option<IntegerRange>],
    environment: &mut [Option<IntegerRange>],
    summary: &mut [RangeSummary],
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value }
                if destination.projections.is_empty()
                    && matches!(destination.base, PlaceBase::Local(_)) =>
            {
                let PlaceBase::Local(local) = destination.base else {
                    unreachable!()
                };
                let range = range_of_rvalue(program, function, value, parameters, environment);
                environment[local.index()] = range;
                record_range(&mut summary[local.index()], range);
            }
            StatementKind::Call {
                results,
                function: callee,
                args,
            } => {
                for result in results {
                    environment[result.index()] = None;
                    record_range(&mut summary[result.index()], None);
                }
                for (index, argument) in args.iter().enumerate() {
                    if program.functions[callee.index()].params[index].mode
                        == crate::PassingMode::ReadWriteReference
                    {
                        if let Some(local) = argument_local(argument) {
                            environment[local.index()] = None;
                            record_range(&mut summary[local.index()], None);
                        }
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                let mut then_environment = environment.to_vec();
                let mut else_environment = environment.to_vec();
                analyze_range_block(
                    program,
                    function,
                    then_block,
                    parameters,
                    &mut then_environment,
                    summary,
                );
                analyze_range_block(
                    program,
                    function,
                    else_block,
                    parameters,
                    &mut else_environment,
                    summary,
                );
                join_range_environments(environment, &then_environment, &else_environment);
            }
            StatementKind::Loop { body } => {
                let mut mutated = Vec::new();
                collect_range_mutations(body, program, &mut mutated);
                let mut body_environment = environment.to_vec();
                for local in &mutated {
                    body_environment[local.index()] = None;
                }
                analyze_range_block(
                    program,
                    function,
                    body,
                    parameters,
                    &mut body_environment,
                    summary,
                );
                for local in mutated {
                    environment[local.index()] = None;
                }
            }
            _ => {}
        }
    }
}

fn range_of_rvalue(
    program: &Program,
    _function: &crate::Function,
    value: &Rvalue,
    parameters: &[Option<IntegerRange>],
    environment: &[Option<IntegerRange>],
) -> Option<IntegerRange> {
    match value {
        Rvalue::Use(value) => range_of_value(*value, environment),
        Rvalue::Load(place) if place.projections.is_empty() => match place.base {
            PlaceBase::Parameter(parameter) => parameters.get(parameter.index()).copied().flatten(),
            PlaceBase::Param(param) => program
                .interface
                .params
                .get(param.index())
                .and_then(|parameter| parameter.range)
                .and_then(integer_range_from_value_range),
            _ => None,
        },
        Rvalue::Unary { op, operand } => {
            let operand = range_of_value(*operand, environment)?;
            match op {
                crate::UnaryOp::Negate => checked_range(
                    operand.scalar,
                    -i128::from(operand.max),
                    -i128::from(operand.min),
                ),
                crate::UnaryOp::BitNot => IntegerRange::full(operand.scalar),
                crate::UnaryOp::LogicalNot => None,
            }
        }
        Rvalue::Binary { op, lhs, rhs } => range_of_binary(
            *op,
            range_of_value(*lhs, environment)?,
            range_of_value(*rhs, environment)?,
        ),
        Rvalue::Cast { value, to } => {
            let source = range_of_value(*value, environment)?;
            match (source.scalar, to) {
                (ScalarType::I32, ScalarType::I64) => {
                    IntegerRange::new(*to, source.min, source.max)
                }
                (ScalarType::I64, ScalarType::I32) => IntegerRange::full(*to),
                (from, to) if from == *to => Some(source),
                _ => None,
            }
        }
        Rvalue::Intrinsic { intrinsic, args }
            if matches!(intrinsic, crate::Intrinsic::Min | crate::Intrinsic::Max)
                && args.len() == 2 =>
        {
            let lhs = range_of_value(args[0], environment)?;
            let rhs = range_of_value(args[1], environment)?;
            if lhs.scalar != rhs.scalar {
                return None;
            }
            match intrinsic {
                crate::Intrinsic::Min => {
                    IntegerRange::new(lhs.scalar, lhs.min.min(rhs.min), lhs.max.min(rhs.max))
                }
                crate::Intrinsic::Max => {
                    IntegerRange::new(lhs.scalar, lhs.min.max(rhs.min), lhs.max.max(rhs.max))
                }
                _ => unreachable!(),
            }
        }
        Rvalue::Intrinsic {
            intrinsic: crate::Intrinsic::RangeClamp,
            args,
        } if args.len() == 3 => {
            let lower = range_of_value(args[1], environment)?;
            let upper = range_of_value(args[2], environment)?;
            if lower.scalar != upper.scalar
                || lower.min != lower.max
                || upper.min != upper.max
                || lower.min > upper.max
            {
                return None;
            }
            IntegerRange::new(lower.scalar, lower.min, upper.max)
        }
        Rvalue::ProcessFrame { .. } => IntegerRange::new(
            ScalarType::I32,
            0,
            i64::from(program.config.block_size.saturating_sub(1)),
        ),
        Rvalue::BufferLen(_)
        | Rvalue::BufferChannels(_)
        | Rvalue::BufferParamLen(_)
        | Rvalue::BufferParamChannels(_)
        | Rvalue::SliceLen(_) => IntegerRange::new(ScalarType::I32, 0, i64::from(i32::MAX)),
        _ => None,
    }
}

fn range_of_binary(op: BinaryOp, lhs: IntegerRange, rhs: IntegerRange) -> Option<IntegerRange> {
    if lhs.scalar != rhs.scalar {
        return None;
    }
    match op {
        BinaryOp::Add => checked_range(
            lhs.scalar,
            i128::from(lhs.min) + i128::from(rhs.min),
            i128::from(lhs.max) + i128::from(rhs.max),
        ),
        BinaryOp::Subtract => checked_range(
            lhs.scalar,
            i128::from(lhs.min) - i128::from(rhs.max),
            i128::from(lhs.max) - i128::from(rhs.min),
        ),
        BinaryOp::Multiply => {
            let products = [
                i128::from(lhs.min) * i128::from(rhs.min),
                i128::from(lhs.min) * i128::from(rhs.max),
                i128::from(lhs.max) * i128::from(rhs.min),
                i128::from(lhs.max) * i128::from(rhs.max),
            ];
            checked_range(lhs.scalar, *products.iter().min()?, *products.iter().max()?)
        }
        BinaryOp::Remainder if !rhs.contains(0) => {
            let magnitude = rhs.min.unsigned_abs().max(rhs.max.unsigned_abs());
            let maximum = i64::try_from(magnitude.saturating_sub(1)).ok()?;
            IntegerRange::new(lhs.scalar, -maximum, maximum)
        }
        BinaryOp::Divide
        | BinaryOp::Remainder
        | BinaryOp::BitAnd
        | BinaryOp::BitOr
        | BinaryOp::BitXor
        | BinaryOp::ShiftLeft
        | BinaryOp::ShiftRight => IntegerRange::full(lhs.scalar),
    }
}

fn checked_range(scalar: ScalarType, min: i128, max: i128) -> Option<IntegerRange> {
    let (type_min, type_max) = integer_type_bounds(scalar)?;
    if min < i128::from(type_min) || max > i128::from(type_max) {
        return IntegerRange::full(scalar);
    }
    IntegerRange::new(scalar, min as i64, max as i64)
}

fn integer_type_bounds(scalar: ScalarType) -> Option<(i64, i64)> {
    match scalar {
        ScalarType::I32 => Some((i64::from(i32::MIN), i64::from(i32::MAX))),
        ScalarType::I64 => Some((i64::MIN, i64::MAX)),
        _ => None,
    }
}

fn integer_range_from_value_range(range: crate::ValueRange) -> Option<IntegerRange> {
    match (range.min, range.max) {
        (ScalarValue::I32(min), ScalarValue::I32(max)) => {
            IntegerRange::new(ScalarType::I32, i64::from(min), i64::from(max))
        }
        (ScalarValue::I64(min), ScalarValue::I64(max)) => {
            IntegerRange::new(ScalarType::I64, min, max)
        }
        _ => None,
    }
}

fn range_of_value(value: Value, environment: &[Option<IntegerRange>]) -> Option<IntegerRange> {
    match value {
        Value::Constant(value) => IntegerRange::singleton(value),
        Value::Local(local) => environment.get(local.index()).copied().flatten(),
    }
}

fn record_range(summary: &mut RangeSummary, range: Option<IntegerRange>) {
    if !summary.seen {
        summary.seen = true;
        summary.range = range;
    } else {
        summary.range = match (summary.range, range) {
            (Some(existing), Some(range)) => existing.join(range),
            _ => None,
        };
    }
}

fn join_range_environments(
    destination: &mut [Option<IntegerRange>],
    lhs: &[Option<IntegerRange>],
    rhs: &[Option<IntegerRange>],
) {
    for (index, destination) in destination.iter_mut().enumerate() {
        *destination = match (lhs[index], rhs[index]) {
            (Some(lhs), Some(rhs)) => lhs.join(rhs),
            _ => None,
        };
    }
}

fn argument_local(argument: &CallArgument) -> Option<LocalId> {
    let place = match argument {
        CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => place,
        _ => return None,
    };
    match place.base {
        PlaceBase::Local(local) => Some(local),
        _ => None,
    }
}

fn collect_range_mutations(block: &Block, program: &Program, mutated: &mut Vec<LocalId>) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, .. } => {
                if let PlaceBase::Local(local) = destination.base {
                    insert_range_mutation(mutated, local);
                }
            }
            StatementKind::Call {
                results,
                function,
                args,
            } => {
                for local in results {
                    insert_range_mutation(mutated, *local);
                }
                for (index, argument) in args.iter().enumerate() {
                    if program.functions[function.index()].params[index].mode
                        == crate::PassingMode::ReadWriteReference
                    {
                        if let Some(local) = argument_local(argument) {
                            insert_range_mutation(mutated, local);
                        }
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_range_mutations(then_block, program, mutated);
                collect_range_mutations(else_block, program, mutated);
            }
            StatementKind::Loop { body } => collect_range_mutations(body, program, mutated),
            _ => {}
        }
    }
}

fn insert_range_mutation(mutated: &mut Vec<LocalId>, local: LocalId) {
    if !mutated.contains(&local) {
        mutated.push(local);
    }
}

fn scan_block(block: &Block, effects: &mut FunctionEffects, calls: &mut Vec<CallSite>) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                scan_place(destination, Access::Write, effects);
                scan_rvalue(value, effects);
            }
            StatementKind::Call { function, args, .. } => {
                for argument in args {
                    scan_call_argument(argument, effects);
                }
                calls.push(CallSite {
                    callee: *function,
                    args: args.clone(),
                });
            }
            StatementKind::OutputStore {
                element,
                bounds,
                frame,
                ..
            } => {
                effects.writes.insert(MemoryRegionSet::OUTPUTS);
                scan_optional_value(*element, effects);
                scan_value(*frame, effects);
                mark_checked(*bounds, effects);
            }
            StatementKind::ControlOutputStore {
                element, bounds, ..
            } => {
                effects.writes.insert(MemoryRegionSet::CONTROL_OUTPUTS);
                scan_optional_value(*element, effects);
                mark_checked(*bounds, effects);
            }
            StatementKind::BufferStore {
                channel,
                index,
                bounds,
                ..
            } => {
                effects.writes.insert(MemoryRegionSet::BUFFERS);
                scan_optional_value(*channel, effects);
                scan_value(*index, effects);
                mark_checked(*bounds, effects);
            }
            StatementKind::BufferParamStore {
                parameter,
                channel,
                index,
                bounds,
                ..
            } => {
                mark_parameter(*parameter, Access::Read, effects);
                effects.writes.insert(MemoryRegionSet::INDIRECT);
                scan_optional_value(*channel, effects);
                scan_value(*index, effects);
                mark_checked(*bounds, effects);
            }
            StatementKind::SliceStore {
                slice,
                index,
                bounds,
                ..
            } => {
                effects.writes.insert(MemoryRegionSet::INDIRECT);
                scan_value(*slice, effects);
                scan_value(*index, effects);
                mark_checked(*bounds, effects);
            }
            StatementKind::SliceFill { destination, .. } => {
                effects.writes.insert(MemoryRegionSet::INDIRECT);
                scan_value(*destination, effects);
            }
            StatementKind::SliceCopy {
                destination,
                source,
            } => {
                effects.reads.insert(MemoryRegionSet::INDIRECT);
                effects.writes.insert(MemoryRegionSet::INDIRECT);
                scan_value(*destination, effects);
                scan_value(*source, effects);
                effects.may_trap = true;
            }
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                scan_value(*condition, effects);
                scan_block(then_block, effects, calls);
                scan_block(else_block, effects, calls);
            }
            StatementKind::Loop { body } => {
                effects.may_not_return = true;
                scan_block(body, effects, calls);
            }
            StatementKind::Return { values } => {
                for value in values {
                    scan_value(*value, effects);
                }
            }
            StatementKind::Break | StatementKind::Continue => {}
        }
    }
}

fn scan_rvalue(value: &Rvalue, effects: &mut FunctionEffects) {
    match value {
        Rvalue::Use(value) | Rvalue::SliceLen(value) => scan_value(*value, effects),
        Rvalue::Load(place) => scan_place(place, Access::Read, effects),
        Rvalue::Unary { operand, .. } => scan_value(*operand, effects),
        Rvalue::Binary { op, lhs, rhs } => {
            scan_value(*lhs, effects);
            scan_value(*rhs, effects);
            if matches!(op, crate::BinaryOp::Divide | crate::BinaryOp::Remainder) {
                effects.may_trap = true;
            }
        }
        Rvalue::Compare { lhs, rhs, .. } => {
            scan_value(*lhs, effects);
            scan_value(*rhs, effects);
        }
        Rvalue::Cast { value, .. } => scan_value(*value, effects),
        Rvalue::Intrinsic { args, .. } => {
            for value in args {
                scan_value(*value, effects);
            }
        }
        Rvalue::ProcessFrame { offset } => {
            scan_value(*offset, effects);
            effects.may_trap = true;
        }
        Rvalue::InputLoad {
            element,
            bounds,
            frame,
            ..
        } => {
            effects.reads.insert(MemoryRegionSet::INPUTS);
            scan_optional_value(*element, effects);
            scan_value(*frame, effects);
            mark_checked(*bounds, effects);
        }
        Rvalue::OutputLoad {
            element,
            bounds,
            frame,
            ..
        } => {
            effects.reads.insert(MemoryRegionSet::OUTPUTS);
            scan_optional_value(*element, effects);
            scan_value(*frame, effects);
            mark_checked(*bounds, effects);
        }
        Rvalue::BufferLoad {
            channel,
            index,
            bounds,
            ..
        } => {
            effects.reads.insert(MemoryRegionSet::BUFFERS);
            scan_optional_value(*channel, effects);
            scan_value(*index, effects);
            mark_checked(*bounds, effects);
        }
        Rvalue::BufferParamLoad {
            parameter,
            channel,
            index,
            bounds,
        } => {
            mark_parameter(*parameter, Access::Read, effects);
            effects.reads.insert(MemoryRegionSet::INDIRECT);
            scan_optional_value(*channel, effects);
            scan_value(*index, effects);
            mark_checked(*bounds, effects);
        }
        Rvalue::BufferLen(_) | Rvalue::BufferChannels(_) | Rvalue::BufferSampleRate(_) => {
            effects.reads.insert(MemoryRegionSet::BUFFERS);
        }
        Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter) => {
            mark_parameter(*parameter, Access::Read, effects);
        }
        Rvalue::ConstDataLoad { index, bounds, .. } => {
            effects.reads.insert(MemoryRegionSet::CONST_DATA);
            scan_value(*index, effects);
            mark_checked(*bounds, effects);
        }
        Rvalue::MakeSlice {
            source,
            start,
            len,
            bounds,
            ..
        } => {
            scan_slice_source(source, effects);
            scan_value(*start, effects);
            scan_value(*len, effects);
            mark_checked(*bounds, effects);
        }
        Rvalue::SliceLoad {
            slice,
            index,
            bounds,
        } => {
            effects.reads.insert(MemoryRegionSet::INDIRECT);
            scan_value(*slice, effects);
            scan_value(*index, effects);
            mark_checked(*bounds, effects);
        }
    }
}

fn scan_slice_source(source: &SliceSource, effects: &mut FunctionEffects) {
    match source {
        SliceSource::Place(place) => scan_place(place, Access::Read, effects),
        SliceSource::Buffer { channel, .. } => {
            effects.reads.insert(MemoryRegionSet::BUFFERS);
            scan_optional_value(*channel, effects);
        }
        SliceSource::BufferParam { parameter, channel } => {
            mark_parameter(*parameter, Access::Read, effects);
            scan_optional_value(*channel, effects);
        }
        SliceSource::ConstData(_) => {
            effects.reads.insert(MemoryRegionSet::CONST_DATA);
        }
    }
}

fn scan_call_argument(argument: &CallArgument, effects: &mut FunctionEffects) {
    match argument {
        CallArgument::Value(value) => scan_value(*value, effects),
        CallArgument::Place(place) => scan_place_indices(place, effects),
        CallArgument::SliceElement {
            slice,
            index,
            bounds,
        } => {
            scan_value(*slice, effects);
            scan_value(*index, effects);
            mark_checked(*bounds, effects);
        }
        CallArgument::ArrayWindow {
            array,
            start,
            bounds,
        } => {
            scan_place_indices(array, effects);
            scan_value(*start, effects);
            mark_checked(*bounds, effects);
        }
        CallArgument::SliceWindow {
            slice,
            start,
            bounds,
        } => {
            scan_value(*slice, effects);
            scan_value(*start, effects);
            mark_checked(*bounds, effects);
        }
        CallArgument::Buffer(_) => {
            effects.reads.insert(MemoryRegionSet::BUFFERS);
        }
    }
}

#[derive(Clone, Copy)]
enum Access {
    Read,
    Write,
}

fn scan_place(place: &Place, access: Access, effects: &mut FunctionEffects) {
    scan_place_indices(place, effects);
    mark_base(place.base, access, effects);
}

fn scan_place_indices(place: &Place, effects: &mut FunctionEffects) {
    for projection in &place.projections {
        if let Projection::Index { index, bounds } = projection {
            scan_value(*index, effects);
            mark_checked(*bounds, effects);
        }
    }
}

fn mark_base(base: PlaceBase, access: Access, effects: &mut FunctionEffects) {
    match base {
        PlaceBase::Local(_) => {}
        PlaceBase::Parameter(parameter) => mark_parameter(parameter, access, effects),
        PlaceBase::State(_) => mark_region(MemoryRegionSet::STATE, access, effects),
        PlaceBase::Param(_) => mark_region(MemoryRegionSet::PARAMS, access, effects),
        PlaceBase::EventParam(_) => mark_region(MemoryRegionSet::EVENT_PAYLOAD, access, effects),
    }
}

fn mark_parameter(parameter: ParameterId, access: Access, effects: &mut FunctionEffects) {
    mark_region(MemoryRegionSet::ARGUMENTS, access, effects);
    let Some(parameter_effects) = effects.parameters.get_mut(parameter.index()) else {
        return;
    };
    match access {
        Access::Read => parameter_effects.reads = true,
        Access::Write => parameter_effects.writes = true,
    }
}

fn mark_region(region: MemoryRegionSet, access: Access, effects: &mut FunctionEffects) {
    match access {
        Access::Read => {
            effects.reads.insert(region);
        }
        Access::Write => {
            effects.writes.insert(region);
        }
    }
}

fn merge_argument_effects(
    caller: &mut FunctionEffects,
    caller_function: &crate::Function,
    argument: &CallArgument,
    access: ReferenceEffects,
) -> bool {
    let (region, caller_parameter) = match argument {
        CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => {
            match place.base {
                PlaceBase::Parameter(parameter)
                    if caller_function.params[parameter.index()].mode
                        == crate::PassingMode::Value =>
                {
                    (MemoryRegionSet::default(), None)
                }
                _ => region_for_base(place.base),
            }
        }
        CallArgument::SliceElement { .. } | CallArgument::SliceWindow { .. } => {
            (MemoryRegionSet::INDIRECT, None)
        }
        CallArgument::Buffer(_) => (MemoryRegionSet::BUFFERS, None),
        CallArgument::Value(_) => (MemoryRegionSet::INDIRECT, None),
    };
    let mut changed = false;
    if access.reads {
        changed |= caller.reads.insert(region);
    }
    if access.writes {
        changed |= caller.writes.insert(region);
    }
    if let Some(parameter) = caller_parameter {
        if let Some(parameter_effects) = caller.parameters.get_mut(parameter.index()) {
            changed |= parameter_effects.merge(access);
        }
    }
    changed
}

fn region_for_base(base: PlaceBase) -> (MemoryRegionSet, Option<ParameterId>) {
    match base {
        PlaceBase::Local(_) => (MemoryRegionSet::default(), None),
        PlaceBase::Parameter(parameter) => (MemoryRegionSet::ARGUMENTS, Some(parameter)),
        PlaceBase::State(_) => (MemoryRegionSet::STATE, None),
        PlaceBase::Param(_) => (MemoryRegionSet::PARAMS, None),
        PlaceBase::EventParam(_) => (MemoryRegionSet::EVENT_PAYLOAD, None),
    }
}

fn scan_optional_value(value: Option<Value>, effects: &mut FunctionEffects) {
    if let Some(value) = value {
        scan_value(value, effects);
    }
}

fn scan_value(_value: Value, _effects: &mut FunctionEffects) {
    // Scalar values do not dereference storage. Slice/buffer descriptors are
    // accounted for by the operation that consumes them.
}

fn mark_checked(bounds: BoundsMode, effects: &mut FunctionEffects) {
    if bounds != BoundsMode::Unchecked {
        effects.may_trap = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        process_function_params, AccessMode, CompileConfig, Function, FunctionAttributes,
        FunctionKind, InlineHint, Local, ScalarType, SourceSpan, StatePersistence, StateSlot,
        Statement, Type, TypeId,
    };

    fn statement(kind: StatementKind) -> Statement {
        Statement {
            kind,
            source: SourceSpan::UNKNOWN,
        }
    }

    fn function(name: &str, params: Vec<crate::FunctionParam>, body: Block) -> Function {
        Function {
            name: name.to_owned(),
            kind: FunctionKind::User,
            attributes: FunctionAttributes {
                origin: crate::FunctionOrigin::CompilerGenerated,
                inline: InlineHint::Auto,
            },
            params,
            results: Vec::new(),
            locals: Vec::new(),
            body,
            source: SourceSpan::UNKNOWN,
        }
    }

    #[test]
    fn effects_follow_reference_arguments_through_calls() {
        let scalar = TypeId::new(0);
        let reference = crate::FunctionParam {
            name: "value".to_owned(),
            ty: scalar,
            mode: crate::PassingMode::ReadWriteReference,
        };
        let aggregate_reference = crate::FunctionParam {
            name: "values".to_owned(),
            ty: TypeId::new(1),
            mode: crate::PassingMode::ReadWriteReference,
        };
        let callee = function(
            "callee",
            vec![reference, aggregate_reference],
            Block {
                statements: vec![statement(StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::Parameter(ParameterId::new(0)),
                        projections: Vec::new(),
                    },
                    value: Rvalue::Use(Value::Constant(crate::ScalarValue::F32(1.0))),
                })],
            },
        );
        let caller = function(
            "caller",
            Vec::new(),
            Block {
                statements: vec![statement(StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(2),
                    args: vec![CallArgument::Place(Place {
                        base: PlaceBase::State(crate::StateId::new(0)),
                        projections: Vec::new(),
                    })],
                })],
            },
        );
        let init = Function {
            name: "init".to_owned(),
            kind: FunctionKind::Init,
            attributes: FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        };
        let process = Function {
            name: "process".to_owned(),
            kind: FunctionKind::Process,
            attributes: FunctionAttributes::default(),
            params: process_function_params(scalar),
            results: Vec::new(),
            locals: vec![Local {
                name: None,
                ty: scalar,
            }],
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        };
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types = vec![
            Type::Scalar(ScalarType::F32),
            Type::Array {
                element: scalar,
                len: 4,
            },
        ];
        program.state.push(StateSlot {
            name: "value".to_owned(),
            ty: scalar,
            persistence: StatePersistence::Snapshot,
        });
        program.functions = vec![init, process, callee, caller];

        let analysis = analyze_effects(&program);
        let callee = analysis.function(FunctionId::new(2));
        assert!(callee.parameters[0].writes);
        assert!(callee.parameters[1].reads);
        assert!(callee.parameters[1].writes);
        assert!(callee.writes.contains(MemoryRegionSet::ARGUMENTS));
        let caller = analysis.function(FunctionId::new(3));
        assert!(caller.writes.contains(MemoryRegionSet::STATE));
        assert!(!caller.writes.contains(MemoryRegionSet::ARGUMENTS));
    }

    #[test]
    fn effects_distinguish_pure_and_read_only_functions() {
        let mut pure = function(
            "pure",
            Vec::new(),
            Block {
                statements: vec![statement(StatementKind::Return { values: Vec::new() })],
            },
        );
        pure.params.push(crate::FunctionParam {
            name: "value".to_owned(),
            ty: TypeId::new(0),
            mode: crate::PassingMode::Value,
        });
        pure.locals.push(Local {
            name: None,
            ty: TypeId::new(0),
        });
        pure.body.statements.insert(
            0,
            statement(StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Load(Place {
                    base: PlaceBase::Parameter(ParameterId::new(0)),
                    projections: Vec::new(),
                }),
            }),
        );
        let reader = function(
            "reader",
            Vec::new(),
            Block {
                statements: vec![statement(StatementKind::Assign {
                    destination: Place::local(crate::LocalId::new(0)),
                    value: Rvalue::Load(Place {
                        base: PlaceBase::State(crate::StateId::new(0)),
                        projections: Vec::new(),
                    }),
                })],
            },
        );
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(0),
        );
        program.types.push(Type::Scalar(ScalarType::F32));
        program.functions = vec![pure, reader];

        let analysis = analyze_effects(&program);
        assert!(analysis.function(FunctionId::new(0)).is_memory_free());
        assert!(analysis.function(FunctionId::new(1)).is_read_only());
        assert!(analysis
            .function(FunctionId::new(1))
            .reads
            .contains(MemoryRegionSet::STATE));
    }

    #[test]
    fn access_mode_type_remains_a_logical_fact() {
        assert_ne!(AccessMode::ReadOnly, AccessMode::ReadWrite);
    }

    #[test]
    fn integer_ranges_include_the_segment_contract_and_checked_arithmetic() {
        let i32_ty = TypeId::new(0);
        let mut process = Function {
            name: "process".to_owned(),
            kind: FunctionKind::Process,
            attributes: FunctionAttributes::default(),
            params: process_function_params(i32_ty),
            results: Vec::new(),
            locals: vec![
                Local {
                    name: None,
                    ty: i32_ty,
                },
                Local {
                    name: None,
                    ty: i32_ty,
                },
            ],
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        };
        process.body.statements.extend([
            statement(StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Load(Place {
                    base: PlaceBase::Parameter(ParameterId::new(
                        crate::PROCESS_FRAMES_PARAM_INDEX as u32,
                    )),
                    projections: Vec::new(),
                }),
            }),
            statement(StatementKind::Assign {
                destination: Place::local(LocalId::new(1)),
                value: Rvalue::Binary {
                    op: BinaryOp::Add,
                    lhs: Value::Local(LocalId::new(0)),
                    rhs: Value::Constant(ScalarValue::I32(1)),
                },
            }),
        ]);
        let init = Function {
            name: "init".to_owned(),
            kind: FunctionKind::Init,
            attributes: FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        };
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.push(Type::Scalar(ScalarType::I32));
        program.functions = vec![init, process];

        let ranges = analyze_integer_ranges(&program, FunctionId::new(1));
        assert_eq!(
            ranges.parameter(ParameterId::new(crate::PROCESS_FRAMES_PARAM_INDEX as u32)),
            IntegerRange::new(ScalarType::I32, 0, 64)
        );
        assert_eq!(
            ranges.local(LocalId::new(1)),
            IntegerRange::new(ScalarType::I32, 1, 65)
        );
    }
}
