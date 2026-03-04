use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::mem::{align_of, size_of};
use std::ptr::null_mut;

use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::*;
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMErrorRef, LLVMGetErrorMessage};
use llvm_sys::orc2::lljit::*;
use llvm_sys::orc2::*;
use llvm_sys::prelude::*;
use llvm_sys::target::{
    LLVM_InitializeNativeAsmParser, LLVM_InitializeNativeAsmPrinter, LLVM_InitializeNativeTarget,
};
use llvm_sys::target_machine::{
    LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetMachine, LLVMDisposeTargetMachine,
    LLVMGetDefaultTargetTriple, LLVMGetHostCPUFeatures, LLVMGetHostCPUName,
    LLVMGetTargetFromTriple, LLVMRelocMode, LLVMTargetMachineRef, LLVMTargetRef,
};
use llvm_sys::transforms::pass_builder::{
    LLVMCreatePassBuilderOptions, LLVMDisposePassBuilderOptions, LLVMRunPasses,
};
use llvm_sys::{LLVMFastMathFlags, LLVMFastMathNone, LLVMIntPredicate, LLVMRealPredicate};
use omni_frontend::{
    AssignTarget, BinaryOp, BuiltinFn, CallArg, CallTypeArg, CmpOp, Diagnostic, Expr, LogicalOp,
    PrimitiveType, Stmt,
};
use omni_semantics::{
    internal_names::{PROC_INDEX_BASE_ARG, PROC_INDEX_BUFFER_SELECT_SENTINEL, PROC_INDEX_EXPR_ARG},
    ProcStepOversampleMeta, TypedArrayInfo, TypedBufferChannels, TypedEventParamType,
    TypedFieldType, TypedFnParam, TypedFunction, TypedProgram, TypedStructField,
};

mod array_access;
mod builtin_intrinsics;
mod call_helpers;
mod def_lowering;
mod expr_common;
mod jit_utils;
mod layout;
mod llvm_helpers;
mod orc_expr_stmt;
mod orc_locals;
mod pointer_helpers;
mod proc_ir;
mod specialization;
mod stmt_common;
mod user_fn_ir;
use array_access::*;
use builtin_intrinsics::*;
use call_helpers::*;
use def_lowering::*;
use expr_common::*;
use jit_utils::*;
use layout::*;
use llvm_helpers::*;
use orc_expr_stmt::*;
use orc_locals::*;
use pointer_helpers::*;
use proc_ir::*;
use specialization::*;
use stmt_common::*;
use user_fn_ir::*;

type OrcProcessFn = unsafe extern "C" fn(
    *const *const u8,
    *const *mut u8,
    u32,
    *const u8,
    *mut u8,
    *const *mut u8,
    *const i32,
    *const i32,
);
type OrcInitFn = unsafe extern "C" fn(*const u8, *mut u8);
type OrcEventFn =
    unsafe extern "C" fn(*const u8, *const u8, *mut u8, *const *mut u8, *const i32, *const i32);

