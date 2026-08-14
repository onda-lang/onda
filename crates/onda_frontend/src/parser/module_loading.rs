use super::loading_support::{
    annotate_diagnostics_with_file, builtin_std_module_source, display_path,
    is_builtin_std_module_path, split_top_level_items, validate_file_mode_transition, FileLoadMode,
    TopLevelItem, ONDA_SOURCE_EXTENSIONS,
};
use super::preprocess::{preprocess_indentation_blocks, split_comment};
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
    source_files: Vec<PathBuf>,
    source_file_set: HashSet<PathBuf>,
    unresolved_source_files: Vec<PathBuf>,
    unresolved_source_file_set: HashSet<PathBuf>,
    unresolved_resolutions: Vec<UnresolvedSourceResolution>,
    unresolved_resolution_set: HashSet<UnresolvedSourceResolution>,
    source_documents: Vec<SourceDocument>,
    source_document_set: HashSet<PathBuf>,
    resolutions: Vec<SourceResolution>,
    resolution_set: HashSet<SourceResolution>,
    stack: Vec<PathBuf>,
    builtin_stack: Vec<String>,
    top_level_const_names: HashSet<String>,
}

impl LoadState {
    fn record_source_file(&mut self, path: &Path) {
        let path = path.to_path_buf();
        if self.source_file_set.insert(path.clone()) {
            self.source_files.push(path);
        }
    }

    fn record_unresolved_resolution(&mut self, resolution: UnresolvedSourceResolution) {
        for candidate in &resolution.candidates {
            if self.unresolved_source_file_set.insert(candidate.clone()) {
                self.unresolved_source_files.push(candidate.clone());
            }
        }
        if self.unresolved_resolution_set.insert(resolution.clone()) {
            self.unresolved_resolutions.push(resolution);
        }
    }

    fn record_source_document(&mut self, path: &Path, contents: String) {
        let path = path.to_path_buf();
        if self.source_document_set.insert(path.clone()) {
            self.source_documents
                .push(SourceDocument { path, contents });
        }
    }

    fn record_resolution(&mut self, resolution: SourceResolution) {
        if self.resolution_set.insert(resolution.clone()) {
            self.resolutions.push(resolution);
        }
    }

    fn source_manifest(&self) -> SourceManifest {
        SourceManifest {
            files: self.source_files.clone(),
            unresolved_files: self.unresolved_source_files.clone(),
            unresolved_resolutions: self.unresolved_resolutions.clone().into_boxed_slice(),
            documents: self.source_documents.clone().into_boxed_slice(),
            resolutions: self.resolutions.clone().into_boxed_slice(),
        }
    }
}

/// One exact non-standard-library source read during project loading.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SourceDocument {
    pub path: PathBuf,
    pub contents: String,
}

/// The directive kind which caused one source to resolve another.
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum SourceReferenceKind {
    Include,
    Import,
}

/// One successful non-standard-library include/import resolution.
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct SourceResolution {
    pub source: PathBuf,
    pub kind: SourceReferenceKind,
    pub specifier: String,
    pub target: PathBuf,
}

/// One syntax-aware replacement for a non-standard-library source reference.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SourceReferenceRewrite {
    pub kind: SourceReferenceKind,
    pub specifier: String,
    pub replacement: String,
}

/// One non-standard-library include/import which did not resolve.
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct UnresolvedSourceResolution {
    pub source: PathBuf,
    pub kind: SourceReferenceKind,
    pub specifier: String,
    pub candidates: Vec<PathBuf>,
}

