use super::*;

impl SemanticConstArtifacts {
    pub(super) fn contains(&self, name: &str) -> bool {
        self.const_defs.contains_key(name) || self.const_values.contains_key(name)
    }

    /// Import visibility while retaining the declaration's identity and cache.
    pub(super) fn import(&mut self, name: &str, source: &Self) {
        if self.contains(name) {
            return;
        }
        if let Some(def) = source.const_defs.get(name) {
            self.const_defs.insert(name.to_owned(), def.clone());
        } else {
            self.const_values.import(name, &source.const_values);
            if let Some(info) = source.const_array_infos.get(name) {
                self.const_array_infos.insert(name.to_owned(), *info);
            }
        }
    }

    pub(super) fn import_all(&mut self, source: &Self) {
        for name in source.const_values.keys().chain(source.const_defs.keys()) {
            self.import(name, source);
        }
    }
}

pub(super) fn record_const_def_artifact(
    artifacts: &mut SemanticConstArtifacts,
    mut def: FunctionDef,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let name = def.name.clone();
    let resolve_size = |size: &Expr, context: &str, errors: &mut Vec<Diagnostic>| {
        eval_const_array_size_with_defs(
            size,
            &HashMap::new(),
            &HashMap::new(),
            &artifacts.const_values,
            const_def_registry(artifacts),
            options,
            context,
            std::slice::from_ref(&name),
            errors,
        )
    };
    let result = const_def_return_type(&def, resolve_size, errors);
    let params = const_def_param_signature(&def, resolve_size, errors);
    // Store concrete dimensions once. Every later type consumer sees precisely
    // this declaration metadata, independent of the caller's sample rate.
    if let Some(params) = &params {
        for (param, kind) in def.params.iter_mut().zip(params) {
            if let (
                Some(FnParamType::SizedArray { size, .. }),
                ConstDefParamKind::Array { len, .. },
            ) = (&mut param.ty, kind)
            {
                *size = Expr::int(*len as i64).with_loc(size.loc());
            }
        }
    }
    if let (Some(FnReturnType::Array { size, .. }), Some(ConstDefReturn::Array { len, .. })) =
        (&mut def.return_ty, result)
    {
        *size = Expr::int(len as i64).with_loc(size.loc());
    }
    // Const and runtime callables share default registration. The initializer
    // remains lazy, and its copy/reference semantics survive catalog capture.
    normalize_function_signature(&mut def, &HashSet::new(), artifacts, options, errors);
    let environment = ConstEnvironment::capture(
        &const_def_dependencies(&def, artifacts),
        &artifacts.const_values,
        &artifacts.const_defs,
    );
    let signature = const_def_signature(&def, result, &environment.defs);
    artifacts.const_defs.insert(
        name.clone(),
        std::rc::Rc::new(ConstDefinition {
            declaration: def,
            signature,
            param_kinds: params,
            result,
            environment,
            type_checks: RefCell::new(HashMap::new()),
        }),
    );
    let def = &artifacts.const_defs[&name];
    validate_const_def_declaration(def, errors);
    validate_const_def_types(def, errors);
}

fn infer_const_array_initializer_info(
    expr: &Expr,
    expected_elem: Option<PrimitiveType>,
    artifacts: &mut SemanticConstArtifacts,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedArrayInfo> {
    let locals = HashMap::new();
    let local_arrays = HashMap::new();
    let (elem_ty, len) = match expr {
        Expr::Var { name, .. } => {
            let info = artifacts.const_array_infos.get(name).or_else(|| {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: unknown const array '{name}'"),
                    expr.loc(),
                ));
                None
            })?;
            (info.elem_ty, info.len)
        }
        Expr::UserCall {
            name, type_args, ..
        } if type_args.is_empty() => {
            let def = artifacts.const_defs.get(name).or_else(|| {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: unknown const def '{name}'"),
                    expr.loc(),
                ));
                None
            })?;
            match def.result? {
                ConstDefReturn::Array { elem_ty, len } => (elem_ty, len),
                ConstDefReturn::Scalar(_) => {
                    errors.push(Diagnostic::semantic_span(
                        format!("{context}: const def '{name}' returns a scalar, not an array"),
                        expr.loc(),
                    ));
                    return None;
                }
            }
        }
        Expr::ArrayLiteral { values, .. } => {
            let first = values.first().or_else(|| {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: array literal cannot be empty"),
                    expr.loc(),
                ));
                None
            })?;
            let elem_ty = match expected_elem {
                Some(elem_ty) => elem_ty,
                None => ConstCheck::for_expression(first, artifacts)
                    .array_literal_element_type(first, errors)?,
            };
            (elem_ty, values.len())
        }
        Expr::ArrayCtor { spec, .. } => {
            let ArrayElemType::Primitive(elem_ty) = spec.elem else {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: const arrays can only use primitive element types"),
                    expr.loc(),
                ));
                return None;
            };
            let len = eval_const_array_size_with_defs(
                &spec.size,
                &locals,
                &local_arrays,
                &artifacts.const_values,
                const_def_registry(artifacts),
                options,
                context,
                &[],
                errors,
            )?;
            (elem_ty, len)
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            if selector.is_some() || channel.is_some() {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: const arrays do not support buffer coordinates"),
                    expr.loc(),
                ));
                return None;
            }
            let info = artifacts.const_array_infos.get(base).or_else(|| {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: unknown const array '{base}'"),
                    expr.loc(),
                ));
                None
            })?;
            let (start, end) = eval_const_slice_bounds_with_defs(
                base,
                info.len,
                start.as_deref(),
                end.as_deref(),
                &locals,
                &local_arrays,
                &artifacts.const_values,
                const_def_registry(artifacts),
                options,
                context,
                &[],
                errors,
            )?;
            (info.elem_ty, end - start)
        }
        _ => {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: expression does not describe a const array"),
                expr.loc(),
            ));
            return None;
        }
    };
    Some(TypedArrayInfo {
        // An annotation owns the element type. Inference supplies the length;
        // the ordinary array checker validates the initializer against both.
        elem_ty: expected_elem.unwrap_or(elem_ty),
        len,
        offset: 0,
    })
}

