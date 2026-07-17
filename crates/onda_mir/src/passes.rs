use std::collections::{HashMap, HashSet};
use std::ops::Deref;

use crate::{
    BinaryOp, Block, CallArgument, CompareOp, Function, Intrinsic, LocalId, PassingMode, Place,
    PlaceBase, Projection, Rvalue, ScalarType, ScalarValue, Statement, StatementKind,
    ValidatedProgram, ValidationError, Value,
};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct PassStats {
    pub iterations: u32,
    pub propagated_values: u64,
    pub folded_rvalues: u64,
    pub simplified_branches: u64,
    pub removed_unreachable_statements: u64,
    pub removed_dead_assignments: u64,
    pub removed_redundant_zero_stores: u64,
    pub removed_locals: u64,
}

/// A validated MIR program brought to the backend-neutral optimization fixed
/// point by [`optimize`].
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizedProgram(ValidatedProgram);

impl OptimizedProgram {
    pub fn as_program(&self) -> &crate::Program {
        self.0.as_program()
    }

    pub fn into_validated(self) -> ValidatedProgram {
        self.0
    }

    pub fn into_program(self) -> crate::Program {
        self.0.into_program()
    }
}

impl Deref for OptimizedProgram {
    type Target = crate::Program;

    fn deref(&self) -> &Self::Target {
        self.as_program()
    }
}

impl AsRef<crate::Program> for OptimizedProgram {
    fn as_ref(&self) -> &crate::Program {
        self.as_program()
    }
}

impl PassStats {
    fn merge(&mut self, other: Self) {
        self.iterations = self.iterations.saturating_add(other.iterations);
        self.propagated_values = self
            .propagated_values
            .saturating_add(other.propagated_values);
        self.folded_rvalues = self.folded_rvalues.saturating_add(other.folded_rvalues);
        self.simplified_branches = self
            .simplified_branches
            .saturating_add(other.simplified_branches);
        self.removed_unreachable_statements = self
            .removed_unreachable_statements
            .saturating_add(other.removed_unreachable_statements);
        self.removed_dead_assignments = self
            .removed_dead_assignments
            .saturating_add(other.removed_dead_assignments);
        self.removed_redundant_zero_stores = self
            .removed_redundant_zero_stores
            .saturating_add(other.removed_redundant_zero_stores);
        self.removed_locals = self.removed_locals.saturating_add(other.removed_locals);
    }

    fn changed(self) -> bool {
        self.propagated_values != 0
            || self.folded_rvalues != 0
            || self.simplified_branches != 0
            || self.removed_unreachable_statements != 0
            || self.removed_dead_assignments != 0
            || self.removed_redundant_zero_stores != 0
            || self.removed_locals != 0
    }
}

