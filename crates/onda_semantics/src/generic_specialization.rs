use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::def_semantics::call_types::{
    declared_call_return_type, infer_array_arg_type, infer_struct_expr_type, join_branch_envs,
    update_call_type_env_after_assign,
};
use crate::*;
use onda_frontend::ast::{FnReturnScalarType, FnReturnType};

mod proc_specialization;
pub(crate) use proc_specialization::*;

pub(crate) fn primitive_sig_code_for_specialization(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

pub(crate) fn specialized_struct_name(base: &str, type_args: &[PrimitiveType]) -> String {
    if type_args.is_empty() {
        return base.to_owned();
    }
    let sig = type_args
        .iter()
        .map(|t| primitive_sig_code_for_specialization(*t))
        .collect::<Vec<_>>()
        .join("_");
    format!("{base}.__gen__{sig}")
}

pub(crate) fn resolve_explicit_call_type_args(
    type_args: &[CallTypeArg],
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut resolved = Vec::<PrimitiveType>::with_capacity(type_args.len());
    for arg in type_args {
        match arg {
            CallTypeArg::Primitive(ty) => {
                if *ty == PrimitiveType::Bool {
                    push_semantic(
                        diag,
                        errors,
                        format!(
                            "{context}: 'bool' is not allowed as a generic type argument; only numeric types (f32, f64, i32, i64) are supported"
                        ),
                    );
                    return None;
                }
                resolved.push(*ty);
            }
            CallTypeArg::Generic(name) => {
                push_semantic(
                    diag,
                    errors,
                    format!(
                        "{context}: generic type argument '{}' is not allowed here; expected concrete primitive type",
                        name
                    ),
                );
                return None;
            }
        }
    }
    Some(resolved)
}

pub(crate) fn substitute_call_type_args_with_bindings_expr(
    expr: &mut Expr,
    bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    let diag = DiagCtx::new(expr.loc());
    match expr {
        Expr::Index { index, .. } => {
            substitute_call_type_args_with_bindings_expr(index, bindings, context, errors);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                substitute_call_type_args_with_bindings_expr(coordinate, bindings, context, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            substitute_call_type_args_with_bindings_expr(&mut spec.size, bindings, context, errors);
            if let ArrayElemType::Struct(type_name) = &mut spec.elem {
                match specialize_generic_type_name(type_name, bindings, context, diag, errors) {
                    Some(SpecializedTypeName::Primitive(bound)) => {
                        spec.elem = ArrayElemType::Primitive(bound);
                    }
                    Some(SpecializedTypeName::Named(name)) => *type_name = name,
                    None => {}
                }
            }
            if let Some(values) = init {
                for value in values {
                    substitute_call_type_args_with_bindings_expr(value, bindings, context, errors);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            substitute_call_type_args_with_bindings_expr(lhs, bindings, context, errors);
            substitute_call_type_args_with_bindings_expr(rhs, bindings, context, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                substitute_call_type_args_with_bindings_expr(arg, bindings, context, errors);
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            substitute_call_type_args_with_bindings_expr(inner, bindings, context, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                substitute_call_type_args_with_bindings_expr(value, bindings, context, errors);
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            for arg in args.iter_mut() {
                substitute_call_type_args_with_bindings_expr(
                    &mut arg.expr,
                    bindings,
                    context,
                    errors,
                );
            }
            for type_arg in type_args.iter_mut() {
                if let CallTypeArg::Generic(param) = type_arg {
                    let Some(bound) = bindings.get(param).copied() else {
                        push_semantic(
                            diag,
                            errors,
                            format!(
                                "{context}: unknown generic type argument '{}'; not declared in current generic owner",
                                param
                            ),
                        );
                        continue;
                    };
                    *type_arg = CallTypeArg::Primitive(bound);
                }
            }
            if type_args.is_empty() && args.len() == 1 && args[0].name.is_none() {
                if let Some(bound) = bindings.get(name).copied() {
                    let arg_expr = args.remove(0).expr;
                    *expr = Expr::Cast {
                        loc: Default::default(),
                        to: bound,
                        expr: Box::new(arg_expr),
                    };
                }
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(crate) fn expr_references_names(
    expr: &Expr,
    variable: &impl Fn(&str) -> bool,
    ty: &impl Fn(&str) -> bool,
) -> bool {
    match expr {
        Expr::Var { name, .. } => variable(name),
        Expr::Index { index, .. } => expr_references_names(index, variable, ty),
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => [selector, channel, start, end]
            .into_iter()
            .flatten()
            .any(|coordinate| expr_references_names(coordinate, variable, ty)),
        Expr::ArrayCtor { spec, init, .. } => {
            matches!(&spec.elem, ArrayElemType::Struct(name) if ty(name))
                || expr_references_names(&spec.size, variable, ty)
                || init
                    .iter()
                    .flatten()
                    .any(|value| expr_references_names(value, variable, ty))
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            expr_references_names(lhs, variable, ty) || expr_references_names(rhs, variable, ty)
        }
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| expr_references_names(arg, variable, ty)),
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            expr_references_names(expr, variable, ty)
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => values
            .iter()
            .any(|value| expr_references_names(value, variable, ty)),
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            ty(name)
                || type_args
                    .iter()
                    .any(|arg| matches!(arg, CallTypeArg::Generic(name) if ty(name)))
                || args
                    .iter()
                    .any(|arg| expr_references_names(&arg.expr, variable, ty))
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => false,
    }
}

pub(crate) fn substitute_call_type_args_with_bindings_stmt(
    stmt: &mut Stmt,
    bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context_mut(stmt, |_diag, stmt| match stmt {
        Stmt::Const { decl, .. } => {
            substitute_call_type_args_with_bindings_expr(&mut decl.expr, bindings, context, errors);
        }
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            expr,
            ..
        } => {
            if let Some(ty) = decl_ty {
                *ty = specialize_decl_type(ty, bindings, context, _diag, errors);
            }
            if let Some(type_name) = generic_decl_ty.clone() {
                match specialize_generic_type_name(&type_name, bindings, context, _diag, errors) {
                    Some(SpecializedTypeName::Primitive(bound)) => {
                        *decl_ty = Some(DeclType::Scalar(bound));
                        *generic_decl_ty = None;
                    }
                    Some(SpecializedTypeName::Named(name)) => {
                        *generic_decl_ty = Some(name);
                    }
                    None => {}
                }
            }
            target.visit_selectors_mut(|selector| {
                substitute_call_type_args_with_bindings_expr(selector, bindings, context, errors)
            });
            substitute_call_type_args_with_bindings_expr(expr, bindings, context, errors);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            substitute_call_type_args_with_bindings_expr(expr, bindings, context, errors);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                substitute_call_type_args_with_bindings_expr(value, bindings, context, errors);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            substitute_call_type_args_with_bindings_expr(cond, bindings, context, errors);
            for nested in then_branch {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
            for nested in else_branch {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            substitute_call_type_args_with_bindings_expr(start, bindings, context, errors);
            substitute_call_type_args_with_bindings_expr(end, bindings, context, errors);
            if let Some(step_expr) = step {
                substitute_call_type_args_with_bindings_expr(step_expr, bindings, context, errors);
            }
            for nested in body {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
        }
        Stmt::While { cond, body, .. } => {
            substitute_call_type_args_with_bindings_expr(cond, bindings, context, errors);
            for nested in body {
                substitute_call_type_args_with_bindings_stmt(nested, bindings, context, errors);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn specialize_generic_struct_template(
    template: &StructDef,
    type_args: &[PrimitiveType],
    errors: &mut Vec<Diagnostic>,
) -> Option<StructDef> {
    let diag = DiagCtx::new(template.loc);
    if type_args.len() != template.type_params.len() {
        push_semantic(
            diag,
            errors,
            format!(
                "struct '{}' expects {} type arguments, got {}",
                template.name,
                template.type_params.len(),
                type_args.len()
            ),
        );
        return None;
    }

    let mut type_bindings = HashMap::<String, PrimitiveType>::new();
    for (param, ty) in template.type_params.iter().zip(type_args.iter()) {
        type_bindings.insert(param.clone(), *ty);
    }

    let mut fields = Vec::<StructField>::new();
    for field in &template.fields {
        let mut default = field.default.clone();
        if let Some(expr) = &mut default {
            substitute_call_type_args_with_bindings_expr(
                expr,
                &type_bindings,
                &format!("struct '{}'", template.name),
                errors,
            );
        }
        let specialized_ty = match &field.ty {
            FieldType::Scalar(prim) => FieldType::Scalar(*prim),
            FieldType::Generic(param) => specialize_named_type_ref(
                param,
                &type_bindings,
                &format!("struct '{}.{}'", template.name, field.name),
                DiagCtx::new(field.ty_loc.or(field.loc)),
                errors,
            )?,
            FieldType::Array(spec) => {
                if let Some(specialized) =
                    reinterpret_specialized_scalar_field_type(spec, &type_bindings, errors)?
                {
                    specialized
                } else {
                    let elem = match &spec.elem {
                        ArrayElemType::Primitive(prim) => ArrayElemType::Primitive(*prim),
                        ArrayElemType::Struct(elem) => {
                            match specialize_named_type_ref(
                                elem,
                                &type_bindings,
                                &format!("struct '{}.{}' array element", template.name, field.name),
                                DiagCtx::new(field.ty_loc.or(field.loc)),
                                errors,
                            )? {
                                FieldType::Scalar(bound) => ArrayElemType::Primitive(bound),
                                FieldType::Generic(name) => ArrayElemType::Struct(name),
                                FieldType::Array(_) | FieldType::Tuple(_) => unreachable!(),
                            }
                        }
                    };
                    FieldType::Array(onda_frontend::ArrayTypeSpec {
                        elem,
                        size: spec.size.clone(),
                    })
                }
            }
            FieldType::Tuple(elem_tys) => FieldType::Tuple(elem_tys.clone()),
        };
        fields.push(StructField {
            loc: field.loc,
            name: field.name.clone(),
            ty: specialized_ty,
            ty_loc: field.ty_loc,
            default,
        });
    }
    let mut methods = template.methods.clone();
    for method in &mut methods {
        let method_context = format!("struct '{}.{}'", template.name, method.name);
        specialize_function_type_annotations(method, &type_bindings, &method_context, errors);
        for param in &mut method.params {
            if let Some(default) = &mut param.default {
                substitute_call_type_args_with_bindings_expr(
                    default,
                    &type_bindings,
                    &format!("{method_context} parameter default"),
                    errors,
                );
            }
        }
        for stmt in &mut method.body {
            substitute_call_type_args_with_bindings_stmt(
                stmt,
                &type_bindings,
                &format!("{method_context} body"),
                errors,
            );
        }
    }

    Some(StructDef {
        loc: template.loc,
        name: specialized_struct_name(&template.name, type_args),
        type_params: Vec::new(),
        fields,
        methods,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct GenericInferenceLocals {
    types: CallTypeEnv,
    facts: Arc<GenericInferenceFacts>,
    default_ctor_missing_type_params_to_f32: bool,
}

#[derive(Debug, Default)]
pub(crate) struct GenericInferenceFacts {
    return_types: HashMap<String, ReturnType>,
    struct_defs: HashMap<String, Vec<TypedStructField>>,
}

pub(crate) fn generic_inference_facts<'a>(
    defs: impl IntoIterator<Item = &'a FunctionDef>,
    structs: impl IntoIterator<Item = &'a StructDef>,
) -> Arc<GenericInferenceFacts> {
    let mut candidates = HashMap::<String, Option<ReturnType>>::new();
    for def in defs {
        let return_type = declared_call_return_type(def);
        candidates
            .entry(def.name.clone())
            .and_modify(|existing| {
                if *existing != return_type {
                    *existing = None;
                }
            })
            .or_insert(return_type);
    }
    let struct_defs = structs
        .into_iter()
        .filter(|strukt| strukt.type_params.is_empty())
        .map(|strukt| {
            let fields = strukt
                .fields
                .iter()
                .filter_map(generic_inference_struct_field)
                .collect();
            (strukt.name.clone(), fields)
        })
        .collect();
    Arc::new(GenericInferenceFacts {
        return_types: candidates
            .into_iter()
            .filter_map(|(name, ty)| ty.map(|ty| (name, ty)))
            .collect(),
        struct_defs,
    })
}

fn generic_inference_struct_field(field: &StructField) -> Option<TypedStructField> {
    let (ty, struct_name, array_elem_ty, array_elem_struct) = match &field.ty {
        FieldType::Scalar(ty) => (TypedFieldType::Scalar(*ty), None, None, None),
        FieldType::Generic(name) => (TypedFieldType::Struct, Some(name.clone()), None, None),
        FieldType::Array(spec) => {
            let len = const_positive_usize_for_call_type(&spec.size)?;
            match &spec.elem {
                ArrayElemType::Primitive(elem) => {
                    (TypedFieldType::Array(len), None, Some(*elem), None)
                }
                ArrayElemType::Struct(elem) => {
                    (TypedFieldType::Array(len), None, None, Some(elem.clone()))
                }
            }
        }
        FieldType::Tuple(elements) => (TypedFieldType::Tuple(elements.clone()), None, None, None),
    };
    Some(TypedStructField {
        name: field.name.clone(),
        ty,
        default: None,
        integer_range: None,
        struct_name,
        array_elem_ty,
        array_elem_struct,
    })
}

impl GenericInferenceFacts {
    fn call_context(&self) -> CallTypeContext<'_> {
        CallTypeContext {
            return_types: &self.return_types,
            struct_defs: &self.struct_defs,
        }
    }
}

impl Default for GenericInferenceLocals {
    fn default() -> Self {
        Self {
            types: CallTypeEnv::default(),
            facts: Arc::default(),
            default_ctor_missing_type_params_to_f32: true,
        }
    }
}

impl GenericInferenceLocals {
    pub(crate) fn with_facts(facts: Arc<GenericInferenceFacts>) -> Self {
        Self {
            facts,
            ..Self::default()
        }
    }

    fn call_context(&self) -> CallTypeContext<'_> {
        self.facts.call_context()
    }

    pub(crate) fn from_scalar_types(
        scalar_types: &HashMap<String, PrimitiveType>,
        default_ctor_missing_type_params_to_f32: bool,
    ) -> Self {
        let mut locals = Self::default();
        locals.types.scalar_types.extend(scalar_types.clone());
        locals.default_ctor_missing_type_params_to_f32 = default_ctor_missing_type_params_to_f32;
        locals
    }
}

pub(crate) fn add_decl_type_to_generic_inference_locals(
    name: &str,
    ty: Option<&DeclType>,
    locals: &mut GenericInferenceLocals,
) {
    match ty {
        Some(DeclType::Scalar(prim)) => {
            locals
                .types
                .scalar_types
                .entry(name.to_owned())
                .or_insert(*prim);
        }
        Some(DeclType::Array { elem, size }) => {
            locals
                .types
                .array_types
                .entry(name.to_owned())
                .or_insert_with(|| {
                    CallArrayType::primitive(*elem, const_positive_usize_for_call_type(size))
                });
        }
        Some(DeclType::Slice(ArrayElemType::Primitive(elem))) => {
            locals
                .types
                .array_types
                .entry(name.to_owned())
                .or_insert_with(|| CallArrayType::primitive(*elem, None));
        }
        Some(DeclType::ArrayGeneric { elem, size }) => {
            locals
                .types
                .array_types
                .entry(name.to_owned())
                .or_insert_with(|| {
                    CallArrayType::nominal(elem.clone(), const_positive_usize_for_call_type(size))
                });
        }
        Some(DeclType::Slice(ArrayElemType::Struct(elem))) => {
            locals
                .types
                .array_types
                .entry(name.to_owned())
                .or_insert_with(|| CallArrayType::nominal(elem.clone(), None));
        }
        Some(DeclType::Generic(struct_name)) => {
            locals
                .types
                .struct_instances
                .entry(name.to_owned())
                .or_insert_with(|| struct_name.clone());
        }
        Some(DeclType::Tuple(elements)) => {
            locals
                .types
                .tuple_elem_types
                .entry(name.to_owned())
                .or_insert_with(|| elements.clone());
        }
        None => {
            locals
                .types
                .scalar_types
                .entry(name.to_owned())
                .or_insert(PrimitiveType::F32);
        }
    }
}

pub(crate) fn generic_inference_seed_for_processor(
    proc: &ProcessorDef,
    facts: Arc<GenericInferenceFacts>,
) -> GenericInferenceLocals {
    let mut locals = GenericInferenceLocals::with_facts(facts);
    for input in &proc.ins {
        add_decl_type_to_generic_inference_locals(&input.name, input.ty.as_ref(), &mut locals);
    }
    for output in &proc.outs {
        add_decl_type_to_generic_inference_locals(&output.name, output.ty.as_ref(), &mut locals);
    }
    for param in &proc.params {
        add_decl_type_to_generic_inference_locals(&param.name, param.ty.as_ref(), &mut locals);
    }
    locals
}

pub(crate) fn generic_inference_seed_for_top_level(
    blocks: &[Block],
    facts: Arc<GenericInferenceFacts>,
) -> GenericInferenceLocals {
    let mut locals = GenericInferenceLocals::with_facts(facts);
    for block in blocks {
        match block {
            Block::Const(_) => {}
            Block::Ins(ports) | Block::Outs(ports) | Block::KOuts(ports) => {
                for port in ports {
                    add_decl_type_to_generic_inference_locals(
                        &port.name,
                        port.ty.as_ref(),
                        &mut locals,
                    );
                }
            }
            Block::Params(params) => {
                for param in params {
                    add_decl_type_to_generic_inference_locals(
                        &param.name,
                        param.ty.as_ref(),
                        &mut locals,
                    );
                }
            }
            _ => {}
        }
    }
    locals
}

pub(crate) fn generic_inference_seed_for_top_level_decls(
    inputs: &[PortDecl],
    outputs: &[PortDecl],
    control_outputs: &[PortDecl],
    params: &[ParamDecl],
    facts: Arc<GenericInferenceFacts>,
) -> GenericInferenceLocals {
    let mut locals = GenericInferenceLocals::with_facts(facts);
    for port in inputs.iter().chain(outputs).chain(control_outputs) {
        add_decl_type_to_generic_inference_locals(&port.name, port.ty.as_ref(), &mut locals);
    }
    for param in params {
        add_decl_type_to_generic_inference_locals(&param.name, param.ty.as_ref(), &mut locals);
    }
    locals
}

pub(crate) fn generic_inference_seed_for_function(
    def: &FunctionDef,
    base: &GenericInferenceLocals,
) -> GenericInferenceLocals {
    let mut locals = base.clone();
    locals.types.set_owner_type_params(&def.type_params);
    for param in &def.params {
        locals.types.bind_function_param(param, &def.type_params);
    }
    locals
}

pub(crate) fn generic_inference_seed_for_event(
    event: &EventDef,
    base: &GenericInferenceLocals,
) -> GenericInferenceLocals {
    let mut locals = base.clone();
    for param in &event.params {
        bind_event_param_type(&mut locals.types, &param.name, &param.ty);
    }
    locals
}

pub(crate) fn generic_inference_seed_for_when(
    when: &WhenDef,
    delegate: Option<&DelegateDef>,
    takes_index: bool,
    base: &GenericInferenceLocals,
) -> GenericInferenceLocals {
    let mut locals = base.clone();
    let mut bindings = when.bindings.iter();
    if takes_index {
        if let Some(binding) = bindings.next() {
            bind_when_scalar(&mut locals.types, &binding.name, PrimitiveType::I32);
        }
    }
    if let Some(delegate) = delegate {
        for (binding, param) in bindings.zip(&delegate.params) {
            if binding.name != "_" {
                bind_event_param_type(&mut locals.types, &binding.name, &param.ty);
            }
        }
    }
    locals
}

pub(crate) fn resolve_generic_when_delegate(
    when: &WhenDef,
    owner_delegates: &[DelegateDef],
    child: Option<(&str, bool)>,
    current_proc: Option<(&str, &[DelegateDef])>,
    proc_delegates: &HashMap<String, Vec<DelegateDef>>,
    generated: &HashMap<String, ProcessorDef>,
) -> (Option<DelegateDef>, bool) {
    if when.target.receiver.is_empty() {
        return (
            owner_delegates
                .iter()
                .find(|delegate| delegate.name == when.target.delegate)
                .cloned(),
            false,
        );
    }
    let Some((proc_name, is_array)) = child else {
        return (None, false);
    };
    let delegate = current_proc
        .filter(|(name, _)| *name == proc_name)
        .map(|(_, delegates)| delegates)
        .or_else(|| {
            generated
                .get(proc_name)
                .map(|proc| proc.delegates.as_slice())
        })
        .or_else(|| proc_delegates.get(proc_name).map(Vec::as_slice))
        .and_then(|delegates| {
            delegates
                .iter()
                .find(|delegate| delegate.name == when.target.delegate)
        })
        .cloned();
    (delegate, is_array && when.target.index.is_none())
}

#[derive(Clone)]
pub(crate) struct ChildProcInstance {
    pub(crate) proc_name: String,
    pub(crate) is_array: bool,
}

pub(crate) fn child_proc_instances(
    init: &[Stmt],
    proc_names: &HashSet<String>,
) -> HashMap<String, ChildProcInstance> {
    let mut instances = HashMap::new();
    for stmt in init {
        let Stmt::Assign {
            target: AssignTarget::Var(instance),
            expr,
            ..
        } = stmt
        else {
            continue;
        };
        let resolved = match expr {
            Expr::UserCall {
                name: proc_name, ..
            } if proc_names.contains(proc_name) => Some((proc_name.clone(), false)),
            Expr::ArrayCtor { spec, .. } => match &spec.elem {
                ArrayElemType::Struct(proc_name) if proc_names.contains(proc_name) => {
                    Some((proc_name.clone(), true))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((proc_name, is_array)) = resolved {
            instances.insert(
                instance.clone(),
                ChildProcInstance {
                    proc_name,
                    is_array,
                },
            );
        }
    }
    instances
}

fn bind_when_scalar(types: &mut CallTypeEnv, name: &str, ty: PrimitiveType) {
    if name != "_" {
        types.shadow_binding(name);
        types.scalar_types.insert(name.to_owned(), ty);
    }
}

fn bind_event_param_type(types: &mut CallTypeEnv, name: &str, ty: &EventParamType) {
    types.shadow_binding(name);
    match ty {
        EventParamType::Scalar(ty) => {
            types.scalar_types.insert(name.to_owned(), *ty);
        }
        EventParamType::Tuple(elem_types) => {
            types
                .tuple_elem_types
                .insert(name.to_owned(), elem_types.clone());
        }
        EventParamType::Array { elem, size } => {
            types.array_types.insert(
                name.to_owned(),
                CallArrayType::primitive(*elem, const_positive_usize_for_call_type(size)),
            );
        }
        EventParamType::Slice { elem } => {
            types
                .array_types
                .insert(name.to_owned(), CallArrayType::primitive(*elem, None));
        }
        EventParamType::GenericScalar { name: struct_name } => {
            types
                .struct_instances
                .insert(name.to_owned(), struct_name.clone());
        }
        EventParamType::GenericArray { elem, size } => {
            types.array_types.insert(
                name.to_owned(),
                CallArrayType::nominal(elem.clone(), const_positive_usize_for_call_type(size)),
            );
        }
        EventParamType::GenericSlice { elem } => {
            types
                .array_types
                .insert(name.to_owned(), CallArrayType::nominal(elem.clone(), None));
        }
    }
}

fn parse_primitive_type_arg_token(token: &str) -> Option<PrimitiveType> {
    match token {
        "f32" => Some(PrimitiveType::F32),
        "f64" => Some(PrimitiveType::F64),
        "i32" => Some(PrimitiveType::I32),
        "i64" => Some(PrimitiveType::I64),
        "bool" => Some(PrimitiveType::Bool),
        _ => None,
    }
}

fn is_simple_ident_token(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn split_trailing_angle_group(text: &str) -> Option<(String, String)> {
    if !text.ends_with('>') {
        return None;
    }
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices().rev() {
        match ch {
            '>' => depth += 1,
            '<' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    let base = text[..idx].trim().to_owned();
                    let args = text[idx + 1..text.len() - 1].trim().to_owned();
                    if base.is_empty() || args.is_empty() {
                        return None;
                    }
                    return Some((base, args));
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_array_struct_elem_with_type_args(text: &str) -> Option<(String, Vec<CallTypeArg>)> {
    let (base, args_text) = split_trailing_angle_group(text)?;
    let mut args = Vec::<CallTypeArg>::new();
    for raw in args_text.split(',') {
        let token = raw.trim();
        if token.is_empty() {
            return None;
        }
        if let Some(prim) = parse_primitive_type_arg_token(token) {
            args.push(CallTypeArg::Primitive(prim));
        } else if is_simple_ident_token(token) {
            args.push(CallTypeArg::Generic(token.to_owned()));
        } else {
            return None;
        }
    }
    Some((base, args))
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum SpecializedTypeName {
    Primitive(PrimitiveType),
    Named(String),
}

fn specialize_generic_type_name(
    name: &str,
    type_bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<SpecializedTypeName> {
    if let Some(bound) = type_bindings.get(name).copied() {
        return Some(SpecializedTypeName::Primitive(bound));
    }
    if let Some((base, explicit_type_args)) = parse_array_struct_elem_with_type_args(name) {
        let mut resolved = Vec::<PrimitiveType>::with_capacity(explicit_type_args.len());
        for arg in explicit_type_args {
            match arg {
                CallTypeArg::Primitive(ty) => resolved.push(ty),
                CallTypeArg::Generic(param) => {
                    let Some(bound) = type_bindings.get(&param).copied() else {
                        push_semantic(
                            diag,
                            errors,
                            format!(
                                "{context} references unknown generic type parameter '{}'",
                                param
                            ),
                        );
                        return None;
                    };
                    resolved.push(bound);
                }
            }
        }
        return Some(SpecializedTypeName::Named(specialized_struct_name(
            &base, &resolved,
        )));
    }
    Some(SpecializedTypeName::Named(name.to_owned()))
}

fn specialize_named_type_ref(
    name: &str,
    type_bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<FieldType> {
    specialize_generic_type_name(name, type_bindings, context, diag, errors).map(|ty| match ty {
        SpecializedTypeName::Primitive(ty) => FieldType::Scalar(ty),
        SpecializedTypeName::Named(name) => FieldType::Generic(name),
    })
}

pub(crate) fn specialize_buffer_type(
    buffer: &BufferType,
    type_bindings: &HashMap<String, PrimitiveType>,
) -> BufferType {
    BufferType {
        elem: match &buffer.elem {
            BufferElemType::Primitive(ty) => BufferElemType::Primitive(*ty),
            BufferElemType::Generic(name) => type_bindings
                .get(name)
                .copied()
                .map(BufferElemType::Primitive)
                .unwrap_or_else(|| BufferElemType::Generic(name.clone())),
        },
        channels: buffer.channels.clone(),
    }
}

fn specialize_fn_param_type(
    ty: &FnParamType,
    type_bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> FnParamType {
    let specialize_name = |name: &str, errors: &mut Vec<Diagnostic>| {
        specialize_generic_type_name(name, type_bindings, context, diag, errors)
            .unwrap_or_else(|| SpecializedTypeName::Named(name.to_owned()))
    };
    match ty {
        FnParamType::Primitive(ty) => FnParamType::Primitive(*ty),
        FnParamType::Struct(name) => match specialize_name(name, errors) {
            SpecializedTypeName::Primitive(ty) => FnParamType::Primitive(ty),
            SpecializedTypeName::Named(name) => FnParamType::Struct(name),
        },
        FnParamType::Buffer(buffer) => {
            FnParamType::Buffer(specialize_buffer_type(buffer, type_bindings))
        }
        FnParamType::BufferArray { buffer, len } => FnParamType::BufferArray {
            buffer: specialize_buffer_type(buffer, type_bindings),
            len: *len,
        },
        FnParamType::Array(elem) => FnParamType::Array(*elem),
        FnParamType::ArrayGeneric(name) => match specialize_name(name, errors) {
            SpecializedTypeName::Primitive(ty) => FnParamType::Array(Some(ty)),
            SpecializedTypeName::Named(name) => FnParamType::ArrayGeneric(name),
        },
        FnParamType::SizedArray {
            elem,
            generic_name,
            size,
        } => match generic_name {
            Some(name) => match specialize_name(name, errors) {
                SpecializedTypeName::Primitive(ty) => FnParamType::SizedArray {
                    elem: Some(ty),
                    generic_name: None,
                    size: size.clone(),
                },
                SpecializedTypeName::Named(name) => FnParamType::SizedArray {
                    elem: *elem,
                    generic_name: Some(name),
                    size: size.clone(),
                },
            },
            None => FnParamType::SizedArray {
                elem: *elem,
                generic_name: None,
                size: size.clone(),
            },
        },
        FnParamType::BareBuffer => FnParamType::BareBuffer,
        FnParamType::Tuple(elements) => FnParamType::Tuple(elements.clone()),
    }
}

fn specialize_fn_return_scalar_type(
    ty: &FnReturnScalarType,
    type_bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> FnReturnScalarType {
    match ty {
        FnReturnScalarType::Primitive(ty) => FnReturnScalarType::Primitive(*ty),
        FnReturnScalarType::Named(name) => {
            match specialize_generic_type_name(name, type_bindings, context, diag, errors) {
                Some(SpecializedTypeName::Primitive(ty)) => FnReturnScalarType::Primitive(ty),
                Some(SpecializedTypeName::Named(name)) => FnReturnScalarType::Named(name),
                None => ty.clone(),
            }
        }
    }
}

fn specialize_fn_return_type(
    ty: &FnReturnType,
    type_bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> FnReturnType {
    let mut specialize_scalar =
        |ty| specialize_fn_return_scalar_type(ty, type_bindings, context, diag, errors);
    match ty {
        FnReturnType::Scalar(ty) => FnReturnType::Scalar(specialize_scalar(ty)),
        FnReturnType::Array { elem, size } => FnReturnType::Array {
            elem: specialize_scalar(elem),
            size: size.clone(),
        },
        FnReturnType::Tuple(elements) => {
            FnReturnType::Tuple(elements.iter().map(specialize_scalar).collect())
        }
    }
}

pub(crate) fn specialize_function_type_annotations(
    def: &mut FunctionDef,
    type_bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        if let Some(ty) = &mut param.ty {
            *ty = specialize_fn_param_type(
                ty,
                type_bindings,
                &format!("{context} parameter '{}'", param.name),
                DiagCtx::new(param.ty_loc.or(param.loc)),
                errors,
            );
        }
    }
    if let Some(ty) = &mut def.return_ty {
        *ty = specialize_fn_return_type(
            ty,
            type_bindings,
            &format!("{context} return type"),
            DiagCtx::new(def.return_ty_loc),
            errors,
        );
    }
}

pub(crate) fn specialize_decl_type(
    ty: &DeclType,
    type_bindings: &HashMap<String, PrimitiveType>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> DeclType {
    let specialize_name = |name: &str, errors: &mut Vec<Diagnostic>| {
        specialize_generic_type_name(name, type_bindings, context, diag, errors)
            .unwrap_or_else(|| SpecializedTypeName::Named(name.to_owned()))
    };
    match ty {
        DeclType::Scalar(ty) => DeclType::Scalar(*ty),
        DeclType::Slice(ArrayElemType::Primitive(ty)) => {
            DeclType::Slice(ArrayElemType::Primitive(*ty))
        }
        DeclType::Slice(ArrayElemType::Struct(name)) => match specialize_name(name, errors) {
            SpecializedTypeName::Primitive(ty) => DeclType::Slice(ArrayElemType::Primitive(ty)),
            SpecializedTypeName::Named(name) => DeclType::Slice(ArrayElemType::Struct(name)),
        },
        DeclType::Generic(name) => match specialize_name(name, errors) {
            SpecializedTypeName::Primitive(ty) => DeclType::Scalar(ty),
            SpecializedTypeName::Named(name) => DeclType::Generic(name),
        },
        DeclType::ArrayGeneric { elem, size } => match specialize_name(elem, errors) {
            SpecializedTypeName::Primitive(ty) => DeclType::Array {
                elem: ty,
                size: size.clone(),
            },
            SpecializedTypeName::Named(elem) => DeclType::ArrayGeneric {
                elem,
                size: size.clone(),
            },
        },
        DeclType::Array { elem, size } => DeclType::Array {
            elem: *elem,
            size: size.clone(),
        },
        DeclType::Tuple(elements) => DeclType::Tuple(elements.clone()),
    }
}

pub(crate) fn rewrite_generic_struct_ctor_expr(
    expr: &mut Expr,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
) {
    expr.visit_mut_postorder(|expr| {
        let diag = DiagCtx::new(expr.loc());
        match expr {
            Expr::ArrayCtor { spec, .. } => {
                if let ArrayElemType::Struct(elem_name) = &mut spec.elem {
                    let elem_text = elem_name.clone();
                    let (template_lookup_name, explicit_type_args) =
                        match parse_array_struct_elem_with_type_args(&elem_text) {
                            Some((base, type_args)) => (base, Some(type_args)),
                            None => (elem_text.clone(), None),
                        };
                    if let Some(template) = templates.get(template_lookup_name.as_str()) {
                        if !template.type_params.is_empty() {
                            let type_args_to_use = if let Some(type_args) = explicit_type_args {
                                let Some(resolved) = resolve_explicit_call_type_args(
                                    &type_args,
                                    &format!("array element type '{}'", elem_text),
                                    diag,
                                    errors,
                                ) else {
                                    return;
                                };
                                if resolved.len() != template.type_params.len() {
                                    push_semantic(
                                        diag,
                                        errors,
                                        format!(
                                        "array element type '{}' expects {} type arguments, got {}",
                                        template_lookup_name.as_str(),
                                        template.type_params.len(),
                                        resolved.len()
                                    ),
                                    );
                                    return;
                                }
                                resolved
                            } else {
                                vec![PrimitiveType::F32; template.type_params.len()]
                            };
                            if let Some(specialized) = specialize_generic_struct_template(
                                template,
                                &type_args_to_use,
                                errors,
                            ) {
                                let specialized_name = specialized.name.clone();
                                generated
                                    .entry(specialized_name.clone())
                                    .or_insert(specialized);
                                *elem_name = specialized_name;
                            }
                        }
                    }
                }
            }
            Expr::UserCall {
                name,
                type_args,
                args,
                ..
            } => {
                if let Some(template) = templates.get(name) {
                    let type_args_to_use = if type_args.is_empty() {
                        infer_generic_struct_ctor_type_args(template, args, locals, diag, errors)
                    } else {
                        resolve_explicit_call_type_args(
                            type_args,
                            &format!("struct constructor '{}'", name),
                            diag,
                            errors,
                        )
                    };
                    let Some(type_args_to_use) = type_args_to_use else {
                        return;
                    };
                    let Some(specialized) =
                        specialize_generic_struct_template(template, &type_args_to_use, errors)
                    else {
                        return;
                    };
                    let specialized_name = specialized.name.clone();
                    generated
                        .entry(specialized_name.clone())
                        .or_insert(specialized);
                    *name = specialized_name;
                    type_args.clear();
                }
            }
            _ => {}
        }
    });
}

pub(crate) trait GenericCtorRewriter {
    fn rewrite_expr(
        &mut self,
        expr: &mut Expr,
        locals: &mut GenericInferenceLocals,
        errors: &mut Vec<Diagnostic>,
    );

    fn rewrite_assignment_types(
        &mut self,
        _decl_ty: &mut Option<DeclType>,
        _generic_decl_ty: &mut Option<String>,
        _diag: DiagCtx,
        _errors: &mut Vec<Diagnostic>,
    ) {
    }

    fn nominal_expr_type(&self, _expr: &Expr) -> Option<String> {
        None
    }
}

fn rewrite_generic_ctor_stmt(
    stmt: &mut Stmt,
    errors: &mut Vec<Diagnostic>,
    locals: &mut GenericInferenceLocals,
    rewriter: &mut impl GenericCtorRewriter,
) {
    with_stmt_diag_context_mut(stmt, |diag, stmt| match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target,
            decl_ty,
            generic_decl_ty,
            is_typed_decl,
            expr,
            ..
        } => {
            rewriter.rewrite_assignment_types(decl_ty, generic_decl_ty, diag, errors);
            let introduces_binding = match target {
                AssignTarget::Var(name) => !locals.types.has_binding(name),
                AssignTarget::Index { .. }
                | AssignTarget::IndexedMember { .. }
                | AssignTarget::Slice { .. }
                | AssignTarget::Tuple(_) => false,
            };
            let prior_default_mode = locals.default_ctor_missing_type_params_to_f32;
            let typed_named_ctor_decl_without_type_args =
                *is_typed_decl && decl_ty.is_none() && generic_decl_ty.is_none();
            if typed_named_ctor_decl_without_type_args {
                locals.default_ctor_missing_type_params_to_f32 = false;
            }
            target.visit_selectors_mut(|selector| rewriter.rewrite_expr(selector, locals, errors));
            rewriter.rewrite_expr(expr, locals, errors);
            let context = locals.facts.call_context();
            update_call_type_env_after_assign(
                target,
                decl_ty.as_ref(),
                generic_decl_ty.as_deref(),
                expr,
                &mut locals.types,
                context,
            );
            if introduces_binding {
                if let (AssignTarget::Var(name), Some(nominal)) =
                    (target, rewriter.nominal_expr_type(expr))
                {
                    locals.types.shadow_binding(name);
                    locals.types.struct_instances.insert(name.clone(), nominal);
                }
            }
            locals.default_ctor_missing_type_params_to_f32 = prior_default_mode;
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewriter.rewrite_expr(expr, locals, errors);
        }
        Stmt::Print { values, .. } => {
            for value in values {
                rewriter.rewrite_expr(value, locals, errors);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewriter.rewrite_expr(cond, locals, errors);
            let mut then_locals = locals.clone();
            for nested in &mut *then_branch {
                rewrite_generic_ctor_stmt(nested, errors, &mut then_locals, rewriter);
            }
            let mut else_locals = locals.clone();
            for nested in &mut *else_branch {
                rewrite_generic_ctor_stmt(nested, errors, &mut else_locals, rewriter);
            }
            let (types, _) = join_branch_envs(
                then_locals.types,
                statement_list_flow(then_branch),
                else_locals.types,
                statement_list_flow(else_branch),
            );
            locals.types = types;
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
            rewriter.rewrite_expr(start, locals, errors);
            rewriter.rewrite_expr(end, locals, errors);
            if let Some(step_expr) = step {
                rewriter.rewrite_expr(step_expr, locals, errors);
            }
            let mut body_locals = locals.clone();
            body_locals.types.shadow_binding(var);
            body_locals.types.scalar_types.insert(var.clone(), *var_ty);
            for nested in body {
                rewrite_generic_ctor_stmt(nested, errors, &mut body_locals, rewriter);
            }
        }
        Stmt::While { cond, body, .. } => {
            rewriter.rewrite_expr(cond, locals, errors);
            let mut body_locals = locals.clone();
            for nested in body {
                rewrite_generic_ctor_stmt(nested, errors, &mut body_locals, rewriter);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

pub(crate) fn rewrite_generic_ctor_stmt_list(
    stmts: &mut [Stmt],
    errors: &mut Vec<Diagnostic>,
    seed_locals: &GenericInferenceLocals,
    rewriter: &mut impl GenericCtorRewriter,
) -> GenericInferenceLocals {
    let mut locals = seed_locals.clone();
    for stmt in stmts {
        rewrite_generic_ctor_stmt(stmt, errors, &mut locals, rewriter);
    }
    locals
}

struct GenericStructCtorRewriter<'a> {
    templates: &'a HashMap<String, StructDef>,
    generated: &'a mut HashMap<String, StructDef>,
}

impl GenericCtorRewriter for GenericStructCtorRewriter<'_> {
    fn rewrite_expr(
        &mut self,
        expr: &mut Expr,
        locals: &mut GenericInferenceLocals,
        errors: &mut Vec<Diagnostic>,
    ) {
        rewrite_generic_struct_ctor_expr(expr, self.templates, self.generated, errors, locals);
    }

    fn rewrite_assignment_types(
        &mut self,
        decl_ty: &mut Option<DeclType>,
        generic_decl_ty: &mut Option<String>,
        diag: DiagCtx,
        errors: &mut Vec<Diagnostic>,
    ) {
        match decl_ty {
            Some(DeclType::Slice(ArrayElemType::Struct(name)))
            | Some(DeclType::ArrayGeneric { elem: name, .. }) => {
                rewrite_resolved_struct_type_name(
                    name,
                    self.templates,
                    self.generated,
                    diag,
                    errors,
                );
            }
            _ => {}
        }
        if let Some(name) = generic_decl_ty {
            rewrite_resolved_struct_type_name(name, self.templates, self.generated, diag, errors);
        }
    }

    fn nominal_expr_type(&self, expr: &Expr) -> Option<String> {
        let Expr::UserCall { name, .. } = expr else {
            return None;
        };
        self.generated.contains_key(name).then(|| name.clone())
    }
}

pub(crate) fn rewrite_generic_struct_ctor_stmt_list(
    stmts: &mut [Stmt],
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
    seed_locals: &GenericInferenceLocals,
) -> GenericInferenceLocals {
    let mut rewriter = GenericStructCtorRewriter {
        templates,
        generated,
    };
    rewrite_generic_ctor_stmt_list(stmts, errors, seed_locals, &mut rewriter)
}

pub(crate) fn infer_scalar_type_for_generic_binding(
    expr: &Expr,
    locals: &GenericInferenceLocals,
) -> Option<PrimitiveType> {
    let inferred = infer_call_scalar_expr_type(expr, &locals.types, locals.call_context());
    effective_untyped_assignment_type(expr, inferred).or(inferred)
}

pub(crate) fn infer_array_elem_type_for_generic_binding(
    expr: &Expr,
    locals: &GenericInferenceLocals,
) -> Option<PrimitiveType> {
    infer_array_arg_type(expr, &locals.types, locals.call_context())?.primitive_elem()
}

pub(crate) fn bind_inferred_generic_type(
    bindings: &mut HashMap<String, PrimitiveType>,
    type_param: &str,
    inferred: PrimitiveType,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(existing) = bindings.get(type_param).copied() {
        if existing == inferred {
            return;
        }
        if let Some(merged) = merge_inferred_return_types(existing, inferred) {
            bindings.insert(type_param.to_owned(), merged);
        } else {
            push_semantic(
                diag,
                errors,
                format!(
                    "{context}: conflicting inferred types {:?} and {:?} for generic parameter '{}'",
                    existing, inferred, type_param
                ),
            );
        }
    } else {
        bindings.insert(type_param.to_owned(), inferred);
    }
}

pub(crate) fn finalize_inferred_generic_type_args(
    owner_name: &str,
    owner_kind: &str,
    type_params: &[String],
    bindings: &HashMap<String, PrimitiveType>,
    default_missing_to_f32: bool,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut out = Vec::<PrimitiveType>::with_capacity(type_params.len());
    for param in type_params {
        let bound = if let Some(bound) = bindings.get(param).copied() {
            bound
        } else if default_missing_to_f32 {
            PrimitiveType::F32
        } else {
            push_semantic(
                diag,
                errors,
                format!(
                    "cannot infer generic type parameter '{}' for {} '{}' constructor; provide explicit type arguments",
                    param, owner_kind, owner_name
                ),
            );
            return None;
        };
        out.push(bound);
    }
    Some(out)
}

fn named_specialization_type_args(
    expected_base: &str,
    actual_name: &str,
) -> Option<Vec<PrimitiveType>> {
    if let Some((actual_base, args)) = parse_array_struct_elem_with_type_args(actual_name) {
        if actual_base != expected_base {
            return None;
        }
        return args
            .iter()
            .map(|arg| match arg {
                CallTypeArg::Primitive(ty) => Some(*ty),
                CallTypeArg::Generic(_) => None,
            })
            .collect();
    }
    let signature = actual_name.strip_prefix(&format!("{expected_base}.__gen__"))?;
    signature
        .split('_')
        .map(parse_primitive_type_arg_token)
        .collect()
}

fn bind_nested_generic_type_args(
    expected_name: &str,
    actual_name: &str,
    owner_type_params: &[String],
    bindings: &mut HashMap<String, PrimitiveType>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) {
    let Some((expected_base, expected_args)) =
        parse_array_struct_elem_with_type_args(expected_name)
    else {
        return;
    };
    let Some(actual_args) = named_specialization_type_args(&expected_base, actual_name) else {
        return;
    };
    if expected_args.len() != actual_args.len() {
        return;
    }
    for (expected, actual) in expected_args.iter().zip(actual_args) {
        if let CallTypeArg::Generic(type_param) = expected {
            if owner_type_params.contains(type_param) {
                bind_inferred_generic_type(bindings, type_param, actual, context, diag, errors);
            }
        }
    }
}

fn infer_nominal_type_for_generic_binding(
    expr: &Expr,
    locals: &GenericInferenceLocals,
) -> Option<String> {
    match expr {
        Expr::UserCall { name, .. } => Some(name.clone()),
        _ => infer_struct_expr_type(expr, &locals.types, locals.call_context()),
    }
}

pub(crate) fn infer_generic_struct_ctor_type_args(
    template: &StructDef,
    args: &[CallArg],
    locals: &GenericInferenceLocals,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let param_names = template
        .fields
        .iter()
        .map(|f| f.name.clone())
        .collect::<Vec<_>>();
    let defaults = template
        .fields
        .iter()
        .map(|f| f.default.clone().or(Some(Expr::number(0.0))))
        .collect::<Vec<_>>();
    let resolved = resolve_call_args(
        args,
        &param_names,
        &defaults,
        false,
        false,
        &format!("struct constructor '{}'", template.name),
        errors,
    );

    let mut bindings = HashMap::<String, PrimitiveType>::new();
    for (idx, field) in template.fields.iter().enumerate() {
        let Some(expr) = resolved.get(idx).and_then(|arg| *arg).or_else(|| {
            defaults
                .get(idx)
                .and_then(|default_expr| default_expr.as_ref())
        }) else {
            continue;
        };
        if let FieldType::Generic(type_param) = &field.ty {
            if template.type_params.contains(type_param) {
                if let Some(inferred) = infer_scalar_type_for_generic_binding(expr, locals) {
                    bind_inferred_generic_type(
                        &mut bindings,
                        type_param,
                        inferred,
                        &format!("struct constructor '{}'", template.name),
                        diag,
                        errors,
                    );
                }
            } else if let Some(actual_name) = infer_nominal_type_for_generic_binding(expr, locals) {
                bind_nested_generic_type_args(
                    type_param,
                    &actual_name,
                    &template.type_params,
                    &mut bindings,
                    &format!("struct constructor '{}'", template.name),
                    diag,
                    errors,
                );
            }
        } else if let FieldType::Array(spec) = &field.ty {
            if let ArrayElemType::Struct(type_param) = &spec.elem {
                if template.type_params.contains(type_param) {
                    if let Some(inferred) = infer_array_elem_type_for_generic_binding(expr, locals)
                    {
                        bind_inferred_generic_type(
                            &mut bindings,
                            type_param,
                            inferred,
                            &format!("struct constructor '{}'", template.name),
                            diag,
                            errors,
                        );
                    }
                } else if let Some(array) =
                    infer_array_arg_type(expr, &locals.types, locals.call_context())
                {
                    if let crate::def_semantics::call_types::CallArrayElemType::Nominal(
                        actual_name,
                    ) = array.elem
                    {
                        bind_nested_generic_type_args(
                            type_param,
                            &actual_name,
                            &template.type_params,
                            &mut bindings,
                            &format!("struct constructor '{}'", template.name),
                            diag,
                            errors,
                        );
                    }
                }
            }
        }
    }
    finalize_inferred_generic_type_args(
        &template.name,
        "struct",
        &template.type_params,
        &bindings,
        locals.default_ctor_missing_type_params_to_f32,
        diag,
        errors,
    )
}

pub(crate) fn infer_generic_proc_ctor_type_args(
    template: &ProcessorDef,
    args: &[CallArg],
    locals: &GenericInferenceLocals,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<PrimitiveType>> {
    let mut ctor_param_names = template
        .params
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    ctor_param_names.extend(template.buffers.iter().map(|b| b.name.clone()));
    let mut ctor_defaults = template
        .params
        .iter()
        .map(|p| p.default.clone())
        .collect::<Vec<_>>();
    ctor_defaults.extend((0..template.buffers.len()).map(|_| None));

    let resolved = resolve_call_args(
        args,
        &ctor_param_names,
        &ctor_defaults,
        false,
        false,
        &format!("processor constructor '{}'", template.name),
        errors,
    );

    let mut bindings = HashMap::<String, PrimitiveType>::new();
    for (idx, param) in template.params.iter().enumerate() {
        let Some(expr) = resolved.get(idx).and_then(|arg| *arg).or_else(|| {
            ctor_defaults
                .get(idx)
                .and_then(|default_expr| default_expr.as_ref())
        }) else {
            continue;
        };
        if let Some(param_ty) = &param.ty {
            match param_ty {
                DeclType::Generic(type_param) => {
                    if let Some(inferred) = infer_scalar_type_for_generic_binding(expr, locals) {
                        bind_inferred_generic_type(
                            &mut bindings,
                            type_param,
                            inferred,
                            &format!("processor constructor '{}'", template.name),
                            diag,
                            errors,
                        );
                    }
                }
                DeclType::ArrayGeneric { elem, .. } => {
                    if let Some(inferred) = infer_array_elem_type_for_generic_binding(expr, locals)
                    {
                        bind_inferred_generic_type(
                            &mut bindings,
                            elem,
                            inferred,
                            &format!("processor constructor '{}'", template.name),
                            diag,
                            errors,
                        );
                    }
                }
                DeclType::Slice(_)
                | DeclType::Scalar(_)
                | DeclType::Array { .. }
                | DeclType::Tuple(_) => {}
            }
        }
    }
    finalize_inferred_generic_type_args(
        &template.name,
        "processor",
        &template.type_params,
        &bindings,
        locals.default_ctor_missing_type_params_to_f32,
        diag,
        errors,
    )
}

pub(crate) fn finalize_generated_generic_struct_specializations(
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
    base_seed: &GenericInferenceLocals,
) {
    let mut processed = HashSet::<String>::new();
    loop {
        let names = generated.keys().cloned().collect::<Vec<_>>();
        let mut progressed = false;
        for name in names {
            if processed.contains(&name) {
                continue;
            }
            let Some(mut spec) = generated.remove(&name) else {
                continue;
            };
            rewrite_generic_struct_field_types(&mut spec, templates, generated, errors);
            let mut nested_specializations = Vec::<String>::new();
            for field in &spec.fields {
                match &field.ty {
                    FieldType::Generic(name) => nested_specializations.push(name.clone()),
                    FieldType::Array(spec) => {
                        if let ArrayElemType::Struct(name) = &spec.elem {
                            nested_specializations.push(name.clone());
                        }
                    }
                    FieldType::Scalar(_) | FieldType::Tuple(_) => {}
                }
            }
            for nested_name in nested_specializations {
                ensure_generated_nested_struct_specialization(
                    &nested_name,
                    templates,
                    generated,
                    errors,
                );
            }
            for field in &mut spec.fields {
                if let Some(default) = &mut field.default {
                    let mut locals = base_seed.clone();
                    rewrite_generic_struct_ctor_expr(
                        default,
                        templates,
                        generated,
                        errors,
                        &mut locals,
                    );
                }
            }
            for method in &mut spec.methods {
                rewrite_explicit_generic_struct_function_types(
                    method, templates, generated, errors,
                );
                let method_seed = generic_inference_seed_for_function(method, base_seed);
                rewrite_generic_struct_ctor_stmt_list(
                    &mut method.body,
                    templates,
                    generated,
                    errors,
                    &method_seed,
                );
            }
            generated.insert(name.clone(), spec);
            processed.insert(name);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
}

pub(crate) fn rewrite_generic_struct_field_types(
    def: &mut StructDef,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
) {
    for field in &mut def.fields {
        rewrite_generic_struct_field_type(
            &mut field.ty,
            templates,
            generated,
            DiagCtx::new(field.ty_loc.or(field.loc)),
            errors,
        );
    }
}

fn rewrite_generic_struct_field_type(
    ty: &mut FieldType,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) {
    match ty {
        FieldType::Scalar(_) | FieldType::Tuple(_) => {}
        FieldType::Generic(name) => {
            if let Some(specialized) =
                specialize_explicit_struct_type_name(name, templates, generated, diag, errors)
            {
                *name = specialized;
            }
        }
        FieldType::Array(spec) => {
            if let Some(specialized) =
                reinterpret_explicit_scalar_field_type(spec, templates, generated, diag, errors)
            {
                *ty = FieldType::Generic(specialized);
                return;
            }
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                if let Some(specialized) =
                    specialize_explicit_struct_type_name(name, templates, generated, diag, errors)
                {
                    *name = specialized;
                }
            }
        }
    }
}

fn ensure_generated_nested_struct_specialization(
    name: &str,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
) {
    if generated.contains_key(name) {
        return;
    }
    let Some((base, type_args)) = parse_specialized_struct_name_ref(name) else {
        return;
    };
    let Some(template) = templates.get(base.as_str()) else {
        return;
    };
    if template.type_params.is_empty() {
        return;
    }
    if let Some(spec) = specialize_generic_struct_template(template, &type_args, errors) {
        generated.entry(name.to_owned()).or_insert(spec);
    }
}

pub(crate) fn validate_deferred_generic_structs(
    def: &FunctionDef,
    templates: &HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
) {
    let owner_params = def.type_params.iter().cloned().collect::<HashSet<_>>();
    let mut validate = |name: &str, type_args: &[CallTypeArg], loc: SourceLoc| {
        let Some(template) = templates.get(name) else {
            return;
        };
        if !type_args.is_empty() && type_args.len() != template.type_params.len() {
            push_semantic(
                DiagCtx::new(loc),
                errors,
                format!(
                    "struct constructor '{}' expects {} type arguments, got {}",
                    name,
                    template.type_params.len(),
                    type_args.len()
                ),
            );
            return;
        }
        for type_arg in type_args {
            match type_arg {
                CallTypeArg::Primitive(PrimitiveType::Bool) => push_semantic(
                    DiagCtx::new(loc),
                    errors,
                    format!(
                        "struct constructor '{}': 'bool' is not allowed as a generic type argument; only numeric types (f32, f64, i32, i64) are supported",
                        name
                    ),
                ),
                CallTypeArg::Generic(param) if !owner_params.contains(param) => push_semantic(
                    DiagCtx::new(loc),
                    errors,
                    format!(
                        "struct constructor '{}': generic type argument '{}' is not declared by generic def '{}'",
                        name, param, def.name
                    ),
                ),
                CallTypeArg::Primitive(_) | CallTypeArg::Generic(_) => {}
            }
        }
    };

    for statement in &def.body {
        statement.visit_exprs(|root| {
            for expr in root.walk() {
                match expr {
                    Expr::UserCall {
                        name,
                        type_args,
                        loc,
                        ..
                    } => validate(name, type_args, (*loc).into()),
                    Expr::ArrayCtor { spec, loc, .. } => {
                        let ArrayElemType::Struct(name) = &spec.elem else {
                            continue;
                        };
                        if let Some((base, type_args)) =
                            parse_array_struct_elem_with_type_args(name)
                        {
                            validate(&base, &type_args, (*loc).into());
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}

pub(crate) struct DeferredGenericStructs {
    templates: HashMap<String, StructDef>,
    materialized: HashSet<String>,
    inference: GenericInferenceLocals,
}

impl DeferredGenericStructs {
    pub(crate) fn new(
        templates: HashMap<String, StructDef>,
        materialized: impl IntoIterator<Item = String>,
        inference: GenericInferenceLocals,
    ) -> Self {
        Self {
            templates,
            materialized: materialized.into_iter().collect(),
            inference,
        }
    }

    pub(crate) fn materialize(
        &mut self,
        functions: &mut [FunctionDef],
        options: AnalysisOptions,
        errors: &mut Vec<Diagnostic>,
    ) -> Vec<(StructDef, TypedStruct)> {
        let mut generated = HashMap::new();
        for function in functions {
            let seed = generic_inference_seed_for_function(function, &self.inference);
            rewrite_generic_struct_ctor_stmt_list(
                &mut function.body,
                &self.templates,
                &mut generated,
                errors,
                &seed,
            );
        }
        finalize_generated_generic_struct_specializations(
            &self.templates,
            &mut generated,
            errors,
            &self.inference,
        );

        let mut names = generated.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
            .into_iter()
            .filter(|name| self.materialized.insert(name.clone()))
            .filter_map(|name| {
                let raw = generated.remove(&name)?;
                let fields = crate::declaration_coercion::coerce_struct_fields(
                    &raw.name,
                    &raw.type_params,
                    &raw.fields,
                    options,
                    errors,
                );
                let typed = TypedStruct {
                    name: raw.name.clone(),
                    fields,
                };
                Some((raw, typed))
            })
            .collect()
    }
}

fn parse_specialized_struct_name_ref(name: &str) -> Option<(String, Vec<PrimitiveType>)> {
    let (base, sig) = name.split_once(".__gen__")?;
    let mut type_args = Vec::<PrimitiveType>::new();
    for raw in sig.split('_') {
        let ty = match raw {
            "f32" => PrimitiveType::F32,
            "f64" => PrimitiveType::F64,
            "i32" => PrimitiveType::I32,
            "i64" => PrimitiveType::I64,
            "bool" => PrimitiveType::Bool,
            _ => return None,
        };
        type_args.push(ty);
    }
    Some((base.to_owned(), type_args))
}

fn specialize_explicit_struct_type_name(
    name: &str,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let (base, explicit_type_args) = parse_array_struct_elem_with_type_args(name)?;
    let template = templates.get(base.as_str())?;
    let resolved = resolve_explicit_call_type_args(
        &explicit_type_args,
        &format!("struct field type '{name}'"),
        diag,
        errors,
    )?;
    let specialized = specialized_struct_name(&base, &resolved);
    if !generated.contains_key(&specialized) {
        if let Some(spec) = specialize_generic_struct_template(template, &resolved, errors) {
            generated.insert(specialized.clone(), spec);
        }
    }
    Some(specialized)
}

fn specialize_resolved_struct_type_name(
    name: &str,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let (_, args) = parse_array_struct_elem_with_type_args(name)?;
    if args
        .iter()
        .any(|arg| matches!(arg, CallTypeArg::Generic(_)))
    {
        return None;
    }
    specialize_explicit_struct_type_name(name, templates, generated, diag, errors)
}

fn rewrite_resolved_struct_type_name(
    name: &mut String,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) {
    if parse_specialized_struct_name_ref(name).is_some() {
        ensure_generated_nested_struct_specialization(name, templates, generated, errors);
        return;
    }
    if let Some(specialized) =
        specialize_resolved_struct_type_name(name, templates, generated, diag, errors)
    {
        *name = specialized;
    }
}

pub(crate) fn rewrite_explicit_generic_struct_function_types(
    def: &mut FunctionDef,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in &mut def.params {
        if let Some(ty) = &mut param.ty {
            let name = match ty {
                FnParamType::Struct(name) | FnParamType::ArrayGeneric(name) => Some(name),
                FnParamType::SizedArray {
                    generic_name: Some(name),
                    ..
                } => Some(name),
                _ => None,
            };
            if let Some(name) = name {
                rewrite_resolved_struct_type_name(
                    name,
                    templates,
                    generated,
                    DiagCtx::new(param.ty_loc.or(param.loc)),
                    errors,
                );
            }
        }
        if let Some(default) = &mut param.default {
            rewrite_generic_struct_ctor_expr(
                default,
                templates,
                generated,
                errors,
                &mut GenericInferenceLocals::default(),
            );
        }
    }
    let Some(return_ty) = &mut def.return_ty else {
        return;
    };
    let mut rewrite_scalar = |scalar: &mut FnReturnScalarType| {
        if let FnReturnScalarType::Named(name) = scalar {
            rewrite_resolved_struct_type_name(
                name,
                templates,
                generated,
                DiagCtx::new(def.return_ty_loc),
                errors,
            );
        }
    };
    match return_ty {
        FnReturnType::Scalar(scalar) => rewrite_scalar(scalar),
        FnReturnType::Array { elem, .. } => rewrite_scalar(elem),
        FnReturnType::Tuple(elements) => elements.iter_mut().for_each(rewrite_scalar),
    }
}

pub(crate) fn rewrite_explicit_generic_struct_event_types(
    params: &mut [EventParamDecl],
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    errors: &mut Vec<Diagnostic>,
) {
    for param in params {
        let name = match &mut param.ty {
            EventParamType::GenericScalar { name }
            | EventParamType::GenericArray { elem: name, .. }
            | EventParamType::GenericSlice { elem: name } => Some(name),
            _ => None,
        };
        if let Some(name) = name {
            rewrite_resolved_struct_type_name(
                name,
                templates,
                generated,
                DiagCtx::new(param.ty_loc.or(param.loc)),
                errors,
            );
        }
        if let Some(default) = &mut param.default {
            rewrite_generic_struct_ctor_expr(
                default,
                templates,
                generated,
                errors,
                &mut GenericInferenceLocals::default(),
            );
        }
    }
}

fn reinterpret_explicit_scalar_field_type(
    spec: &onda_frontend::ArrayTypeSpec,
    templates: &HashMap<String, StructDef>,
    generated: &mut HashMap<String, StructDef>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let ArrayElemType::Struct(base) = &spec.elem else {
        return None;
    };
    let Expr::Var { name: type_arg, .. } = spec.size.as_ref() else {
        return None;
    };
    let type_name = format!("{base}<{type_arg}>");
    specialize_explicit_struct_type_name(&type_name, templates, generated, diag, errors)
}

fn reinterpret_specialized_scalar_field_type(
    spec: &onda_frontend::ArrayTypeSpec,
    type_bindings: &HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Option<FieldType>> {
    let ArrayElemType::Struct(base) = &spec.elem else {
        return Some(None);
    };
    let Expr::Var { name: type_arg, .. } = spec.size.as_ref() else {
        return Some(None);
    };
    if !type_bindings.contains_key(type_arg) && parse_primitive_type_arg_token(type_arg).is_none() {
        return Some(None);
    }
    specialize_named_type_ref(
        &format!("{base}<{type_arg}>"),
        type_bindings,
        &format!("struct field type '{}<{}>'", base, type_arg),
        DiagCtx::new(spec.size.loc()),
        errors,
    )
    .map(Some)
}
