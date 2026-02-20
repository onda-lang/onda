use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use pest::error::LineColLocation;
use pest::iterators::Pair;
use pest::pratt_parser::{Assoc, Op, PrattParser};
use pest::Parser;
use pest_derive::Parser;

use crate::ast::{
    AssignTarget, BinaryOp, Block, BlockExec, BufferChannels, BufferDecl, BufferElemType,
    BufferType, BuiltinFn, CallArg, CallTypeArg, CmpOp, DataElemType, DataTypeSpec, DeclRange,
    DeclType, Expr, FieldType, FnParamDecl, FnParamType, FunctionDef, LogicalOp, ParamDecl,
    PortDecl, PrimitiveType, ProcessorDef, Program, SourceLoc, Stmt, StructDef, StructField,
};
use crate::diagnostics::Diagnostic;

const PROC_INDEX_SENTINEL_PREFIX: &str = "__omni_proc_index__";
const PROC_INDEX_SENTINEL_ARG: &str = "__proc_index";
const BUFFER_READ2_INTERNAL_FN: &str = "__omni_buffer_read2";
const BUFFER_WRITE2_INTERNAL_FN: &str = "__omni_buffer_write2";
const STDLIB_AUTO_IMPORT_MODULE: &str = "std/math";
const STDLIB_MODULE_PREFIX: &str = "std/";

#[derive(Parser)]
#[grammar = "grammar.pest"]
struct OmniParser;

#[derive(Clone, Copy)]
struct PendingIndentBlock {
    line: usize,
    indent: usize,
}

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
        "std/math" => Some(include_str!("../../../stdlib/std/math.omni")),
        "std/osc" => Some(include_str!("../../../stdlib/std/osc.omni")),
        "std/filter" => Some(include_str!("../../../stdlib/std/filter.omni")),
        "std/env" => Some(include_str!("../../../stdlib/std/env.omni")),
        "std/delay" => Some(include_str!("../../../stdlib/std/delay.omni")),
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

fn find_legacy_data_syntax(source: &str) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i] == b'D' && bytes[i + 1] == b'a' && bytes[i + 2] == b't' && bytes[i + 3] == b'a'
        {
            let prev_is_ident =
                i > 0 && ((bytes[i - 1] as char).is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if prev_is_ident {
                i += 1;
                continue;
            }
            let mut j = i + 4;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'[' {
                let line = source[..i].bytes().filter(|b| *b == b'\n').count() + 1;
                let line_start = source[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
                let col = source[line_start..i].chars().count() + 1;
                return Some((line, col));
            }
        }
        i += 1;
    }
    None
}

fn reject_legacy_data_syntax(source: &str) -> Result<(), Vec<Diagnostic>> {
    if let Some((line, col)) = find_legacy_data_syntax(source) {
        return Err(vec![Diagnostic::syntax(
            "legacy Data[...] syntax has been removed; use T[N] array types/declarations",
            line,
            col,
        )]);
    }
    Ok(())
}

pub fn parse_program(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let virtual_path = PathBuf::from("<memory>");
    reject_legacy_data_syntax(source)
        .map_err(|diags| annotate_diagnostics_with_file(diags, &virtual_path, 0))?;
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
                Rule::buffers_block => blocks.push(Block::Buffers(parse_buffers_block(pair)?)),
                Rule::proc_block => blocks.push(Block::Proc(parse_proc_block(pair)?)),
                Rule::struct_block => blocks.push(Block::Struct(parse_struct_block(pair)?)),
                Rule::def_block => blocks.push(Block::Def(parse_def_block(pair)?)),
                Rule::namespace_block => blocks.extend(parse_namespace_block(pair, "")?),
                Rule::init_block => blocks.push(Block::Init(parse_exec_block(pair)?)),
                Rule::block_exec_block => blocks.push(Block::Block(parse_block_exec_block(pair)?)),
                Rule::sample_block => blocks.push(Block::Sample(parse_exec_block(pair)?)),
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
        reject_legacy_data_syntax(&source)
            .map_err(|diags| annotate_diagnostics_with_file(diags, &canonical, 0))?;
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
        reject_legacy_data_syntax(source)
            .map_err(|diags| annotate_diagnostics_with_file(diags, &virtual_path, 0))?;
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

fn stmt_loc_from_pair(pair: &Pair<'_, Rule>) -> Option<SourceLoc> {
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

fn parse_section_default_decl_type(
    pair: Pair<'_, Rule>,
    block_name: &str,
) -> Result<DeclType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::section_default_elem_type {
        return Err(vec![Diagnostic::syntax(
            format!("internal parser error: expected {block_name} section default type"),
            0,
            0,
        )]);
    }
    let mut inner = pair.into_inner();
    let Some(actual) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            format!("missing {block_name} section default type"),
            0,
            0,
        )]);
    };
    parse_decl_type(actual)
}

fn parse_section_default_buffer_type(
    pair: Pair<'_, Rule>,
    block_name: &str,
) -> Result<BufferType, Vec<Diagnostic>> {
    let decl_ty = parse_section_default_decl_type(pair, block_name)?;
    let elem = match decl_ty {
        DeclType::Scalar(prim) => BufferElemType::Primitive(prim),
        DeclType::Generic(param) => BufferElemType::Generic(param),
        DeclType::Array { .. } | DeclType::ArrayGeneric { .. } => {
            return Err(vec![Diagnostic::syntax(
                format!(
                    "{block_name} section default type must be primitive or generic element type"
                ),
                0,
                0,
            )])
        }
    };
    Ok(BufferType {
        elem,
        channels: BufferChannels::Mono,
    })
}

fn parse_decl_range_pair(pair: Pair<'_, Rule>) -> Result<DeclRange, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::decl_range {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected declaration range",
            0,
            0,
        )]);
    }
    let mut inner = pair.into_inner();
    let Some(first_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            "missing declaration range expression",
            0,
            0,
        )]);
    };
    let first = parse_expr_inner(first_pair);
    let second = inner.next().map(parse_expr_inner);
    if inner.next().is_some() {
        return Err(vec![Diagnostic::syntax(
            "declaration range accepts at most two expressions",
            0,
            0,
        )]);
    }
    let (min, max) = match second {
        Some(max) => (Some(first), max),
        None => (None, first),
    };
    Ok(DeclRange { min, max })
}

fn parse_port_block(block_pair: Pair<'_, Rule>) -> Result<Vec<PortDecl>, Vec<Diagnostic>> {
    let (block_name, prefix) = match block_pair.as_rule() {
        Rule::ins_block => ("ins", "in"),
        Rule::outs_block => ("outs", "out"),
        _ => ("ports", "port"),
    };
    let allow_default_and_range = block_pair.as_rule() == Rule::ins_block;

    let mut ports = Vec::new();
    let mut has_list = false;
    let mut count_prefix: Option<usize> = None;
    let mut default_ty: Option<DeclType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_elem_type => {
                default_ty = Some(parse_section_default_decl_type(child, block_name)?);
            }
            Rule::int_lit => {
                if count_prefix.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        format!("{block_name} block count can only be specified once"),
                        0,
                        0,
                    )]);
                }
                let count_i = parse_int(child.as_str())?;
                if count_i <= 0 {
                    return Err(vec![Diagnostic::syntax(
                        format!("{block_name} count shorthand must be greater than zero"),
                        0,
                        0,
                    )]);
                }
                count_prefix = Some(count_i as usize);
            }
            Rule::port_list => {
                has_list = true;
                for item in child.into_inner() {
                    if item.as_rule() != Rule::port_decl {
                        continue;
                    }
                    let mut inner = item.into_inner();
                    let Some(name_pair) = inner.next() else {
                        return Err(vec![Diagnostic::syntax("missing port identifier", 0, 0)]);
                    };
                    let mut ty = None;
                    let mut default = None;
                    let mut range = None;
                    for inner_item in inner {
                        match inner_item.as_rule() {
                            Rule::decl_type
                            | Rule::type_name
                            | Rule::array_type
                            | Rule::qualified_ident => {
                                let actual = if inner_item.as_rule() == Rule::decl_type {
                                    let mut decl_inner = inner_item.into_inner();
                                    decl_inner.next().ok_or_else(|| {
                                        vec![Diagnostic::syntax(
                                            "missing port declaration type",
                                            0,
                                            0,
                                        )]
                                    })?
                                } else {
                                    inner_item
                                };
                                ty = Some(parse_decl_type(actual)?);
                            }
                            Rule::expr => default = Some(parse_expr_inner(inner_item)),
                            Rule::decl_range => range = Some(parse_decl_range_pair(inner_item)?),
                            _ => {}
                        }
                    }
                    if !allow_default_and_range {
                        if default.is_some() {
                            return Err(vec![Diagnostic::syntax(
                                format!("{block_name} declarations do not support default values"),
                                0,
                                0,
                            )]);
                        }
                        if range.is_some() {
                            return Err(vec![Diagnostic::syntax(
                                format!("{block_name} declarations do not support ranges"),
                                0,
                                0,
                            )]);
                        }
                    }
                    let ty = ty.or_else(|| default_ty.clone());
                    ports.push(PortDecl {
                        name: name_pair.as_str().to_owned(),
                        ty,
                        default,
                        range,
                    });
                }
            }
            _ => {}
        }
    }

    if has_list {
        if let Some(n) = count_prefix {
            if n != ports.len() {
                return Err(vec![Diagnostic::syntax(
                    format!(
                        "{block_name} block count prefix ({n}) does not match explicit declaration count ({})",
                        ports.len()
                    ),
                    0,
                    0,
                )]);
            }
        }
    } else if let Some(n) = count_prefix {
        for idx in 1..=n {
            ports.push(PortDecl {
                name: format!("{prefix}{idx}"),
                ty: default_ty.clone(),
                default: None,
                range: None,
            });
        }
    }

    Ok(ports)
}

fn parse_params_block(block_pair: Pair<'_, Rule>) -> Result<Vec<ParamDecl>, Vec<Diagnostic>> {
    let mut params = Vec::new();
    let mut has_list = false;
    let mut count_prefix: Option<usize> = None;
    let mut default_ty: Option<DeclType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_elem_type => {
                default_ty = Some(parse_section_default_decl_type(child, "params")?);
            }
            Rule::int_lit => {
                if count_prefix.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "params block count can only be specified once",
                        0,
                        0,
                    )]);
                }
                let count_i = parse_int(child.as_str())?;
                if count_i <= 0 {
                    return Err(vec![Diagnostic::syntax(
                        "params count shorthand must be greater than zero",
                        0,
                        0,
                    )]);
                }
                count_prefix = Some(count_i as usize);
            }
            Rule::param_list => {
                has_list = true;
                for param_pair in child.into_inner() {
                    if param_pair.as_rule() != Rule::param_decl {
                        continue;
                    }
                    let mut inner = param_pair.into_inner();
                    let Some(name_pair) = inner.next() else {
                        return Err(vec![Diagnostic::syntax("missing param identifier", 0, 0)]);
                    };
                    let mut ty = None;
                    let mut default = None;
                    let mut range = None;
                    for item in inner {
                        match item.as_rule() {
                            Rule::decl_type
                            | Rule::type_name
                            | Rule::array_type
                            | Rule::qualified_ident => {
                                let actual = if item.as_rule() == Rule::decl_type {
                                    let mut decl_inner = item.into_inner();
                                    decl_inner.next().ok_or_else(|| {
                                        vec![Diagnostic::syntax(
                                            "missing param declaration type",
                                            0,
                                            0,
                                        )]
                                    })?
                                } else {
                                    item
                                };
                                ty = Some(parse_decl_type(actual)?);
                            }
                            Rule::expr => default = Some(parse_expr_inner(item)),
                            Rule::decl_range => range = Some(parse_decl_range_pair(item)?),
                            _ => {}
                        }
                    }
                    let ty = ty.or_else(|| default_ty.clone());
                    params.push(ParamDecl {
                        name: name_pair.as_str().to_owned(),
                        ty,
                        default,
                        range,
                    });
                }
            }
            _ => {}
        }
    }

    if has_list {
        if let Some(n) = count_prefix {
            if n != params.len() {
                return Err(vec![Diagnostic::syntax(
                    format!(
                        "params block count prefix ({n}) does not match explicit declaration count ({})",
                        params.len()
                    ),
                    0,
                    0,
                )]);
            }
        }
    } else if let Some(n) = count_prefix {
        for idx in 1..=n {
            params.push(ParamDecl {
                name: format!("param{idx}"),
                ty: default_ty.clone(),
                default: None,
                range: None,
            });
        }
    }

    Ok(params)
}

