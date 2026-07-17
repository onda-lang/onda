use std::collections::{HashMap, HashSet};
use std::fmt;

use onda_frontend::PrimitiveType;
use onda_mir::{
    AccessMode, Block, BufferChannels, CallArgument, ConstantValue, Function, Place, PlaceBase,
    Program, Rvalue, ScalarType, ScalarValue, SliceSource, StatePersistence, StatementKind, Type,
    Value, ValueRange,
};
use onda_semantics::{TypedConstValue, TypedValueRange};

use crate::primitives::{append_typed_const_bytes, primitive_type_bytes};
use crate::runtime_metadata::ProgramMetadata;
use crate::{
    DeclaredBuffer, DeclaredBufferChannels, DeclaredEvent, DeclaredEventParam, DeclaredIo,
    DeclaredState,
};

/// Target-specific offsets computed by the MIR code generator.
///
/// The metadata builder owns no ABI-layout policy: it consumes the exact layout
/// selected by the backend and only derives logical descriptors from MIR.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MirMetadataLayoutView<'a> {
    pub(crate) state_offsets: &'a [usize],
    pub(crate) param_offsets: &'a [usize],
    pub(crate) control_output_offsets: &'a [usize],
    pub(crate) input_bases: &'a [usize],
    pub(crate) output_bases: &'a [usize],
    pub(crate) event_fixed_sizes: &'a [Option<usize>],
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct MirMetadataError {
    pub(crate) message: String,
}

impl MirMetadataError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for MirMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MirMetadataError {}

#[derive(Debug, Clone, Copy)]
struct ScalarArrayShape {
    element: PrimitiveType,
    len: usize,
    is_array: bool,
}

/// Builds the existing runtime descriptor model solely from validated MIR and
/// the physical layout selected by a backend.
pub(crate) fn build_mir_program_metadata(
    program: &Program,
    layout: MirMetadataLayoutView<'_>,
) -> Result<ProgramMetadata, MirMetadataError> {
    validate_layout_lengths(program, layout)?;

    let inputs = build_inputs(program, layout.input_bases)?;
    let outputs = build_outputs(program, layout.output_bases)?;
    let control_outputs = build_control_outputs(program, layout.control_output_offsets)?;
    let params = build_params(program, layout.param_offsets)?;
    let events = build_events(program, layout.event_fixed_sizes)?;
    let buffers = build_buffers(program)?;
    let state_entries =
        build_state_entries(program, layout.state_offsets, layout.control_output_offsets)?;

    Ok(ProgramMetadata {
        input_index: build_io_name_index(&inputs),
        output_index: build_io_name_index(&outputs),
        control_output_index: build_io_name_index(&control_outputs),
        param_index: build_io_name_index(&params),
        event_index: events
            .iter()
            .enumerate()
            .map(|(index, event)| (event.name().to_owned(), index))
            .collect(),
        buffer_index: buffers
            .iter()
            .enumerate()
            .map(|(index, buffer)| (buffer.name().to_owned(), index))
            .collect(),
        inputs,
        outputs,
        control_outputs,
        params,
        events,
        buffers,
        state_entries,
    })
}

fn validate_layout_lengths(
    program: &Program,
    layout: MirMetadataLayoutView<'_>,
) -> Result<(), MirMetadataError> {
    let expected = [
        ("state", program.state.len(), layout.state_offsets.len()),
        (
            "parameter",
            program.interface.params.len(),
            layout.param_offsets.len(),
        ),
        (
            "control-output",
            program.interface.control_outputs.len(),
            layout.control_output_offsets.len(),
        ),
        (
            "input",
            program.interface.inputs.len(),
            layout.input_bases.len(),
        ),
        (
            "output",
            program.interface.outputs.len(),
            layout.output_bases.len(),
        ),
        (
            "event",
            program.interface.events.len(),
            layout.event_fixed_sizes.len(),
        ),
    ];
    for (kind, expected, actual) in expected {
        if expected != actual {
            return Err(MirMetadataError::new(format!(
                "MIR {kind} metadata layout has {actual} entries; expected {expected}"
            )));
        }
    }

    for (index, offset) in layout.control_output_offsets.iter().copied().enumerate() {
        if !layout.state_offsets.contains(&offset) {
            return Err(MirMetadataError::new(format!(
                "MIR control output {index} storage offset {offset} is not a state-slot offset"
            )));
        }
    }
    Ok(())
}

fn build_inputs(program: &Program, bases: &[usize]) -> Result<Vec<DeclaredIo>, MirMetadataError> {
    let mut entries = Vec::with_capacity(program.interface.inputs.len());
    let mut expected_base = 0usize;
    let mut byte_offset = 0usize;
    for (index, input) in program.interface.inputs.iter().enumerate() {
        if bases[index] != expected_base {
            return Err(MirMetadataError::new(format!(
                "MIR input '{}' channel base is {}, expected {expected_base}",
                input.name, bases[index]
            )));
        }
        let descriptor = build_io_descriptor(
            program,
            &input.name,
            input.ty,
            input.default.as_ref(),
            input.range,
            bases[index],
            byte_offset,
            None,
        )?;
        expected_base = checked_add(expected_base, descriptor.array_len, "input channel count")?;
        byte_offset = checked_add(byte_offset, descriptor.byte_size(), "input byte offset")?;
        entries.push(descriptor);
    }
    Ok(entries)
}

