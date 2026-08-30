use std::collections::{HashMap, HashSet};

use crate::*;

#[derive(Debug, Clone, Default)]
pub(crate) struct CallTypeEnv {
    owner_type_params: HashSet<String>,
    /// Lexical bindings whose concrete semantic type still depends on a call
    /// site or an unresolved expression. Keeping existence separate from type
    /// prevents a later reassignment from being mistaken for a declaration.
    pub(crate) unresolved_bindings: HashSet<String>,
    pub(crate) scalar_types: HashMap<String, PrimitiveType>,
    pub(crate) struct_instances: HashMap<String, String>,
    pub(crate) array_types: HashMap<String, CallArrayType>,
    pub(crate) buffer_types: HashMap<String, (PrimitiveType, TypedBufferChannels)>,
    pub(crate) buffer_array_lens: HashMap<String, usize>,
    pub(crate) tuple_elem_types: HashMap<String, Vec<PrimitiveType>>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) enum CallArrayElemType {
    Primitive(PrimitiveType),
    /// A source struct or processor type. The later structural-parameter pass
    /// distinguishes the two using its complete set of declarations.
    Nominal(String),
}

impl CallArrayElemType {
    fn merge(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Primitive(lhs), Self::Primitive(rhs)) => {
                merge_inferred_return_types(lhs, rhs).map(Self::Primitive)
            }
            (Self::Nominal(lhs), Self::Nominal(rhs)) if lhs == rhs => Some(Self::Nominal(lhs)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct CallArrayType {
    pub(crate) elem: CallArrayElemType,
    /// `None` is an unsized array view (`T[]` or a slice), not an unknown
    /// fixed length. Fixed array expressions always retain their length.
    pub(crate) len: Option<usize>,
}

/// Whether execution can reach the next statement in the current statement
/// list. `return`, `break`, and `continue` all terminate that local path; loop
/// callers deliberately discard the body's flow because a loop itself may
/// still fall through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatementFlow {
    Continues,
    Terminates,
}

pub(crate) fn statement_list_flow(stmts: &[Stmt]) -> StatementFlow {
    for stmt in stmts {
        let flow = statement_flow(stmt);
        if flow == StatementFlow::Terminates {
            return flow;
        }
    }
    StatementFlow::Continues
}

pub(crate) fn statement_flow(stmt: &Stmt) -> StatementFlow {
    match stmt {
        Stmt::Return { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {
            StatementFlow::Terminates
        }
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } if statement_list_flow(then_branch) == StatementFlow::Terminates
            && statement_list_flow(else_branch) == StatementFlow::Terminates =>
        {
            StatementFlow::Terminates
        }
        Stmt::Const { .. }
        | Stmt::Assign { .. }
        | Stmt::Expr { .. }
        | Stmt::Print { .. }
        | Stmt::If { .. }
        | Stmt::For { .. }
        | Stmt::While { .. } => StatementFlow::Continues,
    }
}

/// Joins the type environments of an `if` at its continuation point. A
/// terminating branch contributes no constraints there because execution can
/// only arrive through the other branch.
pub(crate) fn join_branch_envs(
    mut then_env: CallTypeEnv,
    then_flow: StatementFlow,
    else_env: CallTypeEnv,
    else_flow: StatementFlow,
) -> (CallTypeEnv, StatementFlow) {
    match (then_flow, else_flow) {
        (StatementFlow::Continues, StatementFlow::Continues) => {
            then_env.intersect_with(&else_env);
            (then_env, StatementFlow::Continues)
        }
        (StatementFlow::Continues, StatementFlow::Terminates) => {
            (then_env, StatementFlow::Continues)
        }
        (StatementFlow::Terminates, StatementFlow::Continues) => {
            (else_env, StatementFlow::Continues)
        }
        (StatementFlow::Terminates, StatementFlow::Terminates) => {
            (then_env, StatementFlow::Terminates)
        }
    }
}

impl CallArrayType {
    pub(crate) fn primitive(elem: PrimitiveType, len: Option<usize>) -> Self {
        Self {
            elem: CallArrayElemType::Primitive(elem),
            len,
        }
    }

    pub(crate) fn nominal(name: impl Into<String>, len: Option<usize>) -> Self {
        Self {
            elem: CallArrayElemType::Nominal(name.into()),
            len,
        }
    }

