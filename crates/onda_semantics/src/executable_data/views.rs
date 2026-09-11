use super::*;

/// Task frames retain scalar selections. A view is reconstructed in the CFG
/// block that uses it, so neither a native pointer nor an expired local
/// descriptor crosses suspension. The ordinary data lowerer owns addressing,
/// clamping, shape checks, and permissions.
#[derive(Clone, Default)]
pub(crate) struct CapturedViews {
    recipes: HashMap<String, Recipe>,
    prefix: String,
    occupied: HashSet<String>,
}

#[derive(Clone)]
enum Recipe {
    Selection(Expr),
    Choice {
        condition: String,
        left: Box<Recipe>,
        right: Box<Recipe>,
    },
}

pub(crate) struct ViewScope<'a> {
    pub prefix: &'a str,
    pub boundary: &'a str,
    pub declared_symbols: &'a DeclaredSymbolMap,
    pub reserved: &'a HashSet<String>,
}

impl CapturedViews {
    pub(crate) fn prepare(
        body: &mut Vec<Stmt>,
        view_names: &HashSet<String>,
        live: &HashSet<String>,
        names: &mut HashMap<String, String>,
        types: &mut DataBindingTypes,
        scope: ViewScope<'_>,
        errors: &mut Vec<Diagnostic>,
    ) -> Self {
        if view_names.is_empty() {
            return Self::default();
        }
        let occupied = scope
            .reserved
            .union(&names.keys().cloned().collect())
            .cloned()
            .collect();
        let mut planner = Planner {
            views: view_names,
            names,
            types,
            next: 0,
            recipes: HashMap::new(),
            prefix: scope.prefix,
            occupied,
        };
        planner.capture(body, &mut HashMap::new());
        let result = Self {
            recipes: planner.recipes,
            prefix: scope.prefix.to_owned(),
            occupied: planner.occupied,
        };
        for name in view_names {
            if live.contains(name) {
                let mut roots = HashSet::new();
                result.storage_uses(name, &mut roots, &mut HashSet::new());
                if roots
                    .iter()
                    .any(|name| has_declared_buffer_symbol_info(scope.declared_symbols, name))
                {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "data view '{}' borrows external memory that cannot survive {}",
                            planner.names.get(name).unwrap_or(name),
                            scope.boundary
                        ),
                        0,
                        0,
                    ));
                }
            }
            planner.names.remove(name);
        }
        result
    }

    pub(crate) fn storage_uses(
        &self,
        name: &str,
        uses: &mut HashSet<String>,
        visited: &mut HashSet<String>,
    ) {
        let root = name.split('.').next().unwrap_or(name);
        if !visited.insert(root.to_owned()) {
            return;
        }
        if let Some(recipe) = self.recipes.get(root) {
            let mut dependencies = HashSet::new();
            recipe.uses(&mut dependencies);
            for dependency in dependencies {
                self.storage_uses(&dependency, uses, visited);
            }
        } else {
            uses.insert(root.to_owned());
        }
    }

    pub(crate) fn extend(&mut self, other: &Self) {
        if self.prefix.is_empty() {
            self.prefix.clone_from(&other.prefix);
        }
        self.recipes.extend(other.recipes.clone());
        self.occupied.extend(other.occupied.iter().cloned());
    }

    pub(crate) fn without_bindings(&self, names: impl IntoIterator<Item = String>) -> Self {
        let mut result = self.clone();
        for name in names {
            result.recipes.remove(&name);
        }
        result
    }

    pub(crate) fn rewrite_storage_names(&mut self, names: &HashMap<String, String>) {
        for recipe in self.recipes.values_mut() {
            recipe.rewrite(names);
        }
    }

    pub(crate) fn expand_body(&self, body: &mut Vec<Stmt>) {
        self.expand_statements(body, &mut 0);
    }

    pub(crate) fn expand_body_with_counter(&self, body: &mut Vec<Stmt>, next: &mut usize) {
        self.expand_statements(body, next);
    }

    pub(crate) fn expand_expression(
        &self,
        expression: &mut Expr,
        body: &mut Vec<Stmt>,
        next: &mut usize,
    ) {
        let mut uses = HashSet::new();
        collect_expr_uses(expression, &mut uses);
        let names = self.materialize_uses(uses, body, next);
        rewrite_binding_expr(expression, &names);
    }

    fn expand_statements(&self, body: &mut Vec<Stmt>, next: &mut usize) {
        let mut result = Vec::new();
        for mut stmt in std::mem::take(body) {
            let mut uses = HashSet::new();
            match &mut stmt {
                Stmt::Assign { target, expr, .. } => {
                    collect_expr_uses(expr, &mut uses);
                    match target {
                        AssignTarget::Var(name) => {
                            uses.insert(name.clone());
                        }
                        AssignTarget::Index { base, index } => {
                            uses.insert(base.clone());
                            collect_expr_uses(index, &mut uses);
                        }
                        AssignTarget::IndexedMember { base, index, .. } => {
                            uses.insert(base.clone());
                            collect_expr_uses(index, &mut uses);
                        }
                        AssignTarget::Slice {
                            base,
                            selector,
                            channel,
                            start,
                            end,
                        } => {
                            uses.insert(base.clone());
                            for expr in [selector, channel, start, end].into_iter().flatten() {
                                collect_expr_uses(expr, &mut uses);
                            }
                        }
                        AssignTarget::Tuple(_) => {}
                    }
                }
                Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                    collect_expr_uses(expr, &mut uses)
                }
                Stmt::Const { decl, .. } => collect_expr_uses(&decl.expr, &mut uses),
                Stmt::Print { values, .. } => {
                    for value in values {
                        collect_expr_uses(value, &mut uses);
                    }
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    collect_expr_uses(cond, &mut uses);
                    self.expand_statements(then_branch, next);
                    self.expand_statements(else_branch, next);
                }
                Stmt::For {
                    start,
                    end,
                    step,
                    body,
                    ..
                } => {
                    collect_expr_uses(start, &mut uses);
                    collect_expr_uses(end, &mut uses);
                    if let Some(step) = step {
                        collect_expr_uses(step, &mut uses);
                    }
                    self.expand_statements(body, next);
                }
                Stmt::While { cond, body, .. } => {
                    collect_expr_uses(cond, &mut uses);
                    self.expand_statements(body, next);
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
            let names = self.materialize_uses(uses, &mut result, next);
            rewrite_binding_stmts(std::slice::from_mut(&mut stmt), &names, &HashSet::new());
            result.push(stmt);
        }
        *body = result;
    }

    fn materialize_uses(
        &self,
        uses: HashSet<String>,
        body: &mut Vec<Stmt>,
        next: &mut usize,
    ) -> HashMap<String, String> {
        let mut roots = uses
            .into_iter()
            .map(|name| name.split('.').next().unwrap().to_owned())
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        let mut names = HashMap::new();
        for root in roots {
            if let Some(recipe) = self.recipes.get(&root) {
                let name = loop {
                    let candidate = format!("{}_use_{}", self.prefix, *next);
                    *next += 1;
                    if !self.occupied.contains(&candidate) {
                        break candidate;
                    }
                };
                self.materialize(recipe, &name, body, next);
                names.insert(root, name);
            }
        }
        names
    }

    fn materialize(&self, recipe: &Recipe, name: &str, body: &mut Vec<Stmt>, next: &mut usize) {
        match recipe {
            Recipe::Selection(expr) => {
                let mut uses = HashSet::new();
                collect_expr_uses(expr, &mut uses);
                let names = self.materialize_uses(uses, body, next);
                let mut expr = expr.clone();
                rewrite_binding_expr(&mut expr, &names);
                body.push(assign_var(name, expr));
            }
            Recipe::Choice {
                condition,
                left,
                right,
            } => {
                let mut then_branch = Vec::new();
                let mut else_branch = Vec::new();
                self.materialize(left, name, &mut then_branch, next);
                self.materialize(right, name, &mut else_branch, next);
                body.push(Stmt::If {
                    loc: Default::default(),
                    cond: Expr::var(condition),
                    then_branch,
                    else_branch,
                });
            }
        }
    }
}

