use super::*;
use crate::processor_lowering::validate_generic_proc_template_forwarded_type_args;

#[derive(Debug, Clone)]
struct NamespaceTemplateRecord {
    decl: NamespaceDecl,
    captured_artifacts: SemanticConstArtifacts,
    captured_template_consts: HashMap<String, Expr>,
}

#[derive(Debug, Clone)]
struct NamespaceAliasRecord {
    declared_ns: String,
    target: Vec<NamespaceRefSegment>,
}

#[derive(Debug, Clone)]
struct UseBinding {
    target: String,
}

#[derive(Debug, Clone)]
struct NamespaceUseBinding {
    target: String,
}

#[derive(Debug, Clone, Default)]
struct UseScope {
    symbols: HashMap<String, Vec<UseBinding>>,
    namespaces: Vec<NamespaceUseBinding>,
}

#[derive(Debug, Default)]
struct NamespaceFlattenState {
    templates: HashMap<String, NamespaceTemplateRecord>,
    aliases: HashMap<String, NamespaceAliasRecord>,
    private_aliases: HashMap<(String, String), NamespaceAliasRecord>,
    public_uses: HashMap<String, UseScope>,
    private_uses: HashMap<(String, String), UseScope>,
    global_value_names: HashSet<String>,
    members: HashSet<String>,
    const_array_names: HashSet<String>,
    scalar_const_names: HashSet<String>,
    const_symbols: HashSet<String>,
    instantiations: HashMap<String, String>,
    next_instantiation_id: u64,
    artifacts: SemanticConstArtifacts,
}

