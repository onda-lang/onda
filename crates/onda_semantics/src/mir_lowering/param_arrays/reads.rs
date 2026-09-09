//! Track array contents separately from slice descriptors. Constructing a view,
//! checking its bounds, or querying its length does not read its elements.
use onda_mir::*;
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Origin {
    Global(ParamId),
    Parameter(ParameterId),
}
type Origins = HashSet<Origin>;

struct Reads {
    contents: Origins,
    calls: Vec<(FunctionId, Vec<Origins>)>,
}

pub(super) fn required_snapshots(program: &Program) -> Vec<Vec<usize>> {
    let reads = program
        .functions
        .iter()
        .map(|function| function_reads(program, function))
        .collect::<Vec<_>>();
    let mut contents = reads
        .iter()
        .map(|reads| reads.contents.clone())
        .collect::<Vec<_>>();
    loop {
        let mut changed = false;
        for (index, reads) in reads.iter().enumerate() {
            let mut inherited = Origins::new();
            for (callee, args) in &reads.calls {
                for origin in &contents[callee.index()] {
                    match origin {
                        Origin::Global(_) => {
                            inherited.insert(*origin);
                        }
                        Origin::Parameter(parameter) => {
                            inherited.extend(args[parameter.index()].iter().copied())
                        }
                    }
                }
            }
            let before = contents[index].len();
            contents[index].extend(inherited);
            changed |= contents[index].len() != before;
        }
        if !changed {
            break;
        }
    }
    contents
        .into_iter()
        .map(|contents| {
            program
                .interface
                .params
                .iter()
                .enumerate()
                .filter_map(|(index, param)| {
                    (param.range.is_some()
                        && matches!(program.types[param.ty.index()], Type::Array { .. })
                        && contents.contains(&Origin::Global(ParamId::new(index as u32))))
                    .then_some(index)
                })
                .collect()
        })
        .collect()
}

fn function_reads(program: &Program, function: &Function) -> Reads {
    let mut aliases = vec![Origins::new(); function.locals.len()];
    // Join every assignment, including loop-carried and branch-selected views.
    loop {
        let mut changed = false;
        walk(&function.body, &mut |statement| {
            if let StatementKind::Assign {
                destination:
                    Place {
                        base: PlaceBase::Local(local),
                        projections,
                    },
                value,
            } = statement
            {
                if !projections.is_empty()
                    || !matches!(
                        program.types[function.locals[local.index()].ty.index()],
                        Type::Array { .. } | Type::Slice { .. }
                    )
                {
                    return;
                }
                let origins = match value {
                    Rvalue::Use(value) => value_origins(*value, &aliases),
                    Rvalue::Load(place) => place_origins(place, &aliases),
                    Rvalue::MakeSlice {
                        source: SliceSource::Place(place),
                        ..
                    } => place_origins(place, &aliases),
                    _ => Origins::new(),
                };
                let before = aliases[local.index()].len();
                aliases[local.index()].extend(origins);
                changed |= aliases[local.index()].len() != before;
            }
        });
        if !changed {
            break;
        }
    }
    let mut reads = Reads {
        contents: Origins::new(),
        calls: Vec::new(),
    };
    walk(&function.body, &mut |statement| {
        match statement {
            StatementKind::Assign { destination, value } => match value {
                // A whole slice load copies only the descriptor. Every other
                // load reads scalar/aggregate contents, including references.
                Rvalue::Load(place) if !is_slice_local(program, function, destination) => {
                    reads.contents.extend(place_origins(place, &aliases));
                }
                Rvalue::SliceLoad { slice, .. } => {
                    reads.contents.extend(value_origins(*slice, &aliases))
                }
                _ => {}
            },
            StatementKind::Call { function, args, .. } => {
                reads.calls.push((
                    *function,
                    args.iter()
                        .map(|arg| argument_origins(arg, &aliases))
                        .collect(),
                ));
            }
            StatementKind::PublishDelegate { args, .. } => {
                for arg in args {
                    reads.contents.extend(argument_origins(arg, &aliases));
                }
            }
            StatementKind::SliceCopy { copies } => {
                for copy in copies {
                    reads.contents.extend(value_origins(copy.source, &aliases));
                }
            }
            // Returning an aggregate exposes its contents to the caller.
            StatementKind::Return { values } => {
                for value in values {
                    reads.contents.extend(value_origins(*value, &aliases));
                }
            }
            _ => {}
        }
    });
    reads
}

fn is_slice_local(program: &Program, function: &Function, place: &Place) -> bool {
    matches!(place.base, PlaceBase::Local(local)
        if place.projections.is_empty() && matches!(program.types[function.locals[local.index()].ty.index()], Type::Slice { .. }))
}

fn value_origins(value: Value, aliases: &[Origins]) -> Origins {
    match value {
        Value::Local(local) => aliases[local.index()].clone(),
        Value::Constant(_) => Origins::new(),
    }
}

fn place_origins(place: &Place, aliases: &[Origins]) -> Origins {
    match place.base {
        PlaceBase::Param(param) => Origins::from([Origin::Global(param)]),
        PlaceBase::Parameter(param) => Origins::from([Origin::Parameter(param)]),
        PlaceBase::Local(local) => aliases[local.index()].clone(),
        PlaceBase::State(_) | PlaceBase::EventParam(_) => Origins::new(),
    }
}

fn argument_origins(argument: &CallArgument, aliases: &[Origins]) -> Origins {
    match argument {
        CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => {
            place_origins(place, aliases)
        }
        CallArgument::Value(value)
        | CallArgument::SliceElement { slice: value, .. }
        | CallArgument::SliceWindow { slice: value, .. } => value_origins(*value, aliases),
        _ => Origins::new(),
    }
}

fn walk(block: &Block, visit: &mut impl FnMut(&StatementKind)) {
    for statement in &block.statements {
        visit(&statement.kind);
        match &statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                walk(then_block, visit);
                walk(else_block, visit);
            }
            StatementKind::Loop { body } => walk(body, visit),
            _ => {}
        }
    }
}
