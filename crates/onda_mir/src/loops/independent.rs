//! Dependence proofs for bounded counted loops over owned arrays. Helpers are
//! interpreted with symbolic arguments, without inlining or changing MIR.
//! Unknown storage, wrapping addresses and unsupported control flow fail closed.
use crate::*;
use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Affine {
    slope: i64,
    offset: i64,
}

impl Affine {
    fn constant(offset: i64) -> Self {
        Self { slope: 0, offset }
    }
    fn at(self, index: i64) -> Option<i64> {
        self.slope.checked_mul(index)?.checked_add(self.offset)
    }
    fn range(self, domain: (i64, i64)) -> Option<(i64, i64)> {
        let a = self.at(domain.0)?;
        let b = self.at(domain.1)?;
        Some((a.min(b), a.max(b)))
    }
    fn binary(self, op: BinaryOp, rhs: Self) -> Option<Self> {
        Some(match op {
            BinaryOp::Add => Self {
                slope: self.slope.checked_add(rhs.slope)?,
                offset: self.offset.checked_add(rhs.offset)?,
            },
            BinaryOp::Subtract => Self {
                slope: self.slope.checked_sub(rhs.slope)?,
                offset: self.offset.checked_sub(rhs.offset)?,
            },
            BinaryOp::Multiply if rhs.slope == 0 => Self {
                slope: self.slope.checked_mul(rhs.offset)?,
                offset: self.offset.checked_mul(rhs.offset)?,
            },
            BinaryOp::Multiply if self.slope == 0 => return rhs.binary(op, self),
            _ => return None,
        })
    }

    fn disjoint_residues(self, rhs: Self) -> bool {
        let (mut a, mut b) = (self.slope.unsigned_abs(), rhs.slope.unsigned_abs());
        while b != 0 {
            (a, b) = (b, a % b);
        }
        // An intersection requires a*i - b*j = offset_b - offset_a.
        // Different residue classes cannot intersect, even when their full
        // ranges overlap (for example interleaved even and odd channels).
        a != 0 && (i128::from(self.offset) - i128::from(rhs.offset)) % i128::from(a) != 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Object {
    State(StateId),
    Constant(ConstDataId),
    Reference(usize),
}

#[derive(Clone, Copy)]
struct Region {
    object: Object,
    offset: i64,
    len: i64,
}

impl Region {
    fn window(self, start: i64, len: i64) -> Option<Self> {
        (start >= 0 && len >= 0 && start <= self.len && len <= self.len - start).then_some(Self {
            object: self.object,
            offset: self.offset + start,
            len,
        })
    }
}

#[derive(Clone, Copy)]
enum Symbol {
    Unknown,
    Index(Affine),
    Region(Region),
}

impl Symbol {
    fn index(self) -> Option<Affine> {
        if let Self::Index(index) = self {
            Some(index)
        } else {
            None
        }
    }
    fn constant(self) -> Option<i64> {
        self.index()
            .filter(|index| index.slope == 0)
            .map(|index| index.offset)
    }
    fn region(self) -> Option<Region> {
        if let Self::Region(region) = self {
            Some(region)
        } else {
            None
        }
    }
}

struct Frame<'a> {
    function: &'a Function,
    parameters: Cow<'a, [Symbol]>,
    initial_locals: Option<&'a HashMap<LocalId, Symbol>>,
    locals: HashMap<LocalId, Symbol>,
}

impl Frame<'_> {
    fn local(&self, local: LocalId) -> Symbol {
        self.locals
            .get(&local)
            .or_else(|| self.initial_locals.and_then(|locals| locals.get(&local)))
            .copied()
            .unwrap_or(Symbol::Unknown)
    }

    fn value(&self, value: Value) -> Symbol {
        match value {
            Value::Local(local) => self.local(local),
            Value::Constant(ScalarValue::I32(value)) => {
                Symbol::Index(Affine::constant(i64::from(value)))
            }
            Value::Constant(ScalarValue::I64(value)) => Symbol::Index(Affine::constant(value)),
            _ => Symbol::Unknown,
        }
    }
}

