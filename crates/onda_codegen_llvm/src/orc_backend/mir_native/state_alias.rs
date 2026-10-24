//! Consume MIR storage identities without asserting disjointness between
//! overlapping views of the same array or between unknown references.
use std::collections::{BTreeSet, HashMap};

use onda_mir::PlaceBase;

use super::*;

mod scopes;

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
    lists: HashMap<Origins, (LLVMValueRef, LLVMValueRef)>,
    alias_kind: u32,
    noalias_kind: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Origins {
    states: Vec<onda_mir::StateId>,
    only_states: bool,
}

impl Origins {
    fn new(origins: &onda_mir::StorageOrigins) -> Option<Self> {
        Some(Self {
            states: origins.states()?.collect(),
            only_states: origins.only_states().is_some(),
        })
    }
}

fn accessed_origins(program: &onda_mir::OptimizedProgram) -> Vec<Origins> {
    let mut origins = BTreeSet::new();
    for (index, function) in program.functions.iter().enumerate() {
        let id = onda_mir::FunctionId::new(index as u32);
        let place = |place: &Place| {
            matches!(place.base, PlaceBase::State(_) | PlaceBase::Parameter(_))
                .then(|| Origins::new(&program.memory().place(program, id, place)))
                .flatten()
        };
        let slice = |value| Origins::new(program.memory().slice(id, value)?);
        let mut pending = vec![&function.body];
        while let Some(block) = pending.pop() {
            for statement in &block.statements {
                match &statement.kind {
                    StatementKind::Assign { destination, value } => {
                        origins.extend(place(destination));
                        match value {
                            Rvalue::Load(source) => origins.extend(place(source)),
                            Rvalue::SliceLoad { slice: source, .. } => {
                                origins.extend(slice(*source));
                            }
                            _ => {}
                        }
                    }
                    StatementKind::SliceStore { slice: source, .. } => {
                        origins.extend(slice(*source));
                    }
                    StatementKind::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        pending.extend([then_block, else_block]);
                    }
                    StatementKind::Loop { body } => pending.push(body),
                    _ => {}
                }
            }
        }
    }
    origins.into_iter().collect()
}

impl StateAliasScopes {
    pub(super) unsafe fn new(
        context: LLVMContextRef,
        program: &onda_mir::OptimizedProgram,
    ) -> Self {
        let origins = accessed_origins(program);
        let plan = scopes::Plan::new(&origins);
        let mut scopes = Vec::new();
        for index in 0..plan.domains {
            let name = format!("onda.state.disjoint.{index}");
            let label = LLVMMDStringInContext2(context, name.as_ptr().cast(), name.len());
            let domain = self_referential_metadata_node(context, &[label]);
            for name in ["left", "right"] {
                let label = LLVMMDStringInContext2(context, name.as_ptr().cast(), name.len());
                scopes.push(self_referential_metadata_node(context, &[domain, label]));
            }
        }
        let list = |indices: &[usize]| {
            if indices.is_empty() {
                null_mut()
            } else {
                let mut nodes: Vec<_> = indices.iter().map(|&index| scopes[index]).collect();
                LLVMMetadataAsValue(
                    context,
                    LLVMMDNodeInContext2(context, nodes.as_mut_ptr(), nodes.len()),
                )
            }
        };
        let lists = origins
            .into_iter()
            .zip(plan.lists)
            .map(|(origins, lists)| {
                // Mixed origins can share non-state storage. Their scoped negative
                // facts remain useful against exclusively owned-state accesses.
                let alias = if origins.only_states {
                    list(&lists.alias)
                } else {
                    null_mut()
                };
                (origins, (alias, list(&lists.noalias)))
            })
            .collect();
        Self {
            lists,
            alias_kind: LLVMGetMDKindIDInContext(context, c"alias.scope".as_ptr(), 11),
            noalias_kind: LLVMGetMDKindIDInContext(context, c"noalias".as_ptr(), 7),
        }
    }

    pub(super) unsafe fn mark(
        &self,
        instruction: LLVMValueRef,
        origins: &onda_mir::StorageOrigins,
    ) {
        let Some(origins) = Origins::new(origins) else {
            return;
        };
        let &(alias, noalias) = self
            .lists
            .get(&origins)
            .expect("collected state alias origins");
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
            self.module.state_alias_scopes.mark(instruction, &origins);
        }
    }

    pub(super) unsafe fn mark_slice_access(&self, instruction: LLVMValueRef, slice: Value) {
        self.mark_parallel_access(instruction);
        if let Some(origins) = self
            .module
            .memory
            .slice(onda_mir::FunctionId::new(self.function_index as u32), slice)
        {
            self.module.state_alias_scopes.mark(instruction, origins);
        }
    }
}
