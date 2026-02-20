use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::ptr::null_mut;
use std::sync::OnceLock;

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
use llvm_sys::{LLVMIntPredicate, LLVMRealPredicate};
use omni_frontend::{
    AssignTarget, BinaryOp, BuiltinFn, CallArg, CallTypeArg, CmpOp, Diagnostic, Expr, LogicalOp,
    PrimitiveType, Stmt,
};
use omni_semantics::{
    TypedArrayInfo, TypedBufferChannels, TypedFieldType, TypedFnParam, TypedFunction, TypedProgram,
    TypedStructField,
};

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

#[derive(Debug, Clone)]
struct StateLayoutEntry {
    name: String,
    ty: PrimitiveType,
    offset: usize,
}

#[derive(Debug, Clone)]
struct ArrayLayoutEntry {
    name: String,
    elem_ty: PrimitiveType,
    len: usize,
    offset: usize,
}

fn is_proc_glue_function_name(name: &str) -> bool {
    name.contains(".__proc_")
}

unsafe fn set_internal_alwaysinline(
    fn_ref: LLVMValueRef,
    context: LLVMContextRef,
) -> Result<(), Diagnostic> {
    LLVMSetLinkage(fn_ref, llvm_sys::LLVMLinkage::LLVMInternalLinkage);
    let alwaysinline_name = b"alwaysinline";
    let kind =
        LLVMGetEnumAttributeKindForName(alwaysinline_name.as_ptr().cast(), alwaysinline_name.len());
    if kind == 0 {
        return Err(Diagnostic::internal(
            "failed to resolve LLVM enum attribute kind for 'alwaysinline'",
        ));
    }
    let attr = LLVMCreateEnumAttribute(context, kind, 0);
    LLVMAddAttributeAtIndex(fn_ref, llvm_sys::LLVMAttributeFunctionIndex, attr);
    Ok(())
}

static NATIVE_INIT_ERR: OnceLock<Option<String>> = OnceLock::new();

extern "C" {
    fn LLVMOrcCreateNewThreadSafeContextFromLLVMContext(
        Ctx: LLVMContextRef,
    ) -> LLVMOrcThreadSafeContextRef;
}

pub(crate) fn compile_orc(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
) -> Result<OrcProcess, Diagnostic> {
    initialize_native_target()?;
    let state_layout = compute_state_layout(typed)?;
    let arrays_layout = compute_arrays_layout(typed, &state_layout)?;
    let state_total_size_bytes = state_total_size_bytes(&state_layout, &arrays_layout);

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

        let (process_fn, init_fn) =
            match compile_module_into_jit(lljit, typed, sample_rate, block_size) {
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
            param_size_bytes: typed
                .params
                .iter()
                .map(|p| primitive_type_bytes(p.ty))
                .sum(),
            in_channels: typed.ins.len(),
            out_channels: typed.outs.len(),
            buffer_count: typed.buffers.len(),
            state_size_bytes: state_total_size_bytes,
        })
    }
}

pub(crate) fn emit_optimized_ir(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
) -> Result<String, Diagnostic> {
    initialize_native_target()?;

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
            let (module, ts_context) =
                build_optimized_module_for_jit(lljit, typed, sample_rate, block_size)?;
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
    param_size_bytes: usize,
    in_channels: usize,
    out_channels: usize,
    buffer_count: usize,
    state_size_bytes: usize,
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
}

fn primitive_type_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
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
) -> Result<(OrcProcessFn, OrcInitFn), Diagnostic> {
    let (module, ts_context) =
        build_optimized_module_for_jit(lljit, typed, sample_rate, block_size)?;

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
    Ok((
        std::mem::transmute::<usize, OrcProcessFn>(process_addr as usize),
        std::mem::transmute::<usize, OrcInitFn>(init_addr as usize),
    ))
}

unsafe fn build_optimized_module_for_jit(
    lljit: LLVMOrcLLJITRef,
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
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
        match build_user_functions_ir(typed, module, context, sample_rate, block_size) {
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
struct DataElementPtr {
    ptr: LLVMValueRef,
    elem_ty: PrimitiveType,
}

#[derive(Clone)]
enum LocalDataAlias {
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

struct LoweringCtx<'a> {
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    module: LLVMModuleRef,
    fn_ref: LLVMValueRef,
    float_ty: LLVMTypeRef,
    float_ptr_ty: LLVMTypeRef,
    i32_ty: LLVMTypeRef,
    sample_rate: f32,
    block_size: f32,
    in_ptrs: LLVMValueRef,
    params_ptr: LLVMValueRef,
    buffer_ptrs: LLVMValueRef,
    buffer_frames_ptr: LLVMValueRef,
    buffer_channels_ptr: LLVMValueRef,
    frame_idx: LLVMValueRef,
    state_slots: &'a HashMap<String, StateSlot>,
    data_base_ptrs: &'a HashMap<String, LLVMValueRef>,
    out_slots: &'a HashMap<String, OutSlot>,
    out_array_base_ptrs: &'a HashMap<String, LLVMValueRef>,
    input_index: &'a HashMap<String, u32>,
    input_types: &'a HashMap<String, PrimitiveType>,
    input_arrays: &'a HashMap<String, TypedArrayInfo>,
    buffer_index: &'a HashMap<String, u32>,
    buffer_elem_types: &'a HashMap<String, PrimitiveType>,
    buffer_channels: &'a HashMap<String, TypedBufferChannels>,
    buffer_mono: &'a HashSet<String>,
    param_byte_offset: &'a HashMap<String, usize>,
    param_types: &'a HashMap<String, PrimitiveType>,
    param_arrays: &'a HashMap<String, TypedArrayInfo>,
    output_arrays: &'a HashMap<String, TypedArrayInfo>,
    data_len: &'a HashMap<String, usize>,
    data_elem_ty: &'a HashMap<String, PrimitiveType>,
    data_struct_roots: &'a HashMap<String, String>,
    data_struct_len: &'a HashMap<String, usize>,
    struct_fields: &'a HashMap<String, Vec<TypedStructField>>,
    allow_struct_ctor: bool,
    user_fn_param_names: &'a HashMap<String, Vec<String>>,
    user_fn_param_defaults: &'a HashMap<String, Vec<Option<Expr>>>,
    user_fn_param_kinds: &'a HashMap<String, Vec<TypedFnParam>>,
    user_fn_param_by_ref: &'a HashMap<String, Vec<bool>>,
    user_registry: *const UserFnRegistry,
}

struct UserFnRegistry {
    defs: HashMap<String, TypedFunction>,
    refs: HashMap<String, LLVMValueRef>,
    tys: HashMap<String, LLVMTypeRef>,
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
    sample_rate: f32,
    block_size: f32,
    return_ty: PrimitiveType,
    return_slot: LLVMValueRef,
    return_block: LLVMBasicBlockRef,
    local_slots: HashMap<String, DefLocalSlot>,
    local_data_aliases: HashMap<String, LocalDataAlias>,
    buffer_params: HashMap<String, DefBufferParamInfo>,
    data_ptrs: HashMap<String, LLVMValueRef>,
    data_len: HashMap<String, usize>,
    data_elem_ty: HashMap<String, PrimitiveType>,
    data_struct_roots: HashMap<String, String>,
    struct_fields: &'a HashMap<String, Vec<TypedStructField>>,
    user_fn_param_names: &'a HashMap<String, Vec<String>>,
    user_fn_param_defaults: &'a HashMap<String, Vec<Option<Expr>>>,
    user_fn_param_kinds: &'a HashMap<String, Vec<TypedFnParam>>,
    user_fn_param_by_ref: &'a HashMap<String, Vec<bool>>,
    user_registry: *const UserFnRegistry,
}

#[derive(Clone)]
struct DefBufferParamInfo {
    ptr: LLVMValueRef,
    frames: LLVMValueRef,
    channels: LLVMValueRef,
    elem_ty: PrimitiveType,
    declared_channels: TypedBufferChannels,
}

unsafe fn build_user_functions_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    sample_rate: f32,
    block_size: usize,
) -> Result<UserFnRegistry, Diagnostic> {
    let float_ptr_ty = LLVMPointerType(LLVMFloatTypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let i8_ty = LLVMInt8TypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let struct_fields = typed
        .structs
        .iter()
        .map(|s| (s.name.clone(), s.fields.clone()))
        .collect::<HashMap<_, _>>();
    let mut defs = HashMap::new();
    let mut refs = HashMap::new();
    let mut tys = HashMap::new();
    let mut base_return_tys = HashMap::new();
    let mut param_names = HashMap::new();
    let mut param_defaults = HashMap::new();
    let mut param_kinds = HashMap::new();
    let mut param_by_ref = HashMap::new();

    for def in &typed.defs {
        defs.insert(def.name.clone(), def.clone());
        base_return_tys.insert(def.name.clone(), def.return_ty);
        let mut arg_tys = Vec::new();
        let mut by_ref_flags = vec![false; def.param_kinds.len()];
        if def.method_of.is_some() && !def.params.is_empty() && def.params[0] == "self" {
            by_ref_flags[0] = true;
        }
        for (idx, kind) in def.param_kinds.iter().enumerate() {
            match kind {
                TypedFnParam::Scalar => arg_tys.push(LLVMFloatTypeInContext(context)),
                TypedFnParam::Struct { struct_name } => {
                    // Phase 2: all struct parameters are passed by reference.
                    by_ref_flags[idx] = true;
                    let fields = struct_fields.get(struct_name).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "unknown struct '{}' in function '{}' parameter lowering",
                            struct_name, def.name
                        ))
                    })?;
                    for field in fields {
                        match field.ty {
                            TypedFieldType::Scalar(prim) => {
                                if by_ref_flags[idx] {
                                    arg_tys.push(float_ptr_ty);
                                } else {
                                    arg_tys.push(llvm_ty_for_primitive(context, prim));
                                }
                            }
                            TypedFieldType::Data(len) => {
                                if let Some(elem_struct) = &field.data_elem_struct {
                                    let mut roots = Vec::new();
                                    let mut leaves = Vec::new();
                                    collect_data_struct_bindings(
                                        &struct_fields,
                                        elem_struct,
                                        &format!("{}.{}", def.params[idx], field.name),
                                        len,
                                        &mut roots,
                                        &mut leaves,
                                        &mut Vec::new(),
                                    )?;
                                    for _ in &leaves {
                                        arg_tys.push(float_ptr_ty);
                                    }
                                } else {
                                    arg_tys.push(float_ptr_ty);
                                }
                            }
                        }
                    }
                }
                TypedFnParam::Buffer { .. } => {
                    arg_tys.push(i8_ptr_ty);
                    arg_tys.push(i32_ty);
                    arg_tys.push(i32_ty);
                }
            }
        }
        let ret_ty = llvm_ty_for_primitive(context, def.return_ty);
        let fn_ty = LLVMFunctionType(ret_ty, arg_tys.as_mut_ptr(), arg_tys.len() as u32, 0);
        let symbol = mangle_user_fn_symbol(&def.name)?;
        let fn_ref = LLVMAddFunction(module, symbol.as_ptr(), fn_ty);
        if fn_ref.is_null() {
            return Err(Diagnostic::internal(format!(
                "failed to add user function '{}'",
                def.name
            )));
        }
        if is_proc_glue_function_name(&def.name) {
            set_internal_alwaysinline(fn_ref, context)?;
        }
        refs.insert(def.name.clone(), fn_ref);
        tys.insert(def.name.clone(), fn_ty);
        param_names.insert(def.name.clone(), def.params.clone());
        param_defaults.insert(def.name.clone(), def.param_defaults.clone());
        param_kinds.insert(def.name.clone(), def.param_kinds.clone());
        param_by_ref.insert(def.name.clone(), by_ref_flags);
    }

    let mut registry = UserFnRegistry {
        defs,
        refs,
        tys,
        base_return_tys,
        mono_refs: HashMap::new(),
        mono_tys: HashMap::new(),
        mono_return_tys: HashMap::new(),
        param_names,
        param_defaults,
        param_kinds,
        param_by_ref,
        in_progress: HashSet::new(),
        return_in_progress: HashSet::new(),
    };
    for def in &typed.defs {
        let fn_ref = *registry.refs.get(&def.name).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing base LLVM function reference for '{}'",
                def.name
            ))
        })?;
        let scalar_sig = default_scalar_signature(def);
        let buffer_sig = default_buffer_signature(def);
        lower_user_function_body(
            def,
            module,
            context,
            &mut registry,
            &struct_fields,
            sample_rate,
            block_size,
            fn_ref,
            def.return_ty,
            &scalar_sig,
            &buffer_sig,
        )?;
    }

    Ok(registry)
}

fn mangle_user_fn_symbol(name: &str) -> Result<CString, Diagnostic> {
    CString::new(format!("omni_def_{name}"))
        .map_err(|_| Diagnostic::internal(format!("invalid function name '{name}'")))
}

fn primitive_sig_code(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "b1",
    }
}

fn buffer_channel_sig_code(channels: &TypedBufferChannels) -> String {
    match channels {
        TypedBufferChannels::Mono => "m".to_owned(),
        TypedBufferChannels::Dynamic => "d".to_owned(),
        TypedBufferChannels::Static(ch) => format!("s{ch}"),
    }
}

fn user_fn_mono_key(
    name: &str,
    scalar_types: &[PrimitiveType],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
) -> String {
    if scalar_types.is_empty() && buffer_types.is_empty() && generic_type_args.is_empty() {
        return format!("{name}__mono");
    }
    let mut sig_parts = generic_type_args
        .iter()
        .map(|t| format!("gen_{}", primitive_sig_code(*t)))
        .collect::<Vec<_>>();
    sig_parts.extend(
        scalar_types
            .iter()
            .map(|t| primitive_sig_code(*t))
            .map(|s| s.to_owned())
            .collect::<Vec<_>>(),
    );
    sig_parts.extend(buffer_types.iter().map(|(elem_ty, channels)| {
        format!(
            "buf_{}_{}",
            primitive_sig_code(*elem_ty),
            buffer_channel_sig_code(channels)
        )
    }));
    let sig = sig_parts.join("_");
    format!("{name}__mono__{sig}")
}

fn mangle_user_fn_symbol_mono(
    name: &str,
    scalar_types: &[PrimitiveType],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
) -> Result<CString, Diagnostic> {
    CString::new(format!(
        "omni_def_{}",
        user_fn_mono_key(name, scalar_types, buffer_types, generic_type_args)
    ))
    .map_err(|_| {
        Diagnostic::internal(format!(
            "invalid monomorphized function name for '{}'",
            name
        ))
    })
}

fn default_scalar_signature(def: &TypedFunction) -> Vec<PrimitiveType> {
    def.param_kinds
        .iter()
        .filter_map(|k| match k {
            TypedFnParam::Scalar => Some(PrimitiveType::F32),
            TypedFnParam::Struct { .. } => None,
            TypedFnParam::Buffer { .. } => None,
        })
        .collect::<Vec<_>>()
}

fn default_buffer_signature(def: &TypedFunction) -> Vec<(PrimitiveType, TypedBufferChannels)> {
    def.param_kinds
        .iter()
        .filter_map(|k| match k {
            TypedFnParam::Buffer { elem_ty, channels } => Some((*elem_ty, channels.clone())),
            _ => None,
        })
        .collect::<Vec<_>>()
}

fn resolve_explicit_call_type_args_for_codegen(
    name: &str,
    context: &str,
    type_args: &[CallTypeArg],
) -> Result<Vec<PrimitiveType>, Diagnostic> {
    let mut out = Vec::<PrimitiveType>::with_capacity(type_args.len());
    for arg in type_args {
        match arg {
            CallTypeArg::Primitive(ty) => out.push(*ty),
            CallTypeArg::Generic(param) => {
                return Err(Diagnostic::internal(format!(
                    "function '{}' in {} has unresolved generic type argument '{}'; expected concrete primitive type",
                    name, context, param
                )));
            }
        }
    }
    Ok(out)
}

fn apply_explicit_generic_type_args_for_call(
    registry: &UserFnRegistry,
    name: &str,
    explicit_type_args: &[PrimitiveType],
    scalar_types: &mut [PrimitiveType],
    context: &str,
) -> Result<(), Diagnostic> {
    let def = registry.defs.get(name).ok_or_else(|| {
        Diagnostic::internal(format!("unknown function '{}' in {}", name, context))
    })?;
    let expected = def.type_params.len();
    if expected == 0 {
        if !explicit_type_args.is_empty() {
            return Err(Diagnostic::internal(format!(
                "function '{}' in {} is not generic but call provided {} type args",
                name,
                context,
                explicit_type_args.len()
            )));
        }
        return Ok(());
    }
    if explicit_type_args.is_empty() {
        if expected > scalar_types.len() {
            return Err(Diagnostic::internal(format!(
                "function '{}' in {} has {} type params but only {} scalar params available for specialization",
                name,
                context,
                expected,
                scalar_types.len()
            )));
        }
        return Ok(());
    }
    if explicit_type_args.len() != expected {
        return Err(Diagnostic::internal(format!(
            "function '{}' in {} expects {} type args, got {}",
            name,
            context,
            expected,
            explicit_type_args.len()
        )));
    }
    if expected > scalar_types.len() {
        return Err(Diagnostic::internal(format!(
            "function '{}' in {} has {} type params but only {} scalar params available for specialization",
            name,
            context,
            expected,
            scalar_types.len()
        )));
    }
    for (idx, ty) in explicit_type_args.iter().enumerate() {
        scalar_types[idx] = *ty;
    }
    Ok(())
}

fn collect_data_struct_bindings(
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    struct_name: &str,
    base: &str,
    len: usize,
    roots: &mut Vec<(String, String, usize)>,
    leaves: &mut Vec<(String, usize, PrimitiveType)>,
    stack: &mut Vec<String>,
) -> Result<(), Diagnostic> {
    if stack.iter().any(|s| s == struct_name) {
        return Err(Diagnostic::internal(format!(
            "recursive Data[Struct] layout encountered while lowering '{}'",
            struct_name
        )));
    }
    let fields = struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{}' while collecting Data[Struct] bindings",
            struct_name
        ))
    })?;
    roots.push((base.to_owned(), struct_name.to_owned(), len));
    stack.push(struct_name.to_owned());
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => leaves.push((flat, len, prim)),
            TypedFieldType::Data(field_len) => {
                let nested_len = len.saturating_mul(field_len);
                if let Some(elem_struct) = &field.data_elem_struct {
                    collect_data_struct_bindings(
                        struct_fields,
                        elem_struct,
                        &flat,
                        nested_len,
                        roots,
                        leaves,
                        stack,
                    )?;
                } else {
                    leaves.push((
                        flat,
                        nested_len,
                        field.data_elem_ty.unwrap_or(PrimitiveType::F32),
                    ));
                }
            }
        }
    }
    stack.pop();
    Ok(())
}

fn merge_inferred_def_return_types(
    lhs: PrimitiveType,
    rhs: PrimitiveType,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (a, b) if a == b => Some(a),
        (Bool, _) | (_, Bool) => None,
        (F64, _) | (_, F64) => Some(F64),
        (F32, I64) | (I64, F32) => Some(F64),
        (F32, I32) | (I32, F32) => Some(F32),
        (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
    }
}

fn is_builtin_constant_symbol(name: &str) -> bool {
    matches!(
        name,
        "PI" | "TWO_PI" | "TWOPI" | "SAMPLE_RATE" | "SR" | "BLOCK_SIZE"
    )
}

fn infer_specialized_expr_return_type(
    expr: &Expr,
    locals: &HashMap<String, PrimitiveType>,
    registry: &mut UserFnRegistry,
) -> Result<Option<PrimitiveType>, Diagnostic> {
    Ok(match expr {
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Int(v) => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::ArrayLiteral(_) | Expr::DataCtor { .. } => None,
        Expr::Var(name) => {
            if is_builtin_constant_symbol(name) {
                Some(PrimitiveType::F32)
            } else {
                locals.get(name).copied()
            }
        }
        Expr::Index { base, .. } => locals.get(base).copied().or(Some(PrimitiveType::F32)),
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Compare { .. } | Expr::Logical { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::Binary { lhs, rhs, .. } => {
            let l = infer_specialized_expr_return_type(lhs, locals, registry)?
                .unwrap_or(PrimitiveType::F32);
            let r = infer_specialized_expr_return_type(rhs, locals, registry)?
                .unwrap_or(PrimitiveType::F32);
            merge_inferred_def_return_types(l, r)
        }
        Expr::Call { func, args } => {
            let mut arg_tys = Vec::<PrimitiveType>::new();
            for arg in args {
                arg_tys.push(
                    infer_specialized_expr_return_type(arg, locals, registry)?
                        .unwrap_or(PrimitiveType::F32),
                );
            }
            match func {
                BuiltinFn::Abs => arg_tys.first().copied().or(Some(PrimitiveType::F32)),
                BuiltinFn::Min | BuiltinFn::Max => {
                    let lhs = arg_tys.first().copied().unwrap_or(PrimitiveType::F32);
                    let rhs = arg_tys.get(1).copied().unwrap_or(PrimitiveType::F32);
                    merge_inferred_def_return_types(lhs, rhs)
                }
                BuiltinFn::Pow => Some(if arg_tys.iter().any(|t| *t == PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
                _ => Some(if arg_tys.iter().any(|t| *t == PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            if parse_data_len_instance_base(name).is_some()
                || parse_buffer_chans_instance_base(name).is_some()
            {
                return Ok(Some(PrimitiveType::I32));
            }
            if matches!(
                name.as_str(),
                "__omni_buffer_read2" | "__omni_buffer_write2" | "unsafe_read" | "unsafe_write"
            ) {
                if let Some(CallArg {
                    expr: Expr::Var(base),
                    ..
                }) = args.first()
                {
                    if let Some(ty) = locals.get(base).copied() {
                        return Ok(Some(ty));
                    }
                }
                return Ok(Some(PrimitiveType::F32));
            }
            let Some(param_names) = registry.param_names.get(name).cloned() else {
                return Ok(Some(PrimitiveType::F32));
            };
            let Some(param_defaults) = registry.param_defaults.get(name).cloned() else {
                return Ok(Some(PrimitiveType::F32));
            };
            let Some(param_kinds) = registry.param_kinds.get(name).cloned() else {
                return Ok(Some(PrimitiveType::F32));
            };
            let forbid_self_named = param_names.first().map(String::as_str) == Some("self");
            let resolved = resolve_call_args_codegen(
                args,
                &param_names,
                &param_defaults,
                forbid_self_named,
                &format!("function '{name}' call in return type inference"),
            )
            .unwrap_or_else(|_| vec![None; param_names.len()]);
            let mut scalar_types = Vec::<PrimitiveType>::new();
            let mut buffer_types = Vec::<(PrimitiveType, TypedBufferChannels)>::new();
            for (idx, kind) in param_kinds.iter().enumerate() {
                match kind {
                    TypedFnParam::Scalar => {
                        let resolved_arg = resolved.get(idx).copied().flatten();
                        let ty = if let Some(arg_expr) = resolved_arg {
                            infer_specialized_expr_return_type(arg_expr, locals, registry)?
                                .unwrap_or(PrimitiveType::F32)
                        } else if let Some(default_expr) =
                            param_defaults.get(idx).and_then(|d| d.as_ref())
                        {
                            infer_specialized_expr_return_type(default_expr, locals, registry)?
                                .unwrap_or(PrimitiveType::F32)
                        } else {
                            PrimitiveType::F32
                        };
                        scalar_types.push(ty);
                    }
                    TypedFnParam::Buffer { elem_ty, channels } => {
                        buffer_types.push((*elem_ty, channels.clone()));
                    }
                    TypedFnParam::Struct { .. } => {}
                }
            }
            let explicit_type_args = resolve_explicit_call_type_args_for_codegen(
                name,
                "return type inference",
                type_args,
            )?;
            apply_explicit_generic_type_args_for_call(
                registry,
                name,
                &explicit_type_args,
                &mut scalar_types,
                "return type inference",
            )?;
            Some(infer_specialized_def_return_type(
                name,
                &scalar_types,
                &buffer_types,
                &explicit_type_args,
                registry,
            )?)
        }
    })
}

fn infer_specialized_stmt_returns(
    stmts: &[Stmt],
    locals: &mut HashMap<String, PrimitiveType>,
    registry: &mut UserFnRegistry,
    out: &mut Vec<PrimitiveType>,
) -> Result<(), Diagnostic> {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                decl_ty,
                expr,
                ..
            } => {
                if name.contains('.') || matches!(expr, Expr::DataCtor { .. }) {
                    continue;
                }
                let inferred = infer_specialized_expr_return_type(expr, locals, registry)?
                    .unwrap_or(PrimitiveType::F32);
                let target_ty = (*decl_ty)
                    .or_else(|| locals.get(name).copied())
                    .unwrap_or(inferred);
                locals.entry(name.clone()).or_insert(target_ty);
            }
            Stmt::Assign {
                target: AssignTarget::Index { .. },
                ..
            }
            | Stmt::Expr { .. } => {}
            Stmt::Return { expr, .. } => {
                let ty = infer_specialized_expr_return_type(expr, locals, registry)?
                    .unwrap_or(PrimitiveType::F32);
                out.push(ty);
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                let mut then_locals = locals.clone();
                let mut else_locals = locals.clone();
                infer_specialized_stmt_returns(then_branch, &mut then_locals, registry, out)?;
                infer_specialized_stmt_returns(else_branch, &mut else_locals, registry, out)?;
                let mut merged = locals.clone();
                for (name, then_ty) in &then_locals {
                    if let Some(else_ty) = else_locals.get(name) {
                        if then_ty == else_ty {
                            merged.insert(name.clone(), *then_ty);
                        }
                    }
                }
                *locals = merged;
            }
            Stmt::For { var, body, .. } => {
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone(), PrimitiveType::I32);
                infer_specialized_stmt_returns(body, &mut loop_locals, registry, out)?;
            }
        }
    }
    Ok(())
}

fn infer_specialized_def_return_type(
    name: &str,
    scalar_types: &[PrimitiveType],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
    registry: &mut UserFnRegistry,
) -> Result<PrimitiveType, Diagnostic> {
    let key = user_fn_mono_key(name, scalar_types, buffer_types, generic_type_args);
    if let Some(ret_ty) = registry.mono_return_tys.get(&key).copied() {
        return Ok(ret_ty);
    }
    if !registry.return_in_progress.insert(key.clone()) {
        return Ok(registry
            .base_return_tys
            .get(name)
            .copied()
            .unwrap_or(PrimitiveType::F32));
    }

    let out = (|| -> Result<PrimitiveType, Diagnostic> {
        let def = registry
            .defs
            .get(name)
            .ok_or_else(|| {
                Diagnostic::internal(format!("unknown function '{}' for return inference", name))
            })?
            .clone();

        let mut locals = HashMap::<String, PrimitiveType>::new();
        let mut scalar_idx = 0usize;
        for (param_name, kind) in def.params.iter().zip(def.param_kinds.iter()) {
            if matches!(kind, TypedFnParam::Scalar) {
                let param_ty = scalar_types
                    .get(scalar_idx)
                    .copied()
                    .unwrap_or(PrimitiveType::F32);
                scalar_idx += 1;
                locals.insert(param_name.clone(), param_ty);
            }
        }

        let mut returns = Vec::<PrimitiveType>::new();
        infer_specialized_stmt_returns(&def.body, &mut locals, registry, &mut returns)?;
        let mut it = returns.into_iter();
        let Some(mut ret_ty) = it.next() else {
            return Ok(registry
                .base_return_tys
                .get(name)
                .copied()
                .unwrap_or(PrimitiveType::F32));
        };
        for ty in it {
            let Some(merged) = merge_inferred_def_return_types(ret_ty, ty) else {
                return Ok(PrimitiveType::F32);
            };
            ret_ty = merged;
        }
        Ok(ret_ty)
    })();

    registry.return_in_progress.remove(&key);
    let ret_ty = out?;
    registry.mono_return_tys.insert(key, ret_ty);
    Ok(ret_ty)
}

unsafe fn ensure_user_fn_specialization(
    module: LLVMModuleRef,
    context: LLVMContextRef,
    registry: &mut UserFnRegistry,
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    sample_rate: f32,
    block_size: usize,
    name: &str,
    scalar_types: &[PrimitiveType],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
) -> Result<(LLVMValueRef, LLVMTypeRef, PrimitiveType), Diagnostic> {
    let def = registry
        .defs
        .get(name)
        .ok_or_else(|| Diagnostic::internal(format!("unknown function '{}'", name)))?
        .clone();

    let scalar_count = def
        .param_kinds
        .iter()
        .filter(|k| matches!(k, TypedFnParam::Scalar))
        .count();
    if scalar_types.len() != scalar_count {
        return Err(Diagnostic::internal(format!(
            "function '{}' scalar signature length mismatch: expected {}, got {}",
            name,
            scalar_count,
            scalar_types.len()
        )));
    }
    let buffer_count = def
        .param_kinds
        .iter()
        .filter(|k| matches!(k, TypedFnParam::Buffer { .. }))
        .count();
    if buffer_types.len() != buffer_count {
        return Err(Diagnostic::internal(format!(
            "function '{}' buffer signature length mismatch: expected {}, got {}",
            name,
            buffer_count,
            buffer_types.len()
        )));
    }

    let default_scalar_sig = default_scalar_signature(&def);
    let default_buffer_sig = default_buffer_signature(&def);
    if generic_type_args.is_empty()
        && scalar_types == default_scalar_sig.as_slice()
        && buffer_types == default_buffer_sig.as_slice()
    {
        let fn_ref = *registry.refs.get(name).ok_or_else(|| {
            Diagnostic::internal(format!("missing base function ref for '{}'", name))
        })?;
        let fn_ty = *registry.tys.get(name).ok_or_else(|| {
            Diagnostic::internal(format!("missing base function type for '{}'", name))
        })?;
        let ret_ty = registry
            .base_return_tys
            .get(name)
            .copied()
            .unwrap_or(PrimitiveType::F32);
        return Ok((fn_ref, fn_ty, ret_ty));
    }

    let key = user_fn_mono_key(name, scalar_types, buffer_types, generic_type_args);
    if let (Some(fn_ref), Some(fn_ty), Some(ret_ty)) = (
        registry.mono_refs.get(&key),
        registry.mono_tys.get(&key),
        registry.mono_return_tys.get(&key),
    ) {
        return Ok((*fn_ref, *fn_ty, *ret_ty));
    }

    let ret_ty = infer_specialized_def_return_type(
        name,
        scalar_types,
        buffer_types,
        generic_type_args,
        registry,
    )?;
    let float_ty = LLVMFloatTypeInContext(context);
    let float_ptr_ty = LLVMPointerType(float_ty, 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let i8_ty = LLVMInt8TypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let by_ref_flags = registry.param_by_ref.get(name).ok_or_else(|| {
        Diagnostic::internal(format!("missing by-ref metadata for function '{}'", name))
    })?;

    let mut arg_tys = Vec::new();
    let mut scalar_idx = 0usize;
    for (param_idx, kind) in def.param_kinds.iter().enumerate() {
        match kind {
            TypedFnParam::Scalar => {
                let param_ty = scalar_types[scalar_idx];
                scalar_idx += 1;
                arg_tys.push(llvm_ty_for_primitive(context, param_ty));
            }
            TypedFnParam::Struct { struct_name } => {
                let fields = struct_fields.get(struct_name).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "unknown struct '{}' in function '{}' parameter lowering",
                        struct_name, def.name
                    ))
                })?;
                for field in fields {
                    match field.ty {
                        TypedFieldType::Scalar(prim) => {
                            if by_ref_flags[param_idx] {
                                arg_tys.push(float_ptr_ty);
                            } else {
                                arg_tys.push(llvm_ty_for_primitive(context, prim));
                            }
                        }
                        TypedFieldType::Data(len) => {
                            if let Some(elem_struct) = &field.data_elem_struct {
                                let mut roots = Vec::new();
                                let mut leaves = Vec::new();
                                collect_data_struct_bindings(
                                    struct_fields,
                                    elem_struct,
                                    &format!("{}.{}", def.params[param_idx], field.name),
                                    len,
                                    &mut roots,
                                    &mut leaves,
                                    &mut Vec::new(),
                                )?;
                                for _ in &leaves {
                                    arg_tys.push(float_ptr_ty);
                                }
                            } else {
                                arg_tys.push(float_ptr_ty);
                            }
                        }
                    }
                }
            }
            TypedFnParam::Buffer { .. } => {
                arg_tys.push(i8_ptr_ty);
                arg_tys.push(i32_ty);
                arg_tys.push(i32_ty);
            }
        }
    }

    let ret_llvm_ty = llvm_ty_for_primitive(context, ret_ty);
    let fn_ty = LLVMFunctionType(ret_llvm_ty, arg_tys.as_mut_ptr(), arg_tys.len() as u32, 0);
    let symbol = mangle_user_fn_symbol_mono(name, scalar_types, buffer_types, generic_type_args)?;
    let fn_ref = LLVMAddFunction(module, symbol.as_ptr(), fn_ty);
    if fn_ref.is_null() {
        return Err(Diagnostic::internal(format!(
            "failed to add monomorphized function '{}'",
            key
        )));
    }
    if is_proc_glue_function_name(name) {
        set_internal_alwaysinline(fn_ref, context)?;
    }

    registry.mono_refs.insert(key.clone(), fn_ref);
    registry.mono_tys.insert(key.clone(), fn_ty);
    registry.mono_return_tys.insert(key.clone(), ret_ty);
    if registry.in_progress.insert(key.clone()) {
        lower_user_function_body(
            &def,
            module,
            context,
            registry,
            struct_fields,
            sample_rate,
            block_size,
            fn_ref,
            ret_ty,
            scalar_types,
            buffer_types,
        )?;
        registry.in_progress.remove(&key);
    }

    Ok((fn_ref, fn_ty, ret_ty))
}

