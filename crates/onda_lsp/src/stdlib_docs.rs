use onda_frontend::{
    stdlib_module_names, stdlib_module_source, Block, ConstDecl, DeclType, Expr, FunctionDef,
    NamespaceDecl, NamespaceItem, OutputTiming, ProcessorDef, Program, StructDef,
};

use crate::formatting::{
    format_buffer_decl, format_buffer_section_default_type, format_const_decl, format_decl_type,
    format_event_signature, format_expr, format_function_signature, format_param_decl,
    format_port_decl, format_proc_header, format_struct_field, format_struct_header,
};

const FRONT_MATTER: &str = r#"---
title: Standard library
description: Generated reference for Onda's built-in standard-library modules, functions, structs, and processors.
permalink: /docs/stdlib/
section: reference
eyebrow: Language reference
---

"#;

pub fn generate_stdlib_reference() -> Result<String, String> {
    let modules = stdlib_module_names()
        .map(|name| {
            let source = stdlib_module_source(name).ok_or_else(|| {
                format!("embedded standard-library module '{name}' has no source")
            })?;
            let program = parse_module_surface(name, source)?;
            Ok((name, source, program))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut out = String::from(FRONT_MATTER);
    out.push_str("# Onda standard library\n\n");
    out.push_str(
        "This page is generated from the standard library embedded in the compiler. Run \
         `scripts/update_stdlib_docs.sh` on Unix or `scripts/update_stdlib_docs.ps1` on Windows after \
         changing `stdlib/`; `npm run docs:stdlib` is the equivalent package command, and CI \
         verifies that the checked-in reference is current. Declarations whose names begin with \
         `_` are implementation helpers and are omitted.\n\n",
    );
    out.push_str(
        "`std/prelude` is imported automatically. It loads `std/math`, `std/lookup`, and \
         `std/random`, including the unqualified forwarding functions from the first two modules. \
         Import the other modules explicitly before using their qualified APIs.\n\n",
    );

    out.push_str("## Modules\n\n| Module | Provides |\n| --- | --- |\n");
    for (name, source, program) in &modules {
        let summary = if *name == "std/prelude" {
            let imports = module_imports(source);
            format!("Automatically imports {}", code_list(&imports))
        } else {
            let names = public_surface_names(program);
            if names.is_empty() {
                "—".to_owned()
            } else {
                code_list(&names)
            }
        };
        out.push_str(&format!("| [`{name}`](#{}) | {summary} |\n", anchor(name)));
    }

    for (name, source, program) in &modules {
        out.push_str(&format!("\n## `{name}`\n\n"));
        if *name == "std/prelude" {
            out.push_str("Imported automatically. It loads:\n\n");
            for import in module_imports(source) {
                out.push_str(&format!("- `{import}`\n"));
            }
            out.push('\n');
            continue;
        }

        out.push_str("```onda\n");
        out.push_str(&format!("import {name}\n"));
        out.push_str("```\n\n");
        render_program_surface(&mut out, program);
    }

    Ok(out)
}

fn parse_module_surface(module: &str, source: &str) -> Result<Program, String> {
    let source_without_imports = source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("import ") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    onda_frontend::parse_program(&source_without_imports).map_err(|diagnostics| {
        format!("failed to parse embedded module '{module}': {diagnostics:?}")
    })
}

fn module_imports(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("import "))
        .map(str::to_owned)
        .collect()
}

fn is_public(name: &str) -> bool {
    !name.starts_with('_')
}

fn public_surface_names(program: &Program) -> Vec<String> {
    let mut names = Vec::new();
    for block in &program.blocks {
        match block {
            Block::Const(decl) if is_public(&decl.name) => names.push(decl.name.clone()),
            Block::Def(def) if is_public(&def.name) => names.push(def.name.clone()),
            Block::Struct(def) if is_public(&def.name) => names.push(def.name.clone()),
            Block::Proc(def) if is_public(&def.name) => names.push(def.name.clone()),
            Block::Namespace(namespace) => {
                names.extend(public_namespace_item_names(&namespace.items));
            }
            _ => {}
        }
    }
    names.sort();
    names.dedup();
    names
}

fn public_namespace_item_names(items: &[NamespaceItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match item {
            NamespaceItem::Const(decl) if is_public(&decl.name) => Some(decl.name.clone()),
            NamespaceItem::Def(def) if is_public(&def.name) => Some(def.name.clone()),
            NamespaceItem::Struct(def) if is_public(&def.name) => Some(def.name.clone()),
            NamespaceItem::Proc(def) if is_public(&def.name) => Some(def.name.clone()),
            NamespaceItem::Namespace(namespace) if is_public(&namespace.name) => {
                Some(namespace.name.clone())
            }
            NamespaceItem::Alias(alias) if is_public(&alias.name) => Some(alias.name.clone()),
            _ => None,
        })
        .collect()
}

fn render_program_surface(out: &mut String, program: &Program) {
    let constants = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Const(decl) if is_public(&decl.name) => Some(decl),
            _ => None,
        })
        .collect::<Vec<_>>();
    let functions = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Def(def) if is_public(&def.name) => Some(def),
            _ => None,
        })
        .collect::<Vec<_>>();

    render_constants(out, 3, "Unqualified constants", &constants);
    render_functions(out, 3, "Unqualified functions", &functions);

    for block in &program.blocks {
        match block {
            Block::Struct(def) if is_public(&def.name) => render_struct(out, 3, def),
            Block::Proc(def) if is_public(&def.name) => render_proc(out, 3, def),
            Block::Namespace(namespace) if is_public(&namespace.name) => {
                out.push_str(&format!(
                    "Namespace: `{}`.\n\n",
                    format_namespace_name(namespace)
                ));
                render_namespace_items(out, 3, &namespace.items);
            }
            _ => {}
        }
    }
}

