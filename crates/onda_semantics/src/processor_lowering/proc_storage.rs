//! Materializes registered processor operations and storage projections.
//!
//! Logical declarations determine all receiver fields and coordinates. Physical
//! storage names are ordinary output symbols, never a source of semantic facts.

use super::*;

const SELECTED_COORDINATE_ARG: &str = "__onda_selected_proc_coordinate";

pub(super) fn selected_coordinate_argument(arguments: &[CallArg]) -> Option<usize> {
    arguments
        .iter()
        .position(|argument| argument.name.as_deref() == Some(SELECTED_COORDINATE_ARG))
}

fn selected_coordinate(index: &Expr, len: usize, access: IndexAccess) -> CallArg {
    let index = cast_expr_to_primitive(index.clone(), PrimitiveType::I32);
    let expr = match access {
        IndexAccess::Clamp => Expr::Call {
            loc: Default::default(),
            func: BuiltinFn::RangeClamp,
            args: vec![index, Expr::int(0), Expr::int(len as i64 - 1)],
        },
        IndexAccess::Unchecked => index,
    };
    CallArg {
        name: Some(SELECTED_COORDINATE_ARG.to_owned()),
        expr,
    }
}

pub(super) fn select_family_coordinate(
    arguments: &mut Vec<CallArg>,
    index: &Expr,
    len: usize,
    access: IndexAccess,
) {
    let selected = selected_coordinate(index, len, access);
    if let Some(position) = selected_coordinate_argument(arguments) {
        arguments[position] = selected;
    } else {
        arguments.push(selected);
    }
}

pub(crate) fn nested_array_bind_hook_call(
    owner: &str,
    first_slot: &str,
    len: usize,
    index: &Expr,
    hook: &str,
) -> Stmt {
    Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name: proc_local_nested_bind_hidden_def_name(owner, first_slot, hook),
            type_args: Vec::new(),
            args: vec![
                CallArg {
                    name: None,
                    expr: Expr::var("self"),
                },
                selected_coordinate(index, len, IndexAccess::Clamp),
            ],
        },
    }
}

pub(super) fn route_dynamic_family_call(
    target: &mut String,
    arguments: &mut Vec<CallArg>,
    family_target: String,
    slot_instances: &[(String, ProcCallInstance)],
    len: usize,
    index: &Expr,
    access: IndexAccess,
) {
    if !slot_instances
        .iter()
        .any(|(_, instance)| instance.routes_owner_delegates)
    {
        return;
    }
    let Some((_, instance)) = slot_instances.first() else {
        return;
    };
    arguments[0].expr = Expr::var("self");
    arguments.push(selected_coordinate(index, len, access));
    arguments.extend(
        instance
            .delegate_context_args
            .iter()
            .cloned()
            .map(|expr| CallArg { name: None, expr }),
    );
    *target = family_target;
}

pub(super) fn family_coordinate(coordinate_arity: usize) -> Option<Expr> {
    coordinate_arity
        .checked_sub(1)
        .map(|axis| Expr::var(coordinate_name(axis)))
}

