use std::collections::HashMap;

use super::*;

#[derive(Debug, Clone)]
pub(super) enum GraphValueType {
    Scalar(PrimitiveType),
    Array { elem_ty: PrimitiveType, len: usize },
}

#[derive(Debug, Clone)]
pub(super) struct GraphProcSurface {
    pub(super) proc_name: String,
    pub(super) api: ProcApi,
    pub(super) in_value_types: HashMap<String, GraphValueType>,
    pub(super) param_value_types: HashMap<String, GraphValueType>,
    pub(super) out_value_types: HashMap<String, GraphValueType>,
    pub(super) in_aliases: HashMap<String, String>,
    pub(super) out_aliases: HashMap<String, String>,
    pub(super) param_array_slots: HashMap<String, Vec<String>>,
    pub(super) out_array_slots: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub(super) struct GraphOwnerSurface {
    pub(super) input_value_types: HashMap<String, GraphValueType>,
    pub(super) param_value_types: HashMap<String, GraphValueType>,
    pub(super) output_value_types: HashMap<String, GraphValueType>,
    pub(super) input_aliases: HashMap<String, String>,
    pub(super) output_aliases: HashMap<String, String>,
}

pub(super) fn build_graph_proc_surfaces(
    program: &Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, GraphProcSurface> {
    let mut out = HashMap::<String, GraphProcSurface>::new();
    for proc in program.blocks.iter().filter_map(|block| match block {
        Block::Proc(proc) => Some(proc),
        _ => None,
    }) {
        let inferred_io = infer_numbered_io_from_sample(&proc.sample);
        let (graph_ins, in_aliases) =
            graph_port_decls_with_numbered_aliases(&proc.ins, "in", inferred_io.max_in);
        let (graph_outs, out_aliases) =
            graph_port_decls_with_numbered_aliases(&proc.outs, "out", inferred_io.max_out);
        let (_ins, _in_types, in_ports, _) =
            expand_proc_port_specs(&proc.name, &graph_ins, "ins", options, errors);
        let (outs, _, _, out_array_slots) =
            expand_proc_port_specs(&proc.name, &graph_outs, "outs", options, errors);
        let (param_specs, param_array_slots) =
            expand_proc_param_specs(&proc.name, &proc.params, options, errors);
        let params = param_specs
            .iter()
            .filter(|spec| !spec.is_private())
            .flat_map(|spec| spec.slots.iter().cloned())
            .map(|slot| (slot.name.clone(), slot))
            .collect::<HashMap<_, _>>();
        let has_bound_params = params.values().any(|slot| slot.bind.is_some());
        let private_param_names = proc
            .params
            .iter()
            .filter(|param| param.private)
            .map(|param| param.name.clone())
            .collect::<std::collections::HashSet<_>>();
        let param_array_slots = param_array_slots
            .into_iter()
            .filter(|(name, _)| !private_param_names.contains(name))
            .collect::<HashMap<_, _>>();
        out.insert(
            proc.name.clone(),
            GraphProcSurface {
                proc_name: proc.name.clone(),
                api: ProcApi {
                    ins: in_ports,
                    params,
                    has_bound_params,
                    outputs: ProcOutputs {
                        names: outs.clone(),
                        timing: proc.outs_timing,
                    },
                    events: HashMap::new(),
                    buffers: Vec::new(),
                    has_block: false,
                    sample_oversample_factor: 1,
                },
                in_value_types: value_types_from_ports(
                    &graph_ins, options, errors, &proc.name, "input",
                ),
                param_value_types: value_types_from_params(
                    &proc.params,
                    options,
                    errors,
                    &proc.name,
                    false,
                ),
                out_value_types: value_types_from_ports(
                    &graph_outs,
                    options,
                    errors,
                    &proc.name,
                    "output",
                ),
                in_aliases,
                out_aliases,
                param_array_slots,
                out_array_slots,
            },
        );
    }
    out
}

pub(super) fn graph_owner_surface_from_program(
    program: &Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> GraphOwnerSurface {
    let sample_body = match program.block(BlockKind::Sample) {
        Some(Block::Sample(sample)) => sample.body.clone(),
        _ => Vec::new(),
    };
    let inferred_io = infer_numbered_io_from_sample(&sample_body);
    let raw_ins = match program.block(BlockKind::Ins) {
        Some(Block::Ins(ports)) => ports.decls.clone(),
        _ => Vec::new(),
    };
    let raw_outs = match program.block(BlockKind::Outs) {
        Some(Block::Outs(ports)) => ports.decls.clone(),
        _ => Vec::new(),
    };
    let (graph_ins, input_aliases) =
        graph_port_decls_with_numbered_aliases(&raw_ins, "in", inferred_io.max_in);
    let (graph_outs, output_aliases) =
        graph_port_decls_with_numbered_aliases(&raw_outs, "out", inferred_io.max_out);
    GraphOwnerSurface {
        input_value_types: value_types_from_ports(
            &graph_ins,
            options,
            errors,
            "top-level",
            "input",
        ),
        param_value_types: match program.block(BlockKind::Params) {
            Some(Block::Params(params)) => {
                value_types_from_params(params, options, errors, "top-level", true)
            }
            _ => HashMap::new(),
        },
        output_value_types: value_types_from_ports(
            &graph_outs,
            options,
            errors,
            "top-level",
            "output",
        ),
        input_aliases,
        output_aliases,
    }
}

pub(super) fn graph_owner_surface_from_proc(
    proc: &ProcessorDef,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> GraphOwnerSurface {
    let inferred_io = infer_numbered_io_from_sample(&proc.sample);
    let (graph_ins, input_aliases) =
        graph_port_decls_with_numbered_aliases(&proc.ins, "in", inferred_io.max_in);
    let (graph_outs, output_aliases) =
        graph_port_decls_with_numbered_aliases(&proc.outs, "out", inferred_io.max_out);
    GraphOwnerSurface {
        input_value_types: value_types_from_ports(&graph_ins, options, errors, &proc.name, "input"),
        param_value_types: value_types_from_params(&proc.params, options, errors, &proc.name, true),
        output_value_types: value_types_from_ports(
            &graph_outs,
            options,
            errors,
            &proc.name,
            "output",
        ),
        input_aliases,
        output_aliases,
    }
}

fn graph_port_decls_with_numbered_aliases(
    explicit: &[PortDecl],
    prefix: &str,
    inferred_max: usize,
) -> (Vec<PortDecl>, HashMap<String, String>) {
    let mut ports = explicit.to_vec();
    let mut aliases = HashMap::<String, String>::new();
    let target_len = explicit.len().max(inferred_max);
    for idx in 0..target_len {
        let alias = format!("{prefix}{}", idx + 1);
        if let Some(port) = explicit.get(idx) {
            if port.name != alias {
                aliases.insert(alias, port.name.clone());
            }
        } else {
            ports.push(PortDecl {
                loc: Default::default(),
                name: alias,
                output_timing: None,
                output_timing_loc: Default::default(),
                ty: None,
                ty_loc: Default::default(),
                default: None,
                range: None,
            });
        }
    }
    (ports, aliases)
}

pub(super) fn resolve_graph_owner_input_name<'a>(
    owner: &'a GraphOwnerSurface,
    name: &'a str,
) -> &'a str {
    owner
        .input_aliases
        .get(name)
        .map(|s| s.as_str())
        .unwrap_or(name)
}

pub(super) fn resolve_graph_owner_output_name<'a>(
    owner: &'a GraphOwnerSurface,
    name: &'a str,
) -> &'a str {
    owner
        .output_aliases
        .get(name)
        .map(|s| s.as_str())
        .unwrap_or(name)
}