fn render_namespace_items(out: &mut String, level: usize, items: &[NamespaceItem]) {
    let constants = items
        .iter()
        .filter_map(|item| match item {
            NamespaceItem::Const(decl) if is_public(&decl.name) => Some(decl),
            _ => None,
        })
        .collect::<Vec<_>>();
    let functions = items
        .iter()
        .filter_map(|item| match item {
            NamespaceItem::Def(def) if is_public(&def.name) => Some(def),
            _ => None,
        })
        .collect::<Vec<_>>();

    render_constants(out, level, "Constants", &constants);
    render_functions(out, level, "Functions", &functions);

    for item in items {
        match item {
            NamespaceItem::Struct(def) if is_public(&def.name) => render_struct(out, level, def),
            NamespaceItem::Proc(def) if is_public(&def.name) => render_proc(out, level, def),
            NamespaceItem::Namespace(namespace) if is_public(&namespace.name) => {
                heading(
                    out,
                    level,
                    &format!("Namespace `{}`", format_namespace_name(namespace)),
                );
                render_namespace_items(out, (level + 1).min(6), &namespace.items);
            }
            _ => {}
        }
    }
}

fn render_constants(out: &mut String, level: usize, title: &str, constants: &[&ConstDecl]) {
    if constants.is_empty() {
        return;
    }
    heading(out, level, title);
    onda_block(
        out,
        constants
            .iter()
            .map(|decl| format_const_decl(decl))
            .collect::<Vec<_>>(),
    );
}

fn render_functions(out: &mut String, level: usize, title: &str, functions: &[&FunctionDef]) {
    if functions.is_empty() {
        return;
    }
    heading(out, level, title);
    onda_block(
        out,
        functions
            .iter()
            .map(|def| format_function_signature(def))
            .collect::<Vec<_>>(),
    );
}

fn render_struct(out: &mut String, level: usize, def: &StructDef) {
    heading(
        out,
        level,
        &format!("Struct `{}`", generic_name(&def.name, &def.type_params)),
    );
    let mut lines = vec![format_struct_header(def)];
    lines.extend(
        def.fields
            .iter()
            .map(|field| format!("  {}", format_struct_field(field))),
    );
    lines.extend(
        def.methods
            .iter()
            .filter(|method| is_public(&method.name))
            .map(|method| format!("  {}", format_function_signature(method))),
    );
    lines.dedup();
    onda_block(out, lines);
}

fn render_proc(out: &mut String, level: usize, def: &ProcessorDef) {
    heading(
        out,
        level,
        &format!("Processor `{}`", generic_name(&def.name, &def.type_params)),
    );
    let mut lines = vec![format_proc_header(def)];
    push_port_section(
        &mut lines,
        "ins",
        &def.ins,
        def.ins_deferred_count.as_ref(),
        def.ins_deferred_default_ty.as_ref(),
    );
    let output_label = match def.outs_timing {
        OutputTiming::Sample => "outs",
        OutputTiming::Block => "kouts",
    };
    push_port_section(
        &mut lines,
        output_label,
        &def.outs,
        def.outs_deferred_count.as_ref(),
        def.outs_deferred_default_ty.as_ref(),
    );
    push_param_section(
        &mut lines,
        &def.params,
        def.params_deferred_count.as_ref(),
        def.params_deferred_default_ty.as_ref(),
    );
    push_buffer_section(&mut lines, def);
    let events = def
        .events
        .iter()
        .filter(|event| is_public(&event.name))
        .collect::<Vec<_>>();
    if !events.is_empty() {
        lines.push("  events:".to_owned());
        lines.extend(
            events
                .into_iter()
                .map(|event| format!("    {}", format_event_signature(event))),
        );
    }
    onda_block(out, lines);
}

