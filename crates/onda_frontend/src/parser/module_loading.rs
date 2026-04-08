use super::loading_support::{
    annotate_diagnostics_with_file, builtin_std_module_source, display_path,
    is_builtin_std_module_path, resolve_import_path, resolve_include_path, split_top_level_items,
    validate_file_mode_transition, FileLoadMode, TopLevelItem,
};
use super::preprocess::preprocess_indentation_blocks;
use super::*;

mod namespaces;
use namespaces::*;

mod rewrite;
use rewrite::*;

#[derive(Default)]
struct LoadState {
    import_once: HashSet<PathBuf>,
    import_once_builtin: HashSet<String>,
    file_modes: HashMap<PathBuf, FileLoadMode>,
    stack: Vec<PathBuf>,
    builtin_stack: Vec<String>,
    namespace_templates: HashMap<String, NamespaceTemplateDecl>,
    namespace_aliases: HashMap<String, NamespaceAliasDecl>,
    namespace_members: HashSet<String>,
    namespace_const_values: HashMap<String, Expr>,
    top_level_const_values: HashMap<String, Expr>,
    namespace_instantiations: HashMap<String, String>,
    next_namespace_instantiation_id: u64,
}

#[derive(Debug, Clone)]
struct ParseLocContext {
    file: String,
    line_offset: usize,
    trace: Vec<String>,
    source_line_map: Vec<usize>,
}

thread_local! {
    static PARSE_LOC_CONTEXT_STACK: RefCell<Vec<ParseLocContext>> = const { RefCell::new(Vec::new()) };
}

fn block_decl_name(block: &Block) -> Option<&str> {
    match block {
        Block::Const(c) => Some(c.name.as_str()),
        Block::Struct(s) => Some(s.name.as_str()),
        Block::Def(d) => Some(d.name.as_str()),
        Block::Proc(p) => Some(p.name.as_str()),
        _ => None,
    }
}

fn parse_assert_decl(pair: Pair<'_, Rule>) -> Result<AssertDecl, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::assert_block {
        return Err(vec![syntax_at_pair(
            &pair,
            "internal parser error: expected assert block",
        )]);
    }
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(expr_pair) = inner.next() else {
        return Err(vec![syntax_at_loc(
            loc.as_ref(),
            "missing assert expression",
        )]);
    };
    Ok(AssertDecl {
        loc,
        expr: parse_expr(expr_pair)?,
    })
}

fn merge_blocks_preferring_existing(existing: &mut Vec<Block>, incoming: Vec<Block>) {
    let shadowed = existing
        .iter()
        .filter_map(block_decl_name)
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>();
    for block in incoming {
        if let Some(name) = block_decl_name(&block) {
            if shadowed.contains(name) {
                continue;
            }
        }
        existing.push(block);
    }
}

fn append_or_merge_event_block(
    blocks: &mut Vec<Block>,
    incoming: EventBlock,
) -> Result<(), Vec<Diagnostic>> {
    for block in blocks.iter_mut() {
        if let Block::Events(existing) = block {
            existing.loc = Span::spanning(existing.loc, incoming.loc);
            merge_event_defs(&mut existing.events, incoming.events)?;
            return Ok(());
        }
    }
    blocks.push(Block::Events(incoming));
    Ok(())
}

