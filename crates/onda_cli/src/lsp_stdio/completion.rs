use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use onda_frontend::{
    is_reserved_word, language_type_names, parse_program, parse_program_file_with_overlays,
    stdlib_module_names, ArrayElemType, Block, BufferBlock, BufferDecl, ConstDecl, EventDef,
    EventParamDecl, Expr, FnParamDecl, FunctionDef, InitBlock, NamespaceAliasDecl, NamespaceDecl,
    NamespaceItem, NamespaceRefSegment, NamespaceTemplateParam, OutputTiming, ParamBlock,
    ParamDecl, PortBlock, PortDecl, ProcessorDef, Program, Span, Stmt, StructDef, UseDecl,
    LANGUAGE_KEYWORDS,
};
use onda_semantics::builtins::{
    builtin_constant_names, public_builtin_function_names, ARRAY_LEN_METHOD,
};
use serde_json::{json, Value};

use crate::formatting::{
    format_buffer_type, format_decl_type, format_event_param_type, format_expr,
    format_fn_param_type,
};

const COMPLETION_ITEM_KIND_METHOD: u32 = 2;
const COMPLETION_ITEM_KIND_FUNCTION: u32 = 3;
const COMPLETION_ITEM_KIND_CONSTRUCTOR: u32 = 4;
const COMPLETION_ITEM_KIND_FIELD: u32 = 5;
const COMPLETION_ITEM_KIND_VARIABLE: u32 = 6;
const COMPLETION_ITEM_KIND_MODULE: u32 = 9;
const COMPLETION_ITEM_KIND_PROPERTY: u32 = 10;
const COMPLETION_ITEM_KIND_KEYWORD: u32 = 14;
const COMPLETION_ITEM_KIND_FILE: u32 = 17;
const COMPLETION_ITEM_KIND_CONSTANT: u32 = 21;
const COMPLETION_ITEM_KIND_STRUCT: u32 = 22;
const COMPLETION_ITEM_KIND_EVENT: u32 = 23;
const COMPLETION_ITEM_KIND_TYPE_PARAMETER: u32 = 25;

const INSERT_TEXT_FORMAT_SNIPPET: u32 = 2;

const MAX_DEFERRED_COUNT_COMPLETIONS: usize = 128;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CompletionPosition {
    pub(super) line: u32,
    pub(super) character: u32,
}

pub(super) fn completion_trigger_characters() -> &'static [&'static str] {
    &[".", ":", "/", " ", "(", ","]
}

pub(super) fn completion_items_for_document(
    source: &str,
    path: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
    position: CompletionPosition,
    snippets: bool,
) -> Vec<Value> {
    let offset = byte_offset_for_position(source, position);
    let context = CompletionContext::from_source(source, offset);
    if context.in_comment {
        return Vec::new();
    }

    if let CompletionContextKind::ImportPath { typed } = &context.kind {
        return filter_and_encode(import_completion_items(path, typed), "", snippets);
    }

    let parsed = parse_for_completion(source, path, overlays, offset);
    let index = CompletionIndex::build(parsed.as_ref(), source, path, position);
    let items = match &context.kind {
        CompletionContextKind::Member { receiver } => index.member_items(receiver, &context.prefix),
        CompletionContextKind::Namespace { namespace } => {
            index.namespace_items(namespace, &context.prefix)
        }
        CompletionContextKind::CallArgs { callee } => index.call_arg_items(callee),
        CompletionContextKind::General => index.general_items(&context.prefix),
        CompletionContextKind::ImportPath { .. } => Vec::new(),
    };

    filter_and_encode(items, &context.prefix, snippets)
}

fn parse_for_completion(
    source: &str,
    path: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
    offset: usize,
) -> Option<Program> {
    if let Some(path) = path {
        if let Ok(program) = parse_program_file_with_overlays(path, overlays) {
            return Some(program);
        }
        let sanitized = source_with_current_line_placeholder(source, offset);
        let mut sanitized_overlays = overlays.clone();
        sanitized_overlays.insert(path.to_path_buf(), sanitized);
        if let Ok(program) = parse_program_file_with_overlays(path, &sanitized_overlays) {
            return Some(program);
        }
    }
    parse_program(source)
        .or_else(|_| parse_program(&source_with_current_line_placeholder(source, offset)))
        .ok()
}

#[derive(Debug, Clone)]
struct CompletionContext {
    prefix: String,
    kind: CompletionContextKind,
    in_comment: bool,
}

#[derive(Debug, Clone)]
enum CompletionContextKind {
    General,
    Member { receiver: String },
    Namespace { namespace: String },
    CallArgs { callee: String },
    ImportPath { typed: String },
}

impl CompletionContext {
    fn from_source(source: &str, offset: usize) -> Self {
        let offset = offset.min(source.len());
        let line_start = source[..offset].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let line_before = &source[line_start..offset];
        let in_comment = line_before
            .find('#')
            .is_some_and(|idx| !inside_string_literal(&line_before[..idx]));
        let line_before_code = line_before.split('#').next().unwrap_or(line_before);
        let trimmed_start = line_before_code.trim_start();

        if let Some(rest) = trimmed_start.strip_prefix("import ") {
            return Self {
                prefix: rest.to_owned(),
                kind: CompletionContextKind::ImportPath {
                    typed: rest.to_owned(),
                },
                in_comment,
            };
        }

        let prefix_start = identifier_prefix_start(source, offset);
        let prefix = source[prefix_start..offset].to_owned();
        let before_prefix = &source[..prefix_start];
        let line_before_prefix = &source[line_start..prefix_start];
        let before_prefix_trimmed = before_prefix.trim_end();

        if before_prefix_trimmed.ends_with('.') {
            let without_dot = &before_prefix_trimmed[..before_prefix_trimmed.len() - 1];
            if let Some(receiver) = scan_receiver_left(without_dot) {
                return Self {
                    prefix,
                    kind: CompletionContextKind::Member { receiver },
                    in_comment,
                };
            }
        }

        if before_prefix_trimmed.ends_with("::") {
            let without_colons = &before_prefix_trimmed[..before_prefix_trimmed.len() - 2];
            if let Some(namespace) = scan_namespace_left(without_colons) {
                return Self {
                    prefix,
                    kind: CompletionContextKind::Namespace { namespace },
                    in_comment,
                };
            }
        }

        if let Some(active_call) = active_call_at(source, prefix_start) {
            return Self {
                prefix,
                kind: if current_call_arg_is_named_value(
                    source,
                    active_call.open_paren,
                    prefix_start,
                ) {
                    CompletionContextKind::General
                } else {
                    CompletionContextKind::CallArgs {
                        callee: active_call.callee,
                    }
                },
                in_comment,
            };
        }

        Self {
            prefix,
            kind: if line_before_prefix.trim_start().starts_with("import ") {
                CompletionContextKind::ImportPath {
                    typed: line_before_prefix
                        .trim_start()
                        .strip_prefix("import ")
                        .unwrap_or_default()
                        .to_owned(),
                }
            } else {
                CompletionContextKind::General
            },
            in_comment,
        }
    }
}

#[derive(Debug, Clone)]
struct CompletionItem {
    label: String,
    kind: u32,
    label_detail: Option<String>,
    detail: Option<String>,
    insert_text: Option<String>,
    snippet_insert_text: Option<String>,
    sort_text: Option<String>,
}

impl CompletionItem {
    fn new(label: impl Into<String>, kind: u32) -> Self {
        Self {
            label: label.into(),
            kind,
            label_detail: None,
            detail: None,
            insert_text: None,
            snippet_insert_text: None,
            sort_text: None,
        }
    }

    fn maybe_label_detail(mut self, label_detail: Option<String>) -> Self {
        self.label_detail = label_detail;
        self
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn insert_text(mut self, insert_text: impl Into<String>) -> Self {
        self.insert_text = Some(insert_text.into());
        self
    }

    fn snippet(mut self, snippet: impl Into<String>) -> Self {
        self.snippet_insert_text = Some(snippet.into());
        self
    }

    fn maybe_snippet(mut self, snippet: Option<String>) -> Self {
        self.snippet_insert_text = snippet;
        self
    }

    fn sort_text(mut self, sort_text: impl Into<String>) -> Self {
        self.sort_text = Some(sort_text.into());
        self
    }

    fn to_lsp(&self, snippets: bool) -> Value {
        let mut item = json!({
            "label": self.label,
            "kind": self.kind,
        });
        if let Some(label_detail) = &self.label_detail {
            item["labelDetails"] = json!({
                "detail": label_detail,
            });
        }
        if let Some(detail) = &self.detail {
            item["detail"] = json!(detail);
        }
        if let Some(sort_text) = &self.sort_text {
            item["sortText"] = json!(sort_text);
        }
        if snippets {
            if let Some(insert_text) = &self.snippet_insert_text {
                item["insertText"] = json!(insert_text);
                item["insertTextFormat"] = json!(INSERT_TEXT_FORMAT_SNIPPET);
                return item;
            }
        }
        if let Some(insert_text) = &self.insert_text {
            item["insertText"] = json!(insert_text);
        }
        item
    }
}

#[derive(Debug, Clone)]
struct SymbolInfo {
    label: String,
    full_name: String,
    namespace: String,
    kind: SymbolKind,
    type_params: Vec<String>,
    label_detail: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SymbolKind {
    Const,
    Def,
    Proc,
    Struct,
    Namespace,
}

#[derive(Debug, Clone, Default)]
struct ProcInfo {
    name: String,
    type_params: Vec<String>,
    ins: Vec<String>,
    outs: Vec<String>,
    params: Vec<ProcParamInfo>,
    events: Vec<EventInfo>,
    buffers: Vec<ProcBufferInfo>,
    local_defs: Vec<FunctionInfo>,
}

#[derive(Debug, Clone)]
struct ProcParamInfo {
    name: String,
    pinned: bool,
    signature: String,
}

#[derive(Debug, Clone)]
struct ProcBufferInfo {
    name: String,
    signature: String,
}

#[derive(Debug, Clone)]
struct EventInfo {
    name: String,
    param_names: Vec<String>,
    signature: String,
}

#[derive(Debug, Clone, Default)]
struct StructInfo {
    name: String,
    type_params: Vec<String>,
    fields: Vec<String>,
    methods: Vec<FunctionInfo>,
}

#[derive(Debug, Clone)]
struct FunctionInfo {
    name: String,
    type_params: Vec<String>,
    params: Vec<String>,
    signature: String,
    is_const: bool,
}

#[derive(Debug, Clone)]
struct InstanceInfo {
    type_name: String,
    is_array: bool,
}

#[derive(Debug, Clone)]
struct UseInfo {
    namespace: String,
    target: String,
    alias: Option<String>,
    public: bool,
    file_key: Option<String>,
}

#[derive(Debug, Clone)]
struct CompletionScope {
    parent: Option<usize>,
    namespace: String,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    depth: usize,
    items: Vec<CompletionItem>,
    instances: HashMap<String, InstanceInfo>,
}

#[derive(Debug, Default)]
struct CompletionIndex {
    current_file_key: Option<String>,
    position: CompletionPosition,
    symbols: Vec<SymbolInfo>,
    members_by_namespace: HashMap<String, Vec<SymbolInfo>>,
    namespaces_by_parent: HashMap<String, BTreeSet<String>>,
    namespace_aliases: HashMap<String, String>,
    uses: Vec<UseInfo>,
    procs: HashMap<String, ProcInfo>,
    structs: HashMap<String, StructInfo>,
    functions: HashMap<String, FunctionInfo>,
    instances: HashMap<String, InstanceInfo>,
    source_variables: BTreeSet<String>,
    scopes: Vec<CompletionScope>,
}

impl CompletionIndex {
    fn build(
        program: Option<&Program>,
        source: &str,
        path: Option<&Path>,
        position: CompletionPosition,
    ) -> Self {
        let mut index = Self {
            current_file_key: path.map(normalize_file_key_for_path),
            position,
            ..Self::default()
        };
        for name in builtin_constant_names() {
            index.symbols.push(SymbolInfo {
                label: name.to_owned(),
                full_name: name.to_owned(),
                namespace: String::new(),
                kind: SymbolKind::Const,
                type_params: Vec::new(),
                label_detail: None,
                detail: Some("builtin const".to_owned()),
            });
        }
        for name in public_builtin_function_names() {
            let info = FunctionInfo {
                name: name.to_owned(),
                type_params: Vec::new(),
                params: Vec::new(),
                signature: "...".to_owned(),
                is_const: false,
            };
            index.functions.insert(name.to_owned(), info);
            index.symbols.push(SymbolInfo {
                label: name.to_owned(),
                full_name: name.to_owned(),
                namespace: String::new(),
                kind: SymbolKind::Def,
                type_params: Vec::new(),
                label_detail: Some("(...)".to_owned()),
                detail: Some(format!("built-in call {name}(...)")),
            });
        }

        if let Some(program) = program {
            for block in &program.blocks {
                index.collect_block(block, "");
            }
            index.rebuild_namespace_member_maps();
            index.collect_scopes(program);
        }

        if program.is_none() {
            index.collect_source_variables(source);
        }
        index.rebuild_namespace_member_maps();
        index
    }

