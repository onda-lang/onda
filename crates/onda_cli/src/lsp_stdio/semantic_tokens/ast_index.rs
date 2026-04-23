use std::fs;
use std::path::Path;

use onda_frontend::{
    AssignTarget, Block, BlockExec, EventDef, FunctionDef, ProcessorDef, Program, Span, Stmt,
};

use super::{SemanticScope, SemanticScopeIndex, BUILTIN_CONSTS};

pub(super) fn build_semantic_scope_index(
    program: &Program,
    current_file_key: Option<&str>,
) -> SemanticScopeIndex {
    let mut index = SemanticScopeIndex::default();
    for &name in BUILTIN_CONSTS {
        index.document_scope.consts.insert(name.to_owned());
    }

    let current_blocks = program
        .blocks
        .iter()
        .filter(|block| block_belongs_to_current_file(block, current_file_key))
        .collect::<Vec<_>>();

    for block in &current_blocks {
        collect_document_scope_symbols(block, &mut index.document_scope);
    }

    build_top_level_runtime_scope(&mut index, &current_blocks);

    for block in current_blocks {
        match block {
            Block::Def(def) => build_function_scope(&mut index, None, def),
            Block::Proc(proc_def) => build_proc_scope(&mut index, proc_def),
            Block::Struct(struct_def) => {
                let Some(owner_idx) = index.push_scope(None, struct_def.loc) else {
                    continue;
                };
                for tp in &struct_def.type_params {
                    index.scopes[owner_idx].scope.types.insert(tp.clone());
                }
                for method in &struct_def.methods {
                    build_function_scope(&mut index, Some(owner_idx), method);
                }
            }
            _ => {}
        }
    }

    index
}

pub(super) fn normalize_file_key_for_path(path: &Path) -> Option<String> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Some(normalize_file_key(&canonical.to_string_lossy()))
}

pub(super) fn normalize_file_key(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = normalized
        .strip_prefix("//?/")
        .or_else(|| normalized.strip_prefix("\\\\?\\"))
        .unwrap_or(&normalized);
    normalized.to_ascii_lowercase()
}

fn block_belongs_to_current_file(block: &Block, current_file_key: Option<&str>) -> bool {
    span_belongs_to_current_file(block.loc().span(), current_file_key)
}

fn span_belongs_to_current_file(span: Span, current_file_key: Option<&str>) -> bool {
    match current_file_key {
        Some(expected) => span
            .file()
            .map(|file| normalize_file_key(&file) == expected)
            .unwrap_or(false),
        None => true,
    }
}

