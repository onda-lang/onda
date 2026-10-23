//! Consume MIR storage identities without asserting disjointness between
//! overlapping views of the same array or between unknown references.
use std::cell::RefCell;
use std::collections::HashMap;

use super::*;

pub(super) fn reference_alignment(
    program: &Program,
    memory: &onda_mir::MemoryAnalysis,
    layouts: &NativeLayouts,
    function: onda_mir::FunctionId,
    parameter: onda_mir::ParameterId,
) -> usize {
    let origins = memory.parameter(function, parameter);
    let Some(states) = origins.only_states() else {
        return 1;
    };
    if let Some(regions) = origins.state_regions() {
        return regions.fold(layouts.state.alignment, |alignment, region| {
            let ty = program.state[region.state.index()].ty;
            let element = match program.types[ty.index()] {
                Type::Array { element, .. } => element,
                _ => ty,
            };
            let offset = layouts.state.offsets[region.state.index()]
                + region.start as usize * layouts.type_sizes[element.index()];
            alignment.min(alignment_at_byte_offset(layouts.state.alignment, offset))
        });
    }
    let ty = program.functions[function.index()].params[parameter.index()].ty;
    let size = layouts.type_sizes[ty.index()];
    let mut alignment = layouts.state.alignment;
    for state in states {
        // A valid reference occupying its entire backing object must start at
        // that object's base. Smaller views may have an arbitrary byte offset.
        if layouts.type_sizes[program.state[state.index()].ty.index()] != size {
            return 1;
        }
        alignment = alignment.min(alignment_at_byte_offset(
            layouts.state.alignment,
            layouts.state.offsets[state.index()],
        ));
    }
    alignment
}

pub(super) struct StateAliasScopes {
    scopes: Vec<LLVMMetadataRef>,
    lists: RefCell<HashMap<Vec<onda_mir::StateId>, (LLVMValueRef, LLVMValueRef)>>,
    alias_kind: u32,
    noalias_kind: u32,
}

impl StateAliasScopes {
    pub(super) unsafe fn new(context: LLVMContextRef, program: &Program) -> Self {
        let name = "onda.state";
        let label = LLVMMDStringInContext2(context, name.as_ptr().cast(), name.len());
        let domain = self_referential_metadata_node(context, &[label]);
        let scopes = program
            .state
            .iter()
            .enumerate()
            .map(|(index, state)| {
                let name = format!("state.{index}.{}", state.name);
                let label = LLVMMDStringInContext2(context, name.as_ptr().cast(), name.len());
                self_referential_metadata_node(context, &[domain, label])
            })
            .collect();
        Self {
            scopes,
            lists: RefCell::new(HashMap::new()),
            alias_kind: LLVMGetMDKindIDInContext(context, c"alias.scope".as_ptr(), 11),
            noalias_kind: LLVMGetMDKindIDInContext(context, c"noalias".as_ptr(), 7),
        }
    }

    pub(super) unsafe fn mark(
        &self,
        context: LLVMContextRef,
        instruction: LLVMValueRef,
        origins: &onda_mir::StorageOrigins,
    ) {
        let Some(states) = origins.states() else {
            return;
        };
        let states: Vec<_> = states.collect();
        let mut lists = self.lists.borrow_mut();
        let (alias, noalias) = *lists.entry(states.clone()).or_insert_with(|| {
            let mut aliases = Vec::new();
            let mut others = Vec::new();
            for (index, &scope) in self.scopes.iter().enumerate() {
                if states
                    .binary_search(&onda_mir::StateId::new(index as u32))
                    .is_ok()
                {
                    aliases.push(scope);
                } else {
                    others.push(scope);
                }
            }
            let list = |nodes: &mut Vec<LLVMMetadataRef>| {
                if nodes.is_empty() {
                    null_mut()
                } else {
                    LLVMMetadataAsValue(
                        context,
                        LLVMMDNodeInContext2(context, nodes.as_mut_ptr(), nodes.len()),
                    )
                }
            };
            (list(&mut aliases), list(&mut others))
        });
        if !alias.is_null() {
            LLVMSetMetadata(instruction, self.alias_kind, alias);
        }
        if !noalias.is_null() {
            LLVMSetMetadata(instruction, self.noalias_kind, noalias);
        }
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) unsafe fn mark_place_access(&self, instruction: LLVMValueRef, place: &Place) {
        if matches!(
            place.base,
            onda_mir::PlaceBase::State(_) | onda_mir::PlaceBase::Parameter(_)
        ) {
            self.mark_parallel_access(instruction);
            let origins = self.module.memory.place(
                self.module.program,
                onda_mir::FunctionId::new(self.function_index as u32),
                place,
            );
            self.module
                .state_alias_scopes
                .mark(self.module.context, instruction, &origins);
        }
    }

    pub(super) unsafe fn mark_slice_access(&self, instruction: LLVMValueRef, slice: Value) {
        self.mark_parallel_access(instruction);
        if let Some(origins) = self
            .module
            .memory
            .slice(onda_mir::FunctionId::new(self.function_index as u32), slice)
        {
            self.module
                .state_alias_scopes
                .mark(self.module.context, instruction, origins);
        }
    }
}
