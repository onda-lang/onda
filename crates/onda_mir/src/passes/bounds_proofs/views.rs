//! Descriptor shape facts remove work without treating payloads as immutable.
use super::*;

pub(super) fn contiguous_length(origins: &crate::StorageOrigins) -> Option<u32> {
    let mut regions = origins.state_regions()?;
    let length = regions.next()?.len;
    regions.all(|region| region.len == length).then_some(length)
}

pub(super) fn simplify(context: &Context<'_>, value: &mut Rvalue, stats: &mut PassStats) -> u64 {
    if let Rvalue::SliceLen(slice) = value {
        if let Some(length) = slice_length(context, *slice).and_then(|n| i32::try_from(n).ok()) {
            *value = Rvalue::Use(Value::Constant(ScalarValue::I32(length)));
            stats.folded_rvalues = stats.folded_rvalues.saturating_add(1);
        }
        return 0;
    }
    let Rvalue::MakeSlice {
        source: crate::SliceSource::Place(place),
        start,
        len,
        bounds,
        ..
    } = value
    else {
        return 0;
    };
    if *bounds == BoundsMode::Unchecked {
        return 0;
    }
    let Some(ty) = place_type(context, place) else {
        return 0;
    };
    let length = match context.program.types[ty.index()] {
        Type::Array { len, .. } => Some(len),
        Type::Scalar(_) => Some(1),
        Type::Slice { .. } if place.projections.is_empty() => match place.base {
            PlaceBase::Local(local) => slice_length(context, Value::Local(local)),
            PlaceBase::Parameter(parameter) => {
                contiguous_length(context.memory.parameter(context.function_id, parameter))
            }
            _ => None,
        },
        _ => None,
    };
    let proven = length
        .zip(value_range(context, *start))
        .zip(value_range(context, *len))
        .is_some_and(|((length, start), len)| {
            start.is_nonnegative()
                && len.is_nonnegative()
                && start
                    .max()
                    .checked_add(len.max())
                    .is_some_and(|end| end <= i64::from(length))
        });
    if proven {
        *bounds = BoundsMode::Unchecked;
    }
    u64::from(proven)
}