unsafe fn lower_user_function_body(
    def: &TypedFunction,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    registry: &mut UserFnRegistry,
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    sample_rate: f32,
    block_size: usize,
    fn_ref: LLVMValueRef,
    return_ty: PrimitiveType,
    scalar_param_types: &[PrimitiveType],
    buffer_param_types: &[(PrimitiveType, TypedBufferChannels)],
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i32_ty = LLVMInt32TypeInContext(context);
    let return_llvm_ty = llvm_ty_for_primitive(context, return_ty);

    let entry_name =
        CString::new("entry").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let entry = LLVMAppendBasicBlockInContext(context, fn_ref, entry_name.as_ptr());
    let ret_block = LLVMAppendBasicBlockInContext(context, fn_ref, b"def_ret\0".as_ptr().cast());
    let builder = LLVMCreateBuilderInContext(context);
    if builder.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM builder"));
    }

    let result = (|| -> Result<(), Diagnostic> {
        LLVMPositionBuilderAtEnd(builder, entry);
        let zero_ret = llvm_zero_for_primitive(context, return_ty);

        let ret_name =
            CString::new("ret").map_err(|_| Diagnostic::internal("invalid local variable name"))?;
        let return_slot = LLVMBuildAlloca(builder, return_llvm_ty, ret_name.as_ptr());
        LLVMBuildStore(builder, zero_ret, return_slot);

        let mut ctx = DefLoweringCtx {
            builder,
            context,
            module,
            fn_ref,
            float_ty,
            i32_ty,
            sample_rate,
            block_size: block_size as f32,
            return_ty,
            return_slot,
            return_block: ret_block,
            local_slots: HashMap::new(),
            local_data_aliases: HashMap::new(),
            buffer_params: HashMap::new(),
            data_ptrs: HashMap::new(),
            data_len: HashMap::new(),
            data_elem_ty: HashMap::new(),
            data_struct_roots: HashMap::new(),
            struct_fields,
            user_fn_param_names: &registry.param_names,
            user_fn_param_defaults: &registry.param_defaults,
            user_fn_param_kinds: &registry.param_kinds,
            user_fn_param_by_ref: &registry.param_by_ref,
            user_registry: registry as *const UserFnRegistry,
        };

        let by_ref_flags = registry.param_by_ref.get(&def.name).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing by-ref metadata for user function '{}'",
                def.name
            ))
        })?;
        let expected_scalar_count = def
            .param_kinds
            .iter()
            .filter(|k| matches!(k, TypedFnParam::Scalar))
            .count();
        if scalar_param_types.len() != expected_scalar_count {
            return Err(Diagnostic::internal(format!(
                "function '{}' scalar parameter type mismatch: expected {}, got {}",
                def.name,
                expected_scalar_count,
                scalar_param_types.len()
            )));
        }
        let expected_buffer_count = def
            .param_kinds
            .iter()
            .filter(|k| matches!(k, TypedFnParam::Buffer { .. }))
            .count();
        if buffer_param_types.len() != expected_buffer_count {
            return Err(Diagnostic::internal(format!(
                "function '{}' buffer parameter type mismatch: expected {}, got {}",
                def.name,
                expected_buffer_count,
                buffer_param_types.len()
            )));
        }

        let mut llvm_param_idx: u32 = 0;
        let mut scalar_param_idx: usize = 0;
        let mut buffer_param_idx: usize = 0;
        for (param_idx, (param_name, kind)) in
            def.params.iter().zip(def.param_kinds.iter()).enumerate()
        {
            match kind {
                TypedFnParam::Scalar => {
                    let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                    if param_val.is_null() {
                        return Err(Diagnostic::internal(format!(
                            "missing LLVM param {} for function '{}'",
                            llvm_param_idx, def.name
                        )));
                    }
                    let param_ty = scalar_param_types[scalar_param_idx];
                    scalar_param_idx += 1;
                    let slot = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, param_ty),
                        &format!("p_{param_name}"),
                    )?;
                    LLVMBuildStore(ctx.builder, param_val, slot);
                    ctx.local_slots.insert(
                        param_name.clone(),
                        DefLocalSlot {
                            ptr: slot,
                            ty: param_ty,
                        },
                    );
                    llvm_param_idx += 1;
                }
                TypedFnParam::Struct { struct_name } => {
                    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "unknown struct '{}' used by function '{}'",
                            struct_name, def.name
                        ))
                    })?;
                    for field in fields {
                        let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                        if param_val.is_null() {
                            return Err(Diagnostic::internal(format!(
                                "missing LLVM param {} for function '{}'",
                                llvm_param_idx, def.name
                            )));
                        }
                        let flat = format!("{param_name}.{}", field.name);
                        match field.ty {
                            TypedFieldType::Scalar(prim) => {
                                if by_ref_flags[param_idx] {
                                    ctx.local_slots.insert(
                                        flat,
                                        DefLocalSlot {
                                            ptr: param_val,
                                            ty: prim,
                                        },
                                    );
                                } else {
                                    let slot = build_local_slot(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, prim),
                                        &format!("p_{flat}"),
                                    )?;
                                    LLVMBuildStore(ctx.builder, param_val, slot);
                                    ctx.local_slots.insert(
                                        flat,
                                        DefLocalSlot {
                                            ptr: slot,
                                            ty: prim,
                                        },
                                    );
                                }
                            }
                            TypedFieldType::Data(len) => {
                                if let Some(elem_struct) = &field.data_elem_struct {
                                    let mut roots = Vec::new();
                                    let mut leaves = Vec::new();
                                    collect_data_struct_bindings(
                                        ctx.struct_fields,
                                        elem_struct,
                                        &flat,
                                        len,
                                        &mut roots,
                                        &mut leaves,
                                        &mut Vec::new(),
                                    )?;
                                    for (root_name, root_struct, root_len) in roots {
                                        ctx.data_struct_roots
                                            .insert(root_name.clone(), root_struct);
                                        ctx.data_len.entry(root_name).or_insert(root_len);
                                    }
                                    let mut leaf_iter = leaves.into_iter();
                                    let first = leaf_iter.next().ok_or_else(|| {
                                        Diagnostic::internal(format!(
                                            "Data[Struct] field '{flat}' produced no leaf bindings in def lowering"
                                        ))
                                    })?;
                                    ctx.data_ptrs.insert(first.0.clone(), param_val);
                                    ctx.data_len.insert(first.0.clone(), first.1);
                                    ctx.data_elem_ty.insert(first.0, first.2);
                                    for (leaf_name, leaf_len, leaf_ty) in leaf_iter {
                                        llvm_param_idx += 1;
                                        let leaf_param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                                        if leaf_param_val.is_null() {
                                            return Err(Diagnostic::internal(format!(
                                                "missing LLVM param {} for function '{}'",
                                                llvm_param_idx, def.name
                                            )));
                                        }
                                        ctx.data_ptrs.insert(leaf_name.clone(), leaf_param_val);
                                        ctx.data_len.insert(leaf_name.clone(), leaf_len);
                                        ctx.data_elem_ty.insert(leaf_name, leaf_ty);
                                    }
                                } else {
                                    ctx.data_ptrs.insert(flat.clone(), param_val);
                                    ctx.data_len.insert(flat.clone(), len);
                                    ctx.data_elem_ty.insert(
                                        flat,
                                        field.data_elem_ty.unwrap_or(PrimitiveType::F32),
                                    );
                                }
                            }
                        }
                        llvm_param_idx += 1;
                    }
                }
                TypedFnParam::Buffer { .. } => {
                    let (elem_ty, channels) = buffer_param_types
                        .get(buffer_param_idx)
                        .cloned()
                        .ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "missing buffer signature for '{}' parameter '{}' at index {}",
                                def.name, param_name, buffer_param_idx
                            ))
                        })?;
                    buffer_param_idx += 1;
                    let ptr_val = LLVMGetParam(fn_ref, llvm_param_idx);
                    let frames_val = LLVMGetParam(fn_ref, llvm_param_idx + 1);
                    let channels_val = LLVMGetParam(fn_ref, llvm_param_idx + 2);
                    if ptr_val.is_null() || frames_val.is_null() || channels_val.is_null() {
                        return Err(Diagnostic::internal(format!(
                            "missing LLVM buffer param tuple at {} for function '{}'",
                            llvm_param_idx, def.name
                        )));
                    }
                    ctx.buffer_params.insert(
                        param_name.clone(),
                        DefBufferParamInfo {
                            ptr: ptr_val,
                            frames: frames_val,
                            channels: channels_val,
                            elem_ty,
                            declared_channels: channels,
                        },
                    );
                    llvm_param_idx += 3;
                }
            }
        }

        let mut terminated = false;
        for stmt in &def.body {
            if lower_def_stmt(stmt, &mut ctx)? {
                terminated = true;
                break;
            }
        }

        if !terminated && !current_block_terminated(ctx.builder) {
            LLVMBuildBr(ctx.builder, ctx.return_block);
        }

        LLVMPositionBuilderAtEnd(ctx.builder, ctx.return_block);
        let ret_value = LLVMBuildLoad2(
            ctx.builder,
            return_llvm_ty,
            ctx.return_slot,
            b"ret_v\0".as_ptr().cast(),
        );
        LLVMBuildRet(ctx.builder, ret_value);
        Ok(())
    })();

    LLVMDisposeBuilder(builder);
    result
}

unsafe fn lower_def_stmt(stmt: &Stmt, ctx: &mut DefLoweringCtx<'_>) -> Result<bool, Diagnostic> {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            is_typed_decl,
            expr,
            ..
        } => match target {
            AssignTarget::Var(target_name) => {
                if let Expr::DataCtor { spec, init } = expr {
                    if *is_typed_decl {
                        if decl_ty.is_some() {
                            return Err(Diagnostic::internal(
                                "typed array declaration cannot include scalar declaration type in def lowering",
                            ));
                        }
                        if ctx.local_slots.contains_key(target_name)
                            || ctx.data_ptrs.contains_key(target_name)
                        {
                            return Err(Diagnostic::internal(format!(
                                "typed array declaration for '{target_name}' conflicts with existing local symbol in def lowering"
                            )));
                        }
                        let elem_ty = match spec.elem {
                            omni_frontend::DataElemType::Primitive(elem_ty) => elem_ty,
                            omni_frontend::DataElemType::Struct(ref name) => {
                                return Err(Diagnostic::internal(format!(
                                    "typed array declaration '{target_name}: {name}[N]' is not yet supported in def lowering"
                                )))
                            }
                        };
                        let len =
                            eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
                        let ptr = build_local_array_slot(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, elem_ty),
                            len,
                            &format!("d_{target_name}"),
                        )?;
                        if let Some(values) = init {
                            if values.len() != len {
                                return Err(Diagnostic::internal(format!(
                                    "typed array declaration '{target_name}' initializer expects {len} elements, got {}",
                                    values.len()
                                )));
                            }
                            for (idx, value_expr) in values.iter().enumerate() {
                                let typed = lower_def_expr(value_expr, ctx)?;
                                let casted = cast_def_value_to(
                                    ctx,
                                    typed,
                                    elem_ty,
                                    b"def_local_arr_init_cast\0",
                                );
                                let idx_v = LLVMConstInt(ctx.i32_ty, idx as u64, 0);
                                let elem_ptr = build_f32_ptr_offset(
                                    ctx.builder,
                                    llvm_ty_for_primitive(ctx.context, elem_ty),
                                    ptr,
                                    idx_v,
                                    b"def_local_arr_init_ptr\0",
                                );
                                LLVMBuildStore(ctx.builder, casted, elem_ptr);
                            }
                        }
                        ctx.data_ptrs.insert(target_name.clone(), ptr);
                        ctx.data_len.insert(target_name.clone(), len);
                        ctx.data_elem_ty.insert(target_name.clone(), elem_ty);
                        return Ok(false);
                    } else {
                        return Err(Diagnostic::internal(
                            "Data constructor assignment in def lowering requires typed array declaration syntax",
                        ));
                    }
                }

                if !ctx.local_slots.contains_key(target_name)
                    && !ctx.local_data_aliases.contains_key(target_name)
                    && !ctx.data_ptrs.contains_key(target_name)
                {
                    if let Expr::Index { base, index } = expr {
                        if let Some(struct_name) = ctx.data_struct_roots.get(base).cloned() {
                            let root_len = *ctx.data_len.get(base).ok_or_else(|| {
                                Diagnostic::internal(format!(
                                    "missing Data[Struct] length metadata for '{base}' in def lowering"
                                ))
                            })?;
                            let raw_index = lower_def_expr(index, ctx)?;
                            let index_i32 = cast_def_value_to(
                                ctx,
                                raw_index,
                                PrimitiveType::I32,
                                b"def_data_alias_idx_i32\0",
                            );
                            let clamped =
                                clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, root_len)?;
                            bind_struct_data_element_aliases_in_def(
                                target_name,
                                &struct_name,
                                base,
                                clamped,
                                ctx,
                            )?;
                            return Ok(false);
                        }
                        if let Some(alias) = ctx.local_data_aliases.get(base).cloned() {
                            match alias {
                                LocalDataAlias::Primitive { .. } => {
                                    return Err(Diagnostic::internal(format!(
                                        "local alias binding '{target_name} = {base}[...]' is not supported for primitive arrays in def lowering; use direct indexed access"
                                    )));
                                }
                                LocalDataAlias::Struct {
                                    root_base,
                                    elem_struct,
                                    len,
                                    start_index,
                                } => {
                                    let raw_index = lower_def_expr(index, ctx)?;
                                    let index_i32 = cast_def_value_to(
                                        ctx,
                                        raw_index,
                                        PrimitiveType::I32,
                                        b"def_data_alias_local_idx_i32\0",
                                    );
                                    let clamped_local =
                                        clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, len)?;
                                    let global_index = LLVMBuildAdd(
                                        ctx.builder,
                                        start_index,
                                        clamped_local,
                                        b"def_data_alias_global_idx\0".as_ptr().cast(),
                                    );
                                    bind_struct_data_element_aliases_in_def(
                                        target_name,
                                        &elem_struct,
                                        &root_base,
                                        global_index,
                                        ctx,
                                    )?;
                                    return Ok(false);
                                }
                            }
                        }
                        if ctx.data_ptrs.contains_key(base) {
                            return Err(Diagnostic::internal(format!(
                                "local alias binding '{target_name} = {base}[...]' is not supported for primitive arrays in def lowering; use direct indexed access"
                            )));
                        }
                    }
                }

                let typed_value = lower_def_expr(expr, ctx)?;
                if let Some(local) = ctx.local_slots.get(target_name).copied() {
                    let casted = cast_def_value_to(ctx, typed_value, local.ty, b"def_store_cast\0");
                    LLVMBuildStore(ctx.builder, casted, local.ptr);
                    return Ok(false);
                }
                if ctx.data_ptrs.contains_key(target_name) {
                    return Err(Diagnostic::internal(format!(
                        "Data symbol '{target_name}' must be assigned via index syntax in def lowering"
                    )));
                }
                if ctx.local_data_aliases.contains_key(target_name) {
                    return Err(Diagnostic::internal(format!(
                        "Data alias '{target_name}' must be assigned via index syntax in def lowering"
                    )));
                }
                let target_ty = decl_ty.unwrap_or(typed_value.ty);
                let slot = build_local_slot(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, target_ty),
                    &format!("v_{target_name}"),
                )?;
                let casted =
                    cast_def_value_to(ctx, typed_value, target_ty, b"def_store_new_cast\0");
                LLVMBuildStore(ctx.builder, casted, slot);
                ctx.local_slots.insert(
                    target_name.clone(),
                    DefLocalSlot {
                        ptr: slot,
                        ty: target_ty,
                    },
                );
                Ok(false)
            }
            AssignTarget::Index { base, index } => {
                let typed_value = lower_def_expr(expr, ctx)?;
                let data = lower_def_data_element_ptr(ctx, base, index, true)?;
                let value =
                    cast_def_value_to(ctx, typed_value, data.elem_ty, b"def_data_store_cast\0");
                LLVMBuildStore(ctx.builder, value, data.ptr);
                Ok(false)
            }
        },
        Stmt::Expr { expr, .. } => {
            let _ = lower_def_expr(expr, ctx)?;
            Ok(false)
        }
        Stmt::Return { expr, .. } => {
            let value = lower_def_expr(expr, ctx)?;
            let ret_v = cast_def_value_to(ctx, value, ctx.return_ty, b"def_ret_cast\0");
            LLVMBuildStore(ctx.builder, ret_v, ctx.return_slot);
            LLVMBuildBr(ctx.builder, ctx.return_block);
            Ok(true)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let cond_value = lower_def_expr(cond, ctx)?;
            let cond_bool = lower_def_condition(ctx, cond_value, b"def_if_cond\0");

            let then_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"def_if_then\0".as_ptr().cast(),
            );
            let else_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"def_if_else\0".as_ptr().cast(),
            );
            let merge_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"def_if_merge\0".as_ptr().cast(),
            );

            LLVMBuildCondBr(ctx.builder, cond_bool, then_bb, else_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, then_bb);
            let mut then_terminated = false;
            for nested in then_branch {
                if lower_def_stmt(nested, ctx)? {
                    then_terminated = true;
                    break;
                }
            }
            if !then_terminated {
                LLVMBuildBr(ctx.builder, merge_bb);
            }

            LLVMPositionBuilderAtEnd(ctx.builder, else_bb);
            let mut else_terminated = false;
            for nested in else_branch {
                if lower_def_stmt(nested, ctx)? {
                    else_terminated = true;
                    break;
                }
            }
            if !else_terminated {
                LLVMBuildBr(ctx.builder, merge_bb);
            }

            LLVMPositionBuilderAtEnd(ctx.builder, merge_bb);
            if then_terminated && else_terminated {
                LLVMBuildBr(ctx.builder, ctx.return_block);
                return Ok(true);
            }
            Ok(false)
        }
        Stmt::For {
            var,
            start,
            end,
            body,
            ..
        } => {
            let preheader_bb = LLVMGetInsertBlock(ctx.builder);
            if preheader_bb.is_null() {
                return Err(Diagnostic::internal(
                    "failed to get def for-loop preheader block",
                ));
            }

            let cond_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"def_for_cond\0".as_ptr().cast(),
            );
            let body_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"def_for_body\0".as_ptr().cast(),
            );
            let end_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"def_for_end\0".as_ptr().cast(),
            );

            let start_value = lower_def_expr(start, ctx)?;
            let start_v =
                cast_def_value_to(ctx, start_value, PrimitiveType::I32, b"def_for_start_i32\0");
            let end_value = lower_def_expr(end, ctx)?;
            let end_v = cast_def_value_to(ctx, end_value, PrimitiveType::I32, b"def_for_end_i32\0");

            LLVMBuildBr(ctx.builder, cond_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, cond_bb);
            let loop_i = LLVMBuildPhi(ctx.builder, ctx.i32_ty, b"def_for_i\0".as_ptr().cast());
            let mut incoming_vals = [start_v];
            let mut incoming_blocks = [preheader_bb];
            LLVMAddIncoming(
                loop_i,
                incoming_vals.as_mut_ptr(),
                incoming_blocks.as_mut_ptr(),
                1,
            );

            let cond = LLVMBuildICmp(
                ctx.builder,
                LLVMIntPredicate::LLVMIntSLT,
                loop_i,
                end_v,
                b"def_for_cmp\0".as_ptr().cast(),
            );
            LLVMBuildCondBr(ctx.builder, cond, body_bb, end_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, body_bb);
            let old_binding = ctx.local_slots.get(var).copied();
            let loop_slot = build_local_slot(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, PrimitiveType::I32),
                &format!("loop_{var}"),
            )?;
            ctx.local_slots.insert(
                var.clone(),
                DefLocalSlot {
                    ptr: loop_slot,
                    ty: PrimitiveType::I32,
                },
            );
            LLVMBuildStore(ctx.builder, loop_i, loop_slot);

            let mut body_terminated = false;
            for nested in body {
                if lower_def_stmt(nested, ctx)? {
                    body_terminated = true;
                    break;
                }
            }

            if let Some(binding) = old_binding {
                ctx.local_slots.insert(var.clone(), binding);
            } else {
                ctx.local_slots.remove(var);
            }

            if !body_terminated {
                let body_end_bb = LLVMGetInsertBlock(ctx.builder);
                if body_end_bb.is_null() {
                    return Err(Diagnostic::internal(
                        "failed to get def for-loop body end block",
                    ));
                }
                let next_i = LLVMBuildAdd(
                    ctx.builder,
                    loop_i,
                    const_i32(ctx.i32_ty, 1),
                    b"def_for_i_next\0".as_ptr().cast(),
                );
                LLVMBuildBr(ctx.builder, cond_bb);
                let mut back_vals = [next_i];
                let mut back_blocks = [body_end_bb];
                LLVMAddIncoming(loop_i, back_vals.as_mut_ptr(), back_blocks.as_mut_ptr(), 1);
            }

            LLVMPositionBuilderAtEnd(ctx.builder, end_bb);
            Ok(false)
        }
    }
}

