//! Symbolic capture selections survive nested branches without creating
//! branch-local descriptors. Physical references are formed at their use site.
use super::*;
use crate::processor_views::{ProcessorResourceShape, ProcessorStorageShape};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ProcessorCapture<T> {
    Direct(T),
    Choice {
        tag: Value,
        then_capture: Box<Self>,
        else_capture: Box<Self>,
    },
}

impl<T> ProcessorCapture<T> {
    pub(super) fn visit_leaves(&self, visit: &mut impl FnMut(&T)) {
        match self {
            Self::Direct(value) => visit(value),
            Self::Choice {
                then_capture,
                else_capture,
                ..
            } => {
                then_capture.visit_leaves(visit);
                else_capture.visit_leaves(visit);
            }
        }
    }

    pub(super) fn visit_values(&mut self, visit: &mut impl FnMut(&mut Value)) {
        if let Self::Choice {
            tag,
            then_capture,
            else_capture,
        } = self
        {
            visit(tag);
            then_capture.visit_values(visit);
            else_capture.visit_values(visit);
        }
    }
}

impl FunctionLowerer<'_> {
    pub(super) fn publish_processor_capture<T: Clone>(
        &mut self,
        capture: &ProcessorCapture<T>,
        active: &mut MirBlock,
        inactive: &mut MirBlock,
        location: SourceLoc,
    ) -> ProcessorCapture<T> {
        match capture {
            ProcessorCapture::Direct(value) => ProcessorCapture::Direct(value.clone()),
            ProcessorCapture::Choice {
                tag,
                then_capture,
                else_capture,
            } => ProcessorCapture::Choice {
                tag: self.join_processor_coordinate(
                    *tag,
                    Value::Constant(ScalarValue::I32(0)),
                    active,
                    inactive,
                    location.clone(),
                ),
                then_capture: Box::new(self.publish_processor_capture(
                    then_capture,
                    active,
                    inactive,
                    location.clone(),
                )),
                else_capture: Box::new(self.publish_processor_capture(
                    else_capture,
                    active,
                    inactive,
                    location,
                )),
            },
        }
    }

    pub(super) fn join_processor_capture<T: Clone + PartialEq>(
        &mut self,
        a: &ProcessorCapture<T>,
        b: &ProcessorCapture<T>,
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        location: SourceLoc,
    ) -> ProcessorCapture<T> {
        if a == b {
            return a.clone();
        }
        ProcessorCapture::Choice {
            tag: self.join_processor_coordinate(
                Value::Constant(ScalarValue::I32(0)),
                Value::Constant(ScalarValue::I32(1)),
                then_block,
                else_block,
                location.clone(),
            ),
            then_capture: Box::new(self.publish_processor_capture(
                a,
                then_block,
                else_block,
                location.clone(),
            )),
            else_capture: Box::new(
                self.publish_processor_capture(b, else_block, then_block, location),
            ),
        }
    }

    pub(super) fn materialize_processor_storage_capture(
        &mut self,
        capture: &ProcessorCapture<String>,
        storage: &ProcessorStorageShape,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<String, MirLoweringError> {
        let ProcessorCapture::Choice {
            tag,
            then_capture,
            else_capture,
        } = capture
        else {
            let ProcessorCapture::Direct(root) = capture else {
                unreachable!()
            };
            return Ok(root.clone());
        };
        let mut then_block = MirBlock::default();
        let a = self.materialize_processor_storage_capture(
            then_capture,
            storage,
            &mut then_block,
            location.clone(),
        )?;
        let mut else_block = MirBlock::default();
        let b = self.materialize_processor_storage_capture(
            else_capture,
            storage,
            &mut else_block,
            location.clone(),
        )?;
        let root = self.join_processor_storage_root(
            &a,
            &b,
            storage,
            &mut then_block,
            &mut else_block,
            location.clone(),
        )?;
        self.push_processor_capture_choice(*tag, then_block, else_block, block, location);
        Ok(root)
    }

    pub(super) fn materialize_processor_resource_capture(
        &mut self,
        capture: &ProcessorCapture<Expr>,
        shape: &ProcessorResourceShape,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Expr, MirLoweringError> {
        let ProcessorCapture::Choice {
            tag,
            then_capture,
            else_capture,
        } = capture
        else {
            let ProcessorCapture::Direct(expression) = capture else {
                unreachable!()
            };
            return Ok(expression.clone());
        };
        let mut then_block = MirBlock::default();
        let a = self.materialize_processor_resource_capture(
            then_capture,
            shape,
            &mut then_block,
            location.clone(),
        )?;
        let mut else_block = MirBlock::default();
        let b = self.materialize_processor_resource_capture(
            else_capture,
            shape,
            &mut else_block,
            location.clone(),
        )?;
        let mut joined = self.join_processor_resource_expressions(
            &[a],
            &[b],
            std::slice::from_ref(shape),
            &mut then_block,
            &mut else_block,
            location.clone(),
        )?;
        self.push_processor_capture_choice(*tag, then_block, else_block, block, location);
        Ok(joined.remove(0))
    }

    fn push_processor_capture_choice(
        &mut self,
        tag: Value,
        then_block: MirBlock,
        else_block: MirBlock,
        block: &mut MirBlock,
        location: SourceLoc,
    ) {
        let condition = self.compare_value(
            block,
            CompareOp::Equal,
            tag,
            Value::Constant(ScalarValue::I32(0)),
            location.clone(),
        );
        self.push_statement(
            block,
            StatementKind::If {
                condition,
                then_block,
                else_block,
            },
            location,
        );
    }
}
