//! Snapshot ranged parameter arrays at exported execution boundaries. All array
//! reads, including slices and references passed to helpers, use the same copy.
use onda_mir::*;
mod reads;

pub(super) fn clamp_parameter_arrays(program: &mut Program) {
    if !program.interface.params.iter().any(|param| {
        param.range.is_some() && matches!(program.types[param.ty.index()], Type::Array { .. })
    }) {
        return;
    }
    let required = reads::required_snapshots(program);
    let user_functions = program
        .functions
        .iter()
        .map(|function| function.kind == FunctionKind::User)
        .collect::<Vec<_>>();
    let i32_ty = intern(&mut program.types, ScalarType::I32);
    let bool_ty = intern(&mut program.types, ScalarType::Bool);
    for (function_index, function) in program.functions.iter_mut().enumerate() {
        let mut mirrors = vec![None; program.interface.params.len()];
        for &index in &required[function_index] {
            let param = &program.interface.params[index];
            let base = match function.kind {
                FunctionKind::User => {
                    let id = ParameterId::new(function.params.len() as u32);
                    function.params.push(FunctionParam {
                        name: format!("__onda_clamped_array__{}", param.name),
                        ty: param.ty,
                        mode: PassingMode::ReadOnlyReference,
                        integer_range: None,
                    });
                    PlaceBase::Parameter(id)
                }
                FunctionKind::Process => {
                    let id = StateId::new(program.state.len() as u32);
                    program.state.push(StateSlot {
                        name: format!("__onda_clamped_array__{}", param.name),
                        ty: param.ty,
                        persistence: StatePersistence::InstanceScratch,
                        authored: false,
                        pinned: false,
                        integer_range: None,
                    });
                    PlaceBase::State(id)
                }
                FunctionKind::Init | FunctionKind::Event(_) => {
                    PlaceBase::Local(local(function, param.ty))
                }
            };
            mirrors[index] = Some(base);
        }
        visit_places(&mut function.body, &mut |place| {
            rewrite_place(place, &mirrors)
        });
        forward_snapshots(&mut function.body, &required, &mirrors, &user_functions);
        if function.kind == FunctionKind::User {
            continue;
        }
        let mut prefix = Block::default();
        for (index, mirror) in mirrors.iter().enumerate() {
            let Some(base) = *mirror else {
                continue;
            };
            let param = &program.interface.params[index];
            let Type::Array { element, len } = program.types[param.ty.index()] else {
                unreachable!()
            };
            let range = param.range.expect("ranged array");
            let source = PlaceBase::Param(ParamId::new(index as u32));
            // Establish whole-array initialization before taking references to a
            // local snapshot. Clamp that private copy in place below.
            let source = if let PlaceBase::Local(id) = base {
                prefix.statements.push(assign(
                    id,
                    Rvalue::Load(Place {
                        base: source,
                        projections: Vec::new(),
                    }),
                ));
                base
            } else {
                source
            };
            let cursor = local(function, i32_ty);
            let value = local(function, element);
            let done = local(function, bool_ty);
            prefix
                .statements
                .push(assign(cursor, Rvalue::Use(integer(0))));
            let indexed = |base| Place {
                base,
                projections: vec![Projection::Index {
                    index: Value::Local(cursor),
                    bounds: BoundsMode::Clamp,
                }],
            };
            prefix.statements.push(statement(StatementKind::Loop {
                body: Block {
                    statements: vec![
                        assign(
                            done,
                            Rvalue::Compare {
                                op: CompareOp::GreaterEqual,
                                lhs: Value::Local(cursor),
                                rhs: integer(len as i32),
                            },
                        ),
                        statement(StatementKind::If {
                            condition: Value::Local(done),
                            then_block: Block {
                                statements: vec![statement(StatementKind::Break)],
                            },
                            else_block: Block::default(),
                        }),
                        assign(value, Rvalue::Load(indexed(source))),
                        statement(StatementKind::Assign {
                            destination: indexed(base),
                            value: Rvalue::Intrinsic {
                                intrinsic: Intrinsic::RangeClamp,
                                args: vec![
                                    Value::Local(value),
                                    Value::Constant(range.min),
                                    Value::Constant(range.max),
                                ],
                            },
                        }),
                        assign(
                            cursor,
                            Rvalue::Binary {
                                op: BinaryOp::Add,
                                lhs: Value::Local(cursor),
                                rhs: integer(1),
                            },
                        ),
                    ],
                },
            }));
        }
        if prefix.statements.is_empty() {
            continue;
        }
        if function.kind == FunctionKind::Process {
            let flags = local(function, i32_ty);
            let begin = local(function, bool_ty);
            prefix = Block {
                statements: vec![
                    assign(
                        flags,
                        Rvalue::Load(Place {
                            base: PlaceBase::Parameter(ParameterId::new(
                                PROCESS_FLAGS_PARAM_INDEX as u32,
                            )),
                            projections: vec![],
                        }),
                    ),
                    assign(
                        flags,
                        Rvalue::Binary {
                            op: BinaryOp::BitAnd,
                            lhs: Value::Local(flags),
                            rhs: integer(PROCESSOR_BEGIN_BLOCK),
                        },
                    ),
                    assign(
                        begin,
                        Rvalue::Compare {
                            op: CompareOp::NotEqual,
                            lhs: Value::Local(flags),
                            rhs: integer(0),
                        },
                    ),
                    statement(StatementKind::If {
                        condition: Value::Local(begin),
                        then_block: prefix,
                        else_block: Block::default(),
                    }),
                ],
            };
        }
        prefix.statements.append(&mut function.body.statements);
        function.body = prefix;
    }
}

