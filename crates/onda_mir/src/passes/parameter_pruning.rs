use crate::{
    Block, BufferParamRef, BufferSpanRef, CallArgument, FunctionKind, ParameterId, Place,
    PlaceBase, Rvalue, SliceSource, StatementKind,
};

/// Removes unreferenced parameters from internal functions and their call sites.
///
/// The transform runs to a fixed point because a parameter may be used only to
/// forward an argument to another parameter removed in an earlier round. A
/// parameter is retained when preparing its argument can fail at any call site,
/// even if the callee never accesses it.
pub(crate) fn prune(program: &mut crate::Program) -> u64 {
    let mut removed = 0_u64;
    loop {
        let fallible_arguments = fallible_call_parameters(program);
        let mappings = program
            .functions
            .iter()
            .zip(&fallible_arguments)
            .map(|(function, fallible_arguments)| parameter_mapping(function, fallible_arguments))
            .collect::<Vec<_>>();
        let round_removed = mappings
            .iter()
            .map(|mapping| mapping.iter().filter(|entry| entry.is_none()).count() as u64)
            .sum::<u64>();
        if round_removed == 0 {
            return removed;
        }

        for (function, mapping) in program.functions.iter_mut().zip(&mappings) {
            rewrite_block_parameters(&mut function.body, mapping);
            let mut old_index = 0usize;
            function.params.retain(|_| {
                let retain = mapping[old_index].is_some();
                old_index += 1;
                retain
            });
        }
        for function in &mut program.functions {
            rewrite_call_arguments(&mut function.body, &mappings);
        }
        removed = removed.saturating_add(round_removed);
    }
}

fn parameter_mapping(
    function: &crate::Function,
    fallible_arguments: &[bool],
) -> Vec<Option<ParameterId>> {
    if function.kind != FunctionKind::User {
        return (0..function.params.len())
            .map(|index| Some(ParameterId::new(index as u32)))
            .collect();
    }

    let mut used = fallible_arguments.to_vec();
    collect_block_parameters(&function.body, &mut used);
    let mut next = 0_u32;
    used.into_iter()
        .map(|used| {
            used.then(|| {
                let id = ParameterId::new(next);
                next += 1;
                id
            })
        })
        .collect()
}

fn fallible_call_parameters(program: &crate::Program) -> Vec<Vec<bool>> {
    let mut fallible = program
        .functions
        .iter()
        .map(|function| vec![false; function.params.len()])
        .collect::<Vec<_>>();
    for function in &program.functions {
        collect_fallible_call_parameters(&function.body, &mut fallible);
    }
    fallible
}

fn collect_fallible_call_parameters(block: &Block, fallible: &mut [Vec<bool>]) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Call { function, args, .. } => {
                let Some(parameters) = fallible.get_mut(function.index()) else {
                    continue;
                };
                for (parameter, argument) in parameters.iter_mut().zip(args) {
                    *parameter |= crate::analysis::call_argument_may_fail(argument);
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_fallible_call_parameters(then_block, fallible);
                collect_fallible_call_parameters(else_block, fallible);
            }
            StatementKind::Loop { body } => collect_fallible_call_parameters(body, fallible),
            _ => {}
        }
    }
}

fn mark_parameter(parameter: ParameterId, used: &mut [bool]) {
    if let Some(used) = used.get_mut(parameter.index()) {
        *used = true;
    } else {
        // Preserve invalid input for the validator instead of either masking
        // it through pruning or making this cleanup pass a panic boundary.
        used.fill(true);
    }
}

fn collect_place_parameters(place: &Place, used: &mut [bool]) {
    if let PlaceBase::Parameter(parameter) = place.base {
        mark_parameter(parameter, used);
    }
}

fn collect_buffer_parameter(reference: BufferParamRef, used: &mut [bool]) {
    mark_parameter(reference.first(), used);
}

fn collect_span_parameter(reference: BufferSpanRef, used: &mut [bool]) {
    if let BufferSpanRef::Parameter { span, .. } = reference {
        mark_parameter(span, used);
    }
}