/// Performs one structured canonicalization round and revalidates the result.
pub fn canonicalize(
    program: ValidatedProgram,
) -> Result<(ValidatedProgram, PassStats), Vec<ValidationError>> {
    let unchecked_bounds = program.unchecked_bounds_proof();
    let mut program = program.into_program();
    let mut stats = PassStats {
        iterations: 1,
        ..PassStats::default()
    };
    let passing_modes = program
        .functions
        .iter()
        .map(|function| {
            function
                .params
                .iter()
                .map(|parameter| parameter.mode)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for function in &mut program.functions {
        propagate_local_constants(function, &passing_modes, &mut stats);
        canonicalize_block(&mut function.body, &mut stats);
    }
    crate::validate::revalidate_owned(program, unchecked_bounds).map(|program| (program, stats))
}

/// Runs backend-neutral MIR cleanup to a fixed point while retaining the
/// structured, non-SSA representation.
pub fn optimize(
    program: ValidatedProgram,
) -> Result<(OptimizedProgram, PassStats), Vec<ValidationError>> {
    let mut program = program;
    let mut total = PassStats::default();
    // Every round is monotonic: it only replaces values/rvalues with their
    // canonical constants or removes branches, statements, and locals. No
    // pass adds executable structure, so the finite program must reach a
    // fixed point without an arbitrary iteration cap.
    loop {
        let (mut next, mut stats) = canonicalize(program)?;
        let unchecked_bounds = next.unchecked_bounds_proof();
        let mut raw = next.into_program();
        if let Some(init) = raw.functions.get_mut(raw.entry_points.init.index()) {
            eliminate_preinitialized_zero_stores(init, &mut stats);
        }
        for function in &mut raw.functions {
            remove_dead_pure_locals(function, &mut stats);
        }
        next = crate::validate::revalidate_owned(raw, unchecked_bounds)?;
        let changed = stats.changed();
        total.merge(stats);
        program = next;
        if !changed {
            return Ok((OptimizedProgram(program), total));
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct StateRegion {
    state: crate::StateId,
    path: Vec<StateProjection>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum StateProjection {
    Field(u32),
    Index(u32),
}

fn eliminate_preinitialized_zero_stores(function: &mut Function, stats: &mut PassStats) {
    let mut dirty = Vec::<StateRegion>::new();
    let mut aliases = HashMap::<LocalId, StateRegion>::new();
    let mut barrier = false;
    let statements = std::mem::take(&mut function.body.statements);
    let mut retained = Vec::with_capacity(statements.len());
    for statement in statements {
        if barrier {
            retained.push(statement);
            continue;
        }
        let mut remove = false;
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                if let Some((region, exact)) = state_region(destination) {
                    if !exact {
                        mark_state_dirty(
                            &mut dirty,
                            StateRegion {
                                state: region.state,
                                path: Vec::new(),
                            },
                        );
                    } else if rvalue_is_all_bits_zero(value) {
                        if !dirty
                            .iter()
                            .any(|dirty| state_regions_overlap(dirty, &region))
                        {
                            remove = true;
                        } else {
                            clear_state_region(&mut dirty, &region);
                        }
                    } else {
                        mark_state_dirty(&mut dirty, region);
                    }
                }

                if let Place {
                    base: PlaceBase::Local(local),
                    projections,
                } = destination
                {
                    if projections.is_empty() {
                        aliases.remove(local);
                        if let Rvalue::MakeSlice {
                            source: crate::SliceSource::Place(place),
                            ..
                        } = value
                        {
                            if let Some((region, exact)) = state_region(place) {
                                if exact {
                                    aliases.insert(*local, region);
                                }
                            }
                        }
                    } else if let Some(region) = aliases.get(local).cloned() {
                        mark_state_dirty(
                            &mut dirty,
                            StateRegion {
                                state: region.state,
                                path: Vec::new(),
                            },
                        );
                    }
                }
            }
            StatementKind::SliceStore { slice, .. } => {
                if let Some(region) = value_alias_region(*slice, &aliases) {
                    mark_state_dirty(
                        &mut dirty,
                        StateRegion {
                            state: region.state,
                            path: Vec::new(),
                        },
                    );
                } else {
                    barrier = true;
                }
            }
            StatementKind::SliceFill { destination, value } => {
                if let Some(region) = value_alias_region(*destination, &aliases) {
                    let whole_state = StateRegion {
                        state: region.state,
                        path: Vec::new(),
                    };
                    if scalar_is_all_bits_zero(*value)
                        && !dirty
                            .iter()
                            .any(|dirty| state_regions_overlap(dirty, &whole_state))
                    {
                        remove = true;
                    } else {
                        mark_state_dirty(&mut dirty, whole_state);
                    }
                } else {
                    barrier = true;
                }
            }
            StatementKind::SliceCopy { destination, .. } => {
                if let Some(region) = value_alias_region(*destination, &aliases) {
                    mark_state_dirty(
                        &mut dirty,
                        StateRegion {
                            state: region.state,
                            path: Vec::new(),
                        },
                    );
                } else {
                    barrier = true;
                }
            }
            StatementKind::Call { .. }
            | StatementKind::If { .. }
            | StatementKind::Loop { .. }
            | StatementKind::Break
            | StatementKind::Continue => {
                barrier = true;
            }
            StatementKind::Return { .. } => {
                barrier = true;
            }
            StatementKind::OutputStore { .. }
            | StatementKind::ControlOutputStore { .. }
            | StatementKind::BufferStore { .. }
            | StatementKind::BufferParamStore { .. } => {}
        }
        if remove {
            stats.removed_redundant_zero_stores =
                stats.removed_redundant_zero_stores.saturating_add(1);
        } else {
            retained.push(statement);
        }
    }
    function.body.statements = retained;
}

fn state_region(place: &Place) -> Option<(StateRegion, bool)> {
    let PlaceBase::State(state) = place.base else {
        return None;
    };
    let mut path = Vec::with_capacity(place.projections.len());
    for projection in &place.projections {
        match projection {
            Projection::Field(field) => path.push(StateProjection::Field(field.raw())),
            Projection::Index { index, .. } => {
                let Value::Constant(ScalarValue::I32(index)) = index else {
                    return Some((
                        StateRegion {
                            state,
                            path: Vec::new(),
                        },
                        false,
                    ));
                };
                let Ok(index) = u32::try_from(*index) else {
                    return Some((
                        StateRegion {
                            state,
                            path: Vec::new(),
                        },
                        false,
                    ));
                };
                path.push(StateProjection::Index(index));
            }
        }
    }
    Some((StateRegion { state, path }, true))
}

fn state_path_is_prefix(prefix: &[StateProjection], path: &[StateProjection]) -> bool {
    prefix.len() <= path.len() && prefix.iter().zip(path).all(|(lhs, rhs)| lhs == rhs)
}

fn state_regions_overlap(lhs: &StateRegion, rhs: &StateRegion) -> bool {
    lhs.state == rhs.state
        && (state_path_is_prefix(&lhs.path, &rhs.path)
            || state_path_is_prefix(&rhs.path, &lhs.path))
}

fn mark_state_dirty(dirty: &mut Vec<StateRegion>, region: StateRegion) {
    if dirty.iter().any(|existing| {
        existing.state == region.state && state_path_is_prefix(&existing.path, &region.path)
    }) {
        return;
    }
    dirty.retain(|existing| {
        existing.state != region.state || !state_path_is_prefix(&region.path, &existing.path)
    });
    dirty.push(region);
}

fn clear_state_region(dirty: &mut Vec<StateRegion>, region: &StateRegion) {
    dirty.retain(|existing| {
        existing.state != region.state || !state_path_is_prefix(&region.path, &existing.path)
    });
}

fn value_alias_region(
    value: Value,
    aliases: &HashMap<LocalId, StateRegion>,
) -> Option<StateRegion> {
    let Value::Local(local) = value else {
        return None;
    };
    aliases.get(&local).cloned()
}

fn rvalue_is_all_bits_zero(value: &Rvalue) -> bool {
    matches!(value, Rvalue::Use(value) if scalar_is_all_bits_zero(*value))
}

fn scalar_is_all_bits_zero(value: Value) -> bool {
    match value {
        Value::Constant(ScalarValue::F32(value)) => value.to_bits() == 0,
        Value::Constant(ScalarValue::F64(value)) => value.to_bits() == 0,
        Value::Constant(ScalarValue::I32(value)) => value == 0,
        Value::Constant(ScalarValue::I64(value)) => value == 0,
        Value::Constant(ScalarValue::Bool(value)) => !value,
        Value::Local(_) => false,
    }
}

fn propagate_local_constants(
    function: &mut Function,
    passing_modes: &[Vec<PassingMode>],
    stats: &mut PassStats,
) {
    let mut facts = vec![None; function.locals.len()];
    propagate_block_constants(&mut function.body, &mut facts, passing_modes, stats);
}

fn propagate_block_constants(
    block: &mut Block,
    facts: &mut Vec<Option<ScalarValue>>,
    passing_modes: &[Vec<PassingMode>],
    stats: &mut PassStats,
) -> bool {
    let mut falls_through = true;
    for statement in &mut block.statements {
        if !falls_through {
            break;
        }
        falls_through = propagate_statement_constants(statement, facts, passing_modes, stats);
    }
    falls_through
}

fn propagate_statement_constants(
    statement: &mut Statement,
    facts: &mut Vec<Option<ScalarValue>>,
    passing_modes: &[Vec<PassingMode>],
    stats: &mut PassStats,
) -> bool {
    match &mut statement.kind {
        StatementKind::Assign { destination, value } => {
            propagate_place_indices(destination, facts, stats);
            propagate_rvalue_values(value, facts, stats);
            if let PlaceBase::Local(local) = destination.base {
                let fact = if destination.projections.is_empty() {
                    match value {
                        Rvalue::Use(Value::Constant(value)) => Some(*value),
                        _ => fold_rvalue(value),
                    }
                } else {
                    None
                };
                if let Some(slot) = facts.get_mut(local.index()) {
                    *slot = fact;
                }
            }
            true
        }
        StatementKind::Call {
            results,
            function,
            args,
        } => {
            for argument in args.iter_mut() {
                propagate_call_argument(argument, facts, stats);
            }
            for (index, argument) in args.iter().enumerate() {
                if passing_modes
                    .get(function.index())
                    .and_then(|modes| modes.get(index))
                    == Some(&PassingMode::ReadWriteReference)
                {
                    if let Some(local) = mutated_argument_local(argument) {
                        invalidate_fact(facts, local);
                    }
                }
            }
            for result in results {
                invalidate_fact(facts, *result);
            }
            true
        }
        StatementKind::OutputStore {
            element,
            frame,
            value,
            ..
        } => {
            propagate_optional_value(element, facts, stats);
            propagate_value(frame, facts, stats);
            propagate_value(value, facts, stats);
            true
        }
        StatementKind::ControlOutputStore { element, value, .. } => {
            propagate_optional_value(element, facts, stats);
            propagate_value(value, facts, stats);
            true
        }
        StatementKind::BufferStore {
            channel,
            index,
            value,
            ..
        }
        | StatementKind::BufferParamStore {
            channel,
            index,
            value,
            ..
        } => {
            propagate_optional_value(channel, facts, stats);
            propagate_value(index, facts, stats);
            propagate_value(value, facts, stats);
            true
        }
        StatementKind::SliceStore {
            slice,
            index,
            value,
            ..
        } => {
            propagate_value(slice, facts, stats);
            propagate_value(index, facts, stats);
            propagate_value(value, facts, stats);
            true
        }
        StatementKind::SliceFill { destination, value } => {
            propagate_value(destination, facts, stats);
            propagate_value(value, facts, stats);
            true
        }
        StatementKind::SliceCopy {
            destination,
            source,
        } => {
            propagate_value(destination, facts, stats);
            propagate_value(source, facts, stats);
            true
        }
        StatementKind::If {
            condition,
            then_block,
            else_block,
        } => {
            propagate_value(condition, facts, stats);
            let constant_condition = match condition {
                Value::Constant(ScalarValue::Bool(condition)) => Some(*condition),
                _ => None,
            };
            let mut then_facts = facts.clone();
            let mut else_facts = facts.clone();
            let then_falls =
                propagate_block_constants(then_block, &mut then_facts, passing_modes, stats);
            let else_falls =
                propagate_block_constants(else_block, &mut else_facts, passing_modes, stats);
            match constant_condition {
                Some(true) => {
                    if then_falls {
                        *facts = then_facts;
                    } else {
                        facts.fill(None);
                    }
                    then_falls
                }
                Some(false) => {
                    if else_falls {
                        *facts = else_facts;
                    } else {
                        facts.fill(None);
                    }
                    else_falls
                }
                None => match (then_falls, else_falls) {
                    (true, true) => {
                        merge_constant_facts(facts, &then_facts, &else_facts);
                        true
                    }
                    (true, false) => {
                        *facts = then_facts;
                        true
                    }
                    (false, true) => {
                        *facts = else_facts;
                        true
                    }
                    (false, false) => {
                        facts.fill(None);
                        false
                    }
                },
            }
        }
        StatementKind::Loop { body } => {
            let mut mutated = HashSet::new();
            collect_mutated_locals(body, passing_modes, &mut mutated);
            for local in &mutated {
                invalidate_fact(facts, *local);
            }
            let mut body_facts = facts.clone();
            propagate_block_constants(body, &mut body_facts, passing_modes, stats);
            for local in mutated {
                invalidate_fact(facts, local);
            }
            block_contains_reachable_break(body)
        }
        StatementKind::Return { values } => {
            for value in values {
                propagate_value(value, facts, stats);
            }
            false
        }
        StatementKind::Break | StatementKind::Continue => false,
    }
}

fn propagate_optional_value(
    value: &mut Option<Value>,
    facts: &[Option<ScalarValue>],
    stats: &mut PassStats,
) {
    if let Some(value) = value {
        propagate_value(value, facts, stats);
    }
}

fn propagate_value(value: &mut Value, facts: &[Option<ScalarValue>], stats: &mut PassStats) {
    let Value::Local(local) = value else {
        return;
    };
    let Some(constant) = facts.get(local.index()).and_then(|fact| *fact) else {
        return;
    };
    *value = Value::Constant(constant);
    stats.propagated_values = stats.propagated_values.saturating_add(1);
}

fn propagate_place_indices(
    place: &mut Place,
    facts: &[Option<ScalarValue>],
    stats: &mut PassStats,
) {
    for projection in &mut place.projections {
        if let Projection::Index { index, .. } = projection {
            propagate_value(index, facts, stats);
        }
    }
}

fn propagate_rvalue_values(
    rvalue: &mut Rvalue,
    facts: &[Option<ScalarValue>],
    stats: &mut PassStats,
) {
    match rvalue {
        Rvalue::Use(value) | Rvalue::SliceLen(value) => {
            propagate_value(value, facts, stats);
        }
        Rvalue::Load(place) => propagate_place_indices(place, facts, stats),
        Rvalue::Unary { operand, .. } => propagate_value(operand, facts, stats),
        Rvalue::Binary { lhs, rhs, .. } | Rvalue::Compare { lhs, rhs, .. } => {
            propagate_value(lhs, facts, stats);
            propagate_value(rhs, facts, stats);
        }
        Rvalue::Cast { value, .. } => propagate_value(value, facts, stats),
        Rvalue::Intrinsic { args, .. } => {
            for value in args {
                propagate_value(value, facts, stats);
            }
        }
        Rvalue::ProcessFrame { offset } => propagate_value(offset, facts, stats),
        Rvalue::InputLoad { element, frame, .. } | Rvalue::OutputLoad { element, frame, .. } => {
            propagate_optional_value(element, facts, stats);
            propagate_value(frame, facts, stats);
        }
        Rvalue::BufferLoad { channel, index, .. }
        | Rvalue::BufferParamLoad { channel, index, .. } => {
            propagate_optional_value(channel, facts, stats);
            propagate_value(index, facts, stats);
        }
        Rvalue::ConstDataLoad { index, .. } => propagate_value(index, facts, stats),
        Rvalue::MakeSlice {
            source, start, len, ..
        } => {
            match source {
                crate::SliceSource::Place(place) => {
                    propagate_place_indices(place, facts, stats);
                }
                crate::SliceSource::Buffer { channel, .. }
                | crate::SliceSource::BufferParam { channel, .. } => {
                    propagate_optional_value(channel, facts, stats);
                }
                crate::SliceSource::ConstData(_) => {}
            }
            propagate_value(start, facts, stats);
            propagate_value(len, facts, stats);
        }
        Rvalue::SliceLoad { slice, index, .. } => {
            propagate_value(slice, facts, stats);
            propagate_value(index, facts, stats);
        }
        Rvalue::BufferLen(_)
        | Rvalue::BufferChannels(_)
        | Rvalue::BufferSampleRate(_)
        | Rvalue::BufferParamLen(_)
        | Rvalue::BufferParamChannels(_)
        | Rvalue::BufferParamSampleRate(_) => {}
    }
}

fn propagate_call_argument(
    argument: &mut CallArgument,
    facts: &[Option<ScalarValue>],
    stats: &mut PassStats,
) {
    match argument {
        CallArgument::Value(value) => propagate_value(value, facts, stats),
        CallArgument::Place(place) => propagate_place_indices(place, facts, stats),
        CallArgument::SliceElement { slice, index, .. } => {
            propagate_value(slice, facts, stats);
            propagate_value(index, facts, stats);
        }
        CallArgument::ArrayWindow { array, start, .. } => {
            propagate_place_indices(array, facts, stats);
            propagate_value(start, facts, stats);
        }
        CallArgument::SliceWindow { slice, start, .. } => {
            propagate_value(slice, facts, stats);
            propagate_value(start, facts, stats);
        }
        CallArgument::Buffer(_) => {}
    }
}

fn invalidate_fact(facts: &mut [Option<ScalarValue>], local: LocalId) {
    if let Some(fact) = facts.get_mut(local.index()) {
        *fact = None;
    }
}

fn merge_constant_facts(
    destination: &mut [Option<ScalarValue>],
    lhs: &[Option<ScalarValue>],
    rhs: &[Option<ScalarValue>],
) {
    for (index, destination) in destination.iter_mut().enumerate() {
        *destination = match (
            lhs.get(index).copied().flatten(),
            rhs.get(index).copied().flatten(),
        ) {
            (Some(lhs), Some(rhs)) if scalar_constants_identical(lhs, rhs) => Some(lhs),
            _ => None,
        };
    }
}

fn scalar_constants_identical(lhs: ScalarValue, rhs: ScalarValue) -> bool {
    match (lhs, rhs) {
        (ScalarValue::F32(lhs), ScalarValue::F32(rhs)) => lhs.to_bits() == rhs.to_bits(),
        (ScalarValue::F64(lhs), ScalarValue::F64(rhs)) => lhs.to_bits() == rhs.to_bits(),
        (ScalarValue::I32(lhs), ScalarValue::I32(rhs)) => lhs == rhs,
        (ScalarValue::I64(lhs), ScalarValue::I64(rhs)) => lhs == rhs,
        (ScalarValue::Bool(lhs), ScalarValue::Bool(rhs)) => lhs == rhs,
        _ => false,
    }
}

fn mutated_argument_local(argument: &CallArgument) -> Option<LocalId> {
    let place = match argument {
        CallArgument::Place(place) => place,
        CallArgument::ArrayWindow { array, .. } => array,
        CallArgument::Value(_)
        | CallArgument::SliceElement { .. }
        | CallArgument::SliceWindow { .. }
        | CallArgument::Buffer(_) => return None,
    };
    let PlaceBase::Local(local) = place.base else {
        return None;
    };
    Some(local)
}

fn collect_mutated_locals(
    block: &Block,
    passing_modes: &[Vec<PassingMode>],
    mutated: &mut HashSet<LocalId>,
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, .. } => {
                if let PlaceBase::Local(local) = destination.base {
                    mutated.insert(local);
                }
            }
            StatementKind::Call {
                results,
                function,
                args,
            } => {
                mutated.extend(results.iter().copied());
                for (index, argument) in args.iter().enumerate() {
                    if passing_modes
                        .get(function.index())
                        .and_then(|modes| modes.get(index))
                        == Some(&PassingMode::ReadWriteReference)
                    {
                        if let Some(local) = mutated_argument_local(argument) {
                            mutated.insert(local);
                        }
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_mutated_locals(then_block, passing_modes, mutated);
                collect_mutated_locals(else_block, passing_modes, mutated);
            }
            StatementKind::Loop { body } => {
                collect_mutated_locals(body, passing_modes, mutated);
            }
            StatementKind::OutputStore { .. }
            | StatementKind::ControlOutputStore { .. }
            | StatementKind::BufferStore { .. }
            | StatementKind::BufferParamStore { .. }
            | StatementKind::SliceStore { .. }
            | StatementKind::SliceFill { .. }
            | StatementKind::SliceCopy { .. }
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Return { .. } => {}
        }
    }
}

fn canonicalize_block(block: &mut Block, stats: &mut PassStats) {
    let original = std::mem::take(&mut block.statements);
    let mut statements = Vec::with_capacity(original.len());
    let mut reachable = true;
    for mut statement in original {
        if !reachable {
            stats.removed_unreachable_statements =
                stats.removed_unreachable_statements.saturating_add(1);
            continue;
        }
        match &mut statement.kind {
            StatementKind::Assign { value, .. } => {
                if let Some(folded) = fold_rvalue(value) {
                    *value = Rvalue::Use(Value::Constant(folded));
                    stats.folded_rvalues = stats.folded_rvalues.saturating_add(1);
                }
            }
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                canonicalize_block(then_block, stats);
                canonicalize_block(else_block, stats);
                if let Value::Constant(ScalarValue::Bool(condition)) = condition {
                    let selected = if *condition {
                        std::mem::take(&mut then_block.statements)
                    } else {
                        std::mem::take(&mut else_block.statements)
                    };
                    stats.simplified_branches = stats.simplified_branches.saturating_add(1);
                    for selected in selected {
                        reachable = statement_falls_through(&selected.kind);
                        statements.push(selected);
                        if !reachable {
                            break;
                        }
                    }
                    continue;
                }
            }
            StatementKind::Loop { body } => canonicalize_block(body, stats),
            StatementKind::Call { .. }
            | StatementKind::OutputStore { .. }
            | StatementKind::ControlOutputStore { .. }
            | StatementKind::BufferStore { .. }
            | StatementKind::BufferParamStore { .. }
            | StatementKind::SliceStore { .. }
            | StatementKind::SliceFill { .. }
            | StatementKind::SliceCopy { .. }
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Return { .. } => {}
        }
        reachable = statement_falls_through(&statement.kind);
        statements.push(statement);
    }
    block.statements = statements;
}

fn statement_falls_through(statement: &StatementKind) -> bool {
    match statement {
        StatementKind::Return { .. } | StatementKind::Break | StatementKind::Continue => false,
        StatementKind::If {
            then_block,
            else_block,
            ..
        } => block_falls_through(then_block) || block_falls_through(else_block),
        StatementKind::Loop { body } => block_contains_reachable_break(body),
        _ => true,
    }
}

fn block_falls_through(block: &Block) -> bool {
    let mut reachable = true;
    for statement in &block.statements {
        if !reachable {
            break;
        }
        reachable = statement_falls_through(&statement.kind);
    }
    reachable
}

fn block_contains_reachable_break(block: &Block) -> bool {
    let mut reachable = true;
    for statement in &block.statements {
        if !reachable {
            break;
        }
        match &statement.kind {
            StatementKind::Break => return true,
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                if block_contains_reachable_break(then_block)
                    || block_contains_reachable_break(else_block)
                {
                    return true;
                }
            }
            StatementKind::Loop { .. } => {}
            _ => {}
        }
        reachable = statement_falls_through(&statement.kind);
    }
    false
}

fn fold_rvalue(rvalue: &Rvalue) -> Option<ScalarValue> {
    match rvalue {
        Rvalue::Unary { op, operand } => fold_unary(*op, constant(*operand)?),
        Rvalue::Binary { op, lhs, rhs } => fold_binary(*op, constant(*lhs)?, constant(*rhs)?),
        Rvalue::Compare { op, lhs, rhs } => {
            fold_compare(*op, constant(*lhs)?, constant(*rhs)?).map(ScalarValue::Bool)
        }
        Rvalue::Cast { value, to } => fold_cast(constant(*value)?, *to),
        Rvalue::Intrinsic { intrinsic, args } => {
            let args = args
                .iter()
                .copied()
                .map(constant)
                .collect::<Option<Vec<_>>>()?;
            fold_intrinsic(*intrinsic, &args)
        }
        _ => None,
    }
}

fn constant(value: Value) -> Option<ScalarValue> {
    match value {
        Value::Constant(value) => Some(value),
        Value::Local(_) => None,
    }
}

fn fold_unary(op: crate::UnaryOp, value: ScalarValue) -> Option<ScalarValue> {
    match (op, value) {
        (crate::UnaryOp::Negate, ScalarValue::F32(value)) => Some(ScalarValue::F32(-value)),
        (crate::UnaryOp::Negate, ScalarValue::F64(value)) => Some(ScalarValue::F64(-value)),
        (crate::UnaryOp::Negate, ScalarValue::I32(value)) => {
            Some(ScalarValue::I32(value.wrapping_neg()))
        }
        (crate::UnaryOp::Negate, ScalarValue::I64(value)) => {
            Some(ScalarValue::I64(value.wrapping_neg()))
        }
        (crate::UnaryOp::LogicalNot, ScalarValue::Bool(value)) => Some(ScalarValue::Bool(!value)),
        (crate::UnaryOp::BitNot, ScalarValue::I32(value)) => Some(ScalarValue::I32(!value)),
        (crate::UnaryOp::BitNot, ScalarValue::I64(value)) => Some(ScalarValue::I64(!value)),
        _ => None,
    }
}

macro_rules! fold_integer_binary {
    ($op:expr, $lhs:expr, $rhs:expr, $variant:path) => {{
        let value = match $op {
            BinaryOp::Add => $lhs.wrapping_add($rhs),
            BinaryOp::Subtract => $lhs.wrapping_sub($rhs),
            BinaryOp::Multiply => $lhs.wrapping_mul($rhs),
            BinaryOp::Divide if $rhs != 0 => $lhs.wrapping_div($rhs),
            BinaryOp::Remainder if $rhs != 0 => $lhs.wrapping_rem($rhs),
            BinaryOp::BitAnd => $lhs & $rhs,
            BinaryOp::BitOr => $lhs | $rhs,
            BinaryOp::BitXor => $lhs ^ $rhs,
            BinaryOp::ShiftLeft => $lhs.wrapping_shl($rhs as u32),
            BinaryOp::ShiftRight => $lhs.wrapping_shr($rhs as u32),
            BinaryOp::Divide | BinaryOp::Remainder => return None,
        };
        Some($variant(value))
    }};
}

fn fold_binary(op: BinaryOp, lhs: ScalarValue, rhs: ScalarValue) -> Option<ScalarValue> {
    match (lhs, rhs) {
        (ScalarValue::I32(lhs), ScalarValue::I32(rhs)) => {
            fold_integer_binary!(op, lhs, rhs, ScalarValue::I32)
        }
        (ScalarValue::I64(lhs), ScalarValue::I64(rhs)) => {
            fold_integer_binary!(op, lhs, rhs, ScalarValue::I64)
        }
        (ScalarValue::F32(lhs), ScalarValue::F32(rhs)) => Some(ScalarValue::F32(match op {
            BinaryOp::Add => lhs + rhs,
            BinaryOp::Subtract => lhs - rhs,
            BinaryOp::Multiply => lhs * rhs,
            BinaryOp::Divide => lhs / rhs,
            BinaryOp::Remainder => lhs % rhs,
            _ => return None,
        })),
        (ScalarValue::F64(lhs), ScalarValue::F64(rhs)) => Some(ScalarValue::F64(match op {
            BinaryOp::Add => lhs + rhs,
            BinaryOp::Subtract => lhs - rhs,
            BinaryOp::Multiply => lhs * rhs,
            BinaryOp::Divide => lhs / rhs,
            BinaryOp::Remainder => lhs % rhs,
            _ => return None,
        })),
        _ => None,
    }
}

macro_rules! compare_values {
    ($op:expr, $lhs:expr, $rhs:expr) => {
        Some(match $op {
            CompareOp::Equal => $lhs == $rhs,
            CompareOp::NotEqual => $lhs != $rhs,
            CompareOp::Less => $lhs < $rhs,
            CompareOp::LessEqual => $lhs <= $rhs,
            CompareOp::Greater => $lhs > $rhs,
            CompareOp::GreaterEqual => $lhs >= $rhs,
        })
    };
}

fn fold_compare(op: CompareOp, lhs: ScalarValue, rhs: ScalarValue) -> Option<bool> {
    match (lhs, rhs) {
        (ScalarValue::F32(lhs), ScalarValue::F32(rhs)) => compare_values!(op, lhs, rhs),
        (ScalarValue::F64(lhs), ScalarValue::F64(rhs)) => compare_values!(op, lhs, rhs),
        (ScalarValue::I32(lhs), ScalarValue::I32(rhs)) => compare_values!(op, lhs, rhs),
        (ScalarValue::I64(lhs), ScalarValue::I64(rhs)) => compare_values!(op, lhs, rhs),
        (ScalarValue::Bool(lhs), ScalarValue::Bool(rhs)) => match op {
            CompareOp::Equal => Some(lhs == rhs),
            CompareOp::NotEqual => Some(lhs != rhs),
            _ => None,
        },
        _ => None,
    }
}

fn fold_cast(value: ScalarValue, to: ScalarType) -> Option<ScalarValue> {
    macro_rules! cast_from {
        ($value:expr) => {
            Some(match to {
                ScalarType::F32 => ScalarValue::F32($value as f32),
                ScalarType::F64 => ScalarValue::F64($value as f64),
                ScalarType::I32 => ScalarValue::I32($value as i32),
                ScalarType::I64 => ScalarValue::I64($value as i64),
                ScalarType::Bool => return None,
            })
        };
    }
    match value {
        ScalarValue::F32(value) => cast_from!(value),
        ScalarValue::F64(value) => cast_from!(value),
        ScalarValue::I32(value) => cast_from!(value),
        ScalarValue::I64(value) => cast_from!(value),
        ScalarValue::Bool(_) => None,
    }
}

fn fold_intrinsic(intrinsic: Intrinsic, args: &[ScalarValue]) -> Option<ScalarValue> {
    match (intrinsic, args) {
        (Intrinsic::Abs, [ScalarValue::I32(value)]) => Some(ScalarValue::I32(value.wrapping_abs())),
        (Intrinsic::Abs, [ScalarValue::I64(value)]) => Some(ScalarValue::I64(value.wrapping_abs())),
        (Intrinsic::Min, [ScalarValue::I32(lhs), ScalarValue::I32(rhs)]) => {
            Some(ScalarValue::I32((*lhs).min(*rhs)))
        }
        (Intrinsic::Min, [ScalarValue::I64(lhs), ScalarValue::I64(rhs)]) => {
            Some(ScalarValue::I64((*lhs).min(*rhs)))
        }
        (Intrinsic::Max, [ScalarValue::I32(lhs), ScalarValue::I32(rhs)]) => {
            Some(ScalarValue::I32((*lhs).max(*rhs)))
        }
        (Intrinsic::Max, [ScalarValue::I64(lhs), ScalarValue::I64(rhs)]) => {
            Some(ScalarValue::I64((*lhs).max(*rhs)))
        }
        (Intrinsic::Abs, [ScalarValue::F32(value)]) if value.is_finite() => Some(ScalarValue::F32(
            f32::from_bits(value.to_bits() & !(1_u32 << 31)),
        )),
        (Intrinsic::Abs, [ScalarValue::F64(value)]) if value.is_finite() => Some(ScalarValue::F64(
            f64::from_bits(value.to_bits() & !(1_u64 << 63)),
        )),
        (Intrinsic::Floor, [ScalarValue::F32(value)]) if value.is_finite() => {
            Some(ScalarValue::F32(value.floor()))
        }
        (Intrinsic::Floor, [ScalarValue::F64(value)]) if value.is_finite() => {
            Some(ScalarValue::F64(value.floor()))
        }
        (Intrinsic::Ceil, [ScalarValue::F32(value)]) if value.is_finite() => {
            Some(ScalarValue::F32(value.ceil()))
        }
        (Intrinsic::Ceil, [ScalarValue::F64(value)]) if value.is_finite() => {
            Some(ScalarValue::F64(value.ceil()))
        }
        (Intrinsic::Round, [ScalarValue::F32(value)]) if value.is_finite() => {
            Some(ScalarValue::F32(value.round()))
        }
        (Intrinsic::Round, [ScalarValue::F64(value)]) if value.is_finite() => {
            Some(ScalarValue::F64(value.round()))
        }
        (Intrinsic::Trunc, [ScalarValue::F32(value)]) if value.is_finite() => {
            Some(ScalarValue::F32(value.trunc()))
        }
        (Intrinsic::Trunc, [ScalarValue::F64(value)]) if value.is_finite() => {
            Some(ScalarValue::F64(value.trunc()))
        }
        (Intrinsic::Min, [ScalarValue::F32(lhs), ScalarValue::F32(rhs)])
            if lhs.is_finite() && rhs.is_finite() =>
        {
            Some(ScalarValue::F32(fold_f32_minimum(*lhs, *rhs)))
        }
        (Intrinsic::Min, [ScalarValue::F64(lhs), ScalarValue::F64(rhs)])
            if lhs.is_finite() && rhs.is_finite() =>
        {
            Some(ScalarValue::F64(fold_f64_minimum(*lhs, *rhs)))
        }
        (Intrinsic::Max, [ScalarValue::F32(lhs), ScalarValue::F32(rhs)])
            if lhs.is_finite() && rhs.is_finite() =>
        {
            Some(ScalarValue::F32(fold_f32_maximum(*lhs, *rhs)))
        }
        (Intrinsic::Max, [ScalarValue::F64(lhs), ScalarValue::F64(rhs)])
            if lhs.is_finite() && rhs.is_finite() =>
        {
            Some(ScalarValue::F64(fold_f64_maximum(*lhs, *rhs)))
        }
        _ => None,
    }
}

fn fold_f32_minimum(lhs: f32, rhs: f32) -> f32 {
    if lhs == rhs {
        if lhs == 0.0 {
            return f32::from_bits(lhs.to_bits() | rhs.to_bits());
        }
        lhs
    } else if lhs < rhs {
        lhs
    } else {
        rhs
    }
}

fn fold_f32_maximum(lhs: f32, rhs: f32) -> f32 {
    if lhs == rhs {
        if lhs == 0.0 {
            return f32::from_bits(lhs.to_bits() & rhs.to_bits());
        }
        lhs
    } else if lhs > rhs {
        lhs
    } else {
        rhs
    }
}

fn fold_f64_minimum(lhs: f64, rhs: f64) -> f64 {
    if lhs == rhs {
        if lhs == 0.0 {
            return f64::from_bits(lhs.to_bits() | rhs.to_bits());
        }
        lhs
    } else if lhs < rhs {
        lhs
    } else {
        rhs
    }
}

fn fold_f64_maximum(lhs: f64, rhs: f64) -> f64 {
    if lhs == rhs {
        if lhs == 0.0 {
            return f64::from_bits(lhs.to_bits() & rhs.to_bits());
        }
        lhs
    } else if lhs > rhs {
        lhs
    } else {
        rhs
    }
}

fn remove_dead_pure_locals(function: &mut Function, stats: &mut PassStats) {
    let mut reads = vec![0_u32; function.locals.len()];
    collect_block_reads(&function.body, &mut reads);
    remove_dead_assignments(&mut function.body, &reads, stats);

    let mut referenced = HashSet::new();
    collect_block_local_references(&function.body, &mut referenced);
    let mut mapping = vec![None; function.locals.len()];
    let mut locals = Vec::new();
    for (index, local) in std::mem::take(&mut function.locals).into_iter().enumerate() {
        if referenced.contains(&LocalId::new(index as u32)) {
            mapping[index] = Some(LocalId::new(locals.len() as u32));
            locals.push(local);
        } else {
            stats.removed_locals = stats.removed_locals.saturating_add(1);
        }
    }
    function.locals = locals;
    rewrite_block_locals(&mut function.body, &mapping);
}

fn remove_dead_assignments(block: &mut Block, reads: &[u32], stats: &mut PassStats) {
    for statement in &mut block.statements {
        match &mut statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                remove_dead_assignments(then_block, reads, stats);
                remove_dead_assignments(else_block, reads, stats);
            }
            StatementKind::Loop { body } => remove_dead_assignments(body, reads, stats),
            _ => {}
        }
    }
    block.statements.retain(|statement| {
        let StatementKind::Assign { destination, value } = &statement.kind else {
            return true;
        };
        let Place {
            base: PlaceBase::Local(local),
            projections,
        } = destination
        else {
            return true;
        };
        let remove = projections.is_empty()
            && reads.get(local.index()) == Some(&0)
            && rvalue_is_discardable(value);
        if remove {
            stats.removed_dead_assignments = stats.removed_dead_assignments.saturating_add(1);
        }
        !remove
    });
}

