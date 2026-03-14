use super::*;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ProcSlotBufferRefLayout {
    pub(super) slot_buffer_indices: Vec<usize>,
    pub(super) ptr_offset: usize,
    pub(super) frames_offset: usize,
    pub(super) channels_offset: usize,
    pub(super) samplerate_offset: usize,
}

fn collect_proc_slot_buffer_ref_signatures(
    typed: &TypedProgram,
    buffer_index: &HashMap<String, usize>,
) -> Vec<Vec<usize>> {
    fn visit_expr(
        expr: &Expr,
        buffer_index: &HashMap<String, usize>,
        out: &mut BTreeSet<Vec<usize>>,
    ) {
        if let Expr::UserCall { name, args, .. } = expr {
            if name == PROC_INDEX_BUFFER_SELECT_SENTINEL {
                let mut signature = Vec::<usize>::new();
                for arg in args {
                    match arg.name.as_deref() {
                        Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG) => {}
                        None => {
                            let Expr::Var { name: symbol, .. } = &arg.expr else {
                                signature.clear();
                                break;
                            };
                            let Some(buffer_idx) = buffer_index.get(symbol).copied() else {
                                signature.clear();
                                break;
                            };
                            signature.push(buffer_idx);
                        }
                        Some(_) => {
                            signature.clear();
                            break;
                        }
                    }
                }
                if !signature.is_empty() {
                    out.insert(signature);
                }
            }
        }

        match expr {
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                visit_expr(lhs, buffer_index, out);
                visit_expr(rhs, buffer_index, out);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    visit_expr(arg, buffer_index, out);
                }
            }
            Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } => {
                visit_expr(expr, buffer_index, out);
            }
            Expr::ArrayLiteral { values, .. } => {
                for value in values {
                    visit_expr(value, buffer_index, out);
                }
            }
            Expr::ArrayCtor { init, .. } => {
                if let Some(values) = init {
                    for value in values {
                        visit_expr(value, buffer_index, out);
                    }
                }
            }
            Expr::UserCall { args, .. } => {
                for arg in args {
                    visit_expr(&arg.expr, buffer_index, out);
                }
            }
            Expr::Index { index, .. } => {
                visit_expr(index, buffer_index, out);
            }
            _ => {}
        }
    }

    fn visit_stmt(
        stmt: &Stmt,
        buffer_index: &HashMap<String, usize>,
        out: &mut BTreeSet<Vec<usize>>,
    ) {
        match stmt {
            Stmt::Const { .. } => {}
            Stmt::Assign { target, expr, .. } => {
                if let AssignTarget::Index { index, .. } = target {
                    visit_expr(index, buffer_index, out);
                }
                visit_expr(expr, buffer_index, out);
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                visit_expr(expr, buffer_index, out);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                visit_expr(cond, buffer_index, out);
                for nested in then_branch {
                    visit_stmt(nested, buffer_index, out);
                }
                for nested in else_branch {
                    visit_stmt(nested, buffer_index, out);
                }
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                visit_expr(start, buffer_index, out);
                visit_expr(end, buffer_index, out);
                if let Some(step_expr) = step {
                    visit_expr(step_expr, buffer_index, out);
                }
                for nested in body {
                    visit_stmt(nested, buffer_index, out);
                }
            }
            Stmt::While { cond, body, .. } => {
                visit_expr(cond, buffer_index, out);
                for nested in body {
                    visit_stmt(nested, buffer_index, out);
                }
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }

    let mut signatures = BTreeSet::<Vec<usize>>::new();
    for stmt in &typed.init {
        visit_stmt(stmt, buffer_index, &mut signatures);
    }
    for stmt in &typed.block_pre {
        visit_stmt(stmt, buffer_index, &mut signatures);
    }
    for stmt in &typed.sample {
        visit_stmt(stmt, buffer_index, &mut signatures);
    }
    for stmt in &typed.block_post {
        visit_stmt(stmt, buffer_index, &mut signatures);
    }
    for def in &typed.defs {
        for stmt in &def.body {
            visit_stmt(stmt, buffer_index, &mut signatures);
        }
    }
    for event in &typed.events {
        for stmt in &event.body {
            visit_stmt(stmt, buffer_index, &mut signatures);
        }
    }
    signatures.into_iter().collect()
}

pub(super) fn compute_proc_slot_buffer_ref_layouts(
    typed: &TypedProgram,
    base_state_size_bytes: usize,
) -> Result<(Vec<ProcSlotBufferRefLayout>, usize), Diagnostic> {
    let buffer_index = typed
        .buffers
        .iter()
        .enumerate()
        .map(|(idx, decl)| (decl.name.clone(), idx))
        .collect::<HashMap<_, _>>();
    let signatures = collect_proc_slot_buffer_ref_signatures(typed, &buffer_index);

    let mut layouts = Vec::<ProcSlotBufferRefLayout>::with_capacity(signatures.len());
    let mut offset = base_state_size_bytes;
    for signature in signatures {
        let slots_len = signature.len();
        if slots_len == 0 {
            continue;
        }

        offset = align_up(offset, align_of::<usize>());
        let ptr_offset = offset;
        let ptr_bytes = slots_len.checked_mul(size_of::<usize>()).ok_or_else(|| {
            Diagnostic::internal("proc buffer-ref pointer array byte size overflow")
        })?;
        offset = offset
            .checked_add(ptr_bytes)
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref pointer layout overflow"))?;

        offset = align_up(offset, align_of::<i32>());
        let frames_offset = offset;
        let i32_bytes = slots_len
            .checked_mul(size_of::<i32>())
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref i32 array byte size overflow"))?;
        offset = offset
            .checked_add(i32_bytes)
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref frames layout overflow"))?;

        offset = align_up(offset, align_of::<i32>());
        let channels_offset = offset;
        offset = offset
            .checked_add(i32_bytes)
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref channels layout overflow"))?;

        offset = align_up(offset, align_of::<f32>());
        let samplerate_offset = offset;
        let f32_bytes = slots_len
            .checked_mul(size_of::<f32>())
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref f32 array byte size overflow"))?;
        offset = offset
            .checked_add(f32_bytes)
            .ok_or_else(|| Diagnostic::internal("proc buffer-ref samplerate layout overflow"))?;

        layouts.push(ProcSlotBufferRefLayout {
            slot_buffer_indices: signature,
            ptr_offset,
            frames_offset,
            channels_offset,
            samplerate_offset,
        });
    }

    Ok((layouts, offset))
}