pub fn parse_program(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let virtual_path = PathBuf::from("<memory>");
    let (preprocessed, preprocessed_line_map) = preprocess_indentation_blocks(source)
        .map_err(|diags| annotate_diagnostics_with_file(diags, &virtual_path, 0))?;
    let items = split_top_level_items(&preprocessed, &preprocessed_line_map, &virtual_path)?;
    let mut blocks = Vec::<Block>::new();
    let mut state = LoadState::default();
    for item in items {
        match item {
            TopLevelItem::Code {
                text,
                start_line,
                source_line_map,
            } => {
                if text.trim().is_empty() {
                    continue;
                }
                let mut parsed = parse_program_preprocessed(
                    &text,
                    &virtual_path,
                    start_line.saturating_sub(1),
                    &[],
                    &source_line_map,
                    &mut state,
                )
                .map_err(|diags| {
                    annotate_diagnostics_with_file(
                        diags,
                        &virtual_path,
                        start_line.saturating_sub(1),
                    )
                })?;
                blocks.append(&mut parsed.blocks);
            }
            TopLevelItem::Include { path, line } => {
                return Err(annotate_diagnostics_with_file(
                    vec![Diagnostic::syntax(
                        format!("include '{path}' is only supported when compiling from a file"),
                        line,
                        1,
                    )],
                    &virtual_path,
                    0,
                ));
            }
            TopLevelItem::Import { module, line } => {
                if !is_builtin_std_module_path(&module) {
                    return Err(annotate_diagnostics_with_file(
                        vec![Diagnostic::syntax(
                            format!(
                                "import '{}' is only supported for built-in std modules when compiling from in-memory source",
                                module
                            ),
                            line,
                            1,
                        )],
                        &virtual_path,
                        0,
                    ));
                }
                if state.import_once_builtin.contains(&module) {
                    continue;
                }
                state.import_once_builtin.insert(module.clone());
                let trace = format!(
                    "import '{}' at {}:{line}",
                    module,
                    display_path(&virtual_path)
                );
                let mut imported =
                    load_builtin_module_blocks(&module, true, &mut state, &[trace.clone()])
                        .map_err(|diags| append_diagnostics_trace(diags, trace))?;
                blocks.append(&mut imported);
            }
        }
    }
    Ok(Program { blocks })
}

pub fn parse_program_with_path(source: &str, path: &Path) -> Result<Program, Vec<Diagnostic>> {
    let mut overlays = HashMap::new();
    overlays.insert(path.to_path_buf(), source.to_owned());
    parse_program_file_with_overlays(path, &overlays)
}

pub fn parse_program_file(path: &Path) -> Result<Program, Vec<Diagnostic>> {
    parse_program_file_with_overlays(path, &HashMap::new())
}

