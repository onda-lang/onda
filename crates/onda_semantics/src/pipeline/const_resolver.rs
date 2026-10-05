use super::*;
use crate::compile_context::CompileContext;
use std::cell::{Cell, OnceCell};
use std::rc::Rc;

/// Constants retain declaration options; synthesized defaults inherit the
/// context of the read. Both use the same declaration and evaluator.
#[derive(Debug, Clone, Copy)]
pub(super) enum ConstContext {
    Declaration(AnalysisOptions),
    Caller(AnalysisOptions),
}

impl ConstContext {
    pub(super) fn declaration_options(self) -> AnalysisOptions {
        match self {
            Self::Declaration(options) | Self::Caller(options) => options,
        }
    }

    fn options(self, caller: AnalysisOptions) -> AnalysisOptions {
        match self {
            Self::Declaration(options) => options,
            Self::Caller(_) => caller,
        }
    }
}

/// One shared catalog for semantic consumers. Cloning a typing environment
/// copies this handle, not constant initializers or computed arrays.
#[derive(Debug)]
pub(crate) struct ConstScope {
    artifacts: SemanticConstArtifacts,
}

impl ConstScope {
    pub(super) fn declaration_count(&self) -> usize {
        self.artifacts.const_values.entries.len() + self.artifacts.const_defs.len()
    }
    pub(super) fn new(artifacts: &SemanticConstArtifacts) -> Rc<Self> {
        Rc::new(Self {
            artifacts: artifacts.clone(),
        })
    }

    pub(crate) fn signatures(&self) -> impl Iterator<Item = (&String, &FnSignature)> {
        self.artifacts
            .const_defs
            .iter()
            .map(|(name, def)| (name, &def.signature))
    }

    pub(crate) fn contains_function(&self, name: &str) -> bool {
        self.artifacts.const_defs.contains_key(name)
    }

    pub(crate) fn validate_call(
        &self,
        name: &str,
        args: &[Option<&Expr>],
        env: crate::expr_analysis::ExprEnv<'_>,
        errors: &mut Vec<Diagnostic>,
    ) {
        if let Some(def) = self.artifacts.const_defs.get(name) {
            validate_const_call(def, args, env, errors);
        }
    }

    pub(crate) fn integer(
        &self,
        expr: &Expr,
        symbols: &DeclaredSymbolMap,
        options: AnalysisOptions,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<i64> {
        // Local bindings take precedence over the catalog. Do not evaluate a
        // runtime selector while trying to prove it is compile-time constant.
        if !self.can_evaluate(expr, symbols) {
            return None;
        }
        eval_const_i64_expr_with_defs(
            expr,
            &HashMap::new(),
            &HashMap::new(),
            &self.artifacts.const_values,
            &self.artifacts.const_defs,
            options,
            "constant selector",
            &[],
            errors,
        )
    }

    pub(crate) fn data_size(
        &self,
        expr: &Expr,
        symbols: &DeclaredSymbolMap,
        options: AnalysisOptions,
        context: &str,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<usize> {
        if !self.can_evaluate(expr, symbols) {
            return eval_data_size_expr(expr, options, context, errors);
        }
        let folded = fold_const_eval_expr(
            expr,
            &HashMap::new(),
            &HashMap::new(),
            &self.artifacts.const_values,
            &self.artifacts.const_defs,
            options,
            context,
            &[],
            errors,
        )?;
        eval_data_size_expr(&folded, options, context, errors)
    }

    fn can_evaluate(&self, expr: &Expr, symbols: &DeclaredSymbolMap) -> bool {
        can_evaluate_const_expression(expr, symbols, &self.artifacts.const_defs)
    }
}

/// A metadata probe may execute only closed constant expressions. Runtime
/// lengths and lexical bindings need the later checked expression environment.
pub(super) fn can_evaluate_const_expression(
    expr: &Expr,
    symbols: &DeclaredSymbolMap,
    defs: &HashMap<String, Rc<ConstDefinition>>,
) -> bool {
    expr.walk()
        .all(|node| can_evaluate_const_node(node, symbols, defs))
}

pub(super) fn can_evaluate_const_node(
    expr: &Expr,
    symbols: &DeclaredSymbolMap,
    defs: &HashMap<String, Rc<ConstDefinition>>,
) -> bool {
    match expr {
        Expr::Var { name, .. } => {
            is_builtin_constant_name(name)
                || matches!(
                    symbols.get(name),
                    Some(
                        DeclaredSymbolInfo::Constant { .. } | DeclaredSymbolInfo::ConstArray { .. }
                    )
                )
        }
        Expr::Index { base, .. } | Expr::Slice { base, .. } => matches!(
            symbols.get(base),
            Some(DeclaredSymbolInfo::ConstArray { .. })
        ),
        Expr::UserCall { name, .. } => {
            defs.contains_key(name)
                || parse_array_len_instance_base(name).is_some_and(|base| {
                    matches!(
                        symbols.get(base),
                        Some(DeclaredSymbolInfo::ConstArray { .. })
                    )
                })
        }
        _ => true,
    }
}

pub(super) fn refresh_const_type_environments<'a>(
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    environments: impl IntoIterator<Item = &'a mut crate::def_semantics::CallTypeEnv>,
) {
    let mut refreshed = None;
    for environment in environments {
        if environment
            .const_symbols
            .const_scope
            .as_ref()
            .is_some_and(|scope| {
                scope.declaration_count()
                    == artifacts.const_values.entries.len() + artifacts.const_defs.len()
            })
        {
            continue;
        }
        let (scalars, scope) = refreshed
            .get_or_insert_with(|| (const_scalar_type_map(artifacts), ConstScope::new(artifacts)));
        environment.bind_constants(scalars, &artifacts.const_array_infos, Some(scope), options);
    }
}

/// Lexical visibility is captured as references to declarations, never copies
/// of their values. Namespace snapshots and subsequent analysis share caches.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConstValues {
    entries: HashMap<String, Rc<ConstEntry>>,
}

