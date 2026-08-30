use super::*;

pub(super) fn apply_compile_inputs(
    program: &mut Program,
    inputs: &CompileInputs,
) -> Result<(), Vec<Diagnostic>> {
    let configurable = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Const(decl) if decl.configurable => Some((decl.name.as_str(), decl.loc)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let ordinary_consts = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Const(decl) if !decl.configurable => Some((decl.name.as_str(), decl.loc)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();

    let mut errors = Vec::new();
    for name in inputs.constants.keys() {
        if configurable.contains_key(name.as_str()) {
            continue;
        }
        if let Some(location) = ordinary_consts.get(name.as_str()) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "constant '{name}' is not host-configurable; declare it with 'config const'"
                ),
                location.as_ref(),
            ));
        } else {
            errors.push(Diagnostic::semantic(
                format!("unknown configuration constant '{name}'"),
                0,
                0,
            ));
        }
    }

    for block in &mut program.blocks {
        let Block::Const(decl) = block else {
            continue;
        };
        if !decl.configurable {
            continue;
        }
        let Some(ty) = decl.ty.as_ref() else {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "configuration constant '{}' requires an explicit type",
                    decl.name
                ),
                decl.loc.as_ref(),
            ));
            continue;
        };
        let Some(value) = inputs.constants.get(&decl.name) else {
            continue;
        };
        if let Some(expr) = compile_input_expr(value, ty, decl) {
            decl.expr = expr;
        } else {
            errors.push(compile_input_type_diagnostic(value, ty, decl));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub(super) fn compile_input_expr(
    value: &ConstValue,
    ty: &ConstType,
    decl: &ConstDecl,
) -> Option<Expr> {
    let location: SourceLoc = decl.loc.into();
    match (value, ty) {
        (ConstValue::Scalar(value), ConstType::Scalar(expected))
            if value.primitive_type() == *expected =>
        {
            Some(typed_const_expr_with_loc(*value, location))
        }
        (
            ConstValue::Array {
                elem_ty,
                len,
                values,
            },
            ConstType::Array { elem, .. } | ConstType::Slice { elem },
        ) if elem_ty == elem
            && *len == values.len()
            && values.iter().all(|value| value.primitive_type() == *elem) =>
        {
            Some(const_array_literal_expr(values, location))
        }
        _ => None,
    }
}

pub(super) fn compile_input_type_diagnostic(
    value: &ConstValue,
    ty: &ConstType,
    decl: &ConstDecl,
) -> Diagnostic {
    let supplied = match value {
        ConstValue::Scalar(value) => value.primitive_type().name().to_owned(),
        ConstValue::Array {
            elem_ty,
            len,
            values,
        } if *len == values.len()
            && values
                .iter()
                .all(|value| value.primitive_type() == *elem_ty) =>
        {
            format!("{}[{len}]", elem_ty.name())
        }
        ConstValue::Array { .. } => "malformed constant array".to_owned(),
    };
    let expected = match ty {
        ConstType::Scalar(ty) => ty.name().to_owned(),
        ConstType::Array { elem, .. } => format!("{}[fixed]", elem.name()),
        ConstType::Slice { elem } => format!("{}[]", elem.name()),
    };
    Diagnostic::semantic_span(
        format!(
            "configuration constant '{}' expects {expected}, but the host supplied {supplied}",
            decl.name
        ),
        decl.loc.as_ref(),
    )
}

pub(super) fn evaluate_asserts(
    program: &mut Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for block in &program.blocks {
        let Block::Assert(assert_decl) = block else {
            continue;
        };
        let context = "assert condition";
        if let Some(passed) = eval_const_bool_expr(&assert_decl.expr, options, context, errors) {
            if !passed {
                errors.push(Diagnostic::semantic_span(
                    "assert failed",
                    assert_decl.expr.loc(),
                ));
            }
        }
    }
    program.blocks.retain(|b| !matches!(b, Block::Assert(_)));
}

pub(super) fn is_const_array_decl(decl: &onda_frontend::ConstDecl) -> bool {
    matches!(
        decl.ty,
        Some(ConstType::Array { .. } | ConstType::Slice { .. })
    ) || matches!(
        decl.expr,
        Expr::ArrayLiteral { .. } | Expr::ArrayCtor { .. } | Expr::Slice { .. }
    )
}

pub(super) fn is_known_const_array_initializer(
    expr: &Expr,
    const_values: &HashMap<String, ConstValue>,
    const_defs: &HashMap<String, FunctionDef>,
) -> bool {
    match expr {
        Expr::Var { name, .. } => matches!(const_values.get(name), Some(ConstValue::Array { .. })),
        Expr::UserCall {
            name, type_args, ..
        } if type_args.is_empty() => const_defs
            .get(name)
            .is_some_and(|def| matches!(def.return_ty, Some(FnReturnType::Array { .. }))),
        _ => false,
    }
}

pub(super) fn const_array_info_map(
    const_arrays: &[TypedConstArray],
) -> HashMap<String, TypedArrayInfo> {
    const_arrays
        .iter()
        .map(|array| {
            (
                array.name.clone(),
                TypedArrayInfo {
                    elem_ty: array.elem_ty,
                    len: array.len,
                    offset: 0,
                },
            )
        })
        .collect()
}

pub(super) fn typed_const_expr_with_loc(value: TypedConstValue, loc: SourceLoc) -> Expr {
    typed_const_expr(value).with_loc(loc)
}

pub(super) fn const_array_literal_expr(values: &[TypedConstValue], loc: SourceLoc) -> Expr {
    Expr::ArrayLiteral {
        loc: loc.into(),
        values: values
            .iter()
            .map(|value| typed_const_expr_with_loc(*value, loc))
            .collect(),
    }
}

pub(super) fn host_sr_const_map(options: AnalysisOptions) -> HashMap<String, TypedConstValue> {
    host_sample_rate_constant_names()
        .map(|name| (name.to_owned(), TypedConstValue::F32(options.sample_rate)))
        .collect()
}

pub(super) fn fold_host_sr_const_type(
    ty: &mut Option<ConstType>,
    consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(ConstType::Array { size, .. }) = ty {
        fold_local_scalar_const_expr(size, consts);
    }
}

pub(super) fn fold_host_sr_const_decl(
    decl: &mut ConstDecl,
    consts: &HashMap<String, TypedConstValue>,
) {
    fold_host_sr_const_type(&mut decl.ty, consts);
    fold_local_scalar_const_expr(&mut decl.expr, consts);
}

pub(super) fn fold_host_sr_event(event: &mut EventDef, consts: &HashMap<String, TypedConstValue>) {
    for param in &mut event.params {
        fold_local_scalar_const_event_param_type(&mut param.ty, consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, consts);
        }
    }
    fold_host_sr_stmts(&mut event.body, consts);
}

pub(super) fn fold_host_sr_delegate(
    delegate: &mut DelegateDef,
    consts: &HashMap<String, TypedConstValue>,
) {
    for param in &mut delegate.params {
        fold_local_scalar_const_event_param_type(&mut param.ty, consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, consts);
        }
    }
}

pub(super) fn fold_host_sr_when(when: &mut WhenDef, consts: &HashMap<String, TypedConstValue>) {
    if let Some(index) = &mut when.target.index {
        fold_local_scalar_const_expr(index, consts);
    }
    fold_host_sr_stmts(&mut when.body, consts);
}

pub(super) fn fold_host_sr_function(
    def: &mut FunctionDef,
    consts: &HashMap<String, TypedConstValue>,
) {
    for param in &mut def.params {
        fold_local_scalar_const_fn_param_type(&mut param.ty, consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, consts);
        }
    }
    fold_local_scalar_const_return_type(&mut def.return_ty, consts);
    fold_host_sr_stmts(&mut def.body, consts);
}