    fn collect_block(&mut self, block: &Block, namespace: &str) {
        match block {
            Block::Const(decl) => self.collect_const(namespace, decl),
            Block::Def(def) => self.collect_def(namespace, def),
            Block::Proc(proc_def) => self.collect_proc(namespace, proc_def),
            Block::Struct(struct_def) => self.collect_struct(namespace, struct_def),
            Block::Namespace(ns) => self.collect_namespace(namespace, ns),
            Block::NamespaceAlias(alias) => self.collect_namespace_alias(namespace, alias),
            Block::Use(use_decl) => self.collect_use(namespace, use_decl),
            _ => {}
        }
    }

    fn collect_const(&mut self, namespace: &str, decl: &ConstDecl) {
        let full_name = namespace_join(namespace, &decl.name);
        self.push_symbol(SymbolInfo {
            label: decl.name.clone(),
            full_name,
            namespace: namespace.to_owned(),
            kind: SymbolKind::Const,
            type_params: Vec::new(),
            label_detail: None,
            detail: Some("const".to_owned()),
        });
    }

    fn collect_def(&mut self, namespace: &str, def: &FunctionDef) {
        let full_name = namespace_join(namespace, &def.name);
        let info = FunctionInfo {
            name: def.name.clone(),
            type_params: def.type_params.clone(),
            params: def.params.iter().map(|p| p.name.clone()).collect(),
            signature: function_signature(def),
            is_const: def.is_const,
        };
        self.functions.insert(full_name.clone(), info.clone());
        self.push_symbol(SymbolInfo {
            label: def.name.clone(),
            full_name,
            namespace: namespace.to_owned(),
            kind: SymbolKind::Def,
            type_params: info.type_params.clone(),
            label_detail: Some(callable_label_detail(&info.type_params, &info.signature)),
            detail: Some(format_function_detail("def", &info)),
        });
    }

    fn collect_proc(&mut self, namespace: &str, proc_def: &ProcessorDef) {
        let full_name = namespace_join(namespace, &proc_def.name);
        let info = ProcInfo {
            name: full_name.clone(),
            type_params: proc_def.type_params.clone(),
            ins: proc_port_names(&proc_def.ins, proc_def.ins_deferred_count.as_ref(), "in"),
            outs: proc_port_names(
                &proc_def.outs,
                proc_def.outs_deferred_count.as_ref(),
                proc_output_prefix(proc_def),
            ),
            params: proc_def
                .params
                .iter()
                .map(|decl| ProcParamInfo {
                    name: decl.name.clone(),
                    pinned: decl.pinned,
                    signature: format_proc_param_signature(decl),
                })
                .chain(proc_deferred_param_infos(
                    &proc_def.params,
                    proc_def.params_deferred_count.as_ref(),
                ))
                .collect(),
            events: proc_def.events.iter().map(event_info_from_def).collect(),
            buffers: proc_def
                .buffers
                .iter()
                .map(|decl| ProcBufferInfo {
                    name: decl.name.clone(),
                    signature: format_buffer_arg_signature(decl),
                })
                .chain(proc_deferred_buffer_infos(
                    &proc_def.buffers,
                    proc_def.buffers_deferred_count.as_ref(),
                ))
                .collect(),
            local_defs: proc_def
                .local_defs
                .iter()
                .map(function_info_from_def)
                .collect(),
        };
        self.procs.insert(full_name.clone(), info.clone());
        self.push_symbol(SymbolInfo {
            label: proc_def.name.clone(),
            full_name,
            namespace: namespace.to_owned(),
            kind: SymbolKind::Proc,
            type_params: info.type_params.clone(),
            label_detail: Some(callable_label_detail(
                &info.type_params,
                &proc_signature(&info),
            )),
            detail: Some(format_proc_detail(&info)),
        });
    }

    fn collect_struct(&mut self, namespace: &str, struct_def: &StructDef) {
        let full_name = namespace_join(namespace, &struct_def.name);
        let info = StructInfo {
            name: full_name.clone(),
            type_params: struct_def.type_params.clone(),
            fields: struct_def
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect(),
            methods: struct_def
                .methods
                .iter()
                .map(function_info_from_def)
                .collect(),
        };
        self.structs.insert(full_name.clone(), info.clone());
        self.push_symbol(SymbolInfo {
            label: struct_def.name.clone(),
            full_name,
            namespace: namespace.to_owned(),
            kind: SymbolKind::Struct,
            type_params: info.type_params.clone(),
            label_detail: generic_label_detail(&info.type_params),
            detail: Some(format_struct_detail(&info)),
        });
    }

    fn collect_namespace(&mut self, parent: &str, ns: &NamespaceDecl) {
        let full_name = namespace_join(parent, &ns.name);
        self.push_namespace(
            parent,
            &ns.name,
            &format_namespace_detail(&full_name, &ns.params),
            namespace_type_param_names(&ns.params),
        );
        for item in &ns.items {
            match item {
                NamespaceItem::Const(decl) => self.collect_const(&full_name, decl),
                NamespaceItem::Def(def) => self.collect_def(&full_name, def),
                NamespaceItem::Proc(proc_def) => self.collect_proc(&full_name, proc_def),
                NamespaceItem::Struct(struct_def) => self.collect_struct(&full_name, struct_def),
                NamespaceItem::Namespace(child) => self.collect_namespace(&full_name, child),
                NamespaceItem::Alias(alias) => self.collect_namespace_alias(&full_name, alias),
                NamespaceItem::Use(use_decl) => self.collect_use(&full_name, use_decl),
                NamespaceItem::Assert(_) => {}
            }
        }
    }

    fn collect_namespace_alias(&mut self, namespace: &str, alias: &NamespaceAliasDecl) {
        let full_name = namespace_join(namespace, &alias.name);
        let target = namespace_segments_key(&alias.target);
        self.namespace_aliases
            .insert(full_name.clone(), target.clone());
        self.push_namespace(namespace, &alias.name, "namespace alias", Vec::new());
    }

    fn collect_use(&mut self, namespace: &str, use_decl: &UseDecl) {
        self.uses.push(use_info_from_decl(namespace, use_decl));
    }

    fn push_namespace(&mut self, parent: &str, name: &str, detail: &str, type_params: Vec<String>) {
        let full_name = namespace_join(parent, name);
        let leaf = name.rsplit("::").next().unwrap_or(name).to_owned();
        let namespace_parent = namespace_parent_of(&full_name);
        self.namespaces_by_parent
            .entry(namespace_parent.clone())
            .or_default()
            .insert(leaf.clone());
        self.push_symbol(SymbolInfo {
            label: leaf,
            full_name,
            namespace: namespace_parent,
            kind: SymbolKind::Namespace,
            label_detail: generic_label_detail(&type_params),
            type_params,
            detail: Some(detail.to_owned()),
        });
    }

    fn push_symbol(&mut self, symbol: SymbolInfo) {
        self.symbols.push(symbol);
    }

    fn rebuild_namespace_member_maps(&mut self) {
        self.members_by_namespace.clear();
        self.namespaces_by_parent.clear();
        let mut namespace_paths = Vec::<String>::new();
        for symbol in &self.symbols {
            self.members_by_namespace
                .entry(symbol.namespace.clone())
                .or_default()
                .push(symbol.clone());
            if symbol.kind == SymbolKind::Namespace {
                namespace_paths.push(symbol.full_name.clone());
            }
        }
        for namespace in namespace_paths {
            self.insert_namespace_path(&namespace);
        }
    }

    fn insert_namespace_path(&mut self, full_name: &str) {
        let mut parent = String::new();
        for segment in full_name.split("::").filter(|segment| !segment.is_empty()) {
            self.namespaces_by_parent
                .entry(parent.clone())
                .or_default()
                .insert(segment.to_owned());
            parent = namespace_join(&parent, segment);
        }
    }

    fn instance_from_expr_in_namespace(
        &self,
        expr: &Expr,
        namespace: &str,
    ) -> Option<InstanceInfo> {
        match expr {
            Expr::UserCall { name, .. } => self
                .resolve_type_name_in_namespace(name, namespace)
                .map(|type_name| InstanceInfo {
                    type_name,
                    is_array: false,
                }),
            Expr::ArrayCtor { spec, .. } => match &spec.elem {
                ArrayElemType::Struct(name) => self
                    .resolve_type_name_in_namespace(name, namespace)
                    .map(|type_name| InstanceInfo {
                        type_name,
                        is_array: true,
                    }),
                ArrayElemType::Primitive(_) => None,
            },
            _ => None,
        }
    }

    fn collect_source_variables(&mut self, source: &str) {
        for line in source.lines() {
            let code = line.split('#').next().unwrap_or(line).trim_start();
            if code.is_empty() {
                continue;
            }
            if let Some(name) = source_assignment_target_name(code) {
                self.source_variables.insert(name.to_owned());
            }
            if let Some(name) = source_for_loop_var(code) {
                self.source_variables.insert(name.to_owned());
            }
        }
    }