fn parse_buffers_block(block_pair: Pair<'_, Rule>) -> Result<Vec<BufferDecl>, Vec<Diagnostic>> {
    let mut out = Vec::<BufferDecl>::new();
    let mut seen = HashSet::<String>::new();

    let mut has_list = false;
    let mut count_shorthand: Option<usize> = None;
    let mut default_ty: Option<BufferType> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::section_default_elem_type => {
                default_ty = Some(parse_section_default_buffer_type(child, "buffers")?);
            }
            Rule::buffer_list => {
                has_list = true;
                for item in child.into_inner() {
                    if item.as_rule() != Rule::buffer_decl {
                        continue;
                    }
                    let mut inner = item.into_inner();
                    let Some(name_pair) = inner.next() else {
                        return Err(vec![Diagnostic::syntax("missing buffer identifier", 0, 0)]);
                    };
                    let name = name_pair.as_str().to_owned();
                    let ty = match inner.next() {
                        Some(ty_pair) => Some(parse_buffer_decl_type(ty_pair)?),
                        None => default_ty.clone(),
                    };
                    if !seen.insert(name.clone()) {
                        return Err(vec![Diagnostic::syntax(
                            format!("duplicate buffer declaration '{name}'"),
                            0,
                            0,
                        )]);
                    }
                    out.push(BufferDecl { name, ty });
                }
            }
            Rule::int_lit => {
                if has_list {
                    return Err(vec![Diagnostic::syntax(
                        "buffers block cannot mix explicit declarations and count shorthand",
                        0,
                        0,
                    )]);
                }
                let count_i = parse_int(child.as_str())?;
                if count_i <= 0 {
                    return Err(vec![Diagnostic::syntax(
                        "buffers count shorthand must be greater than zero",
                        0,
                        0,
                    )]);
                }
                count_shorthand = Some(count_i as usize);
            }
            _ => {}
        }
    }

    if let Some(n) = count_shorthand {
        for idx in 1..=n {
            out.push(BufferDecl {
                name: format!("buf{idx}"),
                ty: default_ty.clone(),
            });
        }
    }

    Ok(out)
}

fn parse_proc_block(block_pair: Pair<'_, Rule>) -> Result<ProcessorDef, Vec<Diagnostic>> {
    let mut name: Option<String> = None;
    let mut type_params = Vec::new();
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    let mut params = Vec::new();
    let mut buffers = Vec::new();
    let mut init: Option<Vec<Stmt>> = None;
    let mut block_exec: Option<BlockExec> = None;
    let mut sample: Option<Vec<Stmt>> = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::generic_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::ident {
                        type_params.push(item.as_str().to_owned());
                    }
                }
            }
            Rule::ins_block => {
                if !ins.is_empty() {
                    return Err(vec![Diagnostic::syntax("duplicate proc ins block", 0, 0)]);
                }
                ins = parse_port_block(child)?;
            }
            Rule::outs_block => {
                if !outs.is_empty() {
                    return Err(vec![Diagnostic::syntax("duplicate proc outs block", 0, 0)]);
                }
                outs = parse_port_block(child)?;
            }
            Rule::params_block => {
                if !params.is_empty() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc params block",
                        0,
                        0,
                    )]);
                }
                params = parse_params_block(child)?;
            }
            Rule::buffers_block => {
                if !buffers.is_empty() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc buffers block",
                        0,
                        0,
                    )]);
                }
                buffers = parse_buffers_block(child)?;
            }
            Rule::init_block => {
                if init.is_some() {
                    return Err(vec![Diagnostic::syntax("duplicate proc init block", 0, 0)]);
                }
                init = Some(parse_exec_block(child)?);
            }
            Rule::block_exec_block => {
                if block_exec.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc block section",
                        0,
                        0,
                    )]);
                }
                block_exec = Some(parse_block_exec_block(child)?);
            }
            Rule::sample_block => {
                if sample.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate proc sample block",
                        0,
                        0,
                    )]);
                }
                sample = Some(parse_exec_block(child)?);
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![Diagnostic::syntax("missing proc name", 0, 0)]);
    };

    let mut block_pre = Vec::new();
    let mut block_post = Vec::new();
    let mut has_block_block = false;
    if let Some(exec) = block_exec {
        has_block_block = true;
        if sample.is_some() {
            return Err(vec![Diagnostic::syntax(
                "proc sample block cannot be declared both directly and inside block section",
                0,
                0,
            )]);
        }
        let Some(nested_sample) = exec.sample else {
            return Err(vec![Diagnostic::syntax(
                "proc block section must include nested 'sample' block",
                0,
                0,
            )]);
        };
        block_pre = exec.pre;
        block_post = exec.post;
        sample = Some(nested_sample);
    }

    Ok(ProcessorDef {
        name,
        type_params,
        ins,
        outs,
        params,
        buffers,
        has_init_block: init.is_some(),
        has_block_block,
        has_sample_block: sample.is_some(),
        init: init.unwrap_or_default(),
        block_pre,
        sample: sample.unwrap_or_default(),
        block_post,
    })
}

fn parse_struct_block(block_pair: Pair<'_, Rule>) -> Result<StructDef, Vec<Diagnostic>> {
    let mut name: Option<String> = None;
    let mut type_params = Vec::new();
    let mut fields = Vec::new();
    let mut methods = Vec::new();

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::generic_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::ident {
                        type_params.push(item.as_str().to_owned());
                    }
                }
            }
            Rule::field_list => {
                for item in child.into_inner() {
                    if item.as_rule() != Rule::field_decl {
                        continue;
                    }
                    let mut decl_inner = item.into_inner();
                    let Some(field_name) = decl_inner.next() else {
                        return Err(vec![Diagnostic::syntax("missing struct field name", 0, 0)]);
                    };
                    let mut parsed_ty = None::<FieldType>;
                    let mut default = None;
                    for part in decl_inner {
                        match part.as_rule() {
                            Rule::field_type => {
                                parsed_ty = Some(parse_field_type(part)?);
                            }
                            Rule::expr => {
                                default = Some(parse_expr_inner(part));
                            }
                            _ => {}
                        }
                    }
                    let ty = if let Some(explicit) = parsed_ty {
                        explicit
                    } else if let Some(ref default_expr) = default {
                        FieldType::Scalar(infer_struct_field_scalar_type_from_default(default_expr))
                    } else {
                        FieldType::Scalar(PrimitiveType::F32)
                    };
                    fields.push(StructField {
                        name: field_name.as_str().to_owned(),
                        ty,
                        default,
                    });
                }
            }
            Rule::struct_method_list => {
                for item in child.into_inner() {
                    if item.as_rule() != Rule::struct_method_decl {
                        continue;
                    }
                    methods.push(parse_struct_method_decl(item)?);
                }
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![Diagnostic::syntax("missing struct name", 0, 0)]);
    };
    Ok(StructDef {
        name,
        type_params,
        fields,
        methods,
    })
}

fn infer_struct_field_scalar_type_from_default(expr: &Expr) -> PrimitiveType {
    infer_expr_primitive_type(expr).unwrap_or(PrimitiveType::F32)
}

fn infer_expr_primitive_type(expr: &Expr) -> Option<PrimitiveType> {
    match expr {
        Expr::Int(_) => Some(PrimitiveType::I32),
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Compare { .. } | Expr::Logical { .. } | Expr::UnaryNot { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::Binary { lhs, rhs, .. } => {
            let left = infer_expr_primitive_type(lhs)?;
            let right = infer_expr_primitive_type(rhs)?;
            merge_inferred_numeric_type(left, right)
        }
        _ => None,
    }
}

fn merge_inferred_numeric_type(a: PrimitiveType, b: PrimitiveType) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (a, b) {
        (Bool, _) | (_, Bool) => None,
        (F64, _) | (_, F64) => Some(F64),
        (F32, _) | (_, F32) => Some(F32),
        (I64, _) | (_, I64) => Some(I64),
        (I32, I32) => Some(I32),
    }
}

fn parse_def_block(block_pair: Pair<'_, Rule>) -> Result<FunctionDef, Vec<Diagnostic>> {
    let mut name: Option<String> = None;
    let mut params = Vec::new();
    let mut body = None;

    for child in block_pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::fn_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::fn_param_decl {
                        params.push(parse_fn_param_decl(item, "function")?);
                    }
                }
            }
            Rule::stmt_block => {
                body = Some(parse_stmt_block(child)?);
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![Diagnostic::syntax("missing function name", 0, 0)]);
    };
    let Some(body) = body else {
        return Err(vec![Diagnostic::syntax("missing function body", 0, 0)]);
    };

    Ok(FunctionDef {
        name,
        type_params: Vec::new(),
        params,
        body,
    })
}

fn parse_fn_param_decl(
    pair: Pair<'_, Rule>,
    context: &str,
) -> Result<FnParamDecl, Vec<Diagnostic>> {
    let mut inner = pair.into_inner();
    let Some(name_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            format!("missing {context} parameter name"),
            0,
            0,
        )]);
    };

    let mut ty = None;
    let mut default = None;
    for item in inner {
        match item.as_rule() {
            Rule::fn_param_type => {
                ty = Some(parse_fn_param_type(item)?);
            }
            Rule::expr => {
                default = Some(parse_expr_inner(item));
            }
            _ => {}
        }
    }

    Ok(FnParamDecl {
        name: name_pair.as_str().to_owned(),
        ty,
        default,
    })
}
fn parse_stmt_list_pair(stmt_list_pair: Pair<'_, Rule>) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    let mut stmts = Vec::new();
    for stmt_pair in stmt_list_pair.into_inner() {
        stmts.push(parse_stmt(stmt_pair)?);
    }
    Ok(stmts)
}

fn parse_exec_block(block_pair: Pair<'_, Rule>) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    for child in block_pair.into_inner() {
        if child.as_rule() == Rule::stmt_list {
            return parse_stmt_list_pair(child);
        }
    }
    Ok(Vec::new())
}

fn parse_block_exec_block(block_pair: Pair<'_, Rule>) -> Result<BlockExec, Vec<Diagnostic>> {
    let mut pre = Vec::new();
    let mut post = Vec::new();
    let mut nested_sample: Option<Vec<Stmt>> = None;

    for child in block_pair.into_inner() {
        if child.as_rule() != Rule::block_exec_list {
            continue;
        }

        for item in child.into_inner() {
            if item.as_rule() == Rule::sample_nested_block {
                if nested_sample.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate nested sample block in block section",
                        0,
                        0,
                    )]);
                }
                let mut parsed_sample = Vec::new();
                for sample_child in item.into_inner() {
                    if sample_child.as_rule() == Rule::stmt_block {
                        parsed_sample = parse_stmt_block(sample_child)?;
                    }
                }
                nested_sample = Some(parsed_sample);
                continue;
            }

            let stmt = parse_stmt(item)?;
            if nested_sample.is_some() {
                post.push(stmt);
            } else {
                pre.push(stmt);
            }
        }
    }

    Ok(BlockExec {
        pre,
        sample: nested_sample,
        post,
    })
}

fn parse_struct_method_decl(pair: Pair<'_, Rule>) -> Result<FunctionDef, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::struct_method_decl {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected struct method declaration",
            0,
            0,
        )]);
    }
    let mut name: Option<String> = None;
    let mut params = Vec::new();
    let mut body = None;
    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::ident => {
                if name.is_none() {
                    name = Some(child.as_str().to_owned());
                }
            }
            Rule::fn_param_list => {
                for item in child.into_inner() {
                    if item.as_rule() == Rule::fn_param_decl {
                        params.push(parse_fn_param_decl(item, "method")?);
                    }
                }
            }
            Rule::stmt_block => {
                body = Some(parse_stmt_block(child)?);
            }
            _ => {}
        }
    }

    let Some(name) = name else {
        return Err(vec![Diagnostic::syntax("missing method name", 0, 0)]);
    };
    let Some(body) = body else {
        return Err(vec![Diagnostic::syntax("missing method body", 0, 0)]);
    };
    Ok(FunctionDef {
        name,
        type_params: Vec::new(),
        params,
        body,
    })
}

fn parse_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::assign_stmt => parse_assign_stmt(pair),
        Rule::return_stmt => parse_return_stmt(pair),
        Rule::if_stmt => parse_if_stmt(pair),
        Rule::for_stmt => parse_for_stmt(pair),
        Rule::loop_stmt => parse_loop_stmt(pair),
        Rule::call_stmt => parse_call_stmt(pair),
        _ => Err(vec![Diagnostic::syntax(
            "unexpected statement kind in parser",
            0,
            0,
        )]),
    }
}