fn rvalue_is_discardable(value: &Rvalue) -> bool {
    match value {
        Rvalue::Use(_)
        | Rvalue::Unary { .. }
        | Rvalue::Compare { .. }
        | Rvalue::Cast { .. }
        | Rvalue::Intrinsic { .. } => true,
        Rvalue::Binary { op, .. } => !matches!(op, BinaryOp::Divide | BinaryOp::Remainder),
        _ => false,
    }
}

fn collect_block_reads(block: &Block, reads: &mut [u32]) {
    for statement in &block.statements {
        collect_statement_reads(statement, reads);
    }
}

fn mark_value_read(value: Value, reads: &mut [u32]) {
    if let Value::Local(local) = value {
        if let Some(reads) = reads.get_mut(local.index()) {
            *reads = reads.saturating_add(1);
        }
    }
}

fn collect_place_index_reads(place: &Place, reads: &mut [u32]) {
    for projection in &place.projections {
        if let Projection::Index { index, .. } = projection {
            mark_value_read(*index, reads);
        }
    }
}

fn collect_place_read(place: &Place, reads: &mut [u32]) {
    collect_place_index_reads(place, reads);
    if let PlaceBase::Local(local) = place.base {
        if let Some(reads) = reads.get_mut(local.index()) {
            *reads = reads.saturating_add(1);
        }
    }
}