/// The non-standard-library source files reached while loading one program.
///
/// Files are unique and ordered deterministically: the entry is first, followed
/// by transitive includes/imports in source discovery order.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct SourceManifest {
    pub files: Vec<PathBuf>,
    /// Non-standard-library paths which were referenced but did not resolve.
    ///
    /// These paths do not contribute to a successful compilation. They are
    /// exposed separately so filesystem hosts can watch for their creation
    /// while a project is temporarily incomplete.
    pub unresolved_files: Vec<PathBuf>,
    /// Unresolved non-standard-library references, in source discovery order.
    ///
    /// `unresolved_files` is the unique, flattened projection of the candidate
    /// paths in this collection for hosts which only need a watch set.
    pub unresolved_resolutions: Box<[UnresolvedSourceResolution]>,
    /// Exact UTF-8 contents read for resolved sources, in discovery order.
    ///
    /// On a read failure this can be a strict subset of `files`.
    pub documents: Box<[SourceDocument]>,
    /// Successful non-standard-library resolutions, in source discovery order.
    pub resolutions: Box<[SourceResolution]>,
}

#[derive(Debug, Clone)]
pub struct LoadedProgram {
    pub program: Program,
    pub sources: SourceManifest,
}

#[derive(Debug, Clone)]
pub struct LoadError {
    pub diagnostics: Vec<Diagnostic>,
    pub sources: SourceManifest,
}

pub type LoadResult = Result<LoadedProgram, LoadError>;

/// Rewrites top-level non-standard-library include/import specifiers while
/// preserving every other source byte.
pub fn rewrite_source_references(
    path: &Path,
    source: &str,
    rewrites: &[SourceReferenceRewrite],
) -> Result<String, Vec<Diagnostic>> {
    #[derive(Debug, Clone, Eq, Hash, PartialEq)]
    struct RewriteKey {
        kind: SourceReferenceKind,
        specifier: String,
    }

    fn line_start(source: &str, line: usize) -> Option<usize> {
        if line == 0 {
            return None;
        }
        if line == 1 {
            return Some(0);
        }
        let mut current = 1usize;
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                current += 1;
                if current == line {
                    return Some(index + 1);
                }
            }
        }
        None
    }

    fn specifier_range(
        line: &str,
        kind: SourceReferenceKind,
        expected: &str,
    ) -> Option<std::ops::Range<usize>> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (code, _) = split_comment(line);
        let leading = code.len().saturating_sub(code.trim_start().len());
        let trimmed = code.trim();
        match kind {
            SourceReferenceKind::Include => {
                let rest = trimmed.strip_prefix("include")?.trim();
                if !rest.starts_with('"') || !rest.ends_with('"') || rest.len() < 2 {
                    return None;
                }
                let value = &rest[1..rest.len() - 1];
                if value != expected {
                    return None;
                }
                let quote = trimmed.find('"')?;
                Some((leading + quote + 1)..(leading + quote + 1 + value.len()))
            }
            SourceReferenceKind::Import => {
                let rest = trimmed.strip_prefix("import")?;
                let whitespace = rest.len().saturating_sub(rest.trim_start().len());
                let value = rest.trim();
                if value != expected {
                    return None;
                }
                let start = leading + "import".len() + whitespace;
                Some(start..(start + value.len()))
            }
        }
    }

    let mut replacements = HashMap::<RewriteKey, &str>::with_capacity(rewrites.len());
    for rewrite in rewrites {
        if rewrite.specifier.is_empty() || rewrite.replacement.is_empty() {
            return Err(vec![Diagnostic::syntax(
                "source reference rewrite paths must not be empty",
                0,
                0,
            )]);
        }
        let key = RewriteKey {
            kind: rewrite.kind,
            specifier: rewrite.specifier.clone(),
        };
        if let Some(previous) = replacements.insert(key, &rewrite.replacement) {
            if previous != rewrite.replacement {
                return Err(vec![Diagnostic::syntax(
                    "source reference rewrites contain conflicting replacements",
                    0,
                    0,
                )]);
            }
        }
    }

    let (preprocessed, line_map) = preprocess_indentation_blocks(source)
        .map_err(|diagnostics| annotate_diagnostics_with_file(diagnostics, path, 0))?;
    let items = split_top_level_items(&preprocessed, &line_map, path)?;
    let mut used = HashSet::<RewriteKey>::new();
    let mut edits = Vec::<(std::ops::Range<usize>, &str)>::new();
    for item in items {
        let (kind, specifier, line) = match item {
            TopLevelItem::Include { path, line } => (SourceReferenceKind::Include, path, line),
            TopLevelItem::Import { module, line } if !is_builtin_std_module_path(&module) => {
                (SourceReferenceKind::Import, module, line)
            }
            _ => continue,
        };
        let key = RewriteKey { kind, specifier };
        let Some(replacement) = replacements.get(&key).copied() else {
            return Err(annotate_diagnostics_with_file(
                vec![Diagnostic::syntax(
                    format!(
                        "no replacement was provided for {} '{}'",
                        match kind {
                            SourceReferenceKind::Include => "include",
                            SourceReferenceKind::Import => "import",
                        },
                        key.specifier
                    ),
                    line,
                    1,
                )],
                path,
                0,
            ));
        };
        let Some(start) = line_start(source, line) else {
            return Err(annotate_diagnostics_with_file(
                vec![Diagnostic::internal("source reference line is invalid")],
                path,
                0,
            ));
        };
        let line_text = &source[start..]
            .split_inclusive('\n')
            .next()
            .unwrap_or_default();
        let Some(range) = specifier_range(line_text, kind, &key.specifier) else {
            return Err(annotate_diagnostics_with_file(
                vec![Diagnostic::internal(
                    "source reference could not be located in the original source",
                )],
                path,
                0,
            ));
        };
        edits.push(((start + range.start)..(start + range.end), replacement));
        used.insert(key);
    }

    if used.len() != replacements.len() {
        return Err(vec![Diagnostic::syntax(
            "source reference rewrites contain a reference not present in the source",
            0,
            0,
        )]);
    }

    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0.start));
    let mut rewritten = source.to_owned();
    for (range, replacement) in edits {
        rewritten.replace_range(range, replacement);
    }
    Ok(rewritten)
}

