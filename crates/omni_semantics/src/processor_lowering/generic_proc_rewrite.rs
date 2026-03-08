use super::*;

pub(super) fn rewrite_and_materialize_generic_processors(
    program: &mut Program,
    errors: &mut Vec<Diagnostic>,
) {
    let initial_proc_defs = program
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Proc(p) => Some(p.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if initial_proc_defs.is_empty() {
        return;
    }

    let mut generic_proc_templates = HashMap::<String, ProcessorDef>::new();
    for p in &initial_proc_defs {
        if p.type_params.is_empty() {
            continue;
        }
        if generic_proc_templates.contains_key(&p.name) {
            errors.push(Diagnostic::semantic(
                format!("duplicate generic processor '{}'", p.name),
                0,
                0,
            ));
            continue;
        }
        let mut seen_tp = HashSet::<String>::new();
        for tp in &p.type_params {
            if !seen_tp.insert(tp.clone()) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "duplicate generic type parameter '{}' in processor '{}'",
                        tp, p.name
                    ),
                    0,
                    0,
                ));
            }
        }
        generic_proc_templates.insert(p.name.clone(), p.clone());
    }

    if generic_proc_templates.is_empty() {
        return;
    }

    let mut generated_specializations = HashMap::<String, ProcessorDef>::new();
    let top_level_seed = generic_inference_seed_for_top_level(&program.blocks);
    let empty_seed = GenericInferenceLocals::default();
    for block in &mut program.blocks {
        match block {
            Block::Struct(s) => {
                let struct_ns = namespace_of_symbol(&s.name);
                for field in &mut s.fields {
                    if let Some(default) = &mut field.default {
                        let mut locals = GenericInferenceLocals::default();
                        rewrite_generic_proc_ctor_expr(
                            default,
                            &generic_proc_templates,
                            &mut generated_specializations,
                            errors,
                            &mut locals,
                            &struct_ns,
                        );
                    }
                }
                for method in &mut s.methods {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut method.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &empty_seed,
                        &struct_ns,
                    );
                }
            }
            Block::Def(d) => {
                let def_ns = namespace_of_symbol(&d.name);
                rewrite_generic_proc_ctor_stmt_list(
                    &mut d.body,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &empty_seed,
                    &def_ns,
                );
            }
            Block::Proc(p) => {
                if !p.type_params.is_empty() {
                    continue;
                }
                let proc_ns = namespace_of_symbol(&p.name);
                let proc_seed = generic_inference_seed_for_processor(p);
                rewrite_generic_proc_ctor_stmt_list(
                    &mut p.init,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &proc_seed,
                    &proc_ns,
                );
                rewrite_generic_proc_ctor_stmt_list(
                    &mut p.block_pre,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &proc_seed,
                    &proc_ns,
                );
                rewrite_generic_proc_ctor_stmt_list(
                    &mut p.sample,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &proc_seed,
                    &proc_ns,
                );
                rewrite_generic_proc_ctor_stmt_list(
                    &mut p.block_post,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &proc_seed,
                    &proc_ns,
                );
                for event in &mut p.events {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut event.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                }
                for def in &mut p.local_defs {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut def.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &proc_seed,
                        &proc_ns,
                    );
                }
            }
            Block::Init(stmts) => {
                rewrite_generic_proc_ctor_stmt_list(
                    &mut stmts.body,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &top_level_seed,
                    "",
                );
            }
            Block::Block(exec) => {
                rewrite_generic_proc_ctor_stmt_list(
                    &mut exec.pre,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &top_level_seed,
                    "",
                );
                if let Some(sample) = &mut exec.sample {
                    rewrite_generic_proc_ctor_stmt_list(
                        sample,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &top_level_seed,
                        "",
                    );
                }
                rewrite_generic_proc_ctor_stmt_list(
                    &mut exec.post,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &top_level_seed,
                    "",
                );
            }
            Block::Sample(stmts) => {
                rewrite_generic_proc_ctor_stmt_list(
                    stmts,
                    &generic_proc_templates,
                    &mut generated_specializations,
                    errors,
                    &top_level_seed,
                    "",
                );
            }
            Block::Events(events) => {
                for event in events {
                    rewrite_generic_proc_ctor_stmt_list(
                        &mut event.body,
                        &generic_proc_templates,
                        &mut generated_specializations,
                        errors,
                        &top_level_seed,
                        "",
                    );
                }
            }
            _ => {}
        }
    }
    finalize_generated_generic_proc_specializations(
        &generic_proc_templates,
        &mut generated_specializations,
        errors,
    );

    program
        .blocks
        .retain(|b| !matches!(b, Block::Proc(p) if !p.type_params.is_empty()));
    let mut generated = generated_specializations.into_values().collect::<Vec<_>>();
    generated.sort_by(|a, b| a.name.cmp(&b.name));
    for proc in generated {
        program.blocks.push(Block::Proc(proc));
    }
}
