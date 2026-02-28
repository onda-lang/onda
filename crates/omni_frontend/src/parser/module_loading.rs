use super::*;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum FileLoadMode {
    Entry,
    Include,
    Import,
}

#[derive(Debug, Clone)]
enum TopLevelItem {
    Code {
        text: String,
        start_line: usize,
        source_line_map: Vec<usize>,
    },
    Include {
        path: String,
        line: usize,
    },
    Import {
        module: String,
        line: usize,
    },
}

#[derive(Default)]
struct LoadState {
    import_once: HashSet<PathBuf>,
    import_once_builtin: HashSet<String>,
    file_modes: HashMap<PathBuf, FileLoadMode>,
    stack: Vec<PathBuf>,
    builtin_stack: Vec<String>,
    namespace_templates: HashMap<String, NamespaceTemplateDecl>,
    namespace_aliases: HashMap<String, NamespaceAliasDecl>,
    namespace_instantiations: HashMap<String, String>,
    next_namespace_instantiation_id: u64,
}

#[derive(Debug, Clone)]
struct NamespaceTemplateDecl {
    name: String,
    params: Vec<NamespaceTemplateParam>,
    items: Vec<NamespaceDeclItem>,
    captured_consts: HashMap<String, Expr>,
}

#[derive(Debug, Clone)]
struct NamespaceTemplateParam {
    name: String,
    default: Expr,
}

#[derive(Debug, Clone)]
enum NamespaceDeclItem {
    Struct(StructDef),
    Def(FunctionDef),
    Proc(ProcessorDef),
    Namespace(NamespaceTemplateDecl),
    Alias(NamespaceAliasLocalDecl),
}

#[derive(Debug, Clone)]
struct NamespaceAliasLocalDecl {
    name: String,
    target: Vec<NamespaceRefSegment>,
}

#[derive(Debug, Clone)]
struct NamespaceAliasDecl {
    declared_ns: String,
    target: Vec<NamespaceRefSegment>,
}

#[derive(Debug, Clone)]
struct NamespaceRefSegment {
    name: String,
    args: Option<Vec<NamespaceCallArg>>,
}

#[derive(Debug, Clone)]
struct NamespaceCallArg {
    name: Option<String>,
    expr: Expr,
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

fn builtin_std_module_source(module: &str) -> Option<&'static str> {
    match module {
        "std/math" => Some(include_str!("../../../../stdlib/std/math.omni")),
        "std/osc" => Some(include_str!("../../../../stdlib/std/osc.omni")),
        "std/filter" => Some(include_str!("../../../../stdlib/std/filter.omni")),
        "std/env" => Some(include_str!("../../../../stdlib/std/env.omni")),
        "std/delay" => Some(include_str!("../../../../stdlib/std/delay.omni")),
        "std/data" => Some(include_str!("../../../../stdlib/std/data.omni")),
        "std/lookup" => Some(include_str!("../../../../stdlib/std/lookup.omni")),
        "std/prelude" => Some(include_str!("../../../../stdlib/std/prelude.omni")),
        _ => None,
    }
}

fn is_builtin_std_module_path(module: &str) -> bool {
    module.starts_with(STDLIB_MODULE_PREFIX)
}

fn block_decl_name(block: &Block) -> Option<&str> {
    match block {
        Block::Struct(s) => Some(s.name.as_str()),
        Block::Def(d) => Some(d.name.as_str()),
        Block::Proc(p) => Some(p.name.as_str()),
        _ => None,
    }
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

fn preprocess_indentation_blocks(source: &str) -> Result<(String, Vec<usize>), Vec<Diagnostic>> {
    fn push_mapped_line(
        out: &mut String,
        line_map: &mut Vec<usize>,
        text: &str,
        source_line: usize,
    ) {
        out.push_str(text);
        out.push('\n');
        line_map.push(source_line);
    }

    let mut out = String::with_capacity(source.len() + source.len() / 4);
    let mut line_map = Vec::<usize>::new();
    let mut indent_stack = vec![0usize];
    let mut pending: Option<PendingIndentBlock> = None;
    let mut last_source_line = 1usize;

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        last_source_line = line_no;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (code_part, _comment_part) = split_comment(line);
        let is_comment_or_blank = code_part.trim().is_empty();

        if is_comment_or_blank {
            push_mapped_line(&mut out, &mut line_map, line, line_no);
            continue;
        }

        let indent_width = leading_indent_width(code_part);

        if let Some(pending_block) = pending.take() {
            if indent_width <= pending_block.indent {
                return Err(vec![Diagnostic::syntax(
                    "expected indented block after ':'",
                    pending_block.line,
                    1,
                )]);
            }
            indent_stack.push(indent_width);
        } else if indent_stack.len() > 1 {
            while indent_stack.len() > 1 && indent_width < *indent_stack.last().unwrap_or(&0) {
                indent_stack.pop();
                push_mapped_line(&mut out, &mut line_map, "}", line_no);
            }
            if indent_stack.len() > 1 && indent_width != *indent_stack.last().unwrap_or(&0) {
                return Err(vec![Diagnostic::syntax(
                    "inconsistent indentation level",
                    line_no,
                    1,
                )]);
            }
        }

        let trimmed_code = code_part.trim_end();
        if trimmed_code.ends_with(':') {
            let header = trimmed_code[..trimmed_code.len() - 1].trim_end();
            if header.is_empty() {
                return Err(vec![Diagnostic::syntax(
                    "missing block header before ':'",
                    line_no,
                    1,
                )]);
            }
            let header_line = format!("{header} {{");
            push_mapped_line(&mut out, &mut line_map, &header_line, line_no);
            pending = Some(PendingIndentBlock {
                line: line_no,
                indent: indent_width,
            });
        } else {
            push_mapped_line(&mut out, &mut line_map, line, line_no);
        }
    }

    if let Some(pending_block) = pending {
        return Err(vec![Diagnostic::syntax(
            "expected indented block after ':'",
            pending_block.line,
            1,
        )]);
    }

    while indent_stack.len() > 1 {
        indent_stack.pop();
        push_mapped_line(&mut out, &mut line_map, "}", last_source_line);
    }

    Ok((out, line_map))
}

fn split_comment(line: &str) -> (&str, Option<&str>) {
    if let Some(idx) = line.find('#') {
        (&line[..idx], Some(&line[idx + 1..]))
    } else {
        (line, None)
    }
}

fn leading_indent_width(line: &str) -> usize {
    let mut width = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => width += 1,
            '\t' => width += 2,
            _ => break,
        }
    }
    width
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

