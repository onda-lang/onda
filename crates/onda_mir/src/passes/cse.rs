use std::collections::HashMap;

use crate::{
    BinaryOp, Block, CompareOp, Function, Intrinsic, LocalId, PassingMode, Rvalue, ScalarType,
    ScalarValue, StatementKind, UnaryOp, Value,
};

use super::PassStats;

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
enum ValueKey {
    Local(u32),
    F32(u32),
    F64(u64),
    I32(i32),
    I64(i64),
    Bool(bool),
}

impl From<Value> for ValueKey {
    fn from(value: Value) -> Self {
        match value {
            Value::Local(local) => Self::Local(local.raw()),
            Value::Constant(ScalarValue::F32(value)) => Self::F32(value.to_bits()),
            Value::Constant(ScalarValue::F64(value)) => Self::F64(value.to_bits()),
            Value::Constant(ScalarValue::I32(value)) => Self::I32(value),
            Value::Constant(ScalarValue::I64(value)) => Self::I64(value),
            Value::Constant(ScalarValue::Bool(value)) => Self::Bool(value),
        }
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
enum ExpressionKey {
    Unary(UnaryOp, ValueKey),
    Binary(BinaryOp, ValueKey, ValueKey),
    Compare(CompareOp, ValueKey, ValueKey),
    Cast(ValueKey, ScalarType),
    Intrinsic(Intrinsic, Vec<ValueKey>),
}

pub(super) fn eliminate_common_subexpressions(
    function: &mut Function,
    passing_modes: &[Vec<PassingMode>],
    stats: &mut PassStats,
) {
    let stable = super::stable_local_values(function, passing_modes);
    let mut available = HashMap::new();
    eliminate_block(&mut function.body, &stable, &mut available, stats);
}

fn eliminate_block(
    block: &mut Block,
    stable: &[bool],
    available: &mut HashMap<ExpressionKey, LocalId>,
    stats: &mut PassStats,
) {
    for statement in &mut block.statements {
        match &mut statement.kind {
            StatementKind::Assign { destination, value } if destination.projections.is_empty() => {
                let crate::PlaceBase::Local(destination) = destination.base else {
                    continue;
                };
                let Some(key) = expression_key(value, stable) else {
                    continue;
                };
                if let Some(existing) = available.get(&key).copied() {
                    *value = Rvalue::Use(Value::Local(existing));
                    stats.eliminated_common_subexpressions =
                        stats.eliminated_common_subexpressions.saturating_add(1);
                } else if stable[destination.index()] {
                    available.insert(key, destination);
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                let mut then_available = available.clone();
                let mut else_available = available.clone();
                eliminate_block(then_block, stable, &mut then_available, stats);
                eliminate_block(else_block, stable, &mut else_available, stats);
            }
            StatementKind::Loop { body } => {
                let mut body_available = available.clone();
                eliminate_block(body, stable, &mut body_available, stats);
            }
            _ => {}
        }
    }
}

fn expression_key(value: &Rvalue, stable: &[bool]) -> Option<ExpressionKey> {
    let key = match value {
        Rvalue::Unary { op, operand } => ExpressionKey::Unary(*op, value_key(*operand, stable)?),
        Rvalue::Binary { op, lhs, rhs } => {
            ExpressionKey::Binary(*op, value_key(*lhs, stable)?, value_key(*rhs, stable)?)
        }
        Rvalue::Compare { op, lhs, rhs } => {
            ExpressionKey::Compare(*op, value_key(*lhs, stable)?, value_key(*rhs, stable)?)
        }
        Rvalue::Cast { value, to } => ExpressionKey::Cast(value_key(*value, stable)?, *to),
        Rvalue::Intrinsic { intrinsic, args } => ExpressionKey::Intrinsic(
            *intrinsic,
            args.iter()
                .copied()
                .map(|value| value_key(value, stable))
                .collect::<Option<Vec<_>>>()?,
        ),
        // Memory and descriptor operations deliberately remain outside local
        // value numbering until alias/provenance facts can identify their
        // concrete source. This keeps the pass correct across state mutation,
        // host-visible buffers, and checked operations.
        _ => return None,
    };
    Some(key)
}

fn value_key(value: Value, stable: &[bool]) -> Option<ValueKey> {
    match value {
        Value::Local(local) if !stable[local.index()] => None,
        _ => Some(value.into()),
    }
}
