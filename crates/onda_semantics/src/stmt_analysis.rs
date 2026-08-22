use std::collections::{HashMap, HashSet};

use crate::*;

mod alias_support;
mod common;
mod indexed_binding;
mod init_analysis;
mod runtime_stmt_analysis;
pub(crate) use alias_support::*;
pub(crate) use common::*;
pub(crate) use indexed_binding::*;
pub(crate) use init_analysis::*;
pub(crate) use runtime_stmt_analysis::*;

#[derive(Clone, Copy)]
pub(crate) struct StmtExprAnalysisEnv<'a> {
    pub(crate) expr_env: ExprEnv<'a>,
    pub(crate) state_scalars: &'a HashMap<String, PrimitiveType>,
    pub(crate) declared_symbols: &'a DeclaredSymbolMap,
    pub(crate) local_aliases: &'a LocalAliasTypes,
    pub(crate) local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    pub(crate) input_names: &'a HashSet<String>,
    pub(crate) output_names: &'a HashSet<String>,
    pub(crate) param_names: &'a HashSet<String>,
}

#[derive(Clone)]
pub(crate) struct ScopeFlowState {
    pub(crate) known_scalars: HashSet<String>,
    pub(crate) local_aliases: LocalAliasTypes,
    pub(crate) integer_ranges: HashMap<String, TypedIntegerRange>,
    pub(crate) local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    pub(crate) local_buffer_aliases: LocalBufferAliases,
    pub(crate) local_proc_aliases: HashMap<String, ProcArrayAliasInfo>,
    pub(crate) local_struct_aliases: HashMap<String, String>,
    pub(crate) tuple_vars: HashMap<String, usize>,
}

impl ScopeFlowState {
    pub(crate) fn from_parts(
        known_scalars: HashSet<String>,
        local_aliases: LocalAliasTypes,
        local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
        local_proc_aliases: HashMap<String, ProcArrayAliasInfo>,
    ) -> Self {
        Self {
            known_scalars,
            local_aliases,
            integer_ranges: HashMap::new(),
            local_array_aliases,
            local_buffer_aliases: HashMap::new(),
            local_proc_aliases,
            local_struct_aliases: HashMap::new(),
            tuple_vars: HashMap::new(),
        }
    }

    pub(crate) fn new(
        known_scalars: HashSet<String>,
        local_aliases: LocalAliasTypes,
        local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    ) -> Self {
        Self::from_parts(
            known_scalars,
            local_aliases,
            local_array_aliases,
            HashMap::new(),
        )
    }
}

pub(crate) fn buffer_reference_expr_info(
    expr: &Expr,
    declared_symbols: &DeclaredSymbolMap,
) -> Option<LocalBufferAliasInfo> {
    let name = match expr {
        Expr::Var { name, .. }
            if has_declared_buffer_symbol_info(declared_symbols, name)
                && !is_declared_buffer_array_info(declared_symbols, name) =>
        {
            name
        }
        Expr::Index { base, .. }
            if has_declared_buffer_symbol_info(declared_symbols, base)
                && is_declared_buffer_array_info(declared_symbols, base) =>
        {
            base
        }
        _ => return None,
    };
    let (elem_ty, channels) = declared_buffer_info(declared_symbols, name)?;
    Some(LocalBufferAliasInfo { elem_ty, channels })
}

pub(crate) fn fork_scope_flow_state_with_tuples(
    known_scalars: &HashSet<String>,
    local_aliases: &LocalAliasTypes,
    integer_ranges: &HashMap<String, TypedIntegerRange>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &HashMap<String, ProcArrayAliasInfo>,
    local_struct_aliases: &HashMap<String, String>,
    local_buffer_aliases: &LocalBufferAliases,
    tuple_vars: &HashMap<String, usize>,
) -> ScopeFlowState {
    let mut st = ScopeFlowState::from_parts(
        known_scalars.clone(),
        local_aliases.clone(),
        local_array_aliases.clone(),
        local_proc_aliases.clone(),
    );
    st.integer_ranges = integer_ranges.clone();
    st.tuple_vars = tuple_vars.clone();
    st.local_buffer_aliases = local_buffer_aliases.clone();
    st.local_struct_aliases = local_struct_aliases.clone();
    st
}

