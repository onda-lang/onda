//! Traversal and reindexing of local references in executable MIR.
use crate::{
    Block, BufferParamRef, BufferRef, BufferSpanRef, CallArgument, LocalId, Place, PlaceBase,
    Projection, Rvalue, SliceCopy, SliceSource, Statement, StatementKind, Value,
};
use std::collections::HashSet;

/// All local storage mentioned by a block, including writes and addresses.
pub fn referenced_locals(block: &Block) -> HashSet<LocalId> {
    let mut locals = HashSet::new();
    read::walk_statements::<true>(&block.statements, &mut |local, _| {
        locals.insert(*local);
    });
    locals
}

/// Adds local storage read or written directly by one statement, excluding nested blocks.
pub fn collect_direct_local_references(statement: &Statement, referenced: &mut HashSet<LocalId>) {
    read::walk_statement::<true>(statement, &mut |local, _| {
        referenced.insert(*local);
    });
}

/// Adds operand and address reads, excluding destination bases, call results and nested blocks.
pub(crate) fn collect_direct_local_reads(statement: &Statement, referenced: &mut HashSet<LocalId>) {
    read::walk_statement::<true>(statement, &mut |local, access| {
        if matches!(access, Access::Read) {
            referenced.insert(*local);
        }
    });
}

/// Adds local assignment destinations and call results throughout the statements.
pub(crate) fn collect_local_writes(statements: &[Statement], referenced: &mut HashSet<LocalId>) {
    read::walk_statements::<false>(statements, &mut |local, _| {
        referenced.insert(*local);
    });
}

/// Reindex local references using a caller-provided mapping. Every referenced
/// local must have a destination in the mapping.
pub fn rewrite_block_locals(block: &mut Block, mapping: &[Option<LocalId>]) {
    write::walk_statements::<true>(&mut block.statements, &mut |local, _| {
        *local = mapping[local.index()].expect("referenced local retained during reindexing");
    });
}

enum Access {
    Read,
    Write,
}

