//! Normalizes surface assignment places into the core executable data model.

use crate::{executable_data::assign_var, *};

const INDEXED_PLACE_PREFIX: &str = "__onda_indexed_place_";

/// Lowers `array[index].field` targets through an element view. Every
/// executable scope then uses the same ordinary field, tuple, and array
/// assignment rules, while both selectors are evaluated exactly once in
/// source order before the right-hand side. Returns the compiler-only views
/// introduced in top-level initialization; they are valid only while init runs.
pub(crate) fn normalize_indexed_member_assignments(program: &mut Program) -> HashSet<String> {
    let mut next = 0usize;
    let mut transient_init_views = HashSet::new();
    for block in &mut program.blocks {
        match block {
            Block::Init(init) => {
                normalize_statements(&mut init.body, &mut next, Some(&mut transient_init_views))
            }
            Block::Block(exec) => {
                normalize_statements(&mut exec.pre, &mut next, None);
                if let Some(sample) = &mut exec.sample {
                    normalize_statements(&mut sample.body, &mut next, None);
                }
                normalize_statements(&mut exec.post, &mut next, None);
            }
            Block::Sample(sample) => normalize_statements(&mut sample.body, &mut next, None),
            Block::Def(def) => normalize_statements(&mut def.body, &mut next, None),
            Block::Events(events) => {
                for event in &mut events.events {
                    normalize_statements(&mut event.body, &mut next, None);
                }
            }
            Block::When(when) => normalize_statements(&mut when.body, &mut next, None),
            Block::Tasks(tasks) => {
                for task in &mut tasks.tasks {
                    normalize_statements(&mut task.body, &mut next, None);
                }
            }
            Block::Struct(def) => {
                for method in &mut def.methods {
                    normalize_statements(&mut method.body, &mut next, None);
                }
            }
            Block::Proc(proc) => {
                normalize_statements(&mut proc.init.body, &mut next, None);
                normalize_statements(&mut proc.block_pre, &mut next, None);
                normalize_statements(&mut proc.sample, &mut next, None);
                normalize_statements(&mut proc.block_post, &mut next, None);
                for event in &mut proc.events {
                    normalize_statements(&mut event.body, &mut next, None);
                }
                for when in &mut proc.whens {
                    normalize_statements(&mut when.body, &mut next, None);
                }
                for task in &mut proc.tasks {
                    normalize_statements(&mut task.body, &mut next, None);
                }
                for def in &mut proc.local_defs {
                    normalize_statements(&mut def.body, &mut next, None);
                }
            }
            Block::Ins(_)
            | Block::Outs(_)
            | Block::KOuts(_)
            | Block::Params(_)
            | Block::Buffers(_)
            | Block::Const(_)
            | Block::Assert(_)
            | Block::Delegates(_)
            | Block::Graph(_)
            | Block::Namespace(_)
            | Block::NamespaceAlias(_)
            | Block::Use(_) => {}
        }
    }
    transient_init_views
}

fn normalize_statements(
    statements: &mut Vec<Stmt>,
    next: &mut usize,
    mut transient_views: Option<&mut HashSet<String>>,
) {
    let mut normalized = Vec::with_capacity(statements.len());
    for mut statement in std::mem::take(statements) {
        match &mut statement {
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                normalize_statements(then_branch, next, transient_views.as_deref_mut());
                normalize_statements(else_branch, next, transient_views.as_deref_mut());
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                normalize_statements(body, next, transient_views.as_deref_mut())
            }
            _ => {}
        }

        let Stmt::Assign {
            target, target_loc, ..
        } = &mut statement
        else {
            normalized.push(statement);
            continue;
        };
        let AssignTarget::IndexedMember {
            base,
            index,
            field,
            field_index,
        } = target
        else {
            normalized.push(statement);
            continue;
        };

        let place_id = *next;
        let alias = format!("{INDEXED_PLACE_PREFIX}{place_id}");
        *next += 1;
        if let Some(transient_views) = transient_views.as_deref_mut() {
            transient_views.insert(alias.clone());
        }
        let selection = Expr::Index {
            loc: *target_loc,
            base: std::mem::take(base),
            index: Box::new(std::mem::replace(index, Expr::int(0))),
        };
        let field = std::mem::take(field);
        *target = match field_index.take() {
            Some(index) => AssignTarget::Index {
                base: format!("{alias}.{field}"),
                index: *index,
            },
            None => AssignTarget::Var(format!("{alias}.{field}")),
        };
        normalized.push(assign_var(alias, selection));
        normalized.push(statement);
    }
    *statements = normalized;
}
