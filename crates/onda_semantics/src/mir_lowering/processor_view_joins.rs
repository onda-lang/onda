//! Joins complete borrowed processor capabilities without inventing references.

use super::processor_views::{LoweredProcessorEnvironment, LoweredProcessorView};
use super::*;
use crate::processor_views::{
    ProcessorEnvironmentShape, ProcessorStorageShape, ProcessorViewShape,
};

fn plain_environment() -> LoweredProcessorEnvironment {
    LoweredProcessorEnvironment {
        shape: ProcessorEnvironmentShape {
            state: None,
            owner_storage: None,
            owner: None,
            resources: Vec::new(),
            outer: None,
        },
        owner: None,
        state_root: None,
        coordinates: Vec::new(),
        resources: Vec::new(),
        outer: None,
    }
}

fn normalize(mut view: LoweredProcessorView) -> LoweredProcessorView {
    if view.shape.environments.is_empty() {
        let plain = plain_environment();
        view.shape.environments.push(plain.shape.clone());
        view.environments.push(plain);
    }
    view
}

impl FunctionLowerer<'_> {
    pub(super) fn join_processor_views(
        &mut self,
        then_view: Option<&LoweredProcessorView>,
        else_view: Option<&LoweredProcessorView>,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<Option<LoweredProcessorView>, MirLoweringError> {
        let Some(example) = then_view.or(else_view) else {
            return Ok(None);
        };
        let plain = LoweredProcessorView {
            shape: ProcessorViewShape::plain(example.shape.proc_name.clone(), example.shape.len),
            tag: None,
            environments: Vec::new(),
        };
        let then_view = normalize(then_view.unwrap_or(&plain).clone());
        let else_view = normalize(else_view.unwrap_or(&plain).clone());
        let shape = then_view.shape.join(&else_view.shape).ok_or_else(|| {
            self.error(
                "processor views reached a join with incompatible nominal types",
                loc.clone(),
            )
        })?;
        let tag = if shape.environments.len() > 1 {
            let tag = self.new_local(None, PrimitiveType::I32);
            self.assign_processor_tag(&then_view, &shape, tag, then_block, loc.clone());
            self.assign_processor_tag(&else_view, &shape, tag, else_block, loc.clone());
            Some(Value::Local(tag))
        } else {
            None
        };
        let mut environments = Vec::with_capacity(shape.environments.len());
        for environment in &shape.environments {
            let then_value = then_view
                .environments
                .iter()
                .find(|value| value.shape == *environment);
            let else_value = else_view
                .environments
                .iter()
                .find(|value| value.shape == *environment);
            let value = match (then_value, else_value) {
                (Some(a), Some(b)) => {
                    self.join_processor_environment(a, b, then_block, else_block, loc.clone())?
                }
                (Some(value), None) => {
                    self.publish_processor_environment(value, then_block, else_block, loc.clone())
                }
                (None, Some(value)) => {
                    self.publish_processor_environment(value, else_block, then_block, loc.clone())
                }
                (None, None) => unreachable!("joined environment comes from one input"),
            };
            environments.push(value);
        }
        Ok(Some(LoweredProcessorView {
            shape,
            tag,
            environments,
        }))
    }

    fn assign_processor_tag(
        &mut self,
        view: &LoweredProcessorView,
        joined: &ProcessorViewShape,
        destination: LocalId,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) {
        let mapping = view
            .shape
            .environments
            .iter()
            .map(|environment| {
                joined
                    .environments
                    .iter()
                    .position(|joined| joined == environment)
                    .unwrap() as i32
            })
            .collect::<Vec<_>>();
        let Some(tag) = view.tag else {
            self.assign_value(
                block,
                destination,
                Value::Constant(ScalarValue::I32(mapping[0])),
                loc,
            );
            return;
        };
        if mapping
            .iter()
            .enumerate()
            .all(|(source, target)| source as i32 == *target)
        {
            self.assign_value(block, destination, tag, loc);
            return;
        }
        let mut dispatch = MirBlock::default();
        self.assign_value(
            &mut dispatch,
            destination,
            Value::Constant(ScalarValue::I32(*mapping.last().unwrap())),
            loc.clone(),
        );
        for (source, target) in mapping.iter().enumerate().rev().skip(1) {
            let condition = self.compare_value(
                block,
                CompareOp::Equal,
                tag,
                Value::Constant(ScalarValue::I32(source as i32)),
                loc.clone(),
            );
            let mut selected = MirBlock::default();
            self.assign_value(
                &mut selected,
                destination,
                Value::Constant(ScalarValue::I32(*target)),
                loc.clone(),
            );
            let mut next = MirBlock::default();
            self.push_statement(
                &mut next,
                StatementKind::If {
                    condition,
                    then_block: selected,
                    else_block: dispatch,
                },
                loc.clone(),
            );
            dispatch = next;
        }
        block.statements.extend(dispatch.statements);
    }

    pub(super) fn join_processor_coordinate(
        &mut self,
        then_value: Value,
        else_value: Value,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Value {
        if then_value == else_value {
            return then_value;
        }
        let local = self.new_local(None, PrimitiveType::I32);
        self.assign_value(then_block, local, then_value, loc.clone());
        self.assign_value(else_block, local, else_value, loc);
        Value::Local(local)
    }

    fn publish_processor_environment(
        &mut self,
        environment: &LoweredProcessorEnvironment,
        active: &mut MirBlock,
        inactive: &mut MirBlock,
        loc: SourceLoc,
    ) -> LoweredProcessorEnvironment {
        let mut result = environment.clone();
        // Capture leaves are persistent roots or parameters. Initialize choice
        // tags on both paths; physical references are reconstructed at use sites.
        result.owner = environment
            .owner
            .as_ref()
            .map(|capture| self.publish_processor_capture(capture, active, inactive, loc.clone()));
        result.state_root = if environment.state_root == environment.owner {
            result.owner.clone()
        } else {
            environment.state_root.as_ref().map(|capture| {
                self.publish_processor_capture(capture, active, inactive, loc.clone())
            })
        };
        result.resources = environment
            .resources
            .iter()
            .map(|capture| self.publish_processor_capture(capture, active, inactive, loc.clone()))
            .collect();
        result.coordinates = environment
            .coordinates
            .iter()
            .map(|coordinate| {
                self.join_processor_coordinate(
                    *coordinate,
                    Value::Constant(ScalarValue::I32(0)),
                    active,
                    inactive,
                    loc.clone(),
                )
            })
            .collect();
        result.outer = environment.outer.as_ref().map(|outer| {
            Box::new(self.publish_processor_environment(outer, active, inactive, loc.clone()))
        });
        result
    }

    fn join_processor_environment(
        &mut self,
        a: &LoweredProcessorEnvironment,
        b: &LoweredProcessorEnvironment,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<LoweredProcessorEnvironment, MirLoweringError> {
        if a.coordinates.len() != b.coordinates.len() {
            return Err(self.error(
                "processor environment coordinate arity changed at a join",
                loc,
            ));
        }
        let owner = match (&a.owner, &b.owner, &a.shape.owner) {
            (Some(a_root), Some(b_root), Some(_)) => Some(self.join_processor_capture(
                a_root,
                b_root,
                then_block,
                else_block,
                loc.clone(),
            )),
            (None, None, None) => None,
            _ => {
                return Err(self.error("processor environment owner capture changed at a join", loc))
            }
        };
        let state_root = match (&a.state_root, &b.state_root, &a.shape.state) {
            (Some(a_root), Some(b_root), Some(_)) => {
                if a.owner.as_ref() == Some(a_root) && b.owner.as_ref() == Some(b_root) {
                    owner.clone()
                } else {
                    Some(self.join_processor_capture(
                        a_root,
                        b_root,
                        then_block,
                        else_block,
                        loc.clone(),
                    ))
                }
            }
            (None, None, _) => None,
            (Some(a_root), Some(b_root), None) if a_root == b_root => Some(a_root.clone()),
            _ => {
                return Err(self.error(
                    "processor state root changed without a typed projection",
                    loc,
                ))
            }
        };
        let coordinates = a
            .coordinates
            .iter()
            .zip(&b.coordinates)
            .map(|(a, b)| {
                self.join_processor_coordinate(*a, *b, then_block, else_block, loc.clone())
            })
            .collect();
        let resources = a
            .resources
            .iter()
            .zip(&b.resources)
            .map(|(a, b)| self.join_processor_capture(a, b, then_block, else_block, loc.clone()))
            .collect();
        let outer = match (&a.outer, &b.outer) {
            (Some(a), Some(b)) => Some(Box::new(self.join_processor_environment(
                a,
                b,
                then_block,
                else_block,
                loc.clone(),
            )?)),
            (None, None) => None,
            _ => return Err(self.error("processor outer environment changed at a join", loc)),
        };
        Ok(LoweredProcessorEnvironment {
            shape: a.shape.clone(),
            owner,
            state_root,
            coordinates,
            resources,
            outer,
        })
    }

    pub(super) fn join_processor_storage_root(
        &mut self,
        a: &str,
        b: &str,
        storage: &ProcessorStorageShape,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<String, MirLoweringError> {
        if a == b {
            return Ok(a.to_owned());
        }
        let Some(len) = storage.len else {
            return self.join_processor_owner(
                a,
                b,
                &storage.proc_name,
                then_block,
                else_block,
                loc,
            );
        };
        let root = self.fresh_data_name();
        let mut fields = Vec::new();
        for shape in self.struct_field_shapes(&storage.proc_name, loc.clone())? {
            let (path, element) = match shape {
                StructFieldShape::Scalar { name, ty } => (name, ty),
                StructFieldShape::Array { name, element, .. } => (name, element),
            };
            let a = self.lower_named_slice(
                &format!("{a}.{path}"),
                SliceSelection::default(),
                None,
                then_block,
                loc.clone(),
            )?;
            let b = self.lower_named_slice(
                &format!("{b}.{path}"),
                SliceSelection::default(),
                None,
                else_block,
                loc.clone(),
            )?;
            let (Value::Local(a_local), Value::Local(b_local)) = (a.value, b.value) else {
                unreachable!("root columns are local slice descriptors")
            };
            let a = Binding::Slice(a_local, element, a.access, None);
            let b = Binding::Slice(b_local, element, b.access, None);
            let Some(Binding::Slice(local, _, _, _)) =
                self.join_array_references(&a, &b, then_block, else_block, loc.clone())?
            else {
                unreachable!("same typed root columns")
            };
            fields.push((path, local, element));
        }
        let binding = Binding::StructArrayParameter {
            struct_name: storage.proc_name.clone(),
            processor: None,
            length: StructArrayLength::Fixed(self.data_extent(len, loc)?),
            fields,
        };
        self.bind_joined_struct_array_leaves(&root, &binding);
        self.bindings.insert(root.clone(), binding);
        Ok(root)
    }

    fn join_processor_owner(
        &mut self,
        a: &str,
        b: &str,
        owner_type: &str,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<String, MirLoweringError> {
        if a == b {
            return Ok(a.to_owned());
        }
        let shapes = self.struct_field_shapes(owner_type, loc.clone())?;
        let a_fields =
            self.struct_reference_arguments(a, &shapes, None, then_block, loc.clone())?;
        let b_fields =
            self.struct_reference_arguments(b, &shapes, None, else_block, loc.clone())?;
        let root = self.fresh_data_name();
        self.bind_data_root(&root, &DataType::Struct(owner_type.to_owned()), loc.clone())?;
        for ((shape, a), b) in shapes.into_iter().zip(a_fields).zip(b_fields) {
            let (path, binding) = match shape {
                StructFieldShape::Scalar { name, ty } => {
                    let binding = |argument| match argument {
                        CallArgument::Place(place) => Binding::PlaceAlias(place, ty),
                        CallArgument::SliceElement {
                            slice: Value::Local(slice),
                            index,
                            ..
                        } => Binding::SliceElementAlias {
                            slice,
                            element: ty,
                            index,
                        },
                        _ => unreachable!("scalar owner fields are references"),
                    };
                    let joined = self
                        .join_scalar_references(
                            &binding(a),
                            &binding(b),
                            then_block,
                            else_block,
                            loc.clone(),
                        )?
                        .expect("same typed owner leaf");
                    (name, joined)
                }
                StructFieldShape::Array { name, element, len } => {
                    let binding = |this: &Self, argument| match argument {
                        CallArgument::SliceWindow {
                            slice: Value::Local(slice),
                            ..
                        } => {
                            let MirType::Slice { access, .. } =
                                this.types[this.locals[slice.index()].ty.index()]
                            else {
                                unreachable!()
                            };
                            Binding::Slice(slice, element, access, Some(len))
                        }
                        _ => unreachable!("array owner fields are slice windows"),
                    };
                    let a = binding(self, a);
                    let b = binding(self, b);
                    let joined = self
                        .join_array_references(&a, &b, then_block, else_block, loc.clone())?
                        .expect("same typed owner tensor");
                    (name, joined)
                }
            };
            self.bindings
                .insert(Self::data_leaf_name(&root, &path), binding);
        }
        Ok(root)
    }
}
