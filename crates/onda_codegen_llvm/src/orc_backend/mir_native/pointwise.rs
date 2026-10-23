//! Version pointwise audio loops for disjoint or exactly equal bindings. The
//! MIR proof concerns iteration independence, not pointer noalias or FP order.
use super::*;

impl FunctionEmitter<'_, '_> {
    pub(super) unsafe fn lower_pointwise_loop(
        &mut self,
        body: &Block,
    ) -> Result<bool, MirCodegenError> {
        if self.parallel_loop.is_some() {
            return Ok(false);
        }
        let Some(plan) = onda_mir::pointwise_audio_loop(
            self.module.program,
            self.module.effects,
            self.function,
            body,
        ) else {
            return Ok(false);
        };
        if plan.ports.len() < 2 {
            return Ok(false);
        }
        let context = self.module.context;
        let guard = append_block(context, self.declaration.value, "audio_alias_guard")?;
        let parallel = append_block(context, self.declaration.value, "audio_independent")?;
        let scalar = append_block(context, self.declaration.value, "audio_overlap")?;
        let merge = append_block(context, self.declaration.value, "audio_loop_done")?;
        let limit = self.lower_value(onda_mir::counted_loop(body).unwrap().limit)?;
        let enough = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSGE,
            limit,
            LLVMConstInt(LLVMTypeOf(limit), 2, 0),
            c"audio_multiple_frames".as_ptr(),
        );
        // A zero-frame process call need not dereference its audio tables.
        LLVMBuildCondBr(self.builder, enough, guard, scalar);
        LLVMPositionBuilderAtEnd(self.builder, guard);
        let integer = llvm_sys::target::LLVMIntPtrTypeInContext(
            context,
            llvm_sys::target::LLVMGetModuleDataLayout(self.module.module),
        );
        let mut pointers = Vec::new();
        for port in &plan.ports {
            let (field, index) = match *port {
                onda_mir::AudioPort::Input(id) => (0, self.module.layouts.input_bases[id.index()]),
                onda_mir::AudioPort::Output(id) => {
                    (1, self.module.layouts.output_bases[id.index()])
                }
            };
            let port = LLVMConstInt(LLVMInt32TypeInContext(context), index as u64, 0);
            let pointer = self.audio_channel_ptr(field, port)?;
            pointers.push(LLVMBuildPtrToInt(
                self.builder,
                pointer,
                integer,
                c"audio_address".as_ptr(),
            ));
        }
        let extent = LLVMConstInt(
            integer,
            u64::from(self.module.program.config.block_size) * scalar_store_size(plan.scalar),
            0,
        );
        let mut safe = LLVMConstInt(LLVMInt1TypeInContext(context), 1, 0);
        for i in 0..pointers.len() {
            for j in i + 1..pointers.len() {
                if matches!(
                    (plan.ports[i], plan.ports[j]),
                    (onda_mir::AudioPort::Input(_), onda_mir::AudioPort::Input(_))
                ) {
                    continue;
                }
                let (a, b) = (pointers[i], pointers[j]);
                let equal = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntEQ,
                    a,
                    b,
                    c"audio_equal".as_ptr(),
                );
                let before = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntULT,
                    a,
                    b,
                    c"audio_before".as_ptr(),
                );
                let ab = LLVMBuildSub(self.builder, b, a, c"audio_distance_ab".as_ptr());
                let ba = LLVMBuildSub(self.builder, a, b, c"audio_distance_ba".as_ptr());
                let distance =
                    LLVMBuildSelect(self.builder, before, ab, ba, c"audio_distance".as_ptr());
                let disjoint = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntUGE,
                    distance,
                    extent,
                    c"audio_disjoint".as_ptr(),
                );
                let pair = LLVMBuildOr(
                    self.builder,
                    equal,
                    disjoint,
                    c"audio_pair_independent".as_ptr(),
                );
                safe = LLVMBuildAnd(
                    self.builder,
                    safe,
                    pair,
                    c"audio_independent_bindings".as_ptr(),
                );
            }
        }
        LLVMBuildCondBr(self.builder, safe, parallel, scalar);
        LLVMPositionBuilderAtEnd(self.builder, parallel);
        self.parallel_loop = Some(super::loop_metadata::ParallelLoop::new(context)?);
        self.lower_plain_loop(body)?;
        self.parallel_loop = None;
        LLVMBuildBr(self.builder, merge);
        LLVMPositionBuilderAtEnd(self.builder, scalar);
        self.lower_plain_loop(body)?;
        LLVMBuildBr(self.builder, merge);
        LLVMPositionBuilderAtEnd(self.builder, merge);
        Ok(true)
    }

    pub(super) unsafe fn mark_parallel_access(&self, instruction: LLVMValueRef) {
        if let Some(loop_) = self.parallel_loop {
            let name = "llvm.access.group";
            let kind = LLVMGetMDKindIDInContext(
                self.module.context,
                name.as_ptr().cast(),
                name.len() as u32,
            );
            LLVMSetMetadata(
                instruction,
                kind,
                LLVMMetadataAsValue(self.module.context, loop_.accesses),
            );
        }
    }

    pub(super) unsafe fn mark_parallel_latch(&self, branch: LLVMValueRef) {
        if let Some(loop_) = self.parallel_loop {
            let kind = LLVMGetMDKindIDInContext(self.module.context, c"llvm.loop".as_ptr(), 9);
            LLVMSetMetadata(
                branch,
                kind,
                LLVMMetadataAsValue(self.module.context, loop_.id),
            );
        }
    }
}