pub(super) fn resolve_graph_proc_input_name<'a>(
    surface: &'a GraphProcSurface,
    name: &'a str,
) -> &'a str {
    surface
        .in_aliases
        .get(name)
        .map(|s| s.as_str())
        .unwrap_or(name)
}

pub(super) fn resolve_graph_proc_output_name<'a>(
    surface: &'a GraphProcSurface,
    name: &'a str,
) -> &'a str {
    surface
        .out_aliases
        .get(name)
        .map(|s| s.as_str())
        .unwrap_or(name)
}

fn value_type_from_decl_type(
    ty: Option<&DeclType>,
    options: AnalysisOptions,
    size_context: Option<(&Expr, String)>,
    errors: &mut Vec<Diagnostic>,
) -> Option<GraphValueType> {
    match ty {
        None => Some(GraphValueType::Scalar(PrimitiveType::F32)),
        Some(DeclType::Scalar(ty)) => Some(GraphValueType::Scalar(*ty)),
        Some(DeclType::Generic(_)) => Some(GraphValueType::Scalar(PrimitiveType::F32)),
        Some(DeclType::Array { elem, .. }) => {
            let (size_expr, context) = size_context?;
            let len = eval_data_size_expr(size_expr, options, &context, errors)?;
            Some(GraphValueType::Array {
                elem_ty: *elem,
                len,
            })
        }
        Some(DeclType::Tuple(_)) => Some(GraphValueType::Scalar(PrimitiveType::F32)),
        Some(DeclType::ArrayGeneric { .. }) => {
            let (size_expr, context) = size_context?;
            let len = eval_data_size_expr(size_expr, options, &context, errors)?;
            Some(GraphValueType::Array {
                elem_ty: PrimitiveType::F32,
                len,
            })
        }
    }
}

fn value_types_from_ports(
    ports: &[PortDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
    owner_context: &str,
    kind: &str,
) -> HashMap<String, GraphValueType> {
    let mut out = HashMap::<String, GraphValueType>::new();
    for port in ports {
        let ty = match port.ty.as_ref() {
            Some(DeclType::Array { size, .. } | DeclType::ArrayGeneric { size, .. }) => {
                value_type_from_decl_type(
                    port.ty.as_ref(),
                    options,
                    Some((
                        size,
                        format!("{owner_context} graph {kind} '{}' size", port.name),
                    )),
                    errors,
                )
            }
            _ => value_type_from_decl_type(port.ty.as_ref(), options, None, errors),
        };
        if let Some(ty) = ty {
            out.insert(port.name.clone(), ty);
        }
    }
    out
}

fn value_types_from_params(
    params: &[ParamDecl],
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
    owner_context: &str,
    include_private: bool,
) -> HashMap<String, GraphValueType> {
    let mut out = HashMap::<String, GraphValueType>::new();
    for param in params {
        if param.private && !include_private {
            continue;
        }
        let ty = match param.ty.as_ref() {
            Some(DeclType::Array { size, .. } | DeclType::ArrayGeneric { size, .. }) => {
                value_type_from_decl_type(
                    param.ty.as_ref(),
                    options,
                    Some((
                        size,
                        format!("{owner_context} graph param '{}' size", param.name),
                    )),
                    errors,
                )
            }
            _ => value_type_from_decl_type(param.ty.as_ref(), options, None, errors),
        };
        if let Some(ty) = ty {
            out.insert(param.name.clone(), ty);
        }
    }
    out
}

pub(super) fn graph_value_type_label(ty: &GraphValueType) -> String {
    match ty {
        GraphValueType::Scalar(prim) => format!("{prim:?}"),
        GraphValueType::Array { elem_ty, len } => format!("{elem_ty:?}[{len}]"),
    }
}
