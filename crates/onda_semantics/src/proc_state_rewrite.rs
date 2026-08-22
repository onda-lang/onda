use super::*;

pub(crate) const PROC_FIELD_SENTINEL_PREFIX: &str = "__onda_proc_field__";
pub(crate) const PROC_FIELD_SENTINEL_ARG: &str = "__onda_proc_field";
pub(crate) const PROC_INDEX_CALL_SENTINEL: &str = "__onda_proc_index_call";
pub(crate) const PROC_INDEX_BUFFER_SELECT_SENTINEL: &str = "__onda_proc_index_buffer_select";
pub(crate) const PROC_INDEX_BASE_ARG: &str = "__onda_proc_index_base";
pub(crate) const PROC_INDEX_EXPR_ARG: &str = "__onda_proc_index_expr";
pub(crate) const PROC_INDEX_UNCHECKED_ARG: &str = "__onda_proc_index_unchecked";
pub(crate) const STRUCT_ARRAY_FIELD_INDEX_SENTINEL: &str = "__onda_struct_array_field_index";
pub(crate) const SAFI_BASE_ARG: &str = "__safi_base";
pub(crate) const SAFI_IDX_ARG: &str = "__safi_idx";
pub(crate) const SAFI_FIELD_ARG: &str = "__safi_field";
pub(crate) const SAFI_FIELD_IDX_ARG: &str = "__safi_field_idx";
pub(crate) const PROC_INIT_FN_SUFFIX: &str = ".__onda_proc_init";
pub(crate) const PROC_BLOCK_PRE_FN_SUFFIX: &str = ".__onda_proc_block_pre";
pub(crate) const PROC_BLOCK_POST_FN_SUFFIX: &str = ".__onda_proc_block_post";
pub(crate) const PROC_STEP_FN_SUFFIX: &str = ".__onda_proc_step";
pub(crate) const PROC_CALL_OUT_FN_PREFIX: &str = ".__onda_proc_call_out";
pub(crate) const PROC_EVENT_FN_PREFIX: &str = ".__onda_proc_event_";

