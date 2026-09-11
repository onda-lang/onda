use super::*;

impl FunctionLowerer<'_> {
    fn reference_place_access(&self, place: &Place) -> onda_mir::AccessMode {
        match place.base {
            PlaceBase::Local(local) => match self.types[self.locals[local.index()].ty.index()] {
                onda_mir::Type::Slice { access, .. } => access,
                _ => onda_mir::AccessMode::ReadWrite,
            },
            PlaceBase::Parameter(parameter)
                if self.params[parameter.index()].mode
                    == onda_mir::PassingMode::ReadOnlyReference =>
            {
                onda_mir::AccessMode::ReadOnly
            }
            PlaceBase::Param(_) | PlaceBase::EventParam(_) => onda_mir::AccessMode::ReadOnly,
            _ => onda_mir::AccessMode::ReadWrite,
        }
    }

    pub(super) fn join_array_references(
        &mut self,
        then_binding: &Binding,
        else_binding: &Binding,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<Option<Binding>, MirLoweringError> {
        let Some((then_source, element, access, len)) = array_reference(then_binding) else {
            return Ok(None);
        };
        let Some((else_source, other_element, other_access, other_len)) =
            array_reference(else_binding)
        else {
            return Ok(None);
        };
        if element != other_element || len != other_len {
            return Ok(None);
        }
        let access = if access == onda_mir::AccessMode::ReadOnly
            || other_access == onda_mir::AccessMode::ReadOnly
        {
            onda_mir::AccessMode::ReadOnly
        } else {
            onda_mir::AccessMode::ReadWrite
        };
        let joined = self.new_slice_local(None, element, access);
        for (block, source) in [(then_block, then_source), (else_block, else_source)] {
            let source_len = if let Some(len) = len {
                Value::Constant(ScalarValue::I32(len as i32))
            } else {
                let PlaceBase::Local(local) = source.base else {
                    unreachable!()
                };
                self.emit_temp(
                    block,
                    PrimitiveType::I32,
                    Rvalue::SliceLen(Value::Local(local)),
                    loc,
                )
                .value
            };
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place::local(joined),
                    value: Rvalue::MakeSlice {
                        source: onda_mir::SliceSource::Place(source),
                        start: Value::Constant(ScalarValue::I32(0)),
                        len: source_len,
                        access,
                        bounds: BoundsMode::Unchecked,
                    },
                },
                loc,
            );
        }
        Ok(Some(Binding::Slice(joined, element, access, len)))
    }

    pub(super) fn join_struct_array_references(
        &mut self,
        then_binding: &Binding,
        else_binding: &Binding,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<Option<Binding>, MirLoweringError> {
        let (
            Binding::StructArrayParameter {
                struct_name,
                length: then_length,
                fields: then_fields,
            },
            Binding::StructArrayParameter {
                struct_name: other_struct,
                length: else_length,
                fields: else_fields,
            },
        ) = (then_binding, else_binding)
        else {
            return Ok(None);
        };
        if struct_name != other_struct || then_fields.len() != else_fields.len() {
            return Ok(None);
        }
        let length = match (then_length, else_length) {
            (StructArrayLength::Fixed(a), StructArrayLength::Fixed(b)) if a == b => {
                StructArrayLength::Fixed(*a)
            }
            (StructArrayLength::Fixed(_), _) | (_, StructArrayLength::Fixed(_)) => return Ok(None),
            _ => {
                let local = self.new_local(None, PrimitiveType::I32);
                for (block, length) in [
                    (&mut *then_block, *then_length),
                    (&mut *else_block, *else_length),
                ] {
                    let value = self.struct_array_length_value(length, block, loc);
                    self.assign_value(block, local, value, loc);
                }
                StructArrayLength::Local(local)
            }
        };
        let mut fields = Vec::new();
        for ((path, then_local, element), (other_path, else_local, other_element)) in
            then_fields.iter().zip(else_fields)
        {
            if path != other_path || element != other_element {
                return Ok(None);
            }
            let binding = |this: &Self, local: LocalId| {
                let onda_mir::Type::Slice { access, .. } =
                    this.types[this.locals[local.index()].ty.index()]
                else {
                    unreachable!()
                };
                Binding::Slice(local, *element, access, None)
            };
            let a = binding(self, *then_local);
            let b = binding(self, *else_local);
            let Some(Binding::Slice(local, _, _, _)) =
                self.join_array_references(&a, &b, then_block, else_block, loc)?
            else {
                return Ok(None);
            };
            fields.push((path.clone(), local, *element));
        }
        Ok(Some(Binding::StructArrayParameter {
            struct_name: struct_name.clone(),
            length,
            fields,
        }))
    }

    pub(super) fn bind_joined_struct_array_leaves(
        &mut self,
        root: &str,
        binding: &Binding,
    ) -> Vec<String> {
        let Binding::StructArrayParameter { fields, .. } = binding else {
            return Vec::new();
        };
        fields
            .iter()
            .map(|(path, local, element)| {
                let onda_mir::Type::Slice { access, .. } =
                    self.types[self.locals[local.index()].ty.index()]
                else {
                    unreachable!()
                };
                let name = Self::data_leaf_name(root, path);
                self.bindings
                    .insert(name.clone(), Binding::Slice(*local, *element, access, None));
                name
            })
            .collect()
    }

    /// A scalar aggregate field is still storage. Join a singleton descriptor,
    /// leaving both candidate objects and their existing aliases untouched.
    pub(super) fn join_scalar_references(
        &mut self,
        then_binding: &Binding,
        else_binding: &Binding,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<Option<Binding>, MirLoweringError> {
        let Some((then_source, then_start, ty)) = scalar_reference(then_binding) else {
            return Ok(None);
        };
        let Some((else_source, else_start, else_ty)) = scalar_reference(else_binding) else {
            return Ok(None);
        };
        if ty != else_ty {
            return Ok(None);
        }
        let access = if self.reference_place_access(&then_source) == onda_mir::AccessMode::ReadOnly
            || self.reference_place_access(&else_source) == onda_mir::AccessMode::ReadOnly
        {
            onda_mir::AccessMode::ReadOnly
        } else {
            onda_mir::AccessMode::ReadWrite
        };
        let slice = self.new_slice_local(None, ty, access);
        for (block, source, start) in [
            (then_block, then_source, then_start),
            (else_block, else_source, else_start),
        ] {
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place::local(slice),
                    value: Rvalue::MakeSlice {
                        source: onda_mir::SliceSource::Place(source),
                        start,
                        len: Value::Constant(ScalarValue::I32(1)),
                        access,
                        bounds: BoundsMode::Unchecked,
                    },
                },
                loc,
            );
        }
        Ok(Some(Binding::SliceElementAlias {
            slice,
            element: ty,
            index: Value::Constant(ScalarValue::I32(0)),
        }))
    }
}