    pub(crate) fn primitive_elem(&self) -> Option<PrimitiveType> {
        match self.elem {
            CallArrayElemType::Primitive(elem) => Some(elem),
            CallArrayElemType::Nominal(_) => None,
        }
    }
}

impl CallTypeEnv {
    pub(crate) fn set_owner_type_params(&mut self, type_params: &[String]) {
        self.owner_type_params.clear();
        self.owner_type_params.extend(type_params.iter().cloned());
    }

    /// Binds a concrete function parameter in the shared call-typing
    /// environment. Generic and structural parameters stay unknown until
    /// monomorphization supplies a concrete signature.
    pub(crate) fn bind_function_param(
        &mut self,
        param: &onda_frontend::FnParamDecl,
        owner_type_params: &[String],
    ) {
        self.bind_function_param_type(&param.name, param.ty.as_ref(), owner_type_params);
    }

    pub(crate) fn bind_function_param_type(
        &mut self,
        name: &str,
        param_ty: Option<&FnParamType>,
        owner_type_params: &[String],
    ) {
        self.shadow_binding(name);
        match param_ty {
            Some(FnParamType::Primitive(prim)) => {
                self.scalar_types.insert(name.to_owned(), *prim);
            }
            Some(FnParamType::Struct(struct_name)) if !owner_type_params.contains(struct_name) => {
                self.struct_instances
                    .insert(name.to_owned(), struct_name.clone());
            }
            Some(FnParamType::Array(Some(prim))) => {
                self.array_types
                    .insert(name.to_owned(), CallArrayType::primitive(*prim, None));
            }
            Some(FnParamType::SizedArray {
                elem: Some(prim),
                size,
                ..
            }) => {
                self.array_types.insert(
                    name.to_owned(),
                    CallArrayType::primitive(*prim, const_positive_usize_for_call_type(size)),
                );
            }
            Some(FnParamType::ArrayGeneric(nominal)) if !owner_type_params.contains(nominal) => {
                self.array_types.insert(
                    name.to_owned(),
                    CallArrayType::nominal(nominal.clone(), None),
                );
            }
            Some(FnParamType::SizedArray {
                generic_name: Some(nominal),
                size,
                ..
            }) if !owner_type_params.contains(nominal) => {
                self.array_types.insert(
                    name.to_owned(),
                    CallArrayType::nominal(
                        nominal.clone(),
                        const_positive_usize_for_call_type(size),
                    ),
                );
            }
            Some(FnParamType::Buffer(buffer_ty)) => {
                if let BufferElemType::Primitive(elem_ty) = buffer_ty.elem {
                    self.buffer_types.insert(
                        name.to_owned(),
                        (elem_ty, resolved_buffer_channels(buffer_ty)),
                    );
                } else {
                    self.unresolved_bindings.insert(name.to_owned());
                }
            }
            Some(FnParamType::BufferArray { buffer, len }) => {
                if let BufferElemType::Primitive(elem_ty) = buffer.elem {
                    self.buffer_types
                        .insert(name.to_owned(), (elem_ty, resolved_buffer_channels(buffer)));
                    self.buffer_array_lens.insert(name.to_owned(), *len);
                } else {
                    self.unresolved_bindings.insert(name.to_owned());
                }
            }
            Some(FnParamType::Tuple(elem_types)) => {
                self.tuple_elem_types
                    .insert(name.to_owned(), elem_types.clone());
            }
            Some(FnParamType::Struct(_))
            | Some(FnParamType::ArrayGeneric(_))
            | Some(FnParamType::SizedArray { .. })
            | Some(FnParamType::Array(None))
            | Some(FnParamType::BareBuffer)
            | None => {
                self.unresolved_bindings.insert(name.to_owned());
            }
        }
    }

    pub(crate) fn has_binding(&self, name: &str) -> bool {
        self.unresolved_bindings.contains(name)
            || self.scalar_types.contains_key(name)
            || self.struct_instances.contains_key(name)
            || self.array_types.contains_key(name)
            || self.buffer_types.contains_key(name)
            || self.buffer_array_lens.contains_key(name)
            || self.tuple_elem_types.contains_key(name)
    }

    pub(crate) fn shadow_binding(&mut self, name: &str) {
        let child_prefix = format!("{name}.");
        self.unresolved_bindings
            .retain(|binding| binding != name && !binding.starts_with(&child_prefix));
        self.scalar_types
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
        self.struct_instances
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
        self.array_types
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
        self.buffer_types
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
        self.buffer_array_lens
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
        self.tuple_elem_types
            .retain(|binding, _| binding != name && !binding.starts_with(&child_prefix));
    }

