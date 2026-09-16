//! Heap-backed expression traversal shared by compiler passes. Children are
//! visited in source order; neither walking nor folding uses the Rust stack.

use crate::Expr;

impl Expr {
    /// Append immediate children in source order, including type-size expressions.
    pub fn children<'a>(&'a self, children: &mut Vec<&'a Expr>) {
        match self {
            Self::Number { .. } | Self::Int { .. } | Self::Bool { .. } | Self::Var { .. } => {}
            Self::Index { index, .. }
            | Self::Cast { expr: index, .. }
            | Self::UnaryNot { expr: index, .. }
            | Self::UnaryBitNot { expr: index, .. } => children.push(index),
            Self::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => {
                children.extend(
                    [selector, channel, start, end]
                        .into_iter()
                        .filter_map(|v| v.as_deref()),
                );
            }
            Self::Binary { lhs, rhs, .. }
            | Self::Compare { lhs, rhs, .. }
            | Self::Logical { lhs, rhs, .. } => children.extend([lhs.as_ref(), rhs.as_ref()]),
            Self::ArrayLiteral { values, .. }
            | Self::Tuple { values, .. }
            | Self::Call { args: values, .. } => children.extend(values),
            Self::UserCall { args, .. } => children.extend(args.iter().map(|arg| &arg.expr)),
            Self::ArrayCtor { spec, init, .. } => {
                children.push(&spec.size);
                children.extend(init.iter().flatten());
            }
        }
    }

    /// Mutable immediate children, in the same order as [`Self::children`].
    pub fn children_mut<'a>(&'a mut self, children: &mut Vec<&'a mut Expr>) {
        match self {
            Self::Number { .. } | Self::Int { .. } | Self::Bool { .. } | Self::Var { .. } => {}
            Self::Index { index, .. }
            | Self::Cast { expr: index, .. }
            | Self::UnaryNot { expr: index, .. }
            | Self::UnaryBitNot { expr: index, .. } => children.push(index),
            Self::Slice {
                selector,
                channel,
                start,
                end,
                ..
            } => {
                children.extend(
                    [selector, channel, start, end]
                        .into_iter()
                        .filter_map(|v| v.as_deref_mut()),
                );
            }
            Self::Binary { lhs, rhs, .. }
            | Self::Compare { lhs, rhs, .. }
            | Self::Logical { lhs, rhs, .. } => children.extend([lhs.as_mut(), rhs.as_mut()]),
            Self::ArrayLiteral { values, .. }
            | Self::Tuple { values, .. }
            | Self::Call { args: values, .. } => children.extend(values),
            Self::UserCall { args, .. } => {
                children.extend(args.iter_mut().map(|arg| &mut arg.expr))
            }
            Self::ArrayCtor { spec, init, .. } => {
                children.push(&mut spec.size);
                children.extend(init.iter_mut().flatten());
            }
        }
    }

    pub fn walk(&self) -> impl Iterator<Item = &Expr> {
        let mut pending = vec![self];
        std::iter::from_fn(move || {
            let node = pending.pop()?;
            let start = pending.len();
            node.children(&mut pending);
            pending[start..].reverse();
            Some(node)
        })
    }

    /// Preorder inspection with pruning, matching [`Self::visit_mut`].
    pub fn visit(&self, mut visit: impl FnMut(&Expr) -> bool) {
        let mut pending = vec![self];
        while let Some(node) = pending.pop() {
            if visit(node) {
                let start = pending.len();
                node.children(&mut pending);
                pending[start..].reverse();
            }
        }
    }

    /// Preorder rewrite. Return false to skip a node's (possibly replaced) children.
    pub fn visit_mut(&mut self, mut visit: impl FnMut(&mut Expr) -> bool) {
        let mut pending = vec![self];
        while let Some(node) = pending.pop() {
            if visit(node) {
                let start = pending.len();
                node.children_mut(&mut pending);
                pending[start..].reverse();
            }
        }
    }

    /// Rewrite children before their parent. Owned frames act as a safe zipper:
    /// child slots temporarily contain a leaf while their values are on the
    /// worklist. No raw references survive a parent replacement.
    pub fn visit_mut_postorder(&mut self, mut visit: impl FnMut(&mut Expr)) {
        let mut pending = vec![(std::mem::replace(self, Expr::int(0)), None)];
        let mut values = Vec::new();
        while let Some((mut node, base)) = pending.pop() {
            if let Some(base) = base {
                let mut children = Vec::new();
                node.children_mut(&mut children);
                for (slot, child) in children.into_iter().zip(values.drain(base..)) {
                    *slot = child;
                }
                visit(&mut node);
                values.push(node);
            } else {
                let mut slots = Vec::new();
                node.children_mut(&mut slots);
                let children: Vec<_> = slots
                    .into_iter()
                    .map(|slot| (std::mem::replace(slot, Expr::int(0)), None))
                    .collect();
                pending.push((node, Some(values.len())));
                pending.extend(children.into_iter().rev());
            }
        }
        *self = values.pop().expect("expression rewrite produces a root");
    }

    /// Postorder fold with pass-specific descent. `descend` appends only the
    /// children needed by this pass; `finish` receives their results in order.
    /// Errors stop traversal before evaluating later siblings.
    pub fn try_fold<'a, T, E>(
        &'a self,
        mut descend: impl FnMut(&'a Expr, &mut Vec<&'a Expr>),
        mut finish: impl FnMut(&'a Expr, &mut std::vec::Drain<'_, T>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut pending = vec![(self, None)];
        let mut children = Vec::new();
        let mut values = Vec::new();
        while let Some((node, base)) = pending.pop() {
            if let Some(base) = base {
                let result = finish(node, &mut values.drain(base..))?;
                values.push(result);
            } else {
                pending.push((node, Some(values.len())));
                descend(node, &mut children);
                pending.extend(children.drain(..).rev().map(|child| (child, None)));
            }
        }
        Ok(values.pop().expect("expression fold produces a root value"))
    }
}