pub(super) fn collect_document_scope_symbols(block: &Block, scope: &mut SemanticScope) {
    match block {
        Block::Const(decl) => {
            scope.consts.insert(decl.name.clone());
        }
        Block::Def(def) => {
            scope.functions.insert(def.name.clone());
        }
        Block::Proc(proc_def) => {
            scope.types.insert(proc_def.name.clone());
        }
        Block::Struct(struct_def) => {
            scope.types.insert(struct_def.name.clone());
            for method in &struct_def.methods {
                scope.functions.insert(method.name.clone());
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
struct RuntimeStmtRegion<'a> {
    span: Span,
    body: &'a [Stmt],
    collect_state_symbols: bool,
}

#[derive(Default)]
struct TopLevelRuntimeSections<'a> {
    span: Option<Span>,
    ports: Vec<String>,
    parameters: Vec<String>,
    stmt_regions: Vec<RuntimeStmtRegion<'a>>,
    events: Vec<&'a EventDef>,
}

fn build_top_level_runtime_scope(
    index: &mut SemanticScopeIndex,
    blocks: &[&Block],
) -> Option<usize> {
    let sections = collect_top_level_runtime_sections(blocks);
    let span = sections.span?;
    let owner_idx = create_runtime_owner_scope(index, None, span, true)?;
    {
        let owner = &mut index.scopes[owner_idx].scope;
        for name in sections.ports {
            owner.ports.insert(name);
        }
        for name in sections.parameters {
            owner.parameters.insert(name);
        }
    }
    build_runtime_stmt_regions(index, owner_idx, &sections.stmt_regions);
    for event in sections.events {
        build_event_scope(index, owner_idx, event);
    }
    Some(owner_idx)
}

fn collect_top_level_runtime_sections<'a>(blocks: &[&'a Block]) -> TopLevelRuntimeSections<'a> {
    let mut sections = TopLevelRuntimeSections::default();

    for block in blocks {
        match block {
            Block::Ins(ports) | Block::Outs(ports) => {
                extend_runtime_owner_span(&mut sections.span, block.loc().span());
                sections
                    .ports
                    .extend(ports.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Params(params) => {
                extend_runtime_owner_span(&mut sections.span, block.loc().span());
                sections
                    .parameters
                    .extend(params.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Buffers(buffers) => {
                extend_runtime_owner_span(&mut sections.span, block.loc().span());
                sections
                    .ports
                    .extend(buffers.decls.iter().map(|decl| decl.name.clone()));
            }
            Block::Init(init) => {
                extend_runtime_owner_span(&mut sections.span, init.loc);
                sections.stmt_regions.push(RuntimeStmtRegion {
                    span: init.loc,
                    body: &init.body,
                    collect_state_symbols: true,
                });
            }
            Block::Block(exec) => {
                extend_runtime_owner_span(&mut sections.span, exec.loc);
                sections
                    .stmt_regions
                    .extend(runtime_regions_for_block_exec(exec));
            }
            Block::Sample(sample) => {
                extend_runtime_owner_span(&mut sections.span, sample.loc);
                sections.stmt_regions.push(RuntimeStmtRegion {
                    span: sample.loc,
                    body: &sample.body,
                    collect_state_symbols: false,
                });
            }
            Block::Events(events) => {
                extend_runtime_owner_span(&mut sections.span, events.loc);
                sections.events.extend(events.events.iter());
            }
            Block::Graph(graph) => {
                extend_runtime_owner_span(&mut sections.span, graph.loc);
            }
            _ => {}
        }
    }

    sections
}

fn create_runtime_owner_scope(
    index: &mut SemanticScopeIndex,
    parent: Option<usize>,
    span: Span,
    allows_implicit_ports: bool,
) -> Option<usize> {
    let scope_idx = index.push_scope(parent, span)?;
    index.scopes[scope_idx].allows_implicit_ports = allows_implicit_ports;
    Some(scope_idx)
}

fn extend_runtime_owner_span(span: &mut Option<Span>, next: Span) {
    *span = Some(match *span {
        Some(current) => Span::spanning(current, next),
        None => next,
    });
}

fn runtime_regions_for_block_exec<'a>(exec: &'a BlockExec) -> Vec<RuntimeStmtRegion<'a>> {
    let mut regions = Vec::new();
    if let Some(span) = span_for_stmt_body(&exec.pre) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &exec.pre,
            collect_state_symbols: true,
        });
    }
    if let Some(sample) = &exec.sample {
        regions.push(RuntimeStmtRegion {
            span: sample.loc,
            body: &sample.body,
            collect_state_symbols: false,
        });
    }
    if let Some(span) = span_for_stmt_body(&exec.post) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &exec.post,
            collect_state_symbols: true,
        });
    }
    regions
}

fn build_proc_scope(index: &mut SemanticScopeIndex, proc_def: &ProcessorDef) {
    let Some(owner_idx) =
        create_runtime_owner_scope(index, None, span_for_proc_scope(proc_def), true)
    else {
        return;
    };
    {
        let owner = &mut index.scopes[owner_idx].scope;
        for tp in &proc_def.type_params {
            owner.types.insert(tp.clone());
        }
        for decl in &proc_def.ins {
            owner.ports.insert(decl.name.clone());
        }
        for decl in &proc_def.outs {
            owner.ports.insert(decl.name.clone());
        }
        for decl in &proc_def.params {
            owner.parameters.insert(decl.name.clone());
        }
        for buffer in &proc_def.buffers {
            owner.ports.insert(buffer.name.clone());
        }
        for def in &proc_def.local_defs {
            owner.functions.insert(def.name.clone());
        }
    }
    let stmt_regions = proc_runtime_stmt_regions(proc_def);
    build_runtime_stmt_regions(index, owner_idx, &stmt_regions);

    for event in &proc_def.events {
        build_event_scope(index, owner_idx, event);
    }
    for def in &proc_def.local_defs {
        build_function_scope(index, Some(owner_idx), def);
    }
}