fn array_reference(
    binding: &Binding,
) -> Option<(Place, PrimitiveType, onda_mir::AccessMode, Option<u32>)> {
    match binding {
        Binding::Array(local, element, len) => Some((
            Place::local(*local),
            *element,
            onda_mir::AccessMode::ReadWrite,
            Some(*len),
        )),
        Binding::ArrayParameter(parameter, element, len) => Some((
            Place {
                base: PlaceBase::Parameter(*parameter),
                projections: Vec::new(),
            },
            *element,
            onda_mir::AccessMode::ReadWrite,
            Some(*len),
        )),
        Binding::Slice(local, element, access, len) => {
            Some((Place::local(*local), *element, *access, *len))
        }
        _ => None,
    }
}

fn scalar_reference(binding: &Binding) -> Option<(Place, Value, PrimitiveType)> {
    let zero = Value::Constant(ScalarValue::I32(0));
    match binding {
        Binding::PlaceAlias(place, ty) => Some((place.clone(), zero, *ty)),
        Binding::ReferenceParameter(parameter, ty) => Some((
            Place {
                base: PlaceBase::Parameter(*parameter),
                projections: Vec::new(),
            },
            zero,
            *ty,
        )),
        Binding::SliceElementAlias {
            slice,
            element,
            index,
        } => Some((Place::local(*slice), *index, *element)),
        _ => None,
    }
}
