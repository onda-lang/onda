use super::loading_support::{
    annotate_diagnostics_with_file, builtin_std_module_source, display_path,
    is_builtin_std_module_path, resolve_import_path, resolve_include_path, split_top_level_items,
    validate_file_mode_transition, FileLoadMode, TopLevelItem,
};
use super::preprocess::preprocess_indentation_blocks;
use super::*;
use std::path::Component;

mod namespaces;
pub use namespaces::parse_namespace_ref_text_ast;
use namespaces::*;

#[derive(Default)]
struct LoadState {
    import_once: HashSet<PathBuf>,
    import_once_builtin: HashSet<String>,
    file_modes: HashMap<PathBuf, FileLoadMode>,
    stack: Vec<PathBuf>,
    builtin_stack: Vec<String>,
    top_level_const_names: HashSet<String>,
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
        Block::Namespace(ns) => Some(ns.name.as_str()),
        Block::NamespaceAlias(alias) => Some(alias.name.as_str()),
        Block::Use(_) => None,
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
                let mut imported = load_builtin_module_blocks(
                    &module,
                    true,
                    &mut state,
                    std::slice::from_ref(&trace),
                )
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
    let loader = SourceLoader::filesystem(overlays);
    parse_program_file_with_loader(path, &loader)
}

/// Parses an in-memory source tree without consulting the host filesystem.
///
/// `root`, `path`, and every source key belong to the same lexical namespace;
/// they need not follow the host target's notion of an absolute path. Every
/// include/import is confined to `root` and must resolve to an entry in
/// `sources` (apart from embedded standard-library modules).
pub fn parse_program_file_from_virtual_sources(
    root: &Path,
    path: &Path,
    sources: &HashMap<PathBuf, String>,
) -> Result<Program, Vec<Diagnostic>> {
    let loader = SourceLoader::virtual_tree(root, sources)
        .map_err(|message| vec![Diagnostic::syntax(message, 0, 0)])?;
    parse_program_file_with_loader(path, &loader)
}

