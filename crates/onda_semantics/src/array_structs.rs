use std::collections::{HashMap, HashSet};

use onda_frontend::ast::{BinaryOp, CallArg, Expr, Span, Stmt};
use onda_frontend::{DiagCtx, Diagnostic, PrimitiveType};

use crate::decl_symbols::{insert_declared_symbol, DeclaredSymbolInfo, DeclaredSymbolMap};
use crate::proc_state_rewrite::{
    PROC_FIELD_SENTINEL_ARG, PROC_FIELD_SENTINEL_PREFIX, PROC_INDEX_BASE_ARG,
    PROC_INDEX_CALL_SENTINEL, PROC_INDEX_EXPR_ARG, SAFI_BASE_ARG, SAFI_FIELD_ARG,
    SAFI_FIELD_IDX_ARG, SAFI_IDX_ARG, STRUCT_ARRAY_FIELD_INDEX_SENTINEL,
};
use crate::{
    push_semantic, ArrayStructRootInfo, LocalAliasTypes, LocalArrayAliasInfo, TypedFieldType,
    TypedStructField,
};

#[derive(Debug, Clone)]
enum StructArrayLayoutKind {
    Scalar(PrimitiveType),
    Array {
        len: usize,
        elem_ty: Option<PrimitiveType>,
        elem_struct: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct StructArrayLayoutField {
    name: String,
    kind: StructArrayLayoutKind,
}

pub(crate) fn validate_data_struct_layout(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    collect_data_struct_layout(struct_name, struct_defs, context, errors).is_some()
}

fn collect_data_struct_layout(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<Vec<StructArrayLayoutField>> {
    let mut stack = Vec::<String>::new();
    collect_data_struct_layout_inner(struct_name, struct_defs, context, errors, &mut stack)
}

fn collect_data_struct_layout_inner(
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
    stack: &mut Vec<String>,
) -> Option<Vec<StructArrayLayoutField>> {
    if stack.iter().any(|s| s == struct_name) {
        let mut cycle = stack.join(" -> ");
        if !cycle.is_empty() {
            cycle.push_str(" -> ");
        }
        cycle.push_str(struct_name);
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context} contains recursive array[Struct, N] cycle: {cycle}"),
        );
        return None;
    }
    let fields = struct_defs.get(struct_name).cloned();
    let Some(fields) = fields else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context} references unknown struct '{struct_name}'"),
        );
        return None;
    };

    stack.push(struct_name.to_owned());
    let mut layout = Vec::new();
    for field in fields {
        match field.ty {
            TypedFieldType::Scalar(prim) => layout.push(StructArrayLayoutField {
                name: field.name,
                kind: StructArrayLayoutKind::Scalar(prim),
            }),
            TypedFieldType::Struct => {}
            TypedFieldType::Array(len) => {
                if let Some(elem_struct) = &field.array_elem_struct {
                    let nested_context = format!(
                        "{context} nested array field '{}.{}'",
                        struct_name, field.name
                    );
                    if collect_data_struct_layout_inner(
                        elem_struct,
                        struct_defs,
                        &nested_context,
                        errors,
                        stack,
                    )
                    .is_none()
                    {
                        stack.pop();
                        return None;
                    }
                }
                layout.push(StructArrayLayoutField {
                    name: field.name,
                    kind: StructArrayLayoutKind::Array {
                        len,
                        elem_ty: field.array_elem_ty,
                        elem_struct: field.array_elem_struct.clone(),
                    },
                });
            }
            TypedFieldType::Tuple(ref elem_tys) => {
                for (idx, prim) in elem_tys.iter().enumerate() {
                    layout.push(StructArrayLayoutField {
                        name: format!("{}.__{idx}", field.name),
                        kind: StructArrayLayoutKind::Scalar(*prim),
                    });
                }
            }
        }
    }
    stack.pop();
    Some(layout)
}