fn push_port_section(
    lines: &mut Vec<String>,
    label: &str,
    ports: &[onda_frontend::PortDecl],
    count: Option<&Expr>,
    default_ty: Option<&DeclType>,
) {
    if ports.is_empty() && count.is_none() {
        return;
    }
    let default_ty = default_ty.map(|ty| format!("<{}>", format_decl_type(ty)));
    lines.push(format!(
        "  {}",
        count_section_header(label, count, default_ty.as_deref(), !ports.is_empty())
    ));
    lines.extend(
        ports
            .iter()
            .map(|port| format!("    {}", format_port_decl(port))),
    );
}

fn push_param_section(
    lines: &mut Vec<String>,
    params: &[onda_frontend::ParamDecl],
    count: Option<&Expr>,
    default_ty: Option<&DeclType>,
) {
    if params.is_empty() && count.is_none() {
        return;
    }
    let default_ty = default_ty.map(|ty| format!("<{}>", format_decl_type(ty)));
    lines.push(format!(
        "  {}",
        count_section_header("params", count, default_ty.as_deref(), !params.is_empty())
    ));
    lines.extend(
        params
            .iter()
            .map(|param| format!("    {}", format_param_decl(param))),
    );
}

fn push_buffer_section(lines: &mut Vec<String>, def: &ProcessorDef) {
    if def.buffers.is_empty() && def.buffers_deferred_count.is_none() {
        return;
    }
    let default_ty = def
        .buffers_deferred_default_ty
        .as_ref()
        .map(format_buffer_section_default_type);
    lines.push(format!(
        "  {}",
        count_section_header(
            "buffers",
            def.buffers_deferred_count.as_ref(),
            default_ty.as_deref(),
            !def.buffers.is_empty(),
        )
    ));
    lines.extend(
        def.buffers
            .iter()
            .map(|buffer| format!("    {}", format_buffer_decl(buffer))),
    );
}

fn count_section_header(
    label: &str,
    count: Option<&Expr>,
    default_ty: Option<&str>,
    has_declarations: bool,
) -> String {
    let mut header = label.to_owned();
    if let Some(default_ty) = default_ty {
        header.push_str(default_ty);
    }
    if let Some(count) = count {
        header.push(' ');
        header.push_str(&format_expr(count));
    }
    if has_declarations || count.is_none() {
        header.push(':');
    }
    header
}

fn format_namespace_name(namespace: &NamespaceDecl) -> String {
    if namespace.params.is_empty() {
        namespace.name.clone()
    } else {
        let params = namespace
            .params
            .iter()
            .map(|param| format!("{} = {}", param.name, format_expr(&param.default)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{params}>", namespace.name)
    }
}

fn generic_name(name: &str, type_params: &[String]) -> String {
    if type_params.is_empty() {
        name.to_owned()
    } else {
        format!("{name}<{}>", type_params.join(", "))
    }
}

fn heading(out: &mut String, level: usize, title: &str) {
    out.push_str(&format!("{} {title}\n\n", "#".repeat(level.min(6))));
}

fn onda_block(out: &mut String, lines: Vec<String>) {
    out.push_str("```onda\n");
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("```\n\n");
}

fn code_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("`{item}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn anchor(module: &str) -> String {
    module.replace('/', "")
}

#[cfg(test)]
mod tests {
    use super::generate_stdlib_reference;

    #[test]
    fn generated_reference_contains_module_and_processor_surfaces() {
        let docs = generate_stdlib_reference().expect("stdlib reference should generate");
        assert!(docs.contains("## `std/osc`"));
        assert!(docs.contains("proc Saw<T>:"));
        assert!(docs.contains("freq: T = 440.0 => update_freq"));
        assert!(docs.contains("## `std/prelude`"));
        assert!(!docs.contains("def _hann_window"));
    }
}