/// Evaluation and runtime arrays share immutable storage. Host configuration
/// descriptors request owned values explicitly through `to_value`.
#[derive(Debug, PartialEq)]
pub(super) enum ResolvedConstValue {
    Scalar(TypedConstValue),
    Array(ConstEvalArray),
}

impl ResolvedConstValue {
    pub(super) fn to_value(&self) -> ConstValue {
        match self {
            Self::Scalar(value) => ConstValue::Scalar(*value),
            Self::Array(array) => ConstValue::Array {
                elem_ty: array.elem_ty,
                len: array.len(),
                values: array.values.as_ref().clone(),
            },
        }
    }
}

#[derive(Debug)]
pub(super) struct ConstEntry {
    pub(super) decl: ConstDecl,
    pub(super) array: Option<TypedArrayInfo>,
    context: ConstContext,
    pub(super) environment: ConstEnvironment,
    pub(super) cache: ConstCache,
    contextual_caches: RefCell<HashMap<CompileContext, Rc<ConstCache>>>,
    pub(super) scalar_type: Option<PrimitiveType>,
}

#[derive(Debug, Default)]
pub(super) struct ConstCache {
    pub(super) evaluated: OnceCell<Result<ResolvedConstValue, Vec<Diagnostic>>>,
    pub(super) resolving: Cell<bool>,
}

#[derive(Clone)]
enum CacheRef<'a> {
    Declaration(&'a ConstCache),
    Contextual(Rc<ConstCache>),
}

/// A lazy read binds its context once, including when an array is forwarded
/// into a different evaluator scope before its payload is demanded.
#[derive(Clone)]
pub(super) struct ConstEvaluation<'a> {
    pub(super) entry: &'a ConstEntry,
    pub(super) options: AnalysisOptions,
    cache: CacheRef<'a>,
}

impl std::ops::Deref for ConstEvaluation<'_> {
    type Target = ConstCache;
    fn deref(&self) -> &Self::Target {
        match &self.cache {
            CacheRef::Declaration(cache) => cache,
            CacheRef::Contextual(cache) => cache,
        }
    }
}

pub(super) fn append_const_diagnostics(errors: &mut Vec<Diagnostic>, diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        if !errors.contains(diagnostic) {
            errors.push(diagnostic.clone());
        }
    }
}

