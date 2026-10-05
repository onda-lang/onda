//! The concrete configuration inherited by a runtime call.
use crate::{AnalysisOptions, Expr};
use std::collections::HashMap;

/// The declaration identity shared by concrete runtime-context variants.
pub(crate) fn source_function_name(name: &str) -> &str {
    name.split_once(".__ctx_sr_").map_or(name, |(name, _)| name)
}

/// Scheduling uses the processor operator's declaration identity; executable
/// lookup continues to use the full concrete argument and context variants.
pub(crate) fn lowered_function_origin(name: &str) -> &str {
    let name = source_function_name(name);
    name.split_once(".__onda_mono__")
        .map_or(name, |(name, _)| name)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub(crate) struct CompileContext {
    sample_rate_bits: u32,
    block_size: usize,
}

impl CompileContext {
    pub(crate) fn new(options: AnalysisOptions) -> Self {
        Self {
            sample_rate_bits: options.sample_rate.to_bits(),
            block_size: options.block_size,
        }
    }

    pub(crate) fn options(self, host: AnalysisOptions) -> AnalysisOptions {
        AnalysisOptions {
            sample_rate: f32::from_bits(self.sample_rate_bits),
            block_size: self.block_size,
            ..host
        }
    }

    pub(crate) fn config(self) -> onda_mir::CompileConfig {
        onda_mir::CompileConfig {
            sample_rate: f32::from_bits(self.sample_rate_bits),
            block_size: self.block_size as u32,
        }
    }

    pub(crate) fn specialized_name(self, name: &str) -> String {
        format!(
            "{name}.__ctx_sr_{:08x}_bs_{:08x}",
            self.sample_rate_bits, self.block_size
        )
    }
}

pub(crate) struct CallContexts<'a> {
    pub(crate) host: AnalysisOptions,
    pub(crate) functions: &'a HashMap<String, usize>,
    pub(crate) instances: &'a HashMap<String, usize>,
}

impl CallContexts<'_> {
    pub(crate) fn callee(
        &self,
        name: &str,
        receiver: Option<&Expr>,
        caller: CompileContext,
    ) -> CompileContext {
        let factor = self
            .functions
            .get(name)
            .copied()
            .filter(|factor| *factor > 1);
        let instance_factor = name
            .contains(".__onda_proc_")
            .then(|| {
                receiver.and_then(|receiver| match receiver {
                    Expr::Var { name, .. } => self.instances.get(name).copied(),
                    Expr::Index { base, index, .. } => {
                        let slot = match index.as_ref() {
                            Expr::Int { value, .. } => {
                                self.instances.get(&format!("{base}[{value}]"))
                            }
                            _ => None,
                        };
                        slot.or_else(|| self.instances.get(base)).copied()
                    }
                    _ => None,
                })
            })
            .flatten()
            .filter(|factor| *factor > 1);
        factor.or(instance_factor).map_or(caller, |factor| {
            CompileContext::new(crate::processor_lowering::proc_runtime_analysis_options(
                self.host, factor,
            ))
        })
    }
}