    /// Retains facts with a representable common type and shape on every path
    /// through a branch. Numeric scalars and tuple elements use the same join
    /// rule as return inference and MIR branch lowering.
    pub(crate) fn intersect_with(&mut self, other: &Self) {
        let common_bindings = self
            .binding_names()
            .intersection(&other.binding_names())
            .cloned()
            .collect::<HashSet<_>>();
        self.scalar_types = self
            .scalar_types
            .iter()
            .filter_map(|(name, lhs)| {
                let rhs = other.scalar_types.get(name)?;
                merge_inferred_return_types(*lhs, *rhs).map(|ty| (name.clone(), ty))
            })
            .collect();
        self.struct_instances
            .retain(|name, ty| other.struct_instances.get(name) == Some(ty));
        self.array_types
            .retain(|name, ty| other.array_types.get(name) == Some(ty));
        let common_buffer_types = self
            .buffer_types
            .iter()
            .filter_map(|(name, ty)| {
                (other.buffer_types.get(name) == Some(ty)
                    && self.buffer_array_lens.get(name) == other.buffer_array_lens.get(name))
                .then_some(name.clone())
            })
            .collect::<HashSet<_>>();
        self.buffer_types
            .retain(|name, _| common_buffer_types.contains(name));
        self.buffer_array_lens
            .retain(|name, _| common_buffer_types.contains(name));
        self.tuple_elem_types = self
            .tuple_elem_types
            .iter()
            .filter_map(|(name, lhs)| {
                let rhs = other.tuple_elem_types.get(name)?;
                if lhs.len() != rhs.len() {
                    return None;
                }
                lhs.iter()
                    .zip(rhs)
                    .map(|(lhs, rhs)| merge_inferred_return_types(*lhs, *rhs))
                    .collect::<Option<Vec<_>>>()
                    .map(|types| (name.clone(), types))
            })
            .collect();

        self.unresolved_bindings = common_bindings
            .into_iter()
            .filter(|name| !self.has_concrete_binding(name))
            .collect();
    }

    fn binding_names(&self) -> HashSet<String> {
        self.unresolved_bindings
            .iter()
            .chain(self.scalar_types.keys())
            .chain(self.struct_instances.keys())
            .chain(self.array_types.keys())
            .chain(self.buffer_types.keys())
            .chain(self.buffer_array_lens.keys())
            .chain(self.tuple_elem_types.keys())
            .cloned()
            .collect()
    }

