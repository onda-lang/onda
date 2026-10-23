//! Difference constraints over nonwrapping affine expressions. Arithmetic is
//! expanded only when the current path proves that every intermediate fits its
//! signed type. Otherwise its result remains an opaque value identity.
use std::collections::BTreeMap;

use super::*;

#[derive(Clone, Default)]
struct Expression {
    constant: i128,
    terms: BTreeMap<Symbol, i128>,
}

impl Expression {
    fn add_term(&mut self, symbol: Symbol, coefficient: i128) -> Option<()> {
        let value = self.terms.entry(symbol).or_default();
        *value = value.checked_add(coefficient)?;
        if *value == 0 {
            self.terms.remove(&symbol);
        }
        Some(())
    }

    fn subtract(mut self, rhs: &Self, strict: bool) -> Option<Self> {
        self.constant = self
            .constant
            .checked_sub(rhs.constant)?
            .checked_add(i128::from(strict))?;
        for (&symbol, &coefficient) in &rhs.terms {
            self.add_term(symbol, coefficient.checked_neg()?)?;
        }
        Some(self)
    }

    /// `left - right <= bound`, with `zero` standing for the constant zero.
    fn difference(&self, zero: Symbol) -> Option<(Symbol, Symbol, i128)> {
        let mut positive = None;
        let mut negative = None;
        for (&symbol, &coefficient) in &self.terms {
            let slot = if coefficient > 0 {
                &mut positive
            } else {
                &mut negative
            };
            if slot.is_some() {
                return None;
            }
            *slot = Some((symbol, coefficient.checked_abs()?));
        }
        let (left, scale) = positive.unwrap_or((zero, negative.map_or(1, |v| v.1)));
        let (right, other_scale) = negative.unwrap_or((zero, scale));
        if scale != other_scale {
            return None;
        }
        Some((left, right, self.constant.checked_neg()?.div_euclid(scale)))
    }
}

struct Expressions<'a, 'b, 'p> {
    proof: &'a Proof<'b, 'p>,
    bounds: Vec<(i128, i128)>,
    cache: HashMap<Symbol, Expression>,
}