fn parse_assign_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(kind_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            "missing assignment statement",
            0,
            0,
        )]);
    };

    match kind_pair.as_rule() {
        Rule::typed_assign_stmt => {
            let mut typed_inner = kind_pair.into_inner();
            let Some(name_pair) = typed_inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing typed assignment target",
                    0,
                    0,
                )]);
            };
            let Some(ty_pair) = typed_inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing typed assignment type",
                    0,
                    0,
                )]);
            };
            let ty_pair = if ty_pair.as_rule() == Rule::typed_decl_type {
                let mut inner = ty_pair.into_inner();
                let Some(actual) = inner.next() else {
                    return Err(vec![Diagnostic::syntax(
                        "missing typed declaration type",
                        0,
                        0,
                    )]);
                };
                actual
            } else {
                ty_pair
            };
            let expr_pair = typed_inner.next();
            match ty_pair.as_rule() {
                Rule::type_name => {
                    let Some(expr_pair) = expr_pair else {
                        return Err(vec![Diagnostic::syntax(
                            "missing typed assignment expression",
                            0,
                            0,
                        )]);
                    };
                    let decl_ty = parse_primitive_type(ty_pair.as_str()).map_err(|d| vec![d])?;
                    Ok(Stmt::Assign {
                        loc: loc.clone(),
                        target: AssignTarget::Var(name_pair.as_str().to_owned()),
                        decl_ty: Some(decl_ty),
                        generic_decl_ty: None,
                        is_typed_decl: true,
                        expr: parse_expr(expr_pair)?,
                    })
                }
                Rule::array_type => {
                    let init = if let Some(expr_pair) = expr_pair {
                        let init_expr = parse_expr(expr_pair)?;
                        match init_expr {
                            Expr::ArrayLiteral(values) => Some(values),
                            _ => {
                                return Err(vec![Diagnostic::syntax(
                                    "array typed declaration initializer must be an array literal like [a, b, ...]",
                                    0,
                                    0,
                                )])
                            }
                        }
                    } else {
                        None
                    };
                    Ok(Stmt::Assign {
                        loc: loc.clone(),
                        target: AssignTarget::Var(name_pair.as_str().to_owned()),
                        decl_ty: None,
                        generic_decl_ty: None,
                        is_typed_decl: true,
                        expr: Expr::DataCtor {
                            spec: parse_array_type_as_data_spec(ty_pair)?,
                            init,
                        },
                    })
                }
                Rule::ident => {
                    let Some(expr_pair) = expr_pair else {
                        return Err(vec![Diagnostic::syntax(
                            "missing typed assignment expression",
                            0,
                            0,
                        )]);
                    };
                    Ok(Stmt::Assign {
                        loc: loc.clone(),
                        target: AssignTarget::Var(name_pair.as_str().to_owned()),
                        decl_ty: None,
                        generic_decl_ty: Some(ty_pair.as_str().to_owned()),
                        is_typed_decl: true,
                        expr: parse_expr(expr_pair)?,
                    })
                }
                _ => Err(vec![Diagnostic::syntax(
                    "unexpected typed declaration type",
                    0,
                    0,
                )]),
            }
        }
        Rule::plain_assign_stmt => {
            let mut plain_inner = kind_pair.into_inner();
            let Some(target_pair) = plain_inner.next() else {
                return Err(vec![Diagnostic::syntax("missing assignment target", 0, 0)]);
            };
            let Some(expr_pair) = plain_inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing assignment expression",
                    0,
                    0,
                )]);
            };
            if target_pair.as_rule() == Rule::index_target {
                let mut target_inner = target_pair.clone().into_inner();
                let Some(base_pair) = target_inner.next() else {
                    return Err(vec![Diagnostic::syntax(
                        "missing indexed assignment base",
                        0,
                        0,
                    )]);
                };
                let Some(first_index_pair) = target_inner.next() else {
                    return Err(vec![Diagnostic::syntax(
                        "missing indexed assignment index",
                        0,
                        0,
                    )]);
                };
                if let Some(second_index_pair) = target_inner.next() {
                    let value_expr = parse_expr(expr_pair)?;
                    return Ok(Stmt::Expr {
                        loc,
                        expr: Expr::UserCall {
                            name: BUFFER_WRITE2_INTERNAL_FN.to_owned(),
                            type_args: Vec::new(),
                            args: vec![
                                CallArg {
                                    name: None,
                                    expr: Expr::Var(base_pair.as_str().to_owned()),
                                },
                                CallArg {
                                    name: None,
                                    expr: parse_expr(first_index_pair)?,
                                },
                                CallArg {
                                    name: None,
                                    expr: parse_expr(second_index_pair)?,
                                },
                                CallArg {
                                    name: None,
                                    expr: value_expr,
                                },
                            ],
                        },
                    });
                }
            }
            Ok(Stmt::Assign {
                loc,
                target: parse_assign_target(target_pair)?,
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                expr: parse_expr(expr_pair)?,
            })
        }
        _ => Err(vec![Diagnostic::syntax(
            "unexpected assignment statement kind",
            0,
            0,
        )]),
    }
}

fn parse_return_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(expr_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing return expression", 0, 0)]);
    };
    let expr = parse_expr(expr_pair)?;
    Ok(Stmt::Return { loc, expr })
}

fn parse_if_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    fn parse_if_cond_pair(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
        match pair.as_rule() {
            Rule::if_cond => {
                let mut inner = pair.into_inner();
                let Some(expr_pair) = inner.next() else {
                    return Err(vec![Diagnostic::syntax("missing if condition", 0, 0)]);
                };
                parse_expr(expr_pair)
            }
            Rule::expr => parse_expr(pair),
            _ => Err(vec![Diagnostic::syntax(
                "internal parser error: expected if condition",
                0,
                0,
            )]),
        }
    }

    let if_loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(cond_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing if condition", 0, 0)]);
    };
    let cond = parse_if_cond_pair(cond_pair)?;

    let Some(then_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing if then block", 0, 0)]);
    };
    let then_branch = parse_stmt_block(then_pair)?;

    let mut elifs = Vec::<(Expr, Vec<Stmt>, Option<SourceLoc>)>::new();
    let mut explicit_else: Option<Vec<Stmt>> = None;
    for item in inner {
        match item.as_rule() {
            Rule::elif_clause => {
                let elif_loc = stmt_loc_from_pair(&item);
                let mut elif_inner = item.into_inner();
                let Some(elif_cond_pair) = elif_inner.next() else {
                    return Err(vec![Diagnostic::syntax("missing elif condition", 0, 0)]);
                };
                let Some(elif_then_pair) = elif_inner.next() else {
                    return Err(vec![Diagnostic::syntax("missing elif block", 0, 0)]);
                };
                elifs.push((
                    parse_if_cond_pair(elif_cond_pair)?,
                    parse_stmt_block(elif_then_pair)?,
                    elif_loc,
                ));
            }
            Rule::stmt_block => {
                if explicit_else.is_some() {
                    return Err(vec![Diagnostic::syntax(
                        "duplicate else block in if statement",
                        0,
                        0,
                    )]);
                }
                explicit_else = Some(parse_stmt_block(item)?);
            }
            _ => {
                return Err(vec![Diagnostic::syntax(
                    "unexpected token in if statement",
                    0,
                    0,
                )]);
            }
        }
    }
    let mut else_branch = explicit_else.unwrap_or_default();
    for (elif_cond, elif_then, elif_loc) in elifs.into_iter().rev() {
        else_branch = vec![Stmt::If {
            loc: elif_loc,
            cond: elif_cond,
            then_branch: elif_then,
            else_branch,
        }];
    }

    Ok(Stmt::If {
        loc: if_loc,
        cond,
        then_branch,
        else_branch,
    })
}

fn parse_call_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(call_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing call expression", 0, 0)]);
    };
    let expr = parse_primary_expr(call_pair);
    Ok(Stmt::Expr { loc, expr })
}

fn parse_for_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(var_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing for loop variable", 0, 0)]);
    };
    let Some(start_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing for loop start", 0, 0)]);
    };
    let Some(end_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing for loop end", 0, 0)]);
    };
    let Some(body_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing for loop body", 0, 0)]);
    };

    let start = parse_for_bound(start_pair)?;
    let end = parse_for_bound(end_pair)?;
    let body = parse_stmt_block(body_pair)?;

    Ok(Stmt::For {
        loc,
        var: var_pair.as_str().to_owned(),
        start,
        end,
        body,
    })
}

fn parse_loop_stmt(pair: Pair<'_, Rule>) -> Result<Stmt, Vec<Diagnostic>> {
    let loc = stmt_loc_from_pair(&pair);
    let mut inner = pair.into_inner();
    let Some(count_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing loop count", 0, 0)]);
    };
    let Some(body_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing loop body", 0, 0)]);
    };

    let count = parse_for_bound(count_pair)?;
    let body = parse_stmt_block(body_pair)?;
    Ok(Stmt::For {
        loc,
        var: "_".to_owned(),
        start: Expr::Int(0),
        end: count,
        body,
    })
}

fn parse_for_bound(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::int_lit => Ok(Expr::Int(parse_int(pair.as_str())? as i64)),
        Rule::path_ident => Ok(Expr::Var(pair.as_str().to_owned())),
        Rule::for_bound => {
            let mut inner = pair.into_inner();
            let Some(inner_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing for/loop bound expression",
                    0,
                    0,
                )]);
            };
            parse_for_bound(inner_pair)
        }
        _ => Err(vec![Diagnostic::syntax(
            "for/loop bound must be an integer literal or variable path",
            0,
            0,
        )]),
    }
}

fn parse_stmt_block(pair: Pair<'_, Rule>) -> Result<Vec<Stmt>, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::stmt_block {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected statement block",
            0,
            0,
        )]);
    }

    let mut stmts = Vec::new();
    for child in pair.into_inner() {
        if child.as_rule() != Rule::stmt_list {
            continue;
        }
        for stmt_pair in child.into_inner() {
            stmts.push(parse_stmt(stmt_pair)?);
        }
    }
    Ok(stmts)
}

fn parse_expr(pair: Pair<'_, Rule>) -> Result<Expr, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::expr {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected expression pair",
            0,
            0,
        )]);
    }
    Ok(parse_expr_inner(pair))
}