    fn has_concrete_binding(&self, name: &str) -> bool {
        self.scalar_types.contains_key(name)
            || self.struct_instances.contains_key(name)
            || self.array_types.contains_key(name)
            || self.buffer_types.contains_key(name)
            || self.buffer_array_lens.contains_key(name)
            || self.tuple_elem_types.contains_key(name)
    }
}

pub(crate) fn resolved_buffer_channels(buffer_ty: &BufferType) -> TypedBufferChannels {
    match &buffer_ty.channels {
        BufferChannels::Mono => TypedBufferChannels::Mono,
        BufferChannels::Dynamic => TypedBufferChannels::Dynamic,
        BufferChannels::Static(expr) => const_positive_usize_for_call_type(expr)
            .map(TypedBufferChannels::Static)
            .unwrap_or(TypedBufferChannels::Dynamic),
    }
}

/// Checks the declared channel contract independently of the buffer element
/// type. Generic buffers use exactly the same shape rules as concrete ones.
pub(crate) fn score_buffer_channels(
    expected: &BufferChannels,
    actual: &TypedBufferChannels,
) -> Option<i32> {
    match expected {
        BufferChannels::Mono => match actual {
            TypedBufferChannels::Mono => Some(0),
            TypedBufferChannels::Static(ch) if *ch == 1 => Some(0),
            _ => None,
        },
        BufferChannels::Dynamic => match actual {
            TypedBufferChannels::Mono | TypedBufferChannels::Static(_) => Some(1),
            TypedBufferChannels::Dynamic => Some(0),
        },
        BufferChannels::Static(expr) => match const_positive_usize_for_call_type(expr) {
            Some(ch) if ch <= 1 => match actual {
                TypedBufferChannels::Mono => Some(0),
                TypedBufferChannels::Static(actual) if *actual == 1 => Some(0),
                _ => None,
            },
            Some(ch) => match actual {
                TypedBufferChannels::Static(actual) if *actual == ch => Some(0),
                _ => None,
            },
            None => match actual {
                TypedBufferChannels::Mono => None,
                TypedBufferChannels::Static(ch) if *ch > 1 => Some(1),
                _ => None,
            },
        },
    }
}

pub(crate) fn const_positive_usize_for_call_type(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Int { value, .. } if *value > 0 => usize::try_from(*value).ok(),
        Expr::Number { value, .. } if *value > 0.0 && value.fract() == 0.0 => {
            usize::try_from(*value as i64).ok()
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CallTypeContext<'a> {
    pub(crate) return_types: &'a HashMap<String, ReturnType>,
    pub(crate) struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
}

/// Whether a source signature still contains a call-site-dependent type or
/// shape and therefore cannot publish a concrete return type yet.
pub(crate) fn signature_has_dependent_call_types(
    signature: &FnSignature,
    generic_templates: &HashSet<String>,
    proc_types: &HashSet<String>,
) -> bool {
    signature.param_types.iter().any(Option::is_none)
        || signature_requires_monomorphization(signature, generic_templates, proc_types)
}

/// Whether a source signature has an ABI that cannot be fixed without a call
/// site. Data-struct array views are already concrete, whereas processor-array
/// parameters need a concrete capacity even when their nominal element type is
/// written explicitly.
pub(crate) fn signature_requires_monomorphization(
    signature: &FnSignature,
    generic_templates: &HashSet<String>,
    proc_types: &HashSet<String>,
) -> bool {
    !signature.type_params.is_empty()
        || signature.param_types.iter().any(|param_ty| match param_ty {
            Some(FnParamType::Struct(name)) => generic_templates.contains(name),
            Some(FnParamType::Array(None) | FnParamType::BareBuffer) => true,
            Some(FnParamType::ArrayGeneric(name)) => proc_types.contains(name),
            None
            | Some(
                FnParamType::Primitive(_)
                | FnParamType::Buffer(_)
                | FnParamType::BufferArray { .. }
                | FnParamType::Array(Some(_))
                | FnParamType::SizedArray { .. }
                | FnParamType::Tuple(_),
            ) => false,
        })
}

fn lookup_struct_field<'a>(
    name: &str,
    env: &CallTypeEnv,
    context: CallTypeContext<'a>,
) -> Option<&'a TypedStructField> {
    let mut path = name.split('.');
    let root = path.next()?;
    let mut fields = path.peekable();
    let mut struct_name = env.struct_instances.get(root)?.as_str();

    while let Some(field_name) = fields.next() {
        let field = resolve_struct_field_decl(struct_name, field_name, context.struct_defs)?;
        if fields.peek().is_none() {
            return Some(field);
        }
        struct_name = field.struct_name.as_deref()?;
    }
    None
}

