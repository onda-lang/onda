use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IntegerBindingRange {
    ty: PrimitiveType,
    normalize: BuiltinFn,
    bounds: IntegerBindingRangeBounds,
    parser_marker: bool,
    lower: Expr,
    upper: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IntegerBindingRangeBounds {
    Count,
    HalfOpen,
    Inclusive,
}

pub(crate) fn typed_integer_range(range: &IntegerBindingRange) -> Option<TypedIntegerRange> {
    let (Expr::Int { value: min, .. }, Expr::Int { value: max, .. }) = (&range.lower, &range.upper)
    else {
        return None;
    };
    Some(TypedIntegerRange {
        ty: range.ty,
        min: *min,
        max: *max,
        wrap: range.normalize == BuiltinFn::RangeWrap,
    })
}

pub(crate) fn integer_binding_range_expr(
    ty: PrimitiveType,
    expr: &Expr,
) -> Option<IntegerBindingRange> {
    if !matches!(ty, PrimitiveType::I32 | PrimitiveType::I64) {
        return None;
    }
    let Expr::Call { func, args, .. } = expr else {
        return None;
    };
    let (normalize, bounds, parser_marker) = match func {
        BuiltinFn::BindingCountClamp => (
            BuiltinFn::RangeClamp,
            IntegerBindingRangeBounds::Count,
            true,
        ),
        BuiltinFn::BindingCountWrap => {
            (BuiltinFn::RangeWrap, IntegerBindingRangeBounds::Count, true)
        }
        BuiltinFn::BindingRangeClamp => (
            BuiltinFn::RangeClamp,
            IntegerBindingRangeBounds::HalfOpen,
            true,
        ),
        BuiltinFn::BindingRangeWrap => (
            BuiltinFn::RangeWrap,
            IntegerBindingRangeBounds::HalfOpen,
            true,
        ),
        BuiltinFn::BindingRangeInclusiveClamp => (
            BuiltinFn::RangeClamp,
            IntegerBindingRangeBounds::Inclusive,
            true,
        ),
        BuiltinFn::BindingRangeInclusiveWrap => (
            BuiltinFn::RangeWrap,
            IntegerBindingRangeBounds::Inclusive,
            true,
        ),
        BuiltinFn::RangeClamp => (
            BuiltinFn::RangeClamp,
            IntegerBindingRangeBounds::Inclusive,
            false,
        ),
        BuiltinFn::RangeWrap => (
            BuiltinFn::RangeWrap,
            IntegerBindingRangeBounds::Inclusive,
            false,
        ),
        _ => return None,
    };
    (args.len() == 3).then(|| IntegerBindingRange {
        ty,
        normalize,
        bounds,
        parser_marker,
        lower: args[1].clone(),
        upper: args[2].clone(),
    })
}

pub(crate) fn integer_binding_range_assignment(stmt: &Stmt) -> Option<(&str, IntegerBindingRange)> {
    let Stmt::Assign {
        target: AssignTarget::Var(name),
        decl_ty: Some(DeclType::Scalar(ty)),
        expr,
        ..
    } = stmt
    else {
        return None;
    };
    integer_binding_range_expr(*ty, expr).map(|range| (name.as_str(), range))
}

pub(crate) fn declared_integer_binding_range(stmt: &Stmt) -> Option<(&str, IntegerBindingRange)> {
    let is_typed_decl = matches!(
        stmt,
        Stmt::Assign {
            is_typed_decl: true,
            ..
        }
    );
    let (name, range) = integer_binding_range_assignment(stmt)?;
    // Processor and generic lowering can turn the original declaration into
    // a plain assignment, but the parser-only marker still identifies it
    // unambiguously until this canonicalization runs.
    (is_typed_decl || range.parser_marker).then_some((name, range))
}

pub(crate) fn collect_integer_binding_range_assignments(
    statements: &[Stmt],
    ranges: &mut HashMap<String, IntegerBindingRange>,
) {
    for statement in statements {
        if let Some((name, range)) = integer_binding_range_assignment(statement) {
            ranges.insert(name.to_owned(), range);
        }
        match statement {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_integer_binding_range_assignments(then_branch, ranges);
                collect_integer_binding_range_assignments(else_branch, ranges);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                collect_integer_binding_range_assignments(body, ranges);
            }
            _ => {}
        }
    }
}