// Share the exhaustive IR traversal between immutable reads and mutable
// reindexing. The two expansions differ only in how they borrow the IR.
macro_rules! define_walk {
    ($($borrow:tt)?) => {
        type LocalRef<'a> = &'a $($borrow)? LocalId;

        pub(super) fn walk_statements<const READS: bool>(
            statements: &$($borrow)? [Statement],
            visit: &mut impl FnMut(LocalRef<'_>, Access),
        ) {
            for statement in statements {
                walk_statement::<READS>(statement, visit);
                match &$($borrow)? statement.kind {
                    StatementKind::If { then_block, else_block, .. } => {
                        walk_statements::<READS>(&$($borrow)? then_block.statements, visit);
                        walk_statements::<READS>(&$($borrow)? else_block.statements, visit);
                    }
                    StatementKind::Loop { body } => {
                        walk_statements::<READS>(&$($borrow)? body.statements, visit);
                    }
                    _ => {}
                }
            }
        }

        pub(super) fn walk_statement<const READS: bool>(
            statement: &$($borrow)? Statement,
            visit: &mut impl FnMut(LocalRef<'_>, Access),
        ) {
            match &$($borrow)? statement.kind {
                StatementKind::Assign { destination, value } => {
                    place::<READS>(destination, Access::Write, visit);
                    if READS {
                        rvalue(value, visit);
                    }
                }
                StatementKind::Call { results, args, .. } => {
                    for result in results {
                        visit(result, Access::Write);
                    }
                    if READS {
                        for arg in args {
                            argument(arg, visit);
                        }
                    }
                }
                _ if !READS => {}
                StatementKind::PublishDelegate { args, .. } => {
                    for arg in args {
                        argument(arg, visit);
                    }
                }
                StatementKind::PublishLog { arguments: values, .. }
                | StatementKind::Return { values } => {
                    for item in values {
                        value(item, visit);
                    }
                }
                StatementKind::OutputStore { element, frame, value: stored, .. } => {
                    optional_value(element, visit);
                    value(frame, visit);
                    value(stored, visit);
                }
                StatementKind::ControlOutputStore { element, value: stored, .. } => {
                    optional_value(element, visit);
                    value(stored, visit);
                }
                StatementKind::BufferStore { buffer, channel, index, value: stored, .. } => {
                    buffer_ref(buffer, visit);
                    optional_value(channel, visit);
                    value(index, visit);
                    value(stored, visit);
                }
                StatementKind::BufferParamStore { parameter, channel, index, value: stored, .. } => {
                    buffer_param_ref(parameter, visit);
                    optional_value(channel, visit);
                    value(index, visit);
                    value(stored, visit);
                }
                StatementKind::SliceStore { slice, index, value: stored, .. } => {
                    value(slice, visit);
                    value(index, visit);
                    value(stored, visit);
                }
                StatementKind::SliceFill { destination, value: stored } => {
                    value(destination, visit);
                    value(stored, visit);
                }
                StatementKind::SliceCopy { copies, .. } => {
                    for SliceCopy { destination, source } in copies {
                        value(destination, visit);
                        value(source, visit);
                    }
                }
                StatementKind::If { condition, .. } => value(condition, visit),
                StatementKind::Loop { .. } | StatementKind::Break | StatementKind::Continue => {}
            }
        }

        fn rvalue(expr: &$($borrow)? Rvalue, visit: &mut impl FnMut(LocalRef<'_>, Access)) {
            match expr {
                Rvalue::MakeBuffer { source, .. } => argument(source, visit),
                Rvalue::InitAll => {}
                Rvalue::Use(operand) | Rvalue::SliceLen(operand) | Rvalue::Unary { operand, .. }
                | Rvalue::Cast { value: operand, .. } | Rvalue::ProcessFrame { offset: operand }
                | Rvalue::ConstDataLoad { index: operand, .. } => value(operand, visit),
                Rvalue::Load(source) => place::<true>(source, Access::Read, visit),
                Rvalue::Binary { lhs, rhs, .. } | Rvalue::Compare { lhs, rhs, .. } => {
                    value(lhs, visit);
                    value(rhs, visit);
                }
                Rvalue::Intrinsic { args, .. } => {
                    for item in args {
                        value(item, visit);
                    }
                }
                Rvalue::NormalizeIndex { index, length, .. } => {
                    value(index, visit);
                    value(length, visit);
                }
                Rvalue::InputLoad { element, frame, .. } | Rvalue::OutputLoad { element, frame, .. } => {
                    optional_value(element, visit);
                    value(frame, visit);
                }
                Rvalue::BufferLoad { buffer, channel, index, .. } => {
                    buffer_ref(buffer, visit);
                    optional_value(channel, visit);
                    value(index, visit);
                }
                Rvalue::BufferParamLoad { parameter, channel, index, .. } => {
                    buffer_param_ref(parameter, visit);
                    optional_value(channel, visit);
                    value(index, visit);
                }
                Rvalue::MakeSlice { source, start, len, .. } => {
                    match source {
                        SliceSource::Place(source) => place::<true>(source, Access::Read, visit),
                        SliceSource::Buffer { buffer, channel } => {
                            buffer_ref(buffer, visit);
                            optional_value(channel, visit);
                        }
                        SliceSource::BufferParam { parameter, channel } => {
                            buffer_param_ref(parameter, visit);
                            optional_value(channel, visit);
                        }
                        SliceSource::ConstData(_) => {}
                    }
                    value(start, visit);
                    value(len, visit);
                }
                Rvalue::SliceLoad { slice, index, .. } => {
                    value(slice, visit);
                    value(index, visit);
                }
                Rvalue::BufferSamples(buffer) | Rvalue::BufferChannels(buffer)
                | Rvalue::BufferSampleRate(buffer) | Rvalue::BufferIsBound(buffer) => {
                    buffer_ref(buffer, visit);
                }
                Rvalue::BufferParamSamples(parameter) | Rvalue::BufferParamChannels(parameter)
                | Rvalue::BufferParamSampleRate(parameter) | Rvalue::BufferParamIsBound(parameter) => {
                    buffer_param_ref(parameter, visit);
                }
            }
        }

        fn argument(arg: &$($borrow)? CallArgument, visit: &mut impl FnMut(LocalRef<'_>, Access)) {
            match arg {
                CallArgument::Value(operand) => value(operand, visit),
                CallArgument::Place(source) => place::<true>(source, Access::Read, visit),
                CallArgument::ArrayWindow { array, start, .. } => {
                    place::<true>(array, Access::Read, visit);
                    value(start, visit);
                }
                CallArgument::SliceElement { slice, index, .. }
                | CallArgument::SliceWindow { slice, start: index, .. } => {
                    value(slice, visit);
                    value(index, visit);
                }
                CallArgument::Buffer(buffer) => buffer_ref(buffer, visit),
                CallArgument::BufferParam(parameter) => buffer_param_ref(parameter, visit),
                CallArgument::BufferSpan(BufferSpanRef::Local { collection, start, .. }) => {
                    visit(collection, Access::Read);
                    value(start, visit);
                }
                CallArgument::BufferSpan(_) => {}
            }
        }

        fn place<const READS: bool>(
            place: &$($borrow)? Place,
            access: Access,
            visit: &mut impl FnMut(LocalRef<'_>, Access),
        ) {
            if let PlaceBase::Local(local) = &$($borrow)? place.base {
                visit(local, access);
            }
            if READS {
                for projection in &$($borrow)? place.projections {
                    if let Projection::Index { index, .. } = projection {
                        value(index, visit);
                    }
                }
            }
        }

        fn buffer_ref(buffer: &$($borrow)? BufferRef, visit: &mut impl FnMut(LocalRef<'_>, Access)) {
            if let BufferRef::ArrayElement { selector, .. } = buffer {
                value(selector, visit);
            }
        }

        fn buffer_param_ref(
            buffer: &$($borrow)? BufferParamRef,
            visit: &mut impl FnMut(LocalRef<'_>, Access),
        ) {
            match buffer {
                BufferParamRef::Local(local) => visit(local, Access::Read),
                BufferParamRef::LocalElement { collection, selector, .. } => {
                    visit(collection, Access::Read);
                    value(selector, visit);
                }
                BufferParamRef::ArrayElement { selector, .. } => value(selector, visit),
                BufferParamRef::Direct(_) => {}
            }
        }

        fn optional_value(
            operand: &$($borrow)? Option<Value>,
            visit: &mut impl FnMut(LocalRef<'_>, Access),
        ) {
            if let Some(operand) = operand {
                value(operand, visit);
            }
        }

        fn value(operand: &$($borrow)? Value, visit: &mut impl FnMut(LocalRef<'_>, Access)) {
            if let Value::Local(local) = operand {
                visit(local, Access::Read);
            }
        }
    };
}

mod read {
    use super::*;
    define_walk!();
}

mod write {
    use super::*;
    define_walk!(mut);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoundsMode, ParameterId, SourceSpan};

    #[test]
    fn buffer_parameter_metadata_tracks_and_reindexes_its_selector() {
        let destination = LocalId::new(0);
        let selector = LocalId::new(2);
        let parameter = BufferParamRef::ArrayElement {
            span: ParameterId::new(0),
            selector: Value::Local(selector),
            bounds: BoundsMode::Clamp,
        };
        for value in [
            Rvalue::BufferParamSamples(parameter),
            Rvalue::BufferParamChannels(parameter),
            Rvalue::BufferParamSampleRate(parameter),
            Rvalue::BufferParamIsBound(parameter),
        ] {
            let mut block = Block {
                statements: vec![Statement {
                    kind: StatementKind::Assign {
                        destination: Place::local(destination),
                        value,
                    },
                    source: SourceSpan::UNKNOWN,
                }],
            };
            let mut reads = HashSet::new();
            collect_direct_local_reads(&block.statements[0], &mut reads);
            assert_eq!(reads, HashSet::from([selector]));
            assert_eq!(
                referenced_locals(&block),
                HashSet::from([destination, selector])
            );

            let new_destination = LocalId::new(1);
            let new_selector = LocalId::new(0);
            rewrite_block_locals(
                &mut block,
                &[Some(new_destination), None, Some(new_selector)],
            );
            reads.clear();
            collect_direct_local_reads(&block.statements[0], &mut reads);
            assert_eq!(reads, HashSet::from([new_selector]));
            assert_eq!(
                referenced_locals(&block),
                HashSet::from([new_destination, new_selector])
            );
        }
    }
}