fn parse_program_file_with_loader(
    path: &Path,
    loader: &SourceLoader,
) -> Result<Program, Vec<Diagnostic>> {
    let canonical = loader.resolve(path).map_err(|err| {
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
    let blocks = load_program_blocks_from_file(&canonical, false, &mut state, &[], loader)?;
    Ok(Program { blocks })
}

pub fn parse_stdlib_module(module: &str) -> Result<Program, Vec<Diagnostic>> {
    let mut state = LoadState::default();
    let blocks = load_builtin_module_blocks(module, false, &mut state, &[])?;
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
        let mut parsed = OndaParser::parse(Rule::program, preprocessed)
            .map_err(|err| vec![diag_from_pest_error(err)])?;
        let program_pair = parsed
            .next()
            .ok_or_else(|| vec![Diagnostic::syntax("empty parse result", 1, 1)])?;

        let mut blocks = Vec::new();
        let mut top_level_const_names = state.top_level_const_names.clone();
        for pair in program_pair.into_inner() {
            match pair.as_rule() {
                Rule::ins_block => blocks.push(Block::Ins(parse_port_block(pair)?)),
                Rule::outs_block => blocks.push(Block::Outs(parse_port_block(pair)?)),
                Rule::kouts_block => blocks.push(Block::KOuts(parse_port_block(pair)?)),
                Rule::params_block | Rule::kins_block => {
                    blocks.push(Block::Params(parse_params_block(pair)?))
                }
                Rule::const_block => {
                    let decl = parse_const_decl(pair)?;
                    if !top_level_const_names.insert(decl.name.clone()) {
                        return Err(vec![Diagnostic::semantic_span(
                            format!("duplicate top-level constant '{}'", decl.name),
                            decl.loc.as_ref(),
                        )]);
                    }
                    state.top_level_const_names = top_level_const_names.clone();
                    blocks.push(Block::Const(decl));
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
                Rule::def_block => {
                    let def = parse_def_block(pair)?;
                    blocks.push(Block::Def(def));
                }
                Rule::namespace_block => {
                    blocks.push(Block::Namespace(parse_namespace_decl_ast(pair)?));
                }
                Rule::namespace_alias_decl => {
                    blocks.push(Block::NamespaceAlias(parse_namespace_alias_decl_ast(pair)?));
                }
                Rule::use_decl => {
                    blocks.push(Block::Use(parse_use_decl_ast(pair)?));
                }
                Rule::init_block => blocks.push(Block::Init(parse_exec_block(pair)?)),
                Rule::block_exec_block => blocks.push(Block::Block(parse_block_exec_block(pair)?)),
                Rule::sample_block => blocks.push(Block::Sample(parse_sample_block(pair)?)),
                Rule::graph_block => blocks.push(Block::Graph(parse_graph_block(pair)?)),
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
    loader: &SourceLoader,
) -> Result<Vec<Block>, Vec<Diagnostic>> {
    let canonical = loader.resolve(file_path).map_err(|err| {
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
        let source = loader.read(&canonical).map_err(|err| {
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
                    let include_path =
                        loader.resolve_include(&canonical, &path).map_err(|msg| {
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
                        loader,
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
                    let import_path =
                        loader.resolve_import(&canonical, &module).map_err(|msg| {
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
                        loader,
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
                    Block::Const(_)
                        | Block::Struct(_)
                        | Block::Def(_)
                        | Block::Proc(_)
                        | Block::Namespace(_)
                        | Block::NamespaceAlias(_)
                        | Block::Use(_)
                ) {
                    return Err(annotate_diagnostics_with_file(
                        vec![Diagnostic::semantic_span(
                            format!(
                                "imported file '{}' can only contain const/struct/def/proc/namespace/use declarations",
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

fn normalize_overlay_paths(overlays: &HashMap<PathBuf, String>) -> HashMap<PathBuf, String> {
    let mut normalized = HashMap::with_capacity(overlays.len());
    for (path, source) in overlays {
        let key = normalize_overlay_path(path);
        normalized.insert(key, source.clone());
    }
    normalized
}

#[derive(Debug, Clone)]
enum SourcePolicy {
    Filesystem,
    Virtual { root: PathBuf },
}

#[derive(Debug, Clone)]
struct SourceLoader {
    overlays: HashMap<PathBuf, String>,
    policy: SourcePolicy,
}

impl SourceLoader {
    fn filesystem(overlays: &HashMap<PathBuf, String>) -> Self {
        Self {
            overlays: normalize_overlay_paths(overlays),
            policy: SourcePolicy::Filesystem,
        }
    }

    fn virtual_tree(root: &Path, sources: &HashMap<PathBuf, String>) -> Result<Self, String> {
        let root = normalize_path_lexically(root);
        if root.as_os_str().is_empty() {
            return Err("virtual source root must identify a lexical namespace".to_owned());
        }
        let mut overlays = HashMap::with_capacity(sources.len());
        for (path, source) in sources {
            let path = normalize_path_lexically(path);
            ensure_virtual_root(&root, &path)?;
            if overlays.insert(path.clone(), source.clone()).is_some() {
                return Err(format!(
                    "duplicate normalized virtual source path '{}'",
                    path.display()
                ));
            }
        }
        Ok(Self {
            overlays,
            policy: SourcePolicy::Virtual { root },
        })
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf, String> {
        match &self.policy {
            SourcePolicy::Filesystem => resolve_file_or_overlay_path(path, &self.overlays)
                .map_err(|error| error.to_string()),
            SourcePolicy::Virtual { root } => {
                let path = normalize_path_lexically(path);
                ensure_virtual_root(root, &path)?;
                if self.overlays.contains_key(&path) {
                    Ok(path)
                } else {
                    Err(format!(
                        "virtual source '{}' is not present in the source map",
                        path.display()
                    ))
                }
            }
        }
    }

    fn read(&self, path: &Path) -> Result<String, String> {
        if let Some(source) = self.overlays.get(path) {
            return Ok(source.clone());
        }
        match self.policy {
            SourcePolicy::Filesystem => fs::read_to_string(path).map_err(|error| error.to_string()),
            SourcePolicy::Virtual { .. } => Err(format!(
                "virtual source '{}' is not present in the source map",
                path.display()
            )),
        }
    }

    fn resolve_include(&self, current_file: &Path, include_path: &str) -> Result<PathBuf, String> {
        match &self.policy {
            SourcePolicy::Filesystem => {
                resolve_include_path_with_overlays(current_file, include_path, &self.overlays)
            }
            SourcePolicy::Virtual { root } => {
                if is_portable_absolute_virtual_path(include_path) {
                    return Err(format!(
                        "virtual source path '{include_path}' escapes project root '{}'",
                        root.display()
                    ));
                }
                let include = Path::new(include_path);
                let unresolved = current_file.parent().unwrap_or(root).join(include);
                let resolved = normalize_path_lexically(&unresolved);
                ensure_virtual_root(root, &resolved)?;
                self.resolve(&resolved)
            }
        }
    }

    fn resolve_import(&self, current_file: &Path, module_path: &str) -> Result<PathBuf, String> {
        match &self.policy {
            SourcePolicy::Filesystem => {
                resolve_import_path_with_overlays(current_file, module_path, &self.overlays)
            }
            SourcePolicy::Virtual { root } => {
                if is_portable_absolute_virtual_path(module_path) {
                    return Err(format!(
                        "virtual source path '{module_path}' escapes project root '{}'",
                        root.display()
                    ));
                }
                let module = Path::new(module_path);
                let base = current_file.parent().unwrap_or(root).join(module);
                for extension in ["onda", "on"] {
                    let candidate = normalize_path_lexically(&base.with_extension(extension));
                    ensure_virtual_root(root, &candidate)?;
                    if self.overlays.contains_key(&candidate) {
                        return Ok(candidate);
                    }
                }
                Err(format!(
                    "failed to resolve imported module '{module_path}' from '{}'",
                    display_path(current_file)
                ))
            }
        }
    }
}

fn is_portable_absolute_virtual_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with('\\')
        || matches!(bytes, [drive, b':', ..] if drive.is_ascii_alphabetic())
}

fn ensure_virtual_root(root: &Path, path: &Path) -> Result<(), String> {
    if path.starts_with(root) {
        Ok(())
    } else {
        Err(format!(
            "virtual source path '{}' escapes project root '{}'",
            path.display(),
            root.display()
        ))
    }
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn normalize_overlay_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };
        let mut normalized = PathBuf::new();
        for component in absolute.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                    normalized.push(component.as_os_str());
                }
            }
        }
        normalized
    })
}

fn resolve_file_or_overlay_path(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> Result<PathBuf, std::io::Error> {
    match fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(err) => {
            let normalized = normalize_overlay_path(path);
            if overlays.contains_key(&normalized) {
                Ok(normalized)
            } else {
                Err(err)
            }
        }
    }
}

fn resolve_include_path_with_overlays(
    current_file: &Path,
    include_path: &str,
    overlays: &HashMap<PathBuf, String>,
) -> Result<PathBuf, String> {
    resolve_include_path(current_file, include_path).or_else(|message| {
        let include = PathBuf::from(include_path);
        let unresolved = if include.is_absolute() {
            include
        } else {
            current_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(include)
        };
        let normalized = normalize_overlay_path(&unresolved);
        if overlays.contains_key(&normalized) {
            Ok(normalized)
        } else {
            Err(message)
        }
    })
}

fn resolve_import_path_with_overlays(
    current_file: &Path,
    module_path: &str,
    overlays: &HashMap<PathBuf, String>,
) -> Result<PathBuf, String> {
    resolve_import_path(current_file, module_path).or_else(|message| {
        let base = if Path::new(module_path).is_absolute() {
            PathBuf::from(module_path)
        } else {
            current_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(module_path)
        };
        for ext in ["onda", "on"] {
            let candidate = normalize_overlay_path(&base.with_extension(ext));
            if overlays.contains_key(&candidate) {
                return Ok(candidate);
            }
        }
        Err(message)
    })
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
                    Block::Const(_)
                        | Block::Struct(_)
                        | Block::Def(_)
                        | Block::Proc(_)
                        | Block::Namespace(_)
                        | Block::NamespaceAlias(_)
                        | Block::Use(_)
                ) {
                    return Err(annotate_diagnostics_with_file(
                        vec![Diagnostic::semantic_span(
                            format!(
                                "imported built-in std module '{module}' can only contain const/struct/def/proc/namespace/use declarations"
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