fn intern(types: &mut Vec<Type>, scalar: ScalarType) -> TypeId {
    let ty = Type::Scalar(scalar);
    let index = types
        .iter()
        .position(|candidate| *candidate == ty)
        .unwrap_or_else(|| {
            types.push(ty);
            types.len() - 1
        });
    TypeId::new(index as u32)
}

fn local(function: &mut Function, ty: TypeId) -> LocalId {
    let id = LocalId::new(function.locals.len() as u32);
    function.locals.push(Local {
        name: None,
        ty,
        integer_range: None,
    });
    id
}

fn integer(value: i32) -> Value {
    Value::Constant(ScalarValue::I32(value))
}
fn statement(kind: StatementKind) -> Statement {
    Statement {
        kind,
        source: SourceSpan::UNKNOWN,
    }
}
fn assign(local: LocalId, value: Rvalue) -> Statement {
    statement(StatementKind::Assign {
        destination: Place::local(local),
        value,
    })
}

fn rewrite_place(place: &mut Place, mirrors: &[Option<PlaceBase>]) {
    if let PlaceBase::Param(param) = place.base {
        if let Some(base) = mirrors[param.index()] {
            place.base = base;
        }
    }
}

fn forward_snapshots(
    block: &mut Block,
    required: &[Vec<usize>],
    mirrors: &[Option<PlaceBase>],
    user_functions: &[bool],
) {
    for statement in &mut block.statements {
        match &mut statement.kind {
            StatementKind::Call { function, args, .. } if user_functions[function.index()] => {
                args.extend(required[function.index()].iter().map(|&index| {
                    CallArgument::Place(Place {
                        base: mirrors[index].expect("callee snapshot is required by caller"),
                        projections: Vec::new(),
                    })
                }));
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                forward_snapshots(then_block, required, mirrors, user_functions);
                forward_snapshots(else_block, required, mirrors, user_functions);
            }
            StatementKind::Loop { body } => {
                forward_snapshots(body, required, mirrors, user_functions);
            }
            _ => {}
        }
    }
}

fn visit_places(block: &mut Block, visit: &mut impl FnMut(&mut Place)) {
    for statement in &mut block.statements {
        match &mut statement.kind {
            StatementKind::Assign { destination, value } => {
                visit(destination);
                match value {
                    Rvalue::Load(place)
                    | Rvalue::MakeSlice {
                        source: SliceSource::Place(place),
                        ..
                    } => visit(place),
                    _ => {}
                }
            }
            StatementKind::Call { args, .. } | StatementKind::PublishDelegate { args, .. } => {
                for arg in args {
                    match arg {
                        CallArgument::Place(place)
                        | CallArgument::ArrayWindow { array: place, .. } => visit(place),
                        _ => {}
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                visit_places(then_block, visit);
                visit_places(else_block, visit);
            }
            StatementKind::Loop { body } => visit_places(body, visit),
            _ => {}
        }
    }
}
