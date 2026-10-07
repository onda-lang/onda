//! Const declarations use ordinary semantic checking without running the interpreter.
use super::*;
use crate::decl_symbols::DeclaredSymbolInfo;
use crate::expr_analysis::{build_expr_env, ExprEnv};
use crate::expr_validation::{
    infer_array_initializer_type, validate_array_initializer, validate_numeric_selector,
};
use crate::stmt_analysis::{validate_assignment_binding, AssignmentBindingKind};

#[derive(Debug, Clone)]
enum ConstSignature {
    Definition(std::rc::Rc<ConstDefinition>),
    Template(std::rc::Rc<FnSignature>),
}

impl ConstSignature {
    fn signature(&self) -> &FnSignature {
        match self {
            Self::Definition(def) => &def.signature,
            Self::Template(signature) => signature,
        }
    }

    fn same_declaration(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Definition(lhs), Self::Definition(rhs)) => std::rc::Rc::ptr_eq(lhs, rhs),
            (Self::Template(lhs), Self::Template(rhs)) => std::rc::Rc::ptr_eq(lhs, rhs),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ConstCheck {
    symbols: DeclaredSymbolMap,
    scalars: LocalAliasTypes,
    locals: HashSet<String>,
    arrays: HashMap<String, LocalArrayAliasInfo>,
    // A fixed local dimension can remain unknown during static const checking.
    // Keep binding kind separate from that optional length metadata.
    fixed_arrays: HashSet<String>,
    lengths: HashMap<String, usize>,
    signatures: HashMap<String, ConstSignature>,
    array_returns: HashMap<String, (ArrayElemType, Option<usize>)>,
    deferred_type_params: HashSet<String>,
    empty_names: HashSet<String>,
    empty_bindings: HashMap<String, String>,
    empty_structs: HashMap<String, Vec<TypedStructField>>,
    pending_calls: Option<PendingConstCalls>,
    body_types: Option<std::rc::Rc<RefCell<ConstBodyTypes>>>,
}

impl ConstCheck {
    pub(super) fn contextual_numeric(&self, expr: &Expr) -> bool {
        crate::expr_typing::is_contextual_numeric_expr(expr, &self.symbols)
    }

    pub(super) fn empty() -> Self {
        let mut check = Self::default();
        check.symbols.constant_context = true;
        check
    }

    pub(super) fn for_expression(expr: &Expr, artifacts: &SemanticConstArtifacts) -> Self {
        let dependencies = const_dependencies_from_exprs([expr], artifacts);
        let environment = ConstEnvironment::capture(
            &dependencies,
            &artifacts.const_values,
            &artifacts.const_defs,
        );
        Self::capture(&environment.values, &environment.defs)
    }

    /// A body owns its local bindings and captures only referenced metadata.
    /// Signatures remain shared, so unrelated declarations add no copying cost.
    pub(super) fn definition_scope(&self, def: &FunctionDef) -> Self {
        let mut check = Self::empty();
        visit_const_def_references(def, &mut |expr| {
            let name = referenced_const_name(expr).or(match expr {
                Expr::UserCall { name, .. } => Some(name.as_str()),
                _ => None,
            });
            if let Some(name) = name {
                check.import(name, self, name);
            }
        });
        // Keep the declaration identity for recursion checks through aliases.
        check.import(&def.name, self, &def.name);
        check
    }

    fn flow_state(&self) -> ScopeFlowState {
        let mut state = ScopeFlowState::from_parts(
            self.locals.clone(),
            self.scalars.clone(),
            self.arrays.clone(),
            HashMap::new(),
        );
        for name in &self.symbols.unresolved_types {
            state.known_scalars.remove(name);
            state.local_aliases.remove(name);
        }
        state
    }
    pub(super) fn capture(values: &ConstValues, defs: ConstDefRegistry<'_>) -> Self {
        let mut check = Self::empty();
        for name in values.keys() {
            check.import_value(name, values);
        }
        for (name, def) in defs {
            check.function(name, def);
        }
        check
    }

    pub(super) fn import_artifact(&mut self, name: &str, artifacts: &SemanticConstArtifacts) {
        if let Some(def) = artifacts.const_defs.get(name) {
            self.function(name, def);
        } else {
            self.import_value(name, &artifacts.const_values);
        }
    }

