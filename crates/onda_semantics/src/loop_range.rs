//! Constant integer ranges, after conversion to the loop's induction type.
use onda_mir::ScalarValue;

#[derive(Clone, Copy)]
pub(crate) enum StaticForPlan {
    Empty,
    NonEmpty {
        min: ScalarValue,
        max: ScalarValue,
        last: ScalarValue,
    },
}

pub(crate) fn static_for_plan(
    start: ScalarValue,
    end: ScalarValue,
    step: ScalarValue,
    inclusive: bool,
) -> Option<StaticForPlan> {
    let (i32_range, start, end, step) = match (start, end, step) {
        (ScalarValue::I32(start), ScalarValue::I32(end), ScalarValue::I32(step)) => {
            (true, i128::from(start), i128::from(end), i128::from(step))
        }
        (ScalarValue::I64(start), ScalarValue::I64(end), ScalarValue::I64(step)) => {
            (false, i128::from(start), i128::from(end), i128::from(step))
        }
        _ => return None,
    };
    if step == 0 {
        return None;
    }
    let last = if step > 0 {
        let upper = if inclusive { end } else { end - 1 };
        if start > upper {
            return Some(StaticForPlan::Empty);
        }
        start + ((upper - start) / step) * step
    } else {
        let lower = if inclusive { end } else { end + 1 };
        if start < lower {
            return Some(StaticForPlan::Empty);
        }
        start - ((start - lower) / -step) * -step
    };
    let scalar = |value| {
        if i32_range {
            ScalarValue::I32(i32::try_from(value).expect("i32-bounded iteration"))
        } else {
            ScalarValue::I64(i64::try_from(value).expect("i64-bounded iteration"))
        }
    };
    Some(StaticForPlan::NonEmpty {
        min: scalar(start.min(last)),
        max: scalar(start.max(last)),
        last: scalar(last),
    })
}
