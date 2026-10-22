//! Line tables for IR inspection and optimization remarks. JIT/object emission
//! does not build this metadata, so diagnostic instrumentation has no runtime cost.
use super::*;
use llvm_sys::debuginfo::*;

pub(super) struct SourceDebug {
    builder: LLVMDIBuilderRef,
    files: Vec<LLVMMetadataRef>,
    fallback: LLVMMetadataRef,
    scopes: Vec<LLVMMetadataRef>,
}

impl SourceDebug {
    pub(super) unsafe fn new(
        module: LLVMModuleRef,
        program: &Program,
        functions: &[FunctionDecl],
    ) -> Self {
        let builder = LLVMCreateDIBuilder(module);
        let file = |path: &str| {
            LLVMDIBuilderCreateFile(builder, path.as_ptr().cast(), path.len(), c"".as_ptr(), 0)
        };
        let files: Vec<_> = program
            .source_files
            .iter()
            .map(|source| file(&source.path))
            .collect();
        let fallback = file("<onda>");
        // Onda has no assigned DW_LANG code. These are line tables for the
        // generated code, without source-language type/debugger semantics.
        LLVMDIBuilderCreateCompileUnit(
            builder,
            LLVMDWARFSourceLanguage::LLVMDWARFSourceLanguageAssembly,
            files.first().copied().unwrap_or(fallback),
            c"Onda".as_ptr(),
            4,
            1,
            c"".as_ptr(),
            0,
            0,
            c"".as_ptr(),
            0,
            LLVMDWARFEmissionKind::LLVMDWARFEmissionKindLineTablesOnly,
            0,
            1,
            0,
            c"".as_ptr(),
            0,
            c"".as_ptr(),
            0,
        );
        let context = LLVMGetModuleContext(module);
        LLVMAddModuleFlag(
            module,
            llvm_sys::LLVMModuleFlagBehavior::LLVMModuleFlagBehaviorWarning,
            c"Debug Info Version".as_ptr(),
            18,
            LLVMValueAsMetadata(LLVMConstInt(
                LLVMInt32TypeInContext(context),
                LLVMDebugMetadataVersion() as u64,
                0,
            )),
        );
        let mut scopes = Vec::with_capacity(functions.len());
        for (function, declaration) in program.functions.iter().zip(functions) {
            let file = function
                .source
                .file
                .and_then(|id| files.get(id.index()))
                .copied()
                .unwrap_or(fallback);
            let ty =
                LLVMDIBuilderCreateSubroutineType(builder, file, null_mut(), 0, LLVMDIFlagZero);
            let name = &function.name;
            let scope = LLVMDIBuilderCreateFunction(
                builder,
                file,
                name.as_ptr().cast(),
                name.len(),
                c"".as_ptr(),
                0,
                file,
                function.source.line,
                ty,
                i32::from(matches!(function.kind, FunctionKind::User)),
                1,
                function.source.line,
                LLVMDIFlagZero,
                1,
            );
            LLVMSetSubprogram(declaration.value, scope);
            scopes.push(scope);
        }
        Self {
            builder,
            files,
            fallback,
            scopes,
        }
    }

    pub(super) unsafe fn location(
        &self,
        context: LLVMContextRef,
        function: usize,
        source: onda_mir::SourceSpan,
    ) -> LLVMMetadataRef {
        let file = source
            .file
            .and_then(|id| self.files.get(id.index()))
            .copied()
            .unwrap_or(self.fallback);
        let scope =
            LLVMDIBuilderCreateLexicalBlockFile(self.builder, self.scopes[function], file, 0);
        LLVMDIBuilderCreateDebugLocation(context, source.line, source.column, scope, null_mut())
    }
}

impl Drop for SourceDebug {
    fn drop(&mut self) {
        unsafe {
            LLVMDIBuilderFinalize(self.builder);
            LLVMDisposeDIBuilder(self.builder);
        }
    }
}
