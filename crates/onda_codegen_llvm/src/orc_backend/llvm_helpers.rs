use std::sync::OnceLock;

use llvm_sys::orc2::LLVMOrcThreadSafeContextRef;
use llvm_sys::prelude::*;

pub(super) static NATIVE_INIT_ERR: OnceLock<Option<String>> = OnceLock::new();
pub(super) static CODEGEN_TARGETS_INIT: OnceLock<()> = OnceLock::new();

extern "C" {
    #[link_name = "LLVMOrcCreateNewThreadSafeContextFromLLVMContext"]
    pub(super) fn llvm_orc_create_new_thread_safe_context_from_llvm_context(
        Ctx: LLVMContextRef,
    ) -> LLVMOrcThreadSafeContextRef;
}
