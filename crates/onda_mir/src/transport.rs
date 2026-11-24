use crate::loops::ordered_reductions::{collect, OrderedReduction};
use crate::{OptimizedProgram, Program};
use serde::Serialize;

/// Both transports use the executable program's derived schema. Only optimized
/// programs with proven process loops add the sparse table of backend hints.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum OptimizedTransport<'a> {
    Plain(&'a Program),
    WithReductions {
        #[serde(flatten)]
        program: &'a Program,
        ordered_reductions: Vec<OrderedReduction>,
    },
}

impl<'a> OptimizedTransport<'a> {
    pub(crate) fn new(optimized: &'a OptimizedProgram) -> Self {
        let ordered_reductions = collect(optimized);
        let program = optimized.as_program();
        if ordered_reductions.is_empty() {
            Self::Plain(program)
        } else {
            Self::WithReductions {
                program,
                ordered_reductions,
            }
        }
    }
}