pub fn parse_program_file(path: &Path) -> Result<Program, Vec<Diagnostic>> {
    let canonical = fs::canonicalize(path).map_err(|err| {
        vec![Diagnostic::syntax(
            format!("failed to resolve '{}': {err}", path.display()),
            0,
            0,
        )]
    })?;
    let mut state = LoadState::default();
    state
        .file_modes
        .insert(canonical.clone(), FileLoadMode::Entry);
    let blocks = load_program_blocks_from_file(&canonical, false, &mut state, &[])?;
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
        let mut parsed = OmniParser::parse(Rule::program, &preprocessed)
            .map_err(|err| vec![diag_from_pest_error(err)])?;
        let program_pair = parsed
            .next()
            .ok_or_else(|| vec![Diagnostic::syntax("empty parse result", 1, 1)])?;

        let mut blocks = Vec::new();
        for pair in program_pair.into_inner() {
            match pair.as_rule() {
                Rule::ins_block => blocks.push(Block::Ins(parse_port_block(pair)?)),
                Rule::outs_block => blocks.push(Block::Outs(parse_port_block(pair)?)),
                Rule::params_block => blocks.push(Block::Params(parse_params_block(pair)?)),
                Rule::events_block => blocks.push(Block::Events(parse_events_block(pair)?)),
                Rule::buffers_block => blocks.push(Block::Buffers(parse_buffers_block(pair)?)),
                Rule::proc_block => blocks.push(Block::Proc(parse_proc_block(pair)?)),
                Rule::struct_block => blocks.push(Block::Struct(parse_struct_block(pair)?)),
                Rule::def_block => blocks.push(Block::Def(parse_def_block(pair)?)),
                Rule::namespace_block => {
                    let ns_decl = parse_namespace_decl(pair)?;
                    process_namespace_decl(ns_decl, "", state, &mut blocks)?
                }
                Rule::namespace_alias_decl => {
                    let alias = parse_namespace_alias_decl(pair)?;
                    register_namespace_alias("", alias, state)?
                }
                Rule::init_block => blocks.push(Block::Init(parse_exec_block(pair)?)),
                Rule::block_exec_block => blocks.push(Block::Block(parse_block_exec_block(pair)?)),
                Rule::sample_block => blocks.push(Block::Sample(parse_sample_block(pair)?)),
                _ => {}
            }
        }

        let mut generated = Vec::<Block>::new();
        rewrite_blocks_namespace_refs(&mut blocks, "", &HashMap::new(), state, &mut generated)?;
        blocks.extend(generated);

        Ok(Program { blocks })
    })
}

