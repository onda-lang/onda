use std::collections::{HashMap, HashSet};
use std::ops::Deref;

use crate::{
    BinaryOp, Block, CallArgument, CompareOp, Function, Intrinsic, LocalId, PassingMode, Place,
    PlaceBase, Projection, Rvalue, ScalarType, ScalarValue, Statement, StatementKind,
    ValidatedProgram, ValidationError, Value,
};

mod bounds_proofs;
mod cse;
pub(crate) mod parameter_pruning;
mod state_promotion;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct PassStats {
    pub iterations: u32,
    pub propagated_values: u64,
    pub propagated_copies: u64,
    pub folded_rvalues: u64,
    pub simplified_branches: u64,
    pub removed_unreachable_statements: u64,
    pub removed_dead_assignments: u64,
    pub removed_redundant_zero_stores: u64,
    pub removed_locals: u64,
    pub promoted_state_slots: u64,
    pub eliminated_common_subexpressions: u64,
    pub algebraic_simplifications: u64,
    pub eliminated_bounds_checks: u64,
    pub removed_function_parameters: u64,
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
        self.propagated_copies = self
            .propagated_copies
            .saturating_add(other.propagated_copies);
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
        self.promoted_state_slots = self
            .promoted_state_slots
            .saturating_add(other.promoted_state_slots);
        self.eliminated_common_subexpressions = self
            .eliminated_common_subexpressions
            .saturating_add(other.eliminated_common_subexpressions);
        self.algebraic_simplifications = self
            .algebraic_simplifications
            .saturating_add(other.algebraic_simplifications);
        self.eliminated_bounds_checks = self
            .eliminated_bounds_checks
            .saturating_add(other.eliminated_bounds_checks);
        self.removed_function_parameters = self
            .removed_function_parameters
            .saturating_add(other.removed_function_parameters);
    }

    fn changed(self) -> bool {
        self.propagated_values != 0
            || self.propagated_copies != 0
            || self.folded_rvalues != 0
            || self.simplified_branches != 0
            || self.removed_unreachable_statements != 0
            || self.removed_dead_assignments != 0
            || self.removed_redundant_zero_stores != 0
            || self.removed_locals != 0
            || self.promoted_state_slots != 0
            || self.eliminated_common_subexpressions != 0
            || self.algebraic_simplifications != 0
            || self.eliminated_bounds_checks != 0
            || self.removed_function_parameters != 0
    }
}

/// Performs one structured canonicalization round and revalidates the result.
pub fn canonicalize(
    program: ValidatedProgram,
) -> Result<(ValidatedProgram, PassStats), Vec<ValidationError>> {
    let producer_proofs = program.producer_proofs();
    let mut program = program.into_program();
    let mut stats = PassStats {
        iterations: 1,
        ..PassStats::default()
    };
    canonicalize_program(&mut program, &mut stats);
    crate::validate::revalidate_owned(program, producer_proofs).map(|program| (program, stats))
}

fn canonicalize_program(program: &mut crate::Program, stats: &mut PassStats) {
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
    let types = program.types.clone();
    for function in &mut program.functions {
        propagate_local_values(function, &passing_modes, stats);
        simplify_algebraic_identities(function, &types, stats);
        cse::eliminate_common_subexpressions(function, &passing_modes, stats);
        canonicalize_block(&mut function.body, stats);
    }
}