pub(super) fn fold_host_sr_assign_target(
    target: &mut AssignTarget,
    consts: &HashMap<String, TypedConstValue>,
) {
    match target {
        AssignTarget::Index { index, .. } => fold_local_scalar_const_expr(index, consts),
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                fold_local_scalar_const_expr(coordinate, consts);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

pub(super) fn fold_host_sr_stmt(stmt: &mut Stmt, consts: &HashMap<String, TypedConstValue>) {
    match stmt {
        Stmt::Const { decl, .. } => fold_host_sr_const_decl(decl, consts),
        Stmt::Assign { target, expr, .. } => {
            fold_host_sr_assign_target(target, consts);
            fold_local_scalar_const_expr(expr, consts);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            fold_local_scalar_const_expr(expr, consts);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                fold_local_scalar_const_expr(value, consts);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            fold_local_scalar_const_expr(cond, consts);
            fold_host_sr_stmts(then_branch, consts);
            fold_host_sr_stmts(else_branch, consts);
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                fold_local_scalar_const_expr(step, consts);
            }
            fold_local_scalar_const_expr(start, consts);
            fold_local_scalar_const_expr(end, consts);
            fold_host_sr_stmts(body, consts);
        }
        Stmt::While { cond, body, .. } => {
            fold_local_scalar_const_expr(cond, consts);
            fold_host_sr_stmts(body, consts);
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn fold_host_sr_stmts(stmts: &mut [Stmt], consts: &HashMap<String, TypedConstValue>) {
    for stmt in stmts {
        fold_host_sr_stmt(stmt, consts);
    }
}

pub(super) fn fold_host_sr_graph(
    graph: &mut GraphBlock,
    consts: &HashMap<String, TypedConstValue>,
) {
    for edge in &mut graph.edges {
        fold_local_scalar_const_expr(&mut edge.source, consts);
        if let Some(delay) = &mut edge.delay {
            fold_local_scalar_const_expr(delay, consts);
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                fold_local_scalar_const_expr(index, consts);
            }
        }
    }
}

pub(super) fn fold_host_sr_namespace_ref_segment(
    segment: &mut NamespaceRefSegment,
    consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(args) = &mut segment.args {
        for arg in args {
            fold_local_scalar_const_expr(&mut arg.expr, consts);
        }
    }
}

pub(super) fn fold_host_sr_namespace_alias(
    alias: &mut NamespaceAliasDecl,
    consts: &HashMap<String, TypedConstValue>,
) {
    for segment in &mut alias.target {
        fold_host_sr_namespace_ref_segment(segment, consts);
    }
}

pub(super) fn fold_host_sr_use(use_decl: &mut UseDecl, consts: &HashMap<String, TypedConstValue>) {
    for segment in &mut use_decl.target {
        fold_host_sr_namespace_ref_segment(segment, consts);
    }
}

pub(super) fn fold_host_sr_namespace(
    namespace: &mut NamespaceDecl,
    consts: &HashMap<String, TypedConstValue>,
) {
    for param in &mut namespace.params {
        fold_local_scalar_const_expr(&mut param.default, consts);
    }
    for item in &mut namespace.items {
        match item {
            NamespaceItem::Assert(assert_decl) => {
                fold_local_scalar_const_expr(&mut assert_decl.expr, consts);
            }
            NamespaceItem::Const(decl) => fold_host_sr_const_decl(decl, consts),
            NamespaceItem::Struct(struct_def) => fold_host_sr_struct(struct_def, consts),
            NamespaceItem::Def(def) => fold_host_sr_function(def, consts),
            NamespaceItem::Proc(proc) => fold_host_sr_proc(proc, consts),
            NamespaceItem::Namespace(inner) => fold_host_sr_namespace(inner, consts),
            NamespaceItem::Alias(alias) => fold_host_sr_namespace_alias(alias, consts),
            NamespaceItem::Use(use_decl) => fold_host_sr_use(use_decl, consts),
        }
    }
}

pub(super) fn fold_host_sr_struct(
    struct_def: &mut StructDef,
    consts: &HashMap<String, TypedConstValue>,
) {
    for field in &mut struct_def.fields {
        fold_local_scalar_const_field_type(&mut field.ty, consts);
        if let Some(default) = &mut field.default {
            fold_local_scalar_const_expr(default, consts);
        }
    }
    for method in &mut struct_def.methods {
        fold_host_sr_function(method, consts);
    }
}

pub(super) fn fold_host_sr_proc(
    proc: &mut ProcessorDef,
    consts: &HashMap<String, TypedConstValue>,
) {
    for decl in &mut proc.consts {
        fold_host_sr_const_decl(decl, consts);
    }
    for expr in [
        &mut proc.ins_deferred_count,
        &mut proc.outs_deferred_count,
        &mut proc.params_deferred_count,
        &mut proc.buffers_deferred_count,
        &mut proc.sample_oversample_factor,
    ]
    .into_iter()
    .flatten()
    {
        fold_local_scalar_const_expr(expr, consts);
    }
    for ty in [
        &mut proc.ins_deferred_default_ty,
        &mut proc.outs_deferred_default_ty,
        &mut proc.params_deferred_default_ty,
        &mut proc.init.default_ty,
    ] {
        fold_local_scalar_const_decl_type(ty, consts);
    }
    fold_local_scalar_const_buffer_type(&mut proc.buffers_deferred_default_ty, consts);
    for decl in &mut proc.ins {
        fold_local_scalar_const_port_decl(decl, consts);
    }
    for decl in &mut proc.outs {
        fold_local_scalar_const_port_decl(decl, consts);
    }
    for decl in &mut proc.params {
        fold_local_scalar_const_param_decl(decl, consts);
    }
    for decl in &mut proc.buffers {
        fold_local_scalar_const_buffer_type(&mut decl.ty, consts);
    }
    fold_host_sr_stmts(&mut proc.init.body, consts);
    fold_host_sr_stmts(&mut proc.block_pre, consts);
    fold_host_sr_stmts(&mut proc.sample, consts);
    fold_host_sr_stmts(&mut proc.block_post, consts);
    if let Some(graph) = &mut proc.graph {
        fold_host_sr_graph(graph, consts);
    }
    for event in &mut proc.events {
        fold_host_sr_event(event, consts);
    }
    for delegate in &mut proc.delegates {
        fold_host_sr_delegate(delegate, consts);
    }
    for when in &mut proc.whens {
        fold_host_sr_when(when, consts);
    }
    for task in &mut proc.tasks {
        fold_host_sr_stmts(&mut task.body, consts);
    }
    for def in &mut proc.local_defs {
        fold_host_sr_function(def, consts);
    }
}

pub(super) fn fold_host_sr_block(block: &mut Block, consts: &HashMap<String, TypedConstValue>) {
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(count) = &mut ports.deferred_count {
                fold_local_scalar_const_expr(count, consts);
            }
            fold_local_scalar_const_decl_type(&mut ports.deferred_default_ty, consts);
            for decl in &mut ports.decls {
                fold_local_scalar_const_port_decl(decl, consts);
            }
        }
        Block::Params(params) => {
            if let Some(count) = &mut params.deferred_count {
                fold_local_scalar_const_expr(count, consts);
            }
            fold_local_scalar_const_decl_type(&mut params.deferred_default_ty, consts);
            for decl in &mut params.decls {
                fold_local_scalar_const_param_decl(decl, consts);
            }
        }
        Block::Buffers(buffers) => {
            if let Some(count) = &mut buffers.deferred_count {
                fold_local_scalar_const_expr(count, consts);
            }
            fold_local_scalar_const_buffer_type(&mut buffers.deferred_default_ty, consts);
            for decl in &mut buffers.decls {
                fold_local_scalar_const_buffer_type(&mut decl.ty, consts);
            }
        }
        Block::Const(decl) => fold_host_sr_const_decl(decl, consts),
        Block::Events(events) => {
            for event in &mut events.events {
                fold_host_sr_event(event, consts);
            }
        }
        Block::Delegates(delegates) => {
            for delegate in &mut delegates.delegates {
                fold_host_sr_delegate(delegate, consts);
            }
        }
        Block::When(when) => fold_host_sr_when(when, consts),
        Block::Tasks(tasks) => {
            for task in &mut tasks.tasks {
                fold_host_sr_stmts(&mut task.body, consts);
            }
        }
        Block::Assert(assert_decl) => {
            fold_local_scalar_const_expr(&mut assert_decl.expr, consts);
        }
        Block::Namespace(namespace) => fold_host_sr_namespace(namespace, consts),
        Block::NamespaceAlias(alias) => fold_host_sr_namespace_alias(alias, consts),
        Block::Use(use_decl) => fold_host_sr_use(use_decl, consts),
        Block::Proc(proc) => fold_host_sr_proc(proc, consts),
        Block::Struct(struct_def) => fold_host_sr_struct(struct_def, consts),
        Block::Def(def) => fold_host_sr_function(def, consts),
        Block::Init(init) => {
            fold_local_scalar_const_decl_type(&mut init.default_ty, consts);
            fold_host_sr_stmts(&mut init.body, consts);
        }
        Block::Block(block_exec) => {
            fold_host_sr_stmts(&mut block_exec.pre, consts);
            if let Some(sample) = &mut block_exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    fold_local_scalar_const_expr(factor, consts);
                }
                fold_host_sr_stmts(&mut sample.body, consts);
            }
            fold_host_sr_stmts(&mut block_exec.post, consts);
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                fold_local_scalar_const_expr(factor, consts);
            }
            fold_host_sr_stmts(&mut sample.body, consts);
        }
        Block::Graph(graph) => fold_host_sr_graph(graph, consts),
    }
}

pub(super) fn fold_host_sr_builtin(program: &mut Program, options: AnalysisOptions) {
    let consts = host_sr_const_map(options);
    for block in &mut program.blocks {
        fold_host_sr_block(block, &consts);
    }
}

pub(super) const CONST_DEF_LOOP_ITERATION_LIMIT: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ConstEvalArray {
    pub(super) elem_ty: PrimitiveType,
    pub(super) values: Vec<TypedConstValue>,
}