pub(super) fn flatten_namespaces_for_semantics(
    program: &mut Program,
    options: AnalysisOptions,
) -> Result<(), Vec<Diagnostic>> {
    let mut state = NamespaceFlattenState::default();
    let mut errors = Vec::<Diagnostic>::new();
    let mut out = Vec::<Block>::new();
    let template_consts = HashMap::<String, Expr>::new();

    state.global_value_names = collect_global_value_names(&program.blocks);

    for block in std::mem::take(&mut program.blocks) {
        process_top_level_block(
            block,
            &template_consts,
            options,
            &mut state,
            &mut out,
            &mut errors,
        );
    }

    if errors.is_empty() {
        program.blocks = out;
        Ok(())
    } else {
        Err(errors)
    }
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

fn namespace_of_symbol(name: &str) -> String {
    name.rsplit_once("::")
        .map(|(ns, _)| ns.to_owned())
        .unwrap_or_default()
}

fn split_namespace_parent_leaf(name: &str) -> (&str, &str) {
    if let Some((parent, leaf)) = name.rsplit_once("::") {
        (parent, leaf)
    } else {
        ("", name)
    }
}

fn looks_like_namespace_ref(name: &str) -> bool {
    name.contains("::") && !name.contains('.')
}

fn split_named_type_base_and_suffix(name: &str) -> (&str, &str) {
    if !name.ends_with('>') {
        return (name, "");
    }

    let mut depth = 0usize;
    for (idx, ch) in name.char_indices().rev() {
        match ch {
            '>' => depth += 1,
            '<' => {
                if depth == 0 {
                    return (name, "");
                }
                depth -= 1;
                if depth == 0 {
                    let base = name[..idx].trim_end();
                    let suffix = name[idx..].trim_start();
                    if base.is_empty() {
                        return (name, "");
                    }
                    return (base, suffix);
                }
            }
            _ => {}
        }
    }

    (name, "")
}

fn format_call_args_as_type_suffix(args: &[NamespaceCallArg]) -> String {
    let parts = args
        .iter()
        .map(|arg| match &arg.expr {
            Expr::Var { name, .. } => name.clone(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>();
    format!("<{}>", parts.join(", "))
}

fn namespace_segments_key(segments: &[NamespaceRefSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join("::")
}

fn strip_type_args_from_path(path: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for ch in path.trim().chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ' ' | '\t' | '\r' if depth > 0 => {}
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
struct RewriteNameScope {
    names: HashSet<String>,
}

impl RewriteNameScope {
    fn from_names(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            names: names.into_iter().collect(),
        }
    }

    fn contains_value_name(&self, name: &str) -> bool {
        if name.is_empty() || name.contains("::") || name.contains('.') {
            return false;
        }
        let (base, _) = split_named_type_base_and_suffix(name);
        self.names.contains(base)
    }

    fn insert_plain(&mut self, name: impl Into<String>) {
        let name = name.into();
        if !name.is_empty() && !name.contains("::") && !name.contains('.') {
            self.names.insert(name);
        }
    }

    fn extend(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            self.insert_plain(name);
        }
    }
}

fn assignment_target_plain_names(target: &AssignTarget) -> Vec<String> {
    match target {
        AssignTarget::Var(name) => vec![name.clone()],
        AssignTarget::Tuple(names) => names.clone(),
        AssignTarget::Index { .. } | AssignTarget::Slice { .. } => Vec::new(),
    }
}

fn collect_top_level_assignment_target_names(stmts: &[Stmt], out: &mut HashSet<String>) {
    for stmt in stmts {
        if let Stmt::Assign { target, .. } = stmt {
            for name in assignment_target_plain_names(target) {
                if !name.contains("::") && !name.contains('.') {
                    out.insert(name);
                }
            }
        }
    }
}

fn collect_global_value_names(blocks: &[Block]) -> HashSet<String> {
    let mut names = HashSet::<String>::new();
    for block in blocks {
        match block {
            Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
                names.extend(ports.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Params(params) => {
                names.extend(params.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Buffers(buffers) => {
                names.extend(buffers.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Events(events) => {
                names.extend(events.events.iter().map(|event| event.name.clone()));
            }
            Block::Init(init) => {
                collect_top_level_assignment_target_names(&init.body, &mut names);
            }
            Block::Block(exec) => {
                collect_top_level_assignment_target_names(&exec.pre, &mut names);
                collect_top_level_assignment_target_names(&exec.post, &mut names);
            }
            Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_)
            | Block::Const(_)
            | Block::Assert(_)
            | Block::Proc(_)
            | Block::Struct(_)
            | Block::Def(_)
            | Block::Sample(_)
            | Block::Graph(_) => {}
        }
    }
    names
}

fn proc_value_name_scope(proc: &ProcessorDef) -> RewriteNameScope {
    let mut scope = RewriteNameScope::default();
    scope.extend(proc.type_params.iter().cloned());
    scope.extend(proc.consts.iter().map(|decl| decl.name.clone()));
    scope.extend(proc.ins.iter().map(|decl| decl.name.clone()));
    scope.extend(proc.outs.iter().map(|decl| decl.name.clone()));
    scope.extend(proc.params.iter().map(|decl| decl.name.clone()));
    scope.extend(proc.buffers.iter().map(|decl| decl.name.clone()));
    scope.extend(proc.events.iter().map(|event| event.name.clone()));
    scope.extend(proc.local_defs.iter().map(|def| def.name.clone()));
    collect_top_level_assignment_target_names(&proc.init.body, &mut scope.names);
    collect_top_level_assignment_target_names(&proc.block_pre, &mut scope.names);
    scope
}

fn typed_const_value_key(value: TypedConstValue) -> String {
    match value {
        TypedConstValue::F32(v) => format!("f32({v})"),
        TypedConstValue::F64(v) => format!("f64({v})"),
        TypedConstValue::I32(v) => format!("i32({v})"),
        TypedConstValue::I64(v) => format!("i64({v})"),
        TypedConstValue::Bool(v) => format!("bool({v})"),
    }
}

fn register_member_for_item(
    item: &NamespaceItem,
    namespace: &str,
    state: &mut NamespaceFlattenState,
) {
    match item {
        NamespaceItem::Struct(s) => {
            state.members.insert(namespace_join(namespace, &s.name));
        }
        NamespaceItem::Def(d) => {
            state.members.insert(namespace_join(namespace, &d.name));
        }
        NamespaceItem::Proc(p) => {
            state.members.insert(namespace_join(namespace, &p.name));
        }
        NamespaceItem::Const(decl) => {
            let full_name = namespace_join(namespace, &decl.name);
            state.members.insert(full_name.clone());
            if is_const_array_decl(decl) {
                state.const_array_names.insert(full_name);
            } else {
                state.scalar_const_names.insert(full_name);
            }
        }
        NamespaceItem::Assert(_)
        | NamespaceItem::Namespace(_)
        | NamespaceItem::Alias(_)
        | NamespaceItem::Use(_) => {}
    }
}

fn register_const_def_artifact(
    def: &FunctionDef,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    if !state.const_symbols.insert(def.name.clone()) {
        errors.push(Diagnostic::semantic_span(
            format!("duplicate const symbol '{}'", def.name),
            def.loc,
        ));
        return;
    }
    state
        .artifacts
        .const_def_order
        .insert(def.name.clone(), state.artifacts.const_def_order.len());
    state
        .artifacts
        .const_defs
        .insert(def.name.clone(), def.clone());
}

fn register_const_decl_artifact(
    decl: &ConstDecl,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    if !state.const_symbols.insert(decl.name.clone()) {
        errors.push(Diagnostic::semantic_span(
            format!("duplicate const symbol '{}'", decl.name),
            decl.loc.as_ref(),
        ));
        return;
    }
    let force_const_array = is_const_array_decl(decl)
        || (decl.ty.is_none()
            && is_known_const_array_initializer(
                &decl.expr,
                &state.artifacts.const_values,
                &state.artifacts.const_defs,
            ));
    if force_const_array {
        if let Some(array) = coerce_const_array(
            decl,
            options,
            &state.artifacts.const_values,
            &state.artifacts.const_defs,
            &state.artifacts.const_def_order,
            errors,
        ) {
            record_const_array_artifact(&mut state.artifacts, array);
        }
    } else {
        let inferred_const_array = if decl.ty.is_none() {
            let mut probe_errors = Vec::new();
            coerce_const_array(
                decl,
                options,
                &state.artifacts.const_values,
                &state.artifacts.const_defs,
                &state.artifacts.const_def_order,
                &mut probe_errors,
            )
        } else {
            None
        };
        if let Some(array) = inferred_const_array {
            record_const_array_artifact(&mut state.artifacts, array);
        } else if let Some(value) = coerce_const_scalar(
            decl,
            options,
            &state.artifacts.const_values,
            &state.artifacts.const_defs,
            &state.artifacts.const_def_order,
            errors,
        ) {
            state
                .artifacts
                .const_values
                .insert(decl.name.clone(), ConstValue::Scalar(value));
        }
    }
}

fn register_artifact_for_block(
    block: &Block,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Def(def) if def.is_const => register_const_def_artifact(def, state, errors),
        Block::Const(decl) => register_const_decl_artifact(decl, options, state, errors),
        _ => {}
    }
}

fn merge_generated_namespace_artifacts(
    target: &mut SemanticConstArtifacts,
    emitted: SemanticConstArtifacts,
    namespace: &str,
) {
    let prefix = format!("{namespace}::");
    let mut generated_defs = emitted
        .const_defs
        .into_iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .collect::<Vec<_>>();
    generated_defs.sort_by_key(|(name, _)| {
        emitted
            .const_def_order
            .get(name)
            .copied()
            .unwrap_or(usize::MAX)
    });
    for (name, def) in generated_defs {
        target
            .const_def_order
            .insert(name.clone(), target.const_def_order.len());
        target.const_defs.insert(name, def);
    }

    for (name, value) in emitted.const_values {
        if name.starts_with(&prefix) && matches!(value, ConstValue::Scalar(_)) {
            target.const_values.insert(name, value);
        }
    }

    for array in emitted
        .const_arrays
        .into_iter()
        .filter(|array| array.name.starts_with(&prefix))
    {
        record_const_array_artifact(target, array);
    }
}

fn process_top_level_block(
    block: Block,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    out: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Namespace(decl) => {
            process_namespace_decl(decl, "", template_consts, options, state, out, errors);
        }
        Block::NamespaceAlias(alias) => {
            register_namespace_alias("", alias, state, errors);
        }
        Block::Use(use_decl) => {
            register_use_decl("", &use_decl, template_consts, options, state, out, errors);
        }
        mut block => {
            register_top_level_member_for_block(&block, state);
            let current_ns = match &block {
                Block::Struct(s) => namespace_of_symbol(&s.name),
                Block::Def(d) => namespace_of_symbol(&d.name),
                Block::Proc(p) => namespace_of_symbol(&p.name),
                _ => String::new(),
            };
            let mut generated = Vec::<Block>::new();
            rewrite_block_namespace_refs(
                &mut block,
                &current_ns,
                template_consts,
                options,
                state,
                &mut generated,
                errors,
            );
            out.extend(generated);
            register_artifact_for_block(&block, options, state, errors);
            out.push(block);
        }
    }
}

fn process_namespace_decl(
    decl: NamespaceDecl,
    parent_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    out: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    let full_ns = namespace_join(parent_ns, &decl.name);
    if decl.params.is_empty() {
        emit_namespace_items(
            &decl.items,
            &full_ns,
            template_consts,
            options,
            state,
            out,
            errors,
        );
    } else {
        if state.templates.contains_key(&full_ns) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate namespace template '{full_ns}'"),
                decl.loc.as_ref(),
            ));
            return;
        }
        let validation_scope = namespace_template_validation_scope(&decl, None);
        validate_namespace_template_proc_refs(&decl, &full_ns, state, &validation_scope, errors);
        state.templates.insert(
            full_ns,
            NamespaceTemplateRecord {
                decl,
                captured_artifacts: state.artifacts.clone(),
                captured_template_consts: template_consts.clone(),
            },
        );
    }
}

#[derive(Debug, Clone, Default)]
struct NamespaceTemplateValidationScope {
    static_names: HashSet<String>,
}

fn namespace_template_validation_scope(
    decl: &NamespaceDecl,
    parent: Option<&NamespaceTemplateValidationScope>,
) -> NamespaceTemplateValidationScope {
    let mut scope = parent.cloned().unwrap_or_default();
    scope
        .static_names
        .extend(decl.params.iter().map(|param| param.name.clone()));
    for item in &decl.items {
        match item {
            NamespaceItem::Const(decl) => {
                scope.static_names.insert(decl.name.clone());
            }
            NamespaceItem::Def(def) if def.is_const => {
                scope.static_names.insert(def.name.clone());
            }
            NamespaceItem::Assert(_)
            | NamespaceItem::Struct(_)
            | NamespaceItem::Def(_)
            | NamespaceItem::Proc(_)
            | NamespaceItem::Namespace(_)
            | NamespaceItem::Alias(_)
            | NamespaceItem::Use(_) => {}
        }
    }
    scope
}

fn validate_namespace_template_proc_refs(
    decl: &NamespaceDecl,
    namespace: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    errors: &mut Vec<Diagnostic>,
) {
    for item in &decl.items {
        match item {
            NamespaceItem::Proc(proc) => {
                let mut proc = proc.clone();
                proc.name = namespace_join(namespace, &proc.name);
                validate_generic_proc_template_forwarded_type_args(&proc, errors);
                validate_template_proc_refs(&proc, namespace, state, scope, errors);
            }
            NamespaceItem::Namespace(child) => {
                let child_namespace = namespace_join(namespace, &child.name);
                let child_scope = namespace_template_validation_scope(child, Some(scope));
                validate_namespace_template_proc_refs(
                    child,
                    &child_namespace,
                    state,
                    &child_scope,
                    errors,
                );
            }
            NamespaceItem::Const(decl) => {
                validate_template_expr_refs(
                    &decl.expr,
                    namespace,
                    state,
                    scope,
                    "namespace template const",
                    errors,
                );
            }
            NamespaceItem::Assert(assert_decl) => {
                validate_template_static_expr_refs(
                    &assert_decl.expr,
                    namespace,
                    state,
                    scope,
                    "namespace template assert",
                    errors,
                );
            }
            NamespaceItem::Struct(struct_def) => {
                for field in &struct_def.fields {
                    if let Some(default) = &field.default {
                        validate_template_expr_refs(
                            default,
                            namespace,
                            state,
                            scope,
                            "namespace template struct field default",
                            errors,
                        );
                    }
                }
                for method in &struct_def.methods {
                    let mut method_scope = scope.clone();
                    method_scope
                        .static_names
                        .extend(method.type_params.iter().cloned());
                    validate_template_stmt_list_refs(
                        &method.body,
                        namespace,
                        state,
                        &method_scope,
                        &format!("method '{}'", method.name),
                        errors,
                    );
                }
            }
            NamespaceItem::Def(def) => {
                let mut def_scope = scope.clone();
                def_scope
                    .static_names
                    .extend(def.type_params.iter().cloned());
                validate_template_stmt_list_refs(
                    &def.body,
                    namespace,
                    state,
                    &def_scope,
                    &format!("def '{}'", def.name),
                    errors,
                );
            }
            NamespaceItem::Alias(alias) => {
                validate_template_namespace_ref_args(
                    &alias.target,
                    namespace,
                    state,
                    scope,
                    "namespace alias target",
                    alias.loc.as_ref().copied().unwrap_or_default(),
                    errors,
                );
            }
            NamespaceItem::Use(use_decl) => {
                validate_template_namespace_ref_args(
                    &use_decl.target,
                    namespace,
                    state,
                    scope,
                    "use target",
                    use_decl.loc.as_ref().copied().unwrap_or_default(),
                    errors,
                );
            }
        }
    }
}

fn validate_template_proc_refs(
    proc: &ProcessorDef,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    errors: &mut Vec<Diagnostic>,
) {
    let mut proc_scope = scope.clone();
    proc_scope
        .static_names
        .extend(proc.type_params.iter().cloned());
    proc_scope
        .static_names
        .extend(proc.consts.iter().map(|decl| decl.name.clone()));

    for decl in &proc.ins {
        validate_template_optional_expr_refs(
            decl.default.as_ref(),
            current_ns,
            state,
            &proc_scope,
            "processor input default",
            errors,
        );
        validate_template_range_refs(
            decl.range.as_ref(),
            current_ns,
            state,
            &proc_scope,
            "processor input range",
            errors,
        );
    }
    for decl in &proc.outs {
        validate_template_range_refs(
            decl.range.as_ref(),
            current_ns,
            state,
            &proc_scope,
            "processor output range",
            errors,
        );
    }
    for decl in &proc.params {
        validate_template_optional_expr_refs(
            decl.default.as_ref(),
            current_ns,
            state,
            &proc_scope,
            "processor parameter default",
            errors,
        );
        validate_template_range_refs(
            decl.range.as_ref(),
            current_ns,
            state,
            &proc_scope,
            "processor parameter range",
            errors,
        );
    }
    for decl in &proc.consts {
        validate_template_expr_refs(
            &decl.expr,
            current_ns,
            state,
            &proc_scope,
            "processor const",
            errors,
        );
    }
    if let Some(factor) = &proc.sample_oversample_factor {
        validate_template_static_expr_refs(
            factor,
            current_ns,
            state,
            &proc_scope,
            "processor oversample factor",
            errors,
        );
    }
    validate_template_stmt_list_refs(
        &proc.init.body,
        current_ns,
        state,
        &proc_scope,
        "processor init",
        errors,
    );
    validate_template_stmt_list_refs(
        &proc.block_pre,
        current_ns,
        state,
        &proc_scope,
        "processor block",
        errors,
    );
    validate_template_stmt_list_refs(
        &proc.sample,
        current_ns,
        state,
        &proc_scope,
        "processor sample",
        errors,
    );
    validate_template_stmt_list_refs(
        &proc.block_post,
        current_ns,
        state,
        &proc_scope,
        "processor block",
        errors,
    );
    for event in &proc.events {
        let context = format!("event '{}'", event.name);
        for param in &event.params {
            validate_template_optional_expr_refs(
                param.default.as_ref(),
                current_ns,
                state,
                &proc_scope,
                &format!("{context} parameter default"),
                errors,
            );
        }
        validate_template_stmt_list_refs(
            &event.body,
            current_ns,
            state,
            &proc_scope,
            &context,
            errors,
        );
    }
    for def in &proc.local_defs {
        let mut def_scope = proc_scope.clone();
        def_scope
            .static_names
            .extend(def.type_params.iter().cloned());
        validate_template_stmt_list_refs(
            &def.body,
            current_ns,
            state,
            &def_scope,
            &format!("local def '{}'", def.name),
            errors,
        );
    }
}

fn validate_template_range_refs(
    range: Option<&DeclRange>,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(range) = range else {
        return;
    };
    validate_template_optional_expr_refs(
        range.min.as_ref(),
        current_ns,
        state,
        scope,
        context,
        errors,
    );
    validate_template_expr_refs(&range.max, current_ns, state, scope, context, errors);
}

fn validate_template_optional_expr_refs(
    expr: Option<&Expr>,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(expr) = expr {
        validate_template_expr_refs(expr, current_ns, state, scope, context, errors);
    }
}

fn validate_template_stmt_list_refs(
    stmts: &[Stmt],
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let mut local_scope = scope.clone();
    for stmt in stmts {
        validate_template_stmt_refs(stmt, current_ns, state, &mut local_scope, context, errors);
    }
}

fn validate_template_stmt_refs(
    stmt: &Stmt,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &mut NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            if let Some(ConstType::Array { size, .. }) = &decl.ty {
                validate_template_static_expr_refs(
                    size,
                    current_ns,
                    state,
                    scope,
                    &format!("{context} const array size"),
                    errors,
                );
            }
            validate_template_expr_refs(&decl.expr, current_ns, state, scope, context, errors);
            scope.static_names.insert(decl.name.clone());
        }
        Stmt::Assign {
            target,
            generic_decl_ty,
            expr,
            ..
        } => {
            validate_template_assign_target_refs(target, current_ns, state, scope, context, errors);
            if let Some(name) = generic_decl_ty {
                validate_template_named_ref(
                    name,
                    current_ns,
                    state,
                    scope,
                    &format!("{context} typed declaration"),
                    stmt.loc().span(),
                    errors,
                );
            }
            validate_template_expr_refs(expr, current_ns, state, scope, context, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            validate_template_expr_refs(expr, current_ns, state, scope, context, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            validate_template_expr_refs(cond, current_ns, state, scope, context, errors);
            validate_template_stmt_list_refs(
                then_branch,
                current_ns,
                state,
                scope,
                context,
                errors,
            );
            validate_template_stmt_list_refs(
                else_branch,
                current_ns,
                state,
                scope,
                context,
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
            validate_template_expr_refs(start, current_ns, state, scope, context, errors);
            validate_template_expr_refs(end, current_ns, state, scope, context, errors);
            validate_template_optional_expr_refs(
                step.as_ref(),
                current_ns,
                state,
                scope,
                context,
                errors,
            );
            validate_template_stmt_list_refs(body, current_ns, state, scope, context, errors);
        }
        Stmt::While { cond, body, .. } => {
            validate_template_expr_refs(cond, current_ns, state, scope, context, errors);
            validate_template_stmt_list_refs(body, current_ns, state, scope, context, errors);
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn validate_template_assign_target_refs(
    target: &AssignTarget,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match target {
        AssignTarget::Var(name) => {
            validate_template_named_ref(
                name,
                current_ns,
                state,
                scope,
                context,
                Span::default(),
                errors,
            );
        }
        AssignTarget::Index { base, index } => {
            validate_template_named_ref(
                base,
                current_ns,
                state,
                scope,
                context,
                index.loc().span(),
                errors,
            );
            validate_template_expr_refs(index, current_ns, state, scope, context, errors);
        }
        AssignTarget::Slice { base, start, end } => {
            validate_template_named_ref(
                base,
                current_ns,
                state,
                scope,
                context,
                start
                    .as_ref()
                    .map(|expr| expr.loc().span())
                    .unwrap_or_default(),
                errors,
            );
            validate_template_optional_expr_refs(
                start.as_ref(),
                current_ns,
                state,
                scope,
                context,
                errors,
            );
            validate_template_optional_expr_refs(
                end.as_ref(),
                current_ns,
                state,
                scope,
                context,
                errors,
            );
        }
        AssignTarget::Tuple(_) => {}
    }
}

fn validate_template_expr_refs(
    expr: &Expr,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Var { name, .. } => {
            validate_template_named_ref(
                name,
                current_ns,
                state,
                scope,
                context,
                expr.loc().span(),
                errors,
            );
        }
        Expr::UserCall { name, args, .. } => {
            validate_template_named_ref(
                name,
                current_ns,
                state,
                scope,
                context,
                expr.loc().span(),
                errors,
            );
            for arg in args {
                validate_template_expr_refs(&arg.expr, current_ns, state, scope, context, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            if let ArrayElemType::Struct(name) = &spec.elem {
                validate_template_named_ref(
                    name,
                    current_ns,
                    state,
                    scope,
                    &format!("{context} array element type"),
                    expr.loc().span(),
                    errors,
                );
            }
            validate_template_static_expr_refs(
                &spec.size,
                current_ns,
                state,
                scope,
                &format!("{context} array size"),
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    validate_template_expr_refs(value, current_ns, state, scope, context, errors);
                }
            }
        }
        Expr::Index { base, index, .. } => {
            validate_template_named_ref(
                base,
                current_ns,
                state,
                scope,
                context,
                expr.loc().span(),
                errors,
            );
            validate_template_expr_refs(index, current_ns, state, scope, context, errors);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            validate_template_named_ref(
                base,
                current_ns,
                state,
                scope,
                context,
                expr.loc().span(),
                errors,
            );
            validate_template_optional_expr_refs(
                start.as_deref(),
                current_ns,
                state,
                scope,
                context,
                errors,
            );
            validate_template_optional_expr_refs(
                end.as_deref(),
                current_ns,
                state,
                scope,
                context,
                errors,
            );
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            validate_template_expr_refs(lhs, current_ns, state, scope, context, errors);
            validate_template_expr_refs(rhs, current_ns, state, scope, context, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_template_expr_refs(arg, current_ns, state, scope, context, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_template_expr_refs(expr, current_ns, state, scope, context, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                validate_template_expr_refs(value, current_ns, state, scope, context, errors);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn validate_template_optional_static_expr_refs(
    expr: Option<&Expr>,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(expr) = expr {
        validate_template_static_expr_refs(expr, current_ns, state, scope, context, errors);
    }
}

fn validate_template_static_expr_refs(
    expr: &Expr,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Var { name, .. } => {
            if looks_like_namespace_ref(name) {
                validate_template_named_ref(
                    name,
                    current_ns,
                    state,
                    scope,
                    context,
                    expr.loc().span(),
                    errors,
                );
            } else if !static_name_known(name, current_ns, state, scope) {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: unknown constant '{name}'"),
                    expr.loc(),
                ));
            }
        }
        Expr::UserCall { name, args, .. } => {
            validate_template_named_ref(
                name,
                current_ns,
                state,
                scope,
                context,
                expr.loc().span(),
                errors,
            );
            for arg in args {
                validate_template_static_expr_refs(
                    &arg.expr, current_ns, state, scope, context, errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            validate_template_static_expr_refs(
                &spec.size, current_ns, state, scope, context, errors,
            );
            if let Some(values) = init {
                for value in values {
                    validate_template_static_expr_refs(
                        value, current_ns, state, scope, context, errors,
                    );
                }
            }
        }
        Expr::Index { base, index, .. } => {
            validate_template_named_ref(
                base,
                current_ns,
                state,
                scope,
                context,
                expr.loc().span(),
                errors,
            );
            validate_template_static_expr_refs(index, current_ns, state, scope, context, errors);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            validate_template_named_ref(
                base,
                current_ns,
                state,
                scope,
                context,
                expr.loc().span(),
                errors,
            );
            validate_template_optional_static_expr_refs(
                start.as_deref(),
                current_ns,
                state,
                scope,
                context,
                errors,
            );
            validate_template_optional_static_expr_refs(
                end.as_deref(),
                current_ns,
                state,
                scope,
                context,
                errors,
            );
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            validate_template_static_expr_refs(lhs, current_ns, state, scope, context, errors);
            validate_template_static_expr_refs(rhs, current_ns, state, scope, context, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_template_static_expr_refs(arg, current_ns, state, scope, context, errors);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_template_static_expr_refs(expr, current_ns, state, scope, context, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                validate_template_static_expr_refs(
                    value, current_ns, state, scope, context, errors,
                );
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

fn validate_template_named_ref(
    name: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) {
    if looks_like_namespace_ref(name) {
        validate_template_namespace_ref_exists(
            name, current_ns, state, scope, context, span, errors,
        );
    }
}

fn validate_template_namespace_ref_args(
    segments: &[NamespaceRefSegment],
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) {
    for segment in segments {
        if let Some(args) = &segment.args {
            for arg in args {
                validate_template_static_expr_refs(
                    &arg.expr, current_ns, state, scope, context, errors,
                );
            }
        }
    }
    let clean = segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>()
        .join("::");
    if !clean.is_empty() {
        validate_template_namespace_ref_exists(
            &clean, current_ns, state, scope, context, span, errors,
        );
    }
}

fn validate_template_namespace_ref_exists(
    name: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
    context: &str,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) {
    let segments = match onda_frontend::parse_namespace_ref_text_ast(name) {
        Ok(segments) => segments,
        Err(_) => return,
    };
    for segment in &segments {
        if let Some(args) = &segment.args {
            for arg in args {
                validate_template_static_expr_refs(
                    &arg.expr, current_ns, state, scope, context, errors,
                );
            }
        }
    }

    let clean = strip_type_args_from_path(name);
    for candidate in namespace_ref_candidates(&clean, current_ns, state) {
        if template_or_member_path_exists(&candidate, state) {
            return;
        }
    }

    let (namespace, symbol) = existing_template_namespace_parent(&clean, current_ns, state)
        .or_else(|| {
            clean
                .rsplit_once("::")
                .map(|(ns, symbol)| (ns.to_owned(), symbol.to_owned()))
        })
        .unwrap_or_default();
    if namespace.is_empty() {
        errors.push(Diagnostic::semantic_span(
            format!("{context}: unknown symbol '{clean}'"),
            span,
        ));
    } else {
        errors.push(Diagnostic::semantic_span(
            format!("{context}: unknown symbol '{symbol}' in namespace '{namespace}'"),
            span,
        ));
    }
}

fn namespace_ref_candidates(
    clean: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
) -> Vec<String> {
    let mut candidates = Vec::<String>::new();
    if let Some(target) = state.aliases.get(clean) {
        push_unique_template_candidate(&mut candidates, namespace_segments_key(&target.target));
    }
    if let Some((head, tail)) = clean.split_once("::") {
        for candidate_ns in namespace_candidates(current_ns) {
            let head_candidate = namespace_join(&candidate_ns, head);
            if let Some(alias) = state.aliases.get(&head_candidate) {
                push_unique_template_candidate(
                    &mut candidates,
                    namespace_join(&namespace_segments_key(&alias.target), tail),
                );
            }
            push_unique_template_candidate(&mut candidates, namespace_join(&head_candidate, tail));
        }
    } else {
        for candidate_ns in namespace_candidates(current_ns) {
            let candidate = namespace_join(&candidate_ns, clean);
            if let Some(alias) = state.aliases.get(&candidate) {
                push_unique_template_candidate(
                    &mut candidates,
                    namespace_segments_key(&alias.target),
                );
            }
            push_unique_template_candidate(&mut candidates, candidate);
        }
    }
    push_unique_template_candidate(&mut candidates, clean.to_owned());
    candidates
}

fn push_unique_template_candidate(candidates: &mut Vec<String>, candidate: String) {
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn template_or_member_path_exists(path: &str, state: &NamespaceFlattenState) -> bool {
    state.members.contains(path)
        || state.templates.contains_key(path)
        || state.aliases.contains_key(path)
        || template_decl_path_exists(path, state)
}

fn template_decl_path_exists(path: &str, state: &NamespaceFlattenState) -> bool {
    let mut prefix = path.to_owned();
    loop {
        if let Some(record) = state.templates.get(&prefix) {
            let suffix = path
                .strip_prefix(&prefix)
                .unwrap_or_default()
                .strip_prefix("::")
                .unwrap_or_default();
            if suffix.is_empty() {
                return true;
            }
            let parts = suffix.split("::").collect::<Vec<_>>();
            return namespace_item_path_exists(&record.decl.items, &parts);
        }
        let Some((parent, _)) = prefix.rsplit_once("::") else {
            return false;
        };
        prefix = parent.to_owned();
    }
}

fn namespace_item_path_exists(items: &[NamespaceItem], parts: &[&str]) -> bool {
    let Some((first, rest)) = parts.split_first() else {
        return true;
    };
    for item in items {
        match item {
            NamespaceItem::Const(decl) if decl.name == *first && rest.is_empty() => return true,
            NamespaceItem::Def(def) if def.name == *first && rest.is_empty() => return true,
            NamespaceItem::Proc(proc) if proc.name == *first && rest.is_empty() => return true,
            NamespaceItem::Struct(def) if def.name == *first && rest.is_empty() => return true,
            NamespaceItem::Alias(alias) if alias.name == *first && rest.is_empty() => return true,
            NamespaceItem::Namespace(child) if child.name == *first => {
                return rest.is_empty() || namespace_item_path_exists(&child.items, rest);
            }
            NamespaceItem::Assert(_)
            | NamespaceItem::Const(_)
            | NamespaceItem::Def(_)
            | NamespaceItem::Proc(_)
            | NamespaceItem::Struct(_)
            | NamespaceItem::Namespace(_)
            | NamespaceItem::Alias(_)
            | NamespaceItem::Use(_) => {}
        }
    }
    false
}

fn existing_template_namespace_parent(
    clean: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
) -> Option<(String, String)> {
    let (parent, symbol) = clean.rsplit_once("::")?;
    for candidate in namespace_ref_candidates(parent, current_ns, state) {
        if template_or_member_path_exists(&candidate, state)
            || has_visible_namespace_prefix(&candidate, state)
        {
            return Some((candidate, symbol.to_owned()));
        }
    }
    None
}

fn static_name_known(
    name: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
    scope: &NamespaceTemplateValidationScope,
) -> bool {
    if name.is_empty()
        || scope.static_names.contains(name)
        || is_builtin_constant_name(name)
        || PrimitiveType::is_name(name)
    {
        return true;
    }
    for candidate_ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&candidate_ns, name);
        if matches!(
            state.artifacts.const_values.get(&candidate),
            Some(ConstValue::Scalar(_))
        ) || state.artifacts.const_defs.contains_key(&candidate)
            || state.scalar_const_names.contains(&candidate)
        {
            return true;
        }
    }
    false
}

fn emit_namespace_items(
    items: &[NamespaceItem],
    namespace: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    out: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for item in items {
        register_member_for_item(item, namespace, state);
    }

    let mut local_const_names = HashSet::<String>::new();
    for item in items {
        match item {
            NamespaceItem::Assert(assert_decl) => {
                let mut block = Block::Assert(assert_decl.clone());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    namespace,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                out.push(block);
            }
            NamespaceItem::Const(decl) => {
                if !local_const_names.insert(decl.name.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "duplicate constant '{}' in namespace '{}'",
                            decl.name, namespace
                        ),
                        decl.loc.as_ref(),
                    ));
                    continue;
                }
                let mut decl = decl.clone();
                decl.name = namespace_join(namespace, &decl.name);
                let mut block = Block::Const(decl);
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    namespace,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                register_artifact_for_block(&block, options, state, errors);
                out.push(block);
            }
            NamespaceItem::Struct(s) => {
                let mut s = s.clone();
                s.name = namespace_join(namespace, &s.name);
                let mut block = Block::Struct(s);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                out.push(block);
            }
            NamespaceItem::Def(d) => {
                let mut d = d.clone();
                d.name = namespace_join(namespace, &d.name);
                let mut block = Block::Def(d);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                register_artifact_for_block(&block, options, state, errors);
                out.push(block);
            }
            NamespaceItem::Proc(p) => {
                let mut p = p.clone();
                p.name = namespace_join(namespace, &p.name);
                let mut block = Block::Proc(p);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                let mut generated = Vec::<Block>::new();
                rewrite_block_namespace_refs(
                    &mut block,
                    &ns,
                    template_consts,
                    options,
                    state,
                    &mut generated,
                    errors,
                );
                out.extend(generated);
                out.push(block);
            }
            NamespaceItem::Namespace(nested) => {
                process_namespace_decl(
                    nested.clone(),
                    namespace,
                    template_consts,
                    options,
                    state,
                    out,
                    errors,
                );
            }
            NamespaceItem::Alias(alias) => {
                let mut alias = alias.clone();
                rewrite_namespace_ref_args(
                    &mut alias.target,
                    namespace,
                    template_consts,
                    options,
                    state,
                    out,
                    errors,
                );
                register_namespace_alias(namespace, alias, state, errors);
            }
            NamespaceItem::Use(use_decl) => {
                register_use_decl(
                    namespace,
                    use_decl,
                    template_consts,
                    options,
                    state,
                    out,
                    errors,
                );
            }
        }
    }
}

fn block_decl_name(block: &Block) -> Option<&str> {
    match block {
        Block::Const(c) => Some(c.name.as_str()),
        Block::Struct(s) => Some(s.name.as_str()),
        Block::Def(d) => Some(d.name.as_str()),
        Block::Proc(p) => Some(p.name.as_str()),
        Block::Namespace(ns) => Some(ns.name.as_str()),
        Block::NamespaceAlias(alias) => Some(alias.name.as_str()),
        Block::Use(_) => None,
        _ => None,
    }
}

fn is_builtin_std_span(loc: Span) -> bool {
    loc.file()
        .as_deref()
        .is_some_and(|file| file.starts_with("<std/"))
}

fn register_top_level_member_for_block(block: &Block, state: &mut NamespaceFlattenState) {
    let Some(name) = block_decl_name(block) else {
        return;
    };
    if name.contains("::") || is_builtin_std_span(block.loc().span()) {
        return;
    }
    match block {
        Block::Const(decl) => {
            state.members.insert(name.to_owned());
            if is_const_array_decl(decl) {
                state.const_array_names.insert(name.to_owned());
            } else {
                state.scalar_const_names.insert(name.to_owned());
            }
        }
        Block::Struct(_) | Block::Def(_) | Block::Proc(_) => {
            state.members.insert(name.to_owned());
        }
        _ => {}
    }
}

fn register_namespace_alias(
    parent_ns: &str,
    alias: NamespaceAliasDecl,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    let full_name = namespace_join(parent_ns, &alias.name);
    if state.aliases.contains_key(&full_name) {
        errors.push(Diagnostic::semantic_span(
            format!("duplicate namespace alias '{full_name}'"),
            alias.loc.as_ref(),
        ));
        return;
    }
    state.aliases.insert(
        full_name,
        NamespaceAliasRecord {
            declared_ns: parent_ns.to_owned(),
            target: alias.target,
        },
    );
}

fn register_private_namespace_alias(
    parent_ns: &str,
    alias: NamespaceAliasDecl,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) {
    let full_name = namespace_join(parent_ns, &alias.name);
    let file = alias.loc.file().unwrap_or_default();
    let key = (full_name.clone(), file);
    if state.private_aliases.contains_key(&key) || state.aliases.contains_key(&full_name) {
        errors.push(Diagnostic::semantic_span(
            format!("duplicate namespace alias '{full_name}'"),
            alias.loc.as_ref(),
        ));
        return;
    }
    state.private_aliases.insert(
        key,
        NamespaceAliasRecord {
            declared_ns: parent_ns.to_owned(),
            target: alias.target,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn register_use_decl(
    current_ns: &str,
    use_decl: &UseDecl,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(target) = resolve_namespace_segments_internal(
        &use_decl.target,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        use_decl.loc.as_ref().copied().unwrap_or_default(),
        0,
    ) else {
        return;
    };

    if let Some(alias) = &use_decl.alias {
        if has_visible_namespace_prefix(&target, state) && !state.members.contains(&target) {
            let alias_decl = NamespaceAliasDecl {
                loc: use_decl.loc,
                name: alias.clone(),
                target: use_decl.target.clone(),
            };
            if use_decl.public {
                register_namespace_alias(current_ns, alias_decl, state, errors);
            } else {
                register_private_namespace_alias(current_ns, alias_decl, state, errors);
            }
        } else {
            register_use_symbol(
                current_ns,
                use_decl.loc,
                use_decl.public,
                alias,
                target,
                state,
            );
        }
        return;
    }

    if state.members.contains(&target) {
        if let Some(leaf) = target.rsplit("::").next().map(ToOwned::to_owned) {
            register_use_symbol(
                current_ns,
                use_decl.loc,
                use_decl.public,
                &leaf,
                target,
                state,
            );
        }
    } else {
        register_use_namespace(current_ns, use_decl.loc, use_decl.public, target, state);
    }
}

fn register_use_symbol(
    current_ns: &str,
    loc: Span,
    public: bool,
    leaf: &str,
    target: String,
    state: &mut NamespaceFlattenState,
) {
    use_scope_mut(state, current_ns, loc, public)
        .symbols
        .entry(leaf.to_owned())
        .or_default()
        .push(UseBinding { target });
}

fn register_use_namespace(
    current_ns: &str,
    loc: Span,
    public: bool,
    target: String,
    state: &mut NamespaceFlattenState,
) {
    let scope = use_scope_mut(state, current_ns, loc, public);
    if !scope
        .namespaces
        .iter()
        .any(|binding| binding.target == target)
    {
        scope.namespaces.push(NamespaceUseBinding { target });
    }
}

fn use_scope_mut<'a>(
    state: &'a mut NamespaceFlattenState,
    current_ns: &str,
    loc: Span,
    public: bool,
) -> &'a mut UseScope {
    if public {
        return state.public_uses.entry(current_ns.to_owned()).or_default();
    }
    let file = loc.file().unwrap_or_default();
    state
        .private_uses
        .entry((current_ns.to_owned(), file))
        .or_default()
}

fn resolve_visible_alias(
    alias_leaf: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
    use_site_span: Span,
) -> Option<NamespaceAliasRecord> {
    let file = use_site_span.file().unwrap_or_default();
    for ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&ns, alias_leaf);
        if let Some(alias) = state.aliases.get(&candidate) {
            return Some(alias.clone());
        }
        if let Some(alias) = state.private_aliases.get(&(candidate, file.clone())) {
            return Some(alias.clone());
        }
    }
    None
}

fn has_visible_namespace_prefix(prefix: &str, state: &NamespaceFlattenState) -> bool {
    let nested_prefix = format!("{prefix}::");
    state.templates.contains_key(prefix)
        || state.aliases.contains_key(prefix)
        || state
            .members
            .iter()
            .any(|name| name.starts_with(&nested_prefix))
        || state
            .templates
            .keys()
            .any(|name| name.starts_with(&nested_prefix))
        || state
            .aliases
            .keys()
            .any(|name| name.starts_with(&nested_prefix))
}

fn qualify_local_namespace_member_name(
    name: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
) -> Option<String> {
    if name.is_empty() || name.contains("::") || name.contains('.') {
        return None;
    }
    let (base, suffix) = split_named_type_base_and_suffix(name);
    for ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&ns, base);
        if state.members.contains(&candidate) {
            return Some(format!("{candidate}{suffix}"));
        }
    }
    None
}

fn resolve_visible_unqualified_member_name(
    name: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
    loc: impl Into<SourceLoc>,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    if name.is_empty() || name.contains("::") || name.contains('.') {
        return None;
    }

    let loc = loc.into();
    let (base, suffix) = split_named_type_base_and_suffix(name);
    let mut candidates = Vec::<String>::new();

    if let Some(local) = qualify_local_namespace_member_name(base, current_ns, state) {
        candidates.push(local);
    }

    let file = loc.file().unwrap_or_default();
    for ns in namespace_candidates(current_ns) {
        if let Some(scope) = state.public_uses.get(&ns) {
            collect_use_scope_candidates(base, scope, state, &mut candidates);
        }
        if let Some(scope) = state.private_uses.get(&(ns.clone(), file.clone())) {
            collect_use_scope_candidates(base, scope, state, &mut candidates);
        }
    }

    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [] => None,
        [only] => Some(format!("{only}{suffix}")),
        many => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "ambiguous unqualified symbol '{base}' from explicit use declarations; qualify the call or type as one of: {}",
                    many.join(", ")
                ),
                loc.span(),
            ));
            None
        }
    }
}

fn resolve_visible_unqualified_const_name(
    name: &str,
    current_ns: &str,
    state: &NamespaceFlattenState,
    loc: impl Into<SourceLoc>,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    if name.is_empty() || name.contains("::") || name.contains('.') {
        return None;
    }

    let loc = loc.into();
    let (base, suffix) = split_named_type_base_and_suffix(name);
    let mut candidates = Vec::<String>::new();

    for ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&ns, base);
        if is_const_member_name(&candidate, state) {
            candidates.push(candidate);
        }
    }

    let file = loc.file().unwrap_or_default();
    for ns in namespace_candidates(current_ns) {
        if let Some(scope) = state.public_uses.get(&ns) {
            collect_use_scope_const_candidates(base, scope, state, &mut candidates);
        }
        if let Some(scope) = state.private_uses.get(&(ns.clone(), file.clone())) {
            collect_use_scope_const_candidates(base, scope, state, &mut candidates);
        }
    }

    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [] => None,
        [only] => Some(format!("{only}{suffix}")),
        many => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "ambiguous unqualified symbol '{base}' from explicit use declarations; qualify the assignment target as one of: {}",
                    many.join(", ")
                ),
                loc.span(),
            ));
            None
        }
    }
}