fn collect_slice_source_parameters(source: &SliceSource, used: &mut [bool]) {
    match source {
        SliceSource::Place(place) => collect_place_parameters(place, used),
        SliceSource::BufferParam { parameter, .. } => {
            collect_buffer_parameter(*parameter, used);
        }
        SliceSource::Buffer { .. } | SliceSource::ConstData(_) => {}
    }
}

fn collect_rvalue_parameters(value: &Rvalue, used: &mut [bool]) {
    match value {
        Rvalue::Load(place) => collect_place_parameters(place, used),
        Rvalue::BufferParamLoad { parameter, .. }
        | Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter) => collect_buffer_parameter(*parameter, used),
        Rvalue::MakeSlice { source, .. } => collect_slice_source_parameters(source, used),
        Rvalue::InitAll
        | Rvalue::Use(_)
        | Rvalue::Unary { .. }
        | Rvalue::Binary { .. }
        | Rvalue::Compare { .. }
        | Rvalue::Cast { .. }
        | Rvalue::Intrinsic { .. }
        | Rvalue::ProcessFrame { .. }
        | Rvalue::InputLoad { .. }
        | Rvalue::OutputLoad { .. }
        | Rvalue::BufferLoad { .. }
        | Rvalue::BufferLen(_)
        | Rvalue::BufferChannels(_)
        | Rvalue::BufferSampleRate(_)
        | Rvalue::ConstDataLoad { .. }
        | Rvalue::SliceLoad { .. }
        | Rvalue::SliceLen(_) => {}
    }
}

fn collect_call_argument_parameters(argument: &CallArgument, used: &mut [bool]) {
    match argument {
        CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => {
            collect_place_parameters(place, used);
        }
        CallArgument::BufferParam(parameter) => collect_buffer_parameter(*parameter, used),
        CallArgument::BufferSpan(span) => collect_span_parameter(*span, used),
        CallArgument::Value(_)
        | CallArgument::SliceElement { .. }
        | CallArgument::SliceWindow { .. }
        | CallArgument::Buffer(_) => {}
    }
}

fn collect_block_parameters(block: &Block, used: &mut [bool]) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                collect_place_parameters(destination, used);
                collect_rvalue_parameters(value, used);
            }
            StatementKind::Call { args, .. } => {
                for argument in args {
                    collect_call_argument_parameters(argument, used);
                }
            }
            StatementKind::BufferParamStore { parameter, .. } => {
                collect_buffer_parameter(*parameter, used);
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_parameters(then_block, used);
                collect_block_parameters(else_block, used);
            }
            StatementKind::Loop { body } => collect_block_parameters(body, used),
            StatementKind::OutputStore { .. }
            | StatementKind::ControlOutputStore { .. }
            | StatementKind::BufferStore { .. }
            | StatementKind::SliceStore { .. }
            | StatementKind::SliceFill { .. }
            | StatementKind::SliceCopy { .. }
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Return { .. } => {}
        }
    }
}

fn mapped_parameter(parameter: &mut ParameterId, mapping: &[Option<ParameterId>]) {
    if let Some(Some(mapped)) = mapping.get(parameter.index()) {
        *parameter = *mapped;
    }
}

fn rewrite_place_parameter(place: &mut Place, mapping: &[Option<ParameterId>]) {
    if let PlaceBase::Parameter(parameter) = &mut place.base {
        mapped_parameter(parameter, mapping);
    }
}

fn rewrite_buffer_parameter(reference: &mut BufferParamRef, mapping: &[Option<ParameterId>]) {
    match reference {
        BufferParamRef::Direct(parameter)
        | BufferParamRef::ArrayElement {
            span: parameter, ..
        } => {
            mapped_parameter(parameter, mapping);
        }
    }
}

fn rewrite_span_parameter(reference: &mut BufferSpanRef, mapping: &[Option<ParameterId>]) {
    if let BufferSpanRef::Parameter { span, .. } = reference {
        mapped_parameter(span, mapping);
    }
}