fn load_program_blocks_from_file(
    file_path: &Path,
    import_module_mode: bool,
    state: &mut LoadState,
    trace: &[String],
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
        let source = fs::read_to_string(&canonical).map_err(|err| {
            annotate_diagnostics_with_file(
                vec![Diagnostic::syntax(
                    format!("failed to read '{}': {err}", display_path(&canonical)),
                    0,
                    0,
                )],
                &canonical,
                0,
            )
        })?;
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
                        state,
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
                        state,
                    )?;
                    if state.import_once.contains(&import_path) {
                        continue;
                    }
                    state.import_once.insert(import_path.clone());
                    let trace_entry =
                        format!("import '{module}' at {}:{line}", display_path(&canonical));
                    let mut nested_trace = trace.to_vec();
                    nested_trace.push(trace_entry.clone());
                    let mut imported =
                        load_program_blocks_from_file(&import_path, true, state, &nested_trace)
                            .map_err(|diags| append_diagnostics_trace(diags, trace_entry))?;
                    blocks.append(&mut imported);
                }
            }
        }

        if import_module_mode {
            for block in &blocks {
                if !matches!(block, Block::Struct(_) | Block::Def(_) | Block::Proc(_)) {
                    return Err(annotate_diagnostics_with_file(
                        vec![Diagnostic::semantic(
                            format!(
                                "imported file '{}' can only contain struct/def/proc declarations",
                                display_path(&canonical)
                            ),
                            0,
                            0,
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

fn load_builtin_module_blocks(
    module: &str,
    import_module_mode: bool,
    state: &mut LoadState,
    trace: &[String],
) -> Result<Vec<Block>, Vec<Diagnostic>> {
    let virtual_path = PathBuf::from(format!("<{module}.omni>"));
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
                            0,
                            0,
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
                                0,
                                0,
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
                if !matches!(block, Block::Struct(_) | Block::Def(_) | Block::Proc(_)) {
                    return Err(annotate_diagnostics_with_file(
                        vec![Diagnostic::semantic(
                            format!(
                                "imported built-in std module '{module}' can only contain struct/def/proc declarations"
                            ),
                            0,
                            0,
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

fn annotate_diagnostics_with_file(
    mut diags: Vec<Diagnostic>,
    file_path: &Path,
    line_offset: usize,
) -> Vec<Diagnostic> {
    let file = display_path(file_path);
    for diag in &mut diags {
        if diag.file.is_none() {
            if diag.line > 0 {
                diag.line += line_offset;
            }
            diag.file = Some(file.clone());
        }
    }
    diags
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
    let context = ParseLocContext {
        file: display_path(file_path),
        line_offset,
        trace: trace.to_vec(),
        source_line_map: source_line_map.to_vec(),
    };
    PARSE_LOC_CONTEXT_STACK.with(|stack| stack.borrow_mut().push(context));
    let out = f();
    PARSE_LOC_CONTEXT_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    out
}

pub(super) fn stmt_loc_from_pair(pair: &Pair<'_, Rule>) -> Option<SourceLoc> {
    PARSE_LOC_CONTEXT_STACK.with(|stack| {
        let context = stack.borrow();
        let current = context.last()?;
        let (line, column) = pair.as_span().start_pos().line_col();
        let mapped_line = current
            .source_line_map
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or_else(|| line.saturating_add(current.line_offset));
        Some(SourceLoc {
            file: Some(current.file.clone()),
            line: mapped_line,
            column,
            trace: current.trace.clone(),
        })
    })
}

fn split_top_level_items(
    preprocessed: &str,
    preprocessed_line_map: &[usize],
    file_path: &Path,
) -> Result<Vec<TopLevelItem>, Vec<Diagnostic>> {
    let mut items = Vec::<TopLevelItem>::new();
    let mut code = String::new();
    let mut code_line_map = Vec::<usize>::new();
    let mut code_start_line = 1usize;
    let mut brace_depth: i32 = 0;

    for (idx, raw_line) in preprocessed.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (code_part, _) = split_comment(line);
        let trimmed = code_part.trim();

        if brace_depth == 0 {
            if let Some(item) = parse_top_level_directive(trimmed, line_no)
                .map_err(|diags| annotate_diagnostics_with_file(diags, file_path, 0))?
            {
                if !code.trim().is_empty() {
                    items.push(TopLevelItem::Code {
                        text: code.clone(),
                        start_line: code_start_line,
                        source_line_map: code_line_map.clone(),
                    });
                    code.clear();
                    code_line_map.clear();
                }
                items.push(item);
                continue;
            }
        }

        if code.is_empty() {
            code_start_line = line_no;
        }
        code.push_str(line);
        code.push('\n');
        code_line_map.push(preprocessed_line_map.get(idx).copied().unwrap_or(line_no));

        for ch in code_part.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        if brace_depth < 0 {
            return Err(annotate_diagnostics_with_file(
                vec![Diagnostic::syntax("unmatched '}'", line_no, 1)],
                file_path,
                0,
            ));
        }
    }

    if !code.trim().is_empty() {
        items.push(TopLevelItem::Code {
            text: code,
            start_line: code_start_line,
            source_line_map: code_line_map,
        });
    }
    Ok(items)
}

fn parse_top_level_directive(
    trimmed: &str,
    line_no: usize,
) -> Result<Option<TopLevelItem>, Vec<Diagnostic>> {
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut parts = trimmed.split_whitespace();
    let Some(keyword) = parts.next() else {
        return Ok(None);
    };

    if keyword == "import" {
        let Some(module) = parts.next() else {
            return Err(vec![Diagnostic::syntax(
                "import requires module path",
                line_no,
                1,
            )]);
        };
        if parts.next().is_some() {
            return Err(vec![Diagnostic::syntax(
                "import expects a single module path",
                line_no,
                1,
            )]);
        }
        if module.contains('\\') {
            return Err(vec![Diagnostic::syntax(
                "import path must use '/' separators",
                line_no,
                1,
            )]);
        }
        if module.ends_with(".omni") {
            return Err(vec![Diagnostic::syntax(
                "import expects module path without '.omni' suffix",
                line_no,
                1,
            )]);
        }
        return Ok(Some(TopLevelItem::Import {
            module: module.to_owned(),
            line: line_no,
        }));
    }

    if keyword == "include" {
        let rest = trimmed["include".len()..].trim();
        if !rest.starts_with('"') || !rest.ends_with('"') || rest.len() < 2 {
            return Err(vec![Diagnostic::syntax(
                "include expects quoted file path, for example include \"lib.omni\"",
                line_no,
                1,
            )]);
        }
        let include_path = &rest[1..rest.len() - 1];
        if include_path.is_empty() {
            return Err(vec![Diagnostic::syntax(
                "include path cannot be empty",
                line_no,
                1,
            )]);
        }
        if include_path.contains('\\') {
            return Err(vec![Diagnostic::syntax(
                "include path must use '/' separators",
                line_no,
                1,
            )]);
        }
        if !include_path.ends_with(".omni") {
            return Err(vec![Diagnostic::syntax(
                "include path must end with '.omni'",
                line_no,
                1,
            )]);
        }
        return Ok(Some(TopLevelItem::Include {
            path: include_path.to_owned(),
            line: line_no,
        }));
    }

    Ok(None)
}

fn resolve_include_path(current_file: &Path, include_path: &str) -> Result<PathBuf, String> {
    let include = PathBuf::from(include_path);
    let resolved = if include.is_absolute() {
        include
    } else {
        current_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(include)
    };
    fs::canonicalize(&resolved)
        .map_err(|err| format!("failed to resolve include '{}': {err}", resolved.display()))
}

fn resolve_import_path(current_file: &Path, module_path: &str) -> Result<PathBuf, String> {
    let mut module_file = PathBuf::from(format!("{module_path}.omni"));
    if !module_file.is_absolute() {
        module_file = current_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(module_file);
    }
    fs::canonicalize(&module_file).map_err(|err| {
        format!(
            "failed to resolve import '{}': {err}",
            module_file.display()
        )
    })
}

fn validate_file_mode_transition(
    target: &Path,
    mode: FileLoadMode,
    source_file: &Path,
    line: usize,
    state: &mut LoadState,
) -> Result<(), Vec<Diagnostic>> {
    if let Some(existing) = state.file_modes.get(target).copied() {
        match (existing, mode) {
            (FileLoadMode::Import, FileLoadMode::Include)
            | (FileLoadMode::Include, FileLoadMode::Import) => {
                return Err(annotate_diagnostics_with_file(
                    vec![Diagnostic::syntax(
                        format!(
                            "file '{}' cannot be both imported and included (referenced from '{}')",
                            display_path(target),
                            display_path(source_file)
                        ),
                        line,
                        1,
                    )],
                    source_file,
                    0,
                ));
            }
            _ => {}
        }
    } else {
        state.file_modes.insert(target.to_owned(), mode);
    }
    Ok(())
}

fn namespace_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}::{child}")
    }
}

fn display_path(path: &Path) -> String {
    let raw = path.display().to_string();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_owned()
}

fn parse_namespace_decl(
    block_pair: Pair<'_, Rule>,
) -> Result<NamespaceTemplateDecl, Vec<Diagnostic>> {
    if block_pair.as_rule() != Rule::namespace_block {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected namespace block",
            0,
            0,
        )]);
    }
    let mut inner = block_pair.into_inner();
    let Some(head_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing namespace name", 0, 0)]);
    };
    let (name, params) = parse_namespace_decl_head(head_pair)?;
    let mut items = Vec::<NamespaceDeclItem>::new();
    for item in inner {
        match item.as_rule() {
            Rule::struct_block => items.push(NamespaceDeclItem::Struct(parse_struct_block(item)?)),
            Rule::def_block => items.push(NamespaceDeclItem::Def(parse_def_block(item)?)),
            Rule::proc_block => items.push(NamespaceDeclItem::Proc(parse_proc_block(item)?)),
            Rule::namespace_block => {
                items.push(NamespaceDeclItem::Namespace(parse_namespace_decl(item)?))
            }
            Rule::namespace_alias_decl => {
                items.push(NamespaceDeclItem::Alias(parse_namespace_alias_decl(item)?))
            }
            _ => {}
        }
    }
    Ok(NamespaceTemplateDecl {
        name,
        params,
        items,
        captured_consts: HashMap::new(),
    })
}

fn parse_namespace_decl_head(
    pair: Pair<'_, Rule>,
) -> Result<(String, Vec<NamespaceTemplateParam>), Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_decl_head {
        return Err(vec![Diagnostic::syntax(
            "missing namespace declaration head",
            0,
            0,
        )]);
    }
    let mut name = None::<String>;
    let mut params = Vec::<NamespaceTemplateParam>::new();
    for item in pair.into_inner() {
        match item.as_rule() {
            Rule::namespace_name => name = Some(item.as_str().to_owned()),
            Rule::namespace_param_list => {
                params = parse_namespace_param_list(item)?;
            }
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![Diagnostic::syntax("missing namespace name", 0, 0)]);
    };
    Ok((name, params))
}

fn parse_namespace_param_list(
    pair: Pair<'_, Rule>,
) -> Result<Vec<NamespaceTemplateParam>, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_param_list {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected namespace parameter list",
            0,
            0,
        )]);
    }
    let mut out = Vec::<NamespaceTemplateParam>::new();
    let mut seen = HashSet::<String>::new();
    for item in pair.into_inner() {
        if item.as_rule() != Rule::namespace_param_decl {
            continue;
        }
        let mut name = None::<String>;
        let mut default = None::<Expr>;
        for part in item.into_inner() {
            match part.as_rule() {
                Rule::ident => name = Some(part.as_str().to_owned()),
                Rule::expr => default = Some(parse_expr(part)?),
                _ => {}
            }
        }
        let Some(name) = name else {
            return Err(vec![Diagnostic::syntax(
                "missing namespace template parameter name",
                0,
                0,
            )]);
        };
        if !seen.insert(name.clone()) {
            return Err(vec![Diagnostic::syntax(
                format!("duplicate namespace template parameter '{name}'"),
                0,
                0,
            )]);
        }
        let Some(default) = default else {
            return Err(vec![Diagnostic::syntax(
                format!("namespace template parameter '{name}' must define a default"),
                0,
                0,
            )]);
        };
        out.push(NamespaceTemplateParam { name, default });
    }
    Ok(out)
}

fn parse_namespace_alias_decl(
    pair: Pair<'_, Rule>,
) -> Result<NamespaceAliasLocalDecl, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_alias_decl {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected namespace alias declaration",
            0,
            0,
        )]);
    }
    let mut name = None::<String>;
    let mut target = None::<Vec<NamespaceRefSegment>>;
    for item in pair.into_inner() {
        match item.as_rule() {
            Rule::ident => name = Some(item.as_str().to_owned()),
            Rule::namespace_any_ref | Rule::namespace_ref => {
                target = Some(parse_namespace_ref_pair(item)?)
            }
            _ => {}
        }
    }
    let Some(name) = name else {
        return Err(vec![Diagnostic::syntax(
            "missing namespace alias name",
            0,
            0,
        )]);
    };
    let Some(target) = target else {
        return Err(vec![Diagnostic::syntax(
            "missing namespace alias target",
            0,
            0,
        )]);
    };
    Ok(NamespaceAliasLocalDecl { name, target })
}