fn collect_use_scope_const_candidates(
    base: &str,
    scope: &UseScope,
    state: &NamespaceFlattenState,
    candidates: &mut Vec<String>,
) {
    if let Some(bindings) = scope.symbols.get(base) {
        candidates.extend(
            bindings
                .iter()
                .filter(|binding| is_const_member_name(&binding.target, state))
                .map(|binding| binding.target.clone()),
        );
    }
    for binding in &scope.namespaces {
        let target = namespace_join(&binding.target, base);
        if is_const_member_name(&target, state) {
            candidates.push(target);
        }
    }
}

fn is_const_member_name(name: &str, state: &NamespaceFlattenState) -> bool {
    state.scalar_const_names.contains(name) || state.const_array_names.contains(name)
}

fn collect_use_scope_candidates(
    base: &str,
    scope: &UseScope,
    state: &NamespaceFlattenState,
    candidates: &mut Vec<String>,
) {
    if let Some(bindings) = scope.symbols.get(base) {
        candidates.extend(bindings.iter().map(|binding| binding.target.clone()));
    }
    for binding in &scope.namespaces {
        let target = namespace_join(&binding.target, base);
        if state.members.contains(&target) {
            candidates.push(target);
        }
    }
}

fn resolve_visible_unqualified_namespace_root(
    base: &str,
    current_ns: &str,
    require_template: bool,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    use_site_span: Span,
    errors: &mut Vec<Diagnostic>,
    depth: usize,
) -> Option<String> {
    let file = use_site_span.file().unwrap_or_default();
    let mut candidates = Vec::<String>::new();
    for ns in namespace_candidates(current_ns) {
        if let Some(scope) = state.public_uses.get(&ns) {
            collect_use_scope_namespace_root_candidates(
                base,
                scope,
                require_template,
                state,
                use_site_span,
                &mut candidates,
            );
        }
        if let Some(scope) = state.private_uses.get(&(ns.clone(), file.clone())) {
            collect_use_scope_namespace_root_candidates(
                base,
                scope,
                require_template,
                state,
                use_site_span,
                &mut candidates,
            );
        }
    }

    let mut resolved_candidates = Vec::<String>::new();
    for candidate in candidates {
        if let Some(alias_target) = resolve_namespace_alias_path(
            &candidate,
            template_consts,
            options,
            state,
            generated,
            errors,
            use_site_span,
            depth + 1,
        ) {
            let alias_target = alias_target?;
            resolved_candidates.push(alias_target);
        } else {
            resolved_candidates.push(candidate);
        }
    }

    resolved_candidates.sort();
    resolved_candidates.dedup();
    match resolved_candidates.as_slice() {
        [] => None,
        [only] => Some(only.clone()),
        many => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "ambiguous unqualified namespace '{base}' from explicit use declarations; qualify the reference as one of: {}",
                    many.join(", ")
                ),
                use_site_span,
            ));
            None
        }
    }
}