pub(crate) fn collect_flattened_proc_integer_ranges(
    proc_name: &str,
    prefix: &str,
    declarations: &HashMap<String, HashMap<String, IntegerBindingRange>>,
    shapes: &HashMap<String, ProcLoweringShape>,
    stack: &mut HashSet<String>,
    ranges: &mut HashMap<String, IntegerBindingRange>,
) {
    if !stack.insert(proc_name.to_owned()) {
        return;
    }
    let declared_proc_name = proc_name
        .split_once(".__gen__")
        .map_or(proc_name, |(source_name, _)| source_name);
    if let Some(declared) = declarations
        .get(proc_name)
        .or_else(|| declarations.get(declared_proc_name))
    {
        for (name, range) in declared {
            let field = name.strip_prefix("self.").unwrap_or(name);
            ranges.insert(format!("self.{prefix}{field}"), range.clone());
        }
    }
    if let Some(shape) = shapes.get(proc_name) {
        for (field, nested) in &shape.state.nested_procs {
            collect_flattened_proc_integer_ranges(
                &nested.proc_name,
                &format!("{prefix}{field}__"),
                declarations,
                shapes,
                stack,
                ranges,
            );
        }
        for (field, nested) in &shape.state.nested_proc_arrays {
            if let Some(slots) = shape.nested_proc_array_slots.get(field) {
                for slot in slots {
                    collect_flattened_proc_integer_ranges(
                        &nested.proc_name,
                        &format!("{prefix}{slot}__"),
                        declarations,
                        shapes,
                        stack,
                        ranges,
                    );
                }
            }
        }
    }
    stack.remove(proc_name);
}

