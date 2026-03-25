use std::collections::{HashMap, HashSet};

use crate::processor_lowering::{
    coerce_typed_events, collect_runtime_state_roots, desugar_processors,
    internal_proc_index_call_signature, lower_graph_blocks, validated_sample_oversample_factor,
    ProcessorDesugarResult,
};
use crate::*;

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

pub fn analyze(program: Program) -> Result<TypedProgram, Vec<Diagnostic>> {
    analyze_with_options(program, AnalysisOptions::default())
}

pub fn lower_graphs_for_inspection_with_options(
    program: Program,
    options: AnalysisOptions,
) -> Result<Program, Vec<Diagnostic>> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'sample_rate' must be finite and greater than zero",
        )]);
    }
    if options.block_size == 0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'block_size' must be greater than zero",
        )]);
    }

    let mut program = program;
    inject_auto_std_math(&mut program)?;

    let mut errors = Vec::new();
    for block in &program.blocks {
        let Block::Assert(assert_decl) = block else {
            continue;
        };
        let context = "assert condition";
        if let Some(passed) = eval_const_bool_expr(&assert_decl.expr, options, context, &mut errors)
        {
            if !passed {
                errors.push(Diagnostic::semantic_span(
                    "assert failed",
                    assert_decl.expr.loc(),
                ));
            }
        }
    }
    program.blocks.retain(|b| !matches!(b, Block::Assert(_)));
    if !errors.is_empty() {
        return Err(errors);
    }

    lower_graph_blocks(&mut program, options, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(program)
}

