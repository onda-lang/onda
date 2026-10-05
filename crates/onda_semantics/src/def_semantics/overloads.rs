use super::call_types::{
    const_positive_usize_for_call_type, infer_array_arg_type, infer_buffer_arg_info,
    infer_scalar_expr_type, infer_struct_expr_type, infer_tuple_arg_types, score_buffer_channels,
    CallArrayElemType, CallArrayType, CallTypeContext, CallTypeEnv, CallTypeRewriter,
};
use crate::*;

#[derive(Debug, Clone)]
pub(crate) struct OverloadCandidate {
    internal_name: String,
    signature: std::rc::Rc<FnSignature>,
}

#[derive(Debug, Clone)]
enum OverloadArgShape {
    Scalar(PrimitiveType),
    Struct(String),
    Array(CallArrayType),
    Buffer {
        elem_ty: PrimitiveType,
        channels: TypedBufferChannels,
        collection_len: Option<usize>,
    },
    Tuple(Vec<PrimitiveType>),
    Unknown,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct OverloadOwnerContext {
    pub(crate) defer_dependent_calls: bool,
}

fn overload_internal_name(public_name: &str, ordinal: usize) -> String {
    format!(
        "__onda_ovl_{}_{}",
        crate::internal_names::encode_internal_symbol_component(public_name),
        ordinal
    )
}

fn infer_overload_arg_shape(
    expr: &Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> OverloadArgShape {
    if let Some(struct_name) = infer_struct_expr_type(expr, env, context) {
        return OverloadArgShape::Struct(struct_name);
    }
    match expr {
        Expr::Var { name, .. } => {
            if let Some((elem_ty, channels)) = env.buffer_types.get(name) {
                return OverloadArgShape::Buffer {
                    elem_ty: *elem_ty,
                    channels: channels.clone(),
                    collection_len: env.buffer_array_lens.get(name).copied(),
                };
            }
            if let Some(array_ty) = infer_array_arg_type(expr, env, context) {
                return OverloadArgShape::Array(array_ty);
            }
            if let Some(elem_types) = infer_tuple_arg_types(expr, env, context) {
                return OverloadArgShape::Tuple(elem_types);
            }
        }
        Expr::Index { base, .. } if env.buffer_array_lens.contains_key(base) => {
            if let Some((elem_ty, channels)) = infer_buffer_arg_info(expr, env) {
                return OverloadArgShape::Buffer {
                    elem_ty,
                    channels,
                    collection_len: None,
                };
            }
        }
        _ => {
            if let Some(array_ty) = infer_array_arg_type(expr, env, context) {
                return OverloadArgShape::Array(array_ty);
            }
            if let Some(elem_types) = infer_tuple_arg_types(expr, env, context) {
                return OverloadArgShape::Tuple(elem_types);
            }
        }
    }
    if let Some(ty) = infer_scalar_expr_type(expr, env, context) {
        OverloadArgShape::Scalar(ty)
    } else {
        OverloadArgShape::Unknown
    }
}

fn score_buffer_match(
    expected: &BufferType,
    arg_elem_ty: PrimitiveType,
    arg_channels: &TypedBufferChannels,
) -> Option<i32> {
    let elem_score = match expected.elem {
        BufferElemType::Primitive(ty) if ty == arg_elem_ty => 0,
        BufferElemType::Primitive(_) => return None,
        BufferElemType::Generic(_) => 2,
    };
    score_buffer_channels(&expected.channels, arg_channels)
        .map(|shape_score| elem_score + shape_score)
}

fn score_contextual_array_literal(
    arg_expr: &Expr,
    expected: PrimitiveType,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<i32> {
    let Expr::ArrayLiteral { values, .. } = arg_expr else {
        return None;
    };
    values.iter().try_fold(0, |score, value| {
        let actual = infer_scalar_expr_type(value, env, context)?;
        if actual == expected {
            Some(score)
        } else if can_assign_expr_to_type(value, actual, expected, &env.const_symbols) {
            Some(score.max(1))
        } else {
            None
        }
    })
}

fn score_tuple_match(
    arg_expr: &Expr,
    actual: &[PrimitiveType],
    expected: &[PrimitiveType],
    env: &CallTypeEnv,
) -> Option<i32> {
    if actual.len() != expected.len() {
        return None;
    }
    if actual == expected {
        return Some(0);
    }
    let literal_values = match arg_expr {
        Expr::Tuple { values, .. } if values.len() == expected.len() => Some(values.as_slice()),
        _ => None,
    };
    actual
        .iter()
        .zip(expected)
        .enumerate()
        .try_fold(0, |score, (index, (actual, expected))| {
            let assignable = literal_values.is_some_and(|values| {
                can_assign_expr_to_type(&values[index], *actual, *expected, &env.const_symbols)
            }) || can_implicitly_assign(*actual, *expected);
            assignable.then_some(score.max(i32::from(actual != expected)))
        })
}

#[derive(Debug)]
struct OverloadParamMatch {
    score: i32,
    generic_constraints: Vec<(String, PrimitiveType, bool)>,
}

fn plain_match(score: i32) -> OverloadParamMatch {
    OverloadParamMatch {
        score,
        generic_constraints: Vec::new(),
    }
}

fn generic_match(name: &str, actual: PrimitiveType, exact: bool, score: i32) -> OverloadParamMatch {
    OverloadParamMatch {
        score,
        generic_constraints: vec![(name.to_owned(), actual, exact)],
    }
}

fn nominal_match(
    actual: &str,
    expected: &str,
    type_params: &[String],
) -> Option<OverloadParamMatch> {
    if actual == expected {
        return Some(plain_match(0));
    }
    let crate::generic_specialization::NamedTypePatternMatch::Matched {
        bindings,
        type_parameter_count,
    } = crate::generic_specialization::match_named_type_pattern(expected, actual, type_params)
    else {
        return None;
    };
    Some(OverloadParamMatch {
        // Concrete nominal types remain exact matches. Generic nominal patterns
        // rank with direct generic parameters, with less-constrained patterns
        // ranked behind more-constrained ones.
        score: if type_parameter_count == 0 {
            0
        } else {
            1 + i32::try_from(type_parameter_count).unwrap_or(i32::MAX - 1)
        },
        generic_constraints: bindings
            .into_iter()
            .map(|(name, actual)| (name, actual, true))
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// Def monomorphization — generic struct, untyped array [], bare buffer params
// ---------------------------------------------------------------------------

fn score_overload_param_match(
    arg_expr: &Expr,
    arg_shape: &OverloadArgShape,
    param_ty: Option<&FnParamType>,
    def_type_params: &[String],
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
) -> Option<OverloadParamMatch> {
    match param_ty {
        Some(FnParamType::Primitive(expected)) => match arg_shape {
            OverloadArgShape::Scalar(src) => {
                if *src == *expected {
                    Some(plain_match(0))
                } else if can_assign_expr_to_type(arg_expr, *src, *expected, &env.const_symbols) {
                    Some(plain_match(1))
                } else {
                    None
                }
            }
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        Some(FnParamType::Struct(expected_struct))
            if !def_type_params.is_empty() && def_type_params.contains(expected_struct) =>
        {
            // Generic type parameter — matches any scalar arg.
            // Score 2: between concrete (0) and untyped (4).
            match arg_shape {
                OverloadArgShape::Scalar(actual) => {
                    Some(generic_match(expected_struct, *actual, false, 2))
                }
                OverloadArgShape::Unknown => Some(plain_match(2)),
                _ => None,
            }
        }
        Some(FnParamType::Struct(expected_struct)) => match arg_shape {
            OverloadArgShape::Struct(actual_struct) => {
                nominal_match(actual_struct, expected_struct, def_type_params)
            }
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        Some(FnParamType::Buffer(expected_buffer)) => match arg_shape {
            OverloadArgShape::Buffer {
                elem_ty,
                channels,
                collection_len: None,
            } => score_buffer_match(expected_buffer, *elem_ty, channels).map(|score| {
                if let BufferElemType::Generic(name) = &expected_buffer.elem {
                    if def_type_params.contains(name) {
                        return generic_match(name, *elem_ty, true, score);
                    }
                }
                plain_match(score)
            }),
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        Some(FnParamType::BufferArray { buffer, len }) => match arg_shape {
            OverloadArgShape::Buffer {
                elem_ty,
                channels,
                collection_len: Some(actual_len),
            } if actual_len == len => score_buffer_match(buffer, *elem_ty, channels).map(|score| {
                if let BufferElemType::Generic(name) = &buffer.elem {
                    if def_type_params.contains(name) {
                        return generic_match(name, *elem_ty, true, score);
                    }
                }
                plain_match(score)
            }),
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        Some(FnParamType::Array(Some(expected_elem))) => match arg_shape {
            OverloadArgShape::Array(CallArrayType {
                elem: CallArrayElemType::Primitive(actual_elem),
                len,
            }) => {
                let shape_score = i32::from(len.is_some());
                if actual_elem == expected_elem {
                    Some(plain_match(shape_score))
                } else {
                    score_contextual_array_literal(arg_expr, *expected_elem, env, context)
                        .map(|conversion_score| plain_match(shape_score + conversion_score))
                }
            }
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        Some(FnParamType::ArrayGeneric(expected)) if def_type_params.contains(expected) => {
            match arg_shape {
                OverloadArgShape::Array(CallArrayType {
                    elem: CallArrayElemType::Primitive(actual),
                    len,
                }) => Some(generic_match(
                    expected,
                    *actual,
                    true,
                    if len.is_some() { 3 } else { 2 },
                )),
                OverloadArgShape::Unknown => Some(plain_match(2)),
                _ => None,
            }
        }
        Some(FnParamType::ArrayGeneric(expected)) => match arg_shape {
            OverloadArgShape::Array(CallArrayType {
                elem: CallArrayElemType::Nominal(actual),
                len,
            }) => nominal_match(actual, expected, def_type_params).map(|mut matched| {
                matched.score += i32::from(len.is_some());
                matched
            }),
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        Some(FnParamType::SizedArray {
            elem,
            generic_name,
            size,
        }) => {
            let expected_len = const_positive_usize_for_call_type(size)?;
            match arg_shape {
                OverloadArgShape::Array(actual) if actual.len == Some(expected_len) => {
                    match (&actual.elem, elem, generic_name) {
                        (CallArrayElemType::Primitive(actual), Some(expected), _) => {
                            if actual == expected {
                                Some(plain_match(0))
                            } else {
                                score_contextual_array_literal(arg_expr, *expected, env, context)
                                    .map(plain_match)
                            }
                        }
                        (CallArrayElemType::Primitive(actual), None, Some(name))
                            if def_type_params.contains(name) =>
                        {
                            Some(generic_match(name, *actual, true, 2))
                        }
                        (CallArrayElemType::Nominal(actual), None, Some(expected))
                            if !def_type_params.contains(expected) =>
                        {
                            nominal_match(actual, expected, def_type_params)
                        }
                        _ => None,
                    }
                }
                OverloadArgShape::Unknown => Some(plain_match(2)),
                _ => None,
            }
        }
        Some(FnParamType::Array(None)) => match arg_shape {
            OverloadArgShape::Array(_) => Some(plain_match(4)),
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        Some(FnParamType::BareBuffer) => match arg_shape {
            OverloadArgShape::Buffer {
                collection_len: None,
                ..
            } => Some(plain_match(4)),
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        Some(FnParamType::Tuple(expected)) => match arg_shape {
            OverloadArgShape::Tuple(actual) => {
                score_tuple_match(arg_expr, actual, expected, env).map(plain_match)
            }
            OverloadArgShape::Unknown => Some(plain_match(2)),
            _ => None,
        },
        None => Some(plain_match(4)),
    }
}

fn generic_primitive_constraints_match(
    constraints: &HashMap<String, Vec<(PrimitiveType, bool, &Expr)>>,
    explicit_bindings: &HashMap<String, PrimitiveType>,
    env: &CallTypeEnv,
) -> bool {
    constraints.iter().all(|(name, constraints)| {
        let exact_target = constraints
            .iter()
            .find_map(|(ty, exact, _)| exact.then_some(*ty));
        let target = if let Some(explicit) = explicit_bindings.get(name).copied() {
            explicit
        } else if let Some(exact_target) = exact_target {
            if constraints
                .iter()
                .any(|(ty, exact, _)| *exact && *ty != exact_target)
            {
                return false;
            }
            exact_target
        } else {
            let Some((first, rest)) = constraints.split_first() else {
                return true;
            };
            let Some(merged) = rest.iter().try_fold(first.0, |merged, (next, _, _)| {
                merge_inferred_return_types(merged, *next)
            }) else {
                return false;
            };
            merged
        };

        target.is_numeric()
            && constraints.iter().all(|(actual, exact, expr)| {
                if *exact {
                    *actual == target
                } else {
                    can_assign_expr_to_type(expr, *actual, target, &env.const_symbols)
                }
            })
    })
}

fn format_fn_param_for_overload(name: &str, ty: Option<&FnParamType>, has_default: bool) -> String {
    let typed = match ty {
        Some(FnParamType::Primitive(prim)) => format!("{name}: {prim:?}").to_lowercase(),
        Some(FnParamType::Struct(struct_name)) => format!("{name}: {struct_name}"),
        Some(FnParamType::Buffer(buffer_ty)) => {
            format!("{name}: {}", format_buffer_type(buffer_ty))
        }
        Some(FnParamType::BufferArray { buffer, len }) => {
            format!("{name}: {} {{{len}}}", format_buffer_type(buffer))
        }
        Some(FnParamType::Array(Some(prim))) => format!("{name}: {prim:?}[]").to_lowercase(),
        Some(FnParamType::ArrayGeneric(param)) => format!("{name}: {param}[]"),
        Some(FnParamType::SizedArray {
            elem,
            generic_name,
            size,
        }) => {
            let type_str = if let Some(prim) = elem {
                format!("{prim:?}").to_lowercase()
            } else if let Some(g) = generic_name {
                g.clone()
            } else {
                "?".to_owned()
            };
            let size = format_size_expr(size);
            format!("{name}: {type_str}[{size}]")
        }
        Some(FnParamType::Array(None)) => format!("{name}: []"),
        Some(FnParamType::BareBuffer) => format!("{name}: buffer"),
        Some(FnParamType::Tuple(elems)) => {
            let inner = elems
                .iter()
                .map(|p| format!("{p:?}").to_lowercase())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{name}: ({inner})")
        }
        None => name.to_owned(),
    };
    if has_default {
        format!("{typed} = ...")
    } else {
        typed
    }
}

fn format_buffer_type(buffer: &BufferType) -> String {
    let elem = match &buffer.elem {
        BufferElemType::Primitive(elem) => elem.name().to_owned(),
        BufferElemType::Generic(name) => name.clone(),
    };
    let channels = match &buffer.channels {
        BufferChannels::Mono => String::new(),
        BufferChannels::Dynamic => "[]".to_owned(),
        BufferChannels::Static(size) => format!("[{}]", format_size_expr(size)),
    };
    format!("buffer<{elem}{channels}>")
}

fn format_size_expr(size: &Expr) -> String {
    const_positive_usize_for_call_type(size)
        .map(|size| size.to_string())
        .or_else(|| match size {
            Expr::Var { name, .. } => Some(name.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "<constant expression>".to_owned())
}

fn format_overload_signature(name: &str, signature: &FnSignature) -> String {
    let params = signature
        .params
        .iter()
        .enumerate()
        .map(|(idx, param_name)| {
            format_fn_param_for_overload(
                param_name,
                signature.param_types.get(idx).and_then(|p| p.as_ref()),
                signature
                    .defaults
                    .get(idx)
                    .and_then(|d| d.as_ref())
                    .is_some(),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({params})")
}

fn resolve_overloaded_call_name(
    public_name: &str,
    type_args: &[CallTypeArg],
    args: &[CallArg],
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
    owner: OverloadOwnerContext,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let candidates = overloads
        .get(public_name)?
        .iter()
        .filter(|candidate| {
            type_args.is_empty()
                || (!candidate.signature.type_params.is_empty()
                    && candidate.signature.type_params.len() == type_args.len())
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        let diagnostic = diag.semantic(
            format!(
                "no generic overload of function '{}' accepts {} type argument{}",
                public_name,
                type_args.len(),
                if type_args.len() == 1 { "" } else { "s" }
            ),
            0,
            0,
        );
        if !errors.contains(&diagnostic) {
            errors.push(diagnostic);
        }
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0].internal_name.clone());
    }

    let has_dependent_argument = owner.defer_dependent_calls
        && args.iter().any(|arg| {
            matches!(
                infer_overload_arg_shape(&arg.expr, env, context),
                OverloadArgShape::Unknown
            )
        });

    let mut scored = Vec::<(i32, usize)>::new();
    for (cand_idx, cand) in candidates.iter().enumerate() {
        let mut bind_errors = Vec::new();
        let resolved = super::resolve_call_args(
            args,
            &cand.signature.params,
            &cand.signature.defaults,
            false,
            false,
            &format!("function '{public_name}' call"),
            &mut bind_errors,
        );
        if !bind_errors.is_empty() {
            continue;
        }

        let mut total_score = 0_i32;
        let mut viable = true;
        let explicit_bindings = cand
            .signature
            .type_params
            .iter()
            .zip(type_args)
            .filter_map(|(name, type_arg)| match type_arg {
                CallTypeArg::Primitive(prim) => Some((name.clone(), *prim)),
                CallTypeArg::Generic(_) => None,
            })
            .collect::<HashMap<_, _>>();
        let mut generic_constraints = HashMap::<String, Vec<(PrimitiveType, bool, &Expr)>>::new();
        for (param_idx, arg_expr) in resolved.into_iter().enumerate() {
            if let Some(arg_expr) = arg_expr {
                let arg_shape = infer_overload_arg_shape(arg_expr, env, context);
                let param_ty = cand
                    .signature
                    .param_types
                    .get(param_idx)
                    .and_then(|t| t.as_ref());
                let Some(param_match) = score_overload_param_match(
                    arg_expr,
                    &arg_shape,
                    param_ty,
                    &cand.signature.type_params,
                    env,
                    context,
                ) else {
                    viable = false;
                    break;
                };
                for (name, actual, exact) in param_match.generic_constraints {
                    generic_constraints
                        .entry(name)
                        .or_default()
                        .push((actual, exact, arg_expr));
                }
                total_score += param_match.score;
            } else {
                // Slight preference for overloads requiring fewer defaulted params.
                total_score += 1;
            }
        }
        if viable
            && generic_primitive_constraints_match(&generic_constraints, &explicit_bindings, env)
        {
            scored.push((total_score, cand_idx));
        }
    }

    if scored.is_empty() {
        let overload_list = candidates
            .iter()
            .map(|c| format_overload_signature(public_name, &c.signature))
            .collect::<Vec<_>>()
            .join(", ");
        push_semantic(
            diag,
            errors,
            format!(
                "no matching overload for function '{}' (candidates: {})",
                public_name, overload_list
            ),
        );
        return Some(candidates[0].internal_name.clone());
    }

    // Unknown arguments matter only when they leave more than one overload
    // viable. Arity, named arguments, or other concrete arguments can still
    // identify a unique candidate without waiting for specialization.
    if has_dependent_argument && scored.len() > 1 {
        return None;
    }

    let best_score = scored
        .iter()
        .map(|(score, _)| *score)
        .min()
        .unwrap_or(i32::MAX);
    let best = scored
        .into_iter()
        .filter(|(score, _)| *score == best_score)
        .map(|(_, idx)| idx)
        .collect::<Vec<_>>();
    if best.len() > 1 {
        let overload_list = best
            .iter()
            .filter_map(|idx| candidates.get(*idx))
            .map(|c| format_overload_signature(public_name, &c.signature))
            .collect::<Vec<_>>()
            .join(", ");
        push_semantic(
            diag,
            errors,
            format!(
                "ambiguous overload for function '{}'; matching candidates: {}",
                public_name, overload_list
            ),
        );
    }

    best.first()
        .and_then(|idx| candidates.get(*idx))
        .map(|cand| cand.internal_name.clone())
}

pub(crate) fn rewrite_overloaded_calls_in_expr(
    expr: &mut Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
    owner: OverloadOwnerContext,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    errors: &mut Vec<Diagnostic>,
) -> usize {
    let mut resolved = 0;
    rewrite_overloaded_calls_in_expr_impl(
        expr,
        env,
        context,
        owner,
        overloads,
        errors,
        &mut resolved,
    );
    resolved
}

fn rewrite_overloaded_calls_in_expr_impl(
    expr: &mut Expr,
    env: &CallTypeEnv,
    context: CallTypeContext<'_>,
    owner: OverloadOwnerContext,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    errors: &mut Vec<Diagnostic>,
    resolved: &mut usize,
) {
    expr.visit_mut_postorder(|expr| {
        if let Expr::Index { base, index, .. } = expr {
            super::call_types::normalize_tuple_index(base, index, env, context);
        }
        if let Some(len) = env.fixed_array_length_call(expr) {
            *expr = typed_const_expr(TypedConstValue::I32(len)).with_loc(expr.loc());
            return;
        }
        with_expr_diag_context_mut(expr, |diag, expr| {
            if let Expr::UserCall {
                name,
                type_args,
                args,
                ..
            } = expr
            {
                if args.first().is_some_and(|arg| {
                    arg.name.as_deref() == Some(crate::internal_names::METHOD_RECEIVER_ARG)
                }) {
                    // Receiver syntax is intentionally unresolved here. Selecting a
                    // same-named free overload would discard the receiver's type.
                    return;
                }
                if let Some(resolved_name) = resolve_overloaded_call_name(
                    name, type_args, args, env, context, owner, overloads, diag, errors,
                ) {
                    if *name != resolved_name {
                        *name = resolved_name;
                        *resolved += 1;
                    }
                }
            }
        });
    });
}

pub(crate) fn rewrite_overloaded_calls_in_stmt_list(
    stmts: &mut [Stmt],
    env: &mut CallTypeEnv,
    context: CallTypeContext<'_>,
    owner: OverloadOwnerContext,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    errors: &mut Vec<Diagnostic>,
) -> usize {
    let mut rewriter = OverloadRewriter {
        context,
        owner,
        overloads,
        errors,
        resolved: 0,
    };
    rewriter.rewrite_stmts(stmts, env);
    rewriter.resolved
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn rewrite_overloaded_calls_in_function(
    def: &mut FunctionDef,
    seed: &CallTypeEnv,
    context: CallTypeContext<'_>,
    owner: OverloadOwnerContext,
    overloads: &HashMap<String, Vec<OverloadCandidate>>,
    errors: &mut Vec<Diagnostic>,
) -> usize {
    let mut env = seed.clone();
    env.enter_function(&def.type_params);
    let mut resolved = 0;
    for param in &mut def.params {
        env.bind_function_param(param, &def.type_params);
        if let Some(default_expr) = &mut param.default {
            resolved += rewrite_overloaded_calls_in_expr(
                default_expr,
                &env,
                context,
                owner,
                overloads,
                errors,
            );
        }
    }
    resolved
        + rewrite_overloaded_calls_in_stmt_list(
            &mut def.body,
            &mut env,
            context,
            owner,
            overloads,
            errors,
        )
}

struct OverloadRewriter<'a> {
    context: CallTypeContext<'a>,
    owner: OverloadOwnerContext,
    overloads: &'a HashMap<String, Vec<OverloadCandidate>>,
    errors: &'a mut Vec<Diagnostic>,
    resolved: usize,
}

impl CallTypeRewriter for OverloadRewriter<'_> {
    fn context(&self) -> CallTypeContext<'_> {
        self.context
    }
    fn rewrite_expr(&mut self, expr: &mut Expr, env: &CallTypeEnv) {
        rewrite_overloaded_calls_in_expr_impl(
            expr,
            env,
            self.context,
            self.owner,
            self.overloads,
            self.errors,
            &mut self.resolved,
        );
    }
}

pub(crate) fn prepare_function_overloads(
    defs: &mut [FunctionDef],
) -> (
    HashMap<String, Vec<OverloadCandidate>>,
    HashMap<String, String>,
) {
    let mut by_public_top_level = HashMap::<String, Vec<usize>>::new();
    for (idx, def) in defs.iter().enumerate() {
        by_public_top_level
            .entry(def.name.clone())
            .or_default()
            .push(idx);
    }

    let mut internal_to_public = HashMap::<String, String>::new();
    for (public_name, indices) in &by_public_top_level {
        if indices.len() <= 1 {
            continue;
        }
        for (ordinal, idx) in indices.iter().enumerate() {
            let internal_name = overload_internal_name(public_name, ordinal + 1);
            defs[*idx].name = internal_name.clone();
            internal_to_public.insert(internal_name, public_name.clone());
        }
    }

    let overloads = function_overloads(defs, &internal_to_public);
    for def in defs.iter() {
        internal_to_public
            .entry(def.name.clone())
            .or_insert_with(|| def.name.clone());
    }
    (overloads, internal_to_public)
}

/// Candidates identify source declarations; concrete instances do not add overloads.
pub(crate) fn function_overloads(
    defs: &[FunctionDef],
    public_names: &HashMap<String, String>,
) -> HashMap<String, Vec<OverloadCandidate>> {
    let mut overloads = HashMap::<String, Vec<OverloadCandidate>>::new();
    for def in defs.iter() {
        let public_name = public_names
            .get(&def.name)
            .cloned()
            .unwrap_or_else(|| def.name.clone());
        let candidate = OverloadCandidate {
            internal_name: def.name.clone(),
            signature: std::rc::Rc::new({
                let mut signature = FnSignature::from_def(def);
                signature.display_name = Some(public_name.clone());
                signature
            }),
        };
        if def.name != public_name {
            overloads
                .entry(def.name.clone())
                .or_default()
                .push(candidate.clone());
        }
        overloads.entry(public_name).or_default().push(candidate);
    }
    overloads
}
