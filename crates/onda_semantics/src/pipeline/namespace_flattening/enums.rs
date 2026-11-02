//! Resolve nominal enum declarations and named scalar uses during namespace
//! processing, before ordinary scalar checking and specialization.
use super::*;

pub(super) fn register_members(def: &EnumDef, namespace: &str, state: &mut NamespaceFlattenState) {
    let name = namespace_join(namespace, &def.name);
    state.members.insert(name.clone());
    for member in &def.members {
        let name = namespace_join(&name, &member.name);
        state.members.insert(name.clone());
        state.scalar_const_names.insert(name);
    }
}

/// Template checking and concrete emission use disjoint nominal identities.
pub(super) fn allocate_type(
    def: &EnumDef,
    state: &mut NamespaceFlattenState,
    errors: &mut Vec<Diagnostic>,
) -> Option<EnumType> {
    let count = match u32::try_from(def.members.len()) {
        Ok(count) if count > 0 && count <= i32::MAX as u32 => count,
        _ => {
            errors.push(Diagnostic::semantic_span(
                "an enum requires 1 through 2147483647 members",
                &def.loc,
            ));
            return None;
        }
    };
    let id = state.next_enum_id;
    let Some(next) = id.checked_add(1) else {
        errors.push(Diagnostic::semantic_span("too many enum types", &def.loc));
        return None;
    };
    state.next_enum_id = next;
    Some(EnumType { id, count })
}

pub(super) fn emit(
    mut def: EnumDef,
    options: AnalysisOptions,
    state: &mut NamespaceFlattenState,
    out: &mut Vec<Block>,
    errors: &mut Vec<Diagnostic>,
) {
    if state.enums.contains_key(&def.name) {
        errors.push(Diagnostic::semantic_span(
            format!("duplicate enum '{}'", def.name),
            &def.loc,
        ));
        return;
    }
    let Some(ty) = allocate_type(&def, state, errors) else {
        return;
    };
    state.enums.insert(def.name.clone(), ty);
    register_members(&def, "", state);
    def.resolved = Some(ty);
    for (ordinal, member) in def.members.iter().enumerate() {
        let decl = ConstDecl {
            loc: member.loc.clone(),
            name: namespace_join(&def.name, &member.name),
            ty: Some(ConstType::Scalar(PrimitiveType::Enum(ty).into())),
            expr: Expr::Int {
                loc: member.loc.clone(),
                value: ordinal as i64,
                const_ty: Some(PrimitiveType::Enum(ty)),
            },
            configurable: false,
        };
        register_const_decl_artifact(&decl, options, state, errors);
        out.push(Block::Const(decl));
    }
    out.push(Block::Enum(def));
}

pub(super) fn decl_type(ty: &mut DeclType, enums: impl Fn(&str) -> Option<PrimitiveType> + Copy) {
    match ty {
        DeclType::Generic(name) => {
            if let Some(value) = enums(name) {
                *ty = DeclType::Scalar(value);
            }
        }
        DeclType::ArrayGeneric { elem, size } => {
            if let Some(value) = enums(elem) {
                *ty = DeclType::Array {
                    elem: value,
                    size: size.clone(),
                };
            }
        }
        DeclType::Slice(elem) => array_element(elem, enums),
        DeclType::Tuple(types) => types.iter_mut().for_each(|ty| scalar_ref(ty, enums)),
        _ => {}
    }
}

pub(super) fn field_type(ty: &mut FieldType, enums: impl Fn(&str) -> Option<PrimitiveType> + Copy) {
    match ty {
        FieldType::Generic(name) => {
            if let Some(value) = enums(name) {
                *ty = FieldType::Scalar(value);
            }
        }
        FieldType::Array(spec) => array_element(&mut spec.elem, enums),
        FieldType::Tuple(types) => types.iter_mut().for_each(|ty| scalar_ref(ty, enums)),
        _ => {}
    }
}

pub(super) fn param_type(
    ty: &mut FnParamType,
    enums: impl Fn(&str) -> Option<PrimitiveType> + Copy,
) {
    match ty {
        FnParamType::Struct(name) => {
            if let Some(value) = enums(name) {
                *ty = FnParamType::Primitive(value);
            }
        }
        FnParamType::ArrayGeneric(name) => {
            if let Some(value) = enums(name) {
                *ty = FnParamType::Array(Some(value));
            }
        }
        FnParamType::SizedArray {
            elem, generic_name, ..
        } => {
            if let Some(value) = generic_name.as_deref().and_then(enums) {
                *elem = Some(value);
                *generic_name = None;
            }
        }
        FnParamType::Tuple(types) => types.iter_mut().for_each(|ty| scalar_ref(ty, enums)),
        _ => {}
    }
}

