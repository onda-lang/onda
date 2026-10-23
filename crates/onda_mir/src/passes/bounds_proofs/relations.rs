//! Path-sensitive integer ordering for bounds. Symbols name values, not
//! mutable storage: copies preserve identity, writes create new identities,
//! and loop entry forgets every value the loop can change. The proof needs no
//! source-language loop pattern or backend-specific representation.
use std::collections::{HashMap, HashSet};

use crate::{BinaryOp, CompareOp, IntegerRange, Intrinsic, LocalId, ScalarType, UnaryOp};

use super::super::collect_block_writes;
use super::*;

mod affine;

type Symbol = usize;
// left <= right, or left < right when strict is true.
type Order = (Symbol, Symbol, bool);

#[derive(Clone, Copy)]
enum Node {
    Unknown,
    Constant(i64),
    Boolean(bool),
    Choice(Symbol, Symbol, Symbol),
    Compare(CompareOp, Symbol, Symbol),
    Not(Symbol),
    Binary(BinaryOp, Symbol, Symbol, ScalarType),
    Extremum(Symbol, Symbol),
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
    predecessors: Vec<Vec<(Symbol, bool)>>,
    ranges: Vec<Option<IntegerRange>>,
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
        predecessors: Vec::new(),
        ranges: Vec::new(),
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
        self.predecessors.push(Vec::new());
        self.ranges.push(None);
        id
    }

    fn local_node(&mut self, local: LocalId, node: Node) -> Symbol {
        let symbol = self.node(node);
        self.ranges[symbol] = value_range(self.context, Value::Local(local));
        symbol
    }

    fn order(&mut self, left: Symbol, right: Symbol, strict: bool) {
        self.edges[left].push((right, strict));
        self.predecessors[right].push((left, strict));
    }

    fn fresh_local(&mut self, local: LocalId) -> Option<Symbol> {
        if self.escaped[local.index()] {
            return None;
        }
        match self.context.program.types[self.context.function.locals[local.index()].ty.index()] {
            Type::Scalar(ScalarType::I32 | ScalarType::I64 | ScalarType::Bool)
            | Type::Slice { .. } => Some(self.local_node(local, Node::Unknown)),
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
            Value::Constant(ScalarValue::Bool(value)) => Some(self.node(Node::Boolean(value))),
            _ => None,
        }
    }

    fn length(&mut self, slice: Value, env: &Environment) -> Option<Symbol> {
        let slice = self.value(slice, env)?;
        if let Some(&len) = self.lengths.get(&slice) {
            return Some(len);
        }
        let len = self.node(Node::Unknown);
        self.ranges[len] = IntegerRange::new(ScalarType::I32, 0, i64::from(i32::MAX));
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
            Rvalue::Binary { op, lhs, rhs }
                if matches!(op, BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply) =>
            {
                let scalar = self.integer_type(*lhs)?;
                let lhs = self.value(*lhs, env)?;
                let rhs = self.value(*rhs, env)?;
                return Some(self.local_node(local, Node::Binary(*op, lhs, rhs, scalar)));
            }
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
                let result = self.local_node(local, Node::Extremum(lhs, rhs));
                if matches!(
                    value,
                    Rvalue::Intrinsic {
                        intrinsic: Intrinsic::Min,
                        ..
                    }
                ) {
                    self.order(result, lhs, false);
                    self.order(result, rhs, false);
                } else {
                    self.order(lhs, result, false);
                    self.order(rhs, result, false);
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

    fn assume(&mut self, condition: Symbol, truth: bool, env: &mut Environment) {
        let mut pending = vec![(condition, truth)];
        self.visited.clear();
        while let Some((condition, truth)) = pending.pop() {
            if !self.visited.insert((condition, truth)) {
                continue;
            }
            match self.nodes[condition] {
                Node::Compare(op, lhs, rhs) => self.assume_order(op, lhs, rhs, truth, env),
                Node::Not(value) => {
                    pending.push((value, !truth));
                }
                // Short-circuit boolean expressions lower to branch joins.
                // If one arm cannot produce the assumed result, execution
                // must have taken the other arm and satisfied its condition.
                Node::Choice(condition, yes, no) => {
                    if matches!(self.nodes[yes], Node::Boolean(value) if value != truth) {
                        pending.extend([(condition, false), (no, truth)]);
                    } else if matches!(self.nodes[no], Node::Boolean(value) if value != truth) {
                        pending.extend([(condition, true), (yes, truth)]);
                    }
                }
                _ => {}
            }
        }
    }

    fn assume_order(
        &mut self,
        op: CompareOp,
        lhs: Symbol,
        rhs: Symbol,
        truth: bool,
        env: &mut Environment,
    ) {
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
        self.known_order(left, right, strict, env)
            || affine::ordered(self, left, right, strict, env)
    }

    fn known_order(
        &mut self,
        left: Symbol,
        right: Symbol,
        strict: bool,
        env: &Environment,
    ) -> bool {
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

    /// Bounds of immutable values at this program point. Only unconditional
    /// ranges and the current path's ordering facts participate; an arithmetic
    /// identity is never used to prove its own absence of overflow.
    fn available(&self, left: Symbol, right: Symbol, env: &Environment) -> HashSet<Symbol> {
        // Unrelated live locals do not participate in this ordering query.
        // Include its definitions and the current path's constraints, rather
        // than rebuilding facts for the function's entire expression history.
        let mut pending = vec![left, right];
        for &(left, right, _) in &env.assumptions {
            pending.extend([left, right]);
        }
        let mut available = HashSet::new();
        while let Some(symbol) = pending.pop() {
            if !available.insert(symbol) {
                continue;
            }
            match self.nodes[symbol] {
                Node::Binary(_, left, right, _)
                | Node::Compare(_, left, right)
                | Node::Extremum(left, right) => pending.extend([left, right]),
                Node::Not(operand) => pending.push(operand),
                Node::Choice(condition, yes, no) => pending.extend([condition, yes, no]),
                _ => {}
            }
            if let Some(&length) = self.lengths.get(&symbol) {
                pending.push(length);
            }
        }
        available
    }

    fn bound(
        &self,
        symbol: Symbol,
        upper: bool,
        env: &Environment,
        available: &HashSet<Symbol>,
    ) -> i128 {
        let mut result = if upper {
            i128::from(i64::MAX)
        } else {
            i128::from(i64::MIN)
        };
        let mut pending = vec![(symbol, false)];
        let mut visited = HashSet::new();
        while let Some((symbol, strict)) = pending.pop() {
            if !available.contains(&symbol) || !visited.insert((symbol, strict)) {
                continue;
            }
            if let Some(bound) = self.unconditional_bound(symbol, upper) {
                let bound = i128::from(bound)
                    + if upper {
                        -i128::from(strict)
                    } else {
                        i128::from(strict)
                    };
                result = if upper {
                    result.min(bound)
                } else {
                    result.max(bound)
                };
            }
            let edges = if upper {
                &self.edges
            } else {
                &self.predecessors
            };
            for &(next, edge_strict) in &edges[symbol] {
                pending.push((next, strict || edge_strict));
            }
            for &(left, right, edge_strict) in &env.assumptions {
                let (from, to) = if upper { (left, right) } else { (right, left) };
                if from == symbol {
                    pending.push((to, strict || edge_strict));
                }
            }
        }
        result
    }

    fn unconditional_bound(&self, symbol: Symbol, upper: bool) -> Option<i64> {
        match self.nodes[symbol] {
            Node::Constant(value) => Some(value),
            _ => self.ranges[symbol].map(|range| if upper { range.max() } else { range.min() }),
        }
    }

    fn access(
        &mut self,
        index: Value,
        limit: accesses::Limit,
        bounds: &mut BoundsMode,
        env: &Environment,
    ) {
        if *bounds == BoundsMode::Unchecked {
            return;
        }
        let Some(value) = self.value(index, env) else {
            return;
        };
        let len = match limit {
            accesses::Limit::Fixed(len) => Some(self.constant(ScalarType::I64, i64::from(len))),
            accesses::Limit::Slice(slice) => self.length(slice, env),
            accesses::Limit::Value(length) => self.value(length, env),
        };
        let Some(len) = len else {
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
                    *slot = match (condition, then_value, *slot) {
                        (Some(condition), Some(yes), Some(no))
                            if !self.escaped[local.index()]
                                && matches!(
                                    self.context.program.types
                                        [self.context.function.locals[local.index()].ty.index()],
                                    Type::Scalar(ScalarType::Bool)
                                ) =>
                        {
                            Some(self.node(Node::Choice(condition, yes, no)))
                        }
                        _ => self.fresh_local(local),
                    };
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
            accesses::visit(
                self.context,
                &mut statement.kind,
                &mut |index, limit, bounds| {
                    self.access(index, limit, bounds, env);
                },
            );
            match &mut statement.kind {
                StatementKind::Assign { destination, value } => {
                    if let PlaceBase::Local(local) = destination.base {
                        env.locals[local.index()] = if destination.projections.is_empty() {
                            self.rvalue(local, value, env)
                        } else {
                            self.fresh_local(local)
                        };
                        // A copy can tighten a shared value's range (for
                        // example the body value of a counted loop). Scope
                        // only new bounds to this assignment: loads and
                        // constants already carry their unconditional bounds.
                        if matches!(value, Rvalue::Use(_) | Rvalue::Load(_)) {
                            if let Some((symbol, range)) = env.locals[local.index()]
                                .zip(value_range(self.context, Value::Local(local)))
                            {
                                if self
                                    .unconditional_bound(symbol, false)
                                    .is_none_or(|lower| lower < range.min())
                                {
                                    let lower = self.constant(range.scalar(), range.min());
                                    env.assumptions.push((lower, symbol, false));
                                }
                                if self
                                    .unconditional_bound(symbol, true)
                                    .is_none_or(|upper| upper > range.max())
                                {
                                    let upper = self.constant(range.scalar(), range.max());
                                    env.assumptions.push((symbol, upper, false));
                                }
                            }
                        }
                    }
                }
                StatementKind::Call { results, .. } => {
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
                    let lower = crate::loops::guarded_loop(body).and_then(|counted| {
                        let mut writes = HashSet::new();
                        // The recognized increment is outside counted.body.
                        // Any other write would invalidate monotonic progress.
                        super::super::collect_statements_writes(counted.body, &mut writes);
                        (!writes.contains(&counted.index))
                            .then_some((counted.index, env.locals[counted.index.index()]))
                    });
                    let written = written_locals(&[body]);
                    for &local in &written {
                        env.locals[local.index()] = self.fresh_local(local);
                    }
                    let base = env.assumptions.len();
                    if let Some((index, Some(initial))) = lower {
                        if let Some(current) = env.locals[index.index()] {
                            // A guarded i32 `i += 1` cannot wrap: its guard
                            // requires i < limit <= i32::MAX on every trip.
                            env.assumptions.push((initial, current, false));
                        }
                    }
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
    let mut written = HashSet::new();
    for block in blocks {
        collect_block_writes(block, &mut written);
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
            StatementKind::Assign { destination, value } => {
                *has_access |= !destination.projections.is_empty();
                match value {
                    Rvalue::Load(place) => *has_access |= !place.projections.is_empty(),
                    Rvalue::SliceLoad { bounds, .. } | Rvalue::NormalizeIndex { bounds, .. } => {
                        *has_access |= *bounds != BoundsMode::Unchecked;
                    }
                    Rvalue::MakeSlice {
                        source: crate::SliceSource::Place(place),
                        ..
                    } => {
                        escape(place, escaped);
                        *has_access |= !place.projections.is_empty();
                    }
                    Rvalue::Use(_)
                    | Rvalue::Unary { .. }
                    | Rvalue::Binary { .. }
                    | Rvalue::Compare { .. }
                    | Rvalue::Cast { .. }
                    | Rvalue::Intrinsic { .. }
                    | Rvalue::SliceLen(_)
                    | Rvalue::InitAll
                    | Rvalue::ProcessFrame { .. } => {}
                    _ => *has_access = true,
                }
            }
            StatementKind::SliceStore { bounds, .. } => {
                *has_access |= *bounds != BoundsMode::Unchecked;
            }
            StatementKind::Call { args, .. } => {
                *has_access |= args
                    .iter()
                    .any(|arg| !matches!(arg, CallArgument::Value(_)));
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
            StatementKind::BufferStore { .. }
            | StatementKind::BufferParamStore { .. }
            | StatementKind::OutputStore {
                element: Some(_), ..
            }
            | StatementKind::ControlOutputStore {
                element: Some(_), ..
            }
            | StatementKind::PublishDelegate { .. } => *has_access = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
