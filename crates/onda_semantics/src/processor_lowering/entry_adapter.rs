//! Project one ordinary entry processor onto the existing host program interface.
//! Emit ordinary declarations, construction, calls and stores; proc lowering owns
//! execution semantics, including port clamping and oversampling.
//! Delegate declarations must precede delegate preparation; the remaining wiring
//! uses checked proc shapes. Scheduling is finalized after automatic child hooks.
use super::*;
use onda_frontend::{DelegateBlock, EventBlock, ParamControl, Span, WhenBinding, WhenTarget};

const INSTANCE: &str = "__onda_entry";
const REFRESH: &str = "__onda_entry_refresh";

pub(super) struct EntryAdapter {
    loc: Span,
    locations: HashMap<String, Span>,
    controls: HashMap<String, ParamControl>,
    events: Vec<EventDef>,
    delegates: Vec<DelegateDef>,
    interface_names: HashMap<String, String>,
}

#[derive(Default)]
pub(super) struct EntryExports {
    pub(super) authored_instance_roots: HashSet<String>,
    pub(super) interface_names: HashMap<String, String>,
}

impl EntryAdapter {
    pub(super) fn prepare(
        program: &mut Program,
        options: AnalysisOptions,
        constants: Option<&std::rc::Rc<crate::pipeline::ConstScope>>,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<Self> {
        let source = program.blocks.iter().find_map(|block| match block {
            Block::Proc(proc) if proc.name == "Main" => Some(proc),
            _ => None,
        })?;
        if !source.type_params.is_empty() {
            errors.push(Diagnostic::semantic_span(
                "top-level proc Main cannot be generic",
                &source.loc,
            ));
            return None;
        }
        let adapter = Self {
            loc: source.loc.clone(),
            locations: source
                .ins
                .iter()
                .chain(&source.outs)
                .map(|decl| (decl.name.clone(), decl.loc.clone()))
                .chain(
                    source
                        .params
                        .iter()
                        .map(|decl| (decl.name.clone(), decl.loc.clone())),
                )
                .chain(
                    source
                        .buffers
                        .iter()
                        .map(|decl| (decl.name.clone(), decl.loc.clone())),
                )
                .collect(),
            controls: source
                .params
                .iter()
                .map(|decl| (decl.name.clone(), decl.control.clone()))
                .collect(),
            events: source
                .events
                .iter()
                .filter(|event| !is_builtin_proc_init_event_name(&event.name))
                .map(|event| EventDef {
                    loc: event.loc.clone(),
                    name: event.name.clone(),
                    params: event.params.clone(),
                    body: Vec::new(),
                })
                .collect(),
            delegates: source.delegates.clone(),
            interface_names: HashMap::new(),
        };
        let mut delegates = adapter.delegates.clone();
        for (index, delegate) in delegates.iter_mut().enumerate() {
            delegate.name = delegate_function_name(index);
            materialize_message_defaults(&mut delegate.params, constants, options, errors);
        }
        if !delegates.is_empty() {
            program.blocks.push(Block::Delegates(DelegateBlock {
                loc: Default::default(),
                delegates,
            }));
        }
        Some(adapter)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn connect(
        &mut self,
        program: &mut Program,
        shape: &ProcLoweringShape,
        api: &ProcApi,
        options: AnalysisOptions,
        constants: Option<&std::rc::Rc<crate::pipeline::ConstScope>>,
        runtime_defs: &mut HashSet<String>,
        errors: &mut Vec<Diagnostic>,
    ) {
        let declaration_options =
            proc_runtime_analysis_options(options, api.sample_oversample_factor);
        let mut ins = project_ports(
            &shape.in_ports,
            &shape.in_types,
            &self.locations,
            None,
            constants,
            options,
            errors,
        );
        let mut outs = project_ports(
            &shape.out_ports,
            &shape.out_types,
            &self.locations,
            Some(api.outputs.timing),
            constants,
            options,
            errors,
        );
        for (index, (declaration, port)) in ins.iter_mut().zip(&shape.in_ports).enumerate() {
            declaration.name = self.bind_interface(
                "input",
                index,
                &port.name,
                port.is_array.then_some(port.slots.len()),
            );
        }
        for (index, (declaration, port)) in outs.iter_mut().zip(&shape.out_ports).enumerate() {
            declaration.name = self.bind_interface(
                "output",
                index,
                &port.name,
                port.is_array.then_some(port.slots.len()),
            );
        }
        if !ins.is_empty() {
            program
                .blocks
                .push(Block::Ins(port_block(ins, OutputTiming::Sample)));
        }
        if !outs.is_empty() {
            let ports = port_block(outs, api.outputs.timing);
            program.blocks.push(match api.outputs.timing {
                OutputTiming::Sample => Block::Outs(ports),
                OutputTiming::Block => Block::KOuts(ports),
            });
        }

        let mut params = Vec::new();
        let mut constructor = Vec::new();
        let mut refresh = Vec::new();
        for (index, spec) in shape
            .param_specs
            .iter()
            .filter(|spec| !spec.is_private())
            .enumerate()
        {
            let Some(first) = spec.slots.first() else {
                continue;
            };
            let array = !spec.is_scalar;
            let binding = self.bind_interface(
                "param",
                index,
                &spec.name,
                array.then_some(spec.slots.len()),
            );
            let mut declaration = ParamDecl {
                loc: self
                    .locations
                    .get(&spec.name)
                    .cloned()
                    .unwrap_or_else(|| self.loc.clone()),
                name: binding.clone(),
                private: false,
                ty: Some(value_type(first.ty, array, spec.slots.len())),
                ty_loc: Default::default(),
                default: None,
                range: source_range(first.range),
                control: self.controls.get(&spec.name).cloned().unwrap_or_default(),
                bind: None,
            };
            let values = spec
                .slots
                .iter()
                .map(|slot| {
                    scalar_default(slot.default.as_ref(), slot.ty, constants, options, errors)
                })
                .collect();
            declaration.default = Some(default_expr(values, array));
            for (attribute, ty) in [
                (&mut declaration.control.curve, PrimitiveType::F64),
                (&mut declaration.control.step, first.ty),
            ] {
                if let Some(expression) = attribute {
                    materialize(expression, constants, declaration_options, errors);
                    if let Some(value) = eval_typed_const_expr(
                        expression,
                        ty,
                        declaration_options,
                        "entry control attribute",
                        false,
                        errors,
                    ) {
                        *expression = typed_const_expr(value);
                    }
                }
            }
            constructor.push(named_arg(&spec.name, Expr::var(&binding)));
            for (index, _) in spec.slots.iter().enumerate() {
                let coordinate = array.then_some(index);
                let target = AssignTarget::Var(format!("{INSTANCE}.{}", spec.slots[index].name));
                let (_, value) = place(&binding, coordinate);
                refresh.push(assign(target, value));
            }
            params.push(declaration);
        }
        if !params.is_empty() {
            program.blocks.push(Block::Params(ParamBlock {
                loc: Default::default(),
                decls: params,
                deferred_count: None,
                deferred_default_ty: None,
                deferred_prefix: "param".to_owned(),
            }));
        }

        let buffers = shape
            .buffer_specs
            .iter()
            .enumerate()
            .map(|(index, buffer)| {
                let binding = self.bind_interface(
                    "buffer",
                    index,
                    &buffer.name,
                    buffer.is_array.then_some(buffer.array_len),
                );
                constructor.push(named_arg(&buffer.name, Expr::var(&binding)));
                BufferDecl {
                    loc: self
                        .locations
                        .get(&buffer.name)
                        .cloned()
                        .unwrap_or_else(|| self.loc.clone()),
                    name: binding,
                    ty: Some(BufferType {
                        elem: BufferElemType::Primitive(buffer.elem_ty),
                        channels: match buffer.channels {
                            TypedBufferChannels::Mono => BufferChannels::Mono,
                            TypedBufferChannels::Dynamic => BufferChannels::Dynamic,
                            TypedBufferChannels::Static(count) => {
                                BufferChannels::Static(Expr::int(count as i64))
                            }
                        },
                    }),
                    ty_loc: Default::default(),
                    array_size: buffer.is_array.then(|| Expr::int(buffer.array_len as i64)),
                }
            })
            .collect::<Vec<_>>();
        if !buffers.is_empty() {
            program.blocks.push(Block::Buffers(BufferBlock {
                loc: Default::default(),
                decls: buffers,
                deferred_count: None,
                deferred_default_ty: None,
            }));
        }
        program.blocks.push(Block::Init(InitBlock {
            loc: Default::default(),
            default_ty: None,
            default_ty_loc: Default::default(),
            pinned_roots: Default::default(),
            compiler_scratch_roots: Default::default(),
            body: vec![assign(
                AssignTarget::Var(INSTANCE.to_owned()),
                call("Main", constructor),
            )],
        }));
        runtime_defs.insert(REFRESH.to_owned());
        program.blocks.push(Block::Def(FunctionDef {
            loc: Default::default(),
            name: REFRESH.to_owned(),
            is_const: false,
            type_params: Vec::new(),
            params: Vec::new(),
            return_ty: None,
            return_ty_loc: Default::default(),
            body: refresh,
        }));

        let mut step = Vec::new();
        if api.steppable {
            step.push(expression(call(
                INSTANCE,
                (0..shape.in_ports.len())
                    .map(|index| CallArg {
                        name: None,
                        expr: Expr::var(interface_binding("input", index)),
                    })
                    .collect(),
            )));
            for (port_index, port) in shape.out_ports.iter().enumerate() {
                for index in 0..port.slots.len() {
                    let coordinate = port.is_array.then_some(index);
                    let (target, _) = place(&interface_binding("output", port_index), coordinate);
                    let value = Expr::var(format!("{INSTANCE}.{}", port.slots[index]));
                    step.push(assign(target, value));
                }
            }
        }
        let (pre, sample) = match api.outputs.timing {
            OutputTiming::Sample => (
                Vec::new(),
                Some(SampleBlock {
                    loc: Default::default(),
                    oversample_factor: None,
                    body: step,
                }),
            ),
            OutputTiming::Block => (step, None),
        };
        program.blocks.push(Block::Block(BlockExec {
            loc: Default::default(),
            compiler_scratch_roots: Vec::new(),
            pre,
            sample,
            post: Vec::new(),
        }));

        let mut events = Vec::new();
        for event in &self.events {
            let mut wrapper = event.clone();
            materialize_message_defaults(&mut wrapper.params, constants, options, errors);
            wrapper.body = vec![
                expression(call(REFRESH, Vec::new())),
                expression(call(
                    &format!("{INSTANCE}.{}", event.name),
                    event
                        .params
                        .iter()
                        .map(|param| named_arg(&param.name, Expr::var(&param.name)))
                        .collect(),
                )),
            ];
            events.push(wrapper);
        }
        if !events.is_empty() {
            program.blocks.push(Block::Events(EventBlock {
                loc: Default::default(),
                events,
            }));
        }
        for (index, delegate) in self.delegates.iter().enumerate() {
            // Bind generated names rather than authored payload names: the latter
            // may shadow a delegate or another symbol used by the forwarding body.
            let names = (0..delegate.params.len())
                .map(|index| format!("__onda_payload_{index}"))
                .collect::<Vec<_>>();
            program.blocks.push(Block::When(WhenDef {
                loc: Default::default(),
                target: WhenTarget {
                    loc: Default::default(),
                    receiver: vec![INSTANCE.to_owned()],
                    index: None,
                    delegate: delegate.name.clone(),
                },
                bindings: names
                    .iter()
                    .map(|name| WhenBinding {
                        loc: Default::default(),
                        name: name.clone(),
                    })
                    .collect(),
                body: vec![expression(call(
                    &delegate_function_name(index),
                    names
                        .iter()
                        .map(|name| CallArg {
                            name: None,
                            expr: Expr::var(name),
                        })
                        .collect(),
                ))],
            }));
        }
    }

    /// Finish the host schedule and return the source-owned instance root.
    /// Only descriptor names are public; generated dispatch symbols stay private.
    pub(super) fn finish(
        self,
        program: &mut Program,
        delegates: &mut [DelegateDef],
    ) -> EntryExports {
        if let Some(Block::Block(block)) = program
            .blocks
            .iter_mut()
            .find(|block| matches!(block, Block::Block(_)))
        {
            block.pre.insert(0, expression(call(REFRESH, Vec::new())));
        }
        for (export, source) in delegates.iter_mut().zip(&self.delegates) {
            export.name.clone_from(&source.name);
        }
        EntryExports {
            authored_instance_roots: HashSet::from([INSTANCE.to_owned()]),
            interface_names: self.interface_names,
        }
    }

    fn bind_interface(
        &mut self,
        kind: &str,
        index: usize,
        public: &str,
        len: Option<usize>,
    ) -> String {
        let binding = interface_binding(kind, index);
        self.interface_names
            .insert(binding.clone(), public.to_owned());
        if let Some(len) = len {
            for index in 0..len {
                self.interface_names
                    .insert(format!("{binding}[{index}]"), format!("{public}[{index}]"));
            }
        }
        binding
    }
}

fn materialize(
    expr: &mut Expr,
    constants: Option<&std::rc::Rc<crate::pipeline::ConstScope>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(constants) = constants {
        constants.materialize(expr, options, errors);
    }
}

fn materialize_message_defaults(
    params: &mut [EventParamDecl],
    constants: Option<&std::rc::Rc<crate::pipeline::ConstScope>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in params {
        if let Some(default) = &mut param.default {
            if let (EventParamType::Array { elem, size }, Some(constants)) = (&param.ty, constants)
            {
                if let Some(len) =
                    eval_data_size_expr(size, options, "message default array size", errors)
                {
                    if let Some(value) =
                        constants.array_literal(default, *elem, len, options, errors)
                    {
                        *default = value;
                    }
                }
            } else {
                materialize(default, constants, options, errors);
            }
        }
    }
}

fn scalar_default(
    default: Option<&Expr>,
    ty: PrimitiveType,
    constants: Option<&std::rc::Rc<crate::pipeline::ConstScope>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let mut value = default
        .cloned()
        .unwrap_or_else(|| typed_const_expr(coerce_const_default_to_typed(0.0, ty)));
    materialize(&mut value, constants, options, errors);
    eval_typed_const_expr(
        &value,
        ty,
        options,
        "entry default",
        is_float_type(ty),
        errors,
    )
    .map(typed_const_expr)
    .unwrap_or(value)
}

fn source_range(range: Option<TypedValueRange>) -> Option<DeclRange> {
    range.map(|range| DeclRange {
        min: Some(typed_const_expr(range.min)),
        max: typed_const_expr(range.max),
    })
}

fn value_type(ty: PrimitiveType, array: bool, len: usize) -> DeclType {
    if array {
        DeclType::Array {
            elem: ty,
            size: Expr::int(len as i64),
        }
    } else {
        DeclType::Scalar(ty)
    }
}

fn default_expr(values: Vec<Expr>, array: bool) -> Expr {
    if array {
        Expr::ArrayLiteral {
            loc: Default::default(),
            values,
        }
    } else {
        values.into_iter().next().unwrap_or_else(|| Expr::int(0))
    }
}

fn project_ports(
    specs: &[ProcPortSpec],
    types: &HashMap<String, PrimitiveType>,
    locations: &HashMap<String, Span>,
    timing: Option<OutputTiming>,
    constants: Option<&std::rc::Rc<crate::pipeline::ConstScope>>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Vec<PortDecl> {
    specs
        .iter()
        .filter_map(|spec| {
            let first = spec.slots.first()?;
            let ty = *types.get(first)?;
            let default = if spec.defaults.iter().any(Option::is_some) {
                Some(default_expr(
                    spec.defaults
                        .iter()
                        .map(|default| {
                            scalar_default(default.as_ref(), ty, constants, options, errors)
                        })
                        .collect(),
                    spec.is_array,
                ))
            } else {
                None
            };
            Some(PortDecl {
                smoothing: None,
                loc: locations.get(&spec.name).cloned().unwrap_or_default(),
                name: spec.name.clone(),
                output_timing: timing,
                output_timing_loc: Default::default(),
                ty: Some(value_type(ty, spec.is_array, spec.slots.len())),
                ty_loc: Default::default(),
                default,
                // Proc calls enforce element ranges; host arrays expose no range.
                range: if spec.is_array {
                    None
                } else {
                    source_range(spec.ranges.first().copied().flatten())
                },
            })
        })
        .collect()
}

fn port_block(decls: Vec<PortDecl>, timing: OutputTiming) -> PortBlock {
    PortBlock {
        loc: Default::default(),
        decls,
        deferred_count: None,
        deferred_default_ty: None,
        deferred_prefix: match timing {
            OutputTiming::Sample => "out",
            OutputTiming::Block => "kout",
        }
        .to_owned(),
        output_timing: timing,
        output_timing_loc: Default::default(),
    }
}

fn place(base: &str, index: Option<usize>) -> (AssignTarget, Expr) {
    match index {
        Some(index) => (
            AssignTarget::Index {
                base: base.to_owned(),
                index: Expr::int(index as i64),
            },
            Expr::Index {
                loc: Default::default(),
                base: base.to_owned(),
                index: Box::new(Expr::int(index as i64)),
            },
        ),
        None => (AssignTarget::Var(base.to_owned()), Expr::var(base)),
    }
}

fn assign(target: AssignTarget, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target,
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        param_write_lowered: false,
        expr,
    }
}

fn named_arg(name: &str, expr: Expr) -> CallArg {
    CallArg {
        name: Some(name.to_owned()),
        expr,
    }
}
fn call(name: &str, args: Vec<CallArg>) -> Expr {
    Expr::UserCall {
        loc: Default::default(),
        name: name.to_owned(),
        type_args: Vec::new(),
        args,
    }
}
fn expression(expr: Expr) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr,
    }
}

fn delegate_function_name(index: usize) -> String {
    format!("__onda_entry_delegate_{index}")
}

fn interface_binding(kind: &str, index: usize) -> String {
    format!("__onda_entry_{kind}_{index}")
}
