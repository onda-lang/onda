//! Shared source declarations and init-scope setup for nominal and physical shapes.
use super::*;

pub(super) struct ProcPortDeclarations {
    /// Keep whole-argument defaults for source signatures before slot expansion.
    pub declarations: Vec<PortDecl>,
    pub slots: Vec<String>,
    pub types: HashMap<String, PrimitiveType>,
    pub specs: Vec<ProcPortSpec>,
    pub arrays: HashMap<String, Vec<String>>,
}

pub(super) struct ProcDeclarations {
    pub inputs: ProcPortDeclarations,
    pub outputs: ProcPortDeclarations,
    pub params: Vec<ProcParamSpec>,
    pub param_arrays: HashMap<String, Vec<String>>,
    pub buffers: Vec<TypedBufferDecl>,
}

/// Bind named constructor arguments in parameter-then-buffer declaration order.
/// Missing parameter values are expanded from their slot defaults by the caller.
pub(super) fn resolve_constructor_args<'a>(
    arguments: &'a [CallArg],
    params: &[ProcParamSpec],
    buffers: &[TypedBufferDecl],
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Option<&'a Expr>> {
    let names = params
        .iter()
        .map(|param| param.name.clone())
        .chain(buffers.iter().map(|buffer| buffer.name.clone()))
        .collect::<Vec<_>>();
    // The argument resolver needs only default presence; the actual defaults
    // belong to the individual slots, including whole-array parameters.
    let defaults = params
        .iter()
        .map(|param| {
            param
                .slots
                .iter()
                .all(|slot| slot.default.is_some())
                .then(|| Expr::number(0.0))
        })
        .chain(buffers.iter().map(|_| None))
        .collect::<Vec<_>>();
    resolve_call_args(arguments, &names, &defaults, false, true, context, errors)
}

pub(super) fn prepare_proc_declarations(
    proc: &ProcessorDef,
    options: AnalysisOptions,
    const_arrays: &HashMap<String, TypedArrayInfo>,
    errors: &mut Vec<Diagnostic>,
) -> ProcDeclarations {
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let inferred_names = infer_numbered_names_from_proc(proc);
    let inputs = normalize_numbered_port_decls(&proc.ins, "in", inferred_io.max_in);
    let outputs = normalize_numbered_port_decls(
        &proc.outs,
        proc_output_numbered_prefix(proc),
        match proc.outs_timing {
            OutputTiming::Sample => inferred_io.max_out,
            OutputTiming::Block => inferred_names.max_kout,
        },
    );
    let params = normalize_numbered_param_decls(&proc.params, "param", inferred_names.max_param);
    let mut ports = |declarations: Vec<PortDecl>, kind| {
        let (slots, types, specs, arrays) = expand_proc_port_specs(
            &proc.name,
            &declarations,
            kind,
            options,
            const_arrays,
            errors,
        );
        ProcPortDeclarations {
            declarations,
            slots,
            types,
            specs,
            arrays,
        }
    };
    let inputs = ports(inputs, "input");
    let outputs = ports(outputs, "output");
    let (params, param_arrays) =
        expand_proc_param_specs(&proc.name, &params, options, const_arrays, errors);
    let buffers = coerce_buffers(&proc.buffers, options, errors);
    ProcDeclarations {
        inputs,
        outputs,
        params,
        param_arrays,
        buffers,
    }
}

pub(super) fn bind_proc_buffer_symbols(
    buffers: &[TypedBufferDecl],
    symbols: &mut DeclaredSymbolMap,
) {
    for buffer in buffers {
        symbols.insert(
            buffer.name.clone(),
            DeclaredSymbolInfo::Buffer {
                elem_ty: buffer.elem_ty,
                channels: match buffer.channels {
                    TypedBufferChannels::Mono => BufferChannelInfo::Mono,
                    TypedBufferChannels::Static(count) => BufferChannelInfo::Static(count),
                    TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
                },
                array_len: buffer.array_len,
                is_array: buffer.is_array,
            },
        );
    }
}

pub(super) struct ProcConstructors<'a> {
    pub reserved: &'a HashSet<String>,
    pub procs: &'a HashSet<String>,
    pub struct_symbols: &'a HashSet<String>,
    pub structs: &'a HashMap<String, StructDef>,
    pub symbols: &'a HashSet<String>,
}

pub(super) fn analyze_processor_init(
    proc: &ProcessorDef,
    common: ScopeAnalysisCtx<'_>,
    constructors: ProcConstructors<'_>,
    state: &mut InitAnalysisState,
    errors: &mut Vec<Diagnostic>,
) {
    let label = format!("processor '{}'", proc.name);
    let namespace = namespace_of_symbol(&proc.name);
    let context = InitAnalysisCtx {
        storage_struct_defs: common.struct_defs,
        init_default_ty: resolve_init_default_ty(proc.init.default_ty.as_ref(), &label, errors),
        context_label: &label,
        common,
        proc_resolution: Some(ProcResolutionCtx {
            owner_proc_name: &proc.name,
            reserved: constructors.reserved,
            current_ns: &namespace,
            proc_symbols: constructors.procs,
            struct_symbols: constructors.struct_symbols,
            frontend_struct_defs: constructors.structs,
            ctor_symbols: constructors.symbols,
            in_init_scope: true,
        }),
        top_level_proc_symbols: None,
    };
    analyze_owner_init_stmts(&proc.init, &context, &HashSet::new(), state, errors);
}
