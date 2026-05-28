#[cfg(test)]
use std::collections::HashSet;

use super::{SemanticScope, SemanticScopeIndex};
use onda_semantics::builtins::builtin_constant_names;

pub(super) struct SourceFallbackIndex {
    proc_index: SemanticScopeIndex,
    top_level_index: SemanticScopeIndex,
}

impl SourceFallbackIndex {
    pub(super) fn token_type_for(&self, name: &str, line: u32, column: u32) -> Option<u32> {
        self.proc_index
            .token_type_for(name, line, column)
            .0
            .or_else(|| self.top_level_index.token_type_for(name, line, column).0)
    }
}

pub(super) fn build_source_scope_index(source: &str) -> SourceFallbackIndex {
    SourceFallbackIndex {
        proc_index: build_source_proc_scope_index(source),
        top_level_index: build_source_top_level_scope_index(source),
    }
}

pub(super) fn identifier_is_in_import_path(source_lines: &[&str], line: u32, start: u32) -> bool {
    let line_text = match source_lines.get(line as usize) {
        Some(line_text) => *line_text,
        None => return false,
    };
    let start = start as usize;
    if start > line_text.len() {
        return false;
    }
    let trimmed = line_text.trim_start();
    let indent = line_text.len().saturating_sub(trimmed.len());
    let Some(rest) = trimmed.strip_prefix("import ") else {
        return false;
    };
    if start < indent + "import ".len() {
        return false;
    }
    let between = &rest[..start - indent - "import ".len()];
    between
        .bytes()
        .all(|b| is_ident_continue_char(b) || b == b'/')
}

