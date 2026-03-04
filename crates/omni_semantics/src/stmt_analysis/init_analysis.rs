use super::*;

pub(crate) struct ProcResolutionCtx<'a> {
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
    pub init_default_ty: Option<PrimitiveType>,
    pub input_names: &'a HashSet<String>,
    pub output_names: &'a HashSet<String>,
    pub param_names: &'a HashSet<String>,
    pub struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub fn_signatures: &'a HashMap<String, FnSignature>,
    pub options: AnalysisOptions,
    pub proc_resolution: Option<ProcResolutionCtx<'a>>,
}

#[derive(Clone)]
pub(crate) struct InitAnalysisState {
    pub known_scalars: HashSet<String>,
    pub local_aliases: LocalAliasTypes,
    pub local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    pub state_scalars: HashMap<String, PrimitiveType>,
    pub state_arrays: HashMap<String, usize>,
    pub state_array_struct_roots: HashMap<String, ArrayStructRootInfo>,
    pub struct_instances: HashMap<String, String>,
    // Proc-specific fields (empty/unused for top-level analysis)
    pub state_array_specs: HashMap<String, omni_frontend::ArrayTypeSpec>,
    pub struct_instance_type_args: HashMap<String, Vec<PrimitiveType>>,
    pub nested_procs: HashMap<String, ProcNestedState>,
    pub nested_proc_arrays: HashMap<String, ProcNestedArrayState>,
}