fn parse_expr_inner(pair: Pair<'_, Rule>) -> Expr {
    let pratt = PrattParser::new()
        .op(Op::prefix(Rule::prefix))
        .op(Op::infix(Rule::or_op, Assoc::Left))
        .op(Op::infix(Rule::and_op, Assoc::Left))
        .op(Op::infix(Rule::cmp_op, Assoc::Left))
        .op(Op::infix(Rule::add_op, Assoc::Left))
        .op(Op::infix(Rule::mul_op, Assoc::Left));

    pratt
        .map_primary(parse_primary_expr)
        .map_prefix(|op, rhs| match op.as_str() {
            "-" => Expr::Binary {
                op: BinaryOp::Sub,
                lhs: Box::new(Expr::Number(0.0)),
                rhs: Box::new(rhs),
            },
            "!" => Expr::UnaryNot {
                expr: Box::new(rhs),
            },
            _ => unreachable!("unknown prefix operator"),
        })
        .map_infix(|lhs, op, rhs| match (op.as_rule(), op.as_str()) {
            (Rule::or_op, "||") => Expr::Logical {
                op: LogicalOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::and_op, "&&") => Expr::Logical {
                op: LogicalOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::cmp_op, "==") => Expr::Compare {
                op: CmpOp::Eq,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::cmp_op, "!=") => Expr::Compare {
                op: CmpOp::Ne,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::cmp_op, "<") => Expr::Compare {
                op: CmpOp::Lt,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::cmp_op, "<=") => Expr::Compare {
                op: CmpOp::Le,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::cmp_op, ">") => Expr::Compare {
                op: CmpOp::Gt,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::cmp_op, ">=") => Expr::Compare {
                op: CmpOp::Ge,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::add_op, "+") => Expr::Binary {
                op: BinaryOp::Add,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::add_op, "-") => Expr::Binary {
                op: BinaryOp::Sub,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::mul_op, "*") => Expr::Binary {
                op: BinaryOp::Mul,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::mul_op, "/") => Expr::Binary {
                op: BinaryOp::Div,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            (Rule::mul_op, "%") => Expr::Binary {
                op: BinaryOp::Mod,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            _ => unreachable!("unknown infix operator"),
        })
        .parse(pair.into_inner())
}

fn parse_primary_expr(pair: Pair<'_, Rule>) -> Expr {
    match pair.as_rule() {
        Rule::number => {
            let text = pair.as_str();
            if text.contains('.') {
                Expr::Number(
                    text.parse::<f32>()
                        .expect("pest number rule produced invalid float literal"),
                )
            } else {
                Expr::Int(
                    text.parse::<i64>()
                        .expect("pest number rule produced invalid int literal"),
                )
            }
        }
        Rule::bool_lit => Expr::Bool(pair.as_str() == "true"),
        Rule::array_lit => Expr::ArrayLiteral(
            pair.into_inner()
                .filter(|p| p.as_rule() == Rule::expr)
                .map(parse_expr_inner)
                .collect(),
        ),
        Rule::ident | Rule::path_ident => Expr::Var(pair.as_str().to_owned()),
        Rule::index_expr => {
            let mut inner = pair.into_inner();
            let base = inner
                .next()
                .expect("index_expr rule must include base path")
                .as_str()
                .to_owned();
            let idx_pair = inner
                .next()
                .expect("index_expr rule must include index expression");
            let idx_first = parse_expr_inner(idx_pair);
            if let Some(idx_second_pair) = inner.next() {
                Expr::UserCall {
                    name: BUFFER_READ2_INTERNAL_FN.to_owned(),
                    type_args: Vec::new(),
                    args: vec![
                        CallArg {
                            name: None,
                            expr: Expr::Var(base),
                        },
                        CallArg {
                            name: None,
                            expr: idx_first,
                        },
                        CallArg {
                            name: None,
                            expr: parse_expr_inner(idx_second_pair),
                        },
                    ],
                }
            } else {
                Expr::Index {
                    base,
                    index: Box::new(idx_first),
                }
            }
        }
        Rule::data_ctor => Expr::DataCtor {
            spec: parse_data_type_spec(pair)
                .expect("data_ctor rule must include capacity expression"),
            init: None,
        },
        Rule::call_index_expr => {
            let mut inner = pair.into_inner();
            let call_pair = inner
                .next()
                .expect("call_index_expr rule must include call expression");
            let index_pair = inner
                .next()
                .expect("call_index_expr rule must include index expression");
            let (name, type_args, mut args) = parse_call_expr_parts(call_pair);
            args.push(CallArg {
                name: Some(PROC_INDEX_SENTINEL_ARG.to_owned()),
                expr: parse_expr_inner(index_pair),
            });
            Expr::UserCall {
                name: format!("{PROC_INDEX_SENTINEL_PREFIX}{name}"),
                type_args,
                args,
            }
        }
        Rule::call_expr => {
            let (name, type_args, mut args) = parse_call_expr_parts(pair);

            if type_args.is_empty() {
                if let Ok(to_ty) = parse_primitive_type(&name) {
                    if to_ty != PrimitiveType::Bool && args.len() == 1 && args[0].name.is_none() {
                        return Expr::Cast {
                            to: to_ty,
                            expr: Box::new(args.remove(0).expr),
                        };
                    }
                }
            }

            if type_args.is_empty() {
                if let Some(func) = parse_builtin_fn(&name) {
                    if args.iter().all(|a| a.name.is_none()) {
                        return Expr::Call {
                            func,
                            args: args.into_iter().map(|a| a.expr).collect(),
                        };
                    }
                }
            }

            Expr::UserCall {
                name,
                type_args,
                args,
            }
        }
        Rule::expr => parse_expr_inner(pair),
        _ => unreachable!("unexpected primary expression token"),
    }
}

fn parse_call_expr_parts(pair: Pair<'_, Rule>) -> (String, Vec<CallTypeArg>, Vec<CallArg>) {
    assert!(
        pair.as_rule() == Rule::call_expr,
        "parse_call_expr_parts expects call_expr pair"
    );
    let mut inner = pair.into_inner();
    let name_pair = inner
        .next()
        .expect("call_expr rule must include function name");
    let name = name_pair.as_str().to_owned();

    let mut type_args = Vec::new();
    let mut args = Vec::new();
    for item in inner {
        match item.as_rule() {
            Rule::generic_type_arg_list => {
                for ty in item.into_inner() {
                    let mut push_arg = |pair: Pair<'_, Rule>| match pair.as_rule() {
                        Rule::type_name => {
                            type_args.push(CallTypeArg::Primitive(
                                parse_primitive_type(pair.as_str()).expect(
                                    "generic_type_arg_list type_name should parse to primitive",
                                ),
                            ));
                        }
                        Rule::ident => {
                            type_args.push(CallTypeArg::Generic(pair.as_str().to_owned()));
                        }
                        _ => {}
                    };
                    match ty.as_rule() {
                        Rule::generic_type_arg => {
                            let mut inner = ty.into_inner();
                            if let Some(inner_pair) = inner.next() {
                                push_arg(inner_pair);
                            }
                        }
                        Rule::type_name | Rule::ident => push_arg(ty),
                        _ => {}
                    }
                }
            }
            Rule::arg_list => {
                for arg_pair in item.into_inner() {
                    if arg_pair.as_rule() == Rule::call_arg {
                        let mut arg_inner = arg_pair.into_inner();
                        let Some(first) = arg_inner.next() else {
                            continue;
                        };
                        let (arg_name, expr_pair) = match (first.as_rule(), arg_inner.next()) {
                            (Rule::ident, Some(expr_pair)) => {
                                (Some(first.as_str().to_owned()), expr_pair)
                            }
                            (Rule::expr, None) => (None, first),
                            _ => continue,
                        };
                        args.push(CallArg {
                            name: arg_name,
                            expr: parse_expr_inner(expr_pair),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    (name, type_args, args)
}

fn parse_int(text: &str) -> Result<i32, Vec<Diagnostic>> {
    text.parse::<i32>().map_err(|_| {
        vec![Diagnostic::syntax(
            format!("invalid integer literal '{text}'"),
            0,
            0,
        )]
    })
}

fn parse_primitive_type(text: &str) -> Result<PrimitiveType, Diagnostic> {
    match text {
        "f32" => Ok(PrimitiveType::F32),
        "f64" => Ok(PrimitiveType::F64),
        "i32" => Ok(PrimitiveType::I32),
        "i64" => Ok(PrimitiveType::I64),
        "bool" => Ok(PrimitiveType::Bool),
        _ => Err(Diagnostic::syntax(
            format!("unsupported primitive type '{text}'"),
            0,
            0,
        )),
    }
}

fn parse_decl_type(pair: Pair<'_, Rule>) -> Result<DeclType, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::type_name => Ok(DeclType::Scalar(
            parse_primitive_type(pair.as_str()).map_err(|d| vec![d])?,
        )),
        Rule::qualified_ident => Ok(DeclType::Generic(pair.as_str().to_owned())),
        Rule::array_type => {
            let mut inner = pair.into_inner();
            let Some(elem_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax("missing array element type", 0, 0)]);
            };
            let Some(size_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax("missing array size", 0, 0)]);
            };
            match elem_pair.as_rule() {
                Rule::type_name => {
                    let elem = parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?;
                    Ok(DeclType::Array {
                        elem,
                        size: parse_expr_inner(size_pair),
                    })
                }
                Rule::qualified_ident => Ok(DeclType::ArrayGeneric {
                    elem: elem_pair.as_str().to_owned(),
                    size: parse_expr_inner(size_pair),
                }),
                _ => Err(vec![Diagnostic::syntax(
                    "array declarations for ports/params require primitive or generic element type",
                    0,
                    0,
                )]),
            }
        }
        _ => Err(vec![Diagnostic::syntax(
            "unsupported declaration type",
            0,
            0,
        )]),
    }
}

fn parse_builtin_fn(name: &str) -> Option<BuiltinFn> {
    match name {
        "sin" => Some(BuiltinFn::Sin),
        "cos" => Some(BuiltinFn::Cos),
        "tan" => Some(BuiltinFn::Tan),
        "tanh" => Some(BuiltinFn::Tanh),
        "atan" => Some(BuiltinFn::Atan),
        "atan2" => Some(BuiltinFn::Atan2),
        "exp" => Some(BuiltinFn::Exp),
        "log" => Some(BuiltinFn::Log),
        "sqrt" => Some(BuiltinFn::Sqrt),
        "pow" => Some(BuiltinFn::Pow),
        "abs" | "fabs" => Some(BuiltinFn::Abs),
        "floor" => Some(BuiltinFn::Floor),
        "ceil" => Some(BuiltinFn::Ceil),
        "round" => Some(BuiltinFn::Round),
        "trunc" => Some(BuiltinFn::Trunc),
        "min" => Some(BuiltinFn::Min),
        "max" => Some(BuiltinFn::Max),
        "fma" => Some(BuiltinFn::Fma),
        _ => None,
    }
}

fn parse_fn_param_type(pair: Pair<'_, Rule>) -> Result<FnParamType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::fn_param_type {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected function parameter type",
            0,
            0,
        )]);
    }
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![Diagnostic::syntax(
            "missing function parameter type",
            0,
            0,
        )]);
    };
    let out = match inner.as_rule() {
        Rule::buffer_type => FnParamType::Buffer(parse_buffer_type(inner)?),
        Rule::type_name => {
            FnParamType::Primitive(parse_primitive_type(inner.as_str()).map_err(|d| vec![d])?)
        }
        Rule::qualified_ident => FnParamType::Struct(inner.as_str().to_owned()),
        _ => {
            return Err(vec![Diagnostic::syntax(
                "unsupported function parameter type",
                0,
                0,
            )])
        }
    };
    Ok(out)
}

fn parse_buffer_type(pair: Pair<'_, Rule>) -> Result<BufferType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::buffer_type {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected buffer type",
            0,
            0,
        )]);
    }
    let Some(inner) = pair.into_inner().next() else {
        return Err(vec![Diagnostic::syntax(
            "missing buffer type contents",
            0,
            0,
        )]);
    };
    if inner.as_rule() != Rule::buffer_inner {
        return Err(vec![Diagnostic::syntax(
            "invalid buffer type contents",
            0,
            0,
        )]);
    }
    parse_buffer_inner(inner)
}

fn parse_buffer_decl_type(pair: Pair<'_, Rule>) -> Result<BufferType, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::buffer_decl_type => {
            let mut inner = pair.into_inner();
            let Some(actual) = inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing buffers declaration type",
                    0,
                    0,
                )]);
            };
            parse_buffer_decl_type(actual)
        }
        Rule::buffer_type => parse_buffer_type(pair),
        Rule::buffer_inner => parse_buffer_inner(pair),
        _ => Err(vec![Diagnostic::syntax(
            "invalid buffers declaration type",
            0,
            0,
        )]),
    }
}

fn parse_buffer_inner(inner_pair: Pair<'_, Rule>) -> Result<BufferType, Vec<Diagnostic>> {
    // Rule::buffer_inner shape:
    //   (type_name|qualified_ident)
    //   (type_name|qualified_ident) "[" "]"
    //   (type_name|qualified_ident) "[" expr "]"
    let text = inner_pair.as_str().trim().to_owned();
    let mut iter = inner_pair.into_inner();
    let Some(elem_pair) = iter.next() else {
        return Err(vec![Diagnostic::syntax(
            "missing buffer element type",
            0,
            0,
        )]);
    };
    let elem = match elem_pair.as_rule() {
        Rule::type_name => BufferElemType::Primitive(
            parse_primitive_type(elem_pair.as_str()).map_err(|d| vec![d])?,
        ),
        Rule::qualified_ident => BufferElemType::Generic(elem_pair.as_str().to_owned()),
        _ => {
            return Err(vec![Diagnostic::syntax(
                "missing or invalid buffer element type",
                0,
                0,
            )]);
        }
    };
    let size_expr = iter.next().map(parse_expr_inner);
    let has_brackets = text.contains('[');
    let channels = if !has_brackets {
        BufferChannels::Mono
    } else if let Some(expr) = size_expr {
        BufferChannels::Static(expr)
    } else {
        BufferChannels::Dynamic
    };
    Ok(BufferType { elem, channels })
}
fn parse_data_elem_type(text: &str) -> DataElemType {
    match parse_primitive_type(text) {
        Ok(prim) => DataElemType::Primitive(prim),
        Err(_) => DataElemType::Struct(text.to_owned()),
    }
}

fn parse_array_type_as_data_spec(pair: Pair<'_, Rule>) -> Result<DataTypeSpec, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::array_type {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected array type",
            0,
            0,
        )]);
    }
    let mut inner = pair.into_inner();
    let Some(elem_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing array element type", 0, 0)]);
    };
    let Some(size_pair) = inner.next() else {
        return Err(vec![Diagnostic::syntax("missing array size", 0, 0)]);
    };
    let elem = match elem_pair.as_rule() {
        Rule::type_name => parse_data_elem_type(elem_pair.as_str()),
        Rule::qualified_ident => DataElemType::Struct(elem_pair.as_str().to_owned()),
        _ => return Err(vec![Diagnostic::syntax("invalid array element type", 0, 0)]),
    };
    Ok(DataTypeSpec {
        elem,
        size: Box::new(parse_expr_inner(size_pair)),
    })
}

fn parse_data_type_spec(pair: Pair<'_, Rule>) -> Result<DataTypeSpec, Vec<Diagnostic>> {
    let mut inner = pair.into_inner();
    let Some(first) = inner.next() else {
        return Err(vec![Diagnostic::syntax(
            "missing Data element or capacity",
            0,
            0,
        )]);
    };

    let (elem, size_pair) = match first.as_rule() {
        Rule::data_elem_type => {
            let Some(size_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax("missing Data capacity", 0, 0)]);
            };
            (parse_data_elem_type(first.as_str()), size_pair)
        }
        Rule::expr => (DataElemType::Primitive(PrimitiveType::F32), first),
        _ => {
            return Err(vec![Diagnostic::syntax(
                "invalid Data type/capacity syntax",
                0,
                0,
            )]);
        }
    };

    Ok(DataTypeSpec {
        elem,
        size: Box::new(parse_expr_inner(size_pair)),
    })
}
fn parse_field_type(pair: Pair<'_, Rule>) -> Result<FieldType, Vec<Diagnostic>> {
    if pair.as_rule() != Rule::field_type {
        return Err(vec![Diagnostic::syntax(
            "internal parser error: expected field type",
            0,
            0,
        )]);
    }
    let text = pair.as_str().trim().to_owned();

    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::type_name => {
                let ty = parse_primitive_type(child.as_str()).map_err(|d| vec![d])?;
                return Ok(FieldType::Scalar(ty));
            }
            Rule::qualified_ident => {
                return Ok(FieldType::Generic(child.as_str().to_owned()));
            }
            Rule::array_type => {
                return Ok(FieldType::Data(parse_array_type_as_data_spec(child)?));
            }
            Rule::data_type => {
                return Ok(FieldType::Data(parse_data_type_spec(child)?));
            }
            _ => {}
        }
    }

    Err(vec![Diagnostic::syntax(
        format!("unsupported struct field type '{text}'"),
        0,
        0,
    )])
}

