//! Block-rate linear parameter ramps, advanced by actual host samples.
use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) struct ParamSmoothing {
    target: onda_mir::ParamId,
    current: onda_mir::StateId,
    start: onda_mir::StateId,
    captured: onda_mir::StateId,
    elapsed: onda_mir::StateId,
    ty: PrimitiveType,
    element_ty: TypeId,
    /// `None` for a scalar parameter, otherwise the fixed array extent.
    array_len: Option<u32>,
    range: Option<onda_mir::ValueRange>,
    samples: i64,
}

impl ParamSmoothing {
    fn state(self, id: onda_mir::StateId, index: Option<LocalId>) -> Place {
        Place {
            base: PlaceBase::State(id),
            projections: projections(index),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn register(
    mir: &mut onda_mir::Program,
    globals: &mut RuntimeGlobals,
    target: onda_mir::ParamId,
    name: &str,
    ty: PrimitiveType,
    state_ty: TypeId,
    array_len: Option<u32>,
    control: &TypedParamControl,
    range: Option<TypedValueRange>,
) {
    let Some(seconds) = control.smooth_seconds else {
        return;
    };
    let samples = crate::declaration_coercion::param_smoothing_samples(
        seconds,
        f64::from(mir.config.sample_rate),
    )
    .expect("semantic validation bounds the smoothing sample count");
    let counter_ty = match array_len {
        Some(len) => intern_array_type(&mut mir.types, PrimitiveType::I64, len),
        None => intern_scalar_type(&mut mir.types, PrimitiveType::I64),
    };
    let current = state(mir, format!("__onda_smoothed_param__{name}"), state_ty);
    let start = state(mir, format!("__onda_param_ramp_start__{name}"), state_ty);
    let captured = state(mir, format!("__onda_param_ramp_target__{name}"), state_ty);
    let elapsed = state(
        mir,
        format!("__onda_param_ramp_elapsed__{name}"),
        counter_ty,
    );
    if globals.param_smoothing_processed.is_none() {
        let ty = intern_scalar_type(&mut mir.types, PrimitiveType::I64);
        globals.param_smoothing_processed =
            Some(state(mir, "__onda_param_ramp_processed".to_owned(), ty));
    }
    globals.smoothed_params.insert(target, current);
    globals.param_smoothing.push(ParamSmoothing {
        target,
        current,
        start,
        captured,
        elapsed,
        ty,
        element_ty: intern_scalar_type(&mut mir.types, ty),
        array_len,
        range: range.map(mir_range),
        samples,
    });
}

fn state(mir: &mut onda_mir::Program, name: String, ty: TypeId) -> onda_mir::StateId {
    let id = onda_mir::StateId::new(mir.state.len() as u32);
    mir.state.push(onda_mir::StateSlot {
        name,
        ty,
        persistence: onda_mir::StatePersistence::Snapshot,
        authored: false,
        pinned: false,
        integer_range: None,
    });
    id
}

/// Initialize before authored init code. A settled ramp reads its constrained
/// target immediately, and all ramp state participates in snapshots and reset.
pub(super) fn prepend_initialization(
    function: &mut onda_mir::Function,
    globals: &RuntimeGlobals,
    types: &mut Vec<MirType>,
) {
    let Some(processed) = globals.param_smoothing_processed else {
        return;
    };
    let mut prefix = MirBlock::default();
    prefix.statements.push(assign(
        Place {
            base: PlaceBase::State(processed),
            projections: Vec::new(),
        },
        Rvalue::Use(integer(0)),
    ));
    for smooth in &globals.param_smoothing {
        let Some(len) = smooth.array_len else {
            initialize_element(function, &mut prefix, *smooth, None);
            continue;
        };
        let i32_ty = intern_scalar_type(types, PrimitiveType::I32);
        let bool_ty = intern_scalar_type(types, PrimitiveType::Bool);
        let index = local(function, i32_ty);
        let done = local(function, bool_ty);
        let mut iteration = MirBlock::default();
        iteration.statements.push(assign(
            Place::local(done),
            Rvalue::Compare {
                op: CompareOp::GreaterEqual,
                lhs: Value::Local(index),
                rhs: Value::Constant(ScalarValue::I32(len as i32)),
            },
        ));
        iteration.statements.push(statement(StatementKind::If {
            condition: Value::Local(done),
            then_block: MirBlock {
                statements: vec![statement(StatementKind::Break)],
            },
            else_block: MirBlock::default(),
        }));
        initialize_element(function, &mut iteration, *smooth, Some(index));
        iteration.statements.push(assign(
            Place::local(index),
            Rvalue::Binary {
                op: MirBinaryOp::Add,
                lhs: Value::Local(index),
                rhs: Value::Constant(ScalarValue::I32(1)),
            },
        ));
        prefix.statements.push(assign(
            Place::local(index),
            Rvalue::Use(Value::Constant(ScalarValue::I32(0))),
        ));
        prefix
            .statements
            .push(statement(StatementKind::Loop { body: iteration }));
    }
    prefix.statements.append(&mut function.body.statements);
    function.body = prefix;
}

fn initialize_element(
    function: &mut onda_mir::Function,
    block: &mut MirBlock,
    smooth: ParamSmoothing,
    index: Option<LocalId>,
) {
    let value = local(function, smooth.element_ty);
    block.statements.push(assign(
        Place::local(value),
        Rvalue::Load(Place {
            base: PlaceBase::Param(smooth.target),
            projections: projections(index),
        }),
    ));
    if let Some(range) = smooth.range {
        block.statements.push(assign(
            Place::local(value),
            Rvalue::Intrinsic {
                intrinsic: Intrinsic::RangeClamp,
                args: vec![
                    Value::Local(value),
                    Value::Constant(range.min),
                    Value::Constant(range.max),
                ],
            },
        ));
    }
    for id in [smooth.current, smooth.start, smooth.captured] {
        block.statements.push(assign(
            smooth.state(id, index),
            Rvalue::Use(Value::Local(value)),
        ));
    }
    block.statements.push(assign(
        smooth.state(smooth.elapsed, index),
        Rvalue::Use(integer(smooth.samples)),
    ));
}

impl FunctionLowerer<'_> {
    /// Publish exactly once per logical block. Segment calls accumulate only
    /// sample counts; no effective value changes between block hooks or reads.
    pub(super) fn advance_smoothed_params(&mut self, block: &mut MirBlock, location: SourceLoc) {
        let Some(globals) = self.runtime_globals else {
            return;
        };
        let Some(processed) = globals.param_smoothing_processed else {
            return;
        };
        let processed_place = Place {
            base: PlaceBase::State(processed),
            projections: Vec::new(),
        };
        let frames = self.load_place_value(block, PrimitiveType::I64, &processed_place, location);
        for smooth in globals.param_smoothing.iter().copied() {
            if let Some(len) = smooth.array_len {
                let index = self.new_local(None, PrimitiveType::I32);
                let mut iteration = MirBlock::default();
                self.advance_param_ramp(&mut iteration, smooth, Some(index), frames, location);
                self.emit_counted_loop(block, index, len as i32, iteration, location);
            } else {
                self.advance_param_ramp(block, smooth, None, frames, location);
            }
        }
        self.assign_place_value(block, processed_place, integer(0), location);
    }

    pub(super) fn record_smoothed_param_samples(
        &mut self,
        block: &mut MirBlock,
        frames: Value,
        location: SourceLoc,
    ) {
        let Some(globals) = self.runtime_globals else {
            return;
        };
        let Some(processed) = globals.param_smoothing_processed else {
            return;
        };
        let limit = globals
            .param_smoothing
            .iter()
            .map(|smooth| smooth.samples)
            .max()
            .unwrap();
        let place = Place {
            base: PlaceBase::State(processed),
            projections: Vec::new(),
        };
        let current = self.load_place_value(block, PrimitiveType::I64, &place, location);
        let frames = self
            .emit_temp(
                block,
                PrimitiveType::I64,
                Rvalue::Cast {
                    value: frames,
                    to: ScalarType::I64,
                },
                location,
            )
            .value;
        let next = self.advance_sample_count(block, current, frames, limit, location);
        self.assign_place_value(block, place, next, location);
    }

    /// Saturate before addition, avoiding overflow even for long durations or
    /// repeated process calls without a BEGIN_BLOCK publication boundary.
    fn advance_sample_count(
        &mut self,
        block: &mut MirBlock,
        elapsed: Value,
        frames: Value,
        limit: i64,
        location: SourceLoc,
    ) -> Value {
        let remaining = self.emit_binary_value(
            block,
            PrimitiveType::I64,
            MirBinaryOp::Subtract,
            integer(limit),
            elapsed,
            location,
        );
        let advance = self
            .emit_temp(
                block,
                PrimitiveType::I64,
                Rvalue::Intrinsic {
                    intrinsic: Intrinsic::Min,
                    args: vec![frames, remaining],
                },
                location,
            )
            .value;
        self.emit_binary_value(
            block,
            PrimitiveType::I64,
            MirBinaryOp::Add,
            elapsed,
            advance,
            location,
        )
    }

    fn advance_param_ramp(
        &mut self,
        block: &mut MirBlock,
        smooth: ParamSmoothing,
        index: Option<LocalId>,
        frames: Value,
        location: SourceLoc,
    ) {
        let elapsed_place = smooth.state(smooth.elapsed, index);
        let elapsed = self.load_place_value(block, PrimitiveType::I64, &elapsed_place, location);
        let active = self.compare_value(
            block,
            CompareOp::Less,
            elapsed,
            integer(smooth.samples),
            location,
        );
        let mut advance = MirBlock::default();
        let elapsed =
            self.advance_sample_count(&mut advance, elapsed, frames, smooth.samples, location);
        self.assign_place_value(&mut advance, elapsed_place.clone(), elapsed, location);
        let completed = self.compare_value(
            &mut advance,
            CompareOp::Equal,
            elapsed,
            integer(smooth.samples),
            location,
        );
        let target = self.load_place_value(
            &mut advance,
            smooth.ty,
            &smooth.state(smooth.captured, index),
            location,
        );
        let mut finish = MirBlock::default();
        self.assign_place_value(
            &mut finish,
            smooth.state(smooth.current, index),
            target,
            location,
        );
        let mut interpolate = MirBlock::default();
        let start = self.load_place_value(
            &mut interpolate,
            smooth.ty,
            &smooth.state(smooth.start, index),
            location,
        );
        let next =
            self.linear_param_value(&mut interpolate, smooth, start, target, elapsed, location);
        self.assign_place_value(
            &mut interpolate,
            smooth.state(smooth.current, index),
            next,
            location,
        );
        self.push_statement(
            &mut advance,
            StatementKind::If {
                condition: completed,
                then_block: finish,
                else_block: interpolate,
            },
            location,
        );
        let has_samples =
            self.compare_value(block, CompareOp::Greater, frames, integer(0), location);
        let mut active_ramp = MirBlock::default();
        self.push_statement(
            &mut active_ramp,
            StatementKind::If {
                condition: has_samples,
                then_block: advance,
                else_block: MirBlock::default(),
            },
            location,
        );
        self.push_statement(
            block,
            StatementKind::If {
                condition: active,
                then_block: active_ramp,
                else_block: MirBlock::default(),
            },
            location,
        );

        // Advance the old ramp before retargeting, so a change starts from the
        // value reached at this boundary, including the previous block's time.
        let target = self.load_place_value(
            block,
            smooth.ty,
            &Place {
                base: PlaceBase::Param(smooth.target),
                projections: projections(index),
            },
            location,
        );
        let target = self.constrain_target(block, target, smooth.range, smooth.ty, location);
        let captured = self.load_place_value(
            block,
            smooth.ty,
            &smooth.state(smooth.captured, index),
            location,
        );
        let changed = self.compare_value(block, CompareOp::NotEqual, target, captured, location);
        let mut retarget = MirBlock::default();
        let current = self.load_place_value(
            &mut retarget,
            smooth.ty,
            &smooth.state(smooth.current, index),
            location,
        );
        self.assign_place_value(
            &mut retarget,
            smooth.state(smooth.start, index),
            current,
            location,
        );
        self.assign_place_value(
            &mut retarget,
            smooth.state(smooth.captured, index),
            target,
            location,
        );
        self.assign_place_value(&mut retarget, elapsed_place, integer(0), location);
        self.push_statement(
            block,
            StatementKind::If {
                condition: changed,
                then_block: retarget,
                else_block: MirBlock::default(),
            },
            location,
        );
    }

    fn constrain_target(
        &mut self,
        block: &mut MirBlock,
        target: Value,
        range: Option<onda_mir::ValueRange>,
        ty: PrimitiveType,
        location: SourceLoc,
    ) -> Value {
        let Some(range) = range else { return target };
        self.emit_temp(
            block,
            ty,
            Rvalue::Intrinsic {
                intrinsic: Intrinsic::RangeClamp,
                args: vec![
                    target,
                    Value::Constant(range.min),
                    Value::Constant(range.max),
                ],
            },
            location,
        )
        .value
    }

    fn linear_param_value(
        &mut self,
        block: &mut MirBlock,
        smooth: ParamSmoothing,
        start: Value,
        target: Value,
        elapsed: Value,
        location: SourceLoc,
    ) -> Value {
        let remaining = self.emit_binary_value(
            block,
            PrimitiveType::I64,
            MirBinaryOp::Subtract,
            integer(smooth.samples),
            elapsed,
            location,
        );
        let from_target =
            self.compare_value(block, CompareOp::Greater, elapsed, remaining, location);
        let base = self.new_local(None, smooth.ty);
        let other = self.new_local(None, smooth.ty);
        let mut late = MirBlock::default();
        self.assign_value(&mut late, base, target, location);
        self.assign_value(&mut late, other, start, location);
        let mut early = MirBlock::default();
        self.assign_value(&mut early, base, start, location);
        self.assign_value(&mut early, other, target, location);
        self.push_statement(
            block,
            StatementKind::If {
                condition: from_target,
                then_block: late,
                else_block: early,
            },
            location,
        );
        let count = self
            .emit_temp(
                block,
                PrimitiveType::I64,
                Rvalue::Intrinsic {
                    intrinsic: Intrinsic::Min,
                    args: vec![elapsed, remaining],
                },
                location,
            )
            .value;
        let weight = self.ramp_weight(block, smooth, count, 0.0, 0.5, location);
        self.interpolate_param_value(
            block,
            smooth,
            Value::Local(base),
            Value::Local(other),
            weight,
            count,
            location,
        )
    }

    fn ramp_weight(
        &mut self,
        block: &mut MirBlock,
        smooth: ParamSmoothing,
        count: Value,
        min: f64,
        max: f64,
        location: SourceLoc,
    ) -> Value {
        let count = self
            .emit_temp(
                block,
                smooth.ty,
                Rvalue::Cast {
                    value: count,
                    to: scalar_type(smooth.ty),
                },
                location,
            )
            .value;
        let weight = self.emit_binary_value(
            block,
            smooth.ty,
            MirBinaryOp::Multiply,
            count,
            Value::Constant(scalar_from_f64(1.0 / smooth.samples as f64, smooth.ty)),
            location,
        );
        // Bound rounding at the endpoints. These independent clamp boundaries
        // also prevent fast-math from pulling a shared reciprocal across the
        // convex sum into products that overflow for large finite endpoints.
        self.emit_temp(
            block,
            smooth.ty,
            Rvalue::Intrinsic {
                intrinsic: Intrinsic::RangeClamp,
                args: vec![
                    weight,
                    Value::Constant(scalar_from_f64(min, smooth.ty)),
                    Value::Constant(scalar_from_f64(max, smooth.ty)),
                ],
            },
            location,
        )
        .value
    }

    #[allow(clippy::too_many_arguments)]
    fn interpolate_param_value(
        &mut self,
        block: &mut MirBlock,
        smooth: ParamSmoothing,
        base: Value,
        other: Value,
        weight: Value,
        count: Value,
        location: SourceLoc,
    ) -> Value {
        // Use the nearer endpoint to preserve small targets and slow updates.
        // Same-sign subtraction is finite; opposite signs need a convex sum.
        let ty = smooth.ty;
        let base_negative =
            self.compare_value(block, CompareOp::Less, base, zero_value(ty), location);
        let other_negative =
            self.compare_value(block, CompareOp::Less, other, zero_value(ty), location);
        let opposite_signs = self.compare_value(
            block,
            CompareOp::NotEqual,
            base_negative,
            other_negative,
            location,
        );
        let next = self.new_local(None, ty);
        let mut opposite = MirBlock::default();
        let base_count = self.emit_binary_value(
            &mut opposite,
            PrimitiveType::I64,
            MirBinaryOp::Subtract,
            integer(smooth.samples),
            count,
            location,
        );
        let base_weight = self.ramp_weight(&mut opposite, smooth, base_count, 0.5, 1.0, location);
        let other_step = self.emit_binary_value(
            &mut opposite,
            ty,
            MirBinaryOp::Multiply,
            other,
            weight,
            location,
        );
        let base_step = self.emit_binary_value(
            &mut opposite,
            ty,
            MirBinaryOp::Multiply,
            base,
            base_weight,
            location,
        );
        let combined = self.emit_binary_value(
            &mut opposite,
            ty,
            MirBinaryOp::Add,
            base_step,
            other_step,
            location,
        );
        self.assign_value(&mut opposite, next, combined, location);
        let mut same = MirBlock::default();
        let delta =
            self.emit_binary_value(&mut same, ty, MirBinaryOp::Subtract, other, base, location);
        let delta = self.emit_binary_value(
            &mut same,
            ty,
            MirBinaryOp::Multiply,
            delta,
            weight,
            location,
        );
        let combined =
            self.emit_binary_value(&mut same, ty, MirBinaryOp::Add, base, delta, location);
        self.assign_value(&mut same, next, combined, location);
        self.push_statement(
            block,
            StatementKind::If {
                condition: opposite_signs,
                then_block: opposite,
                else_block: same,
            },
            location,
        );
        Value::Local(next)
    }
}

fn projections(index: Option<LocalId>) -> Vec<Projection> {
    index
        .into_iter()
        .map(|index| Projection::Index {
            index: Value::Local(index),
            bounds: BoundsMode::Unchecked,
        })
        .collect()
}

fn integer(value: i64) -> Value {
    Value::Constant(ScalarValue::I64(value))
}

fn assign(destination: Place, value: Rvalue) -> Statement {
    statement(StatementKind::Assign { destination, value })
}

fn statement(kind: StatementKind) -> Statement {
    Statement {
        kind,
        source: SourceSpan::UNKNOWN,
    }
}

fn local(function: &mut onda_mir::Function, ty: TypeId) -> LocalId {
    let id = LocalId::new(function.locals.len() as u32);
    function.locals.push(onda_mir::Local {
        name: None,
        ty,
        integer_range: None,
    });
    id
}