    fn collect_scopes(&mut self, program: &Program) {
        let top_level_scope = self.collect_top_level_runtime_scope(&program.blocks);

        for block in &program.blocks {
            match block {
                Block::Def(def) => {
                    self.collect_function_scope(None, "", def, "def");
                }
                Block::Proc(proc_def) => {
                    self.collect_proc_scope(None, "", proc_def);
                }
                Block::Struct(struct_def) => {
                    self.collect_struct_scope(None, "", struct_def);
                }
                Block::Namespace(ns) => {
                    self.collect_namespace_scope(None, "", ns);
                }
                Block::Events(events) => {
                    if let Some(owner_idx) = top_level_scope {
                        for event in &events.events {
                            self.collect_event_scope(owner_idx, event);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_top_level_runtime_scope(&mut self, blocks: &[Block]) -> Option<usize> {
        let mut span = None::<Span>;
        let mut items = Vec::<CompletionItem>::new();
        let mut state_items = Vec::<CompletionItem>::new();
        let mut instances = HashMap::<String, InstanceInfo>::new();
        let mut stmt_regions = Vec::<(Span, &[Stmt])>::new();

        for block in blocks {
            if !self.span_belongs_to_current_file(block.loc().span()) {
                continue;
            }
            match block {
                Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
                    extend_span(&mut span, ports.loc);
                    for name in port_block_names(ports) {
                        items.push(port_item(&name, "port"));
                    }
                }
                Block::Params(params) => {
                    extend_span(&mut span, params.loc);
                    for name in param_block_names(params) {
                        items.push(param_item(&name, "param"));
                    }
                }
                Block::Buffers(buffers) => {
                    extend_span(&mut span, buffers.loc);
                    for name in buffer_block_names(buffers) {
                        items.push(port_item(&name, "buffer"));
                    }
                }
                Block::Events(events) => {
                    extend_span(&mut span, events.loc);
                    for event in &events.events {
                        items.push(event_item(event));
                    }
                }
                Block::Init(init) => {
                    extend_span(&mut span, init.loc);
                    collect_stmt_items(&init.body, "state", true, &mut state_items);
                    self.collect_stmt_scope_instances(&init.body, "", true, &mut instances);
                    stmt_regions.push((init.loc, init.body.as_slice()));
                }
                Block::Block(exec) => {
                    extend_span(&mut span, exec.loc);
                    collect_stmt_items(&exec.pre, "state", true, &mut state_items);
                    self.collect_stmt_scope_instances(&exec.pre, "", true, &mut instances);
                    if let Some(pre_span) = span_for_stmt_body(&exec.pre) {
                        stmt_regions.push((pre_span, exec.pre.as_slice()));
                    }
                    if let Some(sample) = &exec.sample {
                        stmt_regions.push((sample.loc, sample.body.as_slice()));
                    }
                    if let Some(post_span) = span_for_stmt_body(&exec.post) {
                        stmt_regions.push((post_span, exec.post.as_slice()));
                    }
                }
                Block::Sample(sample) => {
                    extend_span(&mut span, sample.loc);
                    stmt_regions.push((sample.loc, sample.body.as_slice()));
                }
                Block::Graph(graph) => {
                    extend_span(&mut span, graph.loc);
                }
                _ => {}
            }
        }

        items.extend(state_items);
        let owner_idx = self.push_scope(None, "", span?, items, instances)?;
        for (span, stmts) in stmt_regions {
            self.collect_stmt_scope(Some(owner_idx), span, stmts);
        }
        Some(owner_idx)
    }

    fn collect_proc_scope(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        proc_def: &ProcessorDef,
    ) -> Option<usize> {
        let mut items = Vec::<CompletionItem>::new();
        let mut instances = HashMap::<String, InstanceInfo>::new();
        for type_param in &proc_def.type_params {
            items.push(type_param_item(type_param));
        }
        for decl in &proc_def.consts {
            items.push(const_item(&decl.name));
        }
        for name in proc_port_names(&proc_def.ins, proc_def.ins_deferred_count.as_ref(), "in") {
            items.push(port_item(&name, "proc input"));
        }
        for name in proc_port_names(
            &proc_def.outs,
            proc_def.outs_deferred_count.as_ref(),
            proc_output_prefix(proc_def),
        ) {
            items.push(port_item(&name, "proc output"));
        }
        for decl in &proc_def.params {
            items.push(param_item(&decl.name, "proc param"));
        }
        for param in
            proc_deferred_param_infos(&proc_def.params, proc_def.params_deferred_count.as_ref())
        {
            items.push(param_item(&param.name, "proc param"));
        }
        for decl in &proc_def.buffers {
            items.push(port_item(&decl.name, "proc buffer"));
        }
        for buffer in
            proc_deferred_buffer_infos(&proc_def.buffers, proc_def.buffers_deferred_count.as_ref())
        {
            items.push(port_item(&buffer.name, "proc buffer"));
        }
        for event in &proc_def.events {
            items.push(event_item(event));
        }
        for def in &proc_def.local_defs {
            let info = function_info_from_def(def);
            items.push(function_item(
                &info,
                "proc-local def",
                COMPLETION_ITEM_KIND_FUNCTION,
            ));
        }
        collect_stmt_items(&proc_def.init.body, "state", true, &mut items);
        collect_stmt_items(&proc_def.block_pre, "state", true, &mut items);
        let scope_namespace = self.child_scope_namespace(parent, namespace);
        self.collect_stmt_scope_instances(
            &proc_def.init.body,
            &scope_namespace,
            true,
            &mut instances,
        );
        self.collect_stmt_scope_instances(
            &proc_def.block_pre,
            &scope_namespace,
            true,
            &mut instances,
        );
        let owner_idx = self.push_scope(
            parent,
            &scope_namespace,
            span_for_proc_scope(proc_def),
            items,
            instances,
        )?;
        self.collect_stmt_scope(
            Some(owner_idx),
            span_for_init_scope(&proc_def.init),
            &proc_def.init.body,
        );
        if let Some(span) = span_for_stmt_body(&proc_def.block_pre) {
            self.collect_stmt_scope(Some(owner_idx), span, &proc_def.block_pre);
        }
        if let Some(span) = span_for_stmt_body(&proc_def.sample) {
            self.collect_stmt_scope(Some(owner_idx), span, &proc_def.sample);
        }
        if let Some(span) = span_for_stmt_body(&proc_def.block_post) {
            self.collect_stmt_scope(Some(owner_idx), span, &proc_def.block_post);
        }
        for event in &proc_def.events {
            self.collect_event_scope(owner_idx, event);
        }
        for def in &proc_def.local_defs {
            self.collect_function_scope(Some(owner_idx), &scope_namespace, def, "proc-local def");
        }
        Some(owner_idx)
    }

    fn collect_struct_scope(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        struct_def: &StructDef,
    ) -> Option<usize> {
        let mut items = Vec::<CompletionItem>::new();
        for type_param in &struct_def.type_params {
            items.push(type_param_item(type_param));
        }
        for field in &struct_def.fields {
            items.push(
                CompletionItem::new(field.name.clone(), COMPLETION_ITEM_KIND_FIELD)
                    .detail("struct field")
                    .sort_text(format!("2_{}", field.name)),
            );
        }
        for method in &struct_def.methods {
            let info = function_info_from_def(method);
            items.push(function_item(&info, "method", COMPLETION_ITEM_KIND_METHOD));
        }
        let scope_namespace = self.child_scope_namespace(parent, namespace);
        let owner_idx = self.push_scope(
            parent,
            &scope_namespace,
            span_for_struct_scope(struct_def),
            items,
            HashMap::new(),
        )?;
        for method in &struct_def.methods {
            self.collect_function_scope(Some(owner_idx), &scope_namespace, method, "method");
        }
        Some(owner_idx)
    }

    fn collect_namespace_scope(
        &mut self,
        parent_idx: Option<usize>,
        parent_namespace: &str,
        ns: &NamespaceDecl,
    ) -> Option<usize> {
        let full_name = namespace_join(parent_namespace, &ns.name);
        let mut items = Vec::<CompletionItem>::new();
        for param in &ns.params {
            items.push(type_param_item(&param.name));
        }
        for item in &ns.items {
            match item {
                NamespaceItem::Const(decl) => items.push(const_item(&decl.name)),
                NamespaceItem::Def(def) => {
                    let info = function_info_from_def(def);
                    items.push(function_item(&info, "def", COMPLETION_ITEM_KIND_FUNCTION));
                }
                NamespaceItem::Proc(proc_def) => {
                    let info = proc_info_from_def(&proc_def.name, proc_def);
                    items.push(
                        CompletionItem::new(
                            proc_def.name.clone(),
                            COMPLETION_ITEM_KIND_CONSTRUCTOR,
                        )
                        .maybe_label_detail(Some(callable_label_detail(
                            &info.type_params,
                            &proc_signature(&info),
                        )))
                        .detail(format_proc_detail(&info))
                        .insert_text(callable_insert_text(&proc_def.name, &info.type_params))
                        .snippet(callable_snippet(&proc_def.name, &info.type_params))
                        .sort_text(format!("1_{}", proc_def.name)),
                    );
                }
                NamespaceItem::Struct(struct_def) => {
                    let info = struct_info_from_def(&struct_def.name, struct_def);
                    let mut item =
                        CompletionItem::new(struct_def.name.clone(), COMPLETION_ITEM_KIND_STRUCT)
                            .maybe_label_detail(generic_label_detail(&info.type_params))
                            .detail(format_struct_detail(&info))
                            .maybe_snippet(type_completion_snippet(
                                &struct_def.name,
                                &info.type_params,
                            ))
                            .sort_text(format!("1_{}", struct_def.name));
                    if let Some(insert_text) = type_insert_text(&struct_def.name, &info.type_params)
                    {
                        item = item.insert_text(insert_text);
                    }
                    items.push(item);
                }
                NamespaceItem::Namespace(child) => {
                    items.push(namespace_item_with_type_params(
                        &child.name,
                        &format_namespace_detail(&child.name, &child.params),
                        &namespace_type_param_names(&child.params),
                    ));
                }
                NamespaceItem::Alias(alias) => {
                    items.push(namespace_item(&alias.name));
                }
                NamespaceItem::Use(use_decl) => {
                    items.extend(self.items_for_use_decl(&full_name, use_decl));
                }
                NamespaceItem::Assert(_) => {}
            }
        }

        let ns_idx = self.push_scope(
            parent_idx,
            &full_name,
            span_for_namespace_scope(ns),
            items,
            HashMap::new(),
        )?;
        for item in &ns.items {
            match item {
                NamespaceItem::Def(def) => {
                    self.collect_function_scope(Some(ns_idx), &full_name, def, "def");
                }
                NamespaceItem::Proc(proc_def) => {
                    self.collect_proc_scope(Some(ns_idx), &full_name, proc_def);
                }
                NamespaceItem::Struct(struct_def) => {
                    self.collect_struct_scope(Some(ns_idx), &full_name, struct_def);
                }
                NamespaceItem::Namespace(child) => {
                    self.collect_namespace_scope(Some(ns_idx), &full_name, child);
                }
                NamespaceItem::Const(_)
                | NamespaceItem::Alias(_)
                | NamespaceItem::Use(_)
                | NamespaceItem::Assert(_) => {}
            }
        }
        Some(ns_idx)
    }

    fn collect_function_scope(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        def: &FunctionDef,
        detail: &str,
    ) -> Option<usize> {
        let mut items = Vec::<CompletionItem>::new();
        for type_param in &def.type_params {
            items.push(type_param_item(type_param));
        }
        for param in &def.params {
            items.push(param_item(&param.name, "argument"));
        }
        collect_stmt_items(&def.body, detail, false, &mut items);
        let mut instances = HashMap::<String, InstanceInfo>::new();
        let scope_namespace = self.child_scope_namespace(parent, namespace);
        self.collect_stmt_scope_instances(&def.body, &scope_namespace, false, &mut instances);
        let scope_idx = self.push_scope(
            parent,
            &scope_namespace,
            span_for_function_scope(def),
            items,
            instances,
        )?;
        self.collect_nested_stmt_scopes(scope_idx, &def.body);
        Some(scope_idx)
    }

    fn collect_event_scope(&mut self, parent: usize, event: &EventDef) -> Option<usize> {
        let mut items = Vec::<CompletionItem>::new();
        for param in &event.params {
            items.push(param_item(&param.name, "event parameter"));
        }
        collect_stmt_items(&event.body, "event local", false, &mut items);
        let mut instances = HashMap::<String, InstanceInfo>::new();
        let scope_namespace = self.child_scope_namespace(Some(parent), "");
        self.collect_stmt_scope_instances(&event.body, &scope_namespace, false, &mut instances);
        let scope_idx = self.push_scope(
            Some(parent),
            &scope_namespace,
            span_for_event_scope(event),
            items,
            instances,
        )?;
        self.collect_nested_stmt_scopes(scope_idx, &event.body);
        Some(scope_idx)
    }

    fn collect_stmt_scope(
        &mut self,
        parent: Option<usize>,
        span: Span,
        stmts: &[Stmt],
    ) -> Option<usize> {
        self.collect_stmt_scope_with_seed(parent, span, stmts, Vec::new(), HashMap::new())
    }

    fn collect_stmt_scope_with_seed(
        &mut self,
        parent: Option<usize>,
        span: Span,
        stmts: &[Stmt],
        mut items: Vec<CompletionItem>,
        mut instances: HashMap<String, InstanceInfo>,
    ) -> Option<usize> {
        collect_stmt_items(stmts, "local", false, &mut items);
        let scope_namespace = self.child_scope_namespace(parent, "");
        self.collect_stmt_scope_instances(stmts, &scope_namespace, false, &mut instances);
        let scope_idx = self.push_scope(parent, &scope_namespace, span, items, instances)?;
        self.collect_nested_stmt_scopes(scope_idx, stmts);
        Some(scope_idx)
    }

    fn collect_nested_stmt_scopes(&mut self, parent: usize, stmts: &[Stmt]) {
        for stmt in stmts {
            match stmt {
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    if let Some(span) = span_for_stmt_body(then_branch) {
                        self.collect_stmt_scope(Some(parent), span, then_branch);
                    }
                    if let Some(span) = span_for_stmt_body(else_branch) {
                        self.collect_stmt_scope(Some(parent), span, else_branch);
                    }
                }
                Stmt::For { var, body, .. } => {
                    if let Some(span) = span_for_stmt_body(body) {
                        self.collect_stmt_scope_with_seed(
                            Some(parent),
                            span,
                            body,
                            vec![variable_item(var, "loop variable")],
                            HashMap::new(),
                        );
                    }
                }
                Stmt::While { body, .. } => {
                    if let Some(span) = span_for_stmt_body(body) {
                        self.collect_stmt_scope(Some(parent), span, body);
                    }
                }
                Stmt::Const { .. }
                | Stmt::Assign { .. }
                | Stmt::Expr { .. }
                | Stmt::Return { .. }
                | Stmt::Break { .. }
                | Stmt::Continue { .. } => {}
            }
        }
    }

    fn child_scope_namespace(&self, parent: Option<usize>, fallback: &str) -> String {
        parent
            .and_then(|idx| self.scopes.get(idx))
            .map(|scope| scope.namespace.clone())
            .unwrap_or_else(|| fallback.to_owned())
    }

    fn collect_stmt_scope_instances(
        &self,
        stmts: &[Stmt],
        namespace: &str,
        top_level_assigns_only: bool,
        out: &mut HashMap<String, InstanceInfo>,
    ) {
        self.collect_visible_stmt_instances(stmts, namespace, top_level_assigns_only, out);
    }

    fn collect_visible_stmt_instances(
        &self,
        stmts: &[Stmt],
        namespace: &str,
        top_level_assigns_only: bool,
        out: &mut HashMap<String, InstanceInfo>,
    ) {
        for stmt in stmts {
            match stmt {
                Stmt::Assign { target, expr, .. } => {
                    if let Some(name) = assign_target_name(target) {
                        if let Some(instance) =
                            self.instance_from_expr_in_namespace(expr, namespace)
                        {
                            out.insert(name.to_owned(), instance);
                        }
                    }
                }
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    if !top_level_assigns_only {
                        let mut then_instances = out.clone();
                        self.collect_visible_stmt_instances(
                            then_branch,
                            namespace,
                            false,
                            &mut then_instances,
                        );
                        let mut else_instances = out.clone();
                        self.collect_visible_stmt_instances(
                            else_branch,
                            namespace,
                            false,
                            &mut else_instances,
                        );
                        let base_names = out.keys().cloned().collect::<BTreeSet<_>>();
                        for (name, instance) in then_instances {
                            if base_names.contains(&name) || else_instances.contains_key(&name) {
                                out.insert(name, instance);
                            }
                        }
                    }
                }
                Stmt::For { .. } | Stmt::While { .. } => {}
                Stmt::Const { .. }
                | Stmt::Expr { .. }
                | Stmt::Return { .. }
                | Stmt::Break { .. }
                | Stmt::Continue { .. } => {}
            }
        }
    }

    fn push_scope(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        span: Span,
        items: Vec<CompletionItem>,
        instances: HashMap<String, InstanceInfo>,
    ) -> Option<usize> {
        if span.is_zero() || !self.span_belongs_to_current_file(span) {
            return None;
        }
        let depth = parent.map(|idx| self.scopes[idx].depth + 1).unwrap_or(0);
        self.scopes.push(CompletionScope {
            parent,
            namespace: namespace.to_owned(),
            start_line: span.line.saturating_sub(1),
            start_column: u32::from(span.column.saturating_sub(1)),
            end_line: span.end_line().saturating_sub(1),
            end_column: u32::from(span.end_column.saturating_sub(1)),
            depth,
            items,
            instances,
        });
        Some(self.scopes.len() - 1)
    }

    fn span_belongs_to_current_file(&self, span: Span) -> bool {
        match &self.current_file_key {
            Some(expected) => span
                .file()
                .map(|file| normalize_file_key(&file) == *expected)
                .unwrap_or(false),
            None => true,
        }
    }

    fn scoped_items_at_position(&self) -> Vec<CompletionItem> {
        let mut out = Vec::new();
        let mut current = self.innermost_scope_index();
        let mut chain = Vec::new();
        while let Some(idx) = current {
            chain.push(idx);
            current = self.scopes[idx].parent;
        }
        chain.reverse();
        for idx in chain {
            let scope = &self.scopes[idx];
            out.extend(scope.items.iter().cloned());
        }
        out
    }

    fn innermost_scope_index(&self) -> Option<usize> {
        let mut best = None::<usize>;
        for (idx, scope) in self.scopes.iter().enumerate() {
            if !scope.contains(self.position.line, self.position.character) {
                continue;
            }
            let replace = match best {
                None => true,
                Some(best_idx) => {
                    scope.depth > self.scopes[best_idx].depth
                        || (scope.depth == self.scopes[best_idx].depth
                            && scope.is_narrower_than(&self.scopes[best_idx]))
                }
            };
            if replace {
                best = Some(idx);
            }
        }
        best
    }

    fn general_items(&self, prefix: &str) -> Vec<CompletionItem> {
        let mut out = Vec::new();
        for &keyword in LANGUAGE_KEYWORDS {
            out.push(
                CompletionItem::new(keyword, COMPLETION_ITEM_KIND_KEYWORD)
                    .detail("keyword")
                    .sort_text(format!("9_{keyword}")),
            );
        }
        for ty in language_type_names() {
            out.push(
                CompletionItem::new(ty, COMPLETION_ITEM_KIND_TYPE_PARAMETER)
                    .detail("type")
                    .sort_text(format!("1_{ty}")),
            );
        }
        for symbol in &self.symbols {
            if symbol.namespace.is_empty() && self.symbol_visible(symbol) {
                out.push(symbol_completion_item(symbol, false));
            }
        }
        for ns in self.namespaces_by_parent.get("").into_iter().flatten() {
            if self
                .symbols
                .iter()
                .any(|symbol| symbol.namespace.is_empty() && symbol.label == *ns)
            {
                continue;
            }
            out.push(
                CompletionItem::new(ns.clone(), COMPLETION_ITEM_KIND_MODULE)
                    .detail("namespace")
                    .sort_text(format!("0_{ns}")),
            );
        }
        for item in self.visible_use_items() {
            out.push(item);
        }
        out.extend(self.scoped_items_at_position());
        for name in &self.source_variables {
            out.push(
                CompletionItem::new(name.clone(), COMPLETION_ITEM_KIND_VARIABLE)
                    .detail("local")
                    .sort_text(format!("2_{name}")),
            );
        }
        if prefix.is_empty() {
            out
        } else {
            out.into_iter()
                .filter(|item| item.label.starts_with(prefix))
                .collect()
        }
    }

    fn namespace_items(&self, namespace: &str, prefix: &str) -> Vec<CompletionItem> {
        let namespace = self.resolve_namespace_key_at_current_position(namespace);
        let mut out = Vec::new();
        if let Some(names) = self.namespaces_by_parent.get(&namespace) {
            for name in names {
                if self
                    .members_by_namespace
                    .get(&namespace)
                    .into_iter()
                    .flatten()
                    .any(|symbol| symbol.label == *name)
                {
                    continue;
                }
                out.push(
                    CompletionItem::new(name.clone(), COMPLETION_ITEM_KIND_MODULE)
                        .detail("namespace")
                        .sort_text(format!("0_{name}")),
                );
            }
        }
        if let Some(members) = self.members_by_namespace.get(&namespace) {
            for symbol in members {
                if self.symbol_visible(symbol) {
                    out.push(symbol_completion_item(symbol, true));
                }
            }
        }
        out.into_iter()
            .filter(|item| prefix.is_empty() || item.label.starts_with(prefix))
            .collect()
    }

    fn member_items(&self, receiver: &str, prefix: &str) -> Vec<CompletionItem> {
        let root = receiver_root(receiver);
        let Some(instance) = self.resolve_instance(root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(proc_info) = self.procs.get(&instance.type_name) {
            for name in &proc_info.ins {
                out.push(
                    CompletionItem::new(name.clone(), COMPLETION_ITEM_KIND_PROPERTY)
                        .detail("proc input")
                        .sort_text(format!("1_{name}")),
                );
            }
            for name in &proc_info.outs {
                out.push(
                    CompletionItem::new(name.clone(), COMPLETION_ITEM_KIND_PROPERTY)
                        .detail("proc output")
                        .sort_text(format!("2_{name}")),
                );
            }
            for param in &proc_info.params {
                if !param.pinned {
                    out.push(
                        CompletionItem::new(param.name.clone(), COMPLETION_ITEM_KIND_PROPERTY)
                            .detail("proc param")
                            .sort_text(format!("3_{}", param.name)),
                    );
                }
            }
            if !proc_info.params.is_empty() && proc_info.params.iter().all(|param| !param.pinned) {
                out.push(
                    CompletionItem::new("params", COMPLETION_ITEM_KIND_PROPERTY)
                        .detail("dynamic proc params")
                        .sort_text("4_params"),
                );
            }
            for event in &proc_info.events {
                out.push(
                    CompletionItem::new(event.name.clone(), COMPLETION_ITEM_KIND_EVENT)
                        .maybe_label_detail(Some(format!("({})", event.signature)))
                        .detail(format!("event {}({})", event.name, event.signature))
                        .insert_text(format!("{}(", event.name))
                        .snippet(format!("{}($1)", event.name))
                        .sort_text(format!("5_{}", event.name)),
                );
            }
            out.push(
                CompletionItem::new("init", COMPLETION_ITEM_KIND_EVENT)
                    .maybe_label_detail(Some(format!("({})", proc_init_signature(proc_info))))
                    .detail(format!("event init({})", proc_init_signature(proc_info)))
                    .insert_text("init(")
                    .snippet("init($1)")
                    .sort_text("5_init"),
            );
            for def in &proc_info.local_defs {
                out.push(
                    CompletionItem::new(def.name.clone(), COMPLETION_ITEM_KIND_METHOD)
                        .maybe_label_detail(Some(callable_label_detail(
                            &def.type_params,
                            &def.signature,
                        )))
                        .detail(format_function_detail("proc-local def", def))
                        .insert_text(format!("{}(", def.name))
                        .snippet(format!("{}($1)", def.name))
                        .sort_text(format!("6_{}", def.name)),
                );
            }
        } else if let Some(struct_info) = self.structs.get(&instance.type_name) {
            for field in &struct_info.fields {
                out.push(
                    CompletionItem::new(field.clone(), COMPLETION_ITEM_KIND_FIELD)
                        .detail("struct field")
                        .sort_text(format!("1_{field}")),
                );
            }
            for method in &struct_info.methods {
                out.push(
                    CompletionItem::new(method.name.clone(), COMPLETION_ITEM_KIND_METHOD)
                        .maybe_label_detail(Some(callable_label_detail(
                            &method.type_params,
                            &method.signature,
                        )))
                        .detail(format_function_detail("method", method))
                        .insert_text(format!("{}(", method.name))
                        .snippet(format!("{}($1)", method.name))
                        .sort_text(format!("2_{}", method.name)),
                );
            }
        }
        if instance.is_array {
            out.push(
                CompletionItem::new(ARRAY_LEN_METHOD, COMPLETION_ITEM_KIND_METHOD)
                    .detail("built-in call .len()")
                    .insert_text(format!("{ARRAY_LEN_METHOD}()"))
                    .sort_text("0_len"),
            );
        }
        out.into_iter()
            .filter(|item| prefix.is_empty() || item.label.starts_with(prefix))
            .collect()
    }

    fn call_arg_items(&self, callee: &str) -> Vec<CompletionItem> {
        let callee = normalize_call_callee(callee);
        if let Some((receiver, member)) = split_member_callee(&callee) {
            if let Some(instance) = self.resolve_instance(receiver_root(receiver)) {
                if let Some(proc_info) = self.procs.get(&instance.type_name) {
                    if member == "init" {
                        return proc_info
                            .params
                            .iter()
                            .map(|param| named_arg_item(&param.name, "proc init param"))
                            .collect();
                    }
                    if let Some(event) = proc_info.events.iter().find(|event| event.name == member)
                    {
                        return event
                            .param_names
                            .iter()
                            .map(|param| named_arg_item(param, "event parameter"))
                            .collect();
                    }
                    if let Some(function) = proc_info
                        .local_defs
                        .iter()
                        .find(|function| function.name == member)
                    {
                        return function
                            .params
                            .iter()
                            .map(|param| named_arg_item(param, "argument"))
                            .collect();
                    }
                }
            }
        }
        if let Some(instance) = self.resolve_instance(receiver_root(&callee)) {
            if let Some(proc_info) = self.procs.get(&instance.type_name) {
                let mut out = Vec::new();
                for input in &proc_info.ins {
                    out.push(named_arg_item(input, "proc input"));
                }
                for param in &proc_info.params {
                    if !param.pinned {
                        out.push(named_arg_item(&param.name, "proc param"));
                    }
                }
                return out;
            }
        }
        if let Some(proc_name) = self.resolve_type_name(&callee) {
            if let Some(proc_info) = self.procs.get(&proc_name) {
                let mut out = Vec::new();
                for param in &proc_info.params {
                    out.push(named_arg_item(&param.name, "proc constructor param"));
                }
                for buffer in &proc_info.buffers {
                    out.push(named_arg_item(&buffer.name, "proc constructor buffer"));
                }
                return out;
            }
        }
        if let Some(function) = self.resolve_function(&callee) {
            return function
                .params
                .iter()
                .map(|param| named_arg_item(param, "argument"))
                .collect();
        }
        Vec::new()
    }

    fn resolve_instance(&self, name: &str) -> Option<&InstanceInfo> {
        let mut current = self.innermost_scope_index();
        while let Some(idx) = current {
            let scope = &self.scopes[idx];
            if let Some(instance) = scope.instances.get(name) {
                return Some(instance);
            }
            current = scope.parent;
        }
        self.instances.get(name)
    }

    fn items_for_use_decl(&self, namespace: &str, use_decl: &UseDecl) -> Vec<CompletionItem> {
        let use_info = use_info_from_decl(namespace, use_decl);
        let target = self.resolve_use_target(&use_info);
        if let Some(alias) = &use_decl.alias {
            if self.namespace_exists(&target) {
                return vec![
                    CompletionItem::new(alias.clone(), COMPLETION_ITEM_KIND_MODULE)
                        .detail(format!("namespace alias for {target}"))
                        .sort_text(format!("0_{alias}")),
                ];
            }
            if let Some(symbol) = self.symbol_by_full_name(&target) {
                let mut item = symbol_completion_item(symbol, false);
                item.label = alias.clone();
                item.insert_text = Some(alias.clone());
                item.detail = Some(format!(
                    "{} alias for {}",
                    symbol_kind_detail(symbol.kind),
                    symbol.full_name
                ));
                return vec![item];
            }
            return Vec::new();
        }

        if self.namespace_exists(&target) {
            return self
                .members_by_namespace
                .get(&target)
                .into_iter()
                .flatten()
                .filter(|symbol| {
                    symbol.kind != SymbolKind::Namespace && self.symbol_visible(symbol)
                })
                .map(|symbol| symbol_completion_item(symbol, false))
                .collect();
        }

        self.symbol_by_full_name(&target)
            .map(|symbol| vec![symbol_completion_item(symbol, false)])
            .unwrap_or_default()
    }

    fn visible_use_items(&self) -> Vec<CompletionItem> {
        let mut out = Vec::new();
        let current_namespace = self.current_namespace();
        for use_info in self.uses_in_namespace(current_namespace) {
            let target = self.resolve_use_target(use_info);
            if let Some(alias) = &use_info.alias {
                if self.namespace_exists(&target) {
                    out.push(
                        CompletionItem::new(alias.clone(), COMPLETION_ITEM_KIND_MODULE)
                            .detail(format!("namespace alias for {target}"))
                            .sort_text(format!("0_{alias}")),
                    );
                } else if let Some(symbol) = self.symbol_by_full_name(&target) {
                    let mut item = symbol_completion_item(symbol, false);
                    item.label = alias.clone();
                    item.insert_text = Some(alias.clone());
                    item.detail = Some(format!(
                        "{} alias for {}",
                        symbol_kind_detail(symbol.kind),
                        symbol.full_name
                    ));
                    out.push(item);
                }
                continue;
            }
            if self.namespace_exists(&target) {
                if let Some(members) = self.members_by_namespace.get(&target) {
                    for symbol in members {
                        if symbol.kind != SymbolKind::Namespace && self.symbol_visible(symbol) {
                            out.push(symbol_completion_item(symbol, false));
                        }
                    }
                }
            } else if let Some(symbol) = self.symbol_by_full_name(&target) {
                out.push(symbol_completion_item(symbol, false));
            }
        }
        out
    }

    fn symbol_visible(&self, symbol: &SymbolInfo) -> bool {
        if symbol.namespace.is_empty() {
            return true;
        }
        true
    }

    fn use_visible(&self, use_info: &UseInfo) -> bool {
        if use_info.public {
            return true;
        }
        match (&self.current_file_key, &use_info.file_key) {
            (Some(current), Some(file)) => current == file,
            (None, _) => true,
            _ => false,
        }
    }

    fn resolve_type_name(&self, name: &str) -> Option<String> {
        let namespace = self.current_namespace();
        self.resolve_type_name_in_namespace(name, namespace)
    }

    fn resolve_type_name_in_namespace(&self, name: &str, namespace: &str) -> Option<String> {
        let clean = strip_type_args_from_path(name);
        for candidate in self.qualified_name_candidates(&clean, namespace) {
            if self.procs.contains_key(&candidate) || self.structs.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        self.symbols.iter().find_map(|symbol| {
            if matches!(symbol.kind, SymbolKind::Proc | SymbolKind::Struct)
                && symbol.label == clean
                && self.symbol_visible(symbol)
            {
                Some(symbol.full_name.clone())
            } else {
                None
            }
        })
    }

    fn resolve_function(&self, name: &str) -> Option<&FunctionInfo> {
        let clean = strip_type_args_from_path(name);
        let namespace = self.current_namespace();
        self.qualified_name_candidates(&clean, namespace)
            .into_iter()
            .find_map(|candidate| self.functions.get(&candidate))
            .or_else(|| {
                self.symbols.iter().find_map(|symbol| {
                    if symbol.kind == SymbolKind::Def && symbol.label == clean {
                        self.functions.get(&symbol.full_name)
                    } else {
                        None
                    }
                })
            })
    }

    fn qualified_name_candidates(&self, name: &str, current_namespace: &str) -> Vec<String> {
        let clean = strip_type_args_from_path(name);
        if clean.is_empty() {
            return Vec::new();
        }

        let mut candidates = self.path_candidates_with_namespace_aliases(&clean, current_namespace);
        if let Some((head, tail)) = clean.split_once("::") {
            for use_info in self.uses_in_namespace(current_namespace) {
                if use_info.alias.as_deref() != Some(head) {
                    continue;
                }
                let target = self.resolve_use_target(use_info);
                if self.namespace_exists(&target) {
                    push_unique_candidate(&mut candidates, namespace_join(&target, tail));
                }
            }
        } else {
            for use_info in self.uses_in_namespace(current_namespace) {
                let target = self.resolve_use_target(use_info);
                match use_info.alias.as_deref() {
                    Some(alias) if alias == clean => {
                        push_unique_candidate(&mut candidates, target);
                    }
                    None => {
                        if self.namespace_exists(&target) {
                            let member = namespace_join(&target, &clean);
                            if self.symbol_by_full_name(&member).is_some() {
                                push_unique_candidate(&mut candidates, member);
                            }
                        }
                        if target.rsplit("::").next() == Some(clean.as_str())
                            && self.symbol_by_full_name(&target).is_some()
                        {
                            push_unique_candidate(&mut candidates, target);
                        }
                    }
                    _ => {}
                }
            }
        }
        candidates
    }

    fn uses_in_namespace<'a>(
        &'a self,
        current_namespace: &'a str,
    ) -> impl Iterator<Item = &'a UseInfo> + 'a {
        self.uses.iter().filter(move |use_info| {
            self.use_visible(use_info)
                && namespace_candidates(current_namespace)
                    .iter()
                    .any(|namespace| namespace == &use_info.namespace)
        })
    }

    fn resolve_use_target(&self, use_info: &UseInfo) -> String {
        for candidate in
            self.path_candidates_with_namespace_aliases(&use_info.target, &use_info.namespace)
        {
            if self.namespace_exists(&candidate) || self.symbol_by_full_name(&candidate).is_some() {
                return candidate;
            }
        }
        strip_type_args_from_path(&use_info.target)
    }

    fn resolve_namespace_key_at_current_position(&self, namespace: &str) -> String {
        let current_namespace = self.current_namespace();
        for candidate in self.namespace_key_candidates(namespace, current_namespace) {
            if self.namespace_exists(&candidate) {
                return candidate;
            }
        }
        strip_type_args_from_path(namespace)
    }

    fn current_namespace(&self) -> &str {
        self.innermost_scope_index()
            .and_then(|idx| self.scopes.get(idx))
            .map(|scope| scope.namespace.as_str())
            .unwrap_or("")
    }

    fn namespace_key_candidates(&self, namespace: &str, current_namespace: &str) -> Vec<String> {
        let clean = strip_type_args_from_path(namespace);
        let mut candidates = self.path_candidates_with_namespace_aliases(&clean, current_namespace);

        if let Some((head, tail)) = clean.split_once("::") {
            for use_info in self.uses_in_namespace(current_namespace) {
                if use_info.alias.as_deref() != Some(head) {
                    continue;
                }
                let target = self.resolve_use_target(use_info);
                if self.namespace_exists(&target) {
                    push_unique_candidate(&mut candidates, namespace_join(&target, tail));
                }
            }
        } else {
            for use_info in self.uses_in_namespace(current_namespace) {
                if use_info.alias.as_deref() != Some(clean.as_str()) {
                    continue;
                }
                let target = self.resolve_use_target(use_info);
                if self.namespace_exists(&target) {
                    push_unique_candidate(&mut candidates, target);
                }
            }
        }

        candidates
    }

    fn path_candidates_with_namespace_aliases(
        &self,
        path: &str,
        current_namespace: &str,
    ) -> Vec<String> {
        let clean = strip_type_args_from_path(path);
        if clean.is_empty() {
            return Vec::new();
        }

        let mut candidates = Vec::<String>::new();

        if let Some((head, tail)) = clean.split_once("::") {
            for candidate_namespace in namespace_candidates(current_namespace) {
                let head_candidate = namespace_join(&candidate_namespace, head);
                if let Some(alias_target) = self.namespace_aliases.get(&head_candidate) {
                    push_unique_candidate(&mut candidates, namespace_join(alias_target, tail));
                }
                push_unique_candidate(&mut candidates, namespace_join(&head_candidate, tail));
            }
        } else {
            for candidate_namespace in namespace_candidates(current_namespace) {
                let candidate = namespace_join(&candidate_namespace, &clean);
                if let Some(alias_target) = self.namespace_aliases.get(&candidate) {
                    push_unique_candidate(&mut candidates, alias_target.clone());
                }
                push_unique_candidate(&mut candidates, candidate);
            }
        }

        push_unique_candidate(&mut candidates, clean);
        candidates
    }

    fn namespace_exists(&self, namespace: &str) -> bool {
        self.members_by_namespace.contains_key(namespace)
            || self.namespaces_by_parent.contains_key(namespace)
            || self.namespace_aliases.contains_key(namespace)
    }

    fn symbol_by_full_name(&self, name: &str) -> Option<&SymbolInfo> {
        self.symbols.iter().find(|symbol| symbol.full_name == name)
    }
}

impl CompletionScope {
    fn contains(&self, line: u32, column: u32) -> bool {
        if line < self.start_line || line > self.end_line {
            return false;
        }
        if line == self.start_line && column < self.start_column {
            return false;
        }
        true
    }

    fn is_narrower_than(&self, other: &Self) -> bool {
        let self_line_span = self.end_line.saturating_sub(self.start_line);
        let other_line_span = other.end_line.saturating_sub(other.start_line);
        if self_line_span != other_line_span {
            return self_line_span < other_line_span;
        }
        let self_col_span = self.end_column.saturating_sub(self.start_column);
        let other_col_span = other.end_column.saturating_sub(other.start_column);
        self_col_span < other_col_span
    }
}

fn collect_stmt_items(
    stmts: &[Stmt],
    detail: &str,
    top_level_assigns_only: bool,
    out: &mut Vec<CompletionItem>,
) {
    collect_visible_stmt_items(stmts, detail, top_level_assigns_only, out);
}

fn collect_visible_stmt_items(
    stmts: &[Stmt],
    detail: &str,
    top_level_assigns_only: bool,
    out: &mut Vec<CompletionItem>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => {
                push_completion_item_once(out, const_item(&decl.name));
            }
            Stmt::Assign { target, .. } => {
                for name in assign_target_names(target) {
                    push_completion_item_once(out, variable_item(name, detail));
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                if !top_level_assigns_only {
                    let mut then_items = out.clone();
                    collect_visible_stmt_items(then_branch, detail, false, &mut then_items);
                    let mut else_items = out.clone();
                    collect_visible_stmt_items(else_branch, detail, false, &mut else_items);
                    let base_labels = out
                        .iter()
                        .map(|item| item.label.clone())
                        .collect::<BTreeSet<_>>();
                    let else_labels = else_items
                        .iter()
                        .map(|item| item.label.clone())
                        .collect::<BTreeSet<_>>();
                    for item in then_items {
                        if base_labels.contains(&item.label) || else_labels.contains(&item.label) {
                            push_completion_item_once(out, item);
                        }
                    }
                }
            }
            Stmt::For { .. } | Stmt::While { .. } => {}
            Stmt::Expr { .. }
            | Stmt::Return { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => {}
        }
    }
}

fn push_completion_item_once(out: &mut Vec<CompletionItem>, item: CompletionItem) {
    if !out.iter().any(|existing| existing.label == item.label) {
        out.push(item);
    }
}

fn assign_target_names(target: &onda_frontend::AssignTarget) -> Vec<&str> {
    match target {
        onda_frontend::AssignTarget::Var(name) => vec![name.as_str()],
        onda_frontend::AssignTarget::Tuple(names) => names.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

fn port_block_names(block: &PortBlock) -> Vec<String> {
    explicit_or_deferred_names(
        block.decls.iter().map(|decl| decl.name.as_str()),
        block.deferred_count.as_ref(),
        &block.deferred_prefix,
    )
}

fn param_block_names(block: &ParamBlock) -> Vec<String> {
    explicit_or_deferred_names(
        block.decls.iter().map(|decl| decl.name.as_str()),
        block.deferred_count.as_ref(),
        &block.deferred_prefix,
    )
}

fn buffer_block_names(block: &BufferBlock) -> Vec<String> {
    explicit_or_deferred_names(
        block.decls.iter().map(|decl| decl.name.as_str()),
        block.deferred_count.as_ref(),
        "buf",
    )
}

fn proc_port_names(decls: &[PortDecl], deferred_count: Option<&Expr>, prefix: &str) -> Vec<String> {
    explicit_or_deferred_names(
        decls.iter().map(|decl| decl.name.as_str()),
        deferred_count,
        prefix,
    )
}

fn proc_deferred_param_infos(
    explicit: &[ParamDecl],
    deferred_count: Option<&Expr>,
) -> Vec<ProcParamInfo> {
    if !explicit.is_empty() {
        return Vec::new();
    }
    deferred_count_names(deferred_count, "param")
        .into_iter()
        .map(|name| ProcParamInfo {
            signature: name.clone(),
            name,
            pinned: false,
        })
        .collect()
}

fn proc_deferred_buffer_infos(
    explicit: &[BufferDecl],
    deferred_count: Option<&Expr>,
) -> Vec<ProcBufferInfo> {
    if !explicit.is_empty() {
        return Vec::new();
    }
    deferred_count_names(deferred_count, "buf")
        .into_iter()
        .map(|name| ProcBufferInfo {
            signature: name.clone(),
            name,
        })
        .collect()
}

fn explicit_or_deferred_names<'a>(
    explicit: impl Iterator<Item = &'a str>,
    deferred_count: Option<&Expr>,
    prefix: &str,
) -> Vec<String> {
    let explicit = explicit.map(ToOwned::to_owned).collect::<Vec<_>>();
    if explicit.is_empty() {
        deferred_count_names(deferred_count, prefix)
    } else {
        explicit
    }
}

fn deferred_count_names(deferred_count: Option<&Expr>, prefix: &str) -> Vec<String> {
    let Some(Expr::Int { value, .. }) = deferred_count else {
        return Vec::new();
    };
    if *value <= 0 {
        return Vec::new();
    }
    let count = (*value as usize).min(MAX_DEFERRED_COUNT_COMPLETIONS);
    (1..=count).map(|idx| format!("{prefix}{idx}")).collect()
}

fn proc_output_prefix(proc_def: &ProcessorDef) -> &'static str {
    match proc_def.outs_timing {
        OutputTiming::Sample => "out",
        OutputTiming::Block => "kout",
    }
}

fn port_item(name: &str, detail: &str) -> CompletionItem {
    CompletionItem::new(name.to_owned(), COMPLETION_ITEM_KIND_PROPERTY)
        .detail(detail)
        .sort_text(format!("2_{name}"))
}

fn param_item(name: &str, detail: &str) -> CompletionItem {
    CompletionItem::new(name.to_owned(), COMPLETION_ITEM_KIND_PROPERTY)
        .detail(detail)
        .sort_text(format!("2_{name}"))
}

fn variable_item(name: &str, detail: &str) -> CompletionItem {
    CompletionItem::new(name.to_owned(), COMPLETION_ITEM_KIND_VARIABLE)
        .detail(detail)
        .sort_text(format!("3_{name}"))
}

fn const_item(name: &str) -> CompletionItem {
    CompletionItem::new(name.to_owned(), COMPLETION_ITEM_KIND_CONSTANT)
        .detail("const")
        .sort_text(format!("1_{name}"))
}

fn type_param_item(name: &str) -> CompletionItem {
    CompletionItem::new(name.to_owned(), COMPLETION_ITEM_KIND_TYPE_PARAMETER)
        .detail("type parameter")
        .sort_text(format!("1_{name}"))
}

fn namespace_item(name: &str) -> CompletionItem {
    namespace_item_with_type_params(name, "namespace", &[])
}

fn namespace_item_with_type_params(
    name: &str,
    detail: &str,
    type_params: &[String],
) -> CompletionItem {
    let mut item = CompletionItem::new(name.to_owned(), COMPLETION_ITEM_KIND_MODULE)
        .maybe_label_detail(generic_label_detail(type_params))
        .detail(detail)
        .maybe_snippet(type_completion_snippet(name, type_params))
        .sort_text(format!("0_{name}"));
    if let Some(insert_text) = type_insert_text(name, type_params) {
        item = item.insert_text(insert_text);
    }
    item
}

fn event_item(event: &EventDef) -> CompletionItem {
    CompletionItem::new(event.name.clone(), COMPLETION_ITEM_KIND_EVENT)
        .maybe_label_detail(Some(format!("({})", event_signature(event))))
        .detail(format!("event {}({})", event.name, event_signature(event)))
        .insert_text(format!("{}(", event.name))
        .snippet(format!("{}($1)", event.name))
        .sort_text(format!("4_{}", event.name))
}

fn function_item(info: &FunctionInfo, detail: &str, kind: u32) -> CompletionItem {
    CompletionItem::new(info.name.clone(), kind)
        .maybe_label_detail(Some(callable_label_detail(
            &info.type_params,
            &info.signature,
        )))
        .detail(format_function_detail(detail, info))
        .insert_text(callable_insert_text(&info.name, &info.type_params))
        .snippet(callable_snippet(&info.name, &info.type_params))
        .sort_text(format!("1_{}", info.name))
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

fn span_for_init_scope(init: &InitBlock) -> Span {
    span_for_stmt_body(&init.body)
        .map(|body_span| Span::spanning(init.loc, body_span))
        .unwrap_or(init.loc)
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
    span = Span::spanning(span, span_for_init_scope(&proc_def.init));
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

fn span_for_struct_scope(struct_def: &StructDef) -> Span {
    let mut span = struct_def.loc;
    for field in &struct_def.fields {
        span = Span::spanning(span, field.loc);
    }
    for method in &struct_def.methods {
        span = Span::spanning(span, span_for_function_scope(method));
    }
    span
}

fn span_for_namespace_scope(ns: &NamespaceDecl) -> Span {
    let mut span = ns.loc;
    for item in &ns.items {
        let item_span = match item {
            NamespaceItem::Assert(decl) => decl.loc,
            NamespaceItem::Const(decl) => decl.loc,
            NamespaceItem::Struct(decl) => span_for_struct_scope(decl),
            NamespaceItem::Def(decl) => span_for_function_scope(decl),
            NamespaceItem::Proc(decl) => span_for_proc_scope(decl),
            NamespaceItem::Namespace(decl) => span_for_namespace_scope(decl),
            NamespaceItem::Alias(decl) => decl.loc,
            NamespaceItem::Use(decl) => decl.loc,
        };
        span = Span::spanning(span, item_span);
    }
    span
}

fn extend_span(span: &mut Option<Span>, next: Span) {
    *span = Some(match *span {
        Some(current) => Span::spanning(current, next),
        None => next,
    });
}

fn symbol_completion_item(symbol: &SymbolInfo, qualified_member: bool) -> CompletionItem {
    let kind = match symbol.kind {
        SymbolKind::Const => COMPLETION_ITEM_KIND_CONSTANT,
        SymbolKind::Def => COMPLETION_ITEM_KIND_FUNCTION,
        SymbolKind::Proc => COMPLETION_ITEM_KIND_CONSTRUCTOR,
        SymbolKind::Struct => COMPLETION_ITEM_KIND_STRUCT,
        SymbolKind::Namespace => COMPLETION_ITEM_KIND_MODULE,
    };
    let mut item = CompletionItem::new(symbol.label.clone(), kind)
        .maybe_label_detail(symbol.label_detail.clone())
        .detail(
            symbol
                .detail
                .clone()
                .unwrap_or_else(|| symbol_kind_detail(symbol.kind).to_owned()),
        )
        .sort_text(format!("1_{}", symbol.label));
    if matches!(symbol.kind, SymbolKind::Def | SymbolKind::Proc) {
        item = item
            .insert_text(callable_insert_text(&symbol.label, &symbol.type_params))
            .snippet(callable_snippet(&symbol.label, &symbol.type_params));
    } else if matches!(symbol.kind, SymbolKind::Struct | SymbolKind::Namespace) {
        item = item.maybe_snippet(type_completion_snippet(&symbol.label, &symbol.type_params));
        if let Some(insert_text) = type_insert_text(&symbol.label, &symbol.type_params) {
            item = item.insert_text(insert_text);
        } else if qualified_member {
            item = item.insert_text(symbol.label.clone());
        }
    } else if qualified_member {
        item = item.insert_text(symbol.label.clone());
    }
    item
}

fn named_arg_item(name: &str, detail: &str) -> CompletionItem {
    CompletionItem::new(name.to_owned(), COMPLETION_ITEM_KIND_PROPERTY)
        .detail(detail)
        .insert_text(format!("{name} = "))
        .snippet(format!("{name} = $1"))
}

fn import_completion_items(path: Option<&Path>, typed: &str) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for module in stdlib_module_names() {
        if module.starts_with(typed) {
            items.push(
                CompletionItem::new(module, COMPLETION_ITEM_KIND_MODULE)
                    .detail("std module")
                    .insert_text(module)
                    .sort_text(format!("0_{module}")),
            );
        }
    }
    if let Some(path) = path.and_then(Path::parent) {
        let prefix_path = PathBuf::from(typed);
        let search_dir = if typed.ends_with('/') {
            path.join(&prefix_path)
        } else {
            path.join(prefix_path.parent().unwrap_or_else(|| Path::new("")))
        };
        if let Ok(entries) = fs::read_dir(search_dir) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                let Some(file_name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                    continue;
                };
                if file_name.starts_with('.') {
                    continue;
                }
                let rel = if typed.ends_with('/') {
                    format!("{typed}{file_name}")
                } else if let Some((dir, partial)) = typed.rsplit_once('/') {
                    if !file_name.starts_with(partial) {
                        continue;
                    }
                    format!("{dir}/{file_name}")
                } else {
                    if !file_name.starts_with(typed) {
                        continue;
                    }
                    file_name
                };
                if entry_path.is_dir() {
                    items.push(
                        CompletionItem::new(format!("{rel}/"), COMPLETION_ITEM_KIND_FILE)
                            .detail("folder")
                            .insert_text(format!("{rel}/"))
                            .sort_text(format!("1_{rel}/")),
                    );
                } else if matches!(
                    entry_path.extension().and_then(|ext| ext.to_str()),
                    Some("onda") | Some("on")
                ) {
                    let without_ext = rel
                        .strip_suffix(".onda")
                        .or_else(|| rel.strip_suffix(".on"))
                        .unwrap_or(&rel);
                    items.push(
                        CompletionItem::new(without_ext.to_owned(), COMPLETION_ITEM_KIND_FILE)
                            .detail("Onda module")
                            .insert_text(without_ext.to_owned())
                            .sort_text(format!("1_{without_ext}")),
                    );
                }
            }
        }
    }
    items
}

fn filter_and_encode(items: Vec<CompletionItem>, prefix: &str, snippets: bool) -> Vec<Value> {
    let mut dedup =
        BTreeMap::<(String, u32, Option<String>, Option<String>), CompletionItem>::new();
    for item in items {
        if !prefix.is_empty() && !item.label.starts_with(prefix) {
            continue;
        }
        let key = (
            item.label.clone(),
            item.kind,
            item.label_detail.clone(),
            item.detail.clone(),
        );
        dedup.entry(key).or_insert(item);
    }
    dedup
        .into_values()
        .map(|item| item.to_lsp(snippets))
        .collect()
}

fn event_info_from_def(event: &EventDef) -> EventInfo {
    EventInfo {
        name: event.name.clone(),
        param_names: event
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect(),
        signature: event_signature(event),
    }
}

fn function_info_from_def(def: &FunctionDef) -> FunctionInfo {
    FunctionInfo {
        name: def.name.clone(),
        type_params: def.type_params.clone(),
        params: def.params.iter().map(|param| param.name.clone()).collect(),
        signature: function_signature(def),
        is_const: def.is_const,
    }
}

fn proc_info_from_def(name: &str, proc_def: &ProcessorDef) -> ProcInfo {
    ProcInfo {
        name: name.to_owned(),
        type_params: proc_def.type_params.clone(),
        ins: proc_port_names(&proc_def.ins, proc_def.ins_deferred_count.as_ref(), "in"),
        outs: proc_port_names(
            &proc_def.outs,
            proc_def.outs_deferred_count.as_ref(),
            proc_output_prefix(proc_def),
        ),
        params: proc_def
            .params
            .iter()
            .map(|decl| ProcParamInfo {
                name: decl.name.clone(),
                pinned: decl.pinned,
                signature: format_proc_param_signature(decl),
            })
            .chain(proc_deferred_param_infos(
                &proc_def.params,
                proc_def.params_deferred_count.as_ref(),
            ))
            .collect(),
        events: proc_def.events.iter().map(event_info_from_def).collect(),
        buffers: proc_def
            .buffers
            .iter()
            .map(|decl| ProcBufferInfo {
                name: decl.name.clone(),
                signature: format_buffer_arg_signature(decl),
            })
            .chain(proc_deferred_buffer_infos(
                &proc_def.buffers,
                proc_def.buffers_deferred_count.as_ref(),
            ))
            .collect(),
        local_defs: proc_def
            .local_defs
            .iter()
            .map(function_info_from_def)
            .collect(),
    }
}

fn struct_info_from_def(name: &str, struct_def: &StructDef) -> StructInfo {
    StructInfo {
        name: name.to_owned(),
        type_params: struct_def.type_params.clone(),
        fields: struct_def
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect(),
        methods: struct_def
            .methods
            .iter()
            .map(function_info_from_def)
            .collect(),
    }
}

fn format_function_detail(kind: &str, info: &FunctionInfo) -> String {
    let const_prefix = if info.is_const { "const " } else { "" };
    let type_params = format_type_params(&info.type_params);
    format!(
        "{const_prefix}{kind} {}{type_params}({})",
        info.name, info.signature
    )
}

fn format_proc_detail(info: &ProcInfo) -> String {
    let type_params = format_type_params(&info.type_params);
    format!("proc {}{type_params}({})", info.name, proc_signature(info))
}

fn proc_signature(info: &ProcInfo) -> String {
    info.params
        .iter()
        .map(|param| param.signature.clone())
        .chain(info.buffers.iter().map(|buffer| buffer.signature.clone()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_struct_detail(info: &StructInfo) -> String {
    let type_params = format_type_params(&info.type_params);
    format!(
        "struct {}{type_params} {{ {} }}",
        info.name,
        info.fields.join(", ")
    )
}

fn format_namespace_detail(name: &str, params: &[NamespaceTemplateParam]) -> String {
    let names = params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    format!("namespace {}{}", name, format_type_params(&names))
}

fn generic_label_detail(type_params: &[String]) -> Option<String> {
    if type_params.is_empty() {
        None
    } else {
        Some(format_type_params(type_params))
    }
}

fn namespace_type_param_names(params: &[NamespaceTemplateParam]) -> Vec<String> {
    params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>()
}

fn format_type_params(type_params: &[String]) -> String {
    if type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", type_params.join(", "))
    }
}

fn callable_label_detail(type_params: &[String], signature: &str) -> String {
    format!("{}({signature})", format_type_params(type_params))
}

fn function_signature(def: &FunctionDef) -> String {
    def.params
        .iter()
        .map(format_fn_param_signature)
        .collect::<Vec<_>>()
        .join(", ")
}

fn event_signature(event: &EventDef) -> String {
    event
        .params
        .iter()
        .map(format_event_param_signature)
        .collect::<Vec<_>>()
        .join(", ")
}

fn proc_init_signature(info: &ProcInfo) -> String {
    info.params
        .iter()
        .map(|param| param.signature.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_fn_param_signature(param: &FnParamDecl) -> String {
    let mut text = param.name.clone();
    if let Some(ty) = &param.ty {
        text.push_str(": ");
        text.push_str(&format_fn_param_type(ty));
    }
    if let Some(default) = &param.default {
        text.push_str(" = ");
        text.push_str(&format_expr(default));
    }
    text
}

fn format_event_param_signature(param: &EventParamDecl) -> String {
    let mut text = param.name.clone();
    text.push_str(": ");
    text.push_str(&format_event_param_type(&param.ty));
    if let Some(default) = &param.default {
        text.push_str(" = ");
        text.push_str(&format_expr(default));
    }
    text
}

fn format_proc_param_signature(param: &ParamDecl) -> String {
    let mut text = String::new();
    if param.pinned {
        text.push_str("pin ");
    }
    text.push_str(&param.name);
    if let Some(ty) = &param.ty {
        text.push_str(": ");
        text.push_str(&format_decl_type(ty));
    }
    if let Some(default) = &param.default {
        text.push_str(" = ");
        text.push_str(&format_expr(default));
    }
    text
}

fn format_buffer_arg_signature(buffer: &BufferDecl) -> String {
    let mut text = buffer.name.clone();
    if let Some(ty) = &buffer.ty {
        text.push_str(": ");
        text.push_str(&format_buffer_type(ty));
    }
    text
}

fn callable_snippet(name: &str, type_params: &[String]) -> String {
    if let Some((suffix, next_tabstop)) = generic_snippet_suffix(type_params, 1) {
        format!("{name}{suffix}(${next_tabstop})")
    } else {
        format!("{name}($1)")
    }
}

fn callable_insert_text(name: &str, type_params: &[String]) -> String {
    format!("{name}{}(", format_type_params(type_params))
}

fn type_completion_snippet(name: &str, type_params: &[String]) -> Option<String> {
    generic_snippet_suffix(type_params, 1).map(|(suffix, _)| format!("{name}{suffix}"))
}

fn type_insert_text(name: &str, type_params: &[String]) -> Option<String> {
    if type_params.is_empty() {
        None
    } else {
        Some(format!("{name}{}", format_type_params(type_params)))
    }
}

fn generic_snippet_suffix(type_params: &[String], first_tabstop: usize) -> Option<(String, usize)> {
    if type_params.is_empty() {
        return None;
    }
    let parts = type_params
        .iter()
        .enumerate()
        .map(|(idx, param)| format!("${{{}:{}}}", first_tabstop + idx, param))
        .collect::<Vec<_>>();
    Some((
        format!("<{}>", parts.join(", ")),
        first_tabstop + type_params.len(),
    ))
}

fn symbol_kind_detail(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Const => "const",
        SymbolKind::Def => "def",
        SymbolKind::Proc => "proc",
        SymbolKind::Struct => "struct",
        SymbolKind::Namespace => "namespace",
    }
}

fn assign_target_name(target: &onda_frontend::AssignTarget) -> Option<&str> {
    match target {
        onda_frontend::AssignTarget::Var(name) => Some(name),
        _ => None,
    }
}

fn source_assignment_target_name(line: &str) -> Option<&str> {
    let (left, _) = line.split_once('=')?;
    let left = left.trim();
    if left.starts_with("if ")
        || left.starts_with("while ")
        || left.starts_with("for ")
        || left.starts_with("return ")
    {
        return None;
    }
    let name = left
        .split(':')
        .next()
        .unwrap_or(left)
        .trim()
        .split(['[', '.', ' '])
        .next()
        .unwrap_or_default();
    if is_identifier(name) {
        Some(name)
    } else {
        None
    }
}

fn source_for_loop_var(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("for ")?;
    let (name, _) = rest.split_once(" in ")?;
    let name = name.split('@').next().unwrap_or(name).trim();
    if is_identifier(name) {
        Some(name)
    } else {
        None
    }
}

fn namespace_segments_key(segments: &[NamespaceRefSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>()
        .join("::")
}

fn use_info_from_decl(namespace: &str, use_decl: &UseDecl) -> UseInfo {
    UseInfo {
        namespace: namespace.to_owned(),
        target: namespace_segments_key(&use_decl.target),
        alias: use_decl.alias.clone(),
        public: use_decl.public,
        file_key: file_key_for_span(use_decl.loc),
    }
}

fn namespace_join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else if child.is_empty() {
        parent.to_owned()
    } else {
        format!("{parent}::{child}")
    }
}

fn namespace_parent_of(name: &str) -> String {
    name.rsplit_once("::")
        .map(|(parent, _)| parent.to_owned())
        .unwrap_or_default()
}

fn namespace_parent(name: &str) -> Option<&str> {
    name.rsplit_once("::").map(|(parent, _)| parent)
}

fn namespace_candidates(current_namespace: &str) -> Vec<String> {
    if current_namespace.is_empty() {
        return vec![String::new()];
    }

    let mut out = Vec::<String>::new();
    let mut current = Some(current_namespace);
    while let Some(namespace) = current {
        out.push(namespace.to_owned());
        current = namespace_parent(namespace);
    }
    out.push(String::new());
    out
}

fn push_unique_candidate(candidates: &mut Vec<String>, candidate: String) {
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
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

fn normalize_call_callee(callee: &str) -> String {
    callee
        .trim()
        .strip_suffix('.')
        .unwrap_or(callee.trim())
        .to_owned()
}

fn receiver_root(receiver: &str) -> &str {
    let receiver = receiver.trim();
    let receiver = receiver.split('[').next().unwrap_or(receiver);
    receiver.rsplit('.').next().unwrap_or(receiver)
}

fn byte_offset_for_position(source: &str, position: CompletionPosition) -> usize {
    let mut offset = 0usize;
    for (line_no, line) in source.split_inclusive('\n').enumerate() {
        if line_no as u32 == position.line {
            let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
            return offset + byte_index_for_character(line_without_newline, position.character);
        }
        offset += line.len();
    }
    source.len()
}

fn byte_index_for_character(line: &str, character: u32) -> usize {
    line.char_indices()
        .nth(character as usize)
        .map(|(idx, _)| idx)
        .unwrap_or(line.len())
}

fn source_with_current_line_placeholder(source: &str, offset: usize) -> String {
    let offset = offset.min(source.len());
    let line_start = source[..offset].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let line_end = source[offset..]
        .find('\n')
        .map(|idx| offset + idx)
        .unwrap_or(source.len());
    let current_line = &source[line_start..line_end];
    let indent = current_line
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect::<String>();
    let mut sanitized = String::with_capacity(source.len());
    sanitized.push_str(&source[..line_start]);
    if indent.is_empty() {
        sanitized.push_str("const __onda_completion_placeholder = 0\n");
    } else {
        sanitized.push_str(&indent);
        sanitized.push_str("__onda_completion_placeholder = 0.0\n");
    }
    if line_end < source.len() {
        sanitized.push_str(&source[line_end + 1..]);
    }
    sanitized
}

fn identifier_prefix_start(source: &str, offset: usize) -> usize {
    let mut start = offset.min(source.len());
    while start > 0 {
        let Some((prev_idx, ch)) = source[..start].char_indices().last() else {
            break;
        };
        if is_ident_continue(ch) {
            start = prev_idx;
        } else {
            break;
        }
    }
    start
}

fn scan_receiver_left(source_before_dot: &str) -> Option<String> {
    let text = source_before_dot.trim_end();
    if text.is_empty() {
        return None;
    }
    if text.ends_with(']') {
        let base_end = matching_index_base_start(text)?;
        let base = &text[..base_end];
        return scan_namespace_left(base);
    }
    scan_namespace_left(text)
}

fn matching_index_base_start(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices().rev() {
        match ch {
            ']' => depth += 1,
            '[' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn scan_namespace_left(source_before_path_end: &str) -> Option<String> {
    let text = source_before_path_end.trim_end();
    let mut start = text.len();
    let mut angle_depth = 0usize;
    while start > 0 {
        let Some((idx, ch)) = text[..start].char_indices().last() else {
            break;
        };
        let allowed = is_ident_continue(ch)
            || ch == ':'
            || ch == '.'
            || ch == '<'
            || ch == '>'
            || (angle_depth > 0
                && (matches!(
                    ch,
                    ',' | '=' | ' ' | '\t' | '+' | '-' | '*' | '/' | '(' | ')'
                )));
        match ch {
            '>' => angle_depth += 1,
            '<' => angle_depth = angle_depth.saturating_sub(1),
            _ => {}
        }
        if !allowed {
            break;
        }
        start = idx;
    }
    let candidate = text[start..].trim();
    if candidate.is_empty() {
        None
    } else {
        Some(strip_type_args_from_path(candidate))
    }
}

fn split_member_callee(callee: &str) -> Option<(&str, &str)> {
    let mut angle_depth = 0usize;
    let mut bracket_depth = 0usize;
    for (idx, ch) in callee.char_indices().rev() {
        match ch {
            '>' => angle_depth += 1,
            '<' => angle_depth = angle_depth.saturating_sub(1),
            ']' => bracket_depth += 1,
            '[' => bracket_depth = bracket_depth.saturating_sub(1),
            '.' if angle_depth == 0 && bracket_depth == 0 => {
                let receiver = callee[..idx].trim();
                let member = callee[idx + 1..].trim();
                if !receiver.is_empty() && !member.is_empty() {
                    return Some((receiver, member));
                }
            }
            _ => {}
        }
    }
    None
}

#[derive(Debug, Clone)]
struct ActiveCall {
    callee: String,
    open_paren: usize,
}

fn active_call_at(source: &str, offset: usize) -> Option<ActiveCall> {
    let mut depth = 0isize;
    let mut paren_idx = None;
    for (idx, ch) in source[..offset.min(source.len())].char_indices().rev() {
        match ch {
            ')' => depth += 1,
            '(' => {
                if depth == 0 {
                    paren_idx = Some(idx);
                    break;
                }
                depth -= 1;
            }
            '\n' if depth == 0 => break,
            _ => {}
        }
    }
    let paren_idx = paren_idx?;
    let before = source[..paren_idx].trim_end();
    let callee = scan_namespace_left(before)?;
    if callee.is_empty() || is_reserved_word(&callee) {
        None
    } else {
        Some(ActiveCall {
            callee,
            open_paren: paren_idx,
        })
    }
}

fn current_call_arg_is_named_value(source: &str, open_paren: usize, offset: usize) -> bool {
    let start = open_paren.saturating_add(1).min(source.len());
    let end = offset.min(source.len());
    if start >= end {
        return false;
    }

    let mut has_top_level_equals = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for ch in source[start..end].chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '<' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                angle_depth += 1;
            }
            '>' if angle_depth > 0 => angle_depth -= 1,
            ',' if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && angle_depth == 0 =>
            {
                has_top_level_equals = false;
            }
            '=' if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && angle_depth == 0 =>
            {
                has_top_level_equals = true;
            }
            _ => {}
        }
    }

    has_top_level_equals
}

fn inside_string_literal(text: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            _ => {}
        }
    }
    in_string
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_') && chars.all(is_ident_continue)
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn file_key_for_span(span: Span) -> Option<String> {
    span.file().map(|file| normalize_file_key(&file))
}

fn normalize_file_key_for_path(path: &Path) -> String {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalize_file_key(&canonical.to_string_lossy())
}

fn normalize_file_key(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = normalized
        .strip_prefix("//?/")
        .or_else(|| normalized.strip_prefix("\\\\?\\"))
        .unwrap_or(&normalized);
    normalized.to_ascii_lowercase()
}
