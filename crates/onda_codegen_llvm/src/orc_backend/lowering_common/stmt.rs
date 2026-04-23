use super::*;

pub(in crate::orc_backend) trait SharedNonAssignStmtBackend {
    type Output: Copy;

    fn const_stmt_result(&self) -> Self::Output;

    unsafe fn lower_expr_stmt(&mut self, expr: &Expr) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_return_stmt(&mut self, expr: &Expr) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_if_stmt(
        &mut self,
        cond: &Expr,
        then_branch: &[Stmt],
        else_branch: &[Stmt],
    ) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_for_stmt(
        &mut self,
        var: &str,
        step: Option<&Expr>,
        start: &Expr,
        end: &Expr,
        end_inclusive: bool,
        body: &[Stmt],
    ) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_while_stmt(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
    ) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_break_stmt(&mut self) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_continue_stmt(&mut self) -> Result<Self::Output, Diagnostic>;
}

pub(in crate::orc_backend) trait SharedStmtBackend:
    SharedNonAssignStmtBackend
{
    unsafe fn lower_var_assign(
        &mut self,
        target_name: &str,
        decl_ty: Option<PrimitiveType>,
        is_typed_decl: bool,
        expr: &Expr,
    ) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_index_assign(
        &mut self,
        base: &str,
        index: &Expr,
        expr: &Expr,
    ) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_slice_assign(
        &mut self,
        base: &str,
        start: Option<&Expr>,
        end: Option<&Expr>,
        expr: &Expr,
    ) -> Result<Self::Output, Diagnostic>;
    unsafe fn lower_tuple_destructure(
        &mut self,
        targets: &[String],
        expr: &Expr,
    ) -> Result<Self::Output, Diagnostic>;
}

pub(in crate::orc_backend) unsafe fn lower_non_assign_stmt_common<B: SharedNonAssignStmtBackend>(
    backend: &mut B,
    stmt: &Stmt,
) -> Result<B::Output, Diagnostic> {
    match stmt {
        Stmt::Const { .. } => Ok(backend.const_stmt_result()),
        Stmt::Expr { expr, .. } => backend.lower_expr_stmt(expr),
        Stmt::Return { expr, .. } => backend.lower_return_stmt(expr),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => backend.lower_if_stmt(cond, then_branch, else_branch),
        Stmt::For {
            var,
            step,
            start,
            end,
            end_inclusive,
            body,
            ..
        } => backend.lower_for_stmt(var, step.as_ref(), start, end, *end_inclusive, body),
        Stmt::While { cond, body, .. } => backend.lower_while_stmt(cond, body),
        Stmt::Break { .. } => backend.lower_break_stmt(),
        Stmt::Continue { .. } => backend.lower_continue_stmt(),
        Stmt::Assign { .. } => unreachable!(
            "assignment statements are lowered outside the shared non-assign dispatcher"
        ),
    }
}

pub(in crate::orc_backend) unsafe fn lower_stmt_common<B: SharedStmtBackend>(
    backend: &mut B,
    stmt: &Stmt,
) -> Result<B::Output, Diagnostic> {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            is_typed_decl,
            expr,
            ..
        } => match target {
            AssignTarget::Var(target_name) => {
                backend.lower_var_assign(target_name, *decl_ty, *is_typed_decl, expr)
            }
            AssignTarget::Index { base, index } => backend.lower_index_assign(base, index, expr),
            AssignTarget::Slice { base, start, end } => {
                backend.lower_slice_assign(base, start.as_ref(), end.as_ref(), expr)
            }
            AssignTarget::Tuple(targets) => backend.lower_tuple_destructure(targets, expr),
        },
        _ => lower_non_assign_stmt_common(backend, stmt),
    }
}
