use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use onda_frontend::{
    is_language_type_name, parse_program, parse_program_file_with_overlays, ArrayElemType,
    AssignTarget, Block, BlockExec, BufferDecl, ConstDecl, EventDef, EventParamDecl, Expr,
    FnParamDecl, FunctionDef, NamespaceAliasDecl, NamespaceDecl, NamespaceItem, ParamDecl,
    PortDecl, ProcessorDef, Program, Span, Stmt, StructDef, StructField, UseDecl,
};
use onda_semantics::builtins::{
    builtin_constant_type, builtin_instance_method_names, is_builtin_function_name,
    ARRAY_LEN_METHOD,
};
use serde_json::{json, Value};

use crate::formatting::{
    format_buffer_type, format_decl_type, format_event_param_type, format_expr,
    format_fn_param_type,
};

use super::namespace_resolution::{
    namespace_join, namespace_parent_of, namespace_segments_key,
    qualified_path_candidates as namespace_qualified_path_candidates,
    resolve_use_target as resolve_namespace_use_target, strip_type_args_from_path,
    visible_uses_in_namespace, AliasTargetPolicy, NamespaceAliasInfo, NamespaceResolutionContext,
    UseInfo,
};
use super::path_utils::path_to_file_uri;
use super::position::{
    byte_index_for_lsp_character, byte_offset_for_lsp_position, fallback_span_end_position,
    fallback_span_start_position, line_at_position, lsp_character_for_byte, span_end_position,
    span_start_position, LspPosition,
};

const SYMBOL_KIND_NAMESPACE: u32 = 3;
const SYMBOL_KIND_METHOD: u32 = 6;
const SYMBOL_KIND_FIELD: u32 = 8;
const SYMBOL_KIND_CONSTRUCTOR: u32 = 9;
const SYMBOL_KIND_FUNCTION: u32 = 12;
const SYMBOL_KIND_VARIABLE: u32 = 13;
const SYMBOL_KIND_CONSTANT: u32 = 14;
const SYMBOL_KIND_STRUCT: u32 = 23;
const SYMBOL_KIND_EVENT: u32 = 24;
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct NavigationPosition {
    pub(super) line: u32,
    pub(super) character: u32,
}

pub(super) fn hover_for_document_with_parsed(
    source: &str,
    path: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
    parsed: Option<&Program>,
    position: NavigationPosition,
) -> Option<Value> {
    let token = source_token_at_position(source, position)?;
    let parsed_owned;
    let parsed = if let Some(parsed) = parsed {
        Some(parsed)
    } else {
        parsed_owned = parse_for_navigation(source, path, overlays, Some(position));
        parsed_owned.as_ref()
    };
    let index = NavigationIndex::build(parsed, source, path, Some(position));
    let hover = index
        .callable_hover_for_token(source, &token)
        .or_else(|| {
            index
                .resolve_token(source, &token)
                .map(|definition| definition.hover_markdown())
        })
        .or_else(|| builtin_hover(&token.name))?;

    Some(json!({
        "contents": {
            "kind": "markdown",
            "value": hover,
        },
        "range": token.range_json(),
    }))
}

pub(super) fn definition_for_document_with_parsed(
    source: &str,
    path: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
    parsed: Option<&Program>,
    position: NavigationPosition,
) -> Option<Value> {
    let token = source_token_at_position(source, position)?;
    if let Some(location) = import_location_at_token(source, path, &token) {
        return Some(location);
    }

    let parsed_owned;
    let parsed = if let Some(parsed) = parsed {
        Some(parsed)
    } else {
        parsed_owned = parse_for_navigation(source, path, overlays, Some(position));
        parsed_owned.as_ref()
    };
    let index = NavigationIndex::build(parsed, source, path, Some(position));
    let definition = index.resolve_token(source, &token)?;
    definition.location_json(path, source, overlays)
}

pub(super) fn document_symbols_for_document_with_parsed(
    source: &str,
    path: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
    parsed: Option<&Program>,
) -> Vec<Value> {
    let parsed_owned;
    let program = if let Some(parsed) = parsed {
        parsed
    } else {
        parsed_owned = parse_for_navigation(source, path, overlays, None);
        let Some(program) = parsed_owned.as_ref() else {
            return Vec::new();
        };
        program
    };
    let current_file_key = path.map(normalize_file_key_for_path);
    document_symbols_for_program(program, current_file_key.as_deref(), source)
}

fn parse_for_navigation(
    source: &str,
    path: Option<&Path>,
    overlays: &HashMap<PathBuf, String>,
    position: Option<NavigationPosition>,
) -> Option<Program> {
    if let Some(path) = path {
        if let Ok(program) = parse_program_file_with_overlays(path, overlays) {
            return Some(program);
        }
        if let Some(position) = position {
            let offset = byte_offset_for_lsp_position(
                source,
                LspPosition::new(position.line, position.character),
            );
            let sanitized = source_with_current_line_placeholder(source, offset);
            let mut sanitized_overlays = overlays.clone();
            sanitized_overlays.insert(path.to_path_buf(), sanitized);
            if let Ok(program) = parse_program_file_with_overlays(path, &sanitized_overlays) {
                return Some(program);
            }
        }
    }
    parse_program(source).ok()
}

#[derive(Debug, Clone)]
struct SourceToken {
    name: String,
    line: u32,
    start_character: u32,
    end_character: u32,
    byte_start: usize,
    byte_end: usize,
    line_start: usize,
    line_end: usize,
}

impl SourceToken {
    fn range_json(&self) -> Value {
        json!({
            "start": {
                "line": self.line,
                "character": self.start_character,
            },
            "end": {
                "line": self.line,
                "character": self.end_character,
            },
        })
    }

    fn line_slice<'a>(&self, source: &'a str) -> &'a str {
        &source[self.line_start..self.line_end]
    }
}

#[derive(Debug, Clone)]
struct DefinitionInfo {
    name: String,
    full_name: String,
    kind: DefinitionKind,
    detail: String,
    span: Span,
    file_key: Option<String>,
    pinned: bool,
}

impl DefinitionInfo {
    fn hover_markdown(&self) -> String {
        let file = self
            .span
            .file()
            .map(|file| format!("\n\n`{file}`"))
            .unwrap_or_default();
        format!("```onda\n{}\n```{}", self.detail, file)
    }

