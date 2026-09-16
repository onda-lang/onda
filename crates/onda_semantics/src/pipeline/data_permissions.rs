use super::*;
use crate::def_semantics::call_types::{
    infer_array_arg_type, infer_struct_expr_type, join_branch_envs, statement_list_flow,
    update_call_type_env_after_assign, CallArrayElemType, CallTypeContext, CallTypeEnv,
    StatementFlow,
};

type Origins = HashSet<String>;

#[derive(Clone)]
struct ReferenceEnv {
    types: CallTypeEnv,
    origins: HashMap<String, Origins>,
    receiver_owner: Option<String>,
}

struct PermissionAnalysis<'a> {
    signatures: &'a HashMap<String, FnSignature>,
    readonly: &'a HashMap<String, Origins>,
    proc_types: &'a HashSet<String>,
    receiver_fields: &'a HashMap<String, Origins>,
    types: CallTypeContext<'a>,
    writes: Origins,
}

fn is_reference(ty: Option<&FnParamType>) -> bool {
    matches!(
        ty,
        Some(
            FnParamType::Struct(_)
                | FnParamType::Array(_)
                | FnParamType::ArrayGeneric(_)
                | FnParamType::SizedArray { .. }
        )
    )
}

impl ReferenceEnv {
    fn for_parameters(seed: &CallTypeEnv, signature: &FnSignature, candidates: &Origins) -> Self {
        let receiver_owner = match (
            signature.params.first().map(String::as_str),
            signature.param_types.first(),
        ) {
            (Some("self"), Some(Some(FnParamType::Struct(owner))))
                if candidates.contains("self") =>
            {
                Some(owner.clone())
            }
            _ => None,
        };
        let mut env = Self {
            types: seed.clone(),
            origins: HashMap::new(),
            receiver_owner,
        };
        env.types.set_owner_type_params(&signature.type_params);
        for (index, name) in signature.params.iter().enumerate() {
            env.types.bind_function_param_type(
                name,
                signature.param_types.get(index).and_then(Option::as_ref),
                &signature.type_params,
            );
            let origins = if candidates.contains(name) {
                Origins::from([name.clone()])
            } else {
                Origins::new()
            };
            env.origins.insert(name.clone(), origins);
        }
        env
    }
    fn storage_origins(&self, name: &str, receiver_fields: &HashMap<String, Origins>) -> Origins {
        let receiver_fields = self
            .receiver_owner
            .as_ref()
            .and_then(|owner| receiver_fields.get(owner));
        let mut path = name;
        loop {
            if let Some(origins) = self.origins.get(path) {
                return origins.clone();
            }
            if receiver_fields.is_some_and(|fields| fields.contains(path)) {
                return Origins::from(["self".to_owned()]);
            }
            let Some((parent, _)) = path.rsplit_once('.') else {
                return Origins::new();
            };
            path = parent;
        }
    }

    fn join(a: Self, a_flow: StatementFlow, b: Self, b_flow: StatementFlow) -> Self {
        if a_flow == StatementFlow::Terminates {
            return b;
        }
        if b_flow == StatementFlow::Terminates {
            return a;
        }
        let origins = a
            .origins
            .into_iter()
            .filter_map(|(name, mut origins)| {
                origins.extend(b.origins.get(&name)?.iter().cloned());
                Some((name, origins))
            })
            .collect();
        Self {
            types: join_branch_envs(a.types, a_flow, b.types, b_flow).0,
            origins,
            receiver_owner: a.receiver_owner,
        }
    }
}

