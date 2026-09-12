use super::*;

/// Specializes constructors in source scopes whose lowering hoists or erases
/// local declarations before the ordinary generic-struct pass can inspect them.
pub(super) fn rewrite_source_task_and_when_generic_structs(
    program: &mut Program,
    errors: &mut Vec<Diagnostic>,
) {
    let templates = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Struct(strukt) if !strukt.type_params.is_empty() => {
                Some((strukt.name.clone(), strukt.clone()))
            }
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    if templates.is_empty() {
        return;
    }

    let facts = generic_inference_facts(
        program
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Def(def) => Some(def),
                _ => None,
            })
            .chain(program.blocks.iter().flat_map(|block| match block {
                Block::Struct(strukt) => strukt.methods.iter(),
                _ => [].iter(),
            }))
            .chain(program.blocks.iter().flat_map(|block| match block {
                Block::Proc(proc) => proc.local_defs.iter(),
                _ => [].iter(),
            })),
        program.blocks.iter().filter_map(|block| match block {
            Block::Struct(strukt) => Some(strukt),
            _ => None,
        }),
    );
    let top_seed = generic_inference_seed_for_top_level(&program.blocks, facts.clone());
    let mut generated = HashMap::new();
    let top_runtime_seed = program
        .blocks
        .iter_mut()
        .find_map(|block| match block {
            Block::Init(init) => Some(rewrite_generic_struct_ctor_stmt_list(
                &mut init.body,
                &templates,
                &mut generated,
                errors,
                &top_seed,
            )),
            _ => None,
        })
        .unwrap_or_else(|| top_seed.clone());
    let proc_delegates = program
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Proc(proc) => Some((proc.name.clone(), proc.delegates.clone())),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let proc_names = proc_delegates.keys().cloned().collect::<HashSet<_>>();
    let top_children = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Init(init) => Some(child_proc_instances(&init.body, &proc_names)),
            _ => None,
        })
        .unwrap_or_default();
    let top_delegates = program
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Delegates(delegates) => Some(delegates.delegates.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let no_generated_procs = HashMap::new();

    for block in &mut program.blocks {
        match block {
            Block::Tasks(tasks) => {
                for task in &mut tasks.tasks {
                    rewrite_generic_struct_ctor_stmt_list(
                        &mut task.body,
                        &templates,
                        &mut generated,
                        errors,
                        &top_runtime_seed,
                    );
                }
            }
            Block::When(when) => {
                let child = when
                    .target
                    .receiver
                    .first()
                    .filter(|_| when.target.receiver.len() == 1)
                    .and_then(|receiver| top_children.get(receiver))
                    .map(|child| (child.proc_name.as_str(), child.is_array));
                let (delegate, takes_index) = resolve_generic_when_delegate(
                    when,
                    &top_delegates,
                    child,
                    None,
                    &proc_delegates,
                    &no_generated_procs,
                );
                let when_seed = generic_inference_seed_for_when(
                    when,
                    delegate.as_ref(),
                    takes_index,
                    &top_runtime_seed,
                );
                rewrite_generic_struct_ctor_stmt_list(
                    &mut when.body,
                    &templates,
                    &mut generated,
                    errors,
                    &when_seed,
                );
            }
            Block::Proc(proc) => {
                let proc_seed = generic_inference_seed_for_processor(proc, facts.clone());
                let runtime_seed = rewrite_generic_struct_ctor_stmt_list(
                    &mut proc.init,
                    &templates,
                    &mut generated,
                    errors,
                    &proc_seed,
                );
                for task in &mut proc.tasks {
                    rewrite_generic_struct_ctor_stmt_list(
                        &mut task.body,
                        &templates,
                        &mut generated,
                        errors,
                        &runtime_seed,
                    );
                }
                let children = child_proc_instances(&proc.init.body, &proc_names);
                for when in &mut proc.whens {
                    let child = when
                        .target
                        .receiver
                        .first()
                        .filter(|_| when.target.receiver.len() == 1)
                        .and_then(|receiver| children.get(receiver))
                        .map(|child| (child.proc_name.as_str(), child.is_array));
                    let (delegate, takes_index) = resolve_generic_when_delegate(
                        when,
                        &proc.delegates,
                        child,
                        Some((&proc.name, &proc.delegates)),
                        &proc_delegates,
                        &no_generated_procs,
                    );
                    let when_seed = generic_inference_seed_for_when(
                        when,
                        delegate.as_ref(),
                        takes_index,
                        &runtime_seed,
                    );
                    rewrite_generic_struct_ctor_stmt_list(
                        &mut when.body,
                        &templates,
                        &mut generated,
                        errors,
                        &when_seed,
                    );
                }
            }
            _ => {}
        }
    }

    finalize_generated_generic_struct_specializations(
        &templates,
        &mut generated,
        errors,
        &GenericInferenceLocals::with_facts(facts),
    );
    let mut generated = generated.into_values().collect::<Vec<_>>();
    generated.sort_by(|left, right| left.name.cmp(&right.name));
    program
        .blocks
        .extend(generated.into_iter().map(Block::Struct));
}
