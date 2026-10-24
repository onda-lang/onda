//! One traversal of indexable surfaces for interval and path-sensitive proofs.
//! Only bounds with the same `0 <= index < length` contract are exposed here.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum Limit {
    Fixed(u32),
    Slice(Value),
    Value(Value),
}

type Prove<'a> = dyn FnMut(Value, Limit, &mut BoundsMode) + 'a;

pub(super) fn visit(context: &Context<'_>, statement: &mut StatementKind, prove: &mut Prove<'_>) {
    match statement {
        StatementKind::Assign { destination, value } => {
            place(context, destination, prove);
            rvalue(context, value, prove);
        }
        StatementKind::Call { function, args, .. } => {
            for (index, arg) in args.iter_mut().enumerate() {
                argument(context, arg, prove);
                if let CallArgument::ArrayWindow {
                    array,
                    start,
                    bounds,
                } = arg
                {
                    let source =
                        place_type(context, array).and_then(|ty| array_len(context.program, ty));
                    let window = array_len(
                        context.program,
                        context.program.functions[function.index()].params[index].ty,
                    );
                    if let Some((source, window)) = source.zip(window) {
                        if let Some(len) = source.checked_sub(window).and_then(|n| n.checked_add(1))
                        {
                            prove(*start, Limit::Fixed(len), bounds);
                        }
                    }
                }
                if let CallArgument::SliceWindow {
                    slice,
                    start,
                    bounds,
                } = arg
                {
                    // Removing this check needs both an index proof and a
                    // contiguous, sufficiently large descriptor payload.
                    let source = context
                        .memory
                        .slice(context.function_id, *slice)
                        .and_then(super::views::contiguous_length);
                    let window = array_len(
                        context.program,
                        context.program.functions[function.index()].params[index].ty,
                    );
                    if let Some((source, window)) = source.zip(window) {
                        if let Some(len) = source.checked_sub(window).and_then(|n| n.checked_add(1))
                        {
                            prove(*start, Limit::Fixed(len), bounds);
                        }
                    }
                }
            }
        }
        StatementKind::PublishDelegate { args, .. } => {
            for arg in args {
                argument(context, arg, prove);
            }
        }
        StatementKind::OutputStore {
            output,
            element: Some(index),
            bounds,
            ..
        } => {
            if let Some(len) = array_len(
                context.program,
                context.program.interface.outputs[output.index()].ty,
            ) {
                prove(*index, Limit::Fixed(len), bounds);
            }
        }
        StatementKind::ControlOutputStore {
            output,
            element: Some(index),
            bounds,
            ..
        } => {
            if let Some(len) = array_len(
                context.program,
                context.program.interface.control_outputs[output.index()].ty,
            ) {
                prove(*index, Limit::Fixed(len), bounds);
            }
        }
        StatementKind::BufferStore { buffer, .. } => buffer_ref(buffer, prove),
        StatementKind::BufferParamStore { parameter, .. } => {
            buffer_param(context, parameter, prove)
        }
        StatementKind::SliceStore {
            slice,
            index,
            bounds,
            ..
        } => {
            prove(*index, Limit::Slice(*slice), bounds);
        }
        _ => {}
    }
}

fn rvalue(context: &Context<'_>, value: &mut Rvalue, prove: &mut Prove<'_>) {
    match value {
        Rvalue::Load(source) => place(context, source, prove),
        Rvalue::InputLoad {
            input,
            element: Some(index),
            bounds,
            ..
        } => {
            if let Some(len) = array_len(
                context.program,
                context.program.interface.inputs[input.index()].ty,
            ) {
                prove(*index, Limit::Fixed(len), bounds);
            }
        }
        Rvalue::OutputLoad {
            output,
            element: Some(index),
            bounds,
            ..
        } => {
            if let Some(len) = array_len(
                context.program,
                context.program.interface.outputs[output.index()].ty,
            ) {
                prove(*index, Limit::Fixed(len), bounds);
            }
        }
        Rvalue::ConstDataLoad {
            data,
            index,
            bounds,
        } => {
            if let Ok(len) = u32::try_from(context.program.const_data[data.index()].values.len()) {
                prove(*index, Limit::Fixed(len), bounds);
            }
        }
        Rvalue::BufferLoad { buffer, .. }
        | Rvalue::BufferLen(buffer)
        | Rvalue::BufferChannels(buffer)
        | Rvalue::BufferSampleRate(buffer)
        | Rvalue::BufferIsBound(buffer) => buffer_ref(buffer, prove),
        Rvalue::BufferParamLoad { parameter, .. }
        | Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter)
        | Rvalue::BufferParamIsBound(parameter) => buffer_param(context, parameter, prove),
        Rvalue::MakeSlice { source, .. } => match source {
            crate::SliceSource::Place(source) => place(context, source, prove),
            crate::SliceSource::Buffer { buffer, .. } => buffer_ref(buffer, prove),
            crate::SliceSource::BufferParam { parameter, .. } => {
                buffer_param(context, parameter, prove)
            }
            crate::SliceSource::ConstData(_) => {}
        },
        Rvalue::SliceLoad {
            slice,
            index,
            bounds,
        } => prove(*index, Limit::Slice(*slice), bounds),
        Rvalue::NormalizeIndex {
            index,
            length,
            bounds,
        } => prove(*index, Limit::Value(*length), bounds),
        _ => {}
    }
}

fn argument(context: &Context<'_>, argument: &mut CallArgument, prove: &mut Prove<'_>) {
    match argument {
        CallArgument::Place(source) | CallArgument::ArrayWindow { array: source, .. } => {
            place(context, source, prove);
        }
        CallArgument::SliceElement {
            slice,
            index,
            bounds,
        } => prove(*index, Limit::Slice(*slice), bounds),
        CallArgument::Buffer(buffer) => buffer_ref(buffer, prove),
        CallArgument::BufferParam(parameter) => buffer_param(context, parameter, prove),
        // A slice window also needs a unit-stride and minimum-shape proof.
        // An index proof alone must not remove its runtime shape checks.
        _ => {}
    }
}

fn buffer_ref(buffer: &mut crate::BufferRef, prove: &mut Prove<'_>) {
    if let crate::BufferRef::ArrayElement {
        len,
        selector,
        bounds,
        ..
    } = buffer
    {
        prove(*selector, Limit::Fixed(*len), bounds);
    }
}

fn buffer_param(
    context: &Context<'_>,
    parameter: &mut crate::BufferParamRef,
    prove: &mut Prove<'_>,
) {
    if let crate::BufferParamRef::ArrayElement {
        span,
        selector,
        bounds,
    } = parameter
    {
        if let Type::BufferSpan { len, .. } =
            context.program.types[context.function.params[span.index()].ty.index()]
        {
            prove(*selector, Limit::Fixed(len), bounds);
        }
    }
}

fn place(context: &Context<'_>, place: &mut Place, prove: &mut Prove<'_>) {
    let mut ty = base_type(context, place.base);
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
                ty = ty.and_then(|ty| match context.program.types.get(ty.index())? {
                    Type::Array { element, len } => {
                        prove(*index, Limit::Fixed(*len), bounds);
                        Some(*element)
                    }
                    _ => None,
                });
            }
        }
    }
}