impl PermissionAnalysis<'_> {
    fn storage_origins(&self, name: &str, env: &ReferenceEnv) -> Origins {
        env.storage_origins(name, self.receiver_fields)
    }

    fn indexed_call_targets_processor(&self, args: &[CallArg], env: &ReferenceEnv) -> bool {
        let Some(base) = crate::proc_call_rewrite::proc_index_base_name(args) else {
            return false;
        };
        if let Some(array) = infer_array_arg_type(&Expr::var(base), &env.types, self.types) {
            return matches!(
                array.elem,
                CallArrayElemType::Nominal(ref name) if self.proc_types.contains(name)
            );
        }
        let Some((root, field)) = split_root_field_path(base) else {
            return false;
        };
        env.types.struct_instances.get(root).is_some_and(|owner| {
            is_flattened_proc_array_field(owner, field, self.types.struct_defs)
        })
    }

    fn reference_origins(&self, expr: &Expr, env: &ReferenceEnv) -> Origins {
        match expr {
            Expr::Var { name, .. } => self.storage_origins(name, env),
            Expr::Slice { base, .. } => self.storage_origins(base, env),
            _ => {
                if let Some(source) = indexed_read_source(expr) {
                    return self.storage_origins(source.base, env);
                }
                Origins::new()
            }
        }
    }

    fn expression(&mut self, expr: &Expr, env: &ReferenceEnv) {
        for expr in expr.walk() {
            if let Expr::UserCall { name, args, .. } = expr {
                if name == PROC_INDEX_CALL_SENTINEL
                    || name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
                        == Some(PROC_INDEX_CALL_SENTINEL)
                        && self.indexed_call_targets_processor(args, env)
                {
                    // Indexed proc execution also updates hidden block-activity storage.
                    if let Some(base) = crate::proc_call_rewrite::proc_index_base_name(args) {
                        self.writes.extend(self.storage_origins(base, env));
                    }
                } else if name == WRITE_UNSAFE_FN {
                    if let Some(arg) = args.first() {
                        self.writes.extend(self.reference_origins(&arg.expr, env));
                    }
                } else if let Some(signature) = self.signatures.get(name) {
                    let mut ignored = Vec::new();
                    let resolved = resolve_call_args_at(
                        args,
                        &signature.params,
                        &signature.defaults,
                        signature.params.first().map(String::as_str) == Some("self"),
                        false,
                        "permission inference",
                        expr.loc(),
                        &mut ignored,
                    );
                    for (index, arg) in resolved.into_iter().enumerate() {
                        let Some(arg) = arg else {
                            continue;
                        };
                        if is_reference(signature.param_types.get(index).and_then(Option::as_ref))
                            && !self
                                .readonly
                                .get(name)
                                .unwrap_or(&signature.readonly_data_params)
                                .contains(&signature.params[index])
                        {
                            self.writes.extend(self.reference_origins(arg, env));
                        }
                    }
                }
            }
        }
    }

    fn statements(&mut self, statements: &[Stmt], env: &mut ReferenceEnv) {
        for stmt in statements {
            match stmt {
                Stmt::Assign {
                    target,
                    decl_ty,
                    generic_decl_ty,
                    is_typed_decl,
                    expr,
                    ..
                } => {
                    target.visit_selectors(|selector| self.expression(selector, env));
                    match target {
                        AssignTarget::Var(name) => {
                            // Existing aggregate names preserve their storage identity.
                            let existing = env.types.has_binding(name)
                                || env.origins.contains_key(name)
                                || !self.storage_origins(name, env).is_empty()
                                || name.contains('.');
                            if existing {
                                self.writes.extend(self.storage_origins(name, env));
                            }
                            self.expression(expr, env);
                            if !existing {
                                let copied =
                                    *is_typed_decl && !matches!(decl_ty, Some(DeclType::Slice(_)));
                                let reference =
                                    infer_struct_expr_type(expr, &env.types, self.types).is_some()
                                        || infer_array_arg_type(expr, &env.types, self.types)
                                            .is_some();
                                let origins = if copied || !reference {
                                    Origins::new()
                                } else {
                                    self.reference_origins(expr, env)
                                };
                                env.origins.insert(name.clone(), origins);
                            }
                        }
                        AssignTarget::Index { base, .. }
                        | AssignTarget::IndexedMember { base, .. } => {
                            self.writes.extend(self.storage_origins(base, env));
                            self.expression(expr, env);
                        }
                        AssignTarget::Slice { base, .. } => {
                            self.writes.extend(self.storage_origins(base, env));
                            self.expression(expr, env);
                        }
                        AssignTarget::Tuple(names) => {
                            for name in names.iter().filter_map(|target| target.binding()) {
                                self.writes.extend(self.storage_origins(name, env));
                            }
                            self.expression(expr, env);
                        }
                    }
                    update_call_type_env_after_assign(
                        target,
                        decl_ty.as_ref(),
                        generic_decl_ty.as_deref(),
                        expr,
                        &mut env.types,
                        self.types,
                    );
                }
                Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => self.expression(expr, env),
                Stmt::Print { values, .. } => {
                    for expr in values {
                        self.expression(expr, env);
                    }
                }
                Stmt::Const { decl, .. } => self.expression(&decl.expr, env),
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.expression(cond, env);
                    let mut a = env.clone();
                    let mut b = env.clone();
                    self.statements(then_branch, &mut a);
                    self.statements(else_branch, &mut b);
                    *env = ReferenceEnv::join(
                        a,
                        statement_list_flow(then_branch),
                        b,
                        statement_list_flow(else_branch),
                    );
                }
                Stmt::For {
                    var,
                    start,
                    end,
                    step,
                    body,
                    ..
                } => {
                    self.expression(start, env);
                    self.expression(end, env);
                    if let Some(step) = step {
                        self.expression(step, env);
                    }
                    let mut nested = env.clone();
                    nested.types.shadow_binding(var);
                    nested.origins.remove(var);
                    self.statements(body, &mut nested);
                }
                Stmt::While { cond, body, .. } => {
                    self.expression(cond, env);
                    self.statements(body, &mut env.clone());
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
            if crate::def_semantics::call_types::statement_flow(stmt) == StatementFlow::Terminates {
                break;
            }
        }
    }
}