pub(super) fn const_scalar_type_map(
    artifacts: &SemanticConstArtifacts,
) -> HashMap<String, PrimitiveType> {
    artifacts
        .const_values
        .keys()
        .filter_map(|name| Some((name.clone(), artifacts.const_values.scalar_type(name)?)))
        .collect()
}

pub(super) fn record_const_array_artifact(
    decl: &onda_frontend::ConstDecl,
    artifacts: &mut SemanticConstArtifacts,
    evaluation_context: ConstContext,
    errors: &mut Vec<Diagnostic>,
) {
    let options = evaluation_context.declaration_options();
    let context = format!("const array '{}'", decl.name);
    let info = match &decl.ty {
        Some(ConstType::Array { elem, size }) => {
            let locals = HashMap::new();
            let local_arrays = HashMap::new();
            eval_const_array_size_with_defs(
                size,
                &locals,
                &local_arrays,
                &artifacts.const_values,
                const_def_registry(artifacts),
                options,
                &format!("{context} size"),
                &[],
                errors,
            )
            .map(|len| TypedArrayInfo {
                elem_ty: *elem,
                len,
                offset: 0,
            })
        }
        Some(ConstType::Slice { elem }) => infer_const_array_initializer_info(
            &decl.expr,
            Some(*elem),
            artifacts,
            options,
            &context,
            errors,
        ),
        Some(ConstType::Scalar(_)) => {
            record_const_scalar_artifact(decl, artifacts, options, errors);
            return;
        }
        None => infer_const_array_initializer_info(
            &decl.expr, None, artifacts, options, &context, errors,
        ),
    };
    if let Some(info) = info {
        let dependencies = const_decl_dependencies(decl, artifacts);
        artifacts.const_array_infos.insert(decl.name.clone(), info);
        artifacts.const_values.register(
            decl,
            Some(info),
            None,
            evaluation_context,
            &dependencies,
            &artifacts.const_defs,
        );
        validate_const_initializer(decl, &decl.name, artifacts, errors);
    }
}

fn const_decl_dependencies(
    decl: &onda_frontend::ConstDecl,
    artifacts: &SemanticConstArtifacts,
) -> ConstDependencies {
    let size = match &decl.ty {
        Some(ConstType::Array { size, .. }) => Some(size),
        _ => None,
    };
    const_dependencies_from_exprs(size.into_iter().chain([&decl.expr]), artifacts)
}

pub(super) fn record_const_scalar_artifact(
    decl: &ConstDecl,
    artifacts: &mut SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    register_scalar_const(decl, artifacts, options, errors);
    // Scalar declarations keep ordinary eager compile-time evaluation. Only
    // array payloads are deferred; a scalar element read is a required use.
    if errors.is_empty() {
        artifacts.const_values.resolve(&decl.name, errors);
    }
}

pub(super) fn register_scalar_const(
    decl: &ConstDecl,
    artifacts: &mut SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let check = ConstCheck::for_expression(&decl.expr, artifacts);
    let ty = check.scalar_constant_type(decl, errors);
    let dependencies = const_decl_dependencies(decl, artifacts);
    artifacts.const_values.register_named(
        &decl.name,
        decl,
        None,
        ty,
        ConstContext::Declaration(options),
        &dependencies,
        &artifacts.const_defs,
    );
}
