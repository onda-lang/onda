//! Concrete const-call checks depend only on argument types and shapes.
//! A heap worklist checks transitive calls without growing the worker stack.
use super::*;
use std::rc::Rc;

pub(super) type PendingConstCalls =
    Rc<RefCell<Vec<(Rc<ConstDefinition>, Vec<ConstArrayExpectation>)>>>;

pub(super) fn const_call_metadata(
    def: &ConstDefinition,
    args: &[Option<&Expr>],
    env: ExprEnv<'_>,
) -> Option<Vec<ConstArrayExpectation>> {
    let params = def.param_kinds.as_ref()?;
    if !params
        .iter()
        .any(|param| matches!(param, ConstDefParamKind::Slice { .. }))
    {
        return Some(Vec::new());
    }
    let mut defaults = None;
    let arrays = params
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let ConstDefParamKind::Slice { elem_ty } = kind else {
                return ConstArrayExpectation::any();
            };
            let (arg, env) = match args[index] {
                Some(arg) => (Some(arg), env),
                None => (
                    def.signature.defaults[index].as_ref(),
                    defaults
                        .get_or_insert_with(|| {
                            ConstCheck::capture(&def.environment.values, &def.environment.defs)
                        })
                        .env(),
                ),
            };
            let info = arg.and_then(|arg| infer_array_value_type(arg, env));
            let mut actual = info.as_ref().and_then(|(elem, _)| match elem {
                Some(ArrayElemType::Primitive(ty)) => Some(*ty),
                _ => None,
            });
            if let Some(Expr::ArrayLiteral { values, .. }) = arg {
                if let Some(first) = values.first() {
                    actual = effective_untyped_assignment_type(first, actual, env.declared_symbols);
                }
            }
            ConstArrayExpectation {
                elem_ty: elem_ty.or(actual),
                len: info.and_then(|(_, len)| len),
            }
        })
        .collect::<Vec<_>>();
    Some(arrays)
}

pub(super) fn validate_const_call(
    def: &ConstDefinition,
    args: &[Option<&Expr>],
    env: crate::expr_analysis::ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(metadata) = const_call_metadata(def, args, env) {
        validate_const_call_metadata(def, &metadata, errors);
    }
}

enum Definition<'a> {
    Borrowed(&'a ConstDefinition),
    Shared(Rc<ConstDefinition>),
}

impl std::ops::Deref for Definition<'_> {
    type Target = ConstDefinition;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(def) => def,
            Self::Shared(def) => def,
        }
    }
}

enum Frame<'a> {
    Check(Definition<'a>, Vec<ConstArrayExpectation>),
    Finish(
        Definition<'a>,
        Vec<ConstArrayExpectation>,
        Rc<ConstBodyTypes>,
        Vec<Diagnostic>,
        usize,
    ),
}

pub(super) fn validate_const_call_metadata(
    def: &ConstDefinition,
    arrays: &[ConstArrayExpectation],
    errors: &mut Vec<Diagnostic>,
) -> Option<Rc<ConstBodyTypes>> {
    // Cache hits, including failures, require no traversal or allocation.
    if let Some(result) = def.type_checks.borrow().get(arrays) {
        if let Err(diagnostics) = result {
            append_const_diagnostics(errors, diagnostics);
        }
        return result.as_ref().ok().cloned();
    }
    let mut frames = vec![Frame::Check(Definition::Borrowed(def), arrays.to_vec())];
    let mut results = Vec::<Vec<Diagnostic>>::new();
    while let Some(frame) = frames.pop() {
        match frame {
            Frame::Check(def, metadata) => {
                if let Some(result) = def.type_checks.borrow().get(&metadata) {
                    results.push(result.as_ref().err().cloned().unwrap_or_default());
                    continue;
                }
                let (Some(params), Some(result)) = (&def.param_kinds, def.result) else {
                    return None;
                };
                let mut check = ConstCheck::capture(&def.environment.values, &def.environment.defs);
                let pending = check.defer_calls();
                let mut diagnostics = Vec::new();
                check.collect_body_types();
                check.body(&def, params, result, Some(&metadata), &mut diagnostics);
                let types = check.into_body_types();
                frames.push(Frame::Finish(
                    def,
                    metadata,
                    types,
                    diagnostics,
                    results.len(),
                ));
                frames.extend(
                    pending
                        .borrow_mut()
                        .drain(..)
                        .rev()
                        .map(|(def, metadata)| Frame::Check(Definition::Shared(def), metadata)),
                );
            }
            Frame::Finish(def, metadata, types, mut diagnostics, base) => {
                for child in results.drain(base..) {
                    append_const_diagnostics(&mut diagnostics, &child);
                }
                let result = if diagnostics.is_empty() {
                    Ok(types)
                } else {
                    Err(diagnostics.clone())
                };
                def.type_checks.borrow_mut().insert(metadata, result);
                results.push(diagnostics);
            }
        }
    }
    let diagnostics = results.pop().expect("const call check result");
    debug_assert!(results.is_empty());
    append_const_diagnostics(errors, &diagnostics);
    def.type_checks.borrow().get(arrays)?.as_ref().ok().cloned()
}