fn build_decl_check_state(st: &InitAnalysisState) -> ProcStateFields {
    let mut psf = ProcStateFields::default();
    psf.scalars = st.state_scalars.clone();
    for name in st.state_arrays.keys() {
        psf.data
            .entry(name.clone())
            .or_insert_with(|| omni_frontend::ArrayTypeSpec {
                elem: ArrayElemType::Primitive(PrimitiveType::F32),
                size: Box::new(Expr::Int(0)),
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
                size: Box::new(Expr::Int(0)),
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
    ctx: &InitAnalysisCtx<'_>,
    st: &mut InitAnalysisState,
    locals: &HashSet<String>,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || {
        let array_vars = merged_data_vars_for_runtime(&st.state_arrays, &st.local_array_aliases);
        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => analyze_assign_init(
                target,
                decl_ty,
                generic_decl_ty,
                *is_typed_decl,
                expr,
                ctx,
                st,
                locals,
                errors,
            ),
            Stmt::Expr { expr, .. } => {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars: &st.known_scalars,
                        locals,
                        outputs: ctx.output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances: &st.struct_instances,
                        struct_defs: ctx.struct_defs,
                        fn_signatures: ctx.fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                let _ = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    &st.state_scalars,
                    None,
                    &st.local_array_aliases,
                    locals,
                    ctx.input_names,
                    ctx.output_names,
                    ctx.param_names,
                    &st.struct_instances,
                    ctx.struct_defs,
                    errors,
                );
            }
            Stmt::Return { .. } => errors.push(Diagnostic::semantic(
                "return is only allowed inside def blocks",
                0,
                0,
            )),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                validate_expr(
                    cond,
                    ExprEnv {
                        known_scalars: &st.known_scalars,
                        locals,
                        outputs: ctx.output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances: &st.struct_instances,
                        struct_defs: ctx.struct_defs,
                        fn_signatures: ctx.fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                let cond_ty = infer_expr_type_for_semantics_with_local_data(
                    cond,
                    &st.state_scalars,
                    None,
                    &st.local_array_aliases,
                    locals,
                    ctx.input_names,
                    ctx.output_names,
                    ctx.param_names,
                    &st.struct_instances,
                    ctx.struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "if condition", errors);

                let mut then_st = st.clone();
                for nested in then_branch {
                    analyze_init_stmt(nested, ctx, &mut then_st, locals, loop_depth, errors);
                }

                let mut else_st = st.clone();
                for nested in else_branch {
                    analyze_init_stmt(nested, ctx, &mut else_st, locals, loop_depth, errors);
                }

                st.state_scalars.extend(then_st.state_scalars);
                st.state_scalars.extend(else_st.state_scalars);
                for (k, v) in then_st.state_arrays {
                    st.state_arrays.entry(k).or_insert(v);
                }
                for (k, v) in then_st.state_array_struct_roots {
                    st.state_array_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in else_st.state_arrays {
                    st.state_arrays.entry(k).or_insert(v);
                }
                for (k, v) in else_st.state_array_struct_roots {
                    st.state_array_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in then_st.struct_instances {
                    st.struct_instances.entry(k).or_insert(v);
                }
                for (k, v) in else_st.struct_instances {
                    st.struct_instances.entry(k).or_insert(v);
                }
                for (k, v) in then_st.state_array_specs {
                    st.state_array_specs.entry(k).or_insert(v);
                }
                for (k, v) in else_st.state_array_specs {
                    st.state_array_specs.entry(k).or_insert(v);
                }
                for (k, v) in then_st.struct_instance_type_args {
                    st.struct_instance_type_args.entry(k).or_insert(v);
                }
                for (k, v) in else_st.struct_instance_type_args {
                    st.struct_instance_type_args.entry(k).or_insert(v);
                }
                for (k, v) in then_st.nested_procs {
                    st.nested_procs.entry(k).or_insert(v);
                }
                for (k, v) in else_st.nested_procs {
                    st.nested_procs.entry(k).or_insert(v);
                }
                for (k, v) in then_st.nested_proc_arrays {
                    st.nested_proc_arrays.entry(k).or_insert(v);
                }
                for (k, v) in else_st.nested_proc_arrays {
                    st.nested_proc_arrays.entry(k).or_insert(v);
                }
                st.known_scalars.extend(st.state_scalars.keys().cloned());
                st.local_aliases.extend(then_st.local_aliases);
                st.local_aliases.extend(else_st.local_aliases);
                for (k, v) in then_st.local_array_aliases {
                    st.local_array_aliases.entry(k).or_insert(v);
                }
                for (k, v) in else_st.local_array_aliases {
                    st.local_array_aliases.entry(k).or_insert(v);
                }
            }
            Stmt::For {
                var,
                step,
                start,
                end,
                body,
                ..
            } => {
                validate_expr(
                    start,
                    ExprEnv {
                        known_scalars: &st.known_scalars,
                        locals,
                        outputs: ctx.output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances: &st.struct_instances,
                        struct_defs: ctx.struct_defs,
                        fn_signatures: ctx.fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                validate_expr(
                    end,
                    ExprEnv {
                        known_scalars: &st.known_scalars,
                        locals,
                        outputs: ctx.output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances: &st.struct_instances,
                        struct_defs: ctx.struct_defs,
                        fn_signatures: ctx.fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                if let Some(step_expr) = step {
                    validate_expr(
                        step_expr,
                        ExprEnv {
                            known_scalars: &st.known_scalars,
                            locals,
                            outputs: ctx.output_names,
                            array_vars: &array_vars,
                            param_structs: &HashMap::new(),
                            struct_instances: &st.struct_instances,
                            struct_defs: ctx.struct_defs,
                            fn_signatures: ctx.fn_signatures,
                            allow_array_ctor: false,
                            scope: ScopeKind::Init,
                        },
                        errors,
                    );
                }
                let start_ty = infer_expr_type_for_semantics_with_local_data(
                    start,
                    &st.state_scalars,
                    None,
                    &st.local_array_aliases,
                    locals,
                    ctx.input_names,
                    ctx.output_names,
                    ctx.param_names,
                    &st.struct_instances,
                    ctx.struct_defs,
                    errors,
                );
                require_numeric_type(start_ty, "for loop start bound", errors);
                let end_ty = infer_expr_type_for_semantics_with_local_data(
                    end,
                    &st.state_scalars,
                    None,
                    &st.local_array_aliases,
                    locals,
                    ctx.input_names,
                    ctx.output_names,
                    ctx.param_names,
                    &st.struct_instances,
                    ctx.struct_defs,
                    errors,
                );
                require_numeric_type(end_ty, "for loop end bound", errors);
                if let Some(step_expr) = step {
                    let step_ty = infer_expr_type_for_semantics_with_local_data(
                        step_expr,
                        &st.state_scalars,
                        None,
                        &st.local_array_aliases,
                        locals,
                        ctx.input_names,
                        ctx.output_names,
                        ctx.param_names,
                        &st.struct_instances,
                        ctx.struct_defs,
                        errors,
                    );
                    require_numeric_type(step_ty, "for loop step", errors);
                    if matches!(step_expr, Expr::Int(0))
                        || matches!(step_expr, Expr::Number(v) if *v == 0.0)
                    {
                        errors.push(Diagnostic::semantic("for loop step cannot be zero", 0, 0));
                    }
                }
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_st = st.clone();
                for nested in body {
                    analyze_init_stmt(
                        nested,
                        ctx,
                        &mut loop_st,
                        &loop_locals,
                        loop_depth + 1,
                        errors,
                    );
                }
                st.state_scalars.extend(loop_st.state_scalars);
                for (k, v) in loop_st.state_arrays {
                    st.state_arrays.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.state_array_struct_roots {
                    st.state_array_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.struct_instances {
                    st.struct_instances.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.state_array_specs {
                    st.state_array_specs.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.struct_instance_type_args {
                    st.struct_instance_type_args.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.nested_procs {
                    st.nested_procs.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.nested_proc_arrays {
                    st.nested_proc_arrays.entry(k).or_insert(v);
                }
                st.known_scalars.extend(st.state_scalars.keys().cloned());
                st.local_aliases = loop_st.local_aliases;
                st.local_array_aliases = loop_st.local_array_aliases;
            }
            Stmt::While { cond, body, .. } => {
                validate_expr(
                    cond,
                    ExprEnv {
                        known_scalars: &st.known_scalars,
                        locals,
                        outputs: ctx.output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances: &st.struct_instances,
                        struct_defs: ctx.struct_defs,
                        fn_signatures: ctx.fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                let cond_ty = infer_expr_type_for_semantics_with_local_data(
                    cond,
                    &st.state_scalars,
                    None,
                    &st.local_array_aliases,
                    locals,
                    ctx.input_names,
                    ctx.output_names,
                    ctx.param_names,
                    &st.struct_instances,
                    ctx.struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "while condition", errors);

                let mut loop_st = st.clone();
                for nested in body {
                    analyze_init_stmt(nested, ctx, &mut loop_st, locals, loop_depth + 1, errors);
                }
                st.state_scalars.extend(loop_st.state_scalars);
                for (k, v) in loop_st.state_arrays {
                    st.state_arrays.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.state_array_struct_roots {
                    st.state_array_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.struct_instances {
                    st.struct_instances.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.state_array_specs {
                    st.state_array_specs.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.struct_instance_type_args {
                    st.struct_instance_type_args.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.nested_procs {
                    st.nested_procs.entry(k).or_insert(v);
                }
                for (k, v) in loop_st.nested_proc_arrays {
                    st.nested_proc_arrays.entry(k).or_insert(v);
                }
                st.known_scalars.extend(st.state_scalars.keys().cloned());
                st.local_aliases = loop_st.local_aliases;
                st.local_array_aliases = loop_st.local_array_aliases;
            }
            Stmt::Break { .. } => {
                if loop_depth == 0 {
                    errors.push(Diagnostic::semantic(
                        "break is only allowed inside for/while/loop bodies",
                        0,
                        0,
                    ));
                }
            }
            Stmt::Continue { .. } => {
                if loop_depth == 0 {
                    errors.push(Diagnostic::semantic(
                        "continue is only allowed inside for/while/loop bodies",
                        0,
                        0,
                    ));
                }
            }
        }
    });
}
fn analyze_assign_init(
    target: &AssignTarget,
    decl_ty: &Option<PrimitiveType>,
    generic_decl_ty: &Option<String>,
    is_typed_decl: bool,
    expr: &Expr,
    ctx: &InitAnalysisCtx<'_>,
    st: &mut InitAnalysisState,
    locals: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let array_vars = merged_data_vars_for_runtime(&st.state_arrays, &st.local_array_aliases);
    match target {
        AssignTarget::Index { base, index } => {
            if st.state_array_struct_roots.contains_key(base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed assignment target '{base}[...]' is array[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                    ),
                    0,
                    0,
                ));
                return;
            }
            if let Some(alias) = st.local_array_aliases.get(base) {
                if !alias.writable {
                    errors.push(Diagnostic::semantic(
                        format!("cannot assign to immutable array alias '{base}'"),
                        0,
                        0,
                    ));
                    return;
                }
                if alias.elem_struct.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "indexed assignment target '{base}[...]' is array[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                        ),
                        0,
                        0,
                    ));
                    return;
                }
            }
            if decl_ty.is_some() || generic_decl_ty.is_some() {
                errors.push(Diagnostic::semantic(
                    "typed declaration is only supported for plain scalar variables",
                    0,
                    0,
                ));
            }
            // Proc mode: declaration-order validation on index target
            if let Some(pctx) = &ctx.proc_resolution {
                let decl_state = build_decl_check_state(st);
                let mut target_ok = true;
                if let Some((root, _field)) = split_field_path(base, errors) {
                    if !is_declared_proc_symbol(root, pctx.reserved, locals, &decl_state) {
                        errors.push(Diagnostic::semantic(
                            format!("symbol '{root}' used before declaration"),
                            0,
                            0,
                        ));
                        target_ok = false;
                    }
                } else if !is_declared_proc_symbol(base, pctx.reserved, locals, &decl_state) {
                    errors.push(Diagnostic::semantic(
                        format!("symbol '{base}' used before declaration"),
                        0,
                        0,
                    ));
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
                && !has_declared_buffer_symbol(&st.known_scalars, base)
            {
                errors.push(Diagnostic::semantic(
                    format!("indexed assignment target '{base}[...]' is not a array/buffer symbol"),
                    0,
                    0,
                ));
            } else if is_declared_multichannel_buffer_symbol(&st.known_scalars, base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                    0,
                    0,
                ));
            }
            validate_expr(
                index,
                ExprEnv {
                    known_scalars: &st.known_scalars,
                    locals,
                    outputs: ctx.output_names,
                    array_vars: &array_vars,
                    param_structs: &HashMap::new(),
                    struct_instances: &st.struct_instances,
                    struct_defs: ctx.struct_defs,
                    fn_signatures: ctx.fn_signatures,
                    allow_array_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars: &st.known_scalars,
                    locals,
                    outputs: ctx.output_names,
                    array_vars: &array_vars,
                    param_structs: &HashMap::new(),
                    struct_instances: &st.struct_instances,
                    struct_defs: ctx.struct_defs,
                    fn_signatures: ctx.fn_signatures,
                    allow_array_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );
            let idx_ty = infer_expr_type_for_semantics_with_local_data(
                index,
                &st.state_scalars,
                None,
                &st.local_array_aliases,
                locals,
                ctx.input_names,
                ctx.output_names,
                ctx.param_names,
                &st.struct_instances,
                ctx.struct_defs,
                errors,
            );
            require_numeric_type(idx_ty, "array index expression", errors);
            let expr_ty = infer_expr_type_for_semantics_with_local_data(
                expr,
                &st.state_scalars,
                None,
                &st.local_array_aliases,
                locals,
                ctx.input_names,
                ctx.output_names,
                ctx.param_names,
                &st.struct_instances,
                ctx.struct_defs,
                errors,
            );
            let expected_ty = st
                .local_array_aliases
                .get(base)
                .map(|a| a.elem_ty)
                .or_else(|| {
                    get_declared_symbol_type(
                        &st.state_scalars,
                        base,
                        DECLARED_DATA_ELEM_TYPE_PREFIX,
                    )
                })
                .or_else(|| {
                    get_declared_symbol_type(
                        &st.state_scalars,
                        base,
                        DECLARED_BUFFER_ELEM_TYPE_PREFIX,
                    )
                })
                .unwrap_or(PrimitiveType::F32);
            require_assignable_type(expr_ty, expected_ty, "array/buffer write", errors);
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
                    errors.push(Diagnostic::semantic(
                        format!(
                            "generic typed declaration for '{name}: {param}' is not supported; '{param}' is not a known type parameter"
                        ),
                        0,
                        0,
                    ));
                }
                None
            } else {
                None
            };
            if locals.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to loop variable '{name}'"),
                    0,
                    0,
                ));
            }
            if is_builtin_constant_name(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to builtin constant '{name}'"),
                    0,
                    0,
                ));
            }
            if ctx.input_names.contains(name)
                || ctx.output_names.contains(name)
                || ctx.param_names.contains(name)
            {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to '{name}' in init block"),
                    0,
                    0,
                ));
            }

            // Proc mode: declaration-order validation
            if let Some(pctx) = &ctx.proc_resolution {
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
                                st.state_scalars.insert(
                                    declared_type_key(DECLARED_INVALID_PLACEHOLDER_PREFIX, name),
                                    PrimitiveType::Bool,
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
                    ExprEnv {
                        known_scalars: &st.known_scalars,
                        locals,
                        outputs: ctx.output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances: &st.struct_instances,
                        struct_defs: ctx.struct_defs,
                        fn_signatures: ctx.fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Init,
                    },
                    errors,
                );
                let expr_ty = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    &st.state_scalars,
                    None,
                    &st.local_array_aliases,
                    locals,
                    ctx.input_names,
                    ctx.output_names,
                    ctx.param_names,
                    &st.struct_instances,
                    ctx.struct_defs,
                    errors,
                );
                require_assignable_type(
                    expr_ty,
                    *st.local_aliases.get(name).unwrap_or(&PrimitiveType::F32),
                    &format!("alias assignment to '{name}'"),
                    errors,
                );
                st.known_scalars.insert(name.clone());
                return;
            }
            if st.local_array_aliases.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("array alias '{name}' must be written using '{name}[index] = value'"),
                    0,
                    0,
                ));
                return;
            }

            if let Some((base, field)) = split_field_path(name, errors) {
                analyze_struct_field_init_assign(
                    base,
                    field,
                    expr,
                    &mut st.known_scalars,
                    locals,
                    &mut st.state_scalars,
                    &mut st.state_arrays,
                    &mut st.state_array_struct_roots,
                    &st.struct_instances,
                    ctx.output_names,
                    ctx.struct_defs,
                    ctx.fn_signatures,
                    ctx.options,
                    errors,
                );
                return;
            }

            if let Expr::ArrayLiteral(values) = expr {
                if declared_ty.is_some() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "typed declaration for '{name}' with array literals must use explicit array type syntax like '{name}: T[N] = [...]'"
                        ),
                        0,
                        0,
                    ));
                    return;
                }
                if st.state_arrays.contains_key(name)
                    || st.state_array_struct_roots.contains_key(name)
                {
                    errors.push(Diagnostic::semantic(
                        format!("array symbol '{name}' can only be initialized once"),
                        0,
                        0,
                    ));
                    return;
                }
                if st.state_scalars.contains_key(name) || st.struct_instances.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!("symbol '{name}' already used with a different state type"),
                        0,
                        0,
                    ));
                    return;
                }
                if values.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!("array initializer for symbol '{name}' cannot be empty"),
                        0,
                        0,
                    ));
                    return;
                }

                for value in values {
                    validate_expr(
                        value,
                        ExprEnv {
                            known_scalars: &st.known_scalars,
                            locals,
                            outputs: ctx.output_names,
                            array_vars: &array_vars,
                            param_structs: &HashMap::new(),
                            struct_instances: &st.struct_instances,
                            struct_defs: ctx.struct_defs,
                            fn_signatures: ctx.fn_signatures,
                            allow_array_ctor: false,
                            scope: ScopeKind::Init,
                        },
                        errors,
                    );
                }

                let elem_ty = infer_expr_type_for_semantics_with_local_data(
                    &values[0],
                    &st.state_scalars,
                    None,
                    &st.local_array_aliases,
                    locals,
                    ctx.input_names,
                    ctx.output_names,
                    ctx.param_names,
                    &st.struct_instances,
                    ctx.struct_defs,
                    errors,
                )
                .unwrap_or(PrimitiveType::F32);
                for (idx, value) in values.iter().enumerate() {
                    let value_ty = infer_expr_type_for_semantics_with_local_data(
                        value,
                        &st.state_scalars,
                        None,
                        &st.local_array_aliases,
                        locals,
                        ctx.input_names,
                        ctx.output_names,
                        ctx.param_names,
                        &st.struct_instances,
                        ctx.struct_defs,
                        errors,
                    );
                    require_assignable_type(
                        value_ty,
                        elem_ty,
                        &format!("array initializer assignment to '{name}[{idx}]'"),
                        errors,
                    );
                }

                st.state_scalars.insert(
                    declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name),
                    elem_ty,
                );
                st.state_arrays.insert(name.clone(), values.len());
                if ctx.proc_resolution.is_some() {
                    st.state_array_specs.entry(name.clone()).or_insert(
                        omni_frontend::ArrayTypeSpec {
                            elem: ArrayElemType::Primitive(elem_ty),
                            size: Box::new(Expr::Int(values.len() as i64)),
                        },
                    );
                }
                st.known_scalars.insert(name.clone());
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
                if let Some(pctx) = &ctx.proc_resolution {
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
                    };
                    if let Some(proc_ctor) = resolved_proc_ctor {
                        if !type_args.is_empty() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "processor '{}' is not generic and cannot take type arguments",
                                    proc_ctor
                                ),
                                0,
                                0,
                            ));
                        } else if let Some(existing) = st.nested_procs.get(name) {
                            if existing.proc_name != proc_ctor {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "{} state symbol '{name}' has conflicting processor types '{}' and '{}'",
                                        ctx.context_label, existing.proc_name, proc_ctor
                                    ),
                                    0,
                                    0,
                                ));
                            }
                        } else {
                            if !pctx.in_init_scope {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "{} state constructor '{name} = {proc_ctor}(...)' is only allowed in {} init block",
                                        ctx.context_label, ctx.context_label
                                    ),
                                    0,
                                    0,
                                ));
                            }
                            if st.state_scalars.contains_key(name)
                                || st.state_arrays.contains_key(name)
                                || st.state_array_specs.contains_key(name)
                                || st.nested_proc_arrays.contains_key(name)
                                || st.struct_instances.contains_key(name)
                            {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "{} state symbol '{name}' is used as both processor instance and non-processor value",
                                        ctx.context_label
                                    ),
                                    0,
                                    0,
                                ));
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
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "{} state constructor '{name} = {struct_ctor}(...)' is only allowed in {} init block",
                                    ctx.context_label, ctx.context_label
                                ),
                                0,
                                0,
                            ));
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
                                            errors,
                                        )
                                    } else {
                                        Some(Vec::new())
                                    }
                                } else if struct_template.type_params.is_empty() {
                                    errors.push(Diagnostic::semantic(
                                            format!(
                                                "struct '{}' is not generic and cannot take type arguments",
                                                struct_ctor
                                            ),
                                            0,
                                            0,
                                        ));
                                    None
                                } else if type_args.len() != struct_template.type_params.len() {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "struct '{}' expects {} type arguments, got {}",
                                            struct_ctor,
                                            struct_template.type_params.len(),
                                            type_args.len()
                                        ),
                                        0,
                                        0,
                                    ));
                                    None
                                } else {
                                    resolve_explicit_call_type_args(
                                        type_args,
                                        &format!("struct constructor '{}'", struct_ctor),
                                        errors,
                                    )
                                }
                            }
                            None => {
                                errors.push(Diagnostic::semantic(
                                    format!("unknown struct '{}'", struct_ctor),
                                    0,
                                    0,
                                ));
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
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "{} state symbol '{name}' is used as both struct instance and non-struct value",
                                        ctx.context_label
                                    ),
                                    0,
                                    0,
                                ));
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
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "{} state symbol '{name}' has conflicting struct constructor specializations",
                                            ctx.context_label
                                        ),
                                        0,
                                        0,
                                    ));
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
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{} state constructor '{name} = {ctor_name}(...)' is only supported for known struct or processor constructors",
                                ctx.context_label
                            ),
                            0,
                            0,
                        ));
                        st.known_scalars.insert(name.clone());
                        return;
                    }
                    // Fall through to scalar handling
                } else {
                    // Top-level mode: use existing typed struct_defs check
                    if ctx.struct_defs.contains_key(ctor_name) {
                        if !type_args.is_empty() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "struct '{}' is not generic and cannot take type arguments",
                                    ctor_name
                                ),
                                0,
                                0,
                            ));
                        }
                        if declared_ty.is_some() {
                            errors.push(Diagnostic::semantic(
                                "typed declaration cannot be used with struct constructor assignment",
                                0,
                                0,
                            ));
                            return;
                        }
                        analyze_struct_ctor_init_assign(
                            name,
                            ctor_name,
                            args,
                            &mut st.known_scalars,
                            locals,
                            &mut st.state_scalars,
                            &mut st.state_arrays,
                            &mut st.state_array_struct_roots,
                            &mut st.struct_instances,
                            ctx.output_names,
                            ctx.struct_defs,
                            ctx.fn_signatures,
                            ctx.options,
                            errors,
                        );
                        return;
                    }
                }
            }

            if let Expr::ArrayCtor { spec, init } = expr {
                // Proc mode: check if this is a proc array constructor
                if let Some(pctx) = &ctx.proc_resolution {
                    if st.state_scalars.contains_key(name) || st.struct_instances.contains_key(name)
                    {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{} state symbol '{name}' is used as both array and non-array value",
                                ctx.context_label
                            ),
                            0,
                            0,
                        ));
                    }
                    if st.nested_procs.contains_key(name) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{} state symbol '{name}' is used as both array and processor instance",
                                ctx.context_label
                            ),
                            0,
                            0,
                        ));
                    }
                    let resolved_proc_ctor = match &spec.elem {
                        ArrayElemType::Struct(elem_name) => {
                            if elem_name.contains("::") {
                                if pctx.proc_symbols.contains(elem_name) {
                                    Some(elem_name.clone())
                                } else {
                                    None
                                }
                            } else {
                                resolve_unqualified_symbol_name(
                                    elem_name,
                                    pctx.current_ns,
                                    pctx.proc_symbols,
                                )
                            }
                        }
                        ArrayElemType::Primitive(_) => None,
                    };
                    if let Some(proc_ctor) = resolved_proc_ctor {
                        if !pctx.in_init_scope {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "{} state constructor '{name}: {proc_ctor}[N]' is only allowed in {} init block",
                                    ctx.context_label, ctx.context_label
                                ),
                                0,
                                0,
                            ));
                        }
                        if st.state_scalars.contains_key(name)
                            || st.state_arrays.contains_key(name)
                            || st.state_array_specs.contains_key(name)
                            || st.nested_procs.contains_key(name)
                            || st.struct_instances.contains_key(name)
                        {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "{} state symbol '{name}' is used as both processor array and non-processor value",
                                    ctx.context_label
                                ),
                                0,
                                0,
                            ));
                        } else if let Some(existing) = st.nested_proc_arrays.get(name) {
                            if existing.proc_name != proc_ctor || existing.size_expr != *spec.size {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "{} state symbol '{name}' has conflicting processor array declarations",
                                        ctx.context_label
                                    ),
                                    0,
                                    0,
                                ));
                            }
                        } else {
                            let size_context = format!(
                                "processor-array '{}' size for symbol '{}'",
                                proc_ctor, name
                            );
                            let len =
                                eval_data_size_expr(&spec.size, ctx.options, &size_context, errors)
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
                        st.state_scalars.insert(
                            declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name),
                            PrimitiveType::F32,
                        );
                        st.known_scalars.insert(name.clone());
                        st.known_scalars
                            .insert(declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name));
                        return;
                    }
                    // Not a proc array - store as regular data array
                    st.state_array_specs
                        .entry(name.clone())
                        .or_insert_with(|| spec.clone());
                    // Also populate state_arrays so Index target validation recognizes this as an array
                    let size_context = format!("array constructor size for symbol '{name}'");
                    if let Some(size_val) =
                        eval_data_size_expr(&spec.size, ctx.options, &size_context, errors)
                    {
                        st.state_arrays.entry(name.clone()).or_insert(size_val);
                        if let ArrayElemType::Primitive(elem_ty) = spec.elem {
                            st.state_scalars.insert(
                                declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name),
                                elem_ty,
                            );
                        }
                    }
                    st.known_scalars.insert(name.clone());
                    return;
                }

                // Top-level mode: existing array constructor logic
                if declared_ty.is_some() {
                    errors.push(Diagnostic::semantic(
                        "typed declaration cannot be used with array[...] constructor assignment",
                        0,
                        0,
                    ));
                    return;
                }
                let context = format!("array constructor for symbol '{name}'");
                let size_context = format!("array constructor size for symbol '{name}'");
                if init.is_some() && !is_typed_decl {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "array constructor for symbol '{name}' does not support inline array initializers"
                        ),
                        0,
                        0,
                    ));
                }
                let Some(size_value) =
                    eval_data_size_expr(&spec.size, ctx.options, &size_context, errors)
                else {
                    return;
                };
                if st.state_arrays.contains_key(name)
                    || st.state_array_struct_roots.contains_key(name)
                {
                    errors.push(Diagnostic::semantic(
                        format!("array symbol '{name}' can only be initialized once"),
                        0,
                        0,
                    ));
                    return;
                }
                if st.state_scalars.contains_key(name) || st.struct_instances.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!("symbol '{name}' already used with a different state type"),
                        0,
                        0,
                    ));
                    return;
                }
                match &spec.elem {
                    ArrayElemType::Primitive(elem_ty) => {
                        st.state_scalars.insert(
                            declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name),
                            *elem_ty,
                        );
                        st.state_arrays.insert(name.clone(), size_value);
                        if let Some(values) = init {
                            if values.len() != size_value {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "typed array declaration '{name}' initializer expects {size_value} elements, got {}",
                                        values.len()
                                    ),
                                    0,
                                    0,
                                ));
                            }
                            for (idx, value) in values.iter().take(size_value).enumerate() {
                                validate_expr(
                                    value,
                                    ExprEnv {
                                        known_scalars: &st.known_scalars,
                                        locals,
                                        outputs: ctx.output_names,
                                        array_vars: &array_vars,
                                        param_structs: &HashMap::new(),
                                        struct_instances: &st.struct_instances,
                                        struct_defs: ctx.struct_defs,
                                        fn_signatures: ctx.fn_signatures,
                                        allow_array_ctor: false,
                                        scope: ScopeKind::Init,
                                    },
                                    errors,
                                );
                                let value_ty = infer_expr_type_for_semantics_with_local_data(
                                    value,
                                    &st.state_scalars,
                                    None,
                                    &st.local_array_aliases,
                                    locals,
                                    ctx.input_names,
                                    ctx.output_names,
                                    ctx.param_names,
                                    &st.struct_instances,
                                    ctx.struct_defs,
                                    errors,
                                );
                                require_assignable_type(
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
                            ctx.struct_defs,
                            &context,
                            &mut st.state_scalars,
                            &mut st.state_arrays,
                            &mut st.state_array_struct_roots,
                            errors,
                        ) {
                            return;
                        }
                        st.known_scalars.insert(name.clone());
                        st.known_scalars
                            .insert(declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name));
                        let prefix = format!("{DECLARED_DATA_ELEM_TYPE_PREFIX}{name}.");
                        let declared_field_keys = st
                            .state_scalars
                            .keys()
                            .filter(|k| k.starts_with(&prefix))
                            .cloned()
                            .collect::<Vec<_>>();
                        st.known_scalars.extend(declared_field_keys);
                    }
                }
                return;
            }

            if !st.state_arrays.contains_key(name)
                && !st.state_array_struct_roots.contains_key(name)
                && !st.state_scalars.contains_key(name)
                && !st.struct_instances.contains_key(name)
                && !ctx.input_names.contains(name)
                && !ctx.output_names.contains(name)
                && !ctx.param_names.contains(name)
                && !st.local_aliases.contains_key(name)
                && !st.local_array_aliases.contains_key(name)
            {
                if let Expr::Index { base, index } = expr {
                    let mut is_scalar_data_base = st.state_arrays.contains_key(base);
                    let mut array_struct_elem_struct = st
                        .state_array_struct_roots
                        .get(base)
                        .map(|r| r.struct_name.clone());
                    if let Some(alias) = st.local_array_aliases.get(base) {
                        if let Some(elem_struct) = &alias.elem_struct {
                            array_struct_elem_struct = Some(elem_struct.clone());
                        } else {
                            is_scalar_data_base = true;
                        }
                    }
                    if !is_scalar_data_base && array_struct_elem_struct.is_none() {
                        if let Some((root, field)) = split_field_path(base, errors) {
                            if let Some(struct_name) = st.struct_instances.get(root) {
                                if let Some(fields) = ctx.struct_defs.get(struct_name) {
                                    if let Some(field_decl) =
                                        fields.iter().find(|f| f.name == field)
                                    {
                                        if matches!(field_decl.ty, TypedFieldType::Array(_)) {
                                            if let Some(elem_struct) = &field_decl.array_elem_struct
                                            {
                                                array_struct_elem_struct =
                                                    Some(elem_struct.clone());
                                            } else {
                                                is_scalar_data_base = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if is_scalar_data_base || array_struct_elem_struct.is_some() {
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars: &st.known_scalars,
                                locals,
                                outputs: ctx.output_names,
                                array_vars: &array_vars,
                                param_structs: &HashMap::new(),
                                struct_instances: &st.struct_instances,
                                struct_defs: ctx.struct_defs,
                                fn_signatures: ctx.fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Init,
                            },
                            errors,
                        );
                        let idx_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            &st.state_scalars,
                            None,
                            &st.local_array_aliases,
                            locals,
                            ctx.input_names,
                            ctx.output_names,
                            ctx.param_names,
                            &st.struct_instances,
                            ctx.struct_defs,
                            errors,
                        );
                        require_numeric_type(idx_ty, "array index expression", errors);
                        if is_scalar_data_base {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "local alias binding '{name} = {base}[...]' is not supported for primitive arrays; use direct indexed access"
                                ),
                                0,
                                0,
                            ));
                        } else if let Some(struct_name) = array_struct_elem_struct {
                            if !add_struct_element_alias_bindings(
                                name,
                                &struct_name,
                                ctx.struct_defs,
                                &mut st.known_scalars,
                                &mut st.local_aliases,
                                &mut st.local_array_aliases,
                                &format!("array alias '{name}' from '{base}[...]'"),
                                errors,
                            ) {
                                return;
                            }
                        }
                        return;
                    }
                }
            }

            if st.state_arrays.contains_key(name) || st.state_array_struct_roots.contains_key(name)
            {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign scalar expression to array symbol '{name}'"),
                    0,
                    0,
                ));
            }
            if st.struct_instances.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign scalar expression to struct instance '{name}'"),
                    0,
                    0,
                ));
            }
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars: &st.known_scalars,
                    locals,
                    outputs: ctx.output_names,
                    array_vars: &array_vars,
                    param_structs: &HashMap::new(),
                    struct_instances: &st.struct_instances,
                    struct_defs: ctx.struct_defs,
                    fn_signatures: ctx.fn_signatures,
                    allow_array_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );

            let expr_ty = infer_expr_type_for_semantics_with_local_data(
                expr,
                &st.state_scalars,
                None,
                &st.local_array_aliases,
                locals,
                ctx.input_names,
                ctx.output_names,
                ctx.param_names,
                &st.struct_instances,
                ctx.struct_defs,
                errors,
            );
            let existing = st.state_scalars.get(name).copied();
            if let (Some(declared), Some(existing)) = (declared_ty, existing) {
                if declared != existing {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{} state symbol '{name}' has conflicting types {:?} and {:?}",
                            ctx.context_label, existing, declared
                        ),
                        0,
                        0,
                    ));
                }
            }
            let target_ty =
                resolve_scalar_assignment_type(existing, declared_ty, expr_ty, ctx.init_default_ty);
            require_assignable_type(
                expr_ty,
                target_ty,
                &format!("init assignment to '{name}'"),
                errors,
            );
            st.state_scalars.insert(name.clone(), target_ty);
            st.known_scalars.insert(name.clone());
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn analyze_struct_ctor_init_assign(
    target: &str,
    struct_name: &str,
    args: &[CallArg],
    known_scalars: &mut HashSet<String>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    struct_instances: &mut HashMap<String, String>,
    outputs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    _options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if struct_instances.contains_key(target) {
        errors.push(Diagnostic::semantic(
            format!("struct instance '{target}' can only be initialized once"),
            0,
            0,
        ));
        return;
    }
    if state_scalars.contains_key(target)
        || state_arrays.contains_key(target)
        || state_array_struct_roots.contains_key(target)
    {
        errors.push(Diagnostic::semantic(
            format!("symbol '{target}' already used with a different state type"),
            0,
            0,
        ));
        return;
    }

    let Some(fields) = struct_defs.get(struct_name) else {
        errors.push(Diagnostic::semantic(
            format!("unknown struct '{struct_name}'"),
            0,
            0,
        ));
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
        .map(|f| f.default.clone().or(Some(Expr::Number(0.0))))
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
                        ExprEnv {
                            known_scalars,
                            locals,
                            outputs,
                            array_vars: state_arrays,
                            param_structs: &HashMap::new(),
                            struct_instances,
                            struct_defs,
                            fn_signatures,
                            allow_array_ctor: false,
                            scope: ScopeKind::Init,
                        },
                        errors,
                    );
                    let arg_ty = infer_expr_type_for_semantics(
                        arg,
                        state_scalars,
                        None,
                        locals,
                        &HashSet::new(),
                        outputs,
                        &HashSet::new(),
                        struct_instances,
                        struct_defs,
                        errors,
                    );
                    require_assignable_type(
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
                        state_arrays,
                        state_array_struct_roots,
                        errors,
                    ) {
                        continue;
                    }
                } else {
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        field.array_elem_ty.unwrap_or(PrimitiveType::F32),
                    );
                    state_arrays.entry(flat).or_insert(len);
                }
            }
        }
    }

    struct_instances.insert(target.to_owned(), struct_name.to_owned());
}