fn build_outputs(program: &Program, bases: &[usize]) -> Result<Vec<DeclaredIo>, MirMetadataError> {
    let mut entries = Vec::with_capacity(program.interface.outputs.len());
    let mut expected_base = 0usize;
    let mut byte_offset = 0usize;
    for (index, output) in program.interface.outputs.iter().enumerate() {
        if bases[index] != expected_base {
            return Err(MirMetadataError::new(format!(
                "MIR output '{}' channel base is {}, expected {expected_base}",
                output.name, bases[index]
            )));
        }
        let descriptor = build_io_descriptor(
            program,
            &output.name,
            output.ty,
            None,
            None,
            bases[index],
            byte_offset,
            None,
        )?;
        expected_base = checked_add(expected_base, descriptor.array_len, "output channel count")?;
        byte_offset = checked_add(byte_offset, descriptor.byte_size(), "output byte offset")?;
        entries.push(descriptor);
    }
    Ok(entries)
}

fn build_control_outputs(
    program: &Program,
    state_offsets: &[usize],
) -> Result<Vec<DeclaredIo>, MirMetadataError> {
    let mut entries = Vec::with_capacity(program.interface.control_outputs.len());
    let mut slot_offset = 0usize;
    let mut byte_offset = 0usize;
    for (index, output) in program.interface.control_outputs.iter().enumerate() {
        let descriptor = build_io_descriptor(
            program,
            &output.name,
            output.ty,
            None,
            None,
            slot_offset,
            byte_offset,
            Some(state_offsets[index]),
        )?;
        slot_offset = checked_add(
            slot_offset,
            descriptor.array_len,
            "control-output slot offset",
        )?;
        byte_offset = checked_add(
            byte_offset,
            descriptor.byte_size(),
            "control-output byte offset",
        )?;
        entries.push(descriptor);
    }
    Ok(entries)
}

fn build_params(program: &Program, offsets: &[usize]) -> Result<Vec<DeclaredIo>, MirMetadataError> {
    let mut entries = Vec::with_capacity(program.interface.params.len());
    let mut slot_offset = 0usize;
    for (index, param) in program.interface.params.iter().enumerate() {
        let descriptor = build_io_descriptor(
            program,
            &param.name,
            param.ty,
            Some(&param.default),
            param.range,
            slot_offset,
            offsets[index],
            None,
        )?;
        slot_offset = checked_add(slot_offset, descriptor.array_len, "parameter slot offset")?;
        entries.push(descriptor);
    }
    Ok(entries)
}

#[allow(clippy::too_many_arguments)]
fn build_io_descriptor(
    program: &Program,
    name: &str,
    ty: onda_mir::TypeId,
    default: Option<&ConstantValue>,
    range: Option<ValueRange>,
    slot_offset: usize,
    byte_offset: usize,
    state_byte_offset: Option<usize>,
) -> Result<DeclaredIo, MirMetadataError> {
    let shape = scalar_array_shape(program, ty, "runtime I/O")?;
    if shape.is_array && range.is_some() {
        return Err(MirMetadataError::new(format!(
            "MIR array descriptor '{name}' unexpectedly has a scalar range"
        )));
    }
    let default_bytes = default
        .map(|value| constant_bytes(program, value, ty))
        .transpose()?;
    let scalar_default = if shape.is_array {
        None
    } else {
        default.map(scalar_constant_value).transpose()?
    };
    let range = range
        .map(|range| typed_range(range, shape.element))
        .transpose()?;

    Ok(DeclaredIo {
        name: name.to_owned(),
        elem_ty: shape.element,
        array_len: shape.len,
        slot_offset,
        byte_offset,
        state_byte_offset,
        default: scalar_default,
        default_bytes,
        range,
    })
}

fn build_state_entries(
    program: &Program,
    offsets: &[usize],
    control_offsets: &[usize],
) -> Result<Vec<DeclaredState>, MirMetadataError> {
    let control_offsets = control_offsets.iter().copied().collect::<HashSet<_>>();
    let mut entries = Vec::new();
    let mut snapshot_offset = 0usize;
    for (index, slot) in program.state.iter().enumerate() {
        if slot.persistence == StatePersistence::InstanceScratch
            || control_offsets.contains(&offsets[index])
        {
            continue;
        }
        let shape = scalar_array_shape(program, slot.ty, "state manifest")?;
        let byte_size = primitive_type_bytes(shape.element)
            .checked_mul(shape.len)
            .ok_or_else(|| MirMetadataError::new("snapshot state byte size overflow"))?;
        entries.push(DeclaredState {
            name: slot.name.clone(),
            elem_ty: shape.element,
            array_len: shape.len,
            byte_offset: snapshot_offset,
            storage_byte_offset: offsets[index],
        });
        snapshot_offset = snapshot_offset
            .checked_add(byte_size)
            .ok_or_else(|| MirMetadataError::new("snapshot layout byte size overflow"))?;
    }
    Ok(entries)
}