fn is_proc_glue_function_name(name: &str) -> bool {
    name.contains(".__proc_")
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ProcSlotBufferRefLayout {
    slot_buffer_indices: Vec<usize>,
    ptr_offset: usize,
    frames_offset: usize,
    channels_offset: usize,
}

fn align_up_runtime(value: usize, align: usize) -> usize {
    if align <= 1 {
        return value;
    }
    let rem = value % align;
    if rem == 0 {
        value
    } else {
        value + (align - rem)
    }
}

fn collect_proc_slot_buffer_ref_signatures(
    typed: &TypedProgram,
    buffer_index: &HashMap<String, usize>,
) -> Vec<Vec<usize>> {
    fn visit_expr(
        expr: &Expr,
        buffer_index: &HashMap<String, usize>,
        out: &mut BTreeSet<Vec<usize>>,
    ) {
        if let Expr::UserCall { name, args, .. } = expr {
            if name == PROC_INDEX_BUFFER_SELECT_SENTINEL {
                let mut signature = Vec::<usize>::new();
                for arg in args {
                    match arg.name.as_deref() {
                        Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG) => {}
                        None => {
                            let Expr::Var(symbol) = &arg.expr else {
                                signature.clear();
                                break;
                            };
                            let Some(buffer_idx) = buffer_index.get(symbol).copied() else {
                                signature.clear();
                                break;
                            };
                            signature.push(buffer_idx);
                        }
                        Some(_) => {
                            signature.clear();
                            break;
                        }
                    }
                }
                if !signature.is_empty() {
                    out.insert(signature);
                }
            }
        }

        match expr {
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                visit_expr(lhs, buffer_index, out);
                visit_expr(rhs, buffer_index, out);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    visit_expr(arg, buffer_index, out);
                }
            }
            Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
                visit_expr(expr, buffer_index, out);
            }
            Expr::ArrayLiteral(values) => {
                for value in values {
                    visit_expr(value, buffer_index, out);
                }
            }
            Expr::ArrayCtor { init, .. } => {
                if let Some(values) = init {
                    for value in values {
                        visit_expr(value, buffer_index, out);
                    }
                }
            }
            Expr::UserCall { args, .. } => {
                for arg in args {
                    visit_expr(&arg.expr, buffer_index, out);
                }
            }
            Expr::Index { index, .. } => {
                visit_expr(index, buffer_index, out);
            }
            _ => {}
        }
    }

    fn visit_stmt(
        stmt: &Stmt,
        buffer_index: &HashMap<String, usize>,
        out: &mut BTreeSet<Vec<usize>>,
    ) {
        match stmt {
            Stmt::Assign { target, expr, .. } => {
                if let AssignTarget::Index { index, .. } = target {
                    visit_expr(index, buffer_index, out);
                }
                visit_expr(expr, buffer_index, out);
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                visit_expr(expr, buffer_index, out);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                visit_expr(cond, buffer_index, out);
                for nested in then_branch {
                    visit_stmt(nested, buffer_index, out);
                }
                for nested in else_branch {
                    visit_stmt(nested, buffer_index, out);
                }
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                visit_expr(start, buffer_index, out);
                visit_expr(end, buffer_index, out);
                if let Some(step_expr) = step {
                    visit_expr(step_expr, buffer_index, out);
                }
                for nested in body {
                    visit_stmt(nested, buffer_index, out);
                }
            }
            Stmt::While { cond, body, .. } => {
                visit_expr(cond, buffer_index, out);
                for nested in body {
                    visit_stmt(nested, buffer_index, out);
                }
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }

    let mut signatures = BTreeSet::<Vec<usize>>::new();
    for stmt in &typed.init {
        visit_stmt(stmt, buffer_index, &mut signatures);
    }
    for stmt in &typed.block_pre {
        visit_stmt(stmt, buffer_index, &mut signatures);
    }
    for stmt in &typed.sample {
        visit_stmt(stmt, buffer_index, &mut signatures);
    }
    for stmt in &typed.block_post {
        visit_stmt(stmt, buffer_index, &mut signatures);
    }
    for def in &typed.defs {
        for stmt in &def.body {
            visit_stmt(stmt, buffer_index, &mut signatures);
        }
    }
    for event in &typed.events {
        for stmt in &event.body {
            visit_stmt(stmt, buffer_index, &mut signatures);
        }
    }
    signatures.into_iter().collect()
}

fn compute_proc_slot_buffer_ref_layouts(
    typed: &TypedProgram,
    base_state_size_bytes: usize,
) -> Result<(Vec<ProcSlotBufferRefLayout>, usize), Diagnostic> {
    let buffer_index = typed
        .buffers
        .iter()
        .enumerate()
        .map(|(idx, decl)| (decl.name.clone(), idx))
        .collect::<HashMap<_, _>>();
    let signatures = collect_proc_slot_buffer_ref_signatures(typed, &buffer_index);

    let mut layouts = Vec::<ProcSlotBufferRefLayout>::with_capacity(signatures.len());
    let mut offset = base_state_size_bytes;
    for signature in signatures {
        let slots_len = signature.len();
        if slots_len == 0 {
            continue;
        }

        offset = align_up_runtime(offset, align_of::<usize>());
        let ptr_offset = offset;
        let ptr_bytes = slots_len.checked_mul(size_of::<usize>()).ok_or_else(|| {
            Diagnostic::internal("proc buffer-ref pointer array byte size overflow")
        })?;
        offset = offset
            .checked_add(ptr_bytes)
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref pointer layout overflow"))?;

        offset = align_up_runtime(offset, align_of::<i32>());
        let frames_offset = offset;
        let i32_bytes = slots_len
            .checked_mul(size_of::<i32>())
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref i32 array byte size overflow"))?;
        offset = offset
            .checked_add(i32_bytes)
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref frames layout overflow"))?;

        offset = align_up_runtime(offset, align_of::<i32>());
        let channels_offset = offset;
        offset = offset
            .checked_add(i32_bytes)
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref channels layout overflow"))?;

        layouts.push(ProcSlotBufferRefLayout {
            slot_buffer_indices: signature,
            ptr_offset,
            frames_offset,
            channels_offset,
        });
    }

    Ok((layouts, offset))
}

pub(crate) fn compile_orc(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<OrcProcess, Diagnostic> {
    initialize_native_target()?;
    let state_layout = compute_state_layout(typed)?;
    let arrays_layout = compute_arrays_layout(typed, &state_layout)?;
    let base_state_size_bytes = state_total_size_bytes(&state_layout, &arrays_layout);
    let (proc_slot_buffer_ref_layouts, state_total_size_bytes) =
        compute_proc_slot_buffer_ref_layouts(typed, base_state_size_bytes)?;

    unsafe {
        let builder = LLVMOrcCreateLLJITBuilder();
        if builder.is_null() {
            return Err(Diagnostic::internal("failed to create LLJIT builder"));
        }

        let jtmb = match create_aggressive_jit_target_machine_builder() {
            Ok(v) => v,
            Err(diag) => {
                LLVMOrcDisposeLLJITBuilder(builder);
                return Err(diag);
            }
        };
        if jtmb.is_null() {
            LLVMOrcDisposeLLJITBuilder(builder);
            return Err(Diagnostic::internal(
                "failed to create aggressive JIT target machine builder",
            ));
        }
        LLVMOrcLLJITBuilderSetJITTargetMachineBuilder(builder, jtmb);

        let mut lljit: LLVMOrcLLJITRef = null_mut();
        let lljit_err = LLVMOrcCreateLLJIT(&mut lljit, builder);
        if !lljit_err.is_null() {
            return Err(llvm_error_to_diag(
                "failed to create LLJIT instance",
                lljit_err,
            ));
        }
        if lljit.is_null() {
            return Err(Diagnostic::internal(
                "LLJIT creation returned null without error",
            ));
        }

        let (process_fn, init_fn, event_fns) = match compile_module_into_jit(
            lljit,
            typed,
            sample_rate,
            block_size,
            fast_math,
            &proc_slot_buffer_ref_layouts,
        ) {
            Ok(f) => f,
            Err(diag) => {
                dispose_lljit_quiet(lljit);
                return Err(diag);
            }
        };

        Ok(OrcProcess {
            lljit,
            process_fn,
            init_fn,
            event_fns,
            param_size_bytes: typed
                .params
                .iter()
                .map(|p| primitive_type_bytes(p.ty))
                .sum(),
            in_channels: typed.ins.len(),
            out_channels: typed.outs.len(),
            buffer_count: typed.buffers.len(),
            state_size_bytes: state_total_size_bytes,
            proc_slot_buffer_ref_layouts,
        })
    }
}

pub(crate) fn emit_optimized_ir(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<String, Diagnostic> {
    initialize_native_target()?;
    let state_layout = compute_state_layout(typed)?;
    let arrays_layout = compute_arrays_layout(typed, &state_layout)?;
    let base_state_size_bytes = state_total_size_bytes(&state_layout, &arrays_layout);
    let (proc_slot_buffer_ref_layouts, _state_total_size_bytes) =
        compute_proc_slot_buffer_ref_layouts(typed, base_state_size_bytes)?;

    unsafe {
        let builder = LLVMOrcCreateLLJITBuilder();
        if builder.is_null() {
            return Err(Diagnostic::internal("failed to create LLJIT builder"));
        }
        let jtmb = match create_aggressive_jit_target_machine_builder() {
            Ok(v) => v,
            Err(diag) => {
                LLVMOrcDisposeLLJITBuilder(builder);
                return Err(diag);
            }
        };
        if jtmb.is_null() {
            LLVMOrcDisposeLLJITBuilder(builder);
            return Err(Diagnostic::internal(
                "failed to create aggressive JIT target machine builder",
            ));
        }
        LLVMOrcLLJITBuilderSetJITTargetMachineBuilder(builder, jtmb);

        let mut lljit: LLVMOrcLLJITRef = null_mut();
        let lljit_err = LLVMOrcCreateLLJIT(&mut lljit, builder);
        if !lljit_err.is_null() {
            return Err(llvm_error_to_diag(
                "failed to create LLJIT instance",
                lljit_err,
            ));
        }
        if lljit.is_null() {
            return Err(Diagnostic::internal(
                "LLJIT creation returned null without error",
            ));
        }

        let result = (|| {
            let (module, ts_context) = build_optimized_module_for_jit(
                lljit,
                typed,
                sample_rate,
                block_size,
                fast_math,
                &proc_slot_buffer_ref_layouts,
            )?;
            let ir = llvm_module_to_string(module)?;
            LLVMDisposeModule(module);
            LLVMOrcDisposeThreadSafeContext(ts_context);
            Ok(ir)
        })();

        dispose_lljit_quiet(lljit);
        result
    }
}

#[derive(Debug)]
pub(crate) struct OrcProcess {
    lljit: LLVMOrcLLJITRef,
    process_fn: OrcProcessFn,
    init_fn: OrcInitFn,
    event_fns: Vec<OrcEventFn>,
    param_size_bytes: usize,
    in_channels: usize,
    out_channels: usize,
    buffer_count: usize,
    state_size_bytes: usize,
    proc_slot_buffer_ref_layouts: Vec<ProcSlotBufferRefLayout>,
}

impl OrcProcess {
    pub(crate) fn state_size_bytes(&self) -> usize {
        self.state_size_bytes
    }

    pub(crate) fn param_size_bytes(&self) -> usize {
        self.param_size_bytes
    }

    pub(crate) fn in_channels(&self) -> usize {
        self.in_channels
    }

    pub(crate) fn out_channels(&self) -> usize {
        self.out_channels
    }

    pub(crate) fn buffer_count(&self) -> usize {
        self.buffer_count
    }

    pub(crate) fn run(
        &self,
        in_ptrs: *const *const u8,
        out_ptrs: *const *mut u8,
        frames: u32,
        params: *const u8,
        state: *mut u8,
        buffer_ptrs: *const *mut u8,
        buffer_frames: *const i32,
        buffer_channels: *const i32,
    ) {
        unsafe {
            (self.process_fn)(
                in_ptrs,
                out_ptrs,
                frames,
                params,
                state,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
        }
    }

    pub(crate) fn run_init(&self, params: *const u8, state: *mut u8) {
        unsafe {
            (self.init_fn)(params, state);
        }
    }

    pub(crate) fn run_event(
        &self,
        event_index: u32,
        payload: *const u8,
        params: *const u8,
        state: *mut u8,
        buffer_ptrs: *const *mut u8,
        buffer_frames: *const i32,
        buffer_channels: *const i32,
    ) {
        let Some(fn_ref) = self.event_fns.get(event_index as usize).copied() else {
            return;
        };
        unsafe {
            (fn_ref)(
                payload,
                params,
                state,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
            );
        }
    }

    pub(crate) fn sync_proc_buffer_refs(
        &self,
        state: *mut u8,
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
    ) -> Result<(), Diagnostic> {
        if self.proc_slot_buffer_ref_layouts.is_empty() {
            return Ok(());
        }
        if state.is_null() {
            return Err(Diagnostic::runtime(
                "runtime state pointer is null during proc buffer-ref sync",
                0,
                0,
            ));
        }

        for layout in &self.proc_slot_buffer_ref_layouts {
            for (slot_idx, buffer_idx) in layout.slot_buffer_indices.iter().enumerate() {
                let Some(ptr_value) = buffer_ptrs.get(*buffer_idx).copied() else {
                    return Err(Diagnostic::internal(
                        "proc buffer-ref sync mapping references out-of-range buffer pointer index",
                    ));
                };
                let Some(frames_value) = buffer_frames.get(*buffer_idx).copied() else {
                    return Err(Diagnostic::internal(
                        "proc buffer-ref sync mapping references out-of-range buffer frames index",
                    ));
                };
                let Some(channels_value) = buffer_channels.get(*buffer_idx).copied() else {
                    return Err(Diagnostic::internal(
                        "proc buffer-ref sync mapping references out-of-range buffer channels index",
                    ));
                };

                let ptr_byte_off = layout
                    .ptr_offset
                    .saturating_add(slot_idx.saturating_mul(size_of::<usize>()));
                let frames_byte_off = layout
                    .frames_offset
                    .saturating_add(slot_idx.saturating_mul(size_of::<i32>()));
                let channels_byte_off = layout
                    .channels_offset
                    .saturating_add(slot_idx.saturating_mul(size_of::<i32>()));

                unsafe {
                    std::ptr::write_unaligned(
                        state.add(ptr_byte_off).cast::<usize>(),
                        ptr_value as usize,
                    );
                    std::ptr::write_unaligned(
                        state.add(frames_byte_off).cast::<i32>(),
                        frames_value,
                    );
                    std::ptr::write_unaligned(
                        state.add(channels_byte_off).cast::<i32>(),
                        channels_value,
                    );
                }
            }
        }

        Ok(())
    }
}

impl Drop for OrcProcess {
    fn drop(&mut self) {
        unsafe {
            let err = LLVMOrcDisposeLLJIT(self.lljit);
            if !err.is_null() {
                let msg_ptr = LLVMGetErrorMessage(err);
                if !msg_ptr.is_null() {
                    LLVMDisposeErrorMessage(msg_ptr);
                }
            }
        }
    }
}

fn initialize_native_target() -> Result<(), Diagnostic> {
    let init_result = NATIVE_INIT_ERR.get_or_init(|| unsafe {
        if LLVM_InitializeNativeTarget() != 0 {
            return Some("LLVM_InitializeNativeTarget failed".to_owned());
        }
        if LLVM_InitializeNativeAsmPrinter() != 0 {
            return Some("LLVM_InitializeNativeAsmPrinter failed".to_owned());
        }
        if LLVM_InitializeNativeAsmParser() != 0 {
            return Some("LLVM_InitializeNativeAsmParser failed".to_owned());
        }
        None
    });

    match init_result {
        Some(msg) => Err(Diagnostic::internal(msg.clone())),
        None => Ok(()),
    }
}

unsafe fn compile_module_into_jit(
    lljit: LLVMOrcLLJITRef,
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    proc_slot_buffer_ref_layouts: &[ProcSlotBufferRefLayout],
) -> Result<(OrcProcessFn, OrcInitFn, Vec<OrcEventFn>), Diagnostic> {
    let (module, ts_context) = build_optimized_module_for_jit(
        lljit,
        typed,
        sample_rate,
        block_size,
        fast_math,
        proc_slot_buffer_ref_layouts,
    )?;

    let thread_safe_module = LLVMOrcCreateNewThreadSafeModule(module, ts_context);
    if thread_safe_module.is_null() {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal(
            "failed to create ORC thread-safe module",
        ));
    }

    let add_err = LLVMOrcLLJITAddLLVMIRModule(
        lljit,
        LLVMOrcLLJITGetMainJITDylib(lljit),
        thread_safe_module,
    );
    if !add_err.is_null() {
        LLVMOrcDisposeThreadSafeModule(thread_safe_module);
        return Err(llvm_error_to_diag(
            "failed to add LLVM IR module to LLJIT",
            add_err,
        ));
    }

    let process_addr = lookup_symbol(lljit, "omni_process", "process function")?;
    let init_addr = lookup_symbol(lljit, "omni_init", "init function")?;
    let mut event_fns = Vec::<OrcEventFn>::new();
    for event_idx in 0..typed.events.len() {
        let symbol_name = format!("omni_event_{event_idx}");
        let event_addr = lookup_symbol(lljit, &symbol_name, "event function")?;
        event_fns.push(std::mem::transmute::<usize, OrcEventFn>(
            event_addr as usize,
        ));
    }
    Ok((
        std::mem::transmute::<usize, OrcProcessFn>(process_addr as usize),
        std::mem::transmute::<usize, OrcInitFn>(init_addr as usize),
        event_fns,
    ))
}

unsafe fn build_optimized_module_for_jit(
    lljit: LLVMOrcLLJITRef,
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    proc_slot_buffer_ref_layouts: &[ProcSlotBufferRefLayout],
) -> Result<(LLVMModuleRef, LLVMOrcThreadSafeContextRef), Diagnostic> {
    let context = LLVMContextCreate();
    if context.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM context"));
    }
    let ts_context = LLVMOrcCreateNewThreadSafeContextFromLLVMContext(context);
    if ts_context.is_null() {
        LLVMContextDispose(context);
        return Err(Diagnostic::internal(
            "failed to create LLVM ORC thread-safe context",
        ));
    }

    let module_name =
        CString::new("omni_module").map_err(|_| Diagnostic::internal("invalid module name"))?;
    let module = LLVMModuleCreateWithNameInContext(module_name.as_ptr(), context);
    if module.is_null() {
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal("failed to create LLVM module"));
    }

    let triple = LLVMOrcLLJITGetTripleString(lljit);
    if triple.is_null() {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal("failed to get JIT target triple"));
    }
    LLVMSetTarget(module, triple);

    let datalayout = LLVMOrcLLJITGetDataLayoutStr(lljit);
    if datalayout.is_null() {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal("failed to get JIT data layout"));
    }
    LLVMSetDataLayout(module, datalayout);

    let mut user_fns =
        match build_user_functions_ir(typed, module, context, sample_rate, block_size, fast_math) {
            Ok(v) => v,
            Err(diag) => {
                LLVMDisposeModule(module);
                LLVMOrcDisposeThreadSafeContext(ts_context);
                return Err(diag);
            }
        };

    if let Err(diag) = build_init_ir(
        typed,
        module,
        context,
        &mut user_fns,
        sample_rate,
        block_size,
        fast_math,
    ) {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(diag);
    }
    if let Err(diag) = build_process_ir(
        typed,
        module,
        context,
        &mut user_fns,
        sample_rate,
        block_size,
        fast_math,
        proc_slot_buffer_ref_layouts,
    ) {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(diag);
    }
    if let Err(diag) = build_event_ir(
        typed,
        module,
        context,
        &mut user_fns,
        sample_rate,
        block_size,
        fast_math,
    ) {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(diag);
    }
    if let Err(diag) = run_default_o3_pipeline(module) {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(diag);
    }

    let mut verify_message: *mut i8 = null_mut();
    let verify_failed = LLVMVerifyModule(
        module,
        LLVMVerifierFailureAction::LLVMReturnStatusAction,
        &mut verify_message,
    );
    if verify_failed != 0 {
        let detail = if verify_message.is_null() {
            "unknown LLVM verification error".to_owned()
        } else {
            let msg = CStr::from_ptr(verify_message).to_string_lossy().to_string();
            LLVMDisposeMessage(verify_message);
            msg
        };
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal(format!(
            "LLVM module verification failed: {detail}"
        )));
    }
    if !verify_message.is_null() {
        LLVMDisposeMessage(verify_message);
    }

    Ok((module, ts_context))
}