fn collect_rvalue_reads(value: &Rvalue, reads: &mut [u32]) {
    match value {
        Rvalue::Use(value) | Rvalue::SliceLen(value) => mark_value_read(*value, reads),
        Rvalue::Load(place) => collect_place_read(place, reads),
        Rvalue::Unary { operand, .. } => mark_value_read(*operand, reads),
        Rvalue::Binary { lhs, rhs, .. } | Rvalue::Compare { lhs, rhs, .. } => {
            mark_value_read(*lhs, reads);
            mark_value_read(*rhs, reads);
        }
        Rvalue::Cast { value, .. } => mark_value_read(*value, reads),
        Rvalue::Intrinsic { args, .. } => {
            for value in args {
                mark_value_read(*value, reads);
            }
        }
        Rvalue::ProcessFrame { offset } => mark_value_read(*offset, reads),
        Rvalue::InputLoad { element, frame, .. } | Rvalue::OutputLoad { element, frame, .. } => {
            if let Some(element) = element {
                mark_value_read(*element, reads);
            }
            mark_value_read(*frame, reads);
        }
        Rvalue::BufferLoad { channel, index, .. }
        | Rvalue::BufferParamLoad { channel, index, .. } => {
            if let Some(channel) = channel {
                mark_value_read(*channel, reads);
            }
            mark_value_read(*index, reads);
        }
        Rvalue::ConstDataLoad { index, .. } => mark_value_read(*index, reads),
        Rvalue::MakeSlice {
            source, start, len, ..
        } => {
            match source {
                crate::SliceSource::Place(place) => collect_place_index_reads(place, reads),
                crate::SliceSource::Buffer { channel, .. }
                | crate::SliceSource::BufferParam { channel, .. } => {
                    if let Some(channel) = channel {
                        mark_value_read(*channel, reads);
                    }
                }
                crate::SliceSource::ConstData(_) => {}
            }
            mark_value_read(*start, reads);
            mark_value_read(*len, reads);
        }
        Rvalue::SliceLoad { slice, index, .. } => {
            mark_value_read(*slice, reads);
            mark_value_read(*index, reads);
        }
        Rvalue::BufferLen(_)
        | Rvalue::BufferChannels(_)
        | Rvalue::BufferSampleRate(_)
        | Rvalue::BufferParamLen(_)
        | Rvalue::BufferParamChannels(_)
        | Rvalue::BufferParamSampleRate(_) => {}
    }
}