pub(crate) fn merge_branch_scope_flow_state(
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    integer_ranges: &mut HashMap<String, TypedIntegerRange>,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    local_struct_aliases: &mut HashMap<String, String>,
    local_buffer_aliases: &mut LocalBufferAliases,
    tuple_vars: &mut HashMap<String, usize>,
    then_state: ScopeFlowState,
    else_state: ScopeFlowState,
    location: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let base_known_scalars = known_scalars.clone();
    let base_local_aliases = local_aliases.clone();
    let base_integer_ranges = integer_ranges.clone();
    let base_array_aliases = local_array_aliases.clone();
    let base_proc_aliases = local_proc_aliases.clone();
    let base_struct_aliases = local_struct_aliases.clone();
    let base_buffer_aliases = local_buffer_aliases.clone();
    let base_tuple_vars = tuple_vars.clone();
    let then_binding_names = tracked_branch_binding_names(&then_state);
    let else_binding_names = tracked_branch_binding_names(&else_state);
    let common_branch_bindings = then_binding_names
        .intersection(&else_binding_names)
        .cloned()
        .collect::<HashSet<_>>();
    *known_scalars = base_known_scalars.clone();
    *local_aliases = base_local_aliases.clone();
    *integer_ranges = base_integer_ranges;
    *local_array_aliases = base_array_aliases;
    *local_proc_aliases = base_proc_aliases;
    *local_struct_aliases = base_struct_aliases;
    *local_buffer_aliases = base_buffer_aliases;
    *tuple_vars = base_tuple_vars;

    macro_rules! incompatible {
        ($name:expr, $detail:expr $(,)?) => {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "binding '{}' has incompatible branch types: {}",
                    $name, $detail
                ),
                location,
            ))
        };
    }

    for name in common_branch_bindings {
        // Bindings that were already visible before the branch keep their
        // established type. In particular, state tuples are seeded in
        // `tuple_vars` without local element aliases, so trying to join them
        // as branch-created tuple locals would spuriously report unresolved
        // element types.
        if base_known_scalars.contains(&name) || tuple_vars.contains_key(&name) {
            continue;
        }
        let then_kind = tracked_branch_binding_kind(&then_state, &name);
        let else_kind = tracked_branch_binding_kind(&else_state, &name);
        if then_kind != else_kind {
            incompatible!(
                &name,
                format!(
                    "{} and {}",
                    then_kind
                        .map(TrackedBranchBindingKind::name)
                        .unwrap_or("unresolved"),
                    else_kind
                        .map(TrackedBranchBindingKind::name)
                        .unwrap_or("unresolved")
                ),
            );
            continue;
        }

        let joined = match then_kind {
            Some(TrackedBranchBindingKind::Scalar) => {
                let then_range = then_state.integer_ranges.get(&name);
                let else_range = else_state.integer_ranges.get(&name);
                if then_range != else_range {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "binding '{name}' has incompatible branch integer range contracts: {} and {}",
                            format_integer_range_contract(then_range),
                            format_integer_range_contract(else_range),
                        ),
                        location,
                    ));
                    continue;
                }
                let then_ty = then_state.local_aliases[&name];
                let else_ty = else_state.local_aliases[&name];
                let Some(ty) = merge_inferred_return_types(then_ty, else_ty) else {
                    incompatible!(&name, format!("{} and {}", then_ty.name(), else_ty.name()));
                    continue;
                };
                local_aliases.insert(name.clone(), ty);
                if let Some(range) = then_range {
                    integer_ranges.insert(name.clone(), *range);
                }
                true
            }
            Some(TrackedBranchBindingKind::Tuple) => {
                let then_arity = then_state.tuple_vars[&name];
                let else_arity = else_state.tuple_vars[&name];
                if then_arity != else_arity {
                    incompatible!(
                        &name,
                        format!("tuple arities {then_arity} and {else_arity}"),
                    );
                    continue;
                }
                let mut element_types = Vec::with_capacity(then_arity);
                for index in 0..then_arity {
                    let element = format!("{name}[{index}]");
                    let Some((then_ty, else_ty)) = then_state
                        .local_aliases
                        .get(&element)
                        .zip(else_state.local_aliases.get(&element))
                    else {
                        element_types.clear();
                        break;
                    };
                    let Some(ty) = merge_inferred_return_types(*then_ty, *else_ty) else {
                        element_types.clear();
                        break;
                    };
                    element_types.push(ty);
                }
                if element_types.len() != then_arity {
                    incompatible!(&name, "tuple element types do not have a common type",);
                    continue;
                }
                tuple_vars.insert(name.clone(), then_arity);
                replace_tracked_tuple_types(local_aliases, &name, Some(&element_types));
                true
            }
            Some(TrackedBranchBindingKind::Array) => {
                let then_info = &then_state.local_array_aliases[&name];
                let else_info = &else_state.local_array_aliases[&name];
                if then_info.elem_ty != else_info.elem_ty
                    || then_info.elem_struct != else_info.elem_struct
                    || then_info.static_len != else_info.static_len
                {
                    incompatible!(
                        &name,
                        "arrays have different element types or fixed lengths",
                    );
                    continue;
                }
                let mut info = then_info.clone();
                info.len = then_info.len.max(else_info.len);
                info.writable = then_info.writable && else_info.writable;
                local_array_aliases.insert(name.clone(), info);
                true
            }
            Some(TrackedBranchBindingKind::Proc) => {
                let then_alias = &then_state.local_proc_aliases[&name];
                let else_alias = &else_state.local_proc_aliases[&name];
                let then_struct = then_state.local_struct_aliases.get(&name);
                let else_struct = else_state.local_struct_aliases.get(&name);
                if then_alias.array_base != else_alias.array_base || then_struct != else_struct {
                    incompatible!(&name, "processor aliases use different arrays");
                    continue;
                }
                local_proc_aliases.insert(name.clone(), then_alias.clone());
                if let Some(struct_name) = then_struct {
                    local_struct_aliases.insert(name.clone(), struct_name.clone());
                }
                copy_matching_root_aliases(
                    local_aliases,
                    &then_state.local_aliases,
                    &else_state.local_aliases,
                    &name,
                );
                true
            }
            Some(TrackedBranchBindingKind::Struct) => {
                let then_struct = &then_state.local_struct_aliases[&name];
                let else_struct = &else_state.local_struct_aliases[&name];
                if then_struct != else_struct {
                    incompatible!(
                        &name,
                        format!("structs '{then_struct}' and '{else_struct}'"),
                    );
                    continue;
                }
                local_struct_aliases.insert(name.clone(), then_struct.clone());
                copy_matching_root_aliases(
                    local_aliases,
                    &then_state.local_aliases,
                    &else_state.local_aliases,
                    &name,
                );
                true
            }
            Some(TrackedBranchBindingKind::Buffer) => {
                incompatible!(
                    &name,
                    "buffer aliases created inside branches cannot escape the branch",
                );
                false
            }
            None => false,
        };
        if joined {
            known_scalars.insert(name);
        }
    }
}