unsafe fn cast_def_value_to(
    ctx: &DefLoweringCtx<'_>,
    value: OrcValue,
    to: PrimitiveType,
    name: &[u8],
) -> LLVMValueRef {
    if value.ty == to {
        return value.value;
    }
    let from = value.ty;
    let builder = ctx.builder;
    let context = ctx.context;
    let i32_ty = ctx.i32_ty;
    let f32_ty = ctx.float_ty;
    let i64_ty = LLVMInt64TypeInContext(context);
    let f64_ty = LLVMDoubleTypeInContext(context);

    match (from, to) {
        (PrimitiveType::F32, PrimitiveType::F64) => {
            LLVMBuildFPExt(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::F32) => {
            LLVMBuildFPTrunc(builder, value.value, f32_ty, name.as_ptr().cast())
        }

        (PrimitiveType::F32, PrimitiveType::I32) => {
            LLVMBuildFPToSI(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F32, PrimitiveType::I64) => {
            LLVMBuildFPToSI(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F32, PrimitiveType::Bool) => {
            let zero = LLVMConstReal(f32_ty, 0.0);
            LLVMBuildFCmp(
                builder,
                LLVMRealPredicate::LLVMRealONE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::F64, PrimitiveType::I32) => {
            LLVMBuildFPToSI(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::I64) => {
            LLVMBuildFPToSI(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::Bool) => {
            let zero = LLVMConstReal(f64_ty, 0.0);
            LLVMBuildFCmp(
                builder,
                LLVMRealPredicate::LLVMRealONE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::I32, PrimitiveType::F32) => {
            LLVMBuildSIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::F64) => {
            LLVMBuildSIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::I64) => {
            LLVMBuildSExt(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::Bool) => {
            let zero = LLVMConstInt(i32_ty, 0, 0);
            LLVMBuildICmp(
                builder,
                LLVMIntPredicate::LLVMIntNE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::I64, PrimitiveType::F32) => {
            LLVMBuildSIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::F64) => {
            LLVMBuildSIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::I32) => {
            LLVMBuildTrunc(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::Bool) => {
            let zero = LLVMConstInt(i64_ty, 0, 0);
            LLVMBuildICmp(
                builder,
                LLVMIntPredicate::LLVMIntNE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::Bool, PrimitiveType::F32) => {
            LLVMBuildUIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::F64) => {
            LLVMBuildUIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::I32) => {
            LLVMBuildZExt(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::I64) => {
            LLVMBuildZExt(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        _ => value.value,
    }
}

unsafe fn lower_def_condition(
    ctx: &DefLoweringCtx<'_>,
    value: OrcValue,
    name: &[u8],
) -> LLVMValueRef {
    cast_def_value_to(ctx, value, PrimitiveType::Bool, name)
}

unsafe fn lower_def_logical_expr(
    op: LogicalOp,
    lhs: &Expr,
    rhs: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    let lhs_value = lower_def_expr(lhs, ctx)?;
    let lhs_bool = lower_def_condition(ctx, lhs_value, b"def_logical_lhs\0");
    let pre_bb = LLVMGetInsertBlock(ctx.builder);
    if pre_bb.is_null() {
        return Err(Diagnostic::internal(
            "failed to get insertion block for def logical expression",
        ));
    }

    let rhs_bb = LLVMAppendBasicBlockInContext(
        ctx.context,
        ctx.fn_ref,
        b"def_logical_rhs\0".as_ptr().cast(),
    );
    let merge_bb = LLVMAppendBasicBlockInContext(
        ctx.context,
        ctx.fn_ref,
        b"def_logical_merge\0".as_ptr().cast(),
    );

    match op {
        LogicalOp::And => LLVMBuildCondBr(ctx.builder, lhs_bool, rhs_bb, merge_bb),
        LogicalOp::Or => LLVMBuildCondBr(ctx.builder, lhs_bool, merge_bb, rhs_bb),
    };

    LLVMPositionBuilderAtEnd(ctx.builder, rhs_bb);
    let rhs_value = lower_def_expr(rhs, ctx)?;
    let rhs_bool = lower_def_condition(ctx, rhs_value, b"def_logical_rhs_bool\0");
    LLVMBuildBr(ctx.builder, merge_bb);
    let rhs_end_bb = LLVMGetInsertBlock(ctx.builder);
    if rhs_end_bb.is_null() {
        return Err(Diagnostic::internal(
            "failed to get rhs block for def logical expression",
        ));
    }

    LLVMPositionBuilderAtEnd(ctx.builder, merge_bb);
    let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
    let phi = LLVMBuildPhi(ctx.builder, bool_ty, b"def_logical_phi\0".as_ptr().cast());
    let lhs_short = match op {
        LogicalOp::And => LLVMConstInt(bool_ty, 0, 0),
        LogicalOp::Or => LLVMConstInt(bool_ty, 1, 0),
    };
    let mut incoming_vals = [lhs_short, rhs_bool];
    let mut incoming_blocks = [pre_bb, rhs_end_bb];
    LLVMAddIncoming(
        phi,
        incoming_vals.as_mut_ptr(),
        incoming_blocks.as_mut_ptr(),
        incoming_vals.len() as u32,
    );
    Ok(OrcValue {
        value: phi,
        ty: PrimitiveType::Bool,
    })
}

unsafe fn llvm_ty_for_primitive(context: LLVMContextRef, ty: PrimitiveType) -> LLVMTypeRef {
    match ty {
        PrimitiveType::F32 => LLVMFloatTypeInContext(context),
        PrimitiveType::F64 => LLVMDoubleTypeInContext(context),
        PrimitiveType::I32 => LLVMInt32TypeInContext(context),
        PrimitiveType::I64 => LLVMInt64TypeInContext(context),
        PrimitiveType::Bool => LLVMInt1TypeInContext(context),
    }
}

unsafe fn llvm_zero_for_primitive(context: LLVMContextRef, ty: PrimitiveType) -> LLVMValueRef {
    match ty {
        PrimitiveType::F32 => LLVMConstReal(LLVMFloatTypeInContext(context), 0.0),
        PrimitiveType::F64 => LLVMConstReal(LLVMDoubleTypeInContext(context), 0.0),
        PrimitiveType::I32 => LLVMConstInt(LLVMInt32TypeInContext(context), 0, 0),
        PrimitiveType::I64 => LLVMConstInt(LLVMInt64TypeInContext(context), 0, 0),
        PrimitiveType::Bool => LLVMConstInt(LLVMInt1TypeInContext(context), 0, 0),
    }
}

unsafe fn merge_numeric_primitive(lhs: PrimitiveType, rhs: PrimitiveType) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (F64, I32)
        | (I32, F64)
        | (F64, I64)
        | (I64, F64)
        | (F64, F32)
        | (F32, F64)
        | (F64, F64) => Some(F64),
        (F32, I32) | (I32, F32) | (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => None,
    }
}

unsafe fn cast_orc_value_to(
    ctx: &LoweringCtx<'_>,
    value: OrcValue,
    to: PrimitiveType,
    name: &[u8],
) -> LLVMValueRef {
    if value.ty == to {
        return value.value;
    }
    let from = value.ty;
    let builder = ctx.builder;
    let context = ctx.context;
    let i32_ty = ctx.i32_ty;
    let f32_ty = ctx.float_ty;
    let i64_ty = LLVMInt64TypeInContext(context);
    let f64_ty = LLVMDoubleTypeInContext(context);

    match (from, to) {
        (PrimitiveType::F32, PrimitiveType::F64) => {
            LLVMBuildFPExt(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::F32) => {
            LLVMBuildFPTrunc(builder, value.value, f32_ty, name.as_ptr().cast())
        }

        (PrimitiveType::F32, PrimitiveType::I32) => {
            LLVMBuildFPToSI(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F32, PrimitiveType::I64) => {
            LLVMBuildFPToSI(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F32, PrimitiveType::Bool) => {
            let zero = LLVMConstReal(f32_ty, 0.0);
            LLVMBuildFCmp(
                builder,
                LLVMRealPredicate::LLVMRealONE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::F64, PrimitiveType::I32) => {
            LLVMBuildFPToSI(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::I64) => {
            LLVMBuildFPToSI(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::Bool) => {
            let zero = LLVMConstReal(f64_ty, 0.0);
            LLVMBuildFCmp(
                builder,
                LLVMRealPredicate::LLVMRealONE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::I32, PrimitiveType::F32) => {
            LLVMBuildSIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::F64) => {
            LLVMBuildSIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::I64) => {
            LLVMBuildSExt(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::Bool) => {
            let zero = LLVMConstInt(i32_ty, 0, 0);
            LLVMBuildICmp(
                builder,
                LLVMIntPredicate::LLVMIntNE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::I64, PrimitiveType::F32) => {
            LLVMBuildSIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::F64) => {
            LLVMBuildSIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::I32) => {
            LLVMBuildTrunc(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::Bool) => {
            let zero = LLVMConstInt(i64_ty, 0, 0);
            LLVMBuildICmp(
                builder,
                LLVMIntPredicate::LLVMIntNE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::Bool, PrimitiveType::F32) => {
            LLVMBuildUIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::F64) => {
            LLVMBuildUIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::I32) => {
            LLVMBuildZExt(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::I64) => {
            LLVMBuildZExt(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        _ => value.value,
    }
}

unsafe fn lower_orc_condition(ctx: &LoweringCtx<'_>, value: OrcValue, name: &[u8]) -> LLVMValueRef {
    cast_orc_value_to(ctx, value, PrimitiveType::Bool, name)
}

unsafe fn lower_orc_logical_expr(
    op: LogicalOp,
    lhs: &Expr,
    rhs: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<OrcValue, Diagnostic> {
    let lhs_value = lower_expr(lhs, ctx, locals, local_aliases, local_data_aliases)?;
    let lhs_bool = lower_orc_condition(ctx, lhs_value, b"logical_lhs\0");
    let pre_bb = LLVMGetInsertBlock(ctx.builder);
    if pre_bb.is_null() {
        return Err(Diagnostic::internal(
            "failed to get insertion block for logical expression",
        ));
    }

    let rhs_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"logical_rhs\0".as_ptr().cast());
    let merge_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"logical_merge\0".as_ptr().cast());

    match op {
        LogicalOp::And => LLVMBuildCondBr(ctx.builder, lhs_bool, rhs_bb, merge_bb),
        LogicalOp::Or => LLVMBuildCondBr(ctx.builder, lhs_bool, merge_bb, rhs_bb),
    };

    LLVMPositionBuilderAtEnd(ctx.builder, rhs_bb);
    let rhs_value = lower_expr(rhs, ctx, locals, local_aliases, local_data_aliases)?;
    let rhs_bool = lower_orc_condition(ctx, rhs_value, b"logical_rhs_bool\0");
    LLVMBuildBr(ctx.builder, merge_bb);
    let rhs_end_bb = LLVMGetInsertBlock(ctx.builder);
    if rhs_end_bb.is_null() {
        return Err(Diagnostic::internal(
            "failed to get rhs block for logical expression",
        ));
    }

    LLVMPositionBuilderAtEnd(ctx.builder, merge_bb);
    let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
    let phi = LLVMBuildPhi(ctx.builder, bool_ty, b"logical_phi\0".as_ptr().cast());
    let lhs_short = match op {
        LogicalOp::And => LLVMConstInt(bool_ty, 0, 0),
        LogicalOp::Or => LLVMConstInt(bool_ty, 1, 0),
    };
    let mut incoming_vals = [lhs_short, rhs_bool];
    let mut incoming_blocks = [pre_bb, rhs_end_bb];
    LLVMAddIncoming(
        phi,
        incoming_vals.as_mut_ptr(),
        incoming_blocks.as_mut_ptr(),
        incoming_vals.len() as u32,
    );
    Ok(OrcValue {
        value: phi,
        ty: PrimitiveType::Bool,
    })
}

unsafe fn lower_def_expr(
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    match expr {
        Expr::Number(value) => Ok(OrcValue {
            value: LLVMConstReal(ctx.float_ty, *value as f64),
            ty: PrimitiveType::F32,
        }),
        Expr::ArrayLiteral(_) => Err(Diagnostic::internal(
            "array literal is not a scalar expression in def lowering",
        )),
        Expr::Int(value) => {
            if *value >= i32::MIN as i64 && *value <= i32::MAX as i64 {
                Ok(OrcValue {
                    value: const_i32(ctx.i32_ty, *value as i32),
                    ty: PrimitiveType::I32,
                })
            } else {
                let i64_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::I64);
                Ok(OrcValue {
                    value: LLVMConstInt(i64_ty, *value as u64, 1),
                    ty: PrimitiveType::I64,
                })
            }
        }
        Expr::Bool(value) => {
            let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
            Ok(OrcValue {
                value: LLVMConstInt(bool_ty, if *value { 1 } else { 0 }, 0),
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Var(name) => {
            if let Some(value) = builtin_constant_value(name, ctx.sample_rate, ctx.block_size) {
                return Ok(OrcValue {
                    value: LLVMConstReal(ctx.float_ty, value as f64),
                    ty: PrimitiveType::F32,
                });
            }
            if ctx.buffer_params.contains_key(name) {
                return Err(Diagnostic::internal(format!(
                    "buffer symbol '{name}' must be indexed in def lowering"
                )));
            }
            let local = ctx.local_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("unknown local '{name}' in def lowering"))
            })?;
            let loaded = LLVMBuildLoad2(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, local.ty),
                local.ptr,
                b"def_load\0".as_ptr().cast(),
            );
            Ok(OrcValue {
                value: loaded,
                ty: local.ty,
            })
        }
        Expr::Binary { op, lhs, rhs } => {
            let left = lower_def_expr(lhs, ctx)?;
            let right = lower_def_expr(rhs, ctx)?;
            let Some(result_ty) = merge_numeric_primitive(left.ty, right.ty) else {
                return Err(Diagnostic::internal(format!(
                    "binary op requires numeric operands, got {:?} and {:?}",
                    left.ty, right.ty
                )));
            };
            let left_v = cast_def_value_to(ctx, left, result_ty, b"def_bin_lhs_cast\0");
            let right_v = cast_def_value_to(ctx, right, result_ty, b"def_bin_rhs_cast\0");
            let value = match result_ty {
                PrimitiveType::F32 | PrimitiveType::F64 => match op {
                    BinaryOp::Add => {
                        LLVMBuildFAdd(ctx.builder, left_v, right_v, b"def_fadd\0".as_ptr().cast())
                    }
                    BinaryOp::Sub => {
                        LLVMBuildFSub(ctx.builder, left_v, right_v, b"def_fsub\0".as_ptr().cast())
                    }
                    BinaryOp::Mul => {
                        LLVMBuildFMul(ctx.builder, left_v, right_v, b"def_fmul\0".as_ptr().cast())
                    }
                    BinaryOp::Div => {
                        LLVMBuildFDiv(ctx.builder, left_v, right_v, b"def_fdiv\0".as_ptr().cast())
                    }
                    BinaryOp::Mod => {
                        LLVMBuildFRem(ctx.builder, left_v, right_v, b"def_frem\0".as_ptr().cast())
                    }
                },
                PrimitiveType::I32 | PrimitiveType::I64 => match op {
                    BinaryOp::Add => {
                        LLVMBuildAdd(ctx.builder, left_v, right_v, b"def_iadd\0".as_ptr().cast())
                    }
                    BinaryOp::Sub => {
                        LLVMBuildSub(ctx.builder, left_v, right_v, b"def_isub\0".as_ptr().cast())
                    }
                    BinaryOp::Mul => {
                        LLVMBuildMul(ctx.builder, left_v, right_v, b"def_imul\0".as_ptr().cast())
                    }
                    BinaryOp::Div => {
                        LLVMBuildSDiv(ctx.builder, left_v, right_v, b"def_idiv\0".as_ptr().cast())
                    }
                    BinaryOp::Mod => {
                        LLVMBuildSRem(ctx.builder, left_v, right_v, b"def_irem\0".as_ptr().cast())
                    }
                },
                PrimitiveType::Bool => {
                    return Err(Diagnostic::internal(
                        "binary op does not support bool operands in def lowering",
                    ));
                }
            };
            Ok(OrcValue {
                value,
                ty: result_ty,
            })
        }
        Expr::Compare { op, lhs, rhs } => {
            let left = lower_def_expr(lhs, ctx)?;
            let right = lower_def_expr(rhs, ctx)?;
            let cmp = if left.ty == PrimitiveType::Bool && right.ty == PrimitiveType::Bool {
                let pred = match op {
                    CmpOp::Eq => LLVMIntPredicate::LLVMIntEQ,
                    CmpOp::Ne => LLVMIntPredicate::LLVMIntNE,
                    _ => {
                        return Err(Diagnostic::internal(
                            "bool comparisons only support == and != in def lowering",
                        ));
                    }
                };
                LLVMBuildICmp(
                    ctx.builder,
                    pred,
                    left.value,
                    right.value,
                    b"def_icmp_bool\0".as_ptr().cast(),
                )
            } else {
                let Some(result_ty) = merge_numeric_primitive(left.ty, right.ty) else {
                    return Err(Diagnostic::internal(format!(
                        "comparison requires compatible operands, got {:?} and {:?}",
                        left.ty, right.ty
                    )));
                };
                let left_v = cast_def_value_to(ctx, left, result_ty, b"def_cmp_lhs_cast\0");
                let right_v = cast_def_value_to(ctx, right, result_ty, b"def_cmp_rhs_cast\0");
                match result_ty {
                    PrimitiveType::F32 | PrimitiveType::F64 => {
                        let pred = match op {
                            CmpOp::Eq => LLVMRealPredicate::LLVMRealOEQ,
                            CmpOp::Ne => LLVMRealPredicate::LLVMRealONE,
                            CmpOp::Lt => LLVMRealPredicate::LLVMRealOLT,
                            CmpOp::Le => LLVMRealPredicate::LLVMRealOLE,
                            CmpOp::Gt => LLVMRealPredicate::LLVMRealOGT,
                            CmpOp::Ge => LLVMRealPredicate::LLVMRealOGE,
                        };
                        LLVMBuildFCmp(
                            ctx.builder,
                            pred,
                            left_v,
                            right_v,
                            b"def_fcmp\0".as_ptr().cast(),
                        )
                    }
                    PrimitiveType::I32 | PrimitiveType::I64 => {
                        let pred = match op {
                            CmpOp::Eq => LLVMIntPredicate::LLVMIntEQ,
                            CmpOp::Ne => LLVMIntPredicate::LLVMIntNE,
                            CmpOp::Lt => LLVMIntPredicate::LLVMIntSLT,
                            CmpOp::Le => LLVMIntPredicate::LLVMIntSLE,
                            CmpOp::Gt => LLVMIntPredicate::LLVMIntSGT,
                            CmpOp::Ge => LLVMIntPredicate::LLVMIntSGE,
                        };
                        LLVMBuildICmp(
                            ctx.builder,
                            pred,
                            left_v,
                            right_v,
                            b"def_icmp\0".as_ptr().cast(),
                        )
                    }
                    PrimitiveType::Bool => unreachable!(),
                }
            };
            Ok(OrcValue {
                value: cmp,
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Cast { to, expr } => {
            let value = lower_def_expr(expr, ctx)?;
            Ok(OrcValue {
                value: cast_def_value_to(ctx, value, *to, b"def_cast\0"),
                ty: *to,
            })
        }
        Expr::UnaryNot { expr } => {
            let value = lower_def_expr(expr, ctx)?;
            let as_bool = cast_def_value_to(ctx, value, PrimitiveType::Bool, b"def_not_bool\0");
            let one = LLVMConstInt(
                llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool),
                1,
                0,
            );
            let not_v = LLVMBuildXor(ctx.builder, as_bool, one, b"def_not\0".as_ptr().cast());
            Ok(OrcValue {
                value: not_v,
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Logical { op, lhs, rhs } => lower_def_logical_expr(*op, lhs, rhs, ctx),
        Expr::Call { func, args } => {
            let mut lowered = Vec::with_capacity(args.len());
            for arg in args {
                lowered.push(lower_def_expr(arg, ctx)?);
            }
            lower_builtin_call_def(ctx, *func, &lowered)
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            if name == "__omni_buffer_read2" {
                return lower_def_buffer_read2_call(args, ctx, true);
            }
            if name == "__omni_buffer_write2" {
                return lower_def_buffer_write2_call(args, ctx, true);
            }
            if let Some(base) = parse_data_len_instance_base(name) {
                return lower_def_data_len_call(name, base, args, ctx);
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                return lower_def_buffer_chans_call(name, base, args, ctx);
            }
            if name == "unsafe_read" {
                return lower_def_unsafe_data_read_call(args, ctx);
            }
            if name == "unsafe_write" {
                return lower_def_unsafe_data_write_call(args, ctx);
            }
            let param_names = ctx.user_fn_param_names.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing parameter metadata for '{name}'"))
            })?;
            let param_defaults = ctx.user_fn_param_defaults.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing parameter default metadata for '{name}'"))
            })?;
            let param_kinds = ctx.user_fn_param_kinds.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing param kind metadata for '{name}'"))
            })?;
            let param_by_ref = ctx.user_fn_param_by_ref.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing by-ref metadata for '{name}'"))
            })?;
            if param_kinds.len() != param_names.len() || param_kinds.len() != param_defaults.len() {
                return Err(Diagnostic::internal(format!(
                    "function '{name}' has inconsistent metadata sizes in def lowering"
                )));
            }
            if param_by_ref.len() != param_kinds.len() {
                return Err(Diagnostic::internal(format!(
                    "function '{name}' by-ref metadata length {} does not match param metadata {} in def lowering",
                    param_by_ref.len(),
                    param_kinds.len()
                )));
            }
            let forbid_self_named = param_names.first().map(String::as_str) == Some("self");
            let resolved = resolve_call_args_codegen(
                args,
                param_names,
                param_defaults,
                forbid_self_named,
                &format!("function '{name}' call in def lowering"),
            )?;

            let mut scalar_values = Vec::new();
            let mut buffer_types = Vec::<(PrimitiveType, TypedBufferChannels)>::new();
            for (idx, kind) in param_kinds.iter().enumerate() {
                let resolved_arg = resolved.get(idx).copied().flatten();
                match kind {
                    TypedFnParam::Scalar => {
                        let value = if let Some(arg_expr) = resolved_arg {
                            lower_def_expr(arg_expr, ctx)?
                        } else {
                            let default_expr = param_defaults
                                .get(idx)
                                .and_then(|d| d.as_ref())
                                .ok_or_else(|| {
                                    Diagnostic::internal(format!(
                                        "function '{name}' missing required argument '{}' in def lowering",
                                        param_names[idx]
                                    ))
                                })?;
                            let default_value = eval_const_default_expr(
                                default_expr,
                                ctx.sample_rate,
                                ctx.block_size,
                            )?;
                            OrcValue {
                                value: LLVMConstReal(ctx.float_ty, default_value as f64),
                                ty: PrimitiveType::F32,
                            }
                        };
                        scalar_values.push(value);
                    }
                    TypedFnParam::Buffer { elem_ty, channels } => {
                        let resolved_ty = if let Some(arg_expr) = resolved_arg {
                            infer_buffer_arg_signature_in_def(ctx, arg_expr, name)?
                        } else {
                            (*elem_ty, channels.clone())
                        };
                        buffer_types.push(resolved_ty);
                    }
                    TypedFnParam::Struct { .. } => {}
                }
            }
            let mut scalar_types = scalar_values.iter().map(|v| v.ty).collect::<Vec<_>>();
            let explicit_type_args =
                resolve_explicit_call_type_args_for_codegen(name, "def lowering", type_args)?;
            apply_explicit_generic_type_args_for_call(
                &*(ctx.user_registry as *const UserFnRegistry),
                name,
                &explicit_type_args,
                &mut scalar_types,
                "def lowering",
            )?;

            let (fn_ref, fn_ty, ret_ty) = ensure_user_fn_specialization(
                ctx.module,
                ctx.context,
                &mut *(ctx.user_registry as *mut UserFnRegistry),
                ctx.struct_fields,
                ctx.sample_rate,
                ctx.block_size as usize,
                name,
                &scalar_types,
                &buffer_types,
                &explicit_type_args,
            )?;

            let mut arg_values = Vec::new();
            let mut scalar_idx = 0usize;
            for (idx, kind) in param_kinds.iter().enumerate() {
                let resolved_arg = resolved.get(idx).copied().flatten();
                match kind {
                    TypedFnParam::Scalar => {
                        let target_ty = scalar_types[scalar_idx];
                        let value = cast_def_value_to(
                            ctx,
                            scalar_values[scalar_idx],
                            target_ty,
                            b"def_call_arg\0",
                        );
                        scalar_idx += 1;
                        arg_values.push(value);
                    }
                    TypedFnParam::Struct { struct_name } => {
                        let arg_expr = resolved_arg.ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "function '{name}' missing required struct argument '{}' in def lowering",
                                param_names[idx]
                            ))
                        })?;
                        lower_struct_call_args_in_def(
                            ctx,
                            &mut arg_values,
                            arg_expr,
                            struct_name,
                            name,
                            param_by_ref[idx],
                        )?;
                    }
                    TypedFnParam::Buffer { .. } => {
                        let arg_expr = resolved_arg.ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "function '{name}' missing required buffer argument '{}' in def lowering",
                                param_names[idx]
                            ))
                        })?;
                        lower_buffer_call_args_in_def(ctx, &mut arg_values, arg_expr, name)?;
                    }
                }
            }
            Ok(OrcValue {
                value: LLVMBuildCall2(
                    ctx.builder,
                    fn_ty,
                    fn_ref,
                    arg_values.as_mut_ptr(),
                    arg_values.len() as u32,
                    b"def_call\0".as_ptr().cast(),
                ),
                ty: ret_ty,
            })
        }
        Expr::Index { base, index } => {
            let data = lower_def_data_element_ptr(ctx, base, index, true)?;
            Ok(OrcValue {
                value: LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, data.elem_ty),
                    data.ptr,
                    b"def_data_load\0".as_ptr().cast(),
                ),
                ty: data.elem_ty,
            })
        }
        Expr::DataCtor { .. } => Err(Diagnostic::internal(
            "Data constructor is not supported in def lowering",
        )),
    }
}

unsafe fn lower_def_data_element_ptr(
    ctx: &mut DefLoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    clamp_index: bool,
) -> Result<DataElementPtr, Diagnostic> {
    if let Some(info) = ctx.buffer_params.get(base).cloned() {
        let total_len = load_def_buffer_total_len_i32(ctx, base, &info)?;
        let final_index = if clamp_index {
            let raw_index = lower_def_expr(index_expr, ctx)?;
            let index_i32 =
                cast_def_value_to(ctx, raw_index, PrimitiveType::I32, b"def_buf_idx_i32\0");
            clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, index_i32, total_len)
        } else {
            let raw_index = lower_def_expr(index_expr, ctx)?;
            cast_def_value_to(ctx, raw_index, PrimitiveType::I32, b"def_buf_idx_i32\0")
        };
        let typed_ptr = LLVMBuildBitCast(
            ctx.builder,
            info.ptr,
            LLVMPointerType(llvm_ty_for_primitive(ctx.context, info.elem_ty), 0),
            b"def_buf_ptr_typed\0".as_ptr().cast(),
        );
        return Ok(DataElementPtr {
            ptr: build_f32_ptr_offset(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, info.elem_ty),
                typed_ptr,
                final_index,
                b"def_buf_elem_ptr\0",
            ),
            elem_ty: info.elem_ty,
        });
    }

    let (base_ptr, len, elem_ty) = if let Some(alias) = ctx.local_data_aliases.get(base) {
        match alias {
            LocalDataAlias::Primitive {
                base_ptr,
                len,
                elem_ty,
            } => (*base_ptr, *len, *elem_ty),
            LocalDataAlias::Struct { .. } => {
                return Err(Diagnostic::internal(format!(
                    "Data symbol '{base}[...]' has struct elements in def lowering; index it via an alias assignment first"
                )));
            }
        }
    } else {
        if ctx.data_struct_roots.contains_key(base) {
            return Err(Diagnostic::internal(format!(
                "Data symbol '{base}[...]' has struct elements in def lowering; index it via an alias assignment first"
            )));
        }
        let base_ptr = *ctx.data_ptrs.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "unknown Data symbol '{base}' in def indexed expression lowering"
            ))
        })?;
        let len = *ctx.data_len.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing Data length for '{base}' in def indexed expression lowering"
            ))
        })?;
        let elem_ty = *ctx.data_elem_ty.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing Data element type for '{base}' in def indexed expression lowering"
            ))
        })?;
        (base_ptr, len, elem_ty)
    };

    if len == 0 {
        return Err(Diagnostic::internal(format!(
            "Data symbol '{base}' has zero length in def lowering"
        )));
    }

    let final_index = if clamp_index {
        if let Some(const_idx) = try_constant_index_i64(index_expr) {
            LLVMConstInt(
                ctx.i32_ty,
                checked_constant_data_index_u64(
                    len,
                    const_idx,
                    &format!("Data index in def lowering for '{base}'"),
                )?,
                0,
            )
        } else {
            let raw_index = lower_def_expr(index_expr, ctx)?;
            let index_i32 =
                cast_def_value_to(ctx, raw_index, PrimitiveType::I32, b"def_data_idx_i32\0");
            clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, len)?
        }
    } else {
        let raw_index = lower_def_expr(index_expr, ctx)?;
        cast_def_value_to(ctx, raw_index, PrimitiveType::I32, b"def_data_idx_i32\0")
    };
    Ok(DataElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, elem_ty),
            base_ptr,
            final_index,
            b"def_data_elem_ptr\0",
        ),
        elem_ty,
    })
}

unsafe fn load_def_buffer_channels_i32(
    ctx: &mut DefLoweringCtx<'_>,
    info: &DefBufferParamInfo,
) -> LLVMValueRef {
    match info.declared_channels {
        TypedBufferChannels::Mono => LLVMConstInt(ctx.i32_ty, 1, 0),
        TypedBufferChannels::Static(ch) => LLVMConstInt(ctx.i32_ty, ch as u64, 0),
        TypedBufferChannels::Dynamic => info.channels,
    }
}

unsafe fn load_def_buffer_total_len_i32(
    ctx: &mut DefLoweringCtx<'_>,
    _base: &str,
    info: &DefBufferParamInfo,
) -> Result<LLVMValueRef, Diagnostic> {
    let frames = info.frames;
    match info.declared_channels {
        TypedBufferChannels::Mono => Ok(frames),
        TypedBufferChannels::Static(ch) => {
            if ch <= 1 {
                Ok(frames)
            } else {
                let channels = LLVMConstInt(ctx.i32_ty, ch as u64, 0);
                Ok(LLVMBuildMul(
                    ctx.builder,
                    frames,
                    channels,
                    b"def_buf_total_len\0".as_ptr().cast(),
                ))
            }
        }
        TypedBufferChannels::Dynamic => Ok(LLVMBuildMul(
            ctx.builder,
            frames,
            info.channels,
            b"def_buf_total_len\0".as_ptr().cast(),
        )),
    }
}