fn parse_assign_target(pair: Pair<'_, Rule>) -> Result<AssignTarget, Vec<Diagnostic>> {
    match pair.as_rule() {
        Rule::path_ident => Ok(AssignTarget::Var(pair.as_str().to_owned())),
        Rule::index_target => {
            let mut inner = pair.into_inner();
            let Some(base_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing indexed assignment base",
                    0,
                    0,
                )]);
            };
            let Some(index_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax(
                    "missing indexed assignment index",
                    0,
                    0,
                )]);
            };
            if inner.next().is_some() {
                return Err(vec![Diagnostic::syntax(
                    "nested indexed assignment targets must use parser rewrite path",
                    0,
                    0,
                )]);
            }
            Ok(AssignTarget::Index {
                base: base_pair.as_str().to_owned(),
                index: parse_expr(index_pair)?,
            })
        }
        Rule::assign_target => {
            let mut inner = pair.into_inner();
            let Some(inner_pair) = inner.next() else {
                return Err(vec![Diagnostic::syntax("missing assignment target", 0, 0)]);
            };
            parse_assign_target(inner_pair)
        }
        _ => Err(vec![Diagnostic::syntax(
            "unexpected assignment target",
            0,
            0,
        )]),
    }
}

fn diag_from_pest_error(err: pest::error::Error<Rule>) -> Diagnostic {
    let (line, column) = match err.line_col {
        LineColLocation::Pos((line, col)) => (line, col),
        LineColLocation::Span((line, col), _) => (line, col),
    };
    Diagnostic::syntax(err.to_string(), line, column)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::ast::{
        BinaryOp, Block, BufferElemType, BuiltinFn, CallTypeArg, DataElemType, DeclType, Expr,
        FieldType, PrimitiveType, Stmt,
    };

    use super::{
        parse_program, parse_program_file, PROC_INDEX_SENTINEL_ARG, PROC_INDEX_SENTINEL_PREFIX,
    };

    fn mk_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("omni_frontend_{prefix}_{nanos}"));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, text).expect("write test file");
    }

    #[test]
    fn parses_gain_program() {
        let src = r#"
ins {
  in1
}
outs {
  out1
}
params {
  gain = 0.5
}
sample {
  out1 = in1 * gain
}
"#;

        let program = parse_program(src).expect("gain should parse");
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Ins(_))));
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Outs(_))));
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Params(_))));
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Sample(_))));
    }

    #[test]
    fn parses_sine_program() {
        let src = r#"
outs {
  out1
}
params {
  freq = 440.0
}
init {
  phase = 0.0
}
sample {
  phase = phase + freq / 48000.0
  out1 = sin(phase)
}
"#;

        let program = parse_program(src).expect("sine should parse");
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Init(_))));
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Sample(_))));
    }

    #[test]
    fn parses_one_pole_program() {
        let src = r#"
inputs {
  in1
}
outputs {
  out1
}
params {
  a = 0.1
}
init {
  z = 0.0
}
sample {
  z = z + a * (in1 - z)
  out1 = z
}
"#;

        let program = parse_program(src).expect("one-pole should parse");
        assert_eq!(program.blocks.len(), 5);
    }

    #[test]
    fn parses_expr_precedence() {
        let src = r#"
outs {
  out1
}
sample {
  out1 = a + b * c
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let expr = match &sample[0] {
            Stmt::Assign { expr, .. } => expr,
            _ => panic!("first statement should be assignment"),
        };
        match expr {
            Expr::Binary {
                op: BinaryOp::Add,
                lhs: _,
                rhs,
            } => match rhs.as_ref() {
                Expr::Binary {
                    op: BinaryOp::Mul, ..
                } => {}
                _ => panic!("rhs should be multiplication"),
            },
            _ => panic!("top-level should be addition"),
        }
    }

    #[test]
    fn parses_modulo_with_mul_div_precedence() {
        let src = r#"
outs {
  out1
}
sample {
  out1 = a + b % c * d
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let expr = match &sample[0] {
            Stmt::Assign { expr, .. } => expr,
            _ => panic!("first statement should be assignment"),
        };
        let Expr::Binary {
            op: BinaryOp::Add,
            rhs,
            ..
        } = expr
        else {
            panic!("top-level should be addition");
        };
        let Expr::Binary {
            op: BinaryOp::Mul,
            lhs: mul_lhs,
            ..
        } = rhs.as_ref()
        else {
            panic!("rhs should be multiplication");
        };
        let Expr::Binary {
            op: BinaryOp::Mod, ..
        } = mul_lhs.as_ref()
        else {
            panic!("left side of multiplication should be modulo");
        };
    }

    #[test]
    fn parses_sin_call_expression() {
        let src = r#"
outs {
  out1
}
sample {
  out1 = sin(a + b)
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let expr = match &sample[0] {
            Stmt::Assign { expr, .. } => expr,
            _ => panic!("first statement should be assignment"),
        };
        match expr {
            Expr::Call { .. } => {}
            _ => panic!("top-level should be a function call"),
        }
    }

    #[test]
    fn parses_variadic_builtin_call_expression() {
        let src = r#"
outs {
  out1
}
sample {
  out1 = fma(a, b, c)
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let expr = match &sample[0] {
            Stmt::Assign { expr, .. } => expr,
            _ => panic!("first statement should be assignment"),
        };
        match expr {
            Expr::Call {
                func: BuiltinFn::Fma,
                args,
            } => assert_eq!(args.len(), 3),
            _ => panic!("top-level should be an fma builtin call"),
        }
    }

    #[test]
    fn parses_semicolon_separated_statements() {
        let src = r#"
outs { out1 }
params { x = 1.0; y = 2.0 }
sample { out1 = x; out1 = out1 + y }
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        assert_eq!(sample.len(), 2);
    }

    #[test]
    fn parses_if_statement() {
        let src = r#"
outs { out1 }
sample {
  if (x > 0.0) { out1 = x } else { out1 = 0.0 }
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        match &sample[0] {
            Stmt::If { .. } => {}
            _ => panic!("expected if statement"),
        }
    }

    #[test]
    fn parses_if_elif_else_statement_as_nested_if() {
        let src = r#"
outs { out1 }
sample {
  if (x > 0.0) { out1 = 1.0 } elif (x > -1.0) { out1 = 0.5 } else { out1 = 0.0 }
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let Stmt::If { else_branch, .. } = &sample[0] else {
            panic!("expected top-level if statement");
        };
        assert_eq!(else_branch.len(), 1, "expected single nested elif");
        let Stmt::If {
            else_branch: nested_else,
            ..
        } = &else_branch[0]
        else {
            panic!("expected nested if for elif");
        };
        assert_eq!(nested_else.len(), 1, "expected trailing else branch");
    }

    #[test]
    fn parses_if_elif_else_without_parentheses() {
        let src = r#"
outs { out1 }
sample {
  if x > 0.0 { out1 = 1.0 } elif x > -1.0 { out1 = 0.5 } else { out1 = 0.0 }
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let Stmt::If { else_branch, .. } = &sample[0] else {
            panic!("expected top-level if statement");
        };
        assert_eq!(else_branch.len(), 1, "expected single nested elif");
        let Stmt::If {
            else_branch: nested_else,
            ..
        } = &else_branch[0]
        else {
            panic!("expected nested if for elif");
        };
        assert_eq!(nested_else.len(), 1, "expected trailing else branch");
    }

    #[test]
    fn parses_for_statement() {
        let src = r#"
outs { out1 }
sample {
  for i in 0..4 { out1 = out1 + 1.0 }
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        match &sample[0] {
            Stmt::For { .. } => {}
            _ => panic!("expected for statement"),
        }
    }

    #[test]
    fn parses_for_statement_with_variable_bound() {
        let src = r#"
outs { out1 }
sample {
  n = 4
  for i in 0..n { out1 = out1 + 1.0 }
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        match &sample[1] {
            Stmt::For { start, end, .. } => {
                assert!(matches!(start, Expr::Int(0)));
                assert!(matches!(end, Expr::Var(v) if v == "n"));
            }
            _ => panic!("expected for statement"),
        }
    }

    #[test]
    fn parses_loop_statement_as_for_sugar() {
        let src = r#"
outs { out1 }
sample {
  loop 4 { out1 = out1 + 1.0 }
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        match &sample[0] {
            Stmt::For {
                var, start, end, ..
            } => {
                assert_eq!(var, "_");
                assert!(matches!(start, Expr::Int(0)));
                assert!(matches!(end, Expr::Int(4)));
            }
            _ => panic!("expected for statement from loop sugar"),
        }
    }

    #[test]
    fn parses_def_and_call() {
        let src = r#"
outs { out1 }
def add2(a, b) {
  x = a + b
  return x
}
sample {
  out1 = add2(0.25, 0.5)
}
"#;
        let program = parse_program(src).expect("program should parse");
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
    }

    #[test]
    fn parses_struct_and_field_access() {
        let src = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
init {
  p = Pair(0.5, 0.25)
}
sample {
  out1 = p.a + p.b
}
"#;
        let program = parse_program(src).expect("program should parse");
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Struct(_))));
    }

    #[test]
    fn parses_struct_methods_after_fields() {
        let src = r#"
outs { out1 }
struct Voice {
  phase: f32
  gain: f32
  def tick(self, freq) {
    self.phase = self.phase + freq
    return self.phase * self.gain
  }
}
init {
  v = Voice(0.0, 0.5)
}
sample {
  out1 = Voice.tick(v, 1.0)
}
"#;
        let program = parse_program(src).expect("program should parse");
        let st = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Struct(s) => Some(s),
                _ => None,
            })
            .expect("struct block");
        assert_eq!(st.methods.len(), 1);
        assert_eq!(st.methods[0].name, "tick");
    }

    #[test]
    fn rejects_generic_struct_method_type_params() {
        let src = r#"
outs { out1 }
struct Voice {
  phase: f32
  def id[T](self, x: T) {
    return x
  }
}
sample {
  out1 = 0.0
}
"#;
        assert!(
            parse_program(src).is_err(),
            "generic method type params should be rejected"
        );
    }

    #[test]
    fn parses_struct_fields_without_explicit_type_as_f32() {
        let src = r#"
outs { out1 }
struct Tap {
  delay_samples
  gain
}
init {
  t = Tap()
}
sample {
  out1 = t.delay_samples + t.gain
}
"#;
        let program = parse_program(src).expect("program should parse");
        let st = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Struct(s) => Some(s),
                _ => None,
            })
            .expect("struct block");
        assert_eq!(st.fields.len(), 2);
        assert!(matches!(
            st.fields[0].ty,
            FieldType::Scalar(crate::ast::PrimitiveType::F32)
        ));
        assert!(matches!(
            st.fields[1].ty,
            FieldType::Scalar(crate::ast::PrimitiveType::F32)
        ));
    }

    #[test]
    fn infers_struct_field_type_from_default_when_untyped() {
        let src = r#"
outs { out1 }
struct X {
  field1 = 0.0
  field2 = 0
  field3: f64 = 0.0
  field4: i64 = 0
}
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("program should parse");
        let st = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Struct(s) => Some(s),
                _ => None,
            })
            .expect("struct block");
        assert_eq!(st.fields.len(), 4);
        assert!(matches!(
            st.fields[0].ty,
            FieldType::Scalar(crate::ast::PrimitiveType::F32)
        ));
        assert!(matches!(
            st.fields[1].ty,
            FieldType::Scalar(crate::ast::PrimitiveType::I32)
        ));
        assert!(matches!(
            st.fields[2].ty,
            FieldType::Scalar(crate::ast::PrimitiveType::F64)
        ));
        assert!(matches!(
            st.fields[3].ty,
            FieldType::Scalar(crate::ast::PrimitiveType::I64)
        ));
    }

    #[test]
    fn parses_data_ctor_and_index_access() {
        let src = r#"
outs { out1 }
init {
  buf: f32[8]
}
sample {
  out1 = buf[1.5]
  buf[2] = out1
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        assert_eq!(sample.len(), 2);
    }

    #[test]
    fn parses_call_statement() {
        let src = r#"
outs { out1 }
struct Voice {
  phase: f32
  def process(self) {
    self.phase = self.phase + 1.0
  }
}
init {
  v = Voice(0.0)
}
sample {
  v.process()
  out1 = v.phase
}
"#;
        let program = parse_program(src).expect("program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        assert_eq!(sample.len(), 2);
        match &sample[0] {
            Stmt::Expr { .. } => {}
            _ => panic!("first statement should be call expression statement"),
        }
    }

    #[test]
    fn parses_proc_block() {
        let src = r#"
proc Gain {
  ins { in1 }
  params { gain = 2.0 }
  outs { out1 }
  init { }
  sample { out1 = in1 * gain }
}
outs { out1 }
init { p = Gain() }
sample { out1 = p(0.5) }
"#;

        let program = parse_program(src).expect("parse_program should succeed");
        let proc = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("expected a proc block");
        assert_eq!(proc.name, "Gain");
        assert_eq!(proc.ins.len(), 1);
        assert_eq!(proc.outs.len(), 1);
        assert_eq!(proc.params.len(), 1);
    }

    #[test]
    fn parses_proc_block_wrapping_sample() {
        let src = r#"
proc Wrapped {
  ins { in1 }
  outs { out1 }
  block {
    acc = 1.0
    sample { out1 = in1 }
    acc = acc + 1.0
  }
}
sample { out1 = 0.0 }
"#;

        let program = parse_program(src).expect("parse_program should succeed");
        let proc = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("expected a proc block");
        assert!(proc.has_block_block);
        assert!(proc.has_sample_block);
        assert_eq!(proc.block_pre.len(), 1);
        assert_eq!(proc.sample.len(), 1);
        assert_eq!(proc.block_post.len(), 1);
    }

    #[test]
    fn rejects_proc_block_without_nested_sample() {
        let src = r#"
proc Wrapped {
  outs { out1 }
  block {
    x = 1.0
  }
}
sample { out1 = 0.0 }
"#;
        let result = parse_program(src);
        assert!(
            result.is_err(),
            "proc block without nested sample should error"
        );
    }

    #[test]
    fn parses_proc_block_wrapping_sample_with_indentation_syntax() {
        let src = r#"
proc Wrapped:
  ins:
    in1
  outs:
    out1
  block:
    acc = 1.0
    sample:
      out1 = in1 * acc
    acc = acc + 1.0
sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let proc = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("expected a proc block");
        assert!(proc.has_block_block);
        assert_eq!(proc.block_pre.len(), 1);
        assert_eq!(proc.sample.len(), 1);
        assert_eq!(proc.block_post.len(), 1);
    }

    #[test]
    fn parses_typed_top_level_ports_and_params() {
        let src = r#"
ins { in1: i32, in2 }
outs { out1: f64, out2 }
params { gain: f64 = 2.5, mode: i32 = 2, gate: bool = 1 }
sample { out1 = in1 * gain; out2 = mode + gate }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let ins = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Ins(v) => Some(v),
                _ => None,
            })
            .expect("ins block");
        assert_eq!(ins[0].name, "in1");
        assert_eq!(ins[0].ty, Some(DeclType::Scalar(PrimitiveType::I32)));
        assert_eq!(ins[1].name, "in2");
        assert_eq!(ins[1].ty, None);

        let outs = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Outs(v) => Some(v),
                _ => None,
            })
            .expect("outs block");
        assert_eq!(outs[0].name, "out1");
        assert_eq!(outs[0].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
        assert_eq!(outs[1].name, "out2");
        assert_eq!(outs[1].ty, None);

        let params = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Params(v) => Some(v),
                _ => None,
            })
            .expect("params block");
        assert_eq!(params[0].name, "gain");
        assert_eq!(params[0].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
        assert_eq!(params[1].name, "mode");
        assert_eq!(params[1].ty, Some(DeclType::Scalar(PrimitiveType::I32)));
        assert_eq!(params[2].name, "gate");
        assert_eq!(params[2].ty, Some(DeclType::Scalar(PrimitiveType::Bool)));
    }

    #[test]
    fn parses_ranges_and_count_prefix_with_explicit_lists() {
        let src = r#"
ins 2:
  in1 = 440 {22000}
  in2 = 440 {0.01, 22000}
outs 1
params 2:
  freq: i32 = 500 {8000}
  mix = 0.5 {0.0, 1.0}
sample:
  out1 = in1 + in2 + freq + mix
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let ins = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Ins(v) => Some(v),
                _ => None,
            })
            .expect("ins block");
        assert_eq!(ins.len(), 2);
        assert_eq!(ins[0].name, "in1");
        assert!(matches!(
            ins[0].default,
            Some(Expr::Number(_)) | Some(Expr::Int(_))
        ));
        let in1_range = ins[0].range.as_ref().expect("in1 range should be parsed");
        assert!(in1_range.min.is_none());
        assert!(matches!(in1_range.max, Expr::Int(22000)));
        let in2_range = ins[1].range.as_ref().expect("in2 range should be parsed");
        assert!(in2_range.min.is_some());
        assert!(matches!(in2_range.max, Expr::Number(_) | Expr::Int(22000)));

        let params = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Params(v) => Some(v),
                _ => None,
            })
            .expect("params block");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "freq");
        assert_eq!(params[0].ty, Some(DeclType::Scalar(PrimitiveType::I32)));
        assert!(matches!(params[0].default, Some(Expr::Int(500))));
        let freq_range = params[0]
            .range
            .as_ref()
            .expect("freq range should be parsed");
        assert!(freq_range.min.is_none());
        assert!(matches!(freq_range.max, Expr::Int(8000)));
        assert_eq!(params[1].name, "mix");
        let mix_range = params[1]
            .range
            .as_ref()
            .expect("mix range should be parsed");
        assert!(mix_range.min.is_some());
        assert!(matches!(mix_range.max, Expr::Number(_) | Expr::Int(1)));
    }

    #[test]
    fn rejects_count_prefix_mismatch_with_explicit_list() {
        let src = r#"
ins 2:
  in1
outs 1
sample:
  out1 = in1
"#;
        let result = parse_program(src);
        assert!(result.is_err(), "expected count prefix mismatch error");
    }

    #[test]
    fn rejects_out_defaults_or_ranges() {
        let src = r#"
outs:
  out1 = 0.0 {0.0, 1.0}
sample:
  out1 = 0.0
"#;
        let result = parse_program(src);
        assert!(result.is_err(), "expected outs default/range rejection");
    }

    #[test]
    fn parses_top_level_ins_outs_params_count_shorthand() {
        let src = r#"
ins 3
outs 2
params 4
sample { out1 = in1 + in2 + in3 + param1 + param2 + param3 + param4; out2 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");

        let ins = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Ins(v) => Some(v),
                _ => None,
            })
            .expect("ins block");
        assert_eq!(ins.len(), 3);
        assert_eq!(ins[0].name, "in1");
        assert_eq!(ins[1].name, "in2");
        assert_eq!(ins[2].name, "in3");
        assert!(ins.iter().all(|d| d.ty.is_none()));

        let outs = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Outs(v) => Some(v),
                _ => None,
            })
            .expect("outs block");
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0].name, "out1");
        assert_eq!(outs[1].name, "out2");
        assert!(outs.iter().all(|d| d.ty.is_none()));

        let params = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Params(v) => Some(v),
                _ => None,
            })
            .expect("params block");
        assert_eq!(params.len(), 4);
        assert_eq!(params[0].name, "param1");
        assert_eq!(params[1].name, "param2");
        assert_eq!(params[2].name, "param3");
        assert_eq!(params[3].name, "param4");
        assert!(params.iter().all(|d| d.ty.is_none() && d.default.is_none()));
    }

    #[test]
    fn parses_top_level_count_shorthand_with_section_default_types() {
        let src = r#"
ins[f64] 2
outs[i32] 1
params[bool] 3
buffers[f32] 2
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");

        let ins = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Ins(v) => Some(v),
                _ => None,
            })
            .expect("ins block");
        assert_eq!(ins.len(), 2);
        assert_eq!(ins[0].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
        assert_eq!(ins[1].ty, Some(DeclType::Scalar(PrimitiveType::F64)));

        let outs = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Outs(v) => Some(v),
                _ => None,
            })
            .expect("outs block");
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].ty, Some(DeclType::Scalar(PrimitiveType::I32)));

        let params = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Params(v) => Some(v),
                _ => None,
            })
            .expect("params block");
        assert_eq!(params.len(), 3);
        assert_eq!(params[0].ty, Some(DeclType::Scalar(PrimitiveType::Bool)));
        assert_eq!(params[1].ty, Some(DeclType::Scalar(PrimitiveType::Bool)));
        assert_eq!(params[2].ty, Some(DeclType::Scalar(PrimitiveType::Bool)));
        assert!(params.iter().all(|d| d.default.is_none()));

        let buffers = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Buffers(v) => Some(v),
                _ => None,
            })
            .expect("buffers block");
        assert_eq!(buffers.len(), 2);
        assert!(buffers.iter().all(|b| matches!(
            b.ty.as_ref().map(|t| (&t.elem, &t.channels)),
            Some((
                BufferElemType::Primitive(PrimitiveType::F32),
                crate::ast::BufferChannels::Mono
            ))
        )));
    }

    #[test]
    fn parses_proc_ins_outs_params_count_shorthand() {
        let src = r#"
proc Gain {
  ins 2
  params 1
  outs 1
  sample { out1 = in1 + in2 + param1 }
}
outs { out1 }
init { p = Gain() }
sample { out1 = p(0.5, 0.25) }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let proc = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("expected a proc block");
        assert_eq!(proc.ins.len(), 2);
        assert_eq!(proc.ins[0].name, "in1");
        assert_eq!(proc.ins[1].name, "in2");
        assert_eq!(proc.outs.len(), 1);
        assert_eq!(proc.outs[0].name, "out1");
        assert_eq!(proc.params.len(), 1);
        assert_eq!(proc.params[0].name, "param1");
        assert_eq!(proc.params[0].ty, None);
        assert_eq!(proc.params[0].default, None);
    }

    #[test]
    fn parses_top_level_buffers_block_and_count_shorthand() {
        let src_explicit = r#"
buffers {
  buf1
  buf2: buffer[f64]
  buf3: buffer[f32[2]]
  buf4: buffer[f32[]]
  buf5: f32
  buf6: f64[2]
}
sample { out1 = 0.0 }
"#;
        let program_explicit = parse_program(src_explicit).expect("parse_program should succeed");
        let buffers = program_explicit
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Buffers(v) => Some(v),
                _ => None,
            })
            .expect("buffers block");
        assert_eq!(buffers.len(), 6);
        assert_eq!(buffers[0].name, "buf1");
        assert!(buffers[0].ty.is_none());
        assert_eq!(buffers[1].name, "buf2");
        assert!(matches!(
            buffers[1].ty.as_ref().map(|t| &t.elem),
            Some(BufferElemType::Primitive(crate::ast::PrimitiveType::F64))
        ));
        assert!(matches!(
            buffers[2].ty.as_ref().map(|t| &t.channels),
            Some(crate::ast::BufferChannels::Static(_))
        ));
        assert!(matches!(
            buffers[3].ty.as_ref().map(|t| &t.channels),
            Some(crate::ast::BufferChannels::Dynamic)
        ));
        assert!(matches!(
            buffers[4].ty.as_ref().map(|t| &t.elem),
            Some(BufferElemType::Primitive(crate::ast::PrimitiveType::F32))
        ));
        assert!(matches!(
            buffers[4].ty.as_ref().map(|t| &t.channels),
            Some(crate::ast::BufferChannels::Mono)
        ));
        assert!(matches!(
            buffers[5].ty.as_ref().map(|t| &t.elem),
            Some(BufferElemType::Primitive(crate::ast::PrimitiveType::F64))
        ));
        assert!(matches!(
            buffers[5].ty.as_ref().map(|t| &t.channels),
            Some(crate::ast::BufferChannels::Static(_))
        ));

        let src_count = r#"