fn proc_runtime_stmt_regions<'a>(proc_def: &'a ProcessorDef) -> Vec<RuntimeStmtRegion<'a>> {
    let mut regions = Vec::new();
    regions.push(RuntimeStmtRegion {
        span: proc_def.init.loc,
        body: &proc_def.init.body,
        collect_state_symbols: true,
    });
    if let Some(span) = span_for_stmt_body(&proc_def.block_pre) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &proc_def.block_pre,
            collect_state_symbols: true,
        });
    }
    if let Some(span) = span_for_stmt_body(&proc_def.sample) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &proc_def.sample,
            collect_state_symbols: false,
        });
    }
    if let Some(span) = span_for_stmt_body(&proc_def.block_post) {
        regions.push(RuntimeStmtRegion {
            span,
            body: &proc_def.block_post,
            collect_state_symbols: true,
        });
    }
    regions
}

fn build_function_scope(index: &mut SemanticScopeIndex, parent: Option<usize>, def: &FunctionDef) {
    let Some(scope_idx) = index.push_scope(parent, span_for_function_scope(def)) else {
        return;
    };
    let reserved = parent
        .map(|idx| index.scopes[idx].scope.clone())
        .unwrap_or_default();
    let allows_implicit_ports = index.scopes[scope_idx].allows_implicit_ports;
    {
        let scope = &mut index.scopes[scope_idx].scope;
        scope.functions.insert(def.name.clone());
        for tp in &def.type_params {
            scope.types.insert(tp.clone());
        }
        for param in &def.params {
            scope.parameters.insert(param.name.clone());
        }
        collect_stmt_symbols(&def.body, scope);
        prune_shadowed_variables(scope, &reserved, allows_implicit_ports);
    }
}

fn build_event_scope(index: &mut SemanticScopeIndex, parent: usize, event: &EventDef) {
    let Some(scope_idx) = index.push_scope(Some(parent), span_for_event_scope(event)) else {
        return;
    };
    let reserved = index.scopes[parent].scope.clone();
    let allows_implicit_ports = index.scopes[scope_idx].allows_implicit_ports;
    {
        let scope = &mut index.scopes[scope_idx].scope;
        scope.functions.insert(event.name.clone());
        for param in &event.params {
            scope.parameters.insert(param.name.clone());
        }
        collect_stmt_symbols(&event.body, scope);
        prune_shadowed_variables(scope, &reserved, allows_implicit_ports);
    }
}

fn build_stmt_scope(index: &mut SemanticScopeIndex, parent: usize, span: Span, stmts: &[Stmt]) {
    let Some(scope_idx) = index.push_scope(Some(parent), span) else {
        return;
    };
    let reserved = index.scopes[parent].scope.clone();
    let allows_implicit_ports = index.scopes[scope_idx].allows_implicit_ports;
    {
        let scope = &mut index.scopes[scope_idx].scope;
        collect_stmt_symbols(stmts, scope);
        prune_shadowed_variables(scope, &reserved, allows_implicit_ports);
    }
}

fn build_runtime_stmt_regions(
    index: &mut SemanticScopeIndex,
    owner_idx: usize,
    stmt_regions: &[RuntimeStmtRegion<'_>],
) {
    for region in stmt_regions {
        if region.collect_state_symbols {
            collect_runtime_state_symbols(region.body, &mut index.scopes[owner_idx].scope);
        }
        if !region.body.is_empty() {
            build_stmt_scope(index, owner_idx, region.span, region.body);
        }
    }
}

fn span_for_stmt(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::If {
            loc,
            then_branch,
            else_branch,
            ..
        } => {
            let mut span = *loc;
            if let Some(branch_span) = span_for_stmt_body(then_branch) {
                span = Span::spanning(span, branch_span);
            }
            if let Some(branch_span) = span_for_stmt_body(else_branch) {
                span = Span::spanning(span, branch_span);
            }
            span
        }
        Stmt::For { loc, body, .. } | Stmt::While { loc, body, .. } => {
            let mut span = *loc;
            if let Some(body_span) = span_for_stmt_body(body) {
                span = Span::spanning(span, body_span);
            }
            span
        }
        _ => stmt.loc().span(),
    }
}

