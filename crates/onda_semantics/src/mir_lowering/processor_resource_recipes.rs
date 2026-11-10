//! Prepare inherited bindings once, in owner order, inside a selected operation.
use super::processor_views::LoweredProcessorEnvironment;
use super::*;
use crate::processor_views::{ProcessorOperationResource, ProcessorResourceBinding};
use std::collections::BTreeSet;

struct ResourceFrame<'a> {
    environment: &'a LoweredProcessorEnvironment,
    needed: BTreeSet<usize>,
    prepared: Vec<Option<Expr>>,
}

impl FunctionLowerer<'_> {
    pub(super) fn processor_operation_resources(
        &mut self,
        environment: &LoweredProcessorEnvironment,
        resources: &[ProcessorOperationResource],
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Vec<Expr>, MirLoweringError> {
        if resources.is_empty() {
            return Ok(Vec::new());
        }
        let mut frames = Vec::new();
        let mut current = Some(environment);
        while let Some(environment) = current {
            if environment.resources.len() != environment.shape.resources.len() {
                return Err(self.error(
                    "processor environment changed its resource schema",
                    location,
                ));
            }
            frames.push(ResourceFrame {
                environment,
                needed: BTreeSet::new(),
                prepared: vec![None; environment.resources.len()],
            });
            current = environment.outer.as_deref();
        }
        for resource in resources {
            frames
                .get_mut(resource.frame_depth)
                .ok_or_else(|| {
                    self.error(
                        "processor operation lost its resource frame",
                        location.clone(),
                    )
                })?
                .needed
                .insert(resource.resource_index);
        }
        // Discover the complete dependency set before emitting any selectors.
        // Resolving in child-use order could both repeat and reorder effects.
        for depth in 0..frames.len() {
            if !frames[depth].needed.is_empty()
                && !frames[depth].environment.resource_sources.is_empty()
            {
                return self.processor_element_resources(
                    environment,
                    depth,
                    resources,
                    block,
                    location,
                );
            }
            for index in frames[depth].needed.clone() {
                let environment = frames[depth].environment;
                let shape = environment.shape.resources.get(index).ok_or_else(|| {
                    self.error(
                        "processor operation lost an ordered resource capture",
                        location.clone(),
                    )
                })?;
                let Some(binding) = &shape.binding else {
                    continue;
                };
                let dependencies = match binding {
                    ProcessorResourceBinding::Forwarded { resource_index } => vec![*resource_index],
                    _ => {
                        let recipe =
                            self.processor_views
                                .resource_recipe(binding)
                                .ok_or_else(|| {
                                    self.error(
                                        "inherited processor resource lost its declaration recipe",
                                        location.clone(),
                                    )
                                })?;
                        let names = recipe
                            .expression
                            .walk()
                            .filter_map(|node| match node {
                                Expr::Var { name, .. }
                                | Expr::Index { base: name, .. }
                                | Expr::Slice { base: name, .. } => {
                                    Some(name.strip_prefix("self.").unwrap_or(name))
                                }
                                _ => None,
                            })
                            .collect::<HashSet<_>>();
                        recipe
                            .parameters
                            .iter()
                            .enumerate()
                            .filter_map(|(index, name)| {
                                names.contains(name.as_str()).then_some(index)
                            })
                            .collect()
                    }
                };
                frames
                    .get_mut(depth + 1)
                    .ok_or_else(|| {
                        self.error(
                            "inherited processor resource lost its owner frame",
                            location.clone(),
                        )
                    })?
                    .needed
                    .extend(dependencies);
            }
        }
        // Parents precede their children; each frame retains declaration order.
        for depth in (0..frames.len()).rev() {
            let (current, parents) = frames.split_at_mut(depth + 1);
            let frame = &mut current[depth];
            let parent_resources = parents
                .first()
                .map_or(&[][..], |parent| parent.prepared.as_slice());
            for &index in &frame.needed {
                frame.prepared[index] = Some(self.lower_processor_resource_recipe(
                    frame.environment,
                    index,
                    parent_resources,
                    block,
                    location.clone(),
                )?);
            }
        }
        resources
            .iter()
            .map(|resource| {
                frames[resource.frame_depth].prepared[resource.resource_index]
                    .clone()
                    .ok_or_else(|| {
                        self.error(
                            "processor operation lost its prepared resource",
                            location.clone(),
                        )
                    })
            })
            .collect()
    }

    fn lower_processor_resource_recipe(
        &mut self,
        frame: &processor_views::LoweredProcessorEnvironment,
        index: usize,
        parent_resources: &[Option<Expr>],
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Expr, MirLoweringError> {
        let (expression, shape) = frame
            .resources
            .get(index)
            .zip(frame.shape.resources.get(index))
            .ok_or_else(|| {
                self.error(
                    "processor operation lost an ordered resource capture",
                    location.clone(),
                )
            })?;
        let mut globals = Vec::new();
        let mut expression = if let Some(binding) = &shape.binding {
            if let crate::processor_views::ProcessorResourceBinding::Forwarded { resource_index } =
                binding
            {
                return parent_resources
                    .get(*resource_index)
                    .and_then(Clone::clone)
                    .ok_or_else(|| {
                        self.error("owned processor lost its prepared resource", location)
                    });
            }
            let recipe = self
                .processor_views
                .resource_recipe(binding)
                .ok_or_else(|| {
                    self.error(
                        "inherited processor resource lost its declaration recipe",
                        location.clone(),
                    )
                })?;
            let expression = recipe.expression.clone();
            let names = recipe.parameters.to_vec();
            globals.extend_from_slice(recipe.globals);
            let parent = frame.outer.as_deref().ok_or_else(|| {
                self.error(
                    "inherited processor resource lost its owner frame",
                    location.clone(),
                )
            })?;
            let needs_owner = expression.walk().any(|node| {
                let name = match node {
                    Expr::Var { name, .. }
                    | Expr::Index { base: name, .. }
                    | Expr::Slice { base: name, .. } => name,
                    _ => return false,
                };
                name == "self"
                    || name
                        .strip_prefix("self.")
                        .is_some_and(|field| !names.iter().any(|name| name == field))
            });
            let owner = needs_owner
                .then(|| {
                    self.processor_environment_state(parent, block, location.clone())
                        .map(Expr::var)
                })
                .transpose()?;
            crate::processor_views::instantiate_resource_recipe(
                &expression,
                owner.as_ref(),
                &names,
                parent_resources,
            )
        } else {
            let expression = expression.as_ref().ok_or_else(|| {
                self.error(
                    "processor environment lost its direct resource capture",
                    location.clone(),
                )
            })?;
            self.materialize_processor_resource_capture(expression, shape, block, location.clone())?
        };
        let context = crate::compile_context::CompileContext::new(AnalysisOptions {
            sample_rate: self.config.sample_rate,
            block_size: self.config.block_size as usize,
        });
        expression.visit_mut_postorder(|node| {
            if let Expr::UserCall { name, .. } = node {
                *name = self
                    .processor_views
                    .materialized_target_for(context, name)
                    .to_owned();
            }
        });
        // Root selectors belong to their declaration scope. A helper parameter
        // with the same name cannot redirect a captured resource recipe.
        let global_bindings = self
            .runtime_globals
            .and_then(|globals| globals.retained_init.as_ref())
            .map(|retained| &retained.bindings);
        let coordinate = frame.coordinates.last().copied().map(|coordinate| {
            self.processor_view_offset(frame.resource_offset, coordinate, block, location.clone())
        });
        if globals.is_empty() || global_bindings.is_none() {
            return self.processor_resource_expression(&expression, shape, coordinate, block);
        }
        let belongs_to_globals = |name: &str| {
            globals.iter().any(|global| {
                name == global
                    || name
                        .strip_prefix(global)
                        .is_some_and(|suffix| suffix.starts_with('.'))
            })
        };
        let declarations = global_bindings
            .into_iter()
            .flat_map(|bindings| bindings.iter())
            .filter(|(name, _)| belongs_to_globals(name))
            .map(|(name, binding)| (name.clone(), binding.clone()))
            .collect::<Vec<_>>();
        let shadowed = self
            .bindings
            .keys()
            .filter(|name| belongs_to_globals(name))
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|name| self.bindings.remove(&name).map(|value| (name, value)))
            .collect::<Vec<_>>();
        self.bindings.extend(declarations);
        let result = self.processor_resource_expression(&expression, shape, coordinate, block);
        self.bindings.retain(|name, _| !belongs_to_globals(name));
        self.bindings.extend(shadowed);
        result
    }
}