pub fn parse_program_file_with_overlays(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> Result<Program, Vec<Diagnostic>> {
    let canonical = fs::canonicalize(path).map_err(|err| {
        vec![Diagnostic::syntax(
            format!("failed to resolve '{}': {err}", path.display()),
            0,
            0,
        )]
    })?;
    let normalized_overlays = normalize_overlay_paths(path, &canonical, overlays);
    let mut state = LoadState::default();
    state
        .file_modes
        .insert(canonical.clone(), FileLoadMode::Entry);
    let blocks =
        load_program_blocks_from_file(&canonical, false, &mut state, &[], &normalized_overlays)?;
    Ok(Program { blocks })
}

pub fn inject_auto_std_prelude(program: &mut Program) -> Result<(), Vec<Diagnostic>> {
    let mut state = LoadState::default();
    for module in STDLIB_AUTO_IMPORT_MODULES {
        let imported = load_builtin_module_blocks(module, true, &mut state, &[])?;
        merge_blocks_preferring_existing(&mut program.blocks, imported);
    }
    Ok(())
}

pub fn inject_auto_std_math(program: &mut Program) -> Result<(), Vec<Diagnostic>> {
    inject_auto_std_prelude(program)
}

fn parse_program_preprocessed(
    preprocessed: &str,
    file_path: &Path,
    line_offset: usize,
    trace: &[String],
    source_line_map: &[usize],
    state: &mut LoadState,
) -> Result<Program, Vec<Diagnostic>> {
    with_parse_loc_context(file_path, line_offset, trace, source_line_map, || {
        let mut parsed = OndaParser::parse(Rule::program, &preprocessed)
            .map_err(|err| vec![diag_from_pest_error(err)])?;
        let program_pair = parsed
            .next()
            .ok_or_else(|| vec![Diagnostic::syntax("empty parse result", 1, 1)])?;

        let mut blocks = Vec::new();
        let mut top_level_consts = state.top_level_const_values.clone();
        let mut top_level_const_names = state
            .top_level_const_values
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        let mut generated = Vec::<Block>::new();
        for pair in program_pair.into_inner() {
            match pair.as_rule() {
                Rule::ins_block => blocks.push(Block::Ins(parse_port_block(pair)?)),
                Rule::outs_block => blocks.push(Block::Outs(parse_port_block(pair)?)),
                Rule::params_block => blocks.push(Block::Params(parse_params_block(pair)?)),
                Rule::const_block => {
                    let mut decl = parse_const_decl(pair)?;
                    if !top_level_const_names.insert(decl.name.clone()) {
                        return Err(vec![Diagnostic::semantic_span(
                            format!("duplicate top-level constant '{}'", decl.name),
                            decl.loc.as_ref(),
                        )]);
                    }
                    let value = finalize_const_decl_expr(
                        &mut decl,
                        "",
                        &top_level_consts,
                        state,
                        &mut generated,
                    )?;
                    top_level_consts.insert(decl.name.clone(), value);
                    state.top_level_const_values = top_level_consts.clone();
                }
                Rule::events_block => {
                    append_or_merge_event_block(&mut blocks, parse_events_block(pair)?)?
                }
                Rule::event_block => {
                    append_or_merge_event_block(&mut blocks, parse_event_block(pair)?)?
                }
                Rule::buffers_block => blocks.push(Block::Buffers(parse_buffers_block(pair)?)),
                Rule::assert_block => blocks.push(Block::Assert(parse_assert_decl(pair)?)),
                Rule::proc_block => blocks.push(Block::Proc(parse_proc_block(pair)?)),
                Rule::struct_block => blocks.push(Block::Struct(parse_struct_block(pair)?)),
                Rule::def_block => blocks.push(Block::Def(parse_def_block(pair)?)),
                Rule::namespace_block => {
                    let ns_decl = parse_namespace_decl(pair)?;
                    process_namespace_decl(ns_decl, "", &top_level_consts, state, &mut blocks)?
                }
                Rule::namespace_alias_decl => {
                    let alias = parse_namespace_alias_decl(pair)?;
                    register_namespace_alias("", alias, state)?
                }
                Rule::init_block => blocks.push(Block::Init(parse_exec_block(pair)?)),
                Rule::block_exec_block => blocks.push(Block::Block(parse_block_exec_block(pair)?)),
                Rule::sample_block => blocks.push(Block::Sample(parse_sample_block(pair)?)),
                Rule::graph_block => blocks.push(Block::Graph(parse_graph_block(pair)?)),
                _ => {}
            }
        }

        rewrite_blocks_namespace_refs(&mut blocks, "", &top_level_consts, state, &mut generated)?;
        blocks.extend(generated);

        Ok(Program { blocks })
    })
}

fn load_program_blocks_from_file(
    file_path: &Path,
    import_module_mode: bool,
    state: &mut LoadState,
    trace: &[String],
    overlays: &HashMap<PathBuf, String>,
) -> Result<Vec<Block>, Vec<Diagnostic>> {
    let canonical = fs::canonicalize(file_path).map_err(|err| {
        annotate_diagnostics_with_file(
            vec![Diagnostic::syntax(
                format!("failed to resolve '{}': {err}", display_path(file_path)),
                0,
                0,
            )],
            file_path,
            0,
        )
    })?;
    if let Some(pos) = state.stack.iter().position(|p| p == &canonical) {
        let mut chain = state.stack[pos..]
            .iter()
            .map(|p| display_path(p))
            .collect::<Vec<_>>();
        chain.push(display_path(&canonical));
        return Err(annotate_diagnostics_with_file(
            vec![Diagnostic::syntax(
                format!("circular include/import detected: {}", chain.join(" -> ")),
                0,
                0,
            )],
            &canonical,
            0,
        ));
    }
    state.stack.push(canonical.clone());

    let result = (|| {
        let source = if let Some(source) = overlays.get(&canonical) {
            source.clone()
        } else {
            fs::read_to_string(&canonical).map_err(|err| {
                annotate_diagnostics_with_file(
                    vec![Diagnostic::syntax(
                        format!("failed to read '{}': {err}", display_path(&canonical)),
                        0,
                        0,
                    )],
                    &canonical,
                    0,
                )
            })?
        };
        let (preprocessed, preprocessed_line_map) = preprocess_indentation_blocks(&source)
            .map_err(|diags| annotate_diagnostics_with_file(diags, &canonical, 0))?;
        let items = split_top_level_items(&preprocessed, &preprocessed_line_map, &canonical)?;

        let mut blocks = Vec::<Block>::new();
        for item in items {
            match item {
                TopLevelItem::Code {
                    text,
                    start_line,
                    source_line_map,
                } => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    let mut parsed = parse_program_preprocessed(
                        &text,
                        &canonical,
                        start_line.saturating_sub(1),
                        trace,
                        &source_line_map,
                        state,
                    )
                    .map_err(|diags| {
                        annotate_diagnostics_with_file(
                            diags,
                            &canonical,
                            start_line.saturating_sub(1),
                        )
                    })?;
                    blocks.append(&mut parsed.blocks);
                }
                TopLevelItem::Include { path, line } => {
                    let include_path = resolve_include_path(&canonical, &path).map_err(|msg| {
                        annotate_diagnostics_with_file(
                            vec![Diagnostic::syntax(msg, line, 1)],
                            &canonical,
                            0,
                        )
                    })?;
                    validate_file_mode_transition(
                        &include_path,
                        FileLoadMode::Include,
                        &canonical,
                        line,
                        &mut state.file_modes,
                    )?;
                    let trace_entry =
                        format!("include '{path}' at {}:{line}", display_path(&canonical));
                    let mut nested_trace = trace.to_vec();
                    nested_trace.push(trace_entry.clone());
                    let mut included = load_program_blocks_from_file(
                        &include_path,
                        import_module_mode,
                        state,
                        &nested_trace,
                        overlays,
                    )
                    .map_err(|diags| append_diagnostics_trace(diags, trace_entry))?;
                    blocks.append(&mut included);
                }
                TopLevelItem::Import { module, line } => {
                    if is_builtin_std_module_path(&module) {
                        if state.import_once_builtin.contains(&module) {
                            continue;
                        }
                        state.import_once_builtin.insert(module.clone());
                        let trace_entry =
                            format!("import '{module}' at {}:{line}", display_path(&canonical));
                        let mut nested_trace = trace.to_vec();
                        nested_trace.push(trace_entry.clone());
                        let mut imported =
                            load_builtin_module_blocks(&module, true, state, &nested_trace)
                                .map_err(|diags| append_diagnostics_trace(diags, trace_entry))?;
                        blocks.append(&mut imported);
                        continue;
                    }
                    let import_path = resolve_import_path(&canonical, &module).map_err(|msg| {
                        annotate_diagnostics_with_file(
                            vec![Diagnostic::syntax(msg, line, 1)],
                            &canonical,
                            0,
                        )
                    })?;
                    validate_file_mode_transition(
                        &import_path,
                        FileLoadMode::Import,
                        &canonical,
                        line,
                        &mut state.file_modes,
                    )?;
                    if state.import_once.contains(&import_path) {
                        continue;
                    }
                    state.import_once.insert(import_path.clone());
                    let trace_entry =
                        format!("import '{module}' at {}:{line}", display_path(&canonical));
                    let mut nested_trace = trace.to_vec();
                    nested_trace.push(trace_entry.clone());
                    let mut imported = load_program_blocks_from_file(
                        &import_path,
                        true,
                        state,
                        &nested_trace,
                        overlays,
                    )
                    .map_err(|diags| append_diagnostics_trace(diags, trace_entry))?;
                    blocks.append(&mut imported);
                }
            }
        }

        if import_module_mode {
            for block in &blocks {
                if !matches!(
                    block,
                    Block::Const(_) | Block::Struct(_) | Block::Def(_) | Block::Proc(_)
                ) {
                    return Err(annotate_diagnostics_with_file(
                        vec![Diagnostic::semantic_span(
                            format!(
                                "imported file '{}' can only contain const/struct/def/proc declarations",
                                display_path(&canonical)
                            ),
                            block.loc(),
                        )],
                        &canonical,
                        0,
                    ));
                }
            }
        }

        Ok(blocks)
    })();

    state.stack.pop();
    result
}

