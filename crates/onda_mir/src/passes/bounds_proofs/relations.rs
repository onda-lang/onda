//! Path-sensitive integer ordering for slice bounds. Symbols name values, not
//! mutable storage: copies preserve identity, writes create new identities,
//! and loop entry forgets every value the loop can change. The proof needs no
//! source-language loop pattern or backend-specific representation.
use std::collections::{HashMap, HashSet};

use crate::{CompareOp, Intrinsic, LocalId, ScalarType, UnaryOp};

use super::*;

type Symbol = usize;
// left <= right, or left < right when strict is true.
type Order = (Symbol, Symbol, bool);

#[derive(Clone, Copy)]
enum Node {
    Unknown,
    Constant(i64),
    Compare(CompareOp, Symbol, Symbol),
    Not(Symbol),
}

struct Environment {
    locals: Vec<Option<Symbol>>,
    assumptions: Vec<Order>,
}

struct Proof<'a, 'p> {
    context: &'a Context<'p>,
    escaped: Vec<bool>,
    nodes: Vec<Node>,
    edges: Vec<Vec<(Symbol, bool)>>,
    constants: HashMap<(ScalarType, i64), Symbol>,
    lengths: HashMap<Symbol, Symbol>,
    visited: HashSet<(Symbol, bool)>,
    pending: Vec<(Symbol, bool)>,
    eliminated: u64,
}

pub(super) fn eliminate(context: &Context<'_>, body: &mut Block) -> u64 {
    let mut escaped = vec![false; context.function.locals.len()];
    let mut has_access = false;
    inspect(body, &mut escaped, &mut has_access);
    if !has_access {
        return 0;
    }
    let mut proof = Proof {
        context,
        escaped,
        nodes: Vec::new(),
        edges: Vec::new(),
        constants: HashMap::new(),
        lengths: HashMap::new(),
        visited: HashSet::new(),
        pending: Vec::new(),
        eliminated: 0,
    };
    let mut environment = Environment {
        locals: vec![None; context.function.locals.len()],
        assumptions: Vec::new(),
    };
    proof.block(body, &mut environment);
    proof.eliminated
}

impl Proof<'_, '_> {
    fn node(&mut self, node: Node) -> Symbol {
        let id = self.nodes.len();
        self.nodes.push(node);
        self.edges.push(Vec::new());
        id
    }

    fn fresh_local(&mut self, local: LocalId) -> Option<Symbol> {
        if self.escaped[local.index()] {
            return None;
        }
        match self.context.program.types[self.context.function.locals[local.index()].ty.index()] {
            Type::Scalar(ScalarType::I32 | ScalarType::I64 | ScalarType::Bool)
            | Type::Slice { .. } => Some(self.node(Node::Unknown)),
            _ => None,
        }
    }

    fn integer_type(&self, value: Value) -> Option<ScalarType> {
        let scalar = match value {
            Value::Constant(ScalarValue::I32(_)) => ScalarType::I32,
            Value::Constant(ScalarValue::I64(_)) => ScalarType::I64,
            Value::Local(local) => {
                let Type::Scalar(scalar) = self.context.program.types
                    [self.context.function.locals[local.index()].ty.index()]
                else {
                    return None;
                };
                scalar
            }
            _ => return None,
        };
        matches!(scalar, ScalarType::I32 | ScalarType::I64).then_some(scalar)
    }

    fn constant(&mut self, scalar: ScalarType, value: i64) -> Symbol {
        if let Some(&symbol) = self.constants.get(&(scalar, value)) {
            return symbol;
        }
        let symbol = self.node(Node::Constant(value));
        self.constants.insert((scalar, value), symbol);
        symbol
    }

    fn value(&mut self, value: Value, env: &Environment) -> Option<Symbol> {
        match value {
            Value::Local(local) => env.locals[local.index()],
            Value::Constant(ScalarValue::I32(value)) => {
                Some(self.constant(ScalarType::I32, i64::from(value)))
            }
            Value::Constant(ScalarValue::I64(value)) => Some(self.constant(ScalarType::I64, value)),
            _ => None,
        }
    }

    fn length(&mut self, slice: Value, env: &Environment) -> Option<Symbol> {
        let slice = self.value(slice, env)?;
        if let Some(&len) = self.lengths.get(&slice) {
            return Some(len);
        }
        let len = self.node(Node::Unknown);
        self.lengths.insert(slice, len);
        Some(len)
    }

