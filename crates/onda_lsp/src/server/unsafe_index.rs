use onda_frontend::{READ_UNSAFE_FN, WRITE_UNSAFE_FN};

#[derive(Debug, Clone, Copy)]
pub(super) struct UnsafeIndexOperation {
    pub(super) name: &'static str,
    pub(super) free_parameters: &'static str,
    pub(super) member_parameters: &'static str,
    pub(super) description: &'static str,
}

pub(super) const UNSAFE_INDEX_CONTRACT: &str =
    "Every index must be valid; violating this contract is memory-unsafe.";

pub(super) const UNSAFE_INDEX_OPERATIONS: &[UnsafeIndexOperation] = &[
    UnsafeIndexOperation {
        name: READ_UNSAFE_FN,
        free_parameters: "(storage, index, ...)",
        member_parameters: "(index, ...)",
        description: "Reads from indexable storage without clamping or a runtime bounds check.",
    },
    UnsafeIndexOperation {
        name: WRITE_UNSAFE_FN,
        free_parameters: "(storage, index, ..., value)",
        member_parameters: "(index, ..., value)",
        description: "Writes to indexable primitive storage without clamping or a runtime bounds check; it is statement-only.",
    },
];

pub(super) fn unsafe_index_operation(name: &str) -> Option<UnsafeIndexOperation> {
    UNSAFE_INDEX_OPERATIONS
        .iter()
        .copied()
        .find(|operation| operation.name == name)
}