impl Clone for Expr {
    fn clone(&self) -> Self {
        self.try_fold(Self::children, |node, children| {
            let mut child = || children.next().expect("cloned expression child");
            Ok::<_, std::convert::Infallible>(match node {
                Self::Number { loc, value } => Self::Number {
                    loc: *loc,
                    value: *value,
                },
                Self::Int { loc, value } => Self::Int {
                    loc: *loc,
                    value: *value,
                },
                Self::Bool { loc, value } => Self::Bool {
                    loc: *loc,
                    value: *value,
                },
                Self::Var { loc, name } => Self::Var {
                    loc: *loc,
                    name: name.clone(),
                },
                Self::Index { loc, base, .. } => Self::Index {
                    loc: *loc,
                    base: base.clone(),
                    index: Box::new(child()),
                },
                Self::Slice {
                    loc,
                    base,
                    selector,
                    channel,
                    start,
                    end,
                } => Self::Slice {
                    loc: *loc,
                    base: base.clone(),
                    selector: selector.as_ref().map(|_| Box::new(child())),
                    channel: channel.as_ref().map(|_| Box::new(child())),
                    start: start.as_ref().map(|_| Box::new(child())),
                    end: end.as_ref().map(|_| Box::new(child())),
                },
                Self::Cast { loc, to, .. } => Self::Cast {
                    loc: *loc,
                    to: *to,
                    expr: Box::new(child()),
                },
                Self::UnaryNot { loc, .. } => Self::UnaryNot {
                    loc: *loc,
                    expr: Box::new(child()),
                },
                Self::UnaryBitNot { loc, .. } => Self::UnaryBitNot {
                    loc: *loc,
                    expr: Box::new(child()),
                },
                Self::Binary { loc, op, .. } => Self::Binary {
                    loc: *loc,
                    op: *op,
                    lhs: Box::new(child()),
                    rhs: Box::new(child()),
                },
                Self::Compare { loc, op, .. } => Self::Compare {
                    loc: *loc,
                    op: *op,
                    lhs: Box::new(child()),
                    rhs: Box::new(child()),
                },
                Self::Logical { loc, op, .. } => Self::Logical {
                    loc: *loc,
                    op: *op,
                    lhs: Box::new(child()),
                    rhs: Box::new(child()),
                },
                Self::Tuple { loc, .. } => Self::Tuple {
                    loc: *loc,
                    values: children.collect(),
                },
                Self::ArrayLiteral { loc, .. } => Self::ArrayLiteral {
                    loc: *loc,
                    values: children.collect(),
                },
                Self::Call { loc, func, .. } => Self::Call {
                    loc: *loc,
                    func: *func,
                    args: children.collect(),
                },
                Self::UserCall {
                    loc,
                    name,
                    type_args,
                    args,
                } => Self::UserCall {
                    loc: *loc,
                    name: name.clone(),
                    type_args: type_args.clone(),
                    args: args
                        .iter()
                        .map(|arg| crate::CallArg {
                            name: arg.name.clone(),
                            expr: child(),
                        })
                        .collect(),
                },
                Self::ArrayCtor {
                    loc,
                    spec,
                    init,
                    init_is_value,
                    initialize,
                } => Self::ArrayCtor {
                    loc: *loc,
                    spec: crate::ArrayTypeSpec {
                        elem: spec.elem.clone(),
                        size: Box::new(child()),
                    },
                    init: init.as_ref().map(|_| children.collect()),
                    init_is_value: *init_is_value,
                    initialize: *initialize,
                },
            })
        })
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrayElemType, ArrayTypeSpec, BinaryOp, CallArg, PrimitiveType, Span};