fn span_for_stmt_body(stmts: &[Stmt]) -> Option<Span> {
    let mut iter = stmts.iter();
    let first = span_for_stmt(iter.next()?);
    Some(iter.fold(first, |span, stmt| {
        Span::spanning(span, span_for_stmt(stmt))
    }))
}

fn span_for_function_scope(def: &FunctionDef) -> Span {
    span_for_stmt_body(&def.body)
        .map(|body_span| Span::spanning(def.loc, body_span))
        .unwrap_or(def.loc)
}

fn span_for_event_scope(event: &EventDef) -> Span {
    span_for_stmt_body(&event.body)
        .map(|body_span| Span::spanning(event.loc, body_span))
        .unwrap_or(event.loc)
}

fn span_for_proc_scope(proc_def: &ProcessorDef) -> Span {
    let mut span = proc_def.loc;

    for decl in &proc_def.consts {
        span = Span::spanning(span, decl.loc);
    }
    for decl in &proc_def.ins {
        span = Span::spanning(span, decl.loc);
    }
    for decl in &proc_def.outs {
        span = Span::spanning(span, decl.loc);
    }
    for decl in &proc_def.params {
        span = Span::spanning(span, decl.loc);
    }
    for decl in &proc_def.buffers {
        span = Span::spanning(span, decl.loc);
    }

    span = Span::spanning(span, proc_def.init.loc);
    if let Some(body_span) = span_for_stmt_body(&proc_def.block_pre) {
        span = Span::spanning(span, body_span);
    }
    if let Some(body_span) = span_for_stmt_body(&proc_def.sample) {
        span = Span::spanning(span, body_span);
    }
    if let Some(body_span) = span_for_stmt_body(&proc_def.block_post) {
        span = Span::spanning(span, body_span);
    }
    if let Some(graph) = &proc_def.graph {
        span = Span::spanning(span, graph.loc);
    }
    for event in &proc_def.events {
        span = Span::spanning(span, span_for_event_scope(event));
    }
    for def in &proc_def.local_defs {
        span = Span::spanning(span, span_for_function_scope(def));
    }

    span
}

fn collect_runtime_state_symbols(stmts: &[Stmt], scope: &mut SemanticScope) {
    let mut collected = SemanticScope::default();
    collect_state_stmt_symbols(stmts, &mut collected, 0);
    scope.state_variables.extend(collected.state_variables);
}

fn prune_shadowed_variables(
    scope: &mut SemanticScope,
    reserved: &SemanticScope,
    allows_implicit_ports: bool,
) {
    scope.variables.retain(|name| {
        reserved.token_type_for(name).is_none()
            && (!allows_implicit_ports || !super::is_implicit_port_name(name))
    });
}

pub(super) fn collect_all_symbols(program: &Program) -> SemanticScope {
    let mut scope = SemanticScope::default();
    for block in &program.blocks {
        collect_block_symbols(block, &mut scope);
    }
    scope
}

fn collect_block_symbols(block: &Block, scope: &mut SemanticScope) {
    match block {
        Block::Const(decl) => {
            scope.consts.insert(decl.name.clone());
        }
        Block::Ins(ports) | Block::Outs(ports) => {
            for decl in &ports.decls {
                scope.ports.insert(decl.name.clone());
            }
        }
        Block::Params(params) => {
            for decl in &params.decls {
                scope.parameters.insert(decl.name.clone());
            }
        }
        Block::Buffers(buffers) => {
            for decl in &buffers.decls {
                scope.ports.insert(decl.name.clone());
            }
        }
        Block::Events(events) => {
            for event in &events.events {
                scope.functions.insert(event.name.clone());
                for param in &event.params {
                    scope.parameters.insert(param.name.clone());
                }
                collect_stmt_symbols(&event.body, scope);
            }
        }
        Block::Init(init) => {
            collect_state_stmt_symbols(&init.body, scope, 0);
        }
        Block::Block(exec) => {
            collect_stmt_symbols(&exec.pre, scope);
            if let Some(sample) = &exec.sample {
                collect_stmt_symbols(&sample.body, scope);
            }
            collect_stmt_symbols(&exec.post, scope);
        }
        Block::Sample(sample) => {
            collect_stmt_symbols(&sample.body, scope);
        }
        Block::Def(def) => {
            scope.functions.insert(def.name.clone());
            collect_def_symbols(def, scope);
        }
        Block::Proc(proc_def) => {
            scope.types.insert(proc_def.name.clone());
            collect_proc_symbols(proc_def, scope);
        }
        Block::Struct(struct_def) => {
            scope.types.insert(struct_def.name.clone());
            for tp in &struct_def.type_params {
                scope.types.insert(tp.clone());
            }
            for method in &struct_def.methods {
                scope.functions.insert(method.name.clone());
                collect_def_symbols(method, scope);
            }
        }
        _ => {}
    }
}

