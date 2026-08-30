use super::*;

pub(super) fn local_display(function: &Function, local: crate::LocalId) -> String {
    function
        .locals
        .get(local.index())
        .and_then(|local| local.name.as_deref())
        .map_or_else(
            || format!("%{}", local.raw()),
            |name| format!("%{} ('{name}')", local.raw()),
        )
}

pub(super) fn static_initialization_path(
    projections: &[crate::Projection],
) -> Option<Vec<InitProjection>> {
    let mut path = Vec::with_capacity(projections.len());
    for projection in projections {
        match projection {
            crate::Projection::Field(field) => path.push(InitProjection::Field(field.raw())),
            crate::Projection::Index { index, .. } => {
                let Value::Constant(crate::ScalarValue::I32(index)) = index else {
                    return None;
                };
                let index = u32::try_from(*index).ok()?;
                path.push(InitProjection::Index(index));
            }
        }
    }
    Some(path)
}

pub(super) fn static_initialization_prefix(
    projections: &[crate::Projection],
) -> Vec<InitProjection> {
    let mut path = Vec::with_capacity(projections.len());
    for projection in projections {
        match projection {
            crate::Projection::Field(field) => path.push(InitProjection::Field(field.raw())),
            crate::Projection::Index { index, .. } => {
                let Value::Constant(crate::ScalarValue::I32(index)) = index else {
                    break;
                };
                let Ok(index) = u32::try_from(*index) else {
                    break;
                };
                path.push(InitProjection::Index(index));
            }
        }
    }
    path
}

pub(super) fn path_is_prefix(prefix: &[InitProjection], path: &[InitProjection]) -> bool {
    prefix.len() <= path.len() && prefix.iter().zip(path).all(|(lhs, rhs)| lhs == rhs)
}

pub(super) fn initialization_is_covered(
    covered: &HashSet<Vec<InitProjection>>,
    requested: &[InitProjection],
) -> bool {
    covered
        .iter()
        .any(|region| path_is_prefix(region, requested))
}

pub(super) fn type_at_initialization_path(
    program: &Program,
    root: crate::TypeId,
    path: &[InitProjection],
) -> Option<crate::TypeId> {
    let mut ty = root;
    for projection in path {
        ty = match projection {
            InitProjection::Field(field) => {
                let Type::Struct(structure) = program.types.get(ty.index())? else {
                    return None;
                };
                program
                    .structs
                    .get(structure.index())?
                    .fields
                    .get(*field as usize)?
                    .ty
            }
            InitProjection::Index(index) => {
                let Type::Array { element, len } = program.types.get(ty.index())? else {
                    return None;
                };
                if index >= len {
                    return None;
                }
                *element
            }
        };
    }
    Some(ty)
}

pub(super) fn direct_initialization_children(
    program: &Program,
    ty: crate::TypeId,
    covered_count: usize,
) -> Option<Vec<InitProjection>> {
    match program.types.get(ty.index())? {
        Type::Array { len, .. } => {
            let len = usize::try_from(*len).ok()?;
            if len > covered_count {
                return None;
            }
            Some(
                (0..len as u32)
                    .map(InitProjection::Index)
                    .collect::<Vec<_>>(),
            )
        }
        Type::Struct(structure) => {
            let field_count = program.structs.get(structure.index())?.fields.len();
            if field_count > covered_count {
                return None;
            }
            Some(
                (0..field_count as u32)
                    .map(InitProjection::Field)
                    .collect::<Vec<_>>(),
            )
        }
        Type::Scalar(_)
        | Type::Tuple(_)
        | Type::Slice { .. }
        | Type::Buffer { .. }
        | Type::BufferSpan { .. } => None,
    }
}

pub(super) fn normalize_initialization_coverage(
    program: &Program,
    root: crate::TypeId,
    covered: &mut HashSet<Vec<InitProjection>>,
    inserted: &[InitProjection],
) {
    for depth in (0..inserted.len()).rev() {
        let parent = &inserted[..depth];
        let Some(parent_ty) = type_at_initialization_path(program, root, parent) else {
            continue;
        };
        let Some(children) = direct_initialization_children(program, parent_ty, covered.len())
        else {
            continue;
        };
        if children.iter().all(|child| {
            let mut path = parent.to_vec();
            path.push(*child);
            initialization_is_covered(covered, &path)
        }) {
            covered.retain(|path| !path_is_prefix(parent, path));
            covered.insert(parent.to_vec());
        }
    }
}