fn collect_use_scope_namespace_root_candidates(
    base: &str,
    scope: &UseScope,
    require_template: bool,
    state: &NamespaceFlattenState,
    use_site_span: Span,
    candidates: &mut Vec<String>,
) {
    if let Some(bindings) = scope.symbols.get(base) {
        candidates.extend(bindings.iter().filter_map(|binding| {
            namespace_root_candidate(&binding.target, require_template, state, use_site_span)
        }));
    }
    for binding in &scope.namespaces {
        let target = namespace_join(&binding.target, base);
        if let Some(candidate) =
            namespace_root_candidate(&target, require_template, state, use_site_span)
        {
            candidates.push(candidate);
        }
    }
}

fn namespace_root_candidate(
    target: &str,
    require_template: bool,
    state: &NamespaceFlattenState,
    use_site_span: Span,
) -> Option<String> {
    if require_template {
        state
            .templates
            .contains_key(target)
            .then(|| target.to_owned())
    } else {
        (has_visible_namespace_prefix(target, state)
            || visible_namespace_alias_record(target, state, use_site_span).is_some())
        .then(|| target.to_owned())
    }
}

fn visible_namespace_alias_record(
    path: &str,
    state: &NamespaceFlattenState,
    use_site_span: Span,
) -> Option<NamespaceAliasRecord> {
    if let Some(alias) = state.aliases.get(path) {
        return Some(alias.clone());
    }
    let file = use_site_span.file().unwrap_or_default();
    state.private_aliases.get(&(path.to_owned(), file)).cloned()
}