    #[test]
    fn deep_expression_clone_preserves_children_and_locations() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let mut tree = Expr::int(0);
                for value in 1..15_000 {
                    tree = Expr::Binary {
                        loc: Span::ZERO,
                        op: BinaryOp::Sub,
                        lhs: Box::new(tree),
                        rhs: Box::new(Expr::int(value)),
                    };
                }
                let cloned = tree.clone();
                let integers = |expr: &Expr| {
                    expr.walk()
                        .filter_map(|node| match node {
                            Expr::Int { value, .. } => Some(*value),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                };
                assert_eq!(integers(&cloned), integers(&tree));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn traversal_covers_slice_coordinates_constructor_size_and_named_arguments() {
        let mut tree = Expr::ArrayCtor {
            loc: Span::ZERO,
            spec: ArrayTypeSpec {
                elem: ArrayElemType::Primitive(PrimitiveType::F32),
                size: Box::new(Expr::int(1)),
            },
            initialize: true,
            init_is_value: false,
            init: Some(vec![
                Expr::Slice {
                    loc: Span::ZERO,
                    base: "data".into(),
                    selector: Some(Box::new(Expr::int(2))),
                    channel: Some(Box::new(Expr::int(3))),
                    start: Some(Box::new(Expr::int(4))),
                    end: Some(Box::new(Expr::int(5))),
                },
                Expr::UserCall {
                    loc: Span::ZERO,
                    name: "f".into(),
                    type_args: vec![],
                    args: vec![CallArg {
                        name: Some("x".into()),
                        expr: Expr::int(6),
                    }],
                },
            ]),
        };
        assert_eq!(format!("{tree:?}"), format!("{:?}", tree.clone()));
        let integers = |tree: &Expr| {
            tree.walk()
                .filter_map(|node| match node {
                    Expr::Int { value, .. } => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(integers(&tree), [1, 2, 3, 4, 5, 6]);
        tree.visit_mut_postorder(|node| {
            if let Expr::Int { value, .. } = node {
                *value += 10;
            }
        });
        assert_eq!(integers(&tree), [11, 12, 13, 14, 15, 16]);
        tree.visit_mut(|node| {
            if let Expr::Int { value, .. } = node {
                *value -= 10;
            }
            true
        });
        assert_eq!(integers(&tree), [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn fold_preserves_noncommutative_order_and_stops_on_error() {
        let tree = Expr::Binary {
            loc: Span::ZERO,
            op: BinaryOp::Sub,
            lhs: Box::new(Expr::int(8)),
            rhs: Box::new(Expr::int(3)),
        };
        let value = tree.try_fold(Expr::children, |node, children| {
            Ok::<_, ()>(match node {
                Expr::Int { value, .. } => *value,
                Expr::Binary { .. } => children.next().unwrap() - children.next().unwrap(),
                _ => unreachable!(),
            })
        });
        assert_eq!(value, Ok(5));
        let mut visited = Vec::new();
        let result = tree.try_fold(
            Expr::children,
            |node, _children: &mut std::vec::Drain<'_, ()>| {
                visited.push(node.loc());
                Err::<(), _>("stop")
            },
        );
        assert_eq!(result, Err("stop"));
        assert_eq!(visited.len(), 1);
    }
}
