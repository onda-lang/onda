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
                let mut branch = MirBlock::default();
                self.push_statement(
                    &mut branch,
                    StatementKind::Call {
                        results: results.clone(),
                        function,
                        args: selected,
                    },
                    location.clone(),
                );
                branch
            })
            .collect::<Vec<_>>();
        let selected =
            self.processor_choice_block(Value::Local(selection.index), calls, 0, location);
        block.statements.extend(selected.statements);
        Ok(())
    }
}
