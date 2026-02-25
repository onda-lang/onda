use super::*;
mod expr_lowering;
mod stmt_lowering;

pub(super) unsafe fn lower_expr(
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<OrcValue, Diagnostic> {
    expr_lowering::lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)
}

pub(super) unsafe fn lower_stmt(
    stmt: &Stmt,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_data_aliases: &mut HashMap<String, LocalDataAlias>,
) -> Result<(), Diagnostic> {
    stmt_lowering::lower_stmt(stmt, ctx, locals, local_aliases, local_data_aliases)
}
