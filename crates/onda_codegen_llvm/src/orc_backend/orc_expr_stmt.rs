use super::*;
mod expr_lowering;
mod stmt_lowering;

pub(super) unsafe fn lower_expr(
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &HashMap<String, Vec<PrimitiveType>>,
) -> Result<OrcValue, Diagnostic> {
    expr_lowering::lower_expr(
        expr,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )
}

pub(super) fn orc_user_call_context<'a>(ctx: &LoweringCtx<'a>) -> UserCallSharedContext<'a> {
    UserCallSharedContext {
        module: ctx.module,
        context: ctx.context,
        float_ty: ctx.float_ty,
        sample_rate: ctx.sample_rate,
        block_size: ctx.block_size,
        fast_math: ctx.fast_math_flags != LLVMFastMathNone,
        struct_fields: ctx.struct_fields,
        user_fn_param_names: ctx.user_fn_param_names,
        user_fn_param_defaults: ctx.user_fn_param_defaults,
        user_fn_param_kinds: ctx.user_fn_param_kinds,
        user_fn_param_by_ref: ctx.user_fn_param_by_ref,
        user_registry: ctx.user_registry as *mut UserFnRegistry,
        builder: ctx.builder,
    }
}

pub(super) unsafe fn lower_stmt(
    stmt: &Stmt,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    stmt_lowering::lower_stmt(
        stmt,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )
}