unsafe fn lower_def_buffer_chans_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "def lowering")?;
    let info = ctx.buffer_params.get(base).cloned().ok_or_else(|| {
        Diagnostic::internal(format!(
            "builtin method '{method_name}' requires a buffer symbol receiver in def lowering, got '{base}'"
        ))
    })?;
    Ok(OrcValue {
        value: load_def_buffer_channels_i32(ctx, &info),
        ty: PrimitiveType::I32,
    })
}

unsafe fn lower_def_buffer_element_ptr_2d(
    ctx: &mut DefLoweringCtx<'_>,
    base: &str,
    channel_expr: &Expr,
    sample_expr: &Expr,
    clamp_index: bool,
) -> Result<DataElementPtr, Diagnostic> {
    let info = ctx.buffer_params.get(base).cloned().ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in def lowering two-dimensional index"
        ))
    })?;
    let channel = lower_def_expr(channel_expr, ctx)?;
    let sample = lower_def_expr(sample_expr, ctx)?;
    let channel_i = cast_def_value_to(ctx, channel, PrimitiveType::I32, b"def_buf_ch_i32\0");
    let sample_i = cast_def_value_to(ctx, sample, PrimitiveType::I32, b"def_buf_sample_i32\0");
    let channels = load_def_buffer_channels_i32(ctx, &info);
    let total_len = load_def_buffer_total_len_i32(ctx, base, &info)?;
    let sample_off = LLVMBuildMul(
        ctx.builder,
        sample_i,
        channels,
        b"def_buf_sample_off\0".as_ptr().cast(),
    );
    let raw_flat = LLVMBuildAdd(
        ctx.builder,
        sample_off,
        channel_i,
        b"def_buf_flat_idx\0".as_ptr().cast(),
    );
    let flat_index = if clamp_index {
        clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, raw_flat, total_len)
    } else {
        raw_flat
    };
    let typed_ptr = LLVMBuildBitCast(
        ctx.builder,
        info.ptr,
        LLVMPointerType(llvm_ty_for_primitive(ctx.context, info.elem_ty), 0),
        b"def_buf_ptr_typed2\0".as_ptr().cast(),
    );
    Ok(DataElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, info.elem_ty),
            typed_ptr,
            flat_index,
            b"def_buf_elem_ptr2\0",
        ),
        elem_ty: info.elem_ty,
    })
}

unsafe fn lower_def_buffer_read2_call(
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    ensure_internal_buffer_2d_call_positional_arity(
        args,
        "__omni_buffer_read2",
        3,
        "def lowering",
    )?;
    let base = builtin_data_call_base_symbol(args, "__omni_buffer_read2", "def lowering")?;
    let ch_expr = &args[1].expr;
    let sample_expr = &args[2].expr;
    let data = lower_def_buffer_element_ptr_2d(ctx, base, ch_expr, sample_expr, clamp_index)?;
    Ok(OrcValue {
        value: LLVMBuildLoad2(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data.elem_ty),
            data.ptr,
            b"def_buf2_read\0".as_ptr().cast(),
        ),
        ty: data.elem_ty,
    })
}

unsafe fn lower_def_buffer_write2_call(
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    ensure_internal_buffer_2d_call_positional_arity(
        args,
        "__omni_buffer_write2",
        4,
        "def lowering",
    )?;
    let base = builtin_data_call_base_symbol(args, "__omni_buffer_write2", "def lowering")?;
    let ch_expr = &args[1].expr;
    let sample_expr = &args[2].expr;
    let value_expr = &args[3].expr;
    let data = lower_def_buffer_element_ptr_2d(ctx, base, ch_expr, sample_expr, clamp_index)?;
    let value = lower_def_expr(value_expr, ctx)?;
    let casted = cast_def_value_to(ctx, value, data.elem_ty, b"def_buf2_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

unsafe fn lower_struct_call_args_in_def(
    ctx: &mut DefLoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
    by_ref: bool,
) -> Result<(), Diagnostic> {
    let base = match arg_expr {
        Expr::Var(name) => name,
        _ => {
            return Err(Diagnostic::internal(format!(
                "function '{callee_name}' expects struct '{struct_name}' argument as a variable reference in def lowering"
            )));
        }
    };
    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{struct_name}' in def call lowering for function '{callee_name}'"
        ))
    })?;
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(_) => {
                let local = ctx.local_slots.get(&flat).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing def local slot for struct field '{flat}' while calling '{callee_name}'"
                    ))
                })?;
                if by_ref {
                    out_args.push(local.ptr);
                } else {
                    let loaded = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, local.ty),
                        local.ptr,
                        b"def_struct_arg_load\0".as_ptr().cast(),
                    );
                    out_args.push(loaded);
                }
            }
            TypedFieldType::Data(_) => {
                if let Some(elem_struct) = &field.data_elem_struct {
                    let root_len = *ctx.data_len.get(&flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def Data[Struct] length metadata for '{flat}' while calling '{callee_name}'"
                        ))
                    })?;
                    let mut roots = Vec::new();
                    let mut leaves = Vec::new();
                    collect_data_struct_bindings(
                        ctx.struct_fields,
                        elem_struct,
                        &flat,
                        root_len,
                        &mut roots,
                        &mut leaves,
                        &mut Vec::new(),
                    )?;
                    for (leaf_name, _, _) in leaves {
                        let ptr = *ctx.data_ptrs.get(&leaf_name).ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "missing def Data pointer for struct field '{leaf_name}' while calling '{callee_name}'"
                            ))
                        })?;
                        out_args.push(ptr);
                    }
                } else {
                    let ptr = *ctx.data_ptrs.get(&flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def Data pointer for struct field '{flat}' while calling '{callee_name}'"
                        ))
                    })?;
                    out_args.push(ptr);
                }
            }
        }
    }
    Ok(())
}

unsafe fn bind_struct_data_element_aliases_in_def(
    alias_name: &str,
    struct_name: &str,
    root_base: &str,
    global_index: LLVMValueRef,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<(), Diagnostic> {
    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{}' while creating def Data alias '{}'",
            struct_name, alias_name
        ))
    })?;
    for field in fields {
        let data_field_base = format!("{root_base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                let data_base_ptr = *ctx.data_ptrs.get(&data_field_base).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing def Data pointer for symbol '{data_field_base}' while creating alias '{alias_name}'"
                    ))
                })?;
                let elem_ptr = build_f32_ptr_offset(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, prim),
                    data_base_ptr,
                    global_index,
                    b"def_struct_data_elem_ptr\0",
                );
                ctx.local_slots.insert(
                    format!("{alias_name}.{}", field.name),
                    DefLocalSlot {
                        ptr: elem_ptr,
                        ty: prim,
                    },
                );
            }
            TypedFieldType::Data(field_len) => {
                let start_idx = build_data_segment_start_index(
                    ctx.builder,
                    ctx.i32_ty,
                    global_index,
                    field_len,
                )?;
                if let Some(elem_struct) = &field.data_elem_struct {
                    ctx.local_data_aliases.insert(
                        format!("{alias_name}.{}", field.name),
                        LocalDataAlias::Struct {
                            root_base: data_field_base.clone(),
                            elem_struct: elem_struct.clone(),
                            len: field_len,
                            start_index: start_idx,
                        },
                    );
                } else {
                    let elem_ty = field.data_elem_ty.unwrap_or(PrimitiveType::F32);
                    let data_base_ptr = *ctx.data_ptrs.get(&data_field_base).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def Data pointer for symbol '{data_field_base}' while creating alias '{alias_name}'"
                        ))
                    })?;
                    let seg_ptr = build_f32_ptr_offset(
                        ctx.builder,
                        ctx.float_ty,
                        data_base_ptr,
                        start_idx,
                        b"def_struct_data_seg_ptr\0",
                    );
                    ctx.local_data_aliases.insert(
                        format!("{alias_name}.{}", field.name),
                        LocalDataAlias::Primitive {
                            base_ptr: seg_ptr,
                            len: field_len,
                            elem_ty,
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

fn build_local_slot(
    builder: LLVMBuilderRef,
    elem_ty: LLVMTypeRef,
    name: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    unsafe {
        let insert_bb = LLVMGetInsertBlock(builder);
        if insert_bb.is_null() {
            return Err(Diagnostic::internal(
                "failed to get insertion block for local allocation",
            ));
        }
        let fn_ref = LLVMGetBasicBlockParent(insert_bb);
        if fn_ref.is_null() {
            return Err(Diagnostic::internal(
                "failed to get function for local allocation",
            ));
        }
        let entry_bb = LLVMGetEntryBasicBlock(fn_ref);
        if entry_bb.is_null() {
            return Err(Diagnostic::internal(
                "failed to get entry block for local allocation",
            ));
        }
        let context = LLVMGetTypeContext(elem_ty);
        if context.is_null() {
            return Err(Diagnostic::internal(
                "failed to get LLVM context for local allocation",
            ));
        }
        let alloca_builder = LLVMCreateBuilderInContext(context);
        if alloca_builder.is_null() {
            return Err(Diagnostic::internal(
                "failed to create LLVM builder for local allocation",
            ));
        }
        let first_inst = LLVMGetFirstInstruction(entry_bb);
        if first_inst.is_null() {
            LLVMPositionBuilderAtEnd(alloca_builder, entry_bb);
        } else {
            LLVMPositionBuilderBefore(alloca_builder, first_inst);
        }
        let c_name =
            CString::new(name).map_err(|_| Diagnostic::internal("invalid local variable name"))?;
        let slot = LLVMBuildAlloca(alloca_builder, elem_ty, c_name.as_ptr());
        LLVMDisposeBuilder(alloca_builder);
        if slot.is_null() {
            return Err(Diagnostic::internal(
                "failed to allocate local variable slot",
            ));
        }
        Ok(slot)
    }
}

fn build_local_array_slot(
    builder: LLVMBuilderRef,
    elem_ty: LLVMTypeRef,
    len: usize,
    name: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    if len == 0 {
        return Err(Diagnostic::internal(
            "local array declaration requires size greater than zero",
        ));
    }
    if len > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "local array declaration size {len} exceeds supported i32 range in ORC lowering"
        )));
    }

    unsafe {
        let insert_bb = LLVMGetInsertBlock(builder);
        if insert_bb.is_null() {
            return Err(Diagnostic::internal(
                "failed to get insertion block for local array allocation",
            ));
        }
        let fn_ref = LLVMGetBasicBlockParent(insert_bb);
        if fn_ref.is_null() {
            return Err(Diagnostic::internal(
                "failed to get function for local array allocation",
            ));
        }
        let entry_bb = LLVMGetEntryBasicBlock(fn_ref);
        if entry_bb.is_null() {
            return Err(Diagnostic::internal(
                "failed to get entry block for local array allocation",
            ));
        }
        let context = LLVMGetTypeContext(elem_ty);
        if context.is_null() {
            return Err(Diagnostic::internal(
                "failed to get LLVM context for local array allocation",
            ));
        }
        let alloca_builder = LLVMCreateBuilderInContext(context);
        if alloca_builder.is_null() {
            return Err(Diagnostic::internal(
                "failed to create LLVM builder for local array allocation",
            ));
        }
        let first_inst = LLVMGetFirstInstruction(entry_bb);
        if first_inst.is_null() {
            LLVMPositionBuilderAtEnd(alloca_builder, entry_bb);
        } else {
            LLVMPositionBuilderBefore(alloca_builder, first_inst);
        }
        let c_name =
            CString::new(name).map_err(|_| Diagnostic::internal("invalid local array name"))?;
        let count = const_i32(LLVMInt32TypeInContext(context), len as i32);
        let slot = LLVMBuildArrayAlloca(alloca_builder, elem_ty, count, c_name.as_ptr());
        LLVMDisposeBuilder(alloca_builder);
        if slot.is_null() {
            return Err(Diagnostic::internal("failed to allocate local array slot"));
        }
        Ok(slot)
    }
}

unsafe fn current_block_terminated(builder: LLVMBuilderRef) -> bool {
    let bb = LLVMGetInsertBlock(builder);
    if bb.is_null() {
        return false;
    }
    !LLVMGetBasicBlockTerminator(bb).is_null()
}

unsafe fn build_process_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    user_fns: &mut UserFnRegistry,
    sample_rate: f32,
    block_size: usize,
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(LLVMInt8TypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let i32_ptr_ty = LLVMPointerType(i32_ty, 0);
    let i8_ptr_ptr_ty = LLVMPointerType(i8_ptr_ty, 0);
    let void_ty = LLVMVoidTypeInContext(context);
    let float_ptr_ty = i8_ptr_ty;
    let float_ptr_ptr_ty = i8_ptr_ptr_ty;

    let mut arg_types = [
        float_ptr_ptr_ty,
        float_ptr_ptr_ty,
        i32_ty,
        i8_ptr_ty,
        i8_ptr_ty,
        i8_ptr_ptr_ty,
        i32_ptr_ty,
        i32_ptr_ty,
    ];

    let fn_name = CString::new("omni_process")
        .map_err(|_| Diagnostic::internal("invalid process function name"))?;
    let fn_ty = LLVMFunctionType(void_ty, arg_types.as_mut_ptr(), arg_types.len() as u32, 0);
    let fn_ref = LLVMAddFunction(module, fn_name.as_ptr(), fn_ty);

    let entry_name =
        CString::new("entry").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let entry = LLVMAppendBasicBlockInContext(context, fn_ref, entry_name.as_ptr());
    let cond_name =
        CString::new("loop_cond").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let body_name =
        CString::new("loop_body").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let exit_name =
        CString::new("loop_exit").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let loop_cond = LLVMAppendBasicBlockInContext(context, fn_ref, cond_name.as_ptr());
    let loop_body = LLVMAppendBasicBlockInContext(context, fn_ref, body_name.as_ptr());
    let loop_exit = LLVMAppendBasicBlockInContext(context, fn_ref, exit_name.as_ptr());

    let builder = LLVMCreateBuilderInContext(context);
    if builder.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM builder"));
    }
    let result = (|| -> Result<(), Diagnostic> {
        LLVMPositionBuilderAtEnd(builder, entry);

        let in_ptrs = LLVMGetParam(fn_ref, 0);
        let out_ptrs = LLVMGetParam(fn_ref, 1);
        let frames = LLVMGetParam(fn_ref, 2);
        let params_ptr = LLVMGetParam(fn_ref, 3);
        let state_ptr = LLVMGetParam(fn_ref, 4);
        let buffer_ptrs = LLVMGetParam(fn_ref, 5);
        let buffer_frames_ptr = LLVMGetParam(fn_ref, 6);
        let buffer_channels_ptr = LLVMGetParam(fn_ref, 7);

        let zero_i32 = LLVMConstInt(i32_ty, 0, 0);
        let one_i32 = LLVMConstInt(i32_ty, 1, 0);

        let frame_idx_name =
            CString::new("frame_idx").map_err(|_| Diagnostic::internal("invalid local name"))?;
        let frame_idx = LLVMBuildAlloca(builder, i32_ty, frame_idx_name.as_ptr());
        LLVMBuildStore(builder, zero_i32, frame_idx);

        let mut input_index = HashMap::new();
        for (idx, name) in typed.ins.iter().enumerate() {
            input_index.insert(name.clone(), idx as u32);
        }
        let mut input_types = HashMap::new();
        for name in &typed.ins {
            input_types.insert(
                name.clone(),
                *typed.in_types.get(name).unwrap_or(&PrimitiveType::F32),
            );
        }
        let mut buffer_index = HashMap::new();
        let mut buffer_elem_types = HashMap::new();
        let mut buffer_channels = HashMap::new();
        let mut buffer_mono = HashSet::new();
        for (idx, decl) in typed.buffers.iter().enumerate() {
            buffer_index.insert(decl.name.clone(), idx as u32);
            buffer_elem_types.insert(decl.name.clone(), decl.elem_ty);
            buffer_channels.insert(decl.name.clone(), decl.channels.clone());
            let mono = match decl.channels {
                TypedBufferChannels::Mono => true,
                TypedBufferChannels::Static(ch) => ch <= 1,
                TypedBufferChannels::Dynamic => false,
            };
            if mono {
                buffer_mono.insert(decl.name.clone());
            }
        }
        let arrays_by_offset = typed
            .param_arrays
            .iter()
            .map(|(name, info)| (info.offset, name.as_str()))
            .collect::<HashMap<_, _>>();
        let mut param_byte_offset = HashMap::new();
        let mut running_param_bytes = 0usize;
        for (slot_idx, param) in typed.params.iter().enumerate() {
            if let Some(base_name) = arrays_by_offset.get(&slot_idx) {
                param_byte_offset.insert((*base_name).to_owned(), running_param_bytes);
            }
            param_byte_offset.insert(param.name.clone(), running_param_bytes);
            running_param_bytes =
                running_param_bytes.saturating_add(primitive_type_bytes(param.ty));
        }
        let mut param_types = HashMap::new();
        for param in &typed.params {
            param_types.insert(param.name.clone(), param.ty);
        }
        let state_layout_entries = compute_state_layout(typed)?;
        let state_layout = state_layout_map(&state_layout_entries);
        let array_layout_entries = compute_arrays_layout(typed, &state_layout_entries)?;
        let data_layout = arrays_layout_map(&array_layout_entries);

        let mut data_base_ptrs = HashMap::new();
        let mut data_len = HashMap::new();
        let mut data_elem_ty = HashMap::new();
        for data_var in &typed.data_vars {
            let (_, offset) = *data_layout.get(&data_var.name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing array layout metadata for '{}'",
                    data_var.name
                ))
            })?;
            let ptr = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                offset,
                data_var.elem_ty,
                b"arr_state_ptr\0",
                b"arr_state_ptr_cast\0",
            );
            data_base_ptrs.insert(data_var.name.clone(), ptr);
            data_len.insert(data_var.name.clone(), data_var.len);
            data_elem_ty.insert(data_var.name.clone(), data_var.elem_ty);
        }
        let mut data_struct_roots = HashMap::new();
        let mut data_struct_len = HashMap::new();
        for root in &typed.data_struct_roots {
            data_struct_roots.insert(root.name.clone(), root.struct_name.clone());
            data_struct_len.insert(root.name.clone(), root.len);
        }
        let struct_fields = typed
            .structs
            .iter()
            .map(|s| (s.name.clone(), s.fields.clone()))
            .collect::<HashMap<_, _>>();
        let mut state_slots = HashMap::new();
        for (idx, name) in typed.state_vars.iter().enumerate() {
            let (state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let slot_name = CString::new(format!("state_{}", idx))
                .map_err(|_| Diagnostic::internal("invalid state slot name"))?;
            let slot = LLVMBuildAlloca(
                builder,
                llvm_ty_for_primitive(context, state_ty),
                slot_name.as_ptr(),
            );
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                state_ty,
                b"state_ptr\0",
                b"state_ptr_cast\0",
            );
            let state_load = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, state_ty),
                state_ptr_elt,
                b"state_load\0".as_ptr().cast(),
            );
            LLVMBuildStore(builder, state_load, slot);
            state_slots.insert(
                name.clone(),
                StateSlot {
                    ptr: slot,
                    ty: state_ty,
                },
            );
        }

        let mut out_slots = HashMap::new();
        let mut out_array_base_ptrs = HashMap::new();
        let mut out_array_names = typed.out_arrays.keys().cloned().collect::<Vec<_>>();
        out_array_names.sort();
        for array_name in out_array_names {
            let array_info = typed
                .out_arrays
                .get(&array_name)
                .ok_or_else(|| Diagnostic::internal("missing output array metadata"))?;
            let base_ptr = build_local_array_slot(
                builder,
                llvm_ty_for_primitive(context, array_info.elem_ty),
                array_info.len,
                &format!("out_arr_{array_name}"),
            )?;
            out_array_base_ptrs.insert(array_name.clone(), base_ptr);
            for idx in 0..array_info.len {
                let idx_v = LLVMConstInt(i32_ty, idx as u64, 0);
                let elem_ptr = build_f32_ptr_offset(
                    builder,
                    llvm_ty_for_primitive(context, array_info.elem_ty),
                    base_ptr,
                    idx_v,
                    b"out_arr_elem_ptr\0",
                );
                out_slots.insert(
                    format!("{array_name}[{idx}]"),
                    OutSlot {
                        ptr: elem_ptr,
                        ty: array_info.elem_ty,
                    },
                );
            }
        }
        for (idx, name) in typed.outs.iter().enumerate() {
            if out_slots.contains_key(name) {
                continue;
            }
            let slot_name = CString::new(format!("out_{}", idx))
                .map_err(|_| Diagnostic::internal("invalid output slot name"))?;
            let out_ty = *typed.out_types.get(name).unwrap_or(&PrimitiveType::F32);
            let out_llvm_ty = llvm_ty_for_primitive(context, out_ty);
            let slot = LLVMBuildAlloca(builder, out_llvm_ty, slot_name.as_ptr());
            LLVMBuildStore(builder, llvm_zero_for_primitive(context, out_ty), slot);
            out_slots.insert(
                name.clone(),
                OutSlot {
                    ptr: slot,
                    ty: out_ty,
                },
            );
        }

        // Run optional block-level code once per process callback before per-sample loop.
        if !typed.block_pre.is_empty() {
            let mut block_ctx = LoweringCtx {
                builder,
                context,
                module,
                fn_ref,
                float_ty,
                float_ptr_ty,
                i32_ty,
                sample_rate,
                block_size: block_size as f32,
                in_ptrs,
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                frame_idx: LLVMConstInt(i32_ty, 0, 0),
                state_slots: &state_slots,
                data_base_ptrs: &data_base_ptrs,
                out_slots: &out_slots,
                out_array_base_ptrs: &out_array_base_ptrs,
                input_index: &input_index,
                input_types: &input_types,
                input_arrays: &typed.in_arrays,
                buffer_index: &buffer_index,
                buffer_elem_types: &buffer_elem_types,
                buffer_channels: &buffer_channels,
                buffer_mono: &buffer_mono,
                param_byte_offset: &param_byte_offset,
                param_types: &param_types,
                param_arrays: &typed.param_arrays,
                output_arrays: &typed.out_arrays,
                data_len: &data_len,
                data_elem_ty: &data_elem_ty,
                data_struct_roots: &data_struct_roots,
                data_struct_len: &data_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
            };
            let mut block_locals = HashMap::new();
            let mut block_aliases = HashMap::new();
            let mut block_data_aliases = HashMap::new();
            for stmt in &typed.block_pre {
                lower_stmt(
                    stmt,
                    &mut block_ctx,
                    &mut block_locals,
                    &mut block_aliases,
                    &mut block_data_aliases,
                )?;
            }
        }

        LLVMBuildBr(builder, loop_cond);

        LLVMPositionBuilderAtEnd(builder, loop_cond);
        let frame_cur = LLVMBuildLoad2(builder, i32_ty, frame_idx, b"frame_cur\0".as_ptr().cast());
        let loop_cmp = LLVMBuildICmp(
            builder,
            LLVMIntPredicate::LLVMIntULT,
            frame_cur,
            frames,
            b"loop_cmp\0".as_ptr().cast(),
        );
        LLVMBuildCondBr(builder, loop_cmp, loop_body, loop_exit);

        LLVMPositionBuilderAtEnd(builder, loop_body);
        for slot in out_slots.values() {
            LLVMBuildStore(builder, llvm_zero_for_primitive(context, slot.ty), slot.ptr);
        }

        let frame_in_body =
            LLVMBuildLoad2(builder, i32_ty, frame_idx, b"frame_body\0".as_ptr().cast());
        let mut lctx = LoweringCtx {
            builder,
            context,
            module,
            fn_ref,
            float_ty,
            float_ptr_ty,
            i32_ty,
            sample_rate,
            block_size: block_size as f32,
            in_ptrs,
            params_ptr,
            buffer_ptrs,
            buffer_frames_ptr,
            buffer_channels_ptr,
            frame_idx: frame_in_body,
            state_slots: &state_slots,
            data_base_ptrs: &data_base_ptrs,
            out_slots: &out_slots,
            out_array_base_ptrs: &out_array_base_ptrs,
            input_index: &input_index,
            input_types: &input_types,
            input_arrays: &typed.in_arrays,
            buffer_index: &buffer_index,
            buffer_elem_types: &buffer_elem_types,
            buffer_channels: &buffer_channels,
            buffer_mono: &buffer_mono,
            param_byte_offset: &param_byte_offset,
            param_types: &param_types,
            param_arrays: &typed.param_arrays,
            output_arrays: &typed.out_arrays,
            data_len: &data_len,
            data_elem_ty: &data_elem_ty,
            data_struct_roots: &data_struct_roots,
            data_struct_len: &data_struct_len,
            struct_fields: &struct_fields,
            allow_struct_ctor: false,
            user_fn_param_names: &user_fns.param_names,
            user_fn_param_defaults: &user_fns.param_defaults,
            user_fn_param_kinds: &user_fns.param_kinds,
            user_fn_param_by_ref: &user_fns.param_by_ref,
            user_registry: user_fns as *const UserFnRegistry,
        };

        let mut locals = HashMap::new();
        let mut local_aliases = HashMap::new();
        let mut local_data_aliases = HashMap::new();
        for stmt in &typed.sample {
            lower_stmt(
                stmt,
                &mut lctx,
                &mut locals,
                &mut local_aliases,
                &mut local_data_aliases,
            )?;
        }

        for (ch, name) in typed.outs.iter().enumerate() {
            let slot = out_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing output slot for '{name}' in ORC lowering"))
            })?;
            let out_ty = *typed.out_types.get(name).unwrap_or(&PrimitiveType::F32);
            let raw_out_value = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, slot.ty),
                slot.ptr,
                b"out_value_raw\0".as_ptr().cast(),
            );
            let out_value = if slot.ty == out_ty {
                raw_out_value
            } else {
                cast_orc_value_to(
                    &lctx,
                    OrcValue {
                        value: raw_out_value,
                        ty: slot.ty,
                    },
                    out_ty,
                    b"out_value_cast\0",
                )
            };
            let ch_idx = LLVMConstInt(i32_ty, ch as u64, 0);
            let out_ptr_ptr =
                build_ptr_offset(builder, float_ptr_ty, out_ptrs, ch_idx, b"out_ch_ptr_ptr\0");
            let out_ch_ptr_raw = LLVMBuildLoad2(
                builder,
                float_ptr_ty,
                out_ptr_ptr,
                b"out_ch_ptr\0".as_ptr().cast(),
            );
            let out_ch_ptr = LLVMBuildBitCast(
                builder,
                out_ch_ptr_raw,
                LLVMPointerType(llvm_ty_for_primitive(context, out_ty), 0),
                b"out_ch_ptr_typed\0".as_ptr().cast(),
            );
            let out_ptr_elt = build_f32_ptr_offset(
                builder,
                llvm_ty_for_primitive(context, out_ty),
                out_ch_ptr,
                frame_in_body,
                b"out_ptr\0",
            );
            LLVMBuildStore(builder, out_value, out_ptr_elt);
        }

        let next_frame = LLVMBuildAdd(
            builder,
            frame_in_body,
            one_i32,
            b"next_frame\0".as_ptr().cast(),
        );
        LLVMBuildStore(builder, next_frame, frame_idx);
        LLVMBuildBr(builder, loop_cond);

        LLVMPositionBuilderAtEnd(builder, loop_exit);

        // Run optional post-sample block-level code once per process callback.
        if !typed.block_post.is_empty() {
            let mut block_ctx = LoweringCtx {
                builder,
                context,
                module,
                fn_ref,
                float_ty,
                float_ptr_ty,
                i32_ty,
                sample_rate,
                block_size: block_size as f32,
                in_ptrs,
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                frame_idx: LLVMConstInt(i32_ty, 0, 0),
                state_slots: &state_slots,
                data_base_ptrs: &data_base_ptrs,
                out_slots: &out_slots,
                out_array_base_ptrs: &out_array_base_ptrs,
                input_index: &input_index,
                input_types: &input_types,
                input_arrays: &typed.in_arrays,
                buffer_index: &buffer_index,
                buffer_elem_types: &buffer_elem_types,
                buffer_channels: &buffer_channels,
                buffer_mono: &buffer_mono,
                param_byte_offset: &param_byte_offset,
                param_types: &param_types,
                param_arrays: &typed.param_arrays,
                output_arrays: &typed.out_arrays,
                data_len: &data_len,
                data_elem_ty: &data_elem_ty,
                data_struct_roots: &data_struct_roots,
                data_struct_len: &data_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
            };
            let mut block_locals = HashMap::new();
            let mut block_aliases = HashMap::new();
            let mut block_data_aliases = HashMap::new();
            for stmt in &typed.block_post {
                lower_stmt(
                    stmt,
                    &mut block_ctx,
                    &mut block_locals,
                    &mut block_aliases,
                    &mut block_data_aliases,
                )?;
            }
        }
        for (_idx, name) in typed.state_vars.iter().enumerate() {
            let slot = state_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state slot for '{name}' in ORC lowering"))
            })?;
            let (_state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let value = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, slot.ty),
                slot.ptr,
                b"state_out\0".as_ptr().cast(),
            );
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                slot.ty,
                b"state_out_ptr\0",
                b"state_out_ptr_cast\0",
            );
            LLVMBuildStore(builder, value, state_ptr_elt);
        }
        LLVMBuildRetVoid(builder);

        Ok(())
    })();
    LLVMDisposeBuilder(builder);
    result
}