pub(super) fn intersect_two_assignment_states(
    function: &Function,
    program: &Program,
    lhs: AssignmentState,
    rhs: AssignmentState,
) -> AssignmentState {
    let locals = lhs
        .locals
        .into_iter()
        .zip(rhs.locals)
        .enumerate()
        .map(|(index, (lhs, rhs))| {
            let mut covered = HashSet::new();
            for left in &lhs.covered {
                for right in &rhs.covered {
                    if path_is_prefix(left, right) {
                        covered.insert(right.clone());
                    } else if path_is_prefix(right, left) {
                        covered.insert(left.clone());
                    }
                }
            }
            if let Some(local_ty) = function.locals.get(index).map(|local| local.ty) {
                let paths = covered.iter().cloned().collect::<Vec<_>>();
                for path in paths {
                    normalize_initialization_coverage(program, local_ty, &mut covered, &path);
                }
            }
            let process_frame = lhs.process_frame
                && rhs.process_frame
                && covered.iter().any(|path| path.is_empty());
            LocalInitialization {
                covered,
                process_frame,
            }
        })
        .collect();
    AssignmentState { locals }
}

pub(super) fn merge_assignment_fallthrough(
    function: &Function,
    program: &Program,
    lhs: Option<AssignmentState>,
    rhs: Option<AssignmentState>,
) -> Option<AssignmentState> {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => {
            Some(intersect_two_assignment_states(function, program, lhs, rhs))
        }
        (Some(state), None) | (None, Some(state)) => Some(state),
        (None, None) => None,
    }
}

pub(super) fn intersect_assignment_states(
    function: &Function,
    program: &Program,
    states: impl IntoIterator<Item = AssignmentState>,
) -> Option<AssignmentState> {
    let mut states = states.into_iter();
    let first = states.next()?;
    Some(states.fold(first, |lhs, rhs| {
        intersect_two_assignment_states(function, program, lhs, rhs)
    }))
}

pub(super) fn find_call_cycle(program: &Program) -> Option<Vec<usize>> {
    let mut edges = Vec::<Vec<usize>>::with_capacity(program.functions.len());
    for function in &program.functions {
        let mut callees = Vec::new();
        collect_block_callees(&function.body, program.functions.len(), &mut callees);
        callees.sort_unstable();
        callees.dedup();
        edges.push(callees);
    }

    let mut visits = vec![0_u8; edges.len()];
    let mut path = Vec::new();
    for function in 0..edges.len() {
        if visits[function] == 0 {
            if let Some(cycle) = find_call_cycle_from(function, &edges, &mut visits, &mut path) {
                return Some(cycle);
            }
        }
    }
    None
}

pub(super) fn find_call_cycle_from(
    function: usize,
    edges: &[Vec<usize>],
    visits: &mut [u8],
    path: &mut Vec<usize>,
) -> Option<Vec<usize>> {
    if visits[function] == 2 {
        return None;
    }
    if visits[function] == 1 {
        let start = path
            .iter()
            .position(|candidate| *candidate == function)
            .unwrap_or(0);
        let mut cycle = path[start..].to_vec();
        cycle.push(function);
        return Some(cycle);
    }

    visits[function] = 1;
    path.push(function);
    for callee in &edges[function] {
        if let Some(cycle) = find_call_cycle_from(*callee, edges, visits, path) {
            return Some(cycle);
        }
    }
    path.pop();
    visits[function] = 2;
    None
}

pub(super) fn collect_block_callees(
    block: &Block,
    function_count: usize,
    callees: &mut Vec<usize>,
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Call { function, .. } => {
                if function.index() < function_count {
                    callees.push(function.index());
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_callees(then_block, function_count, callees);
                collect_block_callees(else_block, function_count, callees);
            }
            StatementKind::Loop { body } => {
                collect_block_callees(body, function_count, callees);
            }
            _ => {}
        }
    }
}

pub(super) fn block_contains_publication(block: &Block) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.kind {
            StatementKind::PublishDelegate { .. } => true,
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => block_contains_publication(then_block) || block_contains_publication(else_block),
            StatementKind::Loop { body } => block_contains_publication(body),
            _ => false,
        })
}

pub(super) fn passing_mode_name(mode: crate::PassingMode) -> &'static str {
    match mode {
        crate::PassingMode::Value => "value",
        crate::PassingMode::ReadOnlyReference => "read-only reference",
        crate::PassingMode::ReadWriteReference => "read-write reference",
    }
}

pub(super) fn logical_scalar_bytes(scalar: crate::ScalarType) -> u64 {
    match scalar {
        crate::ScalarType::F32 | crate::ScalarType::I32 => 4,
        crate::ScalarType::F64 | crate::ScalarType::I64 => 8,
        crate::ScalarType::Bool => 1,
    }
}