pub(super) fn update_readonly_data_param_signatures(
    defs: &[FunctionDef],
    events: &[EventDef],
    signatures: &mut HashMap<String, FnSignature>,
    lexical_seed: &CallTypeEnv,
    owner_seed: &CallTypeEnv,
    runtime_def_names: &HashSet<String>,
    structs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    let candidates = defs
        .iter()
        .map(|def| {
            let signature = &signatures[&def.name];
            let names = signature
                .params
                .iter()
                .zip(&signature.param_types)
                .filter(|(name, ty)| {
                    is_reference(ty.as_ref())
                        || def
                            .params
                            .iter()
                            .any(|param| param.readonly && param.name == **name)
                })
                .map(|(name, _)| name.clone())
                .collect::<Origins>();
            (def.name.clone(), names)
        })
        .collect::<HashMap<_, _>>();
    let returns = signatures
        .iter()
        .filter_map(|(name, signature)| signature.return_type.clone().map(|ty| (name.clone(), ty)))
        .collect();
    let context = CallTypeContext {
        return_types: &returns,
        struct_defs: structs,
    };
    let proc_types = signatures
        .keys()
        .filter_map(|name| {
            name.split_once(PROC_CALL_OUT_FN_PREFIX)
                .map(|(owner, _)| owner.to_owned())
        })
        .collect::<HashSet<_>>();
    let receiver_fields = structs
        .iter()
        .map(|(owner, fields)| {
            let mut names = Origins::new();
            for field in fields {
                names.insert(field.name.clone());
                names.insert(field.name.split(['.', '[']).next().unwrap().to_owned());
            }
            (owner.clone(), names)
        })
        .collect::<HashMap<_, _>>();
    let mut readonly = candidates.clone();
    loop {
        let mut changed = false;
        for def in defs {
            let signature = &signatures[&def.name];
            let seed = def_call_type_env(def, runtime_def_names, lexical_seed, owner_seed);
            let mut env = ReferenceEnv::for_parameters(seed, signature, &candidates[&def.name]);
            let mut analysis = PermissionAnalysis {
                signatures,
                readonly: &readonly,
                proc_types: &proc_types,
                receiver_fields: &receiver_fields,
                types: context,
                writes: Origins::new(),
            };
            analysis.statements(&def.body, &mut env);
            let inferred = candidates[&def.name]
                .difference(&analysis.writes)
                .cloned()
                .collect::<Origins>();
            if readonly[&def.name] != inferred {
                readonly.insert(def.name.clone(), inferred);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for def in defs {
        for param in &def.params {
            if param.readonly
                && candidates[&def.name].contains(&param.name)
                && !readonly[&def.name].contains(&param.name)
            {
                push_semantic(
                    DiagCtx::new(param.loc),
                    errors,
                    format!(
                        "cannot write through read-only payload parameter '{}'",
                        param.name
                    ),
                );
            }
        }
    }
    for event in events {
        let signature = FnSignature::from_event_params(&event.params);
        let candidates = signature.params.iter().cloned().collect();
        let mut env = ReferenceEnv::for_parameters(owner_seed, &signature, &candidates);
        let mut analysis = PermissionAnalysis {
            signatures,
            readonly: &readonly,
            proc_types: &proc_types,
            receiver_fields: &receiver_fields,
            types: context,
            writes: Origins::new(),
        };
        analysis.statements(&event.body, &mut env);
        for param in &event.params {
            if analysis.writes.contains(&param.name) {
                push_semantic(
                    DiagCtx::new(param.loc),
                    errors,
                    format!(
                        "cannot write through read-only payload parameter '{}'",
                        param.name
                    ),
                );
            }
        }
    }
    for (name, params) in readonly {
        signatures.get_mut(&name).unwrap().readonly_data_params = params;
    }
}