pub(crate) fn infer_struct_symbol_type(
    name: &str,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<String> {
    env.struct_instances.get(name).cloned().or_else(|| {
        let field = lookup_struct_field(name, env, context)?;
        if !matches!(field.ty, TypedFieldType::Struct) {
            return None;
        }
        field.struct_name.clone()
    })
}

pub(crate) fn infer_struct_expr_type(
    expr: &Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<String> {
    match expr {
        Expr::Var { name, .. } => infer_struct_symbol_type(name, env, context),
        Expr::Index { base, .. } => {
            let array_ty = infer_array_symbol_type(base, env, context)?;
            match array_ty.elem {
                CallArrayElemType::Nominal(name) => Some(name),
                CallArrayElemType::Primitive(_) => None,
            }
        }
        Expr::UserCall { name, .. } if context.struct_defs.contains_key(name) => Some(name.clone()),
        _ => None,
    }
}

fn has_semantic_binding(name: &str, env: &CallTypeEnv, context: CallTypeContext<'_>) -> bool {
    env.has_binding(name)
        || name
            .split('.')
            .next()
            .is_some_and(|root| env.unresolved_bindings.contains(root))
        || lookup_struct_field(name, env, context).is_some()
}

pub(crate) fn infer_scalar_expr_type(
    expr: &Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { .. } => untyped_literal_type(expr),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Var { name, .. } => {
            if let Some(ty) = builtin_constant_type(name) {
                return Some(ty);
            }
            if let Some(ty) = env.scalar_types.get(name).copied() {
                return Some(ty);
            }
            match lookup_struct_field(name, env, context)?.ty {
                TypedFieldType::Scalar(ty) => Some(ty),
                TypedFieldType::Struct | TypedFieldType::Array(_) | TypedFieldType::Tuple(_) => {
                    None
                }
            }
        }
        Expr::Index { base, index, .. } => {
            if let Some(elem_ty) = infer_array_symbol_elem_type(base, env, context) {
                return Some(elem_ty);
            }
            if let Some((elem_ty, _)) = env.buffer_types.get(base) {
                return (!env.buffer_array_lens.contains_key(base)).then_some(*elem_ty);
            }
            let tuple_elems = infer_tuple_symbol_elem_types(base, env, context)?;
            let Expr::Int { value, .. } = index.as_ref() else {
                return None;
            };
            usize::try_from(*value)
                .ok()
                .and_then(|index| tuple_elems.get(index).copied())
        }
        Expr::UserCall { name, args, .. } => {
            if let Some(base) = parse_array_len_instance_base(name) {
                if infer_array_symbol_type(base, env, context).is_some()
                    || env.buffer_types.contains_key(base)
                {
                    return Some(PrimitiveType::I32);
                }
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                if env.buffer_types.contains_key(base) {
                    return Some(PrimitiveType::I32);
                }
            }
            if let Some(base) = parse_buffer_bound_instance_base(name) {
                if env.buffer_types.contains_key(base) {
                    return Some(PrimitiveType::Bool);
                }
            }
            if let Some(base) = parse_buffer_samplerate_instance_base(name) {
                if env.buffer_types.contains_key(base) {
                    return Some(PrimitiveType::F32);
                }
            }
            if is_internal_buffer_2d_fn(name) {
                if is_builtin_buffer_write_function_name(name) {
                    return None;
                }
                if let Some(first) = args.first() {
                    let base = match &first.expr {
                        Expr::Var { name: base, .. } | Expr::Index { base, .. } => base,
                        _ => return None,
                    };
                    if let Some((elem_ty, _)) = env.buffer_types.get(base) {
                        return Some(*elem_ty);
                    }
                    if let Some(elem_ty) = infer_array_symbol_elem_type(base, env, context) {
                        return Some(elem_ty);
                    }
                }
            }
            if name == &format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}") {
                let base = named_call_var_arg(args, PROC_INDEX_BASE_ARG)?;
                let field_name = named_call_var_arg(args, PROC_FIELD_SENTINEL_ARG)?;
                let array_ty = infer_array_symbol_type(base, env, context)?;
                let CallArrayElemType::Nominal(struct_name) = array_ty.elem else {
                    return None;
                };
                let field =
                    resolve_struct_field_decl(&struct_name, field_name, context.struct_defs)?;
                return match field.ty {
                    TypedFieldType::Scalar(ty) => Some(ty),
                    TypedFieldType::Struct
                    | TypedFieldType::Array(_)
                    | TypedFieldType::Tuple(_) => None,
                };
            }
            context.return_types.get(name).and_then(ReturnType::scalar)
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let lhs_ty = infer_scalar_expr_type(lhs, env, context)?;
            let rhs_ty = infer_scalar_expr_type(rhs, env, context)?;
            let (lhs_ty, rhs_ty) = adapt_binary_operand_types(lhs, rhs, lhs_ty, rhs_ty);
            match op {
                BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight => match (lhs_ty, rhs_ty) {
                    (PrimitiveType::I64, PrimitiveType::I32)
                    | (PrimitiveType::I32, PrimitiveType::I64)
                    | (PrimitiveType::I64, PrimitiveType::I64) => Some(PrimitiveType::I64),
                    (PrimitiveType::I32, PrimitiveType::I32) => Some(PrimitiveType::I32),
                    _ => None,
                },
                _ => merge_numeric_types_without_diagnostics(lhs_ty, rhs_ty),
            }
        }
        Expr::Compare { .. } | Expr::Logical { .. } | Expr::UnaryNot { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr, .. } => {
            let ty = infer_scalar_expr_type(expr, env, context)?;
            matches!(ty, PrimitiveType::I32 | PrimitiveType::I64).then_some(ty)
        }
        Expr::Call { func, args, .. } => {
            let arg_types = args
                .iter()
                .map(|arg| infer_scalar_expr_type(arg, env, context))
                .collect::<Option<Vec<_>>>()?;
            let arg_types = adapt_numeric_argument_types(args, &arg_types);
            intrinsic_result_type(*func, &arg_types)
        }
        Expr::ArrayLiteral { .. }
        | Expr::Tuple { .. }
        | Expr::Slice { .. }
        | Expr::ArrayCtor { .. } => None,
    }
}

