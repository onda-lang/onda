use crate::{
    Block, BoundsMode, CallArgument, Function, FunctionId, FunctionRangeAnalysis, Place, PlaceBase,
    Projection, Rvalue, ScalarValue, StatementKind, Type, TypeId, Value,
};

use super::PassStats;

mod accesses;
mod relations;
mod views;

pub(super) fn eliminate_proven_bounds_checks(
    program: &mut crate::Program,
    stats: &mut PassStats,
) -> bool {
    let mut changed = false;
    let program_ranges = crate::analyze_program_integer_ranges(program);
    let memory = crate::analyze_memory(program);
    for function_index in 0..program.functions.len() {
        let function_id = FunctionId::new(function_index as u32);
        let ranges = program_ranges
            .function(function_id)
            .expect("program range analysis covers every MIR function");
        let mut slice_lengths = fixed_slice_lengths(&program.functions[function_index]);
        for (index, length) in slice_lengths.iter_mut().enumerate() {
            if let Some(known) = memory
                .slice(function_id, Value::Local(crate::LocalId::new(index as u32)))
                .and_then(views::contiguous_length)
            {
                *length = Some(known);
            }
        }
        let mut body = std::mem::take(&mut program.functions[function_index].body);
        let context = Context {
            program,
            function: &program.functions[function_index],
            ranges,
            slice_lengths,
            memory: &memory,
            function_id,
        };
        let eliminated =
            prove_block(&context, &mut body, stats) + relations::eliminate(&context, &mut body);
        program.functions[function_index].body = body;
        stats.eliminated_bounds_checks = stats.eliminated_bounds_checks.saturating_add(eliminated);
        changed |= eliminated != 0;
    }
    changed
}

struct Context<'a> {
    program: &'a crate::Program,
    function: &'a Function,
    ranges: &'a FunctionRangeAnalysis,
    slice_lengths: Vec<Option<u32>>,
    memory: &'a crate::MemoryAnalysis,
    function_id: FunctionId,
}

/// A descriptor assigned once has a stable shape even when its elements are
/// mutable. Multiple definitions or address escapes discard the proof.
fn fixed_slice_lengths(function: &Function) -> Vec<Option<u32>> {
    fn scan(block: &Block, writes: &mut [u32], lengths: &mut [Option<u32>]) {
        for statement in &block.statements {
            match &statement.kind {
                StatementKind::Assign {
                    destination:
                        Place {
                            base: PlaceBase::Local(local),
                            projections,
                        },
                    value,
                } if projections.is_empty() => {
                    writes[local.index()] = writes[local.index()].saturating_add(1);
                    lengths[local.index()] = match value {
                        Rvalue::MakeSlice {
                            len: Value::Constant(ScalarValue::I32(len)),
                            bounds: BoundsMode::Checked | BoundsMode::Unchecked,
                            ..
                        } => u32::try_from(*len).ok(),
                        _ => None,
                    };
                }
                StatementKind::Call { results, args, .. } => {
                    for local in results {
                        writes[local.index()] = 2;
                    }
                    for argument in args {
                        if let CallArgument::Place(Place {
                            base: PlaceBase::Local(local),
                            ..
                        }) = argument
                        {
                            writes[local.index()] = 2;
                        }
                    }
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    scan(then_block, writes, lengths);
                    scan(else_block, writes, lengths);
                }
                StatementKind::Loop { body } => scan(body, writes, lengths),
                _ => {}
            }
        }
    }
    let mut writes = vec![0; function.locals.len()];
    let mut lengths = vec![None; function.locals.len()];
    scan(&function.body, &mut writes, &mut lengths);
    for (count, length) in writes.into_iter().zip(&mut lengths) {
        if count != 1 {
            *length = None;
        }
    }
    lengths
}

fn slice_length(context: &Context<'_>, value: Value) -> Option<u32> {
    match value {
        Value::Local(local) => context.slice_lengths.get(local.index()).copied().flatten(),
        _ => None,
    }
}

