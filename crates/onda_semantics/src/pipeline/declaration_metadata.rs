//! Normalize required declaration metadata. This visits type dimensions,
//! layout requirements and declaration defaults, never predicts runtime reads.
use super::*;
use crate::array_semantics::{check_array_shape, is_array_reference, ArrayInitializer, ArrayShape};

struct Metadata<'a> {
    artifacts: &'a mut SemanticConstArtifacts,
    lazy_defaults: bool,
    deferred_types: HashSet<String>,
    options: AnalysisOptions,
    errors: &'a mut Vec<Diagnostic>,
}

impl Metadata<'_> {
    fn check(&self, expr: &Expr) -> ConstCheck {
        let mut check = ConstCheck::for_expression(expr, self.artifacts);
        check.defer_types(&self.deferred_types);
        check
    }

    fn expr(&mut self, expr: &mut Expr) {
        let entry = match &*expr {
            Expr::Var { name, .. } => self.artifacts.const_values.entry(name),
            _ => None,
        };
        materialize_const_expr(expr, self.artifacts, self.options, self.errors);
        if matches!(expr, Expr::Var { .. }) {
            if let Some(evaluation) = entry.map(|entry| entry.evaluation(self.options)) {
                if let Some(Ok(ResolvedConstValue::Array(array))) = evaluation.evaluated.get() {
                    *expr = const_array_literal_expr(&array.values, expr.loc());
                }
            }
        }
    }

    fn binding_range_bounds(&mut self, expr: &mut Expr, parser_only: bool) {
        let Some((_, _, parser_marker)) = integer_binding_range_call(expr) else {
            return;
        };
        if parser_only && !parser_marker {
            return;
        }
        if let Expr::Call { args, .. } = expr {
            for bound in &mut args[1..] {
                self.expr(bound);
            }
        }
    }

    fn dimension(&mut self, expr: &mut Expr) {
        self.expr(expr);
        // Call-type inference consumes literal dimensions. Preserve unresolved
        // template expressions for specialization and its ordinary diagnostics.
        if let Some(size) =
            eval_data_size_expr(expr, self.options, "array dimension", &mut Vec::new())
        {
            *expr = Expr::int(size as i64).with_loc(expr.loc());
        }
    }

    fn default(
        &mut self,
        expr: &mut Expr,
        target: Option<(Option<PrimitiveType>, &Expr)>,
        context: &str,
    ) {
        if let (Expr::Var { name, .. }, Some((elem, size))) = (&*expr, target) {
            if let Some(info) = self.artifacts.const_values.array_info(name) {
                if let Some(len) = eval_data_size_expr(size, self.options, context, self.errors) {
                    if check_array_shape(
                        ArrayShape::fixed(info.elem_ty, info.len),
                        ArrayShape {
                            elem_ty: elem,
                            len: Some(len),
                        },
                    )
                    .is_err()
                    {
                        let actual = fixed_array_type_label(info.elem_ty, info.len);
                        let expected = elem.map_or_else(
                            || format!("array[{len}]"),
                            |elem| fixed_array_type_label(elem, len),
                        );
                        self.errors.push(Diagnostic::semantic_span(
                            format!("{context} default const array '{name}' has type {actual}, expected {expected}"), expr.loc()));
                        return;
                    }
                }
            }
        }
        if !self.lazy_defaults {
            self.expr(expr);
            return;
        }
        let check = self.check(expr);
        if let Some((elem, size)) = target {
            let Some(len) = eval_data_size_expr(size, self.options, context, self.errors) else {
                return;
            };
            let elem = elem.or_else(|| {
                crate::expr_validation::infer_array_value_type(expr, check.env()).and_then(
                    |(elem, _)| match elem {
                        Some(ArrayElemType::Primitive(ty)) => Some(ty),
                        _ => None,
                    },
                )
            });
            let Some(elem) = elem else {
                return;
            };
            // Scalar defaults broadcast; array defaults retain their own shape.
            if crate::expr_validation::infer_array_value_type(expr, check.env()).is_some() {
                self.array_default(expr, Some(elem), Some(len), context, check);
            } else {
                check.scalar_value(expr, elem, context, self.errors);
            }
        } else {
            check.expression(expr, self.errors);
        }
    }

    fn array_default(
        &mut self,
        expr: &mut Expr,
        elem: Option<PrimitiveType>,
        len: Option<usize>,
        context: &str,
        check: ConstCheck,
    ) {
        let elem = elem.or_else(|| {
            crate::expr_validation::infer_array_initializer_type(expr, check.env()).and_then(
                |(elem, _)| match elem {
                    Some(ArrayElemType::Primitive(elem)) => Some(elem),
                    _ => None,
                },
            )
        });
        let errors_before = self.errors.len();
        let len = check.array_value(expr, elem, len, context, self.errors);
        // Templates and dependent shapes keep their initializer until concrete.
        // A declaration handle already supplies a shared evaluation cache.
        let (Some(elem), Some(len)) = (elem, len) else {
            return;
        };
        if self.errors.len() != errors_before
            || !self.deferred_types.is_empty()
            || self.cached_array_initializer(expr)
        {
            return;
        }
        let reference = is_array_reference(expr);
        let loc = expr.loc();
        let name = format!(
            ".__onda_default_array_{}",
            self.artifacts.const_array_infos.len()
        );
        let decl = ConstDecl {
            loc: loc.span(),
            name: name.clone(),
            ty: Some(ConstType::Array {
                elem,
                size: Expr::int(len as i64),
            }),
            expr: expr.clone(),
            configurable: false,
        };
        record_const_array_artifact(
            &decl,
            self.artifacts,
            ConstContext::Caller(self.options),
            self.errors,
        );
        let source = Expr::var(name).with_loc(loc);
        *expr = if reference {
            source
        } else {
            // Cached contents preserve initializer value semantics. Each write
            // receives an independent copy through the ordinary constructor.
            Expr::ArrayCtor {
                loc: loc.span(),
                spec: ArrayTypeSpec {
                    elem: ArrayElemType::Primitive(elem),
                    size: Box::new(Expr::int(len as i64).with_loc(loc)),
                },
                init: Some(vec![source]),
                init_is_value: true,
                initialize: true,
            }
        };
    }

    /// A normalized value is a fixed constructor copying a declaration handle.
    /// Reusing it makes repeated signature normalization idempotent; explicit
    /// copies of named consts use precisely the same representation.
    fn cached_array_initializer(&self, expr: &Expr) -> bool {
        let source = match expr {
            Expr::ArrayCtor {
                spec,
                init,
                init_is_value,
                ..
            } if matches!(&*spec.size, Expr::Int { .. }) => {
                match ArrayInitializer::new(init.as_deref(), *init_is_value) {
                    ArrayInitializer::Value(source) => source,
                    _ => return false,
                }
            }
            source => source,
        };
        matches!(source, Expr::Var { name, .. }
            if self.artifacts.const_values.array_info(name).is_some())
    }

    fn tuple_default(&mut self, expr: &mut Expr, types: &[Option<PrimitiveType>], context: &str) {
        if !self.lazy_defaults {
            self.expr(expr);
            return;
        }
        let Expr::Tuple { values, .. } = expr else {
            self.errors.push(Diagnostic::semantic_span(
                format!("{context} default must be a constant tuple"),
                expr.loc(),
            ));
            return;
        };
        if values.len() != types.len() {
            self.errors.push(Diagnostic::semantic_span(
                format!("{context} tuple default has the wrong number of components"),
                expr.loc(),
            ));
            return;
        }
        for (value, ty) in values.iter_mut().zip(types) {
            if let Some(ty) = ty {
                self.scalar_default(value, *ty, context);
            } else {
                self.check(value).expression(value, self.errors);
            }
        }
    }

    fn scalar_default(&mut self, expr: &mut Expr, ty: PrimitiveType, context: &str) {
        if self.lazy_defaults {
            self.check(expr)
                .scalar_value(expr, ty, context, self.errors);
        } else {
            self.expr(expr);
        }
    }
}

