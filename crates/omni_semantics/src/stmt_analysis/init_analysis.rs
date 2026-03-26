use super::*;

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

pub(crate) struct ProcResolutionCtx<'a> {
    pub owner_proc_name: &'a str,
    pub reserved: &'a HashSet<String>,
    pub current_ns: &'a str,
    pub proc_symbols: &'a HashSet<String>,
    pub struct_symbols: &'a HashSet<String>,
    pub frontend_struct_defs: &'a HashMap<String, omni_frontend::StructDef>,
    pub ctor_symbols: &'a HashSet<String>,
    pub in_init_scope: bool,
}

pub(crate) struct InitAnalysisCtx<'a> {
    pub context_label: &'a str,
    pub common: ScopeAnalysisCtx<'a>,
    pub init_default_ty: Option<PrimitiveType>,
    pub proc_resolution: Option<ProcResolutionCtx<'a>>,
    pub top_level_proc_symbols: Option<&'a HashSet<String>>,
}

impl<'a> InitAnalysisCtx<'a> {
    fn proc_init_resolution(&self) -> Option<&ProcResolutionCtx<'a>> {
        match self.common.scope_kind() {
            ScopeKind::Init => self.proc_resolution.as_ref(),
            ScopeKind::Block | ScopeKind::Sample | ScopeKind::Def => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct InitStmtAnalysisCtx<'a> {
    pub init: &'a InitAnalysisCtx<'a>,
    pub locals: &'a HashSet<String>,
}

impl<'a> InitStmtAnalysisCtx<'a> {
    fn proc_init_resolution(&self) -> Option<&ProcResolutionCtx<'a>> {
        self.init.proc_init_resolution()
    }
}

fn specialized_proc_template_bases(proc_symbols: &HashSet<String>) -> HashSet<String> {
    proc_symbols
        .iter()
        .filter_map(|name| {
            name.rsplit_once(".__gen__")
                .map(|(base, _)| base.to_owned())
        })
        .collect()
}

fn resolve_proc_ctor_symbol_name(
    ctor_name: &str,
    current_ns: &str,
    proc_symbols: &HashSet<String>,
) -> Option<String> {
    let direct = if ctor_name.contains("::") {
        proc_symbols
            .contains(ctor_name)
            .then_some(ctor_name.to_owned())
    } else {
        resolve_unqualified_symbol_name(ctor_name, current_ns, proc_symbols)
    };
    if direct.is_some() {
        return direct;
    }

    let resolved_base = if ctor_name.contains("::") {
        ctor_name.to_owned()
    } else {
        let template_bases = specialized_proc_template_bases(proc_symbols);
        resolve_unqualified_symbol_name(ctor_name, current_ns, &template_bases)?
    };
    let prefix = format!("{resolved_base}.__gen__");
    let mut matches = proc_symbols
        .iter()
        .filter(|name| name.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

fn resolve_specialized_proc_ctor_name(
    ctor_name: &str,
    type_args: &[CallTypeArg],
    current_ns: &str,
    proc_symbols: &HashSet<String>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    if type_args.is_empty() {
        return None;
    }

    let resolved_type_args = resolve_explicit_call_type_args(
        type_args,
        &format!("processor constructor '{}'", ctor_name),
        diag,
        errors,
    )?;

    let resolved_base = if ctor_name.contains("::") {
        ctor_name.to_owned()
    } else {
        let template_bases = specialized_proc_template_bases(proc_symbols);
        resolve_unqualified_symbol_name(ctor_name, current_ns, &template_bases)?
    };

    let specialized = specialized_struct_name(&resolved_base, &resolved_type_args);
    proc_symbols.contains(&specialized).then_some(specialized)
}

#[derive(Clone)]
pub(crate) struct InitAnalysisState {
    pub known_scalars: HashSet<String>,
    pub local_aliases: LocalAliasTypes,
    pub local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    pub declared_symbols: DeclaredSymbolMap,
    pub state_scalars: HashMap<String, PrimitiveType>,
    pub state_arrays: HashMap<String, usize>,
    pub state_array_struct_roots: HashMap<String, ArrayStructRootInfo>,
    pub struct_instances: HashMap<String, String>,
    pub state_tuples: HashMap<String, Vec<PrimitiveType>>,
    // Proc-specific fields (empty/unused for top-level analysis)
    pub state_array_specs: HashMap<String, omni_frontend::ArrayTypeSpec>,
    pub struct_instance_type_args: HashMap<String, Vec<PrimitiveType>>,
    pub nested_procs: HashMap<String, ProcNestedState>,
    pub nested_proc_arrays: HashMap<String, ProcNestedArrayState>,
}

fn infer_init_slice_alias_info(
    base: &str,
    start: Option<&Expr>,
    end: Option<&Expr>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    infer_scope_slice_alias_info(
        base,
        start,
        end,
        declared_symbols,
        Some(state_arrays),
        local_array_aliases,
        struct_instances,
        struct_defs,
        errors,
    )
}

fn infer_init_data_like_info(
    expr: &Expr,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    infer_scope_data_like_info(
        expr,
        declared_symbols,
        Some(state_arrays),
        local_array_aliases,
        struct_instances,
        struct_defs,
        errors,
    )
}

impl InitAnalysisState {
    fn flow_state(&self) -> ScopeFlowState {
        ScopeFlowState::from_parts(
            self.known_scalars.clone(),
            self.local_aliases.clone(),
            self.local_array_aliases.clone(),
            HashMap::new(),
        )
    }

    fn restore_flow_state(&mut self, flow: ScopeFlowState) {
        self.known_scalars = flow.known_scalars;
        self.local_aliases = flow.local_aliases;
        self.local_array_aliases = flow.local_array_aliases;
    }

    fn sync_known_scalars_with_registered_state(&mut self) {
        self.known_scalars
            .extend(self.state_scalars.keys().cloned());
    }

    fn absorb_registered_state(
        &mut self,
        child: Self,
        context_label: &str,
        errors: &mut Vec<Diagnostic>,
    ) {
        for (k, v) in child.state_scalars {
            if let Some(existing) = self.state_scalars.get(&k).copied() {
                if existing != v {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "{context_label} state symbol '{k}' has conflicting types {:?} and {:?} across branches",
                            existing, v
                        ),
                    );
                }
            }
            self.state_scalars.insert(k, v);
        }
        for (k, v) in child.declared_symbols {
            self.declared_symbols.entry(k).or_insert(v);
        }
        for (k, v) in child.state_arrays {
            self.state_arrays.entry(k).or_insert(v);
        }
        for (k, v) in child.state_array_struct_roots {
            self.state_array_struct_roots.entry(k).or_insert(v);
        }
        for (k, v) in child.struct_instances {
            self.struct_instances.entry(k).or_insert(v);
        }
        for (k, v) in child.state_array_specs {
            self.state_array_specs.entry(k).or_insert(v);
        }
        for (k, v) in child.struct_instance_type_args {
            self.struct_instance_type_args.entry(k).or_insert(v);
        }
        for (k, v) in child.nested_procs {
            self.nested_procs.entry(k).or_insert(v);
        }
        for (k, v) in child.nested_proc_arrays {
            self.nested_proc_arrays.entry(k).or_insert(v);
        }
    }
}

fn build_decl_check_state(st: &InitAnalysisState) -> ProcStateFields {
    let mut psf = ProcStateFields::default();
    psf.scalars = st.state_scalars.clone();
    for name in st.state_arrays.keys() {
        psf.data
            .entry(name.clone())
            .or_insert_with(|| omni_frontend::ArrayTypeSpec {
                elem: ArrayElemType::Primitive(PrimitiveType::F32),
                size: Box::new(Expr::int(0)),
            });
    }
    for (name, spec) in &st.state_array_specs {
        psf.data.entry(name.clone()).or_insert_with(|| spec.clone());
    }
    for name in st.state_array_struct_roots.keys() {
        psf.data
            .entry(name.clone())
            .or_insert_with(|| omni_frontend::ArrayTypeSpec {
                elem: ArrayElemType::Primitive(PrimitiveType::F32),
                size: Box::new(Expr::int(0)),
            });
    }
    psf.nested_procs = st.nested_procs.clone();
    psf.nested_proc_arrays = st.nested_proc_arrays.clone();
    for (k, v) in &st.struct_instances {
        let type_args = st
            .struct_instance_type_args
            .get(k)
            .cloned()
            .unwrap_or_default();
        psf.struct_instances.insert(
            k.clone(),
            ProcStructState {
                struct_name: v.clone(),
                type_args,
            },
        );
    }
    psf
}