impl Recipe {
    fn uses(&self, uses: &mut HashSet<String>) {
        match self {
            Self::Selection(expr) => collect_expr_uses(expr, uses),
            Self::Choice {
                condition,
                left,
                right,
            } => {
                uses.insert(condition.clone());
                left.uses(uses);
                right.uses(uses);
            }
        }
    }

    fn rewrite(&mut self, names: &HashMap<String, String>) {
        match self {
            Self::Selection(expr) => rewrite_binding_expr(expr, names),
            Self::Choice {
                condition,
                left,
                right,
            } => {
                rewrite_binding_path(condition, names);
                left.rewrite(names);
                right.rewrite(names);
            }
        }
    }
}

struct Planner<'a> {
    views: &'a HashSet<String>,
    prefix: &'a str,
    occupied: HashSet<String>,
    names: &'a mut HashMap<String, String>,
    types: &'a mut DataBindingTypes,
    next: usize,
    recipes: HashMap<String, Recipe>,
}

impl Planner<'_> {
    fn fresh(&mut self, purpose: &str) -> String {
        loop {
            let name = format!("{}_{purpose}_{}", self.prefix, self.next);
            self.next += 1;
            if self.occupied.insert(name.clone()) {
                return name;
            }
        }
    }

    fn coordinate(&mut self, expr: &mut Expr, ty: PrimitiveType, body: &mut Vec<Stmt>) -> String {
        let name = self.fresh("selection");
        self.names.insert(name.clone(), name.clone());
        self.types.scalars.insert(name.clone(), ty);
        body.push(typed_assign(
            &name,
            ty,
            cast_expr_to_primitive(expr.clone(), ty),
        ));
        *expr = Expr::var(&name);
        name
    }

    fn contains_view(&self, body: &[Stmt]) -> bool {
        body.iter().any(|stmt| match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            } => self.views.contains(name),
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => self.contains_view(then_branch) || self.contains_view(else_branch),
            Stmt::For { body, .. } | Stmt::While { body, .. } => self.contains_view(body),
            _ => false,
        })
    }

    fn capture(&mut self, body: &mut Vec<Stmt>, visible: &mut HashMap<String, Recipe>) {
        let mut result = Vec::new();
        for mut stmt in std::mem::take(body) {
            match &mut stmt {
                Stmt::Assign {
                    target: AssignTarget::Var(name),
                    expr,
                    decl_ty,
                    generic_decl_ty,
                    is_typed_decl,
                    ..
                } if self.views.contains(name) && !visible.contains_key(name) => {
                    let owns_data = *is_typed_decl && !matches!(decl_ty, Some(DeclType::Slice(_)));
                    if owns_data
                        || (!matches!(
                            expr,
                            Expr::Var { .. } | Expr::Index { .. } | Expr::Slice { .. }
                        ) && indexed_read_source(expr).is_none())
                    {
                        if let Some(struct_name) = self.types.structs.get(name).cloned() {
                            let backing = self.fresh("backing");
                            let mut declaration =
                                BindingStorage::Data(DataType::Struct(struct_name.clone()))
                                    .init_stmt(backing.clone());
                            let Stmt::Assign { expr: value, .. } = &mut declaration else {
                                unreachable!()
                            };
                            *value = expr.clone();
                            result.push(declaration);
                            self.names.insert(backing.clone(), backing.clone());
                            self.types.structs.insert(backing.clone(), struct_name);
                            *expr = Expr::var(backing);
                        }
                        if let Some(info) = self.types.arrays.get(name).cloned() {
                            if let Some(len) = info.static_len.or(info.proven_len) {
                                let backing = self.fresh("backing");
                                let element = info
                                    .elem_struct
                                    .as_ref()
                                    .map(|name| ArrayElemType::Struct(name.clone()))
                                    .unwrap_or(ArrayElemType::Primitive(info.elem_ty));
                                let (init, init_is_value) = match expr {
                                    Expr::ArrayLiteral { values, .. } => (values.clone(), false),
                                    _ => (vec![expr.clone()], true),
                                };
                                let mut declaration = BindingStorage::Data(DataType::Array {
                                    element: element.clone(),
                                    len,
                                })
                                .init_stmt(backing.clone());
                                let Stmt::Assign { expr: value, .. } = &mut declaration else {
                                    unreachable!()
                                };
                                *value = Expr::ArrayCtor {
                                    loc: expr.loc().into(),
                                    spec: ArrayTypeSpec {
                                        elem: element,
                                        size: Box::new(Expr::int(len as i64)),
                                    },
                                    init: Some(init),
                                    initialize: true,
                                    init_is_value,
                                };
                                result.push(declaration);
                                self.names.insert(backing.clone(), backing.clone());
                                self.types.arrays.insert(
                                    backing.clone(),
                                    LocalArrayAliasInfo {
                                        static_len: Some(len),
                                        ..info
                                    },
                                );
                                *expr = Expr::Slice {
                                    loc: expr.loc().into(),
                                    base: backing,
                                    selector: None,
                                    channel: None,
                                    start: None,
                                    end: None,
                                };
                            }
                        }
                    }
                    if owns_data {
                        *decl_ty = None;
                        *generic_decl_ty = None;
                        *is_typed_decl = false;
                    }
                    match expr {
                        Expr::Index { index, .. } => {
                            self.coordinate(index, PrimitiveType::I32, &mut result);
                        }
                        Expr::Slice {
                            selector,
                            channel,
                            start,
                            end,
                            ..
                        } => {
                            for expr in [selector, channel, start, end].into_iter().flatten() {
                                self.coordinate(expr, PrimitiveType::I32, &mut result);
                            }
                        }
                        Expr::UserCall { name, args, .. }
                            if name == READ_UNSAFE_FN && args.len() == 2 =>
                        {
                            self.coordinate(&mut args[1].expr, PrimitiveType::I32, &mut result);
                        }
                        _ => {}
                    }
                    let mut selection = expr.clone();
                    if let Some(source) = indexed_read_source(expr) {
                        selection = Expr::Index {
                            loc: expr.loc().into(),
                            base: source.base.to_owned(),
                            index: Box::new(source.index.clone()),
                        };
                    }
                    let recipe = Recipe::Selection(selection);
                    visible.insert(name.clone(), recipe.clone());
                    self.recipes.insert(name.clone(), recipe);
                    // Form the original view at its declaration too, preserving
                    // failures such as selecting an element of an empty slice.
                    *name = self.fresh("check");
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } if self.contains_view(then_branch) || self.contains_view(else_branch) => {
                    let condition = self.coordinate(cond, PrimitiveType::Bool, &mut result);
                    let mut left = visible.clone();
                    let mut right = visible.clone();
                    self.capture(then_branch, &mut left);
                    self.capture(else_branch, &mut right);
                    for (name, recipe) in left {
                        if visible.contains_key(&name) {
                            continue;
                        }
                        if let Some(other) = right.get(&name) {
                            let recipe = Recipe::Choice {
                                condition: condition.clone(),
                                left: Box::new(recipe),
                                right: Box::new(other.clone()),
                            };
                            visible.insert(name.clone(), recipe.clone());
                            self.recipes.insert(name, recipe);
                        }
                    }
                }
                Stmt::For { body, .. } | Stmt::While { body, .. } => {
                    self.capture(body, &mut visible.clone())
                }
                _ => {}
            }
            result.push(stmt);
        }
        *body = result;
    }
}
