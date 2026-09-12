use super::*;
use onda_processor_abi::payload::{PayloadDefault, PayloadField, PayloadSchema, PayloadType};

pub(super) fn message_schema(
    params: &[crate::TypedEventParam],
    layouts: &AggregateLayoutTable,
) -> Result<PayloadSchema, MirLoweringError> {
    let scalar = |ty| PayloadType::scalar(crate::aggregate_layout::scalar_encoding(ty));
    let structure = |name: &str| {
        layouts
            .layout_for_struct(name)
            .map(|layout| layout.payload_type().clone())
            .ok_or_else(|| {
                MirLoweringError::new(
                    format!("unknown message data type '{name}'"),
                    SourceLoc::ZERO,
                )
            })
    };
    let default_scalar = |value: &TypedConstValue| crate::data_construction::scalar_default(*value);
    Ok(PayloadSchema {
        params: params
            .iter()
            .map(|param| {
                let ty = match &param.ty {
                    TypedEventParamType::StructSlice { name } => PayloadType::Slice {
                        element: Box::new(structure(name)?),
                    },
                    TypedEventParamType::Tuple(types) => PayloadType::Tuple {
                        elements: types.iter().copied().map(scalar).collect(),
                    },
                    TypedEventParamType::Scalar(ty) => scalar(*ty),
                    TypedEventParamType::Array { elem, len } => PayloadType::Array {
                        element: Box::new(scalar(*elem)),
                        len: *len,
                    },
                    TypedEventParamType::Slice { elem } => PayloadType::Slice {
                        element: Box::new(scalar(*elem)),
                    },
                    TypedEventParamType::Data(DataType::Struct(name)) => structure(name)?,
                    TypedEventParamType::Data(DataType::Array { element, len }) => {
                        PayloadType::Array {
                            element: Box::new(match element {
                                ArrayElemType::Primitive(ty) => scalar(*ty),
                                ArrayElemType::Struct(name) => structure(name)?,
                            }),
                            len: *len,
                        }
                    }
                };
                let default = param.default.as_ref().map(|default| match default {
                    TypedEventParamDefault::Scalar(value) => default_scalar(value),
                    TypedEventParamDefault::Array(values) => {
                        PayloadDefault::Aggregate(values.iter().map(default_scalar).collect())
                    }
                });
                Ok(PayloadField {
                    name: param.name.clone(),
                    ty,
                    default,
                })
            })
            .collect::<Result<_, MirLoweringError>>()?,
    })
}

/// Materialized message tensors use the same scalar and array event bindings
/// as primitive messages. Nominal roots remain in the source binding table.
pub(super) fn message_params(
    params: &[crate::TypedEventParam],
    layouts: &AggregateLayoutTable,
) -> Result<Vec<crate::TypedEventParam>, MirLoweringError> {
    let mut result = Vec::new();
    for param in params {
        if let TypedEventParamType::Tuple(types) = &param.ty {
            let defaults = match &param.default {
                Some(TypedEventParamDefault::Array(values)) => Some(values),
                _ => None,
            };
            for (index, ty) in types.iter().copied().enumerate() {
                result.push(crate::TypedEventParam {
                    name: format!("{}.__{index}", param.name),
                    ty: TypedEventParamType::Scalar(ty),
                    default: defaults
                        .and_then(|values| values.get(index))
                        .copied()
                        .map(TypedEventParamDefault::Scalar),
                });
            }
            continue;
        }
        let dynamic = matches!(param.ty, TypedEventParamType::StructSlice { .. });
        let (name, outer) = match &param.ty {
            TypedEventParamType::StructSlice { name } => {
                result.push(crate::TypedEventParam {
                    name: param.name.clone(),
                    ty: TypedEventParamType::Scalar(PrimitiveType::I32),
                    default: None,
                });
                (name, None)
            }
            TypedEventParamType::Data(data) => match data {
                DataType::Struct(name) => (name, None),
                DataType::Array {
                    element: ArrayElemType::Struct(name),
                    len,
                } => (name, Some(*len)),
                DataType::Array { .. } => {
                    return Err(MirLoweringError::new(
                        "primitive message arrays use their scalar element contract",
                        SourceLoc::ZERO,
                    ))
                }
            },
            _ => {
                result.push(param.clone());
                continue;
            }
        };
        let layout = layouts.layout_for_struct(name).ok_or_else(|| {
            MirLoweringError::new(
                format!("unknown message data type '{name}'"),
                SourceLoc::ZERO,
            )
        })?;
        for leaf in &layout.leaves {
            let tensor = if let Some(len) = outer {
                leaf.tensor.with_outer_extent(len)
            } else {
                Ok(leaf.tensor.clone())
            }
            .map_err(|error| MirLoweringError::new(error.to_string(), SourceLoc::ZERO))?;
            result.push(crate::TypedEventParam {
                name: format!("{}.{}", param.name, leaf.storage_path),
                ty: if dynamic {
                    TypedEventParamType::Slice { elem: leaf.scalar }
                } else if tensor.shape.is_empty() {
                    TypedEventParamType::Scalar(leaf.scalar)
                } else {
                    TypedEventParamType::Array {
                        elem: leaf.scalar,
                        len: tensor.element_count,
                    }
                },
                default: None,
            });
        }
    }
    Ok(result)
}

