use super::*;

pub(crate) const PROC_FIELD_SENTINEL_PREFIX: &str = "__omni_proc_field__";
pub(crate) const PROC_FIELD_SENTINEL_ARG: &str = "__proc_field";
pub(crate) const PROC_INIT_FN_SUFFIX: &str = ".__proc_init";
pub(crate) const PROC_BLOCK_PRE_FN_SUFFIX: &str = ".__proc_block_pre";
pub(crate) const PROC_BLOCK_POST_FN_SUFFIX: &str = ".__proc_block_post";
pub(crate) const PROC_STEP_FN_SUFFIX: &str = ".__proc_step";
pub(crate) const PROC_CALL_OUT_FN_PREFIX: &str = ".__proc_call_out";
pub(crate) const PROC_EVENT_FN_PREFIX: &str = ".__proc_event_";
pub(crate) const INTERNAL_BUFFER_READ2_FN: &str = "__omni_buffer_read2";
pub(crate) const INTERNAL_BUFFER_WRITE2_FN: &str = "__omni_buffer_write2";

#[derive(Debug, Clone)]
pub(crate) struct ProcApi {
    pub(crate) ins: Vec<ProcPortSpec>,
    pub(crate) params: HashMap<String, ProcParamSlotSpec>,
    pub(crate) outs: Vec<String>,
    pub(crate) events: HashMap<String, ProcEventSpec>,
    pub(crate) buffers: Vec<ProcBufferSpec>,
    pub(crate) has_block: bool,
    pub(crate) sample_oversample_factor: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcPortSpec {
    pub(crate) name: String,
    pub(crate) slots: Vec<String>,
    pub(crate) defaults: Vec<Option<Expr>>,
    pub(crate) ranges: Vec<Option<TypedValueRange>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcParamSlotSpec {
    pub(crate) name: String,
    pub(crate) ty: PrimitiveType,
    pub(crate) default: Option<Expr>,
    pub(crate) range: Option<TypedValueRange>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcParamSpec {
    pub(crate) name: String,
    pub(crate) slots: Vec<ProcParamSlotSpec>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcBufferSpec {
    pub(crate) name: String,
    pub(crate) elem_ty: PrimitiveType,
    pub(crate) channels: TypedBufferChannels,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcEventSpec {
    pub(crate) params: Vec<ProcEventParamSpec>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcEventParamSpec {
    pub(crate) name: String,
    pub(crate) slots: Vec<ProcEventParamSlotSpec>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcEventParamSlotSpec {
    pub(crate) name: String,
    pub(crate) ty: PrimitiveType,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcCallInstance {
    pub(crate) proc_name: String,
    pub(crate) buffer_args: Vec<Expr>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct ProcStateFields {
    pub(crate) scalars: HashMap<String, PrimitiveType>,
    pub(crate) data: HashMap<String, omni_frontend::DataTypeSpec>,
    pub(crate) nested_procs: HashMap<String, ProcNestedState>,
    pub(crate) struct_instances: HashMap<String, ProcStructState>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcNestedState {
    pub(crate) proc_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcStructState {
    pub(crate) struct_name: String,
    pub(crate) type_args: Vec<PrimitiveType>,
}

pub(crate) fn is_plain_symbol(name: &str) -> bool {
    !name.contains('.')
}

pub(crate) fn record_proc_state_scalar_assignment(
    name: &str,
    decl_ty: Option<PrimitiveType>,
    expr: &Expr,
    out: &mut ProcStateFields,
    state_type_hints: &HashMap<String, PrimitiveType>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    typed_struct_defs: &HashMap<String, Vec<TypedStructField>>,
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    proc_primary_output_types: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) {
    if out.data.contains_key(name)
        || out.struct_instances.contains_key(name)
        || out.nested_procs.contains_key(name)
    {
        errors.push(Diagnostic::semantic(
            format!("processor state symbol '{name}' is used as both scalar and non-scalar value"),
            0,
            0,
        ));
    }
    let existing = out.scalars.get(name).copied();
    let mut inference_scalars = state_type_hints.clone();
    for (state_name, state_ty) in &out.scalars {
        inference_scalars.insert(state_name.clone(), *state_ty);
    }
    for (data_name, spec) in &out.data {
        if let DataElemType::Primitive(elem_ty) = spec.elem {
            inference_scalars.insert(
                declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, data_name),
                elem_ty,
            );
        }
    }
    for (instance_name, state) in &out.struct_instances {
        let Some(struct_template) = struct_defs.get(&state.struct_name) else {
            continue;
        };
        let resolved_struct = if state.type_args.is_empty() {
            Some(struct_template.clone())
        } else {
            specialize_generic_struct_template(struct_template, &state.type_args, errors)
        };
        let Some(resolved_struct) = resolved_struct else {
            continue;
        };
        for field in &resolved_struct.fields {
            let flat = format!("{instance_name}.{}", field.name);
            match field.ty {
                FieldType::Scalar(prim) => {
                    inference_scalars.insert(
                        declared_type_key(DECLARED_STRUCT_FIELD_TYPE_PREFIX, &flat),
                        prim,
                    );
                }
                FieldType::Data(ref spec) => {
                    if let DataElemType::Primitive(elem_ty) = spec.elem {
                        inference_scalars.insert(
                            declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                            elem_ty,
                        );
                    }
                }
                FieldType::Generic(_) => {}
            }
        }
    }
    for (instance, nested) in &out.nested_procs {
        if let Some(ty) = proc_primary_output_types.get(&nested.proc_name).copied() {
            inference_scalars.insert(
                declared_type_key(DECLARED_FUNCTION_RETURN_TYPE_PREFIX, instance),
                ty,
            );
        }
    }
    let struct_instances = out
        .struct_instances
        .iter()
        .map(|(instance_name, state)| (instance_name.clone(), state.struct_name.clone()))
        .collect::<HashMap<_, _>>();
    let ty = match (existing, decl_ty) {
        (Some(existing_ty), Some(declared_ty)) => {
            if existing_ty != declared_ty {
                errors.push(Diagnostic::semantic(
                    format!(
                        "processor state symbol '{name}' has conflicting types {:?} and {:?}",
                        existing_ty, declared_ty
                    ),
                    0,
                    0,
                ));
            }
            existing_ty
        }
        (Some(existing_ty), None) => existing_ty,
        (None, Some(declared_ty)) => declared_ty,
        (None, None) => infer_proc_state_scalar_type(
            expr,
            &inference_scalars,
            input_names,
            output_names,
            param_names,
            &struct_instances,
            typed_struct_defs,
            errors,
        )
        .unwrap_or(PrimitiveType::F32),
    };
    if let Some(existing_ty) = existing {
        if existing_ty != ty {
            errors.push(Diagnostic::semantic(
                format!(
                    "processor state symbol '{name}' has conflicting types {:?} and {:?}",
                    existing_ty, ty
                ),
                0,
                0,
            ));
        }
    } else {
        out.scalars.insert(name.to_owned(), ty);
    }
}

pub(crate) fn is_declared_proc_symbol(
    name: &str,
    reserved: &HashSet<String>,
    locals: &HashSet<String>,
    out: &ProcStateFields,
) -> bool {
    reserved.contains(name)
        || locals.contains(name)
        || out.scalars.contains_key(name)
        || out.data.contains_key(name)
        || out.nested_procs.contains_key(name)
        || out.struct_instances.contains_key(name)
}

pub(crate) fn has_use_before_declaration_error(errors: &[Diagnostic]) -> bool {
    errors
        .iter()
        .any(|d| d.message.contains("used before declaration"))
}

pub(crate) fn validate_proc_expr_decl_order(
    expr: &Expr,
    reserved: &HashSet<String>,
    locals: &HashSet<String>,
    out: &ProcStateFields,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let mut ok = true;
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
        Expr::Var(name) => {
            if is_builtin_constant_name(name) {
                return true;
            }
            if let Some((base, _field)) = split_field_path(name, errors) {
                if !is_declared_proc_symbol(base, reserved, locals, out) {
                    errors.push(Diagnostic::semantic(
                        format!("symbol '{base}' used before declaration"),
                        0,
                        0,
                    ));
                    ok = false;
                }
            } else if !is_declared_proc_symbol(name, reserved, locals, out) {
                errors.push(Diagnostic::semantic(
                    format!("symbol '{name}' used before declaration"),
                    0,
                    0,
                ));
                ok = false;
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, _field)) = split_field_path(base, errors) {
                if !is_declared_proc_symbol(root, reserved, locals, out) {
                    errors.push(Diagnostic::semantic(
                        format!("symbol '{root}' used before declaration"),
                        0,
                        0,
                    ));
                    ok = false;
                }
            } else if !is_declared_proc_symbol(base, reserved, locals, out) {
                errors.push(Diagnostic::semantic(
                    format!("symbol '{base}' used before declaration"),
                    0,
                    0,
                ));
                ok = false;
            }
            ok &= validate_proc_expr_decl_order(index, reserved, locals, out, errors);
        }
        Expr::DataCtor { spec, init } => {
            ok &= validate_proc_expr_decl_order(&spec.size, reserved, locals, out, errors);
            if let Some(values) = init {
                for value in values {
                    ok &= validate_proc_expr_decl_order(value, reserved, locals, out, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            ok &= validate_proc_expr_decl_order(lhs, reserved, locals, out, errors);
            ok &= validate_proc_expr_decl_order(rhs, reserved, locals, out, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                ok &= validate_proc_expr_decl_order(arg, reserved, locals, out, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                ok &= validate_proc_expr_decl_order(&arg.expr, reserved, locals, out, errors);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            ok &= validate_proc_expr_decl_order(inner, reserved, locals, out, errors);
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                ok &= validate_proc_expr_decl_order(value, reserved, locals, out, errors);
            }
        }
    }
    ok
}

pub(crate) fn collect_proc_state_fields(
    stmt: &Stmt,
    reserved: &HashSet<String>,
    locals: &HashSet<String>,
    current_ns: &str,
    proc_symbols: &HashSet<String>,
    state_type_hints: &HashMap<String, PrimitiveType>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    typed_struct_defs: &HashMap<String, Vec<TypedStructField>>,
    proc_primary_output_types: &HashMap<String, PrimitiveType>,
    struct_symbols: &HashSet<String>,
    struct_defs: &HashMap<String, omni_frontend::StructDef>,
    ctor_symbols: &HashSet<String>,
    in_init_scope: bool,
    out: &mut ProcStateFields,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            is_typed_decl: _,
            expr,
            ..
        } => {
            let mut stmt_ok = true;
            match target {
                AssignTarget::Var(_) => {}
                AssignTarget::Index { base, index } => {
                    if let Some((root, _field)) = split_field_path(base, errors) {
                        if !is_declared_proc_symbol(root, reserved, locals, out) {
                            errors.push(Diagnostic::semantic(
                                format!("symbol '{root}' used before declaration"),
                                0,
                                0,
                            ));
                            stmt_ok = false;
                        }
                    } else if !is_declared_proc_symbol(base, reserved, locals, out) {
                        errors.push(Diagnostic::semantic(
                            format!("symbol '{base}' used before declaration"),
                            0,
                            0,
                        ));
                        stmt_ok = false;
                    }
                    stmt_ok &= validate_proc_expr_decl_order(index, reserved, locals, out, errors);
                }
            }
            stmt_ok &= validate_proc_expr_decl_order(expr, reserved, locals, out, errors);
            if stmt_ok {
                if let AssignTarget::Var(name) = target {
                    if is_plain_symbol(name)
                        && !reserved.contains(name)
                        && !is_builtin_constant_name(name)
                    {
                        match expr {
                            Expr::DataCtor { spec, .. } => {
                                if out.scalars.contains_key(name)
                                    || out.struct_instances.contains_key(name)
                                {
                                    errors.push(Diagnostic::semantic(
                                    format!(
                                        "processor state symbol '{name}' is used as both Data and non-Data value"
                                    ),
                                    0,
                                    0,
                                ));
                                }
                                if out.nested_procs.contains_key(name) {
                                    errors.push(Diagnostic::semantic(
                                    format!(
                                        "processor state symbol '{name}' is used as both Data and processor instance"
                                    ),
                                    0,
                                    0,
                                ));
                                }
                                out.data.entry(name.clone()).or_insert_with(|| spec.clone());
                            }
                            Expr::UserCall {
                                name: ctor,
                                type_args,
                                args,
                            } => {
                                let mut handled_as_constructor = false;
                                let resolved_proc_ctor = if ctor.contains("::") {
                                    if proc_symbols.contains(ctor) {
                                        Some(ctor.clone())
                                    } else {
                                        None
                                    }
                                } else {
                                    resolve_unqualified_symbol_name(ctor, current_ns, proc_symbols)
                                };
                                if let Some(proc_ctor) = resolved_proc_ctor {
                                    handled_as_constructor = true;
                                    if !type_args.is_empty() {
                                        errors.push(Diagnostic::semantic(
                                        format!(
                                            "processor '{}' is not generic and cannot take type arguments",
                                            proc_ctor
                                        ),
                                        0,
                                        0,
                                    ));
                                    } else if let Some(existing) = out.nested_procs.get(name) {
                                        if existing.proc_name != proc_ctor {
                                            errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor state symbol '{name}' has conflicting processor types '{}' and '{}'",
                                                existing.proc_name, proc_ctor
                                            ),
                                            0,
                                            0,
                                        ));
                                        }
                                    } else {
                                        if !in_init_scope {
                                            errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor state constructor '{name} = {proc_ctor}(...)' is only allowed in processor init block"
                                            ),
                                            0,
                                            0,
                                        ));
                                        }
                                        if out.scalars.contains_key(name)
                                            || out.data.contains_key(name)
                                            || out.struct_instances.contains_key(name)
                                        {
                                            errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor state symbol '{name}' is used as both processor instance and non-processor value"
                                            ),
                                            0,
                                            0,
                                        ));
                                        } else {
                                            out.nested_procs.insert(
                                                name.clone(),
                                                ProcNestedState {
                                                    proc_name: proc_ctor,
                                                },
                                            );
                                        }
                                    }
                                } else {
                                    let resolved_struct_ctor = if ctor.contains("::") {
                                        if struct_symbols.contains(ctor) {
                                            Some(ctor.clone())
                                        } else {
                                            None
                                        }
                                    } else {
                                        resolve_unqualified_symbol_name(
                                            ctor,
                                            current_ns,
                                            struct_symbols,
                                        )
                                    };
                                    if let Some(struct_ctor) = resolved_struct_ctor {
                                        handled_as_constructor = true;
                                        if !in_init_scope {
                                            errors.push(Diagnostic::semantic(
                                            format!(
                                                "processor state constructor '{name} = {struct_ctor}(...)' is only allowed in processor init block"
                                            ),
                                            0,
                                            0,
                                        ));
                                        }
                                        let resolved_type_args = match struct_defs.get(&struct_ctor)
                                        {
                                            Some(struct_template) => {
                                                if type_args.is_empty() {
                                                    if !struct_template.type_params.is_empty() {
                                                        infer_generic_struct_ctor_type_args(
                                                            struct_template,
                                                            args,
                                                            &out.scalars,
                                                            &HashMap::new(),
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
                                                } else if type_args.len()
                                                    != struct_template.type_params.len()
                                                {
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
                                                        &format!(
                                                            "struct constructor '{}'",
                                                            struct_ctor
                                                        ),
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
                                            if out.scalars.contains_key(name)
                                                || out.data.contains_key(name)
                                                || out.nested_procs.contains_key(name)
                                            {
                                                errors.push(Diagnostic::semantic(
                                                format!(
                                                    "processor state symbol '{name}' is used as both struct instance and non-struct value"
                                                ),
                                                0,
                                                0,
                                            ));
                                            } else if let Some(existing) =
                                                out.struct_instances.get(name)
                                            {
                                                let current = ProcStructState {
                                                    struct_name: struct_ctor.clone(),
                                                    type_args: resolved_type_args.clone(),
                                                };
                                                if existing != &current {
                                                    errors.push(Diagnostic::semantic(
                                                    format!(
                                                        "processor state symbol '{name}' has conflicting struct constructor specializations"
                                                    ),
                                                    0,
                                                    0,
                                                ));
                                                }
                                            } else {
                                                out.struct_instances.insert(
                                                    name.clone(),
                                                    ProcStructState {
                                                        struct_name: struct_ctor,
                                                        type_args: resolved_type_args,
                                                    },
                                                );
                                            }
                                        }
                                    } else if ctor_symbols.contains(ctor) {
                                        handled_as_constructor = true;
                                        errors.push(Diagnostic::semantic(
                                        format!(
                                            "processor state constructor '{name} = {ctor}(...)' is only supported for known struct or processor constructors"
                                        ),
                                        0,
                                        0,
                                    ));
                                    }
                                }
                                if !handled_as_constructor {
                                    record_proc_state_scalar_assignment(
                                        name,
                                        *decl_ty,
                                        expr,
                                        out,
                                        state_type_hints,
                                        input_names,
                                        output_names,
                                        param_names,
                                        typed_struct_defs,
                                        struct_defs,
                                        proc_primary_output_types,
                                        errors,
                                    );
                                }
                            }
                            _ => {
                                record_proc_state_scalar_assignment(
                                    name,
                                    *decl_ty,
                                    expr,
                                    out,
                                    state_type_hints,
                                    input_names,
                                    output_names,
                                    param_names,
                                    typed_struct_defs,
                                    struct_defs,
                                    proc_primary_output_types,
                                    errors,
                                );
                            }
                        }
                    }
                }
            }
            if !stmt_ok {
                if let AssignTarget::Var(name) = target {
                    if is_plain_symbol(name)
                        && !reserved.contains(name)
                        && !is_builtin_constant_name(name)
                        && !out.data.contains_key(name)
                        && !out.struct_instances.contains_key(name)
                        && !out.nested_procs.contains_key(name)
                    {
                        match expr {
                            Expr::DataCtor { .. } | Expr::UserCall { .. } => {}
                            _ => {
                                out.scalars
                                    .entry(name.clone())
                                    .or_insert(decl_ty.unwrap_or(PrimitiveType::F32));
                                out.scalars.insert(
                                    declared_type_key(DECLARED_INVALID_PLACEHOLDER_PREFIX, name),
                                    PrimitiveType::Bool,
                                );
                            }
                        }
                    }
                }
            }
            if stmt_ok {
                collect_proc_state_expr_fields(expr, reserved, ctor_symbols, out, errors);
            }
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            if validate_proc_expr_decl_order(expr, reserved, locals, out, errors) {
                collect_proc_state_expr_fields(expr, reserved, ctor_symbols, out, errors);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            if validate_proc_expr_decl_order(cond, reserved, locals, out, errors) {
                collect_proc_state_expr_fields(cond, reserved, ctor_symbols, out, errors);
            }
            for s in then_branch {
                collect_proc_state_fields(
                    s,
                    reserved,
                    locals,
                    current_ns,
                    proc_symbols,
                    state_type_hints,
                    input_names,
                    output_names,
                    param_names,
                    typed_struct_defs,
                    proc_primary_output_types,
                    struct_symbols,
                    struct_defs,
                    ctor_symbols,
                    in_init_scope,
                    out,
                    errors,
                );
            }
            for s in else_branch {
                collect_proc_state_fields(
                    s,
                    reserved,
                    locals,
                    current_ns,
                    proc_symbols,
                    state_type_hints,
                    input_names,
                    output_names,
                    param_names,
                    typed_struct_defs,
                    proc_primary_output_types,
                    struct_symbols,
                    struct_defs,
                    ctor_symbols,
                    in_init_scope,
                    out,
                    errors,
                );
            }
        }
        Stmt::For {
            var,
            start,
            end,
            step,
            body,
            ..
        } => {
            if validate_proc_expr_decl_order(start, reserved, locals, out, errors) {
                collect_proc_state_expr_fields(start, reserved, ctor_symbols, out, errors);
            }
            if validate_proc_expr_decl_order(end, reserved, locals, out, errors) {
                collect_proc_state_expr_fields(end, reserved, ctor_symbols, out, errors);
            }
            if let Some(step_expr) = step {
                if validate_proc_expr_decl_order(step_expr, reserved, locals, out, errors) {
                    collect_proc_state_expr_fields(step_expr, reserved, ctor_symbols, out, errors);
                }
            }
            let mut loop_locals = locals.clone();
            loop_locals.insert(var.clone());
            for s in body {
                collect_proc_state_fields(
                    s,
                    reserved,
                    &loop_locals,
                    current_ns,
                    proc_symbols,
                    state_type_hints,
                    input_names,
                    output_names,
                    param_names,
                    typed_struct_defs,
                    proc_primary_output_types,
                    struct_symbols,
                    struct_defs,
                    ctor_symbols,
                    in_init_scope,
                    out,
                    errors,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            if validate_proc_expr_decl_order(cond, reserved, locals, out, errors) {
                collect_proc_state_expr_fields(cond, reserved, ctor_symbols, out, errors);
            }
            for s in body {
                collect_proc_state_fields(
                    s,
                    reserved,
                    locals,
                    current_ns,
                    proc_symbols,
                    state_type_hints,
                    input_names,
                    output_names,
                    param_names,
                    typed_struct_defs,
                    proc_primary_output_types,
                    struct_symbols,
                    struct_defs,
                    ctor_symbols,
                    in_init_scope,
                    out,
                    errors,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(crate) fn infer_proc_state_scalar_type(
    expr: &Expr,
    known_scalars: &HashMap<String, PrimitiveType>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    let locals = HashSet::<String>::new();
    let local_data_aliases = HashMap::<String, LocalDataAliasInfo>::new();
    infer_scalar_expr_type(
        expr,
        known_scalars,
        &local_data_aliases,
        &locals,
        input_names,
        output_names,
        param_names,
        struct_instances,
        struct_defs,
        errors,
    )
}

pub(crate) fn collect_proc_state_expr_fields(
    expr: &Expr,
    reserved: &HashSet<String>,
    ctor_symbols: &HashSet<String>,
    out: &mut ProcStateFields,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Index { index, .. } => {
            collect_proc_state_expr_fields(index, reserved, ctor_symbols, out, errors);
        }
        Expr::DataCtor { spec, .. } => {
            collect_proc_state_expr_fields(&spec.size, reserved, ctor_symbols, out, errors);
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_proc_state_expr_fields(lhs, reserved, ctor_symbols, out, errors);
            collect_proc_state_expr_fields(rhs, reserved, ctor_symbols, out, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_proc_state_expr_fields(arg, reserved, ctor_symbols, out, errors);
            }
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                collect_proc_state_expr_fields(value, reserved, ctor_symbols, out, errors);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            collect_proc_state_expr_fields(inner, reserved, ctor_symbols, out, errors);
        }
        Expr::UserCall { name, args, .. } => {
            if name.starts_with(PROC_FIELD_SENTINEL_PREFIX) {
                // Processor instance validation is handled after proc desugaring/rewrite.
            }
            for arg in args {
                collect_proc_state_expr_fields(&arg.expr, reserved, ctor_symbols, out, errors);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

pub(crate) fn rewrite_proc_expr_symbols(
    expr: &mut Expr,
    owner_proc: &str,
    field_names: &HashSet<String>,
    field_array_slots: &HashMap<String, Vec<String>>,
    in_array_slots: &HashMap<String, Vec<String>>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Var(name) => {
            if field_names.contains(name) && is_plain_symbol(name) {
                *name = format!("self.{name}");
            }
        }
        Expr::Index { base, index } => {
            rewrite_proc_expr_symbols(
                index,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
            if let Some(slots) = field_array_slots.get(base.as_str()) {
                if slots.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!("processor array field '{base}' has zero slots"),
                        0,
                        0,
                    ));
                    return;
                }
                if let Some(raw_idx) = try_constant_index_i64(index) {
                    let Some(slot_idx) = resolve_proc_constant_slot_index(
                        raw_idx,
                        slots.len(),
                        &format!("processor array field '{base}'"),
                        errors,
                    ) else {
                        return;
                    };
                    if let Some(slot_name) = slots.get(slot_idx) {
                        *expr = Expr::Var(format!("self.{slot_name}"));
                    }
                } else if slots.len() == 1 {
                    if let Some(slot_name) = slots.first() {
                        *expr = Expr::Var(format!("self.{slot_name}"));
                    }
                } else {
                    let mut args = Vec::<CallArg>::new();
                    args.push(CallArg {
                        name: None,
                        expr: *index.clone(),
                    });
                    for slot in slots {
                        args.push(CallArg {
                            name: None,
                            expr: Expr::Var(format!("self.{slot}")),
                        });
                    }
                    *expr = Expr::UserCall {
                        name: proc_read_helper_name(owner_proc, slots.len(), false),
                        type_args: Vec::new(),
                        args,
                    };
                }
                return;
            }
            if let Some(slots) = in_array_slots.get(base.as_str()) {
                if slots.is_empty() {
                    errors.push(Diagnostic::semantic(
                        format!("processor input array '{base}' has zero slots"),
                        0,
                        0,
                    ));
                    return;
                }
                if let Some(raw_idx) = try_constant_index_i64(index) {
                    let Some(slot_idx) = resolve_proc_constant_slot_index(
                        raw_idx,
                        slots.len(),
                        &format!("processor input array '{base}'"),
                        errors,
                    ) else {
                        return;
                    };
                    if let Some(slot_name) = slots.get(slot_idx) {
                        *expr = Expr::Var(slot_name.clone());
                    }
                } else if slots.len() == 1 {
                    if let Some(slot_name) = slots.first() {
                        *expr = Expr::Var(slot_name.clone());
                    }
                } else {
                    let mut args = Vec::<CallArg>::new();
                    args.push(CallArg {
                        name: None,
                        expr: *index.clone(),
                    });
                    for slot in slots {
                        args.push(CallArg {
                            name: None,
                            expr: Expr::Var(slot.clone()),
                        });
                    }
                    *expr = Expr::UserCall {
                        name: proc_read_helper_name(owner_proc, slots.len(), false),
                        type_args: Vec::new(),
                        args,
                    };
                }
                return;
            }
            if field_names.contains(base) && is_plain_symbol(base) {
                *base = format!("self.{base}");
            }
        }
        Expr::DataCtor { spec, init } => {
            rewrite_proc_expr_symbols(
                &mut spec.size,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    rewrite_proc_expr_symbols(
                        value,
                        owner_proc,
                        field_names,
                        field_array_slots,
                        in_array_slots,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_proc_expr_symbols(
                lhs,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
            rewrite_proc_expr_symbols(
                rhs,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_proc_expr_symbols(
                    arg,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_proc_expr_symbols(
                    &mut arg.expr,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
            }
            if let Expr::UserCall { name, args, .. } = expr {
                if let Some(base) = parse_data_len_instance_base(name) {
                    if field_names.contains(base) && is_plain_symbol(base) {
                        *name = format!("self.{base}.len");
                    }
                } else if let Some(base) = parse_buffer_chans_instance_base(name) {
                    if field_names.contains(base) && is_plain_symbol(base) {
                        *name = format!("self.{base}.chans");
                    }
                }
                if name == "unsafe_read" && args.len() == 2 {
                    if let Expr::Var(base) = &args[0].expr {
                        if let Some(slots) = field_array_slots.get(base.as_str()) {
                            if slots.is_empty() {
                                errors.push(Diagnostic::semantic(
                                    format!("processor array field '{base}' has zero slots"),
                                    0,
                                    0,
                                ));
                                return;
                            }
                            let idx_expr = args[1].expr.clone();
                            if let Some(raw_idx) = try_constant_index_i64(&idx_expr) {
                                if raw_idx >= 0 && raw_idx < slots.len() as i64 {
                                    let slot_idx = raw_idx as usize;
                                    if let Some(slot_name) = slots.get(slot_idx) {
                                        *expr = Expr::Var(format!("self.{slot_name}"));
                                    }
                                } else {
                                    let mut call_args = Vec::<CallArg>::new();
                                    call_args.push(CallArg {
                                        name: None,
                                        expr: idx_expr,
                                    });
                                    for slot in slots {
                                        call_args.push(CallArg {
                                            name: None,
                                            expr: Expr::Var(format!("self.{slot}")),
                                        });
                                    }
                                    *expr = Expr::UserCall {
                                        name: proc_read_helper_name(owner_proc, slots.len(), true),
                                        type_args: Vec::new(),
                                        args: call_args,
                                    };
                                }
                            } else if slots.len() == 1 {
                                if let Some(slot_name) = slots.first() {
                                    *expr = Expr::Var(format!("self.{slot_name}"));
                                }
                            } else {
                                let mut call_args = Vec::<CallArg>::new();
                                call_args.push(CallArg {
                                    name: None,
                                    expr: idx_expr,
                                });
                                for slot in slots {
                                    call_args.push(CallArg {
                                        name: None,
                                        expr: Expr::Var(format!("self.{slot}")),
                                    });
                                }
                                *expr = Expr::UserCall {
                                    name: proc_read_helper_name(owner_proc, slots.len(), true),
                                    type_args: Vec::new(),
                                    args: call_args,
                                };
                            }
                            return;
                        }
                        if let Some(slots) = in_array_slots.get(base.as_str()) {
                            if slots.is_empty() {
                                errors.push(Diagnostic::semantic(
                                    format!("processor input array '{base}' has zero slots"),
                                    0,
                                    0,
                                ));
                                return;
                            }
                            let idx_expr = args[1].expr.clone();
                            if let Some(raw_idx) = try_constant_index_i64(&idx_expr) {
                                if raw_idx >= 0 && raw_idx < slots.len() as i64 {
                                    let slot_idx = raw_idx as usize;
                                    if let Some(slot_name) = slots.get(slot_idx) {
                                        *expr = Expr::Var(slot_name.clone());
                                    }
                                } else {
                                    let mut call_args = Vec::<CallArg>::new();
                                    call_args.push(CallArg {
                                        name: None,
                                        expr: idx_expr,
                                    });
                                    for slot in slots {
                                        call_args.push(CallArg {
                                            name: None,
                                            expr: Expr::Var(slot.clone()),
                                        });
                                    }
                                    *expr = Expr::UserCall {
                                        name: proc_read_helper_name(owner_proc, slots.len(), true),
                                        type_args: Vec::new(),
                                        args: call_args,
                                    };
                                }
                            } else if slots.len() == 1 {
                                if let Some(slot_name) = slots.first() {
                                    *expr = Expr::Var(slot_name.clone());
                                }
                            } else {
                                let mut call_args = Vec::<CallArg>::new();
                                call_args.push(CallArg {
                                    name: None,
                                    expr: idx_expr,
                                });
                                for slot in slots {
                                    call_args.push(CallArg {
                                        name: None,
                                        expr: Expr::Var(slot.clone()),
                                    });
                                }
                                *expr = Expr::UserCall {
                                    name: proc_read_helper_name(owner_proc, slots.len(), true),
                                    type_args: Vec::new(),
                                    args: call_args,
                                };
                            }
                            return;
                        }
                    }
                }
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_proc_expr_symbols(
                inner,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_proc_expr_symbols(
                    value,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

pub(crate) fn rewrite_proc_stmt_symbols(
    stmt: &Stmt,
    owner_proc: &str,
    field_names: &HashSet<String>,
    data_fields: &HashSet<String>,
    ins_names: &HashSet<String>,
    field_array_slots: &HashMap<String, Vec<String>>,
    in_array_slots: &HashMap<String, Vec<String>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Stmt> {
    with_stmt_diag_context(stmt, || {
        let source_loc = stmt.loc().cloned();
        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => {
                let mut expr_rewritten = expr.clone();
                rewrite_proc_expr_symbols(
                    &mut expr_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                match target {
                    AssignTarget::Var(name) => {
                        if ins_names.contains(name) {
                            errors.push(Diagnostic::semantic(
                                format!("cannot assign to processor input '{name}'"),
                                0,
                                0,
                            ));
                            return Some(Stmt::Assign {
                                loc: source_loc.clone(),
                                target: AssignTarget::Var(name.clone()),
                                decl_ty: *decl_ty,
                                generic_decl_ty: generic_decl_ty.clone(),
                                is_typed_decl: *is_typed_decl,
                                expr: expr_rewritten,
                            });
                        }
                        if field_names.contains(name) && is_plain_symbol(name) {
                            if matches!(expr, Expr::DataCtor { .. }) && data_fields.contains(name) {
                                return None;
                            }
                            return Some(Stmt::Assign {
                                loc: source_loc.clone(),
                                target: AssignTarget::Var(format!("self.{name}")),
                                decl_ty: None,
                                generic_decl_ty: None,
                                is_typed_decl: false,
                                expr: expr_rewritten,
                            });
                        }
                        Some(Stmt::Assign {
                            loc: source_loc.clone(),
                            target: AssignTarget::Var(name.clone()),
                            decl_ty: *decl_ty,
                            generic_decl_ty: generic_decl_ty.clone(),
                            is_typed_decl: *is_typed_decl,
                            expr: expr_rewritten,
                        })
                    }
                    AssignTarget::Index { base, index } => {
                        let mut idx_rewritten = index.clone();
                        rewrite_proc_expr_symbols(
                            &mut idx_rewritten,
                            owner_proc,
                            field_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        );
                        if let Some(slots) = in_array_slots.get(base) {
                            errors.push(Diagnostic::semantic(
                                format!("cannot assign to processor input '{base}'"),
                                0,
                                0,
                            ));
                            if let Some(raw_idx) = try_constant_index_i64(&idx_rewritten) {
                                if let Some(slot_idx) = resolve_proc_constant_slot_index(
                                    raw_idx,
                                    slots.len(),
                                    &format!("processor input array assignment '{base}[...]'"),
                                    errors,
                                ) {
                                    if let Some(slot_name) = slots.get(slot_idx) {
                                        return Some(Stmt::Assign {
                                            loc: None,
                                            target: AssignTarget::Var(slot_name.clone()),
                                            decl_ty: *decl_ty,
                                            generic_decl_ty: generic_decl_ty.clone(),
                                            is_typed_decl: *is_typed_decl,
                                            expr: expr_rewritten,
                                        });
                                    }
                                }
                            }
                        }
                        if let Some(slots) = field_array_slots.get(base) {
                            if let Some(raw_idx) = try_constant_index_i64(&idx_rewritten) {
                                let Some(slot_idx) = resolve_proc_constant_slot_index(
                                    raw_idx,
                                    slots.len(),
                                    &format!("processor array field assignment '{base}[...]'"),
                                    errors,
                                ) else {
                                    return Some(Stmt::Assign {
                                        loc: source_loc.clone(),
                                        target: AssignTarget::Index {
                                            base: base.clone(),
                                            index: idx_rewritten,
                                        },
                                        decl_ty: *decl_ty,
                                        generic_decl_ty: generic_decl_ty.clone(),
                                        is_typed_decl: *is_typed_decl,
                                        expr: expr_rewritten,
                                    });
                                };
                                if let Some(slot_name) = slots.get(slot_idx) {
                                    return Some(Stmt::Assign {
                                        loc: source_loc.clone(),
                                        target: AssignTarget::Var(format!("self.{slot_name}")),
                                        decl_ty: None,
                                        generic_decl_ty: None,
                                        is_typed_decl: false,
                                        expr: expr_rewritten,
                                    });
                                }
                            } else {
                                return Some(Stmt::Expr {
                                    loc: source_loc.clone(),
                                    expr: Expr::UserCall {
                                        name: proc_write_helper_name(owner_proc, slots, false),
                                        type_args: Vec::new(),
                                        args: vec![
                                            CallArg {
                                                name: None,
                                                expr: Expr::Var("self".to_owned()),
                                            },
                                            CallArg {
                                                name: None,
                                                expr: idx_rewritten,
                                            },
                                            CallArg {
                                                name: None,
                                                expr: expr_rewritten,
                                            },
                                        ],
                                    },
                                });
                            }
                        }
                        let target_base = if field_names.contains(base) && is_plain_symbol(base) {
                            format!("self.{base}")
                        } else {
                            base.clone()
                        };
                        Some(Stmt::Assign {
                            loc: source_loc.clone(),
                            target: AssignTarget::Index {
                                base: target_base,
                                index: idx_rewritten,
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            expr: expr_rewritten,
                        })
                    }
                }
            }
            Stmt::Expr { expr, .. } => {
                let mut expr_rewritten = expr.clone();
                rewrite_proc_expr_symbols(
                    &mut expr_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                if let Expr::UserCall { name, args, .. } = &expr_rewritten {
                    if name == "unsafe_write" && args.len() == 3 {
                        if let Expr::Var(base) = &args[0].expr {
                            if let Some(slots) = in_array_slots.get(base) {
                                errors.push(Diagnostic::semantic(
                                    format!("cannot assign to processor input '{base}'"),
                                    0,
                                    0,
                                ));
                                if let Some(raw_idx) = try_constant_index_i64(&args[1].expr) {
                                    if raw_idx >= 0 && raw_idx < slots.len() as i64 {
                                        let slot_idx = raw_idx as usize;
                                        if let Some(slot_name) = slots.get(slot_idx) {
                                            return Some(Stmt::Assign {
                                                loc: source_loc.clone(),
                                                target: AssignTarget::Var(slot_name.clone()),
                                                decl_ty: None,
                                                generic_decl_ty: None,
                                                is_typed_decl: false,
                                                expr: args[2].expr.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                            if let Some(slots) = field_array_slots.get(base) {
                                if let Some(raw_idx) = try_constant_index_i64(&args[1].expr) {
                                    if raw_idx >= 0 && raw_idx < slots.len() as i64 {
                                        let slot_idx = raw_idx as usize;
                                        if let Some(slot_name) = slots.get(slot_idx) {
                                            return Some(Stmt::Assign {
                                                loc: source_loc.clone(),
                                                target: AssignTarget::Var(format!(
                                                    "self.{slot_name}"
                                                )),
                                                decl_ty: None,
                                                generic_decl_ty: None,
                                                is_typed_decl: false,
                                                expr: args[2].expr.clone(),
                                            });
                                        }
                                    }
                                }
                                return Some(Stmt::Expr {
                                    loc: source_loc.clone(),
                                    expr: Expr::UserCall {
                                        name: proc_write_helper_name(owner_proc, slots, true),
                                        type_args: Vec::new(),
                                        args: vec![
                                            CallArg {
                                                name: None,
                                                expr: Expr::Var("self".to_owned()),
                                            },
                                            CallArg {
                                                name: None,
                                                expr: args[1].expr.clone(),
                                            },
                                            CallArg {
                                                name: None,
                                                expr: args[2].expr.clone(),
                                            },
                                        ],
                                    },
                                });
                            }
                        }
                    }
                }
                Some(Stmt::Expr {
                    loc: source_loc.clone(),
                    expr: expr_rewritten,
                })
            }
            Stmt::Return { expr, .. } => {
                let mut expr_rewritten = expr.clone();
                rewrite_proc_expr_symbols(
                    &mut expr_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                Some(Stmt::Return {
                    loc: source_loc.clone(),
                    expr: expr_rewritten,
                })
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let mut cond_rewritten = cond.clone();
                rewrite_proc_expr_symbols(
                    &mut cond_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                let then_branch = then_branch
                    .iter()
                    .filter_map(|s| {
                        rewrite_proc_stmt_symbols(
                            s,
                            owner_proc,
                            field_names,
                            data_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                let else_branch = else_branch
                    .iter()
                    .filter_map(|s| {
                        rewrite_proc_stmt_symbols(
                            s,
                            owner_proc,
                            field_names,
                            data_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(Stmt::If {
                    loc: source_loc.clone(),
                    cond: cond_rewritten,
                    then_branch,
                    else_branch,
                })
            }
            Stmt::For {
                loc: _stmt_loc,
                var,
                start,
                end,
                step,
                end_inclusive,
                body,
                ..
            } => {
                let mut start_rewritten = start.clone();
                let mut end_rewritten = end.clone();
                let mut step_rewritten = step.clone();
                rewrite_proc_expr_symbols(
                    &mut start_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                rewrite_proc_expr_symbols(
                    &mut end_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                if let Some(step_expr) = &mut step_rewritten {
                    rewrite_proc_expr_symbols(
                        step_expr,
                        owner_proc,
                        field_names,
                        field_array_slots,
                        in_array_slots,
                        errors,
                    );
                }
                let body = body
                    .iter()
                    .filter_map(|s| {
                        rewrite_proc_stmt_symbols(
                            s,
                            owner_proc,
                            field_names,
                            data_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(Stmt::For {
                    loc: source_loc,
                    var: var.clone(),
                    start: start_rewritten,
                    end: end_rewritten,
                    step: step_rewritten,
                    end_inclusive: *end_inclusive,
                    body,
                })
            }
            Stmt::While {
                loc: _stmt_loc,
                cond,
                body,
                ..
            } => {
                let mut cond_rewritten = cond.clone();
                rewrite_proc_expr_symbols(
                    &mut cond_rewritten,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
                let body = body
                    .iter()
                    .filter_map(|s| {
                        rewrite_proc_stmt_symbols(
                            s,
                            owner_proc,
                            field_names,
                            data_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(Stmt::While {
                    loc: source_loc,
                    cond: cond_rewritten,
                    body,
                })
            }
            Stmt::Break { .. } => Some(Stmt::Break { loc: source_loc }),
            Stmt::Continue { .. } => Some(Stmt::Continue { loc: source_loc }),
        }
    })
}
