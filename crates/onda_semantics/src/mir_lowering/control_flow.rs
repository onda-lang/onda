use super::*;

impl<'a> FunctionLowerer<'a> {
    pub(super) fn lower_statements(
        &mut self,
        statements: &[Stmt],
        block: &mut MirBlock,
        continue_mode: ContinueMode,
    ) -> Result<(), MirLoweringError> {
        for statement in statements {
            match statement {
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
                    if let AssignTarget::Var(name) = target {
                        if self.is_slice_expression(expr) {
                            let slice = self.lower_slice_expression(expr, None, block)?;
                            self.assign_slice_alias(name, slice, block, (*loc).into())?;
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
                    if let AssignTarget::Slice { base, start, end } = target {
                        self.lower_slice_assignment(
                            base,
                            start.as_ref(),
                            end.as_ref(),
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
                }
                Stmt::Expr { expr, .. } => {
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
                }
                Stmt::Return { expr, loc } => {
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
                    self.stop_prezeroed_init_state_proof();
                    let outer_bindings = self.bindings.clone();
                    let outer_nested_proc_aliases = self.nested_proc_aliases.clone();
                    let mut then_block = MirBlock::default();
                    self.lower_statements(then_branch, &mut then_block, continue_mode)?;
                    let then_bindings = self.bindings.clone();
                    let then_nested_proc_aliases = self.nested_proc_aliases.clone();
                    self.bindings = outer_bindings.clone();
                    self.nested_proc_aliases = outer_nested_proc_aliases.clone();
                    let mut else_block = MirBlock::default();
                    self.lower_statements(else_branch, &mut else_block, continue_mode)?;
                    let else_bindings = self.bindings.clone();
                    let else_nested_proc_aliases = self.nested_proc_aliases.clone();
                    self.merge_branch_scopes(
                        outer_bindings,
                        then_bindings,
                        else_bindings,
                        outer_nested_proc_aliases,
                        then_nested_proc_aliases,
                        else_nested_proc_aliases,
                        &mut else_block,
                        (*loc).into(),
                    );
                    self.push_statement(
                        block,
                        StatementKind::If {
                            condition: condition.value,
                            then_block,
                            else_block,
                        },
                        (*loc).into(),
                    );
                }
                Stmt::While { cond, body, loc } => {
                    self.stop_prezeroed_init_state_proof();
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
                }
                Stmt::For {
                    var,
                    step,
                    start,
                    end,
                    end_inclusive,
                    body,
                    loc,
                } => self.lower_for(
                    var,
                    step.as_ref(),
                    start,
                    end,
                    *end_inclusive,
                    body,
                    (*loc).into(),
                    block,
                )?,
                Stmt::Break { loc } => {
                    if matches!(continue_mode, ContinueMode::None) {
                        return Err(self.error("break reached MIR outside a loop", (*loc).into()));
                    }
                    self.push_statement(block, StatementKind::Break, (*loc).into());
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
                            source,
                        } => self.emit_for_increment(block, index, step, source),
                    }
                    self.push_statement(block, StatementKind::Continue, (*loc).into());
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_for(
        &mut self,
        variable: &str,
        step: Option<&Expr>,
        start: &Expr,
        end: &Expr,
        end_inclusive: bool,
        body: &[Stmt],
        location: SourceLoc,
        destination: &mut MirBlock,
    ) -> Result<(), MirLoweringError> {
        let start_value = self.lower_expr(start, destination)?;
        let start_value = self.coerce(start_value, PrimitiveType::I32, destination, start.loc())?;
        let start_value = self.snapshot(start_value, destination, start.loc());

        let end_value = self.lower_expr(end, destination)?;
        let end_value = self.coerce(end_value, PrimitiveType::I32, destination, end.loc())?;
        let end_value = self.snapshot(end_value, destination, end.loc());

        let step_value = if let Some(step) = step {
            let value = self.lower_expr(step, destination)?;
            let value = self.coerce(value, PrimitiveType::I32, destination, step.loc())?;
            self.snapshot(value, destination, step.loc())
        } else {
            LoweredValue {
                value: Value::Constant(ScalarValue::I32(1)),
                ty: PrimitiveType::I32,
            }
        };

        self.stop_prezeroed_init_state_proof();
        let outer_bindings = self.bindings.clone();
        let outer_nested_proc_aliases = self.nested_proc_aliases.clone();
        let index = self.new_local(Some(variable.to_owned()), PrimitiveType::I32);
        self.push_statement(
            destination,
            StatementKind::Assign {
                destination: Place::local(index),
                value: Rvalue::Use(start_value.value),
            },
            location,
        );

        self.bindings.insert(
            variable.to_owned(),
            Binding::Local(index, PrimitiveType::I32),
        );
        let mut loop_body = MirBlock::default();
        self.emit_for_guard(
            &mut loop_body,
            index,
            end_value.value,
            step_value.value,
            end_inclusive,
            location,
        );
        self.lower_statements(
            body,
            &mut loop_body,
            ContinueMode::For {
                index,
                step: step_value.value,
                source: location,
            },
        )?;
        self.emit_for_increment(&mut loop_body, index, step_value.value, location);

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
        else_block: &mut MirBlock,
        location: SourceLoc,
    ) {
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
            if let Some(binding) =
                self.reconcile_branch_binding(then_binding, else_binding, else_block, location)
            {
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
    }

    pub(super) fn reconcile_branch_binding(
        &mut self,
        then_binding: Binding,
        else_binding: Binding,
        else_block: &mut MirBlock,
        location: SourceLoc,
    ) -> Option<Binding> {
        match (then_binding, else_binding) {
            (Binding::Local(then_local, then_ty), Binding::Local(else_local, else_ty))
                if then_ty == else_ty && self.local_types_match(then_local, else_local) =>
            {
                self.copy_branch_local(else_block, then_local, else_local, location);
                Some(Binding::Local(then_local, then_ty))
            }
            (
                Binding::Array(then_local, then_element, then_len),
                Binding::Array(else_local, else_element, else_len),
            ) if then_element == else_element
                && then_len == else_len
                && self.local_types_match(then_local, else_local) =>
            {
                self.copy_branch_local(else_block, then_local, else_local, location);
                Some(Binding::Array(then_local, then_element, then_len))
            }
            (
                Binding::Slice(then_local, then_element, then_access),
                Binding::Slice(else_local, else_element, else_access),
            ) if then_element == else_element
                && then_access == else_access
                && self.local_types_match(then_local, else_local) =>
            {
                self.copy_branch_local(else_block, then_local, else_local, location);
                Some(Binding::Slice(then_local, then_element, then_access))
            }
            (Binding::Tuple(then_values), Binding::Tuple(else_values))
                if then_values.len() == else_values.len()
                    && then_values.iter().zip(&else_values).all(
                        |((then_local, then_ty), (else_local, else_ty))| {
                            then_ty == else_ty && self.local_types_match(*then_local, *else_local)
                        },
                    ) =>
            {
                for ((then_local, _), (else_local, _)) in then_values.iter().zip(&else_values) {
                    self.copy_branch_local(else_block, *then_local, *else_local, location);
                }
                Some(Binding::Tuple(then_values))
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
                Some(Binding::TupleSliceElementAlias(then_values))
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
                Some(Binding::SliceElementAlias {
                    slice: then_slice,
                    element: then_element,
                    index: then_index,
                })
            }
            (
                Binding::StructArrayElementAlias {
                    struct_name: then_struct,
                },
                Binding::StructArrayElementAlias {
                    struct_name: else_struct,
                },
            ) if then_struct == else_struct => Some(Binding::StructArrayElementAlias {
                struct_name: then_struct,
            }),
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
                Some(Binding::StructArrayParameter {
                    struct_name: then_struct,
                    length: StructArrayLength::Fixed(then_len),
                    fields: then_fields,
                })
            }
            _ => None,
        }
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
        let zero = Value::Constant(ScalarValue::I32(0));
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

    pub(super) fn emit_for_increment(
        &mut self,
        block: &mut MirBlock,
        index: LocalId,
        step: Value,
        location: SourceLoc,
    ) {
        self.push_statement(
            block,
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
    }
}