fn collect_statement_reads(statement: &Statement, reads: &mut [u32]) {
    match &statement.kind {
        StatementKind::Assign { destination, value } => {
            collect_place_index_reads(destination, reads);
            collect_rvalue_reads(value, reads);
        }
        StatementKind::Call { args, .. } => {
            for argument in args {
                match argument {
                    CallArgument::Value(value) => mark_value_read(*value, reads),
                    CallArgument::Place(place) => collect_place_read(place, reads),
                    CallArgument::ArrayWindow { array, start, .. } => {
                        collect_place_read(array, reads);
                        mark_value_read(*start, reads);
                    }
                    CallArgument::SliceElement { slice, index, .. } => {
                        mark_value_read(*slice, reads);
                        mark_value_read(*index, reads);
                    }
                    CallArgument::SliceWindow { slice, start, .. } => {
                        mark_value_read(*slice, reads);
                        mark_value_read(*start, reads);
                    }
                    CallArgument::Buffer(_) => {}
                }
            }
        }
        StatementKind::OutputStore {
            element,
            frame,
            value,
            ..
        } => {
            if let Some(element) = element {
                mark_value_read(*element, reads);
            }
            mark_value_read(*frame, reads);
            mark_value_read(*value, reads);
        }
        StatementKind::ControlOutputStore { element, value, .. } => {
            if let Some(element) = element {
                mark_value_read(*element, reads);
            }
            mark_value_read(*value, reads);
        }
        StatementKind::BufferStore {
            channel,
            index,
            value,
            ..
        }
        | StatementKind::BufferParamStore {
            channel,
            index,
            value,
            ..
        } => {
            if let Some(channel) = channel {
                mark_value_read(*channel, reads);
            }
            mark_value_read(*index, reads);
            mark_value_read(*value, reads);
        }
        StatementKind::SliceStore {
            slice,
            index,
            value,
            ..
        } => {
            mark_value_read(*slice, reads);
            mark_value_read(*index, reads);
            mark_value_read(*value, reads);
        }
        StatementKind::SliceFill { destination, value } => {
            mark_value_read(*destination, reads);
            mark_value_read(*value, reads);
        }
        StatementKind::SliceCopy {
            destination,
            source,
        } => {
            mark_value_read(*destination, reads);
            mark_value_read(*source, reads);
        }
        StatementKind::If {
            condition,
            then_block,
            else_block,
        } => {
            mark_value_read(*condition, reads);
            collect_block_reads(then_block, reads);
            collect_block_reads(else_block, reads);
        }
        StatementKind::Loop { body } => collect_block_reads(body, reads),
        StatementKind::Return { values } => {
            for value in values {
                mark_value_read(*value, reads);
            }
        }
        StatementKind::Break | StatementKind::Continue => {}
    }
}

fn collect_block_local_references(block: &Block, referenced: &mut HashSet<LocalId>) {
    collect_block_writes(block, referenced);
    collect_read_references(block, referenced);
}

