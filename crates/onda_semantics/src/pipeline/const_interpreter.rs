//! One continuation stack for expressions, calls, statements and declarations.
//! Reading an uncached constant pushes its initializer onto this stack. The
//! interrupted operation resumes with its locals and intermediate values intact.
use super::*;
use crate::array_semantics::{is_array_reference, ArrayInitializer};
use crate::loop_range::{static_for_plan, StaticForPlan};
use crate::mir_scalar::{mir_scalar, scalar_type, typed_scalar};
use onda_mir::{constant_eval, ScalarValue};
mod array_bindings;
use array_bindings::ArrayBinding;

#[cfg(test)]
thread_local! {
    static LOGICAL_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static BODY_EVALUATIONS: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}

enum Value<'a> {
    Expr(Expr, ExprFacts),
    Array(ArrayBinding<'a>),
}

/// A known boolean result can still require runtime evaluation for its effects.
/// Both facts flow bottom-up, without rescanning unknown logical prefixes.
#[derive(Clone, Copy)]
pub(super) struct ExprFacts {
    pub(super) constant: bool,
    pub(super) boolean: Option<bool>,
}

impl ExprFacts {
    pub(super) fn boolean_value(
        self,
        expr: &Expr,
        options: AnalysisOptions,
        context: &str,
    ) -> Option<bool> {
        self.boolean.or_else(|| {
            if !self.constant {
                return None;
            }
            #[cfg(test)]
            LOGICAL_PROBES.with(|count| count.set(count.get() + 1));
            eval_const_bool_expr(expr, options, context, &mut Vec::new())
        })
    }
}

impl Value<'_> {
    fn expr(expr: Expr, constant: bool) -> Self {
        let boolean = match &expr {
            Expr::Bool { value, .. } => Some(*value),
            _ => None,
        };
        Self::Expr(expr, ExprFacts { constant, boolean })
    }
}

struct Scope<'a> {
    values: &'a ConstValues,
    defs: ConstDefRegistry<'a>,
    options: AnalysisOptions,
    metadata_options: AnalysisOptions,
    context: String,
    locals: HashMap<String, TypedConstValue>,
    arrays: HashMap<String, ArrayBinding<'a>>,
    readonly: HashSet<String>,
    result: Option<ConstDefReturn>,
    runtime: bool,
    body_types: Option<std::rc::Rc<ConstBodyTypes>>,
}

enum LocalBinding<'s, 'a> {
    Scalar(TypedConstValue),
    Array(&'s ArrayBinding<'a>),
}

impl<'a> Scope<'a> {
    fn new(
        values: &'a ConstValues,
        defs: ConstDefRegistry<'a>,
        options: AnalysisOptions,
        context: impl Into<String>,
    ) -> Self {
        Self {
            values,
            defs,
            options,
            metadata_options: options,
            context: context.into(),
            locals: HashMap::new(),
            arrays: HashMap::new(),
            readonly: HashSet::new(),
            result: None,
            runtime: false,
            body_types: None,
        }
    }

    fn local_binding(&self, name: &str) -> Option<LocalBinding<'_, 'a>> {
        if let Some(value) = self.locals.get(name) {
            Some(LocalBinding::Scalar(*value))
        } else {
            self.arrays.get(name).map(LocalBinding::Array)
        }
    }

    fn array_length(&self, name: &str) -> Option<usize> {
        match self.local_binding(name) {
            Some(LocalBinding::Array(array)) => Some(array.len()),
            Some(_) => None,
            None => self.values.array_info(name).map(|info| info.len),
        }
    }

    fn runtime_array_name(&self, name: &str) -> String {
        self.values.entry(name).map_or_else(
            || name.to_owned(),
            |entry| entry.runtime_name(name, self.options),
        )
    }
}

struct Call<'a> {
    def: &'a ConstDefinition,
    args: BoundCallArgs<'a>,
    next: usize,
    locals: HashMap<String, TypedConstValue>,
    arrays: HashMap<String, ArrayBinding<'a>>,
}

struct ArrayBuild<'a> {
    expressions: &'a [Expr],
    values: Vec<TypedConstValue>,
    elem_ty: Option<PrimitiveType>,
    expected: ConstArrayExpectation,
    loc: SourceLoc,
}

struct Loop<'a> {
    var: &'a str,
    body: &'a [Stmt],
    current: ScalarValue,
    last: ScalarValue,
    step: ScalarValue,
    iterations: usize,
    loc: SourceLoc,
    saved: Option<TypedConstValue>,
    scalar_names: HashSet<String>,
    array_names: HashSet<String>,
}