fn tracked_branch_binding_names(state: &ScopeFlowState) -> HashSet<String> {
    let mut names = state.known_scalars.clone();
    names.extend(state.local_array_aliases.keys().cloned());
    names.extend(state.local_proc_aliases.keys().cloned());
    names.extend(state.local_struct_aliases.keys().cloned());
    names.extend(state.local_buffer_aliases.keys().cloned());
    names.extend(state.tuple_vars.keys().cloned());
    names
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TrackedBranchBindingKind {
    Scalar,
    Tuple,
    Array,
    Proc,
    Struct,
    Buffer,
}

impl TrackedBranchBindingKind {
    fn name(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Tuple => "tuple",
            Self::Array => "array",
            Self::Proc => "proc",
            Self::Struct => "struct",
            Self::Buffer => "buffer",
        }
    }
}

fn tracked_branch_binding_kind(
    state: &ScopeFlowState,
    name: &str,
) -> Option<TrackedBranchBindingKind> {
    if state.local_proc_aliases.contains_key(name) {
        Some(TrackedBranchBindingKind::Proc)
    } else if state.tuple_vars.contains_key(name) {
        Some(TrackedBranchBindingKind::Tuple)
    } else if state.local_array_aliases.contains_key(name) {
        Some(TrackedBranchBindingKind::Array)
    } else if state.local_struct_aliases.contains_key(name) {
        Some(TrackedBranchBindingKind::Struct)
    } else if state.local_buffer_aliases.contains_key(name) {
        Some(TrackedBranchBindingKind::Buffer)
    } else if state.local_aliases.contains_key(name) {
        Some(TrackedBranchBindingKind::Scalar)
    } else {
        None
    }
}

