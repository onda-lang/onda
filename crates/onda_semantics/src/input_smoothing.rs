//! Input attributes lower to ordinary stdlib instances. The input boundary owns
//! stepping and caching; reads never step a smoother.
use crate::*;
use onda_frontend::{InputSmoothingKind, Span};

pub(crate) fn templates(program: &Program) -> Vec<ProcessorDef> {
    program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Proc(proc)
                if matches!(
                    proc.name.as_str(),
                    "std::smoothing::Ramp" | "std::smoothing::Lag"
                ) =>
            {
                Some(proc.clone())
            }
            _ => None,
        })
        .collect()
}

struct Smoothers<'a> {
    templates: &'a [ProcessorDef],
    definitions: HashMap<String, ProcessorDef>,
}

fn instance_name(port: &str, index: usize) -> String {
    format!("__onda_input_smoother__{port}__{index}")
}

fn cache_name(port: &str) -> String {
    format!("__onda_smoothed_input__{port}")
}

fn assign(name: String, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Span::ZERO,
        target_loc: Span::ZERO,
        target: AssignTarget::Var(name),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: matches!(expr, Expr::ArrayCtor { .. }),
        typed_decl_ty_loc: Span::ZERO,
        param_write_lowered: false,
        expr,
    }
}

fn call(name: String, args: Vec<CallArg>) -> Expr {
    Expr::UserCall {
        loc: Span::ZERO,
        name,
        type_args: Vec::new(),
        args,
    }
}

fn argument(expr: Expr) -> CallArg {
    CallArg { name: None, expr }
}

fn prepare_ports(
    ports: &mut [PortDecl],
    options: AnalysisOptions,
    const_arrays: &HashMap<String, TypedArrayInfo>,
    top_level: bool,
    smoothers: &mut Smoothers<'_>,
    errors: &mut Vec<Diagnostic>,
) -> (Vec<Stmt>, Vec<Stmt>, HashMap<String, String>) {
    let mut init = Vec::new();
    let mut sample = Vec::new();
    let mut aliases = HashMap::new();
    for port in ports {
        let Some(smoothing) = &port.smoothing else {
            continue;
        };
        let context = format!("input '{}' smoothing duration", port.name);
        let ty = match port.ty.as_ref() {
            None => PrimitiveType::F32,
            Some(DeclType::Scalar(ty) | DeclType::Array { elem: ty, .. }) => *ty,
            _ => {
                errors.push(Diagnostic::semantic_span(
                    "input smoothing requires f32 or f64 inputs",
                    &port.loc,
                ));
                continue;
            }
        };
        if !is_float_type(ty) {
            errors.push(Diagnostic::semantic_span(
                "input smoothing requires f32 or f64 inputs",
                &port.loc,
            ));
            continue;
        }
        let Some(time) =
            eval_typed_const_expr(&smoothing.time_s, ty, options, &context, false, errors)
        else {
            continue;
        };
        if time.to_f64() < 0.0 {
            errors.push(Diagnostic::semantic_span(
                format!("{context} must be nonnegative"),
                smoothing.time_s.loc(),
            ));
            continue;
        }
        if time.to_f64() == 0.0 {
            port.smoothing = None;
            continue;
        }
        let kind = smoothing.kind;
        let processor = match kind {
            InputSmoothingKind::Ramp => "std::smoothing::Ramp",
            InputSmoothingKind::Lag => "std::smoothing::Lag",
        };
        let template = smoothers
            .templates
            .iter()
            .find(|proc| proc.name == processor)
            .expect("input smoothing stdlib was automatically imported");
        let Some(definition) = crate::generic_specialization::specialize_generic_proc_template(
            template,
            &[ty],
            errors,
        ) else {
            continue;
        };
        let processor = definition.name.clone();
        smoothers
            .definitions
            .entry(processor.clone())
            .or_insert(definition);
        let (_, _, specs, _) = expand_proc_port_specs(
            "<input smoothing>",
            std::slice::from_ref(port),
            "input",
            options,
            const_arrays,
            errors,
        );
        let Some(spec) = specs.first() else { continue };
        let mut values = Vec::new();
        for (index, slot) in spec.slots.iter().enumerate() {
            let instance = instance_name(&port.name, index);
            init.push(assign(
                instance.clone(),
                Expr::UserCall {
                    loc: port.loc.clone(),
                    name: processor.clone(),
                    type_args: Vec::new(),
                    args: vec![CallArg {
                        name: Some("time_s".to_owned()),
                        expr: typed_const_expr(time),
                    }],
                },
            ));
            let default = spec.defaults[index].clone().unwrap_or_else(|| {
                let value = typed_const_expr(coerce_const_default_to_typed(0.0, ty));
                spec.ranges[index].map_or(value.clone(), |range| clamp_expr_to_range(value, range))
            });
            init.push(Stmt::Expr {
                loc: port.loc.clone(),
                expr: call(format!("{instance}.reset"), vec![argument(default)]),
            });
            let raw = if top_level && spec.is_array {
                Expr::Index {
                    loc: port.loc.clone(),
                    base: port.name.clone(),
                    index: Box::new(Expr::int(index as i64)),
                }
            } else {
                Expr::var(slot.clone()).with_loc(port.loc.clone())
            };
            let raw = if top_level {
                spec.ranges[index].map_or(raw.clone(), |range| clamp_expr_to_range(raw, range))
            } else {
                raw
            };
            values.push(call(instance, vec![argument(raw)]));
        }
        if top_level {
            let alias = cache_name(&port.name);
            let value = if spec.is_array {
                Expr::ArrayCtor {
                    loc: port.loc.clone(),
                    spec: ArrayTypeSpec {
                        elem: ArrayElemType::Primitive(ty),
                        size: Box::new(Expr::int(values.len() as i64)),
                    },
                    init: Some(values),
                    init_is_value: false,
                    initialize: true,
                }
            } else {
                values.remove(0)
            };
            sample.push(assign(alias.clone(), value));
            aliases.insert(port.name.clone(), alias);
        }
    }
    (init, sample, aliases)
}

