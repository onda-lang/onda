use super::*;

impl FunctionLowerer<'_> {
    pub(super) fn is_struct_array_expression(&self, expr: &Expr) -> bool {
        let source;
        let expr = if let Expr::Slice { base, .. } = expr {
            source = Expr::var(base);
            &source
        } else {
            expr
        };
        matches!(expr, Expr::Var { name, .. } if matches!(self.bindings.get(name), Some(Binding::StructArrayParameter { .. })))
            || matches!(
                self.data_type_of(expr),
                Some(DataType::Array {
                    element: ArrayElemType::Struct(_),
                    ..
                })
            )
    }

    pub(super) fn lower_typed_slice_binding(
        &mut self,
        name: &str,
        element: &ArrayElemType,
        expr: &Expr,
        block: &mut MirBlock,
    ) -> Result<(), MirLoweringError> {
        if let ArrayElemType::Primitive(element) = element {
            if let Some(slice) = self.lower_array_value_slice(
                expr,
                *element,
                onda_mir::AccessMode::ReadWrite,
                block,
            )? {
                return self.assign_slice_alias(name, slice, block, expr.loc());
            }
        }
        let prepared;
        let source = if let Some(data @ DataType::Array { .. }) = self.data_type_of(expr) {
            let root = self.lower_data_expr(expr, &data, block)?;
            prepared = Expr::var(root).with_loc(expr.loc());
            &prepared
        } else {
            expr
        };
        if self.lower_struct_slice_alias(name, source, block)? {
            return Ok(());
        }
        let slice = self.lower_slice_expression(source, None, block)?;
        self.assign_slice_alias(name, slice, block, expr.loc())
    }

    pub(super) fn lower_struct_slice_assignment(
        &mut self,
        target: &Expr,
        expr: &Expr,
        block: &mut MirBlock,
    ) -> Result<bool, MirLoweringError> {
        let destination = self.fresh_data_name();
        if !self.lower_struct_slice_alias(&destination, target, block)? {
            return Ok(false);
        }
        let Some(Binding::StructArrayParameter {
            struct_name,
            length,
            fields: destination_fields,
        }) = self.bindings.get(&destination).cloned()
        else {
            unreachable!()
        };
        if let Some(data @ DataType::Struct(_)) = self.data_type_of(expr) {
            let source = self.lower_data_expr(expr, &data, block)?;
            let captured = self.fresh_data_name();
            self.allocate_data(&captured, &data, expr.loc())?;
            self.copy_data(&captured, &source, &data, block, expr.loc())?;
            let length = self.struct_array_length_value(length, block, expr.loc());
            self.fill_data_array(
                &destination,
                &captured,
                &struct_name,
                length,
                block,
                expr.loc(),
            )?;
            return Ok(true);
        }
        let source = self.fresh_data_name();
        let prepared;
        let expr = if let Some(data @ DataType::Array { .. }) = self.data_type_of(expr) {
            let root = self.lower_data_expr(expr, &data, block)?;
            prepared = Expr::var(root).with_loc(expr.loc());
            &prepared
        } else {
            expr
        };
        if !self.lower_struct_slice_alias(&source, expr, block)? {
            return Err(self.error("struct slice copy source has no data view", expr.loc()));
        }
        let Some(Binding::StructArrayParameter {
            fields: source_fields,
            ..
        }) = self.bindings.get(&source)
        else {
            unreachable!()
        };
        let copies = destination_fields
            .iter()
            .zip(source_fields)
            .map(
                |((_, destination, _), (_, source, _))| onda_mir::SliceCopy {
                    destination: Value::Local(*destination),
                    source: Value::Local(*source),
                },
            )
            .collect();
        // Nominal struct arrays expose canonical contiguous scalar leaves.
        // Slicing scales fixed-width fields in scalar units, preserving equal
        // source/destination strides even when the selected regions overlap.
        self.push_statement(
            block,
            StatementKind::SliceCopy {
                copies,
                preflight: onda_mir::SliceCopyPreflight::ProvenUnnecessary,
            },
            target.loc(),
        );
        Ok(true)
    }

    /// A struct slice carries one logical length and one descriptor per scalar
    /// tensor. Normalize authored selectors once in logical element units.
    pub(super) fn lower_struct_slice_alias(
        &mut self,
        name: &str,
        expr: &Expr,
        block: &mut MirBlock,
    ) -> Result<bool, MirLoweringError> {
        let (base, start, end) = match expr {
            Expr::Slice {
                base,
                selector: None,
                channel: None,
                start,
                end,
                ..
            } => (base, start.as_deref(), end.as_deref()),
            Expr::Var { name, .. } => (name, None, None),
            _ => return Ok(false),
        };
        let loc = expr.loc();
        let struct_name = match self.bindings.get(base) {
            Some(Binding::StructArrayParameter { struct_name, .. }) => struct_name.clone(),
            _ => match self.data_type_of(&Expr::var(base)) {
                Some(DataType::Array {
                    element: ArrayElemType::Struct(name),
                    ..
                }) => name,
                _ => return Ok(false),
            },
        };
        self.prepare_data_array_view(base, block, loc)?;
        let (length, source_fields) = match self.bindings.get(base).cloned() {
            Some(Binding::StructArrayParameter { length, fields, .. }) => (length, fields),
            _ => {
                let len = self
                    .runtime_globals
                    .and_then(|globals| globals.array_struct_roots.get(base))
                    .map(|(_, len)| *len)
                    .ok_or_else(|| self.error("struct slice has no storage origin", loc))?;
                let mut fields = Vec::new();
                for shape in self.struct_field_shapes(&struct_name, loc)? {
                    let (path, element) = match shape {
                        StructFieldShape::Scalar { name, ty } => (name, ty),
                        StructFieldShape::Array { name, element, .. } => (name, element),
                    };
                    let slice = self.lower_slice_expression(
                        &Expr::var(format!("{base}.{path}")),
                        None,
                        block,
                    )?;
                    let Value::Local(local) = slice.value else {
                        unreachable!()
                    };
                    fields.push((path, local, element));
                }
                (StructArrayLength::Fixed(len), fields)
            }
        };
        let length = self.struct_array_length_value(length, block, loc);
        let (start, len) = self.normalize_slice_selection(length, start, end, block, loc)?;
        let shapes = self.struct_field_shapes(&struct_name, loc)?;
        let mut fields = Vec::new();
        for (shape, (path, source, element)) in shapes.into_iter().zip(source_fields) {
            let width = match shape {
                StructFieldShape::Scalar { .. } => 1,
                StructFieldShape::Array { len, .. } => len,
            };
            let access = match self.types[self.locals[source.index()].ty.index()] {
                onda_mir::Type::Slice { access, .. } => access,
                _ => return Err(self.error("struct slice leaf is not a descriptor", loc)),
            };
            let scale = |this: &mut Self, value, block: &mut MirBlock| {
                if width == 1 {
                    value
                } else {
                    this.emit_temp(
                        block,
                        PrimitiveType::I32,
                        Rvalue::Binary {
                            op: MirBinaryOp::Multiply,
                            lhs: value,
                            rhs: Value::Constant(ScalarValue::I32(width as i32)),
                        },
                        loc,
                    )
                    .value
                }
            };
            let leaf_start = scale(self, start, block);
            let leaf_len = scale(self, Value::Local(len), block);
            let slice = self.emit_slice_temp(
                block,
                None,
                element,
                access,
                Rvalue::MakeSlice {
                    source: onda_mir::SliceSource::Place(Place::local(source)),
                    start: leaf_start,
                    len: leaf_len,
                    bounds: BoundsMode::Unchecked,
                    access,
                },
                loc,
            );
            let Value::Local(local) = slice.value else {
                unreachable!()
            };
            self.bindings.insert(
                format!("{name}.{path}"),
                Binding::Slice(local, element, access, None),
            );
            fields.push((path, local, element));
        }
        self.bindings.insert(
            name.to_owned(),
            Binding::StructArrayParameter {
                struct_name,
                length: StructArrayLength::Local(len),
                fields,
            },
        );
        Ok(true)
    }
}
