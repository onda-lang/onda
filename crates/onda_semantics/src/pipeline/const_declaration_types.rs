//! Const callable type policy, shared by concrete and dependent declarations.
use super::*;

impl crate::expr_analysis::SignatureLookup for HashMap<String, std::rc::Rc<ConstDefinition>> {
    fn get(&self, name: &str) -> Option<&FnSignature> {
        HashMap::get(self, name).map(|def| &def.signature)
    }
}

pub(super) fn const_def_signature<L: Into<Option<usize>>>(
    def: &FunctionDef,
    result: Option<ConstDefReturn<L>>,
    signatures: &dyn crate::expr_analysis::SignatureLookup,
) -> FnSignature {
    let mut signature = FnSignature::from_def(def);
    signature.defaults_validated = true;
    signature.readonly_data_params =
        super::data_permissions::const_readonly_data_params(def, &signature, signatures);
    signature.return_type = result.and_then(|result| match result {
        ConstDefReturn::Scalar(ty) => Some(ReturnType::Scalar(ty)),
        ConstDefReturn::Array { elem_ty, len } => len.into().map(|len| {
            ReturnType::Data(DataType::Array {
                element: ArrayElemType::Primitive(elem_ty),
                len,
            })
        }),
    });
    signature
}

pub(super) fn const_def_param_signature<L>(
    def: &FunctionDef,
    mut resolve_size: impl FnMut(&Expr, &str, &mut Vec<Diagnostic>) -> Option<L>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<ConstDefParamKind<L>>> {
    let mut out = Vec::with_capacity(def.params.len());
    for param in &def.params {
        match param.ty.as_ref() {
            Some(FnParamType::Primitive(ty)) => out.push(ConstDefParamKind::Scalar(*ty)),
            Some(FnParamType::Array(elem_ty)) => {
                out.push(ConstDefParamKind::Slice { elem_ty: *elem_ty })
            }
            Some(FnParamType::SizedArray {
                elem: Some(elem_ty),
                generic_name: None,
                size,
            }) => {
                let len = resolve_size(
                    size,
                    &format!(
                        "const def '{}' parameter '{}' array size",
                        def.name, param.name
                    ),
                    errors,
                )?;
                out.push(ConstDefParamKind::Array {
                    elem_ty: *elem_ty,
                    len,
                });
            }
            Some(_) => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "const def '{}' parameter '{}' must use a primitive scalar, fixed primitive array, or primitive array slice type",
                        def.name, param.name
                    ),
                    param.ty_loc.or(param.loc),
                ));
                return None;
            }
            None => {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "const def '{}' parameter '{}' must have an explicit primitive scalar, fixed primitive array, or primitive array slice type",
                        def.name, param.name
                    ),
                    param.loc,
                ));
                return None;
            }
        }
    }
    Some(out)
}

pub(super) fn const_def_return_type<L>(
    def: &FunctionDef,
    mut resolve_size: impl FnMut(&Expr, &str, &mut Vec<Diagnostic>) -> Option<L>,
    errors: &mut Vec<Diagnostic>,
) -> Option<ConstDefReturn<L>> {
    match def.return_ty.as_ref() {
        Some(FnReturnType::Scalar(FnReturnScalarType::Primitive(ty))) => {
            Some(ConstDefReturn::Scalar(*ty))
        }
        Some(FnReturnType::Scalar(FnReturnScalarType::Named(name))) => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const def '{}' return type '{}' is not a concrete primitive scalar",
                    def.name, name
                ),
                def.return_ty_loc,
            ));
            None
        }
        Some(FnReturnType::Array { elem, size }) => {
            let FnReturnScalarType::Primitive(elem) = elem else {
                errors.push(Diagnostic::semantic_span(
                    "const def array return requires primitive elements",
                    def.return_ty_loc,
                ));
                return None;
            };
            let len = resolve_size(
                size,
                &format!("const def '{}' return array size", def.name),
                errors,
            )?;
            Some(ConstDefReturn::Array {
                elem_ty: *elem,
                len,
            })
        }
        Some(FnReturnType::Tuple(_)) => {
            errors.push(Diagnostic::semantic_span(
                format!("const def '{}' cannot return a tuple", def.name),
                def.return_ty_loc,
            ));
            None
        }
        None => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "const def '{}' must declare an explicit return type",
                    def.name
                ),
                def.loc,
            ));
            None
        }
    }
}