#[allow(clippy::too_many_arguments)]
fn resolve_namespace_alias_path(
    path: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_span: Span,
    depth: usize,
) -> Option<Option<String>> {
    let alias = visible_namespace_alias_record(path, state, use_site_span)?;
    Some(resolve_namespace_segments_internal(
        &alias.target,
        &alias.declared_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        use_site_span,
        depth,
    ))
}

fn resolve_namespace_symbol_name(
    name: &str,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_span: Span,
) -> Option<String> {
    if !looks_like_namespace_ref(name) {
        return Some(name.to_owned());
    }
    let segments = match onda_frontend::parse_namespace_ref_text_ast(name) {
        Ok(segments) => segments,
        Err(mut diags) => {
            errors.append(&mut diags);
            return None;
        }
    };
    resolve_namespace_segments_internal(
        &segments,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        use_site_span,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_namespace_segments_internal(
    segments: &[NamespaceRefSegment],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_span: Span,
    depth: usize,
) -> Option<String> {
    if depth > 64 {
        errors.push(Diagnostic::semantic_span(
            "namespace alias/template resolution exceeded recursion depth",
            use_site_span,
        ));
        return None;
    }
    if segments.is_empty() {
        errors.push(Diagnostic::semantic_span(
            "empty namespace reference",
            use_site_span,
        ));
        return None;
    }

    let mut idx = 0usize;
    let mut path = String::new();
    let empty_call_args = Vec::<NamespaceCallArg>::new();

    if segments[0].args.is_none() {
        if let Some(alias) =
            resolve_visible_alias(&segments[0].name, current_ns, state, use_site_span)
        {
            path = resolve_namespace_segments_internal(
                &alias.target,
                &alias.declared_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
                depth + 1,
            )?;
            idx = 1;
        }
    }

    if idx == 0 {
        if let Some(args) = &segments[0].args {
            let mut resolved = None::<String>;
            let before_errors = errors.len();
            for candidate_ns in namespace_candidates(current_ns) {
                let candidate = namespace_join(&candidate_ns, &segments[0].name);
                if state.templates.contains_key(&candidate) {
                    resolved = instantiate_namespace_template(
                        &candidate,
                        args,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        use_site_span,
                    );
                    break;
                }
            }
            if errors.len() == before_errors {
                if let Some(candidate) = resolve_visible_unqualified_namespace_root(
                    &segments[0].name,
                    current_ns,
                    true,
                    template_consts,
                    options,
                    state,
                    generated,
                    use_site_span,
                    errors,
                    depth,
                ) {
                    let used_resolved = instantiate_namespace_template(
                        &candidate,
                        args,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        use_site_span,
                    );
                    match (resolved.as_ref(), used_resolved) {
                        (Some(local), Some(used)) if local != &used => {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "ambiguous unqualified namespace '{}' from explicit use declarations; qualify the reference as either {local} or {used}",
                                    segments[0].name
                                ),
                                use_site_span,
                            ));
                            return None;
                        }
                        (None, used) => resolved = used,
                        _ => {}
                    }
                }
            }
            let Some(found) = resolved else {
                if errors.len() > before_errors {
                    return None;
                }
                errors.push(Diagnostic::semantic_span(
                    format!("unknown namespace template '{}'", segments[0].name),
                    use_site_span,
                ));
                return None;
            };
            path = found;
        } else {
            let mut resolved = None::<String>;
            let before_errors = errors.len();
            for candidate_ns in namespace_candidates(current_ns) {
                let candidate = namespace_join(&candidate_ns, &segments[0].name);
                if state.templates.contains_key(&candidate) {
                    resolved = instantiate_namespace_template(
                        &candidate,
                        &empty_call_args,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        use_site_span,
                    );
                    break;
                }
                if has_visible_namespace_prefix(&candidate, state) {
                    resolved = Some(candidate);
                    break;
                }
            }
            if let Some(candidate) = resolve_visible_unqualified_namespace_root(
                &segments[0].name,
                current_ns,
                false,
                template_consts,
                options,
                state,
                generated,
                use_site_span,
                errors,
                depth,
            ) {
                let used_resolved = if state.templates.contains_key(&candidate) {
                    instantiate_namespace_template(
                        &candidate,
                        &empty_call_args,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        use_site_span,
                    )
                } else {
                    Some(candidate)
                };
                match (resolved.as_ref(), used_resolved) {
                    (Some(local), Some(used)) if local != &used => {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "ambiguous unqualified namespace '{}' from explicit use declarations; qualify the reference as either {local} or {used}",
                                segments[0].name
                            ),
                            use_site_span,
                        ));
                        return None;
                    }
                    (None, used) => resolved = used,
                    _ => {}
                }
            }
            if resolved.is_none() && errors.len() > before_errors {
                return None;
            }
            path = resolved.unwrap_or_else(|| segments[0].name.clone());
            if let Some(alias_target) = resolve_namespace_alias_path(
                &path,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
                depth + 1,
            ) {
                path = alias_target?;
            }
        }
        idx = 1;
    }

    for seg in &segments[idx..] {
        let candidate = namespace_join(&path, &seg.name);
        if let Some(args) = &seg.args {
            if state.templates.contains_key(&candidate) {
                path = instantiate_namespace_template(
                    &candidate,
                    args,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_span,
                )?;
            } else {
                let suffix = format_call_args_as_type_suffix(args);
                path = format!("{candidate}{suffix}");
            }
        } else if state.templates.contains_key(&candidate) {
            path = instantiate_namespace_template(
                &candidate,
                &empty_call_args,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
            )?;
        } else {
            path = candidate;
            if let Some(alias_target) = resolve_namespace_alias_path(
                &path,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
                depth + 1,
            ) {
                path = alias_target?;
            }
        }
    }
    Some(path)
}