#[derive(Clone, Copy)]
struct OrcValue {
    value: LLVMValueRef,
    ty: PrimitiveType,
}

#[derive(Clone, Copy)]
struct ArrayElementPtr {
    ptr: LLVMValueRef,
    elem_ty: PrimitiveType,
}

#[derive(Clone)]
enum LocalArrayAlias {
    Primitive {
        base_ptr: LLVMValueRef,
        len: usize,
        elem_ty: PrimitiveType,
    },
    Struct {
        root_base: String,
        elem_struct: String,
        len: usize,
        start_index: LLVMValueRef,
    },
}

#[derive(Clone, Copy)]
struct DefLocalSlot {
    ptr: LLVMValueRef,
    ty: PrimitiveType,
}

#[derive(Clone, Copy)]
struct StateSlot {
    ptr: LLVMValueRef,
    ty: PrimitiveType,
}

#[derive(Clone, Copy)]
struct OutSlot {
    ptr: LLVMValueRef,
    ty: PrimitiveType,
}

#[derive(Clone, Copy)]
struct AliasSlot {
    ptr: LLVMValueRef,
    ty: PrimitiveType,
}

#[derive(Clone, Copy)]
struct ProcSlotBufferRefArrays {
    ptrs_base: LLVMValueRef,
    frames_base: LLVMValueRef,
    channels_base: LLVMValueRef,
    len: usize,
}