fn collect_read_references(block: &Block, referenced: &mut HashSet<LocalId>) {
    fn value(value: Value, referenced: &mut HashSet<LocalId>) {
        if let Value::Local(local) = value {
            referenced.insert(local);
        }
    }
    fn place(place: &Place, include_base: bool, referenced: &mut HashSet<LocalId>) {
        if include_base {
            if let PlaceBase::Local(local) = place.base {
                referenced.insert(local);
            }
        }
        for projection in &place.projections {
            if let Projection::Index { index, .. } = projection {
                value(*index, referenced);
            }
        }
    }
    fn rvalue(rvalue: &Rvalue, referenced: &mut HashSet<LocalId>) {
        match rvalue {
            Rvalue::Use(v) | Rvalue::SliceLen(v) => value(*v, referenced),
            Rvalue::Load(p) => place(p, true, referenced),
            Rvalue::Unary { operand, .. } => value(*operand, referenced),
            Rvalue::Binary { lhs, rhs, .. } | Rvalue::Compare { lhs, rhs, .. } => {
                value(*lhs, referenced);
                value(*rhs, referenced);
            }
            Rvalue::Cast { value: v, .. } => value(*v, referenced),
            Rvalue::Intrinsic { args, .. } => {
                for v in args {
                    value(*v, referenced);
                }
            }
            Rvalue::ProcessFrame { offset } => value(*offset, referenced),
            Rvalue::InputLoad { element, frame, .. }
            | Rvalue::OutputLoad { element, frame, .. } => {
                if let Some(v) = element {
                    value(*v, referenced);
                }
                value(*frame, referenced);
            }
            Rvalue::BufferLoad { channel, index, .. }
            | Rvalue::BufferParamLoad { channel, index, .. } => {
                if let Some(v) = channel {
                    value(*v, referenced);
                }
                value(*index, referenced);
            }
            Rvalue::ConstDataLoad { index, .. } => value(*index, referenced),
            Rvalue::MakeSlice {
                source, start, len, ..
            } => {
                match source {
                    crate::SliceSource::Place(p) => place(p, false, referenced),
                    crate::SliceSource::Buffer { channel, .. }
                    | crate::SliceSource::BufferParam { channel, .. } => {
                        if let Some(v) = channel {
                            value(*v, referenced);
                        }
                    }
                    crate::SliceSource::ConstData(_) => {}
                }
                value(*start, referenced);
                value(*len, referenced);
            }
            Rvalue::SliceLoad { slice, index, .. } => {
                value(*slice, referenced);
                value(*index, referenced);
            }
            Rvalue::BufferLen(_)
            | Rvalue::BufferChannels(_)
            | Rvalue::BufferSampleRate(_)
            | Rvalue::BufferParamLen(_)
            | Rvalue::BufferParamChannels(_)
            | Rvalue::BufferParamSampleRate(_) => {}
        }
    }
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign {
                destination,
                value: v,
            } => {
                place(destination, false, referenced);
                rvalue(v, referenced);
            }
            StatementKind::Call { args, .. } => {
                for argument in args {
                    match argument {
                        CallArgument::Value(v) => value(*v, referenced),
                        CallArgument::Place(p) => place(p, true, referenced),
                        CallArgument::ArrayWindow { array, start, .. } => {
                            place(array, true, referenced);
                            value(*start, referenced);
                        }
                        CallArgument::SliceElement { slice, index, .. } => {
                            value(*slice, referenced);
                            value(*index, referenced);
                        }
                        CallArgument::SliceWindow { slice, start, .. } => {
                            value(*slice, referenced);
                            value(*start, referenced);
                        }
                        CallArgument::Buffer(_) => {}
                    }
                }
            }
            StatementKind::OutputStore {
                element,
                frame,
                value: v,
                ..
            } => {
                if let Some(v) = element {
                    value(*v, referenced);
                }
                value(*frame, referenced);
                value(*v, referenced);
            }
            StatementKind::ControlOutputStore {
                element, value: v, ..
            } => {
                if let Some(v) = element {
                    value(*v, referenced);
                }
                value(*v, referenced);
            }
            StatementKind::BufferStore {
                channel,
                index,
                value: v,
                ..
            }
            | StatementKind::BufferParamStore {
                channel,
                index,
                value: v,
                ..
            } => {
                if let Some(v) = channel {
                    value(*v, referenced);
                }
                value(*index, referenced);
                value(*v, referenced);
            }
            StatementKind::SliceStore {
                slice,
                index,
                value: v,
                ..
            } => {
                value(*slice, referenced);
                value(*index, referenced);
                value(*v, referenced);
            }
            StatementKind::SliceFill {
                destination,
                value: v,
            } => {
                value(*destination, referenced);
                value(*v, referenced);
            }
            StatementKind::SliceCopy {
                destination,
                source,
            } => {
                value(*destination, referenced);
                value(*source, referenced);
            }
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                value(*condition, referenced);
                collect_read_references(then_block, referenced);
                collect_read_references(else_block, referenced);
            }
            StatementKind::Loop { body } => collect_read_references(body, referenced),
            StatementKind::Return { values } => {
                for v in values {
                    value(*v, referenced);
                }
            }
            StatementKind::Break | StatementKind::Continue => {}
        }
    }
}

fn collect_block_writes(block: &Block, referenced: &mut HashSet<LocalId>) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, .. } => {
                if let PlaceBase::Local(local) = destination.base {
                    referenced.insert(local);
                }
            }
            StatementKind::Call { results, .. } => {
                referenced.extend(results.iter().copied());
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_writes(then_block, referenced);
                collect_block_writes(else_block, referenced);
            }
            StatementKind::Loop { body } => collect_block_writes(body, referenced),
            _ => {}
        }
    }
}

fn rewrite_block_locals(block: &mut Block, mapping: &[Option<LocalId>]) {
    for statement in &mut block.statements {
        rewrite_statement_locals(statement, mapping);
    }
}

fn rewrite_value(value: &mut Value, mapping: &[Option<LocalId>]) {
    if let Value::Local(local) = value {
        *local = mapping[local.index()].expect("referenced local retained during reindexing");
    }
}

fn rewrite_place(place: &mut Place, mapping: &[Option<LocalId>]) {
    if let PlaceBase::Local(local) = &mut place.base {
        *local = mapping[local.index()].expect("referenced local retained during reindexing");
    }
    for projection in &mut place.projections {
        if let Projection::Index { index, .. } = projection {
            rewrite_value(index, mapping);
        }
    }
}

fn rewrite_rvalue(value: &mut Rvalue, mapping: &[Option<LocalId>]) {
    match value {
        Rvalue::Use(value) | Rvalue::SliceLen(value) => rewrite_value(value, mapping),
        Rvalue::Load(place) => rewrite_place(place, mapping),
        Rvalue::Unary { operand, .. } => rewrite_value(operand, mapping),
        Rvalue::Binary { lhs, rhs, .. } | Rvalue::Compare { lhs, rhs, .. } => {
            rewrite_value(lhs, mapping);
            rewrite_value(rhs, mapping);
        }
        Rvalue::Cast { value, .. } => rewrite_value(value, mapping),
        Rvalue::Intrinsic { args, .. } => {
            for value in args {
                rewrite_value(value, mapping);
            }
        }
        Rvalue::ProcessFrame { offset } => rewrite_value(offset, mapping),
        Rvalue::InputLoad { element, frame, .. } | Rvalue::OutputLoad { element, frame, .. } => {
            if let Some(element) = element {
                rewrite_value(element, mapping);
            }
            rewrite_value(frame, mapping);
        }
        Rvalue::BufferLoad { channel, index, .. }
        | Rvalue::BufferParamLoad { channel, index, .. } => {
            if let Some(channel) = channel {
                rewrite_value(channel, mapping);
            }
            rewrite_value(index, mapping);
        }
        Rvalue::ConstDataLoad { index, .. } => rewrite_value(index, mapping),
        Rvalue::MakeSlice {
            source, start, len, ..
        } => {
            match source {
                crate::SliceSource::Place(place) => rewrite_place(place, mapping),
                crate::SliceSource::Buffer { channel, .. }
                | crate::SliceSource::BufferParam { channel, .. } => {
                    if let Some(channel) = channel {
                        rewrite_value(channel, mapping);
                    }
                }
                crate::SliceSource::ConstData(_) => {}
            }
            rewrite_value(start, mapping);
            rewrite_value(len, mapping);
        }
        Rvalue::SliceLoad { slice, index, .. } => {
            rewrite_value(slice, mapping);
            rewrite_value(index, mapping);
        }
        Rvalue::BufferLen(_)
        | Rvalue::BufferChannels(_)
        | Rvalue::BufferSampleRate(_)
        | Rvalue::BufferParamLen(_)
        | Rvalue::BufferParamChannels(_)
        | Rvalue::BufferParamSampleRate(_) => {}
    }
}

