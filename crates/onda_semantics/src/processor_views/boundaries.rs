//! Capture checked operation views using the ordinary lexical scope walk.
use super::*;
use crate::def_semantics::call_types::{CallTypeContext, CallTypeEnv, CallTypeRewriter};
use crate::Stmt;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProcessorBoundary {
    Init,
    BlockPre,
    Sample,
    BlockPost,
    Event(usize),
}

pub(crate) fn record_function_operations(
    function: &mut onda_frontend::FunctionDef,
    seed: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Vec<(String, ProcessorViewShape)> {
    let mut env = seed.clone();
    env.enter_function(&function.type_params);
    for param in &function.params {
        env.bind_function_param(param, &function.type_params);
    }
    env.bind_processor_function_params(&function.name);
    record_boundary_operations(&mut function.body, &mut env, context)
}

pub(crate) fn record_boundary_operations(
    body: &mut [Stmt],
    env: &mut CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Vec<(String, ProcessorViewShape)> {
    struct Recorder<'a> {
        context: CallTypeContext<'a>,
        operations: BTreeSet<(String, ProcessorViewShape)>,
    }
    impl CallTypeRewriter for Recorder<'_> {
        fn context(&self) -> CallTypeContext<'_> {
            self.context
        }

        fn rewrite_expr(&mut self, expression: &mut Expr, env: &CallTypeEnv) {
            for node in expression.walk() {
                let Expr::UserCall { name, args, .. } = node else {
                    continue;
                };
                if env.processor_views.borrow().operation(name).is_none() {
                    continue;
                }
                if let Some(shape) = args
                    .first()
                    .and_then(|arg| env.processor_view_shape(&arg.expr))
                {
                    self.operations.insert((name.clone(), shape));
                }
            }
        }
    }
    let mut recorder = Recorder {
        context,
        operations: BTreeSet::new(),
    };
    recorder.rewrite_stmts(body, env);
    recorder.operations.into_iter().collect()
}