#[allow(clippy::too_many_arguments)]
fn analyze_struct_field_init_assign(
    base: &str,
    field: &str,
    expr: &Expr,
    known_scalars: &mut HashSet<String>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    outputs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(struct_name) = struct_instances.get(base) else {
        errors.push(Diagnostic::semantic(
            format!("unknown struct instance '{base}'"),
            0,
            0,
        ));
        return;
    };
    let Some(fields) = struct_defs.get(struct_name) else {
        errors.push(Diagnostic::semantic(
            format!("unknown struct type '{}'", struct_name),
            0,
            0,
        ));
        return;
    };
    let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
        errors.push(Diagnostic::semantic(
            format!("struct '{}' has no field '{}'", struct_name, field),
            0,
            0,
        ));
        return;
    };

    let flat = format!("{base}.{field}");
    match field_decl.ty {
        TypedFieldType::Scalar(prim) => {
            if matches!(expr, Expr::ArrayCtor { .. }) {
                errors.push(Diagnostic::semantic(
                    format!("field '{flat}' is scalar and cannot be assigned array[...]"),
                    0,
                    0,
                ));
                return;
            }
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs,
                    array_vars: state_arrays,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_array_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );
            let expr_ty = infer_expr_type_for_semantics(
                expr,
                state_scalars,
                None,
                locals,
                &HashSet::new(),
                outputs,
                &HashSet::new(),
                struct_instances,
                struct_defs,
                errors,
            );
            require_assignable_type(
                expr_ty,
                prim,
                &format!("struct field init '{flat}'"),
                errors,
            );
            state_scalars.insert(flat.clone(), prim);
            known_scalars.insert(flat);
        }
        TypedFieldType::Array(expected_len) => {
            let Expr::ArrayCtor { spec, .. } = expr else {
                errors.push(Diagnostic::semantic(
                    format!("field '{flat}' requires array[{expected_len}] initialization"),
                    0,
                    0,
                ));
                return;
            };
            let context = format!("array constructor for '{flat}'");
            let size_context = format!("array constructor size for '{flat}'");
            let Some(actual_len) = eval_data_size_expr(&spec.size, options, &size_context, errors)
            else {
                return;
            };
            if actual_len != expected_len {
                errors.push(Diagnostic::semantic(
                    format!(
                        "field '{flat}' requires array[{expected_len}] but got array[{actual_len}]"
                    ),
                    0,
                    0,
                ));
                return;
            }
            match (&field_decl.array_elem_struct, &spec.elem) {
                (None, ArrayElemType::Primitive(elem_ty)) => {
                    let expected_elem_ty = field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    if expected_elem_ty != *elem_ty {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "field '{flat}' expects array[{:?}, N] but constructor uses array[{:?}, N]",
                                expected_elem_ty, elem_ty
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        expected_elem_ty,
                    );
                    state_arrays.entry(flat).or_insert(expected_len);
                }
                (None, ArrayElemType::Struct(name)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects primitive array but constructor uses struct element type '{name}'"
                        ),
                        0,
                        0,
                    ));
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
                        state_arrays,
                        state_array_struct_roots,
                        errors,
                    ) {
                        return;
                    }
                }
                (Some(expected_struct), ArrayElemType::Struct(actual_struct)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects array[{expected_struct}, N] but constructor uses array[{actual_struct}, N]"
                        ),
                        0,
                        0,
                    ));
                }
                (Some(expected_struct), ArrayElemType::Primitive(other)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects array[{expected_struct}, N] but constructor uses primitive element type {:?}",
                            other
                        ),
                        0,
                        0,
                    ));
                }
            }
        }
    }
}