pub(super) fn is_reserved_word(name: &str) -> bool {
    onda_frontend::is_reserved_word(name)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceProcScopeKind {
    ProcOwner,
    Sample,
    Function,
    Event,
}

#[derive(Clone, Copy)]
struct OpenLineScope<K> {
    idx: usize,
    indent: usize,
    kind: K,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceProcSectionKind {
    Ins,
    Outs,
    Params,
    Buffers,
    Init,
    Events,
    Block,
    Sample,
    Graph,
}

#[derive(Clone, Copy)]
struct SourceProcSection {
    kind: SourceProcSectionKind,
    indent: usize,
    owner_idx: usize,
}

fn build_source_proc_scope_index(source: &str) -> SemanticScopeIndex {
    let mut index = SemanticScopeIndex::default();
    let mut open_scopes: Vec<OpenLineScope<SourceProcScopeKind>> = Vec::new();
    let mut section_stack: Vec<SourceProcSection> = Vec::new();
    let mut prev_nonempty_line = 0_u32;
    let mut saw_nonempty_line = false;

    for (line_no, line) in source.lines().enumerate() {
        let line_no = line_no as u32;
        let indent = line.len().saturating_sub(line.trim_start().len());
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        saw_nonempty_line = true;

        close_open_line_scopes(&mut index, &mut open_scopes, indent, prev_nonempty_line);

        let proc_owner_idx = open_scopes
            .iter()
            .rev()
            .find(|scope| scope.kind == SourceProcScopeKind::ProcOwner)
            .map(|scope| scope.idx);
        trim_section_stack(&mut section_stack, indent, proc_owner_idx);

        if trimmed.starts_with("proc ") || trimmed.starts_with("processor ") {
            let idx = push_line_scope(&mut index, None, line_no, 0, true);
            open_scopes.push(OpenLineScope {
                idx,
                indent,
                kind: SourceProcScopeKind::ProcOwner,
            });

            let after_kw = if trimmed.starts_with("processor ") {
                &trimmed["processor ".len()..]
            } else {
                &trimmed["proc ".len()..]
            };
            if let Some(name) = extract_leading_ident(after_kw.trim_start()) {
                index.document_scope.types.insert(name.to_owned());
                index.scopes[idx].scope.types.insert(name.to_owned());
            }
            extract_type_params(trimmed, &mut index.scopes[idx].scope);
            prev_nonempty_line = line_no;
            continue;
        }

        let Some(proc_owner_idx) = proc_owner_idx else {
            prev_nonempty_line = line_no;
            continue;
        };

        if let Some(section_kind) = detect_source_proc_section_header(trimmed) {
            if matches!(section_kind, SourceProcSectionKind::Sample) {
                let idx = push_line_scope(&mut index, Some(proc_owner_idx), line_no, 0, true);
                open_scopes.push(OpenLineScope {
                    idx,
                    indent,
                    kind: SourceProcScopeKind::Sample,
                });
            }
            section_stack.push(SourceProcSection {
                kind: section_kind,
                indent,
                owner_idx: proc_owner_idx,
            });
            prev_nonempty_line = line_no;
            continue;
        }

        if let Some(section) = section_stack.last().copied() {
            if indent > section.indent {
                match section.kind {
                    SourceProcSectionKind::Ins
                    | SourceProcSectionKind::Outs
                    | SourceProcSectionKind::Buffers => {
                        if let Some(name) = extract_leading_ident(trimmed) {
                            index.scopes[proc_owner_idx]
                                .scope
                                .ports
                                .insert(name.to_owned());
                        }
                    }
                    SourceProcSectionKind::Params => {
                        if let Some(name) = extract_param_decl_name(trimmed, true) {
                            index.scopes[proc_owner_idx]
                                .scope
                                .parameters
                                .insert(name.to_owned());
                        }
                    }
                    SourceProcSectionKind::Init | SourceProcSectionKind::Block => {
                        if let Some(name) = source_assignment_target_name(trimmed) {
                            index.scopes[proc_owner_idx]
                                .scope
                                .insert_state_variable(name.to_owned());
                        }
                    }
                    SourceProcSectionKind::Events => {
                        if let Some((event_name, params)) = parse_event_header(trimmed) {
                            index.scopes[proc_owner_idx]
                                .scope
                                .functions
                                .insert(event_name.to_owned());
                            let idx = push_source_callable_scope(
                                &mut index,
                                Some(proc_owner_idx),
                                line_no,
                                true,
                                event_name,
                                params,
                            );
                            open_scopes.push(OpenLineScope {
                                idx,
                                indent,
                                kind: SourceProcScopeKind::Event,
                            });
                            prev_nonempty_line = line_no;
                            continue;
                        }
                    }
                    SourceProcSectionKind::Sample | SourceProcSectionKind::Graph => {}
                }
            }
        }

        if let Some((def_name, params)) = parse_def_header(trimmed) {
            index.scopes[proc_owner_idx]
                .scope
                .functions
                .insert(def_name.to_owned());
            let idx = push_source_callable_scope(
                &mut index,
                Some(proc_owner_idx),
                line_no,
                true,
                def_name,
                params,
            );
            open_scopes.push(OpenLineScope {
                idx,
                indent,
                kind: SourceProcScopeKind::Function,
            });
            prev_nonempty_line = line_no;
            continue;
        }

        if let Some((event_name, params)) = parse_singular_event_header(trimmed) {
            index.scopes[proc_owner_idx]
                .scope
                .functions
                .insert(event_name.to_owned());
            let idx = push_source_callable_scope(
                &mut index,
                Some(proc_owner_idx),
                line_no,
                true,
                event_name,
                params,
            );
            open_scopes.push(OpenLineScope {
                idx,
                indent,
                kind: SourceProcScopeKind::Event,
            });
            prev_nonempty_line = line_no;
            continue;
        }

        if let Some(active_scope_idx) = open_scopes
            .iter()
            .rev()
            .find(|scope| {
                matches!(
                    scope.kind,
                    SourceProcScopeKind::Sample
                        | SourceProcScopeKind::Function
                        | SourceProcScopeKind::Event
                )
            })
            .map(|scope| scope.idx)
        {
            collect_source_local_symbols(&mut index, active_scope_idx, trimmed);
        }

        prev_nonempty_line = line_no;
    }

    let end_line = if saw_nonempty_line {
        prev_nonempty_line
    } else {
        0
    };
    while let Some(open_scope) = open_scopes.pop() {
        close_line_scope(&mut index, open_scope.idx, end_line);
    }

    index
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceTopLevelScopeKind {
    Sample,
    Function,
    Event,
}

#[derive(Clone, Copy)]
enum SourceTopLevelSectionKind {
    Ins,
    Outs,
    Params,
    Buffers,
    Init,
    Events,
    Block,
    Sample,
    Graph,
}

#[derive(Clone, Copy)]
struct SourceTopLevelSection {
    kind: SourceTopLevelSectionKind,
    indent: usize,
    owner_idx: usize,
}

fn build_source_top_level_scope_index(source: &str) -> SemanticScopeIndex {
    let mut index = SemanticScopeIndex::default();
    let mut open_scopes: Vec<OpenLineScope<SourceTopLevelScopeKind>> = Vec::new();
    let mut section_stack: Vec<SourceTopLevelSection> = Vec::new();
    let mut runtime_owner_idx: Option<usize> = None;
    let mut proc_indent_stack: Vec<usize> = Vec::new();
    let mut prev_nonempty_line = 0_u32;
    let mut saw_nonempty_line = false;

    for (line_no, line) in source.lines().enumerate() {
        let line_no = line_no as u32;
        let indent = line.len().saturating_sub(line.trim_start().len());
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        saw_nonempty_line = true;

        while let Some(proc_indent) = proc_indent_stack.last().copied() {
            if indent > proc_indent {
                break;
            }
            proc_indent_stack.pop();
        }

        close_open_line_scopes(&mut index, &mut open_scopes, indent, prev_nonempty_line);
        trim_section_stack(&mut section_stack, indent, runtime_owner_idx);

        if trimmed.starts_with("proc ") || trimmed.starts_with("processor ") {
            proc_indent_stack.push(indent);
            prev_nonempty_line = line_no;
            continue;
        }
        if !proc_indent_stack.is_empty() {
            prev_nonempty_line = line_no;
            continue;
        }

        if let Some(section_kind) = detect_source_top_level_section_header(trimmed) {
            let owner_idx = *runtime_owner_idx.get_or_insert_with(|| {
                let idx = push_line_scope(&mut index, None, line_no, 0, true);
                index.scopes[idx].end_line = u32::MAX;
                index.scopes[idx].end_column = u32::MAX;
                idx
            });
            if matches!(section_kind, SourceTopLevelSectionKind::Sample) {
                let idx = push_line_scope(&mut index, Some(owner_idx), line_no, 0, true);
                open_scopes.push(OpenLineScope {
                    idx,
                    indent,
                    kind: SourceTopLevelScopeKind::Sample,
                });
            }
            section_stack.push(SourceTopLevelSection {
                kind: section_kind,
                indent,
                owner_idx,
            });
            prev_nonempty_line = line_no;
            continue;
        }

        if let Some(section) = section_stack.last().copied() {
            if indent > section.indent {
                match section.kind {
                    SourceTopLevelSectionKind::Ins
                    | SourceTopLevelSectionKind::Outs
                    | SourceTopLevelSectionKind::Buffers => {
                        if let Some(name) = extract_leading_ident(trimmed) {
                            index.scopes[section.owner_idx]
                                .scope
                                .ports
                                .insert(name.to_owned());
                        }
                    }
                    SourceTopLevelSectionKind::Params => {
                        if let Some(name) = extract_param_decl_name(trimmed, false) {
                            index.scopes[section.owner_idx]
                                .scope
                                .parameters
                                .insert(name.to_owned());
                        }
                    }
                    SourceTopLevelSectionKind::Init | SourceTopLevelSectionKind::Block => {
                        if let Some(name) = source_assignment_target_name(trimmed) {
                            index.scopes[section.owner_idx]
                                .scope
                                .insert_state_variable(name.to_owned());
                        }
                    }
                    SourceTopLevelSectionKind::Events => {
                        if let Some((event_name, params)) = parse_event_header(trimmed) {
                            index.scopes[section.owner_idx]
                                .scope
                                .functions
                                .insert(event_name.to_owned());
                            let idx = push_source_callable_scope(
                                &mut index,
                                Some(section.owner_idx),
                                line_no,
                                true,
                                event_name,
                                params,
                            );
                            open_scopes.push(OpenLineScope {
                                idx,
                                indent,
                                kind: SourceTopLevelScopeKind::Event,
                            });
                            prev_nonempty_line = line_no;
                            continue;
                        }
                    }
                    SourceTopLevelSectionKind::Sample | SourceTopLevelSectionKind::Graph => {}
                }
            }
        }

        if let Some((def_name, params)) = parse_def_header(trimmed) {
            index.document_scope.functions.insert(def_name.to_owned());
            let idx =
                push_source_callable_scope(&mut index, None, line_no, false, def_name, params);
            open_scopes.push(OpenLineScope {
                idx,
                indent,
                kind: SourceTopLevelScopeKind::Function,
            });
            prev_nonempty_line = line_no;
            continue;
        }

        if let Some((event_name, params)) = parse_singular_event_header(trimmed) {
            let owner_idx = *runtime_owner_idx.get_or_insert_with(|| {
                let idx = push_line_scope(&mut index, None, line_no, 0, true);
                index.scopes[idx].end_line = u32::MAX;
                index.scopes[idx].end_column = u32::MAX;
                idx
            });
            index.scopes[owner_idx]
                .scope
                .functions
                .insert(event_name.to_owned());
            let idx = push_source_callable_scope(
                &mut index,
                Some(owner_idx),
                line_no,
                true,
                event_name,
                params,
            );
            open_scopes.push(OpenLineScope {
                idx,
                indent,
                kind: SourceTopLevelScopeKind::Event,
            });
            prev_nonempty_line = line_no;
            continue;
        }

        if let Some(active_scope_idx) = open_scopes
            .iter()
            .rev()
            .find(|scope| {
                matches!(
                    scope.kind,
                    SourceTopLevelScopeKind::Sample
                        | SourceTopLevelScopeKind::Function
                        | SourceTopLevelScopeKind::Event
                )
            })
            .map(|scope| scope.idx)
        {
            collect_source_local_symbols(&mut index, active_scope_idx, trimmed);
        }

        prev_nonempty_line = line_no;
    }

    let end_line = if saw_nonempty_line {
        prev_nonempty_line
    } else {
        0
    };
    while let Some(open_scope) = open_scopes.pop() {
        close_line_scope(&mut index, open_scope.idx, end_line);
    }
    if let Some(owner_idx) = runtime_owner_idx {
        close_line_scope(&mut index, owner_idx, end_line);
    }

    index
}

fn push_line_scope(
    index: &mut SemanticScopeIndex,
    parent: Option<usize>,
    start_line: u32,
    start_column: u32,
    allows_implicit_ports: bool,
) -> usize {
    let depth = parent.map(|idx| index.scopes[idx].depth + 1).unwrap_or(0);
    index.scopes.push(super::ScopedSemanticScope {
        scope: SemanticScope::default(),
        parent,
        start_line,
        start_column,
        end_line: start_line,
        end_column: u32::MAX,
        depth,
        allows_implicit_ports,
    });
    index.scopes.len() - 1
}

fn close_open_line_scopes<K: Copy>(
    index: &mut SemanticScopeIndex,
    open_scopes: &mut Vec<OpenLineScope<K>>,
    indent: usize,
    prev_nonempty_line: u32,
) {
    while let Some(open_scope) = open_scopes.last().copied() {
        if indent > open_scope.indent {
            break;
        }
        close_line_scope(index, open_scope.idx, prev_nonempty_line);
        open_scopes.pop();
    }
}

fn close_line_scope(index: &mut SemanticScopeIndex, idx: usize, end_line: u32) {
    index.scopes[idx].end_line = end_line;
    index.scopes[idx].end_column = u32::MAX;
}

fn push_source_callable_scope(
    index: &mut SemanticScopeIndex,
    parent: Option<usize>,
    line_no: u32,
    allows_implicit_ports: bool,
    name: &str,
    params: Vec<String>,
) -> usize {
    let idx = push_line_scope(index, parent, line_no, 0, allows_implicit_ports);
    let scope = &mut index.scopes[idx].scope;
    scope.functions.insert(name.to_owned());
    for param in params {
        scope.parameters.insert(param);
    }
    idx
}

fn trim_section_stack<S: Copy + SectionOwner>(
    section_stack: &mut Vec<S>,
    indent: usize,
    active_owner_idx: Option<usize>,
) {
    while let Some(section) = section_stack.last().copied() {
        if indent > section.indent() && active_owner_idx == Some(section.owner_idx()) {
            break;
        }
        section_stack.pop();
    }
}

trait SectionOwner {
    fn indent(self) -> usize;
    fn owner_idx(self) -> usize;
}

impl SectionOwner for SourceProcSection {
    fn indent(self) -> usize {
        self.indent
    }

    fn owner_idx(self) -> usize {
        self.owner_idx
    }
}

impl SectionOwner for SourceTopLevelSection {
    fn indent(self) -> usize {
        self.indent
    }

    fn owner_idx(self) -> usize {
        self.owner_idx
    }
}

pub(super) fn collect_source_declaration_symbols(source: &str, scope: &mut SemanticScope) {
    for name in builtin_constant_names() {
        scope.consts.insert(name.to_owned());
    }

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with("namespace ") {
            let rest = &trimmed["namespace ".len()..];
            let path_end = rest
                .find(|c: char| c == '<' || c == ':' || c == '=')
                .unwrap_or(rest.len());
            let path = rest[..path_end].trim();
            for segment in path.split("::") {
                let segment = segment.trim();
                if let Some(name) = extract_leading_ident(segment) {
                    scope.types.insert(name.to_owned());
                }
            }
            extract_namespace_generic_consts(trimmed, scope);
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("const ") {
            if let Some(name) = extract_leading_ident(rest) {
                scope.consts.insert(name.to_owned());
            }
            continue;
        }

        if trimmed.starts_with("proc ")
            || trimmed.starts_with("processor ")
            || trimmed.starts_with("struct ")
        {
            let after_kw = if trimmed.starts_with("processor ") {
                &trimmed["processor ".len()..]
            } else if trimmed.starts_with("proc ") {
                &trimmed["proc ".len()..]
            } else {
                &trimmed["struct ".len()..]
            };
            if let Some(name) = extract_leading_ident(after_kw.trim_start()) {
                scope.types.insert(name.to_owned());
            }
            extract_type_params(trimmed, scope);
            continue;
        }

        if trimmed.starts_with("def ") {
            extract_def_declaration_symbol(trimmed, scope);
            continue;
        }
    }
}

fn detect_source_proc_section_header(trimmed: &str) -> Option<SourceProcSectionKind> {
    let pairs: &[(&str, SourceProcSectionKind)] = &[
        ("ins", SourceProcSectionKind::Ins),
        ("outs", SourceProcSectionKind::Outs),
        ("params", SourceProcSectionKind::Params),
        ("buffers", SourceProcSectionKind::Buffers),
        ("init", SourceProcSectionKind::Init),
        ("events", SourceProcSectionKind::Events),
        ("block", SourceProcSectionKind::Block),
        ("sample", SourceProcSectionKind::Sample),
        ("graph", SourceProcSectionKind::Graph),
    ];
    detect_source_section_header(trimmed, pairs)
}

fn detect_source_top_level_section_header(trimmed: &str) -> Option<SourceTopLevelSectionKind> {
    let pairs: &[(&str, SourceTopLevelSectionKind)] = &[
        ("ins", SourceTopLevelSectionKind::Ins),
        ("outs", SourceTopLevelSectionKind::Outs),
        ("params", SourceTopLevelSectionKind::Params),
        ("buffers", SourceTopLevelSectionKind::Buffers),
        ("init", SourceTopLevelSectionKind::Init),
        ("events", SourceTopLevelSectionKind::Events),
        ("block", SourceTopLevelSectionKind::Block),
        ("sample", SourceTopLevelSectionKind::Sample),
        ("graph", SourceTopLevelSectionKind::Graph),
    ];
    detect_source_section_header(trimmed, pairs)
}

fn detect_source_section_header<K: Copy>(trimmed: &str, pairs: &[(&str, K)]) -> Option<K> {
    for &(kw, kind) in pairs {
        if trimmed.starts_with(kw) {
            let rest = &trimmed[kw.len()..];
            if rest.is_empty()
                || rest.starts_with(':')
                || rest.starts_with('<')
                || rest.starts_with(' ')
                || rest.starts_with('{')
            {
                return Some(kind);
            }
        }
    }
    None
}

fn parse_def_header(trimmed: &str) -> Option<(&str, Vec<String>)> {
    let rest = trimmed.strip_prefix("def ")?.trim_start();
    let name = extract_leading_ident(rest)?;
    let after_name = &rest[name.len()..];
    let after_generics = if let Some(open) = after_name.find('<') {
        if let Some(close) = after_name[open..].find('>') {
            &after_name[open + close + 1..]
        } else {
            after_name
        }
    } else {
        after_name
    };
    Some((name, extract_param_names_from_parens(after_generics)))
}

fn parse_singular_event_header(trimmed: &str) -> Option<(&str, Vec<String>)> {
    let rest = trimmed.strip_prefix("event ")?.trim_start();
    let name = extract_leading_ident(rest)?;
    let after_name = &rest[name.len()..];
    Some((name, extract_param_names_from_parens(after_name)))
}

fn parse_event_header(trimmed: &str) -> Option<(&str, Vec<String>)> {
    let paren = trimmed.find('(')?;
    let name = trimmed[..paren].trim();
    if name.is_empty()
        || !is_ident_start(name.as_bytes()[0])
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    Some((name, extract_param_names_from_parens(&trimmed[paren..])))
}

fn extract_param_names_from_parens(text: &str) -> Vec<String> {
    let inner = if let Some(open) = text.find('(') {
        if let Some(close) = text[open..].find(')') {
            &text[open + 1..open + close]
        } else {
            return Vec::new();
        }
    } else {
        return Vec::new();
    };

    inner
        .split(',')
        .filter_map(|part| extract_leading_ident(part.trim()).map(str::to_owned))
        .collect()
}

fn source_assignment_target_name(trimmed: &str) -> Option<&str> {
    let name = extract_leading_ident(trimmed)?;
    let rest = trimmed[name.len()..].trim_start();
    if (rest.starts_with(':') && !rest.starts_with("::"))
        || (rest.starts_with('=') && !rest.starts_with("=="))
    {
        Some(name)
    } else {
        None
    }
}

fn extract_param_decl_name(trimmed: &str, allow_pin: bool) -> Option<&str> {
    let name = extract_leading_ident(trimmed)?;
    if name != "pin" {
        return Some(name);
    }
    if !allow_pin {
        return None;
    }
    let rest = trimmed[name.len()..].trim_start();
    let pinned_name = extract_leading_ident(rest)?;
    if is_reserved_identifier(pinned_name) {
        None
    } else {
        Some(pinned_name)
    }
}

fn is_reserved_identifier(name: &str) -> bool {
    onda_frontend::is_reserved_identifier(name)
}

fn collect_source_local_symbols(index: &mut SemanticScopeIndex, scope_idx: usize, trimmed: &str) {
    if let Some(rest) = trimmed.strip_prefix("for ") {
        if let Some(in_pos) = rest.find(" in ") {
            let binding = rest[..in_pos].trim();
            let name = binding
                .split_once('@')
                .map(|(name, _)| name.trim())
                .unwrap_or(binding);
            if !name.is_empty() && is_ident_start(name.as_bytes()[0]) {
                insert_source_local_symbol(index, scope_idx, name);
            }
        }
        return;
    }

    if let Some(name) = source_assignment_target_name(trimmed) {
        insert_source_local_symbol(index, scope_idx, name);
    }
}

fn insert_source_local_symbol(index: &mut SemanticScopeIndex, scope_idx: usize, name: &str) {
    if source_local_name_is_reserved(index, scope_idx, name) {
        return;
    }
    index.scopes[scope_idx]
        .scope
        .insert_variable(name.to_owned());
}

fn source_local_name_is_reserved(index: &SemanticScopeIndex, scope_idx: usize, name: &str) -> bool {
    let mut current = Some(scope_idx);
    let mut allows_implicit_ports = false;

    while let Some(idx) = current {
        let scope = &index.scopes[idx];
        allows_implicit_ports |= scope.allows_implicit_ports;
        if scope.scope.token_type_for(name).is_some() {
            return true;
        }
        current = scope.parent;
    }

    index.document_scope.token_type_for(name).is_some()
        || (allows_implicit_ports && super::is_implicit_port_name(name))
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_ident_continue_char(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

fn extract_leading_ident(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || !is_ident_start(bytes[0]) {
        return None;
    }
    let end = bytes
        .iter()
        .position(|&b| !is_ident_continue(b))
        .unwrap_or(bytes.len());
    Some(&text[..end])
}

fn extract_namespace_generic_consts(trimmed: &str, scope: &mut SemanticScope) {
    if let Some(open) = trimmed.find('<') {
        if let Some(close) = trimmed[open..].find('>') {
            let inner = &trimmed[open + 1..open + close];
            for part in inner.split(',') {
                let part = part.trim();
                if let Some(name) = extract_leading_ident(part) {
                    scope.consts.insert(name.to_owned());
                }
            }
        }
    }
}

fn extract_type_params(trimmed: &str, scope: &mut SemanticScope) {
    if let Some(open) = trimmed.find('<') {
        if let Some(close) = trimmed[open..].find('>') {
            let inner = &trimmed[open + 1..open + close];
            for part in inner.split(',') {
                let part = part.trim();
                if let Some(name) = extract_leading_ident(part) {
                    if name.as_bytes()[0].is_ascii_alphabetic() && !part.contains('=') {
                        scope.types.insert(name.to_owned());
                    }
                }
            }
        }
    }
}

fn extract_def_declaration_symbol(trimmed: &str, scope: &mut SemanticScope) {
    let rest = trimmed.strip_prefix("def ").unwrap_or(trimmed).trim_start();
    if let Some(name) = extract_leading_ident(rest) {
        scope.functions.insert(name.to_owned());
        let after_name = &rest[name.len()..];
        if let Some(open) = after_name.find('<') {
            if after_name[open..].find('>').is_some() {
                extract_type_params(after_name, scope);
            }
        }
    }
}

#[cfg(test)]
pub(super) fn collect_const_names(source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("const ") {
            if let Some(name) = extract_leading_ident(rest.trim_start()) {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

pub(super) fn scan_identifiers(
    source: &str,
    mut f: impl FnMut(&str, u32, u32, u32, bool, bool, bool, bool),
) {
    let chars = source.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut line = 0_u32;
    let mut column = 0_u32;
    let mut prev_char = '\0';
    let mut prev_prev_char = '\0';
    let mut call_paren_stack: Vec<bool> = Vec::new();
    let mut next_lparen_is_call = false;

    while index < chars.len() {
        let ch = chars[index];
        if ch == '#' {
            while index < chars.len() && chars[index] != '\n' {
                prev_prev_char = prev_char;
                prev_char = chars[index];
                advance_position(chars[index], &mut line, &mut column);
                index += 1;
            }
            continue;
        }
        if ch == '"' {
            advance_position(ch, &mut line, &mut column);
            index += 1;
            let mut escaped = false;
            while index < chars.len() {
                let ch = chars[index];
                advance_position(ch, &mut line, &mut column);
                index += 1;
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    break;
                }
            }
            prev_prev_char = prev_char;
            prev_char = '"';
            continue;
        }
        if ch == '(' {
            call_paren_stack.push(next_lparen_is_call);
            next_lparen_is_call = false;
        } else if ch == ')' {
            call_paren_stack.pop();
            next_lparen_is_call = false;
        } else if !ch.is_whitespace() && ch != '<' && ch != '>' {
            next_lparen_is_call = false;
        }
        if is_identifier_start_char(ch) {
            let after_dot = prev_char == '.' && prev_prev_char != '.';
            let start_line = line;
            let start_column = column;
            let mut name = String::new();
            while index < chars.len() && is_identifier_continue_char(chars[index]) {
                let ch = chars[index];
                name.push(ch);
                advance_position(ch, &mut line, &mut column);
                index += 1;
            }
            let length = name.encode_utf16().count() as u32;
            let after_colons = prev_char == ':' && prev_prev_char == ':';
            prev_prev_char = prev_char;
            prev_char = name.chars().last().unwrap_or('\0');
            let mut peek = index;
            while peek < chars.len() && chars[peek].is_whitespace() && chars[peek] != '\n' {
                peek += 1;
            }
            let mut is_call = peek < chars.len() && chars[peek] == '(';
            let mut call_paren_index = if is_call { Some(peek) } else { None };
            let mut followed_by_colons =
                peek + 1 < chars.len() && chars[peek] == ':' && chars[peek + 1] == ':';
            if !is_call && !followed_by_colons && peek < chars.len() && chars[peek] == '<' {
                let mut depth = 1;
                peek += 1;
                while peek < chars.len() && depth > 0 {
                    if chars[peek] == '<' {
                        depth += 1;
                    } else if chars[peek] == '>' {
                        depth -= 1;
                    } else if chars[peek] == '\n' {
                        break;
                    }
                    peek += 1;
                }
                if depth == 0 {
                    while peek < chars.len() && chars[peek].is_whitespace() && chars[peek] != '\n' {
                        peek += 1;
                    }
                    is_call = peek < chars.len() && chars[peek] == '(';
                    call_paren_index = if is_call { Some(peek) } else { None };
                    followed_by_colons =
                        peek + 1 < chars.len() && chars[peek] == ':' && chars[peek + 1] == ':';
                }
            }
            let in_ns_path = after_colons || followed_by_colons;
            let opens_call_arg_list = call_paren_index
                .map(|paren_idx| !paren_is_followed_by_colon(&chars, paren_idx))
                .unwrap_or(false);
            let mut assign_peek = index;
            while assign_peek < chars.len()
                && chars[assign_peek].is_whitespace()
                && chars[assign_peek] != '\n'
            {
                assign_peek += 1;
            }
            let is_named_arg_label = call_paren_stack.last().copied().unwrap_or(false)
                && assign_peek < chars.len()
                && chars[assign_peek] == '='
                && (assign_peek + 1 >= chars.len() || chars[assign_peek + 1] != '=');
            next_lparen_is_call = opens_call_arg_list;
            f(
                &name,
                start_line,
                start_column,
                length,
                after_dot,
                is_call,
                in_ns_path,
                is_named_arg_label,
            );
            continue;
        }

        if !ch.is_whitespace() {
            prev_prev_char = prev_char;
            prev_char = ch;
        }
        advance_position(ch, &mut line, &mut column);
        index += 1;
    }
}

fn is_identifier_start_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_identifier_continue_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn paren_is_followed_by_colon(chars: &[char], open_idx: usize) -> bool {
    let mut depth = 0_u32;
    let mut idx = open_idx;
    let mut in_string = false;
    let mut escaped = false;

    while idx < chars.len() {
        let ch = chars[idx];
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            idx += 1;
            continue;
        }

        if ch == '#' {
            while idx < chars.len() && chars[idx] != '\n' {
                idx += 1;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    idx += 1;
                    break;
                }
            }
            _ => {}
        }
        idx += 1;
    }

    while idx < chars.len() {
        let ch = chars[idx];
        if ch == '#' {
            while idx < chars.len() && chars[idx] != '\n' {
                idx += 1;
            }
            continue;
        }
        if ch.is_whitespace() {
            idx += 1;
            continue;
        }
        return ch == ':';
    }

    false
}

fn advance_position(ch: char, line: &mut u32, column: &mut u32) {
    match ch {
        '\n' => {
            *line += 1;
            *column = 0;
        }
        '\r' => {}
        _ => {
            *column += ch.len_utf16() as u32;
        }
    }
}