pub(crate) fn validate_integer_binding_range(
    range: &IntegerBindingRange,
    location: SourceLoc,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<(i64, i64)> {
    let begin = eval_const_expr_i64_exact(
        &range.lower,
        options,
        "integer binding range begin bound",
        errors,
    );
    let end = eval_const_expr_i64_exact(
        &range.upper,
        options,
        match range.bounds {
            IntegerBindingRangeBounds::Count => "integer binding count",
            IntegerBindingRangeBounds::HalfOpen => "integer binding range exclusive end bound",
            IntegerBindingRangeBounds::Inclusive => "integer binding range end bound",
        },
        errors,
    );
    let (Some(begin), Some(end)) = (begin, end) else {
        return None;
    };
    let inclusive_end = match range.bounds {
        IntegerBindingRangeBounds::Count if end > 0 => end
            .checked_sub(1)
            .expect("a positive integer binding count is greater than i64::MIN"),
        IntegerBindingRangeBounds::Count => {
            errors.push(Diagnostic::semantic_span(
                "integer binding count must be positive",
                location,
            ));
            return None;
        }
        IntegerBindingRangeBounds::HalfOpen if begin < end => end
            .checked_sub(1)
            .expect("a non-empty i64 range end is greater than i64::MIN"),
        IntegerBindingRangeBounds::HalfOpen => {
            errors.push(Diagnostic::semantic_span(
                "integer binding range begin bound must be less than its exclusive end bound",
                location,
            ));
            return None;
        }
        IntegerBindingRangeBounds::Inclusive if begin <= end => end,
        IntegerBindingRangeBounds::Inclusive => {
            errors.push(Diagnostic::semantic_span(
                "integer binding range begin bound must not exceed its end bound",
                location,
            ));
            return None;
        }
    };
    if range.ty == PrimitiveType::I32
        && (i32::try_from(begin).is_err() || i32::try_from(inclusive_end).is_err())
    {
        errors.push(Diagnostic::semantic_span(
            "i32 binding range values must fit i32",
            location,
        ));
        return None;
    }
    Some((begin, inclusive_end))
}

pub(crate) fn canonicalize_integer_binding_range(
    range: &mut IntegerBindingRange,
    location: SourceLoc,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some((lower, upper)) = validate_integer_binding_range(range, location, options, errors)
    else {
        return false;
    };
    let lower_loc = range.lower.loc();
    let upper_loc = range.upper.loc();
    range.lower = Expr::int(lower).with_loc(lower_loc);
    range.upper = Expr::int(upper).with_loc(upper_loc);
    range.bounds = IntegerBindingRangeBounds::Inclusive;
    true
}

pub(crate) fn wrap_ranged_assignment(expr: &mut Expr, range: &IntegerBindingRange) {
    if let Expr::Call { func, args, .. } = expr {
        let marker_normalize = match func {
            BuiltinFn::BindingCountClamp
            | BuiltinFn::BindingRangeClamp
            | BuiltinFn::BindingRangeInclusiveClamp => Some(BuiltinFn::RangeClamp),
            BuiltinFn::BindingCountWrap
            | BuiltinFn::BindingRangeWrap
            | BuiltinFn::BindingRangeInclusiveWrap => Some(BuiltinFn::RangeWrap),
            _ => None,
        };
        if let (Some(marker_normalize), [_, lower, upper]) = (marker_normalize, args.as_mut_slice())
        {
            debug_assert_eq!(marker_normalize, range.normalize);
            *func = range.normalize;
            *lower = range.lower.clone();
            *upper = range.upper.clone();
            return;
        }
    }
    if matches!(
        expr,
        Expr::Call { func, args, .. }
            if *func == range.normalize && args.len() == 3
    ) {
        return;
    }
    let location: onda_frontend::Span = expr.loc().into();
    let value = std::mem::replace(expr, Expr::int(0));
    *expr = Expr::Call {
        loc: location,
        func: range.normalize,
        args: vec![value, range.lower.clone(), range.upper.clone()],
    };
}

pub(crate) fn rewrite_integer_binding_ranges_in_list(
    statements: &mut [Stmt],
    inherited: &HashMap<String, IntegerBindingRange>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, IntegerBindingRange> {
    let mut ranges = inherited.clone();
    for statement in statements {
        if let Some((name, mut range)) =
            declared_integer_binding_range(statement).map(|(name, range)| (name.to_owned(), range))
        {
            if canonicalize_integer_binding_range(&mut range, statement.loc(), options, errors) {
                if let Stmt::Assign {
                    expr: Expr::Call { func, args, .. },
                    ..
                } = statement
                {
                    *func = range.normalize;
                    args[1] = range.lower.clone();
                    args[2] = range.upper.clone();
                }
            }
            ranges.insert(name, range);
            continue;
        }
        match statement {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                is_typed_decl,
                expr,
                ..
            } => {
                if *is_typed_decl {
                    ranges.remove(name);
                } else if let Some(range) = ranges.get(name) {
                    wrap_ranged_assignment(expr, range);
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_integer_binding_ranges_in_list(then_branch, &ranges, options, errors);
                rewrite_integer_binding_ranges_in_list(else_branch, &ranges, options, errors);
            }
            Stmt::For { var, body, .. } => {
                let mut loop_ranges = ranges.clone();
                loop_ranges.remove(var);
                rewrite_integer_binding_ranges_in_list(body, &loop_ranges, options, errors);
            }
            Stmt::While { body, .. } => {
                rewrite_integer_binding_ranges_in_list(body, &ranges, options, errors);
            }
            Stmt::Const { .. }
            | Stmt::Assign { .. }
            | Stmt::Expr { .. }
            | Stmt::Print { .. }
            | Stmt::Return { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => {}
        }
    }
    ranges
}

pub(crate) fn integer_binding_ranges_outside_params<'a>(
    inherited: &HashMap<String, IntegerBindingRange>,
    params: impl IntoIterator<Item = &'a String>,
) -> HashMap<String, IntegerBindingRange> {
    let mut ranges = inherited.clone();
    for param in params {
        ranges.remove(param);
    }
    ranges
}

pub(crate) fn proc_integer_binding_range_aliases(
    ranges: &HashMap<String, IntegerBindingRange>,
) -> HashMap<String, IntegerBindingRange> {
    let mut aliases = ranges.clone();
    for (name, range) in ranges {
        let field = name.strip_prefix("self.").unwrap_or(name);
        aliases.insert(field.to_owned(), range.clone());
        aliases.insert(format!("self.{field}"), range.clone());
    }
    aliases
}

pub(crate) fn integer_binding_range_from_typed(range: &TypedIntegerRange) -> IntegerBindingRange {
    IntegerBindingRange {
        ty: range.ty,
        normalize: if range.wrap {
            BuiltinFn::RangeWrap
        } else {
            BuiltinFn::RangeClamp
        },
        bounds: IntegerBindingRangeBounds::Inclusive,
        parser_marker: false,
        lower: Expr::int(range.min),
        upper: Expr::int(range.max),
    }
}

pub(crate) fn extend_struct_field_integer_ranges(
    ranges: &mut HashMap<String, IntegerBindingRange>,
    root: &str,
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    let Some(fields) = struct_defs.get(struct_name) else {
        return;
    };
    for field in fields {
        let Some(range) = &field.integer_range else {
            continue;
        };
        ranges.insert(
            format!("{root}.{}", field.name),
            integer_binding_range_from_typed(range),
        );
    }
}

pub(crate) fn struct_param_integer_ranges(
    def: &FunctionDef,
    param_kinds: &[TypedFnParam],
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> HashMap<String, IntegerBindingRange> {
    let mut ranges = HashMap::new();
    for (param, kind) in def.params.iter().zip(param_kinds) {
        let TypedFnParam::Struct { struct_name } = kind else {
            continue;
        };
        extend_struct_field_integer_ranges(&mut ranges, &param.name, struct_name, struct_defs);
    }
    ranges
}

pub(crate) fn struct_array_param_integer_ranges(
    def: &FunctionDef,
    param_kinds: &[TypedFnParam],
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) -> HashMap<String, IntegerBindingRange> {
    let mut ranges = HashMap::new();
    for (param, kind) in def.params.iter().zip(param_kinds) {
        let TypedFnParam::StructArray { struct_name } = kind else {
            continue;
        };
        extend_struct_field_integer_ranges(&mut ranges, &param.name, struct_name, struct_defs);
    }
    ranges
}

pub(crate) fn normalize_struct_constructor_ranges_in_expr(
    expr: &mut Expr,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    match expr {
        Expr::Index { index, .. } => {
            normalize_struct_constructor_ranges_in_expr(index, struct_defs);
        }
        Expr::Slice {
            selector,
            channel,
            start,
            end,
            ..
        } => {
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                normalize_struct_constructor_ranges_in_expr(coordinate, struct_defs);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            normalize_struct_constructor_ranges_in_expr(&mut spec.size, struct_defs);
            if let Some(values) = init {
                for value in values {
                    normalize_struct_constructor_ranges_in_expr(value, struct_defs);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            normalize_struct_constructor_ranges_in_expr(lhs, struct_defs);
            normalize_struct_constructor_ranges_in_expr(rhs, struct_defs);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                normalize_struct_constructor_ranges_in_expr(arg, struct_defs);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args.iter_mut() {
                normalize_struct_constructor_ranges_in_expr(&mut arg.expr, struct_defs);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            normalize_struct_constructor_ranges_in_expr(expr, struct_defs);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                normalize_struct_constructor_ranges_in_expr(value, struct_defs);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }

    let Expr::UserCall { name, args, .. } = expr else {
        return;
    };
    let Some(fields) = struct_defs.get(name) else {
        return;
    };
    let scalar_fields = fields
        .iter()
        .filter(|field| matches!(field.ty, TypedFieldType::Scalar(_)))
        .collect::<Vec<_>>();
    let mut positional_index = 0usize;
    for arg in args {
        let field = if let Some(arg_name) = &arg.name {
            scalar_fields
                .iter()
                .copied()
                .find(|field| field.name == *arg_name)
        } else {
            let field = scalar_fields.get(positional_index).copied();
            positional_index += 1;
            field
        };
        let Some(range) = field.and_then(|field| field.integer_range.as_ref()) else {
            continue;
        };
        wrap_ranged_assignment(&mut arg.expr, &integer_binding_range_from_typed(range));
    }
}

pub(crate) fn normalize_struct_constructor_ranges_in_list(
    statements: &mut [Stmt],
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
) {
    for statement in statements {
        match statement {
            Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                normalize_struct_constructor_ranges_in_expr(expr, struct_defs);
            }
            Stmt::Print { values, .. } => {
                for value in values {
                    normalize_struct_constructor_ranges_in_expr(value, struct_defs);
                }
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                normalize_struct_constructor_ranges_in_expr(cond, struct_defs);
                normalize_struct_constructor_ranges_in_list(then_branch, struct_defs);
                normalize_struct_constructor_ranges_in_list(else_branch, struct_defs);
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                normalize_struct_constructor_ranges_in_expr(start, struct_defs);
                normalize_struct_constructor_ranges_in_expr(end, struct_defs);
                if let Some(step) = step {
                    normalize_struct_constructor_ranges_in_expr(step, struct_defs);
                }
                normalize_struct_constructor_ranges_in_list(body, struct_defs);
            }
            Stmt::While { cond, body, .. } => {
                normalize_struct_constructor_ranges_in_expr(cond, struct_defs);
                normalize_struct_constructor_ranges_in_list(body, struct_defs);
            }
            Stmt::Const { .. } | Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
}

pub(crate) fn rewrite_indexed_integer_ranges_in_list(
    statements: &mut [Stmt],
    ranges: &HashMap<String, IntegerBindingRange>,
) {
    for statement in statements {
        match statement {
            Stmt::Assign {
                target: AssignTarget::Index { base, .. },
                expr,
                ..
            } => {
                if let Some(range) = ranges.get(base) {
                    wrap_ranged_assignment(expr, range);
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_indexed_integer_ranges_in_list(then_branch, ranges);
                rewrite_indexed_integer_ranges_in_list(else_branch, ranges);
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                rewrite_indexed_integer_ranges_in_list(body, ranges);
            }
            Stmt::Const { .. }
            | Stmt::Assign { .. }
            | Stmt::Expr { .. }
            | Stmt::Print { .. }
            | Stmt::Return { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => {}
        }
    }
}
