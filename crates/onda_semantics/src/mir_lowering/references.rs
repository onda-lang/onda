use super::*;

impl FunctionLowerer<'_> {
    /// Scalar leaves are direct references; fixed tensor leaves are windows
    /// into their source storage.
    pub(super) fn struct_reference_arguments(
        &mut self,
        root: &str,
        shapes: &[StructFieldShape],
        access: Option<onda_mir::AccessMode>,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<Vec<CallArgument>, MirLoweringError> {
        shapes
            .iter()
            .map(|shape| match shape {
                StructFieldShape::Scalar { name, ty } => {
                    self.scalar_reference_argument(&format!("{root}.{name}"), *ty, loc)
                }
                StructFieldShape::Array { name, element, len } => {
                    let name = format!("{root}.{name}");
                    if self.data_type_of(&Expr::var(&name))
                        != Some(DataType::Array {
                            element: ArrayElemType::Primitive(*element),
                            len: *len as usize,
                        })
                    {
                        return Err(
                            self.error(format!("struct array field '{name}' changed shape"), loc)
                        );
                    }
                    let slice = self.lower_named_slice(
                        &name,
                        SliceSelection::default(),
                        access,
                        block,
                        loc,
                    )?;
                    Ok(CallArgument::SliceWindow {
                        slice: slice.value,
                        start: Value::Constant(ScalarValue::I32(0)),
                        bounds: BoundsMode::Unchecked,
                    })
                }
            })
            .collect()
    }

    fn scalar_reference_argument(
        &self,
        name: &str,
        expected: PrimitiveType,
        loc: SourceLoc,
    ) -> Result<CallArgument, MirLoweringError> {
        let (argument, actual) = match self.bindings.get(name) {
            Some(Binding::PlaceAlias(place, ty)) => (CallArgument::Place(place.clone()), *ty),
            Some(Binding::Local(local, ty)) => (CallArgument::Place(Place::local(*local)), *ty),
            Some(Binding::EventParameter(parameter, ty)) => (
                CallArgument::Place(Place {
                    base: PlaceBase::EventParam(*parameter),
                    projections: Vec::new(),
                }),
                *ty,
            ),
            Some(Binding::ReferenceParameter(parameter, ty)) => (
                CallArgument::Place(Place {
                    base: PlaceBase::Parameter(*parameter),
                    projections: Vec::new(),
                }),
                *ty,
            ),
            Some(Binding::SliceElementAlias {
                slice,
                element,
                index,
            }) => (
                CallArgument::SliceElement {
                    slice: Value::Local(*slice),
                    index: *index,
                    bounds: BoundsMode::Unchecked,
                },
                *element,
            ),
            Some(_) => {
                return Err(self.error(
                    format!("binding '{name}' is not scalar reference storage"),
                    loc,
                ));
            }
            None => {
                let Some((state, ty)) = self
                    .runtime_globals_for_unbound(name)
                    .and_then(|globals| globals.states.get(name))
                    .copied()
                else {
                    return Err(self.error(
                        format!("struct scalar field '{name}' has no reference storage"),
                        loc,
                    ));
                };
                (
                    CallArgument::Place(Place {
                        base: PlaceBase::State(state),
                        projections: Vec::new(),
                    }),
                    ty,
                )
            }
        };
        if actual != expected {
            return Err(self.error(format!("struct scalar field '{name}' changed type"), loc));
        }
        Ok(argument)
    }

    pub(super) fn restrict_reference_arguments(
        &mut self,
        arguments: &mut [CallArgument],
        access: onda_mir::AccessMode,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        for argument in arguments {
            let CallArgument::Value(Value::Local(local)) = argument else {
                continue;
            };
            let MirType::Slice { element, .. } = self.types[self.locals[local.index()].ty.index()]
            else {
                continue;
            };
            *argument = CallArgument::Value(self.restrict_slice_access(
                *local,
                source_scalar_type(element),
                access,
                block,
                loc,
            )?);
        }
        Ok(())
    }
}