fn prove_block(context: &Context<'_>, block: &mut Block, stats: &mut PassStats) -> u64 {
    let mut eliminated = 0;
    for statement in &mut block.statements {
        accesses::visit(context, &mut statement.kind, &mut |index, limit, bounds| {
            let len = match limit {
                accesses::Limit::Fixed(len) => Some(len),
                accesses::Limit::Slice(slice) => slice_length(context, slice),
                accesses::Limit::Value(length) => {
                    value_range(context, length).and_then(|range| u32::try_from(range.min()).ok())
                }
            };
            if let Some(len) = len {
                eliminated += prove_index(context, index, len, bounds);
            }
        });
        match &mut statement.kind {
            StatementKind::Assign { value, .. } => {
                eliminated += views::simplify(context, value, stats);
                if let Some(original) = redundant_normalization_operand(context, value) {
                    *value = Rvalue::Use(original);
                    eliminated += 1;
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                eliminated += prove_block(context, then_block, stats);
                eliminated += prove_block(context, else_block, stats);
            }
            StatementKind::Loop { body } => eliminated += prove_block(context, body, stats),
            _ => {}
        }
    }
    eliminated
}

fn redundant_normalization_operand(context: &Context<'_>, value: &Rvalue) -> Option<Value> {
    let Rvalue::Intrinsic { intrinsic, args } = value else {
        return None;
    };
    if !matches!(
        intrinsic,
        crate::Intrinsic::RangeClamp | crate::Intrinsic::RangeWrap
    ) || args.len() != 3
    {
        return None;
    }

    let operand = value_range(context, args[0])?;
    let lower = value_range(context, args[1])?;
    let upper = value_range(context, args[2])?;
    (operand.scalar() == lower.scalar()
        && lower.scalar() == upper.scalar()
        && lower.min() == lower.max()
        && upper.min() == upper.max()
        && operand.min() >= lower.min()
        && operand.max() <= upper.max())
    .then_some(args[0])
}

fn prove_index(context: &Context<'_>, index: Value, len: u32, bounds: &mut BoundsMode) -> u64 {
    if *bounds == BoundsMode::Unchecked || len == 0 {
        return 0;
    }
    let in_bounds = value_range(context, index).is_some_and(|range| {
        range.is_nonnegative() && u64::try_from(range.max()).is_ok_and(|max| max < u64::from(len))
    });
    if in_bounds {
        *bounds = BoundsMode::Unchecked;
        1
    } else {
        0
    }
}

fn value_range(context: &Context<'_>, value: Value) -> Option<crate::IntegerRange> {
    match value {
        Value::Constant(ScalarValue::I32(value)) => {
            crate::IntegerRange::new(crate::ScalarType::I32, i64::from(value), i64::from(value))
        }
        Value::Constant(ScalarValue::I64(value)) => {
            crate::IntegerRange::new(crate::ScalarType::I64, value, value)
        }
        Value::Constant(_) => None,
        Value::Local(local) => context.ranges.local(local),
    }
}

fn base_type(context: &Context<'_>, base: PlaceBase) -> Option<TypeId> {
    match base {
        PlaceBase::Local(local) => context
            .function
            .locals
            .get(local.index())
            .map(|local| local.ty),
        PlaceBase::Parameter(parameter) => context
            .function
            .params
            .get(parameter.index())
            .map(|parameter| parameter.ty),
        PlaceBase::State(state) => context
            .program
            .state
            .get(state.index())
            .map(|state| state.ty),
        PlaceBase::Param(param) => context
            .program
            .interface
            .params
            .get(param.index())
            .map(|param| param.ty),
        PlaceBase::EventParam(parameter) => {
            let crate::FunctionKind::Event(event) = context.function.kind else {
                return None;
            };
            context
                .program
                .interface
                .events
                .get(event.index())?
                .params
                .get(parameter.index())
                .map(|parameter| parameter.ty)
        }
    }
}

fn place_type(context: &Context<'_>, place: &Place) -> Option<TypeId> {
    context.program.place_type(context.function, place)
}

fn array_len(program: &crate::Program, ty: TypeId) -> Option<u32> {
    match program.types.get(ty.index())? {
        Type::Array { len, .. } => Some(*len),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AccessMode, BufferChannels, BufferId, BufferParamRef, BufferRef, CompileConfig, Function,
        FunctionAttributes, FunctionId, FunctionKind, FunctionParam, IntegerRangeInvariant,
        IntegerRangeMode, Intrinsic, Local, LocalId, ParameterId, PassingMode, Program, ScalarType,
        SourceSpan, Statement, Type,
    };

    use super::*;

    #[test]
    fn fixed_slice_proofs_exclude_clamped_and_empty_descriptors() {
        for creation in [
            BoundsMode::Clamp,
            BoundsMode::Checked,
            BoundsMode::Unchecked,
        ] {
            for len in [0, 2] {
                for reassigned in [false, true] {
                    let mut program = Program::new(
                        CompileConfig {
                            sample_rate: 48_000.0,
                            block_size: 1,
                        },
                        FunctionId::new(0),
                        FunctionId::new(0),
                    );
                    program.types = vec![
                        Type::Scalar(ScalarType::I32),
                        Type::Slice {
                            element: ScalarType::I32,
                            access: AccessMode::ReadWrite,
                        },
                    ];
                    let descriptor = Statement {
                        source: SourceSpan::UNKNOWN,
                        kind: StatementKind::Assign {
                            destination: Place::local(LocalId::new(0)),
                            value: Rvalue::MakeSlice {
                                source: crate::SliceSource::Place(Place {
                                    base: PlaceBase::Parameter(ParameterId::new(0)),
                                    projections: vec![],
                                }),
                                start: Value::Constant(ScalarValue::I32(0)),
                                len: Value::Constant(ScalarValue::I32(len)),
                                bounds: creation,
                                access: AccessMode::ReadWrite,
                            },
                        },
                    };
                    let mut statements = vec![
                        descriptor.clone(),
                        Statement {
                            source: SourceSpan::UNKNOWN,
                            kind: StatementKind::Assign {
                                destination: Place::local(LocalId::new(1)),
                                value: Rvalue::SliceLoad {
                                    slice: Value::Local(LocalId::new(0)),
                                    index: Value::Constant(ScalarValue::I32(1)),
                                    bounds: BoundsMode::Clamp,
                                },
                            },
                        },
                        Statement {
                            source: SourceSpan::UNKNOWN,
                            kind: StatementKind::SliceStore {
                                slice: Value::Local(LocalId::new(0)),
                                index: Value::Constant(ScalarValue::I32(1)),
                                value: Value::Constant(ScalarValue::I32(3)),
                                bounds: BoundsMode::Checked,
                            },
                        },
                    ];
                    if reassigned {
                        statements.push(descriptor);
                    }
                    program.functions.push(Function {
                        name: "slice_proof".into(),
                        kind: FunctionKind::User,
                        attributes: FunctionAttributes::default(),
                        params: vec![FunctionParam {
                            name: "source".into(),
                            ty: TypeId::new(1),
                            mode: PassingMode::Value,
                            integer_range: None,
                        }],
                        results: vec![],
                        locals: vec![
                            Local {
                                name: None,
                                ty: TypeId::new(1),
                                integer_range: None,
                            },
                            Local {
                                name: None,
                                ty: TypeId::new(0),
                                integer_range: None,
                            },
                        ],
                        body: Block { statements },
                        source: SourceSpan::UNKNOWN,
                    });
                    let mut stats = PassStats::default();
                    // A later reassignment cannot invalidate a proof for an
                    // earlier access; the relational pass tracks value versions.
                    let proven = creation != BoundsMode::Clamp && len == 2;
                    assert_eq!(
                        eliminate_proven_bounds_checks(&mut program, &mut stats),
                        proven
                    );
                    assert_eq!(stats.eliminated_bounds_checks, if proven { 2 } else { 0 });
                }
            }
        }
    }

    #[test]
    fn eliminates_redundant_normalization_and_fixed_buffer_selectors() {
        let i32_ty = TypeId::new(0);
        let buffer_span_ty = TypeId::new(1);
        let selector_range = IntegerRangeInvariant {
            min: ScalarValue::I32(0),
            max: ScalarValue::I32(3),
            mode: IntegerRangeMode::Wrap,
        };
        let mut function = Function {
            name: "proofs".to_owned(),
            kind: FunctionKind::User,
            attributes: FunctionAttributes::default(),
            params: vec![FunctionParam {
                integer_range: None,
                name: "buffers".to_owned(),
                ty: buffer_span_ty,
                mode: PassingMode::Value,
            }],
            results: Vec::new(),
            locals: vec![
                Local {
                    integer_range: Some(selector_range),
                    name: Some("selector".to_owned()),
                    ty: i32_ty,
                },
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
            body: Block {
                statements: vec![
                    assign_intrinsic(LocalId::new(1), Intrinsic::RangeClamp),
                    assign_intrinsic(LocalId::new(2), Intrinsic::RangeWrap),
                    Statement {
                        kind: StatementKind::Assign {
                            destination: Place::local(LocalId::new(3)),
                            value: Rvalue::BufferLen(BufferRef::ArrayElement {
                                first: BufferId::new(0),
                                len: 4,
                                selector: Value::Local(LocalId::new(0)),
                                bounds: BoundsMode::Clamp,
                            }),
                        },
                        source: SourceSpan::UNKNOWN,
                    },
                    Statement {
                        kind: StatementKind::Assign {
                            destination: Place::local(LocalId::new(4)),
                            value: Rvalue::BufferParamLen(BufferParamRef::ArrayElement {
                                span: ParameterId::new(0),
                                selector: Value::Local(LocalId::new(0)),
                                bounds: BoundsMode::Checked,
                            }),
                        },
                        source: SourceSpan::UNKNOWN,
                    },
                ],
            },
            source: SourceSpan::UNKNOWN,
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
            Type::Scalar(ScalarType::I32),
            Type::BufferSpan {
                element: ScalarType::F32,
                channels: BufferChannels::Static(1),
                access: AccessMode::ReadOnly,
                len: 4,
            },
        ];
        program.functions.push(function.clone());

        let mut stats = super::super::PassStats::default();
        assert!(eliminate_proven_bounds_checks(&mut program, &mut stats));
        assert_eq!(stats.eliminated_bounds_checks, 4);
        function = program
            .functions
            .pop()
            .expect("proof function should remain");

        for statement in &function.body.statements[..2] {
            let StatementKind::Assign {
                value: Rvalue::Use(Value::Local(local)),
                ..
            } = statement.kind
            else {
                panic!("redundant integer normalization should become a direct use")
            };
            assert_eq!(local, LocalId::new(0));
        }
        let StatementKind::Assign {
            value:
                Rvalue::BufferLen(BufferRef::ArrayElement {
                    bounds: buffer_bounds,
                    ..
                }),
            ..
        } = function.body.statements[2].kind
        else {
            panic!("expected fixed interface-buffer selector")
        };
        let StatementKind::Assign {
            value:
                Rvalue::BufferParamLen(BufferParamRef::ArrayElement {
                    bounds: parameter_bounds,
                    ..
                }),
            ..
        } = function.body.statements[3].kind
        else {
            panic!("expected fixed buffer-parameter selector")
        };
        assert_eq!(buffer_bounds, BoundsMode::Unchecked);
        assert_eq!(parameter_bounds, BoundsMode::Unchecked);
    }

    fn assign_intrinsic(destination: LocalId, intrinsic: Intrinsic) -> Statement {
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(destination),
                value: Rvalue::Intrinsic {
                    intrinsic,
                    args: vec![
                        Value::Local(LocalId::new(0)),
                        Value::Constant(ScalarValue::I32(0)),
                        Value::Constant(ScalarValue::I32(3)),
                    ],
                },
            },
            source: SourceSpan::UNKNOWN,
        }
    }
}