fn copy_matching_root_aliases(
    destination: &mut LocalAliasTypes,
    then_aliases: &LocalAliasTypes,
    else_aliases: &LocalAliasTypes,
    root: &str,
) {
    let field_prefix = format!("{root}.");
    let index_prefix = format!("{root}[");
    destination.extend(
        then_aliases
            .iter()
            .filter(|(name, ty)| {
                (name.starts_with(&field_prefix) || name.starts_with(&index_prefix))
                    && else_aliases.get(*name) == Some(*ty)
            })
            .map(|(name, ty)| (name.clone(), *ty)),
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_reachable_branch_scope_flow_state(
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    integer_ranges: &mut HashMap<String, TypedIntegerRange>,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    local_struct_aliases: &mut HashMap<String, String>,
    local_buffer_aliases: &mut LocalBufferAliases,
    tuple_vars: &mut HashMap<String, usize>,
    then_state: ScopeFlowState,
    then_flow: crate::def_semantics::call_types::StatementFlow,
    else_state: ScopeFlowState,
    else_flow: crate::def_semantics::call_types::StatementFlow,
    location: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    use crate::def_semantics::call_types::StatementFlow;

    let continuing_state = match (then_flow, else_flow) {
        (StatementFlow::Continues, StatementFlow::Terminates) => then_state,
        (StatementFlow::Terminates, StatementFlow::Continues) => else_state,
        (StatementFlow::Continues, StatementFlow::Continues)
        | (StatementFlow::Terminates, StatementFlow::Terminates) => {
            return merge_branch_scope_flow_state(
                known_scalars,
                local_aliases,
                integer_ranges,
                local_array_aliases,
                local_proc_aliases,
                local_struct_aliases,
                local_buffer_aliases,
                tuple_vars,
                then_state,
                else_state,
                location,
                errors,
            );
        }
    };
    *known_scalars = continuing_state.known_scalars;
    *local_aliases = continuing_state.local_aliases;
    *integer_ranges = continuing_state.integer_ranges;
    *local_array_aliases = continuing_state.local_array_aliases;
    *local_proc_aliases = continuing_state.local_proc_aliases;
    *local_struct_aliases = continuing_state.local_struct_aliases;
    *local_buffer_aliases = continuing_state.local_buffer_aliases;
    *tuple_vars = continuing_state.tuple_vars;
}

pub(crate) fn track_integer_range_declaration(
    statement: &Stmt,
    ranges: &mut HashMap<String, TypedIntegerRange>,
) {
    let Stmt::Assign {
        target: AssignTarget::Var(name),
        decl_ty,
        is_typed_decl: true,
        expr,
        ..
    } = statement
    else {
        return;
    };
    let range = typed_integer_range_from_expr(expr, *decl_ty);
    if let Some(range) = range {
        ranges.insert(name.clone(), range);
    } else {
        ranges.remove(name);
    }
}

fn format_integer_range_contract(range: Option<&TypedIntegerRange>) -> String {
    range.map_or_else(
        || "unbounded".into(),
        |range| {
            format!(
                "{} {}({}..={})",
                if range.wrap { "wrap" } else { "clamp" },
                range.ty.name(),
                range.min,
                range.max,
            )
        },
    )
}

pub(crate) fn adopt_loop_scope_flow_state(
    known_scalars: &HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    local_struct_aliases: &mut HashMap<String, String>,
    local_buffer_aliases: &mut LocalBufferAliases,
    tuple_vars: &mut HashMap<String, usize>,
    loop_state: ScopeFlowState,
) {
    let base_array_aliases = local_array_aliases.clone();
    let base_proc_aliases = local_proc_aliases.clone();
    let base_struct_aliases = local_struct_aliases.clone();
    let base_buffer_aliases = local_buffer_aliases.clone();
    *tuple_vars = loop_state.tuple_vars;
    tuple_vars.retain(|name, _| known_scalars.contains(name));
    *local_aliases = loop_state.local_aliases;
    local_aliases.retain(|name, _| {
        known_scalars.contains(name)
            || tracked_tuple_element_root(name).is_some_and(|root| tuple_vars.contains_key(root))
    });
    *local_array_aliases = loop_state.local_array_aliases;
    local_array_aliases.retain(|name, _| base_array_aliases.contains_key(name));
    *local_proc_aliases = loop_state.local_proc_aliases;
    local_proc_aliases.retain(|name, _| base_proc_aliases.contains_key(name));
    *local_struct_aliases = loop_state.local_struct_aliases;
    local_struct_aliases
        .retain(|name, struct_name| base_struct_aliases.get(name) == Some(struct_name));
    *local_buffer_aliases = loop_state.local_buffer_aliases;
    local_buffer_aliases.retain(|name, _| base_buffer_aliases.contains_key(name));
}

fn tracked_tuple_element_root(binding: &str) -> Option<&str> {
    let without_bracket = binding.strip_suffix(']')?;
    let (root, index) = without_bracket.rsplit_once('[')?;
    index.parse::<usize>().ok().map(|_| root)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_stmt_expr_env<'a>(
    expr_env: ExprEnv<'a>,
    state_scalars: &'a HashMap<String, PrimitiveType>,
    declared_symbols: &'a DeclaredSymbolMap,
    local_aliases: &'a LocalAliasTypes,
    local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    input_names: &'a HashSet<String>,
    output_names: &'a HashSet<String>,
    param_names: &'a HashSet<String>,
) -> StmtExprAnalysisEnv<'a> {
    StmtExprAnalysisEnv {
        expr_env,
        state_scalars,
        declared_symbols,
        local_aliases,
        local_array_aliases,
        input_names,
        output_names,
        param_names,
    }
}

pub(crate) fn build_scope_stmt_expr_env<'a>(
    inputs: ScopeExprInputs<'a>,
    known_scalars: &'a HashSet<String>,
    local_aliases: &'a LocalAliasTypes,
    local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    array_vars: &'a HashMap<String, usize>,
    scope: ScopeKind,
) -> StmtExprAnalysisEnv<'a> {
    let mut expr_env =
        build_scope_expr_env(inputs, known_scalars, local_aliases, array_vars, scope);
    expr_env.local_array_aliases = local_array_aliases;
    build_stmt_expr_env(
        expr_env,
        inputs.state_scalars,
        inputs.declared_symbols,
        local_aliases,
        local_array_aliases,
        inputs.input_names,
        inputs.output_names,
        inputs.param_names,
    )
}