impl ConstEvalArray {
    pub(super) fn len(&self) -> usize {
        self.values.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ConstEvalValue {
    Scalar(TypedConstValue),
    Array(ConstEvalArray),
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(super) enum ConstDefReturn {
    Scalar(PrimitiveType),
    Array { elem_ty: PrimitiveType, len: usize },
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(super) enum ConstDefParamKind {
    Scalar(PrimitiveType),
    Array { elem_ty: PrimitiveType, len: usize },
    Slice { elem_ty: Option<PrimitiveType> },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(super) struct ConstArrayExpectation {
    pub(super) elem_ty: Option<PrimitiveType>,
    pub(super) len: Option<usize>,
}

impl ConstArrayExpectation {
    pub(super) fn any() -> Self {
        Self {
            elem_ty: None,
            len: None,
        }
    }

    pub(super) fn fixed(elem_ty: PrimitiveType, len: usize) -> Self {
        Self {
            elem_ty: Some(elem_ty),
            len: Some(len),
        }
    }

    pub(super) fn elem(elem_ty: PrimitiveType) -> Self {
        Self {
            elem_ty: Some(elem_ty),
            len: None,
        }
    }

    pub(super) fn is_any(self) -> bool {
        self.elem_ty.is_none() && self.len.is_none()
    }
}

#[derive(Copy, Clone)]
pub(super) struct ConstDefRegistry<'a> {
    pub(super) defs: &'a HashMap<String, FunctionDef>,
    pub(super) order: &'a HashMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SemanticConstArtifacts {
    pub(super) const_arrays: Vec<TypedConstArray>,
    pub(super) const_values: HashMap<String, ConstValue>,
    pub(super) const_defs: HashMap<String, FunctionDef>,
    pub(super) const_def_order: HashMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ProcLocalConstArtifacts {
    pub(super) names: HashSet<String>,
    pub(super) values: HashMap<String, TypedConstValue>,
}

pub(super) fn record_const_array_artifact(
    artifacts: &mut SemanticConstArtifacts,
    array: TypedConstArray,
) {
    artifacts.const_values.insert(
        array.name.clone(),
        ConstValue::Array {
            elem_ty: array.elem_ty,
            len: array.len,
            values: array.values.clone(),
        },
    );
    artifacts.const_arrays.push(array);
}

pub(super) fn ordinary_top_level_symbol_names(program: &Program) -> HashSet<String> {
    program
        .blocks
        .iter()
        .flat_map(|block| match block {
            Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => ports
                .decls
                .iter()
                .map(|decl| decl.name.clone())
                .collect::<Vec<_>>(),
            Block::Params(params) => params
                .decls
                .iter()
                .map(|decl| decl.name.clone())
                .collect::<Vec<_>>(),
            Block::Buffers(buffers) => buffers
                .decls
                .iter()
                .map(|decl| decl.name.clone())
                .collect::<Vec<_>>(),
            Block::Def(def) if !def.is_const => vec![def.name.clone()],
            Block::Struct(s) => vec![s.name.clone()],
            Block::Proc(p) => vec![p.name.clone()],
            _ => Vec::new(),
        })
        .collect()
}

pub(super) fn top_level_const_symbol_names(program: &Program) -> HashSet<String> {
    program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Const(decl) => Some(decl.name.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn namespace_parent(ns: &str) -> Option<&str> {
    ns.rsplit_once("::").map(|(parent, _)| parent)
}

pub(super) fn namespace_candidates(current_ns: &str) -> Vec<String> {
    if current_ns.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::<String>::new();
    let mut cur = Some(current_ns);
    while let Some(ns) = cur {
        out.push(ns.to_owned());
        cur = namespace_parent(ns);
    }
    out.push(String::new());
    out
}

pub(super) fn namespace_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}::{child}")
    }
}

pub(super) fn symbol_namespace(name: &str) -> String {
    name.rsplit_once("::")
        .map(|(namespace, _)| namespace.to_owned())
        .unwrap_or_default()
}

pub(super) fn visible_const_symbol_for_local_name(
    name: &str,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
) -> Option<String> {
    if name.contains('.') {
        return None;
    }
    if name.contains("::") {
        return const_values.contains_key(name).then(|| name.to_owned());
    }
    for ns in namespace_candidates(scope_ns) {
        let candidate = namespace_join(&ns, name);
        if const_values.contains_key(&candidate) {
            return Some(candidate);
        }
    }
    None
}

pub(super) fn zero_const_value(ty: PrimitiveType) -> TypedConstValue {
    match ty {
        PrimitiveType::F32 => TypedConstValue::F32(0.0),
        PrimitiveType::F64 => TypedConstValue::F64(0.0),
        PrimitiveType::I32 => TypedConstValue::I32(0),
        PrimitiveType::I64 => TypedConstValue::I64(0),
        PrimitiveType::Bool => TypedConstValue::Bool(false),
    }
}

pub(super) fn fold_const_array_expr(
    expr: &mut Expr,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
    inline_array_vars: bool,
) {
    let loc = expr.loc();
    if let Expr::Var { name, .. } = expr {
        if let Some(ConstValue::Scalar(value)) = const_values.get(name) {
            *expr = typed_const_expr_with_loc(*value, loc);
            return;
        }
    }
    if inline_array_vars {
        if let Expr::Var { name, .. } = expr {
            if let Some(ConstValue::Array { values, .. }) = const_values.get(name) {
                *expr = const_array_literal_expr(values, loc);
                return;
            }
        }
    }

    match expr {
        Expr::Index { base, index, .. } => {
            fold_const_array_expr(index, const_values, options, errors, false);
            let Some(ConstValue::Array { len, values, .. }) = const_values.get(base) else {
                return;
            };
            if !can_eval_const_expr_exact_int(index) {
                return;
            }
            let Some(raw_idx) = eval_const_expr_i64_exact(
                index,
                options,
                &format!("const array '{base}' index"),
                errors,
            ) else {
                return;
            };
            let Ok(idx) = usize::try_from(raw_idx) else {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "const array '{base}' index {raw_idx} is out of bounds for length {len}"
                    ),
                    expr.loc(),
                ));
                return;
            };
            let Some(value) = values.get(idx).copied() else {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "const array '{base}' index {raw_idx} is out of bounds for length {len}"
                    ),
                    expr.loc(),
                ));
                return;
            };
            *expr = typed_const_expr_with_loc(value, loc);
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                fold_const_array_expr(&mut arg.expr, const_values, options, errors, false);
            }
            if !args.is_empty() {
                return;
            }
            let Some(base) = parse_array_len_instance_base(name) else {
                return;
            };
            if let Some(ConstValue::Array { len, .. }) = const_values.get(base) {
                *expr = Expr::int(*len as i64).with_loc(loc);
            }
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                fold_const_array_expr(coordinate, const_values, options, errors, false);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            fold_const_array_expr(&mut spec.size, const_values, options, errors, false);
            if let Some(init) = init {
                for value in init {
                    fold_const_array_expr(value, const_values, options, errors, false);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            fold_const_array_expr(lhs, const_values, options, errors, false);
            fold_const_array_expr(rhs, const_values, options, errors, false);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                fold_const_array_expr(arg, const_values, options, errors, false);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            fold_const_array_expr(expr, const_values, options, errors, false);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                fold_const_array_expr(value, const_values, options, errors, false);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(super) fn const_def_param_signature(
    def: &FunctionDef,
    options: AnalysisOptions,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<ConstDefParamKind>> {
    let mut out = Vec::with_capacity(def.params.len());
    for param in &def.params {
        match param.ty.as_ref() {
            Some(FnParamType::Primitive(ty)) => out.push(ConstDefParamKind::Scalar(*ty)),
            Some(FnParamType::Array(elem_ty)) => {
                out.push(ConstDefParamKind::Slice { elem_ty: *elem_ty })
            }
            Some(FnParamType::SizedArray {
                elem: Some(elem_ty),
                generic_name: None,
                size,
            }) => {
                let locals = HashMap::new();
                let local_arrays = HashMap::new();
                let len = eval_const_array_size_with_defs(
                    size,
                    &locals,
                    &local_arrays,
                    const_values,
                    const_defs,
                    options,
                    &format!(
                        "const def '{}' parameter '{}' array size",
                        def.name, param.name
                    ),
                    call_stack,
                    errors,
                )?;
                out.push(ConstDefParamKind::Array {
                    elem_ty: *elem_ty,
                    len,
                });
            }
            Some(_) => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "const def '{}' parameter '{}' must use a primitive scalar, fixed primitive array, or read-only primitive array slice type",
                        def.name, param.name
                    ),
                    param.ty_loc.or(param.loc),
                ));
                return None;
            }
            None => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "const def '{}' parameter '{}' must have an explicit primitive scalar, fixed primitive array, or read-only primitive array slice type",
                        def.name, param.name
                    ),
                    param.loc,
                ));
                return None;
            }
        }
    }
    Some(out)
}