pub(crate) fn analyze_init_stmt(
    stmt: &Stmt,
    ctx: InitStmtAnalysisCtx<'_>,
    st: &mut InitAnalysisState,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, |stmt_diag| {
        let init_ctx = ctx.init;
        let common = init_ctx.common;
        let locals = ctx.locals;
        let array_vars = merged_data_vars_for_runtime(&st.state_arrays, &st.local_array_aliases);
        let empty_param_structs = HashMap::<String, String>::new();
        let expr_inputs = build_scope_analysis_expr_inputs(
            common,
            locals,
            &st.state_scalars,
            &st.declared_symbols,
            &empty_param_structs,
            &st.struct_instances,
            common.output_names,
            &st.nested_proc_arrays,
        );
        let stmt_expr_env = |scope| {
            build_scope_stmt_expr_env(
                expr_inputs,
                &st.known_scalars,
                &st.local_aliases,
                &st.local_array_aliases,
                &array_vars,
                scope,
            )
        };
        match stmt {
            Stmt::Const { .. } => {}
            Stmt::Assign {
                target_loc,
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => analyze_assign_init(
                target_loc.as_ref().into(),
                target,
                decl_ty,
                generic_decl_ty,
                *is_typed_decl,
                expr,
                ctx,
                st,
                errors,
            ),
            Stmt::Expr { expr, .. } => {
                let mut handled_proc_event_stmt = false;
                if let Some(_pctx) = ctx.proc_init_resolution() {
                    if let Expr::UserCall { name, args, .. } = expr {
                        if let Some((base, _event_name)) = split_dot_path(name) {
                            if base == PROC_INDEX_CALL_SENTINEL
                                || st.nested_procs.contains_key(base)
                                || st.nested_proc_arrays.contains_key(base)
                            {
                                handled_proc_event_stmt = true;
                                for arg in args {
                                    analyze_proc_event_arg_expr(
                                        &arg.expr,
                                        stmt_expr_env(common.scope_kind()),
                                        errors,
                                    );
                                }
                            }
                        }
                    }
                }
                if !handled_proc_event_stmt {
                    analyze_stmt_expr(expr, stmt_expr_env(common.scope_kind()), errors)
                }
            }
            Stmt::Return { .. } => push_semantic(
                stmt_diag,
                errors,
                "return is only allowed inside def blocks",
            ),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                require_validated_bool_stmt_expr(
                    cond,
                    "if condition",
                    stmt_expr_env(common.scope_kind()),
                    errors,
                );
                let base_flow = st.flow_state();
                let mut then_st = st.clone();
                for nested in then_branch {
                    analyze_init_stmt(nested, ctx, &mut then_st, loop_depth, errors);
                }
                let then_flow = then_st.flow_state();

                let mut else_st = st.clone();
                for nested in else_branch {
                    analyze_init_stmt(nested, ctx, &mut else_st, loop_depth, errors);
                }
                let else_flow = else_st.flow_state();

                st.absorb_registered_state(then_st, init_ctx.context_label, errors);
                st.absorb_registered_state(else_st, init_ctx.context_label, errors);
                st.restore_flow_state(base_flow);
                let mut proc_aliases = HashMap::new();
                merge_branch_scope_flow_state(
                    &mut st.known_scalars,
                    &mut st.local_aliases,
                    &mut st.local_array_aliases,
                    &mut proc_aliases,
                    then_flow,
                    else_flow,
                );
                st.sync_known_scalars_with_registered_state();
            }
            Stmt::For {
                var,
                step,
                start,
                end,
                body,
                ..
            } => {
                require_validated_numeric_stmt_expr(
                    start,
                    "for loop start bound",
                    stmt_expr_env(common.scope_kind()),
                    errors,
                );
                require_validated_numeric_stmt_expr(
                    end,
                    "for loop end bound",
                    stmt_expr_env(common.scope_kind()),
                    errors,
                );
                validate_for_loop_step_expr(step.as_ref(), stmt_expr_env(ScopeKind::Init), errors);
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let base_flow = st.flow_state();
                let mut loop_st = st.clone();
                let loop_ctx = InitStmtAnalysisCtx {
                    locals: &loop_locals,
                    ..ctx
                };
                for nested in body {
                    analyze_init_stmt(nested, loop_ctx, &mut loop_st, loop_depth + 1, errors);
                }
                let loop_flow = loop_st.flow_state();
                st.absorb_registered_state(loop_st, init_ctx.context_label, errors);
                st.restore_flow_state(base_flow);
                let mut proc_aliases = HashMap::new();
                adopt_loop_scope_flow_state(
                    &st.known_scalars,
                    &mut st.local_aliases,
                    &mut st.local_array_aliases,
                    &mut proc_aliases,
                    loop_flow,
                );
                st.sync_known_scalars_with_registered_state();
            }
            Stmt::While { cond, body, .. } => {
                require_validated_bool_stmt_expr(
                    cond,
                    "while condition",
                    stmt_expr_env(common.scope_kind()),
                    errors,
                );
                let base_flow = st.flow_state();
                let mut loop_st = st.clone();
                for nested in body {
                    analyze_init_stmt(nested, ctx, &mut loop_st, loop_depth + 1, errors);
                }
                let loop_flow = loop_st.flow_state();
                st.absorb_registered_state(loop_st, init_ctx.context_label, errors);
                st.restore_flow_state(base_flow);
                let mut proc_aliases = HashMap::new();
                adopt_loop_scope_flow_state(
                    &st.known_scalars,
                    &mut st.local_aliases,
                    &mut st.local_array_aliases,
                    &mut proc_aliases,
                    loop_flow,
                );
                st.sync_known_scalars_with_registered_state();
            }
            Stmt::Break { .. } => require_loop_control_context("break", loop_depth, errors),
            Stmt::Continue { .. } => require_loop_control_context("continue", loop_depth, errors),
        }
    });
}
fn analyze_assign_init(
    target_loc: SourceLoc,
    target: &AssignTarget,
    decl_ty: &Option<PrimitiveType>,
    generic_decl_ty: &Option<String>,
    is_typed_decl: bool,
    expr: &Expr,
    ctx: InitStmtAnalysisCtx<'_>,
    st: &mut InitAnalysisState,
    errors: &mut Vec<Diagnostic>,
) {
    let stmt_ctx = ctx;
    let init_ctx = stmt_ctx.init;
    let ctx = init_ctx;
    let common = ctx.common;
    let locals = stmt_ctx.locals;
    let input_names = common.input_names;
    let output_names = common.output_names;
    let param_names = common.param_names;
    let struct_defs = common.struct_defs;
    let fn_signatures = common.fn_signatures;
    let options = common.options;
    let scope = common.scope_kind();
    let array_vars = merged_data_vars_for_runtime(&st.state_arrays, &st.local_array_aliases);
    let mut rewritten_expr = expr.clone();
    rewrite_struct_array_inline_field_expr(
        &mut rewritten_expr,
        &st.state_array_struct_roots,
        struct_defs,
        errors,
    );
    let expr = &rewritten_expr;
    let empty_param_structs = HashMap::<String, String>::new();
    macro_rules! expr_inputs {
        () => {
            build_scope_analysis_expr_inputs(
                common,
                locals,
                &st.state_scalars,
                &st.declared_symbols,
                &empty_param_structs,
                &st.struct_instances,
                output_names,
                &st.nested_proc_arrays,
            )
        };
    }
    macro_rules! target_error {
        ($message:expr $(,)?) => {
            errors.push(Diagnostic::semantic_span($message, target_loc))
        };
    }
    match target {
        AssignTarget::Index { base, index } => {
            if st.state_array_struct_roots.contains_key(base) {
                target_error!(
                    format!(
                        "indexed assignment target '{base}[...]' is array[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                    ),
                );
                return;
            }
            if let Some(alias) = st.local_array_aliases.get(base) {
                if !alias.writable {
                    target_error!(format!("cannot assign to immutable array alias '{base}'"),);
                    return;
                }
                if alias.elem_struct.is_some() {
                    target_error!(
                        format!(
                            "indexed assignment target '{base}[...]' is array[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                        ),
                    );
                    return;
                }
            }
            if decl_ty.is_some() || generic_decl_ty.is_some() {
                target_error!("typed declaration is only supported for plain scalar variables",);
            }
            // Proc mode: declaration-order validation on index target
            if let Some(pctx) = stmt_ctx.proc_init_resolution() {
                let decl_state = build_decl_check_state(st);
                let mut target_ok = true;
                if let Some((root, _field)) = split_field_path(base, errors) {
                    if !is_declared_proc_symbol(root, pctx.reserved, locals, &decl_state) {
                        target_error!(format!("symbol '{root}' used before declaration"),);
                        target_ok = false;
                    }
                } else if !is_declared_proc_symbol(base, pctx.reserved, locals, &decl_state) {
                    target_error!(format!("symbol '{base}' used before declaration"),);
                    target_ok = false;
                }
                target_ok &= validate_proc_expr_decl_order(
                    index,
                    pctx.reserved,
                    locals,
                    &decl_state,
                    errors,
                );
                target_ok &=
                    validate_proc_expr_decl_order(expr, pctx.reserved, locals, &decl_state, errors);
                if !target_ok {
                    return;
                }
            }
            if !st.state_arrays.contains_key(base)
                && !st.local_array_aliases.contains_key(base)
                && !has_declared_buffer_symbol_info(&st.declared_symbols, base)
            {
                target_error!(format!(
                    "indexed assignment target '{base}[...]' is not a array/buffer symbol"
                ),);
            } else if is_declared_multichannel_buffer_info(&st.declared_symbols, base) {
                target_error!(
                    format!(
                        "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                );
            }
            validate_expr(
                index,
                build_scope_expr_env(
                    expr_inputs!(),
                    &st.known_scalars,
                    &array_vars,
                    ScopeKind::Init,
                ),
                errors,
            );
            validate_expr(
                expr,
                build_scope_expr_env(expr_inputs!(), &st.known_scalars, &array_vars, scope),
                errors,
            );
            let idx_ty = infer_expr_type_for_semantics_with_local_data(
                index,
                &st.state_scalars,
                &st.declared_symbols,
                None,
                &st.local_aliases,
                &st.local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                &st.struct_instances,
                struct_defs,
                errors,
            );
            require_expr_numeric_type(index, idx_ty, "array index expression", errors);
            let expr_ty = infer_expr_type_for_semantics_with_local_data(
                expr,
                &st.state_scalars,
                &st.declared_symbols,
                None,
                &st.local_aliases,
                &st.local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                &st.struct_instances,
                struct_defs,
                errors,
            );
            let expected_ty = st
                .local_array_aliases
                .get(base)
                .map(|a| a.elem_ty)
                .or_else(|| declared_symbol_scalar_type(&st.declared_symbols, base))
                .unwrap_or(PrimitiveType::F32);
            require_expr_assignable_type(expr, expr_ty, expected_ty, "array/buffer write", errors);
        }
        AssignTarget::Slice { base, start, end } => {
            if decl_ty.is_some() || generic_decl_ty.is_some() || is_typed_decl {
                target_error!("typed declaration is only supported for plain scalar variables",);
            }
            if let Some(pctx) = ctx.proc_init_resolution() {
                let decl_state = build_decl_check_state(st);
                let mut target_ok = true;
                if let Some((root, _field)) = split_field_path(base, errors) {
                    if !is_declared_proc_symbol(root, pctx.reserved, locals, &decl_state) {
                        target_error!(format!("symbol '{root}' used before declaration"),);
                        target_ok = false;
                    }
                } else if !is_declared_proc_symbol(base, pctx.reserved, locals, &decl_state) {
                    target_error!(format!("symbol '{base}' used before declaration"),);
                    target_ok = false;
                }
                if let Some(start) = start {
                    target_ok &= validate_proc_expr_decl_order(
                        start,
                        pctx.reserved,
                        locals,
                        &decl_state,
                        errors,
                    );
                }
                if let Some(end) = end {
                    target_ok &= validate_proc_expr_decl_order(
                        end,
                        pctx.reserved,
                        locals,
                        &decl_state,
                        errors,
                    );
                }
                target_ok &=
                    validate_proc_expr_decl_order(expr, pctx.reserved, locals, &decl_state, errors);
                if !target_ok {
                    return;
                }
            }
            let Some(target_info) = infer_init_slice_alias_info(
                base,
                start.as_ref(),
                end.as_ref(),
                &st.declared_symbols,
                &st.state_arrays,
                &st.local_array_aliases,
                &st.struct_instances,
                struct_defs,
                errors,
            ) else {
                return;
            };
            if !target_info.writable {
                target_error!(format!("cannot assign to immutable array alias '{base}'"),);
                return;
            }
            let slice_env = build_scope_expr_env(
                expr_inputs!(),
                &st.known_scalars,
                &array_vars,
                ScopeKind::Init,
            );
            if let Some(start) = start {
                validate_expr(start, slice_env, errors);
                let start_ty = infer_expr_type_for_semantics_with_local_data(
                    start,
                    &st.state_scalars,
                    &st.declared_symbols,
                    None,
                    &st.local_aliases,
                    &st.local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    &st.struct_instances,
                    struct_defs,
                    errors,
                );
                require_expr_numeric_type(start, start_ty, "slice start bound", errors);
            }
            if let Some(end) = end {
                validate_expr(end, slice_env, errors);
                let end_ty = infer_expr_type_for_semantics_with_local_data(
                    end,
                    &st.state_scalars,
                    &st.declared_symbols,
                    None,
                    &st.local_aliases,
                    &st.local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    &st.struct_instances,
                    struct_defs,
                    errors,
                );
                require_expr_numeric_type(end, end_ty, "slice end bound", errors);
            }
            let stmt_env = build_scope_stmt_expr_env(
                expr_inputs!(),
                &st.known_scalars,
                &st.local_aliases,
                &st.local_array_aliases,
                &array_vars,
                ScopeKind::Init,
            );
            if is_data_like_value_expr(expr, stmt_env) {
                validate_data_like_value_expr(expr, stmt_env, errors);
                if let Some(src_info) = infer_init_data_like_info(
                    expr,
                    &st.declared_symbols,
                    &st.state_arrays,
                    &st.local_array_aliases,
                    &st.struct_instances,
                    struct_defs,
                    errors,
                ) {
                    require_expr_assignable_type(
                        expr,
                        Some(src_info.elem_ty),
                        target_info.elem_ty,
                        "slice copy assignment",
                        errors,
                    );
                }
            } else {
                validate_expr(expr, slice_env, errors);
                let expr_ty = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    &st.state_scalars,
                    &st.declared_symbols,
                    None,
                    &st.local_aliases,
                    &st.local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    &st.struct_instances,
                    struct_defs,
                    errors,
                );
                require_expr_assignable_type(
                    expr,
                    expr_ty,
                    target_info.elem_ty,
                    "slice fill assignment",
                    errors,
                );
            }
        }
        AssignTarget::Var(name) => {
            let typed_named_ctor_decl_without_type_args = is_typed_decl
                && decl_ty.is_none()
                && generic_decl_ty.is_none()
                && matches!(expr, Expr::UserCall { type_args, .. } if type_args.is_empty());
            let declared_ty = if let Some(declared) = *decl_ty {
                Some(declared)
            } else if let Some(param) = generic_decl_ty {
                if !typed_named_ctor_decl_without_type_args {
                    target_error!(
                        format!(
                            "generic typed declaration for '{name}: {param}' is not supported; '{param}' is not a known type parameter"
                        ),
                    );
                }
                None
            } else {
                None
            };
            if locals.contains(name) {
                target_error!(format!("cannot assign to loop variable '{name}'"),);
            }
            if is_builtin_constant_name(name) {
                target_error!(format!("cannot assign to builtin constant '{name}'"),);
            }
            if input_names.contains(name)
                || output_names.contains(name)
                || param_names.contains(name)
            {
                target_error!(format!("cannot assign to '{name}' in init block"),);
            }

            // Proc mode: declaration-order validation
            if let Some(pctx) = ctx.proc_init_resolution() {
                let decl_state = build_decl_check_state(st);
                let decl_ok =
                    validate_proc_expr_decl_order(expr, pctx.reserved, locals, &decl_state, errors);
                if !decl_ok {
                    // Register placeholder so downstream doesn't see undeclared symbol
                    if is_plain_symbol(name)
                        && !pctx.reserved.contains(name)
                        && !is_builtin_constant_name(name)
                        && !st.nested_procs.contains_key(name)
                        && !st.nested_proc_arrays.contains_key(name)
                        && !st.state_array_specs.contains_key(name)
                    {
                        match expr {
                            Expr::ArrayCtor { .. } | Expr::UserCall { .. } => {}
                            _ => {
                                st.state_scalars.entry(name.clone()).or_insert(
                                    decl_ty
                                        .or(ctx.init_default_ty)
                                        .unwrap_or(PrimitiveType::F32),
                                );
                                insert_declared_symbol(
                                    &mut st.state_scalars,
                                    &mut st.declared_symbols,
                                    name.clone(),
                                    DeclaredSymbolInfo::InvalidPlaceholder,
                                );
                            }
                        }
                    }
                    st.known_scalars.insert(name.clone());
                    return;
                }
            }

            if st.local_aliases.contains_key(name) {
                validate_expr(
                    expr,
                    build_scope_expr_env(expr_inputs!(), &st.known_scalars, &array_vars, scope),
                    errors,
                );
                let expr_ty = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    &st.state_scalars,
                    &st.declared_symbols,
                    None,
                    &st.local_aliases,
                    &st.local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    &st.struct_instances,
                    struct_defs,
                    errors,
                );
                require_expr_assignable_type(
                    expr,
                    expr_ty,
                    *st.local_aliases.get(name).unwrap_or(&PrimitiveType::F32),
                    &format!("alias assignment to '{name}'"),
                    errors,
                );
                st.known_scalars.insert(name.clone());
                return;
            }
            if st.local_array_aliases.contains_key(name) {
                target_error!(format!(
                    "array alias '{name}' must be written using '{name}[index] = value'"
                ),);
                return;
            }

            if let Some((base, field)) = split_field_path(name, errors) {
                analyze_struct_field_init_assign(
                    base,
                    field,
                    expr,
                    target_loc.into(),
                    &mut st.known_scalars,
                    locals,
                    &mut st.state_scalars,
                    &mut st.declared_symbols,
                    &mut st.state_arrays,
                    &mut st.state_array_struct_roots,
                    &st.struct_instances,
                    output_names,
                    struct_defs,
                    fn_signatures,
                    options,
                    errors,
                );
                return;
            }

            if let Expr::ArrayLiteral { values, .. } = expr {
                if declared_ty.is_some() {
                    target_error!(
                        format!(
                            "typed declaration for '{name}' with array literals must use explicit array type syntax like '{name}: T[N] = [...]'"
                        ),
                    );
                    return;
                }
                if st.state_arrays.contains_key(name)
                    || st.state_array_struct_roots.contains_key(name)
                {
                    target_error!(format!(
                        "array symbol '{name}' can only be initialized once"
                    ),);
                    return;
                }
                if st.state_scalars.contains_key(name) || st.struct_instances.contains_key(name) {
                    target_error!(format!(
                        "symbol '{name}' already used with a different state type"
                    ),);
                    return;
                }
                if values.is_empty() {
                    with_expr_diag_context(expr, |expr_diag| {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!("array initializer for symbol '{name}' cannot be empty"),
                        );
                    });
                    return;
                }

                for value in values {
                    validate_expr(
                        value,
                        build_scope_expr_env(
                            expr_inputs!(),
                            &st.known_scalars,
                            &array_vars,
                            ScopeKind::Init,
                        ),
                        errors,
                    );
                }

                // Use backward-compatible literal type for the first element so
                // that `a = [0, 1]` infers as I32[] not I64[].
                let inferred_first = infer_expr_type_for_semantics_with_local_data(
                    &values[0],
                    &st.state_scalars,
                    &st.declared_symbols,
                    None,
                    &st.local_aliases,
                    &st.local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    &st.struct_instances,
                    struct_defs,
                    errors,
                );
                let elem_ty = untyped_literal_type(&values[0])
                    .or(inferred_first)
                    .unwrap_or(PrimitiveType::F32);
                for (idx, value) in values.iter().enumerate() {
                    let value_ty = infer_expr_type_for_semantics_with_local_data(
                        value,
                        &st.state_scalars,
                        &st.declared_symbols,
                        None,
                        &st.local_aliases,
                        &st.local_array_aliases,
                        locals,
                        input_names,
                        output_names,
                        param_names,
                        &st.struct_instances,
                        struct_defs,
                        errors,
                    );
                    require_expr_assignable_type(
                        value,
                        value_ty,
                        elem_ty,
                        &format!("array initializer assignment to '{name}[{idx}]'"),
                        errors,
                    );
                }

                insert_declared_symbol(
                    &mut st.state_scalars,
                    &mut st.declared_symbols,
                    name.clone(),
                    DeclaredSymbolInfo::DataArray { elem_ty },
                );
                st.state_arrays.insert(name.clone(), values.len());
                if ctx.proc_init_resolution().is_some() {
                    st.state_array_specs.entry(name.clone()).or_insert(
                        omni_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(elem_ty),
                            size: Box::new(Expr::int(values.len() as i64)),
                        },
                    );
                }
                st.known_scalars.insert(name.clone());
                return;
            }
            // Tuple literal assignment: flatten to individual scalar state entries
            if let Expr::Tuple { values, .. } = expr {
                if st.state_scalars.contains_key(name)
                    || st.state_arrays.contains_key(name)
                    || st.state_array_struct_roots.contains_key(name)
                    || st.struct_instances.contains_key(name)
                {
                    target_error!(format!(
                        "symbol '{name}' already used with a different state type"
                    ),);
                    return;
                }
                let mut elem_tys = Vec::new();
                for (idx, value) in values.iter().enumerate() {
                    validate_expr(
                        value,
                        build_scope_expr_env(expr_inputs!(), &st.known_scalars, &array_vars, scope),
                        errors,
                    );
                    let elem_ty = infer_expr_type_for_semantics_with_local_data(
                        value,
                        &st.state_scalars,
                        &st.declared_symbols,
                        None,
                        &st.local_aliases,
                        &st.local_array_aliases,
                        locals,
                        input_names,
                        output_names,
                        param_names,
                        &st.struct_instances,
                        struct_defs,
                        errors,
                    )
                    .unwrap_or(PrimitiveType::F32);
                    elem_tys.push(elem_ty);
                    let flat_name = format!("{name}.__{idx}");
                    st.state_scalars.insert(flat_name, elem_ty);
                }
                st.state_tuples.insert(name.clone(), elem_tys);
                st.known_scalars.insert(name.clone());
                return;
            }

            if let Expr::Slice {
                base, start, end, ..
            } = expr
            {
                if declared_ty.is_some() || generic_decl_ty.is_some() {
                    target_error!(format!(
                        "typed declaration for '{name}' is not supported for slice aliases"
                    ),);
                    return;
                }
                if split_field_path(name, errors).is_some() {
                    target_error!("slice alias target must be a plain variable name",);
                    return;
                }
                if st.state_arrays.contains_key(name)
                    || st.state_array_struct_roots.contains_key(name)
                {
                    target_error!(format!(
                        "array symbol '{name}' can only be initialized once"
                    ),);
                    return;
                }
                if st.state_scalars.contains_key(name)
                    || st.struct_instances.contains_key(name)
                    || st.local_aliases.contains_key(name)
                    || st.local_array_aliases.contains_key(name)
                {
                    target_error!(format!(
                        "symbol '{name}' already used with a different state type"
                    ),);
                    return;
                }
                validate_expr(
                    expr,
                    build_scope_expr_env(
                        expr_inputs!(),
                        &st.known_scalars,
                        &array_vars,
                        ScopeKind::Init,
                    ),
                    errors,
                );
                if let Some(alias) = infer_init_slice_alias_info(
                    base,
                    start.as_deref(),
                    end.as_deref(),
                    &st.declared_symbols,
                    &st.state_arrays,
                    &st.local_array_aliases,
                    &st.struct_instances,
                    struct_defs,
                    errors,
                ) {
                    st.local_array_aliases.insert(name.clone(), alias);
                }
                return;
            }

            if let Expr::UserCall {
                name: ctor_name,
                type_args,
                args,
                ..
            } = expr
            {
                // Proc mode: try proc constructor first
                if let Some(pctx) = ctx.proc_init_resolution() {
                    let resolved_proc_ctor = if ctor_name.contains("::") {
                        if pctx.proc_symbols.contains(ctor_name) {
                            Some(ctor_name.clone())
                        } else {
                            None
                        }
                    } else {
                        resolve_unqualified_symbol_name(
                            ctor_name,
                            pctx.current_ns,
                            pctx.proc_symbols,
                        )
                    }
                    .or_else(|| {
                        resolve_specialized_proc_ctor_name(
                            ctor_name,
                            type_args,
                            pctx.current_ns,
                            pctx.proc_symbols,
                            DiagCtx::new(expr.loc()),
                            errors,
                        )
                    });
                    if let Some(proc_ctor) = resolved_proc_ctor {
                        if proc_ctor == pctx.owner_proc_name {
                            target_error!(format!(
                                "{} cannot instantiate itself as state symbol '{}'",
                                ctx.context_label, name
                            ),);
                            st.known_scalars.insert(name.clone());
                            return;
                        }
                        if !type_args.is_empty() {
                            target_error!(format!(
                                "processor '{}' is not generic and cannot take type arguments",
                                proc_ctor
                            ),);
                        } else if let Some(existing) = st.nested_procs.get(name) {
                            if existing.proc_name != proc_ctor {
                                target_error!(
                                    format!(
                                        "{} state symbol '{name}' has conflicting processor types '{}' and '{}'",
                                        ctx.context_label, existing.proc_name, proc_ctor
                                    ),
                                );
                            }
                        } else {
                            if !pctx.in_init_scope {
                                target_error!(
                                    format!(
                                        "{} state constructor '{name} = {proc_ctor}(...)' is only allowed in {} init block",
                                        ctx.context_label, ctx.context_label
                                    ),
                                );
                            }
                            if st.state_scalars.contains_key(name)
                                || st.state_arrays.contains_key(name)
                                || st.state_array_specs.contains_key(name)
                                || st.nested_proc_arrays.contains_key(name)
                                || st.struct_instances.contains_key(name)
                            {
                                target_error!(
                                    format!(
                                        "{} state symbol '{name}' is used as both processor instance and non-processor value",
                                        ctx.context_label
                                    ),
                                );
                            } else {
                                st.nested_procs.insert(
                                    name.clone(),
                                    ProcNestedState {
                                        proc_name: proc_ctor,
                                    },
                                );
                            }
                        }
                        st.known_scalars.insert(name.clone());
                        return;
                    }

                    // Proc mode: try struct constructor with full type_args resolution
                    let resolved_struct_ctor = if ctor_name.contains("::") {
                        if pctx.struct_symbols.contains(ctor_name) {
                            Some(ctor_name.clone())
                        } else {
                            None
                        }
                    } else {
                        resolve_unqualified_symbol_name(
                            ctor_name,
                            pctx.current_ns,
                            pctx.struct_symbols,
                        )
                    };
                    if let Some(struct_ctor) = resolved_struct_ctor {
                        if !pctx.in_init_scope {
                            target_error!(
                                format!(
                                    "{} state constructor '{name} = {struct_ctor}(...)' is only allowed in {} init block",
                                    ctx.context_label, ctx.context_label
                                ),
                            );
                        }
                        let resolved_type_args = match pctx.frontend_struct_defs.get(&struct_ctor) {
                            Some(struct_template) => {
                                if type_args.is_empty() {
                                    if !struct_template.type_params.is_empty() {
                                        infer_generic_struct_ctor_type_args(
                                            struct_template,
                                            args,
                                            &st.state_scalars,
                                            &HashMap::new(),
                                            !typed_named_ctor_decl_without_type_args,
                                            DiagCtx::new(expr.loc()),
                                            errors,
                                        )
                                    } else {
                                        Some(Vec::new())
                                    }
                                } else if struct_template.type_params.is_empty() {
                                    target_error!(format!(
                                        "struct '{}' is not generic and cannot take type arguments",
                                        struct_ctor
                                    ),);
                                    None
                                } else if type_args.len() != struct_template.type_params.len() {
                                    target_error!(format!(
                                        "struct '{}' expects {} type arguments, got {}",
                                        struct_ctor,
                                        struct_template.type_params.len(),
                                        type_args.len()
                                    ),);
                                    None
                                } else {
                                    resolve_explicit_call_type_args(
                                        type_args,
                                        &format!("struct constructor '{}'", struct_ctor),
                                        DiagCtx::new(expr.loc()),
                                        errors,
                                    )
                                }
                            }
                            None => {
                                target_error!(format!("unknown struct '{}'", struct_ctor),);
                                None
                            }
                        };
                        if let Some(resolved_type_args) = resolved_type_args {
                            if st.state_scalars.contains_key(name)
                                || st.state_arrays.contains_key(name)
                                || st.state_array_specs.contains_key(name)
                                || st.nested_procs.contains_key(name)
                                || st.nested_proc_arrays.contains_key(name)
                            {
                                target_error!(
                                    format!(
                                        "{} state symbol '{name}' is used as both struct instance and non-struct value",
                                        ctx.context_label
                                    ),
                                );
                            } else if let Some(existing_type_args) =
                                st.struct_instance_type_args.get(name)
                            {
                                let existing_name =
                                    st.struct_instances.get(name).cloned().unwrap_or_default();
                                let current = ProcStructState {
                                    struct_name: struct_ctor.clone(),
                                    type_args: resolved_type_args.clone(),
                                };
                                let existing = ProcStructState {
                                    struct_name: existing_name,
                                    type_args: existing_type_args.clone(),
                                };
                                if existing != current {
                                    target_error!(
                                        format!(
                                            "{} state symbol '{name}' has conflicting struct constructor specializations",
                                            ctx.context_label
                                        ),
                                    );
                                }
                            } else {
                                st.struct_instances.insert(name.clone(), struct_ctor);
                                st.struct_instance_type_args
                                    .insert(name.clone(), resolved_type_args);
                            }
                        }
                        st.known_scalars.insert(name.clone());
                        return;
                    } else if pctx.ctor_symbols.contains(ctor_name) {
                        target_error!(
                            format!(
                                "{} state constructor '{name} = {ctor_name}(...)' is only supported for known struct or processor constructors",
                                ctx.context_label
                            ),
                        );
                        st.known_scalars.insert(name.clone());
                        return;
                    }
                    // Fall through to scalar handling
                } else {
                    // Top-level mode: use existing typed struct_defs check
                    if struct_defs.contains_key(ctor_name) {
                        if !type_args.is_empty() {
                            target_error!(format!(
                                "struct '{}' is not generic and cannot take type arguments",
                                ctor_name
                            ),);
                        }
                        if declared_ty.is_some() {
                            target_error!(
                                "typed declaration cannot be used with struct constructor assignment",
                            );
                            return;
                        }
                        analyze_struct_ctor_init_assign(
                            name,
                            ctor_name,
                            args,
                            target_loc.into(),
                            &mut st.known_scalars,
                            locals,
                            &mut st.state_scalars,
                            &mut st.declared_symbols,
                            &mut st.state_arrays,
                            &mut st.state_array_struct_roots,
                            &mut st.struct_instances,
                            &mut st.state_tuples,
                            output_names,
                            struct_defs,
                            fn_signatures,
                            options,
                            errors,
                        );
                        return;
                    }
                }
            }

            if let Expr::ArrayCtor { spec, init, .. } = expr {
                // Proc mode: check if this is a proc array constructor
                if let Some(pctx) = ctx.proc_init_resolution() {
                    if st.state_scalars.contains_key(name) || st.struct_instances.contains_key(name)
                    {
                        target_error!(format!(
                            "{} state symbol '{name}' is used as both array and non-array value",
                            ctx.context_label
                        ),);
                    }
                    if st.nested_procs.contains_key(name) {
                        target_error!(format!(
                            "{} state symbol '{name}' is used as both array and processor instance",
                            ctx.context_label
                        ),);
                    }
                    let resolved_proc_ctor = match &spec.elem {
                        ArrayElemType::Struct(elem_name) => resolve_proc_ctor_symbol_name(
                            elem_name,
                            pctx.current_ns,
                            pctx.proc_symbols,
                        ),
                        ArrayElemType::Primitive(_) => None,
                    };
                    if let Some(proc_ctor) = resolved_proc_ctor {
                        if proc_ctor == pctx.owner_proc_name {
                            target_error!(format!(
                                "{} cannot instantiate itself as processor array '{}'",
                                ctx.context_label, name
                            ),);
                            insert_declared_symbol(
                                &mut st.state_scalars,
                                &mut st.declared_symbols,
                                name.clone(),
                                DeclaredSymbolInfo::DataArray {
                                    elem_ty: PrimitiveType::F32,
                                },
                            );
                            return;
                        }
                        if !pctx.in_init_scope {
                            target_error!(
                                format!(
                                    "{} state constructor '{name}: {proc_ctor}[N]' is only allowed in {} init block",
                                    ctx.context_label, ctx.context_label
                                ),
                            );
                        }
                        if st.state_scalars.contains_key(name)
                            || st.state_arrays.contains_key(name)
                            || st.state_array_specs.contains_key(name)
                            || st.nested_procs.contains_key(name)
                            || st.struct_instances.contains_key(name)
                        {
                            target_error!(
                                format!(
                                    "{} state symbol '{name}' is used as both processor array and non-processor value",
                                    ctx.context_label
                                ),
                            );
                        } else if let Some(existing) = st.nested_proc_arrays.get(name) {
                            if existing.proc_name != proc_ctor || existing.size_expr != *spec.size {
                                target_error!(
                                    format!(
                                        "{} state symbol '{name}' has conflicting processor array declarations",
                                        ctx.context_label
                                    ),
                                );
                            }
                        } else {
                            let size_context = format!(
                                "processor-array '{}' size for symbol '{}'",
                                proc_ctor, name
                            );
                            let len = with_expr_diag_context(&spec.size, |_diag| {
                                eval_data_size_expr(&spec.size, options, &size_context, errors)
                            })
                            .unwrap_or(1);
                            st.nested_proc_arrays.insert(
                                name.clone(),
                                ProcNestedArrayState {
                                    proc_name: proc_ctor.clone(),
                                    size_expr: *spec.size.clone(),
                                },
                            );
                            st.state_array_struct_roots.entry(name.clone()).or_insert(
                                ArrayStructRootInfo {
                                    struct_name: proc_ctor,
                                    len,
                                },
                            );
                        }
                        insert_declared_symbol(
                            &mut st.state_scalars,
                            &mut st.declared_symbols,
                            name.clone(),
                            DeclaredSymbolInfo::DataArray {
                                elem_ty: PrimitiveType::F32,
                            },
                        );
                        st.known_scalars.insert(name.clone());
                        return;
                    }
                    // Not a proc array - store as regular data array
                    st.state_array_specs
                        .entry(name.clone())
                        .or_insert_with(|| spec.clone());
                    // Also populate state_arrays so Index target validation recognizes this as an array
                    let size_context = format!("array constructor size for symbol '{name}'");
                    if let Some(size_val) = with_expr_diag_context(&spec.size, |_diag| {
                        eval_data_size_expr(&spec.size, options, &size_context, errors)
                    }) {
                        st.state_arrays.entry(name.clone()).or_insert(size_val);
                        if let ArrayElemType::Primitive(elem_ty) = spec.elem {
                            insert_declared_symbol(
                                &mut st.state_scalars,
                                &mut st.declared_symbols,
                                name.clone(),
                                DeclaredSymbolInfo::DataArray { elem_ty },
                            );
                        }
                    }
                    st.known_scalars.insert(name.clone());
                    return;
                }
                if let Some(proc_symbols) = ctx.top_level_proc_symbols {
                    if st.state_scalars.contains_key(name) || st.struct_instances.contains_key(name)
                    {
                        target_error!(format!(
                            "{} state symbol '{name}' is used as both array and non-array value",
                            ctx.context_label
                        ),);
                    }
                    if st.nested_procs.contains_key(name) {
                        target_error!(format!(
                            "{} state symbol '{name}' is used as both array and processor instance",
                            ctx.context_label
                        ),);
                    }
                    let resolved_proc_ctor = match &spec.elem {
                        ArrayElemType::Struct(elem_name) => {
                            resolve_proc_ctor_symbol_name(elem_name, "", proc_symbols)
                        }
                        ArrayElemType::Primitive(_) => None,
                    };
                    if let Some(proc_ctor) = resolved_proc_ctor {
                        if st.state_scalars.contains_key(name)
                            || st.state_arrays.contains_key(name)
                            || st.state_array_specs.contains_key(name)
                            || st.nested_procs.contains_key(name)
                            || st.struct_instances.contains_key(name)
                        {
                            target_error!(
                                format!(
                                    "{} state symbol '{name}' is used as both processor array and non-processor value",
                                    ctx.context_label
                                ),
                            );
                        } else if let Some(existing) = st.nested_proc_arrays.get(name) {
                            if existing.proc_name != proc_ctor || existing.size_expr != *spec.size {
                                target_error!(
                                    format!(
                                        "{} state symbol '{name}' has conflicting processor array declarations",
                                        ctx.context_label
                                    ),
                                );
                            }
                        } else {
                            let size_context =
                                format!("top-level processor array '{}' size", name.as_str());
                            let len = with_expr_diag_context(&spec.size, |_diag| {
                                eval_data_size_expr(&spec.size, options, &size_context, errors)
                            })
                            .unwrap_or(1);
                            st.nested_proc_arrays.insert(
                                name.clone(),
                                ProcNestedArrayState {
                                    proc_name: proc_ctor.clone(),
                                    size_expr: *spec.size.clone(),
                                },
                            );
                            st.state_array_struct_roots.entry(name.clone()).or_insert(
                                ArrayStructRootInfo {
                                    struct_name: proc_ctor,
                                    len,
                                },
                            );
                        }
                        insert_declared_symbol(
                            &mut st.state_scalars,
                            &mut st.declared_symbols,
                            name.clone(),
                            DeclaredSymbolInfo::DataArray {
                                elem_ty: PrimitiveType::F32,
                            },
                        );
                        st.known_scalars.insert(name.clone());
                        return;
                    }
                }

                // Top-level mode: existing array constructor logic
                if declared_ty.is_some() {
                    with_expr_diag_context(expr, |expr_diag| {
                        push_semantic(
                            expr_diag,
                            errors,
                            "typed declaration cannot be used with array[...] constructor assignment",
                        );
                    });
                    return;
                }
                let context = format!("array constructor for symbol '{name}'");
                let size_context = format!("array constructor size for symbol '{name}'");
                if init.is_some() && !is_typed_decl {
                    with_expr_diag_context(expr, |expr_diag| {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "array constructor for symbol '{name}' does not support inline array initializers"
                            ),
                        );
                    });
                }
                let Some(size_value) = with_expr_diag_context(&spec.size, |_diag| {
                    eval_data_size_expr(&spec.size, options, &size_context, errors)
                }) else {
                    return;
                };
                if st.state_arrays.contains_key(name)
                    || st.state_array_struct_roots.contains_key(name)
                {
                    target_error!(format!(
                        "array symbol '{name}' can only be initialized once"
                    ),);
                    return;
                }
                if st.state_scalars.contains_key(name) || st.struct_instances.contains_key(name) {
                    target_error!(format!(
                        "symbol '{name}' already used with a different state type"
                    ),);
                    return;
                }
                match &spec.elem {
                    ArrayElemType::Primitive(elem_ty) => {
                        insert_declared_symbol(
                            &mut st.state_scalars,
                            &mut st.declared_symbols,
                            name.clone(),
                            DeclaredSymbolInfo::DataArray { elem_ty: *elem_ty },
                        );
                        st.state_arrays.insert(name.clone(), size_value);
                        if let Some(values) = init {
                            if values.len() != size_value {
                                with_expr_diag_context(expr, |expr_diag| {
                                    push_semantic(
                                        expr_diag,
                                        errors,
                                        format!(
                                            "typed array declaration '{name}' initializer expects {size_value} elements, got {}",
                                            values.len()
                                        ),
                                    );
                                });
                            }
                            for (idx, value) in values.iter().take(size_value).enumerate() {
                                validate_expr(
                                    value,
                                    build_scope_expr_env(
                                        expr_inputs!(),
                                        &st.known_scalars,
                                        &array_vars,
                                        scope,
                                    ),
                                    errors,
                                );
                                let value_ty = infer_expr_type_for_semantics_with_local_data(
                                    value,
                                    &st.state_scalars,
                                    &st.declared_symbols,
                                    None,
                                    &st.local_aliases,
                                    &st.local_array_aliases,
                                    locals,
                                    input_names,
                                    output_names,
                                    param_names,
                                    &st.struct_instances,
                                    struct_defs,
                                    errors,
                                );
                                require_expr_assignable_type(
                                    value,
                                    value_ty,
                                    *elem_ty,
                                    &format!(
                                        "typed array initializer assignment to '{name}[{idx}]'"
                                    ),
                                    errors,
                                );
                            }
                        }
                    }
                    ArrayElemType::Struct(struct_name) => {
                        if !register_data_struct_root(
                            name,
                            struct_name,
                            size_value,
                            struct_defs,
                            &context,
                            &mut st.state_scalars,
                            &mut st.declared_symbols,
                            &mut st.state_arrays,
                            &mut st.state_array_struct_roots,
                            errors,
                        ) {
                            return;
                        }
                        st.known_scalars.insert(name.clone());
                    }
                }
                return;
            }

            if !st.state_arrays.contains_key(name)
                && !st.state_array_struct_roots.contains_key(name)
                && !st.state_scalars.contains_key(name)
                && !st.struct_instances.contains_key(name)
                && !input_names.contains(name)
                && !output_names.contains(name)
                && !param_names.contains(name)
                && !st.local_aliases.contains_key(name)
                && !st.local_array_aliases.contains_key(name)
            {
                if let Expr::Index { base, index, .. } = expr {
                    let empty_proc_array_roots = HashMap::<String, ProcNestedArrayState>::new();
                    if let Some(binding_kind) = classify_runtime_like_indexed_binding(
                        base,
                        &st.local_array_aliases,
                        &st.state_arrays,
                        &st.state_array_struct_roots,
                        &st.struct_instances,
                        struct_defs,
                        &empty_proc_array_roots,
                        errors,
                    ) {
                        validate_expr(
                            index,
                            build_scope_expr_env(
                                expr_inputs!(),
                                &st.known_scalars,
                                &array_vars,
                                scope,
                            ),
                            errors,
                        );
                        let idx_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            &st.state_scalars,
                            &st.declared_symbols,
                            None,
                            &st.local_aliases,
                            &st.local_array_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            &st.struct_instances,
                            struct_defs,
                            errors,
                        );
                        require_expr_numeric_type(index, idx_ty, "array index expression", errors);
                        match binding_kind {
                            IndexedBindingKind::StructElementAlias(struct_name) => {
                                if !add_struct_element_alias_bindings(
                                    name,
                                    &struct_name,
                                    struct_defs,
                                    &mut st.known_scalars,
                                    &mut st.local_aliases,
                                    &mut st.local_array_aliases,
                                    &format!("array alias '{name}' from '{base}[...]'"),
                                    errors,
                                ) {
                                    return;
                                }
                                return;
                            }
                            IndexedBindingKind::PrimitiveScalar => {
                                // Primitive array indexed reads are scalar expressions.
                                // Allow normal first-assignment local inference to handle:
                                //   x = arr[idx]
                            }
                            IndexedBindingKind::ProcArrayAlias => {
                                target_error!(format!(
                                    "indexed expression '{base}[...]' is not a array/buffer symbol"
                                ),);
                            }
                        }
                    }
                }
            }

            if st.state_arrays.contains_key(name) || st.state_array_struct_roots.contains_key(name)
            {
                target_error!(format!(
                    "cannot assign scalar expression to array symbol '{name}'"
                ),);
            }
            if st.struct_instances.contains_key(name) {
                target_error!(format!(
                    "cannot assign scalar expression to struct instance '{name}'"
                ),);
            }
            validate_expr(
                expr,
                build_scope_expr_env(
                    expr_inputs!(),
                    &st.known_scalars,
                    &array_vars,
                    ScopeKind::Init,
                ),
                errors,
            );

            let expr_ty = infer_expr_type_for_semantics_with_local_data(
                expr,
                &st.state_scalars,
                &st.declared_symbols,
                None,
                &st.local_aliases,
                &st.local_array_aliases,
                locals,
                input_names,
                output_names,
                param_names,
                &st.struct_instances,
                struct_defs,
                errors,
            );
            let existing = st.state_scalars.get(name).copied();
            if let (Some(declared), Some(existing)) = (declared_ty, existing) {
                if declared != existing {
                    target_error!(format!(
                        "{} state symbol '{name}' has conflicting types {:?} and {:?}",
                        ctx.context_label, existing, declared
                    ),);
                }
            }
            let effective_expr_ty = if declared_ty.is_none() && existing.is_none() {
                effective_untyped_assignment_type(expr, expr_ty)
            } else {
                expr_ty
            };
            let target_ty = resolve_scalar_assignment_type(
                existing,
                declared_ty,
                effective_expr_ty,
                ctx.init_default_ty,
            );
            require_expr_assignable_type(
                expr,
                expr_ty,
                target_ty,
                &format!("init assignment to '{name}'"),
                errors,
            );
            st.state_scalars.insert(name.clone(), target_ty);
            st.known_scalars.insert(name.clone());
        }
        AssignTarget::Tuple(_) => {}
    }
}
#[allow(clippy::too_many_arguments)]
fn analyze_struct_ctor_init_assign(
    target: &str,
    struct_name: &str,
    args: &[CallArg],
    diag: DiagCtx,
    known_scalars: &mut HashSet<String>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &mut DeclaredSymbolMap,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    struct_instances: &mut HashMap<String, String>,
    state_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
    outputs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    _options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let empty_param_structs = HashMap::<String, String>::new();
    if struct_instances.contains_key(target) {
        push_semantic(
            diag,
            errors,
            format!("struct instance '{target}' can only be initialized once"),
        );
        return;
    }
    if state_scalars.contains_key(target)
        || state_arrays.contains_key(target)
        || state_array_struct_roots.contains_key(target)
    {
        push_semantic(
            diag,
            errors,
            format!("symbol '{target}' already used with a different state type"),
        );
        return;
    }

    let Some(fields) = struct_defs.get(struct_name) else {
        push_semantic(diag, errors, format!("unknown struct '{struct_name}'"));
        return;
    };

    let scalar_param_names = fields
        .iter()
        .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
        .map(|f| f.name.clone())
        .collect::<Vec<_>>();
    let scalar_defaults = fields
        .iter()
        .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
        .map(|f| f.default.clone().or(Some(Expr::number(0.0))))
        .collect::<Vec<_>>();

    let resolved = resolve_call_args(
        args,
        &scalar_param_names,
        &scalar_defaults,
        false,
        false,
        &format!("struct constructor '{struct_name}'"),
        errors,
    );

    let mut scalar_idx = 0usize;
    for field in fields {
        let flat = format!("{target}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                if let Some(arg) = resolved[scalar_idx] {
                    validate_expr(
                        arg,
                        build_expr_env(
                            known_scalars,
                            locals,
                            outputs,
                            state_arrays,
                            declared_symbols,
                            &empty_param_structs,
                            struct_instances,
                            struct_defs,
                            fn_signatures,
                            ScopeKind::Init,
                        ),
                        errors,
                    );
                    let arg_ty = infer_expr_type_for_semantics(
                        arg,
                        state_scalars,
                        declared_symbols,
                        None,
                        locals,
                        &HashSet::new(),
                        outputs,
                        &HashSet::new(),
                        struct_instances,
                        struct_defs,
                        errors,
                    );
                    require_expr_assignable_type(
                        arg,
                        arg_ty,
                        prim,
                        &format!("struct constructor field '{flat}'"),
                        errors,
                    );
                }
                scalar_idx += 1;
                state_scalars.insert(flat.clone(), prim);
                known_scalars.insert(flat);
            }
            TypedFieldType::Tuple(ref elem_tys) => {
                for (idx, prim) in elem_tys.iter().enumerate() {
                    let elem_flat = format!("{flat}.__{idx}");
                    state_scalars.insert(elem_flat, *prim);
                }
                state_tuples.insert(flat.clone(), elem_tys.clone());
                known_scalars.insert(flat);
            }
            TypedFieldType::Struct => {}
            TypedFieldType::Array(len) => {
                if let Some(elem_struct) = &field.array_elem_struct {
                    let context =
                        format!("struct constructor field '{flat}' array element '{elem_struct}'");
                    if !register_data_struct_root(
                        &flat,
                        elem_struct,
                        len,
                        struct_defs,
                        &context,
                        state_scalars,
                        declared_symbols,
                        state_arrays,
                        state_array_struct_roots,
                        errors,
                    ) {
                        continue;
                    }
                } else {
                    insert_declared_symbol(
                        state_scalars,
                        declared_symbols,
                        flat.clone(),
                        DeclaredSymbolInfo::DataArray {
                            elem_ty: field.array_elem_ty.unwrap_or(PrimitiveType::F32),
                        },
                    );
                    state_arrays.entry(flat).or_insert(len);
                }
            }
        }
    }

    register_struct_instance_roots(target, struct_name, struct_defs, struct_instances);
}