pub(super) struct MemoryFacts {
    parameters: Vec<Symbol>,
    locals: HashMap<LocalId, Symbol>,
    reference_regions: Vec<Vec<StateRegion>>,
}

impl MemoryFacts {
    fn new(program: &Program, memory: &MemoryAnalysis, id: FunctionId) -> Self {
        let function = &program.functions[id.index()];
        let mut facts = Self {
            parameters: vec![Symbol::Unknown; function.params.len()],
            locals: HashMap::new(),
            reference_regions: Vec::new(),
        };
        for (index, parameter) in function.params.iter().enumerate() {
            if !matches!(
                (&program.types[parameter.ty.index()], parameter.mode),
                (
                    Type::Array { .. },
                    PassingMode::ReadOnlyReference
                        | PassingMode::ReadWriteReference
                        | PassingMode::ResultReference
                ) | (Type::Slice { .. }, PassingMode::Value)
            ) {
                continue;
            }
            let origins = memory.parameter(id, ParameterId::new(index as u32));
            if let Some(region) =
                known_region(program, parameter.ty, origins, &mut facts.reference_regions)
            {
                facts.parameters[index] = Symbol::Region(region);
            }
        }
        // Slice descriptors are commonly loaded before the loop. Whole-function
        // origins include every definition and discard writable descriptor escapes,
        // so the alternatives cover every view regardless of where it was assigned.
        for (index, local) in function.locals.iter().enumerate() {
            if !matches!(program.types[local.ty.index()], Type::Slice { .. }) {
                continue;
            }
            let origins = memory
                .slice(id, Value::Local(LocalId::new(index as u32)))
                .expect("memory analysis covers every local");
            if let Some(region) =
                known_region(program, local.ty, origins, &mut facts.reference_regions)
            {
                facts
                    .locals
                    .insert(LocalId::new(index as u32), Symbol::Region(region));
            }
        }
        facts
    }
}

struct Access {
    object: Object,
    index: Affine,
    write: bool,
}

struct Proof<'a> {
    program: &'a Program,
    effects: &'a EffectAnalysis,
    reference_regions: &'a [Vec<StateRegion>],
    domain: (i64, i64),
    accesses: Vec<Access>,
    calls: Vec<FunctionId>,
    remaining: usize,
}

pub(super) fn prove(analysis: &super::LoopAnalysis<'_>, body: &Block) -> Option<()> {
    let super::LoopAnalysis {
        program,
        effects,
        ranges,
        function: id,
        ..
    } = analysis;
    let function = &program.functions[id.index()];
    let counted = super::counted_loop(body)?;
    // A whole-function upper bound covers every possible trip count, even
    // when the limit changes. With a nonnegative start and unit increment,
    // the i32 counter cannot wrap before reaching this exclusive bound.
    let limit = match counted.limit {
        Value::Constant(ScalarValue::I32(limit)) => i64::from(limit),
        Value::Local(local) => {
            let range = ranges.function(*id)?.local(local)?;
            if range.scalar() != ScalarType::I32 {
                return None;
            }
            range.max()
        }
        _ => return None,
    };
    let start = analysis.facts.preceding_reset(body)?;
    if start < 0 || i64::from(start) >= limit {
        return None;
    }
    let facts = analysis
        .memory_facts
        .get_or_init(|| MemoryFacts::new(program, analysis.memory, *id));
    let mut frame = Frame {
        function,
        parameters: Cow::Borrowed(&facts.parameters),
        initial_locals: Some(&facts.locals),
        locals: HashMap::from([(
            counted.index,
            Symbol::Index(Affine {
                slope: 1,
                offset: 0,
            }),
        )]),
    };
    let mut proof = Proof {
        program,
        effects,
        reference_regions: &facts.reference_regions,
        domain: (i64::from(start), limit - 1),
        accesses: Vec::new(),
        calls: vec![*id],
        remaining: 4096,
    };
    for statement in counted.body {
        if matches!(&statement.kind, StatementKind::Assign { destination: Place { base: PlaceBase::Local(local), .. }, .. } if *local == counted.index)
            || matches!(&statement.kind, StatementKind::Call { results, .. } if results.contains(&counted.index))
        {
            return None;
        }
    }
    if proof.statements(&mut frame, counted.body)?.is_some() {
        return None;
    }
    if !proof.accesses.iter().any(|access| access.write) {
        return None;
    }
    for (i, a) in proof.accesses.iter().enumerate() {
        for b in &proof.accesses[i..] {
            if !(a.write || b.write) || a.object != b.object {
                continue;
            }
            if a.index == b.index && a.index.slope != 0 {
                continue;
            }
            if a.index.disjoint_residues(b.index) {
                continue;
            }
            let (alo, ahi) = a.index.range(proof.domain)?;
            let (blo, bhi) = b.index.range(proof.domain)?;
            if ahi >= blo && bhi >= alo {
                return None;
            }
        }
    }
    Some(())
}

