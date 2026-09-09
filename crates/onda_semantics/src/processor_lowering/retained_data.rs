use super::*;
use crate::executable_data::*;

/// Proc helpers share their owner's fixed fields. Selections use the same
/// symbolic recipes as task continuations, with backing and coordinates stored
/// on the proc instance rather than on a shared helper invocation.
pub(super) fn retain_proc_data(
    proc: &mut ProcessorDef,
    init_flow: &ScopeFlowState,
    init_bindings: &ScopeFlowState,
    block_bindings: &ScopeFlowState,
    state: &mut ProcStateFields,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    declared_symbols: &DeclaredSymbolMap,
    reserved: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut occupied = reserved.clone();
    occupied.extend(declared_symbols.keys().cloned());
    occupied.extend(declared_names(&proc.init).into_keys());
    occupied.extend(declared_names(&proc.block_pre).into_keys());
    occupied.extend(declared_names(&proc.sample).into_keys());
    occupied.extend(declared_names(&proc.block_post).into_keys());
    for function in &proc.local_defs {
        occupied.extend(function.params.iter().map(|param| param.name.clone()));
        occupied.extend(declared_names(&function.body).into_keys());
    }
    for event in &proc.events {
        occupied.extend(event.params.iter().map(|param| param.name.clone()));
        occupied.extend(declared_names(&event.body).into_keys());
    }
    let mut init_types = DataBindingTypes::from_flow(init_flow);
    init_types.retain_data_structs(struct_defs);
    let mut init_views = HashSet::new();
    collect_view_names(&proc.init, &init_types, &mut init_views);
    let mut init_names = declared_names(&proc.init);
    let original_init_names = init_names.keys().cloned().collect::<HashSet<_>>();
    let retained_init = init_views
        .iter()
        .filter(|name| {
            init_bindings.local_struct_aliases.contains_key(*name)
                || init_bindings.local_array_aliases.contains_key(*name)
        })
        .cloned()
        .collect::<HashSet<_>>();
    let init_recipes = CapturedViews::prepare(
        &mut proc.init,
        &init_views,
        &retained_init,
        &mut init_names,
        &mut init_types,
        ViewScope {
            prefix: "__onda_init_view",
            boundary: "initialization",
            declared_symbols,
            reserved: &occupied,
        },
        errors,
    );
    let local_data = init_flow
        .local_struct_aliases
        .keys()
        .chain(init_flow.local_array_aliases.keys())
        .filter(|name| {
            !retained_init.contains(*name) && !init_views.contains(*name) && !state.has_any(name)
        })
        .cloned()
        .collect::<HashSet<_>>();
    for name in &retained_init {
        let mut uses = HashSet::new();
        init_recipes.storage_uses(name, &mut uses, &mut HashSet::new());
        if !uses.is_disjoint(&local_data) {
            errors.push(Diagnostic::semantic(format!(
                "persistent data view '{name}' borrows init-local storage; declare independent fixed data to retain its contents"
            ), 0, 0));
        }
    }
    for name in init_names
        .keys()
        .filter(|name| !original_init_names.contains(*name))
    {
        if let Some(storage) = init_types.storage(name) {
            register_storage(name, &storage, state);
        }
    }

    let mut block_types = DataBindingTypes::from_flow(block_bindings);
    block_types.retain_data_structs(struct_defs);
    let mut block_views = HashSet::new();
    collect_view_names(&proc.block_pre, &block_types, &mut block_views);
    let mut block_names = declared_names(&proc.block_pre);
    block_names.retain(|name, _| !state.has_any(name) && !reserved.contains(name));
    let original_block_names = block_names.keys().cloned().collect::<HashSet<_>>();
    let mut carried = HashSet::new();
    collect_stmt_uses(&proc.sample, &mut carried);
    collect_stmt_uses(&proc.block_post, &mut carried);
    let block_recipes = CapturedViews::prepare(
        &mut proc.block_pre,
        &block_views,
        &carried,
        &mut block_names,
        &mut block_types,
        ViewScope {
            prefix: "__onda_block_view",
            boundary: "a process boundary",
            declared_symbols,
            reserved: &occupied,
        },
        errors,
    );
    let mut storage_uses = HashSet::new();
    for name in &carried {
        block_recipes.storage_uses(name, &mut storage_uses, &mut HashSet::new());
    }
    let mut renames = HashMap::new();
    let mut owned = HashSet::new();
    let mut names = block_names.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        if !storage_uses.contains(&name) {
            continue;
        }
        let Some(storage) = block_types.storage(&name) else {
            continue;
        };
        // Ordinary scalar block state is already handled by scope analysis;
        // only generated coordinates and aggregate backing need this planner.
        if !matches!(storage, BindingStorage::Data(_)) && original_block_names.contains(&name) {
            continue;
        }
        let prefix = format!("__onda_block_data_{name}");
        let mut field = prefix.clone();
        let mut suffix = 0;
        while !occupied.insert(field.clone()) {
            suffix += 1;
            field = format!("{prefix}_{suffix}");
        }
        register_storage(&field, &storage, state);
        proc.init.body.push(storage.init_stmt(field.clone()));
        renames.insert(name.clone(), field);
        owned.insert(name);
    }
    rewrite_binding_stmts(&mut proc.block_pre, &renames, &owned);
    rewrite_binding_stmts(&mut proc.sample, &renames, &HashSet::new());
    rewrite_binding_stmts(&mut proc.block_post, &renames, &HashSet::new());
    let mut runtime_recipes = block_recipes;
    runtime_recipes.rewrite_storage_names(&renames);
    runtime_recipes.extend(&init_recipes);

    // Each executable helper receives invocation-local descriptors rebuilt from
    // this instance's fields. Declarations retain their original evaluation.
    init_recipes.expand_body(&mut proc.init);
    let mut next = 0;
    runtime_recipes.expand_body_with_counter(&mut proc.block_pre, &mut next);
    runtime_recipes.expand_body_with_counter(&mut proc.sample, &mut next);
    runtime_recipes.expand_body_with_counter(&mut proc.block_post, &mut next);
    let state_names = state
        .scalars
        .keys()
        .chain(state.tuples.keys())
        .chain(state.data.keys())
        .chain(state.struct_instances.keys())
        .map(|name| (name.clone(), format!("self.{name}")))
        .collect();
    let mut helper_recipes = init_recipes.clone();
    helper_recipes.rewrite_storage_names(&state_names);
    for event in &mut proc.events {
        helper_recipes
            .without_bindings(event.params.iter().map(|param| param.name.clone()))
            .expand_body(&mut event.body);
    }
    for function in &mut proc.local_defs {
        helper_recipes
            .without_bindings(function.params.iter().map(|param| param.name.clone()))
            .expand_body(&mut function.body);
    }
}

fn register_storage(name: &str, storage: &BindingStorage, state: &mut ProcStateFields) {
    match storage {
        BindingStorage::Scalar(ty) => {
            state.scalars.insert(name.to_owned(), *ty);
        }
        BindingStorage::Tuple(types) => {
            state.tuples.insert(name.to_owned(), types.clone());
        }
        BindingStorage::Data(DataType::Struct(struct_name)) => {
            state.struct_instances.insert(
                name.to_owned(),
                ProcStructState {
                    struct_name: struct_name.clone(),
                    type_args: Vec::new(),
                },
            );
        }
        BindingStorage::Data(DataType::Array { element, len }) => {
            state.data.insert(
                name.to_owned(),
                ArrayTypeSpec {
                    elem: element.clone(),
                    size: Box::new(Expr::int(*len as i64)),
                },
            );
        }
    }
}

fn declared_names(stmts: &[Stmt]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            } if !name.contains('.') => {
                names.insert(name.clone(), name.clone());
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                names.extend(declared_names(then_branch));
                names.extend(declared_names(else_branch));
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => names.extend(declared_names(body)),
            _ => {}
        }
    }
    names
}
