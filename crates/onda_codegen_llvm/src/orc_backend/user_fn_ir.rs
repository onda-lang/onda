mod lowering;
mod registry;

pub(super) use lowering::lower_user_function_body;
pub(super) use registry::{build_user_functions_ir, ensure_user_fn_specialization};