    fn location_json(
        &self,
        fallback_path: Option<&Path>,
        fallback_source: &str,
        overlays: &HashMap<PathBuf, String>,
    ) -> Option<Value> {
        if self.span.is_zero() {
            return None;
        }
        let uri = uri_for_span(self.span, fallback_path)?;
        let source = source_for_span(self.span, fallback_path, fallback_source, overlays);
        Some(json!({
            "uri": uri,
            "range": span_range_json(self.span, source.as_deref()),
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefinitionKind {
    Const,
    Def,
    Proc,
    Struct,
    Namespace,
    NamespaceAlias,
    Port,
    Param,
    Buffer,
    Event,
    Field,
    Method,
    Variable,
    TypeParam,
    NamespaceParam,
}

#[derive(Debug, Clone, Default)]
struct ProcInfo {
    ins: HashMap<String, usize>,
    outs: HashMap<String, usize>,
    params: HashMap<String, usize>,
    init: Option<usize>,
    events: HashMap<String, usize>,
    buffers: HashMap<String, usize>,
    local_defs: HashMap<String, usize>,
    has_pinned_params: bool,
    call_signature: String,
}

#[derive(Debug, Clone, Default)]
struct StructInfo {
    fields: HashMap<String, usize>,
    methods: HashMap<String, usize>,
}

#[derive(Debug, Clone)]
struct InstanceInfo {
    type_name: String,
    is_array: bool,
}

#[derive(Debug, Clone)]
struct NavigationScope {
    parent: Option<usize>,
    namespace_member_scope: bool,
    namespace: String,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    depth: usize,
    definitions: HashMap<String, usize>,
    instances: HashMap<String, InstanceInfo>,
}

#[derive(Debug, Default)]
struct NavigationIndex {
    current_file_key: Option<String>,
    source: String,
    position: Option<NavigationPosition>,
    definitions: Vec<DefinitionInfo>,
    by_full_name: HashMap<String, usize>,
    namespace_members: HashMap<String, HashMap<String, usize>>,
    namespace_aliases: HashMap<String, NamespaceAliasInfo>,
    uses: Vec<UseInfo>,
    scopes: Vec<NavigationScope>,
    scopes_by_line: Vec<Vec<usize>>,
    procs: HashMap<String, ProcInfo>,
    structs: HashMap<String, StructInfo>,
    function_params: HashMap<String, HashMap<String, usize>>,
    event_params: HashMap<String, HashMap<String, usize>>,
    instances: HashMap<String, InstanceInfo>,
}

impl NavigationIndex {
    fn build(
        program: Option<&Program>,
        source: &str,
        path: Option<&Path>,
        position: Option<NavigationPosition>,
    ) -> Self {
        let mut index = Self {
            current_file_key: path.map(normalize_file_key_for_path),
            source: source.to_owned(),
            position,
            ..Self::default()
        };

        if let Some(program) = program {
            for block in &program.blocks {
                index.collect_block(block, "");
            }
            index.rebuild_namespace_members();
            index.collect_scopes(program);
            index.rebuild_scope_line_index();
        } else {
            index.collect_source_instances(source);
        }

        index
    }

    fn collect_block(&mut self, block: &Block, namespace: &str) {
        match block {
            Block::Const(decl) => {
                self.add_const_definition(namespace, decl);
            }
            Block::Def(def) => {
                self.add_function_definition(namespace, def, DefinitionKind::Def, "def");
            }
            Block::Proc(proc_def) => {
                self.add_proc_definition(namespace, proc_def);
            }
            Block::Struct(struct_def) => {
                self.add_struct_definition(namespace, struct_def);
            }
            Block::Namespace(ns) => {
                self.collect_namespace(namespace, ns);
            }
            Block::NamespaceAlias(alias) => {
                self.collect_namespace_alias(namespace, alias);
            }
            Block::Use(use_decl) => {
                self.collect_use(namespace, use_decl);
            }
            _ => {}
        }
    }

    fn collect_namespace(&mut self, parent: &str, ns: &NamespaceDecl) {
        let full_name = namespace_join(parent, &ns.name);
        let label = namespace_leaf(&ns.name);
        self.add_definition(DefinitionInfo {
            name: label.to_owned(),
            full_name: full_name.clone(),
            kind: DefinitionKind::Namespace,
            detail: format!("namespace {full_name}"),
            span: ns.loc,
            file_key: file_key_for_span(ns.loc),
            pinned: false,
        });
        for param in &ns.params {
            self.add_namespace_param_definition(&full_name, param, ns.loc);
        }

        for item in &ns.items {
            match item {
                NamespaceItem::Const(decl) => {
                    self.add_const_definition(&full_name, decl);
                }
                NamespaceItem::Def(def) => {
                    self.add_function_definition(&full_name, def, DefinitionKind::Def, "def");
                }
                NamespaceItem::Proc(proc_def) => {
                    self.add_proc_definition(&full_name, proc_def);
                }
                NamespaceItem::Struct(struct_def) => {
                    self.add_struct_definition(&full_name, struct_def);
                }
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
        self.namespace_aliases.insert(
            full_name.clone(),
            NamespaceAliasInfo {
                namespace: namespace.to_owned(),
                target: target.clone(),
            },
        );
        self.add_definition(DefinitionInfo {
            name: alias.name.clone(),
            full_name,
            kind: DefinitionKind::NamespaceAlias,
            detail: format!("namespace {} = {target}", alias.name),
            span: alias.loc,
            file_key: file_key_for_span(alias.loc),
            pinned: false,
        });
    }

    fn collect_use(&mut self, namespace: &str, use_decl: &UseDecl) {
        self.uses.push(UseInfo {
            namespace: namespace.to_owned(),
            target: namespace_segments_key(&use_decl.target),
            alias: use_decl.alias.clone(),
            public: use_decl.public,
            file_key: file_key_for_span(use_decl.loc),
        });
    }

    fn add_const_definition(&mut self, namespace: &str, decl: &ConstDecl) -> usize {
        let full_name = namespace_join(namespace, &decl.name);
        self.add_definition(DefinitionInfo {
            name: decl.name.clone(),
            full_name,
            kind: DefinitionKind::Const,
            detail: format!("const {}", decl.name),
            span: decl.loc,
            file_key: file_key_for_span(decl.loc),
            pinned: false,
        })
    }

    fn add_function_definition(
        &mut self,
        namespace: &str,
        def: &FunctionDef,
        kind: DefinitionKind,
        label: &str,
    ) -> usize {
        let full_name = namespace_join(namespace, &def.name);
        let const_prefix = if def.is_const { "const " } else { "" };
        let type_params = format_type_params(&def.type_params);
        let params = def
            .params
            .iter()
            .map(format_fn_param_signature)
            .collect::<Vec<_>>()
            .join(", ");
        let def_idx = self.add_definition(DefinitionInfo {
            name: def.name.clone(),
            full_name: full_name.clone(),
            kind,
            detail: format!("{const_prefix}{label} {}{type_params}({params})", def.name),
            span: def.loc,
            file_key: file_key_for_span(def.loc),
            pinned: false,
        });

        let mut param_indices = HashMap::new();
        for param in &def.params {
            let idx = self.add_fn_param_definition(&full_name, param);
            param_indices.entry(param.name.clone()).or_insert(idx);
        }
        self.function_params
            .entry(full_name)
            .or_default()
            .extend(param_indices);
        def_idx
    }

    fn add_proc_definition(&mut self, namespace: &str, proc_def: &ProcessorDef) -> usize {
        let full_name = namespace_join(namespace, &proc_def.name);
        let type_params = if proc_def.type_params.is_empty() {
            String::new()
        } else {
            format!("<{}>", proc_def.type_params.join(", "))
        };
        let args = proc_def
            .params
            .iter()
            .map(format_proc_param_signature)
            .chain(proc_def.buffers.iter().map(format_buffer_arg_signature))
            .collect::<Vec<_>>()
            .join(", ");
        let def_idx = self.add_definition(DefinitionInfo {
            name: proc_def.name.clone(),
            full_name: full_name.clone(),
            kind: DefinitionKind::Proc,
            detail: format!("proc {}{type_params}({args})", proc_def.name),
            span: proc_def.loc,
            file_key: file_key_for_span(proc_def.loc),
            pinned: false,
        });

        let mut info = ProcInfo {
            has_pinned_params: proc_def.params.iter().any(|param| param.pinned),
            call_signature: proc_call_signature(proc_def),
            ..ProcInfo::default()
        };
        let init_idx = self.add_definition(DefinitionInfo {
            name: "init".to_owned(),
            full_name: namespace_join(&full_name, "init"),
            kind: DefinitionKind::Event,
            detail: format!("event init({})", proc_init_signature(proc_def)),
            span: proc_def.loc,
            file_key: file_key_for_span(proc_def.loc),
            pinned: false,
        });
        info.init = Some(init_idx);
        for decl in &proc_def.ins {
            let idx =
                self.add_port_definition(&full_name, decl, DefinitionKind::Port, "proc input");
            info.ins.insert(decl.name.clone(), idx);
        }
        for decl in &proc_def.outs {
            let idx =
                self.add_port_definition(&full_name, decl, DefinitionKind::Port, "proc output");
            info.outs.insert(decl.name.clone(), idx);
        }
        for decl in &proc_def.params {
            let idx = self.add_param_definition(&full_name, decl, "proc param");
            info.params.insert(decl.name.clone(), idx);
        }
        for decl in &proc_def.buffers {
            let idx = self.add_buffer_definition(&full_name, decl, "proc buffer");
            info.buffers.insert(decl.name.clone(), idx);
        }
        for event in &proc_def.events {
            let idx = self.add_event_definition(&full_name, event);
            info.events.insert(event.name.clone(), idx);
        }
        for def in &proc_def.local_defs {
            let idx = self.add_function_definition(
                &full_name,
                def,
                DefinitionKind::Method,
                "proc-local def",
            );
            info.local_defs.insert(def.name.clone(), idx);
        }
        self.procs.insert(full_name, info);
        def_idx
    }

    fn add_struct_definition(&mut self, namespace: &str, struct_def: &StructDef) -> usize {
        let full_name = namespace_join(namespace, &struct_def.name);
        let fields = struct_def
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let def_idx = self.add_definition(DefinitionInfo {
            name: struct_def.name.clone(),
            full_name: full_name.clone(),
            kind: DefinitionKind::Struct,
            detail: format!("struct {} {{ {fields} }}", struct_def.name),
            span: struct_def.loc,
            file_key: file_key_for_span(struct_def.loc),
            pinned: false,
        });

        let mut info = StructInfo::default();
        for field in &struct_def.fields {
            let idx = self.add_struct_field_definition(&full_name, field);
            info.fields.insert(field.name.clone(), idx);
        }
        for method in &struct_def.methods {
            let idx =
                self.add_function_definition(&full_name, method, DefinitionKind::Method, "method");
            info.methods.insert(method.name.clone(), idx);
        }
        self.structs.insert(full_name, info);
        def_idx
    }

    fn add_port_definition(
        &mut self,
        owner: &str,
        decl: &PortDecl,
        kind: DefinitionKind,
        detail: &str,
    ) -> usize {
        self.add_definition(DefinitionInfo {
            name: decl.name.clone(),
            full_name: namespace_join(owner, &decl.name),
            kind,
            detail: format!("{detail} {}", decl.name),
            span: decl.loc,
            file_key: file_key_for_span(decl.loc),
            pinned: false,
        })
    }

    fn add_param_definition(&mut self, owner: &str, decl: &ParamDecl, detail: &str) -> usize {
        let pin = if decl.pinned { "pin " } else { "" };
        self.add_definition(DefinitionInfo {
            name: decl.name.clone(),
            full_name: namespace_join(owner, &decl.name),
            kind: DefinitionKind::Param,
            detail: format!("{detail} {pin}{}", decl.name),
            span: decl.loc,
            file_key: file_key_for_span(decl.loc),
            pinned: decl.pinned,
        })
    }

    fn add_fn_param_definition(&mut self, owner: &str, decl: &FnParamDecl) -> usize {
        self.add_definition_once(DefinitionInfo {
            name: decl.name.clone(),
            full_name: namespace_join(owner, &decl.name),
            kind: DefinitionKind::Param,
            detail: format!("argument {}", decl.name),
            span: decl.loc,
            file_key: file_key_for_span(decl.loc),
            pinned: false,
        })
    }

    fn add_event_param_definition(&mut self, owner: &str, decl: &EventParamDecl) -> usize {
        self.add_definition_once(DefinitionInfo {
            name: decl.name.clone(),
            full_name: namespace_join(owner, &decl.name),
            kind: DefinitionKind::Param,
            detail: format!("event parameter {}", decl.name),
            span: decl.loc,
            file_key: file_key_for_span(decl.loc),
            pinned: false,
        })
    }

    fn add_buffer_definition(&mut self, owner: &str, decl: &BufferDecl, detail: &str) -> usize {
        self.add_definition(DefinitionInfo {
            name: decl.name.clone(),
            full_name: namespace_join(owner, &decl.name),
            kind: DefinitionKind::Buffer,
            detail: format!("{detail} {}", decl.name),
            span: decl.loc,
            file_key: file_key_for_span(decl.loc),
            pinned: false,
        })
    }

    fn add_event_definition(&mut self, owner: &str, event: &EventDef) -> usize {
        let params = event
            .params
            .iter()
            .map(format_event_param_signature)
            .collect::<Vec<_>>()
            .join(", ");
        let event_owner = namespace_join(owner, &event.name);
        let event_idx = self.add_definition(DefinitionInfo {
            name: event.name.clone(),
            full_name: event_owner.clone(),
            kind: DefinitionKind::Event,
            detail: format!("event {}({params})", event.name),
            span: event.loc,
            file_key: file_key_for_span(event.loc),
            pinned: false,
        });

        let mut param_indices = HashMap::new();
        for param in &event.params {
            let idx = self.add_event_param_definition(&event_owner, param);
            param_indices.entry(param.name.clone()).or_insert(idx);
        }
        self.event_params
            .entry(event_owner)
            .or_default()
            .extend(param_indices);
        event_idx
    }

    fn add_struct_field_definition(&mut self, owner: &str, field: &StructField) -> usize {
        self.add_definition(DefinitionInfo {
            name: field.name.clone(),
            full_name: namespace_join(owner, &field.name),
            kind: DefinitionKind::Field,
            detail: format!("field {}", field.name),
            span: field.loc,
            file_key: file_key_for_span(field.loc),
            pinned: false,
        })
    }

    fn add_local_variable_definition(&mut self, owner: &str, name: &str, span: Span) -> usize {
        self.add_definition(DefinitionInfo {
            name: name.to_owned(),
            full_name: namespace_join(owner, name),
            kind: DefinitionKind::Variable,
            detail: format!("local {name}"),
            span,
            file_key: file_key_for_span(span),
            pinned: false,
        })
    }

    fn add_type_param_definition(&mut self, owner: &str, name: &str, span: Span) -> usize {
        self.add_definition(DefinitionInfo {
            name: name.to_owned(),
            full_name: namespace_join(owner, name),
            kind: DefinitionKind::TypeParam,
            detail: format!("type parameter {name}"),
            span,
            file_key: file_key_for_span(span),
            pinned: false,
        })
    }

    fn add_namespace_param_definition(
        &mut self,
        owner: &str,
        param: &onda_frontend::NamespaceTemplateParam,
        span: Span,
    ) -> usize {
        self.add_definition_once(DefinitionInfo {
            name: param.name.clone(),
            full_name: namespace_join(owner, &param.name),
            kind: DefinitionKind::NamespaceParam,
            detail: format!("namespace parameter {}", param.name),
            span,
            file_key: file_key_for_span(span),
            pinned: false,
        })
    }

    fn add_definition(&mut self, definition: DefinitionInfo) -> usize {
        let idx = self.definitions.len();
        self.by_full_name
            .entry(definition.full_name.clone())
            .or_insert(idx);
        self.definitions.push(definition);
        idx
    }

    fn add_definition_once(&mut self, definition: DefinitionInfo) -> usize {
        if let Some(idx) = self.by_full_name.get(&definition.full_name).copied() {
            return idx;
        }
        self.add_definition(definition)
    }

    fn rebuild_namespace_members(&mut self) {
        self.namespace_members.clear();
        for (idx, definition) in self.definitions.iter().enumerate() {
            if !matches!(
                definition.kind,
                DefinitionKind::Const
                    | DefinitionKind::Def
                    | DefinitionKind::Proc
                    | DefinitionKind::Struct
                    | DefinitionKind::Namespace
                    | DefinitionKind::NamespaceAlias
            ) {
                continue;
            }
            let parent = namespace_parent_of(&definition.full_name);
            self.namespace_members
                .entry(parent)
                .or_default()
                .entry(definition.name.clone())
                .or_insert(idx);
        }
    }

    fn instance_from_expr(&self, expr: &Expr, namespace: &str) -> Option<InstanceInfo> {
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

    fn collect_source_instances(&mut self, source: &str) {
        for line in source.lines() {
            let code = line.split('#').next().unwrap_or(line).trim();
            let Some((target, rhs)) = code.split_once('=') else {
                continue;
            };
            let target = target.split(':').next().unwrap_or(target).trim();
            let Some(type_name) = rhs
                .trim()
                .strip_suffix("()")
                .filter(|name| is_namespace_path(name))
            else {
                continue;
            };
            self.instances.insert(
                target.to_owned(),
                InstanceInfo {
                    type_name: strip_type_args_from_path(type_name),
                    is_array: false,
                },
            );
        }
    }

    fn collect_scopes(&mut self, program: &Program) {
        let top_level_scope = self.collect_top_level_runtime_scope(&program.blocks);
        for block in &program.blocks {
            if !self.block_belongs_to_current_file(block) {
                continue;
            }
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
                            self.collect_event_scope(owner_idx, "", event);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_top_level_runtime_scope(&mut self, blocks: &[Block]) -> Option<usize> {
        let mut span = None::<Span>;
        let mut definitions = HashMap::<String, usize>::new();
        let mut instances = HashMap::<String, InstanceInfo>::new();
        let mut stmt_regions = Vec::<(Span, &[Stmt], bool)>::new();

        for block in blocks {
            if !self.block_belongs_to_current_file(block) {
                continue;
            }
            match block {
                Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
                    extend_span(&mut span, ports.loc);
                    for decl in &ports.decls {
                        let idx = self.add_port_definition("", decl, DefinitionKind::Port, "port");
                        definitions.insert(decl.name.clone(), idx);
                    }
                }
                Block::Params(params) => {
                    extend_span(&mut span, params.loc);
                    for decl in &params.decls {
                        let idx = self.add_param_definition("", decl, "param");
                        definitions.insert(decl.name.clone(), idx);
                    }
                }
                Block::Buffers(buffers) => {
                    extend_span(&mut span, buffers.loc);
                    for decl in &buffers.decls {
                        let idx = self.add_buffer_definition("", decl, "buffer");
                        definitions.insert(decl.name.clone(), idx);
                    }
                }
                Block::Events(events) => {
                    extend_span(&mut span, events.loc);
                    for event in &events.events {
                        let idx = self.add_event_definition("", event);
                        definitions.insert(event.name.clone(), idx);
                    }
                }
                Block::Init(init) => {
                    extend_span(&mut span, init.loc);
                    stmt_regions.push((init.loc, init.body.as_slice(), true));
                }
                Block::Block(exec) => {
                    extend_span(&mut span, exec.loc);
                    for region in runtime_regions_for_block_exec(exec) {
                        stmt_regions.push(region);
                    }
                }
                Block::Sample(sample) => {
                    extend_span(&mut span, sample.loc);
                    stmt_regions.push((sample.loc, sample.body.as_slice(), false));
                }
                Block::Graph(graph) => {
                    extend_span(&mut span, graph.loc);
                }
                _ => {}
            }
        }

        for (_, stmts, state_only) in &stmt_regions {
            self.collect_stmt_definitions("", stmts, *state_only, &mut definitions);
            self.collect_stmt_scope_instances(stmts, "", *state_only, &mut instances);
        }
        let owner_idx = self.push_scope(None, "", span?, definitions, instances)?;
        for (span, stmts, _) in stmt_regions {
            self.collect_stmt_scope(Some(owner_idx), "", span, stmts);
        }
        Some(owner_idx)
    }

    fn collect_proc_scope(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        proc_def: &ProcessorDef,
    ) -> Option<usize> {
        let owner = namespace_join(namespace, &proc_def.name);
        let mut definitions = HashMap::<String, usize>::new();
        let mut instances = HashMap::<String, InstanceInfo>::new();
        for type_param in &proc_def.type_params {
            let idx = self.add_type_param_definition(&owner, type_param, proc_def.loc);
            definitions.insert(type_param.clone(), idx);
        }
        for decl in &proc_def.consts {
            let idx = self.add_const_definition(&owner, decl);
            definitions.insert(decl.name.clone(), idx);
        }
        for decl in &proc_def.ins {
            let idx = self.add_port_definition(&owner, decl, DefinitionKind::Port, "proc input");
            definitions.insert(decl.name.clone(), idx);
        }
        for decl in &proc_def.outs {
            let idx = self.add_port_definition(&owner, decl, DefinitionKind::Port, "proc output");
            definitions.insert(decl.name.clone(), idx);
        }
        for decl in &proc_def.params {
            let idx = self.add_param_definition(&owner, decl, "proc param");
            definitions.insert(decl.name.clone(), idx);
        }
        for decl in &proc_def.buffers {
            let idx = self.add_buffer_definition(&owner, decl, "proc buffer");
            definitions.insert(decl.name.clone(), idx);
        }
        for event in &proc_def.events {
            let idx = self.add_event_definition(&owner, event);
            definitions.insert(event.name.clone(), idx);
        }
        for def in &proc_def.local_defs {
            let idx =
                self.add_function_definition(&owner, def, DefinitionKind::Method, "proc-local def");
            definitions.insert(def.name.clone(), idx);
        }
        self.collect_stmt_definitions(&owner, &proc_def.init.body, true, &mut definitions);
        self.collect_stmt_definitions(&owner, &proc_def.block_pre, true, &mut definitions);
        self.collect_stmt_scope_instances(&proc_def.init.body, namespace, true, &mut instances);
        self.collect_stmt_scope_instances(&proc_def.block_pre, namespace, true, &mut instances);

        let owner_idx = self.push_scope(
            parent,
            namespace,
            span_for_proc_scope(proc_def),
            definitions,
            instances,
        )?;
        self.collect_stmt_scope(
            Some(owner_idx),
            &owner,
            proc_def.init.loc,
            &proc_def.init.body,
        );
        if let Some(span) = span_for_stmt_body(&proc_def.block_pre) {
            self.collect_stmt_scope(Some(owner_idx), &owner, span, &proc_def.block_pre);
        }
        if let Some(span) = span_for_stmt_body(&proc_def.sample) {
            self.collect_stmt_scope(Some(owner_idx), &owner, span, &proc_def.sample);
        }
        if let Some(span) = span_for_stmt_body(&proc_def.block_post) {
            self.collect_stmt_scope(Some(owner_idx), &owner, span, &proc_def.block_post);
        }
        for event in &proc_def.events {
            self.collect_event_scope(owner_idx, &owner, event);
        }
        for def in &proc_def.local_defs {
            self.collect_function_scope(Some(owner_idx), &owner, def, "proc-local def");
        }
        Some(owner_idx)
    }

    fn collect_struct_scope(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        struct_def: &StructDef,
    ) -> Option<usize> {
        let owner = namespace_join(namespace, &struct_def.name);
        let mut definitions = HashMap::<String, usize>::new();
        for type_param in &struct_def.type_params {
            let idx = self.add_type_param_definition(&owner, type_param, struct_def.loc);
            definitions.insert(type_param.clone(), idx);
        }
        for field in &struct_def.fields {
            let idx = self.add_struct_field_definition(&owner, field);
            definitions.insert(field.name.clone(), idx);
        }
        for method in &struct_def.methods {
            let idx =
                self.add_function_definition(&owner, method, DefinitionKind::Method, "method");
            definitions.insert(method.name.clone(), idx);
        }
        let owner_idx = self.push_scope(
            parent,
            namespace,
            span_for_struct_scope(struct_def),
            definitions,
            HashMap::new(),
        )?;
        for method in &struct_def.methods {
            self.collect_function_scope(Some(owner_idx), &owner, method, "method");
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
        let mut definitions = HashMap::<String, usize>::new();
        for param in &ns.params {
            let idx = self.add_namespace_param_definition(&full_name, param, ns.loc);
            definitions.insert(param.name.clone(), idx);
        }
        for item in &ns.items {
            match item {
                NamespaceItem::Const(decl) => {
                    if let Some(idx) = self
                        .by_full_name
                        .get(&namespace_join(&full_name, &decl.name))
                    {
                        definitions.insert(decl.name.clone(), *idx);
                    }
                }
                NamespaceItem::Def(def) => {
                    if let Some(idx) = self
                        .by_full_name
                        .get(&namespace_join(&full_name, &def.name))
                    {
                        definitions.insert(def.name.clone(), *idx);
                    }
                }
                NamespaceItem::Proc(proc_def) => {
                    if let Some(idx) = self
                        .by_full_name
                        .get(&namespace_join(&full_name, &proc_def.name))
                    {
                        definitions.insert(proc_def.name.clone(), *idx);
                    }
                }
                NamespaceItem::Struct(struct_def) => {
                    if let Some(idx) = self
                        .by_full_name
                        .get(&namespace_join(&full_name, &struct_def.name))
                    {
                        definitions.insert(struct_def.name.clone(), *idx);
                    }
                }
                NamespaceItem::Namespace(child) => {
                    if let Some(idx) = self
                        .by_full_name
                        .get(&namespace_join(&full_name, &child.name))
                    {
                        definitions.insert(namespace_leaf(&child.name).to_owned(), *idx);
                    }
                }
                NamespaceItem::Alias(alias) => {
                    if let Some(idx) = self
                        .by_full_name
                        .get(&namespace_join(&full_name, &alias.name))
                    {
                        definitions.insert(alias.name.clone(), *idx);
                    }
                }
                NamespaceItem::Use(use_decl) => {
                    for (name, idx) in self.definition_indices_for_use_decl(&full_name, use_decl) {
                        definitions.entry(name).or_insert(idx);
                    }
                }
                NamespaceItem::Assert(_) => {}
            }
        }

        let ns_idx = self.push_namespace_scope(
            parent_idx,
            &full_name,
            span_for_namespace_scope(ns),
            definitions,
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
        owner: &str,
        def: &FunctionDef,
        _detail: &str,
    ) -> Option<usize> {
        let function_owner = namespace_join(owner, &def.name);
        let mut definitions = HashMap::<String, usize>::new();
        for type_param in &def.type_params {
            let idx = self.add_type_param_definition(&function_owner, type_param, def.loc);
            definitions.insert(type_param.clone(), idx);
        }
        for param in &def.params {
            let idx = self.add_fn_param_definition(&function_owner, param);
            definitions.insert(param.name.clone(), idx);
        }
        self.collect_stmt_definitions(&function_owner, &def.body, false, &mut definitions);
        let mut instances = HashMap::<String, InstanceInfo>::new();
        let scope_namespace = self.child_scope_namespace(parent, owner);
        self.collect_stmt_scope_instances(&def.body, &scope_namespace, false, &mut instances);
        let scope_idx = self.push_scope(
            parent,
            &scope_namespace,
            span_for_function_scope(def),
            definitions,
            instances,
        )?;
        self.collect_nested_stmt_scopes(scope_idx, &function_owner, &def.body);
        Some(scope_idx)
    }

    fn collect_event_scope(
        &mut self,
        parent: usize,
        owner: &str,
        event: &EventDef,
    ) -> Option<usize> {
        let event_owner = namespace_join(owner, &event.name);
        let mut definitions = HashMap::<String, usize>::new();
        for param in &event.params {
            let idx = self.add_event_param_definition(&event_owner, param);
            definitions.insert(param.name.clone(), idx);
        }
        self.collect_stmt_definitions(&event_owner, &event.body, false, &mut definitions);
        let mut instances = HashMap::<String, InstanceInfo>::new();
        let scope_namespace = self.child_scope_namespace(Some(parent), owner);
        self.collect_stmt_scope_instances(&event.body, &scope_namespace, false, &mut instances);
        let scope_idx = self.push_scope(
            Some(parent),
            &scope_namespace,
            span_for_event_scope(event),
            definitions,
            instances,
        )?;
        self.collect_nested_stmt_scopes(scope_idx, &event_owner, &event.body);
        Some(scope_idx)
    }

    fn collect_stmt_scope(
        &mut self,
        parent: Option<usize>,
        owner: &str,
        span: Span,
        stmts: &[Stmt],
    ) -> Option<usize> {
        self.collect_stmt_scope_with_seed(
            parent,
            owner,
            span,
            stmts,
            HashMap::new(),
            HashMap::new(),
        )
    }

    fn collect_stmt_scope_with_seed(
        &mut self,
        parent: Option<usize>,
        owner: &str,
        span: Span,
        stmts: &[Stmt],
        mut definitions: HashMap<String, usize>,
        mut instances: HashMap<String, InstanceInfo>,
    ) -> Option<usize> {
        self.collect_stmt_definitions(owner, stmts, false, &mut definitions);
        let scope_namespace = self.child_scope_namespace(parent, owner);
        self.collect_stmt_scope_instances(stmts, &scope_namespace, false, &mut instances);
        let scope_idx = self.push_scope(parent, &scope_namespace, span, definitions, instances)?;
        self.collect_nested_stmt_scopes(scope_idx, owner, stmts);
        Some(scope_idx)
    }

    fn collect_nested_stmt_scopes(&mut self, parent: usize, owner: &str, stmts: &[Stmt]) {
        for stmt in stmts {
            match stmt {
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    if let Some(span) = span_for_stmt_body(then_branch) {
                        self.collect_stmt_scope(Some(parent), owner, span, then_branch);
                    }
                    if let Some(span) = span_for_stmt_body(else_branch) {
                        self.collect_stmt_scope(Some(parent), owner, span, else_branch);
                    }
                }
                Stmt::For { var, loc, body, .. } => {
                    if let Some(span) = span_for_stmt_body(body) {
                        let mut definitions = HashMap::new();
                        let idx = self.add_local_variable_definition(owner, var, *loc);
                        definitions.insert(var.clone(), idx);
                        self.collect_stmt_scope_with_seed(
                            Some(parent),
                            owner,
                            span,
                            body,
                            definitions,
                            HashMap::new(),
                        );
                    }
                }
                Stmt::While { body, .. } => {
                    if let Some(span) = span_for_stmt_body(body) {
                        self.collect_stmt_scope(Some(parent), owner, span, body);
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
                Stmt::Assign {
                    target,
                    target_loc,
                    expr,
                    ..
                } => {
                    if self.span_start_is_visible(*target_loc) {
                        if let Some(name) = assign_target_name(target) {
                            if let Some(instance) = self.instance_from_expr(expr, namespace) {
                                out.insert(name.to_owned(), instance);
                            }
                        }
                    }
                }
                Stmt::If {
                    loc,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    if !top_level_assigns_only && self.span_end_is_visible(*loc) {
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
                        let base_names = out.keys().cloned().collect::<HashSet<_>>();
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

    fn collect_stmt_definitions(
        &mut self,
        owner: &str,
        stmts: &[Stmt],
        top_level_assigns_only: bool,
        out: &mut HashMap<String, usize>,
    ) {
        self.collect_visible_stmt_definitions(owner, stmts, top_level_assigns_only, out);
    }

    fn collect_visible_stmt_definitions(
        &mut self,
        owner: &str,
        stmts: &[Stmt],
        top_level_assigns_only: bool,
        out: &mut HashMap<String, usize>,
    ) {
        for stmt in stmts {
            match stmt {
                Stmt::Const { loc, decl, .. } => {
                    if self.span_start_is_visible(*loc) {
                        let idx = self.add_const_definition(owner, decl);
                        out.entry(decl.name.clone()).or_insert(idx);
                    }
                }
                Stmt::Assign {
                    target, target_loc, ..
                } => {
                    if self.span_start_is_visible(*target_loc) {
                        for name in assign_target_names(target) {
                            let idx = self.add_local_variable_definition(owner, name, *target_loc);
                            out.entry(name.to_owned()).or_insert(idx);
                        }
                    }
                }
                Stmt::If {
                    loc,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    if !top_level_assigns_only && self.span_end_is_visible(*loc) {
                        let mut then_defs = out.clone();
                        self.collect_visible_stmt_definitions(
                            owner,
                            then_branch,
                            false,
                            &mut then_defs,
                        );
                        let mut else_defs = out.clone();
                        self.collect_visible_stmt_definitions(
                            owner,
                            else_branch,
                            false,
                            &mut else_defs,
                        );
                        let base_names = out.keys().cloned().collect::<HashSet<_>>();
                        for (name, idx) in then_defs {
                            if base_names.contains(&name) || else_defs.contains_key(&name) {
                                out.entry(name).or_insert(idx);
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

    fn span_start_is_visible(&self, span: Span) -> bool {
        let Some(position) = self.position else {
            return true;
        };
        span_start_position(&self.source, span)
            <= LspPosition::new(position.line, position.character)
    }

    fn span_end_is_visible(&self, span: Span) -> bool {
        let Some(position) = self.position else {
            return true;
        };
        span_end_position(&self.source, span) <= LspPosition::new(position.line, position.character)
    }

    fn push_scope(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        span: Span,
        definitions: HashMap<String, usize>,
        instances: HashMap<String, InstanceInfo>,
    ) -> Option<usize> {
        self.push_scope_with_namespace_flag(parent, namespace, span, definitions, instances, false)
    }

    fn push_namespace_scope(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        span: Span,
        definitions: HashMap<String, usize>,
        instances: HashMap<String, InstanceInfo>,
    ) -> Option<usize> {
        self.push_scope_with_namespace_flag(parent, namespace, span, definitions, instances, true)
    }

    fn push_scope_with_namespace_flag(
        &mut self,
        parent: Option<usize>,
        namespace: &str,
        span: Span,
        definitions: HashMap<String, usize>,
        instances: HashMap<String, InstanceInfo>,
        namespace_member_scope: bool,
    ) -> Option<usize> {
        if span.is_zero() || !self.span_belongs_to_current_file(span) {
            return None;
        }
        let depth = parent.map(|idx| self.scopes[idx].depth + 1).unwrap_or(0);
        self.scopes.push(NavigationScope {
            parent,
            namespace_member_scope,
            namespace: namespace.to_owned(),
            start_line: span.line.saturating_sub(1),
            start_column: u32::from(span.column.saturating_sub(1)),
            end_line: span.end_line().saturating_sub(1),
            end_column: u32::from(span.end_column.saturating_sub(1)),
            depth,
            definitions,
            instances,
        });
        Some(self.scopes.len() - 1)
    }

    fn block_belongs_to_current_file(&self, block: &Block) -> bool {
        self.span_belongs_to_current_file(block.loc().span())
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

    fn resolve_token(&self, source: &str, token: &SourceToken) -> Option<&DefinitionInfo> {
        if let Some((receiver, member)) = member_access_at_token(source, token) {
            return self.resolve_member(&receiver, &member, token.line, token.start_character);
        }
        if named_arg_label_at_token(source, token) {
            if let Some(callee) = active_call_callee(source, token.byte_start) {
                if let Some(definition) =
                    self.resolve_named_arg(&callee, &token.name, token.line, token.start_character)
                {
                    return Some(definition);
                }
            }
        }
        if let Some(path) = qualified_path_at_token(source, token) {
            if let Some(definition) =
                self.resolve_qualified_at(&path, token.line, token.start_character)
            {
                return Some(definition);
            }
        }
        self.resolve_unqualified(&token.name, token.line, token.start_character)
    }

    fn callable_hover_for_token(&self, source: &str, token: &SourceToken) -> Option<String> {
        if !token_is_followed_by_call_start(source, token.byte_end) {
            return None;
        }
        let instance = self.resolve_instance(&token.name, token.line, token.start_character)?;
        let proc_info = self.procs.get(&instance.type_name)?;
        Some(format!(
            "```onda\nproc call {}({})\n```",
            token.name, proc_info.call_signature
        ))
    }

    fn resolve_unqualified(&self, name: &str, line: u32, column: u32) -> Option<&DefinitionInfo> {
        if let Some(idx) = self.resolve_local_scope_chain(name, line, column) {
            return self.definitions.get(idx);
        }
        let mut candidates = Vec::<usize>::new();
        if let Some(idx) = self.resolve_nearest_namespace_scope_member(name, line, column) {
            push_unique_index(&mut candidates, idx);
        } else if let Some(idx) = self.resolve_top_level(name, true) {
            push_unique_index(&mut candidates, idx);
        }
        for idx in self.resolve_visible_use_candidates(name, line, column) {
            push_unique_index(&mut candidates, idx);
        }
        match candidates.as_slice() {
            [idx] => return self.definitions.get(*idx),
            [] => {}
            _ => return None,
        }
        self.resolve_top_level(name, false)
            .and_then(|idx| self.definitions.get(idx))
    }

    fn resolve_local_scope_chain(&self, name: &str, line: u32, column: u32) -> Option<usize> {
        let mut current = self.innermost_scope_index(line, column);
        while let Some(idx) = current {
            let scope = &self.scopes[idx];
            if let Some(def_idx) = scope.definitions.get(name) {
                if !scope.namespace_member_scope {
                    return Some(*def_idx);
                }
            }
            current = scope.parent;
        }
        None
    }

    fn resolve_nearest_namespace_scope_member(
        &self,
        name: &str,
        line: u32,
        column: u32,
    ) -> Option<usize> {
        let mut current = self.innermost_scope_index(line, column);
        while let Some(idx) = current {
            let scope = &self.scopes[idx];
            if scope.namespace_member_scope {
                if let Some(def_idx) = scope.definitions.get(name) {
                    return Some(*def_idx);
                }
            }
            current = scope.parent;
        }
        None
    }

    fn resolve_instance(&self, name: &str, line: u32, column: u32) -> Option<&InstanceInfo> {
        let mut current = self.innermost_scope_index(line, column);
        while let Some(idx) = current {
            let scope = &self.scopes[idx];
            if let Some(instance) = scope.instances.get(name) {
                return Some(instance);
            }
            current = scope.parent;
        }
        self.instances.get(name)
    }

    fn innermost_scope_index(&self, line: u32, column: u32) -> Option<usize> {
        let scopes_on_line = self.scopes_by_line.get(line as usize)?;
        let mut best = None::<usize>;
        for &idx in scopes_on_line {
            let scope = &self.scopes[idx];
            if !scope.contains(line, column) {
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

    fn rebuild_scope_line_index(&mut self) {
        self.scopes_by_line.clear();
        let Some(max_line) = self.scopes.iter().map(|scope| scope.end_line).max() else {
            return;
        };
        self.scopes_by_line
            .resize_with(max_line.saturating_add(1) as usize, Vec::new);
        for (idx, scope) in self.scopes.iter().enumerate() {
            for line in scope.start_line..=scope.end_line {
                if let Some(line_scopes) = self.scopes_by_line.get_mut(line as usize) {
                    line_scopes.push(idx);
                }
            }
        }
    }

    fn resolve_top_level(&self, name: &str, current_file_only: bool) -> Option<usize> {
        self.definitions
            .iter()
            .enumerate()
            .find_map(|(idx, definition)| {
                if definition.name != name
                    || !namespace_parent_of(&definition.full_name).is_empty()
                    || !matches!(
                        definition.kind,
                        DefinitionKind::Const
                            | DefinitionKind::Def
                            | DefinitionKind::Proc
                            | DefinitionKind::Struct
                            | DefinitionKind::Namespace
                            | DefinitionKind::NamespaceAlias
                    )
                {
                    return None;
                }
                if current_file_only {
                    match (&self.current_file_key, &definition.file_key) {
                        (Some(current), Some(file)) if current == file => Some(idx),
                        (None, _) => Some(idx),
                        _ => None,
                    }
                } else {
                    Some(idx)
                }
            })
    }

    fn resolve_visible_use_candidates(&self, name: &str, line: u32, column: u32) -> Vec<usize> {
        let current_namespace = self.current_namespace(line, column);
        let mut candidates = Vec::<usize>::new();
        for use_info in visible_uses_in_namespace(self, current_namespace) {
            let target = self.resolve_use_target(use_info);
            if let Some(alias) = &use_info.alias {
                if alias != name {
                    continue;
                }
                if let Some(idx) = self.by_full_name.get(&target) {
                    push_unique_index(&mut candidates, *idx);
                }
                if let Some(idx) = self.namespace_definition_index(&target) {
                    push_unique_index(&mut candidates, idx);
                }
                continue;
            }
            if let Some(members) = self.namespace_members.get(&target) {
                if let Some(idx) = members.get(name) {
                    push_unique_index(&mut candidates, *idx);
                }
            } else if target.rsplit("::").next() == Some(name) {
                if let Some(idx) = self.by_full_name.get(&target) {
                    push_unique_index(&mut candidates, *idx);
                }
            }
        }
        candidates
    }

    fn definition_indices_for_use_decl(
        &self,
        namespace: &str,
        use_decl: &UseDecl,
    ) -> Vec<(String, usize)> {
        let use_info = UseInfo {
            namespace: namespace.to_owned(),
            target: namespace_segments_key(&use_decl.target),
            alias: use_decl.alias.clone(),
            public: use_decl.public,
            file_key: file_key_for_span(use_decl.loc),
        };
        let target = self.resolve_use_target(&use_info);
        if let Some(alias) = &use_decl.alias {
            if let Some(idx) = self
                .by_full_name
                .get(&target)
                .copied()
                .or_else(|| self.namespace_definition_index(&target))
            {
                return vec![(alias.clone(), idx)];
            }
            return Vec::new();
        }
        if let Some(members) = self.namespace_members.get(&target) {
            return members
                .iter()
                .map(|(name, idx)| (name.clone(), *idx))
                .collect();
        }
        self.by_full_name
            .get(&target)
            .and_then(|idx| {
                self.definitions
                    .get(*idx)
                    .map(|definition| vec![(definition.name.clone(), *idx)])
            })
            .unwrap_or_default()
    }

    fn resolve_qualified_at(&self, path: &str, line: u32, column: u32) -> Option<&DefinitionInfo> {
        let namespace = self.current_namespace(line, column);
        self.resolve_qualified_in_namespace(path, namespace)
    }

    fn resolve_qualified_in_namespace(
        &self,
        path: &str,
        namespace: &str,
    ) -> Option<&DefinitionInfo> {
        self.qualified_path_candidates(path, namespace)
            .into_iter()
            .find_map(|candidate| {
                self.definition_index_for_qualified_name(&candidate)
                    .and_then(|idx| self.definitions.get(idx))
            })
    }

    fn definition_index_for_qualified_name(&self, name: &str) -> Option<usize> {
        self.by_full_name
            .get(name)
            .copied()
            .or_else(|| self.namespace_definition_index(name))
    }

    fn current_namespace(&self, line: u32, column: u32) -> &str {
        self.innermost_scope_index(line, column)
            .and_then(|idx| self.scopes.get(idx))
            .map(|scope| scope.namespace.as_str())
            .unwrap_or("")
    }

    fn qualified_path_candidates(&self, path: &str, namespace: &str) -> Vec<String> {
        namespace_qualified_path_candidates(
            self,
            path,
            namespace,
            |ctx, candidate| ctx.has_symbol(candidate),
            |ctx, candidate| ctx.has_symbol(candidate),
            AliasTargetPolicy::Always,
        )
    }

    fn resolve_use_target(&self, use_info: &UseInfo) -> String {
        resolve_namespace_use_target(self, use_info)
    }

    fn namespace_exists(&self, namespace: &str) -> bool {
        self.namespace_members.contains_key(namespace)
            || self.namespace_aliases.contains_key(namespace)
            || self.namespace_definition_index(namespace).is_some()
    }

    fn namespace_definition_index(&self, namespace: &str) -> Option<usize> {
        self.definitions
            .iter()
            .enumerate()
            .find_map(|(idx, definition)| {
                if matches!(
                    definition.kind,
                    DefinitionKind::Namespace | DefinitionKind::NamespaceAlias
                ) && definition.full_name == namespace
                {
                    Some(idx)
                } else {
                    None
                }
            })
    }

    fn resolve_member(
        &self,
        receiver: &str,
        member: &str,
        line: u32,
        column: u32,
    ) -> Option<&DefinitionInfo> {
        let root = receiver_root(receiver);
        let instance = self.resolve_instance(root, line, column)?;
        if member == ARRAY_LEN_METHOD && instance.is_array {
            return None;
        }
        if let Some(proc_info) = self.procs.get(&instance.type_name) {
            if let Some(idx) = proc_info
                .ins
                .get(member)
                .or_else(|| proc_info.outs.get(member))
            {
                return self.definitions.get(*idx);
            }
            if let Some(idx) = proc_info.params.get(member) {
                let definition = self.definitions.get(*idx)?;
                if !definition.pinned {
                    return Some(definition);
                }
                return None;
            }
            if member == "params" && !proc_info.has_pinned_params {
                return None;
            }
            if member == "init" {
                return proc_info.init.and_then(|idx| self.definitions.get(idx));
            }
            if let Some(idx) = proc_info.events.get(member) {
                return self.definitions.get(*idx);
            }
        } else if let Some(struct_info) = self.structs.get(&instance.type_name) {
            if let Some(idx) = struct_info
                .fields
                .get(member)
                .or_else(|| struct_info.methods.get(member))
            {
                return self.definitions.get(*idx);
            }
        }
        None
    }

    fn resolve_named_arg(
        &self,
        callee: &str,
        arg: &str,
        line: u32,
        column: u32,
    ) -> Option<&DefinitionInfo> {
        let callee = normalize_call_callee(callee);
        if let Some((receiver, member)) = split_member_callee(&callee) {
            if let Some(definition) =
                self.resolve_member_named_arg(receiver, member, arg, line, column)
            {
                return Some(definition);
            }
        }
        if let Some(instance) = self.resolve_instance(receiver_root(&callee), line, column) {
            if let Some(proc_info) = self.procs.get(&instance.type_name) {
                if let Some(idx) = proc_info.ins.get(arg) {
                    return self.definitions.get(*idx);
                }
                if let Some(idx) = proc_info.params.get(arg) {
                    let definition = self.definitions.get(*idx)?;
                    if !definition.pinned {
                        return Some(definition);
                    }
                }
                return None;
            }
        }
        if let Some(proc_name) = self.resolve_type_name_at(&callee, line, column) {
            if let Some(proc_info) = self.procs.get(&proc_name) {
                if let Some(idx) = proc_info
                    .params
                    .get(arg)
                    .or_else(|| proc_info.buffers.get(arg))
                {
                    return self.definitions.get(*idx);
                }
                return None;
            }
        }
        let Some(function) = self.resolve_qualified_at(&callee, line, column) else {
            return None;
        };
        if !matches!(function.kind, DefinitionKind::Def | DefinitionKind::Method) {
            return None;
        }
        self.resolve_function_param(&function.full_name, arg)
    }

    fn resolve_member_named_arg(
        &self,
        receiver: &str,
        member: &str,
        arg: &str,
        line: u32,
        column: u32,
    ) -> Option<&DefinitionInfo> {
        let instance = self.resolve_instance(receiver_root(receiver), line, column)?;
        if let Some(proc_info) = self.procs.get(&instance.type_name) {
            if member == "init" {
                if let Some(idx) = proc_info.params.get(arg) {
                    return self.definitions.get(*idx);
                }
                return None;
            }
            if let Some(idx) = proc_info.events.get(member) {
                let event = self.definitions.get(*idx)?;
                return self.resolve_event_param(&event.full_name, arg);
            }
        } else if let Some(struct_info) = self.structs.get(&instance.type_name) {
            if let Some(idx) = struct_info.methods.get(member) {
                let method = self.definitions.get(*idx)?;
                return self.resolve_function_param(&method.full_name, arg);
            }
        }
        None
    }

    fn resolve_function_param(&self, owner: &str, arg: &str) -> Option<&DefinitionInfo> {
        self.function_params
            .get(owner)
            .and_then(|params| params.get(arg))
            .and_then(|idx| self.definitions.get(*idx))
    }

    fn resolve_event_param(&self, owner: &str, arg: &str) -> Option<&DefinitionInfo> {
        self.event_params
            .get(owner)
            .and_then(|params| params.get(arg))
            .and_then(|idx| self.definitions.get(*idx))
    }

    fn resolve_type_name_at(&self, name: &str, line: u32, column: u32) -> Option<String> {
        let namespace = self.current_namespace(line, column);
        self.resolve_type_name_in_namespace(name, namespace)
    }

    fn resolve_type_name_in_namespace(&self, name: &str, namespace: &str) -> Option<String> {
        for candidate in self.qualified_path_candidates(name, namespace) {
            if self.procs.contains_key(&candidate) || self.structs.contains_key(&candidate) {
                return Some(candidate);
            }
        }

        let clean = strip_type_args_from_path(name);
        self.definitions.iter().find_map(|definition| {
            if matches!(
                definition.kind,
                DefinitionKind::Proc | DefinitionKind::Struct
            ) && definition.name == clean
            {
                Some(definition.full_name.clone())
            } else {
                None
            }
        })
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
}

impl NamespaceResolutionContext for NavigationIndex {
    fn namespace_alias(&self, full_name: &str) -> Option<&NamespaceAliasInfo> {
        self.namespace_aliases.get(full_name)
    }

    fn has_namespace(&self, namespace: &str) -> bool {
        self.namespace_exists(namespace)
    }

    fn has_symbol(&self, full_name: &str) -> bool {
        self.definition_index_for_qualified_name(full_name)
            .is_some()
    }

    fn uses(&self) -> &[UseInfo] {
        &self.uses
    }

    fn use_visible(&self, use_info: &UseInfo) -> bool {
        NavigationIndex::use_visible(self, use_info)
    }
}

impl NavigationScope {
    fn contains(&self, line: u32, column: u32) -> bool {
        if line < self.start_line || line > self.end_line {
            return false;
        }
        if line == self.start_line && column < self.start_column {
            return false;
        }
        if line == self.end_line && column > self.end_column {
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

fn document_symbols_for_program(
    program: &Program,
    current_file_key: Option<&str>,
    source: &str,
) -> Vec<Value> {
    let mut symbols = Vec::new();
    for block in &program.blocks {
        if !span_belongs_to_current_file(block.loc().span(), current_file_key) {
            continue;
        }
        if let Some(symbol) = document_symbol_for_block(block, source) {
            symbols.push(symbol);
        }
    }
    symbols
}

fn document_symbol_for_block(block: &Block, source: &str) -> Option<Value> {
    match block {
        Block::Const(decl) => Some(document_symbol(
            &decl.name,
            SYMBOL_KIND_CONSTANT,
            decl.loc,
            source,
            vec![],
        )),
        Block::Def(def) => Some(document_symbol_for_function(
            def,
            SYMBOL_KIND_FUNCTION,
            source,
        )),
        Block::Proc(proc_def) => Some(document_symbol_for_proc(proc_def, source)),
        Block::Struct(struct_def) => Some(document_symbol_for_struct(struct_def, source)),
        Block::Namespace(ns) => Some(document_symbol_for_namespace(ns, source)),
        Block::NamespaceAlias(alias) => Some(document_symbol(
            &alias.name,
            SYMBOL_KIND_NAMESPACE,
            alias.loc,
            source,
            vec![],
        )),
        Block::Events(events) => {
            let children = events
                .events
                .iter()
                .map(|event| document_symbol_for_event(event, source))
                .collect::<Vec<_>>();
            Some(document_symbol(
                "events",
                SYMBOL_KIND_EVENT,
                events.loc,
                source,
                children,
            ))
        }
        Block::Init(init) => Some(document_symbol(
            "init",
            SYMBOL_KIND_METHOD,
            init.loc,
            source,
            vec![],
        )),
        Block::Sample(sample) => Some(document_symbol(
            "sample",
            SYMBOL_KIND_METHOD,
            sample.loc,
            source,
            vec![],
        )),
        Block::Block(exec) => Some(document_symbol(
            "block",
            SYMBOL_KIND_METHOD,
            exec.loc,
            source,
            vec![],
        )),
        Block::Graph(graph) => Some(document_symbol(
            "graph",
            SYMBOL_KIND_METHOD,
            graph.loc,
            source,
            vec![],
        )),
        _ => None,
    }
}

fn document_symbol_for_namespace(ns: &NamespaceDecl, source: &str) -> Value {
    let mut children = Vec::new();
    for item in &ns.items {
        let symbol = match item {
            NamespaceItem::Const(decl) => Some(document_symbol(
                &decl.name,
                SYMBOL_KIND_CONSTANT,
                decl.loc,
                source,
                vec![],
            )),
            NamespaceItem::Def(def) => Some(document_symbol_for_function(
                def,
                SYMBOL_KIND_FUNCTION,
                source,
            )),
            NamespaceItem::Proc(proc_def) => Some(document_symbol_for_proc(proc_def, source)),
            NamespaceItem::Struct(struct_def) => {
                Some(document_symbol_for_struct(struct_def, source))
            }
            NamespaceItem::Namespace(child) => Some(document_symbol_for_namespace(child, source)),
            NamespaceItem::Alias(alias) => Some(document_symbol(
                &alias.name,
                SYMBOL_KIND_NAMESPACE,
                alias.loc,
                source,
                vec![],
            )),
            NamespaceItem::Use(_) | NamespaceItem::Assert(_) => None,
        };
        if let Some(symbol) = symbol {
            children.push(symbol);
        }
    }
    document_symbol(&ns.name, SYMBOL_KIND_NAMESPACE, ns.loc, source, children)
}

fn document_symbol_for_proc(proc_def: &ProcessorDef, source: &str) -> Value {
    let mut children = Vec::new();
    children.extend(
        proc_def.consts.iter().map(|decl| {
            document_symbol(&decl.name, SYMBOL_KIND_CONSTANT, decl.loc, source, vec![])
        }),
    );
    children.extend(
        proc_def
            .ins
            .iter()
            .map(|decl| document_symbol(&decl.name, SYMBOL_KIND_FIELD, decl.loc, source, vec![])),
    );
    children.extend(
        proc_def
            .outs
            .iter()
            .map(|decl| document_symbol(&decl.name, SYMBOL_KIND_FIELD, decl.loc, source, vec![])),
    );
    children.extend(
        proc_def.params.iter().map(|decl| {
            document_symbol(&decl.name, SYMBOL_KIND_VARIABLE, decl.loc, source, vec![])
        }),
    );
    children.extend(
        proc_def
            .buffers
            .iter()
            .map(|decl| document_symbol(&decl.name, SYMBOL_KIND_FIELD, decl.loc, source, vec![])),
    );
    children.extend(
        proc_def
            .events
            .iter()
            .map(|event| document_symbol_for_event(event, source)),
    );
    children.extend(
        proc_def
            .local_defs
            .iter()
            .map(|def| document_symbol_for_function(def, SYMBOL_KIND_METHOD, source)),
    );
    document_symbol(
        &proc_def.name,
        SYMBOL_KIND_CONSTRUCTOR,
        proc_def.loc,
        source,
        children,
    )
}

fn document_symbol_for_struct(struct_def: &StructDef, source: &str) -> Value {
    let mut children = Vec::new();
    children.extend(
        struct_def.fields.iter().map(|field| {
            document_symbol(&field.name, SYMBOL_KIND_FIELD, field.loc, source, vec![])
        }),
    );
    children.extend(
        struct_def
            .methods
            .iter()
            .map(|method| document_symbol_for_function(method, SYMBOL_KIND_METHOD, source)),
    );
    document_symbol(
        &struct_def.name,
        SYMBOL_KIND_STRUCT,
        struct_def.loc,
        source,
        children,
    )
}

fn document_symbol_for_function(def: &FunctionDef, kind: u32, source: &str) -> Value {
    document_symbol(&def.name, kind, def.loc, source, vec![])
}

fn document_symbol_for_event(event: &EventDef, source: &str) -> Value {
    document_symbol(&event.name, SYMBOL_KIND_EVENT, event.loc, source, vec![])
}

fn document_symbol(name: &str, kind: u32, span: Span, source: &str, children: Vec<Value>) -> Value {
    let mut symbol = json!({
        "name": name,
        "kind": kind,
        "range": span_range_json(span, Some(source)),
        "selectionRange": span_range_json(span, Some(source)),
    });
    if !children.is_empty() {
        symbol["children"] = json!(children);
    }
    symbol
}

fn span_range_json(span: Span, source: Option<&str>) -> Value {
    let start = source
        .map(|source| span_start_position(source, span))
        .unwrap_or_else(|| fallback_span_start_position(span));
    let end = source
        .map(|source| span_end_position(source, span))
        .unwrap_or_else(|| fallback_span_end_position(span));
    json!({
        "start": {
            "line": start.line,
            "character": start.character,
        },
        "end": {
            "line": end.line,
            "character": end.character,
        },
    })
}

fn uri_for_span(span: Span, fallback_path: Option<&Path>) -> Option<String> {
    let path = span
        .file()
        .map(|file| path_for_span_file(&file))
        .or_else(|| fallback_path.map(Path::to_path_buf))?;
    Some(path_to_file_uri(&path))
}

fn source_for_span<'a>(
    span: Span,
    fallback_path: Option<&Path>,
    fallback_source: &'a str,
    overlays: &'a HashMap<PathBuf, String>,
) -> Option<Cow<'a, str>> {
    let Some(file) = span.file() else {
        return Some(Cow::Borrowed(fallback_source));
    };
    if let Some(module) = stdlib_module_from_virtual_file(&file) {
        return onda_frontend::stdlib_module_source(module).map(Cow::Borrowed);
    }

    let file_key = normalize_file_key(&file);
    if fallback_path
        .map(normalize_file_key_for_path)
        .is_some_and(|key| key == file_key)
    {
        return Some(Cow::Borrowed(fallback_source));
    }
    for (path, text) in overlays {
        if normalize_file_key_for_path(path) == file_key {
            return Some(Cow::Borrowed(text.as_str()));
        }
    }

    fs::read_to_string(path_for_span_file(&file))
        .ok()
        .map(Cow::Owned)
}

fn path_for_span_file(file: &str) -> PathBuf {
    if let Some(module) = stdlib_module_from_virtual_file(file) {
        if let Some(path) = materialized_stdlib_path(module) {
            return path;
        }
    }
    PathBuf::from(file)
}

fn stdlib_module_from_virtual_file(file: &str) -> Option<&str> {
    file.strip_prefix('<')
        .and_then(|s| s.strip_suffix(".onda>"))
        .filter(|module| module.starts_with("std/"))
}

fn materialized_stdlib_path(module: &str) -> Option<PathBuf> {
    let source = onda_frontend::stdlib_module_source(module)?;
    let path = materialized_stdlib_file_path(module, source)?;
    ensure_materialized_readonly_file(&path, source).ok()?;
    Some(path)
}

fn materialized_stdlib_file_path(module: &str, source: &str) -> Option<PathBuf> {
    let mut root = onda_cache_dir();
    root.push("stdlib");
    root.push(format!(
        "{}-{:016x}",
        env!("CARGO_PKG_VERSION"),
        stable_stdlib_hash(module, source)
    ));
    for part in module.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return None;
        }
        root.push(part);
    }
    root.set_extension("onda");
    Some(root)
}

fn ensure_materialized_readonly_file(path: &Path, source: &str) -> io::Result<()> {
    if file_contents_match(path, source) {
        set_file_readonly(path, true)?;
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    if path.exists() {
        set_file_readonly(path, false).ok();
        fs::remove_file(path)?;
    }

    match fs::write(path, source) {
        Ok(()) => {
            set_file_readonly(path, true)?;
            Ok(())
        }
        Err(err) => {
            if file_contents_match(path, source) {
                set_file_readonly(path, true)?;
                Ok(())
            } else {
                Err(err)
            }
        }
    }
}

fn file_contents_match(path: &Path, expected: &str) -> bool {
    fs::read_to_string(path)
        .map(|existing| existing == expected)
        .unwrap_or(false)
}

fn set_file_readonly(path: &Path, readonly: bool) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    if permissions.readonly() == readonly {
        return Ok(());
    }
    permissions.set_readonly(readonly);
    fs::set_permissions(path, permissions)
}

fn stable_stdlib_hash(module: &str, source: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in module
        .bytes()
        .chain(std::iter::once(0))
        .chain(source.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn onda_cache_dir() -> PathBuf {
    if let Some(path) = nonempty_env_path("ONDA_STDLIB_CACHE_DIR") {
        return path;
    }

    platform_cache_dir()
        .unwrap_or_else(env::temp_dir)
        .join("onda")
}

#[cfg(windows)]
fn platform_cache_dir() -> Option<PathBuf> {
    nonempty_env_path("LOCALAPPDATA").or_else(|| nonempty_env_path("APPDATA"))
}

#[cfg(target_os = "macos")]
fn platform_cache_dir() -> Option<PathBuf> {
    nonempty_env_path("HOME").map(|home| home.join("Library").join("Caches"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_cache_dir() -> Option<PathBuf> {
    nonempty_env_path("XDG_CACHE_HOME")
        .or_else(|| nonempty_env_path("HOME").map(|home| home.join(".cache")))
}

#[cfg(not(any(unix, windows)))]
fn platform_cache_dir() -> Option<PathBuf> {
    None
}

fn nonempty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

fn import_location_at_token(
    source: &str,
    current_path: Option<&Path>,
    token: &SourceToken,
) -> Option<Value> {
    let module = import_module_at_token(source, token)?;
    let path = if module.starts_with("std/") {
        materialized_stdlib_path(&module)?
    } else {
        let current_path = current_path?;
        let base = current_path.parent().unwrap_or_else(|| Path::new("."));
        resolve_local_module_path(base, &module)?
    };
    if !path.exists() {
        return None;
    }
    Some(json!({
        "uri": path_to_file_uri(&path),
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 0 },
        }
    }))
}

fn resolve_local_module_path(base: &Path, module: &str) -> Option<PathBuf> {
    let raw = PathBuf::from(module);
    let base = if raw.is_absolute() {
        raw
    } else {
        base.join(raw)
    };
    for ext in ["onda", "on"] {
        let candidate = base.with_extension(ext);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn import_module_at_token(source: &str, token: &SourceToken) -> Option<String> {
    let line = token.line_slice(source);
    let trimmed = line.trim_start();
    let indent = line.len().saturating_sub(trimmed.len());
    let rest = trimmed.strip_prefix("import ")?;
    if token.byte_start < token.line_start + indent + "import ".len() {
        return None;
    }
    Some(rest.split('#').next().unwrap_or(rest).trim().to_owned())
}

fn builtin_hover(name: &str) -> Option<String> {
    if is_builtin_function_name(name) {
        Some(format!("```onda\nbuilt-in call {name}(...)\n```"))
    } else if builtin_instance_method_names().any(|method| method == name) {
        Some(format!("```onda\nbuilt-in call .{name}(...)\n```"))
    } else if let Some(ty) = builtin_constant_type(name) {
        Some(format!("```onda\nbuiltin const {name}: {}\n```", ty.name()))
    } else if is_language_type_name(name) {
        Some(format!("```onda\ntype {name}\n```"))
    } else {
        None
    }
}

fn format_type_params(type_params: &[String]) -> String {
    if type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", type_params.join(", "))
    }
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

fn proc_init_signature(proc_def: &ProcessorDef) -> String {
    proc_def
        .params
        .iter()
        .map(format_proc_param_signature)
        .collect::<Vec<_>>()
        .join(", ")
}

fn proc_call_signature(proc_def: &ProcessorDef) -> String {
    proc_def
        .ins
        .iter()
        .map(format_port_arg_signature)
        .chain(
            proc_def
                .params
                .iter()
                .filter(|param| !param.pinned)
                .map(format_proc_param_signature),
        )
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_port_arg_signature(port: &PortDecl) -> String {
    let mut text = port.name.clone();
    if let Some(ty) = &port.ty {
        text.push_str(": ");
        text.push_str(&format_decl_type(ty));
    }
    if let Some(default) = &port.default {
        text.push_str(" = ");
        text.push_str(&format_expr(default));
    }
    text
}

fn token_is_followed_by_call_start(source: &str, offset: usize) -> bool {
    let mut rest = &source[offset.min(source.len())..];
    rest = rest.trim_start_matches(char::is_whitespace);
    rest.starts_with('(')
}

fn source_token_at_position(source: &str, position: NavigationPosition) -> Option<SourceToken> {
    let (line_start, line_end, line_text) = line_at_position(source, position.line)?;
    let mut byte = byte_index_for_lsp_character(line_text, position.character);
    if byte == line_text.len() || !is_ident_byte(line_text.as_bytes().get(byte).copied()) {
        if byte == 0 {
            return None;
        }
        let prev = previous_char_start(line_text, byte)?;
        if !is_ident_byte(line_text.as_bytes().get(prev).copied()) {
            return None;
        }
        byte = prev;
    }

    let mut start = byte;
    while start > 0 {
        let prev = previous_char_start(line_text, start)?;
        if is_ident_byte(line_text.as_bytes().get(prev).copied()) {
            start = prev;
        } else {
            break;
        }
    }

    let mut end = byte;
    while end < line_text.len() {
        let ch_len = line_text[end..].chars().next()?.len_utf8();
        if is_ident_byte(line_text.as_bytes().get(end).copied()) {
            end += ch_len;
        } else {
            break;
        }
    }

    let name = line_text[start..end].to_owned();
    if name.is_empty() {
        return None;
    }
    Some(SourceToken {
        name,
        line: position.line,
        start_character: lsp_character_for_byte(line_text, start),
        end_character: lsp_character_for_byte(line_text, end),
        byte_start: line_start + start,
        byte_end: line_start + end,
        line_start,
        line_end,
    })
}

fn previous_char_start(line: &str, byte: usize) -> Option<usize> {
    line[..byte].char_indices().last().map(|(idx, _)| idx)
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
        sanitized.push_str("const __onda_navigation_placeholder = 0\n");
    } else {
        sanitized.push_str(&indent);
        sanitized.push_str("__onda_navigation_placeholder = 0.0\n");
    }
    if line_end < source.len() {
        sanitized.push_str(&source[line_end + 1..]);
    }
    sanitized
}

fn member_access_at_token(source: &str, token: &SourceToken) -> Option<(String, String)> {
    let before = source[..token.byte_start].trim_end();
    let without_dot = before.strip_suffix('.')?;
    let receiver = scan_receiver_left(without_dot)?;
    Some((receiver, token.name.clone()))
}

fn named_arg_label_at_token(source: &str, token: &SourceToken) -> bool {
    let after = &source[token.byte_end..token.line_end];
    after.trim_start().starts_with('=')
}

fn qualified_path_at_token(source: &str, token: &SourceToken) -> Option<String> {
    let line = token.line_slice(source);
    let token_start = token.byte_start - token.line_start;
    let token_end = token.byte_end - token.line_start;
    let mut start = token_start;
    loop {
        let before = line[..start].trim_end();
        let Some(before_colons) = before.strip_suffix("::") else {
            break;
        };
        let segment_start = scan_path_segment_start(before_colons)?;
        start = segment_start;
    }
    let raw = &line[start..token_end];
    if raw.contains("::") {
        Some(strip_type_args_from_path(raw))
    } else {
        None
    }
}

fn scan_path_segment_start(text: &str) -> Option<usize> {
    let mut end = text.len();
    while end > 0 && text.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    if text.as_bytes()[end - 1] == b'>' {
        let mut depth = 0usize;
        for (idx, ch) in text[..end].char_indices().rev() {
            match ch {
                '>' => depth += 1,
                '<' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = idx;
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let mut start = end;
    while start > 0 {
        let prev = previous_char_start(text, start)?;
        let ch = text[prev..].chars().next()?;
        if is_ident_continue(ch) {
            start = prev;
        } else {
            break;
        }
    }
    if start == end {
        None
    } else {
        Some(start)
    }
}

fn active_call_callee(source: &str, offset: usize) -> Option<String> {
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
    scan_call_callee_left(before)
}

fn scan_call_callee_left(source_before_call: &str) -> Option<String> {
    let text = source_before_call.trim_end();
    let mut start = text.len();
    let mut angle_depth = 0usize;
    let mut bracket_depth = 0usize;
    while start > 0 {
        let Some((idx, ch)) = text[..start].char_indices().last() else {
            break;
        };
        match ch {
            '>' => angle_depth += 1,
            '<' => angle_depth = angle_depth.saturating_sub(1),
            ']' => bracket_depth += 1,
            '[' => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }
        let allowed = is_ident_continue(ch)
            || matches!(ch, ':' | '.' | '<' | '>' | '[' | ']')
            || (angle_depth > 0
                && matches!(
                    ch,
                    ',' | '=' | ' ' | '\t' | '+' | '-' | '*' | '/' | '(' | ')'
                ))
            || (bracket_depth > 0 && ch != '\n');
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
            || ch == '<'
            || ch == '>'
            || (angle_depth > 0
                && matches!(
                    ch,
                    ',' | '=' | ' ' | '\t' | '+' | '-' | '*' | '/' | '(' | ')'
                ));
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

fn runtime_regions_for_block_exec(exec: &BlockExec) -> Vec<(Span, &[Stmt], bool)> {
    let mut regions = Vec::new();
    if let Some(span) = span_for_stmt_body(&exec.pre) {
        regions.push((span, exec.pre.as_slice(), true));
    }
    if let Some(sample) = &exec.sample {
        regions.push((sample.loc, sample.body.as_slice(), false));
    }
    if let Some(span) = span_for_stmt_body(&exec.post) {
        regions.push((span, exec.post.as_slice(), true));
    }
    regions
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

fn span_belongs_to_current_file(span: Span, current_file_key: Option<&str>) -> bool {
    match current_file_key {
        Some(expected) => span
            .file()
            .map(|file| normalize_file_key(&file) == expected)
            .unwrap_or(false),
        None => true,
    }
}

fn assign_target_name(target: &AssignTarget) -> Option<&str> {
    match target {
        AssignTarget::Var(name) => Some(name),
        _ => None,
    }
}

fn assign_target_names(target: &AssignTarget) -> Vec<&str> {
    match target {
        AssignTarget::Var(name) => vec![name.as_str()],
        AssignTarget::Tuple(names) => names.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

fn push_unique_index(candidates: &mut Vec<usize>, candidate: usize) {
    if !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn namespace_leaf(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
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

fn is_namespace_path(text: &str) -> bool {
    text.chars()
        .all(|ch| is_ident_continue(ch) || matches!(ch, ':' | '<' | '>' | ',' | ' ' | '\t'))
}

fn is_ident_byte(byte: Option<u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
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

#[cfg(test)]
mod tests {
    use super::*;

    fn position_at(source: &str, needle: &str, token_offset: usize) -> NavigationPosition {
        let byte = source
            .find(needle)
            .map(|idx| idx + token_offset)
            .expect("test needle should exist");
        let line = source[..byte].bytes().filter(|byte| *byte == b'\n').count() as u32;
        let line_start = source[..byte].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        NavigationPosition {
            line,
            character: (byte - line_start) as u32,
        }
    }

    fn definition_at(source: &str, needle: &str, token_offset: usize) -> Option<Value> {
        let parsed = parse_program(source).expect("test source should parse");
        definition_for_document_with_parsed(
            source,
            None,
            &HashMap::new(),
            Some(&parsed),
            position_at(source, needle, token_offset),
        )
    }

    #[test]
    fn hides_proc_local_defs_from_external_member_navigation() {
        let source = r#"proc Voice:
  outs:
    out1
  def helper(x):
    return x
  sample:
    out1 = helper(0.0)

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice.helper(0.0)
"#;

        assert!(
            definition_at(source, "voice.helper", "voice.".len() + 1).is_none(),
            "external member access should not resolve private proc-local defs"
        );
        assert!(
            definition_at(source, "helper(0.0)", 1).is_some(),
            "proc-local def should still resolve inside its owning proc"
        );
    }

    #[test]
    fn hides_pinned_params_from_external_member_navigation() {
        let source = r#"proc Voice:
  params:
    pin cutoff = 1000.0
  outs:
    out1
  sample:
    out1 = cutoff

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice.cutoff
"#;

        assert!(
            definition_at(source, "voice.cutoff", "voice.".len() + 1).is_none(),
            "external member access should not resolve pinned params"
        );
        assert!(
            definition_at(source, "out1 = cutoff", "out1 = ".len() + 1).is_some(),
            "pinned params should still resolve inside their owning proc"
        );
    }
}
