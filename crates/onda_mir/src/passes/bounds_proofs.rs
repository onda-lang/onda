use crate::{
    analyze_integer_ranges, Block, BoundsMode, CallArgument, Function, FunctionId,
    FunctionRangeAnalysis, Place, PlaceBase, Projection, Rvalue, ScalarValue, StatementKind, Type,
    TypeId, Value,
};

use super::PassStats;

pub(super) fn eliminate_proven_bounds_checks(
    program: &mut crate::Program,
    stats: &mut PassStats,
) -> bool {
    let mut changed = false;
    for function_index in 0..program.functions.len() {
        let function_id = FunctionId::new(function_index as u32);
        let ranges = analyze_integer_ranges(program, function_id);
        let function = program.functions[function_index].clone();
        let mut body = std::mem::take(&mut program.functions[function_index].body);
        let context = Context {
            program,
            function: &function,
            ranges: &ranges,
        };
        let eliminated = prove_block(&context, &mut body);
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
}

fn prove_block(context: &Context<'_>, block: &mut Block) -> u64 {
    let mut eliminated = 0;
    for statement in &mut block.statements {
        match &mut statement.kind {
            StatementKind::Assign { destination, value } => {
                eliminated += prove_place(context, destination);
                eliminated += prove_rvalue(context, value);
            }
            StatementKind::Call { function, args, .. } => {
                for (index, argument) in args.iter_mut().enumerate() {
                    eliminated += prove_call_argument(context, *function, index, argument);
                }
            }
            StatementKind::PublishDelegate { args, .. } => {
                for argument in args {
                    eliminated += prove_publish_argument(context, argument);
                }
            }
            StatementKind::OutputStore {
                output,
                element,
                bounds,
                ..
            } => {
                if let Some(index) = element {
                    if let Some(len) = context
                        .program
                        .interface
                        .outputs
                        .get(output.index())
                        .and_then(|output| array_len(context.program, output.ty))
                    {
                        eliminated += prove_index(context, *index, len, bounds);
                    }
                }
            }
            StatementKind::ControlOutputStore {
                output,
                element,
                bounds,
                ..
            } => {
                if let Some(index) = element {
                    if let Some(len) = context
                        .program
                        .interface
                        .control_outputs
                        .get(output.index())
                        .and_then(|output| array_len(context.program, output.ty))
                    {
                        eliminated += prove_index(context, *index, len, bounds);
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                eliminated += prove_block(context, then_block);
                eliminated += prove_block(context, else_block);
            }
            StatementKind::Loop { body } => eliminated += prove_block(context, body),
            StatementKind::BufferStore { buffer, .. } => {
                eliminated += prove_buffer_ref(context, buffer);
            }
            StatementKind::BufferParamStore { parameter, .. } => {
                eliminated += prove_buffer_param_ref(context, parameter);
            }
            StatementKind::SliceStore { .. }
            | StatementKind::SliceFill { .. }
            | StatementKind::SliceCopy { .. }
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Return { .. } => {}
            StatementKind::PublishLog { .. } => {}
        }
    }
    eliminated
}

fn prove_publish_argument(context: &Context<'_>, argument: &mut CallArgument) -> u64 {
    match argument {
        CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => {
            prove_place(context, place)
        }
        CallArgument::Buffer(buffer) => prove_buffer_ref(context, buffer),
        CallArgument::BufferParam(parameter) => prove_buffer_param_ref(context, parameter),
        CallArgument::Value(_)
        | CallArgument::SliceElement { .. }
        | CallArgument::SliceWindow { .. }
        | CallArgument::BufferSpan(_) => 0,
    }
}

fn prove_rvalue(context: &Context<'_>, value: &mut Rvalue) -> u64 {
    if let Some(original) = redundant_normalization_operand(context, value) {
        *value = Rvalue::Use(original);
        return 1;
    }

    match value {
        Rvalue::Load(place) => prove_place(context, place),
        Rvalue::InputLoad {
            input,
            element: Some(index),
            bounds,
            ..
        } => context
            .program
            .interface
            .inputs
            .get(input.index())
            .and_then(|input| array_len(context.program, input.ty))
            .map_or(0, |len| prove_index(context, *index, len, bounds)),
        Rvalue::OutputLoad {
            output,
            element: Some(index),
            bounds,
            ..
        } => context
            .program
            .interface
            .outputs
            .get(output.index())
            .and_then(|output| array_len(context.program, output.ty))
            .map_or(0, |len| prove_index(context, *index, len, bounds)),
        Rvalue::ConstDataLoad {
            data,
            index,
            bounds,
        } => context
            .program
            .const_data
            .get(data.index())
            .and_then(|data| u32::try_from(data.values.len()).ok())
            .map_or(0, |len| prove_index(context, *index, len, bounds)),
        Rvalue::BufferLoad { buffer, .. }
        | Rvalue::BufferLen(buffer)
        | Rvalue::BufferChannels(buffer)
        | Rvalue::BufferSampleRate(buffer) => prove_buffer_ref(context, buffer),
        Rvalue::BufferParamLoad { parameter, .. }
        | Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter) => prove_buffer_param_ref(context, parameter),
        Rvalue::MakeSlice { source, .. } => prove_slice_source(context, source),
        _ => 0,
    }
}

fn prove_call_argument(
    context: &Context<'_>,
    callee: FunctionId,
    parameter_index: usize,
    argument: &mut CallArgument,
) -> u64 {
    match argument {
        CallArgument::Place(place) => prove_place(context, place),
        CallArgument::ArrayWindow {
            array,
            start,
            bounds,
        } => {
            let mut eliminated = prove_place(context, array);
            let source_len =
                place_type(context, array).and_then(|ty| array_len(context.program, ty));
            let window_len = context
                .program
                .functions
                .get(callee.index())
                .and_then(|callee| callee.params.get(parameter_index))
                .and_then(|parameter| array_len(context.program, parameter.ty));
            if let Some((source_len, window_len)) = source_len.zip(window_len) {
                if window_len <= source_len {
                    eliminated += prove_index(context, *start, source_len - window_len + 1, bounds);
                }
            }
            eliminated
        }
        CallArgument::Buffer(buffer) => prove_buffer_ref(context, buffer),
        CallArgument::BufferParam(parameter) => prove_buffer_param_ref(context, parameter),
        _ => 0,
    }
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

fn prove_slice_source(context: &Context<'_>, source: &mut crate::SliceSource) -> u64 {
    match source {
        crate::SliceSource::Place(place) => prove_place(context, place),
        crate::SliceSource::Buffer { buffer, .. } => prove_buffer_ref(context, buffer),
        crate::SliceSource::BufferParam { parameter, .. } => {
            prove_buffer_param_ref(context, parameter)
        }
        crate::SliceSource::ConstData(_) => 0,
    }
}

fn prove_buffer_ref(context: &Context<'_>, buffer: &mut crate::BufferRef) -> u64 {
    let crate::BufferRef::ArrayElement {
        len,
        selector,
        bounds,
        ..
    } = buffer
    else {
        return 0;
    };
    prove_index(context, *selector, *len, bounds)
}

fn prove_buffer_param_ref(context: &Context<'_>, parameter: &mut crate::BufferParamRef) -> u64 {
    let crate::BufferParamRef::ArrayElement {
        span,
        selector,
        bounds,
    } = parameter
    else {
        return 0;
    };
    let Some(len) = context
        .function
        .params
        .get(span.index())
        .and_then(|parameter| context.program.types.get(parameter.ty.index()))
        .and_then(|ty| match ty {
            Type::BufferSpan { len, .. } => Some(*len),
            _ => None,
        })
    else {
        return 0;
    };
    prove_index(context, *selector, len, bounds)
}

fn prove_place(context: &Context<'_>, place: &mut Place) -> u64 {
    let mut ty = base_type(context, place.base);
    let mut eliminated = 0;
    for projection in &mut place.projections {
        match projection {
            Projection::Field(field) => {
                ty = ty.and_then(|ty| match context.program.types.get(ty.index())? {
                    Type::Struct(structure) => context
                        .program
                        .structs
                        .get(structure.index())?
                        .fields
                        .get(field.index())
                        .map(|field| field.ty),
                    _ => None,
                });
            }
            Projection::Index { index, bounds } => {
                let array = ty.and_then(|ty| match context.program.types.get(ty.index())? {
                    Type::Array { element, len } => Some((*element, *len)),
                    _ => None,
                });
                if let Some((element, len)) = array {
                    eliminated += prove_index(context, *index, len, bounds);
                    ty = Some(element);
                } else {
                    ty = None;
                }
            }
        }
    }
    eliminated
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
    let mut ty = base_type(context, place.base)?;
    for projection in &place.projections {
        ty = match projection {
            Projection::Field(field) => {
                let Type::Struct(structure) = context.program.types.get(ty.index())? else {
                    return None;
                };
                context
                    .program
                    .structs
                    .get(structure.index())?
                    .fields
                    .get(field.index())?
                    .ty
            }
            Projection::Index { .. } => {
                let Type::Array { element, .. } = context.program.types.get(ty.index())? else {
                    return None;
                };
                *element
            }
        };
    }
    Some(ty)
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