pub(crate) fn prepare(
    program: &mut Program,
    templates: &[ProcessorDef],
    options: AnalysisOptions,
    const_arrays: &HashMap<String, TypedArrayInfo>,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, String> {
    let mut init = Vec::new();
    let mut sample = Vec::new();
    let mut aliases = HashMap::new();
    let mut smoothers = Smoothers {
        templates,
        definitions: HashMap::new(),
    };
    for block in &mut program.blocks {
        match block {
            Block::Ins(ports) => {
                let (port_init, port_sample, port_aliases) =
                    prepare_ports(ports, options, const_arrays, true, &mut smoothers, errors);
                init.extend(port_init);
                sample.extend(port_sample);
                aliases.extend(port_aliases);
            }
            Block::Proc(proc) if proc.type_params.is_empty() => {
                let (mut prelude, _, _) = prepare_ports(
                    &mut proc.ins,
                    options,
                    const_arrays,
                    false,
                    &mut smoothers,
                    errors,
                );
                prelude.append(&mut proc.init.body);
                proc.init.body = prelude;
            }
            _ => {}
        }
    }
    let mut definitions = smoothers.definitions.into_values().collect::<Vec<_>>();
    definitions.sort_by(|a, b| a.name.cmp(&b.name));
    for definition in definitions {
        if !program
            .blocks
            .iter()
            .any(|block| matches!(block, Block::Proc(proc) if proc.name == definition.name))
        {
            program.blocks.push(Block::Proc(definition));
        }
    }
    if !init.is_empty() {
        if let Some(Block::Init(block)) = program
            .blocks
            .iter_mut()
            .find(|block| matches!(block, Block::Init(_)))
        {
            init.append(&mut block.body);
            block.body = init;
        } else {
            program.blocks.push(Block::Init(InitBlock {
                loc: Span::ZERO,
                default_ty: None,
                default_ty_loc: Span::ZERO,
                pinned_roots: Vec::new(),
                compiler_scratch_roots: Vec::new(),
                body: init,
            }));
        }
    }
    if !sample.is_empty() {
        let target = program.blocks.iter_mut().find_map(|block| match block {
            Block::Sample(block) => Some(&mut block.body),
            Block::Block(block) => block.sample.as_mut().map(|sample| &mut sample.body),
            _ => None,
        });
        if let Some(target) = target {
            sample.append(target);
            *target = sample;
        }
    }
    aliases
}

/// Scalar proc inputs are value parameters after source validation. Overwrite
/// those parameters once, after lowering the ordinary nested-smoother calls.
/// Flattened array and dynamic input reads then share exactly the same values.
pub(crate) fn proc_prelude(
    proc: &ProcessorDef,
    shape: &crate::processor_lowering::ProcLoweringShape,
    lower: impl FnOnce(Vec<Stmt>) -> Vec<Stmt>,
) -> Vec<Stmt> {
    let mut calls = Vec::new();
    let mut stores = Vec::new();
    for port in &proc.ins {
        if port.smoothing.is_none() {
            continue;
        }
        let Some(spec) = shape.in_ports.iter().find(|spec| spec.name == port.name) else {
            continue;
        };
        for (index, slot) in spec.slots.iter().enumerate() {
            let temporary = format!("{}__{index}", cache_name(&port.name));
            calls.push(assign(
                temporary.clone(),
                call(
                    instance_name(&port.name, index),
                    vec![argument(Expr::var(slot.clone()))],
                ),
            ));
            stores.push(assign(slot.clone(), Expr::var(temporary)));
        }
    }
    let mut body = lower(calls);
    body.extend(stores);
    body
}
