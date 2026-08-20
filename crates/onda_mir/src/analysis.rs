//! Backend-neutral semantic facts derived from validated MIR.
//!
//! These analyses deliberately describe logical effects rather than a target
//! ABI.  Optimizers and backends can therefore make the same decisions about
//! calls without reverse-engineering pointer provenance from lowered code.

use std::collections::HashSet;
use std::fmt;

use crate::{
    BinaryOp, Block, BoundsMode, BufferId, CallArgument, Function, FunctionId, FunctionKind,
    LocalId, ParameterId, Place, PlaceBase, Program, Projection, Rvalue, ScalarType, ScalarValue,
    SliceSource, StatementKind, Type, Value,
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
    /// The function may encounter a checked runtime condition that fails.
    pub may_fail: bool,
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
        scan_block(
            program,
            function,
            &function.body,
            &mut effects,
            &mut function_calls,
        );
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
                if callee.may_fail && !functions[caller_index].may_fail {
                    functions[caller_index].may_fail = true;
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

/// Reachable write effects for the program's declared interface buffers.
///
/// This is distinct from [`crate::AccessMode`]: access mode states what a
/// declaration permits, while this analysis reports whether any init,
/// process, or event entry point can actually write the buffer.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BufferWriteAnalysis {
    may_write: Vec<bool>,
}

impl BufferWriteAnalysis {
    pub fn may_write(&self, buffer: BufferId) -> bool {
        self.may_write.get(buffer.index()).copied().unwrap_or(false)
    }

    pub fn buffers(&self) -> &[bool] {
        &self.may_write
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BufferWriteAnalysisError {
    message: String,
}

impl BufferWriteAnalysisError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for BufferWriteAnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BufferWriteAnalysisError {}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
enum ResourceOrigin {
    Buffer(usize),
    Parameter(ParameterSlot),
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
struct ParameterSlot {
    parameter: usize,
    slot: usize,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct FunctionBufferWriteSummary {
    buffers: HashSet<usize>,
    parameters: HashSet<ParameterSlot>,
}

/// Computes per-buffer writes reachable from every externally callable MIR
/// entry point, including writes through calls, buffer parameters, and slices.
pub fn analyze_buffer_writes(
    program: &Program,
) -> Result<BufferWriteAnalysis, BufferWriteAnalysisError> {
    let mut summaries = vec![FunctionBufferWriteSummary::default(); program.functions.len()];
    loop {
        let previous = summaries.clone();
        let mut changed = false;
        for (function_index, function) in program.functions.iter().enumerate() {
            let aliases = infer_local_resource_aliases(program, function);
            let unsupported_results = unsupported_resource_call_results(program, function);
            let mut next = FunctionBufferWriteSummary::default();
            collect_block_resource_writes(
                program,
                function,
                &function.body,
                &aliases,
                &unsupported_results,
                &previous,
                &mut next,
            )?;
            if next != summaries[function_index] {
                summaries[function_index] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut roots = vec![
        program.entry_points.init.index(),
        program.entry_points.process.index(),
    ];
    roots.extend(
        program
            .interface
            .events
            .iter()
            .map(|event| event.handler.index()),
    );

    let mut may_write = vec![false; program.interface.buffers.len()];
    for root in roots {
        let summary = summaries.get(root).ok_or_else(|| {
            BufferWriteAnalysisError::new(format!(
                "MIR buffer-write root function {root} is missing"
            ))
        })?;
        for buffer in &summary.buffers {
            let Some(effect) = may_write.get_mut(*buffer) else {
                return Err(BufferWriteAnalysisError::new(format!(
                    "MIR buffer-write analysis references missing buffer {buffer}"
                )));
            };
            *effect = true;
        }
    }

    Ok(BufferWriteAnalysis { may_write })
}

fn infer_local_resource_aliases(
    program: &Program,
    function: &Function,
) -> Vec<HashSet<ResourceOrigin>> {
    let mut aliases = vec![HashSet::new(); function.locals.len()];
    loop {
        let previous = aliases.clone();
        collect_block_aliases(program, function, &function.body, &previous, &mut aliases);
        if aliases == previous {
            return aliases;
        }
    }
}

fn collect_block_aliases(
    program: &Program,
    function: &Function,
    block: &Block,
    previous: &[HashSet<ResourceOrigin>],
    aliases: &mut [HashSet<ResourceOrigin>],
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
                aliases[local.index()]
                    .extend(rvalue_resource_origins(program, function, value, previous));
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_aliases(program, function, then_block, previous, aliases);
                collect_block_aliases(program, function, else_block, previous, aliases);
            }
            StatementKind::Loop { body } => {
                collect_block_aliases(program, function, body, previous, aliases)
            }
            _ => {}
        }
    }
}

fn rvalue_resource_origins(
    program: &Program,
    function: &Function,
    value: &Rvalue,
    aliases: &[HashSet<ResourceOrigin>],
) -> HashSet<ResourceOrigin> {
    match value {
        Rvalue::Use(value) => value_resource_origins(*value, aliases),
        Rvalue::Load(place) => place_resource_origins(place, aliases),
        Rvalue::MakeSlice { source, .. } => match source {
            SliceSource::Buffer { buffer, .. } => buffer_ref_resource_origins(*buffer),
            SliceSource::BufferParam { parameter, .. } => {
                buffer_param_resource_origins(program, function, *parameter)
            }
            SliceSource::Place(place) => place_resource_origins(place, aliases),
            SliceSource::ConstData(_) => HashSet::new(),
        },
        _ => HashSet::new(),
    }
}

fn value_resource_origins(
    value: Value,
    aliases: &[HashSet<ResourceOrigin>],
) -> HashSet<ResourceOrigin> {
    match value {
        Value::Local(local) => aliases.get(local.index()).cloned().unwrap_or_default(),
        Value::Constant(_) => HashSet::new(),
    }
}

fn place_resource_origins(
    place: &Place,
    aliases: &[HashSet<ResourceOrigin>],
) -> HashSet<ResourceOrigin> {
    match place.base {
        PlaceBase::Parameter(parameter) => {
            HashSet::from([ResourceOrigin::Parameter(ParameterSlot {
                parameter: parameter.index(),
                slot: 0,
            })])
        }
        PlaceBase::Local(local) => aliases.get(local.index()).cloned().unwrap_or_default(),
        PlaceBase::State(_) | PlaceBase::Param(_) | PlaceBase::EventParam(_) => HashSet::new(),
    }
}

fn unsupported_resource_call_results(program: &Program, function: &Function) -> HashSet<usize> {
    let mut locals = HashSet::new();
    collect_unsupported_resource_call_results(program, function, &function.body, &mut locals);
    locals
}

fn collect_unsupported_resource_call_results(
    program: &Program,
    function: &Function,
    block: &Block,
    locals: &mut HashSet<usize>,
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Call { results, .. } => {
                for result in results {
                    let Some(local) = function.locals.get(result.index()) else {
                        continue;
                    };
                    if matches!(
                        program.types.get(local.ty.index()),
                        Some(Type::Slice { .. } | Type::Buffer { .. })
                    ) {
                        locals.insert(result.index());
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_unsupported_resource_call_results(program, function, then_block, locals);
                collect_unsupported_resource_call_results(program, function, else_block, locals);
            }
            StatementKind::Loop { body } => {
                collect_unsupported_resource_call_results(program, function, body, locals);
            }
            _ => {}
        }
    }
}

fn collect_block_resource_writes(
    program: &Program,
    function: &Function,
    block: &Block,
    aliases: &[HashSet<ResourceOrigin>],
    unsupported_results: &HashSet<usize>,
    summaries: &[FunctionBufferWriteSummary],
    output: &mut FunctionBufferWriteSummary,
) -> Result<(), BufferWriteAnalysisError> {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, .. } => {
                if let PlaceBase::Parameter(parameter) = destination.base {
                    output.parameters.insert(ParameterSlot {
                        parameter: parameter.index(),
                        slot: 0,
                    });
                }
            }
            StatementKind::BufferStore { buffer, .. } => {
                mark_resource_origins(buffer_ref_resource_origins(*buffer), output);
            }
            StatementKind::BufferParamStore { parameter, .. } => {
                mark_resource_origins(
                    buffer_param_resource_origins(program, function, *parameter),
                    output,
                );
            }
            StatementKind::SliceStore { slice, .. } => {
                mark_value_resource_write(
                    *slice,
                    aliases,
                    unsupported_results,
                    "slice store",
                    output,
                )?;
            }
            StatementKind::SliceFill { destination, .. }
            | StatementKind::SliceCopy { destination, .. } => {
                mark_value_resource_write(
                    *destination,
                    aliases,
                    unsupported_results,
                    "slice write",
                    output,
                )?;
            }
            StatementKind::Call {
                function: callee,
                args,
                ..
            } => {
                let callee_summary = summaries.get(callee.index()).ok_or_else(|| {
                    BufferWriteAnalysisError::new(format!(
                        "MIR call references missing function {}",
                        callee.raw()
                    ))
                })?;
                output
                    .buffers
                    .extend(callee_summary.buffers.iter().copied());
                for parameter in &callee_summary.parameters {
                    let argument = args.get(parameter.parameter).ok_or_else(|| {
                        BufferWriteAnalysisError::new(format!(
                            "MIR call to function {} has no argument for writable parameter {}",
                            callee.raw(),
                            parameter.parameter
                        ))
                    })?;
                    if call_argument_uses_unsupported_result(argument, unsupported_results) {
                        return Err(BufferWriteAnalysisError::new(
                            "cannot infer interface-buffer writes through a slice or buffer returned by a MIR call",
                        ));
                    }
                    mark_resource_origins(
                        call_argument_resource_origins(
                            program,
                            function,
                            argument,
                            aliases,
                            parameter.slot,
                        ),
                        output,
                    );
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_resource_writes(
                    program,
                    function,
                    then_block,
                    aliases,
                    unsupported_results,
                    summaries,
                    output,
                )?;
                collect_block_resource_writes(
                    program,
                    function,
                    else_block,
                    aliases,
                    unsupported_results,
                    summaries,
                    output,
                )?;
            }
            StatementKind::Loop { body } => collect_block_resource_writes(
                program,
                function,
                body,
                aliases,
                unsupported_results,
                summaries,
                output,
            )?,
            _ => {}
        }
    }
    Ok(())
}

fn mark_value_resource_write(
    value: Value,
    aliases: &[HashSet<ResourceOrigin>],
    unsupported_results: &HashSet<usize>,
    context: &str,
    output: &mut FunctionBufferWriteSummary,
) -> Result<(), BufferWriteAnalysisError> {
    if let Value::Local(local) = value {
        if unsupported_results.contains(&local.index()) {
            return Err(BufferWriteAnalysisError::new(format!(
                "cannot infer interface-buffer writes for {context} through a slice returned by a MIR call"
            )));
        }
    }
    mark_resource_origins(value_resource_origins(value, aliases), output);
    Ok(())
}

fn call_argument_resource_origins(
    program: &Program,
    function: &Function,
    argument: &CallArgument,
    aliases: &[HashSet<ResourceOrigin>],
    slot: usize,
) -> HashSet<ResourceOrigin> {
    match argument {
        CallArgument::Buffer(buffer) => buffer_ref_resource_origins(*buffer),
        CallArgument::BufferParam(parameter) => {
            buffer_param_resource_origins(program, function, *parameter)
        }
        CallArgument::BufferSpan(span) => match span {
            crate::BufferSpanRef::Interface { first, len } if slot < *len as usize => {
                HashSet::from([ResourceOrigin::Buffer(first.index().saturating_add(slot))])
            }
            crate::BufferSpanRef::Parameter {
                span, start, len, ..
            } if slot < *len as usize => {
                HashSet::from([ResourceOrigin::Parameter(ParameterSlot {
                    parameter: span.index(),
                    slot: (*start as usize).saturating_add(slot),
                })])
            }
            crate::BufferSpanRef::Interface { first, len } => (first.index()
                ..first.index().saturating_add(*len as usize))
                .map(ResourceOrigin::Buffer)
                .collect(),
            crate::BufferSpanRef::Parameter { span, start, len } => (*start as usize
                ..(*start as usize).saturating_add(*len as usize))
                .map(|slot| {
                    ResourceOrigin::Parameter(ParameterSlot {
                        parameter: span.index(),
                        slot,
                    })
                })
                .collect(),
        },
        CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => {
            place_resource_origins(place, aliases)
        }
        CallArgument::Value(value)
        | CallArgument::SliceElement { slice: value, .. }
        | CallArgument::SliceWindow { slice: value, .. } => value_resource_origins(*value, aliases),
    }
}

fn call_argument_uses_unsupported_result(
    argument: &CallArgument,
    unsupported_results: &HashSet<usize>,
) -> bool {
    let value = match argument {
        CallArgument::Value(value)
        | CallArgument::SliceElement { slice: value, .. }
        | CallArgument::SliceWindow { slice: value, .. } => Some(*value),
        CallArgument::Place(Place {
            base: PlaceBase::Local(local),
            ..
        })
        | CallArgument::ArrayWindow {
            array:
                Place {
                    base: PlaceBase::Local(local),
                    ..
                },
            ..
        } => return unsupported_results.contains(&local.index()),
        CallArgument::Place(_)
        | CallArgument::ArrayWindow { .. }
        | CallArgument::Buffer(_)
        | CallArgument::BufferParam(_)
        | CallArgument::BufferSpan(_) => None,
    };
    matches!(value, Some(Value::Local(local)) if unsupported_results.contains(&local.index()))
}

fn mark_resource_origins(
    origins: HashSet<ResourceOrigin>,
    output: &mut FunctionBufferWriteSummary,
) {
    for origin in origins {
        match origin {
            ResourceOrigin::Buffer(buffer) => {
                output.buffers.insert(buffer);
            }
            ResourceOrigin::Parameter(parameter) => {
                output.parameters.insert(parameter);
            }
        }
    }
}

fn buffer_ref_resource_origins(buffer: crate::BufferRef) -> HashSet<ResourceOrigin> {
    match buffer {
        crate::BufferRef::Direct(buffer) => HashSet::from([ResourceOrigin::Buffer(buffer.index())]),
        crate::BufferRef::ArrayElement {
            first,
            len,
            selector,
            bounds,
        } => selected_slots(selector, len as usize, bounds)
            .into_iter()
            .map(|slot| ResourceOrigin::Buffer(first.index().saturating_add(slot)))
            .collect(),
    }
}

fn buffer_param_resource_origins(
    program: &Program,
    function: &Function,
    parameter: crate::BufferParamRef,
) -> HashSet<ResourceOrigin> {
    match parameter {
        crate::BufferParamRef::Direct(parameter) => {
            HashSet::from([ResourceOrigin::Parameter(ParameterSlot {
                parameter: parameter.index(),
                slot: 0,
            })])
        }
        crate::BufferParamRef::ArrayElement {
            span,
            selector,
            bounds,
        } => {
            let len = function
                .params
                .get(span.index())
                .and_then(|parameter| program.types.get(parameter.ty.index()))
                .and_then(|ty| match ty {
                    Type::BufferSpan { len, .. } => Some(*len as usize),
                    _ => None,
                })
                .unwrap_or(1);
            selected_slots(selector, len, bounds)
                .into_iter()
                .map(|slot| {
                    ResourceOrigin::Parameter(ParameterSlot {
                        parameter: span.index(),
                        slot,
                    })
                })
                .collect()
        }
    }
}

fn selected_slots(selector: Value, len: usize, bounds: crate::BoundsMode) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let exact = match selector {
        Value::Constant(ScalarValue::I32(selector)) => match bounds {
            crate::BoundsMode::Clamp => {
                let maximum = i32::try_from(len - 1).unwrap_or(i32::MAX);
                Some(selector.clamp(0, maximum) as usize)
            }
            crate::BoundsMode::Checked | crate::BoundsMode::Unchecked => usize::try_from(selector)
                .ok()
                .filter(|selector| *selector < len),
        },
        _ => None,
    };
    exact.map_or_else(|| (0..len).collect(), |slot| vec![slot])
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
    let mut environment = function
        .locals
        .iter()
        .map(|local| local.integer_range.and_then(integer_range_from_invariant))
        .collect::<Vec<_>>();
    let mut summary = environment
        .iter()
        .map(|range| RangeSummary {
            seen: range.is_some(),
            range: *range,
        })
        .collect::<Vec<_>>();
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
    for (range, parameter) in ranges.iter_mut().zip(&function.params) {
        *range = parameter
            .integer_range
            .and_then(integer_range_from_invariant);
    }
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
                let range = function.locals[local.index()]
                    .integer_range
                    .and_then(integer_range_from_invariant)
                    .or_else(|| range_of_rvalue(program, function, value, parameters, environment));
                environment[local.index()] = range;
                record_range(&mut summary[local.index()], range);
            }
            StatementKind::Call {
                results,
                function: callee,
                args,
            } => {
                for result in results {
                    let range = function.locals[result.index()]
                        .integer_range
                        .and_then(integer_range_from_invariant);
                    environment[result.index()] = range;
                    record_range(&mut summary[result.index()], range);
                }
                for (index, argument) in args.iter().enumerate() {
                    if program.functions[callee.index()].params[index].mode
                        == crate::PassingMode::ReadWriteReference
                    {
                        if let Some(local) = argument_local(argument) {
                            let range = function.locals[local.index()]
                                .integer_range
                                .and_then(integer_range_from_invariant);
                            environment[local.index()] = range;
                            record_range(&mut summary[local.index()], range);
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
                    body_environment[local.index()] = function.locals[local.index()]
                        .integer_range
                        .and_then(integer_range_from_invariant);
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
                    environment[local.index()] = function.locals[local.index()]
                        .integer_range
                        .and_then(integer_range_from_invariant);
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
            PlaceBase::Local(local) => environment.get(local.index()).copied().flatten(),
            PlaceBase::Parameter(parameter) => parameters.get(parameter.index()).copied().flatten(),
            // Interface parameter storage contains raw host values. Ranged parameters only
            // acquire their invariant after the generated entry-point normalization.
            PlaceBase::Param(_) => None,
            PlaceBase::State(state) => program
                .state
                .get(state.index())
                .and_then(|slot| slot.integer_range)
                .and_then(integer_range_from_invariant),
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
                (ScalarType::I64, ScalarType::I32) => {
                    IntegerRange::new(*to, source.min, source.max)
                        .or_else(|| IntegerRange::full(*to))
                }
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
            intrinsic: crate::Intrinsic::RangeClamp | crate::Intrinsic::RangeWrap,
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

fn integer_range_from_invariant(range: crate::IntegerRangeInvariant) -> Option<IntegerRange> {
    integer_range_from_value_range(crate::ValueRange {
        min: range.min,
        max: range.max,
    })
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

fn scan_block(
    program: &Program,
    function: &crate::Function,
    block: &Block,
    effects: &mut FunctionEffects,
    calls: &mut Vec<CallSite>,
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                scan_place(destination, Access::Write, effects);
                scan_rvalue(program, function, value, effects);
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
                mark_checked_bounds(*bounds, effects);
            }
            StatementKind::ControlOutputStore {
                element, bounds, ..
            } => {
                effects.writes.insert(MemoryRegionSet::CONTROL_OUTPUTS);
                scan_optional_value(*element, effects);
                mark_checked_bounds(*bounds, effects);
            }
            StatementKind::BufferStore {
                buffer,
                channel,
                index,
                bounds,
                ..
            } => {
                effects.writes.insert(MemoryRegionSet::BUFFERS);
                scan_buffer_ref(*buffer, effects);
                scan_optional_value(*channel, effects);
                scan_value(*index, effects);
                mark_checked_bounds(*bounds, effects);
            }
            StatementKind::BufferParamStore {
                parameter,
                channel,
                index,
                bounds,
                ..
            } => {
                mark_buffer_param_ref(*parameter, Access::Read, effects);
                effects.writes.insert(MemoryRegionSet::INDIRECT);
                scan_optional_value(*channel, effects);
                scan_value(*index, effects);
                mark_checked_bounds(*bounds, effects);
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
                mark_dynamic_bounds(*bounds, effects);
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
                effects.may_fail = true;
            }
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                scan_value(*condition, effects);
                scan_block(program, function, then_block, effects, calls);
                scan_block(program, function, else_block, effects, calls);
            }
            StatementKind::Loop { body } => {
                effects.may_not_return = true;
                scan_block(program, function, body, effects, calls);
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

fn scan_rvalue(
    program: &Program,
    function: &crate::Function,
    value: &Rvalue,
    effects: &mut FunctionEffects,
) {
    match value {
        Rvalue::Use(value) | Rvalue::SliceLen(value) => scan_value(*value, effects),
        Rvalue::Load(place) => scan_place(place, Access::Read, effects),
        Rvalue::Unary { operand, .. } => scan_value(*operand, effects),
        Rvalue::Binary { op, lhs, rhs } => {
            scan_value(*lhs, effects);
            scan_value(*rhs, effects);
            if matches!(op, crate::BinaryOp::Divide | crate::BinaryOp::Remainder)
                && matches!(
                    value_scalar_type(program, function, *lhs),
                    Some(ScalarType::I32 | ScalarType::I64)
                )
            {
                effects.may_fail = true;
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
            effects.may_fail = true;
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
            mark_checked_bounds(*bounds, effects);
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
            mark_checked_bounds(*bounds, effects);
        }
        Rvalue::BufferLoad {
            buffer,
            channel,
            index,
            bounds,
            ..
        } => {
            effects.reads.insert(MemoryRegionSet::BUFFERS);
            scan_buffer_ref(*buffer, effects);
            scan_optional_value(*channel, effects);
            scan_value(*index, effects);
            mark_checked_bounds(*bounds, effects);
        }
        Rvalue::BufferParamLoad {
            parameter,
            channel,
            index,
            bounds,
        } => {
            mark_buffer_param_ref(*parameter, Access::Read, effects);
            effects.reads.insert(MemoryRegionSet::INDIRECT);
            scan_optional_value(*channel, effects);
            scan_value(*index, effects);
            mark_checked_bounds(*bounds, effects);
        }
        Rvalue::BufferLen(buffer)
        | Rvalue::BufferChannels(buffer)
        | Rvalue::BufferSampleRate(buffer) => {
            effects.reads.insert(MemoryRegionSet::BUFFERS);
            scan_buffer_ref(*buffer, effects);
        }
        Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter) => {
            mark_buffer_param_ref(*parameter, Access::Read, effects);
        }
        Rvalue::ConstDataLoad { index, bounds, .. } => {
            effects.reads.insert(MemoryRegionSet::CONST_DATA);
            scan_value(*index, effects);
            mark_checked_bounds(*bounds, effects);
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
            mark_checked_bounds(*bounds, effects);
        }
        Rvalue::SliceLoad {
            slice,
            index,
            bounds,
        } => {
            effects.reads.insert(MemoryRegionSet::INDIRECT);
            scan_value(*slice, effects);
            scan_value(*index, effects);
            mark_dynamic_bounds(*bounds, effects);
        }
    }
}

fn scan_slice_source(source: &SliceSource, effects: &mut FunctionEffects) {
    match source {
        SliceSource::Place(place) => scan_place(place, Access::Read, effects),
        SliceSource::Buffer { buffer, channel } => {
            effects.reads.insert(MemoryRegionSet::BUFFERS);
            scan_buffer_ref(*buffer, effects);
            scan_optional_value(*channel, effects);
        }
        SliceSource::BufferParam { parameter, channel } => {
            mark_buffer_param_ref(*parameter, Access::Read, effects);
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
            mark_dynamic_bounds(*bounds, effects);
        }
        CallArgument::ArrayWindow {
            array,
            start,
            bounds,
        } => {
            scan_place_indices(array, effects);
            scan_value(*start, effects);
            mark_checked_bounds(*bounds, effects);
        }
        CallArgument::SliceWindow {
            slice,
            start,
            bounds,
        } => {
            scan_value(*slice, effects);
            scan_value(*start, effects);
            mark_dynamic_bounds(*bounds, effects);
        }
        CallArgument::Buffer(buffer) => {
            effects.reads.insert(MemoryRegionSet::BUFFERS);
            scan_buffer_ref(*buffer, effects);
        }
        CallArgument::BufferParam(parameter) => {
            mark_buffer_param_ref(*parameter, Access::Read, effects);
        }
        CallArgument::BufferSpan(span) => match span {
            crate::BufferSpanRef::Interface { .. } => {
                effects.reads.insert(MemoryRegionSet::BUFFERS);
            }
            crate::BufferSpanRef::Parameter { span, .. } => {
                effects.reads.insert(MemoryRegionSet::ARGUMENTS);
                if let Some(parameter) = effects.parameters.get_mut(span.index()) {
                    parameter.reads = true;
                }
            }
        },
    }
}

fn scan_buffer_ref(buffer: crate::BufferRef, effects: &mut FunctionEffects) {
    if let crate::BufferRef::ArrayElement {
        selector, bounds, ..
    } = buffer
    {
        scan_value(selector, effects);
        mark_dynamic_bounds(bounds, effects);
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
            mark_checked_bounds(*bounds, effects);
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

fn mark_buffer_param_ref(
    parameter: crate::BufferParamRef,
    access: Access,
    effects: &mut FunctionEffects,
) {
    for index in parameter.possible_indices() {
        mark_parameter(ParameterId::new(index as u32), access, effects);
    }
    if let crate::BufferParamRef::ArrayElement {
        selector, bounds, ..
    } = parameter
    {
        scan_value(selector, effects);
        mark_checked_bounds(bounds, effects);
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
    if let CallArgument::BufferParam(parameter) = argument {
        let mut changed = false;
        if access.reads {
            changed |= caller.reads.insert(MemoryRegionSet::ARGUMENTS);
        }
        if access.writes {
            changed |= caller.writes.insert(MemoryRegionSet::ARGUMENTS);
        }
        for index in parameter.possible_indices() {
            if let Some(parameter_effects) = caller.parameters.get_mut(index) {
                changed |= parameter_effects.merge(access);
            }
        }
        return changed;
    }
    if let CallArgument::BufferSpan(crate::BufferSpanRef::Parameter { span, .. }) = argument {
        let mut changed = false;
        if access.reads {
            changed |= caller.reads.insert(MemoryRegionSet::ARGUMENTS);
        }
        if access.writes {
            changed |= caller.writes.insert(MemoryRegionSet::ARGUMENTS);
        }
        if let Some(parameter_effects) = caller.parameters.get_mut(span.index()) {
            changed |= parameter_effects.merge(access);
        }
        return changed;
    }
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
        CallArgument::BufferParam(_) => unreachable!("buffer parameter arguments return above"),
        CallArgument::BufferSpan(crate::BufferSpanRef::Interface { .. }) => {
            (MemoryRegionSet::BUFFERS, None)
        }
        CallArgument::BufferSpan(crate::BufferSpanRef::Parameter { .. }) => {
            unreachable!("buffer span parameter arguments return above")
        }
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

fn mark_checked_bounds(bounds: BoundsMode, effects: &mut FunctionEffects) {
    if bounds == BoundsMode::Checked {
        effects.may_fail = true;
    }
}

fn mark_dynamic_bounds(bounds: BoundsMode, effects: &mut FunctionEffects) {
    if bounds != BoundsMode::Unchecked {
        effects.may_fail = true;
    }
}

fn value_scalar_type(
    program: &Program,
    function: &crate::Function,
    value: Value,
) -> Option<ScalarType> {
    match value {
        Value::Constant(value) => Some(value.ty()),
        Value::Local(local) => {
            let ty = function.locals.get(local.index())?.ty;
            match program.types.get(ty.index())? {
                crate::Type::Scalar(scalar) => Some(*scalar),
                _ => None,
            }
        }
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
            integer_range: None,
            name: "value".to_owned(),
            ty: scalar,
            mode: crate::PassingMode::ReadWriteReference,
        };
        let aggregate_reference = crate::FunctionParam {
            integer_range: None,
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
                integer_range: None,
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
            integer_range: None,
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
    fn buffer_writes_follow_entry_points_calls_and_slice_aliases() {
        let i32_ty = TypeId::new(1);
        let buffer_ty = TypeId::new(2);
        let slice_ty = TypeId::new(3);

        let mut init = function("init", Vec::new(), Block::default());
        init.kind = FunctionKind::Init;

        let mut process = function(
            "process",
            process_function_params(i32_ty),
            Block {
                statements: vec![
                    statement(StatementKind::Assign {
                        destination: Place::local(LocalId::new(0)),
                        value: Rvalue::MakeSlice {
                            source: SliceSource::Buffer {
                                buffer: crate::BufferRef::Direct(BufferId::new(1)),
                                channel: None,
                            },
                            start: Value::Constant(ScalarValue::I32(0)),
                            len: Value::Constant(ScalarValue::I32(1)),
                            bounds: BoundsMode::Clamp,
                            access: AccessMode::ReadWrite,
                        },
                    }),
                    statement(StatementKind::Call {
                        results: Vec::new(),
                        function: FunctionId::new(2),
                        args: vec![CallArgument::Buffer(crate::BufferRef::Direct(
                            BufferId::new(0),
                        ))],
                    }),
                    statement(StatementKind::Call {
                        results: Vec::new(),
                        function: FunctionId::new(3),
                        args: vec![CallArgument::Value(Value::Local(LocalId::new(0)))],
                    }),
                ],
            },
        );
        process.kind = FunctionKind::Process;
        process.locals.push(Local {
            integer_range: None,
            name: Some("buffer_slice".to_owned()),
            ty: slice_ty,
        });

        let write_buffer = function(
            "write_buffer",
            vec![crate::FunctionParam {
                integer_range: None,
                name: "buffer".to_owned(),
                ty: buffer_ty,
                mode: crate::PassingMode::ReadWriteReference,
            }],
            Block {
                statements: vec![statement(StatementKind::BufferParamStore {
                    parameter: crate::BufferParamRef::Direct(ParameterId::new(0)),
                    channel: None,
                    index: Value::Constant(ScalarValue::I32(0)),
                    value: Value::Constant(ScalarValue::F32(1.0)),
                    bounds: BoundsMode::Clamp,
                })],
            },
        );

        let mut write_slice = function(
            "write_slice",
            vec![crate::FunctionParam {
                integer_range: None,
                name: "slice".to_owned(),
                ty: slice_ty,
                mode: crate::PassingMode::Value,
            }],
            Block {
                statements: vec![
                    statement(StatementKind::Assign {
                        destination: Place::local(LocalId::new(0)),
                        value: Rvalue::Load(Place {
                            base: PlaceBase::Parameter(ParameterId::new(0)),
                            projections: Vec::new(),
                        }),
                    }),
                    statement(StatementKind::SliceStore {
                        slice: Value::Local(LocalId::new(0)),
                        index: Value::Constant(ScalarValue::I32(0)),
                        value: Value::Constant(ScalarValue::F32(0.5)),
                        bounds: BoundsMode::Clamp,
                    }),
                ],
            },
        );
        write_slice.locals.push(Local {
            integer_range: None,
            name: Some("slice_alias".to_owned()),
            ty: slice_ty,
        });

        let unreachable_write = function(
            "unreachable_write",
            Vec::new(),
            Block {
                statements: vec![statement(StatementKind::BufferStore {
                    buffer: crate::BufferRef::Direct(BufferId::new(2)),
                    channel: None,
                    index: Value::Constant(ScalarValue::I32(0)),
                    value: Value::Constant(ScalarValue::F32(1.0)),
                    bounds: BoundsMode::Clamp,
                })],
            },
        );

        let mut event = function(
            "event",
            Vec::new(),
            Block {
                statements: vec![statement(StatementKind::BufferStore {
                    buffer: crate::BufferRef::Direct(BufferId::new(3)),
                    channel: None,
                    index: Value::Constant(ScalarValue::I32(0)),
                    value: Value::Constant(ScalarValue::F32(1.0)),
                    bounds: BoundsMode::Clamp,
                })],
            },
        );
        event.kind = FunctionKind::Event(crate::EventId::new(0));

        let mut program = Program::new(
            CompileConfig::new(48_000.0, 64).expect("valid test config"),
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types = vec![
            Type::Scalar(ScalarType::F32),
            Type::Scalar(ScalarType::I32),
            Type::Buffer {
                element: ScalarType::F32,
                channels: crate::BufferChannels::Mono,
                access: AccessMode::ReadWrite,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadWrite,
            },
        ];
        program.interface.buffers = (0..5)
            .map(|index| crate::Buffer {
                name: format!("buffer_{index}"),
                element: ScalarType::F32,
                channels: crate::BufferChannels::Mono,
                access: AccessMode::ReadWrite,
            })
            .collect();
        program.interface.events.push(crate::Event {
            name: "event".to_owned(),
            params: Vec::new(),
            handler: FunctionId::new(5),
        });
        program.functions = vec![
            init,
            process,
            write_buffer,
            write_slice,
            unreachable_write,
            event,
        ];

        let effects =
            analyze_buffer_writes(&program).expect("buffer-write analysis should succeed");
        assert!(effects.may_write(BufferId::new(0)));
        assert!(effects.may_write(BufferId::new(1)));
        assert!(!effects.may_write(BufferId::new(2)));
        assert!(effects.may_write(BufferId::new(3)));
        assert!(!effects.may_write(BufferId::new(4)));
    }

    #[test]
    fn buffer_writes_narrow_constant_collection_slots_and_widen_unknown_selectors() {
        let i32_ty = TypeId::new(1);
        let mut init = function("init", Vec::new(), Block::default());
        init.kind = FunctionKind::Init;
        let mut process = function(
            "process",
            process_function_params(i32_ty),
            Block {
                statements: vec![statement(StatementKind::BufferStore {
                    buffer: crate::BufferRef::ArrayElement {
                        first: BufferId::new(0),
                        len: 4,
                        selector: Value::Constant(ScalarValue::I32(2)),
                        bounds: BoundsMode::Clamp,
                    },
                    channel: None,
                    index: Value::Constant(ScalarValue::I32(0)),
                    value: Value::Constant(ScalarValue::F32(1.0)),
                    bounds: BoundsMode::Clamp,
                })],
            },
        );
        process.kind = FunctionKind::Process;

        let mut program = Program::new(
            CompileConfig::new(48_000.0, 64).expect("valid test config"),
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types = vec![Type::Scalar(ScalarType::F32), Type::Scalar(ScalarType::I32)];
        program.interface.buffers = (0..4)
            .map(|index| crate::Buffer {
                name: format!("bank[{index}]"),
                element: ScalarType::F32,
                channels: crate::BufferChannels::Mono,
                access: AccessMode::ReadWrite,
            })
            .collect();
        program.functions = vec![init, process];

        let exact = analyze_buffer_writes(&program).expect("analysis should succeed");
        assert_eq!(exact.buffers(), &[false, false, true, false]);

        let process = &mut program.functions[1];
        process.locals.push(Local {
            integer_range: None,
            name: Some("selector".to_owned()),
            ty: i32_ty,
        });
        process.body.statements.insert(
            0,
            statement(StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Load(Place {
                    base: PlaceBase::Parameter(ParameterId::new(0)),
                    projections: Vec::new(),
                }),
            }),
        );
        let StatementKind::BufferStore { buffer, .. } = &mut process.body.statements[1].kind else {
            panic!("expected buffer store")
        };
        let crate::BufferRef::ArrayElement { selector, .. } = buffer else {
            panic!("expected buffer collection selection")
        };
        *selector = Value::Local(LocalId::new(0));

        let unknown = analyze_buffer_writes(&program).expect("analysis should succeed");
        assert_eq!(unknown.buffers(), &[true, true, true, true]);
    }

    #[test]
    fn buffer_writes_translate_slots_through_nested_collection_subspans() {
        let i32_ty = TypeId::new(1);
        let parent_span_ty = TypeId::new(2);
        let child_span_ty = TypeId::new(3);
        let mut init = function("init", Vec::new(), Block::default());
        init.kind = FunctionKind::Init;
        let mut process = function(
            "process",
            process_function_params(i32_ty),
            Block {
                statements: vec![statement(StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(2),
                    args: vec![CallArgument::BufferSpan(crate::BufferSpanRef::Interface {
                        first: BufferId::new(0),
                        len: 4,
                    })],
                })],
            },
        );
        process.kind = FunctionKind::Process;
        let parent = function(
            "parent",
            vec![crate::FunctionParam {
                integer_range: None,
                name: "bank".to_owned(),
                ty: parent_span_ty,
                mode: crate::PassingMode::Value,
            }],
            Block {
                statements: vec![statement(StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(3),
                    args: vec![CallArgument::BufferSpan(crate::BufferSpanRef::Parameter {
                        span: ParameterId::new(0),
                        start: 1,
                        len: 2,
                    })],
                })],
            },
        );
        let child = function(
            "child",
            vec![crate::FunctionParam {
                integer_range: None,
                name: "clips".to_owned(),
                ty: child_span_ty,
                mode: crate::PassingMode::Value,
            }],
            Block {
                statements: vec![statement(StatementKind::BufferParamStore {
                    parameter: crate::BufferParamRef::ArrayElement {
                        span: ParameterId::new(0),
                        selector: Value::Constant(ScalarValue::I32(1)),
                        bounds: BoundsMode::Clamp,
                    },
                    channel: None,
                    index: Value::Constant(ScalarValue::I32(0)),
                    value: Value::Constant(ScalarValue::F32(1.0)),
                    bounds: BoundsMode::Clamp,
                })],
            },
        );

        let mut program = Program::new(
            CompileConfig::new(48_000.0, 64).expect("valid test config"),
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types = vec![
            Type::Scalar(ScalarType::F32),
            Type::Scalar(ScalarType::I32),
            Type::BufferSpan {
                element: ScalarType::F32,
                channels: crate::BufferChannels::Mono,
                access: AccessMode::ReadWrite,
                len: 4,
            },
            Type::BufferSpan {
                element: ScalarType::F32,
                channels: crate::BufferChannels::Mono,
                access: AccessMode::ReadWrite,
                len: 2,
            },
        ];
        program.interface.buffers = (0..4)
            .map(|index| crate::Buffer {
                name: format!("bank[{index}]"),
                element: ScalarType::F32,
                channels: crate::BufferChannels::Mono,
                access: AccessMode::ReadWrite,
            })
            .collect();
        program.functions = vec![init, process, parent, child];

        let effects = analyze_buffer_writes(&program).expect("analysis should succeed");
        assert_eq!(effects.buffers(), &[false, false, true, false]);
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
            integer_range: None,
            name: "value".to_owned(),
            ty: TypeId::new(0),
            mode: crate::PassingMode::Value,
        });
        pure.locals.push(Local {
            integer_range: None,
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
    fn failure_effects_match_lowered_runtime_checks() {
        let f32_ty = TypeId::new(0);
        let i32_ty = TypeId::new(1);
        let array_ty = TypeId::new(2);
        let slice_ty = TypeId::new(3);
        let expression_function = |name: &str, ty: TypeId, value: Rvalue| {
            let mut function = function(name, Vec::new(), Block::default());
            function.locals.push(Local {
                integer_range: None,
                name: None,
                ty,
            });
            function
                .body
                .statements
                .push(statement(StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value,
                }));
            function
        };
        let float_divide = expression_function(
            "float_divide",
            f32_ty,
            Rvalue::Binary {
                op: BinaryOp::Divide,
                lhs: Value::Constant(ScalarValue::F32(1.0)),
                rhs: Value::Constant(ScalarValue::F32(2.0)),
            },
        );
        let integer_divide = expression_function(
            "integer_divide",
            i32_ty,
            Rvalue::Binary {
                op: BinaryOp::Divide,
                lhs: Value::Constant(ScalarValue::I32(1)),
                rhs: Value::Constant(ScalarValue::I32(2)),
            },
        );
        let indexed_array = |name: &str, bounds| {
            let mut function = function(name, Vec::new(), Block::default());
            function.locals.extend([
                Local {
                    integer_range: None,
                    name: None,
                    ty: f32_ty,
                },
                Local {
                    integer_range: None,
                    name: None,
                    ty: array_ty,
                },
            ]);
            function
                .body
                .statements
                .push(statement(StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::Load(Place {
                        base: PlaceBase::Local(LocalId::new(1)),
                        projections: vec![Projection::Index {
                            index: Value::Constant(ScalarValue::I32(0)),
                            bounds,
                        }],
                    }),
                }));
            function
        };
        let clamped_array = indexed_array("clamped_array", BoundsMode::Clamp);
        let checked_array = indexed_array("checked_array", BoundsMode::Checked);
        let mut clamped_slice = function("clamped_slice", Vec::new(), Block::default());
        clamped_slice.locals.extend([
            Local {
                integer_range: None,
                name: None,
                ty: f32_ty,
            },
            Local {
                integer_range: None,
                name: None,
                ty: slice_ty,
            },
        ]);
        clamped_slice
            .body
            .statements
            .push(statement(StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::SliceLoad {
                    slice: Value::Local(LocalId::new(1)),
                    index: Value::Constant(ScalarValue::I32(0)),
                    bounds: BoundsMode::Clamp,
                },
            }));
        let call = |name: &str, callee: u32| {
            function(
                name,
                Vec::new(),
                Block {
                    statements: vec![statement(StatementKind::Call {
                        results: Vec::new(),
                        function: FunctionId::new(callee),
                        args: Vec::new(),
                    })],
                },
            )
        };

        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(0),
        );
        program.types = vec![
            Type::Scalar(ScalarType::F32),
            Type::Scalar(ScalarType::I32),
            Type::Array {
                element: f32_ty,
                len: 4,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
        ];
        program.functions = vec![
            float_divide,
            integer_divide,
            clamped_array,
            checked_array,
            clamped_slice,
            call("calls_float_divide", 0),
            call("calls_integer_divide", 1),
        ];

        let analysis = analyze_effects(&program);
        assert!(!analysis.function(FunctionId::new(0)).may_fail);
        assert!(analysis.function(FunctionId::new(1)).may_fail);
        assert!(!analysis.function(FunctionId::new(2)).may_fail);
        assert!(analysis.function(FunctionId::new(3)).may_fail);
        assert!(analysis.function(FunctionId::new(4)).may_fail);
        assert!(!analysis.function(FunctionId::new(5)).may_fail);
        assert!(analysis.function(FunctionId::new(6)).may_fail);
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
                    integer_range: None,
                    name: None,
                    ty: i32_ty,
                },
                Local {
                    integer_range: None,
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