pub(crate) fn build_scope_expr_env_with_tuples<'a>(
    inputs: ScopeExprInputs<'a>,
    known_scalars: &'a HashSet<String>,
    local_aliases: &'a LocalAliasTypes,
    array_vars: &'a HashMap<String, usize>,
    scope: ScopeKind,
    tuple_vars: &'a HashMap<String, usize>,
) -> ExprEnv<'a> {
    let mut env = build_scope_expr_env(inputs, known_scalars, local_aliases, array_vars, scope);
    env.tuple_vars = tuple_vars;
    env
}

pub(crate) fn build_scope_stmt_expr_env_with_tuples<'a>(
    inputs: ScopeExprInputs<'a>,
    known_scalars: &'a HashSet<String>,
    local_aliases: &'a LocalAliasTypes,
    local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    array_vars: &'a HashMap<String, usize>,
    scope: ScopeKind,
    tuple_vars: &'a HashMap<String, usize>,
) -> StmtExprAnalysisEnv<'a> {
    let mut expr_env = build_scope_expr_env_with_tuples(
        inputs,
        known_scalars,
        local_aliases,
        array_vars,
        scope,
        tuple_vars,
    );
    expr_env.local_array_aliases = local_array_aliases;
    build_stmt_expr_env(
        expr_env,
        inputs.state_scalars,
        inputs.declared_symbols,
        local_aliases,
        local_array_aliases,
        inputs.input_names,
        inputs.output_names,
        inputs.param_names,
    )
}

