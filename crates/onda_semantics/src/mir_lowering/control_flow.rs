use super::*;
use crate::def_semantics::call_types::StatementFlow;
use crate::is_bare_return_expr;

#[derive(Clone, Copy)]
enum StaticForPlan {
    Empty,
    NonEmpty {
        min: ScalarValue,
        max: ScalarValue,
        last: ScalarValue,
    },
}

impl<'a> FunctionLowerer<'a> {
    pub(super) fn lower_statements(
        &mut self,
        statements: &[Stmt],
        block: &mut MirBlock,
        continue_mode: ContinueMode,
    ) -> Result<StatementFlow, MirLoweringError> {
        for statement in statements {
            let flow = match statement {
                Stmt::Const { .. } => {
                    return Err(self.error(
                        "runtime local const declaration survived semantic constant folding",
                        statement.loc(),
                    ));
                }
                Stmt::Assign {
                    target,
                    decl_ty,
                    expr,
                    loc,
                    ..
                } => {
                    let declared_integer_range = integer_range_invariant(expr, *decl_ty);
                    if let AssignTarget::Var(name) = target {
                        if self.is_slice_expression(expr) {
                            let slice = self.lower_slice_expression(expr, None, block)?;
                            self.assign_slice_alias(name, slice, block, (*loc).into())?;
                            continue;
                        }
                        if self.lower_buffer_alias_assignment(name, expr, block, (*loc).into())? {
                            continue;
                        }
                    }
                    if let AssignTarget::Var(name) = target {
                        if self.lower_struct_array_element_alias(
                            name,
                            expr,
                            block,
                            (*loc).into(),
                        )? {
                            continue;
                        }
                        if self.lower_struct_array_state_initializer(
                            name,
                            expr,
                            block,
                            (*loc).into(),
                        )? {
                            continue;
                        }
                        if self.lower_struct_state_initializer(name, expr, block, (*loc).into())? {
                            continue;
                        }
                        if self.lower_state_array_initializer(name, expr, block, (*loc).into())? {
                            continue;
                        }
                        if self.lower_local_array_declaration(name, expr, block, (*loc).into())? {
                            continue;
                        }
                    }
                    if let AssignTarget::Slice {
                        base,
                        selector,
                        channel,
                        start,
                        end,
                    } = target
                    {
                        self.lower_slice_assignment(
                            base,
                            SliceSelection {
                                selector: selector.as_deref(),
                                channel: channel.as_deref(),
                                start: start.as_deref(),
                                end: end.as_deref(),
                            },
                            expr,
                            block,
                            (*loc).into(),
                        )?;
                        continue;
                    }
                    let values = self.lower_value_expr(expr, block)?;
                    match target {
                        AssignTarget::Var(name) => {
                            if !self.assign_runtime_global(
                                name,
                                &values,
                                block,
                                expr.loc(),
                                (*loc).into(),
                            )? {
                                self.assign_variable_values(
                                    name,
                                    values,
                                    *decl_ty,
                                    expr,
                                    block,
                                    (*loc).into(),
                                )?;
                                if let Some(range) = declared_integer_range {
                                    if let Some(Binding::Local(local, _)) =
                                        self.bindings.get(name).cloned()
                                    {
                                        self.locals[local.index()].integer_range = Some(range);
                                    }
                                }
                            }
                        }
                        AssignTarget::Tuple(targets) => self.assign_destructured_values(
                            targets,
                            values,
                            block,
                            expr.loc(),
                            (*loc).into(),
                        )?,
                        AssignTarget::Index { base, index } => self.assign_index_target(
                            base,
                            index,
                            &values,
                            block,
                            expr.loc(),
                            (*loc).into(),
                        )?,
                        AssignTarget::Slice { .. } => {
                            unreachable!("slice assignments are lowered before scalar/tuple values")
                        }
                    }
                    StatementFlow::Continues
                }
                Stmt::Expr { expr, .. } => {
                    if crate::task_lowering::is_task_abort_expr(expr) {
                        self.push_statement(block, StatementKind::Break, expr.loc());
                        return Ok(StatementFlow::Terminates);
                    }
                    if let Expr::UserCall {
                        loc,
                        name,
                        type_args,
                        args,
                    } = expr
                    {
                        if !self.lower_buffer_write_call(name, args, (*loc).into(), block)?
                            && self
                                .lower_buffer_read_call(name, args, (*loc).into(), block)?
                                .is_none()
                            && self
                                .lower_buffer_metadata_call(name, args, (*loc).into(), block)?
                                .is_none()
                        {
                            let _ =
                                self.lower_user_call(name, type_args, args, (*loc).into(), block)?;
                        }
                    } else {
                        let _ = self.lower_expr(expr, block)?;
                    }
                    StatementFlow::Continues
                }
                Stmt::Return { expr, loc } => {
                    if is_bare_return_expr(expr) {
                        if self.function.returns_value {
                            return Err(self.error(
                                "bare return found in a value-returning function",
                                (*loc).into(),
                            ));
                        }
                        self.push_statement(
                            block,
                            StatementKind::Return { values: Vec::new() },
                            (*loc).into(),
                        );
                        return Ok(StatementFlow::Terminates);
                    }
                    if !self.function.returns_value {
                        return Err(self.error(
                            "value return found in a function marked as no-result",
                            (*loc).into(),
                        ));
                    }
                    let result_types = match &self.function.return_ty {
                        ReturnType::Scalar(result) => vec![*result],
                        ReturnType::Tuple(results) => results.clone(),
                    };
                    let values = self.lower_value_expr(expr, block)?;
                    if values.len() != result_types.len() {
                        return Err(self.error(
                            format!(
                                "return arity mismatch after semantic analysis: expected {}, got {}",
                                result_types.len(),
                                values.len()
                            ),
                            (*loc).into(),
                        ));
                    }
                    let mut returned = Vec::with_capacity(values.len());
                    for (value, result_ty) in values.into_iter().zip(result_types) {
                        returned.push(self.coerce(value, result_ty, block, expr.loc())?.value);
                    }
                    self.push_statement(
                        block,
                        StatementKind::Return { values: returned },
                        (*loc).into(),
                    );
                    StatementFlow::Terminates
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    loc,
                } => {
                    let condition = self.lower_expr(cond, block)?;
                    let condition =
                        self.coerce(condition, PrimitiveType::Bool, block, cond.loc())?;
                    let outer_bindings = self.bindings.clone();
                    let outer_nested_proc_aliases = self.nested_proc_aliases.clone();
                    let mut then_block = MirBlock::default();
                    let then_flow =
                        self.lower_statements(then_branch, &mut then_block, continue_mode)?;
                    let then_bindings = self.bindings.clone();
                    let then_nested_proc_aliases = self.nested_proc_aliases.clone();
                    self.bindings = outer_bindings.clone();
                    self.nested_proc_aliases = outer_nested_proc_aliases.clone();
                    let mut else_block = MirBlock::default();
                    let else_flow =
                        self.lower_statements(else_branch, &mut else_block, continue_mode)?;
                    let else_bindings = self.bindings.clone();
                    let else_nested_proc_aliases = self.nested_proc_aliases.clone();
                    match (then_flow, else_flow) {
                        (StatementFlow::Continues, StatementFlow::Terminates) => {
                            self.bindings = then_bindings;
                            self.nested_proc_aliases = then_nested_proc_aliases;
                        }
                        (StatementFlow::Terminates, StatementFlow::Continues) => {
                            self.bindings = else_bindings;
                            self.nested_proc_aliases = else_nested_proc_aliases;
                        }
                        (StatementFlow::Continues, StatementFlow::Continues) => {
                            self.merge_branch_scopes(
                                outer_bindings,
                                then_bindings,
                                else_bindings,
                                outer_nested_proc_aliases,
                                then_nested_proc_aliases,
                                else_nested_proc_aliases,
                                &mut then_block,
                                &mut else_block,
                                (*loc).into(),
                            )?;
                        }
                        (StatementFlow::Terminates, StatementFlow::Terminates) => {
                            self.bindings = outer_bindings;
                            self.nested_proc_aliases = outer_nested_proc_aliases;
                        }
                    }
                    self.push_statement(
                        block,
                        StatementKind::If {
                            condition: condition.value,
                            then_block,
                            else_block,
                        },
                        (*loc).into(),
                    );
                    if then_flow == StatementFlow::Terminates
                        && else_flow == StatementFlow::Terminates
                    {
                        StatementFlow::Terminates
                    } else {
                        StatementFlow::Continues
                    }
                }
                Stmt::While { cond, body, loc } => {
                    let outer_bindings = self.bindings.clone();
                    let outer_nested_proc_aliases = self.nested_proc_aliases.clone();
                    let mut loop_body = MirBlock::default();
                    let condition = self.lower_expr(cond, &mut loop_body)?;
                    let condition =
                        self.coerce(condition, PrimitiveType::Bool, &mut loop_body, cond.loc())?;
                    let mut then_block = MirBlock::default();
                    self.lower_statements(body, &mut then_block, ContinueMode::Plain)?;
                    self.bindings = outer_bindings;
                    self.nested_proc_aliases = outer_nested_proc_aliases;
                    let mut else_block = MirBlock::default();
                    self.push_statement(&mut else_block, StatementKind::Break, (*loc).into());
                    self.push_statement(
                        &mut loop_body,
                        StatementKind::If {
                            condition: condition.value,
                            then_block,
                            else_block,
                        },
                        (*loc).into(),
                    );
                    self.push_statement(
                        block,
                        StatementKind::Loop { body: loop_body },
                        (*loc).into(),
                    );
                    StatementFlow::Continues
                }
                Stmt::For {
                    var,
                    var_ty,
                    step,
                    start,
                    end,
                    end_inclusive,
                    body,
                    loc,
                } => {
                    self.lower_for(
                        var,
                        *var_ty,
                        step.as_ref(),
                        start,
                        end,
                        *end_inclusive,
                        body,
                        (*loc).into(),
                        block,
                    )?;
                    StatementFlow::Continues
                }
                Stmt::Break { loc } => {
                    if matches!(continue_mode, ContinueMode::None) {
                        return Err(self.error("break reached MIR outside a loop", (*loc).into()));
                    }
                    self.push_statement(block, StatementKind::Break, (*loc).into());
                    StatementFlow::Terminates
                }
                Stmt::Continue { loc } => {
                    match continue_mode {
                        ContinueMode::None => {
                            return Err(
                                self.error("continue reached MIR outside a loop", (*loc).into())
                            );
                        }
                        ContinueMode::Plain => {}
                        ContinueMode::For {
                            index,
                            step,
                            last,
                            source,
                        } => self.emit_for_latch(block, index, step, last, source),
                    }
                    self.push_statement(block, StatementKind::Continue, (*loc).into());
                    StatementFlow::Terminates
                }
            };
            if flow == StatementFlow::Terminates {
                return Ok(flow);
            }
        }
        Ok(StatementFlow::Continues)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_for(
        &mut self,
        variable: &str,
        induction_ty: PrimitiveType,
        step: Option<&Expr>,
        start: &Expr,
        end: &Expr,
        end_inclusive: bool,
        body: &[Stmt],
        location: SourceLoc,
        destination: &mut MirBlock,
    ) -> Result<(), MirLoweringError> {
        let start_value = self.lower_expr(start, destination)?;
        let start_value = self.coerce(start_value, induction_ty, destination, start.loc())?;

        let end_value = self.lower_expr(end, destination)?;
        let end_value = self.coerce(end_value, induction_ty, destination, end.loc())?;

        let unit_step = match induction_ty {
            PrimitiveType::I32 => ScalarValue::I32(1),
            PrimitiveType::I64 => ScalarValue::I64(1),
            _ => unreachable!("for induction types are restricted to integers"),
        };
        let step_value = if let Some(step) = step {
            let value = self.lower_expr(step, destination)?;
            self.coerce(value, induction_ty, destination, step.loc())?
        } else {
            LoweredValue {
                value: Value::Constant(unit_step),
                ty: induction_ty,
            }
        };
        let forward_unit_step = step_value.value == Value::Constant(unit_step);

        let static_plan = static_for_plan(
            start_value.value,
            end_value.value,
            step_value.value,
            end_inclusive,
        );
        if matches!(static_plan, Some(StaticForPlan::Empty)) {
            return Ok(());
        }
        let (start_value, end_value, step_value, last, body_range) = match static_plan {
            Some(StaticForPlan::NonEmpty { min, max, last }) => (
                start_value,
                end_value,
                step_value,
                Some(Value::Constant(last)),
                Some(onda_mir::IntegerRangeInvariant {
                    min,
                    max,
                    mode: onda_mir::IntegerRangeMode::Clamp,
                }),
            ),
            None => {
                let start_value = self.snapshot(start_value, destination, start.loc());
                let end_value = self.snapshot(end_value, destination, end.loc());
                let step_location = step.map_or(location, Expr::loc);
                let step_value = self.snapshot(step_value, destination, step_location);
                (start_value, end_value, step_value, None, None)
            }
            Some(StaticForPlan::Empty) => unreachable!("empty loops return above"),
        };

        let outer_bindings = self.bindings.clone();
        let outer_nested_proc_aliases = self.nested_proc_aliases.clone();
        // The source-language loop variable and its induction counter retain
        // the selected integer width end to end. Constant loops stop at their
        // statically computed final iteration; dynamic loops retain ordinary
        // integer arithmetic without a hidden widening/narrowing path.
        let index = self.new_local(Some(format!("{variable}.$induction")), induction_ty);
        self.push_statement(
            destination,
            StatementKind::Assign {
                destination: Place::local(index),
                value: Rvalue::Use(start_value.value),
            },
            location,
        );

        let mut loop_body = MirBlock::default();
        if last.is_none() {
            self.emit_for_guard(
                &mut loop_body,
                index,
                end_value.value,
                step_value.value,
                end_inclusive,
                location,
            );
        }
        let body_index = self.new_local(
            Some(if forward_unit_step {
                format!("{variable}.$forward_body")
            } else {
                format!("{variable}.$body")
            }),
            induction_ty,
        );
        self.locals[body_index.index()].integer_range = body_range;
        self.push_statement(
            &mut loop_body,
            StatementKind::Assign {
                destination: Place::local(body_index),
                value: Rvalue::Use(Value::Local(index)),
            },
            location,
        );
        self.bindings.insert(
            variable.to_owned(),
            Binding::Local(body_index, induction_ty),
        );
        let body_flow = self.lower_statements(
            body,
            &mut loop_body,
            ContinueMode::For {
                index,
                step: step_value.value,
                last,
                source: location,
            },
        )?;
        if body_flow == StatementFlow::Continues {
            self.emit_for_latch(&mut loop_body, index, step_value.value, last, location);
        }

        self.bindings = outer_bindings;
        self.nested_proc_aliases = outer_nested_proc_aliases;
        self.push_statement(
            destination,
            StatementKind::Loop { body: loop_body },
            location,
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn merge_branch_scopes(
        &mut self,
        outer_bindings: HashMap<String, Binding>,
        then_bindings: HashMap<String, Binding>,
        else_bindings: HashMap<String, Binding>,
        outer_nested_proc_aliases: HashMap<String, NestedProcElementAlias>,
        then_nested_proc_aliases: HashMap<String, NestedProcElementAlias>,
        else_nested_proc_aliases: HashMap<String, NestedProcElementAlias>,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let mut merged_binding_names = then_bindings
            .keys()
            .filter(|name| !outer_bindings.contains_key(*name) && else_bindings.contains_key(*name))
            .cloned()
            .collect::<Vec<_>>();
        merged_binding_names.sort();

        self.bindings = outer_bindings;
        for name in merged_binding_names {
            let Some(then_binding) = then_bindings.get(&name).cloned() else {
                continue;
            };
            let Some(else_binding) = else_bindings.get(&name).cloned() else {
                continue;
            };
            if let Some(binding) = self.reconcile_branch_binding(
                &name,
                then_binding,
                else_binding,
                then_block,
                else_block,
                location,
            )? {
                self.bindings.insert(name, binding);
            }
        }

        let mut merged_alias_names = then_nested_proc_aliases
            .keys()
            .filter(|name| {
                !outer_nested_proc_aliases.contains_key(*name)
                    && else_nested_proc_aliases.contains_key(*name)
            })
            .cloned()
            .collect::<Vec<_>>();
        merged_alias_names.sort();

        self.nested_proc_aliases = outer_nested_proc_aliases;
        for name in merged_alias_names {
            let Some(then_alias) = then_nested_proc_aliases.get(&name).cloned() else {
                continue;
            };
            let Some(else_alias) = else_nested_proc_aliases.get(&name).cloned() else {
                continue;
            };
            if then_alias.struct_name == else_alias.struct_name
                && then_alias.alternatives == else_alias.alternatives
                && self.local_types_match(then_alias.index, else_alias.index)
            {
                self.copy_branch_local(else_block, then_alias.index, else_alias.index, location);
                self.nested_proc_aliases.insert(name, then_alias);
            }
        }
        Ok(())
    }

    pub(super) fn reconcile_branch_binding(
        &mut self,
        name: &str,
        then_binding: Binding,
        else_binding: Binding,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Option<Binding>, MirLoweringError> {
        match (then_binding, else_binding) {
            (Binding::Local(then_local, then_ty), Binding::Local(else_local, else_ty)) => self
                .reconcile_branch_scalar(
                    name, then_local, then_ty, else_local, else_ty, then_block, else_block,
                    location,
                )
                .map(|binding| binding.map(|(local, ty)| Binding::Local(local, ty))),
            (
                Binding::Array(then_local, then_element, then_len),
                Binding::Array(else_local, else_element, else_len),
            ) if then_element == else_element
                && then_len == else_len
                && self.local_types_match(then_local, else_local) =>
            {
                self.copy_branch_local(else_block, then_local, else_local, location);
                Ok(Some(Binding::Array(then_local, then_element, then_len)))
            }
            (
                Binding::Slice(then_local, then_element, then_access),
                Binding::Slice(else_local, else_element, else_access),
            ) if then_element == else_element
                && then_access == else_access
                && self.local_types_match(then_local, else_local) =>
            {
                self.copy_branch_local(else_block, then_local, else_local, location);
                Ok(Some(Binding::Slice(then_local, then_element, then_access)))
            }
            (Binding::Tuple(then_values), Binding::Tuple(else_values))
                if then_values.len() == else_values.len()
                    && then_values.iter().zip(&else_values).all(
                        |((_, then_ty), (_, else_ty))| {
                            merge_inferred_return_types(*then_ty, *else_ty).is_some()
                        },
                    ) =>
            {
                let mut joined = Vec::with_capacity(then_values.len());
                for (index, ((then_local, then_ty), (else_local, else_ty))) in
                    then_values.into_iter().zip(else_values).enumerate()
                {
                    let component_name = format!("{name}.{index}");
                    let Some(component) = self.reconcile_branch_scalar(
                        &component_name,
                        then_local,
                        then_ty,
                        else_local,
                        else_ty,
                        then_block,
                        else_block,
                        location,
                    )?
                    else {
                        return Ok(None);
                    };
                    joined.push(component);
                }
                Ok(Some(Binding::Tuple(joined)))
            }
            (
                Binding::TupleSliceElementAlias(then_values),
                Binding::TupleSliceElementAlias(else_values),
            ) if then_values.len() == else_values.len()
                && then_values.iter().zip(&else_values).all(
                    |((then_slice, then_ty, then_index), (else_slice, else_ty, else_index))| {
                        then_ty == else_ty
                            && self.local_types_match(*then_slice, *else_slice)
                            && self.local_types_match(*then_index, *else_index)
                    },
                ) =>
            {
                for ((then_slice, _, then_index), (else_slice, _, else_index)) in
                    then_values.iter().zip(&else_values)
                {
                    self.copy_branch_local(else_block, *then_slice, *else_slice, location);
                    self.copy_branch_local(else_block, *then_index, *else_index, location);
                }
                Ok(Some(Binding::TupleSliceElementAlias(then_values)))
            }
            (
                Binding::SliceElementAlias {
                    slice: then_slice,
                    element: then_element,
                    index: then_index,
                },
                Binding::SliceElementAlias {
                    slice: else_slice,
                    element: else_element,
                    index: else_index,
                },
            ) if then_element == else_element
                && self.local_types_match(then_slice, else_slice)
                && self.local_types_match(then_index, else_index) =>
            {
                self.copy_branch_local(else_block, then_slice, else_slice, location);
                self.copy_branch_local(else_block, then_index, else_index, location);
                Ok(Some(Binding::SliceElementAlias {
                    slice: then_slice,
                    element: then_element,
                    index: then_index,
                }))
            }
            (
                Binding::StructArrayElementAlias {
                    struct_name: then_struct,
                },
                Binding::StructArrayElementAlias {
                    struct_name: else_struct,
                },
            ) if then_struct == else_struct => Ok(Some(Binding::StructArrayElementAlias {
                struct_name: then_struct,
            })),
            (
                Binding::StructArrayParameter {
                    struct_name: then_struct,
                    length: StructArrayLength::Fixed(then_len),
                    fields: then_fields,
                },
                Binding::StructArrayParameter {
                    struct_name: else_struct,
                    length: StructArrayLength::Fixed(else_len),
                    fields: else_fields,
                },
            ) if then_struct == else_struct
                && then_len == else_len
                && then_fields.len() == else_fields.len()
                && then_fields.iter().zip(&else_fields).all(
                    |((then_name, then_local, then_ty), (else_name, else_local, else_ty))| {
                        then_name == else_name
                            && then_ty == else_ty
                            && self.local_types_match(*then_local, *else_local)
                    },
                ) =>
            {
                for ((_, then_local, _), (_, else_local, _)) in then_fields.iter().zip(&else_fields)
                {
                    self.copy_branch_local(else_block, *then_local, *else_local, location);
                }
                Ok(Some(Binding::StructArrayParameter {
                    struct_name: then_struct,
                    length: StructArrayLength::Fixed(then_len),
                    fields: then_fields,
                }))
            }
            _ => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn reconcile_branch_scalar(
        &mut self,
        name: &str,
        then_local: LocalId,
        then_ty: PrimitiveType,
        else_local: LocalId,
        else_ty: PrimitiveType,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Option<(LocalId, PrimitiveType)>, MirLoweringError> {
        if self.locals[then_local.index()].integer_range
            != self.locals[else_local.index()].integer_range
        {
            return Err(self.error(
                format!(
                    "binding '{name}' reached MIR lowering with incompatible branch integer range contracts"
                ),
                location,
            ));
        }
        if then_ty == else_ty && self.local_types_match(then_local, else_local) {
            self.copy_branch_local(else_block, then_local, else_local, location);
            return Ok(Some((then_local, then_ty)));
        }
        let Some(joined_ty) = merge_inferred_return_types(then_ty, else_ty) else {
            return Ok(None);
        };
        let joined_local = self.new_local(Some(name.to_owned()), joined_ty);
        let then_value = self.coerce(
            LoweredValue {
                value: Value::Local(then_local),
                ty: then_ty,
            },
            joined_ty,
            then_block,
            location,
        )?;
        self.assign_value(then_block, joined_local, then_value.value, location);
        let else_value = self.coerce(
            LoweredValue {
                value: Value::Local(else_local),
                ty: else_ty,
            },
            joined_ty,
            else_block,
            location,
        )?;
        self.assign_value(else_block, joined_local, else_value.value, location);
        Ok(Some((joined_local, joined_ty)))
    }

    pub(super) fn local_types_match(&self, lhs: LocalId, rhs: LocalId) -> bool {
        self.locals.get(lhs.index()).map(|local| local.ty)
            == self.locals.get(rhs.index()).map(|local| local.ty)
    }

    pub(super) fn copy_branch_local(
        &mut self,
        block: &mut MirBlock,
        destination: LocalId,
        source: LocalId,
        location: SourceLoc,
    ) {
        if destination != source {
            self.assign_value(block, destination, Value::Local(source), location);
        }
    }

    pub(super) fn emit_for_guard(
        &mut self,
        block: &mut MirBlock,
        index: LocalId,
        end: Value,
        step: Value,
        inclusive: bool,
        location: SourceLoc,
    ) {
        let zero = Value::Constant(match self.types[self.locals[index.index()].ty.index()] {
            MirType::Scalar(ScalarType::I32) => ScalarValue::I32(0),
            MirType::Scalar(ScalarType::I64) => ScalarValue::I64(0),
            _ => unreachable!("for induction locals are restricted to integers"),
        });
        let step_positive = self.compare_value(block, CompareOp::Greater, step, zero, location);

        let mut positive = MirBlock::default();
        let positive_bound = self.compare_value(
            &mut positive,
            if inclusive {
                CompareOp::LessEqual
            } else {
                CompareOp::Less
            },
            Value::Local(index),
            end,
            location,
        );
        self.push_guard_or_break(&mut positive, positive_bound, location);

        let mut non_positive = MirBlock::default();
        let step_negative =
            self.compare_value(&mut non_positive, CompareOp::Less, step, zero, location);
        let mut negative = MirBlock::default();
        let negative_bound = self.compare_value(
            &mut negative,
            if inclusive {
                CompareOp::GreaterEqual
            } else {
                CompareOp::Greater
            },
            Value::Local(index),
            end,
            location,
        );
        self.push_guard_or_break(&mut negative, negative_bound, location);
        let mut zero_step = MirBlock::default();
        self.push_statement(&mut zero_step, StatementKind::Break, location);
        self.push_statement(
            &mut non_positive,
            StatementKind::If {
                condition: step_negative,
                then_block: negative,
                else_block: zero_step,
            },
            location,
        );

        self.push_statement(
            block,
            StatementKind::If {
                condition: step_positive,
                then_block: positive,
                else_block: non_positive,
            },
            location,
        );
    }

    pub(super) fn push_guard_or_break(
        &mut self,
        block: &mut MirBlock,
        condition: Value,
        location: SourceLoc,
    ) {
        let mut failed = MirBlock::default();
        self.push_statement(&mut failed, StatementKind::Break, location);
        self.push_statement(
            block,
            StatementKind::If {
                condition,
                then_block: MirBlock::default(),
                else_block: failed,
            },
            location,
        );
    }

    pub(super) fn emit_for_latch(
        &mut self,
        block: &mut MirBlock,
        index: LocalId,
        step: Value,
        last: Option<Value>,
        location: SourceLoc,
    ) {
        let mut increment = MirBlock::default();
        self.push_statement(
            &mut increment,
            StatementKind::Assign {
                destination: Place::local(index),
                value: Rvalue::Binary {
                    op: MirBinaryOp::Add,
                    lhs: Value::Local(index),
                    rhs: step,
                },
            },
            location,
        );
        let Some(last) = last else {
            block.statements.extend(increment.statements);
            return;
        };
        let at_last =
            self.compare_value(block, CompareOp::Equal, Value::Local(index), last, location);
        let mut finished = MirBlock::default();
        self.push_statement(&mut finished, StatementKind::Break, location);
        self.push_statement(
            block,
            StatementKind::If {
                condition: at_last,
                then_block: finished,
                else_block: increment,
            },
            location,
        );
    }
}

fn static_for_plan(
    start: Value,
    end: Value,
    step: Value,
    inclusive: bool,
) -> Option<StaticForPlan> {
    let (ty, start, end, step) = match (start, end, step) {
        (
            Value::Constant(ScalarValue::I32(start)),
            Value::Constant(ScalarValue::I32(end)),
            Value::Constant(ScalarValue::I32(step)),
        ) => (
            PrimitiveType::I32,
            i128::from(start),
            i128::from(end),
            i128::from(step),
        ),
        (
            Value::Constant(ScalarValue::I64(start)),
            Value::Constant(ScalarValue::I64(end)),
            Value::Constant(ScalarValue::I64(step)),
        ) => (
            PrimitiveType::I64,
            i128::from(start),
            i128::from(end),
            i128::from(step),
        ),
        _ => return None,
    };
    if step == 0 {
        return None;
    }

    let last = if step > 0 {
        let upper = if inclusive { end } else { end - 1 };
        if start > upper {
            return Some(StaticForPlan::Empty);
        }
        start + ((upper - start) / step) * step
    } else {
        let lower = if inclusive { end } else { end + 1 };
        if start < lower {
            return Some(StaticForPlan::Empty);
        }
        start - ((start - lower) / -step) * -step
    };
    let scalar = |value| match ty {
        PrimitiveType::I32 => ScalarValue::I32(
            i32::try_from(value).expect("an i32-bounded loop has an i32 iteration value"),
        ),
        PrimitiveType::I64 => ScalarValue::I64(
            i64::try_from(value).expect("an i64-bounded loop has an i64 iteration value"),
        ),
        _ => unreachable!("static for plans are restricted to integers"),
    };
    Some(StaticForPlan::NonEmpty {
        min: scalar(start.min(last)),
        max: scalar(start.max(last)),
        last: scalar(last),
    })
}
