//! Owned element initializers keep deferred resource selection in source order.
use super::processor_captures::ProcessorCapture;
use super::processor_views::{LoweredProcessorEnvironment, LoweredProcessorView};
use super::*;
use crate::processor_views::{ProcessorOperationResource, ProcessorViewShape};

impl FunctionLowerer<'_> {
    pub(super) fn bind_owned_processor_elements(
        &mut self,
        target: &str,
        sources: &[String],
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let sources = sources
            .iter()
            .map(|source| self.processor_view(source))
            .collect::<Result<Option<Vec<_>>, _>>()?;
        let Some(sources) = sources else {
            return Ok(());
        };
        let Some(shape) = ProcessorViewShape::owned_elements(
            &sources
                .iter()
                .map(|source| source.shape.clone())
                .collect::<Vec<_>>(),
        ) else {
            return Ok(());
        };
        let shape = shape.with_local_lifecycle(self.local_processor_lifetimes);
        let environment = &shape.environments[0];
        let view = LoweredProcessorView {
            start: Value::Constant(ScalarValue::I32(0)),
            tag: None,
            environments: vec![LoweredProcessorEnvironment {
                resource_offset: Value::Constant(ScalarValue::I32(0)),
                shape: environment.clone(),
                owner: None,
                state_root: Some(ProcessorCapture::Direct(target.to_owned())),
                coordinates: Vec::new(),
                resources: vec![None; environment.resources.len()],
                resource_sources: if environment.resource_sources.is_empty() {
                    Vec::new()
                } else {
                    sources
                        .iter()
                        .map(|source| {
                            self.processor_resource_source(source, block, location.clone())
                        })
                        .collect::<Result<_, _>>()?
                },
                outer: None,
            }],
            shape,
        };
        self.initialize_owned_processor_lifecycle(target, &view.shape, block, location.clone())?;
        if let Some(
            Binding::StructArrayParameter { processor, .. }
            | Binding::ProcArrayParameter { processor, .. },
        ) = self.bindings.get_mut(target)
        {
            *processor = Some(view);
        }
        self.register_owned_processor_lifetime(target, block, location)?;
        Ok(())
    }

    /// Collapse row choices into the same canonical symbolic environments used
    /// at source-level branch joins. Only tags and coordinates are evaluated;
    /// resource recipes still execute in the selected operation branch.
    fn processor_resource_source(
        &mut self,
        source: &LoweredProcessorView,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<LoweredProcessorView, MirLoweringError> {
        self.select_processor_capture(
            source.environments.len(),
            (source.tag, 0),
            block,
            location,
            &mut |this, index, block, location| {
                let mut frame = &source.environments[index];
                while frame.shape.forwards_resources() {
                    frame = frame.outer.as_deref().expect("forwarded resource parent");
                }
                if frame.resource_sources.is_empty() {
                    return Ok(LoweredProcessorView {
                        start: Value::Constant(ScalarValue::I32(0)),
                        tag: None,
                        shape: ProcessorViewShape {
                            environments: vec![frame.shape.clone()],
                            ..source.shape.selected()
                        },
                        environments: vec![frame.clone()],
                    });
                }
                let coordinate = frame.coordinates.last().copied().ok_or_else(|| {
                    this.error(
                        "owned processor array lost its selected row",
                        location.clone(),
                    )
                })?;
                let coordinate = this.processor_view_offset(
                    frame.resource_offset,
                    coordinate,
                    block,
                    location.clone(),
                );
                this.select_processor_capture(
                    frame.resource_sources.len(),
                    (Some(coordinate), 0),
                    block,
                    location,
                    &mut |this, index, block, location| {
                        this.processor_resource_source(
                            &frame.resource_sources[index],
                            block,
                            location,
                        )
                    },
                    &mut join_resource_sources,
                )
            },
            &mut join_resource_sources,
        )
    }

    pub(super) fn processor_element_resources(
        &mut self,
        environment: &LoweredProcessorEnvironment,
        depth: usize,
        resources: &[ProcessorOperationResource],
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Vec<Expr>, MirLoweringError> {
        let frame_at = |depth| {
            let mut frame = environment;
            for _ in 0..depth {
                frame = frame.outer.as_deref().expect("typed resource frame");
            }
            frame
        };
        let frame = frame_at(depth);
        let coordinate = frame.coordinates.last().copied().ok_or_else(|| {
            self.error(
                "owned processor array lost its selected row",
                location.clone(),
            )
        })?;
        let coordinate =
            self.processor_view_offset(frame.resource_offset, coordinate, block, location.clone());
        // Operations consume resolved scalar descriptors or contiguous spans,
        // independently of the source view's physical routing representation.
        let shapes = resources
            .iter()
            .map(|resource| {
                let mut shape =
                    frame_at(resource.frame_depth).shape.resources[resource.resource_index].clone();
                shape.mapped = false;
                shape.mapped_len = None;
                shape.alternatives = None;
                shape.binding = None;
                shape
            })
            .collect::<Vec<_>>();
        self.processor_resource_choices(
            coordinate,
            &shapes,
            frame.resource_sources.len(),
            block,
            location.clone(),
            &mut |this, index, branch| {
                let source = &frame.resource_sources[index];
                let lower = &mut |this: &mut Self, variant: usize, branch: &mut MirBlock| {
                    let mut selected = environment.clone();
                    let mut frame = &mut selected;
                    for _ in 0..depth {
                        frame = frame.outer.as_deref_mut().expect("typed resource frame");
                    }
                    frame.resource_sources.clear();
                    frame.shape.resource_sources.clear();
                    frame.outer = Some(Box::new(source.environments[variant].clone()));
                    frame.shape.outer = Some(Box::new(source.shape.environments[variant].clone()));
                    this.processor_operation_resources(
                        &selected,
                        resources,
                        branch,
                        location.clone(),
                    )
                };
                if let Some(tag) = source.tag {
                    this.processor_resource_choices(
                        tag,
                        &shapes,
                        source.environments.len(),
                        branch,
                        location.clone(),
                        lower,
                    )
                } else {
                    lower(this, 0, branch)
                }
            },
        )
    }
}

fn join_resource_sources(
    this: &mut FunctionLowerer<'_>,
    a: &LoweredProcessorView,
    b: &LoweredProcessorView,
    then_block: &mut MirBlock,
    else_block: &mut MirBlock,
    location: SourceLoc,
) -> Result<LoweredProcessorView, MirLoweringError> {
    Ok(this
        .join_processor_views(Some(a), Some(b), then_block, else_block, location)?
        .expect("resource sources have processor views"))
}