fn rewrite_slice_source_parameters(source: &mut SliceSource, mapping: &[Option<ParameterId>]) {
    match source {
        SliceSource::Place(place) => rewrite_place_parameter(place, mapping),
        SliceSource::BufferParam { parameter, .. } => {
            rewrite_buffer_parameter(parameter, mapping);
        }
        SliceSource::Buffer { .. } | SliceSource::ConstData(_) => {}
    }
}

fn rewrite_rvalue_parameters(value: &mut Rvalue, mapping: &[Option<ParameterId>]) {
    match value {
        Rvalue::Load(place) => rewrite_place_parameter(place, mapping),
        Rvalue::BufferParamLoad { parameter, .. }
        | Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter) => rewrite_buffer_parameter(parameter, mapping),
        Rvalue::MakeSlice { source, .. } => rewrite_slice_source_parameters(source, mapping),
        Rvalue::InitAll
        | Rvalue::Use(_)
        | Rvalue::Unary { .. }
        | Rvalue::Binary { .. }
        | Rvalue::Compare { .. }
        | Rvalue::Cast { .. }
        | Rvalue::Intrinsic { .. }
        | Rvalue::ProcessFrame { .. }
        | Rvalue::InputLoad { .. }
        | Rvalue::OutputLoad { .. }
        | Rvalue::BufferLoad { .. }
        | Rvalue::BufferLen(_)
        | Rvalue::BufferChannels(_)
        | Rvalue::BufferSampleRate(_)
        | Rvalue::ConstDataLoad { .. }
        | Rvalue::SliceLoad { .. }
        | Rvalue::SliceLen(_) => {}
    }
}

fn rewrite_call_argument_parameters(argument: &mut CallArgument, mapping: &[Option<ParameterId>]) {
    match argument {
        CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => {
            rewrite_place_parameter(place, mapping);
        }
        CallArgument::BufferParam(parameter) => rewrite_buffer_parameter(parameter, mapping),
        CallArgument::BufferSpan(span) => rewrite_span_parameter(span, mapping),
        CallArgument::Value(_)
        | CallArgument::SliceElement { .. }
        | CallArgument::SliceWindow { .. }
        | CallArgument::Buffer(_) => {}
    }
}

fn rewrite_block_parameters(block: &mut Block, mapping: &[Option<ParameterId>]) {
    for statement in &mut block.statements {
        match &mut statement.kind {
            StatementKind::Assign { destination, value } => {
                rewrite_place_parameter(destination, mapping);
                rewrite_rvalue_parameters(value, mapping);
            }
            StatementKind::Call { args, .. } => {
                for argument in args {
                    rewrite_call_argument_parameters(argument, mapping);
                }
            }
            StatementKind::BufferParamStore { parameter, .. } => {
                rewrite_buffer_parameter(parameter, mapping);
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                rewrite_block_parameters(then_block, mapping);
                rewrite_block_parameters(else_block, mapping);
            }
            StatementKind::Loop { body } => rewrite_block_parameters(body, mapping),
            StatementKind::OutputStore { .. }
            | StatementKind::ControlOutputStore { .. }
            | StatementKind::BufferStore { .. }
            | StatementKind::SliceStore { .. }
            | StatementKind::SliceFill { .. }
            | StatementKind::SliceCopy { .. }
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Return { .. } => {}
        }
    }
}

fn rewrite_call_arguments(block: &mut Block, mappings: &[Vec<Option<ParameterId>>]) {
    for statement in &mut block.statements {
        match &mut statement.kind {
            StatementKind::Call { function, args, .. } => {
                let Some(mapping) = mappings.get(function.index()) else {
                    continue;
                };
                let mut old_index = 0usize;
                args.retain(|_| {
                    let retain = mapping.get(old_index).is_none_or(Option::is_some);
                    old_index += 1;
                    retain
                });
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                rewrite_call_arguments(then_block, mappings);
                rewrite_call_arguments(else_block, mappings);
            }
            StatementKind::Loop { body } => rewrite_call_arguments(body, mappings),
            _ => {}
        }
    }
}
