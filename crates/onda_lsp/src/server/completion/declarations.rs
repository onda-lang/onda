//! Index source declarations and their qualified members for completion.
use super::*;

impl CompletionIndex {
    pub(super) fn collect_block(&mut self, block: &Block, namespace: &str) {
        match block {
            Block::Const(decl) => self.collect_const(namespace, decl),
            Block::Def(def) => self.collect_def(namespace, def),
            Block::Proc(proc_def) => self.collect_proc(namespace, proc_def),
            Block::Enum(def) => self.collect_enum(namespace, def),
            Block::Struct(struct_def) => self.collect_struct(namespace, struct_def),
            Block::Namespace(ns) => self.collect_namespace(namespace, ns),
            Block::NamespaceAlias(alias) => self.collect_namespace_alias(namespace, alias),
            Block::Use(use_decl) => self.collect_use(namespace, use_decl),
            _ => {}
        }
    }

    pub(super) fn collect_enum(&mut self, namespace: &str, def: &onda_frontend::EnumDef) {
        let full_name = namespace_join(namespace, &def.name);
        self.push_symbol(SymbolInfo {
            label: def.name.clone(),
            full_name: full_name.clone(),
            namespace: namespace.to_owned(),
            kind: SymbolKind::Enum,
            type_params: Vec::new(),
            label_detail: None,
            detail: Some(format!("enum {}", def.name)),
        });
        for (ordinal, member) in def.members.iter().enumerate() {
            self.push_symbol(SymbolInfo {
                label: member.name.clone(),
                full_name: namespace_join(&full_name, &member.name),
                namespace: full_name.clone(),
                kind: SymbolKind::EnumMember,
                type_params: Vec::new(),
                label_detail: None,
                detail: Some(format!("{}::{} = {ordinal}", def.name, member.name)),
            });
        }
    }

    pub(super) fn collect_const(&mut self, namespace: &str, decl: &ConstDecl) {
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

    pub(super) fn collect_def(&mut self, namespace: &str, def: &FunctionDef) {
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

    pub(super) fn collect_proc(&mut self, namespace: &str, proc_def: &ProcessorDef) {
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
                    private: decl.private,
                    signature: format_proc_param_signature(decl),
                })
                .chain(proc_deferred_param_infos(
                    &proc_def.params,
                    proc_def.params_deferred_count.as_ref(),
                ))
                .collect(),
            events: proc_def.events.iter().map(event_info_from_def).collect(),
            delegates: proc_def
                .delegates
                .iter()
                .map(delegate_info_from_def)
                .collect(),
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

    pub(super) fn collect_struct(&mut self, namespace: &str, struct_def: &StructDef) {
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

    pub(super) fn collect_namespace(&mut self, parent: &str, ns: &NamespaceDecl) {
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
                NamespaceItem::Enum(def) => self.collect_enum(&full_name, def),
                NamespaceItem::Struct(struct_def) => self.collect_struct(&full_name, struct_def),
                NamespaceItem::Namespace(child) => self.collect_namespace(&full_name, child),
                NamespaceItem::Alias(alias) => self.collect_namespace_alias(&full_name, alias),
                NamespaceItem::Use(use_decl) => self.collect_use(&full_name, use_decl),
                NamespaceItem::Assert(_) => {}
            }
        }
    }

    pub(super) fn collect_namespace_alias(&mut self, namespace: &str, alias: &NamespaceAliasDecl) {
        let full_name = namespace_join(namespace, &alias.name);
        let target = namespace_segments_key(&alias.target);
        self.namespace_aliases.insert(
            full_name.clone(),
            NamespaceAliasInfo {
                namespace: namespace.to_owned(),
                target,
            },
        );
        self.push_namespace(namespace, &alias.name, "namespace alias", Vec::new());
    }

    pub(super) fn collect_use(&mut self, namespace: &str, use_decl: &UseDecl) {
        self.uses.push(use_info_from_decl(namespace, use_decl));
    }

    pub(super) fn push_namespace(
        &mut self,
        parent: &str,
        name: &str,
        detail: &str,
        type_params: Vec<String>,
    ) {
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
}