#[derive(Clone, Copy)]
struct LoopControl {
    break_bb: LLVMBasicBlockRef,
    continue_bb: LLVMBasicBlockRef,
}

struct LoweringCtx<'a> {
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    module: LLVMModuleRef,
    fn_ref: LLVMValueRef,
    float_ty: LLVMTypeRef,
    float_ptr_ty: LLVMTypeRef,
    i32_ty: LLVMTypeRef,
    fast_math_flags: LLVMFastMathFlags,
    sample_rate: f32,
    block_size: f32,
    in_ptrs: LLVMValueRef,
    params_ptr: LLVMValueRef,
    buffer_ptrs: LLVMValueRef,
    buffer_frames_ptr: LLVMValueRef,
    buffer_channels_ptr: LLVMValueRef,
    frame_idx: LLVMValueRef,
    state_slots: &'a HashMap<String, StateSlot>,
    array_base_ptrs: &'a HashMap<String, LLVMValueRef>,
    out_slots: &'a HashMap<String, OutSlot>,
    out_array_base_ptrs: &'a HashMap<String, LLVMValueRef>,
    input_index: &'a HashMap<String, u32>,
    input_types: &'a HashMap<String, PrimitiveType>,
    input_arrays: &'a HashMap<String, TypedArrayInfo>,
    buffer_index: &'a HashMap<String, u32>,
    buffer_elem_types: &'a HashMap<String, PrimitiveType>,
    buffer_channels: &'a HashMap<String, TypedBufferChannels>,
    buffer_mono: &'a HashSet<String>,
    proc_slot_buffer_refs: &'a HashMap<Vec<u32>, ProcSlotBufferRefArrays>,
    param_byte_offset: &'a HashMap<String, usize>,
    param_types: &'a HashMap<String, PrimitiveType>,
    param_arrays: &'a HashMap<String, TypedArrayInfo>,
    output_arrays: &'a HashMap<String, TypedArrayInfo>,
    array_len: &'a HashMap<String, usize>,
    array_elem_ty: &'a HashMap<String, PrimitiveType>,
    array_struct_roots: &'a HashMap<String, String>,
    array_struct_len: &'a HashMap<String, usize>,
    struct_fields: &'a HashMap<String, Vec<TypedStructField>>,
    allow_struct_ctor: bool,
    user_fn_param_names: &'a HashMap<String, Vec<String>>,
    user_fn_param_defaults: &'a HashMap<String, Vec<Option<Expr>>>,
    user_fn_param_kinds: &'a HashMap<String, Vec<TypedFnParam>>,
    user_fn_param_by_ref: &'a HashMap<String, Vec<bool>>,
    user_registry: *const UserFnRegistry,
    oversample_factor: usize,
    oversample_alpha: Option<LLVMValueRef>,
    oversample_prev_inputs: Option<LLVMValueRef>,
    oversample_input_cache: Option<LLVMValueRef>,
    loop_stack: Vec<LoopControl>,
}