pub(crate) fn infer_tracked_tuple_arity(
    expr: &Expr,
    tuple_vars: &HashMap<String, usize>,
    fn_return_types: &HashMap<String, ReturnType>,
) -> Option<usize> {
    match expr {
        Expr::Tuple { values, .. } => Some(values.len()),
        Expr::UserCall { name, .. } => match fn_return_types.get(name.as_str()) {
            Some(ReturnType::Tuple(elem_tys)) => Some(elem_tys.len()),
            _ => None,
        },
        Expr::Var { name, .. } => tuple_vars.get(name).copied(),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn infer_tracked_tuple_types(
    expr: &Expr,
    tuple_vars: &HashMap<String, usize>,
    local_aliases: &LocalAliasTypes,
    state_tuples: Option<&HashMap<String, Vec<PrimitiveType>>>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_return_types: &HashMap<String, ReturnType>,
    mut infer_scalar: impl FnMut(&Expr) -> Option<PrimitiveType>,
) -> Option<Vec<PrimitiveType>> {
    match expr {
        Expr::Tuple { values, .. } => values
            .iter()
            .map(|value| {
                let inferred = infer_scalar(value);
                effective_untyped_assignment_type(value, inferred).or(inferred)
            })
            .collect(),
        Expr::UserCall { name, .. } => match fn_return_types.get(name) {
            Some(ReturnType::Tuple(types)) => Some(types.clone()),
            _ => None,
        },
        Expr::Var { name, .. } => {
            if let Some(types) = state_tuples.and_then(|tuples| tuples.get(name)) {
                return Some(types.clone());
            }
            if let Some(arity) = tuple_vars.get(name).copied() {
                let types = (0..arity)
                    .map(|index| {
                        local_aliases
                            .get(&format!("{name}[{index}]"))
                            .copied()
                            .or_else(|| {
                                infer_scalar(&Expr::Index {
                                    loc: expr.loc().into(),
                                    base: name.clone(),
                                    index: Box::new(Expr::int(index as i64)),
                                })
                            })
                    })
                    .collect::<Option<Vec<_>>>();
                if types.is_some() {
                    return types;
                }
            }
            let (root, field) = split_simple_field_path(name)?;
            let struct_name = struct_instances.get(root)?;
            match &resolve_struct_field_decl(struct_name, field, struct_defs)?.ty {
                TypedFieldType::Tuple(types) => Some(types.clone()),
                TypedFieldType::Scalar(_) | TypedFieldType::Struct | TypedFieldType::Array(_) => {
                    None
                }
            }
        }
        _ => None,
    }
}

pub(crate) fn replace_tracked_tuple_types(
    local_aliases: &mut LocalAliasTypes,
    name: &str,
    types: Option<&[PrimitiveType]>,
) {
    let element_prefix = format!("{name}[");
    local_aliases.retain(|binding, _| !binding.starts_with(&element_prefix));
    if let Some(types) = types {
        local_aliases.extend(
            types
                .iter()
                .enumerate()
                .map(|(index, ty)| (format!("{name}[{index}]"), *ty)),
        );
    }
}

pub(crate) fn tracked_local_tuple_types(
    name: &str,
    tuple_vars: &HashMap<String, usize>,
    local_aliases: &LocalAliasTypes,
) -> Option<Vec<PrimitiveType>> {
    let arity = tuple_vars.get(name).copied()?;
    (0..arity)
        .map(|index| local_aliases.get(&format!("{name}[{index}]")).copied())
        .collect()
}

pub(crate) fn require_tuple_expr_assignable_types(
    name: &str,
    expr: &Expr,
    source_types: &[PrimitiveType],
    target_types: &[PrimitiveType],
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if source_types.len() != target_types.len() {
        errors.push(Diagnostic::semantic_span(
            format!(
                "tuple assignment to '{name}' has arity {}, expected {}",
                source_types.len(),
                target_types.len()
            ),
            expr.loc(),
        ));
        return false;
    }

    let tuple_values = match expr {
        Expr::Tuple { values, .. } => Some(values.as_slice()),
        _ => None,
    };
    let mut compatible = true;
    for (index, (source, target)) in source_types.iter().zip(target_types).enumerate() {
        let component_expr = tuple_values
            .and_then(|values| values.get(index))
            .unwrap_or(expr);
        if !can_assign_expr_to_type(component_expr, *source, *target) {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "tuple assignment to '{name}' element {index} type mismatch: cannot assign {:?} to {:?}",
                    source, target
                ),
                component_expr.loc(),
            ));
            compatible = false;
        }
    }
    compatible
}