/// Runs backend-neutral MIR cleanup to a fixed point while retaining the
/// structured, non-SSA representation.
pub fn optimize(
    program: ValidatedProgram,
) -> Result<(OptimizedProgram, PassStats), Vec<ValidationError>> {
    let mut producer_proofs = program.producer_proofs();
    let mut raw = program.into_program();
    // Prune the already-dead ABI surface before state promotion so an unused
    // reference argument cannot manufacture a promoted load/store pair.
    // Cleanup may expose more unused parameters later, so pruning also remains
    // part of the fixed point below.
    let mut total = PassStats {
        removed_function_parameters: parameter_pruning::prune(&mut raw),
        ..PassStats::default()
    };
    // State promotion may add locals or statements, so run it once before the
    // monotonic cleanup fixed point.
    state_promotion::promote_process_scalar_state(&mut raw, &mut total);
    // Every round is monotonic: it only replaces values, rvalues, or bounds
    // modes with stronger canonical forms, or removes branches, statements,
    // and locals. No pass adds executable structure, so the finite program
    // must reach a fixed point without an arbitrary iteration cap. These are
    // one trusted internal pipeline, so retain the proof status during the
    // fixed point and validate the completed program once rather than
    // rescanning a large MIR after each cleanup round.
    loop {
        let mut stats = PassStats {
            iterations: 1,
            ..PassStats::default()
        };
        // `optimize` owns the complete round, so canonicalization and dead
        // cleanup share one validation boundary instead of validating the
        // same intermediate program twice.
        canonicalize_program(&mut raw, &mut stats);
        if let Some(init) = raw.functions.get_mut(raw.entry_points.init.index()) {
            eliminate_preinitialized_zero_stores(init, &mut stats);
        }
        for function in &mut raw.functions {
            remove_dead_pure_locals(function, &mut stats);
        }
        // Cleanup can remove assignments that widened a whole-function range
        // or expose constant indices. Prove bounds afterward in every round so
        // those opportunities are not permanently missed.
        if bounds_proofs::eliminate_proven_bounds_checks(&mut raw, &mut stats) {
            producer_proofs = crate::validate::ProducerProofStatus::Trusted;
        }
        // Earlier cleanup can remove the final use of a parameter. Pruning it
        // here can in turn expose dead argument preparation in callers, which
        // the next round will remove.
        stats.removed_function_parameters = parameter_pruning::prune(&mut raw);
        let changed = stats.changed();
        total.merge(stats);
        if !changed {
            let program = crate::validate::revalidate_owned(raw, producer_proofs)?;
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

fn stable_local_values(function: &Function, passing_modes: &[Vec<PassingMode>]) -> Vec<bool> {
    let mut writes = vec![0_u32; function.locals.len()];
    let mut unstable = vec![false; function.locals.len()];
    collect_local_stability(
        &function.body,
        false,
        passing_modes,
        &mut writes,
        &mut unstable,
    );
    writes
        .into_iter()
        .zip(unstable)
        .map(|(writes, unstable)| writes == 1 && !unstable)
        .collect()
}

fn collect_local_stability(
    block: &Block,
    inside_loop: bool,
    passing_modes: &[Vec<PassingMode>],
    writes: &mut [u32],
    unstable: &mut [bool],
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, .. } => {
                if let PlaceBase::Local(local) = destination.base {
                    record_local_stability_write(
                        local,
                        inside_loop,
                        !destination.projections.is_empty(),
                        writes,
                        unstable,
                    );
                }
            }
            StatementKind::Call {
                results,
                function,
                args,
            } => {
                for result in results {
                    record_local_stability_write(*result, inside_loop, false, writes, unstable);
                }
                for (index, argument) in args.iter().enumerate() {
                    if passing_modes
                        .get(function.index())
                        .and_then(|modes| modes.get(index))
                        == Some(&PassingMode::ReadWriteReference)
                    {
                        if let Some(local) = mutated_argument_local(argument) {
                            record_local_stability_write(
                                local,
                                inside_loop,
                                true,
                                writes,
                                unstable,
                            );
                        }
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_local_stability(then_block, inside_loop, passing_modes, writes, unstable);
                collect_local_stability(else_block, inside_loop, passing_modes, writes, unstable);
            }
            StatementKind::Loop { body } => {
                collect_local_stability(body, true, passing_modes, writes, unstable)
            }
            _ => {}
        }
    }
}

fn record_local_stability_write(
    local: LocalId,
    inside_loop: bool,
    through_reference: bool,
    writes: &mut [u32],
    unstable: &mut [bool],
) {
    let index = local.index();
    writes[index] = writes[index].saturating_add(1);
    unstable[index] |= inside_loop || through_reference;
}

fn simplify_algebraic_identities(
    function: &mut Function,
    types: &[crate::Type],
    stats: &mut PassStats,
) {
    let local_types = function
        .locals
        .iter()
        .map(|local| local.ty)
        .collect::<Vec<_>>();
    simplify_block_algebra(&mut function.body, &local_types, types, stats);
}

fn simplify_block_algebra(
    block: &mut Block,
    local_types: &[crate::TypeId],
    types: &[crate::Type],
    stats: &mut PassStats,
) {
    for statement in &mut block.statements {
        match &mut statement.kind {
            StatementKind::Assign { value, .. } => {
                if let Some(replacement) = simplify_rvalue_algebra(value, local_types, types) {
                    *value = Rvalue::Use(replacement);
                    stats.algebraic_simplifications =
                        stats.algebraic_simplifications.saturating_add(1);
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                simplify_block_algebra(then_block, local_types, types, stats);
                simplify_block_algebra(else_block, local_types, types, stats);
            }
            StatementKind::Loop { body } => simplify_block_algebra(body, local_types, types, stats),
            _ => {}
        }
    }
}

fn simplify_rvalue_algebra(
    value: &Rvalue,
    local_types: &[crate::TypeId],
    types: &[crate::Type],
) -> Option<Value> {
    match value {
        Rvalue::Binary { op, lhs, rhs } => {
            let scalar = scalar_type_of_value(*lhs, local_types, types)?;
            if !matches!(scalar, ScalarType::I32 | ScalarType::I64) {
                return None;
            }
            match op {
                BinaryOp::Add if scalar_value_is_zero(*rhs) => Some(*lhs),
                BinaryOp::Add if scalar_value_is_zero(*lhs) => Some(*rhs),
                BinaryOp::Subtract if scalar_value_is_zero(*rhs) => Some(*lhs),
                BinaryOp::Subtract if values_identical(*lhs, *rhs) => Some(integer_zero(scalar)),
                BinaryOp::Multiply if scalar_value_is_one(*rhs) => Some(*lhs),
                BinaryOp::Multiply if scalar_value_is_one(*lhs) => Some(*rhs),
                BinaryOp::Multiply if scalar_value_is_zero(*lhs) || scalar_value_is_zero(*rhs) => {
                    Some(integer_zero(scalar))
                }
                BinaryOp::Divide if scalar_value_is_one(*rhs) => Some(*lhs),
                BinaryOp::Remainder if scalar_value_is_one(*rhs) => Some(integer_zero(scalar)),
                BinaryOp::BitAnd if scalar_value_is_zero(*lhs) || scalar_value_is_zero(*rhs) => {
                    Some(integer_zero(scalar))
                }
                BinaryOp::BitAnd if scalar_value_is_all_ones(*rhs) => Some(*lhs),
                BinaryOp::BitAnd if scalar_value_is_all_ones(*lhs) => Some(*rhs),
                BinaryOp::BitOr | BinaryOp::BitXor if scalar_value_is_zero(*rhs) => Some(*lhs),
                BinaryOp::BitOr | BinaryOp::BitXor if scalar_value_is_zero(*lhs) => Some(*rhs),
                BinaryOp::BitXor if values_identical(*lhs, *rhs) => Some(integer_zero(scalar)),
                BinaryOp::ShiftLeft | BinaryOp::ShiftRight if scalar_value_is_zero(*rhs) => {
                    Some(*lhs)
                }
                _ => None,
            }
        }
        Rvalue::Compare { op, lhs, rhs } if values_identical(*lhs, *rhs) => {
            let scalar = scalar_type_of_value(*lhs, local_types, types)?;
            if matches!(scalar, ScalarType::F32 | ScalarType::F64) {
                // NaNs make self-comparisons non-reflexive, and replacing the
                // operation could also discard signaling behavior.
                return None;
            }
            let result = match op {
                CompareOp::Equal | CompareOp::LessEqual | CompareOp::GreaterEqual => true,
                CompareOp::NotEqual | CompareOp::Less | CompareOp::Greater => false,
            };
            Some(Value::Constant(ScalarValue::Bool(result)))
        }
        _ => None,
    }
}

fn scalar_type_of_value(
    value: Value,
    local_types: &[crate::TypeId],
    types: &[crate::Type],
) -> Option<ScalarType> {
    match value {
        Value::Constant(value) => Some(value.ty()),
        Value::Local(local) => match types.get(local_types[local.index()].index()) {
            Some(crate::Type::Scalar(scalar)) => Some(*scalar),
            _ => None,
        },
    }
}

fn scalar_value_is_zero(value: Value) -> bool {
    matches!(
        value,
        Value::Constant(ScalarValue::I32(0) | ScalarValue::I64(0))
    )
}

fn scalar_value_is_one(value: Value) -> bool {
    matches!(
        value,
        Value::Constant(ScalarValue::I32(1) | ScalarValue::I64(1))
    )
}

fn scalar_value_is_all_ones(value: Value) -> bool {
    matches!(
        value,
        Value::Constant(ScalarValue::I32(-1) | ScalarValue::I64(-1))
    )
}

fn integer_zero(scalar: ScalarType) -> Value {
    match scalar {
        ScalarType::I32 => Value::Constant(ScalarValue::I32(0)),
        ScalarType::I64 => Value::Constant(ScalarValue::I64(0)),
        _ => unreachable!("algebraic integer identity requires an integer type"),
    }
}

fn propagate_local_values(
    function: &mut Function,
    passing_modes: &[Vec<PassingMode>],
    stats: &mut PassStats,
) {
    let stable_locals = stable_local_values(function, passing_modes);
    let mut facts = vec![None; function.locals.len()];
    propagate_block_values(
        &mut function.body,
        &mut facts,
        &stable_locals,
        passing_modes,
        stats,
    );
}

fn propagate_block_values(
    block: &mut Block,
    facts: &mut Vec<Option<Value>>,
    stable_locals: &[bool],
    passing_modes: &[Vec<PassingMode>],
    stats: &mut PassStats,
) -> bool {
    let mut falls_through = true;
    for statement in &mut block.statements {
        if !falls_through {
            break;
        }
        falls_through =
            propagate_statement_values(statement, facts, stable_locals, passing_modes, stats);
    }
    falls_through
}

fn propagate_statement_values(
    statement: &mut Statement,
    facts: &mut Vec<Option<Value>>,
    stable_locals: &[bool],
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
                        Rvalue::Use(value @ Value::Constant(_)) => Some(*value),
                        Rvalue::Use(Value::Local(local)) if stable_locals[local.index()] => {
                            Some(Value::Local(*local))
                        }
                        _ => fold_rvalue(value).map(Value::Constant),
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
            buffer,
            channel,
            index,
            value,
            ..
        } => {
            propagate_buffer_ref(buffer, facts, stats);
            propagate_optional_value(channel, facts, stats);
            propagate_value(index, facts, stats);
            propagate_value(value, facts, stats);
            true
        }
        StatementKind::BufferParamStore {
            parameter,
            channel,
            index,
            value,
            ..
        } => {
            propagate_buffer_param_ref(parameter, facts, stats);
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
            let then_falls = propagate_block_values(
                then_block,
                &mut then_facts,
                stable_locals,
                passing_modes,
                stats,
            );
            let else_falls = propagate_block_values(
                else_block,
                &mut else_facts,
                stable_locals,
                passing_modes,
                stats,
            );
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
                        merge_value_facts(facts, &then_facts, &else_facts);
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
            propagate_block_values(body, &mut body_facts, stable_locals, passing_modes, stats);
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
    facts: &[Option<Value>],
    stats: &mut PassStats,
) {
    if let Some(value) = value {
        propagate_value(value, facts, stats);
    }
}

fn propagate_value(value: &mut Value, facts: &[Option<Value>], stats: &mut PassStats) {
    let original = *value;
    let mut resolved = original;
    for _ in 0..facts.len() {
        let Value::Local(local) = resolved else {
            break;
        };
        let Some(next) = facts.get(local.index()).and_then(|fact| *fact) else {
            break;
        };
        if values_identical(next, resolved) {
            break;
        }
        resolved = next;
    }
    if values_identical(original, resolved) {
        return;
    }
    *value = resolved;
    stats.propagated_values = stats.propagated_values.saturating_add(1);
    if matches!(resolved, Value::Local(_)) {
        stats.propagated_copies = stats.propagated_copies.saturating_add(1);
    }
}

fn propagate_buffer_ref(
    buffer: &mut crate::BufferRef,
    facts: &[Option<Value>],
    stats: &mut PassStats,
) {
    if let crate::BufferRef::ArrayElement { selector, .. } = buffer {
        propagate_value(selector, facts, stats);
    }
}

fn propagate_buffer_param_ref(
    buffer: &mut crate::BufferParamRef,
    facts: &[Option<Value>],
    stats: &mut PassStats,
) {
    if let crate::BufferParamRef::ArrayElement { selector, .. } = buffer {
        propagate_value(selector, facts, stats);
    }
}

fn propagate_place_indices(place: &mut Place, facts: &[Option<Value>], stats: &mut PassStats) {
    for projection in &mut place.projections {
        if let Projection::Index { index, .. } = projection {
            propagate_value(index, facts, stats);
        }
    }
}

fn propagate_rvalue_values(rvalue: &mut Rvalue, facts: &[Option<Value>], stats: &mut PassStats) {
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
        Rvalue::BufferLoad {
            buffer,
            channel,
            index,
            ..
        } => {
            propagate_buffer_ref(buffer, facts, stats);
            propagate_optional_value(channel, facts, stats);
            propagate_value(index, facts, stats);
        }
        Rvalue::BufferParamLoad {
            parameter,
            channel,
            index,
            ..
        } => {
            propagate_buffer_param_ref(parameter, facts, stats);
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
                crate::SliceSource::Buffer { buffer, channel } => {
                    propagate_buffer_ref(buffer, facts, stats);
                    propagate_optional_value(channel, facts, stats);
                }
                crate::SliceSource::BufferParam { parameter, channel } => {
                    propagate_buffer_param_ref(parameter, facts, stats);
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
        Rvalue::BufferLen(buffer)
        | Rvalue::BufferChannels(buffer)
        | Rvalue::BufferSampleRate(buffer) => propagate_buffer_ref(buffer, facts, stats),
        Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter) => {
            propagate_buffer_param_ref(parameter, facts, stats);
        }
    }
}

fn propagate_call_argument(
    argument: &mut CallArgument,
    facts: &[Option<Value>],
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
        CallArgument::Buffer(buffer) => propagate_buffer_ref(buffer, facts, stats),
        CallArgument::BufferParam(parameter) => {
            propagate_buffer_param_ref(parameter, facts, stats);
        }
        CallArgument::BufferSpan(_) => {}
    }
}

fn invalidate_fact(facts: &mut [Option<Value>], local: LocalId) {
    if let Some(fact) = facts.get_mut(local.index()) {
        *fact = None;
    }
}

fn merge_value_facts(
    destination: &mut [Option<Value>],
    lhs: &[Option<Value>],
    rhs: &[Option<Value>],
) {
    for (index, destination) in destination.iter_mut().enumerate() {
        *destination = match (
            lhs.get(index).copied().flatten(),
            rhs.get(index).copied().flatten(),
        ) {
            (Some(lhs), Some(rhs)) if values_identical(lhs, rhs) => Some(lhs),
            _ => None,
        };
    }
}

fn values_identical(lhs: Value, rhs: Value) -> bool {
    match (lhs, rhs) {
        (Value::Local(lhs), Value::Local(rhs)) => lhs == rhs,
        (Value::Constant(lhs), Value::Constant(rhs)) => scalar_constants_identical(lhs, rhs),
        _ => false,
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
        | CallArgument::Buffer(_)
        | CallArgument::BufferParam(_)
        | CallArgument::BufferSpan(_) => return None,
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

fn collect_buffer_ref_read(buffer: crate::BufferRef, reads: &mut [u32]) {
    if let crate::BufferRef::ArrayElement { selector, .. } = buffer {
        mark_value_read(selector, reads);
    }
}

fn collect_buffer_param_ref_read(buffer: crate::BufferParamRef, reads: &mut [u32]) {
    if let crate::BufferParamRef::ArrayElement { selector, .. } = buffer {
        mark_value_read(selector, reads);
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
        Rvalue::BufferLoad {
            buffer,
            channel,
            index,
            ..
        } => {
            collect_buffer_ref_read(*buffer, reads);
            if let Some(channel) = channel {
                mark_value_read(*channel, reads);
            }
            mark_value_read(*index, reads);
        }
        Rvalue::BufferParamLoad {
            parameter,
            channel,
            index,
            ..
        } => {
            collect_buffer_param_ref_read(*parameter, reads);
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
                crate::SliceSource::Buffer { buffer, channel } => {
                    collect_buffer_ref_read(*buffer, reads);
                    if let Some(channel) = channel {
                        mark_value_read(*channel, reads);
                    }
                }
                crate::SliceSource::BufferParam { parameter, channel } => {
                    collect_buffer_param_ref_read(*parameter, reads);
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
        Rvalue::BufferLen(buffer)
        | Rvalue::BufferChannels(buffer)
        | Rvalue::BufferSampleRate(buffer) => collect_buffer_ref_read(*buffer, reads),
        Rvalue::BufferParamLen(_)
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
                    CallArgument::Buffer(buffer) => collect_buffer_ref_read(*buffer, reads),
                    CallArgument::BufferParam(parameter) => {
                        collect_buffer_param_ref_read(*parameter, reads);
                    }
                    CallArgument::BufferSpan(_) => {}
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
            buffer,
            channel,
            index,
            value,
            ..
        } => {
            collect_buffer_ref_read(*buffer, reads);
            if let Some(channel) = channel {
                mark_value_read(*channel, reads);
            }
            mark_value_read(*index, reads);
            mark_value_read(*value, reads);
        }
        StatementKind::BufferParamStore {
            parameter,
            channel,
            index,
            value,
            ..
        } => {
            collect_buffer_param_ref_read(*parameter, reads);
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
    fn buffer_ref(buffer: crate::BufferRef, referenced: &mut HashSet<LocalId>) {
        if let crate::BufferRef::ArrayElement { selector, .. } = buffer {
            value(selector, referenced);
        }
    }
    fn buffer_param_ref(buffer: crate::BufferParamRef, referenced: &mut HashSet<LocalId>) {
        if let crate::BufferParamRef::ArrayElement { selector, .. } = buffer {
            value(selector, referenced);
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
            Rvalue::BufferLoad {
                buffer,
                channel,
                index,
                ..
            } => {
                buffer_ref(*buffer, referenced);
                if let Some(v) = channel {
                    value(*v, referenced);
                }
                value(*index, referenced);
            }
            Rvalue::BufferParamLoad {
                parameter,
                channel,
                index,
                ..
            } => {
                buffer_param_ref(*parameter, referenced);
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
                    crate::SliceSource::Buffer { buffer, channel } => {
                        buffer_ref(*buffer, referenced);
                        if let Some(v) = channel {
                            value(*v, referenced);
                        }
                    }
                    crate::SliceSource::BufferParam { parameter, channel } => {
                        buffer_param_ref(*parameter, referenced);
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
            Rvalue::BufferLen(buffer)
            | Rvalue::BufferChannels(buffer)
            | Rvalue::BufferSampleRate(buffer) => buffer_ref(*buffer, referenced),
            Rvalue::BufferParamLen(parameter)
            | Rvalue::BufferParamChannels(parameter)
            | Rvalue::BufferParamSampleRate(parameter) => {
                buffer_param_ref(*parameter, referenced);
            }
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
                        CallArgument::Buffer(buffer) => buffer_ref(*buffer, referenced),
                        CallArgument::BufferParam(parameter) => {
                            buffer_param_ref(*parameter, referenced);
                        }
                        CallArgument::BufferSpan(_) => {}
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
                buffer,
                channel,
                index,
                value: v,
                ..
            } => {
                buffer_ref(*buffer, referenced);
                if let Some(v) = channel {
                    value(*v, referenced);
                }
                value(*index, referenced);
                value(*v, referenced);
            }
            StatementKind::BufferParamStore {
                parameter,
                channel,
                index,
                value: v,
                ..
            } => {
                buffer_param_ref(*parameter, referenced);
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

fn rewrite_buffer_ref(buffer: &mut crate::BufferRef, mapping: &[Option<LocalId>]) {
    if let crate::BufferRef::ArrayElement { selector, .. } = buffer {
        rewrite_value(selector, mapping);
    }
}

fn rewrite_buffer_param_ref(buffer: &mut crate::BufferParamRef, mapping: &[Option<LocalId>]) {
    if let crate::BufferParamRef::ArrayElement { selector, .. } = buffer {
        rewrite_value(selector, mapping);
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
        Rvalue::BufferLoad {
            buffer,
            channel,
            index,
            ..
        } => {
            rewrite_buffer_ref(buffer, mapping);
            if let Some(channel) = channel {
                rewrite_value(channel, mapping);
            }
            rewrite_value(index, mapping);
        }
        Rvalue::BufferParamLoad {
            parameter,
            channel,
            index,
            ..
        } => {
            rewrite_buffer_param_ref(parameter, mapping);
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
                crate::SliceSource::Buffer { buffer, channel } => {
                    rewrite_buffer_ref(buffer, mapping);
                    if let Some(channel) = channel {
                        rewrite_value(channel, mapping);
                    }
                }
                crate::SliceSource::BufferParam { parameter, channel } => {
                    rewrite_buffer_param_ref(parameter, mapping);
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
        Rvalue::BufferLen(buffer)
        | Rvalue::BufferChannels(buffer)
        | Rvalue::BufferSampleRate(buffer) => rewrite_buffer_ref(buffer, mapping),
        Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter) => {
            rewrite_buffer_param_ref(parameter, mapping);
        }
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
                    CallArgument::Buffer(buffer) => rewrite_buffer_ref(buffer, mapping),
                    CallArgument::BufferParam(parameter) => {
                        rewrite_buffer_param_ref(parameter, mapping);
                    }
                    CallArgument::BufferSpan(_) => {}
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
            buffer,
            channel,
            index,
            value,
            ..
        } => {
            rewrite_buffer_ref(buffer, mapping);
            if let Some(channel) = channel {
                rewrite_value(channel, mapping);
            }
            rewrite_value(index, mapping);
            rewrite_value(value, mapping);
        }
        StatementKind::BufferParamStore {
            parameter,
            channel,
            index,
            value,
            ..
        } => {
            rewrite_buffer_param_ref(parameter, mapping);
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
        process_function_params, AccessMode, Block, BoundsMode, CallArgument, CompileConfig,
        ConstData, ConstDataId, Function, FunctionAttributes, FunctionId, FunctionKind, Local,
        Place, PlaceBase, Program, Projection, Rvalue, ScalarType, ScalarValue, SliceSource,
        SourceSpan, StateId, StatePersistence, StateSlot, Statement, StatementKind, Type, TypeId,
        Value,
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
    fn optimize_promotes_process_state_across_reference_calls() {
        let mut program = empty_program();
        let f32_ty = TypeId::new(program.types.len() as u32);
        program.types.push(Type::Scalar(ScalarType::F32));
        program.state.push(StateSlot {
            integer_range: None,
            name: "phase".to_owned(),
            ty: f32_ty,
            persistence: StatePersistence::Snapshot,
        });

        let helper_id = FunctionId::new(program.functions.len() as u32);
        let mut helper = function("generated_step", FunctionKind::User);
        helper.attributes = FunctionAttributes {
            origin: crate::FunctionOrigin::CompilerGenerated,
            inline: crate::InlineHint::Always,
        };
        helper.params.push(crate::FunctionParam {
            integer_range: None,
            name: "phase".to_owned(),
            ty: f32_ty,
            mode: PassingMode::ReadWriteReference,
        });
        helper.body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                    projections: Vec::new(),
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(helper);
        program.functions[program.entry_points.process.index()]
            .body
            .statements
            .push(Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function: helper_id,
                    args: vec![CallArgument::Place(Place {
                        base: PlaceBase::State(StateId::new(0)),
                        projections: Vec::new(),
                    })],
                },
                source: SourceSpan::UNKNOWN,
            });

        let mut aliased_program = program.clone();
        aliased_program.functions[helper_id.index()]
            .locals
            .push(Local {
                integer_range: None,
                name: Some("direct_state_alias".to_owned()),
                ty: f32_ty,
            });
        aliased_program.functions[helper_id.index()]
            .body
            .statements
            .insert(
                0,
                Statement {
                    kind: StatementKind::Assign {
                        destination: Place::local(LocalId::new(0)),
                        value: Rvalue::Load(Place {
                            base: PlaceBase::State(StateId::new(0)),
                            projections: Vec::new(),
                        }),
                    },
                    source: SourceSpan::UNKNOWN,
                },
            );
        let aliased = crate::validate_owned(aliased_program)
            .expect("direct-state alias fixture should validate");
        let (aliased, aliased_stats) =
            optimize(aliased).expect("alias-safe optimization should validate");
        assert_eq!(aliased_stats.promoted_state_slots, 0);
        assert!(matches!(
            &aliased.functions[aliased.entry_points.process.index()].body.statements[0].kind,
            StatementKind::Call { args, .. }
                if matches!(
                    args.as_slice(),
                    [CallArgument::Place(Place {
                        base: PlaceBase::State(_),
                        ..
                    })]
                )
        ));

        let validated = crate::validate_owned(program).expect("test MIR should validate");
        let (optimized, stats) = optimize(validated).expect("transforms should preserve validity");
        assert_eq!(stats.promoted_state_slots, 1);

        let process = &optimized.functions[optimized.entry_points.process.index()];
        assert!(process.body.statements.iter().any(|statement| matches!(
            &statement.kind,
            StatementKind::Call { args, .. }
                if matches!(
                    args.as_slice(),
                    [CallArgument::Place(Place {
                        base: PlaceBase::Local(_),
                        ..
                    })]
                )
        )));
        assert!(process.locals.iter().any(|local| {
            local
                .name
                .as_deref()
                .is_some_and(|name| name == "$promoted.state.phase")
        }));
        assert!(matches!(
            process
                .body
                .statements
                .first()
                .map(|statement| &statement.kind),
            Some(StatementKind::Assign {
                value: Rvalue::Load(Place {
                    base: PlaceBase::State(state),
                    ..
                }),
                ..
            }) if *state == StateId::new(0)
        ));
        assert!(matches!(
            process
                .body
                .statements
                .last()
                .map(|statement| &statement.kind),
            Some(StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::State(state),
                    ..
                },
                ..
            }) if *state == StateId::new(0)
        ));
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
    fn algebraic_identities_preserve_strict_float_edge_semantics() {
        let local_types = [TypeId::new(0), TypeId::new(1)];
        let types = [Type::Scalar(ScalarType::I32), Type::Scalar(ScalarType::F32)];
        assert_eq!(
            simplify_rvalue_algebra(
                &Rvalue::Binary {
                    op: BinaryOp::Subtract,
                    lhs: Value::Local(LocalId::new(1)),
                    rhs: Value::Local(LocalId::new(1)),
                },
                &local_types,
                &types,
            ),
            None
        );
        assert_eq!(
            simplify_rvalue_algebra(
                &Rvalue::Compare {
                    op: CompareOp::Equal,
                    lhs: Value::Local(LocalId::new(1)),
                    rhs: Value::Local(LocalId::new(1)),
                },
                &local_types,
                &types,
            ),
            None
        );
        assert_eq!(
            simplify_rvalue_algebra(
                &Rvalue::Binary {
                    op: BinaryOp::Subtract,
                    lhs: Value::Local(LocalId::new(0)),
                    rhs: Value::Local(LocalId::new(0)),
                },
                &local_types,
                &types,
            ),
            Some(Value::Constant(ScalarValue::I32(0)))
        );
    }

    #[test]
    fn propagates_constant_chains_and_merges_equal_branch_facts() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::Bool));
        let mut user = function("branch_constants", FunctionKind::User);
        user.params.push(crate::FunctionParam {
            integer_range: None,
            name: "condition".to_owned(),
            ty: TypeId::new(1),
            mode: PassingMode::Value,
        });
        user.results.push(TypeId::new(0));
        user.locals.extend([
            Local {
                integer_range: None,
                name: Some("condition".to_owned()),
                ty: TypeId::new(1),
            },
            Local {
                integer_range: None,
                name: Some("seed".to_owned()),
                ty: TypeId::new(0),
            },
            Local {
                integer_range: None,
                name: Some("merged".to_owned()),
                ty: TypeId::new(0),
            },
            Local {
                integer_range: None,
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
    fn eliminates_repeated_pure_expressions_with_stable_operands() {
        let mut program = empty_program();
        let mut user = function("repeated_expression", FunctionKind::User);
        user.params.push(crate::FunctionParam {
            integer_range: None,
            name: "value".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::Value,
        });
        user.results.push(TypeId::new(0));
        user.locals.extend((0..3).map(|_| Local {
            integer_range: None,
            name: None,
            ty: TypeId::new(0),
        }));
        user.body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::Load(Place {
                        base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                        projections: Vec::new(),
                    }),
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(1)),
                    value: Rvalue::Binary {
                        op: BinaryOp::Multiply,
                        lhs: Value::Local(LocalId::new(0)),
                        rhs: Value::Constant(ScalarValue::I32(7)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(2)),
                    value: Rvalue::Binary {
                        op: BinaryOp::Multiply,
                        lhs: Value::Local(LocalId::new(0)),
                        rhs: Value::Constant(ScalarValue::I32(7)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Return {
                    values: vec![Value::Local(LocalId::new(2))],
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);
        program.functions.push(user);

        let validated = crate::validate_owned(program).expect("CSE fixture should validate");
        let (optimized, stats) = optimize(validated).expect("CSE should preserve validity");
        assert_eq!(stats.eliminated_common_subexpressions, 1);
        let user = &optimized.functions[2];
        let StatementKind::Return { values } = &user.body.statements.last().unwrap().kind else {
            panic!("optimized user function should return")
        };
        assert_eq!(values, &[Value::Local(LocalId::new(1))]);
    }

    #[test]
    fn loop_writes_invalidate_incoming_constant_facts() {
        let mut program = empty_program();
        let mut user = function("loop_mutation", FunctionKind::User);
        user.results.push(TypeId::new(0));
        user.locals.push(Local {
            integer_range: None,
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
            integer_range: None,
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
            integer_range: None,
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
    fn bounds_proofs_see_ranges_exposed_by_control_flow_cleanup() {
        let mut program = empty_program();
        program.const_data.push(ConstData {
            name: "table".to_owned(),
            element: ScalarType::I32,
            values: vec![
                ScalarValue::I32(10),
                ScalarValue::I32(20),
                ScalarValue::I32(30),
                ScalarValue::I32(40),
            ],
        });

        let mut user = function("clean_bounds", FunctionKind::User);
        user.results.push(TypeId::new(0));
        user.locals.extend([
            Local {
                integer_range: None,
                name: Some("index".to_owned()),
                ty: TypeId::new(0),
            },
            Local {
                integer_range: None,
                name: Some("result".to_owned()),
                ty: TypeId::new(0),
            },
        ]);
        let assign_index = |value| Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(value))),
            },
            source: SourceSpan::UNKNOWN,
        };
        user.body.statements.extend([
            assign_index(1),
            Statement {
                kind: StatementKind::If {
                    condition: Value::Constant(ScalarValue::Bool(false)),
                    then_block: Block {
                        statements: vec![assign_index(99)],
                    },
                    else_block: Block::default(),
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(1)),
                    value: Rvalue::ConstDataLoad {
                        data: ConstDataId::new(0),
                        index: Value::Local(LocalId::new(0)),
                        bounds: BoundsMode::Clamp,
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
        ]);
        program.functions.push(user);

        let validated = crate::validate_owned(program).expect("fixture is valid before cleanup");
        let (optimized, stats) =
            super::optimize(validated).expect("cleanup should expose the bounds proof");
        assert_eq!(stats.simplified_branches, 1);
        assert_eq!(stats.eliminated_bounds_checks, 1);

        let load = optimized.functions[2]
            .body
            .statements
            .iter()
            .find_map(|statement| match &statement.kind {
                StatementKind::Assign {
                    value: Rvalue::ConstDataLoad { index, bounds, .. },
                    ..
                } => Some((*index, *bounds)),
                _ => None,
            })
            .expect("optimized function should retain the table load");
        assert_eq!(
            load,
            (Value::Constant(ScalarValue::I32(1)), BoundsMode::Unchecked)
        );
    }

    #[test]
    fn optimize_simplifies_structured_control_flow_and_reindexes_locals() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        let mut user = function("choose", FunctionKind::User);
        user.results.push(TypeId::new(1));
        user.locals.extend([
            Local {
                integer_range: None,
                name: Some("dead".to_owned()),
                ty: TypeId::new(1),
            },
            Local {
                integer_range: None,
                name: Some("result".to_owned()),
                ty: TypeId::new(1),
            },
            Local {
                integer_range: None,
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
    fn parameter_pruning_reaches_forwarding_call_chains() {
        let mut program = empty_program();
        let reference_param = |name: &str| crate::FunctionParam {
            name: name.to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::ReadOnlyReference,
            integer_range: None,
        };

        let mut leaf = function("leaf", FunctionKind::User);
        leaf.params.push(reference_param("unused"));
        leaf.body.statements.push(Statement {
            kind: StatementKind::Return { values: Vec::new() },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(leaf);

        let mut middle = function("middle", FunctionKind::User);
        middle.params.push(reference_param("forwarded"));
        middle.body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::Place(Place {
                    base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                    projections: Vec::new(),
                })],
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(middle);

        let mut root = function("root", FunctionKind::User);
        root.params.push(reference_param("forwarded_twice"));
        root.body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(3),
                args: vec![CallArgument::Place(Place {
                    base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                    projections: Vec::new(),
                })],
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(root);

        assert_eq!(crate::prune_unused_function_parameters(&mut program), 3);
        assert!(program.functions[2..]
            .iter()
            .all(|function| function.params.is_empty()));
        assert!(program.functions[3..].iter().all(|function| {
            matches!(
                &function.body.statements[0].kind,
                StatementKind::Call { args, .. } if args.is_empty()
            )
        }));
        crate::validate_owned(program).expect("pruned forwarding calls remain valid");
    }

    #[test]
    fn parameter_pruning_reindexes_retained_parameters_and_arguments() {
        let mut program = empty_program();
        let mut helper = function("uses_second", FunctionKind::User);
        helper
            .params
            .extend(["unused", "used"].map(|name| crate::FunctionParam {
                name: name.to_owned(),
                ty: TypeId::new(0),
                mode: PassingMode::ReadOnlyReference,
                integer_range: None,
            }));
        helper.locals.push(Local {
            name: Some("value".to_owned()),
            ty: TypeId::new(0),
            integer_range: None,
        });
        helper.body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Load(Place {
                    base: PlaceBase::Parameter(crate::ParameterId::new(1)),
                    projections: Vec::new(),
                }),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(helper);
        program
            .state
            .extend(["first", "second"].map(|name| StateSlot {
                name: name.to_owned(),
                ty: TypeId::new(0),
                persistence: StatePersistence::Snapshot,
                integer_range: None,
            }));
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![
                    CallArgument::Place(Place {
                        base: PlaceBase::State(StateId::new(0)),
                        projections: Vec::new(),
                    }),
                    CallArgument::Place(Place {
                        base: PlaceBase::State(StateId::new(1)),
                        projections: Vec::new(),
                    }),
                ],
            },
            source: SourceSpan::UNKNOWN,
        });

        assert_eq!(crate::prune_unused_function_parameters(&mut program), 1);
        assert_eq!(program.functions[2].params[0].name, "used");
        assert!(matches!(
            &program.functions[2].body.statements[0].kind,
            StatementKind::Assign {
                value: Rvalue::Load(Place {
                    base: PlaceBase::Parameter(parameter),
                    ..
                }),
                ..
            } if parameter.index() == 0
        ));
        assert!(matches!(
            &program.functions[1].body.statements[0].kind,
            StatementKind::Call { args, .. }
                if matches!(args.as_slice(), [CallArgument::Place(Place {
                    base: PlaceBase::State(state),
                    ..
                })] if state.index() == 1)
        ));
        crate::validate_owned(program).expect("reindexed parameters remain valid");
    }

    #[test]
    fn parameter_pruning_preserves_fallible_argument_evaluation() {
        let mut program = empty_program();
        let array_ty = TypeId::new(program.types.len() as u32);
        program.types.push(Type::Array {
            element: TypeId::new(0),
            len: 1,
        });
        let slice_ty = TypeId::new(program.types.len() as u32);
        program.types.push(Type::Slice {
            element: ScalarType::I32,
            access: AccessMode::ReadOnly,
        });
        program.state.push(StateSlot {
            name: "values".to_owned(),
            ty: array_ty,
            persistence: StatePersistence::Snapshot,
            integer_range: None,
        });

        let mut helper = function("ignores_value", FunctionKind::User);
        helper.params.push(crate::FunctionParam {
            name: "unused".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::ReadOnlyReference,
            integer_range: None,
        });
        program.functions.push(helper);

        program.functions[0].locals.push(Local {
            name: Some("empty".to_owned()),
            ty: slice_ty,
            integer_range: None,
        });
        program.functions[0].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::MakeSlice {
                        source: SliceSource::Place(Place {
                            base: PlaceBase::State(StateId::new(0)),
                            projections: Vec::new(),
                        }),
                        start: Value::Constant(ScalarValue::I32(0)),
                        len: Value::Constant(ScalarValue::I32(0)),
                        bounds: BoundsMode::Checked,
                        access: AccessMode::ReadOnly,
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(2),
                    args: vec![CallArgument::SliceElement {
                        slice: Value::Local(LocalId::new(0)),
                        index: Value::Constant(ScalarValue::I32(0)),
                        bounds: BoundsMode::Checked,
                    }],
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        let validated =
            crate::validate_owned(program.clone()).expect("fixture should be valid before pruning");
        assert_eq!(crate::prune_unused_function_parameters(&mut program), 0);
        assert_eq!(program.functions[2].params.len(), 1);
        assert!(matches!(
            &program.functions[0].body.statements[1].kind,
            StatementKind::Call { args, .. }
                if matches!(args.as_slice(), [CallArgument::SliceElement {
                    bounds: BoundsMode::Checked,
                    ..
                }])
        ));
        crate::validate_owned(program).expect("preserved call should remain valid");

        let (optimized, stats) = super::optimize(validated).expect("optimization should succeed");
        assert_eq!(stats.removed_function_parameters, 0);
        assert_eq!(optimized.functions[2].params.len(), 1);
    }

    #[test]
    fn parameter_pruning_participates_in_the_optimization_fixed_point() {
        let mut program = empty_program();
        let mut helper = function("conditionally_uses_value", FunctionKind::User);
        helper.params.push(crate::FunctionParam {
            name: "value".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::Value,
            integer_range: None,
        });
        helper.locals.push(Local {
            name: Some("discarded".to_owned()),
            ty: TypeId::new(0),
            integer_range: None,
        });
        helper.body.statements.push(Statement {
            kind: StatementKind::If {
                condition: Value::Constant(ScalarValue::Bool(false)),
                then_block: Block {
                    statements: vec![Statement {
                        kind: StatementKind::Assign {
                            destination: Place::local(LocalId::new(0)),
                            value: Rvalue::Load(Place {
                                base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                                projections: Vec::new(),
                            }),
                        },
                        source: SourceSpan::UNKNOWN,
                    }],
                },
                else_block: Block::default(),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(helper);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::Value(Value::Constant(ScalarValue::I32(7)))],
            },
            source: SourceSpan::UNKNOWN,
        });

        let validated = crate::validate_owned(program).expect("fixture is valid before cleanup");
        let (optimized, stats) =
            super::optimize(validated).expect("cleanup and pruning should converge together");
        assert_eq!(stats.simplified_branches, 1);
        assert_eq!(stats.removed_function_parameters, 1);
        assert!(optimized.functions[2].params.is_empty());
        assert!(matches!(
            &optimized.functions[1].body.statements[0].kind,
            StatementKind::Call { args, .. } if args.is_empty()
        ));

        let fixed_point = optimized.as_program().clone();
        let (second, second_stats) = super::optimize(optimized.into_validated())
            .expect("an optimized program should already be at the fixed point");
        assert_eq!(second.as_program(), &fixed_point);
        assert_eq!(second_stats.iterations, 1);
        assert!(!second_stats.changed());
    }

    #[test]
    fn initial_parameter_pruning_prevents_dead_state_promotion() {
        let mut program = empty_program();
        program.state.push(StateSlot {
            name: "unused".to_owned(),
            ty: TypeId::new(0),
            persistence: StatePersistence::Snapshot,
            integer_range: None,
        });

        let mut helper = function("ignores_state", FunctionKind::User);
        helper.params.push(crate::FunctionParam {
            name: "unused".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::ReadWriteReference,
            integer_range: None,
        });
        program.functions.push(helper);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::Place(Place {
                    base: PlaceBase::State(StateId::new(0)),
                    projections: Vec::new(),
                })],
            },
            source: SourceSpan::UNKNOWN,
        });

        let validated = crate::validate_owned(program).expect("fixture is valid before pruning");
        let (optimized, stats) =
            super::optimize(validated).expect("pruning and promotion should preserve validity");

        assert_eq!(stats.removed_function_parameters, 1);
        assert_eq!(stats.promoted_state_slots, 0);
        assert!(optimized.functions[2].params.is_empty());
        let process = &optimized.functions[1];
        assert!(process.locals.is_empty());
        assert!(matches!(
            process.body.statements.as_slice(),
            [Statement {
                kind: StatementKind::Call { args, .. },
                ..
            }] if args.is_empty()
        ));
    }

    #[test]
    fn copy_propagation_collapses_long_dead_chains_before_the_fixed_point() {
        const PURE_CHAIN_LEN: u32 = 32;

        let mut program = empty_program();
        for index in 0..=PURE_CHAIN_LEN {
            program.functions[1].locals.push(Local {
                integer_range: None,
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
        assert!(stats.propagated_copies >= u64::from(PURE_CHAIN_LEN - 1));
        assert!(
            stats.iterations <= 4,
            "copy chains should not require one cleanup round per link"
        );
        assert_eq!(optimized.functions[1].locals.len(), 1);
        assert_eq!(optimized.functions[1].body.statements.len(), 1);

        let fixed_point = optimized.as_program().clone();
        let (second, second_stats) = super::optimize(optimized.into_validated())
            .expect("an optimized program should already be at the fixed point");
        assert_eq!(second.as_program(), &fixed_point);
        assert_eq!(second_stats.iterations, 1);
        assert_eq!(second_stats.propagated_values, 0);
        assert_eq!(second_stats.propagated_copies, 0);
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
                integer_range: None,
                name: "scalar".to_owned(),
                ty: TypeId::new(1),
                persistence: StatePersistence::Snapshot,
            },
            StateSlot {
                integer_range: None,
                name: "array".to_owned(),
                ty: TypeId::new(2),
                persistence: StatePersistence::Snapshot,
            },
        ]);
        program.functions[0].locals.push(Local {
            integer_range: None,
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
            integer_range: None,
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
