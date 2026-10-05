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
    match decl.ty {
        Some(ConstType::Scalar(_)) => false,
        Some(ConstType::Array { .. } | ConstType::Slice { .. }) => true,
        None => matches!(
            decl.expr,
            Expr::ArrayLiteral { .. } | Expr::ArrayCtor { .. } | Expr::Slice { .. }
        ),
    }
}

pub(super) fn is_known_const_array_initializer(
    expr: &Expr,
    const_values: &ConstValues,
    const_array_infos: &HashMap<String, TypedArrayInfo>,
    const_defs: &HashMap<String, std::rc::Rc<ConstDefinition>>,
) -> bool {
    match expr {
        Expr::Var { name, .. } => {
            const_values.array_info(name).is_some() || const_array_infos.contains_key(name)
        }
        Expr::UserCall {
            name, type_args, ..
        } if type_args.is_empty() => const_defs
            .get(name)
            .is_some_and(|def| matches!(def.return_ty, Some(FnReturnType::Array { .. }))),
        _ => false,
    }
}

pub(super) fn typed_const_expr_with_loc(value: TypedConstValue, loc: SourceLoc) -> Expr {
    typed_const_expr(value).with_loc(loc)
}

/// Concrete values must not become contextual numeric literals when folded.
/// Named constants and constant-indexed reads retain their separate literal policy.
pub(super) fn concrete_const_expr(value: TypedConstValue, loc: SourceLoc) -> Expr {
    let expr = typed_const_expr(value);
    let expr = match value {
        TypedConstValue::F64(_) | TypedConstValue::I64(_) => {
            cast_expr_to_primitive(expr, value.primitive_type())
        }
        _ => expr,
    };
    expr.with_loc(loc)
}

pub(super) fn const_array_literal_expr(values: &[TypedConstValue], loc: SourceLoc) -> Expr {
    Expr::ArrayLiteral {
        loc: loc.into(),
        values: values
            .iter()
            .map(|value| concrete_const_expr(*value, loc))
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
        substitute_scalar_const_expr(size, consts);
    }
}

pub(super) fn fold_host_sr_const_decl(
    decl: &mut ConstDecl,
    consts: &HashMap<String, TypedConstValue>,
) {
    fold_host_sr_const_type(&mut decl.ty, consts);
    substitute_scalar_const_expr(&mut decl.expr, consts);
}

pub(super) fn fold_host_sr_event(event: &mut EventDef, consts: &HashMap<String, TypedConstValue>) {
    for param in &mut event.params {
        substitute_scalar_const_event_param_type(&mut param.ty, consts);
        if let Some(default) = &mut param.default {
            substitute_scalar_const_expr(default, consts);
        }
    }
    fold_host_sr_stmts(&mut event.body, consts);
}

pub(super) fn fold_host_sr_delegate(
    delegate: &mut DelegateDef,
    consts: &HashMap<String, TypedConstValue>,
) {
    for param in &mut delegate.params {
        substitute_scalar_const_event_param_type(&mut param.ty, consts);
        if let Some(default) = &mut param.default {
            substitute_scalar_const_expr(default, consts);
        }
    }
}

pub(super) fn fold_host_sr_when(when: &mut WhenDef, consts: &HashMap<String, TypedConstValue>) {
    if let Some(index) = &mut when.target.index {
        substitute_scalar_const_expr(index, consts);
    }
    fold_host_sr_stmts(&mut when.body, consts);
}

pub(super) fn fold_host_sr_function(
    def: &mut FunctionDef,
    consts: &HashMap<String, TypedConstValue>,
) {
    for param in &mut def.params {
        substitute_scalar_const_fn_param_type(&mut param.ty, consts);
        if let Some(default) = &mut param.default {
            substitute_scalar_const_expr(default, consts);
        }
    }
    substitute_scalar_const_return_type(&mut def.return_ty, consts);
    fold_host_sr_stmts(&mut def.body, consts);
}