fn build_events(
    program: &Program,
    fixed_sizes: &[Option<usize>],
) -> Result<Vec<DeclaredEvent>, MirMetadataError> {
    let mut events = Vec::with_capacity(program.interface.events.len());
    for (event_index, event) in program.interface.events.iter().enumerate() {
        let mut params = Vec::with_capacity(event.params.len());
        let mut minimum_wire_offset = 0usize;
        let mut computed_fixed_size = Some(0usize);

        for param in &event.params {
            match &program.types[param.ty.index()] {
                Type::Scalar(scalar) => {
                    let element = primitive_type(*scalar);
                    let bytes = primitive_type_bytes(element);
                    let default_bytes = param
                        .default
                        .as_ref()
                        .map(|value| constant_bytes(program, value, param.ty))
                        .transpose()?;
                    let default_values = param
                        .default
                        .as_ref()
                        .map(|value| constant_values(program, value, param.ty))
                        .transpose()?;
                    params.push(DeclaredEventParam {
                        name: param.name.clone(),
                        elem_ty: element,
                        array_len: 1,
                        is_slice: false,
                        byte_offset: minimum_wire_offset,
                        default_bytes,
                        default_values,
                    });
                    minimum_wire_offset =
                        checked_add(minimum_wire_offset, bytes, "event parameter wire offset")?;
                    if let Some(size) = computed_fixed_size.as_mut() {
                        *size = checked_add(*size, bytes, "event payload size")?;
                    }
                }
                Type::Array { element, len } => {
                    let Type::Scalar(scalar) =
                        program.types.get(element.index()).ok_or_else(|| {
                            MirMetadataError::new(format!(
                            "MIR event '{}' parameter '{}' references a missing array element type",
                            event.name, param.name
                        ))
                        })?
                    else {
                        return Err(MirMetadataError::new(format!(
                            "MIR event '{}' parameter '{}' is not a one-dimensional scalar array",
                            event.name, param.name
                        )));
                    };
                    let len = usize::try_from(*len).map_err(|_| {
                        MirMetadataError::new("MIR event array length does not fit usize")
                    })?;
                    let elem_ty = primitive_type(*scalar);
                    let bytes = primitive_type_bytes(elem_ty)
                        .checked_mul(len)
                        .ok_or_else(|| MirMetadataError::new("MIR event array size overflow"))?;
                    let default_bytes = param
                        .default
                        .as_ref()
                        .map(|value| constant_bytes(program, value, param.ty))
                        .transpose()?;
                    let default_values = param
                        .default
                        .as_ref()
                        .map(|value| constant_values(program, value, param.ty))
                        .transpose()?;
                    params.push(DeclaredEventParam {
                        name: param.name.clone(),
                        elem_ty,
                        array_len: len,
                        is_slice: false,
                        byte_offset: minimum_wire_offset,
                        default_bytes,
                        default_values,
                    });
                    minimum_wire_offset =
                        checked_add(minimum_wire_offset, bytes, "event parameter wire offset")?;
                    if let Some(size) = computed_fixed_size.as_mut() {
                        *size = checked_add(*size, bytes, "event payload size")?;
                    }
                }
                Type::Slice { element, .. } => {
                    if param.default.is_some() {
                        return Err(MirMetadataError::new(format!(
                            "MIR event '{}' slice parameter '{}' unexpectedly has a default",
                            event.name, param.name
                        )));
                    }
                    params.push(DeclaredEventParam {
                        name: param.name.clone(),
                        elem_ty: primitive_type(*element),
                        array_len: 0,
                        is_slice: true,
                        byte_offset: minimum_wire_offset,
                        default_bytes: None,
                        default_values: None,
                    });
                    // The dynamic native wire format stores an i32 length before
                    // the element bytes. Offsets after a slice are minimum
                    // offsets (the offsets when preceding slice lengths are zero).
                    minimum_wire_offset = checked_add(
                        minimum_wire_offset,
                        std::mem::size_of::<i32>(),
                        "event slice length-prefix offset",
                    )?;
                    computed_fixed_size = None;
                }
                other => {
                    return Err(MirMetadataError::new(format!(
                        "MIR event '{}' parameter '{}' has unsupported runtime type {other:?}",
                        event.name, param.name
                    )));
                }
            }
        }

        if fixed_sizes[event_index] != computed_fixed_size {
            return Err(MirMetadataError::new(format!(
                "MIR event '{}' layout reports {:?} fixed bytes; descriptor shape requires {:?}",
                event.name, fixed_sizes[event_index], computed_fixed_size
            )));
        }
        events.push(DeclaredEvent {
            name: event.name.clone(),
            params,
            payload_bytes: fixed_sizes[event_index],
        });
    }
    Ok(events)
}