impl ConstCache {
    pub(super) fn cached_value(&self, errors: &mut Vec<Diagnostic>) -> Option<&ResolvedConstValue> {
        match self.evaluated.get()? {
            Ok(value) => Some(value),
            Err(diagnostics) => {
                append_const_diagnostics(errors, diagnostics);
                None
            }
        }
    }
}

impl ConstEntry {
    pub(super) fn declaration_options(&self) -> AnalysisOptions {
        self.context.declaration_options()
    }

    pub(super) fn evaluation(&self, caller: AnalysisOptions) -> ConstEvaluation<'_> {
        let options = self.context.options(caller);
        let context = CompileContext::new(options);
        let cache = if context == CompileContext::new(self.declaration_options()) {
            CacheRef::Declaration(&self.cache)
        } else {
            CacheRef::Contextual(
                self.contextual_caches
                    .borrow_mut()
                    .entry(context)
                    .or_default()
                    .clone(),
            )
        };
        ConstEvaluation {
            entry: self,
            options,
            cache,
        }
    }

    pub(super) fn runtime_name(&self, name: &str, caller: AnalysisOptions) -> String {
        let context = CompileContext::new(self.context.options(caller));
        if context == CompileContext::new(self.declaration_options()) {
            name.to_owned()
        } else {
            context.specialized_name(name)
        }
    }
}