impl Expressions<'_, '_, '_> {
    fn operation_range(
        &self,
        op: BinaryOp,
        lhs: Symbol,
        rhs: Symbol,
        scalar: ScalarType,
    ) -> Option<(i128, i128)> {
        let (min, max) = match scalar {
            ScalarType::I32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
            ScalarType::I64 => (i128::from(i64::MIN), i128::from(i64::MAX)),
            _ => return None,
        };
        let (a, b) = self.bounds[lhs];
        let (c, d) = self.bounds[rhs];
        let (a, b, c, d) = (a.max(min), b.min(max), c.max(min), d.min(max));
        let (lower, upper) = match op {
            BinaryOp::Add => (a + c, b + d),
            BinaryOp::Subtract => (a - d, b - c),
            BinaryOp::Multiply => {
                let products = [a * c, a * d, b * c, b * d];
                (
                    *products.iter().min().unwrap(),
                    *products.iter().max().unwrap(),
                )
            }
            _ => return None,
        };
        (lower >= min && upper <= max).then_some((lower, upper))
    }

    fn get(&mut self, symbol: Symbol) -> Option<Expression> {
        if let Some(expression) = self.cache.get(&symbol) {
            return Some(expression.clone());
        }
        let mut result = Expression::default();
        let mut pending = Expression::default();
        pending.add_term(symbol, 1)?;
        // Definitions only reference earlier symbols. Descending traversal
        // combines shared subexpressions before expanding them, avoiding an
        // exponential walk of expressions such as repeated x = x + x.
        while let Some((symbol, coefficient)) = pending.terms.pop_last() {
            match self.proof.nodes[symbol] {
                Node::Constant(value) => {
                    result.constant = result
                        .constant
                        .checked_add(coefficient.checked_mul(i128::from(value))?)?;
                }
                Node::Binary(op, lhs, rhs, scalar)
                    if self.operation_range(op, lhs, rhs, scalar).is_some() =>
                {
                    match op {
                        BinaryOp::Add | BinaryOp::Subtract => {
                            pending.add_term(lhs, coefficient)?;
                            pending.add_term(
                                rhs,
                                if op == BinaryOp::Add {
                                    coefficient
                                } else {
                                    coefficient.checked_neg()?
                                },
                            )?;
                        }
                        BinaryOp::Multiply => {
                            let product = match (self.proof.nodes[lhs], self.proof.nodes[rhs]) {
                                (Node::Constant(value), _) => Some((rhs, value)),
                                (_, Node::Constant(value)) => Some((lhs, value)),
                                _ => None,
                            };
                            if let Some((value, factor)) = product {
                                pending.add_term(
                                    value,
                                    coefficient.checked_mul(i128::from(factor))?,
                                )?;
                            } else {
                                result.add_term(symbol, coefficient)?;
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                _ => result.add_term(symbol, coefficient)?,
            }
        }
        self.cache.insert(symbol, result.clone());
        Some(result)
    }
}

pub(super) fn ordered(
    proof: &Proof<'_, '_>,
    left: Symbol,
    right: Symbol,
    strict: bool,
    env: &Environment,
) -> bool {
    if !proof
        .nodes
        .iter()
        .any(|node| matches!(node, Node::Binary(..)))
    {
        return false;
    }
    let available = proof.available(left, right, env);
    let bounds = (0..proof.nodes.len())
        .map(|symbol| {
            (
                proof.bound(symbol, false, env, &available),
                proof.bound(symbol, true, env, &available),
            )
        })
        .collect();
    let mut expressions = Expressions {
        proof,
        bounds,
        cache: HashMap::new(),
    };
    // Whole-function local summaries can combine several assignments. Recover
    // each immutable definition's tighter range, in dependency order, using
    // only operations whose intermediate arithmetic has already been proved.
    for (symbol, node) in proof.nodes.iter().enumerate() {
        if !available.contains(&symbol) {
            continue;
        }
        if let Node::Binary(op, lhs, rhs, scalar) = *node {
            if let Some((min, max)) = expressions.operation_range(op, lhs, rhs, scalar) {
                expressions.bounds[symbol].0 = expressions.bounds[symbol].0.max(min);
                expressions.bounds[symbol].1 = expressions.bounds[symbol].1.min(max);
            }
        }
    }
    let Some(expression) = expressions
        .get(left)
        .and_then(|lhs| lhs.subtract(&expressions.get(right)?, strict))
    else {
        return false;
    };
    let upper =
        expression
            .terms
            .iter()
            .try_fold(expression.constant, |sum, (&symbol, &coefficient)| {
                let (min, max) = expressions.bounds[symbol];
                sum.checked_add(coefficient.checked_mul(if coefficient > 0 { max } else { min })?)
            });
    if upper.is_some_and(|upper| upper <= 0) {
        return true;
    }

    let zero = proof.nodes.len();
    let Some((left, right, limit)) = expression.difference(zero) else {
        return false;
    };
    let mut edges = Vec::new();
    for (from, orders) in proof.edges.iter().enumerate() {
        if !available.contains(&from) {
            continue;
        }
        for &(to, strict) in orders {
            if !available.contains(&to) {
                continue;
            }
            if let Some((left, right, limit)) = expressions
                .get(from)
                .and_then(|lhs| lhs.subtract(&expressions.get(to)?, strict))
                .and_then(|e| e.difference(zero))
            {
                edges.push((right, left, limit));
            }
        }
    }
    for &(from, to, strict) in &env.assumptions {
        if let Some((left, right, limit)) = expressions
            .get(from)
            .and_then(|lhs| lhs.subtract(&expressions.get(to)?, strict))
            .and_then(|e| e.difference(zero))
        {
            edges.push((right, left, limit));
        }
    }
    for (symbol, &(min, max)) in expressions.bounds.iter().enumerate() {
        if !available.contains(&symbol) {
            continue;
        }
        edges.push((zero, symbol, max));
        edges.push((symbol, zero, -min));
    }
    let mut distances = vec![None::<i128>; zero + 1];
    distances[right] = Some(0);
    // Bellman-Ford supports negative offsets. A bounded number of rounds also
    // terminates for contradictory (unreachable) path conditions.
    for _ in 0..=zero {
        if distances[left].is_some_and(|distance| distance <= limit) {
            return true;
        }
        let mut changed = false;
        for &(from, to, offset) in &edges {
            if let Some(candidate) = distances[from].and_then(|d| d.checked_add(offset)) {
                if distances[to].is_none_or(|old| candidate < old) {
                    distances[to] = Some(candidate);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    distances[left].is_some_and(|distance| distance <= limit)
}