#[allow(clippy::too_many_arguments)]
fn instantiate_namespace_template(
    full_template_name: &str,
    call_args: &[NamespaceCallArg],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_span: Span,
) -> Option<String> {
    let template = match state.templates.get(full_template_name).cloned() {
        Some(template) => template,
        None => {
            errors.push(Diagnostic::semantic_span(
                format!("unknown namespace template '{full_template_name}'"),
                use_site_span,
            ));
            return None;
        }
    };

    let mut named = HashMap::<String, (Expr, Span)>::new();
    let mut positional = Vec::<Expr>::new();
    for arg in call_args {
        let mut expr = arg.expr.clone();
        rewrite_expr(
            &mut expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
        let arg_span = expr.loc().span();
        if let Some(name) = &arg.name {
            if named.insert(name.clone(), (expr, arg_span)).is_some() {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "namespace template '{}' argument '{}' specified more than once",
                        full_template_name, name
                    ),
                    if arg_span.is_zero() {
                        use_site_span
                    } else {
                        arg_span
                    },
                ));
                return None;
            }
        } else {
            positional.push(expr);
        }
    }

    let mut effective_template_consts = template.captured_template_consts.clone();
    let template_parent_ns = namespace_parent(full_template_name).unwrap_or("");
    let mut param_values = Vec::<(String, TypedConstValue, Span)>::new();
    let mut pos_idx = 0usize;
    for param in &template.decl.params {
        let (mut value_expr, value_span, use_captured_artifacts) =
            if let Some((expr, span)) = named.remove(&param.name) {
                (expr, span, false)
            } else if let Some(pos_expr) = positional.get(pos_idx) {
                pos_idx += 1;
                (pos_expr.clone(), pos_expr.loc().span(), false)
            } else {
                let mut default = param.default.clone();
                rewrite_expr(
                    &mut default,
                    template_parent_ns,
                    &effective_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
                let span = default.loc().span();
                (default, span, true)
            };
        if !use_captured_artifacts {
            rewrite_expr(
                &mut value_expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        let eval_artifacts = if use_captured_artifacts {
            &template.captured_artifacts
        } else {
            &state.artifacts
        };
        let value = eval_namespace_template_arg(
            &value_expr,
            eval_artifacts,
            options,
            &format!(
                "namespace template '{}' argument '{}'",
                full_template_name, param.name
            ),
            errors,
        )?;
        effective_template_consts.insert(
            param.name.clone(),
            typed_const_expr_with_loc(value, value_expr.loc()),
        );
        param_values.push((param.name.clone(), value, value_span));
    }

    if pos_idx < positional.len() {
        let extra_arg_span = positional
            .get(pos_idx)
            .map(|expr| expr.loc().span())
            .filter(|span| !span.is_zero())
            .unwrap_or(use_site_span);
        errors.push(Diagnostic::semantic_span(
            format!(
                "namespace template '{}' received too many positional arguments",
                full_template_name
            ),
            extra_arg_span,
        ));
        return None;
    }
    if !named.is_empty() {
        let mut unknown = named.keys().cloned().collect::<Vec<_>>();
        unknown.sort();
        let unknown_span = unknown
            .iter()
            .filter_map(|name| named.get(name).map(|(_, span)| *span))
            .find(|span| !span.is_zero())
            .unwrap_or(use_site_span);
        errors.push(Diagnostic::semantic_span(
            format!(
                "namespace template '{}' received unknown named arguments: {}",
                full_template_name,
                unknown.join(", ")
            ),
            unknown_span,
        ));
        return None;
    }

    let key = {
        let values = param_values
            .iter()
            .map(|(_, value, _)| typed_const_value_key(*value))
            .collect::<Vec<_>>()
            .join(",");
        format!("{full_template_name}[{values}]")
    };
    if let Some(existing) = state.instantiations.get(&key) {
        return Some(existing.clone());
    }

    let (parent, leaf) = split_namespace_parent_leaf(full_template_name);
    let concrete_leaf = format!("{leaf}__nsinst{}", state.next_instantiation_id);
    state.next_instantiation_id += 1;
    let concrete_ns = namespace_join(parent, &concrete_leaf);
    state.instantiations.insert(key, concrete_ns.clone());

    let saved_artifacts =
        std::mem::replace(&mut state.artifacts, template.captured_artifacts.clone());
    emit_namespace_items(
        &template.decl.items,
        &concrete_ns,
        &effective_template_consts,
        options,
        state,
        generated,
        errors,
    );
    let emitted_artifacts = std::mem::replace(&mut state.artifacts, saved_artifacts);
    merge_generated_namespace_artifacts(&mut state.artifacts, emitted_artifacts, &concrete_ns);

    Some(concrete_ns)
}

fn eval_namespace_template_arg(
    expr: &Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    eval_const_scalar_expr_with_defs(
        expr,
        PrimitiveType::I32,
        &locals,
        &local_arrays,
        &artifacts.const_values,
        const_def_registry(artifacts),
        options,
        context,
        &mut Vec::new(),
        errors,
    )
}

#[allow(clippy::too_many_arguments)]
fn rewrite_namespace_ref_args(
    segments: &mut [NamespaceRefSegment],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for seg in segments {
        if let Some(args) = &mut seg.args {
            for arg in args {
                rewrite_expr(
                    &mut arg.expr,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
    }
}

fn rewrite_named_type_ref_name(
    name: &mut String,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    use_site_loc: impl Into<SourceLoc>,
) {
    let use_site_loc = use_site_loc.into();
    let use_site_span = use_site_loc.span();
    if let Some(qualified) =
        resolve_visible_unqualified_member_name(name, current_ns, state, use_site_loc, errors)
    {
        *name = qualified;
        return;
    }

    let (base, suffix) = split_named_type_base_and_suffix(name);
    if !suffix.is_empty() {
        if let Some(qualified) =
            resolve_visible_unqualified_member_name(base, current_ns, state, use_site_loc, errors)
        {
            *name = format!("{qualified}{suffix}");
            return;
        }
        if looks_like_namespace_ref(base) {
            if let Some(resolved) = resolve_namespace_symbol_name(
                base,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_span,
            ) {
                *name = format!("{resolved}{suffix}");
            }
            return;
        }
    }

    if looks_like_namespace_ref(name) {
        if let Some(resolved) = resolve_namespace_symbol_name(
            name,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            use_site_span,
        ) {
            *name = resolved;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn qualify_or_resolve_symbol_name(
    name: &str,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) -> Option<String> {
    let loc = loc.into();
    if let Some(qualified) =
        resolve_visible_unqualified_member_name(name, current_ns, state, loc, errors)
    {
        return Some(qualified);
    }
    if looks_like_namespace_ref(name) {
        return resolve_namespace_symbol_name(
            name,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc.span(),
        );
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn qualify_instance_method_call_name(
    name: &mut String,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
    local_scope: &RewriteNameScope,
) {
    let Some((base, method)) = name.rsplit_once('.') else {
        return;
    };
    if local_scope.contains_value_name(base) {
        return;
    }
    if let Some(resolved) = qualify_or_resolve_symbol_name(
        base,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        loc,
    ) {
        *name = format!("{resolved}.{method}");
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_block_namespace_refs(
    block: &mut Block,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    match block {
        Block::Ins(port_block) | Block::Outs(port_block) | Block::KOuts(port_block) => {
            rewrite_deferred_port_count(
                port_block,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_port_decls(
                &mut port_block.decls,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Params(param_block) => {
            rewrite_deferred_param_count(
                param_block,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_param_decls(
                &mut param_block.decls,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Const(decl) => {
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                rewrite_expr(
                    size,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_expr(
                &mut decl.expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Buffers(buffer_block) => {
            rewrite_deferred_buffer_count(
                buffer_block,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
            for decl in &mut buffer_block.decls {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(
                        ty,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        decl.ty_loc.as_ref().or(decl.loc.as_ref()),
                    );
                }
            }
        }
        Block::Assert(assert_decl) => {
            rewrite_expr(
                &mut assert_decl.expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Events(events) => {
            let global_scope = RewriteNameScope::from_names(state.global_value_names.clone());
            for event in events {
                rewrite_event_def_with_scope(
                    event,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    &global_scope,
                );
            }
        }
        Block::Struct(s) => {
            for field in &mut s.fields {
                rewrite_field_type(
                    &mut field.ty,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    field.ty_loc.as_ref().or(field.loc.as_ref()),
                );
                if let Some(default) = &mut field.default {
                    rewrite_expr(
                        default,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                    );
                }
            }
            for method in &mut s.methods {
                rewrite_function_def(
                    method,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
        Block::Def(d) => rewrite_function_def(
            d,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        ),
        Block::Proc(p) => {
            let proc_template_consts = template_consts.clone();
            let proc_scope = proc_value_name_scope(p);
            for decl in &mut p.consts {
                if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                    rewrite_expr(
                        size,
                        current_ns,
                        &proc_template_consts,
                        options,
                        state,
                        generated,
                        errors,
                    );
                }
                rewrite_expr(
                    &mut decl.expr,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            rewrite_deferred_proc_port_count(
                &mut p.ins_deferred_count,
                &mut p.ins_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_deferred_proc_port_count(
                &mut p.outs_deferred_count,
                &mut p.outs_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_deferred_proc_param_count(
                &mut p.params_deferred_count,
                &mut p.params_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_deferred_proc_buffer_count(
                &mut p.buffers_deferred_count,
                &mut p.buffers_deferred_default_ty,
                &p.loc,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_port_decls(
                &mut p.ins,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_port_decls(
                &mut p.outs,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            rewrite_param_decls(
                &mut p.params,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
            );
            for event in &mut p.events {
                rewrite_event_def_with_scope(
                    event,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    &proc_scope,
                );
            }
            for decl in &mut p.buffers {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(
                        ty,
                        current_ns,
                        &proc_template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        decl.ty_loc.as_ref().or(decl.loc.as_ref()),
                    );
                }
            }
            if let Some(default_ty) = &mut p.init.default_ty {
                rewrite_decl_type(
                    default_ty,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    p.init.default_ty_loc.as_ref().or(p.init.loc.as_ref()),
                );
            }
            let mut init_scope = proc_scope.clone();
            rewrite_stmts_scoped(
                &mut p.init.body,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
                &mut init_scope,
            );
            let mut block_scope = init_scope.clone();
            rewrite_stmts_scoped(
                &mut p.block_pre,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
                &mut block_scope,
            );
            if let Some(os) = &mut p.sample_oversample_factor {
                rewrite_expr(
                    os,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            let mut sample_scope = block_scope.clone();
            rewrite_stmts_scoped(
                &mut p.sample,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
                &mut sample_scope,
            );
            let mut post_scope = block_scope;
            rewrite_stmts_scoped(
                &mut p.block_post,
                current_ns,
                &proc_template_consts,
                options,
                state,
                generated,
                errors,
                &mut post_scope,
            );
            if let Some(graph) = &mut p.graph {
                rewrite_graph_block(
                    graph,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            for def in &mut p.local_defs {
                rewrite_function_def_with_scope(
                    def,
                    current_ns,
                    &proc_template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    &proc_scope,
                );
            }
        }
        Block::Init(init) => {
            if let Some(default_ty) = &mut init.default_ty {
                rewrite_decl_type(
                    default_ty,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    init.default_ty_loc.as_ref().or(init.loc.as_ref()),
                );
            }
            let mut global_scope = RewriteNameScope::from_names(state.global_value_names.clone());
            rewrite_stmts_scoped(
                &mut init.body,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &mut global_scope,
            );
        }
        Block::Block(exec) => {
            let mut block_scope = RewriteNameScope::from_names(state.global_value_names.clone());
            rewrite_stmts_scoped(
                &mut exec.pre,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &mut block_scope,
            );
            if let Some(sample) = &mut exec.sample {
                if let Some(os) = &mut sample.oversample_factor {
                    rewrite_expr(
                        os,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                    );
                }
                let mut sample_scope = block_scope.clone();
                rewrite_stmts_scoped(
                    &mut sample.body,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    &mut sample_scope,
                );
            }
            rewrite_stmts_scoped(
                &mut exec.post,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &mut block_scope,
            );
        }
        Block::Sample(sample) => {
            if let Some(os) = &mut sample.oversample_factor {
                rewrite_expr(
                    os,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            let mut global_scope = RewriteNameScope::from_names(state.global_value_names.clone());
            rewrite_stmts_scoped(
                &mut sample.body,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &mut global_scope,
            );
        }
        Block::Graph(graph) => {
            rewrite_graph_block(
                graph,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        Block::Namespace(_) | Block::NamespaceAlias(_) | Block::Use(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_port_count(
    port_block: &mut PortBlock,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = &mut port_block.deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = &mut port_block.deferred_default_ty {
        rewrite_decl_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            port_block.loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_param_count(
    param_block: &mut ParamBlock,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = &mut param_block.deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = &mut param_block.deferred_default_ty {
        rewrite_decl_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            param_block.loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_buffer_count(
    buffer_block: &mut BufferBlock,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = &mut buffer_block.deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = &mut buffer_block.deferred_default_ty {
        rewrite_buffer_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            buffer_block.loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_proc_port_count(
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    loc: &Span,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = deferred_default_ty {
        rewrite_decl_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_proc_param_count(
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<DeclType>,
    loc: &Span,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_deferred_proc_port_count(
        deferred_count,
        deferred_default_ty,
        loc,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
    );
}

#[allow(clippy::too_many_arguments)]
fn rewrite_deferred_proc_buffer_count(
    deferred_count: &mut Option<Expr>,
    deferred_default_ty: &mut Option<BufferType>,
    loc: &Span,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(count_expr) = deferred_count {
        rewrite_expr(
            count_expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(default_ty) = deferred_default_ty {
        rewrite_buffer_type(
            default_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc.as_ref(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_port_decls(
    decls: &mut [PortDecl],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for decl in decls {
        rewrite_decl_type_default_range(
            &mut decl.ty,
            &mut decl.default,
            &mut decl.range,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            decl.ty_loc.as_ref().or(decl.loc.as_ref()),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_param_decls(
    decls: &mut [ParamDecl],
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for decl in decls {
        rewrite_decl_type_default_range(
            &mut decl.ty,
            &mut decl.default,
            &mut decl.range,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            decl.ty_loc.as_ref().or(decl.loc.as_ref()),
        );
        if let Some(step) = &mut decl.control.step {
            rewrite_expr(
                step,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_decl_type_default_range(
    ty: &mut Option<DeclType>,
    default: &mut Option<Expr>,
    range: &mut Option<DeclRange>,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    if let Some(ty) = ty {
        rewrite_decl_type(
            ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc,
        );
    }
    if let Some(default) = default {
        rewrite_expr(
            default,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
    if let Some(range) = range {
        if let Some(min) = &mut range.min {
            rewrite_expr(
                min,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        rewrite_expr(
            &mut range.max,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_graph_block(
    graph: &mut GraphBlock,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    for edge in &mut graph.edges {
        rewrite_expr(
            &mut edge.source,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
        if let Some(delay) = &mut edge.delay {
            rewrite_expr(
                delay,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        for dest in &mut edge.dests {
            if let GraphEndpoint::ProcIndexedField { index, .. } = dest {
                rewrite_expr(
                    index,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_function_def(
    def: &mut FunctionDef,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    rewrite_function_def_with_scope(
        def,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        &RewriteNameScope::default(),
    );
}

#[allow(clippy::too_many_arguments)]
fn rewrite_function_def_with_scope(
    def: &mut FunctionDef,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    parent_scope: &RewriteNameScope,
) {
    for param in &mut def.params {
        if let Some(ty) = &mut param.ty {
            rewrite_fn_param_type(
                ty,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                param.ty_loc.as_ref().or(param.loc.as_ref()),
            );
        }
        if let Some(default) = &mut param.default {
            rewrite_expr(
                default,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
    }
    if let Some(return_ty) = &mut def.return_ty {
        rewrite_fn_return_type(
            return_ty,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            def.return_ty_loc.as_ref().or(def.loc.as_ref()),
        );
    }
    let mut local_scope = parent_scope.clone();
    local_scope.extend(def.type_params.iter().cloned());
    local_scope.extend(def.params.iter().map(|param| param.name.clone()));
    rewrite_stmts_scoped(
        &mut def.body,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        &mut local_scope,
    );
}

#[allow(clippy::too_many_arguments)]
fn rewrite_event_def_with_scope(
    event: &mut EventDef,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    parent_scope: &RewriteNameScope,
) {
    for param in &mut event.params {
        match &mut param.ty {
            EventParamType::Array { size, .. } => {
                rewrite_expr(
                    size,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            EventParamType::GenericArray { elem, size } => {
                rewrite_named_type_ref_name(
                    elem,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    param.ty_loc.as_ref().or(param.loc.as_ref()),
                );
                rewrite_expr(
                    size,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                );
            }
            EventParamType::GenericScalar { name } => {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    param.ty_loc.as_ref().or(param.loc.as_ref()),
                );
            }
            EventParamType::GenericSlice { elem } => {
                rewrite_named_type_ref_name(
                    elem,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    param.ty_loc.as_ref().or(param.loc.as_ref()),
                );
            }
            EventParamType::Scalar(_) | EventParamType::Slice { .. } => {}
        }
        if let Some(default) = &mut param.default {
            rewrite_expr(
                default,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
    }
    let mut local_scope = parent_scope.clone();
    local_scope.extend(event.params.iter().map(|param| param.name.clone()));
    rewrite_stmts_scoped(
        &mut event.body,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        &mut local_scope,
    );
}

#[allow(clippy::too_many_arguments)]
fn rewrite_decl_type(
    ty: &mut DeclType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        DeclType::Generic(name) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        DeclType::ArrayGeneric { elem, size } => {
            rewrite_named_type_ref_name(
                elem,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        DeclType::Array { size, .. } => {
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        DeclType::Scalar(_) | DeclType::Tuple(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_field_type(
    ty: &mut FieldType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FieldType::Generic(name) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FieldType::Array(spec) => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc,
                );
            }
            rewrite_expr(
                &mut spec.size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FieldType::Scalar(_) | FieldType::Tuple(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_fn_param_type(
    ty: &mut FnParamType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FnParamType::Struct(name) | FnParamType::ArrayGeneric(name) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnParamType::SizedArray {
            generic_name, size, ..
        } => {
            if let Some(name) = generic_name {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc,
                );
            }
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FnParamType::Buffer(buffer_ty) => {
            rewrite_buffer_type(
                buffer_ty,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnParamType::Primitive(_)
        | FnParamType::Array(_)
        | FnParamType::BareBuffer
        | FnParamType::Tuple(_) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_fn_return_type(
    ty: &mut FnReturnType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    match ty {
        FnReturnType::Scalar(FnReturnScalarType::Named(name)) => {
            rewrite_named_type_ref_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                loc,
            );
        }
        FnReturnType::Tuple(elems) => {
            for elem in elems {
                if let FnReturnScalarType::Named(name) = elem {
                    rewrite_named_type_ref_name(
                        name,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        loc,
                    );
                }
            }
        }
        FnReturnType::Array { size, .. } => {
            rewrite_expr(
                size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
            );
        }
        FnReturnType::Scalar(FnReturnScalarType::Primitive(_)) => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_buffer_type(
    ty: &mut BufferType,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    loc: impl Into<SourceLoc>,
) {
    let loc = loc.into();
    if let BufferElemType::Generic(name) = &mut ty.elem {
        rewrite_named_type_ref_name(
            name,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            loc,
        );
    }
    if let BufferChannels::Static(expr) = &mut ty.channels {
        rewrite_expr(
            expr,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_stmts_scoped(
    stmts: &mut Vec<Stmt>,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    local_scope: &mut RewriteNameScope,
) {
    for stmt in stmts {
        rewrite_stmt_scoped(
            stmt,
            current_ns,
            template_consts,
            options,
            state,
            generated,
            errors,
            local_scope,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_stmt_scoped(
    stmt: &mut Stmt,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    local_scope: &mut RewriteNameScope,
) {
    match stmt {
        Stmt::Const { decl, .. } => {
            if let Some(ConstType::Array { size, .. }) = &mut decl.ty {
                rewrite_expr_scoped(
                    size,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    local_scope,
                );
            }
            rewrite_expr_scoped(
                &mut decl.expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
            local_scope.insert_plain(decl.name.clone());
        }
        Stmt::Assign {
            target,
            generic_decl_ty,
            typed_decl_ty_loc,
            target_loc,
            expr,
            ..
        } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some(qualified) = resolve_visible_unqualified_const_name(
                        name,
                        current_ns,
                        state,
                        target_loc.as_ref().map(SourceLoc::from).unwrap_or_default(),
                        errors,
                    ) {
                        *name = qualified;
                    } else if looks_like_namespace_ref(name) {
                        if let Some(resolved) = resolve_namespace_symbol_name(
                            name,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                            target_loc
                                .as_ref()
                                .map(SourceLoc::from)
                                .unwrap_or_default()
                                .span(),
                        ) {
                            *name = resolved;
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some(qualified) = resolve_visible_unqualified_const_name(
                        base,
                        current_ns,
                        state,
                        target_loc.as_ref().map(SourceLoc::from).unwrap_or_default(),
                        errors,
                    ) {
                        *base = qualified;
                    } else if looks_like_namespace_ref(base) {
                        if let Some(resolved) = resolve_namespace_symbol_name(
                            base,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                            target_loc
                                .as_ref()
                                .map(SourceLoc::from)
                                .unwrap_or_default()
                                .span(),
                        ) {
                            *base = resolved;
                        }
                    }
                    rewrite_expr_scoped(
                        index,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        local_scope,
                    );
                }
                AssignTarget::Slice { base, start, end } => {
                    if let Some(qualified) = resolve_visible_unqualified_const_name(
                        base,
                        current_ns,
                        state,
                        target_loc.as_ref().map(SourceLoc::from).unwrap_or_default(),
                        errors,
                    ) {
                        *base = qualified;
                    } else if looks_like_namespace_ref(base) {
                        if let Some(resolved) = resolve_namespace_symbol_name(
                            base,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                            target_loc
                                .as_ref()
                                .map(SourceLoc::from)
                                .unwrap_or_default()
                                .span(),
                        ) {
                            *base = resolved;
                        }
                    }
                    if let Some(start) = start {
                        rewrite_expr_scoped(
                            start,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                            local_scope,
                        );
                    }
                    if let Some(end) = end {
                        rewrite_expr_scoped(
                            end,
                            current_ns,
                            template_consts,
                            options,
                            state,
                            generated,
                            errors,
                            local_scope,
                        );
                    }
                }
                AssignTarget::Tuple(_) => {}
            }
            if let Some(name) = generic_decl_ty {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    typed_decl_ty_loc
                        .as_ref()
                        .or(target_loc.as_ref())
                        .map(SourceLoc::from)
                        .unwrap_or_default(),
                );
            }
            rewrite_expr_scoped(
                expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
            local_scope.extend(assignment_target_plain_names(target));
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_expr_scoped(
                expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr_scoped(
                cond,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
            let mut then_scope = local_scope.clone();
            rewrite_stmts_scoped(
                then_branch,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &mut then_scope,
            );
            let mut else_scope = local_scope.clone();
            rewrite_stmts_scoped(
                else_branch,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &mut else_scope,
            );
            let merged = then_scope
                .names
                .intersection(&else_scope.names)
                .cloned()
                .collect::<Vec<_>>();
            local_scope.extend(merged);
        }
        Stmt::For {
            var,
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                rewrite_expr_scoped(
                    step,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    local_scope,
                );
            }
            rewrite_expr_scoped(
                start,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
            rewrite_expr_scoped(
                end,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
            let mut loop_scope = local_scope.clone();
            loop_scope.insert_plain(var.clone());
            rewrite_stmts_scoped(
                body,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &mut loop_scope,
            );
        }
        Stmt::While { cond, body, .. } => {
            rewrite_expr_scoped(
                cond,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
            let mut loop_scope = local_scope.clone();
            rewrite_stmts_scoped(
                body,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                &mut loop_scope,
            );
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_expr(
    expr: &mut Expr,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    let local_scope = RewriteNameScope::default();
    rewrite_expr_scoped(
        expr,
        current_ns,
        template_consts,
        options,
        state,
        generated,
        errors,
        &local_scope,
    );
}

#[allow(clippy::too_many_arguments)]
fn rewrite_expr_scoped(
    expr: &mut Expr,
    current_ns: &str,
    template_consts: &HashMap<String, Expr>,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    generated: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
    local_scope: &RewriteNameScope,
) {
    let use_site_loc = expr.loc();
    if let Expr::Var { name, .. } = expr {
        if let Some(value) = template_consts.get(name).cloned() {
            *expr = value.with_loc(use_site_loc);
            return;
        }
    }

    match expr {
        Expr::Var { name, .. } => {
            let qualified = if local_scope.contains_value_name(name) {
                None
            } else {
                resolve_visible_unqualified_member_name(
                    name,
                    current_ns,
                    state,
                    use_site_loc,
                    errors,
                )
            };
            if let Some(qualified) = qualified {
                *name = qualified;
            } else if looks_like_namespace_ref(name) {
                if let Some(resolved) = resolve_namespace_symbol_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_loc.span(),
                ) {
                    *name = resolved;
                }
            }
        }
        Expr::Index { base, index, .. } => {
            let qualified = if local_scope.contains_value_name(base) {
                None
            } else {
                resolve_visible_unqualified_member_name(
                    base,
                    current_ns,
                    state,
                    use_site_loc,
                    errors,
                )
            };
            if let Some(qualified) = qualified {
                *base = qualified;
            } else if looks_like_namespace_ref(base) {
                if let Some(resolved) = resolve_namespace_symbol_name(
                    base,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_loc.span(),
                ) {
                    *base = resolved;
                }
            }
            rewrite_expr_scoped(
                index,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            let qualified = if local_scope.contains_value_name(base) {
                None
            } else {
                resolve_visible_unqualified_member_name(
                    base,
                    current_ns,
                    state,
                    use_site_loc,
                    errors,
                )
            };
            if let Some(qualified) = qualified {
                *base = qualified;
            } else if looks_like_namespace_ref(base) {
                if let Some(resolved) = resolve_namespace_symbol_name(
                    base,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_loc.span(),
                ) {
                    *base = resolved;
                }
            }
            if let Some(start) = start {
                rewrite_expr_scoped(
                    start,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    local_scope,
                );
            }
            if let Some(end) = end {
                rewrite_expr_scoped(
                    end,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    local_scope,
                );
            }
        }
        Expr::ArrayCtor { loc, spec, init } => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                rewrite_named_type_ref_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    loc.as_ref(),
                );
            }
            rewrite_expr_scoped(
                &mut spec.size,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_expr_scoped(
                        value,
                        current_ns,
                        template_consts,
                        options,
                        state,
                        generated,
                        errors,
                        local_scope,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_expr_scoped(
                lhs,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
            rewrite_expr_scoped(
                rhs,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_expr_scoped(
                    arg,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    local_scope,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            qualify_instance_method_call_name(
                name,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                use_site_loc,
                local_scope,
            );
            let qualified = if local_scope.contains_value_name(name) {
                None
            } else {
                resolve_visible_unqualified_member_name(
                    name,
                    current_ns,
                    state,
                    use_site_loc,
                    errors,
                )
            };
            if let Some(qualified) = qualified {
                *name = qualified;
            } else if looks_like_namespace_ref(name) {
                if let Some(resolved) = resolve_namespace_symbol_name(
                    name,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    use_site_loc.span(),
                ) {
                    *name = resolved;
                }
            }
            for arg in args {
                rewrite_expr_scoped(
                    &mut arg.expr,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    local_scope,
                );
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            rewrite_expr_scoped(
                expr,
                current_ns,
                template_consts,
                options,
                state,
                generated,
                errors,
                local_scope,
            );
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_expr_scoped(
                    value,
                    current_ns,
                    template_consts,
                    options,
                    state,
                    generated,
                    errors,
                    local_scope,
                );
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}