fn parse_namespace_ref_pair(
    pair: Pair<'_, Rule>,
) -> Result<Vec<NamespaceRefSegment>, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::namespace_ref && pair.as_rule() != Rule::namespace_any_ref {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected namespace reference",
            0,
            0,
        )]);
    }
    let mut out = Vec::<NamespaceRefSegment>::new();
    for seg in pair.into_inner() {
        if seg.as_rule() != Rule::namespace_ref_segment {
            continue;
        }
        let mut name = None::<String>;
        let mut args = None::<Vec<NamespaceCallArg>>;
        for item in seg.into_inner() {
            match item.as_rule() {
                Rule::ident => name = Some(item.as_str().to_owned()),
                Rule::namespace_call_arg_list => {
                    let mut parsed = Vec::<NamespaceCallArg>::new();
                    for arg in item.into_inner() {
                        if arg.as_rule() != Rule::namespace_call_arg {
                            continue;
                        }
                        let mut arg_name = None::<String>;
                        let mut arg_expr = None::<Expr>;
                        for part in arg.into_inner() {
                            match part.as_rule() {
                                Rule::ident => arg_name = Some(part.as_str().to_owned()),
                                Rule::expr => arg_expr = Some(parse_expr(part)?),
                                _ => {}
                            }
                        }
                        let Some(arg_expr) = arg_expr else {
                            return Err(vec![Diagnostic::syntax(
                                "missing namespace template argument expression",
                                0,
                                0,
                            )]);
                        };
                        parsed.push(NamespaceCallArg {
                            name: arg_name,
                            expr: arg_expr,
                        });
                    }
                    args = Some(parsed);
                }
                _ => {}
            }
        }
        let Some(name) = name else {
            return Err(vec![Diagnostic::syntax(
                "missing namespace path segment name",
                0,
                0,
            )]);
        };
        out.push(NamespaceRefSegment { name, args });
    }
    Ok(out)
}

fn process_namespace_decl(
    decl: NamespaceTemplateDecl,
    parent_ns: &str,
    state: &mut LoadState,
    out: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    let full_ns = namespace_join(parent_ns, &decl.name);
    if decl.params.is_empty() {
        emit_namespace_items(&decl.items, &full_ns, state, out)
    } else {
        if state.namespace_templates.contains_key(&full_ns) {
            return Err(vec![Diagnostic::semantic(
                format!("duplicate namespace template '{full_ns}'"),
                0,
                0,
            )]);
        }
        state.namespace_templates.insert(full_ns, decl);
        Ok(())
    }
}

fn emit_namespace_items(
    items: &[NamespaceDeclItem],
    ns: &str,
    state: &mut LoadState,
    out: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for item in items {
        match item {
            NamespaceDeclItem::Struct(s) => {
                let mut s = s.clone();
                s.name = namespace_join(ns, &s.name);
                out.push(Block::Struct(s));
            }
            NamespaceDeclItem::Def(d) => {
                let mut d = d.clone();
                d.name = namespace_join(ns, &d.name);
                out.push(Block::Def(d));
            }
            NamespaceDeclItem::Proc(p) => {
                let mut p = p.clone();
                p.name = namespace_join(ns, &p.name);
                out.push(Block::Proc(p));
            }
            NamespaceDeclItem::Namespace(nested) => {
                process_namespace_decl(nested.clone(), ns, state, out)?;
            }
            NamespaceDeclItem::Alias(alias) => {
                register_namespace_alias(ns, alias.clone(), state)?;
            }
        }
    }
    Ok(())
}