pub(crate) fn register_data_struct_root(
    base: &str,
    struct_name: &str,
    len: usize,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &mut DeclaredSymbolMap,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if !validate_data_struct_layout(struct_name, struct_defs, context, errors) {
        return false;
    }
    let mut stack = Vec::<String>::new();
    register_data_struct_root_inner(
        base,
        struct_name,
        len,
        struct_defs,
        context,
        state_scalars,
        declared_symbols,
        state_arrays,
        state_array_struct_roots,
        errors,
        &mut stack,
    )
}

#[allow(clippy::too_many_arguments)]
fn register_data_struct_root_inner(
    base: &str,
    struct_name: &str,
    len: usize,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    context: &str,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &mut DeclaredSymbolMap,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    errors: &mut Vec<Diagnostic>,
    stack: &mut Vec<String>,
) -> bool {
    if stack.iter().any(|s| s == struct_name) {
        let mut cycle = stack.join(" -> ");
        if !cycle.is_empty() {
            cycle.push_str(" -> ");
        }
        cycle.push_str(struct_name);
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context} contains recursive array[Struct, N] cycle: {cycle}"),
        );
        return false;
    }
    let Some(fields) = struct_defs.get(struct_name).cloned() else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("{context} references unknown struct '{struct_name}'"),
        );
        return false;
    };
    state_array_struct_roots
        .entry(base.to_owned())
        .or_insert(ArrayStructRootInfo {
            struct_name: struct_name.to_owned(),
            len,
        });
    // Mark the root symbol so index validation can recognize `base[idx]` even
    // when the struct has no scalar/array fields to contribute `base.*` keys.
    insert_declared_symbol(
        state_scalars,
        declared_symbols,
        base,
        DeclaredSymbolInfo::DataArray {
            elem_ty: PrimitiveType::F32,
        },
    );

    stack.push(struct_name.to_owned());
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                insert_declared_symbol(
                    state_scalars,
                    declared_symbols,
                    flat.clone(),
                    DeclaredSymbolInfo::DataArray { elem_ty: prim },
                );
                state_arrays.entry(flat).or_insert(len);
            }
            TypedFieldType::Struct => {}
            TypedFieldType::Tuple(ref elem_tys) => {
                for (idx, prim) in elem_tys.iter().enumerate() {
                    let elem_flat = format!("{flat}.__{idx}");
                    insert_declared_symbol(
                        state_scalars,
                        declared_symbols,
                        elem_flat.clone(),
                        DeclaredSymbolInfo::DataArray { elem_ty: *prim },
                    );
                    state_arrays.entry(elem_flat).or_insert(len);
                }
            }
            TypedFieldType::Array(field_len) => {
                let Some(nested_len) = len.checked_mul(field_len) else {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "{context} field '{}.{}' flattened length exceeds addressable size",
                            struct_name, field.name
                        ),
                    );
                    stack.pop();
                    return false;
                };
                if let Some(elem_struct) = &field.array_elem_struct {
                    let nested_context = format!(
                        "{context} nested array field '{}.{}'",
                        struct_name, field.name
                    );
                    if !register_data_struct_root_inner(
                        &flat,
                        elem_struct,
                        nested_len,
                        struct_defs,
                        &nested_context,
                        state_scalars,
                        declared_symbols,
                        state_arrays,
                        state_array_struct_roots,
                        errors,
                        stack,
                    ) {
                        stack.pop();
                        return false;
                    }
                } else {
                    let elem_ty = field.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    insert_declared_symbol(
                        state_scalars,
                        declared_symbols,
                        flat.clone(),
                        DeclaredSymbolInfo::DataArray { elem_ty },
                    );
                    state_arrays.entry(flat).or_insert(nested_len);
                }
            }
        }
    }
    stack.pop();
    true
}