fn declaration_array_target(ty: Option<&DeclType>) -> Option<(Option<PrimitiveType>, &Expr)> {
    match ty? {
        DeclType::Array { elem, size } => Some((Some(*elem), size)),
        DeclType::ArrayGeneric { size, .. } => Some((None, size)),
        _ => None,
    }
}

pub(super) fn normalize_declaration_metadata(
    block: &mut Block,
    artifacts: &mut SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let lazy_defaults = matches!(block, Block::Proc(_) | Block::Struct(_));
    let deferred_types = match block {
        Block::Proc(proc) => proc.type_params.iter().cloned().collect(),
        Block::Struct(def) => def.type_params.iter().cloned().collect(),
        _ => HashSet::new(),
    };
    visit_block_metadata(
        block,
        &mut Metadata {
            artifacts,
            lazy_defaults,
            deferred_types,
            options,
            errors,
        },
    );
}

fn visit_decl_type(ty: Option<&mut DeclType>, visitor: &mut Metadata) {
    if let Some(DeclType::Array { size, .. } | DeclType::ArrayGeneric { size, .. }) = ty {
        visitor.dimension(size);
    }
}

fn visit_buffer_type(ty: Option<&mut BufferType>, visitor: &mut Metadata) {
    if let Some(BufferType {
        channels: BufferChannels::Static(channels),
        ..
    }) = ty
    {
        visitor.dimension(channels);
    }
}