buffers 3
sample { out1 = 0.0 }
"#;
        let program_count = parse_program(src_count).expect("parse_program should succeed");
        let buffers_count = program_count
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Buffers(v) => Some(v),
                _ => None,
            })
            .expect("buffers count block");
        assert_eq!(buffers_count.len(), 3);
        assert_eq!(buffers_count[0].name, "buf1");
        assert_eq!(buffers_count[1].name, "buf2");
        assert_eq!(buffers_count[2].name, "buf3");
    }

    #[test]
    fn parses_proc_buffers_block() {
        let src = r#"
proc Delay {
  buffers {
    line: buffer[f32[2]]
  }
  outs { out1 }
  sample { out1 = 0.0 }
}
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let proc = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("proc block");
        assert_eq!(proc.buffers.len(), 1);
        assert_eq!(proc.buffers[0].name, "line");
    }

    #[test]
    fn parses_two_dim_buffer_indexing_as_internal_calls() {
        let src = r#"
buffers { buf1: buffer[f32[2]] }
sample {
  out1 = buf1[0][3]
  buf1[1][2] = 0.5
}
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(v) => Some(v),
                _ => None,
            })
            .expect("sample block");
        assert_eq!(sample.len(), 2);
        match &sample[0] {
            Stmt::Assign { expr, .. } => match expr {
                Expr::UserCall { name, args, .. } => {
                    assert_eq!(name, "__omni_buffer_read2");
                    assert_eq!(args.len(), 3);
                }
                _ => panic!("expected read2 user call"),
            },
            _ => panic!("expected assignment statement"),
        }
        match &sample[1] {
            Stmt::Expr { expr, .. } => match expr {
                Expr::UserCall { name, args, .. } => {
                    assert_eq!(name, "__omni_buffer_write2");
                    assert_eq!(args.len(), 4);
                }
                _ => panic!("expected write2 user call"),
            },
            _ => panic!("expected expression statement"),
        }
    }

    #[test]
    fn parses_def_buffer_typed_params() {
        let src = r#"
def read_mono(b: buffer[f32]) {
  return 0.0
}
def read_stereo(b: buffer[f32[2]]) {
  return 0.0
}
def read_dyn(b: buffer[f32[]]) {
  return 0.0
}
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let defs = program
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Def(d) => Some(d),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(defs.len(), 3);
        assert!(matches!(
            defs[0].params[0].ty,
            Some(crate::ast::FnParamType::Buffer(_))
        ));
        assert!(matches!(
            defs[1].params[0].ty,
            Some(crate::ast::FnParamType::Buffer(_))
        ));
        assert!(matches!(
            defs[2].params[0].ty,
            Some(crate::ast::FnParamType::Buffer(_))
        ));
    }

    #[test]
    fn rejects_generic_def_type_params() {
        let src = r#"
def pair[T, U](a: T, b: U) {
  return a
}
sample { out1 = 0.0 }
"#;
        assert!(
            parse_program(src).is_err(),
            "generic def type params should be rejected"
        );
    }

    #[test]
    fn parses_generic_proc_type_params_and_decl_types() {
        let src = r#"
proc Gain[T] {
  ins { in1: T, in2: T[2] }
  outs { out1: T }
  params { g: T = 1.0, coeffs: T[2] = [1.0, 0.5] }
  buffers { b: buffer[T], m: buffer[T[2]], d: buffer[T[]] }
  sample { out1 = in1 * g }
}
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let proc_def = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("proc block");
        assert_eq!(proc_def.name, "Gain");
        assert_eq!(proc_def.type_params, vec!["T".to_owned()]);
        assert!(matches!(
            proc_def.ins[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert!(matches!(
            proc_def.ins[1].ty,
            Some(DeclType::ArrayGeneric { ref elem, .. }) if elem == "T"
        ));
        assert!(matches!(
            proc_def.outs[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert!(matches!(
            proc_def.params[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert!(matches!(
            proc_def.params[1].ty,
            Some(DeclType::ArrayGeneric { ref elem, .. }) if elem == "T"
        ));
        assert!(matches!(
            proc_def.buffers[0].ty.as_ref().map(|t| &t.elem),
            Some(BufferElemType::Generic(ref n)) if n == "T"
        ));
        assert!(matches!(
            proc_def.buffers[1].ty.as_ref().map(|t| &t.elem),
            Some(BufferElemType::Generic(ref n)) if n == "T"
        ));
        assert!(matches!(
            proc_def.buffers[2].ty.as_ref().map(|t| &t.elem),
            Some(BufferElemType::Generic(ref n)) if n == "T"
        ));
    }

    #[test]
    fn parses_generic_proc_section_default_types_with_overrides() {
        let src = r#"
proc Fx[T] {
  ins[T] { in1, trig: bool }
  outs[T] { out1, meter: f32 }
  params[T] { gain = 1.0, mode: i32 = 0 }
  buffers[T] { line, flags: i32 }
  sample { out1 = in1 * gain; meter = f32(mode) }
}
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let proc_def = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("proc block");
        assert_eq!(proc_def.name, "Fx");
        assert_eq!(proc_def.type_params, vec!["T".to_owned()]);

        assert!(matches!(
            proc_def.ins[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert_eq!(
            proc_def.ins[1].ty,
            Some(DeclType::Scalar(PrimitiveType::Bool))
        );

        assert!(matches!(
            proc_def.outs[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert_eq!(
            proc_def.outs[1].ty,
            Some(DeclType::Scalar(PrimitiveType::F32))
        );

        assert!(matches!(
            proc_def.params[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert_eq!(
            proc_def.params[1].ty,
            Some(DeclType::Scalar(PrimitiveType::I32))
        );

        assert!(matches!(
            proc_def.buffers[0].ty.as_ref().map(|t| (&t.elem, &t.channels)),
            Some((BufferElemType::Generic(ref n), crate::ast::BufferChannels::Mono)) if n == "T"
        ));
        assert!(matches!(
            proc_def.buffers[1]
                .ty
                .as_ref()
                .map(|t| (&t.elem, &t.channels)),
            Some((
                BufferElemType::Primitive(PrimitiveType::I32),
                crate::ast::BufferChannels::Mono
            ))
        ));
    }

    #[test]
    fn parses_generic_proc_ctor_with_explicit_type_args() {
        let src = r#"
proc Gain[T] {
  outs { out1: T }
  sample { out1 = 0.0 }
}
init { p = Gain[f64]() }
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let init = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Init(v) => Some(v),
                _ => None,
            })
            .expect("init block");
        let Stmt::Assign { expr, .. } = &init[0] else {
            panic!("expected assignment");
        };
        match expr {
            Expr::UserCall {
                name,
                type_args,
                args,
            } => {
                assert_eq!(name, "Gain");
                assert_eq!(
                    type_args.as_slice(),
                    &[CallTypeArg::Primitive(PrimitiveType::F64)]
                );
                assert!(args.is_empty());
            }
            _ => panic!("expected user call"),
        }
    }

    #[test]
    fn parses_user_call_with_explicit_generic_type_args() {
        let src = r#"
sample {
  out1 = id[f64](1.0)
}
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(v) => Some(v),
                _ => None,
            })
            .expect("sample block");
        let Stmt::Assign { expr, .. } = &sample[0] else {
            panic!("expected assignment");
        };
        match expr {
            Expr::UserCall {
                name,
                type_args,
                args,
            } => {
                assert_eq!(name, "id");
                assert_eq!(
                    type_args.as_slice(),
                    &[CallTypeArg::Primitive(PrimitiveType::F64)]
                );
                assert_eq!(args.len(), 1);
            }
            _ => panic!("expected user call"),
        }
    }

    #[test]
    fn parses_user_call_with_generic_type_param_arg() {
        let src = r#"
proc Wrap[T] {
  sample {
    out1 = id[T](1.0)
  }
}
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let proc = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("proc block");
        let Stmt::Assign { expr, .. } = &proc.sample[0] else {
            panic!("expected assignment");
        };
        match expr {
            Expr::UserCall {
                name,
                type_args,
                args,
            } => {
                assert_eq!(name, "id");
                assert_eq!(
                    type_args.as_slice(),
                    &[CallTypeArg::Generic("T".to_owned())]
                );
                assert_eq!(args.len(), 1);
            }
            _ => panic!("expected user call"),
        }
    }

    #[test]
    fn parses_generic_struct_type_params_and_fields() {
        let src = r#"
struct Pair[T] { a: T, b: T }
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let st = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Struct(s) => Some(s),
                _ => None,
            })
            .expect("struct block");
        assert_eq!(st.name, "Pair");
        assert_eq!(st.type_params, vec!["T".to_owned()]);
        assert!(matches!(st.fields[0].ty, FieldType::Generic(ref n) if n == "T"));
        assert!(matches!(st.fields[1].ty, FieldType::Generic(ref n) if n == "T"));
    }

    #[test]
    fn parses_generic_struct_array_field_type() {
        let src = r#"
struct Bank[T] { taps: T[4] }
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let st = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Struct(s) => Some(s),
                _ => None,
            })
            .expect("struct block");
        match &st.fields[0].ty {
            FieldType::Data(spec) => {
                assert!(matches!(spec.elem, DataElemType::Struct(ref n) if n == "T"));
            }
            _ => panic!("expected Data field type"),
        }
    }

    #[test]
    fn parses_generic_struct_ctor_with_explicit_type_args() {
        let src = r#"
struct Pair[T] { a: T, b: T }
init {
  p = Pair[f64](f64(1.0), f64(2.0))
}
sample { out1 = 0.0 }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let init = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Init(v) => Some(v),
                _ => None,
            })
            .expect("init block");
        let Stmt::Assign { expr, .. } = &init[0] else {
            panic!("expected assignment");
        };
        match expr {
            Expr::UserCall {
                name,
                type_args,
                args,
            } => {
                assert_eq!(name, "Pair");
                assert_eq!(
                    type_args.as_slice(),
                    &[CallTypeArg::Primitive(PrimitiveType::F64)]
                );
                assert_eq!(args.len(), 2);
            }
            _ => panic!("expected user call"),
        }
    }

    #[test]
    fn parses_typed_proc_ports_and_params() {
        let src = r#"
proc Typed {
  ins { in1: i32, in2: f64 }
  outs { out1: i64 }
  params { gain: f64 = 2.0, mode: i32 = 1 }
  sample { out1 = i64(in1) + i64(mode) }
}
outs { out1 }
init { p = Typed() }
sample { out1 = p(1, 2.0) }
"#;
        let program = parse_program(src).expect("parse_program should succeed");
        let proc = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("expected a proc block");
        assert_eq!(proc.ins[0].ty, Some(DeclType::Scalar(PrimitiveType::I32)));
        assert_eq!(proc.ins[1].ty, Some(DeclType::Scalar(PrimitiveType::F64)));
        assert_eq!(proc.outs[0].ty, Some(DeclType::Scalar(PrimitiveType::I64)));
        assert_eq!(
            proc.params[0].ty,
            Some(DeclType::Scalar(PrimitiveType::F64))
        );
        assert_eq!(
            proc.params[1].ty,
            Some(DeclType::Scalar(PrimitiveType::I32))
        );
    }

    #[test]
    fn parses_proc_indexed_call_expression() {
        let src = r#"
sample {
  out1 = p(0.25)[1]
}
"#;

        let program = parse_program(src).expect("parse_program should succeed");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let Stmt::Assign { expr, .. } = &sample[0] else {
            panic!("expected assignment");
        };
        match expr {
            Expr::UserCall { name, args, .. } => {
                assert!(
                    name.starts_with(PROC_INDEX_SENTINEL_PREFIX),
                    "expected proc index sentinel call"
                );
                assert!(
                    args.iter().any(|a| {
                        a.name
                            .as_ref()
                            .map(|n| n == PROC_INDEX_SENTINEL_ARG)
                            .unwrap_or(false)
                    }),
                    "expected encoded index argument"
                );
            }
            _ => panic!("expected encoded proc indexed call expression"),
        }
    }

    #[test]
    fn parses_indentation_style_blocks() {
        let src = r#"
outs:
  out1

def add2(a, b):
  return a + b

sample:
  if (1.0 > 0.0):
    out1 = add2(0.25, 0.5)
  else:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("indentation-style program should parse");
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Def(_))));
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        assert_eq!(sample.len(), 1);
        match &sample[0] {
            Stmt::If { .. } => {}
            _ => panic!("expected if statement in sample block"),
        }
    }

    #[test]
    fn parses_indentation_if_elif_else() {
        let src = r#"
outs:
  out1
sample:
  if (x > 0.0):
    out1 = 1.0
  elif (x > -1.0):
    out1 = 0.5
  else:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("indentation elif should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let Stmt::If { else_branch, .. } = &sample[0] else {
            panic!("expected top-level if");
        };
        let Stmt::If {
            else_branch: nested_else,
            ..
        } = &else_branch[0]
        else {
            panic!("expected nested if for elif");
        };
        assert_eq!(nested_else.len(), 1);
    }

    #[test]
    fn parses_indentation_if_elif_else_without_parentheses() {
        let src = r#"
outs:
  out1
sample:
  if x > 0.0:
    out1 = 1.0
  elif x > -1.0:
    out1 = 0.5
  else:
    out1 = 0.0
"#;
        let program = parse_program(src).expect("indentation elif should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        let Stmt::If { else_branch, .. } = &sample[0] else {
            panic!("expected top-level if");
        };
        let Stmt::If {
            else_branch: nested_else,
            ..
        } = &else_branch[0]
        else {
            panic!("expected nested if for elif");
        };
        assert_eq!(nested_else.len(), 1);
    }

    #[test]
    fn parses_indentation_section_default_types() {
        let src = r#"
proc Gain[T]:
  ins[T]:
    in1
  outs[T]:
    out1
  params[T]:
    g = 1.0
  buffers[T]:
    line
  sample:
    out1 = in1 * g
sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("indentation section defaults should parse");
        let proc = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Proc(p) => Some(p),
                _ => None,
            })
            .expect("proc block");
        assert!(matches!(
            proc.ins[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert!(matches!(
            proc.outs[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert!(matches!(
            proc.params[0].ty,
            Some(DeclType::Generic(ref n)) if n == "T"
        ));
        assert!(matches!(
            proc.buffers[0].ty.as_ref().map(|t| (&t.elem, &t.channels)),
            Some((BufferElemType::Generic(ref n), crate::ast::BufferChannels::Mono)) if n == "T"
        ));
    }

    #[test]
    fn parses_mixed_indentation_and_braces() {
        let src = r#"
outs:
  out1
sample {
  if (1.0 > 0.0):
    out1 = 1.0
  else { out1 = 0.0 }
}
"#;
        let program = parse_program(src).expect("mixed syntax program should parse");
        let sample = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Sample(stmts) => Some(stmts),
                _ => None,
            })
            .expect("sample block");
        assert_eq!(sample.len(), 1);
    }

    #[test]
    fn parses_tab_indentation() {
        let src = "outs:
	out1
sample:
	out1 = 1.0
";
        let program = parse_program(src).expect("tab-indented program should parse");
        assert_eq!(program.blocks.len(), 2);
    }

    #[test]
    fn parses_namespace_blocks_and_flattens_symbol_names() {
        let src = r#"
namespace A:
  struct S:
    x: f32
  def make():
    return 1.0
  namespace B:
    def run():
      return make()
sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("namespace source should parse");
        let mut struct_names = program
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Struct(s) => Some(s.name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        struct_names.sort();
        assert_eq!(struct_names, vec!["A::S".to_owned()]);

        let mut def_names = program
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Def(d) => Some(d.name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        def_names.sort();
        assert_eq!(
            def_names,
            vec!["A::B::run".to_owned(), "A::make".to_owned()]
        );
    }

    #[test]
    fn parses_namespace_path_form() {
        let src = r#"
namespace Top::Inner:
  def run(x):
    return x
sample:
  out1 = 0.0
"#;
        let program = parse_program(src).expect("namespace path form should parse");
        let def_name = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Def(d) => Some(d.name.clone()),
                _ => None,
            })
            .expect("def");
        assert_eq!(def_name, "Top::Inner::run");
    }

    #[test]
    fn rejects_inconsistent_indentation() {
        let src = "outs:
  out1
sample:
  if (1.0 > 0.0):
    out1 = 1.0
 out1 = 0.0
";
        let result = parse_program(src);
        assert!(
            result.is_err(),
            "parser should reject inconsistent indentation"
        );
    }

    #[test]
    fn rejects_legacy_data_syntax() {
        let src = r#"
outs { out1 }
init { a = Data[4] }
sample { out1 = 0.0 }
"#;
        let result = parse_program(src);
        assert!(
            result.is_err(),
            "legacy Data[...] syntax should be rejected"
        );
        let diags = result.err().expect("parse should return diagnostics");
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("legacy Data[...] syntax")),
            "expected legacy Data syntax diagnostic"
        );
    }

    #[test]
    fn parses_block_wrapped_sample_section() {
        let src = r#"
outs { out1 }
init { x = 0.0 }
block {
  x = x + 1.0
  sample {
    out1 = x
  }
  x = x + 2.0
}
"#;
        let program = parse_program(src).expect("program with wrapped block sample should parse");
        let block_exec = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Block(exec) => Some(exec),
                _ => None,
            })
            .expect("block section");
        assert_eq!(block_exec.pre.len(), 1);
        assert_eq!(block_exec.sample.as_ref().map(Vec::len), Some(1));
        assert_eq!(block_exec.post.len(), 1);
    }

    #[test]
    fn parses_data_capacity_expression() {
        let src = r#"
outs { out1 }
struct Delay { buf: f32[SR * 2] }
init {
  d = Delay()
  b: f32[BLOCK_SIZE + 4]
}
sample {
  out1 = d.buf[0] + b[0]
}
"#;
        let program =
            parse_program(src).expect("program with Data capacity expressions should parse");
        assert!(program.blocks.iter().any(|b| matches!(b, Block::Struct(_))));
    }

    #[test]
    fn parses_typed_data_syntax_and_f32_alias() {
        let src = r#"
outs { out1 }
struct Delay {
  wide: f64[SR * 2]
  mono: f32[64]
}
init {
  a: i32[BLOCK_SIZE + 1]
  b: f32[8]
}
sample {
  out1 = 0.0
}
"#;
        let program = parse_program(src).expect("typed Data syntax should parse");

        let st = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Struct(s) => Some(s),
                _ => None,
            })
            .expect("struct block");

        match &st.fields[0].ty {
            FieldType::Data(spec) => {
                assert!(matches!(
                    spec.elem,
                    DataElemType::Primitive(crate::ast::PrimitiveType::F64)
                ));
            }
            _ => panic!("expected Data field type"),
        }
        match &st.fields[1].ty {
            FieldType::Data(spec) => {
                assert!(matches!(
                    spec.elem,
                    DataElemType::Primitive(crate::ast::PrimitiveType::F32)
                ));
            }
            _ => panic!("expected Data field type"),
        }

        let init = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Init(stmts) => Some(stmts),
                _ => None,
            })
            .expect("init block");

        match &init[0] {
            Stmt::Assign { expr, .. } => match expr {
                Expr::DataCtor { spec, .. } => {
                    assert!(matches!(
                        spec.elem,
                        DataElemType::Primitive(crate::ast::PrimitiveType::I32)
                    ));
                }
                _ => panic!("expected Data constructor"),
            },
            _ => panic!("expected assignment"),
        }
        match &init[1] {
            Stmt::Assign { expr, .. } => match expr {
                Expr::DataCtor { spec, .. } => {
                    assert!(matches!(
                        spec.elem,
                        DataElemType::Primitive(crate::ast::PrimitiveType::F32)
                    ));
                }
                _ => panic!("expected Data constructor"),
            },
            _ => panic!("expected assignment"),
        }
    }

    #[test]
    fn parses_struct_data_typed_field_in_indentation_and_brace_forms() {
        let src_indent = r#"
outs:
  out1

struct Tap:
  x: f32

struct Voice:
  taps: Tap[3]

sample:
  out1 = 0.0
"#;
        let program_indent = parse_program(src_indent).expect("indentation Struct[N] should parse");
        assert!(
            program_indent
                .blocks
                .iter()
                .any(|b| matches!(b, Block::Struct(_))),
            "expected struct blocks in indentation source"
        );

        let src_brace = r#"
outs { out1 }
struct Tap { x: f32 }
struct Voice { taps: Tap[3] }
sample { out1 = 0.0 }
"#;
        let program_brace = parse_program(src_brace).expect("brace Struct[N] should parse");
        assert!(
            program_brace
                .blocks
                .iter()
                .any(|b| matches!(b, Block::Struct(_))),
            "expected struct blocks in brace source"
        );
    }

    #[test]
    fn parses_array_type_sugar_in_struct_fields_and_init_typed_decls() {
        let src = r#"
outs { out1 }
struct Voice { x: f32 }
struct Bank {
  taps: f32[4]
  voices: Voice[2]
}
init {
  a: f32[8]
  b: Voice[2]
}
sample {
  out1 = a[0]
}
"#;
        let program = parse_program(src).expect("array type sugar should parse");

        let bank = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Struct(s) if s.name == "Bank" => Some(s),
                _ => None,
            })
            .expect("Bank struct");
        assert_eq!(bank.fields.len(), 2);
        match &bank.fields[0].ty {
            FieldType::Data(spec) => {
                assert!(matches!(
                    spec.elem,
                    DataElemType::Primitive(crate::ast::PrimitiveType::F32)
                ));
            }
            _ => panic!("expected Data field from f32[4] sugar"),
        }
        match &bank.fields[1].ty {
            FieldType::Data(spec) => {
                assert!(matches!(spec.elem, DataElemType::Struct(ref s) if s == "Voice"));
            }
            _ => panic!("expected Data field from Voice[2] sugar"),
        }

        let init = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Init(stmts) => Some(stmts),
                _ => None,
            })
            .expect("init block");
        assert_eq!(init.len(), 2);
        for stmt in init {
            match stmt {
                Stmt::Assign { decl_ty, expr, .. } => {
                    assert!(decl_ty.is_none(), "array sugar should lower to Data ctor");
                    assert!(
                        matches!(expr, Expr::DataCtor { .. }),
                        "array sugar should emit Data constructor"
                    );
                }
                _ => panic!("expected assignment in init"),
            }
        }
    }

    #[test]
    fn rejects_non_array_literal_typed_array_initializer_expression() {
        let src = r#"
outs { out1 }
init {
  a: f32[4] = 1.0
}
sample {
  out1 = 0.0
}
"#;
        let result = parse_program(src);
        assert!(
            result.is_err(),
            "typed array declaration with non-array initializer should be rejected"
        );
    }

    #[test]
    fn parses_typed_array_initializer_expression() {
        let src = r#"
outs { out1 }
init {
  a: f32[2] = [1.0, 2.0]
}
sample {
  out1 = a[0]
}
"#;
        let program = parse_program(src).expect("typed array initializer should parse");
        let init = program
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Init(stmts) => Some(stmts),
                _ => None,
            })
            .expect("init block");
        let Stmt::Assign { expr, .. } = &init[0] else {
            panic!("expected assignment");
        };
        match expr {
            Expr::DataCtor {
                init: Some(values), ..
            } => assert_eq!(values.len(), 2),
            _ => panic!("expected DataCtor with array initializer"),
        }
    }

    #[test]
    fn parse_program_file_resolves_include_and_import() {
        let dir = mk_temp_dir("include_import");
        let main = dir.join("main.omni");
        let filter = dir.join("filter.omni");
        let shared = dir.join("shared.omni");

        write_file(
            &shared,
            r#"
def shared_gain(x) {
  return x * 0.5
}
"#,
        );
        write_file(
            &filter,
            r#"
include "./shared.omni"
namespace DSP:
  struct OnePole:
    z: f32
  def process(x):
    return shared_gain(x)
"#,
        );
        write_file(
            &main,
            r#"
import filter
outs { out1 }
sample {
  s = DSP::OnePole()
  out1 = DSP::process(2.0) + s.z
}
"#,
        );

        let program = parse_program_file(&main).expect("parse_program_file should succeed");
        assert!(
            program.blocks.iter().any(|b| matches!(b, Block::Struct(_))),
            "expected imported struct to be present"
        );
        assert!(
            program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
            "expected imported defs to be present"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_program_file_rejects_import_module_with_runtime_blocks() {
        let dir = mk_temp_dir("import_runtime_reject");
        let main = dir.join("main.omni");
        let lib = dir.join("lib.omni");

        write_file(
            &lib,
            r#"
outs { out1 }
sample { out1 = 0.0 }
"#,
        );
        write_file(
            &main,
            r#"
import lib
outs { out1 }
sample { out1 = 0.0 }
"#,
        );

        let result = parse_program_file(&main);
        assert!(
            result.is_err(),
            "imported module with runtime blocks should be rejected"
        );
        let errors = result.expect_err("expected parse error");
        assert!(!errors.is_empty(), "expected at least one diagnostic");
        let first = &errors[0];
        let canonical_lib = fs::canonicalize(&lib).expect("canonical lib path");
        let expected_file = canonical_lib
            .to_string_lossy()
            .to_string()
            .trim_start_matches(r"\\?\")
            .to_owned();
        assert_eq!(
            first.file.as_deref(),
            Some(expected_file.as_str()),
            "expected leaf diagnostic file to point at imported module"
        );
        assert!(
            first.trace.iter().any(|t| t.contains("import 'lib'")),
            "expected trace to include import site"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_program_file_rejects_import_include_same_file_mix() {
        let dir = mk_temp_dir("import_include_mix");
        let main = dir.join("main.omni");
        let dep = dir.join("dep.omni");

        write_file(
            &dep,
            r#"
def f(x) { return x }
"#,
        );
        write_file(
            &main,
            r#"
import dep
include "./dep.omni"
outs { out1 }
sample { out1 = f(1.0) }
"#,
        );

        let result = parse_program_file(&main);
        assert!(
            result.is_err(),
            "same file cannot be both imported and included"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_program_in_memory_supports_builtin_std_imports() {
        let src = r#"
import std/math
outs { out1 }
sample { out1 = clamp(2.0, 0.0, 1.0) }
"#;
        let program = parse_program(src).expect("in-memory std import should parse");
        assert!(
            program.blocks.iter().any(|b| matches!(b, Block::Def(_))),
            "expected std module declarations to be imported"
        );
    }

    #[test]
    fn parse_program_in_memory_rejects_non_std_imports() {
        let src = r#"
import my_lib
outs { out1 }
sample { out1 = 0.0 }
"#;
        let result = parse_program(src);
        assert!(
            result.is_err(),
            "in-memory parser should reject non-std imports without file context"
        );
    }
}
