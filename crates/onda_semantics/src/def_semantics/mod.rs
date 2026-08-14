pub(crate) mod body_analysis;
pub(crate) mod call_types;
pub(crate) mod inference;
mod monomorphization;
mod overloads;

pub(crate) use body_analysis::*;
pub(crate) use call_types::{
    const_positive_usize_for_call_type, infer_scalar_expr_type as infer_call_scalar_expr_type,
    signature_has_dependent_call_types, signature_requires_monomorphization, CallArrayType,
    CallTypeContext, CallTypeEnv,
};
pub(crate) use inference::*;
pub(crate) use monomorphization::{
    monomorphize_calls_in_function, monomorphize_calls_in_stmts,
    refresh_monomorphized_return_types, MonoOwnerContext, MonoParamKey,
};
pub(crate) use overloads::{
    prepare_function_overloads, rewrite_overloaded_calls_in_expr,
    rewrite_overloaded_calls_in_stmt_list, OverloadCandidate, OverloadOwnerContext,
};