fn collect_proc_symbols(proc_def: &ProcessorDef, scope: &mut SemanticScope) {
    for tp in &proc_def.type_params {
        scope.types.insert(tp.clone());
    }
    for decl in &proc_def.ins {
        scope.ports.insert(decl.name.clone());
    }
    for decl in &proc_def.outs {
        scope.ports.insert(decl.name.clone());
    }
    for decl in &proc_def.params {
        scope.parameters.insert(decl.name.clone());
    }
    for buffer in &proc_def.buffers {
        scope.ports.insert(buffer.name.clone());
    }
    collect_state_stmt_symbols(&proc_def.init.body, scope, 0);
    collect_stmt_symbols(&proc_def.block_pre, scope);
    collect_stmt_symbols(&proc_def.sample, scope);
    collect_stmt_symbols(&proc_def.block_post, scope);
    for event in &proc_def.events {
        scope.functions.insert(event.name.clone());
        for param in &event.params {
            scope.parameters.insert(param.name.clone());
        }
        collect_stmt_symbols(&event.body, scope);
    }
    for def in &proc_def.local_defs {
        scope.functions.insert(def.name.clone());
        collect_def_symbols(def, scope);
    }
}

fn collect_def_symbols(def: &FunctionDef, scope: &mut SemanticScope) {
    for tp in &def.type_params {
        scope.types.insert(tp.clone());
    }
    for param in &def.params {
        scope.parameters.insert(param.name.clone());
    }
    collect_stmt_symbols(&def.body, scope);
}

fn collect_stmt_symbols(stmts: &[Stmt], scope: &mut SemanticScope) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => {
                scope.consts.insert(decl.name.clone());
            }
            Stmt::Assign { target, .. } => {
                collect_target_symbols(target, scope);
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_stmt_symbols(then_branch, scope);
                collect_stmt_symbols(else_branch, scope);
            }
            Stmt::For { var, body, .. } => {
                scope.variables.insert(var.clone());
                collect_stmt_symbols(body, scope);
            }
            Stmt::While { body, .. } => {
                collect_stmt_symbols(body, scope);
            }
            _ => {}
        }
    }
}

fn collect_state_stmt_symbols(stmts: &[Stmt], scope: &mut SemanticScope, scope_depth: usize) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => {
                scope.consts.insert(decl.name.clone());
            }
            Stmt::Assign { target, .. } => {
                if scope_depth == 0 {
                    collect_state_target_symbols(target, scope);
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_state_stmt_symbols(then_branch, scope, scope_depth + 1);
                collect_state_stmt_symbols(else_branch, scope, scope_depth + 1);
            }
            Stmt::For { body, .. } => {
                collect_state_stmt_symbols(body, scope, scope_depth + 1);
            }
            Stmt::While { body, .. } => {
                collect_state_stmt_symbols(body, scope, scope_depth + 1);
            }
            _ => {}
        }
    }
}

fn collect_target_symbols(target: &AssignTarget, scope: &mut SemanticScope) {
    match target {
        AssignTarget::Var(name) => {
            scope.insert_variable(name.clone());
        }
        AssignTarget::Tuple(names) => {
            for name in names {
                scope.insert_variable(name.clone());
            }
        }
        _ => {}
    }
}

fn collect_state_target_symbols(target: &AssignTarget, scope: &mut SemanticScope) {
    match target {
        AssignTarget::Var(name) => {
            scope.insert_state_variable(name.clone());
        }
        AssignTarget::Tuple(names) => {
            for name in names {
                scope.insert_state_variable(name.clone());
            }
        }
        _ => {}
    }
}