unsafe fn build_init_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    user_fns: &mut UserFnRegistry,
    sample_rate: f32,
    block_size: usize,
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(LLVMInt8TypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let void_ty = LLVMVoidTypeInContext(context);
    let float_ptr_ty = i8_ptr_ty;
    let float_ptr_ptr_ty = LLVMPointerType(float_ptr_ty, 0);

    let mut arg_types = [i8_ptr_ty, i8_ptr_ty];

    let fn_name = CString::new("omni_init")
        .map_err(|_| Diagnostic::internal("invalid init function name"))?;
    let fn_ty = LLVMFunctionType(void_ty, arg_types.as_mut_ptr(), arg_types.len() as u32, 0);
    let fn_ref = LLVMAddFunction(module, fn_name.as_ptr(), fn_ty);

    let entry_name =
        CString::new("entry").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let entry = LLVMAppendBasicBlockInContext(context, fn_ref, entry_name.as_ptr());

    let builder = LLVMCreateBuilderInContext(context);
    if builder.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM builder"));
    }
    let result = (|| -> Result<(), Diagnostic> {
        LLVMPositionBuilderAtEnd(builder, entry);

        let params_ptr = LLVMGetParam(fn_ref, 0);
        let state_ptr = LLVMGetParam(fn_ref, 1);

        let arrays_by_offset = typed
            .param_arrays
            .iter()
            .map(|(name, info)| (info.offset, name.as_str()))
            .collect::<HashMap<_, _>>();
        let mut param_byte_offset = HashMap::new();
        let mut running_param_bytes = 0usize;
        for (slot_idx, param) in typed.params.iter().enumerate() {
            if let Some(base_name) = arrays_by_offset.get(&slot_idx) {
                param_byte_offset.insert((*base_name).to_owned(), running_param_bytes);
            }
            param_byte_offset.insert(param.name.clone(), running_param_bytes);
            running_param_bytes =
                running_param_bytes.saturating_add(primitive_type_bytes(param.ty));
        }
        let mut param_types = HashMap::new();
        for param in &typed.params {
            param_types.insert(param.name.clone(), param.ty);
        }
        let state_layout_entries = compute_state_layout(typed)?;
        let state_layout = state_layout_map(&state_layout_entries);
        let array_layout_entries = compute_arrays_layout(typed, &state_layout_entries)?;
        let data_layout = arrays_layout_map(&array_layout_entries);

        let mut data_base_ptrs = HashMap::new();
        let mut data_len = HashMap::new();
        let mut data_elem_ty = HashMap::new();
        for data_var in &typed.data_vars {
            let (_, offset) = *data_layout.get(&data_var.name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing array layout metadata for '{}' in ORC init lowering",
                    data_var.name
                ))
            })?;
            let ptr = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                offset,
                data_var.elem_ty,
                b"arr_init_state_ptr\0",
                b"arr_init_state_ptr_cast\0",
            );
            data_base_ptrs.insert(data_var.name.clone(), ptr);
            data_len.insert(data_var.name.clone(), data_var.len);
            data_elem_ty.insert(data_var.name.clone(), data_var.elem_ty);
        }
        let mut data_struct_roots = HashMap::new();
        let mut data_struct_len = HashMap::new();
        for root in &typed.data_struct_roots {
            data_struct_roots.insert(root.name.clone(), root.struct_name.clone());
            data_struct_len.insert(root.name.clone(), root.len);
        }
        let struct_fields = typed
            .structs
            .iter()
            .map(|s| (s.name.clone(), s.fields.clone()))
            .collect::<HashMap<_, _>>();

        let mut state_slots = HashMap::new();
        for (idx, name) in typed.state_vars.iter().enumerate() {
            let (state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let slot_name = CString::new(format!("init_state_{}", idx))
                .map_err(|_| Diagnostic::internal("invalid state slot name"))?;
            let slot = LLVMBuildAlloca(
                builder,
                llvm_ty_for_primitive(context, state_ty),
                slot_name.as_ptr(),
            );
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                state_ty,
                b"init_state_ptr\0",
                b"init_state_ptr_cast\0",
            );
            let state_load = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, state_ty),
                state_ptr_elt,
                b"init_state_load\0".as_ptr().cast(),
            );
            LLVMBuildStore(builder, state_load, slot);
            state_slots.insert(
                name.clone(),
                StateSlot {
                    ptr: slot,
                    ty: state_ty,
                },
            );
        }

        let out_slots = HashMap::<String, OutSlot>::new();
        let out_array_base_ptrs = HashMap::<String, LLVMValueRef>::new();
        let input_index = HashMap::new();
        let input_types = HashMap::new();
        let input_arrays = HashMap::<String, TypedArrayInfo>::new();
        let buffer_index = HashMap::new();
        let buffer_elem_types = HashMap::new();
        let buffer_channels = HashMap::new();
        let buffer_mono = HashSet::<String>::new();
        let output_arrays = HashMap::<String, TypedArrayInfo>::new();

        let mut lctx = LoweringCtx {
            builder,
            context,
            module,
            fn_ref,
            float_ty,
            float_ptr_ty,
            i32_ty,
            sample_rate,
            block_size: block_size as f32,
            in_ptrs: LLVMConstPointerNull(float_ptr_ptr_ty),
            params_ptr,
            buffer_ptrs: LLVMConstPointerNull(LLVMPointerType(i8_ptr_ty, 0)),
            buffer_frames_ptr: LLVMConstPointerNull(LLVMPointerType(i32_ty, 0)),
            buffer_channels_ptr: LLVMConstPointerNull(LLVMPointerType(i32_ty, 0)),
            frame_idx: LLVMConstInt(i32_ty, 0, 0),
            state_slots: &state_slots,
            data_base_ptrs: &data_base_ptrs,
            out_slots: &out_slots,
            out_array_base_ptrs: &out_array_base_ptrs,
            input_index: &input_index,
            input_types: &input_types,
            input_arrays: &input_arrays,
            buffer_index: &buffer_index,
            buffer_elem_types: &buffer_elem_types,
            buffer_channels: &buffer_channels,
            buffer_mono: &buffer_mono,
            param_byte_offset: &param_byte_offset,
            param_types: &param_types,
            param_arrays: &typed.param_arrays,
            output_arrays: &output_arrays,
            data_len: &data_len,
            data_elem_ty: &data_elem_ty,
            data_struct_roots: &data_struct_roots,
            data_struct_len: &data_struct_len,
            struct_fields: &struct_fields,
            allow_struct_ctor: true,
            user_fn_param_names: &user_fns.param_names,
            user_fn_param_defaults: &user_fns.param_defaults,
            user_fn_param_kinds: &user_fns.param_kinds,
            user_fn_param_by_ref: &user_fns.param_by_ref,
            user_registry: user_fns as *const UserFnRegistry,
        };

        let mut locals = HashMap::new();
        let mut local_aliases = HashMap::new();
        let mut local_data_aliases = HashMap::new();
        for stmt in &typed.init {
            lower_stmt(
                stmt,
                &mut lctx,
                &mut locals,
                &mut local_aliases,
                &mut local_data_aliases,
            )?;
        }

        for (_idx, name) in typed.state_vars.iter().enumerate() {
            let slot = state_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing state slot for '{name}' in ORC init lowering"
                ))
            })?;
            let value = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, slot.ty),
                slot.ptr,
                b"init_state_out\0".as_ptr().cast(),
            );
            let (_state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                slot.ty,
                b"init_state_out_ptr\0",
                b"init_state_out_ptr_cast\0",
            );
            LLVMBuildStore(builder, value, state_ptr_elt);
        }
        LLVMBuildRetVoid(builder);

        Ok(())
    })();
    LLVMDisposeBuilder(builder);
    result
}

unsafe fn lower_expr(
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<OrcValue, Diagnostic> {
    match expr {
        Expr::Number(value) => Ok(OrcValue {
            value: LLVMConstReal(ctx.float_ty, *value as f64),
            ty: PrimitiveType::F32,
        }),
        Expr::ArrayLiteral(_) => Err(Diagnostic::internal(
            "array literal is not a scalar expression in ORC lowering",
        )),
        Expr::Int(value) => {
            if *value >= i32::MIN as i64 && *value <= i32::MAX as i64 {
                Ok(OrcValue {
                    value: const_i32(ctx.i32_ty, *value as i32),
                    ty: PrimitiveType::I32,
                })
            } else {
                let i64_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::I64);
                Ok(OrcValue {
                    value: LLVMConstInt(i64_ty, *value as u64, 1),
                    ty: PrimitiveType::I64,
                })
            }
        }
        Expr::Bool(value) => {
            let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
            Ok(OrcValue {
                value: LLVMConstInt(bool_ty, if *value { 1 } else { 0 }, 0),
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Var(name) => {
            if let Some(value) = builtin_constant_value(name, ctx.sample_rate, ctx.block_size) {
                return Ok(OrcValue {
                    value: LLVMConstReal(ctx.float_ty, value as f64),
                    ty: PrimitiveType::F32,
                });
            }
            if let Some(alias) = local_aliases.get(name) {
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, alias.ty),
                        alias.ptr,
                        b"alias_load\0".as_ptr().cast(),
                    ),
                    ty: alias.ty,
                });
            }
            if let Some(local) = locals.get(name) {
                return Ok(*local);
            }
            if let Some(byte_offset) = ctx.param_byte_offset.get(name).copied() {
                if byte_offset > i32::MAX as usize {
                    return Err(Diagnostic::internal(
                        "parameter byte offset exceeds supported i32 index range in ORC lowering",
                    ));
                }
                let param_ty = *ctx.param_types.get(name).unwrap_or(&PrimitiveType::F32);
                let ptr = build_typed_ptr_from_byte_offset(
                    ctx.builder,
                    ctx.context,
                    ctx.params_ptr,
                    const_i32(ctx.i32_ty, byte_offset as i32),
                    param_ty,
                    b"param_ptr_i8\0",
                    b"param_ptr_typed\0",
                );
                let raw = LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, param_ty),
                    ptr,
                    b"param_load\0".as_ptr().cast(),
                );
                return Ok(OrcValue {
                    value: raw,
                    ty: param_ty,
                });
            }
            if let Some(slot) = ctx.state_slots.get(name) {
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, slot.ty),
                        slot.ptr,
                        b"state_load_expr\0".as_ptr().cast(),
                    ),
                    ty: slot.ty,
                });
            }
            if let Some(slot) = ctx.out_slots.get(name) {
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, slot.ty),
                        slot.ptr,
                        b"out_load_expr\0".as_ptr().cast(),
                    ),
                    ty: slot.ty,
                });
            }
            if let Some(ch) = ctx.input_index.get(name) {
                let in_ty = *ctx.input_types.get(name).unwrap_or(&PrimitiveType::F32);
                let ch_v = LLVMConstInt(ctx.i32_ty, *ch as u64, 0);
                let in_ptr_ptr = build_ptr_offset(
                    ctx.builder,
                    ctx.float_ptr_ty,
                    ctx.in_ptrs,
                    ch_v,
                    b"in_ch_ptr_ptr\0",
                );
                let in_ch_ptr = LLVMBuildLoad2(
                    ctx.builder,
                    ctx.float_ptr_ty,
                    in_ptr_ptr,
                    b"in_ch_ptr\0".as_ptr().cast(),
                );
                let in_ch_ptr_typed = LLVMBuildBitCast(
                    ctx.builder,
                    in_ch_ptr,
                    LLVMPointerType(llvm_ty_for_primitive(ctx.context, in_ty), 0),
                    b"in_ch_ptr_typed\0".as_ptr().cast(),
                );
                let ptr = build_f32_ptr_offset(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, in_ty),
                    in_ch_ptr_typed,
                    ctx.frame_idx,
                    b"in_ptr\0",
                );
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, in_ty),
                        ptr,
                        b"in_load\0".as_ptr().cast(),
                    ),
                    ty: in_ty,
                });
            }
            if ctx.data_base_ptrs.contains_key(name) || ctx.data_struct_len.contains_key(name) {
                return Err(Diagnostic::internal(format!(
                    "Data symbol '{name}' must be indexed in ORC expression lowering"
                )));
            }
            if ctx.buffer_index.contains_key(name) {
                return Err(Diagnostic::internal(format!(
                    "buffer symbol '{name}' must be indexed in ORC expression lowering"
                )));
            }
            if ctx.input_arrays.contains_key(name)
                || ctx.param_arrays.contains_key(name)
                || ctx.output_arrays.contains_key(name)
            {
                return Err(Diagnostic::internal(format!(
                    "top-level array symbol '{name}' must be indexed in ORC expression lowering"
                )));
            }
            Err(Diagnostic::internal(format!(
                "unknown symbol '{name}' in ORC expression lowering"
            )))
        }
        Expr::Index { base, index } => {
            if ctx.buffer_index.contains_key(base) {
                let data = lower_buffer_element_ptr(
                    ctx,
                    base,
                    index,
                    locals,
                    local_aliases,
                    local_data_aliases,
                    true,
                )?;
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, data.elem_ty),
                        data.ptr,
                        b"buf_load\0".as_ptr().cast(),
                    ),
                    ty: data.elem_ty,
                });
            }
            if let Some(info) = ctx.input_arrays.get(base).copied() {
                return lower_input_array_index_read(
                    ctx,
                    info,
                    index,
                    locals,
                    local_aliases,
                    local_data_aliases,
                    true,
                );
            }
            if let Some(info) = ctx.param_arrays.get(base).copied() {
                return lower_param_array_index_read(
                    ctx,
                    base,
                    info,
                    index,
                    locals,
                    local_aliases,
                    local_data_aliases,
                    true,
                );
            }
            if ctx.output_arrays.contains_key(base) {
                let data = lower_output_array_element_ptr(
                    ctx,
                    base,
                    index,
                    locals,
                    local_aliases,
                    local_data_aliases,
                    true,
                )?;
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, data.elem_ty),
                        data.ptr,
                        b"out_arr_load\0".as_ptr().cast(),
                    ),
                    ty: data.elem_ty,
                });
            }
            let data = lower_data_element_ptr(
                ctx,
                base,
                index,
                locals,
                local_aliases,
                local_data_aliases,
            )?;
            Ok(OrcValue {
                value: LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, data.elem_ty),
                    data.ptr,
                    b"data_load\0".as_ptr().cast(),
                ),
                ty: data.elem_ty,
            })
        }
        Expr::DataCtor { .. } => Err(Diagnostic::internal(
            "Data constructor is only valid as an init assignment value",
        )),
        Expr::Binary { op, lhs, rhs } => {
            let left = lower_expr(lhs, ctx, locals, local_aliases, local_data_aliases)?;
            let right = lower_expr(rhs, ctx, locals, local_aliases, local_data_aliases)?;
            let Some(result_ty) = merge_numeric_primitive(left.ty, right.ty) else {
                return Err(Diagnostic::internal(format!(
                    "binary op requires numeric operands, got {:?} and {:?}",
                    left.ty, right.ty
                )));
            };
            let left_v = cast_orc_value_to(ctx, left, result_ty, b"bin_lhs_cast\0");
            let right_v = cast_orc_value_to(ctx, right, result_ty, b"bin_rhs_cast\0");
            let value = match result_ty {
                PrimitiveType::F32 | PrimitiveType::F64 => match op {
                    BinaryOp::Add => {
                        LLVMBuildFAdd(ctx.builder, left_v, right_v, b"fadd\0".as_ptr().cast())
                    }
                    BinaryOp::Sub => {
                        LLVMBuildFSub(ctx.builder, left_v, right_v, b"fsub\0".as_ptr().cast())
                    }
                    BinaryOp::Mul => {
                        LLVMBuildFMul(ctx.builder, left_v, right_v, b"fmul\0".as_ptr().cast())
                    }
                    BinaryOp::Div => {
                        LLVMBuildFDiv(ctx.builder, left_v, right_v, b"fdiv\0".as_ptr().cast())
                    }
                    BinaryOp::Mod => {
                        LLVMBuildFRem(ctx.builder, left_v, right_v, b"frem\0".as_ptr().cast())
                    }
                },
                PrimitiveType::I32 | PrimitiveType::I64 => match op {
                    BinaryOp::Add => {
                        LLVMBuildAdd(ctx.builder, left_v, right_v, b"iadd\0".as_ptr().cast())
                    }
                    BinaryOp::Sub => {
                        LLVMBuildSub(ctx.builder, left_v, right_v, b"isub\0".as_ptr().cast())
                    }
                    BinaryOp::Mul => {
                        LLVMBuildMul(ctx.builder, left_v, right_v, b"imul\0".as_ptr().cast())
                    }
                    BinaryOp::Div => {
                        LLVMBuildSDiv(ctx.builder, left_v, right_v, b"idiv\0".as_ptr().cast())
                    }
                    BinaryOp::Mod => {
                        LLVMBuildSRem(ctx.builder, left_v, right_v, b"irem\0".as_ptr().cast())
                    }
                },
                PrimitiveType::Bool => {
                    return Err(Diagnostic::internal(
                        "binary op does not support bool operands in ORC lowering",
                    ));
                }
            };
            Ok(OrcValue {
                value,
                ty: result_ty,
            })
        }
        Expr::Compare { op, lhs, rhs } => {
            let left = lower_expr(lhs, ctx, locals, local_aliases, local_data_aliases)?;
            let right = lower_expr(rhs, ctx, locals, local_aliases, local_data_aliases)?;
            let cmp = if left.ty == PrimitiveType::Bool && right.ty == PrimitiveType::Bool {
                let pred = match op {
                    CmpOp::Eq => LLVMIntPredicate::LLVMIntEQ,
                    CmpOp::Ne => LLVMIntPredicate::LLVMIntNE,
                    _ => {
                        return Err(Diagnostic::internal(
                            "bool comparisons only support == and != in ORC lowering",
                        ));
                    }
                };
                LLVMBuildICmp(
                    ctx.builder,
                    pred,
                    left.value,
                    right.value,
                    b"icmp_bool\0".as_ptr().cast(),
                )
            } else {
                let Some(result_ty) = merge_numeric_primitive(left.ty, right.ty) else {
                    return Err(Diagnostic::internal(format!(
                        "comparison requires compatible operands, got {:?} and {:?}",
                        left.ty, right.ty
                    )));
                };
                let left_v = cast_orc_value_to(ctx, left, result_ty, b"cmp_lhs_cast\0");
                let right_v = cast_orc_value_to(ctx, right, result_ty, b"cmp_rhs_cast\0");
                match result_ty {
                    PrimitiveType::F32 | PrimitiveType::F64 => {
                        let pred = match op {
                            CmpOp::Eq => LLVMRealPredicate::LLVMRealOEQ,
                            CmpOp::Ne => LLVMRealPredicate::LLVMRealONE,
                            CmpOp::Lt => LLVMRealPredicate::LLVMRealOLT,
                            CmpOp::Le => LLVMRealPredicate::LLVMRealOLE,
                            CmpOp::Gt => LLVMRealPredicate::LLVMRealOGT,
                            CmpOp::Ge => LLVMRealPredicate::LLVMRealOGE,
                        };
                        LLVMBuildFCmp(
                            ctx.builder,
                            pred,
                            left_v,
                            right_v,
                            b"fcmp\0".as_ptr().cast(),
                        )
                    }
                    PrimitiveType::I32 | PrimitiveType::I64 => {
                        let pred = match op {
                            CmpOp::Eq => LLVMIntPredicate::LLVMIntEQ,
                            CmpOp::Ne => LLVMIntPredicate::LLVMIntNE,
                            CmpOp::Lt => LLVMIntPredicate::LLVMIntSLT,
                            CmpOp::Le => LLVMIntPredicate::LLVMIntSLE,
                            CmpOp::Gt => LLVMIntPredicate::LLVMIntSGT,
                            CmpOp::Ge => LLVMIntPredicate::LLVMIntSGE,
                        };
                        LLVMBuildICmp(
                            ctx.builder,
                            pred,
                            left_v,
                            right_v,
                            b"icmp\0".as_ptr().cast(),
                        )
                    }
                    PrimitiveType::Bool => unreachable!(),
                }
            };
            Ok(OrcValue {
                value: cmp,
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Cast { to, expr } => {
            let value = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
            let casted = cast_orc_value_to(ctx, value, *to, b"cast\0");
            Ok(OrcValue {
                value: casted,
                ty: *to,
            })
        }
        Expr::UnaryNot { expr } => {
            let value = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
            let as_bool = cast_orc_value_to(ctx, value, PrimitiveType::Bool, b"not_bool\0");
            let one = LLVMConstInt(
                llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool),
                1,
                0,
            );
            let not_v = LLVMBuildXor(ctx.builder, as_bool, one, b"not\0".as_ptr().cast());
            Ok(OrcValue {
                value: not_v,
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Logical { op, lhs, rhs } => lower_orc_logical_expr(
            *op,
            lhs,
            rhs,
            ctx,
            locals,
            local_aliases,
            local_data_aliases,
        ),
        Expr::Call { func, args } => {
            let mut lowered = Vec::with_capacity(args.len());
            for arg in args {
                lowered.push(lower_expr(
                    arg,
                    ctx,
                    locals,
                    local_aliases,
                    local_data_aliases,
                )?);
            }
            lower_builtin_call_orc(ctx, *func, &lowered)
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            if name == "__omni_buffer_read2" {
                return lower_orc_buffer_read2_call(
                    args,
                    ctx,
                    locals,
                    local_aliases,
                    local_data_aliases,
                    true,
                );
            }
            if name == "__omni_buffer_write2" {
                return lower_orc_buffer_write2_call(
                    args,
                    ctx,
                    locals,
                    local_aliases,
                    local_data_aliases,
                    true,
                );
            }
            if let Some(base) = parse_data_len_instance_base(name) {
                return lower_orc_data_len_call(name, base, args, ctx, local_data_aliases);
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                return lower_orc_buffer_chans_call(name, base, args, ctx);
            }
            if name == "unsafe_read" {
                return lower_orc_unsafe_data_read_call(
                    args,
                    ctx,
                    locals,
                    local_aliases,
                    local_data_aliases,
                );
            }
            if name == "unsafe_write" {
                return lower_orc_unsafe_data_write_call(
                    args,
                    ctx,
                    locals,
                    local_aliases,
                    local_data_aliases,
                );
            }
            if ctx.struct_fields.contains_key(name) {
                return Err(Diagnostic::internal(format!(
                    "struct constructor '{name}(...)' used in scalar expression lowering"
                )));
            }
            let param_names = ctx.user_fn_param_names.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing parameter metadata for '{name}'"))
            })?;
            let param_defaults = ctx.user_fn_param_defaults.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing parameter default metadata for '{name}'"))
            })?;
            let param_kinds = ctx.user_fn_param_kinds.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing param kind metadata for '{name}'"))
            })?;
            let param_by_ref = ctx.user_fn_param_by_ref.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing by-ref metadata for '{name}'"))
            })?;
            if param_kinds.len() != param_names.len() || param_kinds.len() != param_defaults.len() {
                return Err(Diagnostic::internal(format!(
                    "function '{name}' has inconsistent metadata sizes in ORC expression lowering"
                )));
            }
            if param_by_ref.len() != param_kinds.len() {
                return Err(Diagnostic::internal(format!(
                    "function '{name}' by-ref metadata length {} does not match param metadata {} in ORC expression lowering",
                    param_by_ref.len(),
                    param_kinds.len()
                )));
            }
            let forbid_self_named = param_names.first().map(String::as_str) == Some("self");
            let resolved = resolve_call_args_codegen(
                args,
                param_names,
                param_defaults,
                forbid_self_named,
                &format!("function '{name}' call in ORC expression lowering"),
            )?;

            let mut scalar_values = Vec::new();
            let mut buffer_types = Vec::<(PrimitiveType, TypedBufferChannels)>::new();
            for (idx, kind) in param_kinds.iter().enumerate() {
                let resolved_arg = resolved.get(idx).copied().flatten();
                match kind {
                    TypedFnParam::Scalar => {
                        let value = if let Some(arg_expr) = resolved_arg {
                            lower_expr(arg_expr, ctx, locals, local_aliases, local_data_aliases)?
                        } else {
                            let default_expr = param_defaults
                                .get(idx)
                                .and_then(|d| d.as_ref())
                                .ok_or_else(|| {
                                    Diagnostic::internal(format!(
                                        "function '{name}' missing required argument '{}' in ORC expression lowering",
                                        param_names[idx]
                                    ))
                                })?;
                            let default_value = eval_const_default_expr(
                                default_expr,
                                ctx.sample_rate,
                                ctx.block_size,
                            )?;
                            OrcValue {
                                value: LLVMConstReal(ctx.float_ty, default_value as f64),
                                ty: PrimitiveType::F32,
                            }
                        };
                        scalar_values.push(value);
                    }
                    TypedFnParam::Buffer { elem_ty, channels } => {
                        let resolved_ty = if let Some(arg_expr) = resolved_arg {
                            infer_buffer_arg_signature_in_orc(ctx, arg_expr, name)?
                        } else {
                            (*elem_ty, channels.clone())
                        };
                        buffer_types.push(resolved_ty);
                    }
                    TypedFnParam::Struct { .. } => {}
                }
            }
            let mut scalar_types = scalar_values.iter().map(|v| v.ty).collect::<Vec<_>>();
            let explicit_type_args = resolve_explicit_call_type_args_for_codegen(
                name,
                "ORC expression lowering",
                type_args,
            )?;
            apply_explicit_generic_type_args_for_call(
                &*(ctx.user_registry as *const UserFnRegistry),
                name,
                &explicit_type_args,
                &mut scalar_types,
                "ORC expression lowering",
            )?;

            let (fn_ref, fn_ty, ret_ty) = ensure_user_fn_specialization(
                ctx.module,
                ctx.context,
                &mut *(ctx.user_registry as *mut UserFnRegistry),
                ctx.struct_fields,
                ctx.sample_rate,
                ctx.block_size as usize,
                name,
                &scalar_types,
                &buffer_types,
                &explicit_type_args,
            )?;

            let mut arg_values = Vec::new();
            let mut scalar_idx = 0usize;
            for (idx, kind) in param_kinds.iter().enumerate() {
                let resolved_arg = resolved.get(idx).copied().flatten();
                match kind {
                    TypedFnParam::Scalar => {
                        let target_ty = scalar_types[scalar_idx];
                        let value = cast_orc_value_to(
                            ctx,
                            scalar_values[scalar_idx],
                            target_ty,
                            b"call_arg\0",
                        );
                        scalar_idx += 1;
                        arg_values.push(value);
                    }
                    TypedFnParam::Struct { struct_name } => {
                        let arg_expr = resolved_arg.ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "function '{name}' missing required struct argument '{}' in ORC expression lowering",
                                param_names[idx]
                            ))
                        })?;
                        lower_struct_call_args_in_orc(
                            ctx,
                            &mut arg_values,
                            arg_expr,
                            struct_name,
                            name,
                            param_by_ref[idx],
                        )?;
                    }
                    TypedFnParam::Buffer { .. } => {
                        let arg_expr = resolved_arg.ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "function '{name}' missing required buffer argument '{}' in ORC expression lowering",
                                param_names[idx]
                            ))
                        })?;
                        lower_buffer_call_args_in_orc(ctx, &mut arg_values, arg_expr, name)?;
                    }
                }
            }
            Ok(OrcValue {
                value: LLVMBuildCall2(
                    ctx.builder,
                    fn_ty,
                    fn_ref,
                    arg_values.as_mut_ptr(),
                    arg_values.len() as u32,
                    b"call\0".as_ptr().cast(),
                ),
                ty: ret_ty,
            })
        }
    }
}

fn ensure_builtin_data_call_positional_arity(
    args: &[CallArg],
    name: &str,
    expected: usize,
    context: &str,
) -> Result<(), Diagnostic> {
    if args.len() != expected {
        return Err(Diagnostic::internal(format!(
            "builtin '{name}' expects {expected} positional arguments in {context}, got {}",
            args.len()
        )));
    }
    if args.iter().any(|a| a.name.is_some()) {
        return Err(Diagnostic::internal(format!(
            "builtin '{name}' does not support named arguments in {context}"
        )));
    }
    Ok(())
}