fn rewrite_statement_locals(statement: &mut Statement, mapping: &[Option<LocalId>]) {
    match &mut statement.kind {
        StatementKind::Assign { destination, value } => {
            rewrite_place(destination, mapping);
            rewrite_rvalue(value, mapping);
        }
        StatementKind::Call { results, args, .. } => {
            for result in results {
                *result = mapping[result.index()].expect("call result local retained");
            }
            for argument in args {
                match argument {
                    CallArgument::Value(value) => rewrite_value(value, mapping),
                    CallArgument::Place(place) => rewrite_place(place, mapping),
                    CallArgument::ArrayWindow { array, start, .. } => {
                        rewrite_place(array, mapping);
                        rewrite_value(start, mapping);
                    }
                    CallArgument::SliceElement { slice, index, .. } => {
                        rewrite_value(slice, mapping);
                        rewrite_value(index, mapping);
                    }
                    CallArgument::SliceWindow { slice, start, .. } => {
                        rewrite_value(slice, mapping);
                        rewrite_value(start, mapping);
                    }
                    CallArgument::Buffer(_) => {}
                }
            }
        }
        StatementKind::OutputStore {
            element,
            frame,
            value,
            ..
        } => {
            if let Some(element) = element {
                rewrite_value(element, mapping);
            }
            rewrite_value(frame, mapping);
            rewrite_value(value, mapping);
        }
        StatementKind::ControlOutputStore { element, value, .. } => {
            if let Some(element) = element {
                rewrite_value(element, mapping);
            }
            rewrite_value(value, mapping);
        }
        StatementKind::BufferStore {
            channel,
            index,
            value,
            ..
        }
        | StatementKind::BufferParamStore {
            channel,
            index,
            value,
            ..
        } => {
            if let Some(channel) = channel {
                rewrite_value(channel, mapping);
            }
            rewrite_value(index, mapping);
            rewrite_value(value, mapping);
        }
        StatementKind::SliceStore {
            slice,
            index,
            value,
            ..
        } => {
            rewrite_value(slice, mapping);
            rewrite_value(index, mapping);
            rewrite_value(value, mapping);
        }
        StatementKind::SliceFill { destination, value } => {
            rewrite_value(destination, mapping);
            rewrite_value(value, mapping);
        }
        StatementKind::SliceCopy {
            destination,
            source,
        } => {
            rewrite_value(destination, mapping);
            rewrite_value(source, mapping);
        }
        StatementKind::If {
            condition,
            then_block,
            else_block,
        } => {
            rewrite_value(condition, mapping);
            rewrite_block_locals(then_block, mapping);
            rewrite_block_locals(else_block, mapping);
        }
        StatementKind::Loop { body } => rewrite_block_locals(body, mapping),
        StatementKind::Return { values } => {
            for value in values {
                rewrite_value(value, mapping);
            }
        }
        StatementKind::Break | StatementKind::Continue => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        process_function_params, AccessMode, Block, BoundsMode, CompileConfig, Function,
        FunctionAttributes, FunctionId, FunctionKind, Local, Place, PlaceBase, Program, Projection,
        Rvalue, ScalarType, ScalarValue, SliceSource, SourceSpan, StateId, StatePersistence,
        StateSlot, Statement, StatementKind, Type, TypeId, Value,
    };

    use super::*;

    fn function(name: &str, kind: FunctionKind) -> Function {
        Function {
            name: name.to_owned(),
            kind,
            attributes: FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        }
    }

    fn empty_program() -> Program {
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.push(Type::Scalar(ScalarType::I32));
        let mut process = function("process", FunctionKind::Process);
        process.params = process_function_params(TypeId::new(0));
        program.functions = vec![function("init", FunctionKind::Init), process];
        program
    }

    #[test]
    fn numeric_constant_folding_matches_mir_edge_semantics() {
        assert_eq!(
            fold_binary(
                BinaryOp::ShiftLeft,
                ScalarValue::I32(1),
                ScalarValue::I32(32),
            ),
            Some(ScalarValue::I32(1))
        );
        assert_eq!(
            fold_binary(
                BinaryOp::Divide,
                ScalarValue::I32(i32::MIN),
                ScalarValue::I32(-1),
            ),
            Some(ScalarValue::I32(i32::MIN))
        );
        assert_eq!(
            fold_binary(
                BinaryOp::Remainder,
                ScalarValue::I64(i64::MIN),
                ScalarValue::I64(-1),
            ),
            Some(ScalarValue::I64(0))
        );
        assert_eq!(
            fold_binary(BinaryOp::Divide, ScalarValue::I32(1), ScalarValue::I32(0),),
            None
        );
        assert_eq!(
            fold_compare(
                CompareOp::NotEqual,
                ScalarValue::F32(f32::NAN),
                ScalarValue::F32(0.0),
            ),
            Some(true)
        );
        assert_eq!(
            fold_compare(
                CompareOp::Less,
                ScalarValue::F64(f64::NAN),
                ScalarValue::F64(0.0),
            ),
            Some(false)
        );
        assert_eq!(
            fold_cast(ScalarValue::F32(f32::NAN), ScalarType::I32),
            Some(ScalarValue::I32(0))
        );
        assert_eq!(
            fold_cast(ScalarValue::F64(f64::INFINITY), ScalarType::I32),
            Some(ScalarValue::I32(i32::MAX))
        );
        assert_eq!(
            fold_intrinsic(Intrinsic::Abs, &[ScalarValue::I32(i32::MIN)]),
            Some(ScalarValue::I32(i32::MIN))
        );
        assert_eq!(
            fold_intrinsic(Intrinsic::Round, &[ScalarValue::F32(-1.5)],),
            Some(ScalarValue::F32(-2.0))
        );
        let minimum = fold_intrinsic(
            Intrinsic::Min,
            &[ScalarValue::F32(0.0), ScalarValue::F32(-0.0)],
        )
        .expect("finite minimum should fold");
        let maximum = fold_intrinsic(
            Intrinsic::Max,
            &[ScalarValue::F64(-0.0), ScalarValue::F64(0.0)],
        )
        .expect("finite maximum should fold");
        assert!(
            matches!(minimum, ScalarValue::F32(value) if value.to_bits() == (-0.0_f32).to_bits())
        );
        assert!(matches!(maximum, ScalarValue::F64(value) if value.to_bits() == 0.0_f64.to_bits()));
        assert_eq!(
            fold_intrinsic(
                Intrinsic::Min,
                &[ScalarValue::F32(f32::NAN), ScalarValue::F32(1.0)],
            ),
            None
        );
        assert!(!scalar_constants_identical(
            ScalarValue::F32(0.0),
            ScalarValue::F32(-0.0)
        ));
        assert!(scalar_constants_identical(
            ScalarValue::F64(f64::from_bits(0x7ff8_0000_0000_0001)),
            ScalarValue::F64(f64::from_bits(0x7ff8_0000_0000_0001))
        ));
    }

    #[test]
    fn propagates_constant_chains_and_merges_equal_branch_facts() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::Bool));
        let mut user = function("branch_constants", FunctionKind::User);
        user.params.push(crate::FunctionParam {
            name: "condition".to_owned(),
            ty: TypeId::new(1),
            mode: PassingMode::Value,
        });
        user.results.push(TypeId::new(0));
        user.locals.extend([
            Local {
                name: Some("condition".to_owned()),
                ty: TypeId::new(1),
            },
            Local {
                name: Some("seed".to_owned()),
                ty: TypeId::new(0),
            },
            Local {
                name: Some("merged".to_owned()),
                ty: TypeId::new(0),
            },
            Local {
                name: Some("result".to_owned()),
                ty: TypeId::new(0),
            },
        ]);
        let assign_local = |local, value| Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(local)),
                value,
            },
            source: SourceSpan::UNKNOWN,
        };
        user.body.statements.extend([
            assign_local(
                0,
                Rvalue::Load(Place {
                    base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                    projections: Vec::new(),
                }),
            ),
            assign_local(1, Rvalue::Use(Value::Constant(ScalarValue::I32(2)))),
            Statement {
                kind: StatementKind::If {
                    condition: Value::Local(LocalId::new(0)),
                    then_block: Block {
                        statements: vec![assign_local(
                            2,
                            Rvalue::Binary {
                                op: BinaryOp::Add,
                                lhs: Value::Local(LocalId::new(1)),
                                rhs: Value::Constant(ScalarValue::I32(5)),
                            },
                        )],
                    },
                    else_block: Block {
                        statements: vec![assign_local(
                            2,
                            Rvalue::Use(Value::Constant(ScalarValue::I32(7))),
                        )],
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            assign_local(
                3,
                Rvalue::Intrinsic {
                    intrinsic: Intrinsic::Max,
                    args: vec![
                        Value::Local(LocalId::new(2)),
                        Value::Constant(ScalarValue::I32(3)),
                    ],
                },
            ),
            Statement {
                kind: StatementKind::Return {
                    values: vec![Value::Local(LocalId::new(3))],
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);
        program.functions.push(user);

        let validated = crate::validate_owned(program).expect("constant-chain fixture is valid");
        let (optimized, stats) =
            super::optimize(validated).expect("constant propagation preserves validity");
        assert!(stats.propagated_values >= 3);
        assert!(stats.folded_rvalues >= 2);
        let user = &optimized.functions[2];
        let StatementKind::Return { values } = &user.body.statements.last().unwrap().kind else {
            panic!("optimized function should return")
        };
        assert_eq!(values, &[Value::Constant(ScalarValue::I32(7))]);
    }

    #[test]
    fn loop_writes_invalidate_incoming_constant_facts() {
        let mut program = empty_program();
        let mut user = function("loop_mutation", FunctionKind::User);
        user.results.push(TypeId::new(0));
        user.locals.push(Local {
            name: Some("value".to_owned()),
            ty: TypeId::new(0),
        });
        user.body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::Use(Value::Constant(ScalarValue::I32(1))),
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Loop {
                    body: Block {
                        statements: vec![
                            Statement {
                                kind: StatementKind::Assign {
                                    destination: Place::local(LocalId::new(0)),
                                    value: Rvalue::Binary {
                                        op: BinaryOp::Add,
                                        lhs: Value::Local(LocalId::new(0)),
                                        rhs: Value::Constant(ScalarValue::I32(1)),
                                    },
                                },
                                source: SourceSpan::UNKNOWN,
                            },
                            Statement {
                                kind: StatementKind::Break,
                                source: SourceSpan::UNKNOWN,
                            },
                        ],
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Return {
                    values: vec![Value::Local(LocalId::new(0))],
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);
        program.functions.push(user);

        let validated = crate::validate_owned(program).expect("loop fixture is valid");
        let (optimized, _) =
            super::optimize(validated).expect("loop invalidation preserves validity");
        let user = &optimized.functions[2];
        let StatementKind::Loop { body } = &user.body.statements[1].kind else {
            panic!("loop should remain")
        };
        assert!(matches!(
            body.statements[0].kind,
            StatementKind::Assign {
                value: Rvalue::Binary {
                    lhs: Value::Local(_),
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            user.body.statements[2].kind,
            StatementKind::Return { ref values }
                if values == &[Value::Local(LocalId::new(0))]
        ));
    }

    #[test]
    fn readwrite_reference_calls_invalidate_local_constant_facts() {
        let mut program = empty_program();
        let mut mutate = function("mutate", FunctionKind::User);
        mutate.params.push(crate::FunctionParam {
            name: "value".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::ReadWriteReference,
        });
        mutate.body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                    projections: Vec::new(),
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(9))),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(mutate);

        let mut caller = function("caller", FunctionKind::User);
        caller.results.push(TypeId::new(0));
        caller.locals.push(Local {
            name: Some("value".to_owned()),
            ty: TypeId::new(0),
        });
        caller.body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::Use(Value::Constant(ScalarValue::I32(5))),
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(2),
                    args: vec![CallArgument::Place(Place::local(LocalId::new(0)))],
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Return {
                    values: vec![Value::Local(LocalId::new(0))],
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);
        program.functions.push(caller);

        let validated = crate::validate_owned(program).expect("reference-call fixture is valid");
        let (optimized, _) =
            super::optimize(validated).expect("reference invalidation preserves validity");
        let caller = &optimized.functions[3];
        assert!(matches!(
            caller.body.statements.last().unwrap().kind,
            StatementKind::Return { ref values }
                if values == &[Value::Local(LocalId::new(0))]
        ));
    }

    #[test]
    fn optimize_simplifies_structured_control_flow_and_reindexes_locals() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        let mut user = function("choose", FunctionKind::User);
        user.results.push(TypeId::new(1));
        user.locals.extend([
            Local {
                name: Some("dead".to_owned()),
                ty: TypeId::new(1),
            },
            Local {
                name: Some("result".to_owned()),
                ty: TypeId::new(1),
            },
            Local {
                name: Some("unreachable".to_owned()),
                ty: TypeId::new(1),
            },
        ]);
        let assign_result = |value| Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(1)),
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(value))),
            },
            source: SourceSpan::UNKNOWN,
        };
        user.body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::Binary {
                        op: BinaryOp::Add,
                        lhs: Value::Constant(ScalarValue::F32(1.0)),
                        rhs: Value::Constant(ScalarValue::F32(2.0)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::If {
                    condition: Value::Constant(ScalarValue::Bool(true)),
                    then_block: Block {
                        statements: vec![assign_result(4.0)],
                    },
                    else_block: Block {
                        statements: vec![assign_result(5.0)],
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Return {
                    values: vec![Value::Local(LocalId::new(1))],
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(2)),
                    value: Rvalue::Use(Value::Constant(ScalarValue::F32(9.0))),
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);
        program.functions.push(user);

        let validated = crate::validate_owned(program).expect("fixture is valid before cleanup");
        let (optimized, stats) = super::optimize(validated).expect("passes preserve validity");
        assert!(stats.folded_rvalues >= 1);
        assert_eq!(stats.simplified_branches, 1);
        assert_eq!(stats.removed_unreachable_statements, 1);
        assert!(stats.removed_dead_assignments >= 1);
        assert_eq!(optimized.functions[2].locals.len(), 0);
        let StatementKind::Return { values } =
            &optimized.functions[2].body.statements.last().unwrap().kind
        else {
            panic!("optimized function should end in return")
        };
        assert_eq!(values, &[Value::Constant(ScalarValue::F32(4.0))]);
    }

    #[test]
    fn optimize_reaches_fixed_point_beyond_sixteen_dead_chain_rounds() {
        const PURE_CHAIN_LEN: u32 = 32;

        let mut program = empty_program();
        for index in 0..=PURE_CHAIN_LEN {
            program.functions[1].locals.push(Local {
                name: Some(format!("chain_{index}")),
                ty: TypeId::new(0),
            });
        }
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Load(Place {
                    base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                    projections: Vec::new(),
                }),
            },
            source: SourceSpan::UNKNOWN,
        });
        for index in 1..=PURE_CHAIN_LEN {
            program.functions[1].body.statements.push(Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(index)),
                    value: Rvalue::Use(Value::Local(LocalId::new(index - 1))),
                },
                source: SourceSpan::UNKNOWN,
            });
        }

        let validated = crate::validate_owned(program).expect("dead-chain fixture is valid");
        let (optimized, stats) =
            super::optimize(validated).expect("monotonic cleanup must converge");
        assert!(
            stats.iterations > 16,
            "regression must exercise convergence beyond the former cap"
        );
        assert_eq!(optimized.functions[1].locals.len(), 1);
        assert_eq!(optimized.functions[1].body.statements.len(), 1);

        let fixed_point = optimized.as_program().clone();
        let (second, second_stats) = super::optimize(optimized.into_validated())
            .expect("an optimized program should already be at the fixed point");
        assert_eq!(second.as_program(), &fixed_point);
        assert_eq!(second_stats.iterations, 1);
        assert_eq!(second_stats.propagated_values, 0);
        assert_eq!(second_stats.folded_rvalues, 0);
        assert_eq!(second_stats.simplified_branches, 0);
        assert_eq!(second_stats.removed_unreachable_statements, 0);
        assert_eq!(second_stats.removed_dead_assignments, 0);
        assert_eq!(second_stats.removed_redundant_zero_stores, 0);
        assert_eq!(second_stats.removed_locals, 0);
    }

    #[test]
    fn optimize_removes_only_proven_redundant_zero_before_init_stores() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Array {
                element: TypeId::new(1),
                len: 2,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadWrite,
            },
        ]);
        program.state.extend([
            StateSlot {
                name: "scalar".to_owned(),
                ty: TypeId::new(1),
                persistence: StatePersistence::Snapshot,
            },
            StateSlot {
                name: "array".to_owned(),
                ty: TypeId::new(2),
                persistence: StatePersistence::Snapshot,
            },
        ]);
        program.functions[0].locals.push(Local {
            name: Some("array_view".to_owned()),
            ty: TypeId::new(3),
        });
        let state_place = |state| Place {
            base: PlaceBase::State(StateId::new(state)),
            projections: Vec::new(),
        };
        let array_element = |index| Place {
            base: PlaceBase::State(StateId::new(1)),
            projections: vec![Projection::Index {
                index: Value::Constant(ScalarValue::I32(index)),
                bounds: BoundsMode::Unchecked,
            }],
        };
        let assign = |destination, value| Statement {
            kind: StatementKind::Assign {
                destination,
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(value))),
            },
            source: SourceSpan::UNKNOWN,
        };
        program.functions[0].body.statements.extend([
            assign(state_place(0), 0.0),
            assign(array_element(0), 0.0),
            assign(array_element(1), 0.0),
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::MakeSlice {
                        source: SliceSource::Place(state_place(1)),
                        start: Value::Constant(ScalarValue::I32(0)),
                        len: Value::Constant(ScalarValue::I32(2)),
                        bounds: BoundsMode::Unchecked,
                        access: AccessMode::ReadWrite,
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::SliceFill {
                    destination: Value::Local(LocalId::new(0)),
                    value: Value::Constant(ScalarValue::F32(0.0)),
                },
                source: SourceSpan::UNKNOWN,
            },
            assign(state_place(0), -0.0),
            assign(state_place(0), 1.0),
            assign(state_place(0), 0.0),
            assign(state_place(0), 0.0),
        ]);

        let validated = unsafe { crate::validate_owned_with_producer_proofs(program) }
            .expect("init fixture is valid");
        let (optimized, stats) = super::optimize(validated).expect("zero DSE preserves validity");
        assert_eq!(stats.removed_redundant_zero_stores, 5);
        let init = &optimized.functions[0];
        assert!(init.body.statements.iter().any(|statement| {
            matches!(
                statement.kind,
                StatementKind::Assign {
                    value: Rvalue::Use(Value::Constant(ScalarValue::F32(value))),
                    ..
                } if value.to_bits() == (-0.0_f32).to_bits()
            )
        }));
        let scalar_writes = init
            .body
            .statements
            .iter()
            .filter(|statement| {
                matches!(
                    statement.kind,
                    StatementKind::Assign {
                        destination: Place {
                            base: PlaceBase::State(state),
                            ..
                        },
                        ..
                    } if state.raw() == 0
                )
            })
            .count();
        assert_eq!(scalar_writes, 3);
    }

    #[test]
    fn zero_before_init_elimination_stops_at_calls() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.state.push(StateSlot {
            name: "value".to_owned(),
            ty: TypeId::new(1),
            persistence: StatePersistence::Snapshot,
        });
        program
            .functions
            .push(function("unknown_effect", FunctionKind::User));
        program.functions[0].body.statements.extend([
            Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(2),
                    args: Vec::new(),
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::State(StateId::new(0)),
                        projections: Vec::new(),
                    },
                    value: Rvalue::Use(Value::Constant(ScalarValue::F32(0.0))),
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        let validated = crate::validate_owned(program).expect("call barrier fixture is valid");
        let (optimized, stats) = super::optimize(validated).expect("optimization should succeed");
        assert_eq!(stats.removed_redundant_zero_stores, 0);
        assert_eq!(optimized.functions[0].body.statements.len(), 2);
    }
}