fn visit_range(range: Option<&mut DeclRange>, visitor: &mut Metadata) {
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            visitor.expr(min);
        }
        visitor.expr(&mut range.max);
    }
}

fn infer_decl_type(ty: &mut Option<DeclType>, default: Option<&Expr>, visitor: &mut Metadata) {
    if ty.is_some() {
        return;
    }
    if let Some(default) = default {
        let check = visitor.check(default);
        let actual = check.expression(default, visitor.errors);
        *ty = effective_untyped_assignment_type(default, actual, check.env().declared_symbols)
            .map(DeclType::Scalar);
    }
}

fn visit_port_decl(decl: &mut PortDecl, label: &str, visitor: &mut Metadata) {
    visit_decl_type(decl.ty.as_mut(), visitor);
    decl.ty.get_or_insert(DeclType::Scalar(PrimitiveType::F32));
    if let Some(default) = &mut decl.default {
        let target = declaration_array_target(decl.ty.as_ref());
        if let Some(DeclType::Scalar(ty)) = decl.ty {
            visitor.scalar_default(default, ty, &format!("{label} '{}'", decl.name));
        } else {
            visitor.default(default, target, &format!("{label} '{}'", decl.name));
        }
    }
    visit_range(decl.range.as_mut(), visitor);
}

fn visit_param_decl(decl: &mut ParamDecl, visitor: &mut Metadata) {
    visit_decl_type(decl.ty.as_mut(), visitor);
    infer_decl_type(&mut decl.ty, decl.default.as_ref(), visitor);
    if let Some(default) = &mut decl.default {
        let context = format!("param '<top-level>.{}'", decl.name);
        if let Some(DeclType::Scalar(ty)) = decl.ty {
            visitor.scalar_default(default, ty, &context);
        } else {
            visitor.default(
                default,
                declaration_array_target(decl.ty.as_ref()),
                &context,
            );
        }
    }
    visit_range(decl.range.as_mut(), visitor);
    for value in [&mut decl.control.curve, &mut decl.control.step]
        .into_iter()
        .flatten()
    {
        visitor.expr(value);
    }
}

fn visit_fn_param_type(ty: Option<&mut FnParamType>, visitor: &mut Metadata) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => visitor.dimension(size),
        Some(FnParamType::Buffer(buffer) | FnParamType::BufferArray { buffer, .. }) => {
            visit_buffer_type(Some(buffer), visitor)
        }
        _ => {}
    }
}

fn visit_event_param(param: &mut EventParamDecl, owner: &str, visitor: &mut Metadata) {
    if let EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } =
        &mut param.ty
    {
        visitor.dimension(size);
    }
    if visitor.lazy_defaults
        && matches!(
            param.ty,
            EventParamType::GenericScalar { .. } | EventParamType::GenericSlice { .. }
        )
    {
        return;
    }
    if let Some(default) = &mut param.default {
        let target = match &param.ty {
            EventParamType::Array { elem, size } => Some((Some(*elem), size)),
            EventParamType::GenericArray { size, .. } => Some((None, size)),
            _ => None,
        };
        if let EventParamType::Scalar(ty) = param.ty {
            visitor.scalar_default(default, ty, &format!("event '{owner}.{}'", param.name));
        } else if let EventParamType::Tuple(types) = &param.ty {
            visitor.tuple_default(
                default,
                &types.iter().copied().map(Some).collect::<Vec<_>>(),
                &format!("event '{owner}.{}'", param.name),
            );
        } else {
            visitor.default(default, target, &format!("event '{owner}.{}'", param.name));
        }
    }
}