fn builtin_data_call_base_symbol<'a>(
    args: &'a [CallArg],
    name: &str,
    context: &str,
) -> Result<&'a str, Diagnostic> {
    let first = args.first().ok_or_else(|| {
        Diagnostic::internal(format!(
            "builtin '{name}' missing first argument in {context}"
        ))
    })?;
    match &first.expr {
        Expr::Var(base) => Ok(base.as_str()),
        _ => Err(Diagnostic::internal(format!(
            "builtin '{name}' requires a Data/buffer symbol variable as first argument in {context}"
        ))),
    }
}

fn ensure_internal_buffer_2d_call_positional_arity(
    args: &[CallArg],
    name: &str,
    expected: usize,
    context: &str,
) -> Result<(), Diagnostic> {
    if args.len() != expected {
        return Err(Diagnostic::internal(format!(
            "internal builtin '{name}' expects {expected} positional arguments in {context}, got {}",
            args.len()
        )));
    }
    if args.iter().any(|a| a.name.is_some()) {
        return Err(Diagnostic::internal(format!(
            "internal builtin '{name}' does not support named arguments in {context}"
        )));
    }
    Ok(())
}

fn parse_data_len_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "len" {
        return None;
    }
    Some(base)
}

fn parse_buffer_chans_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "chans" {
        return None;
    }
    Some(base)
}

fn ensure_builtin_instance_call_no_args(
    method_name: &str,
    args: &[CallArg],
    context: &str,
) -> Result<(), Diagnostic> {
    if !args.is_empty() {
        return Err(Diagnostic::internal(format!(
            "builtin method '{method_name}' expects 0 arguments in {context}, got {}",
            args.len()
        )));
    }
    if args.iter().any(|a| a.name.is_some()) {
        return Err(Diagnostic::internal(format!(
            "builtin method '{method_name}' does not support named arguments in {context}"
        )));
    }
    Ok(())
}

fn checked_len_const_i32(len: usize, context: &str) -> Result<u64, Diagnostic> {
    if len > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "Data length {len} exceeds i32 range in {context}"
        )));
    }
    Ok(len as u64)
}

fn local_data_alias_len(alias: &LocalDataAlias) -> usize {
    match alias {
        LocalDataAlias::Primitive { len, .. } | LocalDataAlias::Struct { len, .. } => *len,
    }
}

fn lookup_orc_data_symbol_len(
    ctx: &LoweringCtx<'_>,
    base: &str,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Option<usize> {
    if let Some(alias) = local_data_aliases.get(base) {
        return Some(local_data_alias_len(alias));
    }
    if let Some(info) = ctx.input_arrays.get(base) {
        return Some(info.len);
    }
    if let Some(info) = ctx.param_arrays.get(base) {
        return Some(info.len);
    }
    if let Some(info) = ctx.output_arrays.get(base) {
        return Some(info.len);
    }
    if let Some(len) = ctx.data_len.get(base) {
        return Some(*len);
    }
    ctx.data_struct_len.get(base).copied()
}

fn lookup_def_data_symbol_len(ctx: &DefLoweringCtx<'_>, base: &str) -> Option<usize> {
    if let Some(alias) = ctx.local_data_aliases.get(base) {
        return Some(local_data_alias_len(alias));
    }
    ctx.data_len.get(base).copied()
}

unsafe fn lower_orc_data_len_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "ORC expression lowering")?;
    if let Some(len) = lookup_orc_data_symbol_len(ctx, base, local_data_aliases) {
        let len_const = checked_len_const_i32(len, "ORC expression lowering")?;
        return Ok(OrcValue {
            value: LLVMConstInt(ctx.i32_ty, len_const, 0),
            ty: PrimitiveType::I32,
        });
    }
    if ctx.buffer_index.contains_key(base) {
        return Ok(OrcValue {
            value: load_orc_buffer_total_len_i32(ctx, base)?,
            ty: PrimitiveType::I32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a Data or buffer symbol receiver in ORC expression lowering, got '{base}'"
    )))
}

unsafe fn lower_orc_buffer_chans_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "ORC expression lowering")?;
    if ctx.buffer_index.contains_key(base) {
        let channels = if ctx.buffer_mono.contains(base) {
            LLVMConstInt(ctx.i32_ty, 1, 0)
        } else {
            load_orc_buffer_channels_i32(ctx, base)?
        };
        return Ok(OrcValue {
            value: channels,
            ty: PrimitiveType::I32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a buffer symbol receiver in ORC expression lowering, got '{base}'"
    )))
}

unsafe fn lower_def_data_len_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "def lowering")?;
    if let Some(len) = lookup_def_data_symbol_len(ctx, base) {
        let len_const = checked_len_const_i32(len, "def lowering")?;
        return Ok(OrcValue {
            value: LLVMConstInt(ctx.i32_ty, len_const, 0),
            ty: PrimitiveType::I32,
        });
    }
    if let Some(info) = ctx.buffer_params.get(base).cloned() {
        return Ok(OrcValue {
            value: load_def_buffer_total_len_i32(ctx, base, &info)?,
            ty: PrimitiveType::I32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a Data or buffer symbol receiver in def lowering, got '{base}'"
    )))
}

unsafe fn lower_orc_buffer_read2_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    ensure_internal_buffer_2d_call_positional_arity(
        args,
        "__omni_buffer_read2",
        3,
        "ORC expression lowering",
    )?;
    let base =
        builtin_data_call_base_symbol(args, "__omni_buffer_read2", "ORC expression lowering")?;
    let ch_expr = &args[1].expr;
    let sample_expr = &args[2].expr;
    let data = lower_buffer_element_ptr_2d(
        ctx,
        base,
        ch_expr,
        sample_expr,
        locals,
        local_aliases,
        local_data_aliases,
        clamp_index,
    )?;
    Ok(OrcValue {
        value: LLVMBuildLoad2(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data.elem_ty),
            data.ptr,
            b"buf2_read\0".as_ptr().cast(),
        ),
        ty: data.elem_ty,
    })
}

unsafe fn lower_orc_buffer_write2_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    ensure_internal_buffer_2d_call_positional_arity(
        args,
        "__omni_buffer_write2",
        4,
        "ORC expression lowering",
    )?;
    let base =
        builtin_data_call_base_symbol(args, "__omni_buffer_write2", "ORC expression lowering")?;
    let ch_expr = &args[1].expr;
    let sample_expr = &args[2].expr;
    let value_expr = &args[3].expr;
    let data = lower_buffer_element_ptr_2d(
        ctx,
        base,
        ch_expr,
        sample_expr,
        locals,
        local_aliases,
        local_data_aliases,
        clamp_index,
    )?;
    let value = lower_expr(value_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"buf2_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

unsafe fn lower_orc_unsafe_data_read_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_data_call_positional_arity(args, "unsafe_read", 2, "ORC expression lowering")?;
    let base = builtin_data_call_base_symbol(args, "unsafe_read", "ORC expression lowering")?;
    let index_expr = &args[1].expr;
    if let Some(info) = ctx.input_arrays.get(base).copied() {
        return lower_input_array_index_read(
            ctx,
            info,
            index_expr,
            locals,
            local_aliases,
            local_data_aliases,
            false,
        );
    }
    if let Some(info) = ctx.param_arrays.get(base).copied() {
        return lower_param_array_index_read(
            ctx,
            base,
            info,
            index_expr,
            locals,
            local_aliases,
            local_data_aliases,
            false,
        );
    }
    if ctx.output_arrays.contains_key(base) {
        let data = lower_output_array_element_ptr(
            ctx,
            base,
            index_expr,
            locals,
            local_aliases,
            local_data_aliases,
            false,
        )?;
        return Ok(OrcValue {
            value: LLVMBuildLoad2(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, data.elem_ty),
                data.ptr,
                b"unsafe_out_arr_read\0".as_ptr().cast(),
            ),
            ty: data.elem_ty,
        });
    }
    if ctx.buffer_index.contains_key(base) {
        let data = lower_buffer_element_ptr(
            ctx,
            base,
            index_expr,
            locals,
            local_aliases,
            local_data_aliases,
            false,
        )?;
        return Ok(OrcValue {
            value: LLVMBuildLoad2(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, data.elem_ty),
                data.ptr,
                b"unsafe_buf_read\0".as_ptr().cast(),
            ),
            ty: data.elem_ty,
        });
    }
    let data = lower_data_element_ptr_unchecked(
        ctx,
        base,
        index_expr,
        locals,
        local_aliases,
        local_data_aliases,
    )?;
    Ok(OrcValue {
        value: LLVMBuildLoad2(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data.elem_ty),
            data.ptr,
            b"unsafe_data_read\0".as_ptr().cast(),
        ),
        ty: data.elem_ty,
    })
}

unsafe fn lower_orc_unsafe_data_write_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_data_call_positional_arity(args, "unsafe_write", 3, "ORC expression lowering")?;
    let base = builtin_data_call_base_symbol(args, "unsafe_write", "ORC expression lowering")?;
    let index_expr = &args[1].expr;
    let value_expr = &args[2].expr;
    if ctx.input_arrays.contains_key(base) || ctx.param_arrays.contains_key(base) {
        return Err(Diagnostic::internal(format!(
            "unsafe_write cannot target immutable top-level array '{base}' in ORC lowering"
        )));
    }
    if ctx.output_arrays.contains_key(base) {
        let data = lower_output_array_element_ptr(
            ctx,
            base,
            index_expr,
            locals,
            local_aliases,
            local_data_aliases,
            false,
        )?;
        let value = lower_expr(value_expr, ctx, locals, local_aliases, local_data_aliases)?;
        let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"unsafe_out_arr_write_cast\0");
        LLVMBuildStore(ctx.builder, casted, data.ptr);
        return Ok(OrcValue {
            value: casted,
            ty: data.elem_ty,
        });
    }
    if ctx.buffer_index.contains_key(base) {
        let data = lower_buffer_element_ptr(
            ctx,
            base,
            index_expr,
            locals,
            local_aliases,
            local_data_aliases,
            false,
        )?;
        let value = lower_expr(value_expr, ctx, locals, local_aliases, local_data_aliases)?;
        let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"unsafe_buf_write_cast\0");
        LLVMBuildStore(ctx.builder, casted, data.ptr);
        return Ok(OrcValue {
            value: casted,
            ty: data.elem_ty,
        });
    }
    let data = lower_data_element_ptr_unchecked(
        ctx,
        base,
        index_expr,
        locals,
        local_aliases,
        local_data_aliases,
    )?;
    let value = lower_expr(value_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"unsafe_data_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

unsafe fn lower_def_unsafe_data_read_call(
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_data_call_positional_arity(args, "unsafe_read", 2, "def lowering")?;
    let base = builtin_data_call_base_symbol(args, "unsafe_read", "def lowering")?;
    let data = lower_def_data_element_ptr(ctx, base, &args[1].expr, false)?;
    Ok(OrcValue {
        value: LLVMBuildLoad2(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data.elem_ty),
            data.ptr,
            b"def_unsafe_data_read\0".as_ptr().cast(),
        ),
        ty: data.elem_ty,
    })
}

unsafe fn lower_def_unsafe_data_write_call(
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_data_call_positional_arity(args, "unsafe_write", 3, "def lowering")?;
    let base = builtin_data_call_base_symbol(args, "unsafe_write", "def lowering")?;
    let data = lower_def_data_element_ptr(ctx, base, &args[1].expr, false)?;
    let value = lower_def_expr(&args[2].expr, ctx)?;
    let casted = cast_def_value_to(ctx, value, data.elem_ty, b"def_unsafe_data_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

fn builtin_constant_value(name: &str, sample_rate: f32, block_size: f32) -> Option<f32> {
    match name {
        "PI" => Some(std::f32::consts::PI),
        "TWO_PI" | "TWOPI" => Some(2.0 * std::f32::consts::PI),
        "SAMPLE_RATE" | "SR" => Some(sample_rate),
        "BLOCK_SIZE" => Some(block_size),
        _ => None,
    }
}

fn eval_const_default_expr(
    expr: &Expr,
    sample_rate: f32,
    block_size: f32,
) -> Result<f32, Diagnostic> {
    match expr {
        Expr::Number(v) => Ok(*v),
        Expr::Int(v) => Ok(*v as f32),
        Expr::Bool(v) => Ok(if *v { 1.0 } else { 0.0 }),
        Expr::Var(name) => builtin_constant_value(name, sample_rate, block_size).ok_or_else(|| {
            Diagnostic::internal(format!(
                "default expression uses non-constant symbol '{name}' in codegen"
            ))
        }),
        Expr::Cast { to, expr } => {
            let v = eval_const_default_expr(expr, sample_rate, block_size)?;
            let out = match to {
                PrimitiveType::F32 | PrimitiveType::F64 => v,
                PrimitiveType::I32 => (v as i32) as f32,
                PrimitiveType::I64 => (v as i64) as f32,
                PrimitiveType::Bool => {
                    if v != 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            };
            Ok(out)
        }
        Expr::UnaryNot { expr } => {
            let v = eval_const_default_expr(expr, sample_rate, block_size)?;
            Ok(if v == 0.0 { 1.0 } else { 0.0 })
        }
        Expr::Logical { op, lhs, rhs } => {
            let l = eval_const_default_expr(lhs, sample_rate, block_size)?;
            match op {
                LogicalOp::And => {
                    if l == 0.0 {
                        Ok(0.0)
                    } else {
                        let r = eval_const_default_expr(rhs, sample_rate, block_size)?;
                        Ok(if r != 0.0 { 1.0 } else { 0.0 })
                    }
                }
                LogicalOp::Or => {
                    if l != 0.0 {
                        Ok(1.0)
                    } else {
                        let r = eval_const_default_expr(rhs, sample_rate, block_size)?;
                        Ok(if r != 0.0 { 1.0 } else { 0.0 })
                    }
                }
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let l = eval_const_default_expr(lhs, sample_rate, block_size)?;
            let r = eval_const_default_expr(rhs, sample_rate, block_size)?;
            let out = match op {
                BinaryOp::Add => l + r,
                BinaryOp::Sub => l - r,
                BinaryOp::Mul => l * r,
                BinaryOp::Div => l / r,
                BinaryOp::Mod => l % r,
            };
            Ok(out)
        }
        Expr::Compare { op, lhs, rhs } => {
            let l = eval_const_default_expr(lhs, sample_rate, block_size)?;
            let r = eval_const_default_expr(rhs, sample_rate, block_size)?;
            let pred = match op {
                CmpOp::Eq => l == r,
                CmpOp::Ne => l != r,
                CmpOp::Lt => l < r,
                CmpOp::Le => l <= r,
                CmpOp::Gt => l > r,
                CmpOp::Ge => l >= r,
            };
            Ok(if pred { 1.0 } else { 0.0 })
        }
        _ => Err(Diagnostic::internal(
            "default expression must be compile-time constant in codegen",
        )),
    }
}

fn eval_const_data_size_expr(
    expr: &Expr,
    sample_rate: f32,
    block_size: f32,
) -> Result<usize, Diagnostic> {
    let value = eval_const_default_expr(expr, sample_rate, block_size)?;
    if !value.is_finite() {
        return Err(Diagnostic::internal(
            "Data size expression must evaluate to a finite constant",
        ));
    }

    let truncated = value.trunc();
    if (value - truncated).abs() > 1e-6 {
        return Err(Diagnostic::internal(
            "Data size expression must evaluate to an integer constant",
        ));
    }
    if truncated <= 0.0 {
        return Err(Diagnostic::internal(
            "Data size expression must be greater than zero",
        ));
    }
    if truncated > usize::MAX as f32 {
        return Err(Diagnostic::internal(
            "Data size expression exceeds supported range",
        ));
    }

    Ok(truncated as usize)
}
fn resolve_call_args_codegen<'a>(
    args: &'a [CallArg],
    param_names: &[String],
    param_defaults: &[Option<Expr>],
    forbid_self_named: bool,
    context: &str,
) -> Result<Vec<Option<&'a Expr>>, Diagnostic> {
    if param_names.len() != param_defaults.len() {
        return Err(Diagnostic::internal(format!(
            "{context}: inconsistent parameter/default metadata"
        )));
    }

    let mut resolved = vec![None; param_names.len()];
    let mut next_pos = 0usize;
    let mut named_seen = HashSet::new();
    let mut saw_named = false;

    for arg in args {
        if let Some(name) = &arg.name {
            saw_named = true;
            if forbid_self_named && name == "self" {
                return Err(Diagnostic::internal(format!(
                    "{context}: 'self' cannot be passed as named argument"
                )));
            }
            if !named_seen.insert(name.clone()) {
                return Err(Diagnostic::internal(format!(
                    "{context}: duplicate named argument '{name}'"
                )));
            }
            let Some(idx) = param_names.iter().position(|p| p == name) else {
                return Err(Diagnostic::internal(format!(
                    "{context}: unknown named argument '{name}'"
                )));
            };
            if resolved[idx].is_some() {
                return Err(Diagnostic::internal(format!(
                    "{context}: argument '{name}' provided more than once"
                )));
            }
            resolved[idx] = Some(&arg.expr);
        } else {
            if saw_named {
                return Err(Diagnostic::internal(format!(
                    "{context}: positional arguments must precede named arguments"
                )));
            }
            while next_pos < resolved.len() && resolved[next_pos].is_some() {
                next_pos += 1;
            }
            if next_pos >= resolved.len() {
                return Err(Diagnostic::internal(format!(
                    "{context}: too many positional arguments (expected at most {})",
                    param_names.len()
                )));
            }
            resolved[next_pos] = Some(&arg.expr);
            next_pos += 1;
        }
    }

    for (idx, arg) in resolved.iter().enumerate() {
        if arg.is_none() && param_defaults[idx].is_none() {
            return Err(Diagnostic::internal(format!(
                "{context}: missing required argument '{}'",
                param_names[idx]
            )));
        }
    }

    Ok(resolved)
}

unsafe fn lower_struct_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
    by_ref: bool,
) -> Result<(), Diagnostic> {
    let base = match arg_expr {
        Expr::Var(name) => name,
        _ => {
            return Err(Diagnostic::internal(format!(
                "function '{callee_name}' expects struct '{struct_name}' argument as a variable reference in ORC lowering"
            )));
        }
    };
    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{struct_name}' in ORC call lowering for function '{callee_name}'"
        ))
    })?;
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(_) => {
                let slot = ctx.state_slots.get(&flat).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing state slot for struct field '{flat}' while calling '{callee_name}'"
                    ))
                })?;
                if by_ref {
                    out_args.push(slot.ptr);
                } else {
                    let value = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, slot.ty),
                        slot.ptr,
                        b"struct_arg_load\0".as_ptr().cast(),
                    );
                    out_args.push(value);
                }
            }
            TypedFieldType::Data(_) => {
                let mut symbols = Vec::<String>::new();
                if let Some(elem_struct) = &field.data_elem_struct {
                    let root_len = *ctx.data_struct_len.get(&flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing Data[Struct] length metadata for '{flat}' while lowering struct argument for '{callee_name}'"
                        ))
                    })?;
                    let mut roots = Vec::new();
                    let mut leaves = Vec::new();
                    collect_data_struct_bindings(
                        ctx.struct_fields,
                        elem_struct,
                        &flat,
                        root_len,
                        &mut roots,
                        &mut leaves,
                        &mut Vec::new(),
                    )?;
                    symbols.extend(leaves.into_iter().map(|(name, _, _)| name));
                } else {
                    symbols.push(flat.clone());
                }
                for symbol in symbols {
                    let data_base_ptr = *ctx.data_base_ptrs.get(&symbol).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing Data symbol '{symbol}' while lowering struct argument for '{callee_name}'"
                        ))
                    })?;
                    out_args.push(data_base_ptr);
                }
            }
        }
    }
    Ok(())
}

unsafe fn load_orc_buffer_binding_tuple(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<(LLVMValueRef, LLVMValueRef, LLVMValueRef), Diagnostic> {
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC buffer call argument lowering"
        )));
    };
    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let i8_ty = LLVMInt8TypeInContext(ctx.context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let ptr_ptr = build_ptr_offset(
        ctx.builder,
        i8_ptr_ty,
        ctx.buffer_ptrs,
        idx,
        b"call_buf_ptr_ptr\0",
    );
    let ptr = LLVMBuildLoad2(
        ctx.builder,
        i8_ptr_ty,
        ptr_ptr,
        b"call_buf_ptr\0".as_ptr().cast(),
    );
    let frames_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        ctx.buffer_frames_ptr,
        idx,
        b"call_buf_frames_ptr\0",
    );
    let frames = LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        frames_ptr,
        b"call_buf_frames\0".as_ptr().cast(),
    );
    let channels = if ctx.buffer_mono.contains(base) {
        LLVMConstInt(ctx.i32_ty, 1, 0)
    } else {
        let channels_ptr = build_ptr_offset(
            ctx.builder,
            ctx.i32_ty,
            ctx.buffer_channels_ptr,
            idx,
            b"call_buf_channels_ptr\0",
        );
        LLVMBuildLoad2(
            ctx.builder,
            ctx.i32_ty,
            channels_ptr,
            b"call_buf_channels\0".as_ptr().cast(),
        )
    };
    Ok((ptr, frames, channels))
}

unsafe fn lower_buffer_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let Expr::Var(base) = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in ORC expression lowering"
        )));
    };
    let (ptr, frames, channels) = load_orc_buffer_binding_tuple(ctx, base)?;
    out_args.push(ptr);
    out_args.push(frames);
    out_args.push(channels);
    Ok(())
}

fn infer_buffer_arg_signature_in_orc(
    ctx: &LoweringCtx<'_>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, TypedBufferChannels), Diagnostic> {
    let Expr::Var(base) = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in ORC expression lowering"
        )));
    };
    let elem_ty = *ctx.buffer_elem_types.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC buffer signature inference"
        ))
    })?;
    let channels = ctx.buffer_channels.get(base).cloned().ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing channel metadata for buffer symbol '{base}' in ORC buffer signature inference"
        ))
    })?;
    Ok((elem_ty, channels))
}

unsafe fn lower_buffer_call_args_in_def(
    ctx: &mut DefLoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let Expr::Var(base) = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in def lowering"
        )));
    };
    let info = ctx.buffer_params.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in def buffer call argument lowering"
        ))
    })?;
    out_args.push(info.ptr);
    out_args.push(info.frames);
    out_args.push(info.channels);
    Ok(())
}

fn infer_buffer_arg_signature_in_def(
    ctx: &DefLoweringCtx<'_>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, TypedBufferChannels), Diagnostic> {
    let Expr::Var(base) = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in def lowering"
        )));
    };
    let info = ctx.buffer_params.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in def buffer signature inference"
        ))
    })?;
    Ok((info.elem_ty, info.declared_channels.clone()))
}

