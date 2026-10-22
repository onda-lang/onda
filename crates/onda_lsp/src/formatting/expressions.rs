//! Iterative expression formatting: one output buffer, no depth-dependent
//! stack use or repeated copies of already formatted subexpressions.
use super::*;
use std::borrow::Cow;
use std::fmt::Write;

enum Item<'a> {
    Expr(&'a Expr, u8),
    Text(Cow<'a, str>),
}

fn text(value: &str) -> Item<'_> {
    Item::Text(Cow::Borrowed(value))
}

fn expressions<'a>(pending: &mut Vec<Item<'a>>, values: &'a [Expr]) {
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            pending.push(text(", "));
        }
        pending.push(Item::Expr(value, 0));
    }
}

pub(super) fn format(expr: &Expr) -> String {
    let mut out = String::new();
    let mut pending = vec![Item::Expr(expr, 0)];
    while let Some(item) = pending.pop() {
        let (expr, parent_prec) = match item {
            Item::Text(value) => {
                out.push_str(&value);
                continue;
            }
            Item::Expr(expr, prec) => (expr, prec),
        };
        let prec = expr_precedence(expr);
        let start = pending.len();
        let wrapped = prec < parent_prec;
        if wrapped {
            pending.push(text("("));
        }
        match expr {
            Expr::Number { value, .. } => out.push_str(&format_number(*value)),
            Expr::Int { value, .. } => {
                write!(out, "{value}").unwrap();
            }
            Expr::Bool { value, .. } => {
                write!(out, "{value}").unwrap();
            }
            Expr::Var { name, .. } => out.push_str(name),
            Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
                let array = matches!(expr, Expr::ArrayLiteral { .. });
                pending.push(text(if array { "[" } else { "(" }));
                expressions(&mut pending, values);
                pending.push(text(if array { "]" } else { ")" }));
            }
            Expr::Index { base, index, .. } => {
                pending.extend([text(base), text("["), Item::Expr(index, 0), text("]")]);
            }
            Expr::Slice {
                base,
                selector,
                channel,
                start,
                end,
                ..
            } => {
                pending.push(text(base));
                if let Some(selector) = selector {
                    pending.extend([text("["), Item::Expr(selector, 0), text("]")]);
                }
                pending.push(text("["));
                if let Some(channel) = channel {
                    pending.extend([Item::Expr(channel, 0), text(", ")]);
                }
                if let Some(start) = start {
                    pending.push(Item::Expr(start, 0));
                }
                pending.push(text(":"));
                if let Some(end) = end {
                    pending.push(Item::Expr(end, 0));
                }
                pending.push(text("]"));
            }
            Expr::ArrayCtor { spec, init, .. } => {
                let elem = match &spec.elem {
                    ArrayElemType::Primitive(ty) => primitive_type_name(*ty),
                    ArrayElemType::Struct(name) => name,
                };
                pending.extend([text(elem), text("["), Item::Expr(&spec.size, 0), text("](")]);
                if let Some(init) = init {
                    expressions(&mut pending, init);
                }
                pending.push(text(")"));
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                let op = match expr {
                    Expr::Compare { op, .. } => format_cmp_op(*op),
                    Expr::Logical { op, .. } => format_logical_op(*op),
                    Expr::Binary { op, .. } => format_binary_op(*op),
                    _ => unreachable!(),
                };
                pending.extend([
                    Item::Expr(lhs, prec),
                    text(" "),
                    text(op),
                    text(" "),
                    Item::Expr(rhs, prec + 1),
                ]);
            }
            Expr::UnaryNot { expr: operand, .. } | Expr::UnaryBitNot { expr: operand, .. } => {
                let op = if matches!(expr, Expr::UnaryNot { .. }) {
                    "!"
                } else {
                    "~"
                };
                pending.extend([text(op), Item::Expr(operand, prec)]);
            }
            Expr::Cast { to, expr, .. } => {
                pending.extend([
                    text(primitive_type_name(*to)),
                    text("("),
                    Item::Expr(expr, 0),
                    text(")"),
                ]);
            }
            Expr::Call { func, args, .. } => {
                pending.extend([text(format_builtin_fn(*func)), text("(")]);
                expressions(&mut pending, args);
                pending.push(text(")"));
            }
            Expr::UserCall {
                name,
                type_args,
                args,
                ..
            } => {
                if !buffer_access(&mut pending, name, args) {
                    pending.push(text(name));
                    if !type_args.is_empty() {
                        pending.push(text("<"));
                        for (index, arg) in type_args.iter().enumerate() {
                            if index != 0 {
                                pending.push(text(", "));
                            }
                            pending.push(Item::Text(Cow::Owned(format_call_type_arg(arg))));
                        }
                        pending.push(text(">"));
                    }
                    pending.push(text("("));
                    for (index, arg) in args.iter().enumerate() {
                        if index != 0 {
                            pending.push(text(", "));
                        }
                        if let Some(name) = &arg.name {
                            pending.extend([text(name), text(" = ")]);
                        }
                        pending.push(Item::Expr(&arg.expr, 0));
                    }
                    pending.push(text(")"));
                }
            }
        }
        if wrapped {
            pending.push(text(")"));
        }
        pending[start..].reverse();
    }
    out
}

fn buffer_access<'a>(pending: &mut Vec<Item<'a>>, name: &str, args: &'a [CallArg]) -> bool {
    let Some(CallArg {
        expr: Expr::Var { name: base, .. },
        ..
    }) = args.first()
    else {
        return false;
    };
    let (separators, write): (&[&str], bool) = match name {
        INTERNAL_BUFFER_READ2_FN => (&["]["], false),
        INTERNAL_BUFFER_WRITE2_FN => (&["]["], true),
        INTERNAL_BUFFER_READ_CHANNEL_FN => (&[", "], false),
        INTERNAL_BUFFER_WRITE_CHANNEL_FN => (&[", "], true),
        INTERNAL_BUFFER_READ3_FN => (&["][", ", "], false),
        INTERNAL_BUFFER_WRITE3_FN => (&["][", ", "], true),
        _ => return false,
    };
    if args.len() != separators.len() + 2 + usize::from(write) {
        return false;
    }
    pending.extend([text(base), text("[")]);
    for (index, arg) in args[1..1 + separators.len() + 1].iter().enumerate() {
        if index != 0 {
            pending.push(text(separators[index - 1]));
        }
        pending.push(Item::Expr(&arg.expr, 0));
    }
    pending.push(text("]"));
    if write {
        pending.extend([text(" = "), Item::Expr(&args.last().unwrap().expr, 0)]);
    }
    true
}
