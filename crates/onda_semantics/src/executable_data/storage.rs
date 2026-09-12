use super::*;

#[derive(Clone)]
pub(crate) enum BindingStorage {
    Scalar(PrimitiveType),
    Data(DataType),
    Tuple(Vec<PrimitiveType>),
}

impl BindingStorage {
    fn tuple_zero_expr(types: &[PrimitiveType]) -> Expr {
        Expr::Tuple {
            loc: Default::default(),
            values: types
                .iter()
                .copied()
                .map(|ty| match ty {
                    PrimitiveType::Bool => Expr::bool(false),
                    _ => cast_expr_to_primitive(zero_expr(ty), ty),
                })
                .collect(),
        }
    }

    fn storage_stmt(&self, name: String, initialize: bool) -> Stmt {
        let (decl_ty, is_typed_decl, expr) = match self {
            Self::Scalar(ty) => (Some(DeclType::Scalar(*ty)), true, zero_expr(*ty)),
            Self::Data(DataType::Struct(struct_name)) => (
                None,
                true,
                Expr::UserCall {
                    loc: Default::default(),
                    name: struct_name.clone(),
                    type_args: Vec::new(),
                    args: Vec::new(),
                },
            ),
            Self::Data(DataType::Array { element, len }) => (
                None,
                true,
                Expr::ArrayCtor {
                    loc: Default::default(),
                    spec: ArrayTypeSpec {
                        elem: element.clone(),
                        size: Box::new(Expr::int(*len as i64)),
                    },
                    init: None,
                    initialize,
                    init_is_value: false,
                },
            ),
            Self::Tuple(types) => (
                Some(DeclType::Tuple(types.clone())),
                true,
                Self::tuple_zero_expr(types),
            ),
        };
        Stmt::Assign {
            loc: Default::default(),
            target_loc: Default::default(),
            target: AssignTarget::Var(name),
            decl_ty,
            generic_decl_ty: match self {
                Self::Data(DataType::Struct(name)) => Some(name.clone()),
                _ => None,
            },
            is_typed_decl,
            typed_decl_ty_loc: Default::default(),
            expr,
        }
    }

    pub(crate) fn init_stmt(&self, name: String) -> Stmt {
        self.storage_stmt(name, true)
    }

    pub(crate) fn declaration_stmt(&self, name: String) -> Stmt {
        self.storage_stmt(name, false)
    }
}

pub(crate) fn assign_var(name: impl Into<String>, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var(name.into()),
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

pub(crate) fn typed_assign(name: impl Into<String>, ty: PrimitiveType, expr: Expr) -> Stmt {
    Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Var(name.into()),
        decl_ty: Some(DeclType::Scalar(ty)),
        generic_decl_ty: None,
        is_typed_decl: true,
        typed_decl_ty_loc: Default::default(),
        expr,
    }
}

#[derive(Default)]
pub(crate) struct DataBindingTypes {
    pub(crate) scalars: HashMap<String, PrimitiveType>,
    pub(crate) arrays: HashMap<String, LocalArrayAliasInfo>,
    pub(crate) tuples: HashMap<String, Vec<PrimitiveType>>,
    pub(crate) structs: HashMap<String, String>,
}

impl DataBindingTypes {
    pub(crate) fn retain_data_structs(&mut self, structs: &HashMap<String, Vec<TypedStructField>>) {
        self.structs.retain(|_, name| structs.contains_key(name));
        self.arrays.retain(|_, info| {
            info.elem_struct
                .as_ref()
                .is_none_or(|name| structs.contains_key(name))
        });
    }

    pub(crate) fn from_flow(flow: &ScopeFlowState) -> Self {
        Self {
            scalars: flow.local_aliases.clone(),
            arrays: flow.local_array_aliases.clone(),
            structs: flow.local_struct_aliases.clone(),
            tuples: flow
                .tuple_vars
                .keys()
                .filter_map(|name| {
                    tracked_local_tuple_types(name, &flow.tuple_vars, &flow.local_aliases)
                        .map(|types| (name.clone(), types))
                })
                .collect(),
        }
    }

    pub(crate) fn storage(&self, name: &str) -> Option<BindingStorage> {
        if let Some(struct_name) = self.structs.get(name) {
            Some(BindingStorage::Data(DataType::Struct(struct_name.clone())))
        } else if let Some(info) = self.arrays.get(name) {
            Some(BindingStorage::Data(DataType::Array {
                element: info
                    .elem_struct
                    .as_ref()
                    .map(|name| ArrayElemType::Struct(name.clone()))
                    .unwrap_or(ArrayElemType::Primitive(info.elem_ty)),
                len: info.static_len?,
            }))
        } else if let Some(types) = self.tuples.get(name) {
            Some(BindingStorage::Tuple(types.clone()))
        } else {
            self.scalars.get(name).copied().map(BindingStorage::Scalar)
        }
    }
}

pub(crate) fn collect_view_names(
    stmts: &[Stmt],
    types: &DataBindingTypes,
    views: &mut HashSet<String>,
) {
    collect_introduced_views(stmts, types, views, &mut HashSet::new());
}

fn collect_introduced_views(
    stmts: &[Stmt],
    types: &DataBindingTypes,
    views: &mut HashSet<String>,
    bound: &mut HashSet<String>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                expr,
                is_typed_decl,
                decl_ty,
                ..
            } if bound.insert(name.clone()) => {
                if (types.arrays.contains_key(name) || types.structs.contains_key(name))
                    && (!*is_typed_decl || matches!(decl_ty, Some(DeclType::Slice(_))))
                    && (matches!(decl_ty, Some(DeclType::Slice(_)))
                        || matches!(expr, Expr::Var { .. } | Expr::Slice { .. })
                        || indexed_read_source(expr).is_some())
                {
                    views.insert(name.clone());
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                let mut left = bound.clone();
                let mut right = bound.clone();
                collect_introduced_views(then_branch, types, views, &mut left);
                collect_introduced_views(else_branch, types, views, &mut right);
                bound.extend(left.intersection(&right).cloned());
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                collect_introduced_views(body, types, views, &mut bound.clone());
            }
            _ => {}
        }
    }
}
