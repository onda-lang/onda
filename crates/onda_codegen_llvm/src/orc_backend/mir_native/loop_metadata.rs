//! Stable access groups survive inlining and loop-ID/debug-info replacement.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct ParallelLoop {
    pub id: LLVMMetadataRef,
    pub accesses: LLVMMetadataRef,
}

impl ParallelLoop {
    pub unsafe fn new(context: LLVMContextRef) -> Result<Self, MirCodegenError> {
        // LLVM 21's C API has no constructor for distinct empty nodes. Read
        // their canonical IR representation through the C API, keeping this
        // usable with LLVM-C shared libraries that do not export C++ symbols.
        // Metadata belongs to the context and outlives this temporary module.
        let ir = b"!onda.access = !{!0}\n!0 = distinct !{}\n";
        let buffer = LLVMCreateMemoryBufferWithMemoryRangeCopy(
            ir.as_ptr().cast(),
            ir.len(),
            c"access_group".as_ptr(),
        );
        let mut module = null_mut();
        let mut error = null_mut();
        if llvm_sys::ir_reader::LLVMParseIRInContext(context, buffer, &mut module, &mut error) != 0
        {
            let message = CStr::from_ptr(error).to_string_lossy().into_owned();
            LLVMDisposeMessage(error);
            return Err(MirCodegenError::llvm(message));
        }
        let module = OwnedLlvm::new(module, LLVMDisposeModule);
        let mut value = null_mut();
        LLVMGetNamedMetadataOperands(module.get(), c"onda.access".as_ptr(), &mut value);
        let accesses = LLVMValueAsMetadata(value);
        let name = c"llvm.loop.parallel_accesses";
        let mut property = [
            LLVMMDStringInContext2(context, name.as_ptr(), name.count_bytes()),
            accesses,
        ];
        let property = LLVMMDNodeInContext2(context, property.as_mut_ptr(), property.len());
        Ok(Self {
            id: self_referential_metadata_node(context, &[property]),
            accesses,
        })
    }
}