pub(super) fn fold_host_sr_assign_target(
    target: &mut AssignTarget,
    consts: &HashMap<String, TypedConstValue>,
) {
    match target {
        AssignTarget::Index { .. } | AssignTarget::IndexedMember { .. } => {
            target.visit_selectors_mut(|selector| substitute_scalar_const_expr(selector, consts))
        }
        AssignTarget::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                substitute_scalar_const_expr(coordinate, consts);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

pub(super) fn fold_host_sr_stmt(stmt: &mut Stmt, consts: &HashMap<String, TypedConstValue>) {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            expr,
            ..
        } => {
            fold_host_sr_assign_target(target, consts);
            substitute_scalar_const_decl_type(decl_ty, consts);
            substitute_scalar_const_expr(expr, consts);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            substitute_scalar_const_expr(expr, consts);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                substitute_scalar_const_expr(value, consts);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            substitute_scalar_const_expr(cond, consts);
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
                substitute_scalar_const_expr(step, consts);
            }
            substitute_scalar_const_expr(start, consts);
            substitute_scalar_const_expr(end, consts);
            fold_host_sr_stmts(body, consts);
        }
        Stmt::While { cond, body, .. } => {
            substitute_scalar_const_expr(cond, consts);
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
        substitute_scalar_const_expr(&mut edge.source, consts);
        if let Some(delay) = &mut edge.delay {
            substitute_scalar_const_expr(delay, consts);
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                substitute_scalar_const_expr(index, consts);
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
            substitute_scalar_const_expr(&mut arg.expr, consts);
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
        substitute_scalar_const_expr(&mut param.default, consts);
    }
    for item in &mut namespace.items {
        match item {
            NamespaceItem::Assert(assert_decl) => {
                substitute_scalar_const_expr(&mut assert_decl.expr, consts);
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
        substitute_scalar_const_field_type(&mut field.ty, consts);
        if let Some(default) = &mut field.default {
            substitute_scalar_const_expr(default, consts);
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
        substitute_scalar_const_expr(expr, consts);
    }
    for ty in [
        &mut proc.ins_deferred_default_ty,
        &mut proc.outs_deferred_default_ty,
        &mut proc.params_deferred_default_ty,
        &mut proc.init.default_ty,
    ] {
        substitute_scalar_const_decl_type(ty, consts);
    }
    substitute_scalar_const_buffer_type(&mut proc.buffers_deferred_default_ty, consts);
    for decl in &mut proc.ins {
        substitute_scalar_const_port_decl(decl, consts);
    }
    for decl in &mut proc.outs {
        substitute_scalar_const_port_decl(decl, consts);
    }
    for decl in &mut proc.params {
        substitute_scalar_const_param_decl(decl, consts);
    }
    for decl in &mut proc.buffers {
        substitute_scalar_const_buffer_type(&mut decl.ty, consts);
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
                substitute_scalar_const_expr(count, consts);
            }
            substitute_scalar_const_decl_type(&mut ports.deferred_default_ty, consts);
            for decl in &mut ports.decls {
                substitute_scalar_const_port_decl(decl, consts);
            }
        }
        Block::Params(params) => {
            if let Some(count) = &mut params.deferred_count {
                substitute_scalar_const_expr(count, consts);
            }
            substitute_scalar_const_decl_type(&mut params.deferred_default_ty, consts);
            for decl in &mut params.decls {
                substitute_scalar_const_param_decl(decl, consts);
            }
        }
        Block::Buffers(buffers) => {
            if let Some(count) = &mut buffers.deferred_count {
                substitute_scalar_const_expr(count, consts);
            }
            substitute_scalar_const_buffer_type(&mut buffers.deferred_default_ty, consts);
            for decl in &mut buffers.decls {
                substitute_scalar_const_buffer_type(&mut decl.ty, consts);
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
            substitute_scalar_const_expr(&mut assert_decl.expr, consts);
        }
        Block::Namespace(namespace) => fold_host_sr_namespace(namespace, consts),
        Block::NamespaceAlias(alias) => fold_host_sr_namespace_alias(alias, consts),
        Block::Use(use_decl) => fold_host_sr_use(use_decl, consts),
        Block::Proc(proc) => fold_host_sr_proc(proc, consts),
        Block::Struct(struct_def) => fold_host_sr_struct(struct_def, consts),
        Block::Def(def) => fold_host_sr_function(def, consts),
        Block::Init(init) => {
            substitute_scalar_const_decl_type(&mut init.default_ty, consts);
            fold_host_sr_stmts(&mut init.body, consts);
        }
        Block::Block(block_exec) => {
            fold_host_sr_stmts(&mut block_exec.pre, consts);
            if let Some(sample) = &mut block_exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    substitute_scalar_const_expr(factor, consts);
                }
                fold_host_sr_stmts(&mut sample.body, consts);
            }
            fold_host_sr_stmts(&mut block_exec.post, consts);
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                substitute_scalar_const_expr(factor, consts);
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
    pub(super) values: std::sync::Arc<Vec<TypedConstValue>>,
}

impl ConstEvalArray {
    pub(super) fn len(&self) -> usize {
        self.values.len()
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(super) enum ConstDefReturn<L = usize> {
    Scalar(PrimitiveType),
    Array { elem_ty: PrimitiveType, len: L },
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(super) enum ConstDefParamKind<L = usize> {
    Scalar(PrimitiveType),
    Array { elem_ty: PrimitiveType, len: L },
    Slice { elem_ty: Option<PrimitiveType> },
}

pub(super) type ConstArrayExpectation = crate::array_semantics::ArrayShape<PrimitiveType>;

/// Types at their lexical program points. The declaration stays inside its Rc,
/// so AST addresses are stable for the lifetime of this private metadata.
#[derive(Debug, Default)]
pub(super) struct ConstBodyTypes {
    pub(super) reads: HashMap<*const Expr, PrimitiveType>,
    pub(super) scalars: HashMap<*const Stmt, PrimitiveType>,
    pub(super) arrays: HashMap<*const Stmt, ConstArrayExpectation>,
    pub(super) branch_scopes: HashMap<*const Stmt, ConstBranchScope>,
}

#[derive(Debug)]
pub(super) struct ConstBranchScope {
    pub(super) bindings: CheckedBranchScope,
    /// Known sibling shapes constrain unresolved lengths at the executed join,
    /// without evaluating the other branch's dimensions or payloads.
    pub(super) arrays: Vec<(String, ConstArrayExpectation)>,
}

type ConstTypeChecks =
    HashMap<Vec<ConstArrayExpectation>, Result<std::rc::Rc<ConstBodyTypes>, Vec<Diagnostic>>>;

/// Declaration metadata is prepared once, in the declaration's context.
#[derive(Debug)]
pub(super) struct ConstDefinition {
    pub(super) declaration: FunctionDef,
    pub(super) signature: FnSignature,
    pub(super) param_kinds: Option<Vec<ConstDefParamKind>>,
    pub(super) result: Option<ConstDefReturn>,
    pub(super) environment: ConstEnvironment,
    /// Body checks depend on argument metadata, never argument values.
    pub(super) type_checks: RefCell<ConstTypeChecks>,
}

impl std::ops::Deref for ConstDefinition {
    type Target = FunctionDef;
    fn deref(&self) -> &Self::Target {
        &self.declaration
    }
}

pub(super) type ConstDefRegistry<'a> = &'a HashMap<String, std::rc::Rc<ConstDefinition>>;

#[derive(Debug, Clone, Default)]
pub(super) struct SemanticConstArtifacts {
    pub(super) const_array_infos: HashMap<String, TypedArrayInfo>,
    pub(super) const_values: ConstValues,
    pub(super) const_defs: HashMap<String, std::rc::Rc<ConstDefinition>>,
}

pub(super) struct ConstSymbolSet<'a>(HashSet<&'a str>);

impl ConstSymbolSet<'_> {
    pub(super) fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

pub(super) fn const_symbol_set(artifacts: &SemanticConstArtifacts) -> ConstSymbolSet<'_> {
    ConstSymbolSet(artifacts.const_values.keys().map(String::as_str).collect())
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
    const_symbols: &ConstSymbolSet<'_>,
) -> Option<String> {
    if name.contains('.') {
        return None;
    }
    if name.contains("::") {
        return const_symbols.contains(name).then(|| name.to_owned());
    }
    for ns in namespace_candidates(scope_ns) {
        let candidate = namespace_join(&ns, name);
        if const_symbols.contains(&candidate) {
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

pub(super) fn validate_const_def_declaration(def: &FunctionDef, errors: &mut Vec<Diagnostic>) {
    crate::callable_validation::validate_function_param_names(def, &def.name, errors);
    if !def.type_params.is_empty() {
        errors.push(Diagnostic::semantic_span(
            format!("const def '{}' cannot declare type parameters", def.name),
            def.loc,
        ));
        return;
    }
    validate_const_def_body_shape(def, errors);
}

pub(super) fn validate_const_def_body_shape(def: &FunctionDef, errors: &mut Vec<Diagnostic>) {
    let read_only_arrays = def
        .params
        .iter()
        .filter(|param| param.readonly)
        .map(|param| param.name.clone())
        .collect::<HashSet<_>>();
    let immutable_loop_vars = HashSet::new();
    validate_const_def_stmt_shapes(
        &def.body,
        &def.name,
        &read_only_arrays,
        &immutable_loop_vars,
        errors,
    );
}

pub(super) fn validate_const_def_stmt_shapes(
    stmts: &[Stmt],
    def_name: &str,
    read_only_arrays: &HashSet<String>,
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
                        if name.contains("::") {
                            errors.push(Diagnostic::semantic_span(
                                format!("cannot assign to constant '{name}'"),
                                stmt.assign_target_loc(),
                            ));
                        } else if immutable_loop_vars.contains(name) {
                            errors.push(Diagnostic::semantic_span(
                                format!("cannot assign to loop variable '{name}'"),
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
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                validate_const_def_stmt_shapes(
                    then_branch,
                    def_name,
                    read_only_arrays,
                    immutable_loop_vars,
                    errors,
                );
                validate_const_def_stmt_shapes(
                    else_branch,
                    def_name,
                    read_only_arrays,
                    immutable_loop_vars,
                    errors,
                );
            }
            Stmt::For { var, body, .. } => {
                let mut loop_vars = immutable_loop_vars.clone();
                loop_vars.insert(var.clone());
                validate_const_def_stmt_shapes(
                    body,
                    def_name,
                    read_only_arrays,
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

#[allow(clippy::too_many_arguments)]
pub(super) fn fold_const_eval_expr(
    expr: &Expr,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &ConstValues,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &[String],
    errors: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    const_interpreter::fold_expression(
        expr,
        locals,
        local_arrays,
        const_values,
        const_defs,
        options,
        context,
        call_stack,
        errors,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_scalar_expr_with_defs(
    expr: &Expr,
    expected_ty: PrimitiveType,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &ConstValues,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &[String],
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
        errors,
    )
}

pub(super) fn check_const_array_shape(
    elem_ty: PrimitiveType,
    len: usize,
    expected: ConstArrayExpectation,
    context: &str,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if crate::array_semantics::check_array_shape(
        ConstArrayExpectation::fixed(elem_ty, len),
        expected,
    )
    .is_ok()
    {
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
            fixed_array_type_label(elem_ty, len)
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
    const_values: &ConstValues,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &[String],
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
    const_values: &ConstValues,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &[String],
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
    eval_array_size_expr(&folded, options, context, errors)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_slice_bound_with_defs(
    expr: Option<&Expr>,
    total_len: usize,
    default_to_len: bool,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &ConstValues,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &[String],
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
    let raw = crate::builtins::eval_const_slice_integer(&folded, options, context, errors)?;
    Some(crate::stmt_analysis::normalize_slice_bound(raw, total_len))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn eval_const_slice_bounds_with_defs(
    base: &str,
    total_len: usize,
    start: Option<&Expr>,
    end: Option<&Expr>,
    locals: &HashMap<String, TypedConstValue>,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &ConstValues,
    const_defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    call_stack: &[String],
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
