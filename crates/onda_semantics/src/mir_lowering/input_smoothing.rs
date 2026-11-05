//! Install the input caches after their ordinary smoother calls have run. Source
//! typing still sees the original read-only input surface, including array views.
use super::*;
use crate::StatementFlow;

impl FunctionLowerer<'_> {
    pub(super) fn lower_sample_with_input_smoothing(
        &mut self,
        sample: &[Stmt],
        block: &mut MirBlock,
    ) -> Result<StatementFlow, MirLoweringError> {
        let aliases = &self
            .runtime_globals
            .expect("sample has runtime globals")
            .input_smoothing_aliases;
        if aliases.is_empty() {
            return self.lower_statements(sample, block, ContinueMode::None);
        }
        let cache_names: HashSet<_> = aliases.values().map(String::as_str).collect();
        let end = sample
            .iter()
            .rposition(|stmt| {
                matches!(stmt,
                    Stmt::Assign { target: AssignTarget::Var(name), .. }
                        if cache_names.contains(name.as_str())
                )
            })
            .ok_or_else(|| self.error("input smoothing prelude is missing", SourceLoc::ZERO))?
            + 1;
        self.lower_statements(&sample[..end], block, ContinueMode::None)?;
        let mut previous = Vec::new();
        for (name, alias) in aliases {
            let binding = self.bindings.get(alias).cloned().ok_or_else(|| {
                self.error(
                    format!("input smoothing cache '{alias}' is missing"),
                    SourceLoc::ZERO,
                )
            })?;
            let globals = self.runtime_globals.expect("sample has runtime globals");
            let input = globals
                .inputs
                .get(name)
                .map(|(input, _)| *input)
                .or_else(|| globals.input_arrays.get(name).map(|(input, _, _)| *input))
                .ok_or_else(|| {
                    self.error(
                        format!("smoothed input '{name}' is missing"),
                        SourceLoc::ZERO,
                    )
                })?;
            self.smoothed_input_endpoints.insert(input, binding.clone());
            previous.push((name.clone(), self.bindings.insert(name.clone(), binding)));
        }
        let result = self.lower_statements(&sample[end..], block, ContinueMode::None);
        for (name, binding) in previous {
            if let Some(binding) = binding {
                self.bindings.insert(name, binding);
            } else {
                self.bindings.remove(&name);
            }
        }
        self.smoothed_input_endpoints.clear();
        result
    }
}