fn visit_function_signature(def: &mut FunctionDef, visitor: &mut Metadata) {
    let inherited_types = visitor.deferred_types.clone();
    let lazy_defaults = visitor.lazy_defaults;
    visitor.lazy_defaults = true;
    visitor
        .deferred_types
        .extend(def.type_params.iter().cloned());
    for param in &mut def.params {
        visit_fn_param_type(param.ty.as_mut(), visitor);
        if let Some(default) = &mut param.default {
            let context = format!("function parameter '{}.{}'", def.name, param.name);
            match &param.ty {
                Some(FnParamType::SizedArray { elem, size, .. }) => {
                    visitor.default(default, Some((*elem, size)), &context)
                }
                Some(FnParamType::Array(elem)) if def.is_const => {
                    let check = visitor.check(default);
                    visitor.array_default(default, *elem, None, &context, check);
                }
                _ => {}
            }
        }
    }
    if let Some(FnReturnType::Array { size, .. }) = &mut def.return_ty {
        visitor.dimension(size);
    }
    visitor.deferred_types = inherited_types;
    visitor.lazy_defaults = lazy_defaults;
}

fn visit_event_value_exprs(event: &mut EventDef, visitor: &mut Metadata) {
    for param in &mut event.params {
        visit_event_param(param, &event.name, visitor);
    }
}

pub(super) fn normalize_function_signature(
    def: &mut FunctionDef,
    deferred_types: &HashSet<String>,
    artifacts: &mut SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    visit_function_signature(
        def,
        &mut Metadata {
            artifacts,
            lazy_defaults: true,
            deferred_types: deferred_types.clone(),
            options,
            errors,
        },
    );
}

pub(super) fn normalize_function_metadata(
    def: &mut FunctionDef,
    artifacts: &mut SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
    inherited: &HashMap<String, IntegerBindingRange>,
) {
    normalize_function_signature(def, &HashSet::new(), artifacts, options, errors);
    normalize_body_metadata(&mut def.body, &def.type_params, artifacts, options, errors);
    rewrite_integer_binding_ranges_in_list(&mut def.body, inherited, options, errors);
}

/// Body metadata uses ordinary declaration options; array payload reads remain lazy.
pub(super) fn normalize_body_metadata(
    body: &mut [Stmt],
    type_params: &[String],
    artifacts: &mut SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    visit_stmt_value_exprs(
        body,
        &mut Metadata {
            artifacts,
            lazy_defaults: true,
            deferred_types: type_params.iter().cloned().collect(),
            options,
            errors,
        },
    );
}

fn visit_stmt_value_exprs(stmts: &mut [Stmt], visitor: &mut Metadata) {
    // Body typing needs fixed storage extents even when runtime reads remain
    // lazy. Traverse nested statements without recursion.
    for stmt in stmts.iter_mut() {
        stmt.visit_statements_mut(|stmt| {
            if let Stmt::Assign { decl_ty, .. } = stmt {
                visit_decl_type(decl_ty.as_mut(), visitor);
            }
        });
    }
    for stmt in stmts {
        stmt.visit_exprs_mut(|root| {
            root.visit_mut(|expr| {
                match expr {
                    Expr::ArrayCtor { spec, .. } => visitor.dimension(&mut spec.size),
                    _ => visitor.binding_range_bounds(expr, true),
                }
                true
            });
        });
    }
}

fn visit_graph_value_exprs(graph: &mut GraphBlock, visitor: &mut Metadata) {
    for edge in &mut graph.edges {
        if let Some(delay) = &mut edge.delay {
            visitor.expr(delay);
        }
        for destination in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = destination {
                visitor.expr(index);
            }
        }
    }
}

