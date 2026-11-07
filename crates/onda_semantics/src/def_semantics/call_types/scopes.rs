//! One lexical-scope traversal for call binding and specialization.
use super::{
    join_branch_envs, update_call_type_env_after_assign, CallTypeContext, CallTypeEnv,
    StatementFlow,
};
use crate::{with_stmt_diag_context_mut, Expr, Stmt};

/// Expression rewrites may discover return types. Subsequent bindings query the
/// updated context, so overload and instance requests share the same scope rules.
pub(crate) trait CallTypeRewriter {
    fn context(&self) -> CallTypeContext<'_>;
    fn rewrite_expr(&mut self, expr: &mut Expr, env: &CallTypeEnv);

    fn expand_assignment(
        &mut self,
        _stmt: &mut Stmt,
        _env: &CallTypeEnv,
        _temp_counter: &mut usize,
    ) -> Option<Vec<Stmt>> {
        None
    }

    fn rewrite_statement_expr(&mut self, expr: &mut Expr, env: &CallTypeEnv) {
        self.rewrite_expr(expr, env);
    }

    fn rewrite_function_resource_recipes(&mut self, function: &str, env: &CallTypeEnv) {
        super::resource_recipes::rewrite_resource_recipes(Some(function), env, |body, env| {
            self.rewrite_stmts(body, env);
        });
    }

    fn rewrite_stmts(&mut self, stmts: &mut Vec<Stmt>, env: &mut CallTypeEnv) -> StatementFlow {
        let mut temp_counter = crate::proc_call_rewrite::projection_temp_counter(stmts);
        self.rewrite_stmts_inner(stmts, env, &mut temp_counter)
    }

    fn rewrite_stmts_inner(
        &mut self,
        stmts: &mut Vec<Stmt>,
        env: &mut CallTypeEnv,
        temp_counter: &mut usize,
    ) -> StatementFlow {
        let mut pending = std::collections::VecDeque::from(std::mem::take(stmts));
        while let Some(mut stmt) = pending.pop_front() {
            if matches!(stmt, Stmt::Assign { .. }) {
                if let Some(expanded) = self.expand_assignment(&mut stmt, env, temp_counter) {
                    for statement in expanded.into_iter().rev() {
                        pending.push_front(statement);
                    }
                    continue;
                }
            }
            let flow = with_stmt_diag_context_mut(&mut stmt, |_diag, stmt| match stmt {
                Stmt::Assign {
                    target,
                    decl_ty,
                    generic_decl_ty,
                    expr,
                    ..
                } => {
                    if let crate::AssignTarget::Index { base, index } = target {
                        super::normalize_tuple_index(base, index, env, self.context());
                    }
                    target.visit_selectors_mut(|selector| self.rewrite_expr(selector, env));
                    self.rewrite_expr(expr, env);
                    update_call_type_env_after_assign(
                        target,
                        decl_ty.as_ref(),
                        generic_decl_ty.as_deref(),
                        expr,
                        env,
                        self.context(),
                    );
                    StatementFlow::Continues
                }
                Stmt::Expr { expr, .. } => {
                    self.rewrite_statement_expr(expr, env);
                    StatementFlow::Continues
                }
                Stmt::Return { expr, .. } => {
                    self.rewrite_expr(expr, env);
                    StatementFlow::Terminates
                }
                Stmt::Print { values, .. } => {
                    for value in values {
                        self.rewrite_expr(value, env);
                    }
                    StatementFlow::Continues
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.rewrite_expr(cond, env);
                    let mut then_env = env.clone();
                    let then_flow =
                        self.rewrite_stmts_inner(then_branch, &mut then_env, temp_counter);
                    let mut else_env = env.clone();
                    let else_flow =
                        self.rewrite_stmts_inner(else_branch, &mut else_env, temp_counter);
                    let (joined, flow) = join_branch_envs(then_env, then_flow, else_env, else_flow);
                    *env = joined;
                    flow
                }
                Stmt::For {
                    var,
                    var_ty,
                    start,
                    end,
                    step,
                    body,
                    ..
                } => {
                    self.rewrite_expr(start, env);
                    self.rewrite_expr(end, env);
                    if let Some(step) = step {
                        self.rewrite_expr(step, env);
                    }
                    let mut body_env = env.clone();
                    body_env.shadow_binding(var);
                    body_env.scalar_types.insert(var.clone(), *var_ty);
                    self.rewrite_stmts_inner(body, &mut body_env, temp_counter);
                    env.join_processor_bindings_from(&body_env);
                    StatementFlow::Continues
                }
                Stmt::While { cond, body, .. } => {
                    self.rewrite_expr(cond, env);
                    let mut body_env = env.clone();
                    self.rewrite_stmts_inner(body, &mut body_env, temp_counter);
                    env.join_processor_bindings_from(&body_env);
                    StatementFlow::Continues
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => StatementFlow::Terminates,
            });
            stmts.push(stmt);
            if flow == StatementFlow::Terminates {
                stmts.extend(pending);
                return flow;
            }
        }
        StatementFlow::Continues
    }
}