    fn import_value(&mut self, name: &str, values: &ConstValues) {
        if let Some(info) = values.array_info(name) {
            self.symbols.insert(
                name.to_owned(),
                DeclaredSymbolInfo::ConstArray {
                    elem_ty: info.elem_ty,
                },
            );
            self.lengths.insert(name.to_owned(), info.len);
        } else if let Some(ty) = values.scalar_type(name) {
            self.symbols.insert(
                name.to_owned(),
                DeclaredSymbolInfo::Constant {
                    ty,
                    contextual: values
                        .entry(name)
                        .is_some_and(|entry| entry.contextual_numeric),
                    value: values.scalar_value(name),
                },
            );
        }
    }

    fn function(&mut self, name: &str, def: &std::rc::Rc<ConstDefinition>) {
        if let Some(ReturnType::Scalar(ty)) = def.signature.return_type {
            self.symbols
                .insert(name.to_owned(), DeclaredSymbolInfo::FunctionReturn { ty });
        }
        if let Some(ConstDefReturn::Array { elem_ty, len }) = def.result {
            self.array_returns.insert(
                name.to_owned(),
                (ArrayElemType::Primitive(elem_ty), Some(len)),
            );
        }
        self.signatures
            .insert(name.to_owned(), ConstSignature::Definition(def.clone()));
    }

    pub(super) fn defer_types(&mut self, names: &HashSet<String>) {
        self.deferred_type_params.extend(names.iter().cloned());
        self.symbols.unresolved_types.extend(names.iter().cloned());
    }

    pub(super) fn env(&self) -> ExprEnv<'_> {
        let mut env = build_expr_env(
            &self.locals,
            &self.scalars,
            &self.locals,
            &self.empty_names,
            &self.lengths,
            &self.symbols,
            &self.empty_bindings,
            &self.empty_bindings,
            &self.empty_structs,
            self,
            ScopeKind::Init,
        );
        env.local_aliases = &self.scalars;
        env.local_array_aliases = &self.arrays;
        env.allow_array_ctor = true;
        env.deferred_type_params = &self.deferred_type_params;
        env
    }

    fn scalar(&mut self, name: &str, ty: PrimitiveType) {
        self.symbols.unresolved_types.remove(name);
        self.arrays.remove(name);
        self.fixed_arrays.remove(name);
        self.lengths.remove(name);
        self.symbols.remove(name);
        self.locals.insert(name.to_owned());
        self.scalars.insert(name.to_owned(), ty);
    }

    pub(super) fn scalar_constant(&mut self, name: &str, ty: PrimitiveType, contextual: bool) {
        self.scalar(name, ty);
        self.symbols.insert(
            name.to_owned(),
            DeclaredSymbolInfo::Constant {
                ty,
                contextual,
                value: None,
            },
        );
    }

    fn array(&mut self, name: &str, elem_ty: PrimitiveType, len: Option<usize>) {
        self.fixed_arrays.insert(name.to_owned());
        self.symbols.unresolved_types.remove(name);
        self.locals.remove(name);
        self.scalars.remove(name);
        self.symbols
            .insert(name.to_owned(), DeclaredSymbolInfo::DataArray { elem_ty });
        self.lengths.remove(name);
        if let Some(len) = len {
            self.lengths.insert(name.to_owned(), len);
        }
        self.arrays.insert(
            name.to_owned(),
            LocalArrayAliasInfo {
                len: len.unwrap_or(1),
                static_len: len,
                proven_len: len.map(SliceLength::Known),
                elem_ty,
                elem_struct: None,
                writable: true,
            },
        );
    }