unsafe fn lower_stmt(
    stmt: &Stmt,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_data_aliases: &mut HashMap<String, LocalDataAlias>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            is_typed_decl,
            expr,
            ..
        } => match target {
            AssignTarget::Var(name) => {
                if ctx.allow_struct_ctor {
                    if let Expr::UserCall {
                        name: struct_name,
                        args,
                        ..
                    } = expr
                    {
                        if let Some(fields) = ctx.struct_fields.get(struct_name).cloned() {
                            let scalar_param_names = fields
                                .iter()
                                .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
                                .map(|f| f.name.clone())
                                .collect::<Vec<_>>();
                            let scalar_defaults = fields
                                .iter()
                                .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
                                .map(|f| Some(f.default.clone().unwrap_or(Expr::Number(0.0))))
                                .collect::<Vec<_>>();
                            let resolved_scalar_args = resolve_call_args_codegen(
                                args,
                                &scalar_param_names,
                                &scalar_defaults,
                                false,
                                &format!("struct constructor '{struct_name}' in ORC init lowering"),
                            )?;
                            let mut scalar_arg_idx = 0usize;
                            for field in &fields {
                                let flat_target = format!("{name}.{}", field.name);
                                match field.ty {
                                    TypedFieldType::Scalar(_) => {
                                        let slot = ctx.state_slots.get(&flat_target).ok_or_else(|| {
                                                Diagnostic::internal(format!(
                                                    "missing state slot for struct field '{flat_target}' in ORC lowering"
                                                ))
                                            })?;
                                        let resolved_arg = resolved_scalar_args
                                            .get(scalar_arg_idx)
                                            .copied()
                                            .flatten();
                                        let value_typed = if let Some(arg_expr) = resolved_arg {
                                            let typed = lower_expr(
                                                arg_expr,
                                                ctx,
                                                locals,
                                                local_aliases,
                                                local_data_aliases,
                                            )?;
                                            cast_orc_value_to(ctx, typed, slot.ty, b"ctor_arg\0")
                                        } else {
                                            let default_expr = scalar_defaults
                                                    .get(scalar_arg_idx)
                                                    .and_then(|d| d.as_ref())
                                                    .ok_or_else(|| {
                                                        Diagnostic::internal(format!(
                                                            "struct constructor '{struct_name}' missing default for field '{}'",
                                                            field.name
                                                        ))
                                                    })?;
                                            let default_value = eval_const_default_expr(
                                                default_expr,
                                                ctx.sample_rate,
                                                ctx.block_size,
                                            )?;
                                            match slot.ty {
                                                PrimitiveType::F32 => LLVMConstReal(
                                                    ctx.float_ty,
                                                    default_value as f64,
                                                ),
                                                PrimitiveType::F64 => LLVMConstReal(
                                                    llvm_ty_for_primitive(
                                                        ctx.context,
                                                        PrimitiveType::F64,
                                                    ),
                                                    default_value as f64,
                                                ),
                                                PrimitiveType::I32 => LLVMConstInt(
                                                    llvm_ty_for_primitive(
                                                        ctx.context,
                                                        PrimitiveType::I32,
                                                    ),
                                                    (default_value as i32) as u64,
                                                    1,
                                                ),
                                                PrimitiveType::I64 => LLVMConstInt(
                                                    llvm_ty_for_primitive(
                                                        ctx.context,
                                                        PrimitiveType::I64,
                                                    ),
                                                    (default_value as i64) as u64,
                                                    1,
                                                ),
                                                PrimitiveType::Bool => LLVMConstInt(
                                                    llvm_ty_for_primitive(
                                                        ctx.context,
                                                        PrimitiveType::Bool,
                                                    ),
                                                    if default_value != 0.0 { 1 } else { 0 },
                                                    0,
                                                ),
                                            }
                                        };
                                        scalar_arg_idx += 1;
                                        LLVMBuildStore(ctx.builder, value_typed, slot.ptr);
                                    }
                                    TypedFieldType::Data(_) => {
                                        if !ctx.data_base_ptrs.contains_key(&flat_target)
                                            && !ctx.data_struct_len.contains_key(&flat_target)
                                        {
                                            return Err(Diagnostic::internal(format!(
                                                    "missing Data symbol '{flat_target}' in ORC lowering"
                                                )));
                                        }
                                    }
                                }
                            }
                            if scalar_arg_idx
                                != fields
                                    .iter()
                                    .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
                                    .count()
                            {
                                return Err(Diagnostic::internal(format!(
                                        "struct constructor '{struct_name}' scalar field mapping mismatch"
                                    )));
                            }
                            return Ok(());
                        }
                    }
                }

                if let Expr::DataCtor { spec, init } = expr {
                    let expected_len = if let Some(len) = ctx.data_len.get(name) {
                        *len
                    } else if let Some(len) = ctx.data_struct_len.get(name) {
                        *len
                    } else if *is_typed_decl {
                        if local_aliases.contains_key(name) || local_data_aliases.contains_key(name)
                        {
                            return Err(Diagnostic::internal(format!(
                                "typed array declaration for '{name}' conflicts with existing local symbol in ORC lowering"
                            )));
                        }
                        if locals.contains_key(name)
                            || ctx.out_slots.contains_key(name)
                            || ctx.state_slots.contains_key(name)
                            || ctx.param_byte_offset.contains_key(name)
                            || ctx.input_index.contains_key(name)
                            || ctx.buffer_index.contains_key(name)
                        {
                            return Err(Diagnostic::internal(format!(
                                "typed array declaration for '{name}' conflicts with existing symbol in ORC lowering"
                            )));
                        }
                        let elem_ty = match spec.elem {
                            omni_frontend::DataElemType::Primitive(elem_ty) => elem_ty,
                            omni_frontend::DataElemType::Struct(ref struct_name) => {
                                return Err(Diagnostic::internal(format!(
                                    "typed array declaration '{name}: {struct_name}[N]' is not yet supported in ORC lowering"
                                )))
                            }
                        };
                        let len =
                            eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
                        let ptr = build_local_array_slot(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, elem_ty),
                            len,
                            &format!("d_{name}"),
                        )?;
                        if let Some(values) = init {
                            if values.len() != len {
                                return Err(Diagnostic::internal(format!(
                                    "typed array declaration '{name}' initializer expects {len} elements, got {}",
                                    values.len()
                                )));
                            }
                            for (idx, value_expr) in values.iter().enumerate() {
                                let typed = lower_expr(
                                    value_expr,
                                    ctx,
                                    locals,
                                    local_aliases,
                                    local_data_aliases,
                                )?;
                                let casted = cast_orc_value_to(
                                    ctx,
                                    typed,
                                    elem_ty,
                                    b"local_arr_init_cast\0",
                                );
                                let idx_v = LLVMConstInt(ctx.i32_ty, idx as u64, 0);
                                let elem_ptr = build_f32_ptr_offset(
                                    ctx.builder,
                                    llvm_ty_for_primitive(ctx.context, elem_ty),
                                    ptr,
                                    idx_v,
                                    b"local_arr_init_ptr\0",
                                );
                                LLVMBuildStore(ctx.builder, casted, elem_ptr);
                            }
                        }
                        local_data_aliases.insert(
                            name.clone(),
                            LocalDataAlias::Primitive {
                                base_ptr: ptr,
                                len,
                                elem_ty,
                            },
                        );
                        return Ok(());
                    } else {
                        return Err(Diagnostic::internal(format!(
                            "Data constructor assigned to non-Data symbol '{name}'"
                        )));
                    };
                    let actual_len =
                        eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
                    if expected_len != actual_len {
                        return Err(Diagnostic::internal(format!(
                            "Data symbol '{name}' expected Data[{expected_len}] but got Data[{actual_len}]"
                        )));
                    }
                    return Ok(());
                }

                if let Some(alias) = local_aliases.get(name) {
                    let typed = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
                    let value = cast_orc_value_to(ctx, typed, alias.ty, b"alias_store_cast\0");
                    LLVMBuildStore(ctx.builder, value, alias.ptr);
                    return Ok(());
                }
                if local_data_aliases.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "Data alias '{name}' must be assigned via index syntax"
                    )));
                }
                if ctx.input_arrays.contains_key(name)
                    || ctx.param_arrays.contains_key(name)
                    || ctx.output_arrays.contains_key(name)
                {
                    return Err(Diagnostic::internal(format!(
                        "top-level array symbol '{name}' must be assigned via index syntax in ORC lowering"
                    )));
                }
                if ctx.buffer_index.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "buffer symbol '{name}' must be assigned via index syntax in ORC lowering"
                    )));
                }

                if !locals.contains_key(name)
                    && !local_aliases.contains_key(name)
                    && !local_data_aliases.contains_key(name)
                    && !ctx.out_slots.contains_key(name)
                    && !ctx.state_slots.contains_key(name)
                    && !ctx.param_byte_offset.contains_key(name)
                    && !ctx.input_index.contains_key(name)
                    && !ctx.data_base_ptrs.contains_key(name)
                    && !ctx.data_struct_len.contains_key(name)
                    && !ctx.buffer_index.contains_key(name)
                {
                    if let Expr::Index { base, index } = expr {
                        if let Some(struct_name) = ctx.data_struct_roots.get(base).cloned() {
                            let root_len = *ctx.data_struct_len.get(base).ok_or_else(|| {
                                Diagnostic::internal(format!(
                                    "missing Data[Struct] length metadata for '{base}'"
                                ))
                            })?;
                            let root_index = lower_clamped_data_index(
                                ctx,
                                index,
                                root_len,
                                locals,
                                local_aliases,
                                local_data_aliases,
                            )?;
                            bind_struct_data_element_aliases(
                                name,
                                &struct_name,
                                base,
                                root_index,
                                ctx,
                                local_aliases,
                                local_data_aliases,
                            )?;
                            return Ok(());
                        }

                        if let Some(alias) = local_data_aliases.get(base).cloned() {
                            match alias {
                                LocalDataAlias::Primitive { .. } => {
                                    return Err(Diagnostic::internal(format!(
                                        "local alias binding '{name} = {base}[...]' is not supported for primitive arrays in ORC lowering; use direct indexed access"
                                    )));
                                }
                                LocalDataAlias::Struct {
                                    root_base,
                                    elem_struct,
                                    len,
                                    start_index,
                                } => {
                                    let local_idx = lower_clamped_data_index(
                                        ctx,
                                        index,
                                        len,
                                        locals,
                                        local_aliases,
                                        local_data_aliases,
                                    )?;
                                    let global_idx = LLVMBuildAdd(
                                        ctx.builder,
                                        start_index,
                                        local_idx,
                                        b"data_alias_global_idx\0".as_ptr().cast(),
                                    );
                                    bind_struct_data_element_aliases(
                                        name,
                                        &elem_struct,
                                        &root_base,
                                        global_idx,
                                        ctx,
                                        local_aliases,
                                        local_data_aliases,
                                    )?;
                                }
                            }
                            return Ok(());
                        }

                        if ctx.data_base_ptrs.contains_key(base) {
                            return Err(Diagnostic::internal(format!(
                                "local alias binding '{name} = {base}[...]' is not supported for primitive arrays in ORC lowering; use direct indexed access"
                            )));
                        }
                    }
                }

                let typed = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
                if let Some(slot) = ctx.out_slots.get(name) {
                    let casted = cast_orc_value_to(ctx, typed, slot.ty, b"out_store_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot.ptr);
                    return Ok(());
                }
                if let Some(slot) = ctx.state_slots.get(name) {
                    let casted = cast_orc_value_to(ctx, typed, slot.ty, b"state_store_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot.ptr);
                    return Ok(());
                }
                if ctx.data_base_ptrs.contains_key(name) || ctx.data_struct_len.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "Data symbol '{name}' must be assigned via index syntax"
                    )));
                }
                if ctx.buffer_index.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "buffer symbol '{name}' must be assigned via index syntax"
                    )));
                }
                if !locals.contains_key(name)
                    && !local_data_aliases.contains_key(name)
                    && !ctx.input_index.contains_key(name)
                    && !ctx.param_byte_offset.contains_key(name)
                    && !ctx.input_arrays.contains_key(name)
                    && !ctx.param_arrays.contains_key(name)
                    && !ctx.output_arrays.contains_key(name)
                {
                    let target_ty = decl_ty.unwrap_or(typed.ty);
                    let slot = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, target_ty),
                        &format!("v_{name}"),
                    )?;
                    let casted =
                        cast_orc_value_to(ctx, typed, target_ty, b"local_store_new_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot);
                    local_aliases.insert(
                        name.clone(),
                        AliasSlot {
                            ptr: slot,
                            ty: target_ty,
                        },
                    );
                    return Ok(());
                }
                Err(Diagnostic::internal(format!(
                    "unknown assignment target '{name}' in ORC lowering"
                )))
            }
            AssignTarget::Index { base, index } => {
                let typed = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
                if ctx.input_arrays.contains_key(base) || ctx.param_arrays.contains_key(base) {
                    return Err(Diagnostic::internal(format!(
                        "cannot assign to immutable top-level array '{base}' in ORC lowering"
                    )));
                }
                let data = if ctx.output_arrays.contains_key(base) {
                    lower_output_array_element_ptr(
                        ctx,
                        base,
                        index,
                        locals,
                        local_aliases,
                        local_data_aliases,
                        true,
                    )?
                } else if ctx.buffer_index.contains_key(base) {
                    lower_buffer_element_ptr(
                        ctx,
                        base,
                        index,
                        locals,
                        local_aliases,
                        local_data_aliases,
                        true,
                    )?
                } else {
                    lower_data_element_ptr(
                        ctx,
                        base,
                        index,
                        locals,
                        local_aliases,
                        local_data_aliases,
                    )?
                };
                let casted = cast_orc_value_to(ctx, typed, data.elem_ty, b"data_store_cast\0");
                LLVMBuildStore(ctx.builder, casted, data.ptr);
                Ok(())
            }
        },
        Stmt::Expr { expr, .. } => {
            let _ = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
            Ok(())
        }
        Stmt::Return { .. } => Err(Diagnostic::internal(
            "return statement is only valid inside def lowering",
        )),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let cond_value = lower_expr(cond, ctx, locals, local_aliases, local_data_aliases)?;
            let cond_bool = lower_orc_condition(ctx, cond_value, b"if_cond\0");

            let then_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"if_then\0".as_ptr().cast(),
            );
            let else_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"if_else\0".as_ptr().cast(),
            );
            let merge_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"if_merge\0".as_ptr().cast(),
            );

            LLVMBuildCondBr(ctx.builder, cond_bool, then_bb, else_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, then_bb);
            let mut then_locals = locals.clone();
            let mut then_aliases = local_aliases.clone();
            let mut then_data_aliases = local_data_aliases.clone();
            for nested in then_branch {
                lower_stmt(
                    nested,
                    ctx,
                    &mut then_locals,
                    &mut then_aliases,
                    &mut then_data_aliases,
                )?;
            }
            LLVMBuildBr(ctx.builder, merge_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, else_bb);
            let mut else_locals = locals.clone();
            let mut else_aliases = local_aliases.clone();
            let mut else_data_aliases = local_data_aliases.clone();
            for nested in else_branch {
                lower_stmt(
                    nested,
                    ctx,
                    &mut else_locals,
                    &mut else_aliases,
                    &mut else_data_aliases,
                )?;
            }
            LLVMBuildBr(ctx.builder, merge_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, merge_bb);
            Ok(())
        }
        Stmt::For {
            var,
            start,
            end,
            body,
            ..
        } => {
            let preheader_bb = LLVMGetInsertBlock(ctx.builder);
            if preheader_bb.is_null() {
                return Err(Diagnostic::internal(
                    "failed to get current block for for-loop lowering",
                ));
            }

            let cond_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"for_cond\0".as_ptr().cast(),
            );
            let body_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"for_body\0".as_ptr().cast(),
            );
            let end_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"for_end\0".as_ptr().cast(),
            );

            let start_value = lower_expr(start, ctx, locals, local_aliases, local_data_aliases)?;
            let start_v =
                cast_orc_value_to(ctx, start_value, PrimitiveType::I32, b"for_start_i32\0");
            let end_value = lower_expr(end, ctx, locals, local_aliases, local_data_aliases)?;
            let end_v = cast_orc_value_to(ctx, end_value, PrimitiveType::I32, b"for_end_i32\0");

            LLVMBuildBr(ctx.builder, cond_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, cond_bb);
            let loop_i = LLVMBuildPhi(ctx.builder, ctx.i32_ty, b"for_i\0".as_ptr().cast());
            let mut incoming_vals = [start_v];
            let mut incoming_blocks = [preheader_bb];
            LLVMAddIncoming(
                loop_i,
                incoming_vals.as_mut_ptr(),
                incoming_blocks.as_mut_ptr(),
                1,
            );

            let cond = LLVMBuildICmp(
                ctx.builder,
                LLVMIntPredicate::LLVMIntSLT,
                loop_i,
                end_v,
                b"for_cmp\0".as_ptr().cast(),
            );
            LLVMBuildCondBr(ctx.builder, cond, body_bb, end_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, body_bb);
            let mut loop_locals = locals.clone();
            let mut loop_aliases = local_aliases.clone();
            let mut loop_data_aliases = local_data_aliases.clone();
            loop_locals.insert(
                var.clone(),
                OrcValue {
                    value: loop_i,
                    ty: PrimitiveType::I32,
                },
            );
            for nested in body {
                lower_stmt(
                    nested,
                    ctx,
                    &mut loop_locals,
                    &mut loop_aliases,
                    &mut loop_data_aliases,
                )?;
            }
            let body_end_bb = LLVMGetInsertBlock(ctx.builder);
            if body_end_bb.is_null() {
                return Err(Diagnostic::internal(
                    "failed to get for-loop body end block",
                ));
            }
            let next_i = LLVMBuildAdd(
                ctx.builder,
                loop_i,
                const_i32(ctx.i32_ty, 1),
                b"for_i_next\0".as_ptr().cast(),
            );
            LLVMBuildBr(ctx.builder, cond_bb);
            let mut back_vals = [next_i];
            let mut back_blocks = [body_end_bb];
            LLVMAddIncoming(loop_i, back_vals.as_mut_ptr(), back_blocks.as_mut_ptr(), 1);

            LLVMPositionBuilderAtEnd(ctx.builder, end_bb);
            Ok(())
        }
    }
}

unsafe fn lower_data_element_ptr(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<DataElementPtr, Diagnostic> {
    lower_data_element_ptr_with_bounds_mode(
        ctx,
        base,
        index_expr,
        locals,
        local_aliases,
        local_data_aliases,
        true,
    )
}

unsafe fn lower_data_element_ptr_unchecked(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<DataElementPtr, Diagnostic> {
    lower_data_element_ptr_with_bounds_mode(
        ctx,
        base,
        index_expr,
        locals,
        local_aliases,
        local_data_aliases,
        false,
    )
}

unsafe fn lower_array_index_i32(
    ctx: &mut LoweringCtx<'_>,
    index_expr: &Expr,
    len: usize,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<LLVMValueRef, Diagnostic> {
    let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let raw_index_i = cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"arr_idx_i32\0");
    if clamp_index {
        clamp_data_index(ctx.builder, ctx.i32_ty, raw_index_i, len)
    } else {
        Ok(raw_index_i)
    }
}

unsafe fn lower_input_array_index_read(
    ctx: &mut LoweringCtx<'_>,
    info: TypedArrayInfo,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    if info.offset > i32::MAX as usize {
        return Err(Diagnostic::internal(
            "input array offset exceeds supported i32 index range in ORC lowering",
        ));
    }
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        info.len,
        locals,
        local_aliases,
        local_data_aliases,
        clamp_index,
    )?;
    let offset_v = LLVMConstInt(ctx.i32_ty, info.offset as u64, 0);
    let ch = LLVMBuildAdd(ctx.builder, offset_v, idx, b"in_arr_ch\0".as_ptr().cast());
    let in_ptr_ptr = build_ptr_offset(
        ctx.builder,
        ctx.float_ptr_ty,
        ctx.in_ptrs,
        ch,
        b"in_arr_ch_ptr_ptr\0",
    );
    let in_ch_ptr = LLVMBuildLoad2(
        ctx.builder,
        ctx.float_ptr_ty,
        in_ptr_ptr,
        b"in_arr_ch_ptr\0".as_ptr().cast(),
    );
    let in_ch_ptr_typed = LLVMBuildBitCast(
        ctx.builder,
        in_ch_ptr,
        LLVMPointerType(llvm_ty_for_primitive(ctx.context, info.elem_ty), 0),
        b"in_arr_ch_ptr_typed\0".as_ptr().cast(),
    );
    let ptr = build_f32_ptr_offset(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, info.elem_ty),
        in_ch_ptr_typed,
        ctx.frame_idx,
        b"in_arr_ptr\0",
    );
    let raw = LLVMBuildLoad2(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, info.elem_ty),
        ptr,
        b"in_arr_load\0".as_ptr().cast(),
    );
    Ok(OrcValue {
        value: raw,
        ty: info.elem_ty,
    })
}

unsafe fn lower_param_array_index_read(
    ctx: &mut LoweringCtx<'_>,
    base_name: &str,
    info: TypedArrayInfo,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    let elem_bytes = primitive_type_bytes(info.elem_ty);
    let base_byte_offset = ctx
        .param_byte_offset
        .get(base_name)
        .copied()
        .ok_or_else(|| Diagnostic::internal(format!("unknown parameter array '{base_name}'")))?;
    if base_byte_offset > i32::MAX as usize {
        return Err(Diagnostic::internal(
            "parameter array offset exceeds supported i32 index range in ORC lowering",
        ));
    }
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        info.len,
        locals,
        local_aliases,
        local_data_aliases,
        clamp_index,
    )?;
    let offset_v = LLVMConstInt(ctx.i32_ty, base_byte_offset as u64, 0);
    let elem_bytes_v = LLVMConstInt(ctx.i32_ty, elem_bytes as u64, 0);
    let scaled = LLVMBuildMul(
        ctx.builder,
        idx,
        elem_bytes_v,
        b"param_arr_scaled\0".as_ptr().cast(),
    );
    let byte_off = LLVMBuildAdd(
        ctx.builder,
        offset_v,
        scaled,
        b"param_arr_byte_off\0".as_ptr().cast(),
    );
    let ptr = build_typed_ptr_from_byte_offset(
        ctx.builder,
        ctx.context,
        ctx.params_ptr,
        byte_off,
        info.elem_ty,
        b"param_arr_ptr_i8\0",
        b"param_arr_ptr_typed\0",
    );
    let raw = LLVMBuildLoad2(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, info.elem_ty),
        ptr,
        b"param_arr_load\0".as_ptr().cast(),
    );
    Ok(OrcValue {
        value: raw,
        ty: info.elem_ty,
    })
}

unsafe fn lower_output_array_element_ptr(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<DataElementPtr, Diagnostic> {
    let info = *ctx
        .output_arrays
        .get(base)
        .ok_or_else(|| Diagnostic::internal(format!("unknown output array '{base}'")))?;
    let base_ptr = *ctx.out_array_base_ptrs.get(base).ok_or_else(|| {
        Diagnostic::internal(format!("missing output array storage for '{base}'"))
    })?;
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        info.len,
        locals,
        local_aliases,
        local_data_aliases,
        clamp_index,
    )?;
    Ok(DataElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, info.elem_ty),
            base_ptr,
            idx,
            b"out_arr_elem_ptr\0",
        ),
        elem_ty: info.elem_ty,
    })
}

unsafe fn load_orc_buffer_total_len_i32(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC lowering"
        )));
    };
    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let frames_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        ctx.buffer_frames_ptr,
        idx,
        b"buf_frames_ptr\0",
    );
    let channels_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        ctx.buffer_channels_ptr,
        idx,
        b"buf_channels_ptr\0",
    );
    let frames = LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        frames_ptr,
        b"buf_frames\0".as_ptr().cast(),
    );
    if ctx.buffer_mono.contains(base) {
        return Ok(frames);
    }
    let channels = LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        channels_ptr,
        b"buf_channels\0".as_ptr().cast(),
    );
    Ok(LLVMBuildMul(
        ctx.builder,
        frames,
        channels,
        b"buf_total_len\0".as_ptr().cast(),
    ))
}

unsafe fn lower_buffer_element_ptr(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<DataElementPtr, Diagnostic> {
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC lowering"
        )));
    };
    let elem_ty = *ctx.buffer_elem_types.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing element type metadata for buffer '{base}' in ORC lowering"
        ))
    })?;

    let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let raw_index_i = cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"buf_idx_i32\0");
    let total_len = load_orc_buffer_total_len_i32(ctx, base)?;
    let final_index = if clamp_index {
        clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, raw_index_i, total_len)
    } else {
        raw_index_i
    };

    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let i8_ty = LLVMInt8TypeInContext(ctx.context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let ptr_ptr = build_ptr_offset(
        ctx.builder,
        i8_ptr_ty,
        ctx.buffer_ptrs,
        idx,
        b"buf_ptr_ptr\0",
    );
    let raw_ptr = LLVMBuildLoad2(
        ctx.builder,
        i8_ptr_ty,
        ptr_ptr,
        b"buf_ptr\0".as_ptr().cast(),
    );
    let typed_ptr = LLVMBuildBitCast(
        ctx.builder,
        raw_ptr,
        LLVMPointerType(llvm_ty_for_primitive(ctx.context, elem_ty), 0),
        b"buf_ptr_typed\0".as_ptr().cast(),
    );
    Ok(DataElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, elem_ty),
            typed_ptr,
            final_index,
            b"buf_elem_ptr\0",
        ),
        elem_ty,
    })
}

unsafe fn load_orc_buffer_channels_i32(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC lowering"
        )));
    };
    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let channels_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        ctx.buffer_channels_ptr,
        idx,
        b"buf_channels_ptr\0",
    );
    Ok(LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        channels_ptr,
        b"buf_channels\0".as_ptr().cast(),
    ))
}

unsafe fn lower_buffer_element_ptr_2d(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    channel_expr: &Expr,
    sample_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<DataElementPtr, Diagnostic> {
    let channel = lower_expr(channel_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let sample = lower_expr(sample_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let channel_i = cast_orc_value_to(ctx, channel, PrimitiveType::I32, b"buf_ch_i32\0");
    let sample_i = cast_orc_value_to(ctx, sample, PrimitiveType::I32, b"buf_sample_i32\0");
    let channels = load_orc_buffer_channels_i32(ctx, base)?;
    let total_len = load_orc_buffer_total_len_i32(ctx, base)?;
    let sample_off = LLVMBuildMul(
        ctx.builder,
        sample_i,
        channels,
        b"buf_sample_off\0".as_ptr().cast(),
    );
    let raw_flat = LLVMBuildAdd(
        ctx.builder,
        sample_off,
        channel_i,
        b"buf_flat_idx\0".as_ptr().cast(),
    );
    let flat_index = if clamp_index {
        clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, raw_flat, total_len)
    } else {
        raw_flat
    };
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC lowering"
        )));
    };
    let elem_ty = *ctx.buffer_elem_types.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing element type metadata for buffer '{base}' in ORC lowering"
        ))
    })?;
    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let i8_ty = LLVMInt8TypeInContext(ctx.context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let ptr_ptr = build_ptr_offset(
        ctx.builder,
        i8_ptr_ty,
        ctx.buffer_ptrs,
        idx,
        b"buf_ptr_ptr2\0",
    );
    let raw_ptr = LLVMBuildLoad2(
        ctx.builder,
        i8_ptr_ty,
        ptr_ptr,
        b"buf_ptr2\0".as_ptr().cast(),
    );
    let typed_ptr = LLVMBuildBitCast(
        ctx.builder,
        raw_ptr,
        LLVMPointerType(llvm_ty_for_primitive(ctx.context, elem_ty), 0),
        b"buf_ptr_typed2\0".as_ptr().cast(),
    );
    Ok(DataElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, elem_ty),
            typed_ptr,
            flat_index,
            b"buf_elem_ptr2\0",
        ),
        elem_ty,
    })
}

unsafe fn lower_data_element_ptr_with_bounds_mode(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
    clamp_index: bool,
) -> Result<DataElementPtr, Diagnostic> {
    if ctx.data_struct_len.contains_key(base) {
        return Err(Diagnostic::internal(format!(
            "Data symbol '{base}[...]' has struct elements; index it via an alias assignment first"
        )));
    }

    let (data_base_ptr, data_len, data_elem_ty) = if let Some(alias) = local_data_aliases.get(base)
    {
        match alias {
            LocalDataAlias::Primitive {
                base_ptr,
                len,
                elem_ty,
            } => (*base_ptr, *len, *elem_ty),
            LocalDataAlias::Struct { .. } => {
                return Err(Diagnostic::internal(format!(
                    "Data alias '{base}[...]' has struct elements; index it via an alias assignment first"
                )));
            }
        }
    } else {
        let data_len = *ctx.data_len.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing Data length for symbol '{base}' in ORC lowering"
            ))
        })?;
        let data_elem_ty = *ctx.data_elem_ty.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing Data element type for symbol '{base}' in ORC lowering"
            ))
        })?;
        let data_base_ptr = load_data_base_ptr_for_symbol(ctx, base)?;
        (data_base_ptr, data_len, data_elem_ty)
    };

    if data_len == 0 {
        return Err(Diagnostic::internal(format!(
            "Data symbol '{base}' has zero length in ORC lowering"
        )));
    }

    let final_index = if clamp_index {
        if let Some(const_idx) = try_constant_index_i64(index_expr) {
            LLVMConstInt(
                ctx.i32_ty,
                checked_constant_data_index_u64(
                    data_len,
                    const_idx,
                    &format!("Data index in ORC lowering for '{base}'"),
                )?,
                0,
            )
        } else {
            let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_data_aliases)?;
            let raw_index_i =
                cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"data_idx_i32\0");
            clamp_data_index(ctx.builder, ctx.i32_ty, raw_index_i, data_len)?
        }
    } else {
        let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_data_aliases)?;
        cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"data_idx_i32\0")
    };
    Ok(DataElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data_elem_ty),
            data_base_ptr,
            final_index,
            b"data_elem_ptr\0",
        ),
        elem_ty: data_elem_ty,
    })
}

unsafe fn load_data_base_ptr_for_symbol(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    ctx.data_base_ptrs.get(base).copied().ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown Data symbol '{base}' in ORC indexed lowering"
        ))
    })
}

unsafe fn lower_clamped_data_index(
    ctx: &mut LoweringCtx<'_>,
    index_expr: &Expr,
    len: usize,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<LLVMValueRef, Diagnostic> {
    let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let raw_index_i = cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"data_idx_i32\0");
    clamp_data_index(ctx.builder, ctx.i32_ty, raw_index_i, len)
}

unsafe fn build_data_segment_start_index(
    builder: LLVMBuilderRef,
    i32_ty: LLVMTypeRef,
    parent_index: LLVMValueRef,
    field_len: usize,
) -> Result<LLVMValueRef, Diagnostic> {
    if field_len > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "nested Data field length {field_len} exceeds supported i32 index range"
        )));
    }
    let stride = LLVMConstInt(i32_ty, field_len as u64, 0);
    Ok(LLVMBuildMul(
        builder,
        parent_index,
        stride,
        b"data_seg_start_idx\0".as_ptr().cast(),
    ))
}

unsafe fn load_data_ptr_at_index(
    ctx: &mut LoweringCtx<'_>,
    symbol: &str,
    elem_ty: PrimitiveType,
    index: LLVMValueRef,
    gep_name: &[u8],
) -> Result<LLVMValueRef, Diagnostic> {
    let data_base_ptr = load_data_base_ptr_for_symbol(ctx, symbol)?;
    Ok(build_f32_ptr_offset(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, elem_ty),
        data_base_ptr,
        index,
        gep_name,
    ))
}

#[allow(clippy::too_many_arguments)]
unsafe fn bind_struct_data_element_aliases(
    alias_name: &str,
    struct_name: &str,
    root_base: &str,
    global_index: LLVMValueRef,
    ctx: &mut LoweringCtx<'_>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_data_aliases: &mut HashMap<String, LocalDataAlias>,
) -> Result<(), Diagnostic> {
    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{}' while creating Data alias '{}'",
            struct_name, alias_name
        ))
    })?;
    for field in fields {
        let data_field_base = format!("{root_base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                let data_ptr = load_data_ptr_at_index(
                    ctx,
                    &data_field_base,
                    prim,
                    global_index,
                    b"struct_data_elem_ptr\0",
                )?;
                local_aliases.insert(
                    format!("{alias_name}.{}", field.name),
                    AliasSlot {
                        ptr: data_ptr,
                        ty: prim,
                    },
                );
            }
            TypedFieldType::Data(field_len) => {
                let start_idx = build_data_segment_start_index(
                    ctx.builder,
                    ctx.i32_ty,
                    global_index,
                    field_len,
                )?;
                if let Some(elem_struct) = &field.data_elem_struct {
                    if !ctx.data_struct_len.contains_key(&data_field_base) {
                        return Err(Diagnostic::internal(format!(
                            "missing nested Data[Struct] metadata for symbol '{data_field_base}'"
                        )));
                    }
                    local_data_aliases.insert(
                        format!("{alias_name}.{}", field.name),
                        LocalDataAlias::Struct {
                            root_base: data_field_base,
                            elem_struct: elem_struct.clone(),
                            len: field_len,
                            start_index: start_idx,
                        },
                    );
                } else {
                    let elem_ty = field.data_elem_ty.unwrap_or(PrimitiveType::F32);
                    let seg_base_ptr = load_data_ptr_at_index(
                        ctx,
                        &data_field_base,
                        elem_ty,
                        start_idx,
                        b"struct_data_seg_ptr\0",
                    )?;
                    local_data_aliases.insert(
                        format!("{alias_name}.{}", field.name),
                        LocalDataAlias::Primitive {
                            base_ptr: seg_base_ptr,
                            len: field_len,
                            elem_ty,
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

unsafe fn clamp_data_index(
    builder: LLVMBuilderRef,
    i32_ty: LLVMTypeRef,
    index: LLVMValueRef,
    len: usize,
) -> Result<LLVMValueRef, Diagnostic> {
    let max_index = len.saturating_sub(1);
    if max_index > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "Data length {len} exceeds supported index range (i32)"
        )));
    }
    let zero = LLVMConstInt(i32_ty, 0, 0);
    let max_v = LLVMConstInt(i32_ty, max_index as u64, 0);

    let below_zero = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSLT,
        index,
        zero,
        b"idx_below_zero\0".as_ptr().cast(),
    );
    let low_clamped = LLVMBuildSelect(
        builder,
        below_zero,
        zero,
        index,
        b"idx_low_clamped\0".as_ptr().cast(),
    );
    let above_max = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSGT,
        low_clamped,
        max_v,
        b"idx_above_max\0".as_ptr().cast(),
    );
    Ok(LLVMBuildSelect(
        builder,
        above_max,
        max_v,
        low_clamped,
        b"idx_clamped\0".as_ptr().cast(),
    ))
}