#[derive(Debug)]
struct SourceResolutionError {
    message: String,
    candidates: Vec<PathBuf>,
}

impl SourceResolutionError {
    fn without_candidates(message: String) -> Self {
        Self {
            message,
            candidates: Vec::new(),
        }
    }
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
    load_program_file(path)
        .map(|loaded| loaded.program)
        .map_err(|error| error.diagnostics)
}

pub fn parse_program_file_with_overlays(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> Result<Program, Vec<Diagnostic>> {
    load_program_file_with_overlays(path, overlays)
        .map(|loaded| loaded.program)
        .map_err(|error| error.diagnostics)
}

pub fn load_program_file(path: &Path) -> LoadResult {
    load_program_file_with_overlays(path, &HashMap::new())
}

pub fn load_program_file_with_overlays(
    path: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> LoadResult {
    let loader = SourceLoader::filesystem(overlays);
    load_program_file_with_loader(path, &loader)
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
    load_program_file_from_virtual_sources(root, path, sources)
        .map(|loaded| loaded.program)
        .map_err(|error| error.diagnostics)
}

pub fn load_program_file_from_virtual_sources(
    root: &Path,
    path: &Path,
    sources: &HashMap<PathBuf, String>,
) -> LoadResult {
    let loader = SourceLoader::virtual_tree(root, sources).map_err(|message| LoadError {
        diagnostics: vec![Diagnostic::syntax(message, 0, 0)],
        sources: SourceManifest::default(),
    })?;
    load_program_file_with_loader(path, &loader)
}

/// Parses an exact previously resolved source graph without consulting the
/// host filesystem or reinterpreting source identifiers as local paths.
pub fn load_program_file_from_snapshot(
    path: &Path,
    sources: &HashMap<PathBuf, String>,
    resolutions: &[SourceResolution],
) -> LoadResult {
    let loader = SourceLoader::snapshot(sources, resolutions).map_err(|message| LoadError {
        diagnostics: vec![Diagnostic::syntax(message, 0, 0)],
        sources: SourceManifest::default(),
    })?;
    load_program_file_with_loader(path, &loader)
}

fn load_program_file_with_loader(path: &Path, loader: &SourceLoader) -> LoadResult {
    let canonical = loader.resolve(path).map_err(|err| LoadError {
        diagnostics: vec![Diagnostic::syntax(
            format!("failed to resolve '{}': {err}", path.display()),
            0,
            0,
        )],
        sources: SourceManifest::default(),
    })?;
    let mut state = LoadState::default();
    state
        .file_modes
        .insert(canonical.clone(), FileLoadMode::Entry);
    match load_program_blocks_from_file(&canonical, false, &mut state, &[], loader) {
        Ok(blocks) => Ok(LoadedProgram {
            program: Program { blocks },
            sources: state.source_manifest(),
        }),
        Err(diagnostics) => Err(LoadError {
            diagnostics,
            sources: state.source_manifest(),
        }),
    }
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
    state.record_source_file(&canonical);
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
        state.record_source_document(&canonical, source.clone());
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
                    let include_path = match loader.resolve_include(&canonical, &path) {
                        Ok(path) => path,
                        Err(error) => {
                            state.record_unresolved_resolution(UnresolvedSourceResolution {
                                source: canonical.clone(),
                                kind: SourceReferenceKind::Include,
                                specifier: path.clone(),
                                candidates: error.candidates,
                            });
                            return Err(annotate_diagnostics_with_file(
                                vec![Diagnostic::syntax(error.message, line, 1)],
                                &canonical,
                                0,
                            ));
                        }
                    };
                    state.record_resolution(SourceResolution {
                        source: canonical.clone(),
                        kind: SourceReferenceKind::Include,
                        specifier: path.clone(),
                        target: include_path.clone(),
                    });
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
                    let import_path = match loader.resolve_import(&canonical, &module) {
                        Ok(path) => path,
                        Err(error) => {
                            state.record_unresolved_resolution(UnresolvedSourceResolution {
                                source: canonical.clone(),
                                kind: SourceReferenceKind::Import,
                                specifier: module.clone(),
                                candidates: error.candidates,
                            });
                            return Err(annotate_diagnostics_with_file(
                                vec![Diagnostic::syntax(error.message, line, 1)],
                                &canonical,
                                0,
                            ));
                        }
                    };
                    state.record_resolution(SourceResolution {
                        source: canonical.clone(),
                        kind: SourceReferenceKind::Import,
                        specifier: module.clone(),
                        target: import_path.clone(),
                    });
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
    Virtual {
        root: PathBuf,
    },
    Snapshot {
        resolutions: HashMap<SourceResolutionKey, PathBuf>,
    },
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct SourceResolutionKey {
    source: PathBuf,
    kind: SourceReferenceKind,
    specifier: String,
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
            let path = normalize_virtual_path(&root, path)?;
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

    fn snapshot(
        sources: &HashMap<PathBuf, String>,
        resolutions: &[SourceResolution],
    ) -> Result<Self, String> {
        let mut resolved = HashMap::with_capacity(resolutions.len());
        for resolution in resolutions {
            if !sources.contains_key(&resolution.source) {
                return Err(format!(
                    "snapshot resolution source '{}' is not present",
                    resolution.source.display()
                ));
            }
            if !sources.contains_key(&resolution.target) {
                return Err(format!(
                    "snapshot resolution target '{}' is not present",
                    resolution.target.display()
                ));
            }
            let key = SourceResolutionKey {
                source: resolution.source.clone(),
                kind: resolution.kind,
                specifier: resolution.specifier.clone(),
            };
            if let Some(previous) = resolved.insert(key, resolution.target.clone()) {
                if previous != resolution.target {
                    return Err(format!(
                        "snapshot contains conflicting resolutions from '{}'",
                        resolution.source.display()
                    ));
                }
            }
        }
        Ok(Self {
            overlays: sources.clone(),
            policy: SourcePolicy::Snapshot {
                resolutions: resolved,
            },
        })
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf, String> {
        if matches!(&self.policy, SourcePolicy::Filesystem) {
            ensure_no_symlink_components(path).map_err(|error| error.to_string())?;
        }
        self.resolve_validated(path)
    }

    fn resolve_validated(&self, path: &Path) -> Result<PathBuf, String> {
        match &self.policy {
            SourcePolicy::Filesystem => resolve_file_or_overlay_path(path, &self.overlays)
                .map_err(|error| error.to_string()),
            SourcePolicy::Virtual { root } => {
                let path = normalize_virtual_path(root, path)?;
                if self.overlays.contains_key(&path) {
                    Ok(path)
                } else {
                    Err(format!(
                        "virtual source '{}' is not present in the source map",
                        path.display()
                    ))
                }
            }
            SourcePolicy::Snapshot { .. } => {
                if self.overlays.contains_key(path) {
                    Ok(path.to_path_buf())
                } else {
                    Err(format!(
                        "snapshot source '{}' is not present",
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
            SourcePolicy::Virtual { .. } | SourcePolicy::Snapshot { .. } => Err(format!(
                "virtual source '{}' is not present in the source map",
                path.display()
            )),
        }
    }

    fn resolve_include(
        &self,
        current_file: &Path,
        include_path: &str,
    ) -> Result<PathBuf, SourceResolutionError> {
        if matches!(&self.policy, SourcePolicy::Snapshot { .. }) {
            return self.resolve_snapshot_reference(
                current_file,
                SourceReferenceKind::Include,
                include_path,
            );
        }
        if let SourcePolicy::Virtual { root } = &self.policy {
            if is_portable_absolute_virtual_path(include_path) {
                return Err(SourceResolutionError::without_candidates(format!(
                    "virtual source path '{include_path}' escapes project root '{}'",
                    root.display()
                )));
            }
        }
        let include = Path::new(include_path);
        let raw_candidate = if include.is_absolute() {
            include.to_path_buf()
        } else {
            current_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(include)
        };
        let candidate = self
            .normalize_candidate(raw_candidate.clone())
            .map_err(SourceResolutionError::without_candidates)?;
        if matches!(self.policy, SourcePolicy::Filesystem) {
            ensure_no_symlink_components(&raw_candidate).map_err(|error| {
                SourceResolutionError {
                    message: format!(
                        "failed to resolve include '{}': {error}",
                        candidate.display()
                    ),
                    candidates: vec![candidate.clone()],
                }
            })?;
        }
        self.resolve(&candidate).map_err(|message| {
            let message = match self.policy {
                SourcePolicy::Filesystem => {
                    format!(
                        "failed to resolve include '{}': {message}",
                        candidate.display()
                    )
                }
                SourcePolicy::Virtual { .. } | SourcePolicy::Snapshot { .. } => message,
            };
            SourceResolutionError {
                message,
                candidates: vec![candidate],
            }
        })
    }

    fn resolve_import(
        &self,
        current_file: &Path,
        module_path: &str,
    ) -> Result<PathBuf, SourceResolutionError> {
        if matches!(&self.policy, SourcePolicy::Snapshot { .. }) {
            return self.resolve_snapshot_reference(
                current_file,
                SourceReferenceKind::Import,
                module_path,
            );
        }
        if let SourcePolicy::Virtual { root } = &self.policy {
            if is_portable_absolute_virtual_path(module_path) {
                return Err(SourceResolutionError::without_candidates(format!(
                    "virtual source path '{module_path}' escapes project root '{}'",
                    root.display()
                )));
            }
        }
        let module = Path::new(module_path);
        let base = if module.is_absolute() {
            module.to_path_buf()
        } else {
            current_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(module)
        };
        let raw_candidates = ONDA_SOURCE_EXTENSIONS
            .iter()
            .copied()
            .map(|extension| base.with_extension(extension))
            .collect::<Vec<_>>();
        let candidates = raw_candidates
            .iter()
            .cloned()
            .map(|candidate| self.normalize_candidate(candidate))
            .collect::<Result<Vec<_>, _>>()
            .map_err(SourceResolutionError::without_candidates)?;
        for (raw_candidate, candidate) in raw_candidates.iter().zip(&candidates) {
            if matches!(self.policy, SourcePolicy::Filesystem) {
                ensure_no_symlink_components(raw_candidate).map_err(|error| {
                    SourceResolutionError {
                        message: format!(
                            "failed to resolve import '{}': {error}",
                            candidate.display()
                        ),
                        candidates: candidates.clone(),
                    }
                })?;
            }
            if let Ok(resolved) = self.resolve_validated(candidate) {
                return Ok(resolved);
            }
        }
        let message = match self.policy {
            SourcePolicy::Filesystem => format!(
                "failed to resolve import '{}.{{{}}}'",
                base.display(),
                ONDA_SOURCE_EXTENSIONS.join(",")
            ),
            SourcePolicy::Virtual { .. } | SourcePolicy::Snapshot { .. } => format!(
                "failed to resolve imported module '{module_path}' from '{}'",
                display_path(current_file)
            ),
        };
        Err(SourceResolutionError {
            message,
            candidates,
        })
    }

    fn normalize_candidate(&self, path: PathBuf) -> Result<PathBuf, String> {
        match &self.policy {
            SourcePolicy::Filesystem => {
                let absolute = if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .map_err(|error| error.to_string())?
                        .join(path)
                };
                Ok(normalize_path_lexically(&absolute))
            }
            SourcePolicy::Virtual { root } => normalize_virtual_path(root, &path),
            SourcePolicy::Snapshot { .. } => Ok(path),
        }
    }

    fn resolve_snapshot_reference(
        &self,
        source: &Path,
        kind: SourceReferenceKind,
        specifier: &str,
    ) -> Result<PathBuf, SourceResolutionError> {
        let SourcePolicy::Snapshot { resolutions } = &self.policy else {
            unreachable!("snapshot resolution requires snapshot policy");
        };
        let key = SourceResolutionKey {
            source: source.to_path_buf(),
            kind,
            specifier: specifier.to_owned(),
        };
        resolutions.get(&key).cloned().ok_or_else(|| {
            SourceResolutionError::without_candidates(format!(
                "snapshot has no recorded resolution for '{}' from '{}'",
                specifier,
                display_path(source)
            ))
        })
    }
}

fn is_portable_absolute_virtual_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || path.starts_with('\\')
        || matches!(bytes, [drive, b':', ..] if drive.is_ascii_alphabetic())
}

fn normalize_virtual_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        format!(
            "virtual source path '{}' escapes project root '{}'",
            path.display(),
            root.display()
        )
    })?;
    let mut normalized = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if normalized != root => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(format!(
                    "virtual source path '{}' escapes project root '{}'",
                    path.display(),
                    root.display()
                ));
            }
        }
    }
    Ok(normalized)
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

/// Returns an absolute, lexically normalized filesystem path without
/// consulting the filesystem or following symbolic links.
pub fn absolute_lexical_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(normalize_path_lexically(&absolute))
}

