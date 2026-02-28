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
    let mut declared = existing
        .iter()
        .filter_map(block_decl_name)
        .map(ToOwned::to_owned)
        .collect::<HashSet<_>>();
    for block in incoming {
        if let Some(name) = block_decl_name(&block) {
            if declared.contains(name) {
                continue;
            }
            declared.insert(name.to_owned());
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

pub fn inject_auto_std_math(program: &mut Program) -> Result<(), Vec<Diagnostic>> {
    let imported = load_builtin_module_blocks(
        STDLIB_AUTO_IMPORT_MODULE,
        true,
        &mut LoadState::default(),
        &[],
    )?;
    merge_blocks_preferring_existing(&mut program.blocks, imported);
    Ok(())
}

fn parse_program_preprocessed(
    preprocessed: &str,
    file_path: &Path,
    line_offset: usize,
    trace: &[String],
    source_line_map: &[usize],
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
                Rule::namespace_block => blocks.extend(parse_namespace_block(pair, "")?),
                Rule::init_block => blocks.push(Block::Init(parse_exec_block(pair)?)),
                Rule::block_exec_block => blocks.push(Block::Block(parse_block_exec_block(pair)?)),
                Rule::sample_block => blocks.push(Block::Sample(parse_sample_block(pair)?)),
                _ => {}
            }
        }

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

fn parse_namespace_block(
    block_pair: Pair<'_, Rule>,
    parent_ns: &str,
) -> Result<Vec<Block>, Vec<Diagnostic>> {
    let mut inner = block_pair.into_inner();
    let Some(ns_name_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing namespace name", 0, 0)]);
    };
    if ns_name_pair.as_rule() != Rule::namespace_name {
        return Err(vec![Diagnostic::syntax("missing namespace name", 0, 0)]);
    }
    let full_ns = namespace_join(parent_ns, ns_name_pair.as_str());
    let mut out = Vec::new();

    for item in inner {
        match item.as_rule() {
            Rule::struct_block => {
                let mut s = parse_struct_block(item)?;
                s.name = namespace_join(&full_ns, &s.name);
                out.push(Block::Struct(s));
            }
            Rule::def_block => {
                let mut d = parse_def_block(item)?;
                d.name = namespace_join(&full_ns, &d.name);
                out.push(Block::Def(d));
            }
            Rule::proc_block => {
                let mut p = parse_proc_block(item)?;
                p.name = namespace_join(&full_ns, &p.name);
                out.push(Block::Proc(p));
            }
            Rule::namespace_block => {
                out.extend(parse_namespace_block(item, &full_ns)?);
            }
            _ => {}
        }
    }

    Ok(out)
}
