use onda_frontend::{DiagCtx, Diagnostic};

pub(crate) fn push_semantic(
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
    message: impl Into<String>,
) {
    errors.push(diag.semantic(message, 0, 0));
}