fn visit_block_metadata(block: &mut Block, visitor: &mut Metadata) {
    let port_label = if matches!(block, Block::Ins(_)) {
        "input"
    } else {
        "output"
    };
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(count) = &mut ports.deferred_count {
                visitor.expr(count);
            }
            for decl in &mut ports.decls {
                visit_port_decl(decl, port_label, visitor);
            }
        }
        Block::Params(params) => {
            if let Some(count) = &mut params.deferred_count {
                visitor.expr(count);
            }
            for decl in &mut params.decls {
                visit_param_decl(decl, visitor);
            }
        }
        Block::Const(decl) => {
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                visitor.dimension(size);
            }
        }
        Block::Events(events) => {
            for event in &mut events.events {
                visit_event_value_exprs(event, visitor);
            }
        }
        Block::Delegates(delegates) => {
            for delegate in &mut delegates.delegates {
                for param in &mut delegate.params {
                    visit_event_param(param, &delegate.name, visitor);
                }
            }
        }
        Block::When(when) => {
            if let Some(index) = &mut when.target.index {
                visitor.expr(index);
            }
        }
        Block::Assert(assertion) => visitor.expr(&mut assertion.expr),
        Block::Proc(proc) => {
            if let Some(factor) = &mut proc.sample_oversample_factor {
                visitor.expr(factor);
            }
            visitor.options = proc_options_for_count_expansion(proc, visitor.options);
            for count in [
                &mut proc.ins_deferred_count,
                &mut proc.outs_deferred_count,
                &mut proc.params_deferred_count,
                &mut proc.buffers_deferred_count,
            ]
            .into_iter()
            .flatten()
            {
                visitor.expr(count);
            }
            visit_decl_type(proc.ins_deferred_default_ty.as_mut(), visitor);
            visit_decl_type(proc.outs_deferred_default_ty.as_mut(), visitor);
            visit_decl_type(proc.params_deferred_default_ty.as_mut(), visitor);
            visit_buffer_type(proc.buffers_deferred_default_ty.as_mut(), visitor);
            for decl in proc.ins.iter_mut().chain(&mut proc.outs) {
                visit_port_decl(decl, "port", visitor);
            }
            for decl in &mut proc.params {
                visit_param_decl(decl, visitor);
            }
            for decl in &mut proc.buffers {
                visit_buffer_type(decl.ty.as_mut(), visitor);
                if let Some(size) = &mut decl.array_size {
                    visitor.dimension(size);
                }
            }
            // Init bindings define persistent processor layout in its declaration context.
            visit_stmt_value_exprs(&mut proc.init.body, visitor);
            for event in &mut proc.events {
                visit_event_value_exprs(event, visitor);
            }
            for delegate in &mut proc.delegates {
                for param in &mut delegate.params {
                    visit_event_param(param, &delegate.name, visitor);
                }
            }
            for when in &mut proc.whens {
                if let Some(index) = &mut when.target.index {
                    visitor.expr(index);
                }
            }
            if let Some(graph) = &mut proc.graph {
                visit_graph_value_exprs(graph, visitor);
            }
            for def in &mut proc.local_defs {
                visit_function_signature(def, visitor);
            }
        }
        Block::Struct(def) => {
            for field in &mut def.fields {
                if let FieldType::Array(spec) = &mut field.ty {
                    visitor.dimension(&mut spec.size);
                }
                if let Some(default) = &mut field.default {
                    if let FieldType::Scalar(ty) = field.ty {
                        // The storage domain is required even when the initializer
                        // is unused or overridden by a constructor argument.
                        if matches!(ty, PrimitiveType::I32 | PrimitiveType::I64) {
                            visitor.binding_range_bounds(default, false);
                        }
                        visitor.scalar_default(
                            default,
                            ty,
                            &format!("struct field '{}.{}'", def.name, field.name),
                        );
                    } else if let FieldType::Tuple(types) = &field.ty {
                        let types = types
                            .iter()
                            .map(|ty| match ty {
                                ScalarTypeRef::Primitive(ty) => Some(*ty),
                                ScalarTypeRef::Named(_) => None,
                            })
                            .collect::<Vec<_>>();
                        visitor.tuple_default(
                            default,
                            &types,
                            &format!("struct field '{}.{}'", def.name, field.name),
                        );
                    } else {
                        // Aggregate fields reject defaults during field coercion.
                    }
                }
            }
            for method in &mut def.methods {
                visit_function_signature(method, visitor);
            }
        }
        Block::Def(def) => visit_function_signature(def, visitor),
        Block::Block(exec) => {
            if let Some(sample) = &mut exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    visitor.expr(factor);
                }
            }
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                visitor.expr(factor);
            }
        }
        Block::Graph(graph) => visit_graph_value_exprs(graph, visitor),
        Block::Buffers(buffers) => {
            if let Some(count) = &mut buffers.deferred_count {
                visitor.expr(count);
            }
            visit_buffer_type(buffers.deferred_default_ty.as_mut(), visitor);
            for decl in &mut buffers.decls {
                visit_buffer_type(decl.ty.as_mut(), visitor);
                if let Some(size) = &mut decl.array_size {
                    visitor.dimension(size);
                }
            }
        }
        Block::Init(init) => visit_stmt_value_exprs(&mut init.body, visitor),
        Block::Tasks(_) | Block::Namespace(_) | Block::NamespaceAlias(_) | Block::Use(_) => {}
    }
}
