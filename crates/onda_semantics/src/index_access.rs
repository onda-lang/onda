use onda_frontend::{CallArg, Expr, Span, READ_UNSAFE_FN};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IndexAccess {
    Clamp,
    Unchecked,
}

#[derive(Clone, Copy)]
pub(crate) struct IndexedReadSource<'a> {
    pub(crate) base: &'a str,
    pub(crate) index: &'a Expr,
    pub(crate) access: IndexAccess,
}

/// Recognizes expressions that select one element from an aggregate array.
///
/// Primitive reads are lowered as values elsewhere. Aggregate reads use this
/// representation to preserve reference/alias semantics while varying only
/// whether the selector is clamped.
pub(crate) fn indexed_read_source(expr: &Expr) -> Option<IndexedReadSource<'_>> {
    match expr {
        Expr::Index { base, index, .. } => Some(IndexedReadSource {
            base,
            index,
            access: IndexAccess::Clamp,
        }),
        Expr::UserCall { name, args, .. }
            if name == READ_UNSAFE_FN
                && args.len() == 2
                && args.iter().all(|argument| argument.name.is_none()) =>
        {
            let Expr::Var { name: base, .. } = &args[0].expr else {
                return None;
            };
            Some(IndexedReadSource {
                base,
                index: &args[1].expr,
                access: IndexAccess::Unchecked,
            })
        }
        _ => None,
    }
}

pub(crate) fn indexed_read_expr(
    base: impl Into<String>,
    index: Expr,
    access: IndexAccess,
    loc: Span,
) -> Expr {
    let base = base.into();
    match access {
        IndexAccess::Clamp => Expr::Index {
            loc,
            base,
            index: Box::new(index),
        },
        IndexAccess::Unchecked => Expr::UserCall {
            loc,
            name: READ_UNSAFE_FN.to_owned(),
            type_args: Vec::new(),
            args: vec![
                CallArg {
                    name: None,
                    expr: Expr::var(base),
                },
                CallArg {
                    name: None,
                    expr: index,
                },
            ],
        },
    }
}