    fn rvalue(&mut self, local: LocalId, value: &Rvalue, env: &Environment) -> Option<Symbol> {
        if self.escaped[local.index()] {
            return None;
        }
        match value {
            Rvalue::Use(value) => return self.value(*value, env),
            Rvalue::Load(Place {
                base: PlaceBase::Local(source),
                projections,
            }) if projections.is_empty() => return env.locals[source.index()],
            Rvalue::SliceLen(slice) => return self.length(*slice, env),
            Rvalue::Compare { op, lhs, rhs }
                if self.integer_type(*lhs).is_some()
                    && self.integer_type(*lhs) == self.integer_type(*rhs) =>
            {
                let lhs = self.value(*lhs, env)?;
                let rhs = self.value(*rhs, env)?;
                return Some(self.node(Node::Compare(*op, lhs, rhs)));
            }
            Rvalue::Unary {
                op: UnaryOp::LogicalNot,
                operand,
            } => {
                let operand = self.value(*operand, env)?;
                return Some(self.node(Node::Not(operand)));
            }
            Rvalue::Intrinsic {
                intrinsic: Intrinsic::Min | Intrinsic::Max,
                args,
            } if args.len() == 2 && self.integer_type(args[0]).is_some() => {
                let lhs = self.value(args[0], env)?;
                let rhs = self.value(args[1], env)?;
                let result = self.node(Node::Unknown);
                if matches!(
                    value,
                    Rvalue::Intrinsic {
                        intrinsic: Intrinsic::Min,
                        ..
                    }
                ) {
                    self.edges[result].extend([(lhs, false), (rhs, false)]);
                } else {
                    self.edges[lhs].push((result, false));
                    self.edges[rhs].push((result, false));
                }
                return Some(result);
            }
            Rvalue::MakeSlice {
                len,
                bounds: BoundsMode::Checked | BoundsMode::Unchecked,
                ..
            } => {
                let result = self.node(Node::Unknown);
                if let Some(len) = self.value(*len, env) {
                    self.lengths.insert(result, len);
                }
                return Some(result);
            }
            _ => {}
        }
        self.fresh_local(local)
    }

    fn assume(&mut self, mut condition: Symbol, mut truth: bool, env: &mut Environment) {
        let (op, lhs, rhs) = loop {
            match self.nodes[condition] {
                Node::Compare(op, lhs, rhs) => break (op, lhs, rhs),
                Node::Not(value) => {
                    condition = value;
                    truth = !truth;
                }
                _ => return,
            }
        };
        use CompareOp::*;
        let order = match (op, truth) {
            (Less, true) | (GreaterEqual, false) => (lhs, rhs, true),
            (LessEqual, true) | (Greater, false) => (lhs, rhs, false),
            (Greater, true) | (LessEqual, false) => (rhs, lhs, true),
            (GreaterEqual, true) | (Less, false) => (rhs, lhs, false),
            (Equal, true) | (NotEqual, false) => {
                env.assumptions.push((rhs, lhs, false));
                (lhs, rhs, false)
            }
            _ => return,
        };
        env.assumptions.push(order);
    }

    fn ordered(&mut self, left: Symbol, right: Symbol, strict: bool, env: &Environment) -> bool {
        self.visited.clear();
        self.pending.clear();
        self.pending.push((left, false));
        while let Some((symbol, path_strict)) = self.pending.pop() {
            if !self.visited.insert((symbol, path_strict)) {
                continue;
            }
            if symbol == right && (!strict || path_strict) {
                return true;
            }
            if let (Node::Constant(left), Node::Constant(right)) =
                (self.nodes[symbol], self.nodes[right])
            {
                if left < right || (left == right && (!strict || path_strict)) {
                    return true;
                }
            }
            for &(next, edge_strict) in &self.edges[symbol] {
                self.pending.push((next, path_strict || edge_strict));
            }
            for &(from, to, edge_strict) in &env.assumptions {
                if from == symbol {
                    self.pending.push((to, path_strict || edge_strict));
                }
            }
        }
        false
    }

    fn access(&mut self, slice: Value, index: Value, bounds: &mut BoundsMode, env: &Environment) {
        if *bounds == BoundsMode::Unchecked {
            return;
        }
        let Some(value) = self.value(index, env) else {
            return;
        };
        let Some(len) = self.length(slice, env) else {
            return;
        };
        let zero = self.constant(ScalarType::I32, 0);
        let nonnegative = value_range(self.context, index)
            .is_some_and(|range| range.is_nonnegative())
            || self.ordered(zero, value, false, env);
        if nonnegative && self.ordered(value, len, true, env) {
            *bounds = BoundsMode::Unchecked;
            self.eliminated += 1;
        }
    }