pub(crate) fn add_struct_element_alias_bindings(
    alias_name: &str,
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some(layout) = collect_data_struct_layout(struct_name, struct_defs, context, errors) else {
        return false;
    };
    for field in layout {
        match field.kind {
            StructArrayLayoutKind::Scalar(prim) => {
                let alias = format!("{alias_name}.{}", field.name);
                local_aliases.insert(alias.clone(), prim);
                known_scalars.insert(alias);
            }
            StructArrayLayoutKind::Array {
                len,
                elem_ty,
                elem_struct,
            } => {
                local_array_aliases.insert(
                    format!("{alias_name}.{}", field.name),
                    LocalArrayAliasInfo {
                        len,
                        elem_ty: elem_ty.unwrap_or(PrimitiveType::F32),
                        elem_struct,
                        writable: true,
                    },
                );
            }
        }
    }
    true
}

pub(crate) fn register_struct_array_param_bindings(
    base: &str,
    struct_name: &str,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    declared_symbols: &mut DeclaredSymbolMap,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    struct_array_roots: &mut HashMap<String, ArrayStructRootInfo>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if !validate_data_struct_layout(
        struct_name,
        struct_defs,
        &format!("function parameter '{base}'"),
        errors,
    ) {
        return false;
    }
    let mut unused_scalars = HashMap::<String, PrimitiveType>::new();
    let mut stack = Vec::<String>::new();
    register_struct_array_param_bindings_inner(
        base,
        struct_name,
        1,
        struct_defs,
        declared_symbols,
        local_array_aliases,
        struct_array_roots,
        &mut unused_scalars,
        errors,
        &mut stack,
    )
}

#[allow(clippy::too_many_arguments)]
fn register_struct_array_param_bindings_inner(
    base: &str,
    struct_name: &str,
    len_factor: usize,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    declared_symbols: &mut DeclaredSymbolMap,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    struct_array_roots: &mut HashMap<String, ArrayStructRootInfo>,
    unused_scalars: &mut HashMap<String, PrimitiveType>,
    errors: &mut Vec<Diagnostic>,
    stack: &mut Vec<String>,
) -> bool {
    if stack.iter().any(|s| s == struct_name) {
        let mut cycle = stack.join(" -> ");
        if !cycle.is_empty() {
            cycle.push_str(" -> ");
        }
        cycle.push_str(struct_name);
        push_semantic(
            DiagCtx::default(),
            errors,
            format!(
                "function parameter '{base}' contains recursive array[Struct, N] cycle: {cycle}"
            ),
        );
        return false;
    }
    let Some(fields) = struct_defs.get(struct_name).cloned() else {
        push_semantic(
            DiagCtx::default(),
            errors,
            format!("function parameter '{base}' references unknown struct '{struct_name}'"),
        );
        return false;
    };

    struct_array_roots
        .entry(base.to_owned())
        .or_insert(ArrayStructRootInfo {
            struct_name: struct_name.to_owned(),
            len: len_factor.max(1),
        });
    local_array_aliases
        .entry(base.to_owned())
        .or_insert(LocalArrayAliasInfo {
            len: len_factor.max(1),
            elem_ty: PrimitiveType::F32,
            elem_struct: Some(struct_name.to_owned()),
            writable: true,
        });
    insert_declared_symbol(
        unused_scalars,
        declared_symbols,
        base,
        DeclaredSymbolInfo::DataArray {
            elem_ty: PrimitiveType::F32,
        },
    );

    stack.push(struct_name.to_owned());
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                local_array_aliases
                    .entry(flat.clone())
                    .or_insert(LocalArrayAliasInfo {
                        len: len_factor.max(1),
                        elem_ty: prim,
                        elem_struct: None,
                        writable: true,
                    });
                insert_declared_symbol(
                    unused_scalars,
                    declared_symbols,
                    flat,
                    DeclaredSymbolInfo::DataArray { elem_ty: prim },
                );
            }
            TypedFieldType::Struct => {}
            TypedFieldType::Tuple(ref elem_tys) => {
                for (idx, prim) in elem_tys.iter().enumerate() {
                    let elem_flat = format!("{flat}.__{idx}");
                    local_array_aliases
                        .entry(elem_flat.clone())
                        .or_insert(LocalArrayAliasInfo {
                            len: len_factor.max(1),
                            elem_ty: *prim,
                            elem_struct: None,
                            writable: true,
                        });
                    insert_declared_symbol(
                        unused_scalars,
                        declared_symbols,
                        elem_flat,
                        DeclaredSymbolInfo::DataArray { elem_ty: *prim },
                    );
                }
            }
            TypedFieldType::Array(field_len) => {
                let Some(nested_factor) = len_factor.checked_mul(field_len) else {
                    push_semantic(
                        DiagCtx::default(),
                        errors,
                        format!(
                            "function parameter '{base}' field '{}.{}' flattened length exceeds addressable size",
                            struct_name, field.name
                        ),
                    );
                    stack.pop();
                    return false;
                };
                let nested_factor = nested_factor.max(1);
                if let Some(elem_struct) = &field.array_elem_struct {
                    if !register_struct_array_param_bindings_inner(
                        &flat,
                        elem_struct,
                        nested_factor,
                        struct_defs,
                        declared_symbols,
                        local_array_aliases,
                        struct_array_roots,
                        unused_scalars,
                        errors,
                        stack,
                    ) {
                        stack.pop();
                        return false;
                    }
                } else {
                    let elem_ty = field.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    local_array_aliases
                        .entry(flat.clone())
                        .or_insert(LocalArrayAliasInfo {
                            len: nested_factor,
                            elem_ty,
                            elem_struct: None,
                            writable: true,
                        });
                    insert_declared_symbol(
                        unused_scalars,
                        declared_symbols,
                        flat,
                        DeclaredSymbolInfo::DataArray { elem_ty },
                    );
                }
            }
        }
    }
    stack.pop();
    true
}

