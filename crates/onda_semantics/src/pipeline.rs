use std::collections::{HashMap, HashSet};

use onda_frontend::Span;

use crate::processor_lowering::{
    coerce_typed_events, collect_runtime_state_roots, desugar_processors,
    internal_proc_index_call_signature, lower_graph_blocks, nested_call_out_fn_name,
    nested_step_fn_name, prepare_processors_for_graph_inspection, proc_runtime_analysis_options,
    validated_sample_oversample_factor, ProcLoweringShape, ProcessorDesugarResult,
    TopLevelProcRewriteMeta,
};
use crate::*;

mod namespace_flattening;
use namespace_flattening::flatten_namespaces_for_semantics;

fn validate_analysis_options(options: AnalysisOptions) -> Result<(), Vec<Diagnostic>> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'sample_rate' must be finite and greater than zero",
        )]);
    }
    if options.block_size == 0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'block_size' must be greater than zero",
        )]);
    }
    Ok(())
}

fn preprocess_program_for_analysis(
    mut program: Program,
    options: AnalysisOptions,
) -> Result<Program, Vec<Diagnostic>> {
    validate_analysis_options(options)?;
    inject_auto_std_math(&mut program)?;
    flatten_namespaces_for_semantics(&mut program, options)?;
    fold_host_sr_builtin(&mut program, options);
    Ok(program)
}