pub(super) fn materialize_processor_projections(
    blocks: &mut Vec<Block>,
    shapes: &HashMap<String, ProcLoweringShape>,
    procs: &HashMap<String, ProcessorDef>,
    structs: &HashMap<String, StructDef>,
    oversample_factors: &mut HashMap<String, usize>,
    oversample_meta: &mut HashMap<String, ProcStepOversampleMeta>,
) {
    let families = wrapper_families(blocks, shapes, procs);
    let storage_symbols = super::storage_projections::register_storage_projections(shapes, structs);
    let mut seen = HashSet::new();
    let mut canonical_contexts = HashMap::new();
    let mut selected_sources = HashMap::new();
    blocks.retain_mut(|block| {
        let Block::Def(function) = block else {
            return true;
        };
        if let Some(family) = families.get(&function.name) {
            let source = function.name.clone();
            function.name = family.name.clone();
            if !seen.insert(function.name.clone()) {
                return false;
            }
            canonical_contexts.insert(function.name.clone(), family.clone());
            selected_sources.insert(function.name.clone(), source);
            for axis in (0..family.axes.len()).rev() {
                function.params.insert(
                    1,
                    FnParamDecl {
                        readonly: false,
                        loc: Default::default(),
                        name: coordinate_name(axis),
                        ty: Some(FnParamType::Primitive(PrimitiveType::I32)),
                        ty_loc: Default::default(),
                        default: None,
                    },
                );
            }
        }
        true
    });
    // Metadata uses the same names as generated functions after family sharing.
    let old_factors = std::mem::take(oversample_factors);
    for (name, factor) in old_factors {
        let name = families
            .get(&name)
            .map_or(name.clone(), |family| family.name.clone());
        oversample_factors.insert(name, factor);
    }
    let old_meta = std::mem::take(oversample_meta);
    for (source, meta) in old_meta {
        let name = families
            .get(&source)
            .map_or(source.clone(), |family| family.name.clone());
        if selected_sources
            .get(&name)
            .is_some_and(|selected| *selected != source)
        {
            continue;
        }
        oversample_meta.insert(name, meta);
    }
    for block in blocks {
        let Block::Def(function) = block else {
            continue;
        };
        let Some(FnParamDecl {
            name,
            ty: Some(FnParamType::Struct(owner)),
            ..
        }) = function.params.first()
        else {
            continue;
        };
        if name != "self" || !shapes.contains_key(owner) {
            continue;
        }
        let mut rewriter = ProjectionMaterializer {
            storage_symbols: &storage_symbols,
            shapes,
            owner: owner.clone(),
            aliases: Vec::new(),
            selected: HashMap::new(),
            view_types: HashMap::new(),
            lexical_roots: function
                .params
                .iter()
                .filter(|param| param.name != "self")
                .map(|param| param.name.clone())
                .collect(),
            families: &families,
            coordinates: canonical_contexts
                .get(&function.name)
                .map(|family| {
                    family
                        .axes
                        .iter()
                        .enumerate()
                        .map(|(axis, coordinate)| {
                            (
                                coordinate.identity.clone(),
                                Expr::var(coordinate_name(axis)),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        rewriter.statements(&mut function.body);
        if let Some(meta) = oversample_meta.get_mut(&function.name) {
            for input in meta.input_state_fields.values_mut() {
                for stage in &mut input.up_stages {
                    rewriter.stage(stage);
                }
            }
            let outputs = std::mem::take(&mut meta.output_state_fields);
            for (mut name, mut output) in outputs {
                for stage in &mut output.down_stages {
                    rewriter.stage(stage);
                }
                rewriter.state_name(&mut name);
                meta.output_state_fields.insert(name, output);
            }
            meta.reference_prologue_len = rewriter.aliases.len();
        }
        rewriter.aliases.append(&mut function.body);
        function.body = rewriter.aliases;
    }
}

struct ProjectionMaterializer<'a> {
    storage_symbols: &'a super::storage_projections::StorageProjectionSymbols,
    shapes: &'a HashMap<String, ProcLoweringShape>,
    owner: String,
    aliases: Vec<Stmt>,
    selected: HashMap<String, String>,
    view_types: HashMap<String, String>,
    lexical_roots: HashSet<String>,
    families: &'a HashMap<String, WrapperFamily>,
    coordinates: HashMap<String, Expr>,
}

impl ProjectionMaterializer<'_> {
    fn state_name(&mut self, name: &mut String) {
        let mut qualified = format!("self.{name}");
        self.name(&mut qualified);
        *name = qualified;
    }

    fn stage(&mut self, stage: &mut ProcSincStageStateFields) {
        for name in [
            &mut stage.a0,
            &mut stage.a1,
            &mut stage.a2,
            &mut stage.a3,
            &mut stage.b0,
            &mut stage.b1,
            &mut stage.b2,
            &mut stage.b3,
        ] {
            self.state_name(name);
        }
    }
    fn name(&mut self, name: &mut String) {
        let relative = if let Some(relative) = name.strip_prefix("self.") {
            relative
        } else {
            if crate::path_or_ancestor_is_declared(name, &self.lexical_roots) {
                return;
            }
            name.as_str()
        };
        let owner = self.owner.clone();
        if let Some(resolved) = self.resolve(&owner, relative) {
            *name = resolved;
        }
    }

    fn resolve(&mut self, owner: &str, symbol: &str) -> Option<String> {
        let projection = self.storage_symbols.get(owner)?.get(symbol)?.clone();
        let mut receiver = "self".to_owned();
        for selection in projection.selections {
            receiver = if let Some(alias) = self.selected.get(&selection.identity) {
                alias.clone()
            } else {
                let alias = format!("__onda_proc_storage_view_{}", self.selected.len());
                self.selected
                    .insert(selection.identity.clone(), alias.clone());
                self.view_types.insert(alias.clone(), selection.proc_name);
                self.aliases.push(Stmt::Assign {
                    loc: Default::default(),
                    target_loc: Default::default(),
                    target: AssignTarget::Var(alias.clone()),
                    decl_ty: None,
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    typed_decl_ty_loc: Default::default(),
                    expr: Expr::Index {
                        loc: Default::default(),
                        base: format!("{receiver}.{}", selection.field),
                        index: Box::new(
                            self.coordinates
                                .get(&selection.identity)
                                .cloned()
                                .unwrap_or_else(|| Expr::int(selection.coordinate as i64)),
                        ),
                    },
                });
                alias
            };
        }
        Some(
            projection
                .field
                .map_or(receiver.clone(), |field| format!("{receiver}.{field}")),
        )
    }

    fn expression(&mut self, expression: &mut Expr) {
        expression.visit_mut(|node| {
            match node {
                Expr::Var { name, .. } => self.name(name),
                Expr::Index { base, .. } | Expr::Slice { base, .. } => self.name(base),
                Expr::UserCall { name, args, .. } => {
                    if let Some(family) = self.families.get(name) {
                        let selected = selected_coordinate_argument(args)
                            .map(|position| args.remove(position).expr);
                        let mut coordinates = family
                            .axes
                            .iter()
                            .map(|axis| {
                                self.coordinates
                                    .get(&axis.identity)
                                    .cloned()
                                    .unwrap_or_else(|| Expr::int(axis.index as i64))
                            })
                            .collect::<Vec<_>>();
                        if let (Some(selected), Some(last)) = (selected, coordinates.last_mut()) {
                            *last = selected;
                        }
                        for coordinate in coordinates.into_iter().rev() {
                            args.insert(
                                1,
                                CallArg {
                                    name: None,
                                    expr: coordinate,
                                },
                            );
                        }
                        *name = family.name.clone();
                    }
                }
                _ => {}
            }
            true
        });
    }

    fn target(&mut self, target: &mut AssignTarget) {
        match target {
            AssignTarget::Var(base)
            | AssignTarget::Index { base, .. }
            | AssignTarget::IndexedMember { base, .. }
            | AssignTarget::Slice { base, .. } => self.name(base),
            AssignTarget::Tuple(targets) => {
                for name in targets.iter_mut().filter_map(|target| target.binding_mut()) {
                    self.name(name);
                }
            }
        }
        target.visit_selectors_mut(|selector| self.expression(selector));
        // Processor parameter rewrites express a selected scalar field as a
        // column write. Preserve the nominal array edge until the ordinary
        // indexed-member normalizer constructs its borrowed element view.
        if let AssignTarget::Index { base, index } = target {
            if let Some((array, field)) = self.nominal_scalar_column(base) {
                *target = AssignTarget::IndexedMember {
                    base: array,
                    index: index.clone(),
                    field,
                    field_index: None,
                };
            }
        }
    }

    fn nominal_scalar_column(&self, base: &str) -> Option<(String, String)> {
        let (receiver, path) = base.split_once('.')?;
        let proc_name = if receiver == "self" {
            &self.owner
        } else {
            self.view_types.get(receiver)?
        };
        let shape = self.shapes.get(proc_name)?;
        for field in &shape.fields {
            let FieldType::Array(spec) = &field.ty else {
                continue;
            };
            let ArrayElemType::Struct(child) = &spec.elem else {
                continue;
            };
            let Some(member) = path.strip_prefix(&format!("{}.", field.name)) else {
                continue;
            };
            if self
                .shapes
                .get(child)?
                .fields
                .iter()
                .any(|field| field.name == member && matches!(field.ty, FieldType::Scalar(_)))
            {
                return Some((format!("{receiver}.{}", field.name), member.to_owned()));
            }
        }
        None
    }

    fn statements(&mut self, statements: &mut [Stmt]) {
        for statement in statements {
            match statement {
                Stmt::Assign {
                    target,
                    expr,
                    is_typed_decl,
                    generic_decl_ty,
                    ..
                } => {
                    if *is_typed_decl || generic_decl_ty.is_some() {
                        self.expression(expr);
                        match target {
                            AssignTarget::Var(name) if !name.contains('.') => {
                                self.lexical_roots.insert(name.clone());
                            }
                            AssignTarget::Tuple(targets) => {
                                self.lexical_roots.extend(
                                    targets
                                        .iter()
                                        .filter_map(|target| target.binding())
                                        .map(str::to_owned),
                                );
                            }
                            _ => {}
                        }
                        self.target(target);
                    } else {
                        self.target(target);
                        self.expression(expr);
                    }
                }
                Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => self.expression(expr),
                Stmt::Print { values, .. } => {
                    for value in values {
                        self.expression(value);
                    }
                }
                Stmt::If {
                    cond,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.expression(cond);
                    let outer = self.lexical_roots.clone();
                    self.statements(then_branch);
                    let then_roots = std::mem::replace(&mut self.lexical_roots, outer);
                    self.statements(else_branch);
                    self.lexical_roots.retain(|name| then_roots.contains(name));
                }
                Stmt::For {
                    start,
                    end,
                    step,
                    body,
                    var,
                    ..
                } => {
                    self.expression(start);
                    self.expression(end);
                    if let Some(step) = step {
                        self.expression(step);
                    }
                    let outer = self.lexical_roots.clone();
                    self.lexical_roots.insert(var.clone());
                    self.statements(body);
                    self.lexical_roots = outer;
                }
                Stmt::While { cond, body, .. } => {
                    self.expression(cond);
                    let outer = self.lexical_roots.clone();
                    self.statements(body);
                    self.lexical_roots = outer;
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
        }
    }
}

#[derive(Clone)]
struct FamilyAxis {
    identity: String,
    index: usize,
}

#[derive(Clone)]
struct WrapperFamily {
    name: String,
    axes: Vec<FamilyAxis>,
}

fn coordinate_name(axis: usize) -> String {
    format!(
        "{}{axis}",
        crate::internal_names::PROC_COORDINATE_PARAM_PREFIX
    )
}

fn wrapper_families(
    _blocks: &[Block],
    shapes: &HashMap<String, ProcLoweringShape>,
    procs: &HashMap<String, ProcessorDef>,
) -> HashMap<String, WrapperFamily> {
    let mut families = HashMap::new();
    for owner in shapes.keys() {
        for declared in super::view_catalog::declared_projections(owner, shapes) {
            if declared.projection.coordinate_arity() == 0 {
                continue;
            }
            let Some(child) = procs.get(declared.proc_name()) else {
                continue;
            };
            let Some(shape) = shapes.get(declared.proc_name()) else {
                continue;
            };
            let mut path = String::new();
            let mut axes = Vec::new();
            for edge in &declared.projection.edges {
                let field = if edge.extent.is_some() {
                    format!("{}[0]", edge.declaration)
                } else {
                    edge.declaration.clone()
                };
                path = if path.is_empty() {
                    field
                } else {
                    nested_field_name(&path, &field)
                };
                if edge.extent.is_some() {
                    axes.push(FamilyAxis {
                        identity: path.clone(),
                        index: 0,
                    });
                }
            }
            for (source, name, _) in
                super::view_catalog::projection_operations(&declared, child, shape)
            {
                families.insert(
                    source,
                    WrapperFamily {
                        name,
                        axes: axes.clone(),
                    },
                );
            }
        }
    }
    families
}