// ---------------------------------------------------------------------------
// Rewrite struct-array inline field sentinels to flattened Index exprs
// ---------------------------------------------------------------------------

/// Rewrite struct-array inline field sentinels in a list of statements.
///
/// Supported forms:
/// - `base[idx].field` -> `Index { base: "base.field", index: idx }` for scalar fields
/// - `base[idx].field[fidx]` -> `Index { base: "base.field", index: idx * stride + fidx }`
///
/// Only real struct-array roots are rewritten here. Proc-array sentinels are left intact for the
/// proc dispatch rewrite path.
pub(crate) fn rewrite_struct_array_inline_field_stmts(
    stmts: &mut [Stmt],
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    for stmt in stmts.iter_mut() {
        rewrite_struct_array_inline_field_stmt(stmt, state_array_struct_roots, struct_defs, errors);
    }
}

fn rewrite_struct_array_inline_field_stmt(
    stmt: &mut Stmt,
    roots: &HashMap<String, ArrayStructRootInfo>,
    defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { expr, .. } | Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_struct_array_inline_field_expr(expr, roots, defs, errors);
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_struct_array_inline_field_expr(cond, roots, defs, errors);
            rewrite_struct_array_inline_field_stmts(then_branch, roots, defs, errors);
            rewrite_struct_array_inline_field_stmts(else_branch, roots, defs, errors);
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_struct_array_inline_field_expr(start, roots, defs, errors);
            rewrite_struct_array_inline_field_expr(end, roots, defs, errors);
            if let Some(s) = step {
                rewrite_struct_array_inline_field_expr(s, roots, defs, errors);
            }
            rewrite_struct_array_inline_field_stmts(body, roots, defs, errors);
        }
        Stmt::While { cond, body, .. } => {
            rewrite_struct_array_inline_field_expr(cond, roots, defs, errors);
            rewrite_struct_array_inline_field_stmts(body, roots, defs, errors);
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(crate) fn rewrite_struct_array_inline_field_expr(
    expr: &mut Expr,
    roots: &HashMap<String, ArrayStructRootInfo>,
    defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::UserCall {
            name, args, loc, ..
        } if name
            .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
            .is_some_and(|raw| raw == PROC_INDEX_CALL_SENTINEL) =>
        {
            let loc = *loc;
            let Some((base, idx, field)) = extract_proc_index_field_args(args) else {
                for arg in args.iter_mut() {
                    rewrite_struct_array_inline_field_expr(&mut arg.expr, roots, defs, errors);
                }
                return;
            };

            let Some(root_info) = roots.get(&base) else {
                for arg in args.iter_mut() {
                    rewrite_struct_array_inline_field_expr(&mut arg.expr, roots, defs, errors);
                }
                return;
            };
            let Some(fields) = defs.get(&root_info.struct_name) else {
                // Proc arrays also share the state_array_struct_roots map but are handled later by
                // proc dispatch rewriting, not by struct-array flattening.
                for arg in args.iter_mut() {
                    rewrite_struct_array_inline_field_expr(&mut arg.expr, roots, defs, errors);
                }
                return;
            };

            let Some(target_field) = fields.iter().find(|f| f.name == field) else {
                errors.push(Diagnostic::semantic_span(
                    format!("struct '{}' has no field '{field}'", root_info.struct_name),
                    loc,
                ));
                return;
            };

            match target_field.ty {
                TypedFieldType::Scalar(_) => {
                    *expr = Expr::Index {
                        loc,
                        base: format!("{base}.{field}"),
                        index: Box::new(idx),
                    };
                }
                TypedFieldType::Array(_) => {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "field '{field}' of struct '{}' is an array and must be indexed explicitly; use {base}[...].{field}[...]",
                            root_info.struct_name
                        ),
                        loc,
                    ));
                }
                TypedFieldType::Tuple(_) => {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "field '{field}' of struct '{}' is a tuple and must be indexed explicitly",
                            root_info.struct_name
                        ),
                        loc,
                    ));
                }
                TypedFieldType::Struct => {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "field '{field}' of struct '{}' is a nested struct and cannot be accessed inline with {base}[...].{field}; use an intermediate alias",
                            root_info.struct_name
                        ),
                        loc,
                    ));
                }
            }
        }
        Expr::UserCall { name, .. } if name == STRUCT_ARRAY_FIELD_INDEX_SENTINEL => {
            let loc = match expr {
                Expr::UserCall { loc, .. } => *loc,
                _ => Span::ZERO,
            };
            let Expr::UserCall { args, .. } = expr else {
                return;
            };
            let (base, idx, field, fidx) = match extract_safi_args(args) {
                Some(v) => v,
                None => {
                    errors.push(Diagnostic::semantic_span(
                        "malformed struct array field index expression",
                        loc,
                    ));
                    return;
                }
            };

            let Some(root_info) = roots.get(&base) else {
                errors.push(Diagnostic::semantic_span(
                    format!("'{base}' is not a struct array; cannot use {base}[...].{field}[...]"),
                    loc,
                ));
                return;
            };

            let Some(fields) = defs.get(&root_info.struct_name) else {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "struct definition '{}' not found for struct array '{base}'",
                        root_info.struct_name
                    ),
                    loc,
                ));
                return;
            };

            let Some(target_field) = fields.iter().find(|f| f.name == field) else {
                errors.push(Diagnostic::semantic_span(
                    format!("struct '{}' has no field '{field}'", root_info.struct_name),
                    loc,
                ));
                return;
            };

            let stride = match target_field.ty {
                TypedFieldType::Array(len) => len,
                TypedFieldType::Scalar(_) => 1,
                TypedFieldType::Tuple(ref elems) => elems.len(),
                TypedFieldType::Struct => {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "field '{field}' of struct '{}' is a nested struct and cannot be indexed with {base}[...].{field}[...]",
                            root_info.struct_name
                        ),
                        loc,
                    ));
                    return;
                }
            };

            let flat_base = format!("{base}.{field}");
            // Compute flattened index: idx * stride + fidx
            let flat_index = if stride == 1 {
                // For scalar fields: just use idx (fidx should be 0 or the only index)
                Expr::Binary {
                    loc: Span::ZERO,
                    op: BinaryOp::Add,
                    lhs: Box::new(idx),
                    rhs: Box::new(fidx),
                }
            } else {
                Expr::Binary {
                    loc: Span::ZERO,
                    op: BinaryOp::Add,
                    lhs: Box::new(Expr::Binary {
                        loc: Span::ZERO,
                        op: BinaryOp::Mul,
                        lhs: Box::new(idx),
                        rhs: Box::new(Expr::int(stride as i64)),
                    }),
                    rhs: Box::new(fidx),
                }
            };

            *expr = Expr::Index {
                loc,
                base: flat_base,
                index: Box::new(flat_index),
            };
        }
        // Recurse into sub-expressions
        Expr::Binary { lhs, rhs, .. }
        | Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. } => {
            rewrite_struct_array_inline_field_expr(lhs, roots, defs, errors);
            rewrite_struct_array_inline_field_expr(rhs, roots, defs, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_struct_array_inline_field_expr(arg, roots, defs, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_struct_array_inline_field_expr(&mut arg.expr, roots, defs, errors);
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            rewrite_struct_array_inline_field_expr(inner, roots, defs, errors);
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for v in values {
                rewrite_struct_array_inline_field_expr(v, roots, defs, errors);
            }
        }
        Expr::Index { index, .. } => {
            rewrite_struct_array_inline_field_expr(index, roots, defs, errors);
        }
        Expr::Slice { start, end, .. } => {
            if let Some(s) = start {
                rewrite_struct_array_inline_field_expr(s, roots, defs, errors);
            }
            if let Some(e) = end {
                rewrite_struct_array_inline_field_expr(e, roots, defs, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_struct_array_inline_field_expr(&mut spec.size, roots, defs, errors);
            if let Some(values) = init {
                for v in values {
                    rewrite_struct_array_inline_field_expr(v, roots, defs, errors);
                }
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

fn extract_safi_args(args: &mut [CallArg]) -> Option<(String, Expr, String, Expr)> {
    let mut base = None::<String>;
    let mut idx = None::<Expr>;
    let mut field = None::<String>;
    let mut fidx = None::<Expr>;
    for arg in args.iter() {
        match arg.name.as_deref() {
            Some(SAFI_BASE_ARG) => {
                if let Expr::Var { name, .. } = &arg.expr {
                    base = Some(name.clone());
                }
            }
            Some(SAFI_IDX_ARG) => idx = Some(arg.expr.clone()),
            Some(SAFI_FIELD_ARG) => {
                if let Expr::Var { name, .. } = &arg.expr {
                    field = Some(name.clone());
                }
            }
            Some(SAFI_FIELD_IDX_ARG) => fidx = Some(arg.expr.clone()),
            _ => {}
        }
    }
    Some((base?, idx?, field?, fidx?))
}

fn extract_proc_index_field_args(args: &[CallArg]) -> Option<(String, Expr, String)> {
    let mut base = None::<String>;
    let mut idx = None::<Expr>;
    let mut field = None::<String>;
    for arg in args {
        match arg.name.as_deref() {
            Some(PROC_INDEX_BASE_ARG) => {
                if let Expr::Var { name, .. } = &arg.expr {
                    base = Some(name.clone());
                }
            }
            Some(PROC_INDEX_EXPR_ARG) => idx = Some(arg.expr.clone()),
            Some(PROC_FIELD_SENTINEL_ARG) => {
                if let Expr::Var { name, .. } = &arg.expr {
                    field = Some(name.clone());
                }
            }
            _ => {}
        }
    }
    Some((base?, idx?, field?))
}