unsafe fn clamp_data_index_dynamic(
    builder: LLVMBuilderRef,
    i32_ty: LLVMTypeRef,
    index: LLVMValueRef,
    len: LLVMValueRef,
) -> LLVMValueRef {
    let zero = LLVMConstInt(i32_ty, 0, 0);
    let one = LLVMConstInt(i32_ty, 1, 0);
    let below_zero = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSLT,
        index,
        zero,
        b"idx_dyn_below_zero\0".as_ptr().cast(),
    );
    let low_clamped = LLVMBuildSelect(
        builder,
        below_zero,
        zero,
        index,
        b"idx_dyn_low_clamped\0".as_ptr().cast(),
    );
    let len_gt_zero = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSGT,
        len,
        zero,
        b"idx_dyn_len_gt_zero\0".as_ptr().cast(),
    );
    let len_safe = LLVMBuildSelect(
        builder,
        len_gt_zero,
        len,
        one,
        b"idx_dyn_len_safe\0".as_ptr().cast(),
    );
    let max_v = LLVMBuildSub(builder, len_safe, one, b"idx_dyn_max\0".as_ptr().cast());
    let above_max = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSGT,
        low_clamped,
        max_v,
        b"idx_dyn_above_max\0".as_ptr().cast(),
    );
    LLVMBuildSelect(
        builder,
        above_max,
        max_v,
        low_clamped,
        b"idx_dyn_clamped\0".as_ptr().cast(),
    )
}

fn try_constant_index_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(v) => Some(*v),
        Expr::Number(v) => {
            let truncated = v.trunc();
            if (v - truncated).abs() <= 1e-6 {
                Some(truncated as i64)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn checked_constant_data_index_u64(len: usize, idx: i64, context: &str) -> Result<u64, Diagnostic> {
    let max_index = len.saturating_sub(1);
    if max_index > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "Data length {len} exceeds supported index range (i32)"
        )));
    }
    if idx < 0 || idx > max_index as i64 {
        return Err(Diagnostic::internal(format!(
            "{context}: constant index {idx} is out of range (expected 0..{max_index})"
        )));
    }
    Ok(idx as u64)
}

unsafe fn const_i32(i32_ty: LLVMTypeRef, v: i32) -> LLVMValueRef {
    LLVMConstInt(i32_ty, v as i64 as u64, 1)
}

fn align_up(value: usize, align: usize) -> usize {
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

fn primitive_size_align(ty: PrimitiveType) -> (usize, usize) {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => (4, 4),
        PrimitiveType::F64 | PrimitiveType::I64 => (8, 8),
        PrimitiveType::Bool => (1, 1),
    }
}

fn compute_state_layout(typed: &TypedProgram) -> Result<Vec<StateLayoutEntry>, Diagnostic> {
    if typed.state_vars.len() != typed.state_types.len() {
        return Err(Diagnostic::internal(format!(
            "typed program state metadata mismatch: {} vars but {} types",
            typed.state_vars.len(),
            typed.state_types.len()
        )));
    }
    let mut layout = Vec::with_capacity(typed.state_vars.len());
    let mut offset = 0usize;
    for (name, ty) in typed.state_vars.iter().zip(typed.state_types.iter()) {
        let (size, align) = primitive_size_align(*ty);
        offset = align_up(offset, align);
        layout.push(StateLayoutEntry {
            name: name.clone(),
            ty: *ty,
            offset,
        });
        offset = offset.saturating_add(size);
    }
    Ok(layout)
}

fn compute_arrays_layout(
    typed: &TypedProgram,
    state_layout: &[StateLayoutEntry],
) -> Result<Vec<ArrayLayoutEntry>, Diagnostic> {
    let mut layout = Vec::with_capacity(typed.data_vars.len());
    let mut offset = state_total_size_bytes(state_layout, &[]);
    for data_var in &typed.data_vars {
        if data_var.len == 0 {
            return Err(Diagnostic::internal(format!(
                "array symbol '{}' cannot have zero length in ORC layout",
                data_var.name
            )));
        }
        let (elem_size, elem_align) = primitive_size_align(data_var.elem_ty);
        offset = align_up(offset, elem_align);
        layout.push(ArrayLayoutEntry {
            name: data_var.name.clone(),
            elem_ty: data_var.elem_ty,
            len: data_var.len,
            offset,
        });
        let byte_len = elem_size.checked_mul(data_var.len).ok_or_else(|| {
            Diagnostic::internal(format!(
                "array symbol '{}' byte size overflow in ORC layout",
                data_var.name
            ))
        })?;
        offset = offset.saturating_add(byte_len);
    }
    Ok(layout)
}

fn state_total_size_bytes(layout: &[StateLayoutEntry], arrays: &[ArrayLayoutEntry]) -> usize {
    let mut total = 0usize;
    for entry in layout {
        let (size, _) = primitive_size_align(entry.ty);
        total = total.max(entry.offset.saturating_add(size));
    }
    for entry in arrays {
        let (elem_size, _) = primitive_size_align(entry.elem_ty);
        total = total.max(
            entry
                .offset
                .saturating_add(elem_size.saturating_mul(entry.len)),
        );
    }
    total
}

fn state_layout_map(layout: &[StateLayoutEntry]) -> HashMap<String, (PrimitiveType, usize)> {
    layout
        .iter()
        .map(|e| (e.name.clone(), (e.ty, e.offset)))
        .collect()
}

fn arrays_layout_map(layout: &[ArrayLayoutEntry]) -> HashMap<String, (PrimitiveType, usize)> {
    layout
        .iter()
        .map(|e| (e.name.clone(), (e.elem_ty, e.offset)))
        .collect()
}

fn builtin_arity(func: BuiltinFn) -> usize {
    match func {
        BuiltinFn::Sin
        | BuiltinFn::Cos
        | BuiltinFn::Tan
        | BuiltinFn::Tanh
        | BuiltinFn::Atan
        | BuiltinFn::Exp
        | BuiltinFn::Log
        | BuiltinFn::Sqrt
        | BuiltinFn::Abs
        | BuiltinFn::Floor
        | BuiltinFn::Ceil
        | BuiltinFn::Round
        | BuiltinFn::Trunc => 1,
        BuiltinFn::Pow | BuiltinFn::Atan2 | BuiltinFn::Min | BuiltinFn::Max => 2,
        BuiltinFn::Fma => 3,
    }
}

fn builtin_intrinsic_name(func: BuiltinFn, use_f64: bool) -> &'static str {
    match func {
        BuiltinFn::Sin => {
            if use_f64 {
                "llvm.sin.f64"
            } else {
                "llvm.sin.f32"
            }
        }
        BuiltinFn::Cos => {
            if use_f64 {
                "llvm.cos.f64"
            } else {
                "llvm.cos.f32"
            }
        }
        BuiltinFn::Tan => {
            if use_f64 {
                "tan"
            } else {
                "tanf"
            }
        }
        BuiltinFn::Tanh => {
            if use_f64 {
                "tanh"
            } else {
                "tanhf"
            }
        }
        BuiltinFn::Atan => {
            if use_f64 {
                "atan"
            } else {
                "atanf"
            }
        }
        BuiltinFn::Atan2 => {
            if use_f64 {
                "atan2"
            } else {
                "atan2f"
            }
        }
        BuiltinFn::Exp => {
            if use_f64 {
                "llvm.exp.f64"
            } else {
                "llvm.exp.f32"
            }
        }
        BuiltinFn::Log => {
            if use_f64 {
                "llvm.log.f64"
            } else {
                "llvm.log.f32"
            }
        }
        BuiltinFn::Sqrt => {
            if use_f64 {
                "llvm.sqrt.f64"
            } else {
                "llvm.sqrt.f32"
            }
        }
        BuiltinFn::Pow => {
            if use_f64 {
                "llvm.pow.f64"
            } else {
                "llvm.pow.f32"
            }
        }
        BuiltinFn::Abs => {
            if use_f64 {
                "llvm.fabs.f64"
            } else {
                "llvm.fabs.f32"
            }
        }
        BuiltinFn::Floor => {
            if use_f64 {
                "llvm.floor.f64"
            } else {
                "llvm.floor.f32"
            }
        }
        BuiltinFn::Ceil => {
            if use_f64 {
                "llvm.ceil.f64"
            } else {
                "llvm.ceil.f32"
            }
        }
        BuiltinFn::Round => {
            if use_f64 {
                "llvm.round.f64"
            } else {
                "llvm.round.f32"
            }
        }
        BuiltinFn::Trunc => {
            if use_f64 {
                "llvm.trunc.f64"
            } else {
                "llvm.trunc.f32"
            }
        }
        BuiltinFn::Min => {
            if use_f64 {
                "llvm.minimum.f64"
            } else {
                "llvm.minimum.f32"
            }
        }
        BuiltinFn::Max => {
            if use_f64 {
                "llvm.maximum.f64"
            } else {
                "llvm.maximum.f32"
            }
        }
        BuiltinFn::Fma => {
            if use_f64 {
                "llvm.fma.f64"
            } else {
                "llvm.fma.f32"
            }
        }
    }
}

unsafe fn build_unary_f32_fn_type(context: LLVMContextRef) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let mut params = [float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

unsafe fn build_unary_f64_fn_type(context: LLVMContextRef) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMDoubleTypeInContext(context);
    let mut params = [float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

unsafe fn build_binary_f32_fn_type(context: LLVMContextRef) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let mut params = [float_ty, float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

unsafe fn build_binary_f64_fn_type(context: LLVMContextRef) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMDoubleTypeInContext(context);
    let mut params = [float_ty, float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

unsafe fn build_ternary_f32_fn_type(context: LLVMContextRef) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let mut params = [float_ty, float_ty, float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

unsafe fn build_ternary_f64_fn_type(context: LLVMContextRef) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMDoubleTypeInContext(context);
    let mut params = [float_ty, float_ty, float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

unsafe fn build_builtin_fn_type(
    context: LLVMContextRef,
    use_f64: bool,
    arity: usize,
) -> Result<LLVMTypeRef, Diagnostic> {
    match (use_f64, arity) {
        (false, 1) => build_unary_f32_fn_type(context),
        (true, 1) => build_unary_f64_fn_type(context),
        (false, 2) => build_binary_f32_fn_type(context),
        (true, 2) => build_binary_f64_fn_type(context),
        (false, 3) => build_ternary_f32_fn_type(context),
        (true, 3) => build_ternary_f64_fn_type(context),
        _ => Err(Diagnostic::internal(format!(
            "unsupported builtin intrinsic arity {arity}"
        ))),
    }
}

unsafe fn lower_builtin_call_def(
    ctx: &mut DefLoweringCtx<'_>,
    func: BuiltinFn,
    args: &[OrcValue],
) -> Result<OrcValue, Diagnostic> {
    let expected_arity = builtin_arity(func);
    if args.len() != expected_arity {
        return Err(Diagnostic::internal(format!(
            "builtin intrinsic call has wrong arity: expected {expected_arity}, got {}",
            args.len()
        )));
    }

    if func == BuiltinFn::Abs && matches!(args[0].ty, PrimitiveType::I32 | PrimitiveType::I64) {
        let result_ty = args[0].ty;
        let int_ty = llvm_ty_for_primitive(ctx.context, result_ty);
        let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
        let mut params = [int_ty, bool_ty];
        let fn_ty = LLVMFunctionType(int_ty, params.as_mut_ptr(), params.len() as u32, 0);
        let fn_name = if result_ty == PrimitiveType::I32 {
            "llvm.abs.i32"
        } else {
            "llvm.abs.i64"
        };
        let fn_ref = ensure_named_fn(ctx.module, fn_name, fn_ty)?;
        let arg0 = cast_def_value_to(ctx, args[0], result_ty, b"def_abs_int_arg\0");
        let no_poison = LLVMConstInt(bool_ty, 0, 0);
        let mut lowered_args = [arg0, no_poison];
        return Ok(OrcValue {
            value: LLVMBuildCall2(
                ctx.builder,
                fn_ty,
                fn_ref,
                lowered_args.as_mut_ptr(),
                lowered_args.len() as u32,
                b"def_abs_int\0".as_ptr().cast(),
            ),
            ty: result_ty,
        });
    }

    if matches!(func, BuiltinFn::Min | BuiltinFn::Max) {
        if let Some(result_ty) = merge_numeric_primitive(args[0].ty, args[1].ty) {
            if matches!(result_ty, PrimitiveType::I32 | PrimitiveType::I64) {
                let int_ty = llvm_ty_for_primitive(ctx.context, result_ty);
                let mut params = [int_ty, int_ty];
                let fn_ty = LLVMFunctionType(int_ty, params.as_mut_ptr(), params.len() as u32, 0);
                let fn_name = match (func, result_ty) {
                    (BuiltinFn::Min, PrimitiveType::I32) => "llvm.smin.i32",
                    (BuiltinFn::Min, PrimitiveType::I64) => "llvm.smin.i64",
                    (BuiltinFn::Max, PrimitiveType::I32) => "llvm.smax.i32",
                    (BuiltinFn::Max, PrimitiveType::I64) => "llvm.smax.i64",
                    _ => unreachable!("min/max integer lowering must use i32/i64"),
                };
                let fn_ref = ensure_named_fn(ctx.module, fn_name, fn_ty)?;
                let arg0 = cast_def_value_to(ctx, args[0], result_ty, b"def_minmax_int_l\0");
                let arg1 = cast_def_value_to(ctx, args[1], result_ty, b"def_minmax_int_r\0");
                let mut lowered_args = [arg0, arg1];
                return Ok(OrcValue {
                    value: LLVMBuildCall2(
                        ctx.builder,
                        fn_ty,
                        fn_ref,
                        lowered_args.as_mut_ptr(),
                        lowered_args.len() as u32,
                        b"def_minmax_int\0".as_ptr().cast(),
                    ),
                    ty: result_ty,
                });
            }
        }
    }

    let use_f64 = args.iter().any(|v| v.ty == PrimitiveType::F64);
    let result_ty = if use_f64 {
        PrimitiveType::F64
    } else {
        PrimitiveType::F32
    };
    let fn_ty = build_builtin_fn_type(ctx.context, use_f64, expected_arity)?;
    let fn_ref = ensure_named_fn(ctx.module, builtin_intrinsic_name(func, use_f64), fn_ty)?;
    let mut lowered_args = Vec::with_capacity(args.len());
    for arg in args {
        lowered_args.push(cast_def_value_to(
            ctx,
            *arg,
            result_ty,
            b"def_builtin_arg\0",
        ));
    }
    Ok(OrcValue {
        value: LLVMBuildCall2(
            ctx.builder,
            fn_ty,
            fn_ref,
            lowered_args.as_mut_ptr(),
            lowered_args.len() as u32,
            b"def_builtin_call\0".as_ptr().cast(),
        ),
        ty: result_ty,
    })
}

unsafe fn lower_builtin_call_orc(
    ctx: &mut LoweringCtx<'_>,
    func: BuiltinFn,
    args: &[OrcValue],
) -> Result<OrcValue, Diagnostic> {
    let expected_arity = builtin_arity(func);
    if args.len() != expected_arity {
        return Err(Diagnostic::internal(format!(
            "builtin intrinsic call has wrong arity: expected {expected_arity}, got {}",
            args.len()
        )));
    }

    if func == BuiltinFn::Abs && matches!(args[0].ty, PrimitiveType::I32 | PrimitiveType::I64) {
        let result_ty = args[0].ty;
        let int_ty = llvm_ty_for_primitive(ctx.context, result_ty);
        let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
        let mut params = [int_ty, bool_ty];
        let fn_ty = LLVMFunctionType(int_ty, params.as_mut_ptr(), params.len() as u32, 0);
        let fn_name = if result_ty == PrimitiveType::I32 {
            "llvm.abs.i32"
        } else {
            "llvm.abs.i64"
        };
        let fn_ref = ensure_named_fn(ctx.module, fn_name, fn_ty)?;
        let arg0 = cast_orc_value_to(ctx, args[0], result_ty, b"abs_int_arg\0");
        let no_poison = LLVMConstInt(bool_ty, 0, 0);
        let mut lowered_args = [arg0, no_poison];
        return Ok(OrcValue {
            value: LLVMBuildCall2(
                ctx.builder,
                fn_ty,
                fn_ref,
                lowered_args.as_mut_ptr(),
                lowered_args.len() as u32,
                b"abs_int\0".as_ptr().cast(),
            ),
            ty: result_ty,
        });
    }

    if matches!(func, BuiltinFn::Min | BuiltinFn::Max) {
        if let Some(result_ty) = merge_numeric_primitive(args[0].ty, args[1].ty) {
            if matches!(result_ty, PrimitiveType::I32 | PrimitiveType::I64) {
                let int_ty = llvm_ty_for_primitive(ctx.context, result_ty);
                let mut params = [int_ty, int_ty];
                let fn_ty = LLVMFunctionType(int_ty, params.as_mut_ptr(), params.len() as u32, 0);
                let fn_name = match (func, result_ty) {
                    (BuiltinFn::Min, PrimitiveType::I32) => "llvm.smin.i32",
                    (BuiltinFn::Min, PrimitiveType::I64) => "llvm.smin.i64",
                    (BuiltinFn::Max, PrimitiveType::I32) => "llvm.smax.i32",
                    (BuiltinFn::Max, PrimitiveType::I64) => "llvm.smax.i64",
                    _ => unreachable!("min/max integer lowering must use i32/i64"),
                };
                let fn_ref = ensure_named_fn(ctx.module, fn_name, fn_ty)?;
                let arg0 = cast_orc_value_to(ctx, args[0], result_ty, b"minmax_int_l\0");
                let arg1 = cast_orc_value_to(ctx, args[1], result_ty, b"minmax_int_r\0");
                let mut lowered_args = [arg0, arg1];
                return Ok(OrcValue {
                    value: LLVMBuildCall2(
                        ctx.builder,
                        fn_ty,
                        fn_ref,
                        lowered_args.as_mut_ptr(),
                        lowered_args.len() as u32,
                        b"minmax_int\0".as_ptr().cast(),
                    ),
                    ty: result_ty,
                });
            }
        }
    }

    let use_f64 = args.iter().any(|v| v.ty == PrimitiveType::F64);
    let result_ty = if use_f64 {
        PrimitiveType::F64
    } else {
        PrimitiveType::F32
    };
    let fn_ty = build_builtin_fn_type(ctx.context, use_f64, expected_arity)?;
    let fn_ref = ensure_named_fn(ctx.module, builtin_intrinsic_name(func, use_f64), fn_ty)?;
    let mut lowered_args = Vec::with_capacity(args.len());
    for arg in args {
        lowered_args.push(cast_orc_value_to(ctx, *arg, result_ty, b"builtin_arg\0"));
    }
    Ok(OrcValue {
        value: LLVMBuildCall2(
            ctx.builder,
            fn_ty,
            fn_ref,
            lowered_args.as_mut_ptr(),
            lowered_args.len() as u32,
            b"builtin_call\0".as_ptr().cast(),
        ),
        ty: result_ty,
    })
}

unsafe fn ensure_named_fn(
    module: LLVMModuleRef,
    name: &str,
    fn_ty: LLVMTypeRef,
) -> Result<LLVMValueRef, Diagnostic> {
    let cname = CString::new(name).map_err(|_| {
        Diagnostic::internal(format!("invalid function name '{name}' for ORC lowering"))
    })?;
    let existing = LLVMGetNamedFunction(module, cname.as_ptr());
    if !existing.is_null() {
        return Ok(existing);
    }

    let added = LLVMAddFunction(module, cname.as_ptr(), fn_ty);
    if added.is_null() {
        return Err(Diagnostic::internal(format!(
            "failed to add function '{name}' for ORC lowering"
        )));
    }
    Ok(added)
}

unsafe fn build_f32_ptr_offset(
    builder: LLVMBuilderRef,
    float_ty: LLVMTypeRef,
    base_ptr: LLVMValueRef,
    index: LLVMValueRef,
    name: &[u8],
) -> LLVMValueRef {
    build_ptr_offset(builder, float_ty, base_ptr, index, name)
}

unsafe fn build_ptr_offset(
    builder: LLVMBuilderRef,
    elem_ty: LLVMTypeRef,
    base_ptr: LLVMValueRef,
    index: LLVMValueRef,
    name: &[u8],
) -> LLVMValueRef {
    let mut indices = [index];
    LLVMBuildGEP2(
        builder,
        elem_ty,
        base_ptr,
        indices.as_mut_ptr(),
        1,
        name.as_ptr().cast(),
    )
}

unsafe fn build_i8_ptr_offset(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    base_ptr: LLVMValueRef,
    offset_bytes: usize,
    name: &[u8],
) -> LLVMValueRef {
    let i8_ty = LLVMInt8TypeInContext(context);
    let mut indices = [LLVMConstInt(
        LLVMInt64TypeInContext(context),
        offset_bytes as u64,
        0,
    )];
    LLVMBuildGEP2(
        builder,
        i8_ty,
        base_ptr,
        indices.as_mut_ptr(),
        1,
        name.as_ptr().cast(),
    )
}

unsafe fn build_typed_ptr_from_byte_offset(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    base_ptr: LLVMValueRef,
    offset_bytes_i32: LLVMValueRef,
    ty: PrimitiveType,
    i8_name: &[u8],
    typed_name: &[u8],
) -> LLVMValueRef {
    let i8_ty = LLVMInt8TypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let base_i8_ptr = LLVMBuildBitCast(builder, base_ptr, i8_ptr_ty, i8_name.as_ptr().cast());
    let byte_ptr = build_ptr_offset(builder, i8_ty, base_i8_ptr, offset_bytes_i32, i8_name);
    LLVMBuildBitCast(
        builder,
        byte_ptr,
        LLVMPointerType(llvm_ty_for_primitive(context, ty), 0),
        typed_name.as_ptr().cast(),
    )
}

unsafe fn build_typed_state_ptr(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    state_base_ptr: LLVMValueRef,
    offset_bytes: usize,
    ty: PrimitiveType,
    gep_name: &[u8],
    cast_name: &[u8],
) -> LLVMValueRef {
    let byte_ptr = build_i8_ptr_offset(builder, context, state_base_ptr, offset_bytes, gep_name);
    let ptr_ty = LLVMPointerType(llvm_ty_for_primitive(context, ty), 0);
    LLVMBuildBitCast(builder, byte_ptr, ptr_ty, cast_name.as_ptr().cast())
}

unsafe fn create_aggressive_jit_target_machine_builder(
) -> Result<LLVMOrcJITTargetMachineBuilderRef, Diagnostic> {
    let tm = create_host_target_machine_aggressive()?;
    let jtmb = LLVMOrcJITTargetMachineBuilderCreateFromTargetMachine(tm);
    if jtmb.is_null() {
        return Err(Diagnostic::internal(
            "failed to create ORC JIT target machine builder from aggressive target machine",
        ));
    }
    Ok(jtmb)
}

unsafe fn run_default_o3_pipeline(module: LLVMModuleRef) -> Result<(), Diagnostic> {
    let options = LLVMCreatePassBuilderOptions();
    if options.is_null() {
        return Err(Diagnostic::internal(
            "failed to create LLVM pass builder options",
        ));
    }

    let tm = match create_host_target_machine_aggressive() {
        Ok(v) => v,
        Err(diag) => {
            LLVMDisposePassBuilderOptions(options);
            return Err(diag);
        }
    };

    let pipeline = CString::new("default<O3>")
        .map_err(|_| Diagnostic::internal("invalid pass pipeline string"))?;
    let run_err = LLVMRunPasses(module, pipeline.as_ptr(), tm, options);
    LLVMDisposeTargetMachine(tm);
    LLVMDisposePassBuilderOptions(options);

    if !run_err.is_null() {
        return Err(llvm_error_to_diag(
            "failed to run LLVM pass pipeline default<O3>",
            run_err,
        ));
    }
    Ok(())
}

unsafe fn llvm_module_to_string(module: LLVMModuleRef) -> Result<String, Diagnostic> {
    let ir_ptr = LLVMPrintModuleToString(module);
    if ir_ptr.is_null() {
        return Err(Diagnostic::internal(
            "failed to print LLVM module to string",
        ));
    }
    let ir = CStr::from_ptr(ir_ptr).to_string_lossy().to_string();
    LLVMDisposeMessage(ir_ptr);
    Ok(ir)
}

unsafe fn create_host_target_machine_aggressive() -> Result<LLVMTargetMachineRef, Diagnostic> {
    let triple = LLVMGetDefaultTargetTriple();
    if triple.is_null() {
        return Err(Diagnostic::internal(
            "LLVMGetDefaultTargetTriple returned null",
        ));
    }
    let cpu = LLVMGetHostCPUName();
    if cpu.is_null() {
        LLVMDisposeMessage(triple);
        return Err(Diagnostic::internal("LLVMGetHostCPUName returned null"));
    }
    let features = LLVMGetHostCPUFeatures();
    if features.is_null() {
        LLVMDisposeMessage(cpu);
        LLVMDisposeMessage(triple);
        return Err(Diagnostic::internal("LLVMGetHostCPUFeatures returned null"));
    }

    let mut target: LLVMTargetRef = null_mut();
    let mut target_err: *mut i8 = null_mut();
    let target_status = LLVMGetTargetFromTriple(triple as *const i8, &mut target, &mut target_err);
    if target_status != 0 {
        let detail = if target_err.is_null() {
            "unknown target lookup error".to_owned()
        } else {
            let msg = CStr::from_ptr(target_err).to_string_lossy().to_string();
            LLVMDisposeMessage(target_err);
            msg
        };
        LLVMDisposeMessage(features);
        LLVMDisposeMessage(cpu);
        LLVMDisposeMessage(triple);
        return Err(Diagnostic::internal(format!(
            "LLVMGetTargetFromTriple failed: {detail}"
        )));
    }

    let tm = LLVMCreateTargetMachine(
        target,
        triple as *const i8,
        cpu as *const i8,
        features as *const i8,
        LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive,
        LLVMRelocMode::LLVMRelocDefault,
        LLVMCodeModel::LLVMCodeModelJITDefault,
    );
    LLVMDisposeMessage(features);
    LLVMDisposeMessage(cpu);
    LLVMDisposeMessage(triple);

    if tm.is_null() {
        return Err(Diagnostic::internal(
            "LLVMCreateTargetMachine returned null for aggressive host setup",
        ));
    }

    Ok(tm)
}

unsafe fn llvm_error_to_diag(prefix: &str, err: LLVMErrorRef) -> Diagnostic {
    if err.is_null() {
        return Diagnostic::internal(prefix);
    }
    let msg_ptr = LLVMGetErrorMessage(err);
    if msg_ptr.is_null() {
        return Diagnostic::internal(prefix);
    }
    let msg = CStr::from_ptr(msg_ptr).to_string_lossy().to_string();
    LLVMDisposeErrorMessage(msg_ptr);
    Diagnostic::internal(format!("{prefix}: {msg}"))
}

unsafe fn lookup_symbol(
    lljit: LLVMOrcLLJITRef,
    symbol: &str,
    what: &str,
) -> Result<LLVMOrcExecutorAddress, Diagnostic> {
    let symbol_name =
        CString::new(symbol).map_err(|_| Diagnostic::internal(format!("invalid {what} symbol")))?;
    let mut address: LLVMOrcExecutorAddress = 0;
    let lookup_err = LLVMOrcLLJITLookup(lljit, &mut address, symbol_name.as_ptr());
    if !lookup_err.is_null() {
        return Err(llvm_error_to_diag(
            &format!("failed to lookup compiled {what}"),
            lookup_err,
        ));
    }
    if address == 0 {
        return Err(Diagnostic::internal(format!(
            "LLJIT lookup returned null address for {what}"
        )));
    }
    Ok(address)
}

unsafe fn dispose_lljit_quiet(lljit: LLVMOrcLLJITRef) {
    if lljit.is_null() {
        return;
    }
    let err = LLVMOrcDisposeLLJIT(lljit);
    if err.is_null() {
        return;
    }
    let msg_ptr = LLVMGetErrorMessage(err);
    if !msg_ptr.is_null() {
        LLVMDisposeErrorMessage(msg_ptr);
    }
}