fn normalize_overlay_paths(
    requested_path: &Path,
    canonical_requested_path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> HashMap<PathBuf, String> {
    let mut normalized = HashMap::with_capacity(overlays.len());
    for (path, source) in overlays {
        let key = if path == requested_path {
            canonical_requested_path.to_path_buf()
        } else {
            fs::canonicalize(path).unwrap_or_else(|_| path.clone())
        };
        normalized.insert(key, source.clone());
    }
    normalized
}

fn load_builtin_module_blocks(
    module: &str,
    import_module_mode: bool,
    state: &mut LoadState,
    trace: &[String],
) -> Result<Vec<Block>, Vec<Diagnostic>> {
    let virtual_path = PathBuf::from(format!("<{module}.onda>"));
    let Some(source) = builtin_std_module_source(module) else {
        return Err(annotate_diagnostics_with_file(
            vec![Diagnostic::syntax(
                format!("unknown built-in std module '{module}'"),
                0,
                0,
            )],
            &virtual_path,
            0,
        ));
    };
    if let Some(pos) = state.builtin_stack.iter().position(|m| m == module) {
        let mut chain = state.builtin_stack[pos..].to_vec();
        chain.push(module.to_owned());
        return Err(annotate_diagnostics_with_file(
            vec![Diagnostic::syntax(
                format!(
                    "circular include/import detected in built-in std modules: {}",
                    chain.join(" -> ")
                ),
                0,
                0,
            )],
            &virtual_path,
            0,
        ));
    }
    state.builtin_stack.push(module.to_owned());

    let result = (|| {
        let (preprocessed, preprocessed_line_map) = preprocess_indentation_blocks(source)
            .map_err(|diags| annotate_diagnostics_with_file(diags, &virtual_path, 0))?;
        let items = split_top_level_items(&preprocessed, &preprocessed_line_map, &virtual_path)?;
        let mut blocks = Vec::<Block>::new();
        for item in items {
            match item {
                TopLevelItem::Code {
                    text,
                    start_line,
                    source_line_map,
                } => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    let mut parsed = parse_program_preprocessed(
                        &text,
                        &virtual_path,
                        start_line.saturating_sub(1),
                        trace,
                        &source_line_map,
                        state,
                    )
                    .map_err(|diags| {
                        annotate_diagnostics_with_file(
                            diags,
                            &virtual_path,
                            start_line.saturating_sub(1),
                        )
                    })?;
                    blocks.append(&mut parsed.blocks);
                }
                TopLevelItem::Include { path, .. } => {
                    return Err(annotate_diagnostics_with_file(
                        vec![Diagnostic::semantic(
                            format!(
                                "built-in std module '{}' cannot include '{}'; use import std/... instead",
                                module, path
                            ),
                            1,
                            1,
                        )],
                        &virtual_path,
                        0,
                    ));
                }
                TopLevelItem::Import {
                    module: imported_module,
                    line,
                } => {
                    if !is_builtin_std_module_path(&imported_module) {
                        return Err(annotate_diagnostics_with_file(
                            vec![Diagnostic::semantic(
                                format!(
                                    "built-in std module imports must use 'std/...'; got '{}'",
                                    imported_module
                                ),
                                line,
                                1,
                            )],
                            &virtual_path,
                            0,
                        ));
                    }
                    if state.import_once_builtin.contains(&imported_module) {
                        continue;
                    }
                    state.import_once_builtin.insert(imported_module.clone());
                    let trace_entry = format!(
                        "import '{}' at {}:{line}",
                        imported_module,
                        display_path(&virtual_path)
                    );
                    let mut nested_trace = trace.to_vec();
                    nested_trace.push(trace_entry.clone());
                    let mut imported =
                        load_builtin_module_blocks(&imported_module, true, state, &nested_trace)
                            .map_err(|diags| append_diagnostics_trace(diags, trace_entry))?;
                    blocks.append(&mut imported);
                }
            }
        }
        if import_module_mode {
            for block in &blocks {
                if !matches!(
                    block,
                    Block::Const(_) | Block::Struct(_) | Block::Def(_) | Block::Proc(_)
                ) {
                    return Err(annotate_diagnostics_with_file(
                        vec![Diagnostic::semantic_span(
                            format!(
                                "imported built-in std module '{module}' can only contain const/struct/def/proc declarations"
                            ),
                            block.loc(),
                        )],
                        &virtual_path,
                        0,
                    ));
                }
            }
        }
        Ok(blocks)
    })();

    state.builtin_stack.pop();
    result
}