pub(super) fn const_def_return_type(
    def: &FunctionDef,
    options: AnalysisOptions,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ConstDefReturn> {
    match def.return_ty.as_ref() {
        Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(ty))) => {
            Some(ConstDefReturn::Scalar(*ty))
        }
        Some(FnReturnType::Scalar(FnReturnScalarType::Named(name))) => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const def '{}' return type '{}' is not a concrete primitive scalar",
                    def.name, name
                ),
                def.return_ty_loc,
            ));
            None
        }
        Some(FnReturnType::Array { elem, size }) => {
            let locals = HashMap::new();
            let local_arrays = HashMap::new();
            let len = eval_const_array_size_with_defs(
                size,
                &locals,
                &local_arrays,
                const_values,
                const_defs,
                options,
                &format!("const def '{}' return array size", def.name),
                call_stack,
                errors,
            )?;
            Some(ConstDefReturn::Array {
                elem_ty: *elem,
                len,
            })
        }
        Some(FnReturnType::Tuple(_)) => {
            errors.push(Diagnostic::semantic_span(
                format!("const def '{}' cannot return a tuple", def.name),
                def.return_ty_loc,
            ));
            None
        }
        None => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const def '{}' must declare an explicit return type",
                    def.name
                ),
                def.loc,
            ));
            None
        }
    }
}

pub(super) fn validate_const_def_declaration(
    def: &FunctionDef,
    options: AnalysisOptions,
    artifacts: &SemanticConstArtifacts,
    errors: &mut Vec<Diagnostic>,
) {
    if !def.type_params.is_empty() {
        errors.push(Diagnostic::semantic_span(
            format!("const def '{}' cannot declare type parameters", def.name),
            def.loc,
        ));
        return;
    }

    let const_defs = const_def_registry(artifacts);
    let mut call_stack = vec![def.name.clone()];
    let _ = const_def_return_type(
        def,
        options,
        &artifacts.const_values,
        const_defs,
        &mut call_stack,
        errors,
    );
    let param_kinds = const_def_param_signature(
        def,
        options,
        &artifacts.const_values,
        const_defs,
        &mut call_stack,
        errors,
    );
    validate_const_def_body_shape(def, param_kinds.as_deref(), errors);
}

pub(super) fn validate_const_def_body_shape(
    def: &FunctionDef,
    param_kinds: Option<&[ConstDefParamKind]>,
    errors: &mut Vec<Diagnostic>,
) {
    let read_only_arrays = param_kinds
        .map(|kinds| {
            def.params
                .iter()
                .zip(kinds.iter())
                .filter(|&(_param, kind)| matches!(kind, ConstDefParamKind::Slice { .. }))
                .map(|(param, _kind)| param.name.clone())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let mut local_const_names = HashSet::new();
    let immutable_loop_vars = HashSet::new();
    validate_const_def_stmt_shapes(
        &def.body,
        &def.name,
        &read_only_arrays,
        &mut local_const_names,
        &immutable_loop_vars,
        errors,
    );
}

pub(super) fn validate_const_def_stmt_shapes(
    stmts: &[Stmt],
    def_name: &str,
    read_only_arrays: &HashSet<String>,
    local_const_names: &mut HashSet<String>,
    immutable_loop_vars: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Return { .. } => {}
            Stmt::Assign {
                target,
                generic_decl_ty,
                ..
            } => {
                if generic_decl_ty.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def '{def_name}' local declarations cannot use generic types"
                        ),
                        stmt.assign_target_loc(),
                    ));
                    continue;
                }
                match target {
                    AssignTarget::Var(name) => {
                        if immutable_loop_vars.contains(name) {
                            errors.push(Diagnostic::semantic_span(
                                format!("cannot assign to loop variable '{name}'"),
                                stmt.assign_target_loc(),
                            ));
                        } else if local_const_names.contains(name) {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "const def '{def_name}' cannot assign to local const '{name}'"
                                ),
                                stmt.assign_target_loc(),
                            ));
                        }
                    }
                    AssignTarget::Index { base, .. } => {
                        if read_only_arrays.contains(base) {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "const def '{def_name}' cannot write read-only array parameter '{base}'"
                                ),
                                stmt.assign_target_loc(),
                            ));
                        }
                    }
                    _ => {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "const def '{def_name}' can only assign scalar locals or indexed local arrays"
                            ),
                            stmt.assign_target_loc(),
                        ));
                    }
                }
            }
            Stmt::Const { decl, .. } => {
                if is_const_array_decl(decl)
                    || matches!(
                        decl.ty,
                        Some(ConstType::Array { .. } | ConstType::Slice { .. })
                    )
                {
                    errors.push(Diagnostic::semantic_span(
                        format!("const def '{def_name}' local const arrays are not supported"),
                        decl.loc.as_ref(),
                    ));
                } else if !local_const_names.insert(decl.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "duplicate constant '{}' in const def '{def_name}'",
                            decl.name
                        ),
                        decl.loc.as_ref(),
                    ));
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                let mut then_const_names = local_const_names.clone();
                validate_const_def_stmt_shapes(
                    then_branch,
                    def_name,
                    read_only_arrays,
                    &mut then_const_names,
                    immutable_loop_vars,
                    errors,
                );
                let mut else_const_names = local_const_names.clone();
                validate_const_def_stmt_shapes(
                    else_branch,
                    def_name,
                    read_only_arrays,
                    &mut else_const_names,
                    immutable_loop_vars,
                    errors,
                );
            }
            Stmt::For { loc, var, body, .. } => {
                if local_const_names.contains(var) {
                    errors.push(Diagnostic::semantic_span(
                        format!("const def '{def_name}' cannot assign to local const '{var}'"),
                        *loc,
                    ));
                }
                let mut loop_const_names = local_const_names.clone();
                let mut loop_vars = immutable_loop_vars.clone();
                loop_vars.insert(var.clone());
                validate_const_def_stmt_shapes(
                    body,
                    def_name,
                    read_only_arrays,
                    &mut loop_const_names,
                    &loop_vars,
                    errors,
                );
            }
            Stmt::Print { .. } => {
                errors.push(Diagnostic::semantic_span(
                    format!("print is not allowed in const def '{def_name}'"),
                    stmt.loc(),
                ));
            }
            Stmt::Expr { .. } | Stmt::While { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {
                errors.push(Diagnostic::semantic_span(
                    format!("const def '{def_name}' statement is not supported"),
                    stmt.loc(),
                ));
            }
        }
    }
}

macro_rules! eval_float_builtin {
    ($func:expr, $values:expr) => {{
        let values = $values;
        match $func {
            BuiltinFn::Sin => values[0].sin(),
            BuiltinFn::Cos => values[0].cos(),
            BuiltinFn::Tan => values[0].tan(),
            BuiltinFn::Tanh => values[0].tanh(),
            BuiltinFn::Atan => values[0].atan(),
            BuiltinFn::Atan2 => values[0].atan2(values[1]),
            BuiltinFn::Exp => values[0].exp(),
            BuiltinFn::Log => values[0].ln(),
            BuiltinFn::Sqrt => values[0].sqrt(),
            BuiltinFn::Pow => values[0].powf(values[1]),
            BuiltinFn::Abs => values[0].abs(),
            BuiltinFn::Floor => values[0].floor(),
            BuiltinFn::Ceil => values[0].ceil(),
            BuiltinFn::Round => values[0].round(),
            BuiltinFn::Trunc => values[0].trunc(),
            BuiltinFn::Min => values[0].min(values[1]),
            BuiltinFn::Max => values[0].max(values[1]),
            BuiltinFn::Fma => values[0].mul_add(values[1], values[2]),
            BuiltinFn::RangeClamp => values[0].max(values[1]).min(values[2]),
            BuiltinFn::RangeWrap => unreachable!("range wrap is integer-only"),
            BuiltinFn::BindingCountClamp
            | BuiltinFn::BindingRangeClamp
            | BuiltinFn::BindingRangeInclusiveClamp
            | BuiltinFn::BindingCountWrap
            | BuiltinFn::BindingRangeWrap
            | BuiltinFn::BindingRangeInclusiveWrap => return None,
        }
    }};
}

