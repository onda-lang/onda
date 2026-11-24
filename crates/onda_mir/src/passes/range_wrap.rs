//! Lower exact integer wrapping to source-width masks after bounds analysis.
use crate::*;

pub(super) fn lower_power_of_two(program: &mut Program) -> u64 {
    let mut lowered = 0;
    for function in &mut program.functions {
        lower_block(
            &mut function.body,
            &mut function.locals,
            &program.types,
            &mut lowered,
        );
    }
    lowered
}

fn lower_block(block: &mut Block, locals: &mut Vec<Local>, types: &[Type], lowered: &mut u64) {
    let mut insertions = Vec::new();
    for (index, statement) in block.statements.iter_mut().enumerate() {
        match &mut statement.kind {
            StatementKind::Assign { value, .. } => {
                if let Some(replacement) =
                    lower_wrap(value, locals, types, statement.source, |assignment| {
                        insertions.push((index, assignment))
                    })
                {
                    *value = replacement;
                    *lowered += 1;
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                lower_block(then_block, locals, types, lowered);
                lower_block(else_block, locals, types, lowered);
            }
            StatementKind::Loop { body } => lower_block(body, locals, types, lowered),
            _ => {}
        }
    }
    if !insertions.is_empty() {
        let mut original = std::mem::take(&mut block.statements).into_iter();
        let mut expanded = Vec::with_capacity(original.len() + insertions.len());
        let mut copied = 0;
        for (index, assignment) in insertions {
            expanded.extend(original.by_ref().take(index - copied));
            expanded.push(assignment);
            copied = index;
        }
        expanded.extend(original);
        block.statements = expanded;
    }
}

fn lower_wrap(
    value: &Rvalue,
    locals: &mut Vec<Local>,
    types: &[Type],
    source: SourceSpan,
    mut insert: impl FnMut(Statement),
) -> Option<Rvalue> {
    let Rvalue::Intrinsic {
        intrinsic: Intrinsic::RangeWrap,
        args,
    } = value
    else {
        return None;
    };
    let [input, Value::Constant(lower), Value::Constant(upper)] = args.as_slice() else {
        return None;
    };
    let (lo, hi, bits) = match (*lower, *upper) {
        (ScalarValue::I32(lo), ScalarValue::I32(hi)) => (i128::from(lo), i128::from(hi), 32),
        (ScalarValue::I64(lo), ScalarValue::I64(hi)) => (i128::from(lo), i128::from(hi), 64),
        _ => return None,
    };
    let width = (hi - lo + 1) as u128;
    if !width.is_power_of_two() {
        return None;
    }
    let lower = Value::Constant(*lower);
    if width == 1 {
        return Some(Rvalue::Use(lower));
    }
    if width == 1 << bits {
        return Some(Rvalue::Use(*input));
    }

    // The mathematical distance can exceed the signed source width. Its low
    // bits still give the Euclidean offset because the width divides 2^bits.
    let mask = Value::Constant(if bits == 32 {
        ScalarValue::I32((width - 1) as i32)
    } else {
        ScalarValue::I64((width - 1) as i64)
    });
    let masked = |distance| Rvalue::Binary {
        op: BinaryOp::BitAnd,
        lhs: distance,
        rhs: mask,
    };
    if lo == 0 {
        return Some(masked(*input));
    }
    if locals.len() + 2 > MAX_FUNCTION_LOCAL_COUNT {
        return None;
    }
    let scalar = if bits == 32 {
        ScalarType::I32
    } else {
        ScalarType::I64
    };
    let ty = TypeId::new(types.iter().position(|ty| *ty == Type::Scalar(scalar))? as u32);
    let mut store = |value| {
        let local = LocalId::new(locals.len() as u32);
        locals.push(Local {
            name: None,
            ty,
            integer_range: None,
        });
        insert(Statement {
            source,
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(local),
                    projections: Vec::new(),
                },
                value,
            },
        });
        Value::Local(local)
    };
    let distance = store(Rvalue::Binary {
        op: BinaryOp::Subtract,
        lhs: *input,
        rhs: lower,
    });
    let offset = store(masked(distance));
    Some(Rvalue::Binary {
        op: BinaryOp::Add,
        lhs: lower,
        rhs: offset,
    })
}