#[allow(clippy::too_many_arguments)]
fn analyze_struct_field_init_assign(
    base: &str,
    field: &str,
    expr: &Expr,
    diag: DiagCtx,
    known_scalars: &mut HashSet<String>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &mut DeclaredSymbolMap,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    outputs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let empty_param_structs = HashMap::<String, String>::new();
    let Some(struct_name) = struct_instances.get(base) else {
        push_semantic(diag, errors, format!("unknown struct instance '{base}'"));
        return;
    };
    let Some(fields) = struct_defs.get(struct_name) else {
        push_semantic(
            diag,
            errors,
            format!("unknown struct type '{}'", struct_name),
        );
        return;
    };
    let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
        push_semantic(
            diag,
            errors,
            format!("struct '{}' has no field '{}'", struct_name, field),
        );
        return;
    };

    let flat = format!("{base}.{field}");
    match field_decl.ty {
        TypedFieldType::Scalar(prim) => {
            if matches!(expr, Expr::ArrayCtor { .. }) {
                push_semantic(
                    diag,
                    errors,
                    format!("field '{flat}' is scalar and cannot be assigned array[...]"),
                );
                return;
            }
            validate_expr(
                expr,
                build_expr_env(
                    known_scalars,
                    locals,
                    outputs,
                    state_arrays,
                    declared_symbols,
                    &empty_param_structs,
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    ScopeKind::Init,
                ),
                errors,
            );
            let expr_ty = infer_expr_type_for_semantics(
                expr,
                state_scalars,
                declared_symbols,
                None,
                locals,
                &HashSet::new(),
                outputs,
                &HashSet::new(),
                struct_instances,
                struct_defs,
                errors,
            );
            require_expr_assignable_type(
                expr,
                expr_ty,
                prim,
                &format!("struct field init '{flat}'"),
                errors,
            );
            state_scalars.insert(flat.clone(), prim);
            known_scalars.insert(flat);
        }
        TypedFieldType::Tuple(_) => {
            push_semantic(
                diag,
                errors,
                format!(
                    "tuple field '{flat}' must be initialized via struct constructor, not direct assignment"
                ),
            );
        }
        TypedFieldType::Struct => {
            push_semantic(
                diag,
                errors,
                format!(
                    "struct field init '{}' must target nested fields or use the containing constructor",
                    flat
                ),
            );
        }
        TypedFieldType::Array(expected_len) => {
            let Expr::ArrayCtor { spec, .. } = expr else {
                with_expr_diag_context(expr, |expr_diag| {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("field '{flat}' requires array[{expected_len}] initialization"),
                    );
                });
                return;
            };
            let context = format!("array constructor for '{flat}'");
            let size_context = format!("array constructor size for '{flat}'");
            let Some(actual_len) = with_expr_diag_context(&spec.size, |_diag| {
                eval_data_size_expr(&spec.size, options, &size_context, errors)
            }) else {
                return;
            };
            if actual_len != expected_len {
                with_expr_diag_context(&spec.size, |expr_diag| {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!(
                            "field '{flat}' requires array[{expected_len}] but got array[{actual_len}]"
                        ),
                    );
                });
                return;
            }
            match (&field_decl.array_elem_struct, &spec.elem) {
                (None, ArrayElemType::Primitive(elem_ty)) => {
                    let expected_elem_ty = field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    if expected_elem_ty != *elem_ty {
                        with_expr_diag_context(expr, |expr_diag| {
                            push_semantic(
                                expr_diag,
                                errors,
                                format!(
                                    "field '{flat}' expects array[{:?}, N] but constructor uses array[{:?}, N]",
                                    expected_elem_ty, elem_ty
                                ),
                            );
                        });
                        return;
                    }
                    insert_declared_symbol(
                        state_scalars,
                        declared_symbols,
                        flat.clone(),
                        DeclaredSymbolInfo::DataArray {
                            elem_ty: expected_elem_ty,
                        },
                    );
                    state_arrays.entry(flat).or_insert(expected_len);
                }
                (None, ArrayElemType::Struct(name)) => {
                    with_expr_diag_context(expr, |expr_diag| {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "field '{flat}' expects primitive array but constructor uses struct element type '{name}'"
                            ),
                        );
                    });
                }
                (Some(expected_struct), ArrayElemType::Struct(actual_struct))
                    if expected_struct == actual_struct =>
                {
                    if !register_data_struct_root(
                        &flat,
                        expected_struct,
                        expected_len,
                        struct_defs,
                        &context,
                        state_scalars,
                        declared_symbols,
                        state_arrays,
                        state_array_struct_roots,
                        errors,
                    ) {
                        return;
                    }
                }
                (Some(expected_struct), ArrayElemType::Struct(actual_struct)) => {
                    with_expr_diag_context(expr, |expr_diag| {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "field '{flat}' expects array[{expected_struct}, N] but constructor uses array[{actual_struct}, N]"
                            ),
                        );
                    });
                }
                (Some(expected_struct), ArrayElemType::Primitive(other)) => {
                    with_expr_diag_context(expr, |expr_diag| {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!(
                                "field '{flat}' expects array[{expected_struct}, N] but constructor uses primitive element type {:?}",
                                other
                            ),
                        );
                    });
                }
            }
        }
    }
}