    pub(super) fn expression(
        &self,
        expr: &Expr,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<PrimitiveType> {
        self.record_reads(expr);
        crate::expr_validation::validate_scalar_expr(expr, "const expression", self.env(), errors)
    }

    pub(super) fn scalar_constant_type(
        &self,
        decl: &ConstDecl,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<PrimitiveType> {
        if let Some(ConstType::Scalar(ty)) = decl.ty {
            self.scalar_value(
                &decl.expr,
                ty,
                &format!("const '{}' initializer", decl.name),
                errors,
            );
            Some(ty)
        } else {
            // Scalar consts retain literal precision. Ordinary first-assignment
            // defaults apply to array elements and mutable bindings instead.
            self.expression(&decl.expr, errors).map(|ty| {
                if self.contextual_numeric(&decl.expr) {
                    crate::expr_typing::full_precision_numeric_type(ty)
                } else {
                    ty
                }
            })
        }
    }

    pub(super) fn array_literal_element_type(
        &self,
        expr: &Expr,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<PrimitiveType> {
        effective_untyped_assignment_type(expr, self.expression(expr, errors), &self.symbols)
    }

    fn record_reads(&self, expr: &Expr) {
        let Some(types) = &self.body_types else {
            return;
        };
        let mut types = types.borrow_mut();
        for node in expr.walk() {
            if let Expr::Var { name, .. } = node {
                if let Some(ty) = self
                    .scalars
                    .get(name)
                    .filter(|_| !self.symbols.unresolved_types.contains(name))
                {
                    types.reads.insert(node, *ty);
                }
            }
        }
    }

    pub(super) fn collect_body_types(&mut self) {
        self.body_types = Some(std::rc::Rc::new(RefCell::new(ConstBodyTypes::default())));
    }

    pub(super) fn into_body_types(mut self) -> std::rc::Rc<ConstBodyTypes> {
        let types = self
            .body_types
            .take()
            .expect("collecting checked body metadata");
        std::rc::Rc::new(
            std::rc::Rc::try_unwrap(types)
                .expect("body check released its metadata")
                .into_inner(),
        )
    }

    pub(super) fn scalar_value(
        &self,
        expr: &Expr,
        ty: PrimitiveType,
        context: &str,
        errors: &mut Vec<Diagnostic>,
    ) {
        let actual = self.expression(expr, errors);
        crate::expr_typing::require_expr_assignable_type(
            expr,
            actual,
            ty,
            context,
            errors,
            &self.symbols,
        );
    }

    pub(super) fn array_value(
        &self,
        expr: &Expr,
        elem_ty: Option<PrimitiveType>,
        len: Option<usize>,
        context: &str,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<usize> {
        self.record_reads(expr);
        if let Expr::ArrayCtor { spec, .. } = expr {
            self.dimension(&spec.size, context, errors);
        }
        validate_array_initializer(
            expr,
            elem_ty.map(ArrayElemType::Primitive),
            len,
            context,
            self.env(),
            errors,
        )
    }

    fn statements(
        &mut self,
        stmts: &[Stmt],
        result: ConstDefReturn<Option<usize>>,
        context: &str,
        errors: &mut Vec<Diagnostic>,
    ) {
        for stmt in stmts {
            match stmt {
                Stmt::Return { expr, .. } => match result {
                    ConstDefReturn::Scalar(ty) => self.scalar_value(expr, ty, context, errors),
                    ConstDefReturn::Array { elem_ty, len } => {
                        self.array_value(expr, Some(elem_ty), len, context, errors);
                    }
                },
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    target_loc,
                    decl_ty,
                    generic_decl_ty,
                    is_typed_decl,
                    expr,
                    ..
                } => {
                    let array = match decl_ty {
                        Some(DeclType::Array { elem, size }) => {
                            let len = self.dimension(size, context, errors);
                            Some((Some(*elem), len))
                        }
                        Some(_) => None,
                        None => self
                            .arrays
                            .get(name)
                            .map(|array| {
                                let elem = (!self.symbols.unresolved_types.contains(name))
                                    .then_some(array.elem_ty);
                                (elem, array.static_len)
                            })
                            .or_else(|| {
                                let (elem, len) = infer_array_initializer_type(expr, self.env())?;
                                match elem {
                                    Some(ArrayElemType::Primitive(elem)) => Some((Some(elem), len)),
                                    None => Some((None, len)),
                                    Some(ArrayElemType::Struct(_)) => None,
                                }
                            }),
                    };
                    // Captured globals can be shadowed by a fresh local. Only
                    // bindings introduced in this body constrain assignment.
                    let existing = if self.arrays.contains_key(name) {
                        Some(AssignmentBindingKind::Data)
                    } else if self.locals.contains(name) {
                        Some(AssignmentBindingKind::Scalar)
                    } else {
                        None
                    };
                    let assigned = if array.is_some() {
                        AssignmentBindingKind::Data
                    } else {
                        AssignmentBindingKind::Scalar
                    };
                    if !validate_assignment_binding(
                        name,
                        existing,
                        assigned,
                        *is_typed_decl || decl_ty.is_some() || generic_decl_ty.is_some(),
                        target_loc.as_ref().into(),
                        errors,
                    ) {
                        continue;
                    }
                    if let Some((elem, len)) = array {
                        let previous = self.arrays.get(name).cloned();
                        if let Some(binding) = &previous {
                            if !crate::stmt_analysis::validate_array_binding_replacement(
                                name,
                                binding,
                                expr,
                                !self.fixed_arrays.contains(name),
                                target_loc.as_ref().into(),
                                errors,
                            ) {
                                continue;
                            }
                        }
                        let source = if decl_ty.is_none() && previous.is_none() {
                            crate::stmt_analysis::resolve_executable_data_like_info(
                                expr,
                                self.env(),
                                errors,
                            )
                        } else {
                            None
                        };
                        let slice = previous.is_none()
                            && source.is_some()
                            && match expr {
                                Expr::Slice { .. } => true,
                                Expr::Var { name, .. } => {
                                    self.arrays.contains_key(name)
                                        && !self.fixed_arrays.contains(name)
                                }
                                _ => false,
                            };
                        let len = self.array_value(expr, elem, len, context, errors);
                        self.array(name, elem.unwrap_or(PrimitiveType::F32), len);
                        if let Some(binding) = previous.or(source) {
                            self.arrays.insert(name.clone(), binding);
                        }
                        if slice {
                            self.fixed_arrays.remove(name);
                        }
                        if elem.is_none() {
                            self.symbols.unresolved_types.insert(name.clone());
                        }
                        if let Some(types) = &self.body_types {
                            types
                                .borrow_mut()
                                .arrays
                                .insert(stmt, ConstArrayExpectation { elem_ty: elem, len });
                        }
                    } else {
                        let actual = self.expression(expr, errors);
                        let ty = match decl_ty {
                            Some(DeclType::Scalar(ty)) => Some(*ty),
                            _ => self.scalars.get(name).copied().or_else(|| {
                                effective_untyped_assignment_type(expr, actual, &self.symbols)
                            }),
                        };
                        if let Some(ty) = ty {
                            crate::expr_typing::require_expr_assignable_type(
                                expr,
                                actual,
                                ty,
                                context,
                                errors,
                                &self.symbols,
                            );
                            self.scalar(name, ty);
                            if let Some(types) = &self.body_types {
                                types.borrow_mut().scalars.insert(stmt, ty);
                            }
                        } else {
                            self.scalar(name, PrimitiveType::F32);
                            self.symbols.unresolved_types.insert(name.clone());
                        }
                    }
                }
                Stmt::Assign {
                    target: AssignTarget::Index { base, index, .. },
                    expr,
                    ..
                } => {
                    self.record_reads(index);
                    validate_numeric_selector(index, "array index expression", self.env(), errors);
                    if matches!(
                        self.symbols.get(base),
                        Some(DeclaredSymbolInfo::ConstArray { .. })
                    ) {
                        errors.push(Diagnostic::semantic_span(
                            format!("{context}: cannot write const array '{base}'"),
                            stmt.loc(),
                        ));
                    } else if let Some(array) = self.arrays.get(base) {
                        crate::stmt_analysis::validate_array_write(base, array, stmt.loc(), errors);
                        self.scalar_value(expr, array.elem_ty, context, errors);
                    } else {
                        errors.push(Diagnostic::semantic_span(
                            format!("{context}: unknown local array '{base}'"),
                            stmt.loc(),
                        ));
                    }
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.scalar_value(cond, PrimitiveType::Bool, context, errors);
                    let mut then_scope = self.clone();
                    let mut else_scope = self.clone();
                    then_scope.statements(then_branch, result, context, errors);
                    else_scope.statements(else_branch, result, context, errors);
                    use crate::def_semantics::{statement_list_flow, StatementFlow};
                    match (
                        statement_list_flow(then_branch),
                        statement_list_flow(else_branch),
                    ) {
                        (StatementFlow::Terminates, StatementFlow::Continues) => {
                            *self = else_scope;
                            continue;
                        }
                        (StatementFlow::Continues, StatementFlow::Terminates) => {
                            *self = then_scope;
                            continue;
                        }
                        _ => {}
                    }
                    let mut joined = self.flow_state();
                    let mut then_state = then_scope.flow_state();
                    let mut else_state = else_scope.flow_state();
                    // A valid join constrains an unknown length to its known
                    // sibling. Keep that fact for subsequent checking, and
                    // validate the selected array when evaluation resolves it.
                    let mut deferred_arrays = Vec::new();
                    for (name, array) in &mut then_state.local_array_aliases {
                        if let Some(other) = else_state.local_array_aliases.get_mut(name) {
                            if array.static_len.is_none() != other.static_len.is_none() {
                                let len = array.static_len.or(other.static_len);
                                array.static_len = len;
                                other.static_len = len;
                                array.proven_len = len.map(SliceLength::Known);
                                other.proven_len = len.map(SliceLength::Known);
                                deferred_arrays.push(name.clone());
                            }
                        }
                    }
                    let bindings = joined.join_branches(
                        then_state,
                        statement_list_flow(then_branch),
                        else_state,
                        statement_list_flow(else_branch),
                        stmt.loc(),
                        errors,
                    );
                    if let Some(types) = &self.body_types {
                        let mut joins = deferred_arrays
                            .into_iter()
                            .filter_map(|name| {
                                let array = joined.local_array_aliases.get(&name)?;
                                Some((
                                    name,
                                    ConstArrayExpectation::fixed(array.elem_ty, array.static_len?),
                                ))
                            })
                            .collect::<Vec<_>>();
                        if !joins.is_empty() || !bindings.is_empty() {
                            joins.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
                            types.borrow_mut().branch_scopes.insert(
                                stmt,
                                ConstBranchScope {
                                    bindings,
                                    arrays: joins,
                                },
                            );
                        }
                    }
                    for (name, ty) in joined.local_aliases {
                        self.scalar(&name, ty);
                    }
                    for (name, array) in joined.local_array_aliases {
                        self.array(&name, array.elem_ty, array.static_len);
                        self.arrays.insert(name.clone(), array);
                        if !then_scope.fixed_arrays.contains(&name)
                            || !else_scope.fixed_arrays.contains(&name)
                        {
                            self.fixed_arrays.remove(&name);
                        }
                    }
                    // Deferred scalar types cannot join until specialization.
                    for (name, ty) in &then_scope.scalars {
                        if else_scope.scalars.contains_key(name)
                            && (then_scope.symbols.unresolved_types.contains(name)
                                || else_scope.symbols.unresolved_types.contains(name))
                        {
                            self.scalar(name, *ty);
                            self.symbols.unresolved_types.insert(name.clone());
                        }
                    }
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
                    for expr in [Some(start), Some(end), step.as_ref()]
                        .into_iter()
                        .flatten()
                    {
                        let ty = self.expression(expr, errors);
                        if ty.is_some_and(|ty| {
                            !matches!(ty, PrimitiveType::I32 | PrimitiveType::I64)
                        }) {
                            errors.push(Diagnostic::semantic_span(
                                format!("{context}: loop bounds require integers"),
                                expr.loc(),
                            ));
                        }
                    }
                    let mut scope = self.clone();
                    scope.scalar(var, *var_ty);
                    scope.statements(body, result, context, errors);
                }
                _ => {} // The existing const statement policy diagnoses unsupported forms.
            }
        }
    }
}

pub(super) fn validate_const_initializer(
    decl: &ConstDecl,
    name: &str,
    artifacts: &SemanticConstArtifacts,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(entry) = artifacts.const_values.entry(name) else {
        return;
    };
    let check = ConstCheck::capture(&entry.environment.values, &entry.environment.defs);
    let context = format!("const '{}' initializer", decl.name);
    if let Some(info) = artifacts.const_values.array_info(name) {
        check.array_value(
            &decl.expr,
            Some(info.elem_ty),
            Some(info.len),
            &context,
            errors,
        );
    } else if let Some(ty) = artifacts.const_values.scalar_type(name) {
        check.scalar_value(&decl.expr, ty, &context, errors);
    }
}

pub(super) fn validate_const_def_types(
    def: &std::rc::Rc<ConstDefinition>,
    errors: &mut Vec<Diagnostic>,
) {
    let (Some(params), Some(result)) = (&def.param_kinds, def.result) else {
        return;
    };
    let mut check = ConstCheck::capture(&def.environment.values, &def.environment.defs);
    check.function(&def.name, def);
    let concrete = !params
        .iter()
        .any(|kind| matches!(kind, ConstDefParamKind::Slice { .. }));
    if concrete {
        check.collect_body_types();
    }
    let start = errors.len();
    check.definition(&def.declaration, params, result, errors);
    if concrete {
        let result = if errors.len() == start {
            Ok(check.into_body_types())
        } else {
            Err(errors[start..].to_vec())
        };
        def.type_checks.borrow_mut().insert(Vec::new(), result);
    }
}

impl ConstCheck {
    /// Resolve dimensions from syntax and eager scalar/length metadata, without
    /// executing constant bodies or reading array payloads.
    pub(super) fn dimension(
        &self,
        expr: &Expr,
        context: &str,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<usize> {
        let ty = self.expression(expr, errors);
        if ty == Some(PrimitiveType::Bool) {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: array size requires an integer"),
                expr.loc(),
            ));
            return None;
        }
        let size = crate::expr_validation::fold_array_length_metadata(expr, self.env())?;
        eval_array_size_expr(&size, AnalysisOptions::default(), context, errors)
    }

    pub(super) fn constant(&mut self, decl: &ConstDecl, errors: &mut Vec<Diagnostic>) {
        let context = format!("const '{}' initializer", decl.name);
        let inferred = infer_array_initializer_type(&decl.expr, self.env());
        let array = match &decl.ty {
            Some(ConstType::Scalar(ty)) => {
                self.scalar_constant_type(decl, errors);
                self.scalar_constant(&decl.name, *ty, false);
                return;
            }
            Some(ConstType::Array { elem, size }) => {
                Some((Some(*elem), self.dimension(size, &context, errors)))
            }
            Some(ConstType::Slice { elem }) => {
                Some((Some(*elem), inferred.as_ref().and_then(|(_, len)| *len)))
            }
            None => inferred.and_then(|(elem, len)| match elem {
                Some(ArrayElemType::Primitive(elem)) => Some((Some(elem), len)),
                None => Some((None, len)),
                _ => None,
            }),
        };
        if let Some((elem, len)) = array {
            let len = self.array_value(&decl.expr, elem, len, &context, errors);
            let unknown_element = elem.is_none();
            let elem = elem.unwrap_or(PrimitiveType::F32);
            self.array(&decl.name, elem, len);
            self.arrays.get_mut(&decl.name).unwrap().writable = false;
            self.symbols.insert(
                decl.name.clone(),
                DeclaredSymbolInfo::ConstArray { elem_ty: elem },
            );
            if unknown_element {
                self.symbols.unresolved_types.insert(decl.name.clone());
            }
        } else {
            let ty = self.scalar_constant_type(decl, errors);
            if let Some(ty) = ty {
                let contextual = self.contextual_numeric(&decl.expr);
                // Host-dependent template values wait for instantiation.
                let independent = decl.expr.walk().all(|node| {
                    !matches!(node, Expr::Var { name, .. } if builtin_constant_type(name).is_some())
                });
                let value = (contextual && ty == PrimitiveType::I64 && independent)
                    .then(|| self.symbols.constant_integer(&decl.expr, &mut Vec::new()))
                    .flatten()
                    .map(TypedConstValue::I64);
                self.scalar_constant(&decl.name, ty, contextual);
                if let Some(DeclaredSymbolInfo::Constant { value: stored, .. }) =
                    self.symbols.get_mut(&decl.name)
                {
                    *stored = value;
                }
            } else {
                self.scalar_constant(&decl.name, PrimitiveType::F32, false);
                self.symbols.unresolved_types.insert(decl.name.clone());
            }
        }
    }

    pub(super) fn template_function(
        &mut self,
        def: &FunctionDef,
        result: ConstDefReturn<Option<usize>>,
    ) {
        let signature = const_def_signature(def, Some(result), self);
        match result {
            ConstDefReturn::Scalar(ty) => {
                self.symbols
                    .insert(def.name.clone(), DeclaredSymbolInfo::FunctionReturn { ty });
            }
            ConstDefReturn::Array { elem_ty, len } => {
                self.array_returns
                    .insert(def.name.clone(), (ArrayElemType::Primitive(elem_ty), len));
            }
        }
        self.signatures.insert(
            def.name.clone(),
            ConstSignature::Template(std::rc::Rc::new(signature)),
        );
    }

    /// Bind an alternate source spelling to existing metadata, without values.
    pub(super) fn import(&mut self, name: &str, source: &Self, target: &str) -> bool {
        self.bind(name, source.binding(target))
    }

    pub(super) fn alias(&mut self, name: &str, target: &str) -> bool {
        if name == target {
            return self.contains(target);
        }
        self.bind(name, self.binding(target))
    }

    fn binding(&self, name: &str) -> Option<ConstBinding> {
        self.contains(name).then(|| ConstBinding {
            symbol: self.symbols.get(name).cloned(),
            scalar: self.scalars.get(name).copied(),
            array: self.arrays.get(name).cloned(),
            len: self.lengths.get(name).copied(),
            signature: self.signatures.get(name).cloned(),
            array_return: self.array_returns.get(name).cloned(),
            unresolved: self.symbols.unresolved_types.contains(name),
        })
    }

    fn clear_binding(&mut self, name: &str) {
        self.symbols.remove(name);
        self.symbols.unresolved_types.remove(name);
        self.scalars.remove(name);
        self.locals.remove(name);
        self.arrays.remove(name);
        self.fixed_arrays.remove(name);
        self.lengths.remove(name);
        self.signatures.remove(name);
        self.array_returns.remove(name);
    }

    fn bind(&mut self, name: &str, binding: Option<ConstBinding>) -> bool {
        let Some(binding) = binding else {
            return false;
        };
        self.clear_binding(name);
        if let Some(symbol) = binding.symbol {
            self.symbols.insert(name.to_owned(), symbol);
        }
        if let Some(ty) = binding.scalar {
            self.scalars.insert(name.to_owned(), ty);
            self.locals.insert(name.to_owned());
        }
        if let Some(array) = binding.array {
            self.arrays.insert(name.to_owned(), array);
        }
        if let Some(len) = binding.len {
            self.lengths.insert(name.to_owned(), len);
        }
        if let Some(signature) = binding.signature {
            self.signatures.insert(name.to_owned(), signature);
        }
        if let Some(result) = binding.array_return {
            self.array_returns.insert(name.to_owned(), result);
        }
        if binding.unresolved {
            self.symbols.unresolved_types.insert(name.to_owned());
        }
        true
    }

    pub(super) fn import_namespace(&mut self, namespace: &str, source: &Self) {
        let prefix = format!("{namespace}::");
        for name in source
            .symbols
            .keys()
            .chain(source.scalars.keys())
            .chain(source.signatures.keys())
        {
            if name.starts_with(&prefix) {
                self.import(name, source, name);
            }
        }
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.symbols.contains_key(name)
            || self.scalars.contains_key(name)
            || self.arrays.contains_key(name)
            || self.signatures.contains_key(name)
    }

    pub(super) fn definition<L: Copy + Into<Option<usize>>>(
        &mut self,
        def: &FunctionDef,
        params: &[ConstDefParamKind<L>],
        result: ConstDefReturn<L>,
        errors: &mut Vec<Diagnostic>,
    ) {
        visit_const_def_references(def, &mut |expr| {
            if matches!(expr, Expr::UserCall { name, .. } if name == &def.name || self.signatures.get(name).zip(self.signatures.get(&def.name)).is_some_and(|(called, own)| called.same_declaration(own)))
            {
                errors.push(Diagnostic::semantic_span(
                    format!("recursive const def call involving '{}'", def.name),
                    expr.loc(),
                ));
            }
        });
        crate::def_semantics::validate_def_return_control_flow(
            std::slice::from_ref(def),
            self,
            errors,
        );
        let context = format!("const def '{}'", def.name);
        for (param, kind) in def.params.iter().zip(params) {
            if let Some(default) = &param.default {
                if !matches!(kind, ConstDefParamKind::Scalar(_)) {
                    let readonly = self.signatures.get(&def.name).is_some_and(|signature| {
                        signature
                            .signature()
                            .readonly_data_params
                            .contains(&param.name)
                    });
                    crate::expr_validation::reject_immutable_array_call_arg(
                        &def.name,
                        &param.name,
                        readonly,
                        default,
                        self.env(),
                        default.loc(),
                        errors,
                    );
                }
                match kind {
                    ConstDefParamKind::Scalar(ty) => {
                        self.scalar_value(default, *ty, &context, errors)
                    }
                    ConstDefParamKind::Array { elem_ty, len } => {
                        self.array_value(default, Some(*elem_ty), (*len).into(), &context, errors);
                    }
                    ConstDefParamKind::Slice {
                        elem_ty: Some(elem_ty),
                    } => {
                        self.array_value(default, Some(*elem_ty), None, &context, errors);
                    }
                    ConstDefParamKind::Slice { elem_ty: None } => {
                        self.array_value(default, None, None, &context, errors);
                    }
                }
            }
        }
        self.body(def, params, result, None, errors);
    }

    pub(super) fn defer_calls(&mut self) -> PendingConstCalls {
        let pending = std::rc::Rc::new(RefCell::new(Vec::new()));
        self.pending_calls = Some(pending.clone());
        pending
    }

    pub(super) fn body<L: Copy + Into<Option<usize>>>(
        &mut self,
        def: &FunctionDef,
        params: &[ConstDefParamKind<L>],
        result: ConstDefReturn<L>,
        arrays: Option<&[ConstArrayExpectation]>,
        errors: &mut Vec<Diagnostic>,
    ) {
        // Template metadata also uses these maps to infer global declarations.
        // A function starts with no local bindings; globals remain in symbols
        // and lengths, so fresh locals can shadow them after their initializer.
        self.locals.clear();
        self.scalars.clear();
        self.arrays.clear();
        self.fixed_arrays.clear();
        for (index, (param, kind)) in def.params.iter().zip(params).enumerate() {
            match kind {
                ConstDefParamKind::Scalar(ty) => self.scalar(&param.name, *ty),
                ConstDefParamKind::Array { elem_ty, len } => {
                    self.array(&param.name, *elem_ty, (*len).into())
                }
                ConstDefParamKind::Slice { elem_ty } => {
                    let metadata =
                        arrays.map_or_else(ConstArrayExpectation::any, |arrays| arrays[index]);
                    let elem_ty = elem_ty.or(metadata.elem_ty);
                    self.array(
                        &param.name,
                        elem_ty.unwrap_or(PrimitiveType::F32),
                        metadata.len,
                    );
                    self.arrays.get_mut(&param.name).unwrap().static_len = None;
                    self.fixed_arrays.remove(&param.name);
                    if elem_ty.is_none() {
                        self.symbols.unresolved_types.insert(param.name.clone());
                    }
                }
            }
            if param.readonly {
                if let Some(array) = self.arrays.get_mut(&param.name) {
                    array.writable = false;
                }
            }
        }
        let result = match result {
            ConstDefReturn::Scalar(ty) => ConstDefReturn::Scalar(ty),
            ConstDefReturn::Array { elem_ty, len } => ConstDefReturn::Array {
                elem_ty,
                len: len.into(),
            },
        };
        let context = format!("const def '{}'", def.name);
        self.statements(&def.body, result, &context, errors);
    }
}

struct ConstBinding {
    symbol: Option<DeclaredSymbolInfo>,
    scalar: Option<PrimitiveType>,
    array: Option<LocalArrayAliasInfo>,
    len: Option<usize>,
    signature: Option<ConstSignature>,
    array_return: Option<(ArrayElemType, Option<usize>)>,
    unresolved: bool,
}

impl crate::expr_analysis::SignatureLookup for ConstCheck {
    fn get(&self, name: &str) -> Option<&FnSignature> {
        self.signatures.get(name).map(ConstSignature::signature)
    }

    fn array_return(&self, name: &str) -> Option<(ArrayElemType, Option<usize>)> {
        self.array_returns.get(name).cloned()
    }

    fn validate_const_call(
        &self,
        name: &str,
        args: &[Option<&Expr>],
        env: ExprEnv<'_>,
        errors: &mut Vec<Diagnostic>,
    ) {
        if let Some(ConstSignature::Definition(def)) = self.signatures.get(name) {
            if let Some(metadata) = const_call_metadata(def, args, env) {
                if let Some(pending) = &self.pending_calls {
                    pending.borrow_mut().push((def.clone(), metadata));
                } else {
                    validate_const_call_metadata(def, &metadata, errors);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_scopes_bound_metadata_to_free_references_and_share_signatures() {
        let program = onda_frontend::parse_program(
            "const def helper(x: i32) -> i32:\n  return x\nconst def read(x: i32 = Fallback) -> i32[Count]:\n  values: i32[Count] = Table\n  values[0] = helper(x)\n  return values\n",
        ).unwrap();
        let Block::Def(helper) = &program.blocks[0] else {
            panic!("expected helper")
        };
        let Block::Def(read) = &program.blocks[1] else {
            panic!("expected read")
        };
        let mut catalog = ConstCheck::default();
        for name in ["Count", "Fallback", "x", "values"] {
            catalog.scalar_constant(name, PrimitiveType::I32, false);
        }
        catalog.array("Table", PrimitiveType::I32, Some(2));
        catalog.template_function(helper, ConstDefReturn::Scalar(PrimitiveType::I32));
        catalog.template_function(
            read,
            ConstDefReturn::Array {
                elem_ty: PrimitiveType::I32,
                len: Some(2),
            },
        );
        for index in 0..1024 {
            let mut unrelated = helper.clone();
            unrelated.name = format!("unrelated_{index}");
            catalog.template_function(&unrelated, ConstDefReturn::Scalar(PrimitiveType::I32));
        }

        let scope = catalog.definition_scope(read);
        assert_eq!(scope.symbols.keys().count(), 4);
        assert_eq!(scope.signatures.len(), 2);
        assert_eq!(scope.array_returns.len(), 1);
        assert_eq!(scope.lengths.get("Table"), Some(&2));
        assert!(!scope.contains("x"));
        assert!(!scope.contains("values"));
        for name in ["helper", "read"] {
            assert!(scope.signatures[name].same_declaration(&catalog.signatures[name]));
        }
    }
}