pub(super) fn eval_typed_const_builtin(
    func: BuiltinFn,
    args: &[TypedConstValue],
    result_ty: PrimitiveType,
) -> Option<TypedConstValue> {
    match result_ty {
        PrimitiveType::F32 => {
            let values = args
                .iter()
                .map(|value| match value {
                    TypedConstValue::F32(value) => Some(*value),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(TypedConstValue::F32(eval_float_builtin!(func, &values)))
        }
        PrimitiveType::F64 => {
            let values = args
                .iter()
                .map(|value| match value {
                    TypedConstValue::F64(value) => Some(*value),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            Some(TypedConstValue::F64(eval_float_builtin!(func, &values)))
        }
        PrimitiveType::I32 => {
            let values = args
                .iter()
                .map(|value| match value {
                    TypedConstValue::I32(value) => Some(*value),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            let value = match func {
                BuiltinFn::Abs => values[0].wrapping_abs(),
                BuiltinFn::Min => values[0].min(values[1]),
                BuiltinFn::Max => values[0].max(values[1]),
                BuiltinFn::RangeClamp => values[0].max(values[1]).min(values[2]),
                BuiltinFn::RangeWrap => {
                    let lower = i128::from(values[1]);
                    let upper = i128::from(values[2]);
                    let width = upper - lower + 1;
                    if width <= 0 {
                        return None;
                    }
                    (lower + (i128::from(values[0]) - lower).rem_euclid(width)) as i32
                }
                _ => return None,
            };
            Some(TypedConstValue::I32(value))
        }
        PrimitiveType::I64 => {
            let values = args
                .iter()
                .map(|value| match value {
                    TypedConstValue::I64(value) => Some(*value),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            let value = match func {
                BuiltinFn::Abs => values[0].wrapping_abs(),
                BuiltinFn::Min => values[0].min(values[1]),
                BuiltinFn::Max => values[0].max(values[1]),
                BuiltinFn::RangeClamp => values[0].max(values[1]).min(values[2]),
                BuiltinFn::RangeWrap => {
                    let lower = i128::from(values[1]);
                    let upper = i128::from(values[2]);
                    let width = upper - lower + 1;
                    if width <= 0 {
                        return None;
                    }
                    (lower + (i128::from(values[0]) - lower).rem_euclid(width)) as i64
                }
                _ => return None,
            };
            Some(TypedConstValue::I64(value))
        }
        PrimitiveType::Bool => None,
    }
}

pub(super) fn eval_const_builtin_call(
    func: BuiltinFn,
    args: &[Expr],
    loc: SourceLoc,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    let arity = builtin_arity(func);
    if args.len() != arity {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{context}: builtin '{}' expects {arity} argument(s), got {}",
                builtin_name(func),
                args.len()
            ),
            loc,
        ));
        return None;
    }
    let arg_types = args
        .iter()
        .map(|arg| infer_const_expr_type(arg, options, context, errors))
        .collect::<Option<Vec<_>>>()?;
    let adapted_types = adapt_numeric_argument_types(args, &arg_types);
    let Some(result_ty) = intrinsic_result_type(func, &adapted_types) else {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{context}: builtin '{}' has incompatible argument types",
                builtin_name(func)
            ),
            loc,
        ));
        return None;
    };
    let values = args
        .iter()
        .enumerate()
        .map(|(idx, arg)| {
            eval_typed_const_expr(
                arg,
                result_ty,
                options,
                &format!("{context} argument {idx}"),
                true,
                matches!(result_ty, PrimitiveType::I32 | PrimitiveType::I64),
                errors,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    let Some(value) = eval_typed_const_builtin(func, &values, result_ty) else {
        errors.push(Diagnostic::internal(format!(
            "constant builtin '{}' could not be evaluated as {}",
            builtin_name(func),
            primitive_type_label(result_ty)
        )));
        return None;
    };
    Some(typed_const_expr_with_loc(value, loc))
}

pub(super) fn const_eval_array_ref_by_name<'a>(
    name: &str,
    local_arrays: &'a HashMap<String, ConstEvalArray>,
    const_values: &'a HashMap<String, ConstValue>,
) -> Option<(PrimitiveType, &'a [TypedConstValue])> {
    if let Some(array) = local_arrays.get(name) {
        return Some((array.elem_ty, &array.values));
    }
    match const_values.get(name) {
        Some(ConstValue::Array {
            elem_ty, values, ..
        }) => Some((*elem_ty, values)),
        _ => None,
    }
}

pub(super) fn const_eval_array_by_name(
    name: &str,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
) -> Option<ConstEvalArray> {
    let (elem_ty, values) = const_eval_array_ref_by_name(name, local_arrays, const_values)?;
    Some(ConstEvalArray {
        elem_ty,
        values: values.to_vec(),
    })
}

pub(super) fn fold_const_eval_expr(
    expr: &Expr,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    let loc = expr.loc();
    match expr {
        Expr::Var { name, .. } => {
            if let Some(value) = locals.get(name).copied() {
                return Some(typed_const_expr_with_loc(value, loc));
            }
            if let Some(ConstValue::Scalar(value)) = const_values.get(name) {
                return Some(typed_const_expr_with_loc(*value, loc));
            }
            Some(expr.clone())
        }
        Expr::UserCall {
            name,
            args,
            type_args,
            ..
        } => {
            if args.is_empty() {
                if let Some(base) = parse_array_len_instance_base(name) {
                    if let Some(array) = local_arrays.get(base) {
                        return Some(Expr::int(array.len() as i64).with_loc(loc));
                    }
                    if let Some(ConstValue::Array { len, .. }) = const_values.get(base) {
                        return Some(Expr::int(*len as i64).with_loc(loc));
                    }
                }
            }
            if !type_args.is_empty() {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: const def calls cannot use explicit type arguments"),
                    loc,
                ));
                return None;
            }
            let value = eval_const_def_call(
                name,
                args,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
                loc,
            )?;
            match value {
                ConstEvalValue::Scalar(value) => Some(typed_const_expr_with_loc(value, loc)),
                ConstEvalValue::Array(_) => {
                    errors.push(Diagnostic::semantic_span(
                        format!("{context}: const def '{name}' returns an array, not a scalar"),
                        loc,
                    ));
                    None
                }
            }
        }
        Expr::Call { func, args, .. } => {
            let folded = args
                .iter()
                .map(|arg| {
                    fold_const_eval_expr(
                        arg,
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        context,
                        call_stack,
                        errors,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            eval_const_builtin_call(*func, &folded, loc, options, context, errors)
        }
        Expr::Index { base, index, .. } => {
            let folded_index = fold_const_eval_expr(
                index,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?;
            if let Some((_, array)) = const_eval_array_ref_by_name(base, local_arrays, const_values)
            {
                if !can_eval_const_expr_exact_int(&folded_index) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context}: const array '{base}' index is not compile-time integer"
                        ),
                        index.loc(),
                    ));
                    return None;
                }
                let raw_idx = eval_const_expr_i64_exact(
                    &folded_index,
                    options,
                    &format!("{context}: const array '{base}' index"),
                    errors,
                )?;
                let Ok(idx) = usize::try_from(raw_idx) else {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context}: const array '{base}' index {raw_idx} is out of bounds for length {}",
                            array.len()
                        ),
                        expr.loc(),
                    ));
                    return None;
                };
                let Some(value) = array.get(idx).copied() else {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context}: const array '{base}' index {raw_idx} is out of bounds for length {}",
                            array.len()
                        ),
                        expr.loc(),
                    ));
                    return None;
                };
                return Some(typed_const_expr_with_loc(value, loc));
            }

            let mut folded = Expr::Index {
                loc: expr.loc().span(),
                base: base.clone(),
                index: Box::new(folded_index),
            };
            fold_const_array_expr(&mut folded, const_values, options, errors, false);
            Some(folded)
        }
        Expr::Slice { .. } => {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: slices are not supported in const def evaluation"),
                loc,
            ));
            None
        }
        Expr::ArrayLiteral { values, .. } => Some(Expr::ArrayLiteral {
            loc: expr.loc().span(),
            values: values
                .iter()
                .map(|value| {
                    fold_const_eval_expr(
                        value,
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        context,
                        call_stack,
                        errors,
                    )
                })
                .collect::<Option<Vec<_>>>()?,
        }),
        Expr::Compare { loc, op, lhs, rhs } => Some(Expr::Compare {
            loc: *loc,
            op: *op,
            lhs: Box::new(fold_const_eval_expr(
                lhs,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
            rhs: Box::new(fold_const_eval_expr(
                rhs,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
        }),
        Expr::Logical { loc, op, lhs, rhs } => Some(Expr::Logical {
            loc: *loc,
            op: *op,
            lhs: Box::new(fold_const_eval_expr(
                lhs,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
            rhs: Box::new(fold_const_eval_expr(
                rhs,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
        }),
        Expr::Binary { loc, op, lhs, rhs } => Some(Expr::Binary {
            loc: *loc,
            op: *op,
            lhs: Box::new(fold_const_eval_expr(
                lhs,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
            rhs: Box::new(fold_const_eval_expr(
                rhs,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
        }),
        Expr::Cast { loc, to, expr } => Some(Expr::Cast {
            loc: *loc,
            to: *to,
            expr: Box::new(fold_const_eval_expr(
                expr,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
        }),
        Expr::UnaryNot { loc, expr } => Some(Expr::UnaryNot {
            loc: *loc,
            expr: Box::new(fold_const_eval_expr(
                expr,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
        }),
        Expr::UnaryBitNot { loc, expr } => Some(Expr::UnaryBitNot {
            loc: *loc,
            expr: Box::new(fold_const_eval_expr(
                expr,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?),
        }),
        Expr::Tuple { .. } | Expr::ArrayCtor { .. } => {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: expression is not supported in const def evaluation"),
                loc,
            ));
            None
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => Some(expr.clone()),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_scalar_expr_with_defs(
    expr: &Expr,
    expected_ty: PrimitiveType,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    let folded = fold_const_eval_expr(
        expr,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        context,
        call_stack,
        errors,
    )?;
    eval_typed_const_expr(
        &folded,
        expected_ty,
        options,
        context,
        is_float_type(expected_ty),
        matches!(expected_ty, PrimitiveType::I32 | PrimitiveType::I64),
        errors,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn infer_const_scalar_expr_type_with_defs(
    expr: &Expr,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    let folded = fold_const_eval_expr(
        expr,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        context,
        call_stack,
        errors,
    )?;
    let inferred = infer_const_expr_type(&folded, options, context, errors);
    effective_untyped_assignment_type(&folded, inferred)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn infer_const_decl_scalar_type_with_defs(
    expr: &Expr,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    let folded = fold_const_eval_expr(
        expr,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        context,
        call_stack,
        errors,
    )?;
    infer_const_expr_type(&folded, options, context, errors)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_def_call(
    name: &str,
    args: &[CallArg],
    inherited_locals: &HashMap<String, TypedConstValue>,
    inherited_local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
    loc: SourceLoc,
) -> Option<ConstEvalValue> {
    let Some(def) = const_defs.defs.get(name) else {
        errors.push(Diagnostic::semantic_span(
            format!("{context}: unknown const def '{name}'"),
            loc,
        ));
        return None;
    };
    if call_stack.iter().any(|entry| entry == name) {
        errors.push(Diagnostic::semantic_span(
            format!("{context}: recursive const def call involving '{name}'"),
            loc,
        ));
        return None;
    }
    if let Some(caller) = call_stack.last() {
        if let (Some(caller_order), Some(callee_order)) =
            (const_defs.order.get(caller), const_defs.order.get(name))
        {
            if callee_order >= caller_order {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "{context}: const def '{name}' is not visible from const def '{caller}'; const defs can only call earlier visible const defs"
                    ),
                    loc,
                ));
                return None;
            }
        }
    }
    if !def.type_params.is_empty() {
        errors.push(Diagnostic::semantic_span(
            format!("const def '{}' cannot declare type parameters", def.name),
            def.loc,
        ));
        return None;
    }
    call_stack.push(name.to_owned());
    let return_ty =
        const_def_return_type(def, options, const_values, const_defs, call_stack, errors);
    let param_kinds =
        const_def_param_signature(def, options, const_values, const_defs, call_stack, errors);
    call_stack.pop();
    let return_ty = return_ty?;
    let param_kinds = param_kinds?;
    let param_names = def
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    let param_defaults = def
        .params
        .iter()
        .map(|param| param.default.clone())
        .collect::<Vec<_>>();
    let before_errors = errors.len();
    let resolved = resolve_call_args_at(
        args,
        &param_names,
        &param_defaults,
        false,
        false,
        &format!("const def '{name}' call"),
        loc,
        errors,
    );
    if errors.len() != before_errors {
        return None;
    }

    let mut locals = HashMap::<String, TypedConstValue>::new();
    let mut local_arrays = HashMap::<String, ConstEvalArray>::new();
    let mut read_only_arrays = HashSet::<String>::new();
    for (idx, param_name) in param_names.iter().enumerate() {
        let explicit_arg = resolved.get(idx).and_then(|expr| *expr);
        let (arg_expr, is_default_arg) = match explicit_arg {
            Some(expr) => (expr, false),
            None => (
                param_defaults.get(idx).and_then(|expr| expr.as_ref())?,
                true,
            ),
        };
        if is_default_arg {
            call_stack.push(name.to_owned());
        }
        let evaluated_arg = match param_kinds[idx] {
            ConstDefParamKind::Scalar(param_ty) => eval_const_scalar_expr_with_defs(
                arg_expr,
                param_ty,
                inherited_locals,
                inherited_local_arrays,
                const_values,
                const_defs,
                options,
                &format!("const def '{name}' argument '{param_name}'"),
                call_stack,
                errors,
            )
            .map(ConstEvalValue::Scalar),
            ConstDefParamKind::Array { elem_ty, len } => eval_const_array_expr_with_defs(
                arg_expr,
                ConstArrayExpectation::fixed(elem_ty, len),
                inherited_locals,
                inherited_local_arrays,
                const_values,
                const_defs,
                options,
                &format!("const def '{name}' argument '{param_name}'"),
                call_stack,
                errors,
            )
            .map(ConstEvalValue::Array),
            ConstDefParamKind::Slice { elem_ty } => eval_const_array_expr_with_defs(
                arg_expr,
                elem_ty.map_or_else(ConstArrayExpectation::any, ConstArrayExpectation::elem),
                inherited_locals,
                inherited_local_arrays,
                const_values,
                const_defs,
                options,
                &format!("const def '{name}' argument '{param_name}'"),
                call_stack,
                errors,
            )
            .map(ConstEvalValue::Array),
        };
        if is_default_arg {
            call_stack.pop();
        }
        match evaluated_arg? {
            ConstEvalValue::Scalar(value) => {
                locals.insert(param_name.clone(), value);
            }
            ConstEvalValue::Array(array) => {
                if matches!(param_kinds[idx], ConstDefParamKind::Slice { .. }) {
                    read_only_arrays.insert(param_name.clone());
                }
                local_arrays.insert(param_name.clone(), array);
            }
        }
    }

    call_stack.push(name.to_owned());
    let out = eval_const_def_body(
        def,
        return_ty,
        &mut locals,
        &mut local_arrays,
        &read_only_arrays,
        const_values,
        const_defs,
        options,
        call_stack,
        errors,
    );
    call_stack.pop();
    out
}

pub(super) fn eval_const_def_body(
    def: &FunctionDef,
    return_ty: ConstDefReturn,
    locals: &mut HashMap<String, TypedConstValue>,
    local_arrays: &mut HashMap<String, ConstEvalArray>,
    read_only_arrays: &HashSet<String>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ConstEvalValue> {
    let mut local_const_names = HashSet::new();
    eval_const_def_stmt_list(
        &def.body,
        return_ty,
        locals,
        local_arrays,
        read_only_arrays,
        &mut local_const_names,
        const_values,
        const_defs,
        options,
        call_stack,
        errors,
        &def.name,
    )
    .or_else(|| {
        errors.push(Diagnostic::semantic_span(
            format!("const def '{}' must return a value", def.name),
            def.loc,
        ));
        None
    })
}

pub(super) fn check_const_eval_array_expected(
    array: &ConstEvalArray,
    expected: ConstArrayExpectation,
    context: &str,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if expected.is_any() {
        return true;
    }
    let elem_ok = expected
        .elem_ty
        .is_none_or(|expected_elem| array.elem_ty == expected_elem);
    let len_ok = expected
        .len
        .is_none_or(|expected_len| array.len() == expected_len);
    if elem_ok && len_ok {
        return true;
    }
    let expected_label = match (expected.elem_ty, expected.len) {
        (Some(elem_ty), Some(len)) => fixed_array_type_label(elem_ty, len),
        (Some(elem_ty), None) => format!("{}[]", primitive_type_label(elem_ty)),
        (None, Some(len)) => format!("array[{len}]"),
        (None, None) => unreachable!(),
    };
    errors.push(Diagnostic::semantic_span(
        format!(
            "{context}: expected {}, got {}",
            expected_label,
            fixed_array_type_label(array.elem_ty, array.len())
        ),
        loc,
    ));
    false
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_i64_expr_with_defs(
    expr: &Expr,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<i64> {
    let folded = fold_const_eval_expr(
        expr,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        context,
        call_stack,
        errors,
    )?;
    if !can_eval_const_expr_exact_int(&folded) {
        errors.push(Diagnostic::semantic_span(
            format!("{context}: expression is not a compile-time integer"),
            expr.loc(),
        ));
        return None;
    }
    eval_const_expr_i64_exact(&folded, options, context, errors)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_array_size_with_defs(
    expr: &Expr,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let folded = fold_const_eval_expr(
        expr,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        context,
        call_stack,
        errors,
    )?;
    eval_data_size_expr(&folded, options, context, errors)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_slice_bound_with_defs(
    expr: Option<&Expr>,
    total_len: usize,
    default_to_len: bool,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let Some(expr) = expr else {
        return Some(if default_to_len { total_len } else { 0 });
    };
    let folded = fold_const_eval_expr(
        expr,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        context,
        call_stack,
        errors,
    )?;
    let raw = if can_eval_const_expr_exact_int(&folded) {
        eval_const_expr_i64_exact(&folded, options, context, errors)?
    } else {
        let value = eval_const_expr_f64(&folded, options, context, errors)?;
        if !value.is_finite() {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: expression must be finite"),
                expr.loc(),
            ));
            return None;
        }
        let rounded = value.round();
        if (value - rounded).abs() > 1e-6 {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: expression is not a compile-time integer"),
                expr.loc(),
            ));
            return None;
        }
        rounded as i64
    };
    let adjusted = if raw < 0 { total_len as i64 + raw } else { raw };
    Some(adjusted.clamp(0, total_len as i64) as usize)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_slice_bounds_with_defs(
    base: &str,
    total_len: usize,
    start: Option<&Expr>,
    end: Option<&Expr>,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<(usize, usize)> {
    let start_idx = eval_const_slice_bound_with_defs(
        start,
        total_len,
        false,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        &format!("{context}: const array '{base}' slice start"),
        call_stack,
        errors,
    )?;
    let end_idx = eval_const_slice_bound_with_defs(
        end,
        total_len,
        true,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        &format!("{context}: const array '{base}' slice end"),
        call_stack,
        errors,
    )?;
    if end_idx <= start_idx {
        let loc = SourceLoc::spanning(
            start.and_then(|expr| expr.loc().cloned()),
            end.and_then(|expr| expr.loc().cloned()),
        );
        errors.push(Diagnostic::semantic_span(
            format!("{context}: const array '{base}' slice must have positive length"),
            loc,
        ));
        return None;
    }
    Some((start_idx, end_idx))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_array_expr_with_defs(
    expr: &Expr,
    expected: ConstArrayExpectation,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ConstEvalArray> {
    let loc = expr.loc();
    let array = match expr {
        Expr::Var { name, .. } => const_eval_array_by_name(name, local_arrays, const_values)
            .or_else(|| {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: unknown const array '{name}'"),
                    loc,
                ));
                None
            })?,
        Expr::UserCall {
            name,
            args,
            type_args,
            ..
        } => {
            if !type_args.is_empty() {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: const def calls cannot use explicit type arguments"),
                    loc,
                ));
                return None;
            }
            match eval_const_def_call(
                name,
                args,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
                loc,
            )? {
                ConstEvalValue::Array(array) => array,
                ConstEvalValue::Scalar(_) => {
                    errors.push(Diagnostic::semantic_span(
                        format!("{context}: const def '{name}' returns a scalar, not an array"),
                        loc,
                    ));
                    return None;
                }
            }
        }
        Expr::ArrayLiteral { values, .. } => {
            if values.is_empty() {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: array literal cannot be empty"),
                    loc,
                ));
                return None;
            }
            if let Some(expected_len) = expected.len {
                if values.len() != expected_len {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context}: expected array length {expected_len}, got {}",
                            values.len()
                        ),
                        loc,
                    ));
                    return None;
                }
            }
            let elem_ty = expected.elem_ty.or_else(|| {
                let folded = fold_const_eval_expr(
                    &values[0],
                    locals,
                    local_arrays,
                    const_values,
                    const_defs,
                    options,
                    &format!("{context} element 0"),
                    call_stack,
                    errors,
                )?;
                let inferred = infer_const_expr_type(
                    &folded,
                    options,
                    &format!("{context} element 0"),
                    errors,
                );
                effective_untyped_assignment_type(&folded, inferred)
            })?;
            let mut typed_values = Vec::with_capacity(values.len());
            for (idx, value) in values.iter().enumerate() {
                typed_values.push(eval_const_scalar_expr_with_defs(
                    value,
                    elem_ty,
                    locals,
                    local_arrays,
                    const_values,
                    const_defs,
                    options,
                    &format!("{context} element {idx}"),
                    call_stack,
                    errors,
                )?);
            }
            ConstEvalArray {
                elem_ty,
                values: typed_values,
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            let ArrayElemType::Primitive(elem_ty) = spec.elem else {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: const arrays can only use primitive element types"),
                    loc,
                ));
                return None;
            };
            let len = eval_const_array_size_with_defs(
                &spec.size,
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?;
            let values = if let Some(init) = init {
                if init.len() != len {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context}: declares length {len}, but initializer has {} element(s)",
                            init.len()
                        ),
                        loc,
                    ));
                    return None;
                }
                init.iter()
                    .enumerate()
                    .map(|(idx, value)| {
                        eval_const_scalar_expr_with_defs(
                            value,
                            elem_ty,
                            locals,
                            local_arrays,
                            const_values,
                            const_defs,
                            options,
                            &format!("{context} element {idx}"),
                            call_stack,
                            errors,
                        )
                    })
                    .collect::<Option<Vec<_>>>()?
            } else {
                vec![zero_const_value(elem_ty); len]
            };
            ConstEvalArray { elem_ty, values }
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            if selector.is_some() || channel.is_some() {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: const arrays do not support buffer coordinates"),
                    loc,
                ));
                return None;
            }
            let Some((elem_ty, array)) =
                const_eval_array_ref_by_name(base, local_arrays, const_values)
            else {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: unknown const array '{base}'"),
                    loc,
                ));
                return None;
            };
            let (start_idx, end_idx) = eval_const_slice_bounds_with_defs(
                base,
                array.len(),
                start.as_deref(),
                end.as_deref(),
                locals,
                local_arrays,
                const_values,
                const_defs,
                options,
                context,
                call_stack,
                errors,
            )?;
            ConstEvalArray {
                elem_ty,
                values: array[start_idx..end_idx].to_vec(),
            }
        }
        _ => {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: expression does not evaluate to a const array"),
                loc,
            ));
            return None;
        }
    };

    if !check_const_eval_array_expected(&array, expected, context, loc, errors) {
        return None;
    }
    Some(array)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_array_index(
    base: &str,
    index: &Expr,
    array_len: usize,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let raw_idx = eval_const_i64_expr_with_defs(
        index,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        &format!("{context}: array '{base}' index"),
        call_stack,
        errors,
    )?;
    let Ok(idx) = usize::try_from(raw_idx) else {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{context}: array '{base}' index {raw_idx} is out of bounds for length {array_len}"
            ),
            index.loc(),
        ));
        return None;
    };
    if idx >= array_len {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{context}: array '{base}' index {raw_idx} is out of bounds for length {array_len}"
            ),
            index.loc(),
        ));
        return None;
    }
    Some(idx)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_for_stmt(
    var: &str,
    step: Option<&Expr>,
    start: &Expr,
    end: &Expr,
    end_inclusive: bool,
    body: &[Stmt],
    return_ty: ConstDefReturn,
    locals: &mut HashMap<String, TypedConstValue>,
    local_arrays: &mut HashMap<String, ConstEvalArray>,
    read_only_arrays: &HashSet<String>,
    local_const_names: &mut HashSet<String>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
    def_name: &str,
) -> Option<ConstEvalValue> {
    let start_value = eval_const_i64_expr_with_defs(
        start,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        &format!("const def '{def_name}' for start"),
        call_stack,
        errors,
    )?;
    let end_value = eval_const_i64_expr_with_defs(
        end,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        &format!("const def '{def_name}' for end"),
        call_stack,
        errors,
    )?;
    let step_value = if let Some(step) = step {
        eval_const_i64_expr_with_defs(
            step,
            locals,
            local_arrays,
            const_values,
            const_defs,
            options,
            &format!("const def '{def_name}' for step"),
            call_stack,
            errors,
        )?
    } else {
        1
    };
    if step_value == 0 {
        errors.push(Diagnostic::semantic_span(
            format!("const def '{def_name}' for step cannot be 0"),
            step.map(Expr::loc).unwrap_or_else(|| start.loc()),
        ));
        return None;
    }
    if local_const_names.contains(var) {
        errors.push(Diagnostic::semantic_span(
            format!("const def '{def_name}' cannot assign to local const '{var}'"),
            start.loc(),
        ));
        return None;
    }

    let saved_loop_var = locals.get(var).copied();
    let pre_loop_scalar_names = locals.keys().cloned().collect::<HashSet<_>>();
    let pre_loop_array_names = local_arrays.keys().cloned().collect::<HashSet<_>>();
    let pre_loop_const_names = local_const_names.clone();
    let mut current = start_value;
    let mut iterations = 0usize;
    loop {
        let in_range = if step_value > 0 {
            if end_inclusive {
                current <= end_value
            } else {
                current < end_value
            }
        } else if end_inclusive {
            current >= end_value
        } else {
            current > end_value
        };
        if !in_range {
            break;
        }
        if iterations >= CONST_DEF_LOOP_ITERATION_LIMIT {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const def '{def_name}' loop exceeded {CONST_DEF_LOOP_ITERATION_LIMIT} iterations"
                ),
                start.loc(),
            ));
            return None;
        }
        iterations += 1;
        let loop_value = match i32::try_from(current) {
            Ok(value) => TypedConstValue::I32(value),
            Err(_) => TypedConstValue::I64(current),
        };
        locals.insert(var.to_owned(), loop_value);
        let before_errors = errors.len();
        if let Some(returned) = eval_const_def_stmt_list(
            body,
            return_ty,
            locals,
            local_arrays,
            read_only_arrays,
            local_const_names,
            const_values,
            const_defs,
            options,
            call_stack,
            errors,
            def_name,
        ) {
            match saved_loop_var {
                Some(value) => {
                    locals.insert(var.to_owned(), value);
                }
                None => {
                    locals.remove(var);
                }
            }
            return Some(returned);
        }
        if errors.len() != before_errors {
            return None;
        }
        current = match current.checked_add(step_value) {
            Some(value) => value,
            None => {
                errors.push(Diagnostic::semantic_span(
                    format!("const def '{def_name}' for loop index overflowed"),
                    start.loc(),
                ));
                return None;
            }
        };
    }

    locals.retain(|name, _| pre_loop_scalar_names.contains(name));
    local_arrays.retain(|name, _| pre_loop_array_names.contains(name));
    local_const_names.retain(|name| pre_loop_const_names.contains(name));
    match saved_loop_var {
        Some(value) => {
            locals.insert(var.to_owned(), value);
        }
        None => {
            locals.remove(var);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_def_stmt_list(
    stmts: &[Stmt],
    return_ty: ConstDefReturn,
    locals: &mut HashMap<String, TypedConstValue>,
    local_arrays: &mut HashMap<String, ConstEvalArray>,
    read_only_arrays: &HashSet<String>,
    local_const_names: &mut HashSet<String>,
    const_values: &HashMap<String, ConstValue>,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    call_stack: &mut Vec<String>,
    errors: &mut Vec<Diagnostic>,
    def_name: &str,
) -> Option<ConstEvalValue> {
    for stmt in stmts {
        match stmt {
            Stmt::Return { expr, .. } => match return_ty {
                ConstDefReturn::Scalar(return_ty) => {
                    return eval_const_scalar_expr_with_defs(
                        expr,
                        return_ty,
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        &format!("const def '{def_name}' return"),
                        call_stack,
                        errors,
                    )
                    .map(ConstEvalValue::Scalar);
                }
                ConstDefReturn::Array { elem_ty, len } => {
                    return eval_const_array_expr_with_defs(
                        expr,
                        ConstArrayExpectation::fixed(elem_ty, len),
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        &format!("const def '{def_name}' return"),
                        call_stack,
                        errors,
                    )
                    .map(ConstEvalValue::Array);
                }
            },
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                expr,
                ..
            } => {
                if generic_decl_ty.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def '{def_name}' local declarations cannot use generic types"
                        ),
                        stmt.assign_target_loc(),
                    ));
                    return None;
                }

                if let AssignTarget::Index { base, index } = target {
                    if read_only_arrays.contains(base) {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "const def '{def_name}' cannot write read-only array parameter '{base}'"
                            ),
                            stmt.assign_target_loc(),
                        ));
                        return None;
                    }
                    let Some(array) = local_arrays.get(base) else {
                        errors.push(Diagnostic::semantic_span(
                            format!("const def '{def_name}' can only write indexed local arrays"),
                            stmt.assign_target_loc(),
                        ));
                        return None;
                    };
                    let idx = eval_const_array_index(
                        base,
                        index,
                        array.len(),
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        &format!("const def '{def_name}'"),
                        call_stack,
                        errors,
                    )?;
                    let value = eval_const_scalar_expr_with_defs(
                        expr,
                        array.elem_ty,
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        &format!("const def '{def_name}' array '{base}' element {idx}"),
                        call_stack,
                        errors,
                    )?;
                    let array = local_arrays.get_mut(base)?;
                    array.values[idx] = value;
                    continue;
                }

                let AssignTarget::Var(name) = target else {
                    errors.push(Diagnostic::semantic_span(
                        format!("const def '{def_name}' can only assign scalar locals or indexed local arrays"),
                        stmt.assign_target_loc(),
                    ));
                    return None;
                };
                if local_const_names.contains(name) {
                    errors.push(Diagnostic::semantic_span(
                        format!("const def '{def_name}' cannot assign to local const '{name}'"),
                        stmt.assign_target_loc(),
                    ));
                    return None;
                }
                if matches!(expr, Expr::ArrayCtor { .. }) {
                    let array = eval_const_array_expr_with_defs(
                        expr,
                        ConstArrayExpectation::any(),
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        &format!("const def '{def_name}' local array '{name}'"),
                        call_stack,
                        errors,
                    )?;
                    locals.remove(name);
                    local_arrays.insert(name.clone(), array);
                    continue;
                }
                if local_arrays.contains_key(name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def '{def_name}' cannot assign a scalar to local array '{name}'"
                        ),
                        stmt.assign_target_loc(),
                    ));
                    return None;
                }
                let ty = if let Some(ty) = decl_ty.as_ref().and_then(DeclType::scalar) {
                    ty
                } else if decl_ty.is_some() {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def '{def_name}' local '{name}' cannot use a tuple declaration"
                        ),
                        stmt.assign_decl_type_loc(),
                    ));
                    return None;
                } else if let Some(existing) = locals.get(name).copied() {
                    existing.primitive_type()
                } else {
                    infer_const_scalar_expr_type_with_defs(
                        expr,
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        &format!("const def '{def_name}' local '{name}'"),
                        call_stack,
                        errors,
                    )?
                };
                let value = eval_const_scalar_expr_with_defs(
                    expr,
                    ty,
                    locals,
                    local_arrays,
                    const_values,
                    const_defs,
                    options,
                    &format!("const def '{def_name}' local '{name}'"),
                    call_stack,
                    errors,
                )?;
                locals.insert(name.clone(), value);
            }
            Stmt::Const { decl, .. } => {
                if is_const_array_decl(decl) {
                    errors.push(Diagnostic::semantic_span(
                        format!("const def '{def_name}' local const arrays are not supported"),
                        decl.loc.as_ref(),
                    ));
                    return None;
                }
                let ty = match decl.ty.as_ref() {
                    Some(ConstType::Scalar(ty)) => *ty,
                    Some(ConstType::Array { .. } | ConstType::Slice { .. }) => {
                        errors.push(Diagnostic::semantic_span(
                            format!("const def '{def_name}' local const arrays are not supported"),
                            decl.loc.as_ref(),
                        ));
                        return None;
                    }
                    None => infer_const_scalar_expr_type_with_defs(
                        &decl.expr,
                        locals,
                        local_arrays,
                        const_values,
                        const_defs,
                        options,
                        &format!("const def '{def_name}' local const '{}'", decl.name),
                        call_stack,
                        errors,
                    )?,
                };
                let value = eval_const_scalar_expr_with_defs(
                    &decl.expr,
                    ty,
                    locals,
                    local_arrays,
                    const_values,
                    const_defs,
                    options,
                    &format!("const def '{def_name}' local const '{}'", decl.name),
                    call_stack,
                    errors,
                )?;
                if !local_const_names.insert(decl.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "duplicate constant '{}' in const def '{def_name}'",
                            decl.name
                        ),
                        decl.loc.as_ref(),
                    ));
                    return None;
                }
                locals.insert(decl.name.clone(), value);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let value = eval_const_scalar_expr_with_defs(
                    cond,
                    PrimitiveType::Bool,
                    locals,
                    local_arrays,
                    const_values,
                    const_defs,
                    options,
                    &format!("const def '{def_name}' if condition"),
                    call_stack,
                    errors,
                )?;
                let TypedConstValue::Bool(take_then) = value else {
                    errors.push(Diagnostic::semantic_span(
                        format!("const def '{def_name}' if condition must be bool"),
                        cond.loc(),
                    ));
                    return None;
                };
                let mut branch_locals = locals.clone();
                let mut branch_arrays = local_arrays.clone();
                let mut branch_const_names = local_const_names.clone();
                if let Some(returned) = eval_const_def_stmt_list(
                    if take_then { then_branch } else { else_branch },
                    return_ty,
                    &mut branch_locals,
                    &mut branch_arrays,
                    read_only_arrays,
                    &mut branch_const_names,
                    const_values,
                    const_defs,
                    options,
                    call_stack,
                    errors,
                    def_name,
                ) {
                    return Some(returned);
                }
                *locals = branch_locals;
                *local_arrays = branch_arrays;
                *local_const_names = branch_const_names;
            }
            Stmt::For {
                var,
                step,
                start,
                end,
                end_inclusive,
                body,
                ..
            } => {
                let before_errors = errors.len();
                if let Some(returned) = eval_const_for_stmt(
                    var,
                    step.as_ref(),
                    start,
                    end,
                    *end_inclusive,
                    body,
                    return_ty,
                    locals,
                    local_arrays,
                    read_only_arrays,
                    local_const_names,
                    const_values,
                    const_defs,
                    options,
                    call_stack,
                    errors,
                    def_name,
                ) {
                    return Some(returned);
                }
                if errors.len() != before_errors {
                    return None;
                }
            }
            Stmt::Print { .. } => {
                errors.push(Diagnostic::semantic_span(
                    format!("print is not allowed in const def '{def_name}'"),
                    stmt.loc(),
                ));
                return None;
            }
            Stmt::Expr { .. } | Stmt::While { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {
                errors.push(Diagnostic::semantic_span(
                    format!("const def '{def_name}' statement is not supported"),
                    stmt.loc(),
                ));
                return None;
            }
        }
    }
    None
}