fn append_diagnostics_trace(mut diags: Vec<Diagnostic>, trace_entry: String) -> Vec<Diagnostic> {
    for diag in &mut diags {
        diag.trace.push(trace_entry.clone());
    }
    diags
}

fn with_parse_loc_context<T>(
    file_path: &Path,
    line_offset: usize,
    trace: &[String],
    source_line_map: &[usize],
    f: impl FnOnce() -> T,
) -> T {
    struct ParseLocContextGuard;

    impl Drop for ParseLocContextGuard {
        fn drop(&mut self) {
            PARSE_LOC_CONTEXT_STACK.with(|stack| {
                stack.borrow_mut().pop();
            });
        }
    }

    let context = ParseLocContext {
        file: display_path(file_path),
        line_offset,
        trace: trace.to_vec(),
        source_line_map: source_line_map.to_vec(),
    };
    PARSE_LOC_CONTEXT_STACK.with(|stack| stack.borrow_mut().push(context));
    let _guard = ParseLocContextGuard;
    f()
}

pub(super) fn stmt_loc_from_pair(pair: &Pair<'_, Rule>) -> Span {
    PARSE_LOC_CONTEXT_STACK.with(|stack| {
        let context = stack.borrow();
        let Some(current) = context.last() else {
            return Span::ZERO;
        };
        let span = pair.as_span();
        let (line, column) = span.start_pos().line_col();
        let (end_line, end_column) = span.end_pos().line_col();
        let mapped_line = current
            .source_line_map
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or_else(|| line.saturating_add(current.line_offset));
        let mapped_end_line = current
            .source_line_map
            .get(end_line.saturating_sub(1))
            .copied()
            .unwrap_or_else(|| end_line.saturating_add(current.line_offset));
        SourceLoc::new(
            Some(current.file.clone()),
            mapped_line,
            column,
            mapped_end_line,
            end_column,
            current.trace.clone(),
        )
        .span()
    })
}

pub(super) fn parse_loc_from_raw(
    line: usize,
    column: usize,
    end_line: usize,
    end_column: usize,
) -> SourceLoc {
    PARSE_LOC_CONTEXT_STACK.with(|stack| {
        let context = stack.borrow();
        let Some(current) = context.last() else {
            return SourceLoc::new(None, line, column, end_line, end_column, Vec::new());
        };
        let mapped_line = current
            .source_line_map
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or_else(|| line.saturating_add(current.line_offset));
        let mapped_end_line = current
            .source_line_map
            .get(end_line.saturating_sub(1))
            .copied()
            .unwrap_or_else(|| end_line.saturating_add(current.line_offset));
        SourceLoc::new(
            Some(current.file.clone()),
            mapped_line,
            column,
            mapped_end_line,
            end_column,
            current.trace.clone(),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::path::Path;

    use super::*;

    #[test]
    fn parse_loc_context_stack_is_cleared_after_panic() {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            with_parse_loc_context(Path::new("<memory>"), 0, &[], &[1], || panic!("boom"));
        }));

        PARSE_LOC_CONTEXT_STACK.with(|stack| {
            assert!(stack.borrow().is_empty());
        });
    }
}