pub(crate) fn track_tuple_var_assignment(
    tuple_vars: &mut HashMap<String, usize>,
    name: &str,
    tuple_arity: Option<usize>,
) {
    if let Some(arity) = tuple_arity {
        tuple_vars.insert(name.to_owned(), arity);
    } else {
        tuple_vars.remove(name);
    }
}

pub(crate) fn clear_tuple_var_bindings<'a>(
    tuple_vars: &mut HashMap<String, usize>,
    names: impl IntoIterator<Item = &'a String>,
) {
    for name in names {
        tuple_vars.remove(name);
    }
}

pub(crate) fn validate_and_infer_stmt_expr_type(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    validate_expr(expr, env.expr_env, errors);
    infer_stmt_expr_type(expr, env, errors)
}

fn infer_stmt_expr_type(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
        expr,
        env.state_scalars,
        env.declared_symbols,
        None,
        env.local_aliases,
        env.local_array_aliases,
        env.expr_env.locals,
        env.input_names,
        env.output_names,
        env.param_names,
        env.expr_env.struct_instances,
        env.expr_env.struct_defs,
        env.expr_env.proc_array_roots,
        errors,
    )
}

pub(crate) fn analyze_stmt_expr(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let _ = validate_and_infer_stmt_expr_type(expr, env, errors);
}

pub(crate) fn analyze_standalone_stmt_expr(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    validate_standalone_expr_statement(expr, env.expr_env, errors);
    if !matches!(expr, Expr::UserCall { name, .. } if name == WRITE_UNSAFE_FN) {
        let _ = infer_stmt_expr_type(expr, env, errors);
    }
}

fn is_bare_array_ref_expr(expr: &Expr, env: StmtExprAnalysisEnv<'_>) -> bool {
    let Expr::Var { name, .. } = expr else {
        return false;
    };
    env.expr_env.array_vars.contains_key(name)
        || is_declared_data_array_symbol(env.declared_symbols, name)
        || has_declared_buffer_symbol_info(env.declared_symbols, name)
        || env.expr_env.proc_array_roots.contains_key(name)
}