fn register_namespace_alias(
    parent_ns: &str,
    alias: NamespaceAliasLocalDecl,
    state: &mut LoadState,
) -> Result<(), Vec<Diagnostic>> {
    let full_name = namespace_join(parent_ns, &alias.name);
    if state.namespace_aliases.contains_key(&full_name) {
        return Err(vec![Diagnostic::semantic(
            format!("duplicate namespace alias '{full_name}'"),
            0,
            0,
        )]);
    }
    state.namespace_aliases.insert(
        full_name.clone(),
        NamespaceAliasDecl {
            declared_ns: parent_ns.to_owned(),
            target: alias.target,
        },
    );
    Ok(())
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

fn parse_namespace_ref_text(text: &str) -> Result<Vec<NamespaceRefSegment>, Vec<Diagnostic>> {
    let mut parsed = OmniParser::parse(Rule::namespace_ref, text)
        .map_err(|err| vec![diag_from_pest_error(err)])?;
    let pair = parsed
        .next()
        .ok_or_else(|| vec![Diagnostic::syntax("missing namespace reference", 0, 0)])?;
    let out = parse_namespace_ref_pair(pair)?;
    if out.len() < 2 {
        return Err(vec![Diagnostic::syntax(
            "namespace reference must contain at least one '::'",
            0,
            0,
        )]);
    }
    Ok(out)
}

fn expr_key(expr: &Expr) -> String {
    match expr {
        Expr::Number(v) => format!("f32({v})"),
        Expr::Int(v) => format!("i64({v})"),
        Expr::Bool(v) => format!("bool({v})"),
        Expr::Var(v) => format!("var({v})"),
        Expr::ArrayLiteral(values) => format!(
            "[{}]",
            values.iter().map(expr_key).collect::<Vec<_>>().join(",")
        ),
        Expr::Index { base, index } => format!("idx({base},{})", expr_key(index)),
        Expr::ArrayCtor { spec, init } => {
            let elem = match &spec.elem {
                ArrayElemType::Primitive(p) => format!("{p:?}"),
                ArrayElemType::Struct(s) => s.clone(),
            };
            let init_key = init
                .as_ref()
                .map(|v| v.iter().map(expr_key).collect::<Vec<_>>().join(","))
                .unwrap_or_default();
            format!("arr({elem};{};{init_key})", expr_key(&spec.size))
        }
        Expr::Compare { op, lhs, rhs } => {
            format!("cmp({op:?},{},{})", expr_key(lhs), expr_key(rhs))
        }
        Expr::Call { func, args } => format!(
            "call({func:?},[{}])",
            args.iter().map(expr_key).collect::<Vec<_>>().join(",")
        ),
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            let type_key = type_args
                .iter()
                .map(|a| match a {
                    CallTypeArg::Primitive(p) => format!("p:{p:?}"),
                    CallTypeArg::Generic(g) => format!("g:{g}"),
                })
                .collect::<Vec<_>>()
                .join(",");
            let arg_key = args
                .iter()
                .map(|a| {
                    let n = a.name.clone().unwrap_or_default();
                    format!("{n}={}", expr_key(&a.expr))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("ucall({name};[{type_key}];[{arg_key}])")
        }
        Expr::Cast { to, expr } => format!("cast({to:?},{})", expr_key(expr)),
        Expr::UnaryNot { expr } => format!("not({})", expr_key(expr)),
        Expr::Logical { op, lhs, rhs } => {
            format!("log({op:?},{},{})", expr_key(lhs), expr_key(rhs))
        }
        Expr::Binary { op, lhs, rhs } => {
            format!("bin({op:?},{},{})", expr_key(lhs), expr_key(rhs))
        }
    }
}

fn is_builtin_compile_time_const(name: &str) -> bool {
    matches!(
        name,
        "PI" | "pi"
            | "TWO_PI"
            | "TWOPI"
            | "two_pi"
            | "twopi"
            | "SAMPLE_RATE"
            | "SAMPLERATE"
            | "SR"
            | "sample_rate"
            | "samplerate"
            | "BLOCK_SIZE"
            | "BLOCKSIZE"
            | "BS"
            | "block_size"
            | "blocksize"
    )
}

fn validate_compile_time_expr(
    expr: &Expr,
    known_consts: &HashMap<String, Expr>,
    context: &str,
) -> Result<(), Vec<Diagnostic>> {
    match expr {
        Expr::Number(_) | Expr::Bool(_) => Ok(()),
        Expr::Int(v) => {
            if *v < i32::MIN as i64 || *v > i32::MAX as i64 {
                Err(vec![Diagnostic::semantic(
                    format!("{context}: value {v} is outside i32 bounds"),
                    0,
                    0,
                )])
            } else {
                Ok(())
            }
        }
        Expr::Var(name) => {
            if known_consts.contains_key(name) || is_builtin_compile_time_const(name) {
                Ok(())
            } else {
                Err(vec![Diagnostic::semantic(
                    format!(
                        "{context}: expression references non-compile-time symbol '{}'",
                        name
                    ),
                    0,
                    0,
                )])
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
            validate_compile_time_expr(expr, known_consts, context)
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            validate_compile_time_expr(lhs, known_consts, context)?;
            validate_compile_time_expr(rhs, known_consts, context)
        }
        Expr::Call { .. }
        | Expr::UserCall { .. }
        | Expr::Index { .. }
        | Expr::ArrayCtor { .. }
        | Expr::ArrayLiteral(_) => Err(vec![Diagnostic::semantic(
            format!("{context}: expression is not compile-time evaluable"),
            0,
            0,
        )]),
    }
}

fn resolve_visible_alias(
    alias_leaf: &str,
    current_ns: &str,
    state: &LoadState,
) -> Option<NamespaceAliasDecl> {
    for ns in namespace_candidates(current_ns) {
        let candidate = namespace_join(&ns, alias_leaf);
        if let Some(alias) = state.namespace_aliases.get(&candidate) {
            return Some(alias.clone());
        }
    }
    None
}

fn resolve_namespace_symbol_name(
    name: &str,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<String, Vec<Diagnostic>> {
    if !looks_like_namespace_ref(name) {
        return Ok(name.to_owned());
    }
    let segments = parse_namespace_ref_text(name)?;
    resolve_namespace_segments_internal(&segments, current_ns, const_env, state, generated, 0)
}

fn resolve_namespace_segments_internal(
    segments: &[NamespaceRefSegment],
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
    depth: usize,
) -> Result<String, Vec<Diagnostic>> {
    if depth > 64 {
        return Err(vec![Diagnostic::semantic(
            "namespace alias/template resolution exceeded recursion depth",
            0,
            0,
        )]);
    }
    if segments.is_empty() {
        return Err(vec![Diagnostic::semantic(
            "empty namespace reference",
            0,
            0,
        )]);
    }

    let mut idx = 0usize;
    let mut path = String::new();

    if segments[0].args.is_none() {
        if let Some(alias) = resolve_visible_alias(&segments[0].name, current_ns, state) {
            path = resolve_namespace_segments_internal(
                &alias.target,
                &alias.declared_ns,
                const_env,
                state,
                generated,
                depth + 1,
            )?;
            idx = 1;
        }
    }

    if idx == 0 {
        if let Some(args) = &segments[0].args {
            let mut resolved = None::<String>;
            for candidate_ns in namespace_candidates(current_ns) {
                let candidate = namespace_join(&candidate_ns, &segments[0].name);
                if state.namespace_templates.contains_key(&candidate) {
                    resolved = Some(instantiate_namespace_template(
                        &candidate, args, const_env, state, generated,
                    )?);
                    break;
                }
            }
            let Some(found) = resolved else {
                return Err(vec![Diagnostic::semantic(
                    format!("unknown namespace template '{}'", segments[0].name),
                    0,
                    0,
                )]);
            };
            path = found;
        } else {
            path = segments[0].name.clone();
        }
        idx = 1;
    }

    for seg in &segments[idx..] {
        let candidate = namespace_join(&path, &seg.name);
        if let Some(args) = &seg.args {
            path = instantiate_namespace_template(&candidate, args, const_env, state, generated)?;
        } else {
            path = candidate;
        }
    }
    Ok(path)
}

fn instantiate_namespace_template(
    full_template_name: &str,
    call_args: &[NamespaceCallArg],
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<String, Vec<Diagnostic>> {
    let template = state
        .namespace_templates
        .get(full_template_name)
        .cloned()
        .ok_or_else(|| {
            vec![Diagnostic::semantic(
                format!("unknown namespace template '{full_template_name}'"),
                0,
                0,
            )]
        })?;

    let mut effective_consts = template.captured_consts.clone();
    for (k, v) in const_env {
        effective_consts
            .entry(k.clone())
            .or_insert_with(|| v.clone());
    }

    let mut named = HashMap::<String, Expr>::new();
    let mut positional = Vec::<Expr>::new();
    for arg in call_args {
        let expr = substitute_expr_with_env(&arg.expr, &effective_consts);
        if let Some(name) = &arg.name {
            if named.insert(name.clone(), expr).is_some() {
                return Err(vec![Diagnostic::semantic(
                    format!(
                        "namespace template '{}' argument '{}' specified more than once",
                        full_template_name, name
                    ),
                    0,
                    0,
                )]);
            }
        } else {
            positional.push(expr);
        }
    }

    let mut pos_idx = 0usize;
    for param in &template.params {
        let value = if let Some(named_expr) = named.remove(&param.name) {
            named_expr
        } else if let Some(pos_expr) = positional.get(pos_idx) {
            pos_idx += 1;
            pos_expr.clone()
        } else {
            substitute_expr_with_env(&param.default, &effective_consts)
        };
        validate_compile_time_expr(
            &value,
            &effective_consts,
            &format!(
                "namespace template '{}' argument '{}'",
                full_template_name, param.name
            ),
        )?;
        let casted = Expr::Cast {
            to: PrimitiveType::I32,
            expr: Box::new(value),
        };
        effective_consts.insert(param.name.clone(), casted);
    }

    if pos_idx < positional.len() {
        return Err(vec![Diagnostic::semantic(
            format!(
                "namespace template '{}' received too many positional arguments",
                full_template_name
            ),
            0,
            0,
        )]);
    }
    if !named.is_empty() {
        let mut unknown = named.keys().cloned().collect::<Vec<_>>();
        unknown.sort();
        return Err(vec![Diagnostic::semantic(
            format!(
                "namespace template '{}' received unknown named arguments: {}",
                full_template_name,
                unknown.join(", ")
            ),
            0,
            0,
        )]);
    }

    let key = {
        let values = template
            .params
            .iter()
            .map(|p| {
                effective_consts
                    .get(&p.name)
                    .map(expr_key)
                    .unwrap_or_else(|| "missing".to_owned())
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{full_template_name}[{values}]")
    };
    if let Some(existing) = state.namespace_instantiations.get(&key) {
        return Ok(existing.clone());
    }

    let (parent, leaf) = split_namespace_parent_leaf(full_template_name);
    let concrete_leaf = format!("{leaf}__nsinst{}", state.next_namespace_instantiation_id);
    state.next_namespace_instantiation_id += 1;
    let concrete_ns = namespace_join(parent, &concrete_leaf);
    state
        .namespace_instantiations
        .insert(key, concrete_ns.clone());

    emit_instantiated_namespace_items(
        &template.items,
        &concrete_ns,
        &effective_consts,
        state,
        generated,
    )?;

    Ok(concrete_ns)
}

fn emit_instantiated_namespace_items(
    items: &[NamespaceDeclItem],
    namespace: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for item in items {
        match item {
            NamespaceDeclItem::Struct(s) => {
                let mut s = s.clone();
                s.name = namespace_join(namespace, &s.name);
                let mut block = Block::Struct(s);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                rewrite_block_namespace_refs(&mut block, &ns, const_env, state, generated)?;
                generated.push(block);
            }
            NamespaceDeclItem::Def(d) => {
                let mut d = d.clone();
                d.name = namespace_join(namespace, &d.name);
                let mut block = Block::Def(d);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                rewrite_block_namespace_refs(&mut block, &ns, const_env, state, generated)?;
                generated.push(block);
            }
            NamespaceDeclItem::Proc(p) => {
                let mut p = p.clone();
                p.name = namespace_join(namespace, &p.name);
                let mut block = Block::Proc(p);
                let ns = namespace_of_symbol(block_decl_name(&block).unwrap_or_default());
                rewrite_block_namespace_refs(&mut block, &ns, const_env, state, generated)?;
                generated.push(block);
            }
            NamespaceDeclItem::Namespace(nested) => {
                let mut nested = nested.clone();
                let full_nested = namespace_join(namespace, &nested.name);
                if nested.params.is_empty() {
                    emit_instantiated_namespace_items(
                        &nested.items,
                        &full_nested,
                        const_env,
                        state,
                        generated,
                    )?;
                } else {
                    let mut captured = nested.captured_consts.clone();
                    for (k, v) in const_env {
                        captured.insert(k.clone(), v.clone());
                    }
                    nested.captured_consts = captured;
                    state.namespace_templates.insert(full_nested, nested);
                }
            }
            NamespaceDeclItem::Alias(alias) => {
                let mut alias = alias.clone();
                substitute_namespace_ref_consts(&mut alias.target, const_env);
                register_namespace_alias(namespace, alias, state)?;
            }
        }
    }
    Ok(())
}

fn substitute_namespace_ref_consts(
    segments: &mut [NamespaceRefSegment],
    const_env: &HashMap<String, Expr>,
) {
    for seg in segments {
        if let Some(args) = &mut seg.args {
            for arg in args {
                arg.expr = substitute_expr_with_env(&arg.expr, const_env);
            }
        }
    }
}

fn substitute_expr_with_env(expr: &Expr, const_env: &HashMap<String, Expr>) -> Expr {
    match expr {
        Expr::Var(name) => const_env
            .get(name)
            .cloned()
            .unwrap_or_else(|| Expr::Var(name.clone())),
        Expr::ArrayLiteral(values) => Expr::ArrayLiteral(
            values
                .iter()
                .map(|v| substitute_expr_with_env(v, const_env))
                .collect(),
        ),
        Expr::Index { base, index } => Expr::Index {
            base: base.clone(),
            index: Box::new(substitute_expr_with_env(index, const_env)),
        },
        Expr::ArrayCtor { spec, init } => Expr::ArrayCtor {
            spec: ArrayTypeSpec {
                elem: spec.elem.clone(),
                size: Box::new(substitute_expr_with_env(&spec.size, const_env)),
            },
            init: init.as_ref().map(|values| {
                values
                    .iter()
                    .map(|v| substitute_expr_with_env(v, const_env))
                    .collect()
            }),
        },
        Expr::Compare { op, lhs, rhs } => Expr::Compare {
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Call { func, args } => Expr::Call {
            func: *func,
            args: args
                .iter()
                .map(|a| substitute_expr_with_env(a, const_env))
                .collect(),
        },
        Expr::UserCall {
            name,
            type_args,
            args,
        } => Expr::UserCall {
            name: name.clone(),
            type_args: type_args.clone(),
            args: args
                .iter()
                .map(|a| CallArg {
                    name: a.name.clone(),
                    expr: substitute_expr_with_env(&a.expr, const_env),
                })
                .collect(),
        },
        Expr::Cast { to, expr } => Expr::Cast {
            to: *to,
            expr: Box::new(substitute_expr_with_env(expr, const_env)),
        },
        Expr::UnaryNot { expr } => Expr::UnaryNot {
            expr: Box::new(substitute_expr_with_env(expr, const_env)),
        },
        Expr::Logical { op, lhs, rhs } => Expr::Logical {
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Binary { op, lhs, rhs } => Expr::Binary {
            op: *op,
            lhs: Box::new(substitute_expr_with_env(lhs, const_env)),
            rhs: Box::new(substitute_expr_with_env(rhs, const_env)),
        },
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => expr.clone(),
    }
}

fn rewrite_blocks_namespace_refs(
    blocks: &mut [Block],
    _current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for block in blocks {
        let current_ns = match block {
            Block::Struct(s) => namespace_of_symbol(&s.name),
            Block::Def(d) => namespace_of_symbol(&d.name),
            Block::Proc(p) => namespace_of_symbol(&p.name),
            _ => String::new(),
        };
        rewrite_block_namespace_refs(block, &current_ns, const_env, state, generated)?;
    }
    Ok(())
}

fn rewrite_block_namespace_refs(
    block: &mut Block,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match block {
        Block::Ins(decls) | Block::Outs(decls) => {
            for decl in decls {
                if let Some(ty) = &mut decl.ty {
                    rewrite_decl_type(ty, current_ns, const_env, state, generated)?;
                }
                if let Some(default) = &mut decl.default {
                    rewrite_expr(default, current_ns, const_env, state, generated)?;
                }
                if let Some(range) = &mut decl.range {
                    if let Some(min) = &mut range.min {
                        rewrite_expr(min, current_ns, const_env, state, generated)?;
                    }
                    rewrite_expr(&mut range.max, current_ns, const_env, state, generated)?;
                }
            }
        }
        Block::Params(decls) => {
            for decl in decls {
                if let Some(ty) = &mut decl.ty {
                    rewrite_decl_type(ty, current_ns, const_env, state, generated)?;
                }
                if let Some(default) = &mut decl.default {
                    rewrite_expr(default, current_ns, const_env, state, generated)?;
                }
                if let Some(range) = &mut decl.range {
                    if let Some(min) = &mut range.min {
                        rewrite_expr(min, current_ns, const_env, state, generated)?;
                    }
                    rewrite_expr(&mut range.max, current_ns, const_env, state, generated)?;
                }
            }
        }
        Block::Buffers(decls) => {
            for decl in decls {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(ty, current_ns, const_env, state, generated)?;
                }
            }
        }
        Block::Events(events) => {
            for event in events {
                rewrite_event_def(event, current_ns, const_env, state, generated)?;
            }
        }
        Block::Struct(s) => {
            for field in &mut s.fields {
                rewrite_field_type(&mut field.ty, current_ns, const_env, state, generated)?;
                if let Some(default) = &mut field.default {
                    rewrite_expr(default, current_ns, const_env, state, generated)?;
                }
            }
            for method in &mut s.methods {
                rewrite_function_def(method, current_ns, const_env, state, generated)?;
            }
        }
        Block::Def(d) => {
            rewrite_function_def(d, current_ns, const_env, state, generated)?;
        }
        Block::Proc(p) => {
            for decl in &mut p.ins {
                if let Some(ty) = &mut decl.ty {
                    rewrite_decl_type(ty, current_ns, const_env, state, generated)?;
                }
                if let Some(default) = &mut decl.default {
                    rewrite_expr(default, current_ns, const_env, state, generated)?;
                }
                if let Some(range) = &mut decl.range {
                    if let Some(min) = &mut range.min {
                        rewrite_expr(min, current_ns, const_env, state, generated)?;
                    }
                    rewrite_expr(&mut range.max, current_ns, const_env, state, generated)?;
                }
            }
            for decl in &mut p.outs {
                if let Some(ty) = &mut decl.ty {
                    rewrite_decl_type(ty, current_ns, const_env, state, generated)?;
                }
            }
            for decl in &mut p.params {
                if let Some(ty) = &mut decl.ty {
                    rewrite_decl_type(ty, current_ns, const_env, state, generated)?;
                }
                if let Some(default) = &mut decl.default {
                    rewrite_expr(default, current_ns, const_env, state, generated)?;
                }
                if let Some(range) = &mut decl.range {
                    if let Some(min) = &mut range.min {
                        rewrite_expr(min, current_ns, const_env, state, generated)?;
                    }
                    rewrite_expr(&mut range.max, current_ns, const_env, state, generated)?;
                }
            }
            for event in &mut p.events {
                rewrite_event_def(event, current_ns, const_env, state, generated)?;
            }
            for decl in &mut p.buffers {
                if let Some(ty) = &mut decl.ty {
                    rewrite_buffer_type(ty, current_ns, const_env, state, generated)?;
                }
            }
            if let Some(default_ty) = &mut p.init.default_ty {
                rewrite_decl_type(default_ty, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut p.init.body, current_ns, const_env, state, generated)?;
            rewrite_stmts(&mut p.block_pre, current_ns, const_env, state, generated)?;
            if let Some(os) = &mut p.sample_oversample_factor {
                rewrite_expr(os, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut p.sample, current_ns, const_env, state, generated)?;
            rewrite_stmts(&mut p.block_post, current_ns, const_env, state, generated)?;
        }
        Block::Init(init) => {
            if let Some(default_ty) = &mut init.default_ty {
                rewrite_decl_type(default_ty, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut init.body, current_ns, const_env, state, generated)?;
        }
        Block::Block(exec) => {
            rewrite_stmts(&mut exec.pre, current_ns, const_env, state, generated)?;
            if let Some(sample) = &mut exec.sample {
                if let Some(os) = &mut sample.oversample_factor {
                    rewrite_expr(os, current_ns, const_env, state, generated)?;
                }
                rewrite_stmts(&mut sample.body, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut exec.post, current_ns, const_env, state, generated)?;
        }
        Block::Sample(sample) => {
            if let Some(os) = &mut sample.oversample_factor {
                rewrite_expr(os, current_ns, const_env, state, generated)?;
            }
            rewrite_stmts(&mut sample.body, current_ns, const_env, state, generated)?;
        }
    }
    Ok(())
}

fn rewrite_function_def(
    def: &mut FunctionDef,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for param in &mut def.params {
        if let Some(ty) = &mut param.ty {
            rewrite_fn_param_type(ty, current_ns, const_env, state, generated)?;
        }
        if let Some(default) = &mut param.default {
            rewrite_expr(default, current_ns, const_env, state, generated)?;
        }
    }
    rewrite_stmts(&mut def.body, current_ns, const_env, state, generated)
}

fn rewrite_event_def(
    event: &mut EventDef,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for param in &mut event.params {
        if let EventParamType::Array { size, .. } = &mut param.ty {
            rewrite_expr(size, current_ns, const_env, state, generated)?;
        }
    }
    rewrite_stmts(&mut event.body, current_ns, const_env, state, generated)
}

fn rewrite_decl_type(
    ty: &mut DeclType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match ty {
        DeclType::Generic(name) => {
            if looks_like_namespace_ref(name) {
                *name =
                    resolve_namespace_symbol_name(name, current_ns, const_env, state, generated)?;
            }
        }
        DeclType::ArrayGeneric { elem, size } => {
            if looks_like_namespace_ref(elem) {
                *elem =
                    resolve_namespace_symbol_name(elem, current_ns, const_env, state, generated)?;
            }
            rewrite_expr(size, current_ns, const_env, state, generated)?;
        }
        DeclType::Array { size, .. } => {
            rewrite_expr(size, current_ns, const_env, state, generated)?;
        }
        DeclType::Scalar(_) => {}
    }
    Ok(())
}

fn rewrite_field_type(
    ty: &mut FieldType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match ty {
        FieldType::Generic(name) => {
            if looks_like_namespace_ref(name) {
                *name =
                    resolve_namespace_symbol_name(name, current_ns, const_env, state, generated)?;
            }
        }
        FieldType::Array(spec) => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                if looks_like_namespace_ref(name) {
                    *name = resolve_namespace_symbol_name(
                        name, current_ns, const_env, state, generated,
                    )?;
                }
            }
            rewrite_expr(&mut spec.size, current_ns, const_env, state, generated)?;
        }
        FieldType::Scalar(_) => {}
    }
    Ok(())
}

fn rewrite_fn_param_type(
    ty: &mut FnParamType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match ty {
        FnParamType::Struct(name) => {
            if looks_like_namespace_ref(name) {
                *name =
                    resolve_namespace_symbol_name(name, current_ns, const_env, state, generated)?;
            }
        }
        FnParamType::Buffer(buffer_ty) => {
            rewrite_buffer_type(buffer_ty, current_ns, const_env, state, generated)?;
        }
        FnParamType::Primitive(_) => {}
    }
    Ok(())
}

fn rewrite_buffer_type(
    ty: &mut BufferType,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    if let BufferElemType::Generic(name) = &mut ty.elem {
        if looks_like_namespace_ref(name) {
            *name = resolve_namespace_symbol_name(name, current_ns, const_env, state, generated)?;
        }
    }
    if let BufferChannels::Static(expr) = &mut ty.channels {
        rewrite_expr(expr, current_ns, const_env, state, generated)?;
    }
    Ok(())
}

fn rewrite_stmts(
    stmts: &mut [Stmt],
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    for stmt in stmts {
        rewrite_stmt(stmt, current_ns, const_env, state, generated)?;
    }
    Ok(())
}

fn rewrite_stmt(
    stmt: &mut Stmt,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    match stmt {
        Stmt::Assign {
            target,
            generic_decl_ty,
            expr,
            ..
        } => {
            match target {
                AssignTarget::Var(name) => {
                    if looks_like_namespace_ref(name) {
                        *name = resolve_namespace_symbol_name(
                            name, current_ns, const_env, state, generated,
                        )?;
                    }
                }
                AssignTarget::Index { base, index } => {
                    if looks_like_namespace_ref(base) {
                        *base = resolve_namespace_symbol_name(
                            base, current_ns, const_env, state, generated,
                        )?;
                    }
                    rewrite_expr(index, current_ns, const_env, state, generated)?;
                }
            }
            if let Some(name) = generic_decl_ty {
                if looks_like_namespace_ref(name) {
                    *name = resolve_namespace_symbol_name(
                        name, current_ns, const_env, state, generated,
                    )?;
                }
            }
            rewrite_expr(expr, current_ns, const_env, state, generated)?;
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_expr(expr, current_ns, const_env, state, generated)?;
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr(cond, current_ns, const_env, state, generated)?;
            rewrite_stmts(then_branch, current_ns, const_env, state, generated)?;
            rewrite_stmts(else_branch, current_ns, const_env, state, generated)?;
        }
        Stmt::For {
            step,
            start,
            end,
            body,
            ..
        } => {
            if let Some(step) = step {
                rewrite_expr(step, current_ns, const_env, state, generated)?;
            }
            rewrite_expr(start, current_ns, const_env, state, generated)?;
            rewrite_expr(end, current_ns, const_env, state, generated)?;
            rewrite_stmts(body, current_ns, const_env, state, generated)?;
        }
        Stmt::While { cond, body, .. } => {
            rewrite_expr(cond, current_ns, const_env, state, generated)?;
            rewrite_stmts(body, current_ns, const_env, state, generated)?;
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
    Ok(())
}

fn rewrite_expr(
    expr: &mut Expr,
    current_ns: &str,
    const_env: &HashMap<String, Expr>,
    state: &mut LoadState,
    generated: &mut Vec<Block>,
) -> Result<(), Vec<Diagnostic>> {
    if let Expr::Var(name) = expr {
        if let Some(value) = const_env.get(name).cloned() {
            *expr = value;
            return rewrite_expr(expr, current_ns, const_env, state, generated);
        }
    }

    match expr {
        Expr::Var(name) => {
            if looks_like_namespace_ref(name) {
                *name =
                    resolve_namespace_symbol_name(name, current_ns, const_env, state, generated)?;
            }
        }
        Expr::Index { base, index } => {
            if looks_like_namespace_ref(base) {
                *base =
                    resolve_namespace_symbol_name(base, current_ns, const_env, state, generated)?;
            }
            rewrite_expr(index, current_ns, const_env, state, generated)?;
        }
        Expr::ArrayCtor { spec, init } => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                if looks_like_namespace_ref(name) {
                    *name = resolve_namespace_symbol_name(
                        name, current_ns, const_env, state, generated,
                    )?;
                }
            }
            rewrite_expr(&mut spec.size, current_ns, const_env, state, generated)?;
            if let Some(values) = init {
                for value in values {
                    rewrite_expr(value, current_ns, const_env, state, generated)?;
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_expr(lhs, current_ns, const_env, state, generated)?;
            rewrite_expr(rhs, current_ns, const_env, state, generated)?;
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_expr(arg, current_ns, const_env, state, generated)?;
            }
        }
        Expr::UserCall { name, args, .. } => {
            if looks_like_namespace_ref(name) {
                *name =
                    resolve_namespace_symbol_name(name, current_ns, const_env, state, generated)?;
            }
            for arg in args {
                rewrite_expr(&mut arg.expr, current_ns, const_env, state, generated)?;
            }
        }
        Expr::Cast { expr: arg, .. } | Expr::UnaryNot { expr: arg } => {
            rewrite_expr(arg, current_ns, const_env, state, generated)?;
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_expr(value, current_ns, const_env, state, generated)?;
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
    Ok(())
}