#[derive(Debug, Clone)]
pub(crate) struct ProcApi {
    pub(crate) ins: Vec<ProcPortSpec>,
    pub(crate) params: HashMap<String, ProcParamSlotSpec>,
    pub(crate) has_bound_params: bool,
    pub(crate) outputs: ProcOutputs,
    pub(crate) events: HashMap<String, ProcEventSpec>,
    pub(crate) buffers: Vec<ProcBufferSpec>,
    pub(crate) has_block: bool,
    pub(crate) sample_oversample_factor: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcOutputs {
    pub(crate) names: Vec<String>,
    pub(crate) timing: OutputTiming,
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
    pub(crate) private: bool,
    pub(crate) ty: PrimitiveType,
    pub(crate) default: Option<Expr>,
    pub(crate) range: Option<TypedValueRange>,
    pub(crate) bind: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcParamSpec {
    pub(crate) name: String,
    pub(crate) slots: Vec<ProcParamSlotSpec>,
}

impl ProcParamSpec {
    pub(crate) fn is_private(&self) -> bool {
        self.slots.iter().any(|slot| slot.private)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProcBufferSpec {
    pub(crate) name: String,
    pub(crate) elem_ty: PrimitiveType,
    pub(crate) channels: TypedBufferChannels,
    pub(crate) array_len: usize,
    pub(crate) is_array: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcEventSpec {
    pub(crate) params: Vec<ProcEventParamSpec>,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcEventParamSpec {
    pub(crate) name: String,
    pub(crate) slots: Vec<ProcEventParamSlotSpec>,
    pub(crate) ty: ProcEventParamTypeSpec,
    pub(crate) default: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcEventParamTypeSpec {
    Scalar { ty: PrimitiveType },
    FixedArray { elem_ty: PrimitiveType, len: usize },
    Slice { elem_ty: PrimitiveType },
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
    pub(crate) data: HashMap<String, onda_frontend::ArrayTypeSpec>,
    pub(crate) nested_procs: HashMap<String, ProcNestedState>,
    pub(crate) nested_proc_arrays: HashMap<String, ProcNestedArrayState>,
    pub(crate) struct_instances: HashMap<String, ProcStructState>,
}

impl ProcStateFields {
    pub(crate) fn has_non_scalar(&self, name: &str) -> bool {
        self.data.contains_key(name)
            || self.struct_instances.contains_key(name)
            || self.nested_procs.contains_key(name)
            || self.nested_proc_arrays.contains_key(name)
    }

    pub(crate) fn has_any(&self, name: &str) -> bool {
        self.scalars.contains_key(name) || self.has_non_scalar(name)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProcNestedState {
    pub(crate) proc_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcNestedArrayState {
    pub(crate) proc_name: String,
    pub(crate) size_expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcStructState {
    pub(crate) struct_name: String,
    pub(crate) type_args: Vec<PrimitiveType>,
}

pub(crate) fn is_plain_symbol(name: &str) -> bool {
    !name.contains('.')
}

pub(crate) fn convert_init_state_to_proc_fields(st: &InitAnalysisState) -> ProcStateFields {
    let mut psf = ProcStateFields::default();

    // Copy scalar state declared during init analysis.
    for (name, ty) in &st.state_scalars {
        psf.scalars.insert(name.clone(), *ty);
    }

    // Merge array specs: prefer state_array_specs (full spec), fall back to state_arrays + elem type keys
    for (name, spec) in &st.state_array_specs {
        psf.data.insert(name.clone(), spec.clone());
    }
    for (name, size) in &st.state_arrays {
        if !psf.data.contains_key(name) {
            let elem_ty = declared_symbol_scalar_type(&st.declared_symbols, name)
                .unwrap_or(PrimitiveType::F32);
            psf.data.insert(
                name.clone(),
                onda_frontend::ArrayTypeSpec {
                    elem: ArrayElemType::Primitive(elem_ty),
                    size: Box::new(Expr::int(*size as i64)),
                },
            );
        }
    }

    // Nested procs and proc arrays
    psf.nested_procs = st.nested_procs.clone();
    psf.nested_proc_arrays = st.nested_proc_arrays.clone();

    // Struct instances with type_args
    for (name, struct_name) in &st.struct_instances {
        let type_args = st
            .struct_instance_type_args
            .get(name)
            .cloned()
            .unwrap_or_default();
        psf.struct_instances.insert(
            name.clone(),
            ProcStructState {
                struct_name: struct_name.clone(),
                type_args,
            },
        );
    }

    psf
}

pub(crate) fn is_declared_proc_symbol(
    name: &str,
    reserved: &HashSet<String>,
    locals: &HashSet<String>,
    local_aliases: &LocalAliasTypes,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    out: &ProcStateFields,
) -> bool {
    reserved.contains(name)
        || locals.contains(name)
        || local_aliases.contains_key(name)
        || local_array_aliases.contains_key(name)
        || out.has_any(name)
}

pub(crate) fn validate_proc_expr_decl_order(
    expr: &Expr,
    reserved: &HashSet<String>,
    locals: &HashSet<String>,
    local_aliases: &LocalAliasTypes,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    out: &ProcStateFields,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let mut ok = true;
    let expr_diag = DiagCtx::new(expr.loc());
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::Var { name, .. } => {
            if is_builtin_constant_name(name) {
                return true;
            }
            if let Some((base, _field)) = split_field_path(name, errors) {
                if !is_declared_proc_symbol(
                    base,
                    reserved,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    out,
                ) {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("symbol '{base}' used before declaration"),
                    );
                    ok = false;
                }
            } else if !is_declared_proc_symbol(
                name,
                reserved,
                locals,
                local_aliases,
                local_array_aliases,
                out,
            ) {
                push_semantic(
                    expr_diag,
                    errors,
                    format!("symbol '{name}' used before declaration"),
                );
                ok = false;
            }
        }
        Expr::Index { base, index, .. } => {
            if let Some((root, _field)) = split_field_path(base, errors) {
                if !is_declared_proc_symbol(
                    root,
                    reserved,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    out,
                ) {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("symbol '{root}' used before declaration"),
                    );
                    ok = false;
                }
            } else if !is_declared_proc_symbol(
                base,
                reserved,
                locals,
                local_aliases,
                local_array_aliases,
                out,
            ) {
                push_semantic(
                    expr_diag,
                    errors,
                    format!("symbol '{base}' used before declaration"),
                );
                ok = false;
            }
            ok &= validate_proc_expr_decl_order(
                index,
                reserved,
                locals,
                local_aliases,
                local_array_aliases,
                out,
                errors,
            );
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            if let Some((root, _field)) = split_field_path(base, errors) {
                if !is_declared_proc_symbol(
                    root,
                    reserved,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    out,
                ) {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("symbol '{root}' used before declaration"),
                    );
                    ok = false;
                }
            } else if !is_declared_proc_symbol(
                base,
                reserved,
                locals,
                local_aliases,
                local_array_aliases,
                out,
            ) {
                push_semantic(
                    expr_diag,
                    errors,
                    format!("symbol '{base}' used before declaration"),
                );
                ok = false;
            }
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                ok &= validate_proc_expr_decl_order(
                    coordinate,
                    reserved,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    out,
                    errors,
                );
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            ok &= validate_proc_expr_decl_order(
                &spec.size,
                reserved,
                locals,
                local_aliases,
                local_array_aliases,
                out,
                errors,
            );
            if let Some(values) = init {
                for value in values {
                    ok &= validate_proc_expr_decl_order(
                        value,
                        reserved,
                        locals,
                        local_aliases,
                        local_array_aliases,
                        out,
                        errors,
                    );
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            ok &= validate_proc_expr_decl_order(
                lhs,
                reserved,
                locals,
                local_aliases,
                local_array_aliases,
                out,
                errors,
            );
            ok &= validate_proc_expr_decl_order(
                rhs,
                reserved,
                locals,
                local_aliases,
                local_array_aliases,
                out,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                ok &= validate_proc_expr_decl_order(
                    arg,
                    reserved,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    out,
                    errors,
                );
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                ok &= validate_proc_expr_decl_order(
                    &arg.expr,
                    reserved,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    out,
                    errors,
                );
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            ok &= validate_proc_expr_decl_order(
                inner,
                reserved,
                locals,
                local_aliases,
                local_array_aliases,
                out,
                errors,
            );
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                ok &= validate_proc_expr_decl_order(
                    value,
                    reserved,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    out,
                    errors,
                );
            }
        }
    }
    ok
}

pub(crate) fn rewrite_proc_expr_symbols(
    expr: &mut Expr,
    owner_proc: &str,
    field_names: &HashSet<String>,
    field_array_slots: &HashMap<String, Vec<String>>,
    in_array_slots: &HashMap<String, Vec<String>>,
    errors: &mut Vec<Diagnostic>,
) {
    let expr_diag = DiagCtx::new(expr.loc());
    match expr {
        Expr::Var { name, .. } => {
            if field_names.contains(name) && is_plain_symbol(name) {
                *name = format!("self.{name}");
            } else if let Some((base, field)) = split_field_path(name, errors) {
                if field_names.contains(base) && is_plain_symbol(base) {
                    *name = format!("self.{base}.{field}");
                }
            }
        }
        Expr::Index { base, index, .. } => {
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
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("processor array field '{base}' has zero slots"),
                    );
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
                        *expr = Expr::var(format!("self.{slot_name}"));
                    }
                } else if slots.len() == 1 {
                    if let Some(slot_name) = slots.first() {
                        *expr = Expr::var(format!("self.{slot_name}"));
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
                            expr: Expr::var(format!("self.{slot}")),
                        });
                    }
                    *expr = Expr::UserCall {
                        loc: Default::default(),
                        name: proc_read_helper_name(owner_proc, slots.len(), false),
                        type_args: Vec::new(),
                        args,
                    };
                }
                return;
            }
            if let Some(slots) = in_array_slots.get(base.as_str()) {
                if slots.is_empty() {
                    push_semantic(
                        expr_diag,
                        errors,
                        format!("processor input array '{base}' has zero slots"),
                    );
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
                        *expr = Expr::var(slot_name.clone());
                    }
                } else if slots.len() == 1 {
                    if let Some(slot_name) = slots.first() {
                        *expr = Expr::var(slot_name.clone());
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
                            expr: Expr::var(slot.clone()),
                        });
                    }
                    *expr = Expr::UserCall {
                        loc: Default::default(),
                        name: proc_read_helper_name(owner_proc, slots.len(), false),
                        type_args: Vec::new(),
                        args,
                    };
                }
                return;
            }
            if field_names.contains(base) && is_plain_symbol(base) {
                *base = format!("self.{base}");
            } else if let Some((root, field)) = split_field_path(base, errors) {
                if field_names.contains(root) && is_plain_symbol(root) {
                    *base = format!("self.{root}.{field}");
                }
            }
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_proc_expr_symbols(
                    coordinate,
                    owner_proc,
                    field_names,
                    field_array_slots,
                    in_array_slots,
                    errors,
                );
            }
            if field_names.contains(base) && is_plain_symbol(base) {
                *base = format!("self.{base}");
            } else if let Some((root, field)) = split_field_path(base, errors) {
                if field_names.contains(root) && is_plain_symbol(root) {
                    *base = format!("self.{root}.{field}");
                }
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
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
            if let Expr::UserCall { name, .. } = expr {
                if let Some(base) = parse_array_len_instance_base(name) {
                    if field_names.contains(base) && is_plain_symbol(base) {
                        *name = format!("self.{base}.len");
                    } else if let Some((root, field)) = split_field_path(base, errors) {
                        if field_names.contains(root) && is_plain_symbol(root) {
                            *name = format!("self.{root}.{field}.len");
                        }
                    }
                } else if let Some(base) = parse_buffer_chans_instance_base(name) {
                    if field_names.contains(base) && is_plain_symbol(base) {
                        *name = format!("self.{base}.chans");
                    } else if let Some((root, field)) = split_field_path(base, errors) {
                        if field_names.contains(root) && is_plain_symbol(root) {
                            *name = format!("self.{root}.{field}.chans");
                        }
                    }
                } else if let Some(base) = parse_buffer_samplerate_instance_base(name) {
                    if field_names.contains(base) && is_plain_symbol(base) {
                        *name = format!("self.{base}.samplerate");
                    } else if let Some((root, field)) = split_field_path(base, errors) {
                        if field_names.contains(root) && is_plain_symbol(root) {
                            *name = format!("self.{root}.{field}.samplerate");
                        }
                    }
                }
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            rewrite_proc_expr_symbols(
                inner,
                owner_proc,
                field_names,
                field_array_slots,
                in_array_slots,
                errors,
            );
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
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
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

pub(crate) fn rewrite_proc_stmt_symbols(
    stmt: &Stmt,
    owner_proc: &str,
    field_names: &HashSet<String>,
    array_fields: &HashSet<String>,
    ins_names: &HashSet<String>,
    field_array_slots: &HashMap<String, Vec<String>>,
    in_array_slots: &HashMap<String, Vec<String>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Stmt> {
    with_stmt_diag_context(stmt, |diag| {
        let source_loc = stmt.loc().cloned();
        match stmt {
            Stmt::Const { .. } => None,
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
                            push_semantic(
                                diag,
                                errors,
                                format!("cannot assign to processor input '{name}'"),
                            );
                            return Some(Stmt::Assign {
                                loc: source_loc.into(),
                                target_loc: Default::default(),
                                target: AssignTarget::Var(name.clone()),
                                decl_ty: *decl_ty,
                                generic_decl_ty: generic_decl_ty.clone(),
                                is_typed_decl: *is_typed_decl,
                                typed_decl_ty_loc: Default::default(),
                                expr: expr_rewritten,
                            });
                        }
                        if field_names.contains(name) && is_plain_symbol(name) {
                            if matches!(expr, Expr::ArrayCtor { .. }) && array_fields.contains(name)
                            {
                                return None;
                            }
                            return Some(Stmt::Assign {
                                loc: source_loc.into(),
                                target_loc: Default::default(),
                                target: AssignTarget::Var(format!("self.{name}")),
                                decl_ty: None,
                                generic_decl_ty: None,
                                is_typed_decl: false,
                                typed_decl_ty_loc: Default::default(),
                                expr: expr_rewritten,
                            });
                        }
                        if let Some((base, field)) = split_field_path(name, errors) {
                            if field_names.contains(base) && is_plain_symbol(base) {
                                return Some(Stmt::Assign {
                                    loc: source_loc.into(),
                                    target_loc: Default::default(),
                                    target: AssignTarget::Var(format!("self.{base}.{field}")),
                                    decl_ty: None,
                                    generic_decl_ty: None,
                                    is_typed_decl: false,
                                    typed_decl_ty_loc: Default::default(),
                                    expr: expr_rewritten,
                                });
                            }
                        }
                        Some(Stmt::Assign {
                            loc: source_loc.into(),
                            target_loc: Default::default(),
                            target: AssignTarget::Var(name.clone()),
                            decl_ty: *decl_ty,
                            generic_decl_ty: generic_decl_ty.clone(),
                            is_typed_decl: *is_typed_decl,
                            typed_decl_ty_loc: Default::default(),
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
                            push_semantic(
                                diag,
                                errors,
                                format!("cannot assign to processor input '{base}'"),
                            );
                            if let Some(raw_idx) = try_constant_index_i64(&idx_rewritten) {
                                if let Some(slot_idx) = resolve_proc_constant_slot_index(
                                    raw_idx,
                                    slots.len(),
                                    &format!("processor input array assignment '{base}[...]'"),
                                    errors,
                                ) {
                                    if let Some(slot_name) = slots.get(slot_idx) {
                                        return Some(Stmt::Assign {
                                            loc: Default::default(),
                                            target_loc: Default::default(),
                                            target: AssignTarget::Var(slot_name.clone()),
                                            decl_ty: *decl_ty,
                                            generic_decl_ty: generic_decl_ty.clone(),
                                            is_typed_decl: *is_typed_decl,
                                            typed_decl_ty_loc: Default::default(),
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
                                        loc: source_loc.into(),
                                        target_loc: Default::default(),
                                        target: AssignTarget::Index {
                                            base: base.clone(),
                                            index: idx_rewritten,
                                        },
                                        decl_ty: *decl_ty,
                                        generic_decl_ty: generic_decl_ty.clone(),
                                        is_typed_decl: *is_typed_decl,
                                        typed_decl_ty_loc: Default::default(),
                                        expr: expr_rewritten,
                                    });
                                };
                                if let Some(slot_name) = slots.get(slot_idx) {
                                    return Some(Stmt::Assign {
                                        loc: source_loc.into(),
                                        target_loc: Default::default(),
                                        target: AssignTarget::Var(format!("self.{slot_name}")),
                                        decl_ty: None,
                                        generic_decl_ty: None,
                                        is_typed_decl: false,
                                        typed_decl_ty_loc: Default::default(),
                                        expr: expr_rewritten,
                                    });
                                }
                            } else {
                                return Some(Stmt::Expr {
                                    loc: source_loc.into(),
                                    expr: Expr::UserCall {
                                        loc: Default::default(),
                                        name: proc_write_helper_name(owner_proc, slots, false),
                                        type_args: Vec::new(),
                                        args: vec![
                                            CallArg {
                                                name: None,
                                                expr: Expr::var("self"),
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
                        } else if let Some((root, field)) = split_field_path(base, errors) {
                            if field_names.contains(root) && is_plain_symbol(root) {
                                format!("self.{root}.{field}")
                            } else {
                                base.clone()
                            }
                        } else {
                            base.clone()
                        };
                        Some(Stmt::Assign {
                            loc: source_loc.into(),
                            target_loc: Default::default(),
                            target: AssignTarget::Index {
                                base: target_base,
                                index: idx_rewritten,
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            typed_decl_ty_loc: Default::default(),
                            expr: expr_rewritten,
                        })
                    }
                    AssignTarget::Slice {
                        base,
                        selector,
                        channel,
                        start,
                        end,
                    } => {
                        let mut selector_rewritten = selector.clone();
                        let mut channel_rewritten = channel.clone();
                        let mut start_rewritten = start.clone();
                        let mut end_rewritten = end.clone();
                        for coordinate in [
                            selector_rewritten.as_mut(),
                            channel_rewritten.as_mut(),
                            start_rewritten.as_mut(),
                            end_rewritten.as_mut(),
                        ]
                        .into_iter()
                        .flatten()
                        {
                            rewrite_proc_expr_symbols(
                                coordinate,
                                owner_proc,
                                field_names,
                                field_array_slots,
                                in_array_slots,
                                errors,
                            );
                        }
                        if in_array_slots.contains_key(base) {
                            push_semantic(
                                diag,
                                errors,
                                format!("cannot assign to processor input '{base}'"),
                            );
                        }
                        let target_base = if field_names.contains(base) && is_plain_symbol(base) {
                            format!("self.{base}")
                        } else if let Some((root, field)) = split_field_path(base, errors) {
                            if field_names.contains(root) && is_plain_symbol(root) {
                                format!("self.{root}.{field}")
                            } else {
                                base.clone()
                            }
                        } else {
                            base.clone()
                        };
                        Some(Stmt::Assign {
                            loc: source_loc.into(),
                            target_loc: Default::default(),
                            target: AssignTarget::Slice {
                                base: target_base,
                                selector: selector_rewritten,
                                channel: channel_rewritten,
                                start: start_rewritten,
                                end: end_rewritten,
                            },
                            decl_ty: None,
                            generic_decl_ty: None,
                            is_typed_decl: false,
                            typed_decl_ty_loc: Default::default(),
                            expr: expr_rewritten,
                        })
                    }
                    AssignTarget::Tuple(_) => Some(Stmt::Assign {
                        loc: source_loc.into(),
                        target_loc: Default::default(),
                        target: target.clone(),
                        decl_ty: *decl_ty,
                        generic_decl_ty: generic_decl_ty.clone(),
                        is_typed_decl: *is_typed_decl,
                        typed_decl_ty_loc: Default::default(),
                        expr: expr_rewritten,
                    }),
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
                Some(Stmt::Expr {
                    loc: source_loc.into(),
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
                    loc: source_loc.into(),
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
                            array_fields,
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
                            array_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(Stmt::If {
                    loc: source_loc.into(),
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
                            array_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(Stmt::For {
                    loc: source_loc.into(),
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
                            array_fields,
                            ins_names,
                            field_array_slots,
                            in_array_slots,
                            errors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some(Stmt::While {
                    loc: source_loc.into(),
                    cond: cond_rewritten,
                    body,
                })
            }
            Stmt::Break { .. } => Some(Stmt::Break {
                loc: source_loc.into(),
            }),
            Stmt::Continue { .. } => Some(Stmt::Continue {
                loc: source_loc.into(),
            }),
        }
    })
}
