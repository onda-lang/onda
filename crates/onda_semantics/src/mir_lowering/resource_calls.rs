//! Balanced direct calls for genuinely distinct authored resource bindings.
//!
//! Uniform processors never use this selection. Arguments and the receiver
//! coordinate have already been evaluated once before entering this module.

use super::*;

impl FunctionLowerer<'_> {
    pub(super) fn push_resource_selected_call(
        &mut self,
        block: &mut MirBlock,
        function: FunctionId,
        results: Vec<LocalId>,
        args: Vec<CallArgument>,
        selection: Option<ResourceCallSelection>,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let Some(selection) = selection else {
            self.push_statement(
                block,
                StatementKind::Call {
                    results,
                    function,
                    args,
                },
                location,
            );
            return Ok(());
        };
        if selection.choices == 0
            || selection
                .arguments
                .iter()
                .any(|(index, choices)| *index >= args.len() || choices.len() != selection.choices)
        {
            return Err(self.error(
                "resource call selection has inconsistent argument alternatives",
                location,
            ));
        }
        let calls = (0..selection.choices)
            .map(|choice| {
                let mut selected = args.clone();
                for (index, alternatives) in &selection.arguments {
                    selected[*index] = alternatives[choice].clone();
                }
                selected
            })
            .collect::<Vec<_>>();
        let selected =
            self.resource_call_selection(selection.index, function, &results, &calls, 0, location);
        block.statements.extend(selected.statements);
        Ok(())
    }

    fn resource_call_selection(
        &mut self,
        index: LocalId,
        function: FunctionId,
        results: &[LocalId],
        calls: &[Vec<CallArgument>],
        first: usize,
        location: SourceLoc,
    ) -> MirBlock {
        let mut block = MirBlock::default();
        if calls.len() == 1 {
            self.push_statement(
                &mut block,
                StatementKind::Call {
                    results: results.to_vec(),
                    function,
                    args: calls[0].clone(),
                },
                location,
            );
            return block;
        }
        let mid = calls.len() / 2;
        let condition = self.compare_value(
            &mut block,
            CompareOp::Less,
            Value::Local(index),
            Value::Constant(ScalarValue::I32((first + mid) as i32)),
            location.clone(),
        );
        let then_block = self.resource_call_selection(
            index,
            function,
            results,
            &calls[..mid],
            first,
            location.clone(),
        );
        let else_block = self.resource_call_selection(
            index,
            function,
            results,
            &calls[mid..],
            first + mid,
            location.clone(),
        );
        self.push_statement(
            &mut block,
            StatementKind::If {
                condition,
                then_block,
                else_block,
            },
            location,
        );
        block
    }
}