struct UserFnRegistry {
    defs: HashMap<String, TypedFunction>,
    sample_oversample_factors: HashMap<String, usize>,
    proc_step_oversample_meta: HashMap<String, ProcStepOversampleMeta>,
    refs: HashMap<String, LLVMValueRef>,
    base_return_tys: HashMap<String, PrimitiveType>,
    mono_refs: HashMap<String, LLVMValueRef>,
    mono_tys: HashMap<String, LLVMTypeRef>,
    mono_return_tys: HashMap<String, PrimitiveType>,
    param_names: HashMap<String, Vec<String>>,
    param_defaults: HashMap<String, Vec<Option<Expr>>>,
    param_kinds: HashMap<String, Vec<TypedFnParam>>,
    param_by_ref: HashMap<String, Vec<bool>>,
    in_progress: HashSet<String>,
    return_in_progress: HashSet<String>,
}

struct DefLoweringCtx<'a> {
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    module: LLVMModuleRef,
    fn_ref: LLVMValueRef,
    float_ty: LLVMTypeRef,
    i32_ty: LLVMTypeRef,
    fast_math_flags: LLVMFastMathFlags,
    sample_rate: f32,
    block_size: f32,
    return_ty: PrimitiveType,
    return_slot: LLVMValueRef,
    return_block: LLVMBasicBlockRef,
    local_slots: HashMap<String, DefLocalSlot>,
    local_array_aliases: HashMap<String, LocalArrayAlias>,
    buffer_params: HashMap<String, DefBufferParamInfo>,
    array_ptrs: HashMap<String, LLVMValueRef>,
    array_len: HashMap<String, usize>,
    array_len_values: HashMap<String, LLVMValueRef>,
    array_elem_ty: HashMap<String, PrimitiveType>,
    array_struct_roots: HashMap<String, String>,
    struct_fields: &'a HashMap<String, Vec<TypedStructField>>,
    user_fn_param_names: &'a HashMap<String, Vec<String>>,
    user_fn_param_defaults: &'a HashMap<String, Vec<Option<Expr>>>,
    user_fn_param_kinds: &'a HashMap<String, Vec<TypedFnParam>>,
    user_fn_param_by_ref: &'a HashMap<String, Vec<bool>>,
    user_registry: *const UserFnRegistry,
    loop_stack: Vec<LoopControl>,
}

#[derive(Clone)]
struct DefBufferParamInfo {
    ptr: LLVMValueRef,
    frames: LLVMValueRef,
    channels: LLVMValueRef,
    elem_ty: PrimitiveType,
    declared_channels: TypedBufferChannels,
}