pub(crate) fn is_data_like_value_expr(expr: &Expr, env: StmtExprAnalysisEnv<'_>) -> bool {
    matches!(expr, Expr::Slice { .. }) || is_bare_array_ref_expr(expr, env)
}

pub(crate) fn validate_data_like_value_expr(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(name) = dynamic_param_surface_value_name(expr, env.expr_env) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "dynamic param array '{name}' is not a first-class value; use '{name}[i]' directly in block or sample"
            ),
            expr.loc(),
        ));
        return;
    }
    if let Some(name) = io_surface_value_name(expr, env.expr_env) {
        if env.expr_env.io_surface_access_allowed {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "I/O array '{name}' is not a first-class value; use '{name}[i]' directly in block or sample"
                ),
                expr.loc(),
            ));
        } else {
            push_io_surface_scope_error(errors, expr.loc(), name);
        }
        return;
    }
    if is_bare_array_ref_expr(expr, env) {
        return;
    }
    validate_expr(expr, env.expr_env, errors);
}

pub(crate) fn analyze_proc_event_arg_expr(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if is_data_like_value_expr(expr, env) {
        validate_data_like_value_expr(expr, env, errors);
        return;
    }
    analyze_stmt_expr(expr, env, errors);
}

pub(crate) fn require_validated_bool_stmt_expr(
    expr: &Expr,
    context: &str,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expr_ty = validate_and_infer_stmt_expr_type(expr, env, errors);
    require_expr_bool_type(expr, expr_ty, context, errors);
}

pub(crate) fn require_validated_numeric_stmt_expr(
    expr: &Expr,
    context: &str,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expr_ty = validate_and_infer_stmt_expr_type(expr, env, errors);
    require_expr_numeric_type(expr, expr_ty, context, errors);
}

pub(crate) fn validate_for_loop_step_expr(
    step_expr: Option<&Expr>,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(step_expr) = step_expr {
        require_validated_numeric_stmt_expr(step_expr, "for loop step", env, errors);
        if matches!(step_expr, Expr::Int { value: 0, .. })
            || matches!(step_expr, Expr::Number { value: v, .. } if *v == 0.0)
        {
            errors.push(Diagnostic::semantic_span(
                "for loop step cannot be zero",
                step_expr.loc(),
            ));
        }
    }
}

pub(crate) fn infer_static_slice_len_hint(
    total_len: Option<usize>,
    start: Option<&Expr>,
    end: Option<&Expr>,
) -> usize {
    let Some(total_len) = total_len else {
        return 1;
    };
    let start = normalize_static_slice_bound(start, total_len, false);
    let end = normalize_static_slice_bound(end, total_len, true);
    end.saturating_sub(start).max(1)
}

fn normalize_static_slice_bound(
    expr: Option<&Expr>,
    total_len: usize,
    default_to_len: bool,
) -> usize {
    let Some(expr) = expr else {
        return if default_to_len { total_len } else { 0 };
    };
    let raw =
        const_slice_bound_i64(expr).unwrap_or(if default_to_len { total_len as i64 } else { 0 });
    let adjusted = if raw < 0 { total_len as i64 + raw } else { raw };
    adjusted.clamp(0, total_len as i64) as usize
}

fn const_slice_bound_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int { value: v, .. } => Some(*v),
        Expr::Number { value: v, .. } => {
            let truncated = v.trunc();
            if (v - truncated).abs() <= 1e-6 {
                Some(truncated as i64)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(crate) fn require_loop_control_context(
    keyword: &str,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    if loop_depth == 0 {
        errors.push(Diagnostic::semantic_span(
            format!("{keyword} is only allowed inside for/while/loop bodies"),
            None::<SourceLoc>,
        ));
    }
}