impl ConstValues {
    pub(super) fn contains_key(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    /// Inspect the declaration-context cache without triggering evaluation.
    #[cfg(test)]
    pub(super) fn get(&self, name: &str) -> Option<&ResolvedConstValue> {
        self.entries.get(name)?.cache.evaluated.get()?.as_ref().ok()
    }

    #[cfg(test)]
    pub(super) fn iter(&self) -> impl Iterator<Item = (&String, &ResolvedConstValue)> {
        self.entries
            .iter()
            .filter_map(|(name, entry)| Some((name, entry.cache.evaluated.get()?.as_ref().ok()?)))
    }

    pub(super) fn scalar_type(&self, name: &str) -> Option<PrimitiveType> {
        self.entries.get(name)?.scalar_type
    }

    /// Eager scalar declarations provide fixed metadata. Caller-context
    /// defaults remain deferred even if one context has populated their cache.
    pub(super) fn scalar_value(&self, name: &str) -> Option<TypedConstValue> {
        let entry = self.entries.get(name)?;
        if !matches!(entry.context, ConstContext::Declaration(_)) {
            return None;
        }
        match entry.cache.evaluated.get()?.as_ref().ok()? {
            ResolvedConstValue::Scalar(value) => Some(*value),
            ResolvedConstValue::Array(_) => None,
        }
    }

    pub(super) fn array_info(&self, name: &str) -> Option<TypedArrayInfo> {
        self.entries.get(name)?.array
    }

    pub(super) fn register(
        &mut self,
        decl: &ConstDecl,
        array: Option<TypedArrayInfo>,
        scalar_type: Option<PrimitiveType>,
        context: ConstContext,
        dependencies: &ConstDependencies,
        defs: &HashMap<String, Rc<ConstDefinition>>,
    ) {
        self.register_named(
            &decl.name,
            decl,
            array,
            scalar_type,
            context,
            dependencies,
            defs,
        );
    }

    pub(super) fn register_named(
        &mut self,
        name: &str,
        decl: &ConstDecl,
        array: Option<TypedArrayInfo>,
        scalar_type: Option<PrimitiveType>,
        context: ConstContext,
        dependencies: &ConstDependencies,
        defs: &HashMap<String, Rc<ConstDefinition>>,
    ) {
        let environment = ConstEnvironment::capture(dependencies, self, defs);
        self.entries.insert(
            name.to_owned(),
            Rc::new(ConstEntry {
                decl: decl.clone(),
                array,
                context,
                environment,
                cache: ConstCache::default(),
                contextual_caches: RefCell::new(HashMap::new()),
                scalar_type,
            }),
        );
    }

    pub(super) fn entry(&self, name: &str) -> Option<&ConstEntry> {
        self.entries.get(name).map(Rc::as_ref)
    }

    /// The interpreter suspends at a read and evaluates that declaration on its
    /// own heap frame. Neither callers nor initializers are restarted.
    pub(super) fn resolve(
        &self,
        name: &str,
        errors: &mut Vec<Diagnostic>,
    ) -> Option<&ResolvedConstValue> {
        let entry = self.entry(name)?;
        if entry.cache.evaluated.get().is_none() {
            const_interpreter::evaluate_constant(entry, errors);
        }
        entry.cache.cached_value(errors)
    }

    pub(super) fn import(&mut self, name: &str, source: &Self) {
        if let Some(entry) = source.entries.get(name) {
            self.entries.insert(name.to_owned(), entry.clone());
        }
    }

    /// Export only references left in executable code after materialization.
    /// Compile-time intermediates stay inside the semantic transaction.
    pub(super) fn runtime_arrays(&self, names: &HashSet<String>) -> Vec<TypedConstArray> {
        let mut arrays = Vec::new();
        let mut append = |name: &str, context: Option<CompileContext>, cache: &ConstCache| {
            if let Some(Ok(ResolvedConstValue::Array(array))) = cache.evaluated.get() {
                let name = context
                    .map_or_else(|| name.to_owned(), |context| context.specialized_name(name));
                if !names.contains(&name) {
                    return;
                }
                arrays.push(TypedConstArray {
                    name,
                    elem_ty: array.elem_ty,
                    len: array.len(),
                    values: array.values.clone(),
                });
            }
        };
        for (name, entry) in &self.entries {
            append(name, None, &entry.cache);
            for (context, cache) in entry.contextual_caches.borrow().iter() {
                append(name, Some(*context), cache);
            }
        }
        arrays.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
        arrays
    }
}

/// Direct lexical references shared by constant declarations and const defs.
#[derive(Debug, Default)]
pub(super) struct ConstEnvironment {
    pub(super) values: ConstValues,
    pub(super) defs: HashMap<String, Rc<ConstDefinition>>,
}

impl ConstEnvironment {
    pub(super) fn capture(
        dependencies: &ConstDependencies,
        values: &ConstValues,
        defs: &HashMap<String, Rc<ConstDefinition>>,
    ) -> Self {
        Self {
            values: ConstValues {
                entries: dependencies
                    .values
                    .iter()
                    .filter_map(|name| Some((name.clone(), values.entries.get(name)?.clone())))
                    .collect(),
            },
            defs: dependencies
                .defs
                .iter()
                .filter_map(|name| Some((name.clone(), defs.get(name)?.clone())))
                .collect(),
        }
    }
}

// Environments form a DAG of both constants and defs. Release it on the heap
// so dropping a long dependency chain cannot overflow an analysis worker stack.
enum Declaration {
    Value(Rc<ConstEntry>),
    Def(Rc<ConstDefinition>),
}

fn release_declarations(mut pending: Vec<Declaration>) {
    while let Some(declaration) = pending.pop() {
        let environment = match declaration {
            Declaration::Value(value) => Rc::try_unwrap(value)
                .ok()
                .map(|mut value| std::mem::take(&mut value.environment)),
            Declaration::Def(def) => Rc::try_unwrap(def)
                .ok()
                .map(|mut def| std::mem::take(&mut def.environment)),
        };
        if let Some(mut environment) = environment {
            pending.extend(
                std::mem::take(&mut environment.values.entries)
                    .into_values()
                    .map(Declaration::Value),
            );
            pending.extend(
                std::mem::take(&mut environment.defs)
                    .into_values()
                    .map(Declaration::Def),
            );
        }
    }
}

impl Drop for ConstValues {
    fn drop(&mut self) {
        release_declarations(
            std::mem::take(&mut self.entries)
                .into_values()
                .map(Declaration::Value)
                .collect(),
        );
    }
}

impl Drop for ConstEnvironment {
    fn drop(&mut self) {
        let mut pending = std::mem::take(&mut self.values.entries)
            .into_values()
            .map(Declaration::Value)
            .collect::<Vec<_>>();
        pending.extend(
            std::mem::take(&mut self.defs)
                .into_values()
                .map(Declaration::Def),
        );
        release_declarations(pending);
    }
}