fn named_call_var_arg<'a>(args: &'a [CallArg], arg_name: &str) -> Option<&'a str> {
    args.iter().find_map(|arg| {
        (arg.name.as_deref() == Some(arg_name))
            .then_some(&arg.expr)
            .and_then(|expr| match expr {
                Expr::Var { name, .. } => Some(name.as_str()),
                _ => None,
            })
    })
}

fn infer_array_symbol_elem_type(
    name: &str,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<PrimitiveType> {
    infer_array_symbol_type(name, env, context)?.primitive_elem()
}

fn infer_array_symbol_type(
    name: &str,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<CallArrayType> {
    if let Some(array_ty) = env.array_types.get(name) {
        return Some(array_ty.clone());
    }
    let field = lookup_struct_field(name, env, context)?;
    let TypedFieldType::Array(len) = field.ty else {
        return None;
    };
    if let Some(elem) = field.array_elem_ty {
        return Some(CallArrayType::primitive(elem, Some(len)));
    }
    field
        .array_elem_struct
        .as_ref()
        .map(|elem| CallArrayType::nominal(elem.clone(), Some(len)))
}

fn infer_tuple_symbol_elem_types(
    name: &str,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<Vec<PrimitiveType>> {
    if let Some(elem_types) = env.tuple_elem_types.get(name) {
        return Some(elem_types.clone());
    }
    match &lookup_struct_field(name, env, context)?.ty {
        TypedFieldType::Tuple(elem_types) => Some(elem_types.clone()),
        TypedFieldType::Scalar(_) | TypedFieldType::Struct | TypedFieldType::Array(_) => None,
    }
}

fn infer_array_literal_elem_type(
    values: &[Expr],
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<CallArrayElemType> {
    let mut elems = values.iter().map(|value| {
        let inferred = infer_scalar_expr_type(value, env, context);
        effective_untyped_assignment_type(value, inferred)
            .or(inferred)
            .map(CallArrayElemType::Primitive)
            .or_else(|| infer_struct_expr_type(value, env, context).map(CallArrayElemType::Nominal))
    });
    let first = elems.next()??;
    elems.try_fold(first, |merged, elem| merged.merge(elem?))
}

/// Infers the type established by an untyped array assignment. Array literals
/// acquire their element type from the first element, just like the semantic
/// analyzer and MIR lowerer; direct call literals remain free to merge all
/// elements contextually through `infer_array_arg_type`.
fn infer_assigned_array_type(
    expr: &Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<CallArrayType> {
    let Expr::ArrayLiteral { values, .. } = expr else {
        return infer_array_arg_type(expr, env, context);
    };
    let first = values.first()?;
    let inferred = infer_scalar_expr_type(first, env, context);
    let elem = effective_untyped_assignment_type(first, inferred)
        .or(inferred)
        .map(CallArrayElemType::Primitive)
        .or_else(|| infer_struct_expr_type(first, env, context).map(CallArrayElemType::Nominal))?;
    Some(CallArrayType {
        elem,
        len: Some(values.len()),
    })
}

pub(crate) fn infer_array_arg_type(
    expr: &Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<CallArrayType> {
    match expr {
        Expr::Var { name, .. } => infer_array_symbol_type(name, env, context),
        Expr::Slice { base, .. } => infer_array_symbol_type(base, env, context)
            .map(|mut ty| {
                ty.len = None;
                ty
            })
            .or_else(|| {
                env.buffer_types
                    .get(base)
                    .map(|(elem_ty, _)| CallArrayType::primitive(*elem_ty, None))
            }),
        Expr::ArrayLiteral { values, .. } => {
            let elem = infer_array_literal_elem_type(values, env, context)?;
            Some(CallArrayType {
                elem,
                len: Some(values.len()),
            })
        }
        Expr::ArrayCtor { spec, .. } => match &spec.elem {
            ArrayElemType::Primitive(elem_ty) => Some(CallArrayType::primitive(
                *elem_ty,
                const_positive_usize_for_call_type(&spec.size),
            )),
            ArrayElemType::Struct(name) => Some(CallArrayType::nominal(
                name.clone(),
                const_positive_usize_for_call_type(&spec.size),
            )),
        },
        _ => None,
    }
}

pub(crate) fn infer_buffer_arg_info(
    expr: &Expr,
    env: &CallTypeEnv,
) -> Option<(PrimitiveType, TypedBufferChannels)> {
    let name = match expr {
        Expr::Var { name, .. } => name,
        Expr::Index { base, .. } if env.buffer_array_lens.contains_key(base) => base,
        _ => return None,
    };
    env.buffer_types
        .get(name)
        .map(|(elem_ty, channels)| (*elem_ty, channels.clone()))
}

pub(crate) fn infer_tuple_arg_types(
    expr: &Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<Vec<PrimitiveType>> {
    match expr {
        Expr::Tuple { values, .. } => values
            .iter()
            .map(|value| {
                let inferred = infer_scalar_expr_type(value, env, context);
                effective_untyped_assignment_type(value, inferred).or(inferred)
            })
            .collect(),
        Expr::Var { name, .. } => infer_tuple_symbol_elem_types(name, env, context),
        Expr::UserCall { name, .. } => match context.return_types.get(name) {
            Some(ReturnType::Tuple(elem_types)) => Some(elem_types.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn update_call_type_env_after_assign(
    target: &AssignTarget,
    decl_ty: Option<PrimitiveType>,
    generic_decl_ty: Option<&str>,
    expr: &Expr,
    env: &mut CallTypeEnv,
    context: CallTypeContext<'_>,
) {
    if let AssignTarget::Tuple(names) = target {
        let elem_types = infer_tuple_arg_types(expr, env, context);
        for (index, target) in names.iter().enumerate() {
            let Some(name) = target.binding() else {
                continue;
            };
            if env.has_binding(name) {
                continue;
            }
            env.shadow_binding(name);
            if let Some(elem_ty) = elem_types
                .as_ref()
                .and_then(|elem_types| elem_types.get(index))
                .copied()
            {
                env.scalar_types.insert(name.to_owned(), elem_ty);
            } else {
                env.unresolved_bindings.insert(name.to_owned());
            }
        }
        return;
    }

    let AssignTarget::Var(name) = target else {
        return;
    };

    if let Some(declared) = decl_ty {
        env.shadow_binding(name);
        env.scalar_types.insert(name.clone(), declared);
        return;
    }
    if generic_decl_ty.is_some_and(|type_name| env.owner_type_params.contains(type_name)) {
        env.shadow_binding(name);
        env.unresolved_bindings.insert(name.clone());
        return;
    }

    // Reassignment changes a value, not the binding's semantic type. The RHS
    // may be contextually converted by the assignment after call inference.
    if has_semantic_binding(name, env, context) {
        return;
    }

    if let Some(buffer_info) = infer_buffer_arg_info(expr, env) {
        let buffer_array_len = match expr {
            Expr::Var { name: source, .. } => env.buffer_array_lens.get(source).copied(),
            _ => None,
        };
        env.shadow_binding(name);
        env.buffer_types.insert(name.clone(), buffer_info);
        if let Some(len) = buffer_array_len {
            env.buffer_array_lens.insert(name.clone(), len);
        }
        return;
    }
    if let Some(array_ty) = infer_assigned_array_type(expr, env, context) {
        env.shadow_binding(name);
        env.array_types.insert(name.clone(), array_ty);
        return;
    }
    if let Some(elem_types) = infer_tuple_arg_types(expr, env, context) {
        env.shadow_binding(name);
        env.tuple_elem_types.insert(name.clone(), elem_types);
        return;
    }
    if let Some(struct_name) = infer_struct_expr_type(expr, env, context) {
        env.shadow_binding(name);
        env.struct_instances.insert(name.clone(), struct_name);
        return;
    }
    let inferred = infer_scalar_expr_type(expr, env, context);
    if let Some(ty) = effective_untyped_assignment_type(expr, inferred).or(inferred) {
        env.shadow_binding(name);
        env.scalar_types.insert(name.clone(), ty);
        return;
    }

    env.shadow_binding(name);
    env.unresolved_bindings.insert(name.clone());
}