impl FunctionLowerer<'_> {
    pub(super) fn bind_event_struct_slices(
        &mut self,
        block: &mut MirBlock,
    ) -> Result<(), MirLoweringError> {
        for (name, struct_name) in self.event_struct_slices.clone() {
            let length = self.lower_expr(&Expr::var(&name), block)?;
            let Value::Local(length) = length.value else {
                unreachable!("event scalar loads are locals")
            };
            let mut fields = Vec::new();
            for shape in self.struct_field_shapes(&struct_name, SourceLoc::ZERO)? {
                let field = match shape {
                    StructFieldShape::Scalar { name, .. }
                    | StructFieldShape::Array { name, .. } => name,
                };
                let Some(Binding::Slice(local, element, _, _)) =
                    self.bindings.get(&format!("{name}.{field}"))
                else {
                    return Err(self.error("event struct slice is missing a leaf", SourceLoc::ZERO));
                };
                fields.push((field, *local, *element));
            }
            self.bindings.insert(
                name,
                Binding::StructArrayParameter {
                    struct_name,
                    length: StructArrayLength::Local(length),
                    fields,
                },
            );
        }
        Ok(())
    }

    /// Publication captures scalar leaves at the dispatch point. Tensor
    /// descriptors stay borrowed until the host record has been serialized.
    pub(super) fn message_publication_arguments(
        &mut self,
        kinds: &[TypedFnParam],
        args: Vec<CallArgument>,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<Vec<CallArgument>, MirLoweringError> {
        let mut args = args.into_iter();
        let mut result = Vec::new();
        for kind in kinds {
            let shapes = match kind {
                TypedFnParam::Struct { struct_name } => {
                    Some(self.struct_field_shapes(struct_name, loc)?)
                }
                TypedFnParam::StructArray { struct_name, len } => {
                    let length = args
                        .next()
                        .ok_or_else(|| self.error("message array is missing its length", loc))?;
                    if len.is_none() {
                        result.push(length);
                    }
                    Some(
                        self.struct_field_shapes(struct_name, loc)?
                            .into_iter()
                            .map(|shape| match shape {
                                StructFieldShape::Scalar { name, ty } => StructFieldShape::Array {
                                    name,
                                    element: ty,
                                    len: 1,
                                },
                                shape => shape,
                            })
                            .collect(),
                    )
                }
                _ => None,
            };
            if let Some(shapes) = shapes {
                for shape in shapes {
                    let arg = args
                        .next()
                        .ok_or_else(|| self.error("message is missing a tensor", loc))?;
                    let arg = if let StructFieldShape::Scalar { ty, .. } = shape {
                        let read = match arg {
                            CallArgument::Place(place) => Rvalue::Load(place),
                            CallArgument::SliceElement {
                                slice,
                                index,
                                bounds,
                            } => Rvalue::SliceLoad {
                                slice,
                                index,
                                bounds,
                            },
                            _ => {
                                return Err(
                                    self.error("message scalar has no reference storage", loc)
                                )
                            }
                        };
                        CallArgument::Value(self.emit_temp(block, ty, read, loc).value)
                    } else {
                        arg
                    };
                    result.push(arg);
                }
            } else {
                let count = match kind {
                    TypedFnParam::Tuple { elem_tys } => elem_tys.len(),
                    _ => 1,
                };
                for _ in 0..count {
                    result.push(
                        args.next()
                            .ok_or_else(|| self.error("message is missing an argument", loc))?,
                    );
                }
            }
        }
        if args.next().is_some() {
            return Err(self.error("message has excess arguments", loc));
        }
        Ok(result)
    }
}

pub(super) fn event_body(event: &TypedEvent) -> Vec<Stmt> {
    let mut body = Vec::new();
    for param in &event.params {
        if let TypedEventParamType::Tuple(types) = &param.ty {
            body.push(Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Var(param.name.clone()),
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::Tuple {
                    loc: Default::default(),
                    values: (0..types.len())
                        .map(|index| Expr::var(format!("{}.__{index}", param.name)))
                        .collect(),
                },
            });
        }
    }
    body.extend(event.body.clone());
    body
}