pub(super) fn buffer_static_channel_validation_error(
    channels: u32,
    element: crate::ScalarType,
) -> Option<String> {
    let maximum = (i32::MAX as u64) / logical_scalar_bytes(element);
    if channels == 0 {
        Some("has a zero-channel static buffer layout".to_owned())
    } else if u64::from(channels) > maximum {
        Some(format!(
            "static channel count exceeds the signed i32 buffer byte-extent limit; maximum is {maximum}"
        ))
    } else {
        None
    }
}

pub(super) fn scalar_sequence_fits_i32_bytes(len: usize, scalar: crate::ScalarType) -> bool {
    u64::try_from(len)
        .ok()
        .and_then(|len| len.checked_mul(logical_scalar_bytes(scalar)))
        .is_some_and(|bytes| bytes <= i32::MAX as u64)
}

pub(super) fn access_permits(source: crate::AccessMode, requested: crate::AccessMode) -> bool {
    source == crate::AccessMode::ReadWrite || requested == crate::AccessMode::ReadOnly
}

pub(super) fn buffer_channels_accept(
    expected: crate::BufferChannels,
    actual: crate::BufferChannels,
) -> bool {
    match expected {
        crate::BufferChannels::Dynamic => true,
        crate::BufferChannels::Mono => matches!(
            actual,
            crate::BufferChannels::Mono | crate::BufferChannels::Static(1)
        ),
        crate::BufferChannels::Static(1) => matches!(
            actual,
            crate::BufferChannels::Mono | crate::BufferChannels::Static(1)
        ),
        crate::BufferChannels::Static(expected) => {
            actual == crate::BufferChannels::Static(expected)
        }
    }
}

pub(super) fn validate_float_param_control_grid(
    scalar: crate::ScalarType,
    min: f64,
    max: f64,
    default: f64,
    step: f64,
    step_count: u32,
) -> Option<String> {
    if !step.is_finite() || step <= 0.0 {
        return Some("step must be finite and greater than zero".to_owned());
    }
    if crate::validated_step_count(scalar, min, max, step) != Some(step_count) {
        return Some("step_count does not match the range and step".to_owned());
    }
    if !crate::value_is_on_step_grid(scalar, min, default, step, step_count) {
        return Some("default is not on the step grid".to_owned());
    }
    None
}

pub(super) fn intrinsic_arity(intrinsic: crate::Intrinsic) -> usize {
    match intrinsic {
        crate::Intrinsic::Sin
        | crate::Intrinsic::Cos
        | crate::Intrinsic::Tan
        | crate::Intrinsic::Tanh
        | crate::Intrinsic::Atan
        | crate::Intrinsic::Exp
        | crate::Intrinsic::Log
        | crate::Intrinsic::Sqrt
        | crate::Intrinsic::Abs
        | crate::Intrinsic::Floor
        | crate::Intrinsic::Ceil
        | crate::Intrinsic::Round
        | crate::Intrinsic::Trunc => 1,
        crate::Intrinsic::Atan2
        | crate::Intrinsic::Pow
        | crate::Intrinsic::Min
        | crate::Intrinsic::Max => 2,
        crate::Intrinsic::Fma => 3,
        crate::Intrinsic::RangeClamp | crate::Intrinsic::RangeWrap => 3,
    }
}

pub(super) fn intrinsic_name(intrinsic: crate::Intrinsic) -> &'static str {
    match intrinsic {
        crate::Intrinsic::Sin => "sin",
        crate::Intrinsic::Cos => "cos",
        crate::Intrinsic::Tan => "tan",
        crate::Intrinsic::Tanh => "tanh",
        crate::Intrinsic::Atan => "atan",
        crate::Intrinsic::Atan2 => "atan2",
        crate::Intrinsic::Exp => "exp",
        crate::Intrinsic::Log => "log",
        crate::Intrinsic::Sqrt => "sqrt",
        crate::Intrinsic::Pow => "pow",
        crate::Intrinsic::Abs => "abs",
        crate::Intrinsic::Floor => "floor",
        crate::Intrinsic::Ceil => "ceil",
        crate::Intrinsic::Round => "round",
        crate::Intrinsic::Trunc => "trunc",
        crate::Intrinsic::Min => "min",
        crate::Intrinsic::Max => "max",
        crate::Intrinsic::Fma => "fma",
        crate::Intrinsic::RangeClamp => "range_clamp",
        crate::Intrinsic::RangeWrap => "range_wrap",
    }
}