pub fn analyze_with_options(
    program: Program,
    options: AnalysisOptions,
) -> Result<TypedProgram, Vec<Diagnostic>> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'sample_rate' must be finite and greater than zero",
        )]);
    }
    if options.block_size == 0 {
        return Err(vec![Diagnostic::internal(
            "analysis option 'block_size' must be greater than zero",
        )]);
    }

    let original_last_block_loc = program
        .blocks
        .iter()
        .rev()
        .map(Block::loc)
        .find(|loc| !loc.is_zero());
    let mut program = program;
    inject_auto_std_math(&mut program)?;

    let mut errors = Vec::new();
    for block in &program.blocks {
        let Block::Assert(assert_decl) = block else {
            continue;
        };
        let context = "assert condition";
        if let Some(passed) = eval_const_bool_expr(&assert_decl.expr, options, context, &mut errors)
        {
            if !passed {
                errors.push(Diagnostic::semantic_span(
                    "assert failed",
                    assert_decl.expr.loc(),
                ));
            }
        }
    }
    program.blocks.retain(|b| !matches!(b, Block::Assert(_)));
    if !errors.is_empty() {
        return Err(errors);
    }

    let ProcessorDesugarResult {
        program,
        def_sample_oversample_factors,
        proc_step_oversample_meta,
    } = desugar_processors(program, options, &mut errors);

    let mut seen_singleton = HashSet::new();
    for block in &program.blocks {
        let kind = block.kind();
        if matches!(kind, BlockKind::Def | BlockKind::Struct | BlockKind::Proc) {
            continue;
        }
        if !seen_singleton.insert(kind) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate block '{:?}'", kind).to_lowercase(),
                block.loc(),
            ));
        }
    }

    let ins_explicit = program.block(BlockKind::Ins).is_some();
    let raw_ins = match program.block(BlockKind::Ins) {
        Some(Block::Ins(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let outs_explicit = program.block(BlockKind::Outs).is_some();
    let raw_outs = match program.block(BlockKind::Outs) {
        Some(Block::Outs(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let params_explicit = program.block(BlockKind::Params).is_some();
    let params = match program.block(BlockKind::Params) {
        Some(Block::Params(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let mut events = match program.block(BlockKind::Events) {
        Some(Block::Events(v)) => v.events.clone(),
        _ => Vec::new(),
    };
    let buffers = match program.block(BlockKind::Buffers) {
        Some(Block::Buffers(v)) => v.decls.clone(),
        _ => Vec::new(),
    };
    let mut struct_defs_raw = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Struct(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Def(def) => Some(def.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();

    for def in &defs {
        if !def.type_params.is_empty() {
            let mut seen = HashSet::new();
            for tp in &def.type_params {
                if !seen.insert(tp.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!("duplicate type parameter '{}' in def '{}'", tp, def.name),
                        def.loc,
                    ));
                }
            }
        }
    }

    let (init_default_decl_ty, mut init) = match program.block(BlockKind::Init) {
        Some(Block::Init(v)) => (v.default_ty.clone(), v.body.clone()),
        _ => (None, Vec::new()),
    };
    let mut block_pre = Vec::new();
    let mut block_post = Vec::new();
    let mut nested_block_sample = None;
    if let Some(Block::Block(exec)) = program.block(BlockKind::Block) {
        block_pre = exec.pre.clone();
        block_post = exec.post.clone();
        nested_block_sample = exec.sample.clone();
        if nested_block_sample.is_none() {
            errors.push(Diagnostic::semantic_span(
                "block section must include nested 'sample' block",
                exec.loc.as_ref(),
            ));
        }
    }
    let top_sample = match program.block(BlockKind::Sample) {
        Some(Block::Sample(v)) => Some(v.clone()),
        _ => None,
    };
    let sample_conflict_loc = top_sample
        .as_ref()
        .and_then(|sample| sample.loc.cloned())
        .or_else(|| {
            nested_block_sample
                .as_ref()
                .and_then(|sample| sample.loc.cloned())
        });

    let sample_block = match (nested_block_sample, top_sample) {
        (Some(_), Some(_)) => {
            errors.push(Diagnostic::semantic_span(
                "sample block cannot be declared both at top-level and inside block",
                sample_conflict_loc.as_ref(),
            ));
            SampleBlock {
                loc: Default::default(),
                oversample_factor: None,
                body: Vec::new(),
            }
        }
        (Some(v), None) => v,
        (None, Some(v)) => v,
        (None, None) => SampleBlock {
            loc: Default::default(),
            oversample_factor: None,
            body: Vec::new(),
        },
    };
    let sample_oversample_factor = validated_sample_oversample_factor(
        sample_block.oversample_factor.as_ref(),
        "sample block",
        &mut errors,
    );
    let mut sample = sample_block.body;

    let missing_sample_loc = original_last_block_loc.as_ref();
    if sample.is_empty()
        && program.block(BlockKind::Block).is_none()
        && requires_entry_sample(&program)
    {
        errors.push(Diagnostic::semantic_span(
            "missing required 'sample' block",
            missing_sample_loc,
        ));
    }

    {
        let struct_symbols = struct_defs_raw
            .iter()
            .map(|s| s.name.clone())
            .collect::<HashSet<_>>();
        let mut callable_symbols = defs.iter().map(|d| d.name.clone()).collect::<HashSet<_>>();
        for p in program.blocks.iter().filter_map(|b| match b {
            Block::Proc(proc_def) => Some(proc_def),
            _ => None,
        }) {
            callable_symbols.insert(p.name.clone());
        }
        for s in &struct_defs_raw {
            callable_symbols.insert(s.name.clone());
            for method in &s.methods {
                callable_symbols.insert(format!("{}.{}", s.name, method.name));
            }
        }
        let struct_namespaces = collect_declared_namespaces(&struct_symbols);
        let callable_namespaces = collect_declared_namespaces(&callable_symbols);

        for s in &mut struct_defs_raw {
            let struct_ns = namespace_of_symbol(&s.name);
            for field in &mut s.fields {
                if let FieldType::Array(spec) = &mut field.ty {
                    if let ArrayElemType::Struct(name) = &mut spec.elem {
                        qualify_struct_type_name(
                            name,
                            &struct_ns,
                            &struct_symbols,
                            &struct_namespaces,
                            &format!("struct '{}.{}' array element type", s.name, field.name),
                            &mut errors,
                        );
                    }
                }
                if let Some(default) = &mut field.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("struct '{}.{}' default", s.name, field.name),
                    );
                }
            }
            for method in &mut s.methods {
                for param in &mut method.params {
                    if let Some(FnParamType::Struct(name)) = &mut param.ty {
                        qualify_struct_type_name(
                            name,
                            &struct_ns,
                            &struct_symbols,
                            &struct_namespaces,
                            &format!(
                                "method '{}.{}' parameter '{}'",
                                s.name, method.name, param.name
                            ),
                            &mut errors,
                        );
                    }
                    if let Some(default) = &mut param.default {
                        qualify_expr_namespaced_symbols(
                            default,
                            &struct_ns,
                            &callable_symbols,
                            &callable_namespaces,
                            &struct_symbols,
                            &struct_namespaces,
                            &mut errors,
                            &format!(
                                "method '{}.{}' parameter '{}' default",
                                s.name, method.name, param.name
                            ),
                        );
                    }
                }
                for stmt in &mut method.body {
                    qualify_stmt_namespaced_symbols(
                        stmt,
                        &struct_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("method '{}.{}' body", s.name, method.name),
                    );
                }
            }
        }

        for def in &mut defs {
            let def_ns = namespace_of_symbol(&def.name);
            for param in &mut def.params {
                if let Some(FnParamType::Struct(name)) = &mut param.ty {
                    qualify_struct_type_name(
                        name,
                        &def_ns,
                        &struct_symbols,
                        &struct_namespaces,
                        &format!("function '{}' parameter '{}'", def.name, param.name),
                        &mut errors,
                    );
                }
                if let Some(default) = &mut param.default {
                    qualify_expr_namespaced_symbols(
                        default,
                        &def_ns,
                        &callable_symbols,
                        &callable_namespaces,
                        &struct_symbols,
                        &struct_namespaces,
                        &mut errors,
                        &format!("function '{}' parameter '{}' default", def.name, param.name),
                    );
                }
            }
            for stmt in &mut def.body {
                qualify_stmt_namespaced_symbols(
                    stmt,
                    &def_ns,
                    &callable_symbols,
                    &callable_namespaces,
                    &struct_symbols,
                    &struct_namespaces,
                    &mut errors,
                    &format!("function '{}' body", def.name),
                );
            }
        }

        for stmt in &mut init {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "init",
            );
        }
        for stmt in &mut block_pre {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "block pre",
            );
        }
        for stmt in &mut block_post {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "block post",
            );
        }
        for stmt in &mut sample {
            qualify_stmt_namespaced_symbols(
                stmt,
                "",
                &callable_symbols,
                &callable_namespaces,
                &struct_symbols,
                &struct_namespaces,
                &mut errors,
                "sample",
            );
        }
        for event in &mut events {
            for stmt in &mut event.body {
                qualify_stmt_namespaced_symbols(
                    stmt,
                    "",
                    &callable_symbols,
                    &callable_namespaces,
                    &struct_symbols,
                    &struct_namespaces,
                    &mut errors,
                    &format!("event '{}'", event.name),
                );
            }
        }
    }

    let generic_struct_template_names: HashSet<String>;
    {
        let mut concrete_structs = Vec::<StructDef>::new();
        let mut generic_templates = HashMap::<String, StructDef>::new();
        for s in struct_defs_raw.drain(..) {
            if s.type_params.is_empty() {
                concrete_structs.push(s);
                continue;
            }
            if generic_templates.contains_key(&s.name) {
                errors.push(Diagnostic::semantic_span(
                    format!("duplicate generic struct '{}'", s.name),
                    s.loc,
                ));
                continue;
            }
            let mut seen = HashSet::new();
            for tp in &s.type_params {
                if !seen.insert(tp.clone()) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "duplicate generic type parameter '{}' in struct '{}'",
                            tp, s.name
                        ),
                        s.loc,
                    ));
                }
            }
            for method in &s.methods {
                for tp in &method.type_params {
                    if s.type_params.contains(tp) {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "type parameter '{}' on method '{}.{}' shadows '{}' from struct '{}'; use a different name",
                                tp, s.name, method.name, tp, s.name
                            ),
                            method.loc,
                        ));
                    }
                }
            }
            generic_templates.insert(s.name.clone(), s);
        }
        generic_struct_template_names = generic_templates.keys().cloned().collect();

        let mut generated_specializations = HashMap::<String, StructDef>::new();
        for s in &mut concrete_structs {
            rewrite_generic_struct_field_types(
                s,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
            for field in &mut s.fields {
                if let Some(default) = &mut field.default {
                    let mut locals = GenericInferenceLocals::default();
                    rewrite_generic_struct_ctor_expr(
                        default,
                        &generic_templates,
                        &mut generated_specializations,
                        &mut errors,
                        &mut locals,
                    );
                }
            }
            for method in &mut s.methods {
                rewrite_generic_struct_ctor_stmt_list(
                    &mut method.body,
                    &generic_templates,
                    &mut generated_specializations,
                    &mut errors,
                );
            }
        }
        for def in &mut defs {
            rewrite_generic_struct_ctor_stmt_list(
                &mut def.body,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
        }
        rewrite_generic_struct_ctor_stmt_list(
            &mut init,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut block_pre,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut block_post,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        rewrite_generic_struct_ctor_stmt_list(
            &mut sample,
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );
        for event in &mut events {
            rewrite_generic_struct_ctor_stmt_list(
                &mut event.body,
                &generic_templates,
                &mut generated_specializations,
                &mut errors,
            );
        }

        finalize_generated_generic_struct_specializations(
            &generic_templates,
            &mut generated_specializations,
            &mut errors,
        );

        struct_defs_raw = concrete_structs;
        let mut generated = generated_specializations.into_values().collect::<Vec<_>>();
        generated.sort_by(|a, b| a.name.cmp(&b.name));
        struct_defs_raw.extend(generated);
    }

    check_local_port_duplicates(&raw_ins, "input", &mut errors);
    check_local_port_duplicates(&raw_outs, "output", &mut errors);

    let inferred_io = infer_numbered_io_from_sample(&sample);
    let ins_ports = normalize_numbered_port_decls(&raw_ins, "in", inferred_io.max_in);
    let outs_ports = normalize_numbered_port_decls(&raw_outs, "out", inferred_io.max_out);
    let input_declared_names = ins_ports.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let output_declared_names = outs_ports
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let (ins, in_types, in_arrays, in_defaults, in_ranges) =
        expand_port_decls(&ins_ports, "input", options, &mut errors);
    let (outs, out_types, out_arrays, _out_defaults, _out_ranges) =
        expand_port_decls(&outs_ports, "output", options, &mut errors);

    let (typed_params, param_arrays) = coerce_params(&params, options, &mut errors);
    let typed_buffers = coerce_buffers(&buffers, options, &mut errors);
    let param_types = typed_params
        .iter()
        .map(|p| (p.name.clone(), p.ty))
        .collect::<HashMap<_, _>>();
    let param_ranges = typed_params
        .iter()
        .filter_map(|p| p.range.map(|r| (p.name.clone(), r)))
        .collect::<HashMap<_, _>>();
    let mut occupied_temp_names = HashSet::<String>::new();
    occupied_temp_names.extend(input_declared_names.iter().cloned());
    occupied_temp_names.extend(output_declared_names.iter().cloned());
    occupied_temp_names.extend(params.iter().map(|p| p.name.clone()));
    occupied_temp_names.extend(typed_buffers.iter().map(|b| b.name.clone()));

    let mut make_unique_temp = |base: String| -> String {
        if occupied_temp_names.insert(base.clone()) {
            return base;
        }
        let mut idx = 1usize;
        loop {
            let candidate = format!("{base}_{idx}");
            if occupied_temp_names.insert(candidate.clone()) {
                return candidate;
            }
            idx += 1;
        }
    };

    let mut input_aliases = HashMap::<String, String>::new();
    let mut input_hoists = Vec::<Stmt>::new();
    let mut input_names = in_ranges.keys().cloned().collect::<Vec<_>>();
    input_names.sort();
    for name in input_names {
        let Some(range) = in_ranges.get(&name).copied() else {
            continue;
        };
        let ty = *in_types.get(&name).unwrap_or(&PrimitiveType::F32);
        let alias = make_unique_temp(format!(
            "__omni_clamped_in__{}",
            sanitize_symbol_component(&name)
        ));
        input_aliases.insert(name.clone(), alias.clone());
        input_hoists.push(build_top_level_range_hoist_assign(alias, &name, ty, range));
    }

    let mut param_aliases = HashMap::<String, String>::new();
    let mut param_hoists = Vec::<Stmt>::new();
    let mut param_names_sorted = param_ranges.keys().cloned().collect::<Vec<_>>();
    param_names_sorted.sort();
    for name in param_names_sorted {
        let Some(range) = param_ranges.get(&name).copied() else {
            continue;
        };
        let ty = *param_types.get(&name).unwrap_or(&PrimitiveType::F32);
        let alias = make_unique_temp(format!(
            "__omni_clamped_param__{}",
            sanitize_symbol_component(&name)
        ));
        param_aliases.insert(name.clone(), alias.clone());
        param_hoists.push(build_top_level_range_hoist_assign(alias, &name, ty, range));
    }

    for stmt in &mut block_pre {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, false, true);
    }
    for stmt in &mut sample {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, true, true);
    }
    for stmt in &mut block_post {
        rewrite_top_level_range_clamps_in_stmt(stmt, &input_aliases, &param_aliases, false, true);
    }

    if !param_hoists.is_empty() {
        let mut rewritten = param_hoists;
        rewritten.append(&mut block_pre);
        block_pre = rewritten;
    }
    if !input_hoists.is_empty() {
        let mut rewritten = input_hoists;
        rewritten.append(&mut sample);
        sample = rewritten;
    }

    let mut all_declared = HashSet::new();
    check_unique_set(
        &input_declared_names,
        "input",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &output_declared_names,
        "output",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &params.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        "param",
        &mut all_declared,
        &mut errors,
    );
    check_unique_set(
        &typed_buffers
            .iter()
            .map(|b| b.name.clone())
            .collect::<Vec<_>>(),
        "buffer",
        &mut all_declared,
        &mut errors,
    );

    let mut struct_defs = HashMap::new();
    let mut typed_structs = Vec::new();
    let mut method_self_struct = HashMap::<String, String>::new();
    let mut callable_symbols_for_method_sugar =
        defs.iter().map(|d| d.name.clone()).collect::<HashSet<_>>();
    for s in &struct_defs_raw {
        for method in &s.methods {
            callable_symbols_for_method_sugar.insert(format!("{}.{}", s.name, method.name));
        }
    }
    for s in &struct_defs_raw {
        if is_builtin_constant_name(&s.name) {
            errors.push(Diagnostic::semantic_span(
                format!("struct name '{}' is reserved as a builtin constant", s.name),
                s.loc,
            ));
            continue;
        }
        if all_declared.contains(&s.name) {
            errors.push(Diagnostic::semantic_span(
                format!("struct name '{}' conflicts with existing symbol", s.name),
                s.loc,
            ));
            continue;
        }
        if struct_defs.contains_key(&s.name) {
            errors.push(Diagnostic::semantic_span(
                format!("duplicate struct '{}'", s.name),
                s.loc,
            ));
            continue;
        }
        let typed_fields = coerce_struct_fields(
            &s.name,
            &s.type_params,
            &s.fields,
            &struct_defs,
            options,
            &mut errors,
        );
        struct_defs.insert(s.name.clone(), typed_fields.clone());
        typed_structs.push(TypedStruct {
            name: s.name.clone(),
            fields: typed_fields,
        });
        all_declared.insert(s.name.clone());

        for method in &s.methods {
            for tp in &method.type_params {
                if s.type_params.contains(tp) {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "type parameter '{}' on method '{}.{}' shadows '{}' from struct '{}'; use a different name",
                            tp, s.name, method.name, tp, s.name
                        ),
                        method.loc,
                    ));
                }
            }
            if !method.type_params.is_empty() {
                let mut seen = HashSet::new();
                for tp in &method.type_params {
                    if !seen.insert(tp.clone()) {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "duplicate type parameter '{}' in method '{}.{}'",
                                tp, s.name, method.name
                            ),
                            method.loc,
                        ));
                    }
                }
            }
            if method.params.first().map(|p| p.name.as_str()) != Some("self") {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "method '{}.{}' must declare 'self' as first parameter",
                        s.name, method.name
                    ),
                    method.loc,
                ));
            }
            let fq_name = format!("{}.{}", s.name, method.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                method_self_struct.insert(fq_name.clone(), s.name.clone());
            }
            let mut desugared_method_body = method.body.clone();
            let mut method_struct_instances = HashMap::<String, String>::new();
            let mut method_struct_array_roots = HashMap::<String, String>::new();
            let method_ns = namespace_of_symbol(&s.name);
            if method.params.first().map(|p| p.name.as_str()) == Some("self") {
                register_struct_instance_and_array_roots(
                    "self",
                    &s.name,
                    &struct_defs,
                    &mut method_struct_instances,
                    &mut method_struct_array_roots,
                );
            }
            for stmt in &mut desugared_method_body {
                desugar_init_instance_method_calls(
                    stmt,
                    &mut method_struct_instances,
                    &mut method_struct_array_roots,
                    &struct_defs,
                    &method_ns,
                    &callable_symbols_for_method_sugar,
                );
            }
            defs.push(FunctionDef {
                loc: method.loc.clone(),
                type_params: method.type_params.clone(),
                name: fq_name,
                params: method.params.clone(),
                body: desugared_method_body,
            });
        }
    }

    for (struct_name, fields) in &struct_defs {
        for field in fields {
            if let Some(elem_struct) = &field.array_elem_struct {
                let context = format!("field '{}.{}' array element", struct_name, field.name);
                let _ =
                    validate_data_struct_layout(elem_struct, &struct_defs, &context, &mut errors);
            }
        }
    }

    let mut desugar_struct_instances = HashMap::<String, String>::new();
    let mut desugar_struct_array_roots = HashMap::<String, String>::new();
    for stmt in &mut init {
        desugar_init_instance_method_calls(
            stmt,
            &mut desugar_struct_instances,
            &mut desugar_struct_array_roots,
            &struct_defs,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut block_pre {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut block_post {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for stmt in &mut sample {
        desugar_sample_instance_method_calls(
            stmt,
            &desugar_struct_instances,
            &desugar_struct_array_roots,
            "",
            &callable_symbols_for_method_sugar,
        );
    }
    for event in &mut events {
        for stmt in &mut event.body {
            desugar_sample_instance_method_calls(
                stmt,
                &desugar_struct_instances,
                &desugar_struct_array_roots,
                "",
                &callable_symbols_for_method_sugar,
            );
        }
    }
    for def in &mut defs {
        if method_self_struct.contains_key(&def.name) {
            continue;
        }
        let mut def_struct_instances = HashMap::<String, String>::new();
        let mut def_struct_array_roots = HashMap::<String, String>::new();
        for param in &def.params {
            if let Some(FnParamType::Struct(struct_name)) = &param.ty {
                if struct_defs.contains_key(struct_name) {
                    register_struct_instance_and_array_roots(
                        &param.name,
                        struct_name,
                        &struct_defs,
                        &mut def_struct_instances,
                        &mut def_struct_array_roots,
                    );
                }
            }
        }
        let def_ns = namespace_of_symbol(&def.name);
        for stmt in &mut def.body {
            desugar_sample_instance_method_calls(
                stmt,
                &def_struct_instances,
                &def_struct_array_roots,
                &def_ns,
                &callable_symbols_for_method_sugar,
            );
        }
    }

    let (overload_candidates, def_public_name_by_internal) =
        crate::def_semantics::prepare_function_overloads(&mut defs);
    let method_self_struct_internal = defs
        .iter()
        .filter_map(|def| {
            let public_name = def_public_name_by_internal
                .get(&def.name)
                .cloned()
                .unwrap_or_else(|| def.name.clone());
            method_self_struct
                .get(&public_name)
                .cloned()
                .map(|owner| (def.name.clone(), owner))
        })
        .collect::<HashMap<_, _>>();

    let mut top_level_env = crate::def_semantics::OverloadRewriteEnv::default();
    top_level_env
        .scalar_types
        .extend(in_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .scalar_types
        .extend(out_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env
        .scalar_types
        .extend(param_types.iter().map(|(name, ty)| (name.clone(), *ty)));
    top_level_env.array_elem_types.extend(
        in_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.array_elem_types.extend(
        out_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.array_elem_types.extend(
        param_arrays
            .iter()
            .map(|(name, info)| (name.clone(), info.elem_ty)),
    );
    top_level_env.buffer_types.extend(
        typed_buffers
            .iter()
            .map(|b| (b.name.clone(), (b.elem_ty, b.channels.clone()))),
    );
    top_level_env
        .struct_instances
        .extend(desugar_struct_instances.clone());

    let mut init_rewrite_env = top_level_env.clone();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut init,
        &mut init_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut block_pre_rewrite_env = top_level_env.clone();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut block_pre,
        &mut block_pre_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut sample_rewrite_env = top_level_env.clone();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut sample,
        &mut sample_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    let mut block_post_rewrite_env = top_level_env.clone();
    crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
        &mut block_post,
        &mut block_post_rewrite_env,
        &overload_candidates,
        &struct_defs,
        &mut errors,
    );
    for event in &mut events {
        let mut event_env = top_level_env.clone();
        crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
            &mut event.body,
            &mut event_env,
            &overload_candidates,
            &struct_defs,
            &mut errors,
        );
    }
    for def in &mut defs {
        let mut def_env = top_level_env.clone();
        for param in &mut def.params {
            match &param.ty {
                Some(FnParamType::Primitive(prim)) => {
                    def_env.scalar_types.insert(param.name.clone(), *prim);
                }
                Some(FnParamType::Struct(struct_name)) => {
                    def_env
                        .struct_instances
                        .insert(param.name.clone(), struct_name.clone());
                }
                Some(FnParamType::Buffer(buffer_ty)) => {
                    let channels = match &buffer_ty.channels {
                        BufferChannels::Mono => TypedBufferChannels::Mono,
                        BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                        BufferChannels::Static(expr) => {
                            crate::def_semantics::const_positive_usize_for_overload(expr)
                                .map(TypedBufferChannels::Static)
                                .unwrap_or(TypedBufferChannels::Dynamic)
                        }
                    };
                    let elem_ty = match &buffer_ty.elem {
                        BufferElemType::Primitive(ty) => *ty,
                        BufferElemType::Generic(_) => PrimitiveType::F32,
                    };
                    def_env
                        .buffer_types
                        .insert(param.name.clone(), (elem_ty, channels));
                }
                Some(FnParamType::Array(Some(prim))) => {
                    def_env.array_elem_types.insert(param.name.clone(), *prim);
                }
                Some(FnParamType::SizedArray {
                    elem: Some(prim), ..
                }) => {
                    def_env.array_elem_types.insert(param.name.clone(), *prim);
                }
                Some(FnParamType::ArrayGeneric(_))
                | Some(FnParamType::SizedArray { .. })
                | Some(FnParamType::Tuple(_)) => {}
                Some(FnParamType::Array(None)) | Some(FnParamType::BareBuffer) | None => {}
            }
            if let Some(default_expr) = &mut param.default {
                crate::def_semantics::rewrite_overloaded_calls_in_expr(
                    default_expr,
                    &def_env,
                    &overload_candidates,
                    &mut errors,
                );
            }
        }
        crate::def_semantics::rewrite_overloaded_calls_in_stmt_list(
            &mut def.body,
            &mut def_env,
            &overload_candidates,
            &struct_defs,
            &mut errors,
        );
    }

    let mut fn_signatures: HashMap<String, FnSignature> = HashMap::new();
    let mut seen_public_function_symbols = HashSet::<String>::new();
    for def in &defs {
        let public_name = def_public_name_by_internal
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| def.name.clone());
        let def_diag = DiagCtx::new(def.loc);
        if is_builtin_constant_name(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!(
                    "function name '{}' is reserved as a builtin constant",
                    public_name
                ),
            );
            continue;
        }
        if is_builtin_function_name(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("cannot redefine builtin function '{}'", public_name),
            );
            continue;
        }
        if struct_defs.contains_key(&public_name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("function name '{}' conflicts with struct name", public_name),
            );
            continue;
        }
        if all_declared.contains(&public_name)
            && !seen_public_function_symbols.contains(&public_name)
        {
            push_semantic(
                def_diag,
                &mut errors,
                format!(
                    "function name '{}' conflicts with existing symbol",
                    public_name
                ),
            );
            continue;
        }
        if fn_signatures.contains_key(&def.name) {
            push_semantic(
                def_diag,
                &mut errors,
                format!("duplicate function '{}'", def.name),
            );
            continue;
        }
        fn_signatures.insert(
            def.name.clone(),
            FnSignature {
                params: def.params.iter().map(|p| p.name.clone()).collect(),
                defaults: def.params.iter().map(|p| p.default.clone()).collect(),
                param_types: def.params.iter().map(|p| p.ty.clone()).collect(),
                type_params: def.type_params.clone(),
            },
        );
        if seen_public_function_symbols.insert(public_name.clone()) {
            all_declared.insert(public_name.clone());
        }

        let mut local_params = HashSet::new();
        for p in &def.params {
            let param_diag = DiagCtx::new(p.ty_loc.or(p.loc));
            if is_builtin_constant_name(&p.name) {
                push_semantic(
                    param_diag,
                    &mut errors,
                    format!(
                        "function parameter '{}' in '{}' is reserved as a builtin constant",
                        p.name, def.name
                    ),
                );
            }
            if !local_params.insert(p.name.clone()) {
                push_semantic(
                    param_diag,
                    &mut errors,
                    format!(
                        "duplicate function parameter '{}' in '{}'",
                        p.name, public_name
                    ),
                );
            }
            if let Some(default) = &p.default {
                if matches!(
                    p.ty,
                    Some(FnParamType::Buffer(_))
                        | Some(FnParamType::Array(_))
                        | Some(FnParamType::ArrayGeneric(_))
                        | Some(FnParamType::BareBuffer)
                ) {
                    push_semantic(
                        param_diag,
                        &mut errors,
                        format!(
                            "function parameter '{}.{}' is a buffer and cannot have a default value",
                            public_name, p.name
                        ),
                    );
                }
                validate_default_expr(
                    default,
                    &mut errors,
                    &format!("function parameter '{}.{}'", public_name, p.name),
                );
            }
        }
    }

    // --- Pre-mono validation: check generic def call-site type args ---
    validate_generic_def_type_args_in_stmts(&init, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&block_pre, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&sample, &fn_signatures, &mut errors);
    validate_generic_def_type_args_in_stmts(&block_post, &fn_signatures, &mut errors);
    for event in &events {
        validate_generic_def_type_args_in_stmts(&event.body, &fn_signatures, &mut errors);
    }
    for def in &defs {
        validate_generic_def_type_args_in_stmts(&def.body, &fn_signatures, &mut errors);
    }

    // --- Def monomorphization pass ---
    // Identify defs whose parameters require monomorphization (generic struct,
    // untyped array `[]`, bare `buffer`, or generic def type params `<T>`).
    {
        let mono_eligible: HashSet<String> = fn_signatures
            .iter()
            .filter_map(|(name, sig)| {
                let needs_mono = !sig.type_params.is_empty()
                    || sig.param_types.iter().any(|pt| match pt {
                        Some(FnParamType::Struct(s))
                            if generic_struct_template_names.contains(s) =>
                        {
                            true
                        }
                        Some(FnParamType::Array(None))
                        | Some(FnParamType::ArrayGeneric(_))
                        | Some(FnParamType::SizedArray {
                            generic_name: Some(_),
                            ..
                        })
                        | Some(FnParamType::BareBuffer) => true,
                        _ => false,
                    });
                if needs_mono {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        // Also run mono pass when any def has untyped params that could be
        // inferred as tuple from call-site tuple literal args.
        let has_untyped_params = fn_signatures
            .values()
            .any(|sig| sig.param_types.iter().any(|pt| pt.is_none()));

        if !mono_eligible.is_empty() || has_untyped_params {
            let mut generated_defs = Vec::<FunctionDef>::new();
            let mut generated_sigs = HashMap::<String, FnSignature>::new();
            let mut mono_cache =
                HashMap::<(String, Vec<crate::def_semantics::MonoParamKey>), String>::new();
            let original_defs_snapshot = defs.clone();

            // Rewrite calls in-place across all scopes.
            crate::def_semantics::monomorphize_calls_in_stmts(
                &mut init,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
                &mut errors,
                &[],
            );
            crate::def_semantics::monomorphize_calls_in_stmts(
                &mut block_pre,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
                &mut errors,
                &[],
            );
            crate::def_semantics::monomorphize_calls_in_stmts(
                &mut sample,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
                &mut errors,
                &[],
            );
            crate::def_semantics::monomorphize_calls_in_stmts(
                &mut block_post,
                &top_level_env,
                &mono_eligible,
                &fn_signatures,
                &defs,
                &generic_struct_template_names,
                &struct_defs,
                &mut generated_defs,
                &mut generated_sigs,
                &mut mono_cache,
                &mut errors,
                &[],
            );
            for event in &mut events {
                crate::def_semantics::monomorphize_calls_in_stmts(
                    &mut event.body,
                    &top_level_env,
                    &mono_eligible,
                    &fn_signatures,
                    &defs,
                    &generic_struct_template_names,
                    &struct_defs,
                    &mut generated_defs,
                    &mut generated_sigs,
                    &mut mono_cache,
                    &mut errors,
                    &[],
                );
            }
            // Also walk def bodies (def-to-def mono calls)
            for def in &mut defs {
                let mut def_env = top_level_env.clone();
                for param in &def.params {
                    match &param.ty {
                        Some(FnParamType::Primitive(prim)) => {
                            def_env.scalar_types.insert(param.name.clone(), *prim);
                        }
                        Some(FnParamType::Struct(struct_name)) => {
                            def_env
                                .struct_instances
                                .insert(param.name.clone(), struct_name.clone());
                        }
                        Some(FnParamType::Array(Some(prim))) => {
                            def_env.array_elem_types.insert(param.name.clone(), *prim);
                        }
                        Some(FnParamType::ArrayGeneric(_)) => {}
                        Some(FnParamType::Buffer(buffer_ty)) => {
                            let channels = match &buffer_ty.channels {
                                BufferChannels::Mono => TypedBufferChannels::Mono,
                                BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
                                BufferChannels::Static(expr) => {
                                    crate::def_semantics::const_positive_usize_for_overload(expr)
                                        .map(TypedBufferChannels::Static)
                                        .unwrap_or(TypedBufferChannels::Dynamic)
                                }
                            };
                            let elem_ty = match &buffer_ty.elem {
                                BufferElemType::Primitive(ty) => *ty,
                                BufferElemType::Generic(_) => PrimitiveType::F32,
                            };
                            def_env
                                .buffer_types
                                .insert(param.name.clone(), (elem_ty, channels));
                        }
                        _ => {}
                    }
                }
                crate::def_semantics::monomorphize_calls_in_stmts(
                    &mut def.body,
                    &def_env,
                    &mono_eligible,
                    &fn_signatures,
                    &original_defs_snapshot,
                    &generic_struct_template_names,
                    &struct_defs,
                    &mut generated_defs,
                    &mut generated_sigs,
                    &mut mono_cache,
                    &mut errors,
                    &def.type_params,
                );
            }

            // Mono-rewrite generated defs' bodies (def-to-def mono calls).
            // E.g. quad.__mono__g_f32 may call double(...) which also needs mono.
            // Loop until no new defs are generated.
            loop {
                let prev_count = generated_defs.len();
                let snapshot_for_gen = original_defs_snapshot.clone();
                let mut extra_defs = Vec::new();
                let mut extra_sigs = HashMap::new();
                for def in generated_defs.iter_mut() {
                    let mut def_env = top_level_env.clone();
                    for param in &def.params {
                        if let Some(FnParamType::Primitive(prim)) = &param.ty {
                            def_env.scalar_types.insert(param.name.clone(), *prim);
                        }
                    }
                    // Use both fn_signatures and already-generated sigs for lookup.
                    let mut combined_sigs = fn_signatures.clone();
                    for (k, v) in &generated_sigs {
                        combined_sigs.insert(k.clone(), v.clone());
                    }
                    crate::def_semantics::monomorphize_calls_in_stmts(
                        &mut def.body,
                        &def_env,
                        &mono_eligible,
                        &combined_sigs,
                        &snapshot_for_gen,
                        &generic_struct_template_names,
                        &struct_defs,
                        &mut extra_defs,
                        &mut extra_sigs,
                        &mut mono_cache,
                        &mut errors,
                        &def.type_params,
                    );
                }
                if extra_defs.is_empty() {
                    break;
                }
                generated_defs.extend(extra_defs);
                for (k, v) in extra_sigs {
                    generated_sigs.insert(k, v);
                }
                if generated_defs.len() == prev_count {
                    break;
                }
            }

            // Register generated defs and signatures
            for sig in generated_sigs {
                fn_signatures.insert(sig.0, sig.1);
            }
            defs.extend(generated_defs);

            // Remove original mono-eligible defs — only specialized copies should
            // be processed by inference.  Also remove their fn_signatures.
            for name in &mono_eligible {
                fn_signatures.remove(name);
            }
            defs.retain(|d| !mono_eligible.contains(&d.name));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let input_names: HashSet<String> = ins.iter().cloned().collect();
    let output_names: HashSet<String> = outs.iter().cloned().collect();
    let param_names: HashSet<String> = typed_params.iter().map(|p| p.name.clone()).collect();
    let def_return_types = infer_def_return_types(&defs, &fn_signatures, &struct_defs);
    validate_def_return_types(
        &defs,
        &fn_signatures,
        &def_return_types,
        &struct_defs,
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(errors);
    }
    fn_signatures
        .entry(PROC_INDEX_CALL_SENTINEL.to_owned())
        .or_insert_with(|| internal_proc_index_call_signature(false));
    fn_signatures
        .entry(format!(
            "{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"
        ))
        .or_insert_with(|| internal_proc_index_call_signature(true));

    let mut state_scalars = HashMap::<String, PrimitiveType>::new();
    let mut declared_symbols = DeclaredSymbolMap::new();
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &input_names,
        &in_types,
        DeclaredScalarSymbolKind::Input,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &output_names,
        &out_types,
        DeclaredScalarSymbolKind::Output,
    );
    set_declared_symbol_types(
        &mut state_scalars,
        &mut declared_symbols,
        &param_names,
        &param_types,
        DeclaredScalarSymbolKind::Param,
    );
    for (fn_name, ret_ty) in &def_return_types {
        if let ReturnType::Scalar(scalar_ty) = ret_ty {
            insert_declared_symbol(
                &mut state_scalars,
                &mut declared_symbols,
                fn_name.clone(),
                DeclaredSymbolInfo::FunctionReturn { ty: *scalar_ty },
            );
        }
    }
    for buffer in &typed_buffers {
        let channels = match buffer.channels {
            TypedBufferChannels::Mono => BufferChannelInfo::Mono,
            TypedBufferChannels::Static(ch) => BufferChannelInfo::Static(ch),
            TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
        };
        insert_declared_symbol(
            &mut state_scalars,
            &mut declared_symbols,
            buffer.name.clone(),
            DeclaredSymbolInfo::Buffer {
                elem_ty: buffer.elem_ty,
                channels,
            },
        );
    }
    let state_arrays = HashMap::new();
    let state_array_struct_roots = HashMap::<String, ArrayStructRootInfo>::new();
    let struct_instances = HashMap::new();
    let mut init_known_scalars = param_names.clone();
    init_known_scalars.extend(state_scalars.keys().cloned());
    let init_locals = HashSet::new();
    let init_local_aliases = LocalAliasTypes::new();
    let mut init_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut init_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &out_arrays, false);
    seed_top_level_array_aliases(&mut init_local_data_aliases, &param_arrays, false);

    let init_default_ty =
        resolve_init_default_ty(init_default_decl_ty.as_ref(), "top-level", &mut errors);

    let init_ctx = InitAnalysisCtx {
        context_label: "top-level",
        common: ScopeAnalysisCtx {
            policy: ScopePolicy::Init,
            input_names: &input_names,
            output_names: &output_names,
            param_names: &param_names,
            struct_defs: &struct_defs,
            fn_signatures: &fn_signatures,
            fn_return_types: &def_return_types,
            options,
            port_index_ins: None,
            port_index_outs: None,
            port_index_params: None,
        },
        init_default_ty,
        proc_resolution: None,
    };
    let mut init_st = InitAnalysisState {
        known_scalars: init_known_scalars,
        local_aliases: init_local_aliases,
        local_array_aliases: init_local_data_aliases,
        declared_symbols,
        state_scalars,
        state_arrays,
        state_array_struct_roots,
        struct_instances,
        state_tuples: HashMap::new(),
        state_array_specs: HashMap::new(),
        struct_instance_type_args: HashMap::new(),
        nested_procs: HashMap::new(),
        nested_proc_arrays: HashMap::new(),
    };
    for stmt in &init {
        analyze_init_stmt(
            stmt,
            InitStmtAnalysisCtx {
                init: &init_ctx,
                locals: &init_locals,
            },
            &mut init_st,
            0,
            &mut errors,
        );
    }
    let InitAnalysisState {
        known_scalars: _init_known_scalars,
        local_aliases: _init_local_aliases,
        local_array_aliases: _init_local_data_aliases,
        declared_symbols,
        mut state_scalars,
        state_arrays,
        state_array_struct_roots,
        struct_instances,
        nested_proc_arrays,
        state_tuples,
        ..
    } = init_st;
    let init_writable_roots = collect_runtime_state_roots(&state_scalars, &state_arrays);
    let empty_nested_proc_instances = HashMap::<String, ProcNestedState>::new();

    // Rewrite struct-array field index sentinels (e.g. data[0].field[i]) to flattened Index exprs.
    rewrite_struct_array_field_index_stmts(
        &mut block_pre,
        &state_array_struct_roots,
        &struct_defs,
        &mut errors,
    );
    rewrite_struct_array_field_index_stmts(
        &mut block_post,
        &state_array_struct_roots,
        &struct_defs,
        &mut errors,
    );
    rewrite_struct_array_field_index_stmts(
        &mut sample,
        &state_array_struct_roots,
        &struct_defs,
        &mut errors,
    );

    let port_index_ins = if ins_explicit && !ins.is_empty() {
        uniform_port_type(&ins, &in_types).map(|ty| PortIndexInfo {
            count: ins.len(),
            elem_ty: ty,
        })
    } else {
        None
    };
    let port_index_outs = if outs_explicit && !outs.is_empty() {
        uniform_port_type(&outs, &out_types).map(|ty| PortIndexInfo {
            count: outs.len(),
            elem_ty: ty,
        })
    } else {
        None
    };
    let port_index_params = if params_explicit && !typed_params.is_empty() {
        uniform_port_type_from_params(&typed_params).map(|ty| PortIndexInfo {
            count: typed_params.len(),
            elem_ty: ty,
        })
    } else {
        None
    };

    let block_known_scalars = build_known_scalars_from_state(&param_names, &state_scalars);
    let block_locals = HashSet::new();
    let mut block_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut block_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut block_local_data_aliases, &out_arrays, true);
    seed_top_level_array_aliases(&mut block_local_data_aliases, &param_arrays, false);
    let empty_inputs = HashSet::new();
    let empty_outputs = HashSet::new();
    let block_forbidden_assigns = output_names.clone();
    register_and_analyze_runtime_scope(
        block_pre.iter().chain(block_post.iter()),
        ScopeAnalysisCtx {
            policy: ScopePolicy::Runtime(ScopeKind::Block),
            input_names: &empty_inputs,
            output_names: &empty_outputs,
            param_names: &param_names,
            struct_defs: &struct_defs,
            fn_signatures: &fn_signatures,
            fn_return_types: &def_return_types,
            options,
            port_index_ins,
            port_index_outs,
            port_index_params,
        },
        RuntimeRegistrationMode::Block,
        &mut state_scalars,
        &declared_symbols,
        &state_arrays,
        &state_array_struct_roots,
        &empty_nested_proc_instances,
        &nested_proc_arrays,
        &struct_instances,
        &input_names,
        &output_names,
        &param_names,
        &block_locals,
        block_known_scalars,
        LocalAliasTypes::new(),
        block_local_data_aliases,
        &block_forbidden_assigns,
        &state_tuples,
        &mut errors,
    );

    let mut sample_base = param_names.clone();
    sample_base.extend(input_names.iter().cloned());
    let sample_known_scalars = build_known_scalars_from_state(&sample_base, &state_scalars);
    let sample_locals = HashSet::new();
    let mut sample_local_data_aliases = HashMap::new();
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &in_arrays, false);
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &out_arrays, true);
    seed_top_level_array_aliases(&mut sample_local_data_aliases, &param_arrays, false);
    let sample_forbidden_assigns = HashSet::new();
    register_and_analyze_runtime_scope(
        sample.iter(),
        ScopeAnalysisCtx {
            policy: ScopePolicy::Runtime(ScopeKind::Sample),
            input_names: &input_names,
            output_names: &output_names,
            param_names: &param_names,
            struct_defs: &struct_defs,
            fn_signatures: &fn_signatures,
            fn_return_types: &def_return_types,
            options,
            port_index_ins,
            port_index_outs,
            port_index_params,
        },
        RuntimeRegistrationMode::Sample,
        &mut state_scalars,
        &declared_symbols,
        &state_arrays,
        &state_array_struct_roots,
        &empty_nested_proc_instances,
        &nested_proc_arrays,
        &struct_instances,
        &input_names,
        &output_names,
        &param_names,
        &sample_locals,
        sample_known_scalars,
        LocalAliasTypes::new(),
        sample_local_data_aliases,
        &sample_forbidden_assigns,
        &state_tuples,
        &mut errors,
    );

    let typed_events = coerce_typed_events(&events, true, "top-level", options, &mut errors);
    let final_state_roots = collect_runtime_state_roots(&state_scalars, &state_arrays);
    let immutable_event_roots = final_state_roots
        .difference(&init_writable_roots)
        .cloned()
        .collect::<HashSet<_>>();
    let event_known_scalars_seed = build_known_scalars_from_state(&param_names, &state_scalars);
    let mut event_array_alias_seed = HashMap::new();
    seed_top_level_array_aliases(&mut event_array_alias_seed, &param_arrays, false);
    analyze_runtime_events(
        &typed_events,
        &event_known_scalars_seed,
        &event_array_alias_seed,
        &param_names,
        &init_writable_roots,
        &immutable_event_roots,
        &input_names,
        &output_names,
        &state_scalars,
        &declared_symbols,
        &state_arrays,
        &state_array_struct_roots,
        &empty_nested_proc_instances,
        &nested_proc_arrays,
        &struct_instances,
        ScopeAnalysisCtx {
            policy: ScopePolicy::Event,
            input_names: &input_names,
            output_names: &output_names,
            param_names: &param_names,
            struct_defs: &struct_defs,
            fn_signatures: &fn_signatures,
            fn_return_types: &def_return_types,
            options,
            port_index_ins: None,
            port_index_outs: None,
            port_index_params: None,
        },
        &mut errors,
    );

    let mut block_exec = block_pre.clone();
    block_exec.extend(block_post.clone());
    let mut sample_and_event_exec = sample.clone();
    for event in &typed_events {
        sample_and_event_exec.extend(event.body.clone());
    }

    let current_declared_symbols = &declared_symbols;
    let mut inferred_array_bindings = HashMap::<String, InferredArrayParam>::new();
    for (name, info) in &out_arrays {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, info) in &param_arrays {
        inferred_array_bindings.insert(
            name.clone(),
            InferredArrayParam {
                elem_ty: info.elem_ty,
                len: info.len,
            },
        );
    }
    for (name, len) in &state_arrays {
        let elem_ty = declared_symbol_scalar_type(current_declared_symbols, name)
            .unwrap_or(PrimitiveType::F32);
        inferred_array_bindings.insert(name.clone(), InferredArrayParam { elem_ty, len: *len });
    }
    let inferred_struct_array_roots = state_array_struct_roots
        .iter()
        .map(|(name, info)| (name.clone(), info.struct_name.clone()))
        .collect::<HashMap<_, _>>();

    let (inferred_def_params, synthesized_struct_defs) = infer_def_param_kinds(
        &defs,
        &init,
        &block_exec,
        &sample_and_event_exec,
        &struct_instances,
        &inferred_struct_array_roots,
        &inferred_array_bindings,
        &typed_buffers
            .iter()
            .map(|b| {
                (
                    b.name.clone(),
                    vec![InferredBufferParam {
                        elem_ty: b.elem_ty,
                        channels: b.channels.clone(),
                    }],
                )
            })
            .collect::<HashMap<_, _>>(),
        &fn_signatures,
        &method_self_struct_internal,
        &struct_defs,
        options,
        &mut errors,
    );

    let mut def_struct_defs = struct_defs.clone();
    for (name, fields) in &synthesized_struct_defs {
        def_struct_defs.insert(name.clone(), fields.clone());
    }

    let def_global_inputs = HashSet::<String>::new();
    let def_global_outputs = HashSet::<String>::new();
    let def_global_params = HashSet::<String>::new();
    for def in &defs {
        let fn_known = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<HashSet<_>>();
        let mut def_state_scalars = HashMap::<String, PrimitiveType>::new();
        let mut def_declared_symbols = declared_symbols
            .iter()
            .filter_map(|(name, info)| match info {
                DeclaredSymbolInfo::FunctionReturn { .. } => Some((name.clone(), info.clone())),
                _ => None,
            })
            .collect::<DeclaredSymbolMap>();
        let fn_sig = fn_signatures.get(&def.name);
        // Def parameters are function-local and should be visible for local
        // type inference even though top-level runtime symbols are not.
        for (idx, param) in def.params.iter().enumerate() {
            let explicit_prim = fn_sig
                .and_then(|sig| sig.param_types.get(idx))
                .and_then(|ty| ty.as_ref())
                .and_then(|ty| match ty {
                    FnParamType::Primitive(prim) => Some(*prim),
                    FnParamType::Struct(_)
                    | FnParamType::Buffer(_)
                    | FnParamType::Array(_)
                    | FnParamType::ArrayGeneric(_)
                    | FnParamType::SizedArray { .. }
                    | FnParamType::BareBuffer
                    | FnParamType::Tuple(_) => None,
                });
            if let Some(param_ty) = explicit_prim {
                def_state_scalars.insert(param.name.clone(), param_ty);
            } else {
                def_state_scalars.remove(&param.name);
            }
        }
        let fn_locals = HashSet::new();
        let fn_local_aliases = LocalAliasTypes::new();
        let mut fn_local_data_aliases = HashMap::new();
        let fn_local_proc_aliases = HashMap::new();
        let param_names_vec = def
            .params
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();
        let param_structs = inferred_def_params
            .get(&def.name)
            .map(|k| param_struct_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_buffers = inferred_def_params
            .get(&def.name)
            .map(|k| param_buffer_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        let param_arrays = inferred_def_params
            .get(&def.name)
            .map(|k| param_array_map_from_kinds(&param_names_vec, k))
            .unwrap_or_default();
        for (param_name, elem_ty) in &param_arrays {
            fn_local_data_aliases.insert(
                param_name.clone(),
                LocalArrayAliasInfo {
                    len: 1,
                    elem_ty: *elem_ty,
                    elem_struct: None,
                    writable: true,
                },
            );
        }
        for (param_name, (elem_ty, channels)) in &param_buffers {
            let channel_info = match channels {
                TypedBufferChannels::Mono => BufferChannelInfo::Mono,
                TypedBufferChannels::Static(ch) => BufferChannelInfo::Static(*ch),
                TypedBufferChannels::Dynamic => BufferChannelInfo::Dynamic,
            };
            insert_declared_symbol(
                &mut def_state_scalars,
                &mut def_declared_symbols,
                param_name.clone(),
                DeclaredSymbolInfo::Buffer {
                    elem_ty: *elem_ty,
                    channels: channel_info,
                },
            );
        }
        let def_ctx = DefStmtAnalysisCtx {
            common: ScopeAnalysisCtx {
                policy: ScopePolicy::Def,
                input_names: &def_global_inputs,
                output_names: &def_global_outputs,
                param_names: &def_global_params,
                struct_defs: &def_struct_defs,
                fn_signatures: &fn_signatures,
                fn_return_types: &def_return_types,
                options,
                port_index_ins: None,
                port_index_outs: None,
                port_index_params: None,
            },
            locals: &fn_locals,
            declared_symbols: &def_declared_symbols,
            param_structs: &param_structs,
            state_scalars: &def_state_scalars,
            def_return_types: &def_return_types,
        };
        let mut def_state = DefStmtAnalysisState::from_parts(
            fn_known,
            fn_local_aliases,
            fn_local_data_aliases,
            fn_local_proc_aliases,
        );
        // Register tuple params as tuple_vars for indexing validation
        if let Some(kinds) = inferred_def_params.get(&def.name) {
            for (param, kind) in def.params.iter().zip(kinds.iter()) {
                if let TypedFnParam::Tuple { elem_tys } = kind {
                    def_state
                        .tuple_vars
                        .insert(param.name.clone(), elem_tys.len());
                }
            }
        }
        for stmt in &def.body {
            analyze_def_stmt(stmt, def_ctx, &mut def_state, 0, &mut errors);
        }
    }

    if errors.is_empty() {
        let mut sorted_state = state_scalars.keys().cloned().collect::<Vec<_>>();
        sorted_state.sort();
        let state_types = sorted_state
            .iter()
            .map(|name| {
                state_scalars
                    .get(name)
                    .copied()
                    .unwrap_or(PrimitiveType::F32)
            })
            .collect::<Vec<_>>();

        let mut typed_data = state_arrays
            .into_iter()
            .map(|(name, len)| {
                let elem_ty = declared_symbol_scalar_type(current_declared_symbols, &name)
                    .unwrap_or(PrimitiveType::F32);
                TypedArrayVar { name, len, elem_ty }
            })
            .collect::<Vec<_>>();
        typed_data.sort_by(|a, b| a.name.cmp(&b.name));
        let mut typed_data_roots = state_array_struct_roots
            .into_iter()
            .map(|(name, info)| TypedArrayStructRoot {
                name,
                struct_name: info.struct_name,
                len: info.len,
            })
            .collect::<Vec<_>>();
        typed_data_roots.sort_by(|a, b| a.name.cmp(&b.name));

        let mut synth_names = synthesized_struct_defs.keys().cloned().collect::<Vec<_>>();
        synth_names.sort();
        for name in synth_names {
            if let Some(fields) = synthesized_struct_defs.get(&name) {
                typed_structs.push(TypedStruct {
                    name,
                    fields: fields.clone(),
                });
            }
        }

        let typed_defs = defs
            .into_iter()
            .map(|d| {
                let param_kinds = inferred_def_params
                    .get(&d.name)
                    .cloned()
                    .unwrap_or_else(|| vec![TypedFnParam::Scalar { ty: None }; d.params.len()]);
                TypedFunction {
                    method_of: method_self_struct_internal.get(&d.name).cloned(),
                    type_params: d.type_params.clone(),
                    param_defaults: d.params.iter().map(|p| p.default.clone()).collect(),
                    param_kinds,
                    return_ty: def_return_types
                        .get(&d.name)
                        .cloned()
                        .unwrap_or(ReturnType::Scalar(PrimitiveType::F32)),
                    name: d.name,
                    params: d.params.into_iter().map(|p| p.name).collect(),
                    body: d.body,
                }
            })
            .collect::<Vec<_>>();

        Ok(TypedProgram {
            ins,
            outs,
            in_types,
            out_types,
            param_types,
            in_defaults,
            in_ranges,
            in_arrays,
            out_arrays,
            param_arrays,
            params: typed_params,
            buffers: typed_buffers,
            structs: typed_structs,
            defs: typed_defs,
            events: typed_events,
            def_sample_oversample_factors,
            proc_step_oversample_meta,
            init,
            block_pre,
            sample_oversample_factor,
            sample,
            block_post,
            state_vars: sorted_state,
            state_types,
            state_tuples,
            array_vars: typed_data,
            array_struct_roots: typed_data_roots,
            ins_explicit,
            outs_explicit,
            params_explicit,
        })
    } else {
        Err(errors)
    }
}

pub(crate) fn uniform_port_type(
    names: &[String],
    types: &HashMap<String, PrimitiveType>,
) -> Option<PrimitiveType> {
    let mut it = names.iter().filter_map(|n| types.get(n).copied());
    let first = it.next().unwrap_or(PrimitiveType::F32);
    if it.all(|t| t == first) {
        Some(first)
    } else {
        None
    }
}

fn uniform_port_type_from_params(params: &[TypedParam]) -> Option<PrimitiveType> {
    if params.is_empty() {
        return None;
    }
    let first = params[0].ty;
    if params.iter().all(|p| p.ty == first) {
        Some(first)
    } else {
        None
    }
}

fn requires_entry_sample(program: &Program) -> bool {
    program.blocks.iter().any(|block| {
        matches!(
            block.kind(),
            BlockKind::Ins
                | BlockKind::Outs
                | BlockKind::Params
                | BlockKind::Events
                | BlockKind::Buffers
                | BlockKind::Init
                | BlockKind::Block
                | BlockKind::Sample
                | BlockKind::Graph
        )
    })
}

/// Pre-monomorphization validation of generic def call-site type arguments.
/// Checks for bool type args and type arg count mismatches BEFORE mono rewrites calls.
fn validate_generic_def_type_args_in_stmts(
    stmts: &[Stmt],
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        validate_generic_def_type_args_in_stmt(stmt, fn_signatures, errors);
    }
}

fn validate_generic_def_type_args_in_stmt(
    stmt: &Stmt,
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            validate_generic_def_type_args_in_expr(expr, fn_signatures, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            validate_generic_def_type_args_in_expr(cond, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(then_branch, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(else_branch, fn_signatures, errors);
        }
        Stmt::For { body, .. } => {
            validate_generic_def_type_args_in_stmts(body, fn_signatures, errors);
        }
        Stmt::While { cond, body, .. } => {
            validate_generic_def_type_args_in_expr(cond, fn_signatures, errors);
            validate_generic_def_type_args_in_stmts(body, fn_signatures, errors);
        }
        _ => {}
    }
}

fn validate_generic_def_type_args_in_expr(
    expr: &Expr,
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            if let Some(sig) = fn_signatures.get(name.as_str()) {
                if !type_args.is_empty() && !sig.type_params.is_empty() {
                    if type_args.len() != sig.type_params.len() {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "function '{}' expects {} type arguments, got {}",
                                name,
                                sig.type_params.len(),
                                type_args.len()
                            ),
                            expr.loc(),
                        ));
                    }
                    for ta in type_args {
                        if matches!(ta, CallTypeArg::Primitive(PrimitiveType::Bool)) {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "'bool' is not valid as a generic type argument for '{}'; use f32, f64, i32, or i64",
                                    name
                                ),
                                expr.loc(),
                            ));
                        }
                    }
                }
            }
            for arg in args {
                validate_generic_def_type_args_in_expr(&arg.expr, fn_signatures, errors);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_generic_def_type_args_in_expr(arg, fn_signatures, errors);
            }
        }
        Expr::Binary { lhs, rhs, .. }
        | Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            validate_generic_def_type_args_in_expr(lhs, fn_signatures, errors);
            validate_generic_def_type_args_in_expr(rhs, fn_signatures, errors);
        }
        Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. }
        | Expr::Cast { expr: inner, .. } => {
            validate_generic_def_type_args_in_expr(inner, fn_signatures, errors);
        }
        Expr::Tuple { values, .. } | Expr::ArrayLiteral { values, .. } => {
            for v in values {
                validate_generic_def_type_args_in_expr(v, fn_signatures, errors);
            }
        }
        Expr::Index { index, .. } => {
            validate_generic_def_type_args_in_expr(index, fn_signatures, errors);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(s) = start {
                validate_generic_def_type_args_in_expr(s, fn_signatures, errors);
            }
            if let Some(e) = end {
                validate_generic_def_type_args_in_expr(e, fn_signatures, errors);
            }
        }
        _ => {}
    }
}