    // Only locals written by a branch are saved/joined. Copying the complete
    // environment at every branch makes large functions needlessly quadratic.
    fn branch(
        &mut self,
        condition: Value,
        then: &mut Block,
        otherwise: &mut Block,
        env: &mut Environment,
    ) -> bool {
        let condition = self.value(condition, env);
        let written = written_locals(&[then, otherwise]);
        let incoming: Vec<_> = written
            .iter()
            .map(|local| env.locals[local.index()])
            .collect();
        let base = env.assumptions.len();
        if let Some(condition) = condition {
            self.assume(condition, true, env);
        }
        let then_continues = self.block(then, env);
        let then_values: Vec<_> = written
            .iter()
            .map(|local| env.locals[local.index()])
            .collect();
        let then_assumptions = env.assumptions[base..].to_vec();
        for (&local, &value) in written.iter().zip(&incoming) {
            env.locals[local.index()] = value;
        }
        env.assumptions.truncate(base);
        if let Some(condition) = condition {
            self.assume(condition, false, env);
        }
        let else_continues = self.block(otherwise, env);
        if then_continues {
            for (&local, &then_value) in written.iter().zip(&then_values) {
                let slot = &mut env.locals[local.index()];
                if !else_continues {
                    *slot = then_value;
                } else if *slot != then_value {
                    *slot = self.fresh_local(local);
                }
            }
            if else_continues {
                let common: Vec<_> = env.assumptions[base..]
                    .iter()
                    .copied()
                    .filter(|order| then_assumptions.contains(order))
                    .collect();
                env.assumptions.truncate(base);
                env.assumptions.extend(common);
            } else {
                env.assumptions.truncate(base);
                env.assumptions.extend(then_assumptions);
            }
        }
        then_continues || else_continues
    }

    fn block(&mut self, block: &mut Block, env: &mut Environment) -> bool {
        for statement in &mut block.statements {
            match &mut statement.kind {
                StatementKind::Assign { destination, value } => {
                    if let Rvalue::SliceLoad {
                        slice,
                        index,
                        bounds,
                    } = value
                    {
                        self.access(*slice, *index, bounds, env);
                    }
                    if let PlaceBase::Local(local) = destination.base {
                        env.locals[local.index()] = if destination.projections.is_empty() {
                            self.rvalue(local, value, env)
                        } else {
                            self.fresh_local(local)
                        };
                    }
                }
                StatementKind::SliceStore {
                    slice,
                    index,
                    bounds,
                    ..
                } => self.access(*slice, *index, bounds, env),
                StatementKind::Call { results, args, .. } => {
                    for arg in args {
                        if let CallArgument::SliceElement {
                            slice,
                            index,
                            bounds,
                        } = arg
                        {
                            self.access(*slice, *index, bounds, env);
                        }
                    }
                    for &local in results.iter() {
                        env.locals[local.index()] = self.fresh_local(local);
                    }
                }
                StatementKind::If {
                    condition,
                    then_block,
                    else_block,
                } => {
                    if !self.branch(*condition, then_block, else_block, env) {
                        return false;
                    }
                }
                StatementKind::Loop { body } => {
                    let written = written_locals(&[body]);
                    for &local in &written {
                        env.locals[local.index()] = self.fresh_local(local);
                    }
                    let base = env.assumptions.len();
                    self.block(body, env);
                    env.assumptions.truncate(base);
                    for local in written {
                        env.locals[local.index()] = self.fresh_local(local);
                    }
                }
                StatementKind::Break | StatementKind::Continue | StatementKind::Return { .. } => {
                    return false
                }
                _ => {}
            }
        }
        true
    }
}

fn written_locals(blocks: &[&Block]) -> Vec<LocalId> {
    fn collect(block: &Block, written: &mut HashSet<LocalId>) {
        for statement in &block.statements {
            match &statement.kind {
                StatementKind::Assign {
                    destination:
                        Place {
                            base: PlaceBase::Local(local),
                            ..
                        },
                    ..
                } => {
                    written.insert(*local);
                }
                StatementKind::Call { results, .. } => written.extend(results),
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    collect(then_block, written);
                    collect(else_block, written);
                }
                StatementKind::Loop { body } => collect(body, written),
                _ => {}
            }
        }
    }
    let mut written = HashSet::new();
    for block in blocks {
        collect(block, &mut written);
    }
    let mut written: Vec<_> = written.into_iter().collect();
    written.sort_unstable();
    written
}

fn inspect(block: &Block, escaped: &mut [bool], has_access: &mut bool) {
    let escape = |place: &Place, escaped: &mut [bool]| {
        if let PlaceBase::Local(local) = place.base {
            escaped[local.index()] = true;
        }
    };
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign {
                value:
                    Rvalue::MakeSlice {
                        source: crate::SliceSource::Place(place),
                        ..
                    },
                ..
            } => escape(place, escaped),
            StatementKind::Assign {
                value: Rvalue::SliceLoad { bounds, .. },
                ..
            }
            | StatementKind::SliceStore { bounds, .. } => {
                *has_access |= *bounds != BoundsMode::Unchecked;
            }
            StatementKind::Call { args, .. } => {
                for arg in args {
                    match arg {
                        CallArgument::Place(place)
                        | CallArgument::ArrayWindow { array: place, .. } => escape(place, escaped),
                        CallArgument::SliceElement { bounds, .. } => {
                            *has_access |= *bounds != BoundsMode::Unchecked;
                        }
                        _ => {}
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                inspect(then_block, escaped, has_access);
                inspect(else_block, escaped, has_access);
            }
            StatementKind::Loop { body } => inspect(body, escaped, has_access),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