enum Frame<'a> {
    Options(AnalysisOptions),
    SliceBound,
    Expr(&'a Expr),
    Finish(&'a Expr, usize),
    Logical(&'a Expr, usize),
    Array(&'a Expr, ConstArrayExpectation),
    SliceFinish(&'a Expr, ConstArrayExpectation, usize),
    ArraySize(&'a Expr, ConstArrayExpectation),
    CopyArray,
    FillArray(PrimitiveType, usize),
    ArrayBuild(ArrayBuild<'a>),
    ArrayElement(ArrayBuild<'a>),
    Constant(ConstEvaluation<'a>),
    Cache(ConstEvaluation<'a>, Vec<String>),
    Call(Call<'a>),
    Argument(Call<'a>, bool, String),
    EndCall(&'a ConstDefinition),
    ScalarCall(SourceLoc),
    CheckArray(ConstArrayExpectation, SourceLoc),
    DemandArray,
    Statements(&'a [Stmt], usize),
    Assign(&'a Stmt),
    AssignmentArraySize(&'a Expr, ConstArrayExpectation),
    If(&'a Stmt),
    BranchScope(&'a Stmt),
    For(&'a Stmt, usize),
    Loop(Loop<'a>),
    Return,
}

struct Interpreter<'a, 'e> {
    scopes: Vec<Scope<'a>>,
    frames: Vec<Frame<'a>>,
    stack: Vec<Value<'a>>,
    calls: Vec<String>,
    active_constants: Vec<(ConstEvaluation<'a>, usize)>,
    errors: &'e mut Vec<Diagnostic>,
}

impl<'a, 'e> Interpreter<'a, 'e> {
    fn new(
        values: &'a ConstValues,
        defs: ConstDefRegistry<'a>,
        options: AnalysisOptions,
        context: &str,
        locals: &HashMap<String, TypedConstValue>,
        arrays: &HashMap<String, ConstEvalArray>,
        calls: &[String],
        errors: &'e mut Vec<Diagnostic>,
    ) -> Self {
        Self {
            scopes: vec![Scope {
                locals: locals.clone(),
                arrays: arrays
                    .iter()
                    .map(|(name, array)| (name.clone(), array.clone().into()))
                    .collect(),
                ..Scope::new(values, defs, options, context)
            }],
            frames: Vec::new(),
            stack: Vec::new(),
            calls: calls.to_vec(),
            active_constants: Vec::new(),
            errors,
        }
    }

    fn scope(&self) -> &Scope<'a> {
        self.scopes.last().unwrap()
    }
    fn scope_mut(&mut self) -> &mut Scope<'a> {
        self.scopes.last_mut().unwrap()
    }

    fn slice_bound(&mut self, expr: &'a Expr) {
        // Freeze closed bounds in their checked layout context before returning
        // to the executable context. Dynamic bounds retain runtime evaluation.
        self.frames.push(Frame::Options(self.scope().options));
        self.frames.push(Frame::SliceBound);
        self.frames.push(Frame::Expr(expr));
        self.frames
            .push(Frame::Options(self.scope().metadata_options));
    }

    fn scoped_statements(&mut self, stmts: &'a [Stmt]) {
        self.frames.push(Frame::Statements(stmts, 0));
    }

    fn error(&mut self, message: impl Into<String>, loc: impl Into<Span>) -> Option<()> {
        self.errors.push(Diagnostic::semantic_span(message, loc));
        None
    }

    fn expr(&mut self) -> Option<Expr> {
        match self.stack.pop()? {
            Value::Expr(expr, _) => Some(expr),
            Value::Array(_) => {
                self.error(
                    format!("{}: expected a scalar, got an array", self.scope().context),
                    SourceLoc::default(),
                );
                None
            }
        }
    }

    fn boolean(&self, expr: &Expr, facts: ExprFacts) -> Option<bool> {
        facts.boolean_value(expr, self.scope().options, &self.scope().context)
    }

    fn typed(&mut self, expr: &Expr, ty: PrimitiveType) -> Option<TypedConstValue> {
        let scope = self.scopes.last().unwrap();
        eval_typed_const_expr(
            expr,
            ty,
            scope.options,
            &scope.context,
            is_float_type(ty),
            self.errors,
        )
    }

    fn local_read(&mut self, node: &Expr, value: TypedConstValue) -> Option<TypedConstValue> {
        let ty = self
            .scope()
            .body_types
            .as_ref()
            .and_then(|types| types.reads.get(&(node as *const Expr)))
            .copied();
        if let Some(ty) = ty.filter(|ty| *ty != value.primitive_type()) {
            self.typed(
                &cast_expr_to_primitive(concrete_const_expr(value, node.loc()), ty),
                ty,
            )
        } else {
            Some(value)
        }
    }

    fn inferred(&mut self, expr: &Expr) -> Option<PrimitiveType> {
        let scope = self.scopes.last().unwrap();
        let inferred = infer_const_expr_type(expr, &scope.context, self.errors);
        effective_untyped_assignment_type(expr, inferred, &DeclaredSymbolMap::new())
    }

    fn integer(&mut self, expr: &Expr) -> Option<i64> {
        if !can_eval_const_expr_exact_int(expr) {
            self.error(
                format!(
                    "{}: expression is not a compile-time integer",
                    self.scope().context
                ),
                expr.loc(),
            );
            return None;
        }
        let scope = self.scopes.last().unwrap();
        eval_const_expr_i64_exact(expr, scope.options, &scope.context, self.errors)
    }

    fn array(&self, name: &str) -> Option<ArrayBinding<'a>> {
        let scope = self.scopes.last().unwrap();
        match scope.local_binding(name) {
            Some(LocalBinding::Array(array)) => return Some(array.clone()),
            Some(_) => return None,
            None => {}
        }
        ArrayBinding::declaration(scope.values.entry(name)?.evaluation(scope.options))
    }

    fn is_array_value(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Var { name, .. } => self.scope().array_length(name).is_some(),
            Expr::UserCall { name, .. } => self
                .scope()
                .defs
                .get(name)
                .is_some_and(|def| matches!(def.result, Some(ConstDefReturn::Array { .. }))),
            Expr::ArrayLiteral { .. } | Expr::ArrayCtor { .. } | Expr::Slice { .. } => true,
            _ => false,
        }
    }

    fn checked_array_size(
        &mut self,
        size: &Expr,
        elem_ty: PrimitiveType,
        expected: ConstArrayExpectation,
        loc: SourceLoc,
    ) -> Option<usize> {
        let scope = self.scopes.last().unwrap();
        let len = eval_array_size_expr(size, scope.options, &scope.context, self.errors)?;
        check_const_array_shape(elem_ty, len, expected, &scope.context, loc, self.errors)
            .then_some(len)
    }

    /// Suspend this operation at precisely the read, without replaying it.
    fn require(&mut self, name: &str, resume: Frame<'a>) -> Option<bool> {
        let scope = self.scopes.last().unwrap();
        let evaluation = match scope.local_binding(name) {
            Some(LocalBinding::Array(array)) => array.evaluation(),
            Some(_) => None,
            None => scope
                .values
                .entry(name)
                .map(|entry| entry.evaluation(scope.options)),
        };
        self.require_evaluation(evaluation, resume)
    }

    fn require_evaluation(
        &mut self,
        evaluation: Option<ConstEvaluation<'a>>,
        resume: Frame<'a>,
    ) -> Option<bool> {
        let Some(evaluation) = evaluation else {
            return Some(false);
        };
        if evaluation.evaluated.get().is_some() {
            return evaluation.cached_value(self.errors).map(|_| false);
        }
        self.frames.push(resume);
        self.frames.push(Frame::Constant(evaluation));
        Some(true)
    }

    fn start_call(&mut self, name: &str, args: &'a [CallArg], loc: SourceLoc) -> Option<()> {
        let scope = self.scopes.last().unwrap();
        // Declaration checking rejects recursion; captured definitions only
        // reference earlier visible definitions, so calls form a DAG.
        let Some(def) = scope.defs.get(name).map(std::rc::Rc::as_ref) else {
            let message = match self.calls.last() {
                Some(caller) => format!(
                    "{}: const def '{name}' is not visible from const def '{caller}'; const defs can only call earlier visible const defs",
                    scope.context,
                ),
                None => format!("{}: unknown const def '{name}'", scope.context),
            };
            return self.error(message, loc);
        };
        def.result?;
        def.param_kinds.as_ref()?;
        let before = self.errors.len();
        let resolved = bind_call_args_at(
            args,
            &def.signature.params,
            &def.signature.defaults,
            false,
            false,
            &format!("const def '{name}' call"),
            loc,
            self.errors,
        );
        if self.errors.len() != before {
            return None;
        }
        self.frames.push(Frame::Call(Call {
            def,
            args: resolved,
            next: 0,
            locals: HashMap::new(),
            arrays: HashMap::new(),
        }));
        Some(())
    }

    fn step(&mut self, frame: Frame<'a>) -> Option<()> {
        match frame {
            Frame::Constant(evaluation) => {
                let entry = evaluation.entry;
                if evaluation.evaluated.get().is_some() {
                    return Some(());
                }
                if evaluation.resolving.replace(true) {
                    return self.error(
                        format!("cyclic constant dependency involving '{}'", entry.decl.name),
                        entry.decl.loc,
                    );
                }
                let error_start = self.errors.len();
                self.active_constants
                    .push((evaluation.clone(), error_start));
                let saved_calls = std::mem::take(&mut self.calls);
                // The read selects declaration or caller options once. Nested
                // reads apply their own policy while retaining lexical visibility.
                self.scopes.push(Scope::new(
                    &entry.environment.values,
                    &entry.environment.defs,
                    evaluation.options,
                    format!(
                        "const {} '{}'",
                        if entry.array.is_some() {
                            "array"
                        } else {
                            "scalar"
                        },
                        entry.decl.name
                    ),
                ));
                self.frames.push(Frame::Cache(evaluation, saved_calls));
                if let Some(info) = entry.array {
                    self.frames.push(Frame::DemandArray);
                    self.frames.push(Frame::Array(
                        &entry.decl.expr,
                        ConstArrayExpectation::fixed(info.elem_ty, info.len),
                    ));
                } else {
                    self.frames.push(Frame::Expr(&entry.decl.expr));
                }
            }
            Frame::Cache(evaluation, saved_calls) => {
                let entry = evaluation.entry;
                let value = match self.stack.pop()? {
                    Value::Array(array) => ResolvedConstValue::Array(array.materialize()?),
                    Value::Expr(expr, _) => {
                        let ty = match entry.decl.ty {
                            Some(ConstType::Scalar(ty)) => ty,
                            _ => entry.scalar_type?,
                        };
                        ResolvedConstValue::Scalar(self.typed(&expr, ty)?)
                    }
                };
                evaluation
                    .evaluated
                    .set(Ok(value))
                    .expect("constant evaluated once");
                evaluation.resolving.set(false);
                self.active_constants.pop();
                self.calls = saved_calls;
                self.scopes.pop();
            }
            Frame::Options(options) => self.scope_mut().options = options,
            Frame::SliceBound => {
                let Value::Expr(mut expr, facts) = self.stack.pop()? else {
                    unreachable!()
                };
                if facts.constant {
                    let scope = self.scopes.last().unwrap();
                    let value = crate::const_scalar::eval_const_scalar(
                        &expr,
                        None,
                        scope.options,
                        &scope.context,
                        self.errors,
                    )?;
                    expr = typed_const_expr_with_loc(value, expr.loc());
                }
                self.stack.push(Value::Expr(expr, facts));
            }
            Frame::Expr(node) => match node {
                Expr::Var { name, .. } => match self.scope().local_binding(name) {
                    Some(LocalBinding::Scalar(value)) => {
                        let value = self.local_read(node, value)?;
                        self.stack
                            .push(Value::expr(concrete_const_expr(value, node.loc()), true));
                    }
                    Some(LocalBinding::Array(_)) => {
                        return self.error(
                            format!(
                                "{}: local '{name}' is an array, not a scalar",
                                self.scope().context
                            ),
                            node.loc(),
                        );
                    }
                    None if self.scope().values.contains_key(name) => {
                        if self.require(name, Frame::Expr(node))? {
                            return Some(());
                        }
                        let entry = self.scope().values.entry(name)?;
                        let evaluation = entry.evaluation(self.scope().options);
                        match evaluation.evaluated.get()?.as_ref().ok()? {
                            ResolvedConstValue::Scalar(value) => self.stack.push(Value::expr(
                                if entry.contextual_numeric {
                                    contextual_const_expr(*value).with_loc(node.loc())
                                } else {
                                    concrete_const_expr(*value, node.loc())
                                },
                                true,
                            )),
                            ResolvedConstValue::Array(_) if self.scope().runtime => {
                                self.stack.push(Value::expr(
                                    Expr::var(self.scope().runtime_array_name(name))
                                        .with_loc(node.loc()),
                                    false,
                                ))
                            }
                            _ => {
                                return self.error(
                                    format!(
                                        "{}: constant '{name}' is an array, not a scalar",
                                        self.scope().context
                                    ),
                                    node.loc(),
                                )
                            }
                        }
                    }
                    None => self.stack.push(Value::expr(
                        node.clone(),
                        builtin_constant_value_f64(name, self.scope().options).is_some(),
                    )),
                },
                Expr::UserCall {
                    name,
                    args,
                    type_args,
                    ..
                } => {
                    if args.is_empty() {
                        if let Some(base) = parse_array_len_instance_base(name) {
                            if matches!(
                                self.scope().local_binding(base),
                                Some(LocalBinding::Scalar(_))
                            ) {
                                return self.error(
                                    format!(
                                        "{}: local '{base}' is a scalar, not an array",
                                        self.scope().context
                                    ),
                                    node.loc(),
                                );
                            }
                            if let Some(len) = self.scope().array_length(base) {
                                let scope = self.scopes.last().unwrap();
                                let len = checked_array_length(
                                    len,
                                    &scope.context,
                                    node.loc(),
                                    self.errors,
                                )?;
                                self.stack.push(Value::expr(
                                    typed_const_expr_with_loc(
                                        TypedConstValue::I32(len),
                                        node.loc(),
                                    ),
                                    true,
                                ));
                                return Some(());
                            }
                        }
                    }
                    if self.scope().runtime && !self.scope().defs.contains_key(name) {
                        self.frames.push(Frame::Finish(node, self.stack.len()));
                        self.frames
                            .extend(args.iter().rev().map(|arg| Frame::Expr(&arg.expr)));
                        return Some(());
                    }
                    if !type_args.is_empty() {
                        return self.error(
                            format!(
                                "{}: const def calls cannot use explicit type arguments",
                                self.scope().context
                            ),
                            node.loc(),
                        );
                    }
                    self.frames.push(Frame::ScalarCall(node.loc()));
                    if self.scope().runtime {
                        self.frames.push(Frame::DemandArray);
                    }
                    self.start_call(name, args, node.loc())?;
                }
                Expr::Logical { lhs, .. } => {
                    self.frames.push(Frame::Logical(node, self.stack.len()));
                    self.frames.push(Frame::Expr(lhs));
                }
                Expr::ArrayCtor { .. }
                | Expr::ArrayLiteral { .. }
                | Expr::Slice { .. }
                | Expr::Tuple { .. }
                    if self.scope().runtime =>
                {
                    if let Expr::Slice { base, .. } = node {
                        if self.require(base, Frame::Expr(node))? {
                            return Some(());
                        }
                    }
                    self.frames.push(Frame::Finish(node, self.stack.len()));
                    if let Expr::Slice {
                        selector,
                        channel,
                        start,
                        end,
                        ..
                    } = node
                    {
                        for bound in [end, start].into_iter().flatten() {
                            self.slice_bound(bound);
                        }
                        self.frames.extend(
                            [channel, selector]
                                .into_iter()
                                .flatten()
                                .map(|expr| Frame::Expr(expr)),
                        );
                    } else {
                        let mut children = Vec::new();
                        node.children(&mut children);
                        self.frames
                            .extend(children.into_iter().rev().map(Frame::Expr));
                    }
                }
                Expr::ArrayCtor { .. }
                | Expr::ArrayLiteral { .. }
                | Expr::Slice { .. }
                | Expr::Tuple { .. } => {
                    return self.error(
                        format!(
                            "{}: expression is not supported in const def evaluation",
                            self.scope().context
                        ),
                        node.loc(),
                    );
                }
                _ => {
                    self.frames.push(Frame::Finish(node, self.stack.len()));
                    let mut children = Vec::new();
                    node.children(&mut children);
                    self.frames
                        .extend(children.into_iter().rev().map(Frame::Expr));
                }
            },
            Frame::Logical(node, base) => {
                let Expr::Logical { op, rhs, .. } = node else {
                    unreachable!()
                };
                let Value::Expr(mut left, mut facts) = self.stack.pop()? else {
                    unreachable!()
                };
                let value = if self.scope().runtime {
                    self.boolean(&left, facts)
                } else {
                    let TypedConstValue::Bool(value) = self.typed(&left, PrimitiveType::Bool)?
                    else {
                        unreachable!()
                    };
                    Some(value)
                };
                facts.boolean = value;
                if let Some(value) = value.filter(|_| facts.constant) {
                    left = typed_const_expr_with_loc(TypedConstValue::Bool(value), left.loc());
                }
                if value == Some(matches!(op, onda_frontend::LogicalOp::Or)) {
                    self.stack.push(Value::Expr(left, facts));
                    return Some(());
                }
                self.stack.push(Value::Expr(left, facts));
                self.frames.push(Frame::Finish(node, base));
                self.frames.push(Frame::Expr(rhs));
            }
            Frame::Finish(node, base) => {
                if let Expr::Index { base: name, .. } = node {
                    if self.require(name, Frame::Finish(node, base))? {
                        return Some(());
                    }
                }
                let constant = self.stack[base..]
                    .iter()
                    .all(|value| matches!(value, Value::Expr(_, facts) if facts.constant))
                    && !matches!(node, Expr::UserCall { .. } | Expr::Slice { .. });
                let mut booleans = self.stack[base..].iter().map(|value| {
                    let Value::Expr(expr, facts) = value else {
                        unreachable!()
                    };
                    self.boolean(expr, *facts)
                });
                let boolean = match node {
                    Expr::Logical { op, .. } => {
                        let lhs = booleans.next().unwrap();
                        let rhs = booleans.next().unwrap();
                        let short = matches!(op, onda_frontend::LogicalOp::Or);
                        if lhs == Some(short) || rhs == Some(short) {
                            Some(short)
                        } else if lhs == Some(!short) {
                            rhs
                        } else {
                            None
                        }
                    }
                    Expr::UnaryNot { .. } => match &self.stack[base] {
                        Value::Expr(_, facts) => facts.boolean.map(|value| !value),
                        _ => unreachable!(),
                    },
                    Expr::Cast {
                        to: PrimitiveType::Bool,
                        ..
                    } => match &self.stack[base] {
                        Value::Expr(_, facts) => facts.boolean,
                        _ => unreachable!(),
                    },
                    Expr::Compare { op, .. } => booleans
                        .next()
                        .unwrap()
                        .zip(booleans.next().unwrap())
                        .and_then(|(lhs, rhs)| {
                            constant_eval::compare(
                                crate::mir_scalar::map_compare(*op),
                                ScalarValue::Bool(lhs),
                                ScalarValue::Bool(rhs),
                            )
                        }),
                    _ => None,
                };
                let mut children = self
                    .stack
                    .drain(base..)
                    .map(|value| match value {
                        Value::Expr(expr, _) => expr,
                        Value::Array(_) => unreachable!(),
                    })
                    .collect::<Vec<_>>()
                    .into_iter();
                let mut child = || children.next().expect("expression operand");
                let loc = node.loc();
                let expr = match node {
                    Expr::Call { loc, func, .. } => Expr::Call {
                        loc: *loc,
                        func: *func,
                        args: children.collect(),
                    },
                    Expr::UserCall {
                        loc,
                        name,
                        args,
                        type_args,
                    } => Expr::UserCall {
                        loc: *loc,
                        name: name.clone(),
                        type_args: type_args.clone(),
                        args: args
                            .iter()
                            .map(|arg| CallArg {
                                name: arg.name.clone(),
                                expr: child(),
                            })
                            .collect(),
                    },
                    Expr::ArrayLiteral { loc, .. } => Expr::ArrayLiteral {
                        loc: *loc,
                        values: children.collect(),
                    },
                    Expr::Tuple { loc, .. } => Expr::Tuple {
                        loc: *loc,
                        values: children.collect(),
                    },
                    Expr::ArrayCtor {
                        loc,
                        spec,
                        init,
                        init_is_value,
                        initialize,
                    } => {
                        let mut spec = spec.clone();
                        spec.size = Box::new(child());
                        Expr::ArrayCtor {
                            loc: *loc,
                            spec,
                            init: init.as_ref().map(|_| children.collect()),
                            init_is_value: *init_is_value,
                            initialize: *initialize,
                        }
                    }
                    Expr::Slice {
                        loc,
                        base,
                        selector,
                        channel,
                        start,
                        end,
                    } => Expr::Slice {
                        loc: *loc,
                        base: self.scope().runtime_array_name(base),
                        selector: selector.as_ref().map(|_| Box::new(child())),
                        channel: channel.as_ref().map(|_| Box::new(child())),
                        start: start.as_ref().map(|_| Box::new(child())),
                        end: end.as_ref().map(|_| Box::new(child())),
                    },
                    Expr::Index { base, .. } => {
                        let index = child();
                        if self.scope().runtime
                            && (!can_eval_const_expr_exact_int(&index)
                                || self.array(base).is_none())
                        {
                            self.stack.push(Value::expr(
                                Expr::Index {
                                    loc: loc.span(),
                                    base: self.scope().runtime_array_name(base),
                                    index: Box::new(index),
                                },
                                false,
                            ));
                            return Some(());
                        }
                        let raw = self.integer(&index)?;
                        let Some(array) = self.array(base) else {
                            return self.error(
                                format!("{}: unknown const array '{base}'", self.scope().context),
                                loc,
                            );
                        };
                        let Some(value) = usize::try_from(raw)
                            .ok()
                            .and_then(|idx| array.values()?.get(idx).copied())
                        else {
                            return self.error(format!("{}: const array '{base}' index {raw} is out of bounds for length {}", self.scope().context, array.len()), loc);
                        };
                        concrete_const_expr(value, loc)
                    }
                    Expr::Binary { loc, op, .. } => Expr::Binary {
                        loc: *loc,
                        op: *op,
                        lhs: Box::new(child()),
                        rhs: Box::new(child()),
                    },
                    Expr::Compare { loc, op, .. } => Expr::Compare {
                        loc: *loc,
                        op: *op,
                        lhs: Box::new(child()),
                        rhs: Box::new(child()),
                    },
                    Expr::Logical { loc, op, .. } => {
                        let lhs = child();
                        let rhs = child();
                        if let (Expr::Bool { value: left, .. }, Expr::Bool { value: right, .. }) =
                            (&lhs, &rhs)
                        {
                            Expr::Bool {
                                loc: *loc,
                                value: match op {
                                    onda_frontend::LogicalOp::And => *left && *right,
                                    onda_frontend::LogicalOp::Or => *left || *right,
                                },
                            }
                        } else {
                            Expr::Logical {
                                loc: *loc,
                                op: *op,
                                lhs: Box::new(lhs),
                                rhs: Box::new(rhs),
                            }
                        }
                    }
                    Expr::Cast { loc, to, .. } => Expr::Cast {
                        loc: *loc,
                        to: *to,
                        expr: Box::new(child()),
                    },
                    Expr::UnaryNot { loc, .. } => match child() {
                        Expr::Bool { value, .. } => Expr::Bool {
                            loc: *loc,
                            value: !value,
                        },
                        expr => Expr::UnaryNot {
                            loc: *loc,
                            expr: Box::new(expr),
                        },
                    },
                    Expr::UnaryBitNot { loc, .. } => Expr::UnaryBitNot {
                        loc: *loc,
                        expr: Box::new(child()),
                    },
                    _ => node.clone(),
                };
                let mut value = Value::expr(expr, constant);
                if let Value::Expr(_, facts) = &mut value {
                    facts.boolean = facts.boolean.or(boolean);
                }
                self.stack.push(value);
            }
            Frame::Array(node, expected) => match node {
                Expr::Var { name, .. } => {
                    let Some(array) = self.array(name) else {
                        return self.error(
                            format!("{}: unknown const array '{name}'", self.scope().context),
                            node.loc(),
                        );
                    };
                    self.stack.push(Value::Array(array));
                    self.frames.push(Frame::CheckArray(expected, node.loc()));
                }
                Expr::UserCall {
                    name,
                    args,
                    type_args,
                    ..
                } => {
                    if !type_args.is_empty() {
                        return self.error(
                            format!(
                                "{}: const def calls cannot use explicit type arguments",
                                self.scope().context
                            ),
                            node.loc(),
                        );
                    }
                    self.frames.push(Frame::CheckArray(expected, node.loc()));
                    self.start_call(name, args, node.loc())?;
                }
                Expr::ArrayLiteral { values, .. } => {
                    if values.is_empty() {
                        return self.error(
                            format!("{}: array literal cannot be empty", self.scope().context),
                            node.loc(),
                        );
                    }
                    if let Some(len) = expected.len {
                        if values.len() != len {
                            return self.error(
                                format!(
                                    "{}: expected array length {len}, got {}",
                                    self.scope().context,
                                    values.len()
                                ),
                                node.loc(),
                            );
                        }
                    }
                    self.frames.push(Frame::ArrayBuild(ArrayBuild {
                        expressions: values,
                        values: Vec::with_capacity(values.len()),
                        elem_ty: expected.elem_ty,
                        expected,
                        loc: node.loc(),
                    }));
                }
                Expr::ArrayCtor { spec, .. } => {
                    self.frames.push(Frame::ArraySize(node, expected));
                    self.frames.push(Frame::Expr(&spec.size));
                }
                Expr::Slice {
                    selector,
                    channel,
                    start,
                    end,
                    ..
                } => {
                    if selector.is_some() || channel.is_some() {
                        return self.error(
                            format!(
                                "{}: const arrays do not support buffer coordinates",
                                self.scope().context
                            ),
                            node.loc(),
                        );
                    }
                    self.frames
                        .push(Frame::SliceFinish(node, expected, self.stack.len()));
                    for bound in [end, start].into_iter().flatten() {
                        self.slice_bound(bound);
                    }
                }
                _ => {
                    return self.error(
                        format!(
                            "{}: expression does not evaluate to a const array",
                            self.scope().context
                        ),
                        node.loc(),
                    );
                }
            },
            Frame::SliceFinish(node, expected, base) => {
                let operands = self
                    .stack
                    .drain(base..)
                    .map(|value| match value {
                        Value::Expr(expr, _) => expr,
                        Value::Array(_) => unreachable!(),
                    })
                    .collect::<Vec<_>>();
                let array = match node {
                    Expr::Slice {
                        base, start, end, ..
                    } => {
                        let array = self.array(base)?;
                        let len = array.len();
                        let scope = self.scopes.last().unwrap();
                        let (start_idx, end_idx) = eval_const_slice_bounds_with_defs(
                            base,
                            len,
                            start.as_ref().map(|_| &operands[0]),
                            end.as_ref()
                                .map(|_| &operands[usize::from(start.is_some())]),
                            &HashMap::new(),
                            &HashMap::new(),
                            scope.values,
                            scope.defs,
                            scope.options,
                            &scope.context,
                            &[],
                            self.errors,
                        )?;
                        array.slice(start_idx, end_idx)
                    }
                    _ => unreachable!(),
                };
                self.stack.push(Value::Array(array));
                self.frames.push(Frame::CheckArray(expected, node.loc()));
            }
            Frame::ArraySize(node, expected) => {
                let Expr::ArrayCtor {
                    spec,
                    init,
                    init_is_value,
                    ..
                } = node
                else {
                    unreachable!()
                };
                let ArrayElemType::Primitive(elem_ty) = spec.elem else {
                    return self.error(
                        format!(
                            "{}: const arrays can only use primitive element types",
                            self.scope().context
                        ),
                        node.loc(),
                    );
                };
                let size = self.expr()?;
                let len = self.checked_array_size(&size, elem_ty, expected, node.loc())?;
                let expected = ConstArrayExpectation::fixed(elem_ty, len);
                match ArrayInitializer::new(init.as_deref(), *init_is_value) {
                    ArrayInitializer::Elements(values) => {
                        if values.len() != len {
                            return self.error(
                                format!(
                                    "{}: declares length {len}, but initializer has {} element(s)",
                                    self.scope().context,
                                    values.len()
                                ),
                                node.loc(),
                            );
                        }
                        self.frames.push(Frame::ArrayBuild(ArrayBuild {
                            expressions: values,
                            values: Vec::with_capacity(len),
                            elem_ty: Some(elem_ty),
                            expected,
                            loc: node.loc(),
                        }));
                    }
                    ArrayInitializer::Value(value) if self.is_array_value(value) => {
                        self.frames.push(Frame::CopyArray);
                        self.frames.push(Frame::Array(value, expected));
                    }
                    ArrayInitializer::Value(value) => {
                        self.frames.push(Frame::FillArray(elem_ty, len));
                        self.frames.push(Frame::Expr(value));
                    }
                    ArrayInitializer::Zero => {
                        self.stack.push(Value::Array(
                            ConstEvalArray {
                                elem_ty,
                                values: vec![zero_const_value(elem_ty); len].into(),
                            }
                            .into(),
                        ));
                    }
                }
            }
            Frame::CopyArray => {
                let Value::Array(array) = self.stack.pop()? else {
                    unreachable!()
                };
                self.stack.push(Value::Array(array.copy()));
            }
            Frame::FillArray(elem_ty, len) => {
                let expr = self.expr()?;
                let value = self.typed(&expr, elem_ty)?;
                self.stack.push(Value::Array(
                    ConstEvalArray {
                        elem_ty,
                        values: vec![value; len].into(),
                    }
                    .into(),
                ));
            }
            Frame::ArrayBuild(build) => {
                if let Some(expr) = build.expressions.get(build.values.len()) {
                    self.frames.push(Frame::ArrayElement(build));
                    self.frames.push(Frame::Expr(expr));
                } else {
                    self.stack.push(Value::Array(
                        ConstEvalArray {
                            elem_ty: build.elem_ty?,
                            values: build.values.into(),
                        }
                        .into(),
                    ));
                    self.frames
                        .push(Frame::CheckArray(build.expected, build.loc));
                }
            }
            Frame::ArrayElement(mut build) => {
                let expr = self.expr()?;
                let ty = build.elem_ty.or_else(|| self.inferred(&expr))?;
                build.elem_ty = Some(ty);
                build.values.push(self.typed(&expr, ty)?);
                self.frames.push(Frame::ArrayBuild(build));
            }
            Frame::CheckArray(expected, loc) => {
                let Some(Value::Array(array)) = self.stack.last() else {
                    return self.error(
                        format!(
                            "{}: const def returns a scalar, not an array",
                            self.scope().context
                        ),
                        loc,
                    );
                };
                let scope = self.scopes.last().unwrap();
                if !check_const_array_shape(
                    array.elem_ty(),
                    array.len(),
                    expected,
                    &scope.context,
                    loc,
                    self.errors,
                ) {
                    return None;
                }
            }
            Frame::ScalarCall(loc) => {
                if matches!(self.stack.last(), Some(Value::Array(_))) && self.scope().runtime {
                    let Value::Array(array) = self.stack.pop()? else {
                        unreachable!()
                    };
                    self.stack.push(Value::expr(
                        const_array_literal_expr(&array.values()?, loc),
                        true,
                    ));
                } else if !matches!(self.stack.last(), Some(Value::Expr(_, _))) {
                    return self.error(
                        format!(
                            "{}: const def returns an array, not a scalar",
                            self.scope().context
                        ),
                        loc,
                    );
                }
            }
            Frame::DemandArray => {
                if let Some(Value::Array(array)) = self.stack.last() {
                    self.require_evaluation(array.evaluation(), Frame::DemandArray)?;
                }
            }
            Frame::Call(call) => {
                if call.next == call.args.evaluation_order.len() {
                    let params = call.def.param_kinds.as_ref()?;
                    let metadata = if params
                        .iter()
                        .any(|kind| matches!(kind, ConstDefParamKind::Slice { .. }))
                    {
                        params
                            .iter()
                            .zip(&call.def.signature.params)
                            .map(|(kind, name)| {
                                if matches!(kind, ConstDefParamKind::Slice { .. }) {
                                    let array = &call.arrays[name];
                                    ConstArrayExpectation::fixed(array.elem_ty(), array.len())
                                } else {
                                    ConstArrayExpectation::any()
                                }
                            })
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    let body_types =
                        validate_const_call_metadata(call.def, &metadata, self.errors)?;
                    #[cfg(test)]
                    BODY_EVALUATIONS.with(|counts| {
                        *counts
                            .borrow_mut()
                            .entry(call.def.name.clone())
                            .or_default() += 1
                    });
                    let scope = self.scopes.last().unwrap();
                    self.scopes.push(Scope {
                        locals: call.locals,
                        arrays: call.arrays,
                        readonly: call
                            .def
                            .params
                            .iter()
                            .filter(|param| param.readonly)
                            .map(|param| param.name.clone())
                            .collect(),
                        result: call.def.result,
                        body_types: Some(body_types),
                        ..Scope::new(
                            &call.def.environment.values,
                            &call.def.environment.defs,
                            scope.options,
                            format!("const def '{}'", call.def.name),
                        )
                    });
                    self.calls.push(call.def.name.clone());
                    self.frames.push(Frame::EndCall(call.def));
                    self.frames.push(Frame::Statements(&call.def.body, 0));
                } else {
                    let idx = call.args.evaluation_order[call.next];
                    let default = call.args.by_param[idx].is_none();
                    let expr =
                        call.args.by_param[idx].or(call.def.signature.defaults[idx].as_ref())?;
                    let kind = call.def.param_kinds.as_ref()?[idx];
                    if default {
                        self.calls.push(call.def.name.clone());
                    }
                    let context = std::mem::replace(
                        &mut self.scope_mut().context,
                        format!(
                            "const def '{}' argument '{}'",
                            call.def.name, call.def.signature.params[idx]
                        ),
                    );
                    if default {
                        let scope = self.scopes.last().unwrap();
                        self.scopes.push(Scope::new(
                            &call.def.environment.values,
                            &call.def.environment.defs,
                            scope.options,
                            scope.context.clone(),
                        ));
                    }
                    self.frames.push(Frame::Argument(call, default, context));
                    self.frames.push(match kind {
                        ConstDefParamKind::Scalar(_) => Frame::Expr(expr),
                        ConstDefParamKind::Array { elem_ty, len } => {
                            Frame::Array(expr, ConstArrayExpectation::fixed(elem_ty, len))
                        }
                        ConstDefParamKind::Slice { elem_ty } => Frame::Array(
                            expr,
                            elem_ty.map_or_else(
                                ConstArrayExpectation::any,
                                ConstArrayExpectation::elem,
                            ),
                        ),
                    });
                }
            }
            Frame::Argument(mut call, default, context) => {
                if default {
                    self.calls.pop();
                }
                let idx = call.args.evaluation_order[call.next];
                let name = &call.def.signature.params[idx];
                match self.stack.pop()? {
                    Value::Expr(expr, _) => {
                        let ConstDefParamKind::Scalar(ty) = call.def.param_kinds.as_ref()?[idx]
                        else {
                            unreachable!()
                        };
                        call.locals.insert(name.clone(), self.typed(&expr, ty)?);
                    }
                    Value::Array(array) => {
                        // Fixed array parameters retain the caller's storage.
                        call.arrays.insert(name.clone(), array);
                    }
                }
                if default {
                    self.scopes.pop();
                }
                self.scope_mut().context = context;
                call.next += 1;
                self.frames.push(Frame::Call(call));
            }
            Frame::EndCall(def) => {
                return self.error(
                    format!("const def '{}' must return a value", def.name),
                    def.loc,
                );
            }
            Frame::Statements(stmts, index) => {
                let Some(stmt) = stmts.get(index) else {
                    return Some(());
                };
                self.frames.push(Frame::Statements(stmts, index + 1));
                match stmt {
                    Stmt::Return { expr, .. } => {
                        self.scope_mut().context.push_str(" return");
                        self.frames.push(Frame::Return);
                        self.frames.push(match self.scope().result? {
                            ConstDefReturn::Scalar(_) => Frame::Expr(expr),
                            ConstDefReturn::Array { elem_ty, len } => {
                                Frame::Array(expr, ConstArrayExpectation::fixed(elem_ty, len))
                            }
                        });
                    }
                    Stmt::Assign { target, expr, .. } => {
                        self.frames.push(Frame::Assign(stmt));
                        // Replacement preserves the destination's concrete
                        // shape, including dimensions deferred by static checking.
                        let expected = match target {
                            AssignTarget::Var(name) => self.scope().arrays.get(name).map(|array| {
                                ConstArrayExpectation::fixed(array.elem_ty(), array.len())
                            }),
                            _ => None,
                        }
                        .or_else(|| {
                            self.scope()
                                .body_types
                                .as_ref()
                                .and_then(|types| types.arrays.get(&(stmt as *const Stmt)))
                                .copied()
                        });
                        if let Some(expected) = expected {
                            if let Stmt::Assign {
                                decl_ty: Some(DeclType::Array { size, .. }),
                                ..
                            } = stmt
                            {
                                self.frames.push(Frame::AssignmentArraySize(expr, expected));
                                self.frames.push(Frame::Expr(size));
                            } else {
                                self.frames.push(Frame::Array(expr, expected));
                            }
                        } else {
                            self.frames.push(Frame::Expr(expr));
                        }
                        if let AssignTarget::Index { index, .. } = target {
                            self.frames.push(Frame::Expr(index));
                        }
                    }
                    Stmt::If { cond, .. } => {
                        self.frames.push(Frame::If(stmt));
                        self.frames.push(Frame::Expr(cond));
                    }
                    Stmt::For {
                        start, end, step, ..
                    } => {
                        self.frames.push(Frame::For(stmt, self.stack.len()));
                        if let Some(step) = step {
                            self.frames.push(Frame::Expr(step));
                        }
                        self.frames.push(Frame::Expr(end));
                        self.frames.push(Frame::Expr(start));
                    }
                    _ => {
                        return self.error(
                            format!("{}: statement is not supported", self.scope().context),
                            stmt.loc(),
                        );
                    }
                }
            }
            Frame::AssignmentArraySize(expr, mut expected) => {
                let size = self.expr()?;
                expected.len = Some(self.checked_array_size(
                    &size,
                    expected.elem_ty?,
                    expected,
                    expr.loc(),
                )?);
                self.frames.push(Frame::Array(expr, expected));
            }
            Frame::Assign(stmt) => {
                let Stmt::Assign {
                    target,
                    generic_decl_ty,
                    decl_ty,
                    expr,
                    ..
                } = stmt
                else {
                    unreachable!()
                };
                if generic_decl_ty.is_some() {
                    return self.error(
                        format!(
                            "{} local declarations cannot use generic types",
                            self.scope().context
                        ),
                        stmt.assign_target_loc(),
                    );
                }
                if let AssignTarget::Index { base, .. } = target {
                    if self.scope().readonly.contains(base) {
                        return self.error(
                            format!(
                                "{} cannot write read-only array parameter '{base}'",
                                self.scope().context
                            ),
                            stmt.assign_target_loc(),
                        );
                    }
                    if self.require(base, Frame::Assign(stmt))? {
                        return Some(());
                    }
                    let value_expr = self.expr()?;
                    let index_expr = self.expr()?;
                    let raw = self.integer(&index_expr)?;
                    let Some(LocalBinding::Array(array)) = self.scope().local_binding(base) else {
                        return self.error(
                            format!(
                                "{} can only write indexed local arrays",
                                self.scope().context
                            ),
                            stmt.assign_target_loc(),
                        );
                    };
                    let len = array.len();
                    let ty = array.elem_ty();
                    let Some(idx) = usize::try_from(raw).ok().filter(|idx| *idx < len) else {
                        return self.error(
                            format!(
                                "{}: array '{base}' index {raw} is out of bounds for length {len}",
                                self.scope().context
                            ),
                            index_expr.loc(),
                        );
                    };
                    let value = self.typed(&value_expr, ty)?;
                    self.scope_mut().arrays.get_mut(base)?.write(idx, value)?;
                } else if let AssignTarget::Var(name) = target {
                    if self
                        .scope()
                        .arrays
                        .get(name)
                        .is_some_and(|array| !array.covers_storage())
                    {
                        // Replacing a fixed parameter backed by a slice merges
                        // its contents into the caller's surrounding storage.
                        if self.require(name, Frame::Assign(stmt))? {
                            return Some(());
                        }
                        if let Some(Value::Array(array)) = self.stack.last() {
                            if self.require_evaluation(array.evaluation(), Frame::Assign(stmt))? {
                                return Some(());
                            }
                        }
                    }
                    match self.stack.pop()? {
                        Value::Array(array) => {
                            self.scope_mut().locals.remove(name);
                            let scope = self.scopes.last().unwrap();
                            if let Some(target) = scope.arrays.get(name) {
                                target.replace(&array, &scope.context, expr.loc(), self.errors)?;
                            } else {
                                let array = if matches!(decl_ty, Some(DeclType::Array { .. }))
                                    && is_array_reference(expr)
                                {
                                    array.copy()
                                } else {
                                    array
                                };
                                self.scope_mut().arrays.insert(name.clone(), array);
                            }
                        }
                        Value::Expr(expr, _) => {
                            if matches!(
                                self.scope().local_binding(name),
                                Some(LocalBinding::Array(_))
                            ) {
                                return self.error(
                                    format!(
                                        "{} cannot assign a scalar to local array '{name}'",
                                        self.scope().context
                                    ),
                                    stmt.assign_target_loc(),
                                );
                            }
                            let ty =
                                self.scope().body_types.as_ref()?.scalars[&(stmt as *const Stmt)];
                            let value = self.typed(&expr, ty)?;
                            self.scope_mut().locals.insert(name.clone(), value);
                        }
                    }
                } else {
                    return self.error(
                        format!(
                            "{} can only assign scalar locals or indexed local arrays",
                            self.scope().context
                        ),
                        stmt.assign_target_loc(),
                    );
                }
            }
            Frame::If(stmt) => {
                let Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } = stmt
                else {
                    unreachable!()
                };
                let expr = self.expr()?;
                let TypedConstValue::Bool(value) = self.typed(&expr, PrimitiveType::Bool)? else {
                    unreachable!()
                };
                if self
                    .scope()
                    .body_types
                    .as_ref()
                    .is_some_and(|types| types.branch_scopes.contains_key(&(stmt as *const Stmt)))
                {
                    self.frames.push(Frame::BranchScope(stmt));
                }
                self.scoped_statements(if value { then_branch } else { else_branch });
            }
            Frame::BranchScope(stmt) => {
                let scope = self.scopes.last_mut().unwrap();
                let joined = &scope.body_types.as_ref()?.branch_scopes[&(stmt as *const Stmt)];
                joined.bindings.discard_bindings(&mut scope.locals);
                joined.bindings.discard_bindings(&mut scope.arrays);
                for (name, ty) in &joined.bindings.widened {
                    let ReturnType::Scalar(ty) = ty else {
                        unreachable!("const defs only join scalar or array locals")
                    };
                    let value = scope.locals.get_mut(name)?;
                    *value = typed_scalar(
                        constant_eval::cast(mir_scalar(*value), scalar_type(*ty))
                            .expect("checked scalar widening"),
                    );
                }
                for (name, expected) in &joined.arrays {
                    let array = scope.arrays.get(name)?;
                    if !check_const_array_shape(
                        array.elem_ty(),
                        array.len(),
                        *expected,
                        &scope.context,
                        stmt.loc(),
                        self.errors,
                    ) {
                        return None;
                    }
                }
            }
            Frame::For(stmt, base) => {
                let Stmt::For {
                    var,
                    var_ty,
                    body,
                    end_inclusive,
                    start,
                    step,
                    ..
                } = stmt
                else {
                    unreachable!()
                };
                let operands = self
                    .stack
                    .drain(base..)
                    .map(|value| match value {
                        Value::Expr(expr, _) => expr,
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>();
                let induction_value = |value| {
                    constant_eval::cast(
                        ScalarValue::I64(value),
                        crate::mir_scalar::scalar_type(*var_ty),
                    )
                    .expect("integer induction type")
                };
                let current = induction_value(self.integer(&operands[0])?);
                let end = induction_value(self.integer(&operands[1])?);
                let step_value = if step.is_some() {
                    induction_value(self.integer(&operands[2])?)
                } else {
                    induction_value(1)
                };
                if matches!(step_value, ScalarValue::I32(0) | ScalarValue::I64(0)) {
                    return self.error(
                        format!("{} for step cannot be 0", self.scope().context),
                        step.as_ref().map(Expr::loc).unwrap_or_else(|| start.loc()),
                    );
                }
                let StaticForPlan::NonEmpty { last, .. } =
                    static_for_plan(current, end, step_value, *end_inclusive)
                        .expect("nonzero integer range")
                else {
                    return Some(());
                };
                let scope = self.scopes.last().unwrap();
                self.frames.push(Frame::Loop(Loop {
                    var,
                    body,
                    current,
                    last,
                    step: step_value,
                    iterations: 0,
                    loc: start.loc(),
                    saved: scope.locals.get(var).copied(),
                    scalar_names: scope.locals.keys().cloned().collect(),
                    array_names: scope.arrays.keys().cloned().collect(),
                }));
            }
            Frame::Loop(mut state) => {
                // Each body execution has a fresh lexical scope. Preserve
                // updates to outer bindings, but drop locals before re-entry.
                if state.iterations > 0 {
                    let scope = self.scope_mut();
                    scope
                        .locals
                        .retain(|name, _| state.scalar_names.contains(name));
                    scope
                        .arrays
                        .retain(|name, _| state.array_names.contains(name));
                }
                if state.iterations > 0 && state.current == state.last {
                    let scope = self.scope_mut();
                    if let Some(value) = state.saved {
                        scope.locals.insert(state.var.to_owned(), value);
                    } else {
                        scope.locals.remove(state.var);
                    }
                } else {
                    if state.iterations >= CONST_DEF_LOOP_ITERATION_LIMIT {
                        return self.error(
                            format!(
                                "{} loop exceeded {CONST_DEF_LOOP_ITERATION_LIMIT} iterations",
                                self.scope().context
                            ),
                            state.loc,
                        );
                    }
                    if state.iterations > 0 {
                        state.current = constant_eval::binary(
                            onda_mir::BinaryOp::Add,
                            state.current,
                            state.step,
                        )
                        .expect("nonfinal bounded iteration");
                    }
                    state.iterations += 1;
                    let value = crate::mir_scalar::typed_scalar(state.current);
                    self.scope_mut().locals.insert(state.var.to_owned(), value);
                    let body = state.body;
                    self.frames.push(Frame::Loop(state));
                    self.scoped_statements(body);
                }
            }
            Frame::Return => {
                if let ConstDefReturn::Scalar(ty) = self.scope().result? {
                    let expr = self.expr()?;
                    let value = self.typed(&expr, ty)?;
                    self.stack
                        .push(Value::expr(concrete_const_expr(value, expr.loc()), true));
                } else if let Some(Value::Array(array)) = self.stack.last_mut() {
                    // Fixed returns capture values, even when returning a caller alias.
                    *array = array.copy();
                }
                while !matches!(self.frames.pop()?, Frame::EndCall(_)) {}
                self.calls.pop();
                self.scopes.pop();
            }
        }
        Some(())
    }

    fn run(mut self) -> Option<Value<'a>> {
        while let Some(frame) = self.frames.pop() {
            if self.step(frame).is_none() {
                // Cache every suspended failure, including shared dependencies.
                for (evaluation, start) in self.active_constants.drain(..).rev() {
                    evaluation
                        .evaluated
                        .set(Err(self.errors[start..].to_vec()))
                        .expect("constant failure cached once");
                    evaluation.resolving.set(false);
                }
                return None;
            }
        }
        self.stack.pop()
    }
}

fn with_diagnostics<T>(
    errors: &mut Vec<Diagnostic>,
    evaluate: impl FnOnce(&mut Vec<Diagnostic>) -> Option<T>,
) -> Option<T> {
    let mut diagnostics = Vec::new();
    let result = evaluate(&mut diagnostics);
    append_const_diagnostics(errors, &diagnostics);
    result
}

pub(super) fn evaluate_constant(entry: &ConstEntry, errors: &mut Vec<Diagnostic>) {
    with_diagnostics(errors, |diagnostics| {
        let mut interpreter = Interpreter::new(
            &entry.environment.values,
            &entry.environment.defs,
            entry.declaration_options(),
            "constant",
            &HashMap::new(),
            &HashMap::new(),
            &[],
            diagnostics,
        );
        interpreter.frames.push(Frame::Constant(
            entry.evaluation(entry.declaration_options()),
        ));
        interpreter.run();
        Some(())
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn fold_expression(
    expr: &Expr,
    locals: &HashMap<String, TypedConstValue>,
    arrays: &HashMap<String, ConstEvalArray>,
    values: &ConstValues,
    defs: ConstDefRegistry<'_>,
    options: AnalysisOptions,
    context: &str,
    calls: &[String],
    errors: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    with_diagnostics(errors, |diagnostics| {
        let mut interpreter = Interpreter::new(
            values,
            defs,
            options,
            context,
            locals,
            arrays,
            calls,
            diagnostics,
        );
        interpreter.frames.push(Frame::Expr(expr));
        match interpreter.run()? {
            Value::Expr(expr, _) => Some(expr),
            _ => None,
        }
    })
}

/// Substitute actual constant reads in an already typed executable expression.
/// Runtime operands remain expressions; constant calls use the same interpreter.
pub(super) fn materialize_expression(
    expr: &Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    materialize_expression_with_facts(expr, artifacts, options, None, errors).map(|(expr, _)| expr)
}

pub(super) fn materialize_expression_with_facts(
    expr: &Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    metadata_options: Option<AnalysisOptions>,
    errors: &mut Vec<Diagnostic>,
) -> Option<(Expr, ExprFacts)> {
    with_diagnostics(errors, |diagnostics| {
        let mut interpreter = Interpreter::new(
            &artifacts.const_values,
            const_def_registry(artifacts),
            options,
            "constant expression",
            &HashMap::new(),
            &HashMap::new(),
            &[],
            diagnostics,
        );
        interpreter.scope_mut().runtime = true;
        if let Some(options) = metadata_options {
            interpreter.scope_mut().metadata_options = options;
        }
        interpreter.frames.push(Frame::Expr(expr));
        match interpreter.run()? {
            Value::Expr(expr, facts) => Some((expr, facts)),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests;