/// Rejects filesystem paths that traverse a symlink while allowing a missing
/// suffix, so hosts can still watch unresolved source candidates.
pub fn ensure_no_symlink_components(path: &Path) -> std::io::Result<()> {
    #[cfg(target_family = "wasm")]
    {
        // Browser sources are in-memory overlays. wasm32 has no filesystem to
        // inspect, and some browser runtimes report `Unsupported` here before
        // the overlay resolver gets a chance to match the document.
        let _ = path;
        return Ok(());
    }

    #[cfg(not(target_family = "wasm"))]
    {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        ensure_no_symlink_components_once(&path)?;

        // Keep checking the spelling supplied by the caller so `link/../file`
        // cannot hide a traversed symlink. Also check the lexical destination:
        // once a missing component is reached, `missing/../link` would otherwise
        // stop the first walk before observing `link`.
        let normalized = normalize_path_lexically(&path);
        if normalized != path {
            ensure_no_symlink_components_once(&normalized)?;
        }
        Ok(())
    }
}

#[cfg(not(target_family = "wasm"))]
fn ensure_no_symlink_components_once(path: &Path) -> std::io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if !matches!(component, Component::Normal(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "symlink component '{}' is not supported in filesystem-backed Onda path '{}'",
                        current.display(),
                        path.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
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