fn evaluate_asserts(program: &mut Program, options: AnalysisOptions, errors: &mut Vec<Diagnostic>) {
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

fn is_const_array_decl(decl: &onda_frontend::ConstDecl) -> bool {
    matches!(
        decl.ty,
        Some(ConstType::Array { .. } | ConstType::Slice { .. })
    ) || matches!(
        decl.expr,
        Expr::ArrayLiteral { .. } | Expr::ArrayCtor { .. } | Expr::Slice { .. }
    )
}

fn is_known_const_array_initializer(
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

fn const_array_info_map(const_arrays: &[TypedConstArray]) -> HashMap<String, TypedArrayInfo> {
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

fn typed_const_expr_with_loc(value: TypedConstValue, loc: SourceLoc) -> Expr {
    typed_const_expr(value).with_loc(loc)
}

fn const_array_literal_expr(values: &[TypedConstValue], loc: SourceLoc) -> Expr {
    Expr::ArrayLiteral {
        loc: loc.into(),
        values: values
            .iter()
            .map(|value| typed_const_expr_with_loc(*value, loc))
            .collect(),
    }
}

fn host_sr_const_map(options: AnalysisOptions) -> HashMap<String, TypedConstValue> {
    host_sample_rate_constant_names()
        .map(|name| (name.to_owned(), TypedConstValue::F32(options.sample_rate)))
        .collect()
}

fn fold_host_sr_const_type(ty: &mut Option<ConstType>, consts: &HashMap<String, TypedConstValue>) {
    if let Some(ConstType::Array { size, .. }) = ty {
        fold_local_scalar_const_expr(size, consts);
    }
}

fn fold_host_sr_const_decl(decl: &mut ConstDecl, consts: &HashMap<String, TypedConstValue>) {
    fold_host_sr_const_type(&mut decl.ty, consts);
    fold_local_scalar_const_expr(&mut decl.expr, consts);
}

fn fold_host_sr_event(event: &mut EventDef, consts: &HashMap<String, TypedConstValue>) {
    for param in &mut event.params {
        fold_local_scalar_const_event_param_type(&mut param.ty, consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, consts);
        }
    }
    fold_host_sr_stmts(&mut event.body, consts);
}

fn fold_host_sr_function(def: &mut FunctionDef, consts: &HashMap<String, TypedConstValue>) {
    for param in &mut def.params {
        fold_local_scalar_const_fn_param_type(&mut param.ty, consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, consts);
        }
    }
    fold_local_scalar_const_return_type(&mut def.return_ty, consts);
    fold_host_sr_stmts(&mut def.body, consts);
}

fn fold_host_sr_assign_target(
    target: &mut AssignTarget,
    consts: &HashMap<String, TypedConstValue>,
) {
    match target {
        AssignTarget::Index { index, .. } => fold_local_scalar_const_expr(index, consts),
        AssignTarget::Slice { start, end, .. } => {
            if let Some(start) = start {
                fold_local_scalar_const_expr(start, consts);
            }
            if let Some(end) = end {
                fold_local_scalar_const_expr(end, consts);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

fn fold_host_sr_stmt(stmt: &mut Stmt, consts: &HashMap<String, TypedConstValue>) {
    match stmt {
        Stmt::Const { decl, .. } => fold_host_sr_const_decl(decl, consts),
        Stmt::Assign { target, expr, .. } => {
            fold_host_sr_assign_target(target, consts);
            fold_local_scalar_const_expr(expr, consts);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            fold_local_scalar_const_expr(expr, consts);
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

fn fold_host_sr_stmts(stmts: &mut [Stmt], consts: &HashMap<String, TypedConstValue>) {
    for stmt in stmts {
        fold_host_sr_stmt(stmt, consts);
    }
}

fn fold_host_sr_graph(graph: &mut GraphBlock, consts: &HashMap<String, TypedConstValue>) {
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

fn fold_host_sr_namespace_ref_segment(
    segment: &mut NamespaceRefSegment,
    consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(args) = &mut segment.args {
        for arg in args {
            fold_local_scalar_const_expr(&mut arg.expr, consts);
        }
    }
}

fn fold_host_sr_namespace_alias(
    alias: &mut NamespaceAliasDecl,
    consts: &HashMap<String, TypedConstValue>,
) {
    for segment in &mut alias.target {
        fold_host_sr_namespace_ref_segment(segment, consts);
    }
}

fn fold_host_sr_use(use_decl: &mut UseDecl, consts: &HashMap<String, TypedConstValue>) {
    for segment in &mut use_decl.target {
        fold_host_sr_namespace_ref_segment(segment, consts);
    }
}

fn fold_host_sr_namespace(
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

fn fold_host_sr_struct(struct_def: &mut StructDef, consts: &HashMap<String, TypedConstValue>) {
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

fn fold_host_sr_proc(proc: &mut ProcessorDef, consts: &HashMap<String, TypedConstValue>) {
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
    for def in &mut proc.local_defs {
        fold_host_sr_function(def, consts);
    }
}

fn fold_host_sr_block(block: &mut Block, consts: &HashMap<String, TypedConstValue>) {
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

fn fold_host_sr_builtin(program: &mut Program, options: AnalysisOptions) {
    let consts = host_sr_const_map(options);
    for block in &mut program.blocks {
        fold_host_sr_block(block, &consts);
    }
}

fn typed_const_value_type(value: TypedConstValue) -> PrimitiveType {
    match value {
        TypedConstValue::F32(_) => PrimitiveType::F32,
        TypedConstValue::F64(_) => PrimitiveType::F64,
        TypedConstValue::I32(_) => PrimitiveType::I32,
        TypedConstValue::I64(_) => PrimitiveType::I64,
        TypedConstValue::Bool(_) => PrimitiveType::Bool,
    }
}

const CONST_DEF_LOOP_ITERATION_LIMIT: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq)]
struct ConstEvalArray {
    elem_ty: PrimitiveType,
    values: Vec<TypedConstValue>,
}

impl ConstEvalArray {
    fn len(&self) -> usize {
        self.values.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ConstEvalValue {
    Scalar(TypedConstValue),
    Array(ConstEvalArray),
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum ConstDefReturn {
    Scalar(PrimitiveType),
    Array { elem_ty: PrimitiveType, len: usize },
}

#[derive(Debug, Copy, Clone, PartialEq)]
enum ConstDefParamKind {
    Scalar(PrimitiveType),
    Array { elem_ty: PrimitiveType, len: usize },
    Slice { elem_ty: Option<PrimitiveType> },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct ConstArrayExpectation {
    elem_ty: Option<PrimitiveType>,
    len: Option<usize>,
}

impl ConstArrayExpectation {
    fn any() -> Self {
        Self {
            elem_ty: None,
            len: None,
        }
    }

    fn fixed(elem_ty: PrimitiveType, len: usize) -> Self {
        Self {
            elem_ty: Some(elem_ty),
            len: Some(len),
        }
    }

    fn elem(elem_ty: PrimitiveType) -> Self {
        Self {
            elem_ty: Some(elem_ty),
            len: None,
        }
    }

    fn is_any(self) -> bool {
        self.elem_ty.is_none() && self.len.is_none()
    }
}

#[derive(Copy, Clone)]
struct ConstDefRegistry<'a> {
    defs: &'a HashMap<String, FunctionDef>,
    order: &'a HashMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
struct SemanticConstArtifacts {
    const_arrays: Vec<TypedConstArray>,
    const_values: HashMap<String, ConstValue>,
    const_defs: HashMap<String, FunctionDef>,
    const_def_order: HashMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
struct ProcLocalConstArtifacts {
    values: HashMap<String, TypedConstValue>,
}

fn record_const_array_artifact(artifacts: &mut SemanticConstArtifacts, array: TypedConstArray) {
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

fn ordinary_top_level_symbol_names(program: &Program) -> HashSet<String> {
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

fn top_level_const_symbol_names(program: &Program) -> HashSet<String> {
    program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Const(decl) => Some(decl.name.clone()),
            _ => None,
        })
        .collect()
}

fn namespace_parent(ns: &str) -> Option<&str> {
    ns.rsplit_once("::").map(|(parent, _)| parent)
}

fn namespace_candidates(current_ns: &str) -> Vec<String> {
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

fn namespace_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}::{child}")
    }
}

fn symbol_namespace(name: &str) -> String {
    name.rsplit_once("::")
        .map(|(namespace, _)| namespace.to_owned())
        .unwrap_or_default()
}

fn visible_const_symbol_for_local_name(
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

fn zero_const_value(ty: PrimitiveType) -> TypedConstValue {
    match ty {
        PrimitiveType::F32 => TypedConstValue::F32(0.0),
        PrimitiveType::F64 => TypedConstValue::F64(0.0),
        PrimitiveType::I32 => TypedConstValue::I32(0),
        PrimitiveType::I64 => TypedConstValue::I64(0),
        PrimitiveType::Bool => TypedConstValue::Bool(false),
    }
}

fn fold_const_array_expr(
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
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                fold_const_array_expr(start, const_values, options, errors, false);
            }
            if let Some(end) = end {
                fold_const_array_expr(end, const_values, options, errors, false);
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

fn const_def_param_signature(
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

fn const_def_return_type(
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

fn validate_const_def_declaration(
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

fn validate_const_def_body_shape(
    def: &FunctionDef,
    param_kinds: Option<&[ConstDefParamKind]>,
    errors: &mut Vec<Diagnostic>,
) {
    let read_only_arrays = param_kinds
        .map(|kinds| {
            def.params
                .iter()
                .zip(kinds.iter())
                .filter_map(|(param, kind)| {
                    matches!(kind, ConstDefParamKind::Slice { .. }).then(|| param.name.clone())
                })
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let mut local_const_names = HashSet::new();
    validate_const_def_stmt_shapes(
        &def.body,
        &def.name,
        &read_only_arrays,
        &mut local_const_names,
        errors,
    );
}

fn validate_const_def_stmt_shapes(
    stmts: &[Stmt],
    def_name: &str,
    read_only_arrays: &HashSet<String>,
    local_const_names: &mut HashSet<String>,
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
                        if local_const_names.contains(name) {
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
                    errors,
                );
                let mut else_const_names = local_const_names.clone();
                validate_const_def_stmt_shapes(
                    else_branch,
                    def_name,
                    read_only_arrays,
                    &mut else_const_names,
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
                validate_const_def_stmt_shapes(
                    body,
                    def_name,
                    read_only_arrays,
                    &mut loop_const_names,
                    errors,
                );
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

fn eval_const_builtin_call(
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
    let values = args
        .iter()
        .enumerate()
        .map(|(idx, arg)| {
            eval_const_expr_f64(arg, options, &format!("{context} argument {idx}"), errors)
        })
        .collect::<Option<Vec<_>>>()?;
    let value = match func {
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
        BuiltinFn::Abs
        | BuiltinFn::Floor
        | BuiltinFn::Ceil
        | BuiltinFn::Round
        | BuiltinFn::Trunc => match func {
            BuiltinFn::Abs => values[0].abs(),
            BuiltinFn::Floor => values[0].floor(),
            BuiltinFn::Ceil => values[0].ceil(),
            BuiltinFn::Round => values[0].round(),
            BuiltinFn::Trunc => values[0].trunc(),
            _ => unreachable!(),
        },
        BuiltinFn::Min => values[0].min(values[1]),
        BuiltinFn::Max => values[0].max(values[1]),
        BuiltinFn::Fma => values[0].mul_add(values[1], values[2]),
    };
    Some(Expr::number(value).with_loc(loc))
}

fn const_eval_array_by_name(
    name: &str,
    local_arrays: &HashMap<String, ConstEvalArray>,
    const_values: &HashMap<String, ConstValue>,
) -> Option<ConstEvalArray> {
    if let Some(array) = local_arrays.get(name) {
        return Some(array.clone());
    }
    match const_values.get(name) {
        Some(ConstValue::Array {
            elem_ty, values, ..
        }) => Some(ConstEvalArray {
            elem_ty: *elem_ty,
            values: values.clone(),
        }),
        _ => None,
    }
}

fn fold_const_eval_expr(
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
            if let Some(array) = const_eval_array_by_name(base, local_arrays, const_values) {
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
                let Some(value) = array.values.get(idx).copied() else {
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
fn eval_const_scalar_expr_with_defs(
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
fn infer_const_scalar_expr_type_with_defs(
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
fn infer_const_decl_scalar_type_with_defs(
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
fn eval_const_def_call(
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

fn eval_const_def_body(
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

fn check_const_eval_array_expected(
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
        .map_or(true, |expected_elem| array.elem_ty == expected_elem);
    let len_ok = expected
        .len
        .map_or(true, |expected_len| array.len() == expected_len);
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
fn eval_const_i64_expr_with_defs(
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
fn eval_const_array_size_with_defs(
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
fn eval_const_slice_bound_with_defs(
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
fn eval_const_slice_bounds_with_defs(
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
fn eval_const_array_expr_with_defs(
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
            base, start, end, ..
        } => {
            let Some(array) = const_eval_array_by_name(base, local_arrays, const_values) else {
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
                elem_ty: array.elem_ty,
                values: array.values[start_idx..end_idx].to_vec(),
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
fn eval_const_array_index(
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
fn eval_const_for_stmt(
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
fn eval_const_def_stmt_list(
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
                    let Some(array) = local_arrays.get_mut(base) else {
                        return None;
                    };
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
                let ty = if let Some(ty) = decl_ty {
                    *ty
                } else if let Some(existing) = locals.get(name).copied() {
                    typed_const_value_type(existing)
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

fn fold_decl_type_const_arrays(
    ty: &mut Option<DeclType>,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            fold_const_array_expr(size, const_values, options, errors, false);
        }
        Some(DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_)) | None => {}
    }
}

fn fixed_array_default_target(
    ty: &Option<DeclType>,
    options: AnalysisOptions,
) -> Option<(Option<PrimitiveType>, usize)> {
    match ty {
        Some(DeclType::Array { elem, size }) => {
            eval_data_size_expr_silent(size, options).map(|len| (Some(*elem), len))
        }
        Some(DeclType::ArrayGeneric { size, .. }) => {
            eval_data_size_expr_silent(size, options).map(|len| (None, len))
        }
        _ => None,
    }
}

fn event_array_default_target(
    ty: &EventParamType,
    options: AnalysisOptions,
) -> Option<(Option<PrimitiveType>, usize)> {
    match ty {
        EventParamType::Array { elem, size } => {
            eval_data_size_expr_silent(size, options).map(|len| (Some(*elem), len))
        }
        EventParamType::GenericArray { size, .. } => {
            eval_data_size_expr_silent(size, options).map(|len| (None, len))
        }
        _ => None,
    }
}

fn eval_data_size_expr_silent(expr: &Expr, options: AnalysisOptions) -> Option<usize> {
    let mut ignored = Vec::new();
    eval_data_size_expr(expr, options, "array size", &mut ignored)
}

fn fixed_array_type_label(elem_ty: PrimitiveType, len: usize) -> String {
    format!("{}[{len}]", primitive_type_label(elem_ty))
}

fn const_array_default_incompatible(
    default_expr: &Expr,
    expected: (Option<PrimitiveType>, usize),
    context: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Expr::Var { name, .. } = default_expr else {
        return false;
    };
    let Some(ConstValue::Array { elem_ty, len, .. }) = const_values.get(name) else {
        return false;
    };
    let (expected_elem, expected_len) = expected;
    match expected_elem {
        Some(expected_elem) => {
            if *elem_ty == expected_elem && *len == expected_len {
                return false;
            }
            errors.push(Diagnostic::semantic_span(
                format!(
                    "{context} default const array '{name}' has type {}, expected {}",
                    fixed_array_type_label(*elem_ty, *len),
                    fixed_array_type_label(expected_elem, expected_len)
                ),
                default_expr.loc(),
            ));
            true
        }
        None => {
            if *len == expected_len {
                return false;
            }
            errors.push(Diagnostic::semantic_span(
                format!(
                    "{context} default const array '{name}' has length {len}, expected {expected_len}"
                ),
                default_expr.loc(),
            ));
            true
        }
    }
}

fn fold_fixed_array_default_const_arrays(
    default: &mut Option<Expr>,
    target: Option<(Option<PrimitiveType>, usize)>,
    context: &str,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let (Some(default_expr), Some(target)) = (default.as_ref(), target) {
        if const_array_default_incompatible(default_expr, target, context, const_values, errors) {
            *default = None;
            return;
        }
    }

    if let Some(default_expr) = default {
        fold_const_array_expr(
            default_expr,
            const_values,
            options,
            errors,
            target.is_some(),
        );
    }
}

fn fold_decl_range_const_arrays(
    range: &mut Option<DeclRange>,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            fold_const_array_expr(min, const_values, options, errors, false);
        }
        fold_const_array_expr(&mut range.max, const_values, options, errors, false);
    }
}

fn fold_port_decl_const_arrays(
    decl: &mut PortDecl,
    kind: &str,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    fold_decl_type_const_arrays(&mut decl.ty, const_values, options, errors);
    let target = fixed_array_default_target(&decl.ty, options);
    fold_fixed_array_default_const_arrays(
        &mut decl.default,
        target,
        &format!("{kind} '{}'", decl.name),
        const_values,
        options,
        errors,
    );
    fold_decl_range_const_arrays(&mut decl.range, const_values, options, errors);
}

fn fold_param_decl_const_arrays(
    decl: &mut ParamDecl,
    context: &str,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    fold_decl_type_const_arrays(&mut decl.ty, const_values, options, errors);
    let target = fixed_array_default_target(&decl.ty, options);
    fold_fixed_array_default_const_arrays(
        &mut decl.default,
        target,
        context,
        const_values,
        options,
        errors,
    );
    fold_decl_range_const_arrays(&mut decl.range, const_values, options, errors);
}

fn fold_buffer_type_const_arrays(
    ty: &mut Option<BufferType>,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        fold_const_array_expr(expr, const_values, options, errors, false);
    }
}

fn fold_fn_param_type_const_arrays(
    ty: &mut Option<FnParamType>,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            fold_const_array_expr(size, const_values, options, errors, false);
        }
        Some(FnParamType::Buffer(buffer_ty)) => {
            if let BufferChannels::Static(expr) = &mut buffer_ty.channels {
                fold_const_array_expr(expr, const_values, options, errors, false);
            }
        }
        Some(
            FnParamType::Primitive(_)
            | FnParamType::Struct(_)
            | FnParamType::Array(_)
            | FnParamType::ArrayGeneric(_)
            | FnParamType::BareBuffer
            | FnParamType::Tuple(_),
        )
        | None => {}
    }
}

fn fold_event_param_type_const_arrays(
    ty: &mut EventParamType,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            fold_const_array_expr(size, const_values, options, errors, false);
        }
        _ => {}
    }
}

fn fold_field_type_const_arrays(
    ty: &mut FieldType,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let FieldType::Array(spec) = ty {
        fold_const_array_expr(&mut spec.size, const_values, options, errors, false);
    }
}

fn fold_stmt_const_arrays(
    stmt: &mut Stmt,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            fold_const_array_expr(&mut decl.expr, const_values, options, errors, false);
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                fold_const_array_expr(size, const_values, options, errors, false);
            }
        }
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Index { index, .. } => {
                    fold_const_array_expr(index, const_values, options, errors, false);
                }
                AssignTarget::Slice { start, end, .. } => {
                    if let Some(start) = start {
                        fold_const_array_expr(start, const_values, options, errors, false);
                    }
                    if let Some(end) = end {
                        fold_const_array_expr(end, const_values, options, errors, false);
                    }
                }
                AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
            }
            fold_const_array_expr(expr, const_values, options, errors, false);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            fold_const_array_expr(expr, const_values, options, errors, false);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            fold_const_array_expr(cond, const_values, options, errors, false);
            for nested in then_branch {
                fold_stmt_const_arrays(nested, const_values, options, errors);
            }
            for nested in else_branch {
                fold_stmt_const_arrays(nested, const_values, options, errors);
            }
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                fold_const_array_expr(step, const_values, options, errors, false);
            }
            fold_const_array_expr(start, const_values, options, errors, false);
            fold_const_array_expr(end, const_values, options, errors, false);
            for nested in body {
                fold_stmt_const_arrays(nested, const_values, options, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            fold_const_array_expr(cond, const_values, options, errors, false);
            for nested in body {
                fold_stmt_const_arrays(nested, const_values, options, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn fold_function_const_arrays(
    def: &mut FunctionDef,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        fold_fn_param_type_const_arrays(&mut param.ty, const_values, options, errors);
        if let Some(default) = &mut param.default {
            fold_const_array_expr(default, const_values, options, errors, false);
        }
    }
    for stmt in &mut def.body {
        fold_stmt_const_arrays(stmt, const_values, options, errors);
    }
}

fn fold_event_const_arrays(
    event: &mut EventDef,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut event.params {
        fold_event_param_type_const_arrays(&mut param.ty, const_values, options, errors);
        let target = event_array_default_target(&param.ty, options);
        fold_fixed_array_default_const_arrays(
            &mut param.default,
            target,
            &format!("event '{}.{}'", event.name, param.name),
            const_values,
            options,
            errors,
        );
    }
    for stmt in &mut event.body {
        fold_stmt_const_arrays(stmt, const_values, options, errors);
    }
}

fn fold_graph_const_arrays(
    graph: &mut GraphBlock,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &mut graph.edges {
        fold_const_array_expr(&mut edge.source, const_values, options, errors, false);
        if let Some(delay) = &mut edge.delay {
            fold_const_array_expr(delay, const_values, options, errors, false);
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                fold_const_array_expr(index, const_values, options, errors, false);
            }
        }
    }
}

fn reject_forward_const_ref_name(
    name: &str,
    loc: SourceLoc,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if future_consts.contains(name) && !visible_consts.contains_key(name) {
        errors.push(Diagnostic::semantic_span(
            format!("constant '{name}' is not visible before its declaration"),
            loc,
        ));
    }
}

fn reject_forward_const_refs_expr(
    expr: &Expr,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Var { name, .. } => {
            reject_forward_const_ref_name(name, expr.loc(), visible_consts, future_consts, errors);
        }
        Expr::Index { base, index, .. } => {
            reject_forward_const_ref_name(base, expr.loc(), visible_consts, future_consts, errors);
            reject_forward_const_refs_expr(index, visible_consts, future_consts, errors);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            reject_forward_const_ref_name(base, expr.loc(), visible_consts, future_consts, errors);
            if let Some(start) = start {
                reject_forward_const_refs_expr(start, visible_consts, future_consts, errors);
            }
            if let Some(end) = end {
                reject_forward_const_refs_expr(end, visible_consts, future_consts, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            reject_forward_const_refs_expr(&spec.size, visible_consts, future_consts, errors);
            if let Some(init) = init {
                for value in init {
                    reject_forward_const_refs_expr(value, visible_consts, future_consts, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            reject_forward_const_refs_expr(lhs, visible_consts, future_consts, errors);
            reject_forward_const_refs_expr(rhs, visible_consts, future_consts, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                reject_forward_const_refs_expr(arg, visible_consts, future_consts, errors);
            }
        }
        Expr::UserCall { name, args, .. } => {
            if args.is_empty() {
                if let Some(base) = parse_array_len_instance_base(name) {
                    reject_forward_const_ref_name(
                        base,
                        expr.loc(),
                        visible_consts,
                        future_consts,
                        errors,
                    );
                }
            }
            if let Some((base, _method)) = name.rsplit_once('.') {
                reject_forward_const_ref_name(
                    base,
                    expr.loc(),
                    visible_consts,
                    future_consts,
                    errors,
                );
            }
            for arg in args {
                reject_forward_const_refs_expr(&arg.expr, visible_consts, future_consts, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                reject_forward_const_refs_expr(value, visible_consts, future_consts, errors);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn reject_forward_const_refs_decl_type(
    ty: &Option<DeclType>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
        }
        Some(DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_)) | None => {}
    }
}

fn reject_forward_const_refs_decl_range(
    range: &Option<DeclRange>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(range) = range {
        if let Some(min) = &range.min {
            reject_forward_const_refs_expr(min, visible_consts, future_consts, errors);
        }
        reject_forward_const_refs_expr(&range.max, visible_consts, future_consts, errors);
    }
}

fn reject_forward_const_refs_port_decl(
    decl: &PortDecl,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    reject_forward_const_refs_decl_type(&decl.ty, visible_consts, future_consts, errors);
    if let Some(default) = &decl.default {
        reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
    }
    reject_forward_const_refs_decl_range(&decl.range, visible_consts, future_consts, errors);
}

fn reject_forward_const_refs_param_decl(
    decl: &ParamDecl,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    reject_forward_const_refs_decl_type(&decl.ty, visible_consts, future_consts, errors);
    if let Some(default) = &decl.default {
        reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
    }
    reject_forward_const_refs_decl_range(&decl.range, visible_consts, future_consts, errors);
}

fn reject_forward_const_refs_buffer_type(
    ty: &Option<BufferType>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
    }
}

fn reject_forward_const_refs_fn_param_type(
    ty: &Option<FnParamType>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
        }
        Some(FnParamType::Buffer(buffer_ty)) => {
            if let BufferChannels::Static(expr) = &buffer_ty.channels {
                reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
            }
        }
        Some(
            FnParamType::Primitive(_)
            | FnParamType::Struct(_)
            | FnParamType::Array(_)
            | FnParamType::ArrayGeneric(_)
            | FnParamType::BareBuffer
            | FnParamType::Tuple(_),
        )
        | None => {}
    }
}

fn reject_forward_const_refs_event_param_type(
    ty: &EventParamType,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
        }
        _ => {}
    }
}

fn reject_forward_const_refs_field_type(
    ty: &FieldType,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    if let FieldType::Array(spec) = ty {
        reject_forward_const_refs_expr(&spec.size, visible_consts, future_consts, errors);
    }
}

fn reject_forward_const_refs_return_type(
    ty: &Option<FnReturnType>,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnReturnType::Array { size, .. }) => {
            reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
        }
        Some(FnReturnType::Scalar(_) | FnReturnType::Tuple(_)) | None => {}
    }
}

fn reject_forward_const_refs_assign_target(
    target: &AssignTarget,
    target_loc: SourceLoc,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match target {
        AssignTarget::Index { base, index } => {
            reject_forward_const_ref_name(base, target_loc, visible_consts, future_consts, errors);
            reject_forward_const_refs_expr(index, visible_consts, future_consts, errors);
        }
        AssignTarget::Slice { base, start, end } => {
            reject_forward_const_ref_name(base, target_loc, visible_consts, future_consts, errors);
            if let Some(start) = start {
                reject_forward_const_refs_expr(start, visible_consts, future_consts, errors);
            }
            if let Some(end) = end {
                reject_forward_const_refs_expr(end, visible_consts, future_consts, errors);
            }
        }
        AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
    }
}

fn reject_forward_const_refs_stmt(
    stmt: &Stmt,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            reject_forward_const_refs_expr(&decl.expr, visible_consts, future_consts, errors);
            if let Some(ConstType::Array { size, .. }) = &decl.ty {
                reject_forward_const_refs_expr(size, visible_consts, future_consts, errors);
            }
        }
        Stmt::Assign {
            target_loc,
            target,
            expr,
            ..
        } => {
            reject_forward_const_refs_assign_target(
                target,
                target_loc.as_ref().into(),
                visible_consts,
                future_consts,
                errors,
            );
            reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            reject_forward_const_refs_expr(expr, visible_consts, future_consts, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            reject_forward_const_refs_expr(cond, visible_consts, future_consts, errors);
            for nested in then_branch {
                reject_forward_const_refs_stmt(nested, visible_consts, future_consts, errors);
            }
            for nested in else_branch {
                reject_forward_const_refs_stmt(nested, visible_consts, future_consts, errors);
            }
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                reject_forward_const_refs_expr(step, visible_consts, future_consts, errors);
            }
            reject_forward_const_refs_expr(start, visible_consts, future_consts, errors);
            reject_forward_const_refs_expr(end, visible_consts, future_consts, errors);
            for nested in body {
                reject_forward_const_refs_stmt(nested, visible_consts, future_consts, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            reject_forward_const_refs_expr(cond, visible_consts, future_consts, errors);
            for nested in body {
                reject_forward_const_refs_stmt(nested, visible_consts, future_consts, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn reject_forward_const_refs_function(
    def: &FunctionDef,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &def.params {
        reject_forward_const_refs_fn_param_type(&param.ty, visible_consts, future_consts, errors);
        if let Some(default) = &param.default {
            reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
        }
    }
    reject_forward_const_refs_return_type(&def.return_ty, visible_consts, future_consts, errors);
    for stmt in &def.body {
        reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
    }
}

fn reject_forward_const_refs_event(
    event: &EventDef,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &event.params {
        reject_forward_const_refs_event_param_type(
            &param.ty,
            visible_consts,
            future_consts,
            errors,
        );
        if let Some(default) = &param.default {
            reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
        }
    }
    for stmt in &event.body {
        reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
    }
}

fn reject_forward_const_refs_graph(
    graph: &GraphBlock,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &graph.edges {
        reject_forward_const_refs_expr(&edge.source, visible_consts, future_consts, errors);
        if let Some(delay) = &edge.delay {
            reject_forward_const_refs_expr(delay, visible_consts, future_consts, errors);
        }
        for dest in &edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                reject_forward_const_refs_expr(index, visible_consts, future_consts, errors);
            }
        }
    }
}

fn reject_forward_const_refs_in_block(
    block: &Block,
    visible_consts: &HashMap<String, ConstValue>,
    future_consts: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(count) = &ports.deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            for decl in &ports.decls {
                reject_forward_const_refs_port_decl(decl, visible_consts, future_consts, errors);
            }
        }
        Block::Params(params) => {
            if let Some(count) = &params.deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            for decl in &params.decls {
                reject_forward_const_refs_param_decl(decl, visible_consts, future_consts, errors);
            }
        }
        Block::Events(events) => {
            for event in &events.events {
                reject_forward_const_refs_event(event, visible_consts, future_consts, errors);
            }
        }
        Block::Buffers(buffers) => {
            if let Some(count) = &buffers.deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            for decl in &buffers.decls {
                reject_forward_const_refs_buffer_type(
                    &decl.ty,
                    visible_consts,
                    future_consts,
                    errors,
                );
            }
        }
        Block::Init(init) => {
            for stmt in &init.body {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
        }
        Block::Block(block_exec) => {
            for stmt in &block_exec.pre {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            if let Some(sample) = &block_exec.sample {
                if let Some(factor) = &sample.oversample_factor {
                    reject_forward_const_refs_expr(factor, visible_consts, future_consts, errors);
                }
                for stmt in &sample.body {
                    reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
                }
            }
            for stmt in &block_exec.post {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
        }
        Block::Sample(sample) => {
            if let Some(factor) = &sample.oversample_factor {
                reject_forward_const_refs_expr(factor, visible_consts, future_consts, errors);
            }
            for stmt in &sample.body {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
        }
        Block::Graph(graph) => {
            reject_forward_const_refs_graph(graph, visible_consts, future_consts, errors);
        }
        Block::Assert(assert_decl) => {
            reject_forward_const_refs_expr(
                &assert_decl.expr,
                visible_consts,
                future_consts,
                errors,
            );
        }
        Block::Def(def) if !def.is_const => {
            reject_forward_const_refs_function(def, visible_consts, future_consts, errors);
        }
        Block::Struct(struct_def) => {
            for field in &struct_def.fields {
                reject_forward_const_refs_field_type(
                    &field.ty,
                    visible_consts,
                    future_consts,
                    errors,
                );
                if let Some(default) = &field.default {
                    reject_forward_const_refs_expr(default, visible_consts, future_consts, errors);
                }
            }
            for method in &struct_def.methods {
                reject_forward_const_refs_function(method, visible_consts, future_consts, errors);
            }
        }
        Block::Proc(proc) => {
            if let Some(count) = &proc.ins_deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            if let Some(default_ty) = &proc.ins_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, visible_consts, future_consts, errors);
            }
            if let Some(count) = &proc.outs_deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            if let Some(default_ty) = &proc.outs_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, visible_consts, future_consts, errors);
            }
            if let Some(count) = &proc.params_deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            if let Some(default_ty) = &proc.params_deferred_default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, visible_consts, future_consts, errors);
            }
            if let Some(count) = &proc.buffers_deferred_count {
                reject_forward_const_refs_expr(count, visible_consts, future_consts, errors);
            }
            reject_forward_const_refs_buffer_type(
                &proc.buffers_deferred_default_ty,
                visible_consts,
                future_consts,
                errors,
            );
            for decl in &proc.ins {
                reject_forward_const_refs_port_decl(decl, visible_consts, future_consts, errors);
            }
            for decl in &proc.outs {
                reject_forward_const_refs_port_decl(decl, visible_consts, future_consts, errors);
            }
            for decl in &proc.params {
                reject_forward_const_refs_param_decl(decl, visible_consts, future_consts, errors);
            }
            for decl in &proc.buffers {
                reject_forward_const_refs_buffer_type(
                    &decl.ty,
                    visible_consts,
                    future_consts,
                    errors,
                );
            }
            if let Some(default_ty) = &proc.init.default_ty {
                let ty = Some(default_ty.clone());
                reject_forward_const_refs_decl_type(&ty, visible_consts, future_consts, errors);
            }
            if let Some(factor) = &proc.sample_oversample_factor {
                reject_forward_const_refs_expr(factor, visible_consts, future_consts, errors);
            }
            for event in &proc.events {
                reject_forward_const_refs_event(event, visible_consts, future_consts, errors);
            }
            for stmt in &proc.init.body {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            for stmt in &proc.block_pre {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            for stmt in &proc.sample {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            for stmt in &proc.block_post {
                reject_forward_const_refs_stmt(stmt, visible_consts, future_consts, errors);
            }
            if let Some(graph) = &proc.graph {
                reject_forward_const_refs_graph(graph, visible_consts, future_consts, errors);
            }
            for def in &proc.local_defs {
                reject_forward_const_refs_function(def, visible_consts, future_consts, errors);
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

fn fold_const_array_exprs_in_block(
    block: &mut Block,
    const_values: &HashMap<String, ConstValue>,
    options: AnalysisOptions,
    structural_options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(ports) => {
            if let Some(count) = &mut ports.deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut ports.decls {
                fold_port_decl_const_arrays(decl, "input", const_values, options, errors);
            }
        }
        Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(count) = &mut ports.deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut ports.decls {
                fold_port_decl_const_arrays(decl, "output", const_values, options, errors);
            }
        }
        Block::Params(params) => {
            if let Some(count) = &mut params.deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut params.decls {
                fold_param_decl_const_arrays(
                    decl,
                    &format!("param '<top-level>.{}'", decl.name),
                    const_values,
                    options,
                    errors,
                );
            }
        }
        Block::Events(events) => {
            for event in &mut events.events {
                fold_event_const_arrays(event, const_values, options, errors);
            }
        }
        Block::Buffers(buffers) => {
            if let Some(count) = &mut buffers.deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut buffers.decls {
                fold_buffer_type_const_arrays(&mut decl.ty, const_values, options, errors);
            }
        }
        Block::Init(init) => {
            for stmt in &mut init.body {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
        }
        Block::Block(block_exec) => {
            for stmt in &mut block_exec.pre {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            if let Some(sample) = &mut block_exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    fold_const_array_expr(factor, const_values, options, errors, false);
                }
                for stmt in &mut sample.body {
                    fold_stmt_const_arrays(stmt, const_values, options, errors);
                }
            }
            for stmt in &mut block_exec.post {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                fold_const_array_expr(factor, const_values, options, errors, false);
            }
            for stmt in &mut sample.body {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
        }
        Block::Graph(graph) => {
            fold_graph_const_arrays(graph, const_values, options, errors);
        }
        Block::Assert(assert_decl) => {
            fold_const_array_expr(&mut assert_decl.expr, const_values, options, errors, false);
        }
        Block::Def(def) if !def.is_const => {
            fold_function_const_arrays(def, const_values, options, errors);
        }
        Block::Struct(struct_def) => {
            for field in &mut struct_def.fields {
                fold_field_type_const_arrays(&mut field.ty, const_values, options, errors);
                if let Some(default) = &mut field.default {
                    fold_const_array_expr(default, const_values, options, errors, false);
                }
            }
            for method in &mut struct_def.methods {
                fold_function_const_arrays(method, const_values, options, errors);
            }
        }
        Block::Proc(proc) => {
            for decl in &mut proc.ins {
                fold_port_decl_const_arrays(
                    decl,
                    &format!("processor '{}' input", proc.name),
                    const_values,
                    options,
                    errors,
                );
            }
            if let Some(count) = &mut proc.ins_deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut proc.outs {
                fold_port_decl_const_arrays(
                    decl,
                    &format!("processor '{}' output", proc.name),
                    const_values,
                    options,
                    errors,
                );
            }
            if let Some(count) = &mut proc.outs_deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut proc.params {
                fold_param_decl_const_arrays(
                    decl,
                    &format!("processor '{}' param '{}'", proc.name, decl.name),
                    const_values,
                    options,
                    errors,
                );
            }
            if let Some(count) = &mut proc.params_deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            for decl in &mut proc.buffers {
                fold_buffer_type_const_arrays(&mut decl.ty, const_values, options, errors);
            }
            if let Some(count) = &mut proc.buffers_deferred_count {
                fold_const_array_expr(count, const_values, options, errors, false);
            }
            if let Some(factor) = &mut proc.sample_oversample_factor {
                fold_const_array_expr(factor, const_values, structural_options, errors, false);
            }
            for event in &mut proc.events {
                fold_event_const_arrays(event, const_values, options, errors);
            }
            for stmt in &mut proc.init.body {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            for stmt in &mut proc.block_pre {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            for stmt in &mut proc.sample {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            for stmt in &mut proc.block_post {
                fold_stmt_const_arrays(stmt, const_values, options, errors);
            }
            if let Some(graph) = &mut proc.graph {
                fold_graph_const_arrays(graph, const_values, options, errors);
            }
            for def in &mut proc.local_defs {
                fold_function_const_arrays(def, const_values, options, errors);
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

fn reject_const_assignment_target(
    target: &AssignTarget,
    target_loc: &Span,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    if let AssignTarget::Var(name) = target {
        if const_values.contains_key(name) {
            errors.push(Diagnostic::semantic_span(
                format!("cannot assign to constant '{name}'"),
                target_loc.as_ref(),
            ));
        }
    }
}

fn reject_const_shadowing_name(
    symbol_kind: &str,
    name: &str,
    loc: Span,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(const_name) = visible_const_symbol_for_local_name(name, scope_ns, const_values) {
        errors.push(Diagnostic::semantic_span(
            format!("{symbol_kind} '{name}' conflicts with constant '{const_name}'"),
            loc.as_ref(),
        ));
    }
}

fn reject_const_shadowing_stmt(
    stmt: &Stmt,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target, target_loc, ..
        } => {
            if let AssignTarget::Tuple(names) = target {
                for name in names {
                    reject_const_shadowing_name(
                        "tuple assignment target",
                        name,
                        *target_loc,
                        scope_ns,
                        const_values,
                        errors,
                    );
                }
            }
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                reject_const_shadowing_stmt(nested, scope_ns, const_values, errors);
            }
            for nested in else_branch {
                reject_const_shadowing_stmt(nested, scope_ns, const_values, errors);
            }
        }
        Stmt::For { var, loc, body, .. } => {
            reject_const_shadowing_name("loop variable", var, *loc, scope_ns, const_values, errors);
            for nested in body {
                reject_const_shadowing_stmt(nested, scope_ns, const_values, errors);
            }
        }
        Stmt::While { body, .. } => {
            for nested in body {
                reject_const_shadowing_stmt(nested, scope_ns, const_values, errors);
            }
        }
        Stmt::Const { .. }
        | Stmt::Expr { .. }
        | Stmt::Return { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => {}
    }
}

fn reject_const_shadowing_function(
    def: &FunctionDef,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &def.params {
        reject_const_shadowing_name(
            "function parameter",
            &param.name,
            param.loc,
            scope_ns,
            const_values,
            errors,
        );
    }
    for stmt in &def.body {
        reject_const_shadowing_stmt(stmt, scope_ns, const_values, errors);
    }
}

fn reject_const_shadowing_event(
    event: &EventDef,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &event.params {
        reject_const_shadowing_name(
            "event parameter",
            &param.name,
            param.loc,
            scope_ns,
            const_values,
            errors,
        );
    }
    for stmt in &event.body {
        reject_const_shadowing_stmt(stmt, scope_ns, const_values, errors);
    }
}

fn reject_const_shadowing_proc_decl(
    proc_name: &str,
    symbol_kind: &str,
    name: &str,
    loc: Span,
    scope_ns: &str,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(const_name) = visible_const_symbol_for_local_name(name, scope_ns, const_values) {
        errors.push(Diagnostic::semantic_span(
            format!("{symbol_kind} '{name}' in processor '{proc_name}' conflicts with constant '{const_name}'"),
            loc.as_ref(),
        ));
    }
}

fn reject_const_shadowing_in_program(
    program: &Program,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for block in &program.blocks {
        match block {
            Block::Events(events) => {
                for event in &events.events {
                    let scope_ns = symbol_namespace(&event.name);
                    reject_const_shadowing_event(event, &scope_ns, const_values, errors);
                }
            }
            Block::Init(init) => {
                for stmt in &init.body {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
            }
            Block::Block(block_exec) => {
                for stmt in &block_exec.pre {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
                if let Some(sample) = &block_exec.sample {
                    for stmt in &sample.body {
                        reject_const_shadowing_stmt(stmt, "", const_values, errors);
                    }
                }
                for stmt in &block_exec.post {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
            }
            Block::Sample(sample) => {
                for stmt in &sample.body {
                    reject_const_shadowing_stmt(stmt, "", const_values, errors);
                }
            }
            Block::Def(def) if !def.is_const => {
                let scope_ns = symbol_namespace(&def.name);
                reject_const_shadowing_function(def, &scope_ns, const_values, errors);
            }
            Block::Struct(struct_def) => {
                let scope_ns = symbol_namespace(&struct_def.name);
                for method in &struct_def.methods {
                    reject_const_shadowing_function(method, &scope_ns, const_values, errors);
                }
            }
            Block::Proc(proc) => {
                let scope_ns = symbol_namespace(&proc.name);
                for decl in &proc.ins {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor input",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for decl in &proc.outs {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor output",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for decl in &proc.params {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor parameter",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for decl in &proc.buffers {
                    reject_const_shadowing_proc_decl(
                        &proc.name,
                        "processor buffer",
                        &decl.name,
                        decl.loc,
                        &scope_ns,
                        const_values,
                        errors,
                    );
                }
                for event in &proc.events {
                    reject_const_shadowing_event(event, &scope_ns, const_values, errors);
                }
                for stmt in &proc.init.body {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                }
                for stmt in &proc.block_pre {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                }
                for stmt in &proc.sample {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                }
                for stmt in &proc.block_post {
                    reject_const_shadowing_stmt(stmt, &scope_ns, const_values, errors);
                }
                for def in &proc.local_defs {
                    reject_const_shadowing_function(def, &scope_ns, const_values, errors);
                }
            }
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Buffers(_)
            | Block::Graph(_)
            | Block::Const(_)
            | Block::Def(_)
            | Block::Assert(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_) => {}
        }
    }
}

fn reject_const_assignments_stmt(
    stmt: &Stmt,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target_loc, target, ..
        } => reject_const_assignment_target(target, target_loc, const_values, errors),
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            for nested in then_branch {
                reject_const_assignments_stmt(nested, const_values, errors);
            }
            for nested in else_branch {
                reject_const_assignments_stmt(nested, const_values, errors);
            }
        }
        Stmt::For { body, .. } | Stmt::While { body, .. } => {
            for nested in body {
                reject_const_assignments_stmt(nested, const_values, errors);
            }
        }
        Stmt::Const { .. }
        | Stmt::Expr { .. }
        | Stmt::Return { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => {}
    }
}

fn reject_const_assignments_function(
    def: &FunctionDef,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in &def.body {
        reject_const_assignments_stmt(stmt, const_values, errors);
    }
}

fn reject_const_assignments_event(
    event: &EventDef,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in &event.body {
        reject_const_assignments_stmt(stmt, const_values, errors);
    }
}

fn reject_const_assignments_in_program(
    program: &Program,
    const_values: &HashMap<String, ConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for block in &program.blocks {
        match block {
            Block::Events(events) => {
                for event in &events.events {
                    reject_const_assignments_event(event, const_values, errors);
                }
            }
            Block::Init(init) => {
                for stmt in &init.body {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
            }
            Block::Block(block_exec) => {
                for stmt in &block_exec.pre {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                if let Some(sample) = &block_exec.sample {
                    for stmt in &sample.body {
                        reject_const_assignments_stmt(stmt, const_values, errors);
                    }
                }
                for stmt in &block_exec.post {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
            }
            Block::Sample(sample) => {
                for stmt in &sample.body {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
            }
            Block::Def(def) => reject_const_assignments_function(def, const_values, errors),
            Block::Struct(struct_def) => {
                for method in &struct_def.methods {
                    reject_const_assignments_function(method, const_values, errors);
                }
            }
            Block::Proc(proc) => {
                for event in &proc.events {
                    reject_const_assignments_event(event, const_values, errors);
                }
                for stmt in &proc.init.body {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                for stmt in &proc.block_pre {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                for stmt in &proc.sample {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                for stmt in &proc.block_post {
                    reject_const_assignments_stmt(stmt, const_values, errors);
                }
                for def in &proc.local_defs {
                    reject_const_assignments_function(def, const_values, errors);
                }
            }
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Buffers(_)
            | Block::Graph(_)
            | Block::Const(_)
            | Block::Assert(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_) => {}
        }
    }
}

fn const_def_registry(artifacts: &SemanticConstArtifacts) -> ConstDefRegistry<'_> {
    ConstDefRegistry {
        defs: &artifacts.const_defs,
        order: &artifacts.const_def_order,
    }
}

fn fold_direct_const_def_call_expr(
    expr: &mut Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let direct_call = match expr {
        Expr::UserCall {
            name,
            args,
            type_args,
            ..
        } if artifacts.const_defs.contains_key(name) => Some((
            name.clone(),
            args.clone(),
            !type_args.is_empty(),
            expr.loc(),
        )),
        _ => None,
    };
    if let Some((name, args, has_type_args, loc)) = direct_call {
        if has_type_args {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: const def calls cannot use explicit type arguments"),
                loc,
            ));
            return;
        }
        let locals = HashMap::new();
        let local_arrays = HashMap::new();
        if let Some(value) = eval_const_def_call(
            &name,
            &args,
            &locals,
            &local_arrays,
            &artifacts.const_values,
            const_def_registry(artifacts),
            options,
            context,
            &mut Vec::new(),
            errors,
            loc,
        ) {
            *expr = match value {
                ConstEvalValue::Scalar(value) => typed_const_expr_with_loc(value, loc),
                ConstEvalValue::Array(array) => const_array_literal_expr(&array.values, loc),
            };
        }
        return;
    }

    match expr {
        Expr::Index { index, .. } => {
            fold_direct_const_def_call_expr(index, artifacts, options, context, errors);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                fold_direct_const_def_call_expr(start, artifacts, options, context, errors);
            }
            if let Some(end) = end {
                fold_direct_const_def_call_expr(end, artifacts, options, context, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            fold_direct_const_def_call_expr(&mut spec.size, artifacts, options, context, errors);
            if let Some(init) = init {
                for value in init {
                    fold_direct_const_def_call_expr(value, artifacts, options, context, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            fold_direct_const_def_call_expr(lhs, artifacts, options, context, errors);
            fold_direct_const_def_call_expr(rhs, artifacts, options, context, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                fold_direct_const_def_call_expr(arg, artifacts, options, context, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                fold_direct_const_def_call_expr(&mut arg.expr, artifacts, options, context, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            fold_direct_const_def_call_expr(expr, artifacts, options, context, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                fold_direct_const_def_call_expr(value, artifacts, options, context, errors);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn fold_direct_const_def_decl_type(
    ty: &mut Option<DeclType>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            fold_direct_const_def_call_expr(size, artifacts, options, context, errors);
        }
        Some(DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_)) | None => {}
    }
}

fn fold_direct_const_def_buffer_type(
    ty: &mut Option<BufferType>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        fold_direct_const_def_call_expr(expr, artifacts, options, context, errors);
    }
}

fn fold_direct_const_def_fn_param_type(
    ty: &mut Option<FnParamType>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            fold_direct_const_def_call_expr(size, artifacts, options, context, errors);
        }
        Some(FnParamType::Buffer(buffer_ty)) => {
            if let BufferChannels::Static(expr) = &mut buffer_ty.channels {
                fold_direct_const_def_call_expr(expr, artifacts, options, context, errors);
            }
        }
        Some(
            FnParamType::Primitive(_)
            | FnParamType::Struct(_)
            | FnParamType::Array(_)
            | FnParamType::ArrayGeneric(_)
            | FnParamType::BareBuffer
            | FnParamType::Tuple(_),
        )
        | None => {}
    }
}

fn fold_direct_const_def_event_param_type(
    ty: &mut EventParamType,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            fold_direct_const_def_call_expr(size, artifacts, options, context, errors);
        }
        _ => {}
    }
}

fn fold_direct_const_def_field_type(
    ty: &mut FieldType,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let FieldType::Array(spec) = ty {
        fold_direct_const_def_call_expr(&mut spec.size, artifacts, options, context, errors);
    }
}

fn fold_direct_const_def_return_type(
    ty: &mut Option<FnReturnType>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        Some(FnReturnType::Array { size, .. }) => {
            fold_direct_const_def_call_expr(size, artifacts, options, context, errors);
        }
        Some(FnReturnType::Scalar(_) | FnReturnType::Tuple(_)) | None => {}
    }
}

fn fold_direct_const_def_decl_range(
    range: &mut Option<DeclRange>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            fold_direct_const_def_call_expr(min, artifacts, options, context, errors);
        }
        fold_direct_const_def_call_expr(&mut range.max, artifacts, options, context, errors);
    }
}

fn fold_direct_const_def_port_decl(
    decl: &mut PortDecl,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    fold_direct_const_def_decl_type(&mut decl.ty, artifacts, options, context, errors);
    if let Some(default) = &mut decl.default {
        fold_direct_const_def_call_expr(default, artifacts, options, context, errors);
    }
    fold_direct_const_def_decl_range(&mut decl.range, artifacts, options, context, errors);
}

fn fold_direct_const_def_param_decl(
    decl: &mut ParamDecl,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    fold_direct_const_def_decl_type(&mut decl.ty, artifacts, options, context, errors);
    if let Some(default) = &mut decl.default {
        fold_direct_const_def_call_expr(default, artifacts, options, context, errors);
    }
    fold_direct_const_def_decl_range(&mut decl.range, artifacts, options, context, errors);
}

fn fold_direct_const_def_stmt(
    stmt: &mut Stmt,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                fold_direct_const_def_call_expr(
                    size,
                    artifacts,
                    options,
                    &format!("local const '{}' size", decl.name),
                    errors,
                );
            }
            fold_direct_const_def_call_expr(
                &mut decl.expr,
                artifacts,
                options,
                &format!("local const '{}'", decl.name),
                errors,
            );
        }
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Index { index, .. } => {
                    fold_direct_const_def_call_expr(
                        index,
                        artifacts,
                        options,
                        "assignment target index",
                        errors,
                    );
                }
                AssignTarget::Slice { start, end, .. } => {
                    if let Some(start) = start {
                        fold_direct_const_def_call_expr(
                            start,
                            artifacts,
                            options,
                            "assignment target slice start",
                            errors,
                        );
                    }
                    if let Some(end) = end {
                        fold_direct_const_def_call_expr(
                            end,
                            artifacts,
                            options,
                            "assignment target slice end",
                            errors,
                        );
                    }
                }
                AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
            }
            fold_direct_const_def_call_expr(expr, artifacts, options, "assignment", errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            fold_direct_const_def_call_expr(expr, artifacts, options, "expression", errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            fold_direct_const_def_call_expr(cond, artifacts, options, "if condition", errors);
            for nested in then_branch {
                fold_direct_const_def_stmt(nested, artifacts, options, errors);
            }
            for nested in else_branch {
                fold_direct_const_def_stmt(nested, artifacts, options, errors);
            }
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                fold_direct_const_def_call_expr(step, artifacts, options, "for loop step", errors);
            }
            fold_direct_const_def_call_expr(start, artifacts, options, "for loop start", errors);
            fold_direct_const_def_call_expr(end, artifacts, options, "for loop end", errors);
            for nested in body {
                fold_direct_const_def_stmt(nested, artifacts, options, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            fold_direct_const_def_call_expr(cond, artifacts, options, "while condition", errors);
            for nested in body {
                fold_direct_const_def_stmt(nested, artifacts, options, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn fold_direct_const_def_function(
    def: &mut FunctionDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        fold_direct_const_def_fn_param_type(
            &mut param.ty,
            artifacts,
            options,
            &format!("function '{}' parameter '{}'", def.name, param.name),
            errors,
        );
        if let Some(default) = &mut param.default {
            fold_direct_const_def_call_expr(
                default,
                artifacts,
                options,
                &format!("function '{}' parameter '{}'", def.name, param.name),
                errors,
            );
        }
    }
    fold_direct_const_def_return_type(
        &mut def.return_ty,
        artifacts,
        options,
        &format!("function '{}' return type", def.name),
        errors,
    );
    for stmt in &mut def.body {
        fold_direct_const_def_stmt(stmt, artifacts, options, errors);
    }
}

fn fold_direct_const_def_event(
    event: &mut EventDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut event.params {
        fold_direct_const_def_event_param_type(
            &mut param.ty,
            artifacts,
            options,
            &format!("event '{}.{}'", event.name, param.name),
            errors,
        );
        if let Some(default) = &mut param.default {
            fold_direct_const_def_call_expr(
                default,
                artifacts,
                options,
                &format!("event '{}.{}'", event.name, param.name),
                errors,
            );
        }
    }
    for stmt in &mut event.body {
        fold_direct_const_def_stmt(stmt, artifacts, options, errors);
    }
}

fn fold_direct_const_def_graph(
    graph: &mut GraphBlock,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &mut graph.edges {
        fold_direct_const_def_call_expr(&mut edge.source, artifacts, options, "graph edge", errors);
        if let Some(delay) = &mut edge.delay {
            fold_direct_const_def_call_expr(delay, artifacts, options, "graph edge delay", errors);
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                fold_direct_const_def_call_expr(
                    index,
                    artifacts,
                    options,
                    "graph endpoint index",
                    errors,
                );
            }
        }
    }
}

fn fold_direct_const_def_calls_in_block(
    block: &mut Block,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    structural_options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(default_ty) = &mut ports.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    "section default type",
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut ports.decls {
                fold_direct_const_def_port_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("port '{}'", decl.name),
                    errors,
                );
            }
        }
        Block::Params(params) => {
            if let Some(default_ty) = &mut params.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    "params default type",
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut params.decls {
                fold_direct_const_def_param_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("param '{}'", decl.name),
                    errors,
                );
            }
        }
        Block::Events(events) => {
            for event in &mut events.events {
                fold_direct_const_def_event(event, artifacts, options, errors);
            }
        }
        Block::Buffers(buffers) => {
            if let Some(default_ty) = &mut buffers.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_buffer_type(
                    &mut ty,
                    artifacts,
                    options,
                    "buffers default type",
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut buffers.decls {
                fold_direct_const_def_buffer_type(
                    &mut decl.ty,
                    artifacts,
                    options,
                    &format!("buffer '{}'", decl.name),
                    errors,
                );
            }
        }
        Block::Init(init) => {
            for stmt in &mut init.body {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
        }
        Block::Block(block_exec) => {
            for stmt in &mut block_exec.pre {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            if let Some(sample) = &mut block_exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    fold_direct_const_def_call_expr(
                        factor,
                        artifacts,
                        options,
                        "sample oversample factor",
                        errors,
                    );
                }
                for stmt in &mut sample.body {
                    fold_direct_const_def_stmt(stmt, artifacts, options, errors);
                }
            }
            for stmt in &mut block_exec.post {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                fold_direct_const_def_call_expr(
                    factor,
                    artifacts,
                    options,
                    "sample oversample factor",
                    errors,
                );
            }
            for stmt in &mut sample.body {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
        }
        Block::Graph(graph) => {
            fold_direct_const_def_graph(graph, artifacts, options, errors);
        }
        Block::Assert(assert_decl) => {
            fold_direct_const_def_call_expr(
                &mut assert_decl.expr,
                artifacts,
                options,
                "assert condition",
                errors,
            );
        }
        Block::Def(def) if !def.is_const => {
            fold_direct_const_def_function(def, artifacts, options, errors);
        }
        Block::Struct(struct_def) => {
            for field in &mut struct_def.fields {
                fold_direct_const_def_field_type(
                    &mut field.ty,
                    artifacts,
                    options,
                    &format!("struct '{}' field '{}'", struct_def.name, field.name),
                    errors,
                );
                if let Some(default) = &mut field.default {
                    fold_direct_const_def_call_expr(
                        default,
                        artifacts,
                        options,
                        &format!("struct '{}' field '{}'", struct_def.name, field.name),
                        errors,
                    );
                }
            }
            for method in &mut struct_def.methods {
                fold_direct_const_def_function(method, artifacts, options, errors);
            }
        }
        Block::Proc(proc) => {
            if let Some(default_ty) = &mut proc.ins_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' input default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(default_ty) = &mut proc.outs_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' output default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(default_ty) = &mut proc.params_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' param default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(default_ty) = &mut proc.buffers_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_buffer_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' buffer default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut proc.ins {
                fold_direct_const_def_port_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("processor '{}' input '{}'", proc.name, decl.name),
                    errors,
                );
            }
            for decl in &mut proc.outs {
                fold_direct_const_def_port_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("processor '{}' output '{}'", proc.name, decl.name),
                    errors,
                );
            }
            for decl in &mut proc.params {
                fold_direct_const_def_param_decl(
                    decl,
                    artifacts,
                    options,
                    &format!("processor '{}' param '{}'", proc.name, decl.name),
                    errors,
                );
            }
            for decl in &mut proc.buffers {
                fold_direct_const_def_buffer_type(
                    &mut decl.ty,
                    artifacts,
                    options,
                    &format!("processor '{}' buffer '{}'", proc.name, decl.name),
                    errors,
                );
            }
            if let Some(default_ty) = &mut proc.init.default_ty {
                let mut ty = Some(default_ty.clone());
                fold_direct_const_def_decl_type(
                    &mut ty,
                    artifacts,
                    options,
                    &format!("processor '{}' init default type", proc.name),
                    errors,
                );
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(factor) = &mut proc.sample_oversample_factor {
                fold_direct_const_def_call_expr(
                    factor,
                    artifacts,
                    structural_options,
                    &format!("processor '{}' sample oversample factor", proc.name),
                    errors,
                );
            }
            for event in &mut proc.events {
                fold_direct_const_def_event(event, artifacts, options, errors);
            }
            for stmt in &mut proc.init.body {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            for stmt in &mut proc.block_pre {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            for stmt in &mut proc.sample {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            for stmt in &mut proc.block_post {
                fold_direct_const_def_stmt(stmt, artifacts, options, errors);
            }
            if let Some(graph) = &mut proc.graph {
                fold_direct_const_def_graph(graph, artifacts, options, errors);
            }
            for def in &mut proc.local_defs {
                fold_direct_const_def_function(def, artifacts, options, errors);
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

fn fold_local_scalar_const_expr(expr: &mut Expr, local_consts: &HashMap<String, TypedConstValue>) {
    let loc = expr.loc();
    if let Expr::Var { name, .. } = expr {
        if let Some(value) = local_consts.get(name).copied() {
            *expr = typed_const_expr_with_loc(value, loc);
            return;
        }
    }

    match expr {
        Expr::Index { index, .. } => {
            fold_local_scalar_const_expr(index, local_consts);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                fold_local_scalar_const_expr(start, local_consts);
            }
            if let Some(end) = end {
                fold_local_scalar_const_expr(end, local_consts);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            fold_local_scalar_const_expr(&mut spec.size, local_consts);
            if let Some(init) = init {
                for value in init {
                    fold_local_scalar_const_expr(value, local_consts);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            fold_local_scalar_const_expr(lhs, local_consts);
            fold_local_scalar_const_expr(rhs, local_consts);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                fold_local_scalar_const_expr(arg, local_consts);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                fold_local_scalar_const_expr(&mut arg.expr, local_consts);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            fold_local_scalar_const_expr(expr, local_consts);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                fold_local_scalar_const_expr(value, local_consts);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn fold_local_scalar_const_decl_type(
    ty: &mut Option<DeclType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(DeclType::Array { size, .. }) | Some(DeclType::ArrayGeneric { size, .. }) => {
            fold_local_scalar_const_expr(size, local_consts);
        }
        Some(DeclType::Scalar(_) | DeclType::Generic(_) | DeclType::Tuple(_)) | None => {}
    }
}

fn fold_local_scalar_const_buffer_type(
    ty: &mut Option<BufferType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(BufferType {
        channels: BufferChannels::Static(expr),
        ..
    }) = ty
    {
        fold_local_scalar_const_expr(expr, local_consts);
    }
}

fn fold_local_scalar_const_fn_param_type(
    ty: &mut Option<FnParamType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(FnParamType::SizedArray { size, .. }) => {
            fold_local_scalar_const_expr(size, local_consts);
        }
        Some(FnParamType::Buffer(buffer_ty)) => {
            if let BufferChannels::Static(expr) = &mut buffer_ty.channels {
                fold_local_scalar_const_expr(expr, local_consts);
            }
        }
        Some(
            FnParamType::Primitive(_)
            | FnParamType::Struct(_)
            | FnParamType::Array(_)
            | FnParamType::ArrayGeneric(_)
            | FnParamType::BareBuffer
            | FnParamType::Tuple(_),
        )
        | None => {}
    }
}

fn fold_local_scalar_const_event_param_type(
    ty: &mut EventParamType,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        EventParamType::Array { size, .. } | EventParamType::GenericArray { size, .. } => {
            fold_local_scalar_const_expr(size, local_consts);
        }
        _ => {}
    }
}

fn fold_local_scalar_const_field_type(
    ty: &mut FieldType,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let FieldType::Array(spec) = ty {
        fold_local_scalar_const_expr(&mut spec.size, local_consts);
    }
}

fn fold_local_scalar_const_return_type(
    ty: &mut Option<FnReturnType>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    match ty {
        Some(FnReturnType::Array { size, .. }) => {
            fold_local_scalar_const_expr(size, local_consts);
        }
        Some(FnReturnType::Scalar(_) | FnReturnType::Tuple(_)) | None => {}
    }
}

fn fold_local_scalar_const_decl_range(
    range: &mut Option<DeclRange>,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            fold_local_scalar_const_expr(min, local_consts);
        }
        fold_local_scalar_const_expr(&mut range.max, local_consts);
    }
}

fn fold_local_scalar_const_port_decl(
    decl: &mut PortDecl,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    fold_local_scalar_const_decl_type(&mut decl.ty, local_consts);
    if let Some(default) = &mut decl.default {
        fold_local_scalar_const_expr(default, local_consts);
    }
    fold_local_scalar_const_decl_range(&mut decl.range, local_consts);
}

fn fold_local_scalar_const_param_decl(
    decl: &mut ParamDecl,
    local_consts: &HashMap<String, TypedConstValue>,
) {
    fold_local_scalar_const_decl_type(&mut decl.ty, local_consts);
    if let Some(default) = &mut decl.default {
        fold_local_scalar_const_expr(default, local_consts);
    }
    fold_local_scalar_const_decl_range(&mut decl.range, local_consts);
}

fn eval_local_scalar_const_decl(
    decl: &onda_frontend::ConstDecl,
    local_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context_prefix: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    if is_builtin_constant_name(&decl.name) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "constant name '{}' is reserved as a builtin constant",
                decl.name
            ),
            decl.loc.as_ref(),
        ));
        return None;
    }

    if is_const_array_decl(decl) {
        errors.push(Diagnostic::semantic_span(
            "const arrays are only supported at top-level and namespace scope",
            decl.loc.as_ref(),
        ));
        return None;
    }

    let expected_ty = match &decl.ty {
        Some(ConstType::Scalar(ty)) => Some(*ty),
        Some(ConstType::Array { .. } | ConstType::Slice { .. }) => {
            errors.push(Diagnostic::semantic_span(
                "const arrays are only supported at top-level and namespace scope",
                decl.loc.as_ref(),
            ));
            return None;
        }
        None => None,
    };

    let local_arrays = HashMap::new();
    let context = format!("{context_prefix} local const '{}'", decl.name);
    let ty = match expected_ty {
        Some(ty) => ty,
        None => infer_const_decl_scalar_type_with_defs(
            &decl.expr,
            local_consts,
            &local_arrays,
            &artifacts.const_values,
            const_def_registry(artifacts),
            options,
            &context,
            &mut Vec::new(),
            errors,
        )?,
    };
    eval_const_scalar_expr_with_defs(
        &decl.expr,
        ty,
        local_consts,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        &context,
        &mut Vec::new(),
        errors,
    )
}

fn proc_sample_oversample_factor_for_proc_context(
    proc: &ProcessorDef,
    proc_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
) -> usize {
    let Some(factor) = proc.sample_oversample_factor.as_ref() else {
        return 1;
    };
    let local_arrays = HashMap::new();
    let mut scratch = Vec::<Diagnostic>::new();
    let Some(folded) = fold_const_eval_expr(
        factor,
        proc_consts,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        &format!("processor '{}' sample oversample factor", proc.name),
        &mut Vec::new(),
        &mut scratch,
    ) else {
        return 1;
    };
    validated_sample_oversample_factor(
        Some(&folded),
        options,
        &format!("processor '{}' sample oversample factor", proc.name),
        &mut scratch,
    )
}

fn preprocess_local_const_stmt(
    stmt: &mut Stmt,
    local_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target_loc,
            target,
            expr,
            ..
        } => {
            match target {
                AssignTarget::Var(name) => {
                    if local_consts.contains_key(name) {
                        errors.push(Diagnostic::semantic_span(
                            format!("cannot assign to constant '{name}'"),
                            target_loc.as_ref(),
                        ));
                    }
                }
                AssignTarget::Index { index, .. } => {
                    fold_local_scalar_const_expr(index, local_consts);
                }
                AssignTarget::Slice { start, end, .. } => {
                    if let Some(start) = start {
                        fold_local_scalar_const_expr(start, local_consts);
                    }
                    if let Some(end) = end {
                        fold_local_scalar_const_expr(end, local_consts);
                    }
                }
                AssignTarget::Tuple(names) => {
                    for name in names {
                        if local_consts.contains_key(name) {
                            errors.push(Diagnostic::semantic_span(
                                format!("cannot assign to constant '{name}'"),
                                target_loc.as_ref(),
                            ));
                        }
                    }
                }
            }
            fold_local_scalar_const_expr(expr, local_consts);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            fold_local_scalar_const_expr(expr, local_consts);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            fold_local_scalar_const_expr(cond, local_consts);
            preprocess_local_const_stmts(
                then_branch,
                local_consts,
                artifacts,
                options,
                "if branch",
                errors,
            );
            preprocess_local_const_stmts(
                else_branch,
                local_consts,
                artifacts,
                options,
                "else branch",
                errors,
            );
        }
        Stmt::For {
            loc,
            var,
            step,
            start,
            end,
            body,
            ..
        } => {
            if local_consts.contains_key(var) {
                errors.push(Diagnostic::semantic_span(
                    format!("loop variable '{var}' conflicts with local constant '{var}'"),
                    loc.as_ref(),
                ));
            }
            if let Some(step) = step {
                fold_local_scalar_const_expr(step, local_consts);
            }
            fold_local_scalar_const_expr(start, local_consts);
            fold_local_scalar_const_expr(end, local_consts);
            preprocess_local_const_stmts(
                body,
                local_consts,
                artifacts,
                options,
                "for loop",
                errors,
            );
        }
        Stmt::While { cond, body, .. } => {
            fold_local_scalar_const_expr(cond, local_consts);
            preprocess_local_const_stmts(
                body,
                local_consts,
                artifacts,
                options,
                "while loop",
                errors,
            );
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn preprocess_local_const_stmts(
    stmts: &mut Vec<Stmt>,
    inherited_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context_prefix: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let mut scope_consts = inherited_consts.clone();
    let mut local_names = HashSet::<String>::new();
    let mut rewritten = Vec::<Stmt>::with_capacity(stmts.len());
    for mut stmt in std::mem::take(stmts) {
        if let Stmt::Const { decl, .. } = &stmt {
            if !local_names.insert(decl.name.clone()) {
                errors.push(Diagnostic::semantic_span(
                    format!("duplicate constant '{}' in scope", decl.name),
                    decl.loc.as_ref(),
                ));
                continue;
            }
            if let Some(value) = eval_local_scalar_const_decl(
                decl,
                &scope_consts,
                artifacts,
                options,
                context_prefix,
                errors,
            ) {
                scope_consts.insert(decl.name.clone(), value);
            }
            continue;
        }
        preprocess_local_const_stmt(&mut stmt, &scope_consts, artifacts, options, errors);
        rewritten.push(stmt);
    }
    *stmts = rewritten;
}

fn preprocess_local_const_function(
    def: &mut FunctionDef,
    inherited_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        if inherited_consts.contains_key(&param.name) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "function parameter '{}' in '{}' conflicts with local constant '{}'",
                    param.name, def.name, param.name
                ),
                param.loc.as_ref(),
            ));
        }
        fold_local_scalar_const_fn_param_type(&mut param.ty, inherited_consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, inherited_consts);
        }
    }
    fold_local_scalar_const_return_type(&mut def.return_ty, inherited_consts);
    preprocess_local_const_stmts(
        &mut def.body,
        inherited_consts,
        artifacts,
        options,
        &format!("function '{}'", def.name),
        errors,
    );
}

fn preprocess_local_const_event(
    event: &mut EventDef,
    inherited_consts: &HashMap<String, TypedConstValue>,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut event.params {
        if inherited_consts.contains_key(&param.name) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "event parameter '{}' in '{}' conflicts with local constant '{}'",
                    param.name, event.name, param.name
                ),
                param.loc.as_ref(),
            ));
        }
        fold_local_scalar_const_event_param_type(&mut param.ty, inherited_consts);
        if let Some(default) = &mut param.default {
            fold_local_scalar_const_expr(default, inherited_consts);
        }
    }
    preprocess_local_const_stmts(
        &mut event.body,
        inherited_consts,
        artifacts,
        options,
        &format!("event '{}'", event.name),
        errors,
    );
}

fn preprocess_local_const_graph(
    graph: &mut GraphBlock,
    inherited_consts: &HashMap<String, TypedConstValue>,
) {
    for edge in &mut graph.edges {
        fold_local_scalar_const_expr(&mut edge.source, inherited_consts);
        if let Some(delay) = &mut edge.delay {
            fold_local_scalar_const_expr(delay, inherited_consts);
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                fold_local_scalar_const_expr(index, inherited_consts);
            }
        }
    }
}

fn reject_proc_local_const_decl_name(
    proc_name: &str,
    symbol_kind: &str,
    name: &str,
    loc: Span,
    proc_consts: &HashMap<String, TypedConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    if proc_consts.contains_key(name) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{symbol_kind} '{name}' in processor '{proc_name}' conflicts with local constant '{name}'"
            ),
            loc.as_ref(),
        ));
    }
}

fn reject_proc_local_const_decl_conflicts(
    proc: &ProcessorDef,
    proc_consts: &HashMap<String, TypedConstValue>,
    errors: &mut Vec<Diagnostic>,
) {
    for decl in &proc.ins {
        reject_proc_local_const_decl_name(
            &proc.name,
            "processor input",
            &decl.name,
            decl.loc,
            proc_consts,
            errors,
        );
    }
    for decl in &proc.outs {
        reject_proc_local_const_decl_name(
            &proc.name,
            "processor output",
            &decl.name,
            decl.loc,
            proc_consts,
            errors,
        );
    }
    for decl in &proc.params {
        reject_proc_local_const_decl_name(
            &proc.name,
            "processor parameter",
            &decl.name,
            decl.loc,
            proc_consts,
            errors,
        );
    }
    for decl in &proc.buffers {
        reject_proc_local_const_decl_name(
            &proc.name,
            "processor buffer",
            &decl.name,
            decl.loc,
            proc_consts,
            errors,
        );
    }
}

fn preprocess_proc_local_const_decls(
    proc_name: &str,
    consts: &[onda_frontend::ConstDecl],
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ProcLocalConstArtifacts {
    let mut out = ProcLocalConstArtifacts::default();
    let mut proc_const_names = HashSet::<String>::new();
    for decl in consts {
        if !proc_const_names.insert(decl.name.clone()) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate proc constant '{}'", decl.name),
                decl.loc.as_ref(),
            ));
            continue;
        }
        if let Some(value) = eval_local_scalar_const_decl(
            decl,
            &out.values,
            artifacts,
            options,
            &format!("processor '{proc_name}'"),
            errors,
        ) {
            out.values.insert(decl.name.clone(), value);
        }
    }
    out
}

fn preprocess_proc_local_consts(
    proc: &mut ProcessorDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> ProcLocalConstArtifacts {
    let out =
        preprocess_proc_local_const_decls(&proc.name, &proc.consts, artifacts, options, errors);
    proc.consts.clear();
    out
}

fn preprocess_local_consts_in_block(
    block: &mut Block,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let empty_consts = HashMap::<String, TypedConstValue>::new();
    match block {
        Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
            if let Some(default_ty) = &mut ports.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &empty_consts);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut ports.decls {
                fold_local_scalar_const_port_decl(decl, &empty_consts);
            }
        }
        Block::Params(params) => {
            if let Some(default_ty) = &mut params.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &empty_consts);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut params.decls {
                fold_local_scalar_const_param_decl(decl, &empty_consts);
            }
        }
        Block::Buffers(buffers) => {
            if let Some(default_ty) = &mut buffers.deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_buffer_type(&mut ty, &empty_consts);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut buffers.decls {
                fold_local_scalar_const_buffer_type(&mut decl.ty, &empty_consts);
            }
        }
        Block::Events(events) => {
            for event in &mut events.events {
                preprocess_local_const_event(event, &empty_consts, artifacts, options, errors);
            }
        }
        Block::Init(init) => {
            if let Some(default_ty) = &mut init.default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &empty_consts);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            preprocess_local_const_stmts(
                &mut init.body,
                &empty_consts,
                artifacts,
                options,
                "init block",
                errors,
            );
        }
        Block::Block(block_exec) => {
            preprocess_local_const_stmts(
                &mut block_exec.pre,
                &empty_consts,
                artifacts,
                options,
                "block pre",
                errors,
            );
            if let Some(sample) = &mut block_exec.sample {
                if let Some(factor) = &mut sample.oversample_factor {
                    fold_local_scalar_const_expr(factor, &empty_consts);
                }
                preprocess_local_const_stmts(
                    &mut sample.body,
                    &empty_consts,
                    artifacts,
                    options,
                    "sample block",
                    errors,
                );
            }
            preprocess_local_const_stmts(
                &mut block_exec.post,
                &empty_consts,
                artifacts,
                options,
                "block post",
                errors,
            );
        }
        Block::Sample(sample) => {
            if let Some(factor) = &mut sample.oversample_factor {
                fold_local_scalar_const_expr(factor, &empty_consts);
            }
            preprocess_local_const_stmts(
                &mut sample.body,
                &empty_consts,
                artifacts,
                options,
                "sample block",
                errors,
            );
        }
        Block::Graph(graph) => {
            preprocess_local_const_graph(graph, &empty_consts);
        }
        Block::Assert(assert_decl) => {
            fold_local_scalar_const_expr(&mut assert_decl.expr, &empty_consts);
        }
        Block::Def(def) if !def.is_const => {
            preprocess_local_const_function(def, &empty_consts, artifacts, options, errors);
        }
        Block::Struct(struct_def) => {
            for field in &mut struct_def.fields {
                fold_local_scalar_const_field_type(&mut field.ty, &empty_consts);
                if let Some(default) = &mut field.default {
                    fold_local_scalar_const_expr(default, &empty_consts);
                }
            }
            for method in &mut struct_def.methods {
                preprocess_local_const_function(method, &empty_consts, artifacts, options, errors);
            }
        }
        Block::Proc(proc) => {
            let factor_proc_consts = {
                let mut scratch_errors = Vec::new();
                preprocess_proc_local_const_decls(
                    &proc.name,
                    &proc.consts,
                    artifacts,
                    options,
                    &mut scratch_errors,
                )
            };
            let sample_oversample_factor = proc_sample_oversample_factor_for_proc_context(
                proc,
                &factor_proc_consts.values,
                artifacts,
                options,
            );
            let proc_options = proc_runtime_analysis_options(options, sample_oversample_factor);
            let proc_consts = preprocess_proc_local_consts(proc, artifacts, proc_options, errors);
            reject_proc_local_const_decl_conflicts(proc, &proc_consts.values, errors);
            if let Some(count) = &mut proc.ins_deferred_count {
                fold_local_scalar_const_expr(count, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.ins_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(count) = &mut proc.outs_deferred_count {
                fold_local_scalar_const_expr(count, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.outs_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(count) = &mut proc.params_deferred_count {
                fold_local_scalar_const_expr(count, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.params_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(count) = &mut proc.buffers_deferred_count {
                fold_local_scalar_const_expr(count, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.buffers_deferred_default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_buffer_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            for decl in &mut proc.ins {
                fold_local_scalar_const_port_decl(decl, &proc_consts.values);
            }
            for decl in &mut proc.outs {
                fold_local_scalar_const_port_decl(decl, &proc_consts.values);
            }
            for decl in &mut proc.params {
                fold_local_scalar_const_param_decl(decl, &proc_consts.values);
            }
            for decl in &mut proc.buffers {
                fold_local_scalar_const_buffer_type(&mut decl.ty, &proc_consts.values);
            }
            if let Some(default_ty) = &mut proc.init.default_ty {
                let mut ty = Some(default_ty.clone());
                fold_local_scalar_const_decl_type(&mut ty, &proc_consts.values);
                if let Some(folded) = ty {
                    *default_ty = folded;
                }
            }
            if let Some(factor) = &mut proc.sample_oversample_factor {
                fold_local_scalar_const_expr(factor, &factor_proc_consts.values);
            }
            for event in &mut proc.events {
                preprocess_local_const_event(
                    event,
                    &proc_consts.values,
                    artifacts,
                    proc_options,
                    errors,
                );
            }
            preprocess_local_const_stmts(
                &mut proc.init.body,
                &proc_consts.values,
                artifacts,
                proc_options,
                &format!("processor '{}' init", proc.name),
                errors,
            );
            preprocess_local_const_stmts(
                &mut proc.block_pre,
                &proc_consts.values,
                artifacts,
                proc_options,
                &format!("processor '{}' block pre", proc.name),
                errors,
            );
            preprocess_local_const_stmts(
                &mut proc.sample,
                &proc_consts.values,
                artifacts,
                proc_options,
                &format!("processor '{}' sample", proc.name),
                errors,
            );
            preprocess_local_const_stmts(
                &mut proc.block_post,
                &proc_consts.values,
                artifacts,
                proc_options,
                &format!("processor '{}' block post", proc.name),
                errors,
            );
            if let Some(graph) = &mut proc.graph {
                preprocess_local_const_graph(graph, &proc_consts.values);
            }
            for def in &mut proc.local_defs {
                preprocess_local_const_function(
                    def,
                    &proc_consts.values,
                    artifacts,
                    proc_options,
                    errors,
                );
            }
        }
        Block::Const(_)
        | Block::Def(_)
        | Block::Namespace(_)
        | Block::NamespaceAlias(_)
        | Block::Use(_) => {}
    }
}

fn eval_count_shorthand(
    expr: &Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    let folded = fold_const_eval_expr(
        expr,
        &locals,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        context,
        &mut Vec::new(),
        errors,
    )?;
    eval_data_size_expr(&folded, options, context, errors)
}

#[allow(clippy::too_many_arguments)]
fn expand_port_count_shorthand(
    decls: &mut Vec<PortDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    prefix: &str,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(PortDecl {
                loc,
                name: format!("{prefix}{idx}"),
                output_timing: None,
                output_timing_loc: Span::ZERO,
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_param_count_shorthand(
    decls: &mut Vec<ParamDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    prefix: &str,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(ParamDecl {
                loc,
                name: format!("{prefix}{idx}"),
                pinned: false,
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
                default: None,
                range: None,
                bind: None,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_buffer_count_shorthand(
    decls: &mut Vec<BufferDecl>,
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<BufferType>,
    block_label: &str,
    loc: Span,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(count_expr) = deferred_count.take() else {
        return;
    };
    let Some(count) = eval_count_shorthand(
        &count_expr,
        artifacts,
        options,
        &format!("{block_label} count expression"),
        errors,
    ) else {
        return;
    };
    let default_ty = deferred_default_ty.take();
    if decls.is_empty() {
        for idx in 1..=count {
            decls.push(BufferDecl {
                loc,
                name: format!("buf{idx}"),
                ty: default_ty.clone(),
                ty_loc: Span::ZERO,
            });
        }
    } else if decls.len() != count {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{block_label} block count ({count}) does not match explicit declaration count ({})",
                decls.len()
            ),
            loc.as_ref(),
        ));
    }
}

fn expand_proc_count_shorthand(
    proc: &mut ProcessorDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let loc = proc.loc;
    let proc_name = proc.name.clone();
    expand_port_count_shorthand(
        &mut proc.ins,
        &mut proc.ins_deferred_count,
        &mut proc.ins_deferred_default_ty,
        "in",
        &format!("processor '{proc_name}' ins"),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_port_count_shorthand(
        &mut proc.outs,
        &mut proc.outs_deferred_count,
        &mut proc.outs_deferred_default_ty,
        match proc.outs_timing {
            OutputTiming::Sample => "out",
            OutputTiming::Block => "kout",
        },
        &format!(
            "processor '{proc_name}' {}",
            match proc.outs_timing {
                OutputTiming::Sample => "outs",
                OutputTiming::Block => "kouts",
            }
        ),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_param_count_shorthand(
        &mut proc.params,
        &mut proc.params_deferred_count,
        &mut proc.params_deferred_default_ty,
        "param",
        &format!("processor '{proc_name}' params"),
        loc,
        artifacts,
        options,
        errors,
    );
    expand_buffer_count_shorthand(
        &mut proc.buffers,
        &mut proc.buffers_deferred_count,
        &mut proc.buffers_deferred_default_ty,
        &format!("processor '{proc_name}' buffers"),
        loc,
        artifacts,
        options,
        errors,
    );
}

fn proc_options_for_count_expansion(
    proc: &ProcessorDef,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
) -> AnalysisOptions {
    let empty_proc_consts = HashMap::new();
    let sample_oversample_factor = proc_sample_oversample_factor_for_proc_context(
        proc,
        &empty_proc_consts,
        artifacts,
        options,
    );
    proc_runtime_analysis_options(options, sample_oversample_factor)
}

fn coerce_consts_and_expand_counts(
    program: &mut Program,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> SemanticConstArtifacts {
    let mut artifacts = SemanticConstArtifacts::default();
    let mut seen = HashSet::<String>::new();
    let ordinary_symbols = ordinary_top_level_symbol_names(program);
    let mut future_const_symbols = top_level_const_symbol_names(program);
    for name in &ordinary_symbols {
        future_const_symbols.remove(name);
    }
    for block in &mut program.blocks {
        if let Block::Const(decl) = block {
            future_const_symbols.remove(&decl.name);
        }
        let preprocess_local_consts = match block {
            Block::Const(_) => false,
            Block::Def(def) if def.is_const => false,
            _ => true,
        };
        if preprocess_local_consts {
            preprocess_local_consts_in_block(block, &artifacts, options, errors);
        }
        match block {
            Block::Def(def) if def.is_const => {
                if is_builtin_constant_name(&def.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def name '{}' is reserved as a builtin constant",
                            def.name
                        ),
                        def.loc,
                    ));
                    continue;
                }
                if ordinary_symbols.contains(&def.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "const def name '{}' conflicts with existing symbol",
                            def.name
                        ),
                        def.loc,
                    ));
                    continue;
                }
                if !seen.insert(def.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate const symbol '{}'", def.name),
                        def.loc,
                    ));
                    continue;
                }
                artifacts
                    .const_def_order
                    .insert(def.name.clone(), artifacts.const_def_order.len());
                artifacts.const_defs.insert(def.name.clone(), def.clone());
                validate_const_def_declaration(def, options, &artifacts, errors);
            }
            Block::Const(decl) => {
                if is_builtin_constant_name(&decl.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "constant name '{}' is reserved as a builtin constant",
                            decl.name
                        ),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                if ordinary_symbols.contains(&decl.name) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "constant name '{}' conflicts with existing symbol",
                            decl.name
                        ),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                if !seen.insert(decl.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate const symbol '{}'", decl.name),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                let force_const_array = is_const_array_decl(decl)
                    || (decl.ty.is_none()
                        && is_known_const_array_initializer(
                            &decl.expr,
                            &artifacts.const_values,
                            &artifacts.const_defs,
                        ));
                if force_const_array {
                    if let Some(array) = coerce_const_array(
                        decl,
                        options,
                        &artifacts.const_values,
                        &artifacts.const_defs,
                        &artifacts.const_def_order,
                        errors,
                    ) {
                        record_const_array_artifact(&mut artifacts, array);
                    }
                } else {
                    let inferred_const_array = if decl.ty.is_none() {
                        let mut probe_errors = Vec::new();
                        coerce_const_array(
                            decl,
                            options,
                            &artifacts.const_values,
                            &artifacts.const_defs,
                            &artifacts.const_def_order,
                            &mut probe_errors,
                        )
                    } else {
                        None
                    };
                    if let Some(array) = inferred_const_array {
                        record_const_array_artifact(&mut artifacts, array);
                    } else if let Some(value) = coerce_const_scalar(
                        decl,
                        options,
                        &artifacts.const_values,
                        &artifacts.const_defs,
                        &artifacts.const_def_order,
                        errors,
                    ) {
                        artifacts
                            .const_values
                            .insert(decl.name.clone(), ConstValue::Scalar(value));
                    }
                }
            }
            Block::Ins(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "ins",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::Outs(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "outs",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::KOuts(ports) => {
                let prefix = ports.deferred_prefix.clone();
                expand_port_count_shorthand(
                    &mut ports.decls,
                    &mut ports.deferred_count,
                    &mut ports.deferred_default_ty,
                    &prefix,
                    "kouts",
                    ports.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::Params(params) => {
                let prefix = params.deferred_prefix.clone();
                let block_label = if prefix == "kin" { "kins" } else { "params" };
                expand_param_count_shorthand(
                    &mut params.decls,
                    &mut params.deferred_count,
                    &mut params.deferred_default_ty,
                    &prefix,
                    block_label,
                    params.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::Buffers(buffers) => {
                expand_buffer_count_shorthand(
                    &mut buffers.decls,
                    &mut buffers.deferred_count,
                    &mut buffers.deferred_default_ty,
                    "buffers",
                    buffers.loc,
                    &artifacts,
                    options,
                    errors,
                );
                fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors);
            }
            Block::Proc(proc) => {
                let proc_options = proc_options_for_count_expansion(proc, &artifacts, options);
                expand_proc_count_shorthand(proc, &artifacts, proc_options, errors);
                fold_direct_const_def_calls_in_block(
                    block,
                    &artifacts,
                    proc_options,
                    options,
                    errors,
                );
            }
            _ => fold_direct_const_def_calls_in_block(block, &artifacts, options, options, errors),
        }
        let const_array_options = match block {
            Block::Proc(proc) => proc_options_for_count_expansion(proc, &artifacts, options),
            _ => options,
        };
        fold_const_array_exprs_in_block(
            block,
            &artifacts.const_values,
            const_array_options,
            options,
            errors,
        );
        reject_forward_const_refs_in_block(
            block,
            &artifacts.const_values,
            &future_const_symbols,
            errors,
        );
    }
    artifacts
}

fn coerce_const_array(
    decl: &onda_frontend::ConstDecl,
    options: AnalysisOptions,
    const_values: &HashMap<String, ConstValue>,
    const_defs: &HashMap<String, FunctionDef>,
    const_def_order: &HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstArray> {
    let (decl_elem_ty, decl_len) = match &decl.ty {
        Some(ConstType::Array { elem, size }) => {
            let context = format!("const array '{}' size", decl.name);
            let locals = HashMap::new();
            let local_arrays = HashMap::new();
            let const_defs = ConstDefRegistry {
                defs: const_defs,
                order: const_def_order,
            };
            let len = eval_const_array_size_with_defs(
                size,
                &locals,
                &local_arrays,
                const_values,
                const_defs,
                options,
                &context,
                &mut Vec::new(),
                errors,
            )?;
            (Some(*elem), Some(len))
        }
        Some(ConstType::Slice { elem }) => (Some(*elem), None),
        Some(ConstType::Scalar(_)) => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const array '{}' cannot use a scalar type annotation",
                    decl.name
                ),
                decl.loc.as_ref(),
            ));
            (None, None)
        }
        None => (None, None),
    };

    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    let const_defs = ConstDefRegistry {
        defs: const_defs,
        order: const_def_order,
    };
    let expected = match (decl_elem_ty, decl_len) {
        (Some(elem_ty), Some(len)) => ConstArrayExpectation::fixed(elem_ty, len),
        (Some(elem_ty), None) => ConstArrayExpectation::elem(elem_ty),
        (None, Some(len)) => ConstArrayExpectation {
            elem_ty: None,
            len: Some(len),
        },
        (None, None) => ConstArrayExpectation::any(),
    };
    let context = format!("const array '{}'", decl.name);
    let array = eval_const_array_expr_with_defs(
        &decl.expr,
        expected,
        &locals,
        &local_arrays,
        const_values,
        const_defs,
        options,
        &context,
        &mut Vec::new(),
        errors,
    )?;

    Some(TypedConstArray {
        name: decl.name.clone(),
        elem_ty: array.elem_ty,
        len: array.len(),
        values: array.values,
    })
}

fn coerce_const_scalar(
    decl: &onda_frontend::ConstDecl,
    options: AnalysisOptions,
    const_values: &HashMap<String, ConstValue>,
    const_defs: &HashMap<String, FunctionDef>,
    const_def_order: &HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    let expected_ty = match &decl.ty {
        Some(ConstType::Scalar(ty)) => Some(*ty),
        Some(ConstType::Array { .. } | ConstType::Slice { .. }) => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const scalar '{}' cannot use an array type annotation",
                    decl.name
                ),
                decl.loc.as_ref(),
            ));
            None
        }
        None => None,
    };

    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    let const_defs = ConstDefRegistry {
        defs: const_defs,
        order: const_def_order,
    };
    let context = format!("const scalar '{}'", decl.name);
    let ty = match expected_ty {
        Some(ty) => ty,
        None => infer_const_decl_scalar_type_with_defs(
            &decl.expr,
            &locals,
            &local_arrays,
            const_values,
            const_defs,
            options,
            &context,
            &mut Vec::new(),
            errors,
        )?,
    };
    eval_const_scalar_expr_with_defs(
        &decl.expr,
        ty,
        &locals,
        &local_arrays,
        const_values,
        const_defs,
        options,
        &context,
        &mut Vec::new(),
        errors,
    )
}

fn collect_struct_field_dependencies(def: &StructDef) -> Vec<String> {
    let mut deps = Vec::<String>::new();
    for field in &def.fields {
        match &field.ty {
            FieldType::Generic(name) => deps.push(name.clone()),
            FieldType::Array(spec) => {
                if let ArrayElemType::Struct(name) = &spec.elem {
                    deps.push(name.clone());
                }
            }
            FieldType::Scalar(_) | FieldType::Tuple(_) => {}
        }
    }
    deps
}

fn order_struct_defs_for_field_dependencies(structs: &mut Vec<StructDef>) {
    let index_by_name = structs
        .iter()
        .enumerate()
        .map(|(idx, def)| (def.name.clone(), idx))
        .collect::<HashMap<_, _>>();
    if index_by_name.is_empty() {
        return;
    }

    let deps_by_index = structs
        .iter()
        .map(collect_struct_field_dependencies)
        .collect::<Vec<_>>();
    let mut remaining = (0..structs.len()).collect::<Vec<_>>();
    let mut ordered = Vec::<StructDef>::with_capacity(structs.len());

    while !remaining.is_empty() {
        let remaining_names = remaining
            .iter()
            .map(|idx| structs[*idx].name.as_str())
            .collect::<HashSet<_>>();
        let mut ready_pos = None;
        for (pos, idx) in remaining.iter().enumerate() {
            let self_name = structs[*idx].name.as_str();
            let ready = deps_by_index[*idx].iter().all(|dep| {
                dep == self_name
                    || !index_by_name.contains_key(dep)
                    || !remaining_names.contains(dep.as_str())
            });
            if ready {
                ready_pos = Some(pos);
                break;
            }
        }

        let Some(pos) = ready_pos else {
            ordered.extend(remaining.drain(..).map(|idx| structs[idx].clone()));
            break;
        };
        let idx = remaining.remove(pos);
        ordered.push(structs[idx].clone());
    }

    *structs = ordered;
}

pub fn analyze(program: Program) -> Result<TypedProgram, Vec<Diagnostic>> {
    analyze_with_options(program, AnalysisOptions::default())
}

#[cfg(test)]
pub(crate) fn preprocess_const_semantics_for_lowering(
    program: Program,
    options: AnalysisOptions,
) -> Result<Program, Vec<Diagnostic>> {
    let mut program = preprocess_program_for_analysis(program, options)?;

    let mut errors = Vec::new();
    let const_artifacts = coerce_consts_and_expand_counts(&mut program, options, &mut errors);
    reject_const_shadowing_in_program(&program, &const_artifacts.const_values, &mut errors);
    reject_const_assignments_in_program(&program, &const_artifacts.const_values, &mut errors);
    evaluate_asserts(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    program
        .blocks
        .retain(|block| !matches!(block, Block::Def(def) if def.is_const));
    Ok(program)
}

pub fn lower_graphs_for_inspection_with_options(
    program: Program,
    options: AnalysisOptions,
) -> Result<Program, Vec<Diagnostic>> {
    let mut program = preprocess_program_for_analysis(program, options)?;

    let mut errors = Vec::new();
    let const_artifacts = coerce_consts_and_expand_counts(&mut program, options, &mut errors);
    reject_const_shadowing_in_program(&program, &const_artifacts.const_values, &mut errors);
    reject_const_assignments_in_program(&program, &const_artifacts.const_values, &mut errors);
    evaluate_asserts(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    program
        .blocks
        .retain(|block| !matches!(block, Block::Def(def) if def.is_const));
    prepare_processors_for_graph_inspection(&mut program, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    lower_graph_blocks(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(program)
}

pub fn analyze_with_options(
    program: Program,
    options: AnalysisOptions,
) -> Result<TypedProgram, Vec<Diagnostic>> {
    let original_last_block_loc = program
        .blocks
        .iter()
        .rev()
        .map(Block::loc)
        .find(|loc| !loc.is_zero());
    let mut program = preprocess_program_for_analysis(program, options)?;

    let mut errors = Vec::new();
    let const_artifacts = coerce_consts_and_expand_counts(&mut program, options, &mut errors);
    let const_array_infos = const_array_info_map(&const_artifacts.const_arrays);
    let mut const_scalar_names = const_artifacts
        .const_values
        .iter()
        .filter_map(|(name, value)| match value {
            ConstValue::Scalar(_) => Some(name.clone()),
            ConstValue::Array { .. } => None,
        })
        .collect::<Vec<_>>();
    const_scalar_names.sort();
    let mut const_def_names = const_artifacts
        .const_defs
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    const_def_names.sort();
    reject_const_shadowing_in_program(&program, &const_artifacts.const_values, &mut errors);
    reject_const_assignments_in_program(&program, &const_artifacts.const_values, &mut errors);
    evaluate_asserts(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    program
        .blocks
        .retain(|block| !matches!(block, Block::Def(def) if def.is_const));
    let const_arrays = const_artifacts.const_arrays;
    let ProcessorDesugarResult {
        program,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
        proc_instance_oversample_factors,
        proc_api,
        lowering_shapes,
        top_level_proc_rewrite,
    } = desugar_processors(program, options, &const_array_infos, &mut errors);

    let mut seen_singleton = HashSet::new();
    for block in &program.blocks {
        let kind = block.kind();
        if matches!(
            kind,
            BlockKind::Def | BlockKind::Struct | BlockKind::Proc | BlockKind::Const
        ) {
            continue;
        }
        if !seen_singleton.insert(kind) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate block '{:?}'", kind).to_lowercase(),
                block.loc(),
            ));
        }
    }

    let ins_explicit = program.block(BlockKind::Ins).is_some();
    let raw_ins = match program.block(BlockKind::Ins) {
        Some(Block::Ins(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let audio_outs_explicit = program.block(BlockKind::Outs).is_some();
    let raw_outs = match program.block(BlockKind::Outs) {
        Some(Block::Outs(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let control_outs_explicit = program.block(BlockKind::KOuts).is_some();
    let raw_kouts = match program.block(BlockKind::KOuts) {
        Some(Block::KOuts(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let outs_explicit = audio_outs_explicit || control_outs_explicit;
    let params_block = program
        .block(BlockKind::Params)
        .and_then(|block| match block {
            Block::Params(v) => Some(v),
            _ => None,
        });
    let params_explicit = params_block.is_some();
    let raw_params = params_block.map(|v| v.decls.clone()).unwrap_or_default();
    let explicit_param_prefix = params_block.map(|v| v.deferred_prefix.as_str());
    let params_block_is_kins = matches!(explicit_param_prefix, Some("kin"));
    let mut events = match program.block(BlockKind::Events) {
        Some(Block::Events(v)) => v.events.clone(),
        _ => Vec::new(),
    };
    let buffers = match program.block(BlockKind::Buffers) {
        Some(Block::Buffers(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let mut struct_defs_raw = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(def) => Some(def.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();

    for def in &mut defs {
        if !def.type_params.is_empty() {
            let mut seen = HashSet::new();
            for tp in &def.type_params {
                if !seen.insert(tp.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate type parameter '{}' in def '{}'", tp, def.name),
                        def.loc,
                    ));
                }
            }
        }
    }

    let (init_default_decl_ty, mut init) = match program.block(BlockKind::Init) {
        Some(Block::Init(v)) => (v.default_ty.clone(), v.body.clone()),
        _ => (None, Vec::new()),
    };
    let mut block_pre = Vec::new();
    let mut block_post = Vec::new();
    let mut nested_block_sample = None;
    if let Some(Block::Block(exec)) = program.block(BlockKind::Block) {
        block_pre = exec.pre.clone();
        block_post = exec.post.clone();
        nested_block_sample = exec.sample.clone();
    }
    let block_section_without_sample =
        program.block(BlockKind::Block).is_some() && nested_block_sample.is_none();
    let top_sample = match program.block(BlockKind::Sample) {
        Some(Block::Sample(v)) => Some(v.clone()),
        _ => None,
    };
    let has_top_sample_block = top_sample.is_some();
    let sample_conflict_loc = top_sample
        .as_ref()
        .and_then(|sample| sample.loc.cloned())
        .or_else(|| {
            nested_block_sample
                .as_ref()
                .and_then(|sample| sample.loc.cloned())
        });

    let sample_block = match (nested_block_sample, top_sample) {
        (Some(_), Some(_)) => {
            errors.push(Diagnostic::semantic_span(
                "sample block cannot be declared both at top-level and inside block",
                sample_conflict_loc.as_ref(),
            ));
            SampleBlock {
                loc: Default::default(),
                oversample_factor: None,
                body: Vec::new(),
            }
        }
        (Some(v), None) => v,
        (None, Some(v)) => v,
        (None, None) => SampleBlock {
            loc: Default::default(),
            oversample_factor: None,
            body: Vec::new(),
        },
    };
    let sample_oversample_factor = validated_sample_oversample_factor(
        sample_block.oversample_factor.as_ref(),
        options,
        "sample block",
        &mut errors,
    );
    let mut sample = sample_block.body;

    let missing_sample_loc = original_last_block_loc.as_ref();
    if sample.is_empty()
        && program.block(BlockKind::Block).is_none()
        && requires_entry_sample(&program)
    {
        let missing_entry_message = if control_outs_explicit && !audio_outs_explicit {
            "missing required 'block' section"
        } else {
            "missing required 'sample' block"
        };
        errors.push(Diagnostic::semantic_span(
            missing_entry_message,
            missing_sample_loc,
        ));
    }

    {
        let struct_symbols = struct_defs_raw
            .iter()
            .map(|s| s.name.clone())
            .collect::<HashSet<_>>();
        let mut callable_symbols = defs.iter().map(|d| d.name.clone()).collect::<HashSet<_>>();
        for p in program.blocks.iter().filter_map(|b| match b {
            Block::Proc(proc_def) => Some(proc_def),
            _ => None,
        }) {
            callable_symbols.insert(p.name.clone());
        }
        for s in &struct_defs_raw {
            callable_symbols.insert(s.name.clone());
            for method in &s.methods {
                callable_symbols.insert(format!("{}.{}", s.name, method.name));
            }
        }
        let struct_namespaces = collect_declared_namespaces(&struct_symbols);
        let callable_namespaces = collect_declared_namespaces(&callable_symbols);
        let nominal_symbols = struct_symbols
            .union(&callable_symbols)
            .cloned()
            .collect::<HashSet<_>>();
        let nominal_namespaces = collect_declared_namespaces(&nominal_symbols);

        for s in &mut struct_defs_raw {
            let struct_ns = namespace_of_symbol(&s.name);
            for field in &mut s.fields {
                if let FieldType::Array(spec) = &mut field.ty {
                    if let ArrayElemType::Struct(name) = &mut spec.elem {
                        qualify_struct_type_name(
                            name,
                            &struct_ns,
                            &struct_symbols,
                            &struct_namespaces,
                            &format!("struct '{}.{}' array element type", s.name, field.name),
                            DiagCtx::new(field.ty_loc),
                            &mut errors,
                        );
                    }
                }
                if let Some(default) = &mut field.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &nominal_symbols,
                        &nominal_namespaces,
                        &mut errors,
                        &format!("struct '{}.{}' default", s.name, field.name),
                        None,
                    );
                }
            }
            for method in &mut s.methods {
                for param in &mut method.params {
                    if let Some(FnParamType::Struct(name)) = &mut param.ty {
                        qualify_struct_type_name(
                            name,
                            &struct_ns,
                            &struct_symbols,
                            &struct_namespaces,
                            &format!(
                                "method '{}.{}' parameter '{}'",
                                s.name, method.name, param.name
                            ),
                            DiagCtx::new(param.ty_loc),
                            &mut errors,
                        );
                    }
                    if let Some(default) = &mut param.default {
                        qualify_expr_namespaced_symbols(
                            default,
                            &struct_ns,
                            &callable_symbols,
                            &callable_namespaces,
                            &nominal_symbols,
                            &nominal_namespaces,
                            &mut errors,
                            &format!(
                                "method '{}.{}' parameter '{}' default",
                                s.name, method.name, param.name
                            ),
                            None,
                        );
                    }
                }
                for stmt in &mut method.body {
                    qualify_stmt_namespaced_symbols(
                        stmt,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &nominal_symbols,
                        &nominal_namespaces,
                        &mut errors,
                        &format!("method '{}.{}' body", s.name, method.name),
                    );
                }
            }
        }

        for def in &mut defs {
            let def_ns = namespace_of_symbol(&def.name);
            for param in &mut def.params {
                if let Some(FnParamType::Struct(name)) = &mut param.ty {
                    qualify_struct_type_name(
                        name,
                        &def_ns,
                        &struct_symbols,
                        &struct_namespaces,
                        &format!("function '{}' parameter '{}'", def.name, param.name),
                        DiagCtx::new(param.ty_loc),
                        &mut errors,
                    );
                }
                if let Some(default) = &mut param.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &def_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &nominal_symbols,
                        &nominal_namespaces,
                        &mut errors,
                        &format!("function '{}' parameter '{}' default", def.name, param.name),
                        None,
                    );
                }
            }
            for stmt in &mut def.body {
                qualify_stmt_namespaced_symbols(
                    stmt,
                    &def_ns,
                    &callable_symbols,
                    &callable_namespaces,
                    &nominal_symbols,
                    &nominal_namespaces,
                    &mut errors,
                    &format!("function '{}' body", def.name),
                );
            }
        }

        for stmt in &mut init {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &nominal_symbols,
                &nominal_namespaces,
                &mut errors,
                "init",
            );
        }
        for stmt in &mut block_pre {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &nominal_symbols,
                &nominal_namespaces,
                &mut errors,
                "block pre",
            );
        }
        for stmt in &mut block_post {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &nominal_symbols,
                &nominal_namespaces,
                &mut errors,
                "block post",
            );
        }
        for stmt in &mut sample {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &nominal_symbols,
                &nominal_namespaces,
                &mut errors,
                "sample",
            );
        }
        for event in &mut events {
            for stmt in &mut event.body {
                qualify_stmt_namespaced_symbols(
                    stmt,
                    "",
                    &callable_symbols,
                    &callable_namespaces,
                    &nominal_symbols,
                    &nominal_namespaces,
                    &mut errors,
                    &format!("event '{}'", event.name),
                );
            }
        }
    }

    let generic_struct_template_names: HashSet<String>;
    {
        let mut concrete_structs = Vec::<StructDef>::new();
        let mut generic_templates = HashMap::<String, StructDef>::new();
        for s in struct_defs_raw.drain(..) {
            if s.type_params.is_empty() {
                concrete_structs.push(s);
                continue;
            }
            if generic_templates.contains_key(&s.name) {
                errors.push(Diagnostic::semantic_span(
                    format!("duplicate generic struct '{}'", s.name),
                    s.loc,
                ));
                continue;
            }
            let mut seen = HashSet::new();
            for tp in &s.type_params {
                if !seen.insert(tp.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "duplicate generic type parameter '{}' in struct '{}'",
                            tp, s.name
                        ),
                        s.loc,
                    ));
                }
            }
            for method in &s.methods {
                for tp in &method.type_params {
                    if s.type_params.contains(tp) {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "type parameter '{}' on method '{}.{}' shadows '{}' from struct '{}'; use a different name",
                                tp, s.name, method.name, tp, s.name
                            ),
                            method.loc,
                        ));
                    }
                }
            }
            generic_templates.insert(s.name.clone(), s);
        }
        generic_struct_template_names = generic_templates.keys().cloned().collect();

        let mut generated_specializations = HashMap::<String, StructDef>::new();
        for s in &mut concrete_structs {
            rewrite_generic_struct_field_types(
                s,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
            for field in &mut s.fields {
                if let Some(default) = &mut field.default {
                    let mut locals = GenericInferenceLocals::default();
                    rewrite_generic_struct_ctor_expr(
                        default,
                        &generic_templates,
                        &mut generated_specializations,
                        &mut errors,
                        &mut locals,
                    );
                }
            }
            for method in &mut s.methods {
                rewrite_generic_struct_ctor_stmt_list(
                    &mut method.body,
                    &generic_templates,
                    &mut generated_specializations,
                    &mut errors,
                );
            }
        }
        for def in &mut defs {
            rewrite_generic_struct_ctor_stmt_list(
                &mut def.body,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
        }
        rewrite_generic_struct_ctor_stmt_list(
            &mut init,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut block_pre,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut block_post,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut sample,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        for event in &mut events {
            rewrite_generic_struct_ctor_stmt_list(
                &mut event.body,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
        }

        finalize_generated_generic_struct_specializations(
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );

        struct_defs_raw = concrete_structs;
        let mut generated = generated_specializations.into_values().collect::<Vec<_>>();
        generated.sort_by(|a, b| a.name.cmp(&b.name));
        struct_defs_raw.extend(generated);
    }
    order_struct_defs_for_field_dependencies(&mut struct_defs_raw);

    check_local_port_duplicates(&raw_ins, "input", &mut errors);
    check_local_port_duplicates(&raw_outs, "output", &mut errors);
    check_local_port_duplicates(&raw_kouts, "control output", &mut errors);
    check_local_param_duplicates(&raw_params, &mut errors);
    check_control_output_reserved_audio_names(&raw_kouts, "control output", &mut errors);

    let inferred_io = infer_numbered_io_from_sample(&sample);
    let mut inferred_owner_names = IoInference::default();
    for stmt in &init {
        infer_io_from_stmt(stmt, &mut inferred_owner_names);
    }
    for stmt in &block_pre {
        infer_io_from_stmt(stmt, &mut inferred_owner_names);
    }
    for stmt in &sample {
        infer_io_from_stmt(stmt, &mut inferred_owner_names);
    }
    for stmt in &block_post {
        infer_io_from_stmt(stmt, &mut inferred_owner_names);
    }
    for event in &events {
        for stmt in &event.body {
            infer_io_from_stmt(stmt, &mut inferred_owner_names);
        }
    }
    let inferred_param_prefix = match explicit_param_prefix {
        Some("kin") => {
            if inferred_owner_names.max_param > 0 {
                errors.push(Diagnostic::semantic_span(
                    "implicit paramN parameters cannot be mixed with a kins block; use kinN names with kins",
                    Span::ZERO,
                ));
            }
            "kin"
        }
        Some(_) => {
            if inferred_owner_names.max_kin > 0 {
                errors.push(Diagnostic::semantic_span(
                    "implicit kinN parameters cannot be mixed with a params block; use paramN names with params",
                    Span::ZERO,
                ));
            }
            "param"
        }
        None if inferred_owner_names.max_kin > 0 && inferred_owner_names.max_param == 0 => "kin",
        None => {
            if inferred_owner_names.max_kin > 0 && inferred_owner_names.max_param > 0 {
                errors.push(Diagnostic::semantic_span(
                    "implicit kinN and paramN parameters cannot be mixed in the same top-level program",
                    Span::ZERO,
                ));
            }
            "param"
        }
    };
    let inferred_param_max = if inferred_param_prefix == "kin" {
        inferred_owner_names.max_kin
    } else {
        inferred_owner_names.max_param
    };
    let params =
        normalize_numbered_param_decls(&raw_params, inferred_param_prefix, inferred_param_max);

    let ins_ports = normalize_numbered_port_decls(&raw_ins, "in", inferred_io.max_in);
    let mut audio_out_ports = normalize_numbered_port_decls(&raw_outs, "out", inferred_io.max_out);
    for port in &mut audio_out_ports {
        port.output_timing = Some(OutputTiming::Sample);
    }
    let mut control_out_ports =
        normalize_numbered_port_decls(&raw_kouts, "kout", inferred_owner_names.max_kout);
    for port in &mut control_out_ports {
        port.output_timing = Some(OutputTiming::Block);
    }
    check_port_name_conflicts(
        &audio_out_ports,
        "output",
        &control_out_ports,
        "control output",
        &mut errors,
    );
    let input_declared_names = ins_ports.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let mut output_declared_names = audio_out_ports
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    output_declared_names.extend(control_out_ports.iter().map(|p| p.name.clone()));
    let (ins, in_types, in_arrays, in_defaults, in_ranges) =
        expand_port_decls(&ins_ports, "input", options, &mut errors);
    let (outs, out_types, out_arrays, _out_defaults, _out_ranges) =
        expand_port_decls(&audio_out_ports, "output", options, &mut errors);
    let (
        control_outs,
        control_out_types,
        control_out_arrays,
        _control_out_defaults,
        _control_out_ranges,
    ) = expand_port_decls(&control_out_ports, "control output", options, &mut errors);
    if !outs.is_empty() && block_section_without_sample && !has_top_sample_block {
        let block_loc = program
            .block(BlockKind::Block)
            .map(Block::loc)
            .unwrap_or_default();
        errors.push(Diagnostic::semantic_span(
            "block section with sample-rate outputs must include nested 'sample' block",
            block_loc,
        ));
    }

    let (typed_params, param_arrays) = coerce_params(&params, options, &mut errors);
    let port_index_params = uniform_port_index_info_from_types(
        params_explicit,
        typed_params.len(),
        typed_params.iter().map(|param| param.ty),
    );
    let mut dynamic_param_array_names = HashSet::<String>::new();
    if port_index_params.is_some() {
        dynamic_param_array_names.insert("params".to_owned());
        if params_block_is_kins {
            dynamic_param_array_names.insert("kins".to_owned());
        }
    }
    let typed_buffers = coerce_buffers(&buffers, options, &mut errors);
    let param_types = typed_params
        .iter()
        .map(|p| (p.name.clone(), p.ty))
        .collect::<HashMap<_, _>>();
    let param_ranges = typed_params
        .iter()
        .filter_map(|p| p.range.map(|r| (p.name.clone(), r)))
        .collect::<HashMap<_, _>>();
    let mut occupied_temp_names = HashSet::<String>::new();
    occupied_temp_names.extend(input_declared_names.iter().cloned());
    occupied_temp_names.extend(output_declared_names.iter().cloned());
    occupied_temp_names.extend(params.iter().map(|p| p.name.clone()));
    occupied_temp_names.extend(typed_buffers.iter().map(|b| b.name.clone()));

    let mut make_unique_temp = |base: String| -> String {
        if occupied_temp_names.insert(base.clone()) {
            return base;
        }
        let mut idx = 1usize;
        loop {
            let candidate = format!("{base}_{idx}");
            if occupied_temp_names.insert(candidate.clone()) {
                return candidate;
            }
            idx += 1;
        }
    };

    let mut input_aliases = HashMap::<String, String>::new();
    let mut input_hoists = Vec::<Stmt>::new();
    let mut input_names = in_ranges.keys().cloned().collect::<Vec<_>>();
    input_names.sort();
    for name in input_names {
        let Some(range) = in_ranges.get(&name).copied() else {
            continue;
        };
        let ty = *in_types.get(&name).unwrap_or(&PrimitiveType::F32);
        let alias = make_unique_temp(format!(
            "__onda_clamped_in__{}",
            sanitize_symbol_component(&name)
        ));
        input_aliases.insert(name.clone(), alias.clone());
        input_hoists.push(build_top_level_range_hoist_assign(alias, &name, ty, range));
    }

    let mut param_aliases = HashMap::<String, String>::new();
    let mut param_hoists = Vec::<Stmt>::new();
    let mut param_names_sorted = param_ranges.keys().cloned().collect::<Vec<_>>();
    param_names_sorted.sort();
    for name in param_names_sorted {
        let Some(range) = param_ranges.get(&name).copied() else {
            continue;
        };
        let ty = *param_types.get(&name).unwrap_or(&PrimitiveType::F32);
        let alias = make_unique_temp(format!(
            "__onda_clamped_param__{}",
            sanitize_symbol_component(&name)
        ));
        param_aliases.insert(name.clone(), alias.clone());
        param_hoists.push(build_top_level_range_hoist_assign(alias, &name, ty, range));
    }

    for stmt in &mut block_pre {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, false, true);
    }
    for stmt in &mut sample {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, true, true);
    }
    for stmt in &mut block_post {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, false, true);
    }

    if !param_hoists.is_empty() {
        let mut rewritten = param_hoists;
        rewritten.append(&mut block_pre);
        block_pre = rewritten;
    }
    if !input_hoists.is_empty() {
        let mut rewritten = input_hoists;
        rewritten.append(&mut sample);
        sample = rewritten;
    }

    let mut all_declared = HashSet::new();
    check_unique_set(
        &input_declared_names,
        "input",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &output_declared_names,
        "output",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        "param",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &typed_buffers
            .iter()
            .map(|b| b.name.clone())
            .collect::<Vec<_>>(),
        "buffer",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(&const_scalar_names, "const", &mut all_declared, &mut errors);
    check_unique_set(
        &const_arrays
            .iter()
            .map(|array| array.name.clone())
            .collect::<Vec<_>>(),
        "const array",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &const_def_names,
        "const def",
        &mut all_declared,
        &mut errors,
    );

    let mut struct_defs = HashMap::new();
    let mut seen_struct_defs = HashMap::<String, StructDef>::new();
    let mut typed_structs = Vec::new();
    let mut method_self_struct = HashMap::<String, String>::new();
    let mut callable_symbols_for_method_sugar =
        defs.iter().map(|d| d.name.clone()).collect::<HashSet<_>>();
    for s in &struct_defs_raw {
        for method in &s.methods {
            callable_symbols_for_method_sugar.insert(format!("{}.{}", s.name, method.name));
        }
    }
    for s in &struct_defs_raw {
        if is_builtin_constant_name(&s.name) {
            errors.push(Diagnostic::semantic_span(
                format!("struct name '{}' is reserved as a builtin constant", s.name),
                s.loc,
            ));
            continue;
        }
        if let Some(existing) = seen_struct_defs.get(&s.name) {
            if existing == s {
                continue;
            }
            errors.push(Diagnostic::semantic_span(
                format!("duplicate struct '{}'", s.name),
                s.loc,
            ));
            continue;
        }
        if all_declared.contains(&s.name) {
            errors.push(Diagnostic::semantic_span(
                format!("struct name '{}' conflicts with existing symbol", s.name),
                s.loc,
            ));
            continue;
        }
        if struct_defs.contains_key(&s.name) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate struct '{}'", s.name),
                s.loc,
            ));
            continue;
        }
        let typed_fields = coerce_struct_fields(
            &s.name,
            &s.type_params,
            &s.fields,
            &struct_defs,
            options,
            &mut errors,
        );
        struct_defs.insert(s.name.clone(), typed_fields.clone());
        typed_structs.push(TypedStruct {
            name: s.name.clone(),
            fields: typed_fields,
        });
        all_declared.insert(s.name.clone());
        seen_struct_defs.insert(s.name.clone(), s.clone());

        for method in &s.methods {
            for tp in &method.type_params {
                if s.type_params.contains(tp) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "type parameter '{}' on method '{}.{}' shadows '{}' from struct '{}'; use a different name",
                            tp, s.name, method.name, tp, s.name
                        ),
                        method.loc,
                    ));
                }
            }
            if !method.type_params.is_empty() {
                let mut seen = HashSet::new();
                for tp in &method.type_params {
                    if !seen.insert(tp.clone()) {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "duplicate type parameter '{}' in method '{}.{}'",
                                tp, s.name, method.name
                            ),
                            method.loc,
                        ));
                    }
                }
            }
            if method.params.first().map(|p| p.name.as_str()) != Some("self") {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "method '{}.{}' must declare 'self' as first parameter",
                        s.name, method.name
                    ),
                    method.loc,
                ));
            }
            let fq_name = format!("{}.{}", s.name, method.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                method_self_struct.insert(fq_name.clone(), s.name.clone());
            }
            let mut desugared_method_body = method.body.clone();
            let mut method_struct_instances = HashMap::<String, String>::new();
            let mut method_struct_array_roots = HashMap::<String, String>::new();
            let method_ns = namespace_of_symbol(&s.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                register_struct_instance_and_array_roots(
                    "self",
                    &s.name,
                    &struct_defs,
                    &mut method_struct_instances,
                    &mut method_struct_array_roots,
                );
            }
            for stmt in &mut desugared_method_body {
                desugar_init_instance_method_calls(
                    stmt,
                    &mut method_struct_instances,
                    &mut method_struct_array_roots,
                    &struct_defs,
                    &method_ns,
                    &callable_symbols_for_method_sugar,
                );
            }
            defs.push(FunctionDef {
                loc: method.loc.clone(),
                is_const: false,
                type_params: method.type_params.clone(),
                name: fq_name,
                params: method.params.clone(),
                return_ty: method.return_ty.clone(),
                return_ty_loc: method.return_ty_loc,
                body: desugared_method_body,
            });
        }
    }

    for (struct_name, fields) in &struct_defs {
        for field in fields {
            if let Some(elem_struct) = &field.array_elem_struct {
                let context = format!("field '{}.{}' array element", struct_name, field.name);
                let _ =
                    validate_data_struct_layout(elem_struct, &struct_defs, &context, &mut errors);
            }
        }
    }

    let mut desugar_struct_instances = HashMap::<String, String>::new();
    let mut desugar_struct_array_roots = HashMap::<String, String>::new();
    for stmt in &mut init {
        desugar_init_instance_method_calls(
            stmt,
            &mut desugar_struct_instances,
            &mut desugar_struct_array_roots,
            &struct_defs,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut block_pre {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut block_post {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut sample {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for event in &mut events {
        for stmt in &mut event.body {
            desugar_sample_instance_method_calls(
                stmt,
                &desugar_struct_instances,
                &desugar_struct_array_roots,
                "",
                &callable_symbols_for_method_sugar,
            );
        }
    }
    for def in &mut defs {
        if method_self_struct.contains_key(&def.name) {
            continue;
        }
        let mut def_struct_instances = HashMap::<String, String>::new();
        let mut def_struct_array_roots = HashMap::<String, String>::new();
        for param in &def.params {
            if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                if struct_defs.contains_key(struct_name) {
                    register_struct_instance_and_array_roots(
                        &param.name,
                        struct_name,
                        &struct_defs,
                        &mut def_struct_instances,
                        &mut def_struct_array_roots,
                    );
                }
            }
        }
        let def_ns = namespace_of_symbol(&def.name);
        for stmt in &mut def.body {
            desugar_sample_instance_method_calls(
                stmt,
                &def_struct_instances,
                &def_struct_array_roots,
                &def_ns,
                &callable_symbols_for_method_sugar,
            );
        }
    }

    let (overload_candidates, def_public_name_by_internal) =
        crate::def_semantics::prepare_function_overloads(&mut defs);
    let method_self_struct_internal = defs
        .iter()
        .filter_map(|def| {
            let public_name = def_public_name_by_internal
                .get(&def.name)
                .cloned()
                .unwrap_or_else(|| def.name.clone());
            method_self_struct
                .get(&public_name)
                .cloned()
                .map(|owner| (def.name.clone(), owner))
        })
        .collect::<HashMap<_, _>>();

    let mut top_level_env = crate::def_semantics::OverloadRewriteEnv::default();
    top_level_env
        .scalar_types
        .extend(in_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .scalar_types
        .extend(out_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .scalar_types
        .extend(param_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env.array_elem_types.extend(
        in_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.array_elem_types.extend(
        out_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.array_elem_types.extend(
        param_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.array_elem_types.extend(
        const_array_infos
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.buffer_types.extend(
        typed_buffers
            .iter()
            .map(|b| (b.name.clone(), (b.elem_ty, b.channels.clone()))),
    );
    top_level_env
        .struct_instances
        .extend(desugar_struct_instances.clone());

    let mut init_rewrite_env = top_level_env.clone();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut init,
        &mut init_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut block_pre_rewrite_env = top_level_env.clone();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut block_pre,
        &mut block_pre_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut sample_rewrite_env = top_level_env.clone();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut sample,
        &mut sample_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut block_post_rewrite_env = top_level_env.clone();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut block_post,
        &mut block_post_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    for event in &mut events {
        let mut event_env = top_level_env.clone();
        crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
            &mut event.body,
            &mut event_env,
            &overload_candidates,
            &struct_defs,
            &mut errors,
        );
    }
    for def in &mut defs {
        let mut def_env = top_level_env.clone();
        for param in &mut def.params {
            match &param.ty {
                Some(FnParamType::Primitive(prim)) => {
                    def_env.scalar_types.insert(param.name.clone(), *prim);
                }
                Some(FnParamType::Struct(struct_name)) => {
                    if !def.type_params.contains(struct_name) {
                        def_env
                            .struct_instances
                            .insert(param.name.clone(), struct_name.clone());
                    }
                }
                Some(FnParamType::Buffer(buffer_ty)) => {
                    let channels = match &buffer_ty.channels {
                        BufferChannels::Mono => TypedBufferChannels::Mono,
                        BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                        BufferChannels::Static(expr) => {
                            crate::def_semantics::const_positive_usize_for_overload(expr)
                                .map(TypedBufferChannels::Static)
                                .unwrap_or(TypedBufferChannels::Dynamic)
                        }
                    };
                    let elem_ty = match &buffer_ty.elem {
                        BufferElemType::Primitive(ty) => *ty,
                        BufferElemType::Generic(_) => PrimitiveType::F32,
                    };
                    def_env
                        .buffer_types
                        .insert(param.name.clone(), (elem_ty, channels));
                }
                Some(FnParamType::Array(Some(prim))) => {
                    def_env.array_elem_types.insert(param.name.clone(), *prim);
                }
                Some(FnParamType::SizedArray {
                    elem: Some(prim), ..
                }) => {
                    def_env.array_elem_types.insert(param.name.clone(), *prim);
                }
                Some(FnParamType::ArrayGeneric(_))
                | Some(FnParamType::SizedArray { .. })
                | Some(FnParamType::Tuple(_)) => {}
                Some(FnParamType::Array(None)) | Some(FnParamType::BareBuffer) | None => {}
            }
            if let Some(default_expr) = &mut param.default {
                crate::def_semantics::rewrite_overloaded_calls_in_expr(
                    default_expr,
                    &def_env,
                    &overload_candidates,
                    &mut errors,
                );
            }
        }
        crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
            &mut def.body,
            &mut def_env,
            &overload_candidates,
            &struct_defs,
            &mut errors,
        );
    }

    let mut fn_signatures: HashMap<String, FnSignature> = HashMap::new();
    let mut seen_public_function_symbols = HashSet::<String>::new();
    for def in &defs {
        let public_name = def_public_name_by_internal
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| def.name.clone());
        let def_diag = DiagCtx::new(def.loc);
        if is_builtin_constant_name(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!(
                    "function name '{}' is reserved as a builtin constant",
                    public_name
                ),
            );
            continue;
        }
        if is_builtin_function_name(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("cannot redefine builtin function '{}'", public_name),
            );
            continue;
        }
        if struct_defs.contains_key(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("function name '{}' conflicts with struct name", public_name),
            );
            continue;
        }
        if all_declared.contains(&public_name)
            && !seen_public_function_symbols.contains(&public_name)
        {
            push_semantic(
                def_diag,
                &mut errors,
                format!(
                    "function name '{}' conflicts with existing symbol",
                    public_name
                ),
            );
            continue;
        }
        if fn_signatures.contains_key(&def.name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("duplicate function '{}'", def.name),
            );
            continue;
        }
        fn_signatures.insert(
            def.name.clone(),
            FnSignature {
                params: def.params.iter().map(|p| p.name.clone()).collect(),
                defaults: def.params.iter().map(|p| p.default.clone()).collect(),
                param_types: def.params.iter().map(|p| p.ty.clone()).collect(),
                type_params: def.type_params.clone(),
                readonly_array_params: HashSet::new(),
            },
        );
        if seen_public_function_symbols.insert(public_name.clone()) {
            all_declared.insert(public_name.clone());
        }

        let mut local_params = HashSet::new();
        for p in &def.params {
            let param_diag = DiagCtx::new(p.ty_loc.or(p.loc));
            if is_builtin_constant_name(&p.name) {
                push_semantic(
                    param_diag,
                    &mut errors,
                    format!(
                        "function parameter '{}' in '{}' is reserved as a builtin constant",
                        p.name, def.name
                    ),
                );
            }
            if !local_params.insert(p.name.clone()) {
                push_semantic(
                    param_diag,
                    &mut errors,
                    format!(
                        "duplicate function parameter '{}' in '{}'",
                        p.name, public_name
                    ),
                );
            }
            if let Some(default) = &p.default {
                if matches!(
                    p.ty,
                    Some(FnParamType::Buffer(_))
                        | Some(FnParamType::Array(_))
                        | Some(FnParamType::ArrayGeneric(_))
                        | Some(FnParamType::BareBuffer)
                ) {
                    push_semantic(
                        param_diag,
                        &mut errors,
                        format!(
                            "function parameter '{}.{}' is a buffer and cannot have a default value",
                            public_name, p.name
                        ),
                    );
                }
                validate_default_expr(
                    default,
                    &mut errors,
                    &format!("function parameter '{}.{}'", public_name, p.name),
                );
            }
        }
    }

    // --- Pre-mono validation: check generic def call-site type args ---
    validate_generic_def_type_args_in_stmts(&init, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&block_pre, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&sample, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&block_post, &fn_signatures, &mut errors);
    for event in &events {
        validate_generic_def_type_args_in_stmts(&event.body, &fn_signatures, &mut errors);
    }
    for def in &defs {
        validate_generic_def_type_args_in_stmts(&def.body, &fn_signatures, &mut errors);
    }

    // --- Def monomorphization pass ---
    // Identify defs whose parameters require monomorphization (generic struct,
    // untyped array `[]`, bare `buffer`, or generic def type params `<T>`).
    {
        let mono_eligible: HashSet<String> = fn_signatures
            .iter()
            .filter_map(|(name, sig)| {
                let needs_mono = !sig.type_params.is_empty()
                    || sig.param_types.iter().any(|pt| match pt {
                        Some(FnParamType::Struct(s))
                            if generic_struct_template_names.contains(s) =>
                        {
                            true
                        }
                        Some(FnParamType::Array(None)) | Some(FnParamType::BareBuffer) => true,
                        Some(FnParamType::ArrayGeneric(name)) => !struct_defs.contains_key(name),
                        Some(FnParamType::SizedArray {
                            generic_name: Some(name),
                            ..
                        }) => !struct_defs.contains_key(name),
                        _ => false,
                    });
                if needs_mono {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        // Also run mono pass when any def has untyped params that could be
        // inferred as tuple from call-site tuple literal args.
        let has_untyped_params = fn_signatures
            .values()
            .any(|sig| sig.param_types.iter().any(|pt| pt.is_none()));

        if !mono_eligible.is_empty() || has_untyped_params {
            let mut mono_return_types = infer_def_return_types(&defs, &fn_signatures, &struct_defs);
            let mut generated_defs = Vec::<FunctionDef>::new();
            let mut generated_sigs = HashMap::<String, FnSignature>::new();
            let mut mono_cache =
                HashMap::<(String, Vec<crate::def_semantics::MonoParamKey>), String>::new();
            let original_defs_snapshot = defs.clone();

            // Rewrite calls in-place across all scopes.
            crate::def_semantics::monomorphize_calls_in_stmts(
                &mut init,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
                &mut mono_return_types,
                &mut errors,
                &[],
            );
            crate::def_semantics::monomorphize_calls_in_stmts(
                &mut block_pre,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
                &mut mono_return_types,
                &mut errors,
                &[],
            );
            crate::def_semantics::monomorphize_calls_in_stmts(
                &mut sample,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
                &mut mono_return_types,
                &mut errors,
                &[],
            );
            crate::def_semantics::monomorphize_calls_in_stmts(
                &mut block_post,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
                &mut mono_return_types,
                &mut errors,
                &[],
            );
            for event in &mut events {
                crate::def_semantics::monomorphize_calls_in_stmts(
                    &mut event.body,
                    &top_level_env,
                    &mono_eligible,
                    &fn_signatures,
                    &defs,
                    &generic_struct_template_names,
                    &struct_defs,
                    &mut generated_defs,
                    &mut generated_sigs,
                    &mut mono_cache,
                    &mut mono_return_types,
                    &mut errors,
                    &[],
                );
            }
            // Also walk def bodies (def-to-def mono calls)
            for def in &mut defs {
                let mut def_env = top_level_env.clone();
                for param in &def.params {
                    match &param.ty {
                        Some(FnParamType::Primitive(prim)) => {
                            def_env.scalar_types.insert(param.name.clone(), *prim);
                        }
                        Some(FnParamType::Struct(struct_name)) => {
                            def_env
                                .struct_instances
                                .insert(param.name.clone(), struct_name.clone());
                        }
                        Some(FnParamType::Array(Some(prim))) => {
                            def_env.array_elem_types.insert(param.name.clone(), *prim);
                        }
                        Some(FnParamType::ArrayGeneric(_)) => {}
                        Some(FnParamType::Buffer(buffer_ty)) => {
                            let channels = match &buffer_ty.channels {
                                BufferChannels::Mono => TypedBufferChannels::Mono,
                                BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                                BufferChannels::Static(expr) => {
                                    crate::def_semantics::const_positive_usize_for_overload(expr)
                                        .map(TypedBufferChannels::Static)
                                        .unwrap_or(TypedBufferChannels::Dynamic)
                                }
                            };
                            let elem_ty = match &buffer_ty.elem {
                                BufferElemType::Primitive(ty) => *ty,
                                BufferElemType::Generic(_) => PrimitiveType::F32,
                            };
                            def_env
                                .buffer_types
                                .insert(param.name.clone(), (elem_ty, channels));
                        }
                        _ => {}
                    }
                }
                crate::def_semantics::monomorphize_calls_in_stmts(
                    &mut def.body,
                    &def_env,
                    &mono_eligible,
                    &fn_signatures,
                    &original_defs_snapshot,
                    &generic_struct_template_names,
                    &struct_defs,
                    &mut generated_defs,
                    &mut generated_sigs,
                    &mut mono_cache,
                    &mut mono_return_types,
                    &mut errors,
                    &def.type_params,
                );
            }

            // Mono-rewrite generated defs' bodies (def-to-def mono calls).
            // E.g. quad.__mono__g_f32 may call double(...) which also needs mono.
            // Loop until no new defs are generated.
            loop {
                let prev_count = generated_defs.len();
                let snapshot_for_gen = original_defs_snapshot.clone();
                let mut extra_defs = Vec::new();
                let mut extra_sigs = HashMap::new();
                for def in generated_defs.iter_mut() {
                    let mut def_env = top_level_env.clone();
                    for param in &def.params {
                        if let Some(FnParamType::Primitive(prim)) = &param.ty {
                            def_env.scalar_types.insert(param.name.clone(), *prim);
                        }
                    }
                    // Use both fn_signatures and already-generated sigs for lookup.
                    let mut combined_sigs = fn_signatures.clone();
                    for (k, v) in &generated_sigs {
                        combined_sigs.insert(k.clone(), v.clone());
                    }
                    crate::def_semantics::monomorphize_calls_in_stmts(
                        &mut def.body,
                        &def_env,
                        &mono_eligible,
                        &combined_sigs,
                        &snapshot_for_gen,
                        &generic_struct_template_names,
                        &struct_defs,
                        &mut extra_defs,
                        &mut extra_sigs,
                        &mut mono_cache,
                        &mut mono_return_types,
                        &mut errors,
                        &def.type_params,
                    );
                }
                if extra_defs.is_empty() {
                    break;
                }
                generated_defs.extend(extra_defs);
                for (k, v) in extra_sigs {
                    generated_sigs.insert(k, v);
                }
                if generated_defs.len() == prev_count {
                    break;
                }
            }

            // Register generated defs and signatures
            for sig in generated_sigs {
                fn_signatures.insert(sig.0, sig.1);
            }
            defs.extend(generated_defs);

            // Remove original mono-eligible defs — only specialized copies should
            // be processed by inference.  Also remove their fn_signatures.
            for name in &mono_eligible {
                fn_signatures.remove(name);
            }
            defs.retain(|d| !mono_eligible.contains(&d.name));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let input_names: HashSet<String> = ins.iter().cloned().collect();
    let output_names: HashSet<String> = outs.iter().cloned().collect();
    let control_output_names: HashSet<String> = control_outs.iter().cloned().collect();
    let all_output_names = output_names
        .union(&control_output_names)
        .cloned()
        .collect::<HashSet<_>>();
    let all_output_array_names = out_arrays
        .keys()
        .chain(control_out_arrays.keys())
        .cloned()
        .collect::<HashSet<_>>();
    let mut io_surface_array_names = in_arrays
        .keys()
        .chain(all_output_array_names.iter())
        .cloned()
        .collect::<HashSet<_>>();
    if ins_explicit && !ins.is_empty() {
        io_surface_array_names.insert("ins".to_owned());
    }
    if audio_outs_explicit && !outs.is_empty() {
        io_surface_array_names.insert("outs".to_owned());
    }
    if control_outs_explicit && !control_outs.is_empty() {
        io_surface_array_names.insert("kouts".to_owned());
    }
    let mut io_surface_names = input_names
        .union(&all_output_names)
        .cloned()
        .collect::<HashSet<_>>();
    io_surface_names.extend(io_surface_array_names.iter().cloned());
    let mut all_output_types = out_types.clone();
    all_output_types.extend(
        control_out_types
            .iter()
            .map(|(name, ty)| (name.clone(), *ty)),
    );
    let param_names: HashSet<String> = typed_params.iter().map(|p| p.name.clone()).collect();
    let def_return_types = infer_def_return_types(&defs, &fn_signatures, &struct_defs);
    validate_def_return_types(
        &defs,
        &fn_signatures,
        &def_return_types,
        &struct_defs,
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(errors);
    }
    fn_signatures
        .entry(PROC_INDEX_CALL_SENTINEL.to_owned())
        .or_insert_with(|| internal_proc_index_call_signature(false));
    fn_signatures
        .entry(format!(
            "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
        ))
        .or_insert_with(|| internal_proc_index_call_signature(true));
    update_readonly_array_param_signatures(&defs, &mut fn_signatures);

    let mut state_scalars = HashMap::<String, PrimitiveType>::new();
    let mut declared_symbols = DeclaredSymbolMap::new();
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &input_names,
        &in_types,
        DeclaredScalarSymbolKind::Input,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &all_output_names,
        &all_output_types,
        DeclaredScalarSymbolKind::Output,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &param_names,
        &param_types,
        DeclaredScalarSymbolKind::Param,
    );
    for (fn_name, ret_ty) in &def_return_types {
        if let ReturnType::Scalar(scalar_ty) = ret_ty {
            insert_declared_symbol(
                &mut state_scalars,
                &mut declared_symbols,
                fn_name.clone(),
                DeclaredSymbolInfo::FunctionReturn { ty: *scalar_ty },
            );
        }
    }
    for buffer in &typed_buffers {
        let channels = match buffer.channels {
            TypedBufferChannels::Mono => BufferChannelInfo::Mono,
            TypedBufferChannels::Static(ch) => BufferChannelInfo::Static(ch),
            TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
        };
        insert_declared_symbol(
            &mut state_scalars,
            &mut declared_symbols,
            buffer.name.clone(),
            DeclaredSymbolInfo::Buffer {
                elem_ty: buffer.elem_ty,
                channels,
            },
        );
    }
    let state_arrays = HashMap::new();
    let state_array_struct_roots = HashMap::<String, ArrayStructRootInfo>::new();
    let struct_instances = HashMap::new();
    let mut init_known_scalars = param_names.clone();
    init_known_scalars.extend(state_scalars.keys().cloned());
    let init_locals = HashSet::new();
    let init_local_aliases = LocalAliasTypes::new();
    let mut init_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut init_local_data_aliases, &param_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &const_array_infos, false);

    let init_default_ty =
        resolve_init_default_ty(init_default_decl_ty.as_ref(), "top-level", &mut errors);
    let top_level_proc_symbols = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Proc(proc_def) => Some(proc_def.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let no_proc_event_names = HashSet::<String>::new();
    let init_ctx = InitAnalysisCtx {
        context_label: "top-level",
        common: ScopeAnalysisCtx {
            policy: ScopePolicy::Init,
            input_names: &input_names,
            output_names: &all_output_names,
            output_array_names: &all_output_array_names,
            io_surface_names: &io_surface_names,
            io_surface_array_names: &io_surface_array_names,
            dynamic_param_array_names: &dynamic_param_array_names,
            param_names: &param_names,
            struct_defs: &struct_defs,
            fn_signatures: &fn_signatures,
            fn_return_types: &def_return_types,
            options,
            port_index_ins: None,
            port_index_outs: None,
            port_index_params: None,
            port_index_kins: None,
            proc_event_names: &no_proc_event_names,
        },
        init_default_ty,
        proc_resolution: None,
        top_level_proc_symbols: Some(&top_level_proc_symbols),
    };
    let mut init_st = InitAnalysisState {
        known_scalars: init_known_scalars,
        local_aliases: init_local_aliases,
        local_array_aliases: init_local_data_aliases,
        declared_symbols,
        state_scalars,
        state_arrays,
        state_array_struct_roots,
        struct_instances,
        state_tuples: HashMap::new(),
        state_array_specs: HashMap::new(),
        struct_instance_type_args: HashMap::new(),
        nested_procs: HashMap::new(),
        nested_proc_arrays: HashMap::new(),
    };
    analyze_owner_init_stmts(&init, &init_ctx, &init_locals, &mut init_st, &mut errors);
    let InitAnalysisState {
        known_scalars: _init_known_scalars,
        local_aliases: _init_local_aliases,
        local_array_aliases: _init_local_data_aliases,
        mut declared_symbols,
        mut state_scalars,
        mut state_arrays,
        state_array_struct_roots,
        struct_instances,
        nested_proc_arrays,
        state_tuples,
        ..
    } = init_st;
    let control_out_array_slots = control_out_arrays
        .iter()
        .flat_map(|(name, info)| (0..info.len).map(move |idx| format!("{name}[{idx}]")))
        .collect::<HashSet<_>>();
    for name in &control_outs {
        if control_out_array_slots.contains(name) {
            continue;
        }
        let ty = *control_out_types.get(name).unwrap_or(&PrimitiveType::F32);
        state_scalars.insert(name.clone(), ty);
    }
    for (name, info) in &control_out_arrays {
        state_arrays.insert(name.clone(), info.len);
        insert_declared_symbol(
            &mut state_scalars,
            &mut declared_symbols,
            name.clone(),
            DeclaredSymbolInfo::DataArray {
                elem_ty: info.elem_ty,
            },
        );
    }
    let init_writable_roots = collect_runtime_state_roots(&state_scalars, &state_arrays);
    let empty_nested_proc_instances = HashMap::<String, ProcNestedState>::new();

    // Rewrite struct-array inline field sentinels (for example `data[0].field` and
    // `data[0].field[i]`) to flattened Index exprs before runtime analysis.
    rewrite_owner_struct_array_inline_fields(
        ExecutableOwnerBodies {
            init: &mut init,
            block_pre: &mut block_pre,
            sample: &mut sample,
            block_post: &mut block_post,
            events: &mut events,
        },
        &state_array_struct_roots,
        &struct_defs,
        &mut errors,
    );

    let port_index_ins = uniform_port_index_info_from_names(ins_explicit, &ins, &in_types);
    let sample_port_index_outs =
        uniform_port_index_info_from_names(audio_outs_explicit, &outs, &out_types);
    let block_port_index_outs = uniform_port_index_info_from_names(
        control_outs_explicit,
        &control_outs,
        &control_out_types,
    );
    let port_index_kins = if params_block_is_kins {
        port_index_params
    } else {
        None
    };

    let typed_events = coerce_typed_events(&events, true, "top-level", options, &mut errors);
    let analysis_plan_seeds = build_top_level_owner_analysis_plan_seeds(
        &param_names,
        &input_names,
        &output_names,
        &control_output_names,
        &state_scalars,
        &in_arrays,
        &out_arrays,
        &control_out_arrays,
        &param_arrays,
        &const_array_infos,
    );
    {
        let mut runtime_state = ExecutableOwnerRuntimeState {
            state_scalars: &mut state_scalars,
            declared_symbols: &declared_symbols,
            state_arrays: &state_arrays,
            state_array_struct_roots: &state_array_struct_roots,
            nested_proc_instances: &empty_nested_proc_instances,
            proc_array_roots: &nested_proc_arrays,
            struct_instances: &struct_instances,
            state_tuples: &state_tuples,
        };
        analyze_owner_runtime_scopes(
            &mut runtime_state,
            analysis_plan_seeds.runtime_scope_plans(
                RuntimeScopeBodies {
                    block_pre: &block_pre,
                    sample: &sample,
                    block_post: &block_post,
                },
                RuntimeScopePlanInputs {
                    sample_input_names: &input_names,
                    block_output_names: &control_output_names,
                    sample_output_names: &output_names,
                    output_array_names: &all_output_array_names,
                    io_surface_names: &io_surface_names,
                    io_surface_array_names: &io_surface_array_names,
                    dynamic_param_array_names: &dynamic_param_array_names,
                    param_names: &param_names,
                    struct_defs: &struct_defs,
                    fn_signatures: &fn_signatures,
                    fn_return_types: &def_return_types,
                    options,
                    port_index_ins,
                    block_port_index_outs,
                    sample_port_index_outs,
                    port_index_params,
                    port_index_kins,
                    registration_input_names: &input_names,
                    registration_output_names: &all_output_names,
                    registration_param_names: &param_names,
                    proc_event_names: &no_proc_event_names,
                },
            ),
            &mut errors,
        );

        analyze_owner_events(
            &runtime_state,
            analysis_plan_seeds.event_plan(
                runtime_state.state_scalars,
                EventPlanInputs {
                    typed_events: &typed_events,
                    init_writable_roots: &init_writable_roots,
                    input_names: &input_names,
                    output_names: &all_output_names,
                    output_array_names: &all_output_array_names,
                    io_surface_names: &io_surface_names,
                    io_surface_array_names: &io_surface_array_names,
                    dynamic_param_array_names: &dynamic_param_array_names,
                    param_names: &param_names,
                    validation_input_names: &input_names,
                    validation_output_names: &all_output_names,
                    struct_defs: &struct_defs,
                    fn_signatures: &fn_signatures,
                    fn_return_types: &def_return_types,
                    options,
                    port_index_ins: None,
                    port_index_outs: None,
                    port_index_params: None,
                    port_index_kins: None,
                    proc_event_names: &no_proc_event_names,
                },
            ),
            &mut errors,
        );
    }

    let mut block_exec = block_pre.clone();
    block_exec.extend(block_post.clone());
    let mut sample_and_event_exec = sample.clone();
    for event in &typed_events {
        sample_and_event_exec.extend(event.body.clone());
    }

    let current_declared_symbols = &declared_symbols;
    let mut inferred_array_bindings = HashMap::<String, InferredArrayParam>::new();
    for (name, info) in &out_arrays {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, info) in &control_out_arrays {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, info) in &param_arrays {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, info) in &const_array_infos {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, len) in &state_arrays {
        let elem_ty = declared_symbol_scalar_type(current_declared_symbols, name)
            .unwrap_or(PrimitiveType::F32);
        inferred_array_bindings.insert(name.clone(), InferredArrayParam { elem_ty, len: *len });
    }
    let inferred_struct_array_roots = state_array_struct_roots
        .iter()
        .map(|(name, info)| (name.clone(), info.struct_name.clone()))
        .collect::<HashMap<_, _>>();
    let mut inferred_proc_array_roots = nested_proc_arrays
        .iter()
        .filter_map(|(name, info)| {
            let size_context = format!("top-level processor array '{}' size", name);
            let len = eval_data_size_expr(&info.size_expr, options, &size_context, &mut errors)?;
            Some((
                name.clone(),
                InferredProcArrayParam {
                    proc_name: info.proc_name.clone(),
                    len,
                },
            ))
        })
        .collect::<HashMap<_, _>>();
    for (name, info) in &state_array_struct_roots {
        if !proc_api.contains_key(&info.struct_name) {
            continue;
        }
        inferred_proc_array_roots
            .entry(name.clone())
            .or_insert_with(|| InferredProcArrayParam {
                proc_name: info.struct_name.clone(),
                len: info.len,
            });
    }

    let (inferred_def_params, synthesized_struct_defs) = infer_def_param_kinds(
        &defs,
        &init,
        &block_exec,
        &sample_and_event_exec,
        &struct_instances,
        &inferred_struct_array_roots,
        &inferred_proc_array_roots,
        &inferred_array_bindings,
        &typed_buffers
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    vec![InferredBufferParam {
                        elem_ty: b.elem_ty,
                        channels: b.channels.clone(),
                    }],
                )
            })
            .collect::<HashMap<_, _>>(),
        &fn_signatures,
        &method_self_struct_internal,
        &struct_defs,
        options,
        &mut errors,
    );
    let reachable_def_names =
        collect_reachable_def_names(&init, &block_exec, &sample_and_event_exec, &defs);

    let mut def_struct_defs = struct_defs.clone();
    for (name, fields) in &synthesized_struct_defs {
        def_struct_defs.insert(name.clone(), fields.clone());
    }

    let def_global_inputs = HashSet::<String>::new();
    let def_global_outputs = HashSet::<String>::new();
    let def_global_params = HashSet::<String>::new();
    for def in defs.iter_mut().filter(|def| {
        reachable_def_names.contains(&def.name)
            || def_has_concrete_param_contract(def, &method_self_struct_internal, &struct_defs)
    }) {
        let def_param_names = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<HashSet<_>>();
        let mut def_io_surface_names = io_surface_names.clone();
        let mut def_io_surface_array_names = io_surface_array_names.clone();
        for param in &def_param_names {
            def_io_surface_names.remove(param);
            def_io_surface_array_names.remove(param);
        }
        let fn_known = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<HashSet<_>>();
        let mut def_state_scalars = HashMap::<String, PrimitiveType>::new();
        let mut def_declared_symbols = declared_symbols
            .iter()
            .filter_map(|(name, info)| match info {
                DeclaredSymbolInfo::FunctionReturn { .. } => Some((name.clone(), info.clone())),
                _ => None,
            })
            .collect::<DeclaredSymbolMap>();
        let fn_sig = fn_signatures.get(&def.name);
        // Def parameters are function-local and should be visible for local
        // type inference even though top-level runtime symbols are not.
        for (idx, param) in def.params.iter().enumerate() {
            let explicit_prim = fn_sig
                .and_then(|sig| sig.param_types.get(idx))
                .and_then(|ty| ty.as_ref())
                .and_then(|ty| match ty {
                    FnParamType::Primitive(prim) => Some(*prim),
                    FnParamType::Struct(_)
                    | FnParamType::Buffer(_)
                    | FnParamType::Array(_)
                    | FnParamType::ArrayGeneric(_)
                    | FnParamType::SizedArray { .. }
                    | FnParamType::BareBuffer
                    | FnParamType::Tuple(_) => None,
                });
            if let Some(param_ty) = explicit_prim {
                def_state_scalars.insert(param.name.clone(), param_ty);
            } else {
                def_state_scalars.remove(&param.name);
            }
        }
        let fn_locals = HashSet::new();
        let fn_local_aliases = LocalAliasTypes::new();
        let mut fn_local_data_aliases = HashMap::new();
        seed_top_level_array_aliases(&mut fn_local_data_aliases, &const_array_infos, false);
        let fn_local_proc_aliases = HashMap::new();
        let param_names_vec = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();
        let param_structs = inferred_def_params
            .get(&def.name)
            .map(|k| param_struct_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let mut param_struct_arrays = def
            .params
            .iter()
            .filter_map(|param| match param.ty.as_ref() {
                Some(FnParamType::ArrayGeneric(struct_name))
                    if def_struct_defs.contains_key(struct_name)
                        && !def.type_params.contains(struct_name) =>
                {
                    Some((param.name.clone(), struct_name.clone()))
                }
                Some(FnParamType::SizedArray {
                    generic_name: Some(struct_name),
                    ..
                }) if def_struct_defs.contains_key(struct_name)
                    && !def.type_params.contains(struct_name) =>
                {
                    Some((param.name.clone(), struct_name.clone()))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        for (name, struct_name) in inferred_def_params
            .get(&def.name)
            .map(|k| param_struct_array_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default()
        {
            param_struct_arrays.entry(name).or_insert(struct_name);
        }
        let param_buffers = inferred_def_params
            .get(&def.name)
            .map(|k| param_buffer_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_proc_arrays = inferred_def_params
            .get(&def.name)
            .map(|k| param_proc_array_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_arrays = inferred_def_params
            .get(&def.name)
            .map(|k| param_array_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        for (param_name, elem_ty) in &param_arrays {
            fn_local_data_aliases.insert(
                param_name.clone(),
                LocalArrayAliasInfo {
                    len: 1,
                    elem_ty: *elem_ty,
                    elem_struct: None,
                    writable: true,
                },
            );
        }
        for (param_name, proc_info) in &param_proc_arrays {
            let has_block = proc_api
                .get(&proc_info.proc_name)
                .map(|api| api.has_block)
                .unwrap_or(false);
            if !has_block {
                continue;
            }
            let len = match &proc_info.size_expr {
                Expr::Int { value, .. } if *value >= 0 => *value as usize,
                _ => 1,
            };
            let active_symbol = runtime_proc_array_active_symbol(param_name);
            fn_local_data_aliases.insert(
                active_symbol.clone(),
                LocalArrayAliasInfo {
                    len,
                    elem_ty: PrimitiveType::Bool,
                    elem_struct: None,
                    writable: true,
                },
            );
            insert_declared_symbol(
                &mut def_state_scalars,
                &mut def_declared_symbols,
                active_symbol,
                DeclaredSymbolInfo::DataArray {
                    elem_ty: PrimitiveType::Bool,
                },
            );
        }
        let mut def_param_array_struct_roots = HashMap::<String, ArrayStructRootInfo>::new();
        for (param_name, struct_name) in &param_struct_arrays {
            register_struct_array_param_bindings(
                param_name,
                struct_name,
                &def_struct_defs,
                &mut def_declared_symbols,
                &mut fn_local_data_aliases,
                &mut def_param_array_struct_roots,
                &mut errors,
            );
        }
        for (param_name, (elem_ty, channels)) in &param_buffers {
            let channel_info = match channels {
                TypedBufferChannels::Mono => BufferChannelInfo::Mono,
                TypedBufferChannels::Static(ch) => BufferChannelInfo::Static(*ch),
                TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
            };
            insert_declared_symbol(
                &mut def_state_scalars,
                &mut def_declared_symbols,
                param_name.clone(),
                DeclaredSymbolInfo::Buffer {
                    elem_ty: *elem_ty,
                    channels: channel_info,
                },
            );
        }
        let mut def_proc_vars = HashMap::<String, ProcCallInstance>::new();
        let mut def_proc_array_slots = HashMap::<String, Vec<String>>::new();
        let mut def_proc_block_active_symbols = HashMap::<String, String>::new();
        if let Some(kinds) = inferred_def_params.get(&def.name) {
            for (param_name, kind) in def.params.iter().map(|p| &p.name).zip(kinds.iter()) {
                match kind {
                    TypedFnParam::Struct { struct_name } => {
                        if let Some(shape) = lowering_shapes.get(struct_name) {
                            for (base, slots) in &shape.nested_proc_array_slots {
                                def_proc_array_slots
                                    .entry(base.clone())
                                    .or_insert_with(|| slots.clone());
                                let prefixed_base = format!("{param_name}.{base}");
                                let prefixed_slots = slots
                                    .iter()
                                    .map(|slot| format!("{param_name}.{slot}"))
                                    .collect::<Vec<_>>();
                                def_proc_array_slots
                                    .entry(prefixed_base.clone())
                                    .or_insert(prefixed_slots);
                                if let Some(active_field) =
                                    shape.nested_proc_array_active_fields.get(base)
                                {
                                    def_proc_block_active_symbols
                                        .entry(prefixed_base)
                                        .or_insert_with(|| format!("{param_name}.{active_field}"));
                                }
                                for slot in slots {
                                    if let Some(nested) = shape.state.nested_procs.get(slot) {
                                        def_proc_vars.entry(slot.clone()).or_insert(
                                            ProcCallInstance {
                                                proc_name: nested.proc_name.clone(),
                                                buffer_args: Vec::new(),
                                            },
                                        );
                                        let prefixed_slot = format!("{param_name}.{slot}");
                                        def_proc_vars.entry(prefixed_slot).or_insert(
                                            ProcCallInstance {
                                                proc_name: nested.proc_name.clone(),
                                                buffer_args: Vec::new(),
                                            },
                                        );
                                    }
                                }
                            }
                            for (instance_name, nested) in &shape.state.nested_procs {
                                def_proc_vars.entry(instance_name.clone()).or_insert(
                                    ProcCallInstance {
                                        proc_name: nested.proc_name.clone(),
                                        buffer_args: Vec::new(),
                                    },
                                );
                                let prefixed_instance = format!("{param_name}.{instance_name}");
                                def_proc_vars.entry(prefixed_instance).or_insert(
                                    ProcCallInstance {
                                        proc_name: nested.proc_name.clone(),
                                        buffer_args: Vec::new(),
                                    },
                                );
                            }
                        }
                        if proc_api.contains_key(struct_name) {
                            def_proc_vars.insert(
                                param_name.clone(),
                                ProcCallInstance {
                                    proc_name: struct_name.clone(),
                                    buffer_args: Vec::new(),
                                },
                            );
                        }
                    }
                    TypedFnParam::ProcArray { proc_name, len } => {
                        let slot_names = (0..*len)
                            .map(|idx| format!("{param_name}.__proc_array_slot_{idx}"))
                            .collect::<Vec<_>>();
                        for slot_name in &slot_names {
                            def_proc_vars.insert(
                                slot_name.clone(),
                                ProcCallInstance {
                                    proc_name: proc_name.clone(),
                                    buffer_args: Vec::new(),
                                },
                            );
                        }
                        def_proc_array_slots.insert(param_name.clone(), slot_names);
                        def_proc_block_active_symbols.insert(
                            param_name.clone(),
                            runtime_proc_array_active_symbol(param_name),
                        );
                    }
                    _ => {}
                }
            }
        }
        if !def_proc_vars.is_empty() || !def_proc_array_slots.is_empty() {
            rewrite_proc_calls_in_stmts(
                &mut def.body,
                &def_proc_vars,
                &def_proc_array_slots,
                &proc_api,
                &mut errors,
            );
            let mut rewritten_def_body = Vec::<Stmt>::new();
            for stmt in std::mem::take(&mut def.body) {
                rewritten_def_body.extend(rewrite_stmt_for_def_proc_block_guards(
                    stmt,
                    &proc_api,
                    &def_proc_block_active_symbols,
                ));
            }
            def.body = rewritten_def_body;
        }
        let def_ctx = DefStmtAnalysisCtx {
            common: ScopeAnalysisCtx {
                policy: ScopePolicy::Def,
                input_names: &def_global_inputs,
                output_names: &def_global_outputs,
                output_array_names: &all_output_array_names,
                io_surface_names: &def_io_surface_names,
                io_surface_array_names: &def_io_surface_array_names,
                dynamic_param_array_names: &dynamic_param_array_names,
                param_names: &def_global_params,
                struct_defs: &def_struct_defs,
                fn_signatures: &fn_signatures,
                fn_return_types: &def_return_types,
                options,
                port_index_ins: None,
                port_index_outs: None,
                port_index_params: None,
                port_index_kins: None,
                proc_event_names: &no_proc_event_names,
            },
            locals: &fn_locals,
            declared_symbols: &def_declared_symbols,
            param_structs: &param_structs,
            proc_array_roots: &param_proc_arrays,
            state_scalars: &def_state_scalars,
            def_return_types: &def_return_types,
        };
        let mut def_state = DefStmtAnalysisState::from_parts(
            fn_known,
            fn_local_aliases,
            fn_local_data_aliases,
            fn_local_proc_aliases,
        );
        rewrite_struct_array_inline_field_stmts(
            &mut def.body,
            &def_param_array_struct_roots,
            &def_struct_defs,
            &mut errors,
        );
        // Register tuple params as tuple_vars for indexing validation
        if let Some(kinds) = inferred_def_params.get(&def.name) {
            for (param, kind) in def.params.iter().zip(kinds.iter()) {
                if let TypedFnParam::Tuple { elem_tys } = kind {
                    def_state
                        .tuple_vars
                        .insert(param.name.clone(), elem_tys.len());
                }
            }
        }
        for stmt in &def.body {
            analyze_def_stmt(stmt, def_ctx, &mut def_state, 0, &mut errors);
        }
    }

    if errors.is_empty() {
        let mut sorted_state = state_scalars.keys().cloned().collect::<Vec<_>>();
        sorted_state.sort();
        let state_types = sorted_state
            .iter()
            .map(|name| {
                state_scalars
                    .get(name)
                    .copied()
                    .unwrap_or(PrimitiveType::F32)
            })
            .collect::<Vec<_>>();

        let mut typed_data = state_arrays
            .into_iter()
            .map(|(name, len)| {
                let elem_ty = declared_symbol_scalar_type(current_declared_symbols, &name)
                    .unwrap_or(PrimitiveType::F32);
                TypedArrayVar { name, len, elem_ty }
            })
            .collect::<Vec<_>>();
        typed_data.sort_by(|a, b| a.name.cmp(&b.name));
        let mut typed_data_roots = state_array_struct_roots
            .into_iter()
            .map(|(name, info)| TypedArrayStructRoot {
                name,
                struct_name: info.struct_name,
                len: info.len,
            })
            .collect::<Vec<_>>();
        typed_data_roots.sort_by(|a, b| a.name.cmp(&b.name));

        let mut synth_names = synthesized_struct_defs.keys().cloned().collect::<Vec<_>>();
        synth_names.sort();
        for name in synth_names {
            if let Some(fields) = synthesized_struct_defs.get(&name) {
                typed_structs.push(TypedStruct {
                    name,
                    fields: fields.clone(),
                });
            }
        }

        reject_non_sample_proc_operator_calls(
            &init,
            &block_pre,
            &sample,
            &block_post,
            &typed_events,
            &defs,
            &proc_api,
            &lowering_shapes,
            &mut errors,
        );
        if !errors.is_empty() {
            return Err(errors);
        }

        let all_typed_defs = defs
            .into_iter()
            .map(|d| {
                let param_kinds = inferred_def_params
                    .get(&d.name)
                    .cloned()
                    .unwrap_or_else(|| vec![TypedFnParam::Scalar { ty: None }; d.params.len()]);
                TypedFunction {
                    method_of: method_self_struct_internal.get(&d.name).cloned(),
                    type_params: d.type_params.clone(),
                    param_defaults: d.params.iter().map(|p| p.default.clone()).collect(),
                    param_kinds,
                    return_ty: def_return_types
                        .get(&d.name)
                        .cloned()
                        .unwrap_or(ReturnType::Scalar(PrimitiveType::F32)),
                    name: d.name,
                    params: d.params.into_iter().map(|p| p.name).collect(),
                    body: d.body,
                }
            })
            .collect::<Vec<_>>();
        inject_sample_def_owner_proc_block_hooks(
            &sample,
            &mut block_pre,
            &mut block_post,
            &all_typed_defs,
            &proc_api,
            &top_level_proc_rewrite.global_proc_instances,
            &top_level_proc_rewrite.global_proc_array_slots,
            &mut errors,
        );
        let reachable_defs = collect_reachable_typed_def_names(
            &init,
            &block_pre,
            &sample,
            &block_post,
            &typed_events,
            &all_typed_defs,
        );
        let typed_defs = all_typed_defs
            .into_iter()
            .filter(|def| reachable_defs.contains(&def.name))
            .collect::<Vec<_>>();
        let mut proc_instance_oversample_factors = proc_instance_oversample_factors;
        if sample_oversample_factor > 1 {
            let defs_by_name = typed_defs
                .iter()
                .map(|def| (def.name.clone(), def))
                .collect::<HashMap<_, _>>();
            collect_def_proc_arg_oversample_factors_from_stmts(
                &sample,
                sample_oversample_factor,
                &defs_by_name,
                &top_level_proc_rewrite,
                &proc_api,
                &mut proc_instance_oversample_factors,
                &mut errors,
            );
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(TypedProgram {
            ins,
            outs,
            control_outs,
            in_types,
            out_types,
            control_out_types,
            param_types,
            in_defaults,
            in_ranges,
            in_arrays,
            out_arrays,
            control_out_arrays,
            param_arrays,
            const_arrays,
            params: typed_params,
            buffers: typed_buffers,
            structs: typed_structs,
            defs: typed_defs,
            events: typed_events,
            def_sample_oversample_factors,
            proc_step_oversample_meta,
            proc_instance_oversample_factors,
            init,
            block_pre,
            sample_oversample_factor,
            sample,
            block_post,
            state_vars: sorted_state,
            state_types,
            state_tuples,
            array_vars: typed_data,
            array_struct_roots: typed_data_roots,
            ins_explicit,
            audio_outs_explicit,
            control_outs_explicit,
            outs_explicit,
            params_explicit,
        })
    } else {
        Err(errors)
    }
}

fn requires_entry_sample(program: &Program) -> bool {
    program.blocks.iter().any(|block| {
        matches!(
            block.kind(),
            BlockKind::Ins
                | BlockKind::Outs
                | BlockKind::KOuts
                | BlockKind::Params
                | BlockKind::Events
                | BlockKind::Buffers
                | BlockKind::Init
                | BlockKind::Block
                | BlockKind::Sample
                | BlockKind::Graph
        )
    })
}

fn record_top_level_proc_arg_oversample_factor(
    instance_name: &str,
    sample_oversample_factor: usize,
    top_level_proc_rewrite: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(instance) = top_level_proc_rewrite
        .global_proc_instances
        .get(instance_name)
    else {
        return;
    };
    let Some(api) = proc_api.get(&instance.proc_name) else {
        return;
    };
    let proc_oversample_factor = api.sample_oversample_factor.max(1);
    if sample_oversample_factor > 1 && proc_oversample_factor > 1 {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "cannot pass explicitly oversampled processor '{}' (sample {}) into oversampled context (sample {})",
                instance.proc_name, proc_oversample_factor, sample_oversample_factor
            ),
        );
    }
    let effective_factor = if proc_oversample_factor > 1 {
        proc_oversample_factor
    } else {
        sample_oversample_factor.max(1)
    };
    if effective_factor <= 1 {
        return;
    }
    if let Some(previous) = out.get(instance_name).copied() {
        if previous != effective_factor {
            push_semantic(
                DiagCtx::default(),
                errors,
                format!(
                    "processor instance '{}' is required at both sample {} and sample {}; a physical processor instance can only have one effective oversampling rate",
                    instance_name, previous, effective_factor
                ),
            );
        }
        return;
    }
    out.insert(instance_name.to_owned(), effective_factor);
}

fn record_top_level_proc_array_arg_oversample_factor(
    base: &str,
    sample_oversample_factor: usize,
    top_level_proc_rewrite: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(slots) = top_level_proc_rewrite.global_proc_array_slots.get(base) else {
        return;
    };
    for slot in slots {
        record_top_level_proc_arg_oversample_factor(
            slot,
            sample_oversample_factor,
            top_level_proc_rewrite,
            proc_api,
            out,
            errors,
        );
    }
    let mut slot_factors = slots.iter().filter_map(|slot| out.get(slot).copied());
    let Some(first) = slot_factors.next() else {
        return;
    };
    if slots
        .iter()
        .all(|slot| out.get(slot).copied() == Some(first))
    {
        out.insert(base.to_owned(), first);
    }
}

fn resolved_def_call_arg<'a>(
    args: &'a [CallArg],
    param_names: &[String],
    param_idx: usize,
) -> Option<&'a Expr> {
    let param_name = param_names.get(param_idx)?;
    if let Some(named) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(param_name.as_str()))
    {
        return Some(&named.expr);
    }
    let mut positional_idx = 0usize;
    for arg in args {
        if arg.name.is_some() {
            continue;
        }
        if positional_idx == param_idx {
            return Some(&arg.expr);
        }
        positional_idx += 1;
    }
    None
}

fn collect_def_proc_arg_oversample_factors_from_expr(
    expr: &Expr,
    sample_oversample_factor: usize,
    defs_by_name: &HashMap<String, &TypedFunction>,
    top_level_proc_rewrite: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            if let Some(def) = defs_by_name.get(name) {
                for (param_idx, kind) in def.param_kinds.iter().enumerate() {
                    let Some(arg_expr) = resolved_def_call_arg(args, &def.params, param_idx) else {
                        continue;
                    };
                    match (kind, arg_expr) {
                        (TypedFnParam::ProcArray { .. }, Expr::Var { name: base, .. }) => {
                            record_top_level_proc_array_arg_oversample_factor(
                                base,
                                sample_oversample_factor,
                                top_level_proc_rewrite,
                                proc_api,
                                out,
                                errors,
                            );
                        }
                        (TypedFnParam::Struct { struct_name }, Expr::Var { name, .. }) => {
                            let Some(instance) =
                                top_level_proc_rewrite.global_proc_instances.get(name)
                            else {
                                continue;
                            };
                            if &instance.proc_name == struct_name {
                                record_top_level_proc_arg_oversample_factor(
                                    name,
                                    sample_oversample_factor,
                                    top_level_proc_rewrite,
                                    proc_api,
                                    out,
                                    errors,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            for arg in args {
                collect_def_proc_arg_oversample_factors_from_expr(
                    &arg.expr,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
        }
        Expr::Index { index, .. } => collect_def_proc_arg_oversample_factors_from_expr(
            index,
            sample_oversample_factor,
            defs_by_name,
            top_level_proc_rewrite,
            proc_api,
            out,
            errors,
        ),
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_def_proc_arg_oversample_factors_from_expr(
                    start,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
            if let Some(end) = end {
                collect_def_proc_arg_oversample_factors_from_expr(
                    end,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_def_proc_arg_oversample_factors_from_expr(
                &spec.size,
                sample_oversample_factor,
                defs_by_name,
                top_level_proc_rewrite,
                proc_api,
                out,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    collect_def_proc_arg_oversample_factors_from_expr(
                        value,
                        sample_oversample_factor,
                        defs_by_name,
                        top_level_proc_rewrite,
                        proc_api,
                        out,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_def_proc_arg_oversample_factors_from_expr(
                lhs,
                sample_oversample_factor,
                defs_by_name,
                top_level_proc_rewrite,
                proc_api,
                out,
                errors,
            );
            collect_def_proc_arg_oversample_factors_from_expr(
                rhs,
                sample_oversample_factor,
                defs_by_name,
                top_level_proc_rewrite,
                proc_api,
                out,
                errors,
            );
        }
        Expr::Call { args, .. }
        | Expr::ArrayLiteral { values: args, .. }
        | Expr::Tuple { values: args, .. } => {
            for arg in args {
                collect_def_proc_arg_oversample_factors_from_expr(
                    arg,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_def_proc_arg_oversample_factors_from_expr(
                expr,
                sample_oversample_factor,
                defs_by_name,
                top_level_proc_rewrite,
                proc_api,
                out,
                errors,
            );
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn collect_def_proc_arg_oversample_factors_from_stmts(
    stmts: &[Stmt],
    sample_oversample_factor: usize,
    defs_by_name: &HashMap<String, &TypedFunction>,
    top_level_proc_rewrite: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashMap<String, usize>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_def_proc_arg_oversample_factors_from_expr(
                    expr,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_def_proc_arg_oversample_factors_from_expr(
                    cond,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                collect_def_proc_arg_oversample_factors_from_stmts(
                    then_branch,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                collect_def_proc_arg_oversample_factors_from_stmts(
                    else_branch,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_def_proc_arg_oversample_factors_from_expr(
                    start,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                collect_def_proc_arg_oversample_factors_from_expr(
                    end,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                if let Some(step) = step {
                    collect_def_proc_arg_oversample_factors_from_expr(
                        step,
                        sample_oversample_factor,
                        defs_by_name,
                        top_level_proc_rewrite,
                        proc_api,
                        out,
                        errors,
                    );
                }
                collect_def_proc_arg_oversample_factors_from_stmts(
                    body,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
            Stmt::While { cond, body, .. } => {
                collect_def_proc_arg_oversample_factors_from_expr(
                    cond,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
                collect_def_proc_arg_oversample_factors_from_stmts(
                    body,
                    sample_oversample_factor,
                    defs_by_name,
                    top_level_proc_rewrite,
                    proc_api,
                    out,
                    errors,
                );
            }
        }
    }
}

/// Pre-monomorphization validation of generic def call-site type arguments.
/// Checks for bool type args and type arg count mismatches BEFORE mono rewrites calls.
fn validate_generic_def_type_args_in_stmts(
    stmts: &[Stmt],
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        validate_generic_def_type_args_in_stmt(stmt, fn_signatures, errors);
    }
}

fn validate_generic_def_type_args_in_stmt(
    stmt: &Stmt,
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            validate_generic_def_type_args_in_expr(expr, fn_signatures, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            validate_generic_def_type_args_in_expr(cond, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(then_branch, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(else_branch, fn_signatures, errors);
        }
        Stmt::For { body, .. } => {
            validate_generic_def_type_args_in_stmts(body, fn_signatures, errors);
        }
        Stmt::While { cond, body, .. } => {
            validate_generic_def_type_args_in_expr(cond, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(body, fn_signatures, errors);
        }
        _ => {}
    }
}

fn validate_generic_def_type_args_in_expr(
    expr: &Expr,
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            if let Some(sig) = fn_signatures.get(name.as_str()) {
                if !type_args.is_empty() && !sig.type_params.is_empty() {
                    if type_args.len() != sig.type_params.len() {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "function '{}' expects {} type arguments, got {}",
                                name,
                                sig.type_params.len(),
                                type_args.len()
                            ),
                            expr.loc(),
                        ));
                    }
                    for ta in type_args {
                        if matches!(ta, CallTypeArg::Primitive(PrimitiveType::Bool)) {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "'bool' is not valid as a generic type argument for '{}'; use f32, f64, i32, or i64",
                                    name
                                ),
                                expr.loc(),
                            ));
                        }
                    }
                }
            }
            for arg in args {
                validate_generic_def_type_args_in_expr(&arg.expr, fn_signatures, errors);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_generic_def_type_args_in_expr(arg, fn_signatures, errors);
            }
        }
        Expr::Binary { lhs, rhs, .. }
        | Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            validate_generic_def_type_args_in_expr(lhs, fn_signatures, errors);
            validate_generic_def_type_args_in_expr(rhs, fn_signatures, errors);
        }
        Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. }
        | Expr::Cast { expr: inner, .. } => {
            validate_generic_def_type_args_in_expr(inner, fn_signatures, errors);
        }
        Expr::Tuple { values, .. } | Expr::ArrayLiteral { values, .. } => {
            for v in values {
                validate_generic_def_type_args_in_expr(v, fn_signatures, errors);
            }
        }
        Expr::Index { index, .. } => {
            validate_generic_def_type_args_in_expr(index, fn_signatures, errors);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(s) = start {
                validate_generic_def_type_args_in_expr(s, fn_signatures, errors);
            }
            if let Some(e) = end {
                validate_generic_def_type_args_in_expr(e, fn_signatures, errors);
            }
        }
        _ => {}
    }
}

fn collect_reachable_typed_def_names(
    init: &[Stmt],
    block_pre: &[Stmt],
    sample: &[Stmt],
    block_post: &[Stmt],
    events: &[TypedEvent],
    defs: &[TypedFunction],
) -> HashSet<String> {
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let def_names = def_map.keys().cloned().collect::<HashSet<_>>();
    let mut pending = Vec::<String>::new();
    let mut seen_pending = HashSet::<String>::new();

    seed_called_typed_defs_from_stmts(init, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(block_pre, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(sample, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(block_post, &def_names, &mut pending, &mut seen_pending);
    for event in events {
        seed_called_typed_defs_from_stmts(&event.body, &def_names, &mut pending, &mut seen_pending);
    }

    let mut reachable = HashSet::<String>::new();
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(def) = def_map.get(&name) else {
            continue;
        };
        seed_called_typed_defs_from_stmts(&def.body, &def_names, &mut pending, &mut seen_pending);
    }
    reachable
}

fn collect_reachable_def_names(
    init: &[Stmt],
    block_exec: &[Stmt],
    sample_and_event_exec: &[Stmt],
    defs: &[FunctionDef],
) -> HashSet<String> {
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let def_names = def_map.keys().cloned().collect::<HashSet<_>>();
    let mut pending = Vec::<String>::new();
    let mut seen_pending = HashSet::<String>::new();

    seed_called_typed_defs_from_stmts(init, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(block_exec, &def_names, &mut pending, &mut seen_pending);
    seed_called_typed_defs_from_stmts(
        sample_and_event_exec,
        &def_names,
        &mut pending,
        &mut seen_pending,
    );

    let mut reachable = HashSet::<String>::new();
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(def) = def_map.get(&name) else {
            continue;
        };
        seed_called_typed_defs_from_stmts(&def.body, &def_names, &mut pending, &mut seen_pending);
    }
    reachable
}

fn def_is_block_generated_root(name: &str) -> bool {
    name.ends_with(PROC_BLOCK_PRE_FN_SUFFIX) || name.ends_with(PROC_BLOCK_POST_FN_SUFFIX)
}

fn def_is_neither_phase_generated_root(name: &str) -> bool {
    name.ends_with(PROC_INIT_FN_SUFFIX) || name.contains(PROC_EVENT_FN_PREFIX)
}

fn proc_name_for_lowered_proc_call(name: &str) -> Option<&str> {
    if let Some(step_proc) = name.strip_suffix(PROC_STEP_FN_SUFFIX) {
        return Some(step_proc);
    }
    let (call_proc, out_idx_raw) = name.rsplit_once(PROC_CALL_OUT_FN_PREFIX)?;
    out_idx_raw.parse::<usize>().ok()?;
    Some(call_proc)
}

fn lowered_proc_call_timing(
    name: &str,
    proc_api: &HashMap<String, ProcApi>,
    generated_proc_call_timing: &HashMap<String, OutputTiming>,
) -> Option<OutputTiming> {
    if let Some(timing) = generated_proc_call_timing.get(name).copied() {
        return Some(timing);
    }
    let proc_name = proc_name_for_lowered_proc_call(name)?;
    proc_api.get(proc_name).map(|api| api.outputs.timing)
}

fn generated_proc_call_timing_map(
    proc_api: &HashMap<String, ProcApi>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
) -> HashMap<String, OutputTiming> {
    let mut out = HashMap::<String, OutputTiming>::new();
    for (owner_proc, shape) in lowering_shapes {
        for (nested_var, nested) in &shape.state.nested_procs {
            let Some(api) = proc_api.get(&nested.proc_name) else {
                continue;
            };
            out.insert(
                nested_step_fn_name(owner_proc, nested_var),
                api.outputs.timing,
            );
            for out_idx in 0..api.outputs.names.len() {
                out.insert(
                    nested_call_out_fn_name(owner_proc, nested_var, out_idx),
                    api.outputs.timing,
                );
            }
        }
    }
    out
}

fn collect_proc_call_diags_from_expr(
    expr: &Expr,
    proc_api: &HashMap<String, ProcApi>,
    generated_proc_call_timing: &HashMap<String, OutputTiming>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_proc_call_diags_from_expr(value, proc_api, generated_proc_call_timing, out);
            }
        }
        Expr::Index { index, .. } => {
            collect_proc_call_diags_from_expr(index, proc_api, generated_proc_call_timing, out)
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_proc_call_diags_from_expr(start, proc_api, generated_proc_call_timing, out);
            }
            if let Some(end) = end {
                collect_proc_call_diags_from_expr(end, proc_api, generated_proc_call_timing, out);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_proc_call_diags_from_expr(
                &spec.size,
                proc_api,
                generated_proc_call_timing,
                out,
            );
            if let Some(values) = init {
                for value in values {
                    collect_proc_call_diags_from_expr(
                        value,
                        proc_api,
                        generated_proc_call_timing,
                        out,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_proc_call_diags_from_expr(lhs, proc_api, generated_proc_call_timing, out);
            collect_proc_call_diags_from_expr(rhs, proc_api, generated_proc_call_timing, out);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_proc_call_diags_from_expr(arg, proc_api, generated_proc_call_timing, out);
            }
        }
        Expr::UserCall {
            name, args, loc, ..
        } => {
            if let Some(timing) =
                lowered_proc_call_timing(name, proc_api, generated_proc_call_timing)
            {
                out.push((DiagCtx::new(*loc), timing));
            }
            for arg in args {
                collect_proc_call_diags_from_expr(
                    &arg.expr,
                    proc_api,
                    generated_proc_call_timing,
                    out,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_proc_call_diags_from_expr(expr, proc_api, generated_proc_call_timing, out);
        }
    }
}

fn collect_proc_call_diags_from_stmts(
    stmts: &[Stmt],
    proc_api: &HashMap<String, ProcApi>,
    generated_proc_call_timing: &HashMap<String, OutputTiming>,
    out: &mut Vec<(DiagCtx, OutputTiming)>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => collect_proc_call_diags_from_expr(
                &decl.expr,
                proc_api,
                generated_proc_call_timing,
                out,
            ),
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_proc_call_diags_from_expr(expr, proc_api, generated_proc_call_timing, out);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_proc_call_diags_from_expr(cond, proc_api, generated_proc_call_timing, out);
                collect_proc_call_diags_from_stmts(
                    then_branch,
                    proc_api,
                    generated_proc_call_timing,
                    out,
                );
                collect_proc_call_diags_from_stmts(
                    else_branch,
                    proc_api,
                    generated_proc_call_timing,
                    out,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_proc_call_diags_from_expr(start, proc_api, generated_proc_call_timing, out);
                collect_proc_call_diags_from_expr(end, proc_api, generated_proc_call_timing, out);
                if let Some(step) = step {
                    collect_proc_call_diags_from_expr(
                        step,
                        proc_api,
                        generated_proc_call_timing,
                        out,
                    );
                }
                collect_proc_call_diags_from_stmts(body, proc_api, generated_proc_call_timing, out);
            }
            Stmt::While { cond, body, .. } => {
                collect_proc_call_diags_from_expr(cond, proc_api, generated_proc_call_timing, out);
                collect_proc_call_diags_from_stmts(body, proc_api, generated_proc_call_timing, out);
            }
        }
    }
}

fn push_proc_call_phase_errors(
    stmts: &[Stmt],
    context: &str,
    allowed: Option<OutputTiming>,
    proc_api: &HashMap<String, ProcApi>,
    generated_proc_call_timing: &HashMap<String, OutputTiming>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut diags = Vec::<(DiagCtx, OutputTiming)>::new();
    collect_proc_call_diags_from_stmts(stmts, proc_api, generated_proc_call_timing, &mut diags);
    for (diag, timing) in diags {
        if Some(timing) == allowed {
            continue;
        }
        let required = match timing {
            OutputTiming::Sample => "sample",
            OutputTiming::Block => "block",
        };
        push_semantic(
            diag,
            errors,
            format!("proc operator '()' for {required}-rate proc is only allowed in {required}; found use in {context}"),
        );
    }
}

fn collect_reachable_defs_for_phase(
    roots: &[&[Stmt]],
    generated_root: impl Fn(&str) -> bool,
    defs: &[FunctionDef],
    def_names: &HashSet<String>,
    def_map: &HashMap<String, &FunctionDef>,
) -> HashSet<String> {
    let mut pending = Vec::<String>::new();
    let mut seen_pending = HashSet::<String>::new();
    for root in roots {
        seed_called_typed_defs_from_stmts(root, def_names, &mut pending, &mut seen_pending);
    }
    for def in defs {
        if generated_root(&def.name) && seen_pending.insert(def.name.clone()) {
            pending.push(def.name.clone());
        }
    }

    let mut reachable = HashSet::<String>::new();
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(def) = def_map.get(&name) else {
            continue;
        };
        seed_called_typed_defs_from_stmts(&def.body, def_names, &mut pending, &mut seen_pending);
    }
    reachable
}

fn reject_non_sample_proc_operator_calls(
    init: &[Stmt],
    block_pre: &[Stmt],
    sample: &[Stmt],
    block_post: &[Stmt],
    events: &[TypedEvent],
    defs: &[FunctionDef],
    proc_api: &HashMap<String, ProcApi>,
    lowering_shapes: &HashMap<String, ProcLoweringShape>,
    errors: &mut Vec<Diagnostic>,
) {
    let generated_proc_call_timing = generated_proc_call_timing_map(proc_api, lowering_shapes);
    push_proc_call_phase_errors(
        init,
        "init",
        None,
        proc_api,
        &generated_proc_call_timing,
        errors,
    );
    push_proc_call_phase_errors(
        block_pre,
        "block pre",
        Some(OutputTiming::Block),
        proc_api,
        &generated_proc_call_timing,
        errors,
    );
    push_proc_call_phase_errors(
        sample,
        "sample",
        Some(OutputTiming::Sample),
        proc_api,
        &generated_proc_call_timing,
        errors,
    );
    push_proc_call_phase_errors(
        block_post,
        "block post",
        Some(OutputTiming::Block),
        proc_api,
        &generated_proc_call_timing,
        errors,
    );
    for event in events {
        push_proc_call_phase_errors(
            &event.body,
            &format!("event '{}'", event.name),
            None,
            proc_api,
            &generated_proc_call_timing,
            errors,
        );
    }

    let def_names = defs
        .iter()
        .map(|def| def.name.clone())
        .collect::<HashSet<_>>();
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let mut defs_with_proc_calls = HashMap::<String, Vec<(DiagCtx, OutputTiming)>>::new();
    for def in defs {
        let mut proc_call_diags = Vec::<(DiagCtx, OutputTiming)>::new();
        collect_proc_call_diags_from_stmts(
            &def.body,
            proc_api,
            &generated_proc_call_timing,
            &mut proc_call_diags,
        );
        if !proc_call_diags.is_empty() {
            defs_with_proc_calls.insert(def.name.clone(), proc_call_diags);
        }
    }

    let block_reachable_defs = collect_reachable_defs_for_phase(
        &[block_pre, block_post],
        |name| {
            def_is_block_generated_root(name)
                || lowered_proc_call_timing(name, proc_api, &generated_proc_call_timing)
                    == Some(OutputTiming::Block)
        },
        defs,
        &def_names,
        &def_map,
    );
    let sample_reachable_defs = collect_reachable_defs_for_phase(
        &[sample],
        |name| {
            lowered_proc_call_timing(name, proc_api, &generated_proc_call_timing)
                == Some(OutputTiming::Sample)
        },
        defs,
        &def_names,
        &def_map,
    );
    let mut neither_roots = Vec::<&[Stmt]>::new();
    neither_roots.push(init);
    for event in events {
        neither_roots.push(&event.body);
    }
    let neither_reachable_defs = collect_reachable_defs_for_phase(
        &neither_roots,
        def_is_neither_phase_generated_root,
        defs,
        &def_names,
        &def_map,
    );

    for (def_name, proc_call_diags) in defs_with_proc_calls {
        for (diag, timing) in proc_call_diags {
            let allowed = match timing {
                OutputTiming::Sample => {
                    sample_reachable_defs.contains(&def_name)
                        && !block_reachable_defs.contains(&def_name)
                        && !neither_reachable_defs.contains(&def_name)
                }
                OutputTiming::Block => {
                    block_reachable_defs.contains(&def_name)
                        && !sample_reachable_defs.contains(&def_name)
                        && !neither_reachable_defs.contains(&def_name)
                }
            };
            if allowed {
                continue;
            }
            let required = match timing {
                OutputTiming::Sample => "sample",
                OutputTiming::Block => "block",
            };
            push_semantic(
                diag,
                errors,
                format!("proc operator '()' for {required}-rate proc is only allowed in {required}; call in '{def_name}' is not provably {required}-only"),
            );
        }
    }
}

fn is_array_param_type(ty: Option<&FnParamType>) -> bool {
    matches!(
        ty,
        Some(FnParamType::Array(_))
            | Some(FnParamType::ArrayGeneric(_))
            | Some(FnParamType::SizedArray { .. })
    )
}

fn initial_readonly_array_param_candidates(def: &FunctionDef) -> HashSet<String> {
    def.params
        .iter()
        .filter(|param| is_array_param_type(param.ty.as_ref()))
        .map(|param| param.name.clone())
        .collect()
}

fn readonly_alias_source(expr: &Expr, aliases: &HashMap<String, String>) -> Option<String> {
    match expr {
        Expr::Var { name, .. } => aliases.get(name).cloned(),
        Expr::Slice { base, .. } => aliases.get(base).cloned(),
        _ => None,
    }
}

fn mark_readonly_param_expr_uses_as_mutable(
    expr: &Expr,
    aliases: &HashMap<String, String>,
    fn_signatures: &HashMap<String, FnSignature>,
    readonly_params: &HashMap<String, HashSet<String>>,
    mutable_params: &mut HashSet<String>,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            if name == UNSAFE_WRITE_FN {
                if let Some(source_param) = args
                    .first()
                    .and_then(|arg| readonly_alias_source(&arg.expr, aliases))
                {
                    mutable_params.insert(source_param.to_owned());
                }
            } else if let Some((base, method)) = name.rsplit_once('.') {
                if method == UNSAFE_WRITE_FN {
                    if let Some(source_param) = aliases.get(base) {
                        mutable_params.insert(source_param.clone());
                    }
                }
            }

            if let Some(sig) = fn_signatures.get(name) {
                let mut ignored = Vec::new();
                let resolved = resolve_call_args_at(
                    args,
                    &sig.params,
                    &sig.defaults,
                    sig.params.first().map(String::as_str) == Some("self"),
                    false,
                    &format!("function '{name}' call"),
                    expr.loc(),
                    &mut ignored,
                );
                if ignored.is_empty() {
                    for (idx, arg) in resolved.into_iter().enumerate() {
                        let Some(arg) = arg else {
                            continue;
                        };
                        let Some(source_param) = readonly_alias_source(arg, aliases) else {
                            continue;
                        };
                        let callee_param_name = sig.params.get(idx).map(String::as_str);
                        let callee_param_ty = sig.param_types.get(idx).and_then(|ty| ty.as_ref());
                        let callee_param_readonly = callee_param_name.is_some_and(|param| {
                            sig.readonly_array_params.contains(param)
                                || readonly_params
                                    .get(name)
                                    .is_some_and(|params| params.contains(param))
                        });
                        if is_array_param_type(callee_param_ty) && !callee_param_readonly {
                            mutable_params.insert(source_param.to_owned());
                        }
                    }
                }
            }

            for arg in args {
                mark_readonly_param_expr_uses_as_mutable(
                    &arg.expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        }
        Expr::Call { args, .. }
        | Expr::ArrayLiteral { values: args, .. }
        | Expr::Tuple { values: args, .. } => {
            for arg in args {
                mark_readonly_param_expr_uses_as_mutable(
                    arg,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                lhs,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            mark_readonly_param_expr_uses_as_mutable(
                rhs,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                expr,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Expr::Index { index, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                index,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                mark_readonly_param_expr_uses_as_mutable(
                    start,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            if let Some(end) = end {
                mark_readonly_param_expr_uses_as_mutable(
                    end,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                &spec.size,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            if let Some(init) = init {
                for value in init {
                    mark_readonly_param_expr_uses_as_mutable(
                        value,
                        aliases,
                        fn_signatures,
                        readonly_params,
                        mutable_params,
                    );
                }
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn mark_readonly_param_stmt_uses_as_mutable(
    stmt: &Stmt,
    aliases: &mut HashMap<String, String>,
    fn_signatures: &HashMap<String, FnSignature>,
    readonly_params: &HashMap<String, HashSet<String>>,
    mutable_params: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => match target {
            AssignTarget::Var(name) => {
                mark_readonly_param_expr_uses_as_mutable(
                    expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
                if let Some(source_param) = readonly_alias_source(expr, aliases) {
                    aliases.insert(name.clone(), source_param.to_owned());
                } else {
                    aliases.remove(name);
                }
            }
            AssignTarget::Index { base, index } => {
                if let Some(source_param) = aliases.get(base) {
                    mutable_params.insert(source_param.clone());
                }
                mark_readonly_param_expr_uses_as_mutable(
                    index,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
                mark_readonly_param_expr_uses_as_mutable(
                    expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            AssignTarget::Slice { base, start, end } => {
                if let Some(source_param) = aliases.get(base) {
                    mutable_params.insert(source_param.clone());
                }
                if let Some(start) = start {
                    mark_readonly_param_expr_uses_as_mutable(
                        start,
                        aliases,
                        fn_signatures,
                        readonly_params,
                        mutable_params,
                    );
                }
                if let Some(end) = end {
                    mark_readonly_param_expr_uses_as_mutable(
                        end,
                        aliases,
                        fn_signatures,
                        readonly_params,
                        mutable_params,
                    );
                }
                mark_readonly_param_expr_uses_as_mutable(
                    expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            AssignTarget::Tuple(_) => {
                mark_readonly_param_expr_uses_as_mutable(
                    expr,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
        },
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                expr,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Stmt::Const { decl, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                &decl.expr,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            mark_readonly_param_expr_uses_as_mutable(
                cond,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            let mut then_aliases = aliases.clone();
            for stmt in then_branch {
                mark_readonly_param_stmt_uses_as_mutable(
                    stmt,
                    &mut then_aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            let mut else_aliases = aliases.clone();
            for stmt in else_branch {
                mark_readonly_param_stmt_uses_as_mutable(
                    stmt,
                    &mut else_aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            aliases.extend(then_aliases);
            aliases.extend(else_aliases);
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                mark_readonly_param_expr_uses_as_mutable(
                    step,
                    aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            mark_readonly_param_expr_uses_as_mutable(
                start,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            mark_readonly_param_expr_uses_as_mutable(
                end,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            let mut loop_aliases = aliases.clone();
            for stmt in body {
                mark_readonly_param_stmt_uses_as_mutable(
                    stmt,
                    &mut loop_aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            aliases.extend(loop_aliases);
        }
        Stmt::While { cond, body, .. } => {
            mark_readonly_param_expr_uses_as_mutable(
                cond,
                aliases,
                fn_signatures,
                readonly_params,
                mutable_params,
            );
            let mut loop_aliases = aliases.clone();
            for stmt in body {
                mark_readonly_param_stmt_uses_as_mutable(
                    stmt,
                    &mut loop_aliases,
                    fn_signatures,
                    readonly_params,
                    mutable_params,
                );
            }
            aliases.extend(loop_aliases);
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn infer_readonly_array_params_for_def(
    def: &FunctionDef,
    fn_signatures: &HashMap<String, FnSignature>,
    readonly_params: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    let candidates = initial_readonly_array_param_candidates(def);
    if candidates.is_empty() {
        return candidates;
    }
    let mut aliases = candidates
        .iter()
        .map(|name| (name.clone(), name.clone()))
        .collect::<HashMap<_, _>>();
    let mut mutable_params = HashSet::<String>::new();
    for stmt in &def.body {
        mark_readonly_param_stmt_uses_as_mutable(
            stmt,
            &mut aliases,
            fn_signatures,
            readonly_params,
            &mut mutable_params,
        );
    }
    candidates
        .into_iter()
        .filter(|param| !mutable_params.contains(param))
        .collect()
}

fn update_readonly_array_param_signatures(
    defs: &[FunctionDef],
    fn_signatures: &mut HashMap<String, FnSignature>,
) {
    let mut readonly_params = defs
        .iter()
        .map(|def| {
            (
                def.name.clone(),
                initial_readonly_array_param_candidates(def),
            )
        })
        .collect::<HashMap<_, _>>();

    loop {
        let mut changed = false;
        for def in defs {
            let inferred =
                infer_readonly_array_params_for_def(def, fn_signatures, &readonly_params);
            let entry = readonly_params.entry(def.name.clone()).or_default();
            if *entry != inferred {
                *entry = inferred;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for (name, params) in readonly_params {
        if let Some(sig) = fn_signatures.get_mut(&name) {
            sig.readonly_array_params = params;
        }
    }
}

fn def_has_concrete_param_contract(
    def: &FunctionDef,
    method_self_struct: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> bool {
    def.params.iter().enumerate().all(|(idx, param)| {
        if idx == 0 && method_self_struct.contains_key(&def.name) {
            return true;
        }
        match param.ty.as_ref() {
            Some(FnParamType::Primitive(_)) | Some(FnParamType::Tuple(_)) => true,
            Some(FnParamType::Struct(struct_name)) => {
                !def.type_params.contains(struct_name) && struct_defs.contains_key(struct_name)
            }
            Some(FnParamType::Buffer(buffer_ty)) => {
                matches!(buffer_ty.elem, BufferElemType::Primitive(_))
            }
            Some(FnParamType::Array(Some(_))) => true,
            Some(FnParamType::SizedArray { elem: Some(_), .. }) => true,
            Some(FnParamType::ArrayGeneric(struct_name)) => {
                !def.type_params.contains(struct_name) && struct_defs.contains_key(struct_name)
            }
            Some(FnParamType::SizedArray {
                generic_name: Some(struct_name),
                ..
            }) => !def.type_params.contains(struct_name) && struct_defs.contains_key(struct_name),
            Some(FnParamType::Array(None))
            | Some(FnParamType::BareBuffer)
            | Some(FnParamType::SizedArray { .. })
            | None => false,
        }
    })
}

fn try_indexed_proc_call_meta_in_def<'a>(
    expr: &'a Expr,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<(&'a str, &'a [CallArg], &'a str, &'a Expr)> {
    let Expr::UserCall {
        name,
        args,
        type_args: _,
        ..
    } = expr
    else {
        return None;
    };
    let proc_name = if let Some(step_proc) = name.strip_suffix(PROC_STEP_FN_SUFFIX) {
        step_proc
    } else if let Some((call_proc, out_idx_raw)) = name.rsplit_once(PROC_CALL_OUT_FN_PREFIX) {
        if out_idx_raw.parse::<usize>().is_ok() {
            call_proc
        } else {
            return None;
        }
    } else {
        return None;
    };
    let api = proc_api.get(proc_name)?;
    if !api.has_block {
        return None;
    }
    let self_arg = args.first()?;
    let Expr::Index { base, index, .. } = &self_arg.expr else {
        return None;
    };
    Some((proc_name, args, base.as_str(), index.as_ref()))
}

fn rewrite_stmt_for_def_proc_block_guards(
    stmt: Stmt,
    proc_api: &HashMap<String, ProcApi>,
    proc_block_active_symbols: &HashMap<String, String>,
) -> Vec<Stmt> {
    fn collect_guards(
        expr: &Expr,
        proc_api: &HashMap<String, ProcApi>,
        proc_block_active_symbols: &HashMap<String, String>,
        guards: &mut Vec<Stmt>,
    ) {
        match expr {
            Expr::Index { index, .. } => {
                collect_guards(index, proc_api, proc_block_active_symbols, guards)
            }
            Expr::Slice { start, end, .. } => {
                if let Some(start) = start {
                    collect_guards(start, proc_api, proc_block_active_symbols, guards);
                }
                if let Some(end) = end {
                    collect_guards(end, proc_api, proc_block_active_symbols, guards);
                }
            }
            Expr::ArrayCtor { spec, init, .. } => {
                collect_guards(&spec.size, proc_api, proc_block_active_symbols, guards);
                if let Some(values) = init {
                    for value in values {
                        collect_guards(value, proc_api, proc_block_active_symbols, guards);
                    }
                }
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                collect_guards(lhs, proc_api, proc_block_active_symbols, guards);
                collect_guards(rhs, proc_api, proc_block_active_symbols, guards);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    collect_guards(arg, proc_api, proc_block_active_symbols, guards);
                }
            }
            Expr::UserCall { args, .. } => {
                for arg in args {
                    collect_guards(&arg.expr, proc_api, proc_block_active_symbols, guards);
                }
                let Some((proc_name, args, array_base, index_expr)) =
                    try_indexed_proc_call_meta_in_def(expr, proc_api)
                else {
                    return;
                };
                let Some(api) = proc_api.get(proc_name) else {
                    return;
                };
                let Some(active_symbol) = proc_block_active_symbols.get(array_base).cloned() else {
                    return;
                };
                let input_slots = api.ins.iter().map(|port| port.slots.len()).sum::<usize>();
                let buffer_start = 1 + input_slots;
                let mut pre_args = Vec::<CallArg>::new();
                pre_args.push(CallArg {
                    name: None,
                    expr: Expr::Index {
                        loc: Default::default(),
                        base: array_base.to_owned(),
                        index: Box::new(index_expr.clone()),
                    },
                });
                pre_args.extend(args.iter().skip(buffer_start).cloned());
                guards.push(Stmt::If {
                    loc: Default::default(),
                    cond: Expr::UnaryNot {
                        loc: Default::default(),
                        expr: Box::new(Expr::Index {
                            loc: Default::default(),
                            base: active_symbol.clone(),
                            index: Box::new(index_expr.clone()),
                        }),
                    },
                    then_branch: vec![
                        Stmt::Expr {
                            loc: Default::default(),
                            expr: Expr::UserCall {
                                loc: Default::default(),
                                name: format!("{proc_name}{PROC_BLOCK_PRE_FN_SUFFIX}"),
                                type_args: Vec::new(),
                                args: pre_args,
                            },
                        },
                        Stmt::Assign {
                            loc: Default::default(),
                            target_loc: Default::default(),
                            target: AssignTarget::Index {
                                base: active_symbol,
                                index: index_expr.clone(),
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            typed_decl_ty_loc: Default::default(),
                            expr: Expr::bool(true),
                        },
                    ],
                    else_branch: Vec::new(),
                });
            }
            Expr::Cast { expr: inner, .. }
            | Expr::UnaryNot { expr: inner, .. }
            | Expr::UnaryBitNot { expr: inner, .. } => {
                collect_guards(inner, proc_api, proc_block_active_symbols, guards)
            }
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                for value in values {
                    collect_guards(value, proc_api, proc_block_active_symbols, guards);
                }
            }
            Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        }
    }

    let mut guards = Vec::<Stmt>::new();
    match &stmt {
        Stmt::Expr { expr, .. } | Stmt::Assign { expr, .. } | Stmt::Return { expr, .. } => {
            collect_guards(expr, proc_api, proc_block_active_symbols, &mut guards);
        }
        Stmt::If { cond, .. } | Stmt::While { cond, .. } => {
            collect_guards(cond, proc_api, proc_block_active_symbols, &mut guards);
        }
        Stmt::For {
            start, end, step, ..
        } => {
            collect_guards(start, proc_api, proc_block_active_symbols, &mut guards);
            collect_guards(end, proc_api, proc_block_active_symbols, &mut guards);
            if let Some(step) = step {
                collect_guards(step, proc_api, proc_block_active_symbols, &mut guards);
            }
        }
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
    if guards.is_empty() {
        return match stmt {
            Stmt::If {
                loc,
                cond,
                then_branch,
                else_branch,
            } => {
                let mut rewritten_then = Vec::<Stmt>::new();
                for nested in then_branch {
                    rewritten_then.extend(rewrite_stmt_for_def_proc_block_guards(
                        nested,
                        proc_api,
                        proc_block_active_symbols,
                    ));
                }
                let mut rewritten_else = Vec::<Stmt>::new();
                for nested in else_branch {
                    rewritten_else.extend(rewrite_stmt_for_def_proc_block_guards(
                        nested,
                        proc_api,
                        proc_block_active_symbols,
                    ));
                }
                vec![Stmt::If {
                    loc,
                    cond,
                    then_branch: rewritten_then,
                    else_branch: rewritten_else,
                }]
            }
            Stmt::For {
                loc,
                var,
                start,
                end,
                end_inclusive,
                step,
                body,
            } => {
                let mut rewritten_body = Vec::<Stmt>::new();
                for nested in body {
                    rewritten_body.extend(rewrite_stmt_for_def_proc_block_guards(
                        nested,
                        proc_api,
                        proc_block_active_symbols,
                    ));
                }
                vec![Stmt::For {
                    loc,
                    var,
                    start,
                    end,
                    end_inclusive,
                    step,
                    body: rewritten_body,
                }]
            }
            Stmt::While { loc, cond, body } => {
                let mut rewritten_body = Vec::<Stmt>::new();
                for nested in body {
                    rewritten_body.extend(rewrite_stmt_for_def_proc_block_guards(
                        nested,
                        proc_api,
                        proc_block_active_symbols,
                    ));
                }
                vec![Stmt::While {
                    loc,
                    cond,
                    body: rewritten_body,
                }]
            }
            other => vec![other],
        };
    }
    let mut rewritten = guards;
    match stmt {
        Stmt::If {
            loc,
            cond,
            then_branch,
            else_branch,
        } => {
            let mut rewritten_then = Vec::<Stmt>::new();
            for nested in then_branch {
                rewritten_then.extend(rewrite_stmt_for_def_proc_block_guards(
                    nested,
                    proc_api,
                    proc_block_active_symbols,
                ));
            }
            let mut rewritten_else = Vec::<Stmt>::new();
            for nested in else_branch {
                rewritten_else.extend(rewrite_stmt_for_def_proc_block_guards(
                    nested,
                    proc_api,
                    proc_block_active_symbols,
                ));
            }
            rewritten.push(Stmt::If {
                loc,
                cond,
                then_branch: rewritten_then,
                else_branch: rewritten_else,
            });
        }
        Stmt::For {
            loc,
            var,
            start,
            end,
            end_inclusive,
            step,
            body,
        } => {
            let mut rewritten_body = Vec::<Stmt>::new();
            for nested in body {
                rewritten_body.extend(rewrite_stmt_for_def_proc_block_guards(
                    nested,
                    proc_api,
                    proc_block_active_symbols,
                ));
            }
            rewritten.push(Stmt::For {
                loc,
                var,
                start,
                end,
                end_inclusive,
                step,
                body: rewritten_body,
            });
        }
        Stmt::While { loc, cond, body } => {
            let mut rewritten_body = Vec::<Stmt>::new();
            for nested in body {
                rewritten_body.extend(rewrite_stmt_for_def_proc_block_guards(
                    nested,
                    proc_api,
                    proc_block_active_symbols,
                ));
            }
            rewritten.push(Stmt::While {
                loc,
                cond,
                body: rewritten_body,
            });
        }
        other => rewritten.push(other),
    }
    rewritten
}

fn typed_def_owner_proc_param_index_for_symbol(
    symbol: &str,
    def: &TypedFunction,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<usize> {
    let root = symbol.split('.').next().unwrap_or(symbol);
    let param_idx = def.params.iter().position(|param| param == root)?;
    let TypedFnParam::Struct { struct_name } = def.param_kinds.get(param_idx)? else {
        return None;
    };
    let api = proc_api.get(struct_name)?;
    if api.has_block {
        Some(param_idx)
    } else {
        None
    }
}

fn typed_def_owner_proc_param_index_for_expr(
    expr: &Expr,
    def: &TypedFunction,
    proc_api: &HashMap<String, ProcApi>,
) -> Option<usize> {
    match expr {
        Expr::Var { name, .. } => typed_def_owner_proc_param_index_for_symbol(name, def, proc_api),
        Expr::Index { base, .. } => {
            typed_def_owner_proc_param_index_for_symbol(base, def, proc_api)
        }
        _ => None,
    }
}

fn collect_typed_def_owner_proc_hook_params_from_expr(
    expr: &Expr,
    def: &TypedFunction,
    def_map: &HashMap<String, &TypedFunction>,
    known_requirements: &HashMap<String, HashSet<usize>>,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashSet<usize>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    value,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
        }
        Expr::Index { index, .. } => {
            collect_typed_def_owner_proc_hook_params_from_expr(
                index,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    start,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            if let Some(end) = end {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    end,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_typed_def_owner_proc_hook_params_from_expr(
                &spec.size,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
            if let Some(values) = init {
                for value in values {
                    collect_typed_def_owner_proc_hook_params_from_expr(
                        value,
                        def,
                        def_map,
                        known_requirements,
                        proc_api,
                        out,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_typed_def_owner_proc_hook_params_from_expr(
                lhs,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
            collect_typed_def_owner_proc_hook_params_from_expr(
                rhs,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    arg,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    &arg.expr,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            if let Some((_proc_name, _args, array_base, _index_expr)) =
                try_indexed_proc_call_meta_in_def(expr, proc_api)
            {
                if let Some(param_idx) =
                    typed_def_owner_proc_param_index_for_symbol(array_base, def, proc_api)
                {
                    out.insert(param_idx);
                }
            }
            let Some(callee) = def_map.get(name) else {
                return;
            };
            let Some(required_params) = known_requirements.get(name) else {
                return;
            };
            if required_params.is_empty() {
                return;
            }
            let mut call_errors = Vec::new();
            let resolved = resolve_call_args(
                args,
                &callee.params,
                &callee.param_defaults,
                false,
                false,
                &format!("call '{}(...)'", callee.name),
                &mut call_errors,
            );
            for required_idx in required_params {
                let Some(Some(arg_expr)) = resolved.get(*required_idx) else {
                    continue;
                };
                if let Some(param_idx) =
                    typed_def_owner_proc_param_index_for_expr(arg_expr, def, proc_api)
                {
                    out.insert(param_idx);
                }
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            collect_typed_def_owner_proc_hook_params_from_expr(
                inner,
                def,
                def_map,
                known_requirements,
                proc_api,
                out,
            );
        }
    }
}

fn collect_typed_def_owner_proc_hook_params_from_stmts(
    stmts: &[Stmt],
    def: &TypedFunction,
    def_map: &HashMap<String, &TypedFunction>,
    known_requirements: &HashMap<String, HashSet<usize>>,
    proc_api: &HashMap<String, ProcApi>,
    out: &mut HashSet<usize>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    expr,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    cond,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                collect_typed_def_owner_proc_hook_params_from_stmts(
                    then_branch,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                collect_typed_def_owner_proc_hook_params_from_stmts(
                    else_branch,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    start,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                collect_typed_def_owner_proc_hook_params_from_expr(
                    end,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                if let Some(step) = step {
                    collect_typed_def_owner_proc_hook_params_from_expr(
                        step,
                        def,
                        def_map,
                        known_requirements,
                        proc_api,
                        out,
                    );
                }
                collect_typed_def_owner_proc_hook_params_from_stmts(
                    body,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
            Stmt::While { cond, body, .. } => {
                collect_typed_def_owner_proc_hook_params_from_expr(
                    cond,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
                collect_typed_def_owner_proc_hook_params_from_stmts(
                    body,
                    def,
                    def_map,
                    known_requirements,
                    proc_api,
                    out,
                );
            }
        }
    }
}

fn collect_typed_def_owner_proc_hook_requirements(
    defs: &[TypedFunction],
    proc_api: &HashMap<String, ProcApi>,
) -> HashMap<String, HashSet<usize>> {
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let mut requirements = HashMap::<String, HashSet<usize>>::new();

    loop {
        let mut changed = false;
        for def in defs {
            let mut direct = HashSet::<usize>::new();
            collect_typed_def_owner_proc_hook_params_from_stmts(
                &def.body,
                def,
                &def_map,
                &requirements,
                proc_api,
                &mut direct,
            );
            let entry = requirements.entry(def.name.clone()).or_default();
            for param_idx in direct {
                if entry.insert(param_idx) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    requirements
}

fn expr_global_proc_instance_name(
    expr: &Expr,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
) -> Option<String> {
    match expr {
        Expr::Var { name, .. } if global_proc_instances.contains_key(name) => Some(name.clone()),
        Expr::Index { base, index, .. } => {
            let Expr::Int { value, .. } = index.as_ref() else {
                return None;
            };
            let slot_name = format!("{base}[{value}]");
            global_proc_instances
                .contains_key(&slot_name)
                .then_some(slot_name)
        }
        _ => None,
    }
}

fn stmt_has_proc_block_hook_for_instance(
    stmt: &Stmt,
    proc_name: &str,
    suffix: &str,
    instance_name: &str,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
) -> bool {
    let Stmt::Expr {
        expr:
            Expr::UserCall {
                name,
                args,
                type_args: _,
                ..
            },
        ..
    } = stmt
    else {
        return false;
    };
    if name != &format!("{proc_name}{suffix}") {
        return false;
    }
    let Some(self_arg) = args.first() else {
        return false;
    };
    expr_global_proc_instance_name(&self_arg.expr, global_proc_instances).as_deref()
        == Some(instance_name)
}

fn collect_sample_owner_proc_hook_instances_from_expr(
    expr: &Expr,
    def_map: &HashMap<String, &TypedFunction>,
    requirements: &HashMap<String, HashSet<usize>>,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
    out: &mut HashSet<String>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_sample_owner_proc_hook_instances_from_expr(
                    value,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
        }
        Expr::Index { index, .. } => {
            collect_sample_owner_proc_hook_instances_from_expr(
                index,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_sample_owner_proc_hook_instances_from_expr(
                    start,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            if let Some(end) = end {
                collect_sample_owner_proc_hook_instances_from_expr(
                    end,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_sample_owner_proc_hook_instances_from_expr(
                &spec.size,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
            if let Some(values) = init {
                for value in values {
                    collect_sample_owner_proc_hook_instances_from_expr(
                        value,
                        def_map,
                        requirements,
                        global_proc_instances,
                        out,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_sample_owner_proc_hook_instances_from_expr(
                lhs,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
            collect_sample_owner_proc_hook_instances_from_expr(
                rhs,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_sample_owner_proc_hook_instances_from_expr(
                    arg,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                collect_sample_owner_proc_hook_instances_from_expr(
                    &arg.expr,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            let Some(callee) = def_map.get(name) else {
                return;
            };
            let Some(required_params) = requirements.get(name) else {
                return;
            };
            if required_params.is_empty() {
                return;
            }
            let mut call_errors = Vec::new();
            let resolved = resolve_call_args(
                args,
                &callee.params,
                &callee.param_defaults,
                false,
                false,
                &format!("call '{}(...)'", callee.name),
                &mut call_errors,
            );
            for required_idx in required_params {
                let Some(Some(arg_expr)) = resolved.get(*required_idx) else {
                    continue;
                };
                if let Some(instance_name) =
                    expr_global_proc_instance_name(arg_expr, global_proc_instances)
                {
                    out.insert(instance_name);
                }
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            collect_sample_owner_proc_hook_instances_from_expr(
                inner,
                def_map,
                requirements,
                global_proc_instances,
                out,
            );
        }
    }
}

fn collect_sample_owner_proc_hook_instances_from_stmts(
    stmts: &[Stmt],
    def_map: &HashMap<String, &TypedFunction>,
    requirements: &HashMap<String, HashSet<usize>>,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
    out: &mut HashSet<String>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                collect_sample_owner_proc_hook_instances_from_expr(
                    expr,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_sample_owner_proc_hook_instances_from_expr(
                    cond,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                collect_sample_owner_proc_hook_instances_from_stmts(
                    then_branch,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                collect_sample_owner_proc_hook_instances_from_stmts(
                    else_branch,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_sample_owner_proc_hook_instances_from_expr(
                    start,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                collect_sample_owner_proc_hook_instances_from_expr(
                    end,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                if let Some(step) = step {
                    collect_sample_owner_proc_hook_instances_from_expr(
                        step,
                        def_map,
                        requirements,
                        global_proc_instances,
                        out,
                    );
                }
                collect_sample_owner_proc_hook_instances_from_stmts(
                    body,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
            Stmt::While { cond, body, .. } => {
                collect_sample_owner_proc_hook_instances_from_expr(
                    cond,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
                collect_sample_owner_proc_hook_instances_from_stmts(
                    body,
                    def_map,
                    requirements,
                    global_proc_instances,
                    out,
                );
            }
        }
    }
}

fn inject_sample_def_owner_proc_block_hooks(
    sample: &[Stmt],
    block_pre: &mut Vec<Stmt>,
    block_post: &mut Vec<Stmt>,
    defs: &[TypedFunction],
    proc_api: &HashMap<String, ProcApi>,
    global_proc_instances: &HashMap<String, ProcCallInstance>,
    global_proc_array_slots: &HashMap<String, Vec<String>>,
    errors: &mut Vec<Diagnostic>,
) {
    if defs.is_empty() || global_proc_instances.is_empty() {
        return;
    }
    let requirements = collect_typed_def_owner_proc_hook_requirements(defs, proc_api);
    if requirements.values().all(HashSet::is_empty) {
        return;
    }
    let def_map = defs
        .iter()
        .map(|def| (def.name.clone(), def))
        .collect::<HashMap<_, _>>();
    let mut instance_names = HashSet::<String>::new();
    collect_sample_owner_proc_hook_instances_from_stmts(
        sample,
        &def_map,
        &requirements,
        global_proc_instances,
        &mut instance_names,
    );
    if instance_names.is_empty() {
        return;
    }

    let mut ordered_instances = instance_names.into_iter().collect::<Vec<_>>();
    ordered_instances.sort();
    let mut injected_pre = Vec::<Stmt>::new();
    let mut injected_post = Vec::<Stmt>::new();
    for instance_name in ordered_instances {
        let Some(instance) = global_proc_instances.get(&instance_name) else {
            continue;
        };
        let Some(api) = proc_api.get(&instance.proc_name) else {
            continue;
        };
        if !api.has_block {
            continue;
        }
        let has_existing_pre = block_pre.iter().any(|stmt| {
            stmt_has_proc_block_hook_for_instance(
                stmt,
                &instance.proc_name,
                PROC_BLOCK_PRE_FN_SUFFIX,
                &instance_name,
                global_proc_instances,
            )
        });
        if !has_existing_pre {
            let mut pre_args = vec![CallArg {
                name: None,
                expr: proc_instance_self_expr(&instance_name, global_proc_array_slots),
            }];
            pre_args.extend(expand_proc_buffer_call_args(
                instance,
                api,
                &instance_name,
                errors,
            ));
            injected_pre.push(Stmt::Expr {
                loc: Default::default(),
                expr: Expr::UserCall {
                    loc: Default::default(),
                    name: format!("{}{}", instance.proc_name, PROC_BLOCK_PRE_FN_SUFFIX),
                    type_args: Vec::new(),
                    args: pre_args,
                },
            });
        }

        let has_existing_post = block_post.iter().any(|stmt| {
            stmt_has_proc_block_hook_for_instance(
                stmt,
                &instance.proc_name,
                PROC_BLOCK_POST_FN_SUFFIX,
                &instance_name,
                global_proc_instances,
            )
        });
        if !has_existing_post {
            let mut post_args = vec![CallArg {
                name: None,
                expr: proc_instance_self_expr(&instance_name, global_proc_array_slots),
            }];
            post_args.extend(expand_proc_buffer_call_args(
                instance,
                api,
                &instance_name,
                errors,
            ));
            injected_post.push(Stmt::Expr {
                loc: Default::default(),
                expr: Expr::UserCall {
                    loc: Default::default(),
                    name: format!("{}{}", instance.proc_name, PROC_BLOCK_POST_FN_SUFFIX),
                    type_args: Vec::new(),
                    args: post_args,
                },
            });
        }
    }
    if !injected_pre.is_empty() {
        let mut new_block_pre = injected_pre;
        new_block_pre.append(block_pre);
        *block_pre = new_block_pre;
    }
    if !injected_post.is_empty() {
        block_post.extend(injected_post);
    }
}

fn seed_called_typed_defs_from_stmts(
    stmts: &[Stmt],
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    for stmt in stmts {
        collect_called_typed_defs_in_stmt(stmt, def_names, pending, seen_pending);
    }
}

fn collect_called_typed_defs_in_stmt(
    stmt: &Stmt,
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    match stmt {
        Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            collect_called_typed_defs_in_expr(expr, def_names, pending, seen_pending);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_called_typed_defs_in_expr(cond, def_names, pending, seen_pending);
            seed_called_typed_defs_from_stmts(then_branch, def_names, pending, seen_pending);
            seed_called_typed_defs_from_stmts(else_branch, def_names, pending, seen_pending);
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            collect_called_typed_defs_in_expr(start, def_names, pending, seen_pending);
            collect_called_typed_defs_in_expr(end, def_names, pending, seen_pending);
            if let Some(step) = step {
                collect_called_typed_defs_in_expr(step, def_names, pending, seen_pending);
            }
            seed_called_typed_defs_from_stmts(body, def_names, pending, seen_pending);
        }
        Stmt::While { cond, body, .. } => {
            collect_called_typed_defs_in_expr(cond, def_names, pending, seen_pending);
            seed_called_typed_defs_from_stmts(body, def_names, pending, seen_pending);
        }
    }
}

fn collect_called_typed_defs_in_expr(
    expr: &Expr,
    def_names: &HashSet<String>,
    pending: &mut Vec<String>,
    seen_pending: &mut HashSet<String>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_called_typed_defs_in_expr(value, def_names, pending, seen_pending);
            }
        }
        Expr::Index { index, .. } => {
            collect_called_typed_defs_in_expr(index, def_names, pending, seen_pending);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                collect_called_typed_defs_in_expr(start, def_names, pending, seen_pending);
            }
            if let Some(end) = end {
                collect_called_typed_defs_in_expr(end, def_names, pending, seen_pending);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_called_typed_defs_in_expr(&spec.size, def_names, pending, seen_pending);
            if let Some(values) = init {
                for value in values {
                    collect_called_typed_defs_in_expr(value, def_names, pending, seen_pending);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_called_typed_defs_in_expr(lhs, def_names, pending, seen_pending);
            collect_called_typed_defs_in_expr(rhs, def_names, pending, seen_pending);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_called_typed_defs_in_expr(arg, def_names, pending, seen_pending);
            }
        }
        Expr::UserCall { name, args, .. } => {
            if def_names.contains(name) && seen_pending.insert(name.clone()) {
                pending.push(name.clone());
            }
            for arg in args {
                collect_called_typed_defs_in_expr(&arg.expr, def_names, pending, seen_pending);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_called_typed_defs_in_expr(expr, def_names, pending, seen_pending);
        }
    }
}