fn known_region(
    program: &Program,
    ty: TypeId,
    origins: &StorageOrigins,
    references: &mut Vec<Vec<StateRegion>>,
) -> Option<Region> {
    let regions = if let Some(regions) = origins.state_regions() {
        regions.collect::<Vec<_>>()
    } else if let Type::Array { len, .. } = program.types[ty.index()] {
        // A successful complete-object reference starts at the backing base,
        // even when the incoming descriptor's range itself is unresolved.
        origins
            .only_states()?
            .map(|state| {
                (program.state[state.index()].ty == ty).then_some(StateRegion {
                    state,
                    start: 0,
                    len,
                })
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        return None;
    };
    let (element, len) = match program.types[ty.index()] {
        Type::Array { element, len } => {
            let Type::Scalar(element) = program.types[element.index()] else {
                return None;
            };
            (element, len)
        }
        Type::Slice { element, .. } => (element, regions.first()?.len),
        _ => return None,
    };
    if regions.iter().any(|region| {
        region.len != len || !matches!(program.types[program.state[region.state.index()].ty.index()],
            Type::Array { element: backing, .. } if program.types[backing.index()] == Type::Scalar(element))
    }) { return None; }
    let object = Object::Reference(references.len());
    references.push(regions);
    Some(Region {
        object,
        offset: 0,
        len: i64::from(len),
    })
}

impl<'a> Proof<'a> {
    fn array(&self, state: StateId) -> Option<Region> {
        let Type::Array { element, len } =
            self.program.types[self.program.state[state.index()].ty.index()]
        else {
            return None;
        };
        if !matches!(self.program.types[element.index()], Type::Scalar(_)) {
            return None;
        }
        Some(Region {
            object: Object::State(state),
            offset: 0,
            len: i64::from(len),
        })
    }
    fn constant_data(&self, data: ConstDataId) -> Region {
        Region {
            object: Object::Constant(data),
            offset: 0,
            len: self.program.const_data[data.index()].values.len() as i64,
        }
    }
    fn base(&self, frame: &Frame<'_>, base: &PlaceBase) -> Option<Symbol> {
        Some(match base {
            PlaceBase::Local(local) => frame.local(*local),
            PlaceBase::Parameter(parameter) => frame.parameters[parameter.index()],
            PlaceBase::State(state) => Symbol::Region(self.array(*state)?),
            _ => return None,
        })
    }
    fn region(&self, frame: &Frame<'_>, place: &Place) -> Option<Region> {
        if !place.projections.is_empty() {
            return None;
        }
        self.base(frame, &place.base)?.region()
    }
    fn access(&mut self, region: Region, index: Affine, write: bool) -> Option<()> {
        let (low, high) = index.range(self.domain)?;
        if low < 0 || high >= region.len || (write && matches!(region.object, Object::Constant(_)))
        {
            return None;
        }
        let index = index.binary(BinaryOp::Add, Affine::constant(region.offset))?;
        if let Object::Reference(reference) = region.object {
            let origins = &self.reference_regions[reference];
            self.remaining = self.remaining.checked_sub(origins.len())?;
            for origin in origins {
                self.accesses.push(Access {
                    object: Object::State(origin.state),
                    index: index
                        .binary(BinaryOp::Add, Affine::constant(i64::from(origin.start)))?,
                    write,
                });
            }
        } else {
            self.accesses.push(Access {
                object: region.object,
                index,
                write,
            });
        }
        Some(())
    }
    fn place_access(&mut self, frame: &Frame<'_>, place: &Place, write: bool) -> Option<()> {
        let [Projection::Index { index, .. }] = place.projections.as_slice() else {
            return None;
        };
        self.access(
            self.base(frame, &place.base)?.region()?,
            frame.value(*index).index()?,
            write,
        )
    }
    fn within_type(&self, index: Affine, ty: TypeId) -> Option<Symbol> {
        let (low, high) = index.range(self.domain)?;
        match self.program.types[ty.index()] {
            Type::Scalar(ScalarType::I32)
                if low >= i64::from(i32::MIN) && high <= i64::from(i32::MAX) =>
            {
                Some(Symbol::Index(index))
            }
            Type::Scalar(ScalarType::I64) => Some(Symbol::Index(index)),
            _ => None,
        }
    }
    fn rvalue(&mut self, frame: &Frame<'_>, value: &Rvalue, ty: TypeId) -> Option<Symbol> {
        Some(match value {
            Rvalue::Use(value) => frame.value(*value),
            Rvalue::Load(place)
                if place.projections.is_empty() && matches!(place.base, PlaceBase::Local(_)) =>
            {
                self.base(frame, &place.base)?
            }
            Rvalue::Load(Place {
                base: PlaceBase::Parameter(parameter),
                projections,
            }) if projections.is_empty()
                && frame.function.params[parameter.index()].mode == PassingMode::Value =>
            {
                frame.parameters[parameter.index()]
            }
            Rvalue::Load(place) => {
                self.place_access(frame, place, false)?;
                Symbol::Unknown
            }
            Rvalue::MakeSlice {
                source, start, len, ..
            } => {
                let region = match source {
                    SliceSource::Place(place) => self.region(frame, place)?,
                    SliceSource::ConstData(data) => self.constant_data(*data),
                    _ => return None,
                };
                Symbol::Region(region.window(
                    frame.value(*start).constant()?,
                    frame.value(*len).constant()?,
                )?)
            }
            Rvalue::SliceLen(slice) => {
                Symbol::Index(Affine::constant(frame.value(*slice).region()?.len))
            }
            Rvalue::SliceLoad { slice, index, .. } => {
                self.access(
                    frame.value(*slice).region()?,
                    frame.value(*index).index()?,
                    false,
                )?;
                Symbol::Unknown
            }
            Rvalue::ConstDataLoad { data, index, .. } => {
                self.access(
                    self.constant_data(*data),
                    frame.value(*index).index()?,
                    false,
                )?;
                Symbol::Unknown
            }
            Rvalue::Binary { op, lhs, rhs } => {
                if matches!(
                    self.program.types[ty.index()],
                    Type::Scalar(ScalarType::F32 | ScalarType::F64)
                ) {
                    Symbol::Unknown
                } else if let (Some(lhs), Some(rhs)) =
                    (frame.value(*lhs).index(), frame.value(*rhs).index())
                {
                    self.within_type(lhs.binary(*op, rhs)?, ty)?
                } else {
                    return None;
                }
            }
            Rvalue::Unary {
                op: UnaryOp::Negate,
                operand,
            } if frame.value(*operand).index().is_some() => self.within_type(
                Affine::constant(0).binary(BinaryOp::Subtract, frame.value(*operand).index()?)?,
                ty,
            )?,
            Rvalue::Cast {
                value,
                to: ScalarType::I32 | ScalarType::I64,
            } if frame.value(*value).index().is_some() => {
                self.within_type(frame.value(*value).index()?, ty)?
            }
            Rvalue::NormalizeIndex { index, length, .. } => {
                let index = frame.value(*index).index()?;
                let (low, high) = index.range(self.domain)?;
                if low < 0 || high >= frame.value(*length).constant()? {
                    return None;
                }
                Symbol::Index(index)
            }
            Rvalue::Unary { .. }
            | Rvalue::Compare { .. }
            | Rvalue::Cast { .. }
            | Rvalue::Intrinsic { .. } => Symbol::Unknown,
            _ => return None,
        })
    }
    fn argument(
        &self,
        frame: &Frame<'_>,
        argument: &CallArgument,
        parameter: &FunctionParam,
    ) -> Option<Symbol> {
        if parameter.mode != PassingMode::Value {
            let Type::Array { element, len } = self.program.types[parameter.ty.index()] else {
                return None;
            };
            if !matches!(self.program.types[element.index()], Type::Scalar(_)) {
                return None;
            }
            let (region, start) = match argument {
                CallArgument::Place(place) => (self.region(frame, place)?, 0),
                CallArgument::SliceWindow { slice, start, .. } => (
                    frame.value(*slice).region()?,
                    frame.value(*start).constant()?,
                ),
                CallArgument::ArrayWindow { array, start, .. } => {
                    (self.region(frame, array)?, frame.value(*start).constant()?)
                }
                _ => return None,
            };
            return Some(Symbol::Region(region.window(start, i64::from(len))?));
        }
        match argument {
            CallArgument::Value(value) => Some(frame.value(*value)),
            _ => None,
        }
    }
    fn statements(
        &mut self,
        frame: &mut Frame<'a>,
        statements: &[Statement],
    ) -> Option<Option<Vec<Symbol>>> {
        for statement in statements {
            self.remaining = self.remaining.checked_sub(1)?;
            match &statement.kind {
                StatementKind::Assign { destination, value } => {
                    if let Place {
                        base: PlaceBase::Local(local),
                        projections,
                    } = destination
                    {
                        if projections.is_empty() {
                            let ty = frame.function.locals[local.index()].ty;
                            if !matches!(
                                self.program.types[ty.index()],
                                Type::Scalar(_) | Type::Slice { .. }
                            ) {
                                return None;
                            }
                            let value = self.rvalue(frame, value, ty)?;
                            frame.locals.insert(*local, value);
                            continue;
                        }
                    }
                    let ty = self.program.place_type(frame.function, destination)?;
                    self.rvalue(frame, value, ty)?;
                    self.place_access(frame, destination, true)?;
                }
                StatementKind::SliceStore { slice, index, .. } => self.access(
                    frame.value(*slice).region()?,
                    frame.value(*index).index()?,
                    true,
                )?,
                StatementKind::Call {
                    function,
                    args,
                    results,
                } => {
                    let effects = self.effects.function(*function);
                    if effects.may_fail
                        || effects.may_not_return
                        || self.calls.contains(function)
                        || self.calls.len() >= 32
                    {
                        return None;
                    }
                    let callee = &self.program.functions[function.index()];
                    let parameters = args
                        .iter()
                        .zip(&callee.params)
                        .map(|(argument, parameter)| self.argument(frame, argument, parameter))
                        .collect::<Option<Vec<_>>>()?;
                    let mut called = Frame {
                        function: callee,
                        parameters: Cow::Owned(parameters),
                        initial_locals: None,
                        locals: HashMap::new(),
                    };
                    self.calls.push(*function);
                    let returned = self
                        .statements(&mut called, &callee.body.statements)?
                        .unwrap_or_default();
                    self.calls.pop();
                    if returned.len() != results.len() {
                        return None;
                    }
                    for (local, value) in results.iter().zip(returned) {
                        frame.locals.insert(*local, value);
                    }
                }
                StatementKind::Return { values } => {
                    return Some(Some(
                        values.iter().map(|value| frame.value(*value)).collect(),
                    ))
                }
                _ => return None,
            }
        }
        Some(None)
    }
}