fn build_buffers(program: &Program) -> Result<Vec<DeclaredBuffer>, MirMetadataError> {
    let written = infer_written_interface_buffers(program)?;
    program
        .interface
        .buffers
        .iter()
        .enumerate()
        .map(|(index, buffer)| {
            let may_write = written.contains(&index);
            if may_write && buffer.access != AccessMode::ReadWrite {
                return Err(MirMetadataError::new(format!(
                    "MIR writes read-only interface buffer '{}'",
                    buffer.name
                )));
            }
            let channels = match buffer.channels {
                BufferChannels::Mono => DeclaredBufferChannels::Mono,
                BufferChannels::Static(channels) => {
                    DeclaredBufferChannels::Static(usize::try_from(channels).map_err(|_| {
                        MirMetadataError::new("MIR buffer channel count does not fit usize")
                    })?)
                }
                BufferChannels::Dynamic => DeclaredBufferChannels::Dynamic,
            };
            Ok(DeclaredBuffer {
                name: buffer.name.clone(),
                elem_ty: primitive_type(buffer.element),
                channels,
                may_write,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
enum ResourceOrigin {
    Buffer(usize),
    Parameter(usize),
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
struct FunctionWriteSummary {
    buffers: HashSet<usize>,
    parameters: HashSet<usize>,
}

fn infer_written_interface_buffers(program: &Program) -> Result<HashSet<usize>, MirMetadataError> {
    let mut summaries = vec![FunctionWriteSummary::default(); program.functions.len()];
    loop {
        let mut changed = false;
        let previous = summaries.clone();
        for (function_index, function) in program.functions.iter().enumerate() {
            let aliases = infer_local_resource_aliases(program, function);
            let unsupported_results = unsupported_resource_call_results(program, function);
            let mut next = FunctionWriteSummary::default();
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

    let mut written = HashSet::new();
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
    for root in roots {
        let summary = summaries.get(root).ok_or_else(|| {
            MirMetadataError::new(format!("MIR metadata root function {root} is missing"))
        })?;
        written.extend(summary.buffers.iter().copied());
    }
    Ok(written)
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
                let origins = rvalue_resource_origins(value, previous);
                aliases[local.index()].extend(origins);
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
                collect_block_aliases(program, function, body, previous, aliases);
            }
            _ => {}
        }
    }
}

fn rvalue_resource_origins(
    value: &Rvalue,
    aliases: &[HashSet<ResourceOrigin>],
) -> HashSet<ResourceOrigin> {
    match value {
        Rvalue::Use(value) => value_resource_origins(*value, aliases),
        Rvalue::Load(place) => place_resource_origins(place, aliases),
        Rvalue::MakeSlice { source, .. } => match source {
            SliceSource::Buffer { buffer, .. } => {
                HashSet::from([ResourceOrigin::Buffer(buffer.index())])
            }
            SliceSource::BufferParam { parameter, .. } => {
                HashSet::from([ResourceOrigin::Parameter(parameter.index())])
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
            HashSet::from([ResourceOrigin::Parameter(parameter.index())])
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

#[allow(clippy::too_many_arguments)]
fn collect_block_resource_writes(
    program: &Program,
    function: &Function,
    block: &Block,
    aliases: &[HashSet<ResourceOrigin>],
    unsupported_results: &HashSet<usize>,
    summaries: &[FunctionWriteSummary],
    output: &mut FunctionWriteSummary,
) -> Result<(), MirMetadataError> {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, .. } => {
                if let PlaceBase::Parameter(parameter) = destination.base {
                    output.parameters.insert(parameter.index());
                }
            }
            StatementKind::BufferStore { buffer, .. } => {
                output.buffers.insert(buffer.index());
            }
            StatementKind::BufferParamStore { parameter, .. } => {
                output.parameters.insert(parameter.index());
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
                    MirMetadataError::new(format!(
                        "MIR call references missing function {}",
                        callee.raw()
                    ))
                })?;
                output
                    .buffers
                    .extend(callee_summary.buffers.iter().copied());
                for parameter in &callee_summary.parameters {
                    let argument = args.get(*parameter).ok_or_else(|| {
                        MirMetadataError::new(format!(
                            "MIR call to function {} has no argument for writable parameter {parameter}",
                            callee.raw()
                        ))
                    })?;
                    let origins = call_argument_resource_origins(argument, aliases);
                    if call_argument_uses_unsupported_result(argument, unsupported_results) {
                        return Err(MirMetadataError::new(
                            "cannot infer interface-buffer writes through a slice or buffer returned by a MIR call",
                        ));
                    }
                    mark_resource_origins(origins, output);
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
    output: &mut FunctionWriteSummary,
) -> Result<(), MirMetadataError> {
    if let Value::Local(local) = value {
        if unsupported_results.contains(&local.index()) {
            return Err(MirMetadataError::new(format!(
                "cannot infer interface-buffer writes for {context} through a slice returned by a MIR call"
            )));
        }
    }
    mark_resource_origins(value_resource_origins(value, aliases), output);
    Ok(())
}

fn call_argument_resource_origins(
    argument: &CallArgument,
    aliases: &[HashSet<ResourceOrigin>],
) -> HashSet<ResourceOrigin> {
    match argument {
        CallArgument::Buffer(buffer) => HashSet::from([ResourceOrigin::Buffer(buffer.index())]),
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
        CallArgument::Place(_) | CallArgument::ArrayWindow { .. } | CallArgument::Buffer(_) => None,
    };
    matches!(value, Some(Value::Local(local)) if unsupported_results.contains(&local.index()))
}

fn mark_resource_origins(origins: HashSet<ResourceOrigin>, output: &mut FunctionWriteSummary) {
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

fn scalar_array_shape(
    program: &Program,
    id: onda_mir::TypeId,
    context: &str,
) -> Result<ScalarArrayShape, MirMetadataError> {
    match program.types.get(id.index()) {
        Some(Type::Scalar(scalar)) => Ok(ScalarArrayShape {
            element: primitive_type(*scalar),
            len: 1,
            is_array: false,
        }),
        Some(Type::Array { element, len }) => {
            let Some(Type::Scalar(scalar)) = program.types.get(element.index()) else {
                return Err(MirMetadataError::new(format!(
                    "MIR {context} type {} is not a one-dimensional scalar array",
                    id.raw()
                )));
            };
            let len = usize::try_from(*len)
                .map_err(|_| MirMetadataError::new("MIR array length does not fit usize"))?;
            Ok(ScalarArrayShape {
                element: primitive_type(*scalar),
                len,
                is_array: true,
            })
        }
        Some(other) => Err(MirMetadataError::new(format!(
            "MIR {context} type {} has unsupported descriptor shape {other:?}",
            id.raw()
        ))),
        None => Err(MirMetadataError::new(format!(
            "MIR {context} references missing type {}",
            id.raw()
        ))),
    }
}

fn primitive_type(ty: ScalarType) -> PrimitiveType {
    match ty {
        ScalarType::F32 => PrimitiveType::F32,
        ScalarType::F64 => PrimitiveType::F64,
        ScalarType::I32 => PrimitiveType::I32,
        ScalarType::I64 => PrimitiveType::I64,
        ScalarType::Bool => PrimitiveType::Bool,
    }
}

fn scalar_constant_value(value: &ConstantValue) -> Result<TypedConstValue, MirMetadataError> {
    let ConstantValue::Scalar(value) = value else {
        return Err(MirMetadataError::new(
            "MIR scalar descriptor has an aggregate constant",
        ));
    };
    Ok(typed_scalar(*value))
}

fn typed_scalar(value: ScalarValue) -> TypedConstValue {
    match value {
        ScalarValue::F32(value) => TypedConstValue::F32(value),
        ScalarValue::F64(value) => TypedConstValue::F64(value),
        ScalarValue::I32(value) => TypedConstValue::I32(value),
        ScalarValue::I64(value) => TypedConstValue::I64(value),
        ScalarValue::Bool(value) => TypedConstValue::Bool(value),
    }
}

fn typed_range(
    range: ValueRange,
    expected: PrimitiveType,
) -> Result<TypedValueRange, MirMetadataError> {
    let min = typed_scalar(range.min);
    let max = typed_scalar(range.max);
    if primitive_type(range.min.ty()) != expected || primitive_type(range.max.ty()) != expected {
        return Err(MirMetadataError::new(
            "MIR scalar range does not match its descriptor element type",
        ));
    }
    Ok(TypedValueRange { min, max })
}

fn constant_bytes(
    program: &Program,
    value: &ConstantValue,
    ty: onda_mir::TypeId,
) -> Result<Vec<u8>, MirMetadataError> {
    let mut bytes = Vec::new();
    append_constant_bytes(program, value, ty, &mut bytes)?;
    Ok(bytes)
}

fn constant_values(
    program: &Program,
    value: &ConstantValue,
    ty: onda_mir::TypeId,
) -> Result<Vec<TypedConstValue>, MirMetadataError> {
    let mut values = Vec::new();
    append_constant_values(program, value, ty, &mut values)?;
    Ok(values)
}

fn append_constant_values(
    program: &Program,
    value: &ConstantValue,
    ty: onda_mir::TypeId,
    output: &mut Vec<TypedConstValue>,
) -> Result<(), MirMetadataError> {
    match (program.types.get(ty.index()), value) {
        (Some(Type::Scalar(expected)), ConstantValue::Scalar(value)) if *expected == value.ty() => {
            output.push(typed_scalar(*value));
            Ok(())
        }
        (Some(Type::Array { element, len }), ConstantValue::Aggregate(values))
            if values.len() == *len as usize =>
        {
            for value in values {
                append_constant_values(program, value, *element, output)?;
            }
            Ok(())
        }
        (Some(expected), _) => Err(MirMetadataError::new(format!(
            "MIR constant does not match descriptor type {expected:?}"
        ))),
        (None, _) => Err(MirMetadataError::new(format!(
            "MIR constant references missing type {}",
            ty.raw()
        ))),
    }
}

fn append_constant_bytes(
    program: &Program,
    value: &ConstantValue,
    ty: onda_mir::TypeId,
    output: &mut Vec<u8>,
) -> Result<(), MirMetadataError> {
    match (program.types.get(ty.index()), value) {
        (Some(Type::Scalar(expected)), ConstantValue::Scalar(value)) if *expected == value.ty() => {
            append_typed_const_bytes(output, typed_scalar(*value), primitive_type(*expected));
            Ok(())
        }
        (Some(Type::Array { element, len }), ConstantValue::Aggregate(values))
            if values.len() == *len as usize =>
        {
            for value in values {
                append_constant_bytes(program, value, *element, output)?;
            }
            Ok(())
        }
        (Some(expected), _) => Err(MirMetadataError::new(format!(
            "MIR constant does not match descriptor type {expected:?}"
        ))),
        (None, _) => Err(MirMetadataError::new(format!(
            "MIR constant references missing type {}",
            ty.raw()
        ))),
    }
}

fn build_io_name_index(entries: &[DeclaredIo]) -> HashMap<String, usize> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.name().to_owned(), index))
        .collect()
}

fn checked_add(lhs: usize, rhs: usize, context: &str) -> Result<usize, MirMetadataError> {
    lhs.checked_add(rhs)
        .ok_or_else(|| MirMetadataError::new(format!("MIR {context} overflow")))
}

#[cfg(test)]
mod tests {
    use super::*;

    use onda_mir::{
        process_function_params, Buffer, BufferId, CompileConfig, EntryPoints, Event, EventParam,
        FunctionId, FunctionKind, FunctionParam, Input, Interface, Local, LocalId, Output, Param,
        ParameterId, PassingMode, SourceSpan, StateSlot, Statement,
    };

    fn empty_function(name: &str, kind: FunctionKind, params: Vec<FunctionParam>) -> Function {
        Function {
            name: name.to_owned(),
            kind,
            attributes: onda_mir::FunctionAttributes::default(),
            params,
            results: Vec::new(),
            locals: Vec::new(),
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        }
    }

    fn statement(kind: StatementKind) -> Statement {
        Statement {
            kind,
            source: SourceSpan::UNKNOWN,
        }
    }

    fn descriptor_program() -> Program {
        let types = vec![
            Type::Scalar(ScalarType::F32),  // 0
            Type::Scalar(ScalarType::F64),  // 1
            Type::Scalar(ScalarType::I32),  // 2
            Type::Scalar(ScalarType::I64),  // 3
            Type::Scalar(ScalarType::Bool), // 4
            Type::Array {
                // 5
                element: onda_mir::TypeId::new(1),
                len: 2,
            },
            Type::Array {
                // 6
                element: onda_mir::TypeId::new(0),
                len: 2,
            },
            Type::Array {
                // 7
                element: onda_mir::TypeId::new(4),
                len: 4,
            },
            Type::Slice {
                // 8
                element: ScalarType::F64,
                access: AccessMode::ReadOnly,
            },
        ];
        let init = empty_function("init", FunctionKind::Init, Vec::new());
        let process = empty_function(
            "process",
            FunctionKind::Process,
            process_function_params(onda_mir::TypeId::new(2)),
        );
        let fixed_event = empty_function(
            "note",
            FunctionKind::Event(onda_mir::EventId::new(0)),
            Vec::new(),
        );
        let dynamic_event = empty_function(
            "curve",
            FunctionKind::Event(onda_mir::EventId::new(1)),
            Vec::new(),
        );
        Program {
            schema_version: onda_mir::MIR_SCHEMA_VERSION,
            config: CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            source_files: Vec::new(),
            types,
            structs: Vec::new(),
            interface: Interface {
                inputs: vec![
                    Input {
                        name: "gain".to_owned(),
                        ty: onda_mir::TypeId::new(0),
                        default: Some(ConstantValue::Scalar(ScalarValue::F32(0.25))),
                        range: Some(ValueRange {
                            min: ScalarValue::F32(0.0),
                            max: ScalarValue::F32(1.0),
                        }),
                    },
                    Input {
                        name: "stereo".to_owned(),
                        ty: onda_mir::TypeId::new(5),
                        default: Some(ConstantValue::Aggregate(vec![
                            ConstantValue::Scalar(ScalarValue::F64(0.5)),
                            ConstantValue::Scalar(ScalarValue::F64(0.75)),
                        ])),
                        range: None,
                    },
                ],
                outputs: vec![
                    Output {
                        name: "pair".to_owned(),
                        ty: onda_mir::TypeId::new(6),
                    },
                    Output {
                        name: "counter".to_owned(),
                        ty: onda_mir::TypeId::new(3),
                    },
                ],
                control_outputs: vec![onda_mir::ControlOutput {
                    name: "meter".to_owned(),
                    ty: onda_mir::TypeId::new(0),
                    mirror: onda_mir::StateId::new(1),
                }],
                params: vec![
                    Param {
                        name: "mode".to_owned(),
                        ty: onda_mir::TypeId::new(2),
                        default: ConstantValue::Scalar(ScalarValue::I32(2)),
                        range: Some(ValueRange {
                            min: ScalarValue::I32(0),
                            max: ScalarValue::I32(4),
                        }),
                    },
                    Param {
                        name: "mix".to_owned(),
                        ty: onda_mir::TypeId::new(6),
                        default: ConstantValue::Aggregate(vec![
                            ConstantValue::Scalar(ScalarValue::F32(0.2)),
                            ConstantValue::Scalar(ScalarValue::F32(0.8)),
                        ]),
                        range: None,
                    },
                ],
                buffers: vec![
                    Buffer {
                        name: "mono".to_owned(),
                        element: ScalarType::F32,
                        channels: BufferChannels::Mono,
                        access: AccessMode::ReadWrite,
                    },
                    Buffer {
                        name: "bus".to_owned(),
                        element: ScalarType::F64,
                        channels: BufferChannels::Static(2),
                        access: AccessMode::ReadWrite,
                    },
                    Buffer {
                        name: "samples".to_owned(),
                        element: ScalarType::F32,
                        channels: BufferChannels::Dynamic,
                        access: AccessMode::ReadOnly,
                    },
                ],
                events: vec![
                    Event {
                        name: "note".to_owned(),
                        params: vec![
                            EventParam {
                                name: "key".to_owned(),
                                ty: onda_mir::TypeId::new(2),
                                default: Some(ConstantValue::Scalar(ScalarValue::I32(60))),
                            },
                            EventParam {
                                name: "levels".to_owned(),
                                ty: onda_mir::TypeId::new(6),
                                default: Some(ConstantValue::Aggregate(vec![
                                    ConstantValue::Scalar(ScalarValue::F32(0.25)),
                                    ConstantValue::Scalar(ScalarValue::F32(0.5)),
                                ])),
                            },
                        ],
                        handler: FunctionId::new(2),
                    },
                    Event {
                        name: "curve".to_owned(),
                        params: vec![
                            EventParam {
                                name: "enabled".to_owned(),
                                ty: onda_mir::TypeId::new(4),
                                default: Some(ConstantValue::Scalar(ScalarValue::Bool(true))),
                            },
                            EventParam {
                                name: "values".to_owned(),
                                ty: onda_mir::TypeId::new(8),
                                default: None,
                            },
                            EventParam {
                                name: "stamp".to_owned(),
                                ty: onda_mir::TypeId::new(3),
                                default: None,
                            },
                        ],
                        handler: FunctionId::new(3),
                    },
                ],
            },
            state: vec![
                StateSlot {
                    name: "phase".to_owned(),
                    ty: onda_mir::TypeId::new(1),
                    persistence: StatePersistence::Snapshot,
                },
                StateSlot {
                    name: "meter".to_owned(),
                    ty: onda_mir::TypeId::new(0),
                    persistence: StatePersistence::ControlMirror,
                },
                StateSlot {
                    name: "$scratch".to_owned(),
                    ty: onda_mir::TypeId::new(7),
                    persistence: StatePersistence::InstanceScratch,
                },
                StateSlot {
                    name: "history".to_owned(),
                    ty: onda_mir::TypeId::new(6),
                    persistence: StatePersistence::Snapshot,
                },
            ],
            const_data: Vec::new(),
            functions: vec![init, process, fixed_event, dynamic_event],
            entry_points: EntryPoints {
                init: FunctionId::new(0),
                process: FunctionId::new(1),
            },
        }
    }

    #[test]
    fn builds_runtime_descriptors_from_mir_and_native_offsets() {
        let program = descriptor_program();
        onda_mir::validate(&program).expect("metadata fixture should be valid MIR");
        let metadata = build_mir_program_metadata(
            &program,
            MirMetadataLayoutView {
                state_offsets: &[0, 8, 12, 16],
                param_offsets: &[0, 4],
                control_output_offsets: &[8],
                input_bases: &[0, 1],
                output_bases: &[0, 2],
                event_fixed_sizes: &[Some(12), None],
            },
        )
        .expect("MIR metadata should build");

        assert_eq!(metadata.inputs.len(), 2);
        assert_eq!(metadata.inputs[0].name(), "gain");
        assert_eq!(metadata.inputs[0].slot_offset(), 0);
        assert_eq!(metadata.inputs[0].byte_offset(), 0);
        assert_eq!(
            metadata.inputs[0].default(),
            Some(TypedConstValue::F32(0.25))
        );
        assert_eq!(
            metadata.inputs[0].range(),
            Some(TypedValueRange {
                min: TypedConstValue::F32(0.0),
                max: TypedConstValue::F32(1.0),
            })
        );
        assert_eq!(metadata.inputs[1].type_repr(), "f64[2]");
        assert_eq!(metadata.inputs[1].slot_offset(), 1);
        assert_eq!(metadata.inputs[1].byte_offset(), 4);
        assert_eq!(metadata.inputs[1].default(), None);
        let stereo = metadata.inputs[1].default_bytes().unwrap();
        assert_eq!(f64::from_ne_bytes(stereo[0..8].try_into().unwrap()), 0.5);
        assert_eq!(f64::from_ne_bytes(stereo[8..16].try_into().unwrap()), 0.75);

        assert_eq!(metadata.outputs[0].type_repr(), "f32[2]");
        assert_eq!(metadata.outputs[1].slot_offset(), 2);
        assert_eq!(metadata.outputs[1].byte_offset(), 8);
        assert_eq!(metadata.control_outputs[0].state_byte_offset(), Some(8));

        assert_eq!(metadata.params[0].byte_offset(), 0);
        assert_eq!(metadata.params[1].byte_offset(), 4);
        assert_eq!(metadata.params[1].slot_offset(), 1);
        assert_eq!(metadata.params[1].default_bytes().unwrap().len(), 8);

        let state = metadata
            .state_entries
            .iter()
            .map(|entry| (entry.name(), entry.byte_offset()))
            .collect::<Vec<_>>();
        assert_eq!(state, vec![("phase", 0), ("history", 8)]);

        assert_eq!(metadata.buffers[0].channels(), DeclaredBufferChannels::Mono);
        assert_eq!(
            metadata.buffers[1].channels(),
            DeclaredBufferChannels::Static(2)
        );
        assert_eq!(
            metadata.buffers[2].channels(),
            DeclaredBufferChannels::Dynamic
        );
        assert!(metadata.buffers.iter().all(|buffer| !buffer.may_write()));

        let note = &metadata.events[0];
        assert_eq!(note.payload_bytes(), Some(12));
        assert_eq!(note.params()[0].byte_offset(), 0);
        assert_eq!(note.params()[1].byte_offset(), 4);
        assert_eq!(note.params()[1].default_bytes().unwrap().len(), 8);
        let curve = &metadata.events[1];
        assert_eq!(curve.payload_bytes(), None);
        assert_eq!(curve.params()[0].byte_offset(), 0);
        assert_eq!(curve.params()[1].byte_offset(), 1);
        assert!(curve.params()[1].is_slice());
        assert_eq!(curve.params()[2].byte_offset(), 5);

        assert_eq!(metadata.input_index["stereo"], 1);
        assert_eq!(metadata.output_index["counter"], 1);
        assert_eq!(metadata.control_output_index["meter"], 0);
        assert_eq!(metadata.param_index["mix"], 1);
        assert_eq!(metadata.event_index["curve"], 1);
        assert_eq!(metadata.buffer_index["samples"], 2);
    }

    fn buffer_write_program() -> Program {
        let i32_ty = onda_mir::TypeId::new(1);
        let buffer_ty = onda_mir::TypeId::new(2);
        let slice_ty = onda_mir::TypeId::new(3);

        let init = empty_function("init", FunctionKind::Init, Vec::new());
        let mut process = empty_function(
            "process",
            FunctionKind::Process,
            process_function_params(i32_ty),
        );
        process.locals.push(Local {
            name: Some("buffer_slice".to_owned()),
            ty: slice_ty,
        });
        process.body.statements = vec![
            statement(StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::MakeSlice {
                    source: SliceSource::Buffer {
                        buffer: BufferId::new(1),
                        channel: None,
                    },
                    start: Value::Constant(ScalarValue::I32(0)),
                    len: Value::Constant(ScalarValue::I32(1)),
                    bounds: onda_mir::BoundsMode::Clamp,
                    access: AccessMode::ReadWrite,
                },
            }),
            statement(StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::Buffer(BufferId::new(0))],
            }),
            statement(StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(4),
                args: vec![CallArgument::Value(Value::Local(LocalId::new(0)))],
            }),
        ];

        let mut forward = empty_function(
            "forward",
            FunctionKind::User,
            vec![FunctionParam {
                name: "buffer".to_owned(),
                ty: buffer_ty,
                mode: PassingMode::ReadWriteReference,
            }],
        );
        forward.body.statements.push(statement(StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(3),
            args: vec![CallArgument::Place(Place {
                base: PlaceBase::Parameter(ParameterId::new(0)),
                projections: Vec::new(),
            })],
        }));

        let mut write_buffer = empty_function(
            "write_buffer",
            FunctionKind::User,
            vec![FunctionParam {
                name: "buffer".to_owned(),
                ty: buffer_ty,
                mode: PassingMode::ReadWriteReference,
            }],
        );
        write_buffer
            .body
            .statements
            .push(statement(StatementKind::BufferParamStore {
                parameter: ParameterId::new(0),
                channel: None,
                index: Value::Constant(ScalarValue::I32(0)),
                value: Value::Constant(ScalarValue::F32(1.0)),
                bounds: onda_mir::BoundsMode::Clamp,
            }));

        let mut write_slice = empty_function(
            "write_slice",
            FunctionKind::User,
            vec![FunctionParam {
                name: "slice".to_owned(),
                ty: slice_ty,
                mode: PassingMode::Value,
            }],
        );
        write_slice.locals.push(Local {
            name: Some("slice.local".to_owned()),
            ty: slice_ty,
        });
        write_slice.body.statements = vec![
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
                bounds: onda_mir::BoundsMode::Clamp,
            }),
        ];

        Program {
            schema_version: onda_mir::MIR_SCHEMA_VERSION,
            config: CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            source_files: Vec::new(),
            types: vec![
                Type::Scalar(ScalarType::F32),
                Type::Scalar(ScalarType::I32),
                Type::Buffer {
                    element: ScalarType::F32,
                    channels: BufferChannels::Mono,
                    access: AccessMode::ReadWrite,
                },
                Type::Slice {
                    element: ScalarType::F32,
                    access: AccessMode::ReadWrite,
                },
            ],
            structs: Vec::new(),
            interface: Interface {
                buffers: vec![
                    Buffer {
                        name: "forwarded".to_owned(),
                        element: ScalarType::F32,
                        channels: BufferChannels::Mono,
                        access: AccessMode::ReadWrite,
                    },
                    Buffer {
                        name: "sliced".to_owned(),
                        element: ScalarType::F32,
                        channels: BufferChannels::Mono,
                        access: AccessMode::ReadWrite,
                    },
                    Buffer {
                        name: "untouched".to_owned(),
                        element: ScalarType::F32,
                        channels: BufferChannels::Mono,
                        access: AccessMode::ReadWrite,
                    },
                ],
                ..Interface::default()
            },
            state: Vec::new(),
            const_data: Vec::new(),
            functions: vec![init, process, forward, write_buffer, write_slice],
            entry_points: EntryPoints {
                init: FunctionId::new(0),
                process: FunctionId::new(1),
            },
        }
    }

    #[test]
    fn infers_reachable_buffer_writes_through_calls_and_slice_aliases() {
        let program = buffer_write_program();
        onda_mir::validate(&program).expect("buffer-write fixture should be valid MIR");
        let metadata = build_mir_program_metadata(
            &program,
            MirMetadataLayoutView {
                state_offsets: &[],
                param_offsets: &[],
                control_output_offsets: &[],
                input_bases: &[],
                output_bases: &[],
                event_fixed_sizes: &[],
            },
        )
        .expect("MIR metadata should build");

        assert!(metadata.buffers[0].may_write());
        assert!(metadata.buffers[1].may_write());
        assert!(!metadata.buffers[2].may_write());
    }

    #[test]
    fn rejects_layout_event_size_that_disagrees_with_mir_shape() {
        let program = descriptor_program();
        let result = build_mir_program_metadata(
            &program,
            MirMetadataLayoutView {
                state_offsets: &[0, 8, 12, 16],
                param_offsets: &[0, 4],
                control_output_offsets: &[8],
                input_bases: &[0, 1],
                output_bases: &[0, 2],
                event_fixed_sizes: &[Some(16), None],
            },
        );
        let error = match result {
            Ok(_) => panic!("event layout mismatch should fail"),
            Err(error) => error,
        };
        assert!(error.message.contains("requires Some(12)"));
    }
}