pub(super) fn scalar_ref(
    ty: &mut ScalarTypeRef,
    enums: impl Fn(&str) -> Option<PrimitiveType> + Copy,
) {
    if let ScalarTypeRef::Named(name) = ty {
        if let Some(value) = enums(name) {
            *ty = ScalarTypeRef::Primitive(value);
        }
    }
}

pub(super) fn return_type(
    ty: &mut FnReturnType,
    enums: impl Fn(&str) -> Option<PrimitiveType> + Copy,
) {
    match ty {
        FnReturnType::Scalar(ty) | FnReturnType::Array { elem: ty, .. } => scalar_ref(ty, enums),
        FnReturnType::Tuple(types) => types.iter_mut().for_each(|ty| scalar_ref(ty, enums)),
    }
}

pub(super) fn event_type(
    ty: &mut EventParamType,
    enums: impl Fn(&str) -> Option<PrimitiveType> + Copy,
) {
    match ty {
        EventParamType::GenericScalar { name } => {
            if let Some(value) = enums(name) {
                *ty = EventParamType::Scalar(value);
            }
        }
        EventParamType::GenericArray { elem, size } => {
            if let Some(value) = enums(elem) {
                *ty = EventParamType::Array {
                    elem: value,
                    size: size.clone(),
                };
            }
        }
        EventParamType::GenericSlice { elem } => {
            if let Some(value) = enums(elem) {
                *ty = EventParamType::Slice { elem: value };
            }
        }
        EventParamType::Tuple(types) => types.iter_mut().for_each(|ty| scalar_ref(ty, enums)),
        _ => {}
    }
}

fn array_element(elem: &mut ArrayElemType, enums: impl Fn(&str) -> Option<PrimitiveType> + Copy) {
    if let ArrayElemType::Struct(name) = elem {
        if let Some(value) = enums(name) {
            *elem = ArrayElemType::Primitive(value);
        }
    }
}

pub(super) fn rewrite_expression(
    expr: &mut Expr,
    enums: impl Fn(&str) -> Option<PrimitiveType> + Copy,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::ArrayCtor { spec, .. } => array_element(&mut spec.elem, enums),
        Expr::UserCall {
            name,
            args,
            type_args,
            loc,
        } => {
            if let Some(ty) = enums(name) {
                if args.is_empty() && type_args.is_empty() {
                    *expr = Expr::Int {
                        loc: loc.clone(),
                        value: 0,
                        const_ty: Some(ty),
                    };
                } else if args.len() != 1 || args[0].name.is_some() || !type_args.is_empty() {
                    errors.push(Diagnostic::semantic_span(
                        "enum conversion requires one positional integer argument",
                        loc.as_ref(),
                    ));
                } else {
                    *expr = Expr::Cast {
                        loc: loc.clone(),
                        to: ty,
                        expr: Box::new(args.remove(0).expr),
                    };
                }
            }
        }
        _ => {}
    }
}

pub(super) fn rewrite_const_expr(
    expr: &mut Expr,
    resolve: impl Fn(&str) -> Option<PrimitiveType> + Copy,
    errors: &mut Vec<Diagnostic>,
) {
    expr.visit_mut(|expr| {
        rewrite_expression(expr, resolve, errors);
        true
    });
}

pub(super) fn rewrite_const_function(
    def: &mut FunctionDef,
    resolve: impl Fn(&str) -> Option<PrimitiveType> + Copy,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        if let Some(ty) = &mut param.ty {
            param_type(ty, resolve);
            if let FnParamType::SizedArray { size, .. } = ty {
                rewrite_const_expr(size, resolve, errors);
            }
        }
        if let Some(default) = &mut param.default {
            rewrite_const_expr(default, resolve, errors);
        }
    }
    if let Some(ty) = &mut def.return_ty {
        return_type(ty, resolve);
        if let FnReturnType::Array { size, .. } = ty {
            rewrite_const_expr(size, resolve, errors);
        }
    }
    for stmt in &mut def.body {
        stmt.visit_statements_mut(|stmt| {
            if let Stmt::Assign {
                decl_ty,
                generic_decl_ty,
                ..
            } = stmt
            {
                if let Some(ty) = generic_decl_ty.as_deref().and_then(resolve) {
                    *decl_ty = Some(DeclType::Scalar(ty));
                    *generic_decl_ty = None;
                }
                if let Some(ty) = decl_ty {
                    decl_type(ty, resolve);
                    if let DeclType::Array { size, .. } | DeclType::ArrayGeneric { size, .. } = ty {
                        rewrite_const_expr(size, resolve, errors);
                    }
                }
            }
        });
        stmt.visit_exprs_mut(|expr| rewrite_const_expr(expr, resolve, errors));
    }
}
