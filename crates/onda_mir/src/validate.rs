use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Deref;

use crate::{
    Block, CallArgument, Function, FunctionId, FunctionKind, FunctionOrigin, Place, PlaceBase,
    Program, Rvalue, SliceSource, SourceSpan, StatementKind, Type, Value, MIR_SCHEMA_VERSION,
    PROCESS_PARAM_COUNT, PROCESS_PARAM_NAMES,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ValidationError {
    pub message: String,
    pub function: Option<FunctionId>,
    pub source: SourceSpan,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(function) = self.function {
            write!(f, "MIR function {}: {}", function.raw(), self.message)
        } else {
            write!(f, "MIR: {}", self.message)
        }
    }
}

impl std::error::Error for ValidationError {}

/// An owned MIR program that has passed backend-neutral structural validation
/// and carries the provenance of any producer-proved unchecked accesses or
/// integer invariants.
///
/// Backend-specific capability/legalization checks remain the backend's
/// responsibility. The inner program is intentionally immutable so this type
/// cannot silently lose its validation guarantee.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProducerProofStatus {
    Absent,
    Trusted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedProgram {
    program: Program,
    producer_proofs: ProducerProofStatus,
}

impl ValidatedProgram {
    pub fn as_program(&self) -> &Program {
        &self.program
    }

    pub fn into_program(self) -> Program {
        self.program
    }

    pub(crate) fn from_validated(program: Program, producer_proofs: ProducerProofStatus) -> Self {
        Self {
            program,
            producer_proofs,
        }
    }

    pub(crate) const fn producer_proofs(&self) -> ProducerProofStatus {
        self.producer_proofs
    }
}

impl Deref for ValidatedProgram {
    type Target = Program;

    fn deref(&self) -> &Self::Target {
        self.as_program()
    }
}

impl AsRef<Program> for ValidatedProgram {
    fn as_ref(&self) -> &Program {
        self.as_program()
    }
}

impl TryFrom<Program> for ValidatedProgram {
    type Error = Vec<ValidationError>;

    fn try_from(program: Program) -> Result<Self, Self::Error> {
        validate_owned(program)
    }
}

pub fn validate(program: &Program) -> Result<(), Vec<ValidationError>> {
    validate_with_proof_status(program, ProducerProofStatus::Absent)
}

/// Validates MIR emitted by a trusted producer that has proved its unchecked
/// accesses and declared integer invariants in its source-level lowering logic.
///
/// # Safety
///
/// Every unchecked index, slice, and reference window in `program` must be in
/// bounds for every execution reaching it. Backends may lower those operations
/// without runtime checks. Every [`crate::IntegerRangeInvariant`] attached to a
/// state slot, function parameter, or local must also contain every value
/// observable from that storage. This includes values supplied by callers or
/// restored from external state. Backends may use those invariants as hard
/// optimization assumptions without inserting normalization or checks. Every
/// pinned state slot must be fully overwritten by the init entry on every
/// successful full-initialization path before it can be observed. Backends may
/// omit those slots from the physical state pre-clear.
pub unsafe fn validate_with_producer_proofs(program: &Program) -> Result<(), Vec<ValidationError>> {
    validate_with_proof_status(program, ProducerProofStatus::Trusted)
}

fn validate_with_proof_status(
    program: &Program,
    producer_proofs: ProducerProofStatus,
) -> Result<(), Vec<ValidationError>> {
    let mut validator = Validator {
        program,
        errors: Vec::new(),
        producer_proofs,
    };
    validator.run();
    if validator.errors.is_empty() {
        Ok(())
    } else {
        Err(validator.errors)
    }
}

pub fn validate_owned(program: Program) -> Result<ValidatedProgram, Vec<ValidationError>> {
    validate(&program)?;
    Ok(ValidatedProgram::from_validated(
        program,
        ProducerProofStatus::Absent,
    ))
}

/// Takes ownership of MIR emitted by a trusted producer after structural
/// validation while retaining the producer's proofs.
///
/// # Safety
///
/// Every unchecked index, slice, and reference window in `program` must be in
/// bounds for every execution reaching it. Every
/// [`crate::IntegerRangeInvariant`] attached to a state slot, function
/// parameter, or local must contain every value observable from that storage,
/// including values supplied by callers or restored from external state. Every
/// pinned state slot must be fully overwritten by the init entry on every
/// successful full-initialization path before it can be observed.
pub unsafe fn validate_owned_with_producer_proofs(
    program: Program,
) -> Result<ValidatedProgram, Vec<ValidationError>> {
    unsafe { validate_with_producer_proofs(&program)? };
    Ok(ValidatedProgram::from_validated(
        program,
        ProducerProofStatus::Trusted,
    ))
}

pub(crate) fn revalidate_owned(
    program: Program,
    producer_proofs: ProducerProofStatus,
) -> Result<ValidatedProgram, Vec<ValidationError>> {
    validate_with_proof_status(&program, producer_proofs)?;
    Ok(ValidatedProgram::from_validated(program, producer_proofs))
}

struct Validator<'a> {
    program: &'a Program,
    errors: Vec<ValidationError>,
    producer_proofs: ProducerProofStatus,
}

#[derive(Clone, Copy, Debug, Default)]
struct ControlFlow {
    falls_through: bool,
    returns: bool,
    breaks: bool,
    continues: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum InitProjection {
    Field(u32),
    Index(u32),
}

#[derive(Debug, Clone, Default)]
struct LocalInitialization {
    covered: HashSet<Vec<InitProjection>>,
    process_frame: bool,
}

#[derive(Debug, Clone)]
struct AssignmentState {
    locals: Vec<LocalInitialization>,
}

impl AssignmentState {
    fn new(local_count: usize) -> Self {
        Self {
            locals: vec![LocalInitialization::default(); local_count],
        }
    }
}

#[derive(Debug, Default)]
struct AssignmentFlow {
    fallthrough: Option<AssignmentState>,
    breaks: Vec<AssignmentState>,
    continues: Vec<AssignmentState>,
}

impl ControlFlow {
    fn fallthrough() -> Self {
        Self {
            falls_through: true,
            ..Self::default()
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            falls_through: self.falls_through || other.falls_through,
            returns: self.returns || other.returns,
            breaks: self.breaks || other.breaks,
            continues: self.continues || other.continues,
        }
    }
}

fn block_control_flow(block: &Block) -> ControlFlow {
    let mut flow = ControlFlow::fallthrough();
    for statement in &block.statements {
        let statement_is_reachable = flow.falls_through;
        flow.falls_through = false;
        if statement_is_reachable {
            flow = flow.union(statement_control_flow(&statement.kind));
        }
    }
    flow
}

fn statement_control_flow(statement: &StatementKind) -> ControlFlow {
    match statement {
        StatementKind::If {
            then_block,
            else_block,
            ..
        } => block_control_flow(then_block).union(block_control_flow(else_block)),
        StatementKind::Loop { body } => {
            let body_flow = block_control_flow(body);
            ControlFlow {
                // A structured MIR loop repeats on body fallthrough or
                // `continue`; only a reachable `break` reaches the statement
                // following the loop.
                falls_through: body_flow.breaks,
                returns: body_flow.returns,
                breaks: false,
                continues: false,
            }
        }
        StatementKind::Break => ControlFlow {
            breaks: true,
            ..ControlFlow::default()
        },
        StatementKind::Continue => ControlFlow {
            continues: true,
            ..ControlFlow::default()
        },
        StatementKind::Return { .. } => ControlFlow {
            returns: true,
            ..ControlFlow::default()
        },
        _ => ControlFlow::fallthrough(),
    }
}

fn statement_uses_unchecked_bounds(statement: &StatementKind) -> bool {
    match statement {
        StatementKind::Assign { destination, value } => {
            place_uses_unchecked_bounds(destination) || rvalue_uses_unchecked_bounds(value)
        }
        StatementKind::Call { args, .. } | StatementKind::PublishDelegate { args, .. } => {
            args.iter().any(call_arg_uses_unchecked_bounds)
        }
        StatementKind::PublishLog { .. } => false,
        StatementKind::BufferStore { buffer, bounds, .. } => {
            *bounds == crate::BoundsMode::Unchecked || buffer_ref_uses_unchecked_bounds(*buffer)
        }
        StatementKind::BufferParamStore {
            parameter, bounds, ..
        } => {
            *bounds == crate::BoundsMode::Unchecked
                || buffer_param_ref_uses_unchecked_bounds(*parameter)
        }
        StatementKind::OutputStore { bounds, .. }
        | StatementKind::ControlOutputStore { bounds, .. }
        | StatementKind::SliceStore { bounds, .. } => *bounds == crate::BoundsMode::Unchecked,
        StatementKind::If { .. }
        | StatementKind::Loop { .. }
        | StatementKind::SliceFill { .. }
        | StatementKind::SliceCopy { .. }
        | StatementKind::Break
        | StatementKind::Continue
        | StatementKind::Return { .. } => false,
    }
}

fn rvalue_uses_unchecked_bounds(value: &Rvalue) -> bool {
    match value {
        Rvalue::Load(place) => place_uses_unchecked_bounds(place),
        Rvalue::BufferLoad { buffer, bounds, .. } => {
            *bounds == crate::BoundsMode::Unchecked || buffer_ref_uses_unchecked_bounds(*buffer)
        }
        Rvalue::BufferParamLoad {
            parameter, bounds, ..
        } => {
            *bounds == crate::BoundsMode::Unchecked
                || buffer_param_ref_uses_unchecked_bounds(*parameter)
        }
        Rvalue::InputLoad { bounds, .. }
        | Rvalue::OutputLoad { bounds, .. }
        | Rvalue::ConstDataLoad { bounds, .. }
        | Rvalue::SliceLoad { bounds, .. } => *bounds == crate::BoundsMode::Unchecked,
        Rvalue::MakeSlice { source, bounds, .. } => {
            *bounds == crate::BoundsMode::Unchecked || slice_source_uses_unchecked_bounds(source)
        }
        Rvalue::InitAll
        | Rvalue::Use(_)
        | Rvalue::Unary { .. }
        | Rvalue::Binary { .. }
        | Rvalue::Compare { .. }
        | Rvalue::Cast { .. }
        | Rvalue::Intrinsic { .. }
        | Rvalue::ProcessFrame { .. }
        | Rvalue::SliceLen(_) => false,
        Rvalue::BufferLen(buffer)
        | Rvalue::BufferChannels(buffer)
        | Rvalue::BufferSampleRate(buffer)
        | Rvalue::BufferIsBound(buffer) => buffer_ref_uses_unchecked_bounds(*buffer),
        Rvalue::BufferParamLen(parameter)
        | Rvalue::BufferParamChannels(parameter)
        | Rvalue::BufferParamSampleRate(parameter)
        | Rvalue::BufferParamIsBound(parameter) => {
            buffer_param_ref_uses_unchecked_bounds(*parameter)
        }
    }
}

fn call_arg_uses_unchecked_bounds(argument: &CallArgument) -> bool {
    match argument {
        CallArgument::Place(place) => place_uses_unchecked_bounds(place),
        CallArgument::SliceElement { bounds, .. } | CallArgument::SliceWindow { bounds, .. } => {
            *bounds == crate::BoundsMode::Unchecked
        }
        CallArgument::ArrayWindow { array, bounds, .. } => {
            *bounds == crate::BoundsMode::Unchecked || place_uses_unchecked_bounds(array)
        }
        CallArgument::Value(_) => false,
        CallArgument::Buffer(buffer) => buffer_ref_uses_unchecked_bounds(*buffer),
        CallArgument::BufferParam(parameter) => buffer_param_ref_uses_unchecked_bounds(*parameter),
        CallArgument::BufferSpan(_) => false,
    }
}

fn buffer_ref_uses_unchecked_bounds(buffer: crate::BufferRef) -> bool {
    matches!(
        buffer,
        crate::BufferRef::ArrayElement {
            bounds: crate::BoundsMode::Unchecked,
            ..
        }
    )
}

fn buffer_param_ref_uses_unchecked_bounds(parameter: crate::BufferParamRef) -> bool {
    matches!(
        parameter,
        crate::BufferParamRef::ArrayElement {
            bounds: crate::BoundsMode::Unchecked,
            ..
        }
    )
}

fn slice_source_uses_unchecked_bounds(source: &SliceSource) -> bool {
    match source {
        SliceSource::Place(place) => place_uses_unchecked_bounds(place),
        SliceSource::Buffer { buffer, .. } => buffer_ref_uses_unchecked_bounds(*buffer),
        SliceSource::BufferParam { parameter, .. } => {
            buffer_param_ref_uses_unchecked_bounds(*parameter)
        }
        SliceSource::ConstData(_) => false,
    }
}

fn place_uses_unchecked_bounds(place: &Place) -> bool {
    place.projections.iter().any(|projection| {
        matches!(
            projection,
            crate::Projection::Index {
                bounds: crate::BoundsMode::Unchecked,
                ..
            }
        )
    })
}

impl Validator<'_> {
    fn run(&mut self) {
        if self.program.schema_version != MIR_SCHEMA_VERSION {
            self.program_error(format!(
                "schema version {} does not match compiler schema version {}",
                self.program.schema_version, MIR_SCHEMA_VERSION
            ));
        }
        if let Err(error) = self.program.config.validate() {
            self.program_error(error.to_string());
        }

        for (index, ty) in self.program.types.iter().enumerate() {
            match ty {
                Type::Tuple(elements) => {
                    for element in elements {
                        self.require_type(*element, None, SourceSpan::UNKNOWN);
                    }
                }
                Type::Array { element, len } => {
                    self.require_type(*element, None, SourceSpan::UNKNOWN);
                    if *len == 0 {
                        self.program_error(format!("type {index} is a zero-length array"));
                    }
                    if *len > i32::MAX as u32 {
                        self.program_error(format!(
                            "type {index} array length exceeds the signed i32 MIR boundary"
                        ));
                    }
                }
                Type::Struct(id) => {
                    if id.index() >= self.program.structs.len() {
                        self.program_error(format!(
                            "type {index} references missing struct {}",
                            id.raw()
                        ));
                    }
                }
                Type::Buffer {
                    element, channels, ..
                } => {
                    if let crate::BufferChannels::Static(channels) = channels {
                        if let Some(reason) =
                            buffer_static_channel_validation_error(*channels, *element)
                        {
                            self.program_error(format!("type {index} {reason}"));
                        }
                    }
                }
                Type::BufferSpan {
                    element,
                    channels,
                    len,
                    ..
                } => {
                    if *len == 0 || *len > i32::MAX as u32 {
                        self.program_error(format!(
                            "type {index} buffer span length must be in 1..={}",
                            i32::MAX
                        ));
                    }
                    if let crate::BufferChannels::Static(channels) = channels {
                        if let Some(reason) =
                            buffer_static_channel_validation_error(*channels, *element)
                        {
                            self.program_error(format!("type {index} {reason}"));
                        }
                    }
                }
                Type::Scalar(_) | Type::Slice { .. } => {}
            }
        }

        for structure in &self.program.structs {
            for field in &structure.fields {
                self.require_type(field.ty, None, SourceSpan::UNKNOWN);
            }
        }
        self.validate_fixed_aggregate_types();
        self.validate_interface_names();
        for input in &self.program.interface.inputs {
            self.require_type(input.ty, None, SourceSpan::UNKNOWN);
            self.reject_runtime_handle_storage(
                input.ty,
                None,
                SourceSpan::UNKNOWN,
                format!("input '{}'", input.name),
            );
            if let Some(default) = &input.default {
                if !self.constant_matches_type(default, input.ty) {
                    self.program_error(format!(
                        "input '{}' default does not match type {}",
                        input.name,
                        self.type_name(input.ty)
                    ));
                }
            }
            if let Some(range) = input.range {
                if let Some(reason) = self.value_range_validation_error(range, input.ty) {
                    self.program_error(format!("input '{}' range {reason}", input.name));
                } else if input
                    .default
                    .as_ref()
                    .is_some_and(|default| !self.constant_is_within_range(default, range))
                {
                    self.program_error(format!(
                        "input '{}' default is outside its declared range",
                        input.name
                    ));
                }
            }
        }
        for output in &self.program.interface.outputs {
            self.require_type(output.ty, None, SourceSpan::UNKNOWN);
            self.reject_runtime_handle_storage(
                output.ty,
                None,
                SourceSpan::UNKNOWN,
                format!("output '{}'", output.name),
            );
        }
        for output in &self.program.interface.control_outputs {
            self.require_type(output.ty, None, SourceSpan::UNKNOWN);
            self.reject_runtime_handle_storage(
                output.ty,
                None,
                SourceSpan::UNKNOWN,
                format!("control output '{}'", output.name),
            );
        }
        for param in &self.program.interface.params {
            self.require_type(param.ty, None, SourceSpan::UNKNOWN);
            self.reject_runtime_handle_storage(
                param.ty,
                None,
                SourceSpan::UNKNOWN,
                format!("parameter '{}'", param.name),
            );
            if !self.constant_matches_type(&param.default, param.ty) {
                self.program_error(format!(
                    "parameter '{}' default does not match type {}",
                    param.name,
                    self.type_name(param.ty)
                ));
            }
            if let Some(range) = param.range {
                if let Some(reason) = self.value_range_validation_error(range, param.ty) {
                    self.program_error(format!("parameter '{}' range {reason}", param.name));
                } else if !self.constant_is_within_range(&param.default, range) {
                    self.program_error(format!(
                        "parameter '{}' default is outside its declared range",
                        param.name
                    ));
                } else if let Some(reason) = self.param_control_validation_error(param, range) {
                    self.program_error(format!("parameter '{}' control {reason}", param.name));
                }
            } else if param.control != crate::ParamControl::default() {
                self.program_error(format!(
                    "parameter '{}' has control metadata without a range",
                    param.name
                ));
            }
        }
        for state in &self.program.state {
            self.require_type(state.ty, None, SourceSpan::UNKNOWN);
            let storage = match state.persistence {
                crate::StatePersistence::Snapshot => "snapshot state",
                crate::StatePersistence::InstanceScratch => "instance-scratch state",
                crate::StatePersistence::ControlMirror => "control-mirror state",
            };
            self.reject_runtime_handle_storage(
                state.ty,
                None,
                SourceSpan::UNKNOWN,
                format!("{storage} '{}'", state.name),
            );
            if state.pinned && self.producer_proofs == ProducerProofStatus::Absent {
                self.program_error(format!(
                    "{storage} '{}' pinned initialization requires trusted producer validation",
                    state.name
                ));
            }
            if let Some(range) = state.integer_range {
                if self.producer_proofs == ProducerProofStatus::Absent {
                    self.program_error(format!(
                        "{storage} '{}' integer range requires trusted producer validation",
                        state.name
                    ));
                }
                if let Some(reason) = self.integer_range_validation_error(range, state.ty) {
                    self.program_error(format!(
                        "{storage} '{}' integer range {reason}",
                        state.name
                    ));
                }
            }
        }
        self.validate_control_output_mirrors();
        for buffer in &self.program.interface.buffers {
            if let crate::BufferChannels::Static(channels) = buffer.channels {
                if let Some(reason) =
                    buffer_static_channel_validation_error(channels, buffer.element)
                {
                    self.program_error(format!("buffer '{}' {reason}", buffer.name));
                }
            }
        }
        self.validate_buffer_arrays();
        // Constant-data items are scalar-element arrays by construction:
        // `ConstData::element` is a `ScalarType`, so runtime handles cannot be
        // serialized into this storage class.
        for data in &self.program.const_data {
            if data.values.len() > i32::MAX as usize {
                self.program_error(format!(
                    "constant data '{}' length exceeds the signed i32 MIR boundary",
                    data.name
                ));
            }
            if !scalar_sequence_fits_i32_bytes(data.values.len(), data.element) {
                self.program_error(format!(
                    "constant data '{}' logical byte size exceeds the signed i32 MIR boundary",
                    data.name
                ));
            }
            for value in &data.values {
                if value.ty() != data.element {
                    self.program_error(format!(
                        "constant data '{}' contains {:?}, expected {:?}",
                        data.name,
                        value.ty(),
                        data.element
                    ));
                }
            }
        }

        for (index, site) in self.program.log_sites.iter().enumerate() {
            if let Some(file) = site.source.file {
                if file.index() >= self.program.source_files.len() {
                    self.program_error(format!(
                        "log site {index} source references missing file {}",
                        file.raw()
                    ));
                }
            }
            let payload_size = site.argument_types.iter().fold(0_u32, |size, ty| {
                size.saturating_add(match ty {
                    crate::ScalarType::F32 | crate::ScalarType::I32 => 4,
                    crate::ScalarType::F64 | crate::ScalarType::I64 => 8,
                    crate::ScalarType::Bool => 1,
                })
            });
            if site.payload_size != payload_size {
                self.program_error(format!(
                    "log site {index} payload size is {}, expected {payload_size}",
                    site.payload_size
                ));
            }
        }

        self.require_function_kind(
            self.program.entry_points.init,
            FunctionKind::Init,
            "init entry point",
        );
        self.validate_parameterless_entry_signature(
            self.program.entry_points.init,
            "init entry point",
        );
        self.require_function_kind(
            self.program.entry_points.process,
            FunctionKind::Process,
            "process entry point",
        );
        self.validate_process_entry_signature();

        for (event_index, event) in self.program.interface.events.iter().enumerate() {
            for param in &event.params {
                self.require_type(param.ty, None, SourceSpan::UNKNOWN);
                if self.type_contains_runtime_handle(param.ty) {
                    match self.program.types.get(param.ty.index()) {
                        Some(Type::Slice {
                            access: crate::AccessMode::ReadOnly,
                            ..
                        }) => {}
                        Some(Type::Slice { .. }) => self.program_error(format!(
                            "event '{}' parameter '{}' slice must be read-only",
                            event.name, param.name
                        )),
                        _ => self.program_error(format!(
                            "event '{}' parameter '{}' may only use a direct read-only slice as a runtime handle",
                            event.name, param.name
                        )),
                    }
                }
                if param
                    .default
                    .as_ref()
                    .is_some_and(|default| !self.constant_matches_type(default, param.ty))
                {
                    self.program_error(format!(
                        "event '{}' parameter '{}' default does not match type {}",
                        event.name,
                        param.name,
                        self.type_name(param.ty)
                    ));
                }
            }
            let expected = FunctionKind::Event(crate::EventId::new(event_index as u32));
            self.require_function_kind(event.handler, expected, "event handler");
            self.validate_parameterless_entry_signature(
                event.handler,
                &format!("event '{}' handler", event.name),
            );
        }
        for delegate in &self.program.interface.delegates {
            for param in &delegate.params {
                self.require_type(param.ty, None, SourceSpan::UNKNOWN);
                match self.program.types.get(param.ty.index()) {
                    Some(Type::Scalar(_)) => {}
                    Some(Type::Array { element, .. }) => {
                        if !matches!(self.program.types.get(element.index()), Some(Type::Scalar(_)))
                        {
                            self.program_error(format!(
                                "delegate '{}' parameter '{}' fixed array element must be a primitive scalar",
                                delegate.name, param.name
                            ));
                        }
                    }
                    Some(Type::Slice {
                        access: crate::AccessMode::ReadOnly,
                        ..
                    }) => {}
                    Some(Type::Slice { .. }) => self.program_error(format!(
                        "delegate '{}' parameter '{}' slice must be read-only",
                        delegate.name, param.name
                    )),
                    Some(_) => self.program_error(format!(
                        "delegate '{}' parameter '{}' must be a primitive scalar, fixed primitive array, or read-only primitive slice",
                        delegate.name, param.name
                    )),
                    None => {}
                }
            }
        }

        self.validate_entry_role_ownership();
        for index in 0..self.program.functions.len() {
            let id = FunctionId::new(index as u32);
            let function = &self.program.functions[index];
            self.validate_function(id, function);
        }
        self.validate_init_cannot_reach_publication();
        self.validate_acyclic_call_graph();
    }

    fn validate_interface_names(&mut self) {
        let mut names = HashMap::<String, &str>::new();
        let mut insert = |name: &str, kind: &'static str, errors: &mut Vec<ValidationError>| {
            if let Some(previous) = names.insert(name.to_owned(), kind) {
                errors.push(ValidationError {
                    message: format!(
                        "interface name '{name}' is used by both {previous} and {kind}"
                    ),
                    function: None,
                    source: SourceSpan::UNKNOWN,
                });
            }
        };
        for input in &self.program.interface.inputs {
            insert(&input.name, "an input", &mut self.errors);
        }
        for output in &self.program.interface.outputs {
            insert(&output.name, "an audio output", &mut self.errors);
        }
        for output in &self.program.interface.control_outputs {
            insert(&output.name, "a control output", &mut self.errors);
        }
        for param in &self.program.interface.params {
            insert(&param.name, "a parameter", &mut self.errors);
        }
        for buffer in &self.program.interface.buffers {
            insert(&buffer.name, "a buffer", &mut self.errors);
        }
        for array in &self.program.interface.buffer_arrays {
            insert(&array.name, "a buffer array", &mut self.errors);
        }
        for event in &self.program.interface.events {
            insert(&event.name, "an event", &mut self.errors);
            let mut event_params = HashSet::new();
            for param in &event.params {
                if !event_params.insert(param.name.as_str()) {
                    self.program_error(format!(
                        "event '{}' has duplicate parameter name '{}'",
                        event.name, param.name
                    ));
                }
            }
        }
        for delegate in &self.program.interface.delegates {
            insert(&delegate.name, "a delegate", &mut self.errors);
            let mut delegate_params = HashSet::new();
            for param in &delegate.params {
                if !delegate_params.insert(param.name.as_str()) {
                    self.program_error(format!(
                        "delegate '{}' has duplicate parameter name '{}'",
                        delegate.name, param.name
                    ));
                }
            }
        }
    }

    fn validate_buffer_arrays(&mut self) {
        let groups = self.program.interface.buffer_arrays.clone();
        let mut occupied = vec![None::<String>; self.program.interface.buffers.len()];
        for group in groups {
            if group.len == 0 {
                self.program_error(format!("buffer array '{}' has zero length", group.name));
                continue;
            }
            let first = group.first.index();
            let Some(end) = first.checked_add(group.len as usize) else {
                self.program_error(format!("buffer array '{}' range overflows", group.name));
                continue;
            };
            if end > self.program.interface.buffers.len() {
                self.program_error(format!(
                    "buffer array '{}' range {}..{} exceeds {} buffers",
                    group.name,
                    first,
                    end,
                    self.program.interface.buffers.len()
                ));
                continue;
            }
            let expected = self.program.interface.buffers[first].clone();
            for (offset, occupied_by) in occupied[first..end].iter_mut().enumerate() {
                let index = first + offset;
                if let Some(previous) = occupied_by.replace(group.name.clone()) {
                    self.program_error(format!(
                        "buffer arrays '{}' and '{}' overlap at buffer {}",
                        previous, group.name, index
                    ));
                }
                let incompatible = {
                    let buffer = &self.program.interface.buffers[index];
                    (buffer.element != expected.element
                        || buffer.channels != expected.channels
                        || buffer.access != expected.access)
                        .then(|| buffer.name.clone())
                };
                if let Some(buffer_name) = incompatible {
                    self.program_error(format!(
                        "buffer array '{}' contains incompatible descriptor '{}'",
                        group.name, buffer_name
                    ));
                }
            }
        }
    }

    fn validate_control_output_mirrors(&mut self) {
        let mut mirror_counts = vec![0_u32; self.program.state.len()];
        for output in &self.program.interface.control_outputs {
            let Some(state) = self.program.state.get(output.mirror.index()) else {
                self.program_error(format!(
                    "control output '{}' references missing mirror state {}",
                    output.name,
                    output.mirror.raw()
                ));
                continue;
            };
            mirror_counts[output.mirror.index()] =
                mirror_counts[output.mirror.index()].saturating_add(1);
            if state.persistence != crate::StatePersistence::ControlMirror {
                self.program_error(format!(
                    "control output '{}' mirror state '{}' is not a control mirror",
                    output.name, state.name
                ));
            }
            if !self.program.types_equivalent(output.ty, state.ty) {
                self.program_error(format!(
                    "control output '{}' type {} does not match mirror state '{}' type {}",
                    output.name,
                    self.type_name(output.ty),
                    state.name,
                    self.type_name(state.ty)
                ));
            }
        }
        for (index, state) in self.program.state.iter().enumerate() {
            if state.persistence != crate::StatePersistence::ControlMirror {
                continue;
            }
            match mirror_counts[index] {
                0 => self.program_error(format!(
                    "control mirror state '{}' is not referenced by a control output",
                    state.name
                )),
                1 => {}
                count => self.program_error(format!(
                    "control mirror state '{}' is referenced by {count} control outputs",
                    state.name
                )),
            }
        }
    }

    fn validate_fixed_aggregate_types(&mut self) {
        #[derive(Clone, Copy)]
        enum LogicalSize {
            Pending,
            Invalid,
            Unsized,
            Fixed(u64),
        }

        let dependencies = self
            .program
            .types
            .iter()
            .map(|ty| match ty {
                Type::Scalar(_)
                | Type::Slice { .. }
                | Type::Buffer { .. }
                | Type::BufferSpan { .. } => Vec::new(),
                Type::Tuple(elements) => elements.clone(),
                Type::Array { element, .. } => vec![*element],
                Type::Struct(structure) => self
                    .program
                    .structs
                    .get(structure.index())
                    .map(|structure| {
                        structure
                            .fields
                            .iter()
                            .map(|field| field.ty)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        let mut sizes = self
            .program
            .types
            .iter()
            .map(|ty| match ty {
                Type::Scalar(scalar) => LogicalSize::Fixed(logical_scalar_bytes(*scalar)),
                Type::Slice { .. } | Type::Buffer { .. } | Type::BufferSpan { .. } => {
                    LogicalSize::Unsized
                }
                Type::Tuple(_) | Type::Array { .. } | Type::Struct(_) => LogicalSize::Pending,
            })
            .collect::<Vec<_>>();

        loop {
            let mut changed = false;
            for index in 0..sizes.len() {
                if !matches!(sizes[index], LogicalSize::Pending) {
                    continue;
                }
                if dependencies[index]
                    .iter()
                    .any(|dependency| dependency.index() >= sizes.len())
                {
                    sizes[index] = LogicalSize::Invalid;
                    changed = true;
                    continue;
                }
                if dependencies[index]
                    .iter()
                    .any(|dependency| matches!(sizes[dependency.index()], LogicalSize::Pending))
                {
                    continue;
                }
                if dependencies[index]
                    .iter()
                    .any(|dependency| matches!(sizes[dependency.index()], LogicalSize::Invalid))
                {
                    sizes[index] = LogicalSize::Invalid;
                    changed = true;
                    continue;
                }
                if dependencies[index]
                    .iter()
                    .any(|dependency| matches!(sizes[dependency.index()], LogicalSize::Unsized))
                {
                    sizes[index] = LogicalSize::Unsized;
                    changed = true;
                    continue;
                }

                let computed = match &self.program.types[index] {
                    Type::Tuple(elements) => elements.iter().try_fold(0_u64, |total, element| {
                        let LogicalSize::Fixed(bytes) = sizes[element.index()] else {
                            unreachable!("aggregate dependency was resolved as fixed")
                        };
                        total.checked_add(bytes)
                    }),
                    Type::Array { element, len } => {
                        let LogicalSize::Fixed(bytes) = sizes[element.index()] else {
                            unreachable!("aggregate dependency was resolved as fixed")
                        };
                        bytes.checked_mul(u64::from(*len))
                    }
                    Type::Struct(structure) => self
                        .program
                        .structs
                        .get(structure.index())
                        .and_then(|structure| {
                            structure.fields.iter().try_fold(0_u64, |total, field| {
                                let LogicalSize::Fixed(bytes) = sizes[field.ty.index()] else {
                                    unreachable!("aggregate dependency was resolved as fixed")
                                };
                                total.checked_add(bytes)
                            })
                        }),
                    Type::Scalar(_)
                    | Type::Slice { .. }
                    | Type::Buffer { .. }
                    | Type::BufferSpan { .. } => unreachable!(),
                };
                match computed {
                    Some(bytes) if bytes <= i32::MAX as u64 => {
                        sizes[index] = LogicalSize::Fixed(bytes);
                    }
                    _ => {
                        sizes[index] = LogicalSize::Invalid;
                        self.program_error(format!(
                            "type {index} fixed aggregate logical size exceeds the signed i32 MIR boundary"
                        ));
                    }
                }
                changed = true;
            }
            if !changed {
                break;
            }
        }

        for (index, size) in sizes.iter_mut().enumerate() {
            if matches!(size, LogicalSize::Pending) {
                *size = LogicalSize::Invalid;
                self.program_error(format!(
                    "type {index} participates in a recursive fixed aggregate definition"
                ));
            }
        }
    }

    fn validate_acyclic_call_graph(&mut self) {
        let Some(cycle) = find_call_cycle(self.program) else {
            return;
        };
        let function_index = cycle[0];
        let function = &self.program.functions[function_index];
        let display = cycle
            .iter()
            .map(|index| self.program.functions[*index].name.as_str())
            .collect::<Vec<_>>()
            .join(" -> ");
        self.function_error(
            FunctionId::new(function_index as u32),
            function.source,
            format!("recursive call cycle is not realtime-safe: {display}"),
        );
    }

    fn validate_init_cannot_reach_publication(&mut self) {
        let init = self.program.entry_points.init.index();
        if init >= self.program.functions.len() {
            return;
        }
        let mut pending = vec![init];
        let mut visited = vec![false; self.program.functions.len()];
        while let Some(index) = pending.pop() {
            if visited[index] {
                continue;
            }
            visited[index] = true;
            let function = &self.program.functions[index];
            if block_contains_publication(&function.body) {
                self.function_error(
                    FunctionId::new(index as u32),
                    function.source,
                    "init entry point reaches delegate publication",
                );
            }
            let mut callees = Vec::new();
            collect_block_callees(&function.body, self.program.functions.len(), &mut callees);
            pending.extend(callees);
        }
    }

    fn validate_function(&mut self, id: FunctionId, function: &Function) {
        if let Some(file) = function.source.file {
            if file.index() >= self.program.source_files.len() {
                self.function_error(
                    id,
                    function.source,
                    "function source references missing file",
                );
            }
        }
        for param in &function.params {
            self.require_type(param.ty, Some(id), function.source);
            if let Some(range) = param.integer_range {
                if self.producer_proofs == ProducerProofStatus::Absent {
                    self.function_error(
                        id,
                        function.source,
                        format!(
                            "parameter '{}' integer range requires trusted producer validation",
                            param.name
                        ),
                    );
                }
                if let Some(reason) = self.integer_range_validation_error(range, param.ty) {
                    self.function_error(
                        id,
                        function.source,
                        format!("parameter '{}' integer range {reason}", param.name),
                    );
                }
            }
            match self.program.types.get(param.ty.index()) {
                Some(Type::BufferSpan { .. }) if param.mode != crate::PassingMode::Value => {
                    self.function_error(
                        id,
                        function.source,
                        format!(
                            "buffer span parameter '{}' must use value passing mode",
                            param.name
                        ),
                    );
                }
                Some(Type::Buffer { .. }) if param.mode == crate::PassingMode::Value => {
                    self.function_error(
                        id,
                        function.source,
                        format!(
                            "buffer parameter '{}' must use reference passing mode",
                            param.name
                        ),
                    );
                }
                _ => {}
            }
        }
        for result in &function.results {
            self.require_type(*result, Some(id), function.source);
            self.reject_runtime_handle_storage(
                *result,
                Some(id),
                function.source,
                "function result",
            );
        }
        for local in &function.locals {
            self.require_type(local.ty, Some(id), function.source);
            if let Some(range) = local.integer_range {
                if self.producer_proofs == ProducerProofStatus::Absent {
                    self.function_error(
                        id,
                        function.source,
                        format!(
                            "local {:?} integer range requires trusted producer validation",
                            local.name
                        ),
                    );
                }
                if let Some(reason) = self.integer_range_validation_error(range, local.ty) {
                    self.function_error(
                        id,
                        function.source,
                        format!("local {:?} integer range {reason}", local.name),
                    );
                }
            }
        }
        if self.producer_proofs == ProducerProofStatus::Absent {
            self.reject_unchecked_bounds(id, &function.body);
        }
        self.validate_block(id, function, &function.body, 0);
        self.validate_definite_assignment(id, function);
        if !function.results.is_empty() && block_control_flow(&function.body).falls_through {
            self.function_error(
                id,
                function.source,
                "result-bearing function has a reachable path that falls through without returning a value",
            );
        }
    }

    fn reject_unchecked_bounds(&mut self, function: FunctionId, block: &Block) {
        for statement in &block.statements {
            if statement_uses_unchecked_bounds(&statement.kind) {
                self.function_error(
                    function,
                    statement.source,
                    "unchecked bounds require a trusted MIR producer proof",
                );
            }
            match &statement.kind {
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    self.reject_unchecked_bounds(function, then_block);
                    self.reject_unchecked_bounds(function, else_block);
                }
                StatementKind::Loop { body } => self.reject_unchecked_bounds(function, body),
                _ => {}
            }
        }
    }

    fn validate_block(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        block: &Block,
        loop_depth: usize,
    ) {
        for statement in &block.statements {
            if let Some(file) = statement.source.file {
                if file.index() >= self.program.source_files.len() {
                    self.function_error(
                        function_id,
                        statement.source,
                        "statement source references missing file",
                    );
                }
            }
            match &statement.kind {
                StatementKind::Assign { destination, value } => {
                    self.validate_place(function_id, function, destination, statement.source);
                    self.validate_rvalue(function_id, function, value, statement.source);
                    if matches!(value, Rvalue::ProcessFrame { .. })
                        && !matches!(
                            destination,
                            Place {
                                base: PlaceBase::Local(_),
                                projections,
                            } if projections.is_empty()
                        )
                    {
                        self.function_error(
                            function_id,
                            statement.source,
                            "process_frame must be the unique definition of an unprojected local",
                        );
                    }
                    if !self.place_is_writable(function, destination) {
                        self.function_error(
                            function_id,
                            statement.source,
                            "assignment destination is not writable",
                        );
                    }
                    if let Some(destination_ty) = self.place_type(function, destination) {
                        if !self.rvalue_matches_type(function, value, destination_ty) {
                            self.function_error(
                                function_id,
                                statement.source,
                                format!(
                                    "assignment value does not match destination type {}",
                                    self.type_name(destination_ty)
                                ),
                            );
                        }
                    }
                }
                StatementKind::Call {
                    results,
                    function: callee,
                    args,
                } => {
                    let Some(callee_fn) = self.program.functions.get(callee.index()) else {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!("call references missing function {}", callee.raw()),
                        );
                        continue;
                    };
                    if !matches!(callee_fn.kind, FunctionKind::User) {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!(
                                "calls may only target user functions, not '{}'",
                                callee_fn.name
                            ),
                        );
                    }
                    if results.len() != callee_fn.results.len() {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!(
                                "call to '{}' has {} result destinations but function returns {} values",
                                callee_fn.name,
                                results.len(),
                                callee_fn.results.len()
                            ),
                        );
                    }
                    if args.len() != callee_fn.params.len() {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!(
                                "call to '{}' has {} arguments but function takes {}",
                                callee_fn.name,
                                args.len(),
                                callee_fn.params.len()
                            ),
                        );
                    }
                    for result in results {
                        self.require_local(function_id, function, *result, statement.source);
                    }
                    for (index, (result, expected)) in
                        results.iter().zip(callee_fn.results.iter()).enumerate()
                    {
                        let Some(actual) =
                            function.locals.get(result.index()).map(|local| local.ty)
                        else {
                            continue;
                        };
                        if !self.program.types_equivalent(actual, *expected) {
                            self.function_error(
                                function_id,
                                statement.source,
                                format!(
                                    "call to '{}' result {index} has destination type {}, expected {}",
                                    callee_fn.name,
                                    self.type_name(actual),
                                    self.type_name(*expected)
                                ),
                            );
                        }
                    }
                    for arg in args {
                        match arg {
                            CallArgument::Value(value) => {
                                self.validate_value(function_id, function, *value, statement.source)
                            }
                            CallArgument::Place(place) => {
                                self.validate_place(function_id, function, place, statement.source)
                            }
                            CallArgument::SliceElement { slice, index, .. } => {
                                self.validate_value(
                                    function_id,
                                    function,
                                    *slice,
                                    statement.source,
                                );
                                self.require_i32_value(
                                    function_id,
                                    function,
                                    *index,
                                    statement.source,
                                    "slice call argument index",
                                );
                                if self.value_slice_type(function, *slice).is_none() {
                                    self.function_error(
                                        function_id,
                                        statement.source,
                                        "slice call argument base is not slice-typed",
                                    );
                                }
                            }
                            CallArgument::ArrayWindow { array, start, .. } => {
                                self.validate_place(function_id, function, array, statement.source);
                                self.validate_value(
                                    function_id,
                                    function,
                                    *start,
                                    statement.source,
                                );
                                self.require_i32_value(
                                    function_id,
                                    function,
                                    *start,
                                    statement.source,
                                    "array-window start",
                                );
                            }
                            CallArgument::SliceWindow { slice, start, .. } => {
                                self.validate_value(
                                    function_id,
                                    function,
                                    *slice,
                                    statement.source,
                                );
                                self.validate_value(
                                    function_id,
                                    function,
                                    *start,
                                    statement.source,
                                );
                                self.require_i32_value(
                                    function_id,
                                    function,
                                    *start,
                                    statement.source,
                                    "slice-window start",
                                );
                                if self.value_slice_type(function, *slice).is_none() {
                                    self.function_error(
                                        function_id,
                                        statement.source,
                                        "slice-window call argument base is not slice-typed",
                                    );
                                }
                            }
                            CallArgument::Buffer(buffer) => {
                                self.require_direct_buffer_capability(
                                    function_id,
                                    function,
                                    statement.source,
                                    "buffer call argument",
                                );
                                self.require_buffer(
                                    function_id,
                                    function,
                                    *buffer,
                                    statement.source,
                                );
                            }
                            CallArgument::BufferParam(parameter) => {
                                if self
                                    .function_buffer_param_ref(function, *parameter)
                                    .is_none()
                                {
                                    self.function_error(
                                        function_id,
                                        statement.source,
                                        "buffer-parameter call argument references a non-buffer parameter",
                                    );
                                }
                                if let crate::BufferParamRef::ArrayElement { selector, .. } =
                                    parameter
                                {
                                    self.require_i32_value(
                                        function_id,
                                        function,
                                        *selector,
                                        statement.source,
                                        "buffer-parameter collection selector",
                                    );
                                }
                            }
                            CallArgument::BufferSpan(_) => {}
                        }
                    }
                    for (index, (arg, param)) in
                        args.iter().zip(callee_fn.params.iter()).enumerate()
                    {
                        if !self.call_argument_matches(function, arg, param) {
                            self.function_error(
                                function_id,
                                statement.source,
                                format!(
                                    "call to '{}' argument {index} does not match {} parameter type {}",
                                    callee_fn.name,
                                    passing_mode_name(param.mode),
                                    self.type_name(param.ty)
                                ),
                            );
                        }
                    }
                }
                StatementKind::PublishDelegate { delegate, args } => {
                    let Some(descriptor) = self.program.interface.delegates.get(delegate.index())
                    else {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!("publication references missing delegate {}", delegate.raw()),
                        );
                        continue;
                    };
                    if matches!(function.kind, FunctionKind::Init) {
                        self.function_error(
                            function_id,
                            statement.source,
                            "init entry point cannot publish delegates",
                        );
                    }
                    if matches!(function.kind, FunctionKind::User)
                        && !function.attributes.runtime_context
                    {
                        self.function_error(
                            function_id,
                            statement.source,
                            "delegate publication requires a runtime-context user function",
                        );
                    }
                    if args.len() != descriptor.params.len() {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!(
                                "publication of delegate '{}' has {} arguments but the descriptor declares {}",
                                descriptor.name,
                                args.len(),
                                descriptor.params.len()
                            ),
                        );
                    }
                    for (index, argument) in args.iter().enumerate() {
                        let CallArgument::Value(value) = argument else {
                            self.function_error(
                                function_id,
                                statement.source,
                                format!(
                                    "publication of delegate '{}' argument {index} must be an evaluated value",
                                    descriptor.name
                                ),
                            );
                            continue;
                        };
                        self.validate_value(function_id, function, *value, statement.source);
                        let Some(param) = descriptor.params.get(index) else {
                            continue;
                        };
                        let matches = match self.program.types.get(param.ty.index()) {
                            Some(Type::Scalar(_)) => {
                                self.value_matches_type(function, *value, param.ty)
                            }
                            Some(Type::Array { element, .. }) => {
                                match self.program.types.get(element.index()) {
                                    Some(Type::Scalar(expected)) => self
                                        .value_slice_type(function, *value)
                                        .is_some_and(|(actual, _)| actual == *expected),
                                    _ => false,
                                }
                            }
                            Some(Type::Slice { element, .. }) => self
                                .value_slice_type(function, *value)
                                .is_some_and(|(actual, _)| actual == *element),
                            _ => false,
                        };
                        if !matches {
                            self.function_error(
                                function_id,
                                statement.source,
                                format!(
                                    "publication of delegate '{}' argument {index} does not match payload parameter '{}' type {}",
                                    descriptor.name,
                                    param.name,
                                    self.type_name(param.ty)
                                ),
                            );
                        }
                    }
                }
                StatementKind::PublishLog { site, arguments } => {
                    let Some(descriptor) = self.program.log_sites.get(site.index()) else {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!(
                                "print publication references missing log site {}",
                                site.raw()
                            ),
                        );
                        continue;
                    };
                    if matches!(function.kind, FunctionKind::User)
                        && !function.attributes.runtime_context
                    {
                        self.function_error(
                            function_id,
                            statement.source,
                            "print publication requires a runtime-context user function",
                        );
                    }
                    if arguments.len() != descriptor.argument_types.len() {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!(
                                "print publication at site {} has {} arguments but the descriptor declares {}",
                                site.raw(),
                                arguments.len(),
                                descriptor.argument_types.len()
                            ),
                        );
                    }
                    for (index, value) in arguments.iter().enumerate() {
                        self.validate_value(function_id, function, *value, statement.source);
                        if let Some(expected) = descriptor.argument_types.get(index) {
                            if self.value_scalar_type(function, *value) != Some(*expected) {
                                self.function_error(
                                    function_id,
                                    statement.source,
                                    format!(
                                        "print publication at site {} argument {index} does not match {:?}",
                                        site.raw(), expected
                                    ),
                                );
                            }
                        }
                    }
                }
                StatementKind::OutputStore {
                    output,
                    element,
                    frame,
                    value,
                    ..
                } => {
                    self.require_audio_io_capability(
                        function_id,
                        function,
                        statement.source,
                        "audio output store",
                    );
                    if output.index() >= self.program.interface.outputs.len() {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!("store references missing output {}", output.raw()),
                        );
                    }
                    self.validate_optional_value(function_id, function, *element, statement.source);
                    self.validate_value(function_id, function, *frame, statement.source);
                    self.validate_value(function_id, function, *value, statement.source);
                    self.require_process_frame_value(
                        function_id,
                        function,
                        *frame,
                        statement.source,
                        "audio output store",
                    );
                    self.validate_optional_index(
                        function_id,
                        function,
                        *element,
                        statement.source,
                        "output element",
                    );
                    if let Some(expected) = self
                        .program
                        .interface
                        .outputs
                        .get(output.index())
                        .and_then(|output| self.indexed_type(output.ty, element.is_some()))
                    {
                        if !self.value_matches_type(function, *value, expected) {
                            self.function_error(
                                function_id,
                                statement.source,
                                format!(
                                    "output store value does not match output type {}",
                                    self.type_name(expected)
                                ),
                            );
                        }
                    }
                }
                StatementKind::ControlOutputStore {
                    output,
                    element,
                    value,
                    ..
                } => {
                    self.require_control_output_capability(function_id, function, statement.source);
                    if output.index() >= self.program.interface.control_outputs.len() {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!("store references missing control output {}", output.raw()),
                        );
                    }
                    self.validate_optional_value(function_id, function, *element, statement.source);
                    self.validate_value(function_id, function, *value, statement.source);
                    self.validate_optional_index(
                        function_id,
                        function,
                        *element,
                        statement.source,
                        "control output element",
                    );
                    if let Some(expected) = self
                        .program
                        .interface
                        .control_outputs
                        .get(output.index())
                        .and_then(|output| self.indexed_type(output.ty, element.is_some()))
                    {
                        if !self.value_matches_type(function, *value, expected) {
                            self.function_error(
                                function_id,
                                statement.source,
                                format!(
                                    "control output store value does not match output type {}",
                                    self.type_name(expected)
                                ),
                            );
                        }
                    }
                }
                StatementKind::BufferStore {
                    buffer,
                    channel,
                    index,
                    value,
                    ..
                } => {
                    self.require_direct_buffer_capability(
                        function_id,
                        function,
                        statement.source,
                        "buffer store",
                    );
                    self.require_buffer(function_id, function, *buffer, statement.source);
                    self.validate_optional_value(function_id, function, *channel, statement.source);
                    self.validate_value(function_id, function, *index, statement.source);
                    self.validate_value(function_id, function, *value, statement.source);
                    self.validate_optional_index(
                        function_id,
                        function,
                        *channel,
                        statement.source,
                        "buffer channel",
                    );
                    self.require_i32_value(
                        function_id,
                        function,
                        *index,
                        statement.source,
                        "buffer index",
                    );
                    if let Some(buffer) = self.program.interface.buffers.get(buffer.index()) {
                        if buffer.access != crate::AccessMode::ReadWrite {
                            self.function_error(
                                function_id,
                                statement.source,
                                "cannot store to a read-only interface buffer",
                            );
                        }
                        if !self.value_matches_scalar(function, *value, buffer.element) {
                            self.function_error(
                                function_id,
                                statement.source,
                                format!(
                                    "buffer store value does not match element type {}",
                                    buffer.element.name()
                                ),
                            );
                        }
                    }
                }
                StatementKind::BufferParamStore {
                    parameter,
                    channel,
                    index,
                    value,
                    ..
                } => {
                    self.validate_buffer_param_ref(
                        function_id,
                        function,
                        *parameter,
                        statement.source,
                    );
                    self.validate_optional_value(function_id, function, *channel, statement.source);
                    self.validate_value(function_id, function, *index, statement.source);
                    self.validate_value(function_id, function, *value, statement.source);
                    self.validate_optional_index(
                        function_id,
                        function,
                        *channel,
                        statement.source,
                        "buffer channel",
                    );
                    self.require_i32_value(
                        function_id,
                        function,
                        *index,
                        statement.source,
                        "buffer index",
                    );
                    match self.function_buffer_param_ref(function, *parameter) {
                        Some((element, crate::AccessMode::ReadWrite)) => {
                            if !self.value_matches_scalar(function, *value, element) {
                                self.function_error(
                                    function_id,
                                    statement.source,
                                    format!(
                                        "buffer parameter store value does not match element type {}",
                                        element.name()
                                    ),
                                );
                            }
                        }
                        Some((_, crate::AccessMode::ReadOnly)) => self.function_error(
                            function_id,
                            statement.source,
                            "cannot store through a read-only buffer parameter",
                        ),
                        None => self.function_error(
                            function_id,
                            statement.source,
                            format!(
                                "buffer operation references non-buffer parameter {}",
                                parameter.raw()
                            ),
                        ),
                    }
                }
                StatementKind::SliceStore {
                    slice,
                    index,
                    value,
                    ..
                } => {
                    self.validate_value(function_id, function, *slice, statement.source);
                    self.validate_value(function_id, function, *index, statement.source);
                    self.validate_value(function_id, function, *value, statement.source);
                    self.require_i32_value(
                        function_id,
                        function,
                        *index,
                        statement.source,
                        "slice index",
                    );
                    match self.value_slice_type(function, *slice) {
                        Some((element, crate::AccessMode::ReadWrite)) => {
                            if !self.value_matches_scalar(function, *value, element) {
                                self.function_error(
                                    function_id,
                                    statement.source,
                                    format!(
                                        "slice store value does not match element type {}",
                                        element.name()
                                    ),
                                );
                            }
                        }
                        Some((_, crate::AccessMode::ReadOnly)) => self.function_error(
                            function_id,
                            statement.source,
                            "slice store destination is read-only",
                        ),
                        None => self.function_error(
                            function_id,
                            statement.source,
                            "slice store destination is not a slice",
                        ),
                    }
                }
                StatementKind::SliceFill { destination, value } => {
                    self.validate_value(function_id, function, *destination, statement.source);
                    self.validate_value(function_id, function, *value, statement.source);
                    match self.value_slice_type(function, *destination) {
                        Some((element, crate::AccessMode::ReadWrite)) => {
                            if !self.value_matches_scalar(function, *value, element) {
                                self.function_error(
                                    function_id,
                                    statement.source,
                                    format!(
                                        "slice fill value does not match element type {}",
                                        element.name()
                                    ),
                                );
                            }
                        }
                        Some((_, crate::AccessMode::ReadOnly)) => self.function_error(
                            function_id,
                            statement.source,
                            "slice fill destination is read-only",
                        ),
                        None => self.function_error(
                            function_id,
                            statement.source,
                            "slice fill destination is not a slice",
                        ),
                    }
                }
                StatementKind::SliceCopy {
                    destination,
                    source,
                } => {
                    self.validate_value(function_id, function, *destination, statement.source);
                    self.validate_value(function_id, function, *source, statement.source);
                    let destination_ty = self.value_slice_type(function, *destination);
                    let source_ty = self.value_slice_type(function, *source);
                    match (destination_ty, source_ty) {
                        (
                            Some((destination_element, crate::AccessMode::ReadWrite)),
                            Some((source_element, _)),
                        ) => {
                            if source_element != destination_element {
                                self.function_error(
                                    function_id,
                                    statement.source,
                                    format!(
                                        "slice copy requires identical element types, got {} and {}",
                                        source_element.name(),
                                        destination_element.name()
                                    ),
                                );
                            }
                        }
                        (Some((_, crate::AccessMode::ReadOnly)), Some(_)) => self.function_error(
                            function_id,
                            statement.source,
                            "slice copy destination is read-only",
                        ),
                        (None, _) => self.function_error(
                            function_id,
                            statement.source,
                            "slice copy destination is not a slice",
                        ),
                        (_, None) => self.function_error(
                            function_id,
                            statement.source,
                            "slice copy source is not a slice",
                        ),
                    }
                }
                StatementKind::If {
                    condition,
                    then_block,
                    else_block,
                } => {
                    self.validate_value(function_id, function, *condition, statement.source);
                    if !self.value_matches_scalar(function, *condition, crate::ScalarType::Bool) {
                        self.function_error(
                            function_id,
                            statement.source,
                            "if condition must have type bool",
                        );
                    }
                    self.validate_block(function_id, function, then_block, loop_depth);
                    self.validate_block(function_id, function, else_block, loop_depth);
                }
                StatementKind::Loop { body } => {
                    self.validate_block(function_id, function, body, loop_depth + 1);
                }
                StatementKind::Break | StatementKind::Continue => {
                    if loop_depth == 0 {
                        self.function_error(
                            function_id,
                            statement.source,
                            "loop control appears outside a loop",
                        );
                    }
                }
                StatementKind::Return { values } => {
                    if values.len() != function.results.len() {
                        self.function_error(
                            function_id,
                            statement.source,
                            format!(
                                "return has {} values but function declares {} results",
                                values.len(),
                                function.results.len()
                            ),
                        );
                    }
                    for (index, value) in values.iter().enumerate() {
                        self.validate_value(function_id, function, *value, statement.source);
                        if let Some(expected) = function.results.get(index) {
                            if !self.value_matches_type(function, *value, *expected) {
                                self.function_error(
                                    function_id,
                                    statement.source,
                                    format!(
                                        "return value {index} has a different type than declared result {}",
                                        self.type_name(*expected)
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn validate_definite_assignment(&mut self, function_id: FunctionId, function: &Function) {
        let initial = AssignmentState::new(function.locals.len());
        let _ = self.validate_assignment_block(function_id, function, &function.body, initial);
    }

    fn validate_assignment_block(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        block: &Block,
        initial: AssignmentState,
    ) -> AssignmentFlow {
        let mut current = Some(initial);
        let mut breaks = Vec::new();
        let mut continues = Vec::new();
        for statement in &block.statements {
            let Some(mut state) = current.take() else {
                break;
            };
            match &statement.kind {
                StatementKind::If {
                    condition,
                    then_block,
                    else_block,
                } => {
                    self.assignment_read_value(
                        function_id,
                        function,
                        *condition,
                        statement.source,
                        &state,
                    );
                    let then_flow = self.validate_assignment_block(
                        function_id,
                        function,
                        then_block,
                        state.clone(),
                    );
                    let else_flow =
                        self.validate_assignment_block(function_id, function, else_block, state);
                    current = merge_assignment_fallthrough(
                        function,
                        self.program,
                        then_flow.fallthrough,
                        else_flow.fallthrough,
                    );
                    breaks.extend(then_flow.breaks);
                    breaks.extend(else_flow.breaks);
                    continues.extend(then_flow.continues);
                    continues.extend(else_flow.continues);
                }
                StatementKind::Loop { body } => {
                    let body_flow =
                        self.validate_assignment_block(function_id, function, body, state);
                    current = intersect_assignment_states(function, self.program, body_flow.breaks);
                }
                StatementKind::Break => {
                    breaks.push(state);
                    current = None;
                }
                StatementKind::Continue => {
                    continues.push(state);
                    current = None;
                }
                StatementKind::Return { values } => {
                    for value in values {
                        self.assignment_read_value(
                            function_id,
                            function,
                            *value,
                            statement.source,
                            &state,
                        );
                    }
                    current = None;
                }
                _ => {
                    self.validate_assignment_statement(
                        function_id,
                        function,
                        statement,
                        &mut state,
                    );
                    current = Some(state);
                }
            }
        }
        AssignmentFlow {
            fallthrough: current,
            breaks,
            continues,
        }
    }

    fn validate_assignment_statement(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        statement: &crate::Statement,
        state: &mut AssignmentState,
    ) {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                self.assignment_read_rvalue(function_id, function, value, statement.source, state);
                self.assignment_write_place(
                    function_id,
                    function,
                    destination,
                    matches!(value, Rvalue::ProcessFrame { .. }),
                    statement.source,
                    state,
                );
            }
            StatementKind::Call {
                results,
                function: callee,
                args,
            } => {
                for argument in args {
                    match argument {
                        CallArgument::Value(value) => self.assignment_read_value(
                            function_id,
                            function,
                            *value,
                            statement.source,
                            state,
                        ),
                        CallArgument::Place(place) => self.assignment_read_place(
                            function_id,
                            function,
                            place,
                            statement.source,
                            state,
                        ),
                        CallArgument::SliceElement { slice, index, .. } => {
                            self.assignment_read_value(
                                function_id,
                                function,
                                *slice,
                                statement.source,
                                state,
                            );
                            self.assignment_read_value(
                                function_id,
                                function,
                                *index,
                                statement.source,
                                state,
                            );
                        }
                        CallArgument::ArrayWindow { array, start, .. } => {
                            self.assignment_read_place(
                                function_id,
                                function,
                                array,
                                statement.source,
                                state,
                            );
                            self.assignment_read_value(
                                function_id,
                                function,
                                *start,
                                statement.source,
                                state,
                            );
                        }
                        CallArgument::SliceWindow { slice, start, .. } => {
                            self.assignment_read_value(
                                function_id,
                                function,
                                *slice,
                                statement.source,
                                state,
                            );
                            self.assignment_read_value(
                                function_id,
                                function,
                                *start,
                                statement.source,
                                state,
                            );
                        }
                        CallArgument::Buffer(buffer) => self.assignment_read_buffer_ref(
                            function_id,
                            function,
                            *buffer,
                            statement.source,
                            state,
                        ),
                        CallArgument::BufferParam(parameter) => {
                            if let crate::BufferParamRef::ArrayElement { selector, .. } = parameter
                            {
                                self.assignment_read_value(
                                    function_id,
                                    function,
                                    *selector,
                                    statement.source,
                                    state,
                                );
                            }
                        }
                        CallArgument::BufferSpan(_) => {}
                    }
                }
                if let Some(parameters) = self
                    .program
                    .functions
                    .get(callee.index())
                    .map(|callee| callee.params.as_slice())
                {
                    for (argument, parameter) in args.iter().zip(parameters) {
                        if parameter.mode == crate::PassingMode::ReadWriteReference {
                            self.assignment_invalidate_read_write_argument(argument, state);
                        }
                    }
                }
                for result in results {
                    self.assignment_write_local(function, *result, false, state);
                }
            }
            StatementKind::PublishDelegate { args, .. } => {
                for argument in args {
                    match argument {
                        CallArgument::Value(value) => self.assignment_read_value(
                            function_id,
                            function,
                            *value,
                            statement.source,
                            state,
                        ),
                        CallArgument::Place(place) => self.assignment_read_place(
                            function_id,
                            function,
                            place,
                            statement.source,
                            state,
                        ),
                        CallArgument::SliceElement { slice, index, .. } => {
                            self.assignment_read_value(
                                function_id,
                                function,
                                *slice,
                                statement.source,
                                state,
                            );
                            self.assignment_read_value(
                                function_id,
                                function,
                                *index,
                                statement.source,
                                state,
                            );
                        }
                        CallArgument::ArrayWindow { array, start, .. } => {
                            self.assignment_read_place(
                                function_id,
                                function,
                                array,
                                statement.source,
                                state,
                            );
                            self.assignment_read_value(
                                function_id,
                                function,
                                *start,
                                statement.source,
                                state,
                            );
                        }
                        CallArgument::SliceWindow { slice, start, .. } => {
                            self.assignment_read_value(
                                function_id,
                                function,
                                *slice,
                                statement.source,
                                state,
                            );
                            self.assignment_read_value(
                                function_id,
                                function,
                                *start,
                                statement.source,
                                state,
                            );
                        }
                        CallArgument::Buffer(_)
                        | CallArgument::BufferParam(_)
                        | CallArgument::BufferSpan(_) => {}
                    }
                }
            }
            StatementKind::PublishLog { arguments, .. } => {
                for value in arguments {
                    self.assignment_read_value(
                        function_id,
                        function,
                        *value,
                        statement.source,
                        state,
                    );
                }
            }
            StatementKind::OutputStore {
                element,
                frame,
                value,
                ..
            } => {
                self.assignment_read_optional_value(
                    function_id,
                    function,
                    *element,
                    statement.source,
                    state,
                );
                self.assignment_read_process_frame(
                    function_id,
                    function,
                    *frame,
                    statement.source,
                    "audio output store",
                    state,
                );
                self.assignment_read_value(function_id, function, *value, statement.source, state);
            }
            StatementKind::ControlOutputStore { element, value, .. } => {
                self.assignment_read_optional_value(
                    function_id,
                    function,
                    *element,
                    statement.source,
                    state,
                );
                self.assignment_read_value(function_id, function, *value, statement.source, state);
            }
            StatementKind::BufferStore {
                buffer,
                channel,
                index,
                value,
                ..
            } => {
                self.assignment_read_buffer_ref(
                    function_id,
                    function,
                    *buffer,
                    statement.source,
                    state,
                );
                self.assignment_read_optional_value(
                    function_id,
                    function,
                    *channel,
                    statement.source,
                    state,
                );
                self.assignment_read_value(function_id, function, *index, statement.source, state);
                self.assignment_read_value(function_id, function, *value, statement.source, state);
            }
            StatementKind::BufferParamStore {
                channel,
                index,
                value,
                ..
            } => {
                self.assignment_read_optional_value(
                    function_id,
                    function,
                    *channel,
                    statement.source,
                    state,
                );
                self.assignment_read_value(function_id, function, *index, statement.source, state);
                self.assignment_read_value(function_id, function, *value, statement.source, state);
            }
            StatementKind::SliceStore {
                slice,
                index,
                value,
                ..
            } => {
                for value in [*slice, *index, *value] {
                    self.assignment_read_value(
                        function_id,
                        function,
                        value,
                        statement.source,
                        state,
                    );
                }
            }
            StatementKind::SliceFill { destination, value } => {
                for value in [*destination, *value] {
                    self.assignment_read_value(
                        function_id,
                        function,
                        value,
                        statement.source,
                        state,
                    );
                }
            }
            StatementKind::SliceCopy {
                destination,
                source,
            } => {
                for value in [*destination, *source] {
                    self.assignment_read_value(
                        function_id,
                        function,
                        value,
                        statement.source,
                        state,
                    );
                }
            }
            StatementKind::If { .. }
            | StatementKind::Loop { .. }
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Return { .. } => unreachable!("structured flow handled by caller"),
        }
    }

    fn assignment_read_rvalue(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        rvalue: &Rvalue,
        source: SourceSpan,
        state: &AssignmentState,
    ) {
        match rvalue {
            Rvalue::InitAll => {}
            Rvalue::Use(value) => {
                self.assignment_read_value(function_id, function, *value, source, state)
            }
            Rvalue::Load(place) => {
                self.assignment_read_place(function_id, function, place, source, state)
            }
            Rvalue::Unary { operand, .. } => {
                self.assignment_read_value(function_id, function, *operand, source, state)
            }
            Rvalue::Binary { lhs, rhs, .. } | Rvalue::Compare { lhs, rhs, .. } => {
                for value in [*lhs, *rhs] {
                    self.assignment_read_value(function_id, function, value, source, state);
                }
            }
            Rvalue::Cast { value, .. } => {
                self.assignment_read_value(function_id, function, *value, source, state)
            }
            Rvalue::Intrinsic { args, .. } => {
                for value in args {
                    self.assignment_read_value(function_id, function, *value, source, state);
                }
            }
            Rvalue::ProcessFrame { offset } => {
                self.assignment_read_value(function_id, function, *offset, source, state)
            }
            Rvalue::InputLoad { element, frame, .. }
            | Rvalue::OutputLoad { element, frame, .. } => {
                self.assignment_read_optional_value(function_id, function, *element, source, state);
                self.assignment_read_process_frame(
                    function_id,
                    function,
                    *frame,
                    source,
                    "audio I/O",
                    state,
                );
            }
            Rvalue::BufferLoad {
                buffer,
                channel,
                index,
                ..
            } => {
                self.assignment_read_buffer_ref(function_id, function, *buffer, source, state);
                self.assignment_read_optional_value(function_id, function, *channel, source, state);
                self.assignment_read_value(function_id, function, *index, source, state);
            }
            Rvalue::BufferParamLoad { channel, index, .. } => {
                self.assignment_read_optional_value(function_id, function, *channel, source, state);
                self.assignment_read_value(function_id, function, *index, source, state);
            }
            Rvalue::BufferLen(buffer)
            | Rvalue::BufferChannels(buffer)
            | Rvalue::BufferSampleRate(buffer)
            | Rvalue::BufferIsBound(buffer) => {
                self.assignment_read_buffer_ref(function_id, function, *buffer, source, state);
            }
            Rvalue::BufferParamLen(_)
            | Rvalue::BufferParamChannels(_)
            | Rvalue::BufferParamSampleRate(_)
            | Rvalue::BufferParamIsBound(_) => {}
            Rvalue::ConstDataLoad { index, .. } => {
                self.assignment_read_value(function_id, function, *index, source, state)
            }
            Rvalue::MakeSlice {
                source: slice_source,
                start,
                len,
                ..
            } => {
                match slice_source {
                    SliceSource::Place(place) => {
                        self.assignment_read_place_indices(
                            function_id,
                            function,
                            place,
                            source,
                            state,
                        );
                        if self.place_type(function, place).is_some_and(|ty| {
                            matches!(self.program.types.get(ty.index()), Some(Type::Slice { .. }))
                        }) {
                            self.assignment_read_place(function_id, function, place, source, state);
                        }
                    }
                    SliceSource::Buffer { buffer, channel } => {
                        self.assignment_read_buffer_ref(
                            function_id,
                            function,
                            *buffer,
                            source,
                            state,
                        );
                        self.assignment_read_optional_value(
                            function_id,
                            function,
                            *channel,
                            source,
                            state,
                        );
                    }
                    SliceSource::BufferParam { channel, .. } => {
                        self.assignment_read_optional_value(
                            function_id,
                            function,
                            *channel,
                            source,
                            state,
                        );
                    }
                    SliceSource::ConstData(_) => {}
                }
                self.assignment_read_value(function_id, function, *start, source, state);
                self.assignment_read_value(function_id, function, *len, source, state);
            }
            Rvalue::SliceLoad { slice, index, .. } => {
                for value in [*slice, *index] {
                    self.assignment_read_value(function_id, function, value, source, state);
                }
            }
            Rvalue::SliceLen(value) => {
                self.assignment_read_value(function_id, function, *value, source, state)
            }
        }
    }

    fn assignment_read_optional_value(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: Option<Value>,
        source: SourceSpan,
        state: &AssignmentState,
    ) {
        if let Some(value) = value {
            self.assignment_read_value(function_id, function, value, source, state);
        }
    }

    fn assignment_read_buffer_ref(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        buffer: crate::BufferRef,
        source: SourceSpan,
        state: &AssignmentState,
    ) {
        if let crate::BufferRef::ArrayElement { selector, .. } = buffer {
            self.assignment_read_value(function_id, function, selector, source, state);
        }
    }

    fn assignment_read_value(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: Value,
        source: SourceSpan,
        state: &AssignmentState,
    ) {
        let Value::Local(local) = value else {
            return;
        };
        let Some(initialization) = state.locals.get(local.index()) else {
            return;
        };
        if !initialization
            .covered
            .iter()
            .any(|covered| covered.is_empty())
        {
            self.function_error(
                function_id,
                source,
                format!(
                    "local {} is read before it is definitely assigned",
                    local_display(function, local)
                ),
            );
        }
    }

    fn assignment_read_process_frame(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: Value,
        source: SourceSpan,
        operation: &str,
        state: &AssignmentState,
    ) {
        self.assignment_read_value(function_id, function, value, source, state);
        let Value::Local(local) = value else {
            self.function_error(
                function_id,
                source,
                format!("{operation} frame must be dominated by process_frame"),
            );
            return;
        };
        if !state
            .locals
            .get(local.index())
            .is_some_and(|initialization| initialization.process_frame)
        {
            self.function_error(
                function_id,
                source,
                format!("{operation} frame must be dominated by process_frame"),
            );
        }
    }

    fn assignment_read_place_indices(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        place: &Place,
        source: SourceSpan,
        state: &AssignmentState,
    ) {
        for projection in &place.projections {
            if let crate::Projection::Index { index, .. } = projection {
                self.assignment_read_value(function_id, function, *index, source, state);
            }
        }
    }

    fn assignment_read_place(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        place: &Place,
        source: SourceSpan,
        state: &AssignmentState,
    ) {
        self.assignment_read_place_indices(function_id, function, place, source, state);
        let PlaceBase::Local(local) = place.base else {
            return;
        };
        let Some(initialization) = state.locals.get(local.index()) else {
            return;
        };
        let Some(path) = static_initialization_path(&place.projections) else {
            let prefix = static_initialization_prefix(&place.projections);
            if !initialization_is_covered(&initialization.covered, &prefix) {
                self.function_error(
                    function_id,
                    source,
                    format!(
                        "local {} is indexed before it is definitely assigned",
                        local_display(function, local)
                    ),
                );
            }
            return;
        };
        if !initialization_is_covered(&initialization.covered, &path) {
            self.function_error(
                function_id,
                source,
                format!(
                    "local {} is read before it is definitely assigned",
                    local_display(function, local)
                ),
            );
        }
    }

    fn assignment_write_place(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        place: &Place,
        process_frame: bool,
        source: SourceSpan,
        state: &mut AssignmentState,
    ) {
        self.assignment_read_place_indices(function_id, function, place, source, state);
        let PlaceBase::Local(local) = place.base else {
            return;
        };
        let Some(initialization) = state.locals.get_mut(local.index()) else {
            return;
        };
        if place.projections.is_empty() {
            initialization.covered.clear();
            initialization.covered.insert(Vec::new());
            initialization.process_frame = process_frame;
            return;
        }
        initialization.process_frame = false;
        let Some(path) = static_initialization_path(&place.projections) else {
            return;
        };
        initialization.covered.insert(path.clone());
        let Some(local_ty) = function.locals.get(local.index()).map(|local| local.ty) else {
            return;
        };
        normalize_initialization_coverage(
            self.program,
            local_ty,
            &mut initialization.covered,
            &path,
        );
    }

    fn assignment_write_local(
        &self,
        function: &Function,
        local: crate::LocalId,
        process_frame: bool,
        state: &mut AssignmentState,
    ) {
        let Some(initialization) = state.locals.get_mut(local.index()) else {
            return;
        };
        let _ = function;
        initialization.covered.clear();
        initialization.covered.insert(Vec::new());
        initialization.process_frame = process_frame;
    }

    fn assignment_invalidate_read_write_argument(
        &self,
        argument: &CallArgument,
        state: &mut AssignmentState,
    ) {
        let local = match argument {
            CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => {
                let PlaceBase::Local(local) = place.base else {
                    return;
                };
                local
            }
            CallArgument::SliceWindow {
                slice: Value::Local(local),
                ..
            } => *local,
            CallArgument::Value(_)
            | CallArgument::SliceElement { .. }
            | CallArgument::SliceWindow { .. }
            | CallArgument::Buffer(_)
            | CallArgument::BufferParam(_)
            | CallArgument::BufferSpan(_) => return,
        };
        if let Some(initialization) = state.locals.get_mut(local.index()) {
            // A read-write reference call may replace any value reachable
            // through the argument. Definite initialization remains true
            // because reference arguments are read before the call, but
            // value-specific provenance such as `process_frame` does not.
            initialization.process_frame = false;
        }
    }

    fn validate_rvalue(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: &Rvalue,
        source: SourceSpan,
    ) {
        match value {
            Rvalue::InitAll => {
                if function.kind != FunctionKind::Init {
                    self.function_error(
                        function_id,
                        source,
                        "init_all is only valid in the init entry point",
                    );
                }
            }
            Rvalue::Use(value) => self.validate_value(function_id, function, *value, source),
            Rvalue::Load(place) => self.validate_place(function_id, function, place, source),
            Rvalue::Unary { operand, .. } => {
                self.validate_value(function_id, function, *operand, source)
            }
            Rvalue::Binary { lhs, rhs, .. } => {
                self.validate_value(function_id, function, *lhs, source);
                self.validate_value(function_id, function, *rhs, source);
            }
            Rvalue::Compare { op, lhs, rhs } => {
                self.validate_value(function_id, function, *lhs, source);
                self.validate_value(function_id, function, *rhs, source);
                match (
                    self.value_scalar_type(function, *lhs),
                    self.value_scalar_type(function, *rhs),
                ) {
                    (Some(lhs), Some(rhs)) if lhs == rhs => {
                        if lhs == crate::ScalarType::Bool
                            && !matches!(op, crate::CompareOp::Equal | crate::CompareOp::NotEqual)
                        {
                            self.function_error(
                                function_id,
                                source,
                                "boolean comparisons only support equality and inequality",
                            );
                        }
                    }
                    (Some(lhs), Some(rhs)) => self.function_error(
                        function_id,
                        source,
                        format!(
                            "comparison operands must have the same scalar type, got {} and {}",
                            lhs.name(),
                            rhs.name()
                        ),
                    ),
                    _ => self.function_error(
                        function_id,
                        source,
                        "comparison operands must be scalar values",
                    ),
                }
            }
            Rvalue::Cast { value, to } => {
                self.validate_value(function_id, function, *value, source);
                match self.value_scalar_type(function, *value) {
                    Some(from) if from.is_numeric() && to.is_numeric() => {}
                    Some(from) => self.function_error(
                        function_id,
                        source,
                        format!(
                            "cast requires numeric scalar types, got {} to {}",
                            from.name(),
                            to.name()
                        ),
                    ),
                    None => self.function_error(
                        function_id,
                        source,
                        "cast source must be a numeric scalar value",
                    ),
                }
            }
            Rvalue::Intrinsic { intrinsic, args } => {
                let expected_arity = intrinsic_arity(*intrinsic);
                if args.len() != expected_arity {
                    self.function_error(
                        function_id,
                        source,
                        format!(
                            "intrinsic '{}' expects {expected_arity} arguments, got {}",
                            intrinsic_name(*intrinsic),
                            args.len()
                        ),
                    );
                }

                let numeric = matches!(
                    intrinsic,
                    crate::Intrinsic::Abs
                        | crate::Intrinsic::Min
                        | crate::Intrinsic::Max
                        | crate::Intrinsic::RangeClamp
                        | crate::Intrinsic::RangeWrap
                );
                let mut argument_type = None;
                for (index, arg) in args.iter().enumerate() {
                    self.validate_value(function_id, function, *arg, source);
                    let Some(scalar) = self.value_scalar_type(function, *arg) else {
                        self.function_error(
                            function_id,
                            source,
                            format!(
                                "intrinsic '{}' argument {index} must be a scalar value",
                                intrinsic_name(*intrinsic)
                            ),
                        );
                        continue;
                    };
                    let domain_matches = if *intrinsic == crate::Intrinsic::RangeWrap {
                        matches!(scalar, crate::ScalarType::I32 | crate::ScalarType::I64)
                    } else if numeric {
                        scalar.is_numeric()
                    } else {
                        matches!(scalar, crate::ScalarType::F32 | crate::ScalarType::F64)
                    };
                    if !domain_matches {
                        self.function_error(
                            function_id,
                            source,
                            format!(
                                "intrinsic '{}' argument {index} has unsupported type {}",
                                intrinsic_name(*intrinsic),
                                scalar.name()
                            ),
                        );
                    }
                    if let Some(expected) = argument_type {
                        if scalar != expected {
                            self.function_error(
                                function_id,
                                source,
                                format!(
                                    "intrinsic '{}' arguments must have the same scalar type",
                                    intrinsic_name(*intrinsic)
                                ),
                            );
                        }
                    } else {
                        argument_type = Some(scalar);
                    }
                }
                if *intrinsic == crate::Intrinsic::RangeWrap && args.len() == 3 {
                    let bounds = match (args[1], args[2]) {
                        (
                            crate::Value::Constant(crate::ScalarValue::I32(lower)),
                            crate::Value::Constant(crate::ScalarValue::I32(upper)),
                        ) => Some((i128::from(lower), i128::from(upper))),
                        (
                            crate::Value::Constant(crate::ScalarValue::I64(lower)),
                            crate::Value::Constant(crate::ScalarValue::I64(upper)),
                        ) => Some((i128::from(lower), i128::from(upper))),
                        _ => None,
                    };
                    match bounds {
                        Some((lower, upper)) if lower <= upper => {}
                        Some(_) => self.function_error(
                            function_id,
                            source,
                            "range_wrap lower bound exceeds its upper bound",
                        ),
                        None => self.function_error(
                            function_id,
                            source,
                            format!("range_wrap requires constant integer bounds, got {args:?}"),
                        ),
                    }
                }
            }
            Rvalue::ProcessFrame { offset } => {
                self.require_audio_io_capability(function_id, function, source, "process_frame");
                self.validate_value(function_id, function, *offset, source);
                self.require_i32_value(
                    function_id,
                    function,
                    *offset,
                    source,
                    "process-frame offset",
                );
            }
            Rvalue::InputLoad {
                input,
                element,
                frame,
                ..
            } => {
                self.require_audio_io_capability(function_id, function, source, "audio input load");
                if input.index() >= self.program.interface.inputs.len() {
                    self.function_error(
                        function_id,
                        source,
                        format!("load references missing input {}", input.raw()),
                    );
                }
                self.validate_optional_value(function_id, function, *element, source);
                self.validate_value(function_id, function, *frame, source);
                self.validate_optional_index(
                    function_id,
                    function,
                    *element,
                    source,
                    "input element",
                );
                self.require_process_frame_value(
                    function_id,
                    function,
                    *frame,
                    source,
                    "audio input load",
                );
            }
            Rvalue::OutputLoad {
                output,
                element,
                frame,
                ..
            } => {
                self.require_audio_io_capability(
                    function_id,
                    function,
                    source,
                    "audio output load",
                );
                if output.index() >= self.program.interface.outputs.len() {
                    self.function_error(
                        function_id,
                        source,
                        format!("load references missing output {}", output.raw()),
                    );
                }
                self.validate_optional_value(function_id, function, *element, source);
                self.validate_value(function_id, function, *frame, source);
                self.validate_optional_index(
                    function_id,
                    function,
                    *element,
                    source,
                    "output element",
                );
                self.require_process_frame_value(
                    function_id,
                    function,
                    *frame,
                    source,
                    "audio output load",
                );
            }
            Rvalue::BufferLoad {
                buffer,
                channel,
                index,
                ..
            } => {
                self.require_direct_buffer_capability(function_id, function, source, "buffer load");
                self.require_buffer(function_id, function, *buffer, source);
                self.validate_optional_value(function_id, function, *channel, source);
                self.validate_value(function_id, function, *index, source);
                self.validate_optional_index(
                    function_id,
                    function,
                    *channel,
                    source,
                    "buffer channel",
                );
                self.require_i32_value(function_id, function, *index, source, "buffer index");
            }
            Rvalue::BufferParamLoad {
                parameter,
                channel,
                index,
                ..
            } => {
                self.validate_buffer_param_ref(function_id, function, *parameter, source);
                if self
                    .function_buffer_param_ref(function, *parameter)
                    .is_none()
                {
                    self.function_error(
                        function_id,
                        source,
                        format!(
                            "buffer operation references non-buffer parameter {}",
                            parameter.raw()
                        ),
                    );
                }
                self.validate_optional_value(function_id, function, *channel, source);
                self.validate_value(function_id, function, *index, source);
                self.validate_optional_index(
                    function_id,
                    function,
                    *channel,
                    source,
                    "buffer channel",
                );
                self.require_i32_value(function_id, function, *index, source, "buffer index");
            }
            Rvalue::BufferLen(buffer)
            | Rvalue::BufferChannels(buffer)
            | Rvalue::BufferSampleRate(buffer)
            | Rvalue::BufferIsBound(buffer) => {
                self.require_direct_buffer_capability(
                    function_id,
                    function,
                    source,
                    "buffer metadata query",
                );
                self.require_buffer(function_id, function, *buffer, source);
            }
            Rvalue::BufferParamLen(parameter)
            | Rvalue::BufferParamChannels(parameter)
            | Rvalue::BufferParamSampleRate(parameter)
            | Rvalue::BufferParamIsBound(parameter) => {
                self.validate_buffer_param_ref(function_id, function, *parameter, source);
                if self
                    .function_buffer_param_ref(function, *parameter)
                    .is_none()
                {
                    self.function_error(
                        function_id,
                        source,
                        format!(
                            "buffer metadata references non-buffer parameter {}",
                            parameter.raw()
                        ),
                    );
                }
            }
            Rvalue::ConstDataLoad { data, index, .. } => {
                if data.index() >= self.program.const_data.len() {
                    self.function_error(
                        function_id,
                        source,
                        format!("load references missing const data {}", data.raw()),
                    );
                }
                self.validate_value(function_id, function, *index, source);
                self.require_i32_value(
                    function_id,
                    function,
                    *index,
                    source,
                    "constant-data index",
                );
            }
            Rvalue::MakeSlice {
                source: slice_source,
                start,
                len,
                bounds,
                ..
            } => {
                match slice_source {
                    SliceSource::Place(place) => {
                        self.validate_place(function_id, function, place, source)
                    }
                    SliceSource::Buffer { buffer, channel } => {
                        self.require_direct_buffer_capability(
                            function_id,
                            function,
                            source,
                            "buffer slice",
                        );
                        self.require_buffer(function_id, function, *buffer, source);
                        self.validate_optional_value(function_id, function, *channel, source);
                        self.validate_optional_index(
                            function_id,
                            function,
                            *channel,
                            source,
                            "buffer slice channel",
                        );
                    }
                    SliceSource::BufferParam { parameter, channel } => {
                        self.validate_buffer_param_ref(function_id, function, *parameter, source);
                        if self
                            .function_buffer_param_ref(function, *parameter)
                            .is_none()
                        {
                            self.function_error(
                                function_id,
                                source,
                                format!(
                                    "slice references non-buffer parameter {}",
                                    parameter.raw()
                                ),
                            );
                        }
                        self.validate_optional_value(function_id, function, *channel, source);
                        self.validate_optional_index(
                            function_id,
                            function,
                            *channel,
                            source,
                            "buffer slice channel",
                        );
                    }
                    SliceSource::ConstData(data) => {
                        if data.index() >= self.program.const_data.len() {
                            self.function_error(
                                function_id,
                                source,
                                format!("slice references missing const data {}", data.raw()),
                            );
                        }
                    }
                }
                self.validate_value(function_id, function, *start, source);
                self.validate_value(function_id, function, *len, source);
                self.require_i32_value(function_id, function, *start, source, "slice start");
                self.require_i32_value(function_id, function, *len, source, "slice length");
                if *bounds == crate::BoundsMode::Unchecked
                    && !self.unchecked_make_slice_constants_are_valid(
                        function,
                        slice_source,
                        *start,
                        *len,
                    )
                {
                    self.function_error(
                        function_id,
                        source,
                        "unchecked make_slice has a statically out-of-bounds range",
                    );
                }
            }
            Rvalue::SliceLoad { slice, index, .. } => {
                self.validate_value(function_id, function, *slice, source);
                self.validate_value(function_id, function, *index, source);
                self.require_i32_value(function_id, function, *index, source, "slice index");
                if self.value_slice_type(function, *slice).is_none() {
                    self.function_error(function_id, source, "slice load source is not a slice");
                }
            }
            Rvalue::SliceLen(value) => {
                self.validate_value(function_id, function, *value, source);
                if self.value_slice_type(function, *value).is_none() {
                    self.function_error(function_id, source, "slice length source is not a slice");
                }
            }
        }
    }

    fn validate_place(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        place: &Place,
        source: SourceSpan,
    ) {
        let mut ty = match place.base {
            PlaceBase::Local(local) => {
                self.require_local(function_id, function, local, source);
                function.locals.get(local.index()).map(|local| local.ty)
            }
            PlaceBase::Parameter(parameter) => {
                if parameter.index() >= function.params.len() {
                    self.function_error(
                        function_id,
                        source,
                        format!(
                            "place references missing function parameter {}",
                            parameter.raw()
                        ),
                    );
                }
                function.params.get(parameter.index()).map(|param| param.ty)
            }
            PlaceBase::State(state) => {
                if state.index() >= self.program.state.len() {
                    self.function_error(
                        function_id,
                        source,
                        format!("place references missing state slot {}", state.raw()),
                    );
                }
                self.program.state.get(state.index()).map(|state| state.ty)
            }
            PlaceBase::Param(param) => {
                if param.index() >= self.program.interface.params.len() {
                    self.function_error(
                        function_id,
                        source,
                        format!("place references missing parameter {}", param.raw()),
                    );
                }
                self.program
                    .interface
                    .params
                    .get(param.index())
                    .map(|param| param.ty)
            }
            PlaceBase::EventParam(event_param) => {
                let FunctionKind::Event(event_id) = function.kind else {
                    self.function_error(
                        function_id,
                        source,
                        "event parameter place used outside an event handler",
                    );
                    return;
                };
                let Some(event) = self.program.interface.events.get(event_id.index()) else {
                    self.function_error(
                        function_id,
                        source,
                        format!("event handler references missing event {}", event_id.raw()),
                    );
                    return;
                };
                if event_param.index() >= event.params.len() {
                    self.function_error(
                        function_id,
                        source,
                        format!(
                            "place references missing event parameter {}",
                            event_param.raw()
                        ),
                    );
                }
                event.params.get(event_param.index()).map(|param| param.ty)
            }
        };
        for projection in &place.projections {
            match projection {
                crate::Projection::Field(field) => {
                    let Some(current) = ty else {
                        continue;
                    };
                    match self.program.types.get(current.index()) {
                        Some(Type::Struct(structure)) => {
                            let Some(structure) = self.program.structs.get(structure.index())
                            else {
                                self.function_error(
                                    function_id,
                                    source,
                                    "field projection references a missing struct definition",
                                );
                                ty = None;
                                continue;
                            };
                            let Some(projected) = structure.fields.get(field.index()) else {
                                self.function_error(
                                    function_id,
                                    source,
                                    format!(
                                        "field projection references missing field {} of struct '{}'",
                                        field.raw(),
                                        structure.name
                                    ),
                                );
                                ty = None;
                                continue;
                            };
                            ty = Some(projected.ty);
                        }
                        Some(_) => {
                            self.function_error(
                                function_id,
                                source,
                                format!(
                                    "field projection requires a struct place, got {}",
                                    self.type_name(current)
                                ),
                            );
                            ty = None;
                        }
                        None => {
                            self.function_error(
                                function_id,
                                source,
                                format!(
                                    "field projection base references missing type {}",
                                    current.raw()
                                ),
                            );
                            ty = None;
                        }
                    }
                }
                crate::Projection::Index { index, .. } => {
                    self.validate_value(function_id, function, *index, source);
                    self.require_i32_value(function_id, function, *index, source, "place index");
                    let Some(current) = ty else {
                        continue;
                    };
                    match self.program.types.get(current.index()) {
                        Some(Type::Array { element, .. }) => ty = Some(*element),
                        Some(_) => {
                            self.function_error(
                                function_id,
                                source,
                                format!(
                                    "index projection requires an array place, got {}",
                                    self.type_name(current)
                                ),
                            );
                            ty = None;
                        }
                        None => {
                            self.function_error(
                                function_id,
                                source,
                                format!(
                                    "index projection base references missing type {}",
                                    current.raw()
                                ),
                            );
                            ty = None;
                        }
                    }
                }
            }
        }
    }

    fn place_type(&self, function: &Function, place: &Place) -> Option<crate::TypeId> {
        let mut ty = match place.base {
            PlaceBase::Local(local) => function.locals.get(local.index())?.ty,
            PlaceBase::Parameter(parameter) => function.params.get(parameter.index())?.ty,
            PlaceBase::State(state) => self.program.state.get(state.index())?.ty,
            PlaceBase::Param(param) => self.program.interface.params.get(param.index())?.ty,
            PlaceBase::EventParam(event_param) => {
                let FunctionKind::Event(event) = function.kind else {
                    return None;
                };
                self.program
                    .interface
                    .events
                    .get(event.index())?
                    .params
                    .get(event_param.index())?
                    .ty
            }
        };
        for projection in &place.projections {
            ty = match projection {
                crate::Projection::Field(field) => {
                    let Type::Struct(structure) = self.program.types.get(ty.index())? else {
                        return None;
                    };
                    self.program
                        .structs
                        .get(structure.index())?
                        .fields
                        .get(field.index())?
                        .ty
                }
                crate::Projection::Index { .. } => {
                    let Type::Array { element, .. } = self.program.types.get(ty.index())? else {
                        return None;
                    };
                    *element
                }
            };
        }
        Some(ty)
    }

    fn place_is_writable(&self, function: &Function, place: &Place) -> bool {
        match place.base {
            PlaceBase::Local(_) => true,
            PlaceBase::State(state) => self
                .program
                .state
                .get(state.index())
                .is_some_and(|slot| slot.persistence != crate::StatePersistence::ControlMirror),
            PlaceBase::Parameter(parameter) => function
                .params
                .get(parameter.index())
                .is_some_and(|param| param.mode == crate::PassingMode::ReadWriteReference),
            PlaceBase::Param(_) | PlaceBase::EventParam(_) => false,
        }
    }

    fn rvalue_matches_type(
        &self,
        function: &Function,
        value: &Rvalue,
        expected: crate::TypeId,
    ) -> bool {
        match value {
            Rvalue::Use(value) => self.value_matches_type(function, *value, expected),
            Rvalue::Load(place) => self
                .place_type(function, place)
                .is_some_and(|actual| self.program.types_equivalent(actual, expected)),
            Rvalue::Unary { op, operand } => match op {
                crate::UnaryOp::LogicalNot => {
                    self.type_is_scalar(expected, crate::ScalarType::Bool)
                        && self.value_matches_scalar(function, *operand, crate::ScalarType::Bool)
                }
                crate::UnaryOp::Negate => {
                    self.type_is_numeric_scalar(expected)
                        && self.value_matches_type(function, *operand, expected)
                }
                crate::UnaryOp::BitNot => {
                    self.type_is_integer_scalar(expected)
                        && self.value_matches_type(function, *operand, expected)
                }
            },
            Rvalue::Binary { op, lhs, rhs } => {
                let valid_result = if matches!(
                    op,
                    crate::BinaryOp::BitAnd
                        | crate::BinaryOp::BitOr
                        | crate::BinaryOp::BitXor
                        | crate::BinaryOp::ShiftLeft
                        | crate::BinaryOp::ShiftRight
                ) {
                    self.type_is_integer_scalar(expected)
                } else {
                    self.type_is_numeric_scalar(expected)
                };
                valid_result
                    && self.value_matches_type(function, *lhs, expected)
                    && self.value_matches_type(function, *rhs, expected)
            }
            Rvalue::Compare { lhs, rhs, .. } => {
                self.type_is_scalar(expected, crate::ScalarType::Bool)
                    && self.values_have_same_type(function, *lhs, *rhs)
            }
            Rvalue::Cast { to, .. } => self.type_is_scalar(expected, *to),
            Rvalue::Intrinsic { args, .. } => {
                self.type_is_numeric_scalar(expected)
                    && args
                        .iter()
                        .all(|arg| self.value_matches_type(function, *arg, expected))
            }
            Rvalue::InitAll => self.type_is_scalar(expected, crate::ScalarType::Bool),
            Rvalue::ProcessFrame { .. } => self.type_is_scalar(expected, crate::ScalarType::I32),
            Rvalue::InputLoad { input, element, .. } => self
                .program
                .interface
                .inputs
                .get(input.index())
                .and_then(|input| self.indexed_type(input.ty, element.is_some()))
                .is_some_and(|actual| self.program.types_equivalent(actual, expected)),
            Rvalue::OutputLoad {
                output, element, ..
            } => self
                .program
                .interface
                .outputs
                .get(output.index())
                .and_then(|output| self.indexed_type(output.ty, element.is_some()))
                .is_some_and(|actual| self.program.types_equivalent(actual, expected)),
            Rvalue::BufferLoad { buffer, .. } => self
                .program
                .interface
                .buffers
                .get(buffer.index())
                .is_some_and(|buffer| self.type_is_scalar(expected, buffer.element)),
            Rvalue::BufferParamLoad { parameter, .. } => self
                .function_buffer_param_ref(function, *parameter)
                .is_some_and(|(element, _)| self.type_is_scalar(expected, element)),
            Rvalue::BufferLen(_)
            | Rvalue::BufferChannels(_)
            | Rvalue::BufferParamLen(_)
            | Rvalue::BufferParamChannels(_) => {
                self.type_is_scalar(expected, crate::ScalarType::I32)
            }
            Rvalue::BufferSampleRate(_) | Rvalue::BufferParamSampleRate(_) => {
                self.type_is_scalar(expected, crate::ScalarType::F32)
            }
            Rvalue::BufferIsBound(_) | Rvalue::BufferParamIsBound(_) => {
                self.type_is_scalar(expected, crate::ScalarType::Bool)
            }
            Rvalue::ConstDataLoad { data, .. } => self
                .program
                .const_data
                .get(data.index())
                .is_some_and(|data| self.type_is_scalar(expected, data.element)),
            Rvalue::MakeSlice { source, access, .. } => {
                self.program.types.get(expected.index()).is_some_and(|ty| {
                    let Type::Slice {
                        element,
                        access: expected_access,
                    } = ty
                    else {
                        return false;
                    };
                    *expected_access == *access
                        && self.slice_source_type(function, source).is_some_and(
                            |(source_element, source_access)| {
                                source_element == *element && access_permits(source_access, *access)
                            },
                        )
                })
            }
            Rvalue::SliceLoad { slice, .. } => self
                .value_slice_type(function, *slice)
                .is_some_and(|(element, _)| self.type_is_scalar(expected, element)),
            Rvalue::SliceLen(_) => self.type_is_scalar(expected, crate::ScalarType::I32),
        }
    }

    fn value_type(&self, function: &Function, value: Value) -> Option<crate::TypeId> {
        match value {
            Value::Local(local) => function.locals.get(local.index()).map(|local| local.ty),
            Value::Constant(value) => self
                .program
                .types
                .iter()
                .position(|ty| *ty == Type::Scalar(value.ty()))
                .map(|index| crate::TypeId::new(index as u32)),
        }
    }

    fn value_slice_type(
        &self,
        function: &Function,
        value: Value,
    ) -> Option<(crate::ScalarType, crate::AccessMode)> {
        let ty = self.value_type(function, value)?;
        match self.program.types.get(ty.index())? {
            Type::Slice { element, access } => Some((*element, *access)),
            _ => None,
        }
    }

    fn value_scalar_type(&self, function: &Function, value: Value) -> Option<crate::ScalarType> {
        match value {
            Value::Local(local) => {
                let ty = function.locals.get(local.index())?.ty;
                let Type::Scalar(scalar) = self.program.types.get(ty.index())? else {
                    return None;
                };
                Some(*scalar)
            }
            Value::Constant(value) => Some(value.ty()),
        }
    }

    fn slice_source_type(
        &self,
        function: &Function,
        source: &SliceSource,
    ) -> Option<(crate::ScalarType, crate::AccessMode)> {
        match source {
            SliceSource::Place(place) => {
                let ty = self.place_type(function, place)?;
                match self.program.types.get(ty.index())? {
                    Type::Array { element, .. } => {
                        let Type::Scalar(element) = self.program.types.get(element.index())? else {
                            return None;
                        };
                        Some((
                            *element,
                            if self.place_is_writable(function, place) {
                                crate::AccessMode::ReadWrite
                            } else {
                                crate::AccessMode::ReadOnly
                            },
                        ))
                    }
                    Type::Slice { element, access } => Some((*element, *access)),
                    _ => None,
                }
            }
            SliceSource::Buffer { buffer, .. } => self
                .program
                .interface
                .buffers
                .get(buffer.index())
                .map(|buffer| (buffer.element, buffer.access)),
            SliceSource::BufferParam { parameter, .. } => {
                self.function_buffer_param_ref(function, *parameter)
            }
            SliceSource::ConstData(data) => self
                .program
                .const_data
                .get(data.index())
                .map(|data| (data.element, crate::AccessMode::ReadOnly)),
        }
    }

    fn slice_source_static_len(&self, function: &Function, source: &SliceSource) -> Option<u32> {
        match source {
            SliceSource::Place(place) => {
                let ty = self.place_type(function, place)?;
                match self.program.types.get(ty.index())? {
                    Type::Array { len, .. } => Some(*len),
                    Type::Slice { .. } => None,
                    _ => None,
                }
            }
            SliceSource::ConstData(data) => self
                .program
                .const_data
                .get(data.index())
                .and_then(|data| u32::try_from(data.values.len()).ok()),
            SliceSource::Buffer { .. } | SliceSource::BufferParam { .. } => None,
        }
    }

    fn unchecked_make_slice_constants_are_valid(
        &self,
        function: &Function,
        source: &SliceSource,
        start: Value,
        len: Value,
    ) -> bool {
        let Some(source_len) = self.slice_source_static_len(function, source) else {
            return true;
        };
        let (
            Value::Constant(crate::ScalarValue::I32(start)),
            Value::Constant(crate::ScalarValue::I32(len)),
        ) = (start, len)
        else {
            return true;
        };
        start >= 0
            && len >= 0
            && i64::from(start) <= i64::from(source_len)
            && i64::from(len) <= i64::from(source_len) - i64::from(start)
    }

    fn window_start_is_statically_valid(
        &self,
        start: Value,
        bounds: crate::BoundsMode,
        source_len: u32,
        required_len: u32,
    ) -> bool {
        if bounds != crate::BoundsMode::Unchecked {
            return true;
        }
        let Value::Constant(crate::ScalarValue::I32(start)) = start else {
            return true;
        };
        start >= 0 && i64::from(start) + i64::from(required_len) <= i64::from(source_len)
    }

    fn validate_value(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: Value,
        source: SourceSpan,
    ) {
        if let Value::Local(local) = value {
            self.require_local(function_id, function, local, source);
        }
    }

    fn value_matches_type(
        &self,
        function: &Function,
        value: Value,
        expected: crate::TypeId,
    ) -> bool {
        match value {
            Value::Local(local) => function
                .locals
                .get(local.index())
                .is_some_and(|local| self.program.types_equivalent(local.ty, expected)),
            Value::Constant(value) => self
                .program
                .types
                .get(expected.index())
                .is_some_and(|ty| *ty == Type::Scalar(value.ty())),
        }
    }

    fn value_matches_scalar(
        &self,
        function: &Function,
        value: Value,
        expected: crate::ScalarType,
    ) -> bool {
        match value {
            Value::Local(local) => {
                function
                    .locals
                    .get(local.index())
                    .and_then(|local| self.program.types.get(local.ty.index()))
                    == Some(&Type::Scalar(expected))
            }
            Value::Constant(value) => value.ty() == expected,
        }
    }

    fn values_have_same_type(&self, function: &Function, lhs: Value, rhs: Value) -> bool {
        match lhs {
            Value::Local(local) => function
                .locals
                .get(local.index())
                .is_some_and(|local| self.value_matches_type(function, rhs, local.ty)),
            Value::Constant(value) => self.value_matches_scalar(function, rhs, value.ty()),
        }
    }

    fn call_argument_matches(
        &self,
        function: &Function,
        argument: &CallArgument,
        parameter: &crate::FunctionParam,
    ) -> bool {
        match (parameter.mode, argument) {
            (crate::PassingMode::Value, CallArgument::Value(value)) => {
                self.value_matches_type(function, *value, parameter.ty)
            }
            (crate::PassingMode::Value, CallArgument::BufferSpan(span)) => {
                self.buffer_span_matches_type(function, *span, parameter.ty)
            }
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::Place(place),
            ) => self.place_type(function, place).is_some_and(|actual| {
                self.reference_type_matches(actual, parameter.ty)
                    && (parameter.mode != crate::PassingMode::ReadWriteReference
                        || self.place_is_writable(function, place))
            }),
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::SliceElement { slice, .. },
            ) => {
                let requested_access = match parameter.mode {
                    crate::PassingMode::ReadOnlyReference => crate::AccessMode::ReadOnly,
                    crate::PassingMode::ReadWriteReference => crate::AccessMode::ReadWrite,
                    crate::PassingMode::Value => unreachable!(),
                };
                self.value_slice_type(function, *slice).is_some_and(
                    |(slice_element, slice_access)| {
                        access_permits(slice_access, requested_access)
                            && match self.program.types.get(parameter.ty.index()) {
                                Some(Type::Scalar(expected)) => *expected == slice_element,
                                _ => false,
                            }
                    },
                )
            }
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::ArrayWindow {
                    array,
                    start,
                    bounds,
                },
            ) => {
                let requested_access = match parameter.mode {
                    crate::PassingMode::ReadOnlyReference => crate::AccessMode::ReadOnly,
                    crate::PassingMode::ReadWriteReference => crate::AccessMode::ReadWrite,
                    crate::PassingMode::Value => unreachable!(),
                };
                let Some(Type::Array {
                    element: expected_element,
                    len: required_len,
                }) = self.program.types.get(parameter.ty.index())
                else {
                    return false;
                };
                let Some(actual_ty) = self.place_type(function, array) else {
                    return false;
                };
                let Some(Type::Array {
                    element: actual_element,
                    len: actual_len,
                }) = self.program.types.get(actual_ty.index())
                else {
                    return false;
                };
                self.program
                    .types_equivalent(*actual_element, *expected_element)
                    && required_len <= actual_len
                    && access_permits(
                        if self.place_is_writable(function, array) {
                            crate::AccessMode::ReadWrite
                        } else {
                            crate::AccessMode::ReadOnly
                        },
                        requested_access,
                    )
                    && self.window_start_is_statically_valid(
                        *start,
                        *bounds,
                        *actual_len,
                        *required_len,
                    )
            }
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::SliceWindow { slice, .. },
            ) => {
                let requested_access = match parameter.mode {
                    crate::PassingMode::ReadOnlyReference => crate::AccessMode::ReadOnly,
                    crate::PassingMode::ReadWriteReference => crate::AccessMode::ReadWrite,
                    crate::PassingMode::Value => unreachable!(),
                };
                let Some(Type::Array { element, .. }) =
                    self.program.types.get(parameter.ty.index())
                else {
                    return false;
                };
                let Some(Type::Scalar(expected_element)) = self.program.types.get(element.index())
                else {
                    return false;
                };
                self.value_slice_type(function, *slice).is_some_and(
                    |(slice_element, slice_access)| {
                        slice_element == *expected_element
                            && access_permits(slice_access, requested_access)
                    },
                )
            }
            (_, CallArgument::Buffer(buffer)) => self.buffer_matches_type(*buffer, parameter.ty),
            (
                crate::PassingMode::ReadOnlyReference | crate::PassingMode::ReadWriteReference,
                CallArgument::BufferParam(reference),
            ) => self.buffer_param_ref_matches_type(function, *reference, parameter.ty),
            _ => false,
        }
    }

    fn buffer_matches_type(&self, buffer: crate::BufferRef, expected: crate::TypeId) -> bool {
        let Some(first_buffer) = self.program.interface.buffers.get(buffer.index()) else {
            return false;
        };
        let matches = self.program.types.get(expected.index()).is_some_and(|ty| {
            matches!(
                ty,
                Type::Buffer {
                    element,
                    channels,
                    access,
                } if *element == first_buffer.element
                    && buffer_channels_accept(*channels, first_buffer.channels)
                    && access_permits(first_buffer.access, *access)
            )
        });
        matches
            && buffer.possible_indices().all(|index| {
                self.program
                    .interface
                    .buffers
                    .get(index)
                    .is_some_and(|candidate| {
                        candidate.element == first_buffer.element
                            && candidate.channels == first_buffer.channels
                            && candidate.access == first_buffer.access
                    })
            })
    }

    fn buffer_span_matches_type(
        &self,
        function: &Function,
        span: crate::BufferSpanRef,
        expected: crate::TypeId,
    ) -> bool {
        let Some(Type::BufferSpan {
            element: expected_element,
            channels: expected_channels,
            access: expected_access,
            len: expected_len,
        }) = self.program.types.get(expected.index())
        else {
            return false;
        };
        match span {
            crate::BufferSpanRef::Interface { first, len } => {
                if len != *expected_len {
                    return false;
                }
                let Some(source) = self.program.interface.buffers.get(first.index()) else {
                    return false;
                };
                source.element == *expected_element
                    && buffer_channels_accept(*expected_channels, source.channels)
                    && access_permits(source.access, *expected_access)
                    && (first.index()..first.index().saturating_add(len as usize)).all(|index| {
                        self.program
                            .interface
                            .buffers
                            .get(index)
                            .is_some_and(|candidate| {
                                candidate.element == source.element
                                    && candidate.channels == source.channels
                                    && candidate.access == source.access
                            })
                    })
            }
            crate::BufferSpanRef::Parameter { span, start, len } => {
                if len != *expected_len {
                    return false;
                }
                let Some(source) = function.params.get(span.index()) else {
                    return false;
                };
                let Some(Type::BufferSpan {
                    element,
                    channels,
                    access,
                    len: source_len,
                }) = self.program.types.get(source.ty.index())
                else {
                    return false;
                };
                start.checked_add(len).is_some_and(|end| end <= *source_len)
                    && element == expected_element
                    && buffer_channels_accept(*expected_channels, *channels)
                    && access_permits(*access, *expected_access)
            }
        }
    }

    fn reference_type_matches(&self, actual: crate::TypeId, expected: crate::TypeId) -> bool {
        match (
            self.program.types.get(actual.index()),
            self.program.types.get(expected.index()),
        ) {
            (
                Some(Type::Buffer {
                    element: actual_element,
                    channels: actual_channels,
                    access: actual_access,
                }),
                Some(Type::Buffer {
                    element: expected_element,
                    channels: expected_channels,
                    access: expected_access,
                }),
            ) => {
                actual_element == expected_element
                    && buffer_channels_accept(*expected_channels, *actual_channels)
                    && access_permits(*actual_access, *expected_access)
            }
            _ => self.program.types_equivalent(actual, expected),
        }
    }

    fn function_buffer_param(
        &self,
        function: &Function,
        parameter: crate::ParameterId,
    ) -> Option<(crate::ScalarType, crate::AccessMode)> {
        let parameter = function.params.get(parameter.index())?;
        match self.program.types.get(parameter.ty.index())? {
            Type::Buffer {
                element, access, ..
            } => Some((*element, *access)),
            _ => None,
        }
    }

    fn buffer_param_ref_matches_type(
        &self,
        function: &Function,
        reference: crate::BufferParamRef,
        expected: crate::TypeId,
    ) -> bool {
        let actual = function
            .params
            .get(reference.index())
            .and_then(|parameter| self.program.types.get(parameter.ty.index()));
        let (actual_element, actual_channels, actual_access) = match (reference, actual) {
            (
                crate::BufferParamRef::Direct(_),
                Some(Type::Buffer {
                    element,
                    channels,
                    access,
                }),
            )
            | (
                crate::BufferParamRef::ArrayElement { .. },
                Some(Type::BufferSpan {
                    element,
                    channels,
                    access,
                    ..
                }),
            ) => (*element, *channels, *access),
            _ => return false,
        };
        matches!(
            self.program.types.get(expected.index()),
            Some(Type::Buffer {
                element,
                channels,
                access,
            }) if *element == actual_element
                && buffer_channels_accept(*channels, actual_channels)
                && access_permits(actual_access, *access)
        )
    }

    fn function_buffer_param_ref(
        &self,
        function: &Function,
        reference: crate::BufferParamRef,
    ) -> Option<(crate::ScalarType, crate::AccessMode)> {
        match reference {
            crate::BufferParamRef::Direct(parameter) => {
                self.function_buffer_param(function, parameter)
            }
            crate::BufferParamRef::ArrayElement { span, .. } => {
                let parameter = function.params.get(span.index())?;
                match self.program.types.get(parameter.ty.index())? {
                    Type::BufferSpan {
                        element, access, ..
                    } => Some((*element, *access)),
                    _ => None,
                }
            }
        }
    }

    fn validate_buffer_param_ref(
        &mut self,
        function_id: crate::FunctionId,
        function: &Function,
        reference: crate::BufferParamRef,
        source: crate::SourceSpan,
    ) {
        if let crate::BufferParamRef::ArrayElement { selector, .. } = reference {
            self.validate_value(function_id, function, selector, source);
            self.require_i32_value(
                function_id,
                function,
                selector,
                source,
                "buffer-parameter collection selector",
            );
        }
    }

    fn indexed_type(&self, ty: crate::TypeId, indexed: bool) -> Option<crate::TypeId> {
        if !indexed {
            return Some(ty);
        }
        match self.program.types.get(ty.index())? {
            Type::Array { element, .. } => Some(*element),
            _ => None,
        }
    }

    fn type_is_scalar(&self, ty: crate::TypeId, expected: crate::ScalarType) -> bool {
        self.program.types.get(ty.index()) == Some(&Type::Scalar(expected))
    }

    fn type_is_numeric_scalar(&self, ty: crate::TypeId) -> bool {
        self.program
            .types
            .get(ty.index())
            .is_some_and(|ty| matches!(ty, Type::Scalar(scalar) if scalar.is_numeric()))
    }

    fn type_is_integer_scalar(&self, ty: crate::TypeId) -> bool {
        matches!(
            self.program.types.get(ty.index()),
            Some(Type::Scalar(
                crate::ScalarType::I32 | crate::ScalarType::I64
            ))
        )
    }

    fn type_name(&self, ty: crate::TypeId) -> String {
        match self.program.types.get(ty.index()) {
            Some(Type::Scalar(scalar)) => scalar.name().to_owned(),
            Some(other) => format!("{other:?}"),
            None => format!("missing type {}", ty.raw()),
        }
    }

    fn constant_matches_type(&self, value: &crate::ConstantValue, ty: crate::TypeId) -> bool {
        let Some(ty) = self.program.types.get(ty.index()) else {
            return false;
        };
        match (value, ty) {
            (crate::ConstantValue::Scalar(value), Type::Scalar(expected)) => {
                value.ty() == *expected
            }
            (crate::ConstantValue::Aggregate(values), Type::Tuple(elements)) => {
                values.len() == elements.len()
                    && values
                        .iter()
                        .zip(elements)
                        .all(|(value, element)| self.constant_matches_type(value, *element))
            }
            (crate::ConstantValue::Aggregate(values), Type::Array { element, len }) => {
                values.len() == *len as usize
                    && values
                        .iter()
                        .all(|value| self.constant_matches_type(value, *element))
            }
            (crate::ConstantValue::Aggregate(values), Type::Struct(structure)) => self
                .program
                .structs
                .get(structure.index())
                .is_some_and(|structure| {
                    values.len() == structure.fields.len()
                        && values
                            .iter()
                            .zip(&structure.fields)
                            .all(|(value, field)| self.constant_matches_type(value, field.ty))
                }),
            _ => false,
        }
    }

    fn value_range_validation_error(
        &self,
        range: crate::ValueRange,
        ty: crate::TypeId,
    ) -> Option<String> {
        let Some(Type::Scalar(expected)) = self.program.types.get(ty.index()) else {
            return Some(format!(
                "does not apply to non-scalar type {}",
                self.type_name(ty)
            ));
        };
        if range.min.ty() != *expected || range.max.ty() != *expected {
            return Some(format!("does not match scalar type {}", self.type_name(ty)));
        }
        match (range.min, range.max) {
            (crate::ScalarValue::F32(min), crate::ScalarValue::F32(max)) => {
                if !min.is_finite() || !max.is_finite() {
                    Some("endpoints must be finite".to_owned())
                } else if min > max {
                    Some("minimum is greater than its maximum".to_owned())
                } else {
                    None
                }
            }
            (crate::ScalarValue::F64(min), crate::ScalarValue::F64(max)) => {
                if !min.is_finite() || !max.is_finite() {
                    Some("endpoints must be finite".to_owned())
                } else if min > max {
                    Some("minimum is greater than its maximum".to_owned())
                } else {
                    None
                }
            }
            (crate::ScalarValue::I32(min), crate::ScalarValue::I32(max)) => {
                (min > max).then(|| "minimum is greater than its maximum".to_owned())
            }
            (crate::ScalarValue::I64(min), crate::ScalarValue::I64(max)) => {
                (min > max).then(|| "minimum is greater than its maximum".to_owned())
            }
            (crate::ScalarValue::Bool(_), crate::ScalarValue::Bool(_)) => {
                Some("is not supported for bool".to_owned())
            }
            _ => Some(format!("does not match scalar type {}", self.type_name(ty))),
        }
    }

    fn integer_range_validation_error(
        &self,
        range: crate::IntegerRangeInvariant,
        ty: crate::TypeId,
    ) -> Option<String> {
        if let Some(reason) = self.value_range_validation_error(
            crate::ValueRange {
                min: range.min,
                max: range.max,
            },
            ty,
        ) {
            return Some(reason);
        }
        match range.min.ty() {
            crate::ScalarType::I32 | crate::ScalarType::I64 => None,
            _ => Some("requires an i32 or i64 scalar type".to_owned()),
        }
    }

    fn param_control_validation_error(
        &self,
        param: &crate::Param,
        range: crate::ValueRange,
    ) -> Option<String> {
        use crate::{ParamScale, ScalarType, ScalarValue, Type};

        if param
            .control
            .unit
            .as_ref()
            .is_some_and(|unit| unit.contains('\0'))
        {
            return Some("unit must not contain a NUL character".to_owned());
        }
        let Some(Type::Scalar(ty)) = self.program.types.get(param.ty.index()) else {
            return Some("requires a scalar parameter".to_owned());
        };
        if let (ScalarValue::I64(min), ScalarValue::I64(max)) = (range.min, range.max) {
            let exact_limit = i128::from(crate::MAX_EXACT_HOST_CONTROL_INTEGER);
            let width = i128::from(max) - i128::from(min);
            if i128::from(min).abs() > exact_limit
                || i128::from(max).abs() > exact_limit
                || width > exact_limit
            {
                return Some(
                    "i64 range and width must fit the exact host integer range".to_owned(),
                );
            }
        }
        let min = range.min.as_f64();
        let max = range.max.as_f64();
        if min >= max {
            return Some("range requires min < max".to_owned());
        }
        if param.control.curve.is_some_and(|curve| !curve.is_finite()) {
            return Some("curve must be finite".to_owned());
        }
        if param.control.scale == ParamScale::Log {
            if param.control.curve.is_some() {
                return Some("cannot combine logarithmic scale with curve".to_owned());
            }
            if !matches!(ty, ScalarType::F32 | ScalarType::F64) {
                return Some("logarithmic scale requires f32 or f64".to_owned());
            }
            if min <= 0.0 {
                return Some("logarithmic scale requires min > 0".to_owned());
            }
            if param.control.step.is_some() || param.control.step_count.is_some() {
                return Some("cannot combine logarithmic scale with step".to_owned());
            }
        }

        let (Some(step), Some(step_count)) = (param.control.step, param.control.step_count) else {
            if param.control.step.is_some() != param.control.step_count.is_some() {
                return Some(
                    "step and step_count must either both be present or absent".to_owned(),
                );
            }
            if matches!(ty, ScalarType::I32 | ScalarType::I64) {
                return Some("integer ranges require a step".to_owned());
            }
            return None;
        };
        if step.ty() != *ty {
            return Some("step does not match the parameter scalar type".to_owned());
        }
        if step_count == 0 {
            return Some("step_count must be greater than zero".to_owned());
        }

        let default = match &param.default {
            crate::ConstantValue::Scalar(value) => *value,
            crate::ConstantValue::Aggregate(_) => {
                return Some("requires a scalar default".to_owned())
            }
        };
        match (range.min, range.max, default, step) {
            (
                ScalarValue::I32(min),
                ScalarValue::I32(max),
                ScalarValue::I32(default),
                ScalarValue::I32(step),
            ) => {
                if step <= 0 {
                    return Some("step must be greater than zero".to_owned());
                }
                let width = i64::from(max) - i64::from(min);
                let step = i64::from(step);
                if width % step != 0 || u32::try_from(width / step).ok() != Some(step_count) {
                    return Some("step_count does not match the range and step".to_owned());
                }
                if (i64::from(default) - i64::from(min)) % step != 0 {
                    return Some("default is not on the step grid".to_owned());
                }
            }
            (
                ScalarValue::I64(min),
                ScalarValue::I64(max),
                ScalarValue::I64(default),
                ScalarValue::I64(step),
            ) => {
                let width = i128::from(max) - i128::from(min);
                if step <= 0 {
                    return Some("step must be greater than zero".to_owned());
                }
                let step = i128::from(step);
                if width % step != 0 || u32::try_from(width / step).ok() != Some(step_count) {
                    return Some("step_count does not match the range and step".to_owned());
                }
                if (i128::from(default) - i128::from(min)) % step != 0 {
                    return Some("default is not on the step grid".to_owned());
                }
            }
            (
                ScalarValue::F32(min),
                ScalarValue::F32(max),
                ScalarValue::F32(default),
                ScalarValue::F32(step),
            ) => {
                return validate_float_param_control_grid(
                    ScalarType::F32,
                    min as f64,
                    max as f64,
                    default as f64,
                    step as f64,
                    step_count,
                );
            }
            (
                ScalarValue::F64(min),
                ScalarValue::F64(max),
                ScalarValue::F64(default),
                ScalarValue::F64(step),
            ) => {
                return validate_float_param_control_grid(
                    ScalarType::F64,
                    min,
                    max,
                    default,
                    step,
                    step_count,
                );
            }
            _ => return Some("step requires a numeric scalar parameter".to_owned()),
        }
        None
    }

    fn constant_is_within_range(
        &self,
        value: &crate::ConstantValue,
        range: crate::ValueRange,
    ) -> bool {
        match (value, range.min, range.max) {
            (
                crate::ConstantValue::Scalar(crate::ScalarValue::F32(value)),
                crate::ScalarValue::F32(min),
                crate::ScalarValue::F32(max),
            ) => value.is_finite() && *value >= min && *value <= max,
            (
                crate::ConstantValue::Scalar(crate::ScalarValue::F64(value)),
                crate::ScalarValue::F64(min),
                crate::ScalarValue::F64(max),
            ) => value.is_finite() && *value >= min && *value <= max,
            (
                crate::ConstantValue::Scalar(crate::ScalarValue::I32(value)),
                crate::ScalarValue::I32(min),
                crate::ScalarValue::I32(max),
            ) => *value >= min && *value <= max,
            (
                crate::ConstantValue::Scalar(crate::ScalarValue::I64(value)),
                crate::ScalarValue::I64(min),
                crate::ScalarValue::I64(max),
            ) => *value >= min && *value <= max,
            _ => false,
        }
    }

    fn validate_optional_value(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: Option<Value>,
        source: SourceSpan,
    ) {
        if let Some(value) = value {
            self.validate_value(function_id, function, value, source);
        }
    }

    fn validate_optional_index(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: Option<Value>,
        source: SourceSpan,
        label: &str,
    ) {
        if let Some(value) = value {
            self.require_i32_value(function_id, function, value, source, label);
        }
    }

    fn require_i32_value(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: Value,
        source: SourceSpan,
        label: &str,
    ) {
        if !self.value_matches_scalar(function, value, crate::ScalarType::I32) {
            self.function_error(function_id, source, format!("{label} must have type i32"));
        }
    }

    fn require_process_frame_value(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        value: Value,
        source: SourceSpan,
        operation: &str,
    ) {
        self.require_i32_value(function_id, function, value, source, "audio frame");
        let _ = operation;
    }

    fn require_audio_io_capability(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        source: SourceSpan,
        operation: &str,
    ) {
        if !matches!(function.kind, FunctionKind::Process) {
            self.function_error(
                function_id,
                source,
                format!("{operation} is only valid in the process entry point"),
            );
        }
    }

    fn require_control_output_capability(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        source: SourceSpan,
    ) {
        if !matches!(function.kind, FunctionKind::Process) {
            self.function_error(
                function_id,
                source,
                "control output stores are only valid in the process entry point",
            );
        }
    }

    fn require_direct_buffer_capability(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        source: SourceSpan,
        operation: &str,
    ) {
        let has_runtime_buffer_capability = matches!(
            function.kind,
            FunctionKind::Init | FunctionKind::Process | FunctionKind::Event(_)
        ) || (function.kind == FunctionKind::User
            && function.attributes.origin == FunctionOrigin::CompilerGenerated
            && function.attributes.runtime_context);
        if !has_runtime_buffer_capability {
            self.function_error(
                function_id,
                source,
                format!(
                    "{operation} requires init, process, or event host-buffer capability; user functions must receive buffers as parameters"
                ),
            );
        }
    }

    fn require_type(
        &mut self,
        ty: crate::TypeId,
        function: Option<FunctionId>,
        source: SourceSpan,
    ) {
        if ty.index() >= self.program.types.len() {
            self.errors.push(ValidationError {
                message: format!("references missing type {}", ty.raw()),
                function,
                source,
            });
        }
    }

    /// Returns whether a logical type transitively contains a slice or buffer
    /// descriptor. These types carry runtime addresses once lowered, so they
    /// cannot be embedded in portable host-owned storage or returned from a
    /// function. A worklist keeps malformed recursive aggregate definitions
    /// from making this validation recursive itself.
    fn type_contains_runtime_handle(&self, root: crate::TypeId) -> bool {
        let mut pending = vec![root];
        let mut visited = vec![false; self.program.types.len()];
        while let Some(ty) = pending.pop() {
            let Some(seen) = visited.get_mut(ty.index()) else {
                continue;
            };
            if *seen {
                continue;
            }
            *seen = true;
            match &self.program.types[ty.index()] {
                Type::Slice { .. } | Type::Buffer { .. } | Type::BufferSpan { .. } => return true,
                Type::Tuple(elements) => pending.extend(elements.iter().copied()),
                Type::Array { element, .. } => pending.push(*element),
                Type::Struct(structure) => {
                    if let Some(structure) = self.program.structs.get(structure.index()) {
                        pending.extend(structure.fields.iter().map(|field| field.ty));
                    }
                }
                Type::Scalar(_) => {}
            }
        }
        false
    }

    fn reject_runtime_handle_storage(
        &mut self,
        ty: crate::TypeId,
        function: Option<FunctionId>,
        source: SourceSpan,
        context: impl Into<String>,
    ) {
        if self.type_contains_runtime_handle(ty) {
            self.errors.push(ValidationError {
                message: format!(
                    "{} type must not contain runtime-only slice or buffer handles",
                    context.into()
                ),
                function,
                source,
            });
        }
    }

    fn require_function_kind(&mut self, id: FunctionId, expected: FunctionKind, label: &str) {
        match self.program.functions.get(id.index()) {
            Some(function) if function.kind == expected => {}
            Some(function) => self.program_error(format!(
                "{label} {} has kind {:?}, expected {:?}",
                id.raw(),
                function.kind,
                expected
            )),
            None => self.program_error(format!("{label} references missing function {}", id.raw())),
        }
    }

    fn validate_parameterless_entry_signature(&mut self, id: FunctionId, label: &str) {
        let Some(function) = self.program.functions.get(id.index()) else {
            return;
        };
        let source = function.source;
        let has_params = !function.params.is_empty();
        let has_results = !function.results.is_empty();
        if has_params {
            self.function_error(
                id,
                source,
                format!(
                    "{label} must not declare function parameters; entry ABI values use dedicated MIR places"
                ),
            );
        }
        if has_results {
            self.function_error(id, source, format!("{label} must not return values"));
        }
    }

    fn validate_entry_role_ownership(&mut self) {
        for (index, function) in self.program.functions.iter().enumerate() {
            let id = FunctionId::new(index as u32);
            match function.kind {
                FunctionKind::Init if id != self.program.entry_points.init => {
                    self.function_error(
                        id,
                        function.source,
                        "init-kind function is not the program init entry point",
                    );
                }
                FunctionKind::Process if id != self.program.entry_points.process => {
                    self.function_error(
                        id,
                        function.source,
                        "process-kind function is not the program process entry point",
                    );
                }
                FunctionKind::Event(event_id) => {
                    let Some(event) = self.program.interface.events.get(event_id.index()) else {
                        self.function_error(
                            id,
                            function.source,
                            format!(
                                "event-kind function references missing event {}",
                                event_id.raw()
                            ),
                        );
                        continue;
                    };
                    if event.handler != id {
                        self.function_error(
                            id,
                            function.source,
                            format!(
                                "event-kind function is not the registered handler for event '{}'",
                                event.name
                            ),
                        );
                    }
                }
                FunctionKind::Init | FunctionKind::Process | FunctionKind::User => {}
            }
        }
    }

    fn validate_process_entry_signature(&mut self) {
        let id = self.program.entry_points.process;
        let Some(function) = self.program.functions.get(id.index()) else {
            return;
        };
        let source = function.source;
        let has_results = !function.results.is_empty();
        let params = function.params.clone();

        if has_results {
            self.function_error(id, source, "process entry point must not return values");
        }
        if params.len() != PROCESS_PARAM_COUNT {
            self.function_error(
                id,
                source,
                format!(
                    "process entry point must have exactly {PROCESS_PARAM_COUNT} parameters (start_frame, frames, flags)"
                ),
            );
            return;
        }

        for (index, (parameter, expected_name)) in
            params.iter().zip(PROCESS_PARAM_NAMES).enumerate()
        {
            if parameter.name != expected_name {
                self.function_error(
                    id,
                    source,
                    format!(
                        "process parameter {index} must be named '{expected_name}', found '{}'",
                        parameter.name
                    ),
                );
            }
            if parameter.mode != crate::PassingMode::Value {
                self.function_error(
                    id,
                    source,
                    format!("process parameter '{expected_name}' must use value passing mode"),
                );
            }
            if !matches!(
                self.program.types.get(parameter.ty.index()),
                Some(Type::Scalar(crate::ScalarType::I32))
            ) {
                self.function_error(
                    id,
                    source,
                    format!("process parameter '{expected_name}' must have type i32"),
                );
            }
        }
    }

    fn require_local(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        local: crate::LocalId,
        source: SourceSpan,
    ) {
        if local.index() >= function.locals.len() {
            self.function_error(
                function_id,
                source,
                format!("references missing local {}", local.raw()),
            );
        }
    }

    fn require_buffer(
        &mut self,
        function_id: FunctionId,
        function: &Function,
        buffer: crate::BufferRef,
        source: SourceSpan,
    ) {
        if let crate::BufferRef::ArrayElement { len, selector, .. } = buffer {
            if len == 0 {
                self.function_error(
                    function_id,
                    source,
                    "buffer array reference has zero length",
                );
            }
            self.require_i32_value(
                function_id,
                function,
                selector,
                source,
                "buffer-array selector",
            );
        }
        if buffer
            .possible_indices()
            .any(|index| index >= self.program.interface.buffers.len())
        {
            self.function_error(
                function_id,
                source,
                format!(
                    "references invalid buffer range beginning at {}",
                    buffer.raw()
                ),
            );
            return;
        }
        let Some(first) = self.program.interface.buffers.get(buffer.index()) else {
            return;
        };
        if buffer.possible_indices().any(|index| {
            self.program
                .interface
                .buffers
                .get(index)
                .is_none_or(|candidate| {
                    candidate.element != first.element
                        || candidate.channels != first.channels
                        || candidate.access != first.access
                })
        }) {
            self.function_error(
                function_id,
                source,
                "buffer array reference spans incompatible descriptors",
            );
        }
    }

    fn program_error(&mut self, message: impl Into<String>) {
        self.errors.push(ValidationError {
            message: message.into(),
            function: None,
            source: SourceSpan::UNKNOWN,
        });
    }

    fn function_error(
        &mut self,
        function: FunctionId,
        source: SourceSpan,
        message: impl Into<String>,
    ) {
        self.errors.push(ValidationError {
            message: message.into(),
            function: Some(function),
            source,
        });
    }
}

fn local_display(function: &Function, local: crate::LocalId) -> String {
    function
        .locals
        .get(local.index())
        .and_then(|local| local.name.as_deref())
        .map_or_else(
            || format!("%{}", local.raw()),
            |name| format!("%{} ('{name}')", local.raw()),
        )
}

fn static_initialization_path(projections: &[crate::Projection]) -> Option<Vec<InitProjection>> {
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

fn static_initialization_prefix(projections: &[crate::Projection]) -> Vec<InitProjection> {
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

fn path_is_prefix(prefix: &[InitProjection], path: &[InitProjection]) -> bool {
    prefix.len() <= path.len() && prefix.iter().zip(path).all(|(lhs, rhs)| lhs == rhs)
}

fn initialization_is_covered(
    covered: &HashSet<Vec<InitProjection>>,
    requested: &[InitProjection],
) -> bool {
    covered
        .iter()
        .any(|region| path_is_prefix(region, requested))
}

fn type_at_initialization_path(
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

fn direct_initialization_children(
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

fn normalize_initialization_coverage(
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

fn intersect_two_assignment_states(
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

fn merge_assignment_fallthrough(
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

fn intersect_assignment_states(
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

fn find_call_cycle(program: &Program) -> Option<Vec<usize>> {
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

fn find_call_cycle_from(
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

fn collect_block_callees(block: &Block, function_count: usize, callees: &mut Vec<usize>) {
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

fn block_contains_publication(block: &Block) -> bool {
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

fn passing_mode_name(mode: crate::PassingMode) -> &'static str {
    match mode {
        crate::PassingMode::Value => "value",
        crate::PassingMode::ReadOnlyReference => "read-only reference",
        crate::PassingMode::ReadWriteReference => "read-write reference",
    }
}

fn logical_scalar_bytes(scalar: crate::ScalarType) -> u64 {
    match scalar {
        crate::ScalarType::F32 | crate::ScalarType::I32 => 4,
        crate::ScalarType::F64 | crate::ScalarType::I64 => 8,
        crate::ScalarType::Bool => 1,
    }
}

fn buffer_static_channel_validation_error(
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

fn scalar_sequence_fits_i32_bytes(len: usize, scalar: crate::ScalarType) -> bool {
    u64::try_from(len)
        .ok()
        .and_then(|len| len.checked_mul(logical_scalar_bytes(scalar)))
        .is_some_and(|bytes| bytes <= i32::MAX as u64)
}

fn access_permits(source: crate::AccessMode, requested: crate::AccessMode) -> bool {
    source == crate::AccessMode::ReadWrite || requested == crate::AccessMode::ReadOnly
}

fn buffer_channels_accept(expected: crate::BufferChannels, actual: crate::BufferChannels) -> bool {
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

fn validate_float_param_control_grid(
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

fn intrinsic_arity(intrinsic: crate::Intrinsic) -> usize {
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

fn intrinsic_name(intrinsic: crate::Intrinsic) -> &'static str {
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

#[cfg(test)]
mod tests {
    use crate::{
        AccessMode, Buffer, BufferChannels, BufferId, BufferRef, CallArgument, CompareOp,
        CompileConfig, ConstantValue, Delegate, DelegateParam, Event, EventId, EventParam, FieldId,
        Function, FunctionId, FunctionKind, FunctionParam, Intrinsic, Local, LocalId, Output,
        OutputId, Param, ParamId, PassingMode, Place, PlaceBase, Program, Projection, Rvalue,
        ScalarType, ScalarValue, SliceSource, SourceSpan, StatePersistence, StateSlot, Statement,
        StatementKind, StructField, StructType, Type, TypeId, Value,
    };

    fn function(name: &str, kind: FunctionKind) -> Function {
        Function {
            name: name.to_owned(),
            kind,
            attributes: crate::FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: crate::Block::default(),
            source: SourceSpan::UNKNOWN,
        }
    }

    // Test-specific types start after the canonical process ABI's i32 type.
    fn test_type(index: u32) -> TypeId {
        TypeId::new(index + 1)
    }

    fn empty_program() -> Program {
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 128,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.push(Type::Scalar(ScalarType::I32));
        let mut process = function("onda_process", FunctionKind::Process);
        process.params = crate::process_function_params(TypeId::new(0));
        program.functions = vec![function("onda_init", FunctionKind::Init), process];
        program
    }

    fn validate_producer(program: &Program) -> Result<(), Vec<super::ValidationError>> {
        // SAFETY: producer fixtures in this module deliberately exercise
        // unchecked operations alongside the structural invariant under test.
        unsafe { super::validate_with_producer_proofs(program) }
    }

    #[test]
    fn accepts_well_formed_empty_program() {
        assert!(super::validate(&empty_program()).is_ok());
    }

    #[test]
    fn delegate_fixed_arrays_require_primitive_elements() {
        let mut program = empty_program();
        program.structs.push(StructType {
            name: "Payload".to_owned(),
            fields: Vec::new(),
        });
        program.types.extend([
            Type::Struct(crate::StructId::new(0)),
            Type::Array {
                element: test_type(0),
                len: 2,
            },
        ]);
        program.interface.delegates.push(Delegate {
            name: "invalid".to_owned(),
            params: vec![DelegateParam {
                name: "values".to_owned(),
                ty: test_type(1),
            }],
        });

        let errors = super::validate(&program)
            .expect_err("delegate arrays with aggregate elements must fail validation");
        assert!(errors.iter().any(|error| error
            .message
            .contains("fixed array element must be a primitive scalar")));
    }

    #[test]
    fn pinned_state_requires_a_trusted_full_initialization_proof() {
        let mut program = empty_program();
        program.state.push(StateSlot {
            name: "cursor".to_owned(),
            ty: TypeId::new(0),
            persistence: StatePersistence::Snapshot,
            authored: true,
            pinned: true,
            integer_range: None,
        });
        program.functions[0].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::State(crate::StateId::new(0)),
                    projections: Vec::new(),
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(0))),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program)
            .expect_err("ordinary MIR cannot assert the pinned initialization invariant");
        assert!(errors.iter().any(|error| error
            .message
            .contains("pinned initialization requires trusted producer validation")));

        // SAFETY: the init entry above overwrites the complete pinned scalar
        // before returning on its only successful path.
        unsafe { super::validate_with_producer_proofs(&program) }
            .expect("trusted producer validation should retain the proof");
    }

    #[test]
    fn logical_type_equivalence_is_recursive_but_structs_are_nominal() {
        let mut program = empty_program();
        program.structs = vec![
            StructType {
                name: "Left".to_owned(),
                fields: vec![StructField {
                    name: "value".to_owned(),
                    ty: TypeId::new(1),
                }],
            },
            StructType {
                name: "Right".to_owned(),
                fields: vec![StructField {
                    name: "value".to_owned(),
                    ty: TypeId::new(2),
                }],
            },
        ];
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Scalar(ScalarType::F32),
            Type::Array {
                element: TypeId::new(1),
                len: 2,
            },
            Type::Array {
                element: TypeId::new(2),
                len: 2,
            },
            Type::Tuple(vec![TypeId::new(3)]),
            Type::Tuple(vec![TypeId::new(4)]),
            Type::Struct(crate::StructId::new(0)),
            Type::Struct(crate::StructId::new(1)),
        ]);

        assert!(program.types_equivalent(TypeId::new(3), TypeId::new(4)));
        assert!(program.types_equivalent(TypeId::new(5), TypeId::new(6)));
        assert!(!program.types_equivalent(TypeId::new(7), TypeId::new(8)));
    }

    #[test]
    fn rejects_block_size_outside_signed_process_abi() {
        let mut program = empty_program();
        program.config.block_size = i32::MAX as u32 + 1;

        let errors = super::validate(&program).expect_err("oversized block should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("2147483647")));
    }

    #[test]
    fn validates_numeric_interface_ranges_and_defaults() {
        let mut valid = empty_program();
        valid.types.push(Type::Scalar(ScalarType::F32));
        valid.interface.inputs.push(crate::Input {
            name: "gain".to_owned(),
            ty: test_type(0),
            default: Some(ConstantValue::Scalar(ScalarValue::F32(0.5))),
            range: Some(crate::ValueRange {
                min: ScalarValue::F32(0.0),
                max: ScalarValue::F32(1.0),
            }),
        });
        super::validate(&valid).expect("finite ordered range containing its default is valid");

        let mut invalid = empty_program();
        invalid.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Scalar(ScalarType::Bool),
        ]);
        invalid.interface.inputs.push(crate::Input {
            name: "reversed".to_owned(),
            ty: test_type(0),
            default: Some(ConstantValue::Scalar(ScalarValue::F32(1.5))),
            range: Some(crate::ValueRange {
                min: ScalarValue::F32(2.0),
                max: ScalarValue::F32(1.0),
            }),
        });
        invalid.interface.params.extend([
            Param {
                name: "non_finite".to_owned(),
                ty: test_type(0),
                default: ConstantValue::Scalar(ScalarValue::F32(0.0)),
                range: Some(crate::ValueRange {
                    min: ScalarValue::F32(f32::NEG_INFINITY),
                    max: ScalarValue::F32(1.0),
                }),
                control: crate::ParamControl::default(),
            },
            Param {
                name: "boolean".to_owned(),
                ty: test_type(1),
                default: ConstantValue::Scalar(ScalarValue::Bool(false)),
                range: Some(crate::ValueRange {
                    min: ScalarValue::Bool(false),
                    max: ScalarValue::Bool(true),
                }),
                control: crate::ParamControl::default(),
            },
            Param {
                name: "outside".to_owned(),
                ty: TypeId::new(0),
                default: ConstantValue::Scalar(ScalarValue::I32(3)),
                range: Some(crate::ValueRange {
                    min: ScalarValue::I32(0),
                    max: ScalarValue::I32(2),
                }),
                control: crate::ParamControl::default(),
            },
        ]);

        let errors = super::validate(&invalid).expect_err("invalid ranges must fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("input 'reversed' range minimum is greater")));
        assert!(errors.iter().any(|error| error
            .message
            .contains("parameter 'non_finite' range endpoints must be finite")));
        assert!(errors.iter().any(|error| error
            .message
            .contains("parameter 'boolean' range is not supported for bool")));
        assert!(errors.iter().any(|error| error
            .message
            .contains("parameter 'outside' default is outside")));
    }

    #[test]
    fn validates_parameter_control_metadata() {
        let mut valid = empty_program();
        valid.types.push(Type::Scalar(ScalarType::F32));
        valid.interface.params.push(Param {
            name: "cutoff".to_owned(),
            ty: test_type(0),
            default: ConstantValue::Scalar(ScalarValue::F32(440.0)),
            range: Some(crate::ValueRange {
                min: ScalarValue::F32(20.0),
                max: ScalarValue::F32(20_000.0),
            }),
            control: crate::ParamControl {
                scale: crate::ParamScale::Log,
                curve: None,
                unit: Some("Hz".to_owned()),
                step: None,
                step_count: None,
            },
        });
        super::validate(&valid).expect("valid logarithmic control metadata should pass");

        let mut non_finite_curve = valid.clone();
        non_finite_curve.interface.params[0].control.scale = crate::ParamScale::Linear;
        non_finite_curve.interface.params[0].control.curve = Some(f64::NAN);
        let errors =
            super::validate(&non_finite_curve).expect_err("non-finite curve metadata must fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("curve must be finite")));

        let mut mixed_log_curve = valid.clone();
        mixed_log_curve.interface.params[0].control.curve = Some(-4.0);
        let errors =
            super::validate(&mixed_log_curve).expect_err("logarithmic curve metadata must fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("cannot combine logarithmic scale with curve")));

        let mut invalid = empty_program();
        invalid.types.push(Type::Scalar(ScalarType::I32));
        invalid.interface.params.push(Param {
            name: "mode".to_owned(),
            ty: test_type(0),
            default: ConstantValue::Scalar(ScalarValue::I32(4)),
            range: Some(crate::ValueRange {
                min: ScalarValue::I32(0),
                max: ScalarValue::I32(10),
            }),
            control: crate::ParamControl {
                step: Some(ScalarValue::I32(2)),
                step_count: Some(4),
                ..crate::ParamControl::default()
            },
        });
        let errors = super::validate(&invalid).expect_err("wrong step count must fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("step_count does not match the range and step")));

        let mut inexact_i64 = empty_program();
        inexact_i64.types.push(Type::Scalar(ScalarType::I64));
        inexact_i64.interface.params.push(Param {
            name: "wide".to_owned(),
            ty: test_type(0),
            default: ConstantValue::Scalar(ScalarValue::I64(9_007_199_254_740_992)),
            range: Some(crate::ValueRange {
                min: ScalarValue::I64(9_007_199_254_740_992),
                max: ScalarValue::I64(9_007_199_254_741_002),
            }),
            control: crate::ParamControl {
                step: Some(ScalarValue::I64(1)),
                step_count: Some(10),
                ..crate::ParamControl::default()
            },
        });
        let errors =
            super::validate(&inexact_i64).expect_err("inexact i64 control range must fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("must fit the exact host integer range")));
    }

    #[test]
    fn rejects_noncanonical_process_parameter_count() {
        let mut program = empty_program();
        program.functions[1].params.pop();

        let errors = super::validate(&program).expect_err("short process signature should fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("exactly 3 parameters (start_frame, frames, flags)")));
    }

    #[test]
    fn rejects_noncanonical_process_parameter_names_modes_and_types() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.functions[1].params[0].name = "frames".to_owned();
        program.functions[1].params[1].mode = PassingMode::ReadOnlyReference;
        program.functions[1].params[2].ty = test_type(0);

        let errors = super::validate(&program).expect_err("invalid process signature should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("must be named 'start_frame'")));
        assert!(errors
            .iter()
            .any(|error| error.message.contains("'frames' must use value")));
        assert!(errors
            .iter()
            .any(|error| error.message.contains("'flags' must have type i32")));
    }

    #[test]
    fn rejects_process_results() {
        let mut program = empty_program();
        program.functions[1].results.push(TypeId::new(0));

        let errors = super::validate(&program).expect_err("process result should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("must not return values")));
    }

    #[test]
    fn rejects_explicit_init_and_event_entry_signatures() {
        let mut program = empty_program();
        program.functions[0].params.push(FunctionParam {
            integer_range: None,
            name: "hidden".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::Value,
        });
        program.functions[0].results.push(TypeId::new(0));

        let handler_id = FunctionId::new(2);
        let mut handler = function("onda_event::tick", FunctionKind::Event(EventId::new(0)));
        handler.params.push(FunctionParam {
            integer_range: None,
            name: "payload".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::Value,
        });
        handler.results.push(TypeId::new(0));
        program.interface.events.push(Event {
            name: "tick".to_owned(),
            params: Vec::new(),
            handler: handler_id,
        });
        program.functions.push(handler);

        let errors =
            super::validate(&program).expect_err("explicit entry signatures must be rejected");
        assert!(errors.iter().any(|error| error
            .message
            .contains("init entry point must not declare function parameters")));
        assert!(errors
            .iter()
            .any(|error| error.message.contains("init entry point must not return")));
        assert!(errors.iter().any(|error| error
            .message
            .contains("event 'tick' handler must not declare function parameters")));
        assert!(errors.iter().any(|error| error
            .message
            .contains("event 'tick' handler must not return")));
    }

    #[test]
    fn rejects_unowned_entry_role_functions() {
        let mut program = empty_program();
        let handler_id = FunctionId::new(2);
        program.interface.events.push(Event {
            name: "tick".to_owned(),
            params: Vec::new(),
            handler: handler_id,
        });
        program.functions.push(function(
            "onda_event::tick",
            FunctionKind::Event(EventId::new(0)),
        ));
        program
            .functions
            .push(function("extra_init", FunctionKind::Init));
        program.functions.push(function(
            "extra_event",
            FunctionKind::Event(EventId::new(0)),
        ));
        let mut extra_process = function("extra_process", FunctionKind::Process);
        extra_process.params = crate::process_function_params(TypeId::new(0));
        program.functions.push(extra_process);

        let errors = super::validate(&program).expect_err("entry roles must be one-to-one");
        assert!(errors.iter().any(|error| error
            .message
            .contains("init-kind function is not the program init entry point")));
        assert!(errors.iter().any(|error| error
            .message
            .contains("event-kind function is not the registered handler")));
        assert!(errors.iter().any(|error| error
            .message
            .contains("process-kind function is not the program process entry point")));
    }

    #[test]
    fn rejects_recursive_user_function_call_cycles() {
        let mut program = empty_program();
        let mut first = function("first", FunctionKind::User);
        first.body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(3),
                args: Vec::new(),
            },
            source: SourceSpan::UNKNOWN,
        });
        let mut second = function("second", FunctionKind::User);
        second.body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: Vec::new(),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.extend([first, second]);

        let errors = super::validate(&program).expect_err("recursive MIR should fail");
        assert!(errors.iter().any(|error| {
            error.function == Some(FunctionId::new(2))
                && error
                    .message
                    .contains("recursive call cycle is not realtime-safe: first -> second -> first")
        }));
    }

    #[test]
    fn rejects_result_function_with_empty_fallthrough_body() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        let mut callee = function("partial", FunctionKind::User);
        callee.results.push(test_type(0));
        program.functions.push(callee);

        let errors = super::validate(&program).expect_err("result fallthrough should fail");
        assert!(errors.iter().any(|error| {
            error.function == Some(FunctionId::new(2))
                && error.message.contains("falls through without returning")
        }));
    }

    #[test]
    fn rejects_result_function_when_only_one_if_branch_returns() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        let mut callee = function("partial", FunctionKind::User);
        callee.results.push(test_type(0));
        callee.body.statements.push(Statement {
            kind: StatementKind::If {
                condition: Value::Constant(ScalarValue::Bool(true)),
                then_block: crate::Block {
                    statements: vec![Statement {
                        kind: StatementKind::Return {
                            values: vec![Value::Constant(ScalarValue::F32(1.0))],
                        },
                        source: SourceSpan::UNKNOWN,
                    }],
                },
                else_block: crate::Block::default(),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(callee);

        let errors = super::validate(&program).expect_err("partial branch return should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("falls through without returning")));
    }

    #[test]
    fn accepts_result_function_when_both_if_branches_return() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        let return_block = |value| crate::Block {
            statements: vec![Statement {
                kind: StatementKind::Return {
                    values: vec![Value::Constant(ScalarValue::F32(value))],
                },
                source: SourceSpan::UNKNOWN,
            }],
        };
        let mut callee = function("total", FunctionKind::User);
        callee.results.push(test_type(0));
        callee.body.statements.push(Statement {
            kind: StatementKind::If {
                condition: Value::Constant(ScalarValue::Bool(true)),
                then_block: return_block(1.0),
                else_block: return_block(2.0),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(callee);

        super::validate(&program).expect("complete branch returns should validate");
    }

    #[test]
    fn result_function_infinite_loop_has_no_reachable_fallthrough() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        let mut callee = function("spin", FunctionKind::User);
        callee.results.push(test_type(0));
        callee.body.statements.push(Statement {
            kind: StatementKind::Loop {
                body: crate::Block {
                    statements: vec![Statement {
                        kind: StatementKind::Continue,
                        source: SourceSpan::UNKNOWN,
                    }],
                },
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(callee);

        super::validate(&program).expect("non-terminating loop cannot fall through");
    }

    #[test]
    fn reachable_loop_break_requires_a_following_return() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        let mut callee = function("breaks", FunctionKind::User);
        callee.results.push(test_type(0));
        callee.body.statements.push(Statement {
            kind: StatementKind::Loop {
                body: crate::Block {
                    statements: vec![Statement {
                        kind: StatementKind::Break,
                        source: SourceSpan::UNKNOWN,
                    }],
                },
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(callee);

        let errors = super::validate(&program).expect_err("loop break should reach fallthrough");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("falls through without returning")));
    }

    #[test]
    fn no_result_function_may_fall_through() {
        let mut program = empty_program();
        program
            .functions
            .push(function("observe", FunctionKind::User));
        super::validate(&program).expect("no-result function may fall through");
    }

    #[test]
    fn rejects_loop_control_outside_loop() {
        let mut program = empty_program();
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Break,
            source: SourceSpan::UNKNOWN,
        });
        let errors = super::validate(&program).expect_err("invalid MIR should be rejected");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("outside a loop")));
    }

    #[test]
    fn rejects_wrong_entry_point_kind() {
        let mut program = empty_program();
        program.functions[1].kind = FunctionKind::User;
        let errors = super::validate(&program).expect_err("invalid MIR should be rejected");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("process entry point")));
    }

    #[test]
    fn rejects_mistyped_multi_value_call_destination() {
        let mut program = empty_program();
        program
            .types
            .extend([Type::Scalar(ScalarType::F32), Type::Scalar(ScalarType::I32)]);
        let mut callee = function("pair", FunctionKind::User);
        callee.results = vec![test_type(0), test_type(1)];
        program.functions.push(callee);
        program.functions[1].locals.extend([
            Local {
                integer_range: None,
                name: None,
                ty: test_type(0),
            },
            Local {
                integer_range: None,
                name: None,
                ty: test_type(0),
            },
        ]);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Call {
                results: vec![LocalId::new(0), LocalId::new(1)],
                function: FunctionId::new(2),
                args: Vec::<CallArgument>::new(),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("mistyped call result should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("result 1") && error.message.contains("i32")));
    }

    #[test]
    fn rejects_mistyped_return_component() {
        let mut program = empty_program();
        program
            .types
            .extend([Type::Scalar(ScalarType::F32), Type::Scalar(ScalarType::I32)]);
        let mut callee = function("pair", FunctionKind::User);
        callee.results = vec![test_type(0), test_type(1)];
        callee.body.statements.push(Statement {
            kind: StatementKind::Return {
                values: vec![
                    Value::Constant(ScalarValue::F32(1.0)),
                    Value::Constant(ScalarValue::F32(2.0)),
                ],
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(callee);

        let errors = super::validate(&program).expect_err("mistyped return should fail");
        assert!(errors.iter().any(|error| {
            error.message.contains("return value 1") && error.message.contains("i32")
        }));
    }

    #[test]
    fn rejects_mistyped_assignment() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(1))),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("mistyped assignment should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("assignment value")));
    }

    #[test]
    fn rejects_mistyped_call_argument() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        let mut callee = function("consume", FunctionKind::User);
        callee.params.push(FunctionParam {
            integer_range: None,
            name: "value".to_owned(),
            ty: test_type(0),
            mode: PassingMode::Value,
        });
        program.functions.push(callee);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::Value(Value::Constant(ScalarValue::I32(1)))],
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("mistyped call argument should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("argument 0")));
    }

    #[test]
    fn accepts_reference_call_argument_into_a_slice() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Scalar(ScalarType::I32),
            Type::Array {
                element: test_type(0),
                len: 2,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadWrite,
            },
        ]);
        program.state.push(StateSlot {
            integer_range: None,
            name: "values".to_owned(),
            ty: test_type(2),
            persistence: StatePersistence::Snapshot,
            authored: true,
            pinned: false,
        });
        let mut callee = function("update", FunctionKind::User);
        callee.params.push(FunctionParam {
            integer_range: None,
            name: "value".to_owned(),
            ty: test_type(0),
            mode: PassingMode::ReadWriteReference,
        });
        program.functions.push(callee);
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("view".to_owned()),
            ty: test_type(3),
        });
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::MakeSlice {
                        source: SliceSource::Place(Place {
                            base: PlaceBase::State(crate::StateId::new(0)),
                            projections: Vec::new(),
                        }),
                        start: Value::Constant(ScalarValue::I32(0)),
                        len: Value::Constant(ScalarValue::I32(2)),
                        bounds: crate::BoundsMode::Unchecked,
                        access: AccessMode::ReadWrite,
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(2),
                    args: vec![CallArgument::SliceElement {
                        slice: Value::Local(LocalId::new(0)),
                        index: Value::Constant(ScalarValue::I32(1)),
                        bounds: crate::BoundsMode::Clamp,
                    }],
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        validate_producer(&program).expect("slice element should satisfy scalar reference");
    }

    #[test]
    fn rejects_mistyped_output_frame_and_value() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.outputs.push(Output {
            name: "out1".to_owned(),
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::OutputStore {
                output: OutputId::new(0),
                element: None,
                bounds: crate::BoundsMode::Unchecked,
                frame: Value::Constant(ScalarValue::F32(0.0)),
                value: Value::Constant(ScalarValue::I32(1)),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("mistyped output store should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("audio frame")));
        assert!(errors
            .iter()
            .any(|error| error.message.contains("output store value")));
    }

    #[test]
    fn validates_explicit_output_loads() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.outputs.push(Output {
            name: "out1".to_owned(),
            ty: test_type(0),
        });
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("frame".to_owned()),
            ty: TypeId::new(0),
        });
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("current".to_owned()),
            ty: test_type(0),
        });
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::ProcessFrame {
                        offset: Value::Constant(ScalarValue::I32(0)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(1)),
                    value: Rvalue::OutputLoad {
                        output: OutputId::new(0),
                        element: None,
                        bounds: crate::BoundsMode::Unchecked,
                        frame: Value::Local(LocalId::new(0)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        validate_producer(&program).expect("well-typed output load should validate");

        let Rvalue::OutputLoad { frame, .. } =
            (match &mut program.functions[1].body.statements[1].kind {
                StatementKind::Assign { value, .. } => value,
                _ => unreachable!(),
            })
        else {
            unreachable!()
        };
        *frame = Value::Constant(ScalarValue::F32(0.0));
        let errors = super::validate(&program).expect_err("mistyped output frame should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("audio frame")));
    }

    #[test]
    fn rejects_audio_io_without_process_capability_or_canonical_frame() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.inputs.push(crate::Input {
            name: "in1".to_owned(),
            ty: test_type(0),
            default: None,
            range: None,
        });
        program.interface.outputs.push(Output {
            name: "out1".to_owned(),
            ty: test_type(0),
        });
        program.functions[0].locals.push(Local {
            integer_range: None,
            name: Some("sample".to_owned()),
            ty: test_type(0),
        });
        program.functions[0].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::InputLoad {
                    input: crate::InputId::new(0),
                    element: None,
                    bounds: crate::BoundsMode::Unchecked,
                    frame: Value::Constant(ScalarValue::I32(0)),
                },
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::OutputStore {
                output: OutputId::new(0),
                element: None,
                bounds: crate::BoundsMode::Unchecked,
                frame: Value::Constant(ScalarValue::I32(-1)),
                value: Value::Constant(ScalarValue::F32(0.0)),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("unsafe audio I/O must fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("only valid in the process entry point")));
        assert!(errors
            .iter()
            .any(|error| error.message.contains("must be dominated by process_frame")));
    }

    #[test]
    fn read_write_reference_call_invalidates_process_frame_provenance() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.outputs.push(Output {
            name: "out1".to_owned(),
            ty: test_type(0),
        });

        let mut mutator = function("mutate_frame", FunctionKind::User);
        mutator.params.push(FunctionParam {
            integer_range: None,
            name: "frame".to_owned(),
            ty: TypeId::new(0),
            mode: PassingMode::ReadWriteReference,
        });
        mutator.body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                    projections: Vec::new(),
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(i32::MAX))),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(mutator);

        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("frame".to_owned()),
            ty: TypeId::new(0),
        });
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::ProcessFrame {
                        offset: Value::Constant(ScalarValue::I32(0)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(2),
                    args: vec![CallArgument::Place(Place::local(LocalId::new(0)))],
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::OutputStore {
                    output: OutputId::new(0),
                    element: None,
                    bounds: crate::BoundsMode::Unchecked,
                    frame: Value::Local(LocalId::new(0)),
                    value: Value::Constant(ScalarValue::F32(0.0)),
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        let errors = super::validate(&program)
            .expect_err("a read-write call may replace a checked process frame");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("must be dominated by process_frame")));
    }

    #[test]
    fn boolean_comparisons_only_support_equality() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::Bool));
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("comparison".to_owned()),
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Compare {
                    op: CompareOp::Less,
                    lhs: Value::Constant(ScalarValue::Bool(false)),
                    rhs: Value::Constant(ScalarValue::Bool(true)),
                },
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors =
            super::validate(&program).expect_err("relational boolean comparison must fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("boolean comparisons only support")));

        let StatementKind::Assign {
            value: Rvalue::Compare { op, .. },
            ..
        } = &mut program.functions[1].body.statements[0].kind
        else {
            unreachable!()
        };
        *op = CompareOp::Equal;
        super::validate(&program).expect("boolean equality should remain valid");
    }

    #[test]
    fn rejects_interface_array_default_with_wrong_shape() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Array {
                element: test_type(0),
                len: 2,
            },
        ]);
        program.interface.inputs.push(crate::Input {
            name: "stereo".to_owned(),
            ty: test_type(1),
            default: Some(crate::ConstantValue::Aggregate(vec![
                crate::ConstantValue::Scalar(ScalarValue::F32(0.0)),
            ])),
            range: None,
        });

        let errors = super::validate(&program).expect_err("short array default should fail");
        assert!(errors.iter().any(|error| {
            error.message.contains("input 'stereo' default") && error.message.contains("Array")
        }));
    }

    #[test]
    fn rejects_runtime_handles_nested_in_interface_storage() {
        let mut program = empty_program();
        program.types.extend([
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
            Type::Array {
                element: test_type(0),
                len: 2,
            },
            Type::Buffer {
                element: ScalarType::F32,
                channels: BufferChannels::Mono,
                access: AccessMode::ReadOnly,
            },
            Type::Struct(crate::StructId::new(0)),
            Type::Tuple(vec![test_type(0)]),
        ]);
        program.structs.push(StructType {
            name: "ResourceBox".to_owned(),
            fields: vec![StructField {
                name: "resource".to_owned(),
                ty: test_type(2),
            }],
        });
        program.interface.inputs.push(crate::Input {
            name: "input_view".to_owned(),
            ty: test_type(1),
            default: None,
            range: None,
        });
        program.interface.outputs.push(Output {
            name: "output_buffer".to_owned(),
            ty: test_type(2),
        });
        program
            .interface
            .control_outputs
            .push(crate::ControlOutput {
                name: "control_resource".to_owned(),
                ty: test_type(3),
                mirror: crate::StateId::new(0),
            });
        program.state.push(StateSlot {
            integer_range: None,
            name: "control_resource_mirror".to_owned(),
            ty: test_type(3),
            persistence: StatePersistence::ControlMirror,
            authored: true,
            pinned: false,
        });
        program.interface.params.push(Param {
            name: "parameter_view".to_owned(),
            ty: test_type(4),
            default: ConstantValue::Aggregate(Vec::new()),
            range: None,
            control: crate::ParamControl::default(),
        });

        let errors = super::validate(&program).expect_err("runtime handles must not be stored");
        for context in [
            "input 'input_view'",
            "output 'output_buffer'",
            "control output 'control_resource'",
            "parameter 'parameter_view'",
        ] {
            assert!(errors.iter().any(|error| {
                error.message.contains(context)
                    && error
                        .message
                        .contains("runtime-only slice or buffer handles")
            }));
        }
    }

    #[test]
    fn rejects_runtime_handles_nested_in_persistent_state() {
        let mut program = empty_program();
        program.types.extend([
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadWrite,
            },
            Type::Array {
                element: test_type(0),
                len: 2,
            },
            Type::Struct(crate::StructId::new(0)),
            Type::Buffer {
                element: ScalarType::F32,
                channels: BufferChannels::Dynamic,
                access: AccessMode::ReadWrite,
            },
            Type::Tuple(vec![test_type(3)]),
        ]);
        program.structs.push(StructType {
            name: "NestedView".to_owned(),
            fields: vec![StructField {
                name: "views".to_owned(),
                ty: test_type(1),
            }],
        });
        program.state.extend([
            StateSlot {
                integer_range: None,
                name: "saved_view".to_owned(),
                ty: test_type(2),
                persistence: StatePersistence::Snapshot,
                authored: true,
                pinned: false,
            },
            StateSlot {
                integer_range: None,
                name: "scratch_buffer".to_owned(),
                ty: test_type(4),
                persistence: StatePersistence::InstanceScratch,
                authored: false,
                pinned: false,
            },
        ]);

        let errors = super::validate(&program).expect_err("runtime handles must not persist");
        assert!(errors.iter().any(|error| error
            .message
            .contains("snapshot state 'saved_view' type must not contain runtime-only")));
        assert!(errors.iter().any(|error| error.message.contains(
            "instance-scratch state 'scratch_buffer' type must not contain runtime-only"
        )));
    }

    #[test]
    fn rejects_runtime_handles_nested_in_function_results() {
        let mut program = empty_program();
        program.types.extend([
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
            Type::Array {
                element: test_type(0),
                len: 2,
            },
        ]);
        let mut callee = function("return_view", FunctionKind::User);
        callee.results.push(test_type(1));
        callee.body.statements.push(Statement {
            kind: StatementKind::Loop {
                body: crate::Block {
                    statements: vec![Statement {
                        kind: StatementKind::Continue,
                        source: SourceSpan::UNKNOWN,
                    }],
                },
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(callee);

        let errors = super::validate(&program).expect_err("runtime handle result should fail");
        assert!(errors.iter().any(|error| {
            error.function == Some(FunctionId::new(2))
                && error
                    .message
                    .contains("function result type must not contain runtime-only")
        }));
    }

    #[test]
    fn accepts_runtime_handles_in_transient_function_values() {
        let mut program = empty_program();
        program.types.extend([
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
            Type::Buffer {
                element: ScalarType::F32,
                channels: BufferChannels::Mono,
                access: AccessMode::ReadWrite,
            },
        ]);
        let mut callee = function("consume_resources", FunctionKind::User);
        callee.params.extend([
            FunctionParam {
                integer_range: None,
                name: "view".to_owned(),
                ty: test_type(0),
                mode: PassingMode::Value,
            },
            FunctionParam {
                integer_range: None,
                name: "buffer".to_owned(),
                ty: test_type(1),
                mode: PassingMode::ReadWriteReference,
            },
        ]);
        callee.locals.push(Local {
            integer_range: None,
            name: Some("window".to_owned()),
            ty: test_type(0),
        });
        program.functions.push(callee);

        super::validate(&program).expect("function parameters and locals are transient");
    }

    #[test]
    fn accepts_direct_read_only_slice_event_parameters() {
        let mut program = empty_program();
        program.types.push(Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        });
        program.interface.events.push(Event {
            name: "set_curve".to_owned(),
            params: vec![EventParam {
                name: "values".to_owned(),
                ty: test_type(0),
                default: None,
            }],
            handler: FunctionId::new(2),
        });
        program.functions.push(function(
            "onda_event::set_curve",
            FunctionKind::Event(EventId::new(0)),
        ));

        super::validate(&program).expect("read-only event slices are a supported payload type");
    }

    #[test]
    fn rejects_mutable_buffer_and_nested_event_handles() {
        let mut program = empty_program();
        program.types.extend([
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadWrite,
            },
            Type::Buffer {
                element: ScalarType::F32,
                channels: BufferChannels::Mono,
                access: AccessMode::ReadOnly,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
            Type::Array {
                element: test_type(2),
                len: 2,
            },
        ]);
        program.interface.events.push(Event {
            name: "invalid".to_owned(),
            params: vec![
                EventParam {
                    name: "mutable_view".to_owned(),
                    ty: test_type(0),
                    default: None,
                },
                EventParam {
                    name: "buffer".to_owned(),
                    ty: test_type(1),
                    default: None,
                },
                EventParam {
                    name: "nested_views".to_owned(),
                    ty: test_type(3),
                    default: None,
                },
            ],
            handler: FunctionId::new(2),
        });
        program.functions.push(function(
            "onda_event::invalid",
            FunctionKind::Event(EventId::new(0)),
        ));

        let errors = super::validate(&program).expect_err("invalid event handles should fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("parameter 'mutable_view' slice must be read-only")));
        for name in ["buffer", "nested_views"] {
            assert!(errors.iter().any(|error| {
                error.message.contains(&format!("parameter '{name}'"))
                    && error.message.contains("direct read-only slice")
            }));
        }
    }

    #[test]
    fn accepts_slice_construction_load_and_store() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Scalar(ScalarType::I32),
            Type::Array {
                element: test_type(0),
                len: 4,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadWrite,
            },
        ]);
        program.state.push(StateSlot {
            integer_range: None,
            name: "values".to_owned(),
            ty: test_type(2),
            persistence: StatePersistence::Snapshot,
            authored: true,
            pinned: false,
        });
        program.functions[1].locals.extend([
            Local {
                integer_range: None,
                name: Some("view".to_owned()),
                ty: test_type(3),
            },
            Local {
                integer_range: None,
                name: None,
                ty: test_type(0),
            },
        ]);
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::MakeSlice {
                        source: SliceSource::Place(Place {
                            base: PlaceBase::State(crate::StateId::new(0)),
                            projections: Vec::new(),
                        }),
                        start: Value::Constant(ScalarValue::I32(1)),
                        len: Value::Constant(ScalarValue::I32(2)),
                        bounds: crate::BoundsMode::Unchecked,
                        access: AccessMode::ReadWrite,
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(1)),
                    value: Rvalue::SliceLoad {
                        slice: Value::Local(LocalId::new(0)),
                        index: Value::Constant(ScalarValue::I32(0)),
                        bounds: crate::BoundsMode::Clamp,
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::SliceStore {
                    slice: Value::Local(LocalId::new(0)),
                    index: Value::Constant(ScalarValue::I32(1)),
                    value: Value::Local(LocalId::new(1)),
                    bounds: crate::BoundsMode::Clamp,
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        validate_producer(&program).expect("well-typed slice MIR should validate");
    }

    #[test]
    fn rejects_store_through_read_only_slice() {
        let mut program = empty_program();
        program.types.push(Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        });
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("view".to_owned()),
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::SliceStore {
                slice: Value::Local(LocalId::new(0)),
                index: Value::Constant(ScalarValue::I32(0)),
                value: Value::Constant(ScalarValue::F32(1.0)),
                bounds: crate::BoundsMode::Clamp,
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("read-only slice store should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("destination is read-only")));
    }

    #[test]
    fn rejects_field_projection_on_a_scalar_place() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(LocalId::new(0)),
                    projections: vec![Projection::Field(FieldId::new(0))],
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("scalar field projection should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("field projection requires a struct")));
    }

    #[test]
    fn rejects_index_projection_on_a_scalar_place() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(LocalId::new(0)),
                    projections: vec![Projection::Index {
                        index: Value::Constant(ScalarValue::I32(0)),
                        bounds: crate::BoundsMode::Checked,
                    }],
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("scalar index projection should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("index projection requires an array")));
    }

    #[test]
    fn rejects_missing_struct_field_projection() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Struct(crate::StructId::new(0)),
        ]);
        program.structs.push(StructType {
            name: "Pair".to_owned(),
            fields: vec![StructField {
                name: "first".to_owned(),
                ty: test_type(0),
            }],
        });
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: None,
            ty: test_type(1),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(LocalId::new(0)),
                    projections: vec![Projection::Field(FieldId::new(1))],
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("missing field projection should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("missing field 1")));
    }

    #[test]
    fn rejects_readwrite_reference_to_a_readonly_place() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.params.push(Param {
            name: "gain".to_owned(),
            ty: test_type(0),
            default: ConstantValue::Scalar(ScalarValue::F32(1.0)),
            range: None,
            control: crate::ParamControl::default(),
        });
        let mut callee = function("mutate", FunctionKind::User);
        callee.params.push(FunctionParam {
            integer_range: None,
            name: "value".to_owned(),
            ty: test_type(0),
            mode: PassingMode::ReadWriteReference,
        });
        program.functions.push(callee);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::Place(Place {
                    base: PlaceBase::Param(ParamId::new(0)),
                    projections: Vec::new(),
                })],
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors =
            super::validate(&program).expect_err("read-write reference escalation should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("argument 0")));
    }

    #[test]
    fn rejects_store_to_readonly_interface_buffer() {
        let mut program = empty_program();
        program.interface.buffers.push(Buffer {
            name: "samples".to_owned(),
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadOnly,
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::BufferStore {
                buffer: crate::BufferRef::Direct(BufferId::new(0)),
                channel: None,
                index: Value::Constant(ScalarValue::I32(0)),
                value: Value::Constant(ScalarValue::F32(1.0)),
                bounds: crate::BoundsMode::Checked,
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("read-only buffer store should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("read-only interface buffer")));
    }

    #[test]
    fn ordinary_validation_rejects_unchecked_buffer_collection_selectors() {
        let mut program = empty_program();
        program.interface.buffers.push(Buffer {
            name: "samples".to_owned(),
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
        });
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("frames".to_owned()),
            ty: TypeId::new(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(LocalId::new(0)),
                    projections: Vec::new(),
                },
                value: Rvalue::BufferLen(BufferRef::ArrayElement {
                    first: BufferId::new(0),
                    len: 1,
                    selector: Value::Constant(ScalarValue::I32(0)),
                    bounds: crate::BoundsMode::Unchecked,
                }),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program)
            .expect_err("unchecked descriptor selection requires producer proof");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("trusted MIR producer proof")));
        validate_producer(&program).expect("trusted producer selector is structurally valid");
    }

    #[test]
    fn buffer_collection_references_require_homogeneous_descriptors() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.buffers.extend([
            Buffer {
                name: "float_samples".to_owned(),
                element: ScalarType::F32,
                channels: BufferChannels::Mono,
                access: AccessMode::ReadWrite,
            },
            Buffer {
                name: "bool_samples".to_owned(),
                element: ScalarType::Bool,
                channels: BufferChannels::Mono,
                access: AccessMode::ReadWrite,
            },
        ]);
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("sample".to_owned()),
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(LocalId::new(0)),
                    projections: Vec::new(),
                },
                value: Rvalue::BufferLoad {
                    buffer: BufferRef::ArrayElement {
                        first: BufferId::new(0),
                        len: 2,
                        selector: Value::Constant(ScalarValue::I32(1)),
                        bounds: crate::BoundsMode::Clamp,
                    },
                    channel: None,
                    index: Value::Constant(ScalarValue::I32(0)),
                    bounds: crate::BoundsMode::Clamp,
                },
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program)
            .expect_err("mixed descriptor collection must fail validation");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("spans incompatible descriptors")));
    }

    #[test]
    fn direct_buffers_in_user_functions_require_compiler_runtime_provenance() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.buffers.push(Buffer {
            name: "samples".to_owned(),
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
        });
        let mut handler = function("delegate_handler", FunctionKind::User);
        handler.locals.push(Local {
            integer_range: None,
            name: Some("sample".to_owned()),
            ty: test_type(0),
        });
        handler.body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(LocalId::new(0)),
                    projections: Vec::new(),
                },
                value: Rvalue::BufferLoad {
                    buffer: BufferRef::Direct(BufferId::new(0)),
                    channel: None,
                    index: Value::Constant(ScalarValue::I32(0)),
                    bounds: crate::BoundsMode::Clamp,
                },
            },
            source: SourceSpan::UNKNOWN,
        });

        handler.attributes.runtime_context = true;
        program.functions.push(handler.clone());
        let errors = super::validate(&program)
            .expect_err("source user functions must not gain direct buffer capability");
        assert!(errors.iter().any(|error| error
            .message
            .contains("user functions must receive buffers")));

        handler.attributes.origin = crate::FunctionOrigin::CompilerGenerated;
        program.functions[2] = handler;
        super::validate(&program)
            .expect("compiler-generated runtime handlers may access direct host buffers");
    }

    #[test]
    fn static_buffer_channels_must_fit_signed_byte_extents() {
        let mut program = empty_program();
        program.types.push(Type::Buffer {
            element: ScalarType::F32,
            channels: BufferChannels::Static((i32::MAX as u32 / 4) + 1),
            access: AccessMode::ReadWrite,
        });
        program.interface.buffers.push(Buffer {
            name: "huge".to_owned(),
            element: ScalarType::F64,
            channels: BufferChannels::Static((i32::MAX as u32 / 8) + 1),
            access: AccessMode::ReadWrite,
        });

        let errors = super::validate(&program).expect_err("oversized channels must fail");
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.message.contains("buffer byte-extent limit"))
                .count(),
            2
        );
    }

    #[test]
    fn rejects_readonly_buffer_for_readwrite_buffer_parameter() {
        let mut program = empty_program();
        program.types.push(Type::Buffer {
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
        });
        program.interface.buffers.push(Buffer {
            name: "samples".to_owned(),
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadOnly,
        });
        let mut callee = function("mutate_buffer", FunctionKind::User);
        callee.params.push(FunctionParam {
            integer_range: None,
            name: "samples".to_owned(),
            ty: test_type(0),
            mode: PassingMode::ReadWriteReference,
        });
        program.functions.push(callee);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::Buffer(crate::BufferRef::Direct(
                    BufferId::new(0),
                ))],
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program)
            .expect_err("read-only buffer capability escalation should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("argument 0")));
    }

    #[test]
    fn rejects_cast_from_a_non_scalar_value() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
        ]);
        program.functions[1].locals.extend([
            Local {
                integer_range: None,
                name: Some("view".to_owned()),
                ty: test_type(1),
            },
            Local {
                integer_range: None,
                name: None,
                ty: test_type(0),
            },
        ]);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(1)),
                value: Rvalue::Cast {
                    value: Value::Local(LocalId::new(0)),
                    to: ScalarType::F32,
                },
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("aggregate cast source should fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("cast source must be a numeric scalar")));
    }

    #[test]
    fn rejects_casts_between_bool_and_numeric_types() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::I32));
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Cast {
                    value: Value::Constant(ScalarValue::Bool(true)),
                    to: ScalarType::I32,
                },
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("bool cast should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("cast requires numeric scalar types")));
    }

    #[test]
    fn rejects_wrong_intrinsic_arity() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Intrinsic {
                    intrinsic: Intrinsic::Sin,
                    args: Vec::new(),
                },
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("missing intrinsic argument should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("sin") && error.message.contains("expects 1")));
    }

    #[test]
    fn rejects_integer_argument_to_float_intrinsic() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::I32));
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Intrinsic {
                    intrinsic: Intrinsic::Sin,
                    args: vec![Value::Constant(ScalarValue::I32(1))],
                },
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("integer sin argument should fail");
        assert!(errors.iter().any(|error| {
            error.message.contains("sin") && error.message.contains("unsupported type i32")
        }));
    }

    #[test]
    fn rejects_comparison_of_non_scalar_values() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::Bool),
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
        ]);
        program.functions[1].locals.extend([
            Local {
                integer_range: None,
                name: Some("left".to_owned()),
                ty: test_type(1),
            },
            Local {
                integer_range: None,
                name: Some("right".to_owned()),
                ty: test_type(1),
            },
            Local {
                integer_range: None,
                name: None,
                ty: test_type(0),
            },
        ]);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(2)),
                value: Rvalue::Compare {
                    op: CompareOp::Equal,
                    lhs: Value::Local(LocalId::new(0)),
                    rhs: Value::Local(LocalId::new(1)),
                },
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("slice comparison should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("comparison operands must be scalar")));
    }

    #[test]
    fn rejects_slice_copy_with_different_element_types() {
        let mut program = empty_program();
        program.types.extend([
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadWrite,
            },
            Type::Slice {
                element: ScalarType::I32,
                access: AccessMode::ReadOnly,
            },
        ]);
        program.functions[1].locals.extend([
            Local {
                integer_range: None,
                name: Some("destination".to_owned()),
                ty: test_type(0),
            },
            Local {
                integer_range: None,
                name: Some("source".to_owned()),
                ty: test_type(1),
            },
        ]);
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::SliceCopy {
                destination: Value::Local(LocalId::new(0)),
                source: Value::Local(LocalId::new(1)),
            },
            source: SourceSpan::UNKNOWN,
        });

        let errors = super::validate(&program).expect_err("converting slice copy should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("identical element types")));
    }

    #[test]
    fn rejects_local_not_assigned_on_every_fallthrough_branch() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::Bool),
            Type::Scalar(ScalarType::F32),
        ]);
        program.functions[1].locals.extend([
            Local {
                integer_range: None,
                name: Some("condition".to_owned()),
                ty: test_type(0),
            },
            Local {
                integer_range: None,
                name: Some("value".to_owned()),
                ty: test_type(1),
            },
            Local {
                integer_range: None,
                name: Some("copy".to_owned()),
                ty: test_type(1),
            },
        ]);
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::Compare {
                        op: CompareOp::Equal,
                        lhs: Value::Constant(ScalarValue::I32(0)),
                        rhs: Value::Constant(ScalarValue::I32(1)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::If {
                    condition: Value::Local(LocalId::new(0)),
                    then_block: crate::Block {
                        statements: vec![Statement {
                            kind: StatementKind::Assign {
                                destination: Place::local(LocalId::new(1)),
                                value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
                            },
                            source: SourceSpan::UNKNOWN,
                        }],
                    },
                    else_block: crate::Block::default(),
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(2)),
                    value: Rvalue::Use(Value::Local(LocalId::new(1))),
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        let errors = super::validate(&program).expect_err("partial assignment must fail");
        assert!(errors.iter().any(|error| {
            error.message.contains("value")
                && error.message.contains("before it is definitely assigned")
        }));
    }

    #[test]
    fn accepts_local_assigned_on_both_fallthrough_branches() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::Bool),
            Type::Scalar(ScalarType::F32),
        ]);
        program.functions[1].locals.extend([
            Local {
                integer_range: None,
                name: Some("condition".to_owned()),
                ty: test_type(0),
            },
            Local {
                integer_range: None,
                name: Some("value".to_owned()),
                ty: test_type(1),
            },
            Local {
                integer_range: None,
                name: Some("copy".to_owned()),
                ty: test_type(1),
            },
        ]);
        let assignment = |value| Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(1)),
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(value))),
            },
            source: SourceSpan::UNKNOWN,
        };
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::Compare {
                        op: CompareOp::Equal,
                        lhs: Value::Constant(ScalarValue::I32(0)),
                        rhs: Value::Constant(ScalarValue::I32(1)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::If {
                    condition: Value::Local(LocalId::new(0)),
                    then_block: crate::Block {
                        statements: vec![assignment(1.0)],
                    },
                    else_block: crate::Block {
                        statements: vec![assignment(2.0)],
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(2)),
                    value: Rvalue::Use(Value::Local(LocalId::new(1))),
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        super::validate(&program).expect("both branches establish the local");
    }

    #[test]
    fn rejects_process_frame_that_is_overwritten_before_audio_use() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.outputs.push(Output {
            name: "out1".to_owned(),
            ty: test_type(0),
        });
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("frame".to_owned()),
            ty: TypeId::new(0),
        });
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::ProcessFrame {
                        offset: Value::Constant(ScalarValue::I32(0)),
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::Use(Value::Constant(ScalarValue::I32(0))),
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::OutputStore {
                    output: OutputId::new(0),
                    element: None,
                    bounds: crate::BoundsMode::Unchecked,
                    frame: Value::Local(LocalId::new(0)),
                    value: Value::Constant(ScalarValue::F32(0.0)),
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        let errors = super::validate(&program).expect_err("overwritten frame must fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("dominated by process_frame")));
    }

    #[test]
    fn tracks_elementwise_fixed_array_initialization() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Array {
                element: test_type(0),
                len: 2,
            },
        ]);
        program.functions[1].locals.extend([
            Local {
                integer_range: None,
                name: Some("values".to_owned()),
                ty: test_type(1),
            },
            Local {
                integer_range: None,
                name: Some("index".to_owned()),
                ty: TypeId::new(0),
            },
            Local {
                integer_range: None,
                name: Some("value".to_owned()),
                ty: test_type(0),
            },
        ]);
        for index in 0..2 {
            program.functions[1].body.statements.push(Statement {
                kind: StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::Local(LocalId::new(0)),
                        projections: vec![Projection::Index {
                            index: Value::Constant(ScalarValue::I32(index)),
                            bounds: crate::BoundsMode::Unchecked,
                        }],
                    },
                    value: Rvalue::Use(Value::Constant(ScalarValue::F32(index as f32))),
                },
                source: SourceSpan::UNKNOWN,
            });
        }
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(1)),
                    value: Rvalue::Use(Value::Constant(ScalarValue::I32(1))),
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(2)),
                    value: Rvalue::Load(Place {
                        base: PlaceBase::Local(LocalId::new(0)),
                        projections: vec![Projection::Index {
                            index: Value::Local(LocalId::new(1)),
                            bounds: crate::BoundsMode::Checked,
                        }],
                    }),
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        validate_producer(&program).expect("all fixed-array elements are initialized");

        program.functions[1].body.statements.remove(1);
        let errors =
            super::validate(&program).expect_err("dynamic read of a partial array must fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("indexed before it is definitely assigned")));
    }

    #[test]
    fn validates_checked_slice_ranges_and_rejects_invalid_unchecked_constants() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Array {
                element: test_type(0),
                len: 4,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
        ]);
        program.state.push(StateSlot {
            integer_range: None,
            name: "values".to_owned(),
            ty: test_type(1),
            persistence: StatePersistence::Snapshot,
            authored: true,
            pinned: false,
        });
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("view".to_owned()),
            ty: test_type(2),
        });
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::MakeSlice {
                    source: SliceSource::Place(Place {
                        base: PlaceBase::State(crate::StateId::new(0)),
                        projections: Vec::new(),
                    }),
                    start: Value::Constant(ScalarValue::I32(4)),
                    len: Value::Constant(ScalarValue::I32(0)),
                    bounds: crate::BoundsMode::Checked,
                    access: AccessMode::ReadOnly,
                },
            },
            source: SourceSpan::UNKNOWN,
        });
        super::validate(&program).expect("one-past-end empty slice is valid");

        let make_slice = &mut program.functions[1].body.statements[0].kind;
        let StatementKind::Assign {
            value: Rvalue::MakeSlice { bounds, .. },
            ..
        } = make_slice
        else {
            unreachable!()
        };
        *bounds = crate::BoundsMode::Unchecked;
        let errors = super::validate(&program)
            .expect_err("ordinary validation must reject producer-proved unchecked access");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("trusted MIR producer proof")));
        unsafe { super::validate_with_producer_proofs(&program) }
            .expect("trusted producer validation accepts the proven empty range");

        let StatementKind::Assign {
            value: Rvalue::MakeSlice { len, .. },
            ..
        } = &mut program.functions[1].body.statements[0].kind
        else {
            unreachable!()
        };
        *len = Value::Constant(ScalarValue::I32(1));
        let errors = unsafe { super::validate_with_producer_proofs(&program) }
            .expect_err("trusted producers still reject statically invalid unchecked slices");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("statically out-of-bounds range")));
    }

    #[test]
    fn control_outputs_use_explicit_one_to_one_mirror_identity() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.state.push(StateSlot {
            integer_range: None,
            name: "different_debug_name".to_owned(),
            ty: test_type(0),
            persistence: StatePersistence::ControlMirror,
            authored: true,
            pinned: false,
        });
        program
            .interface
            .control_outputs
            .push(crate::ControlOutput {
                name: "meter".to_owned(),
                ty: test_type(0),
                mirror: crate::StateId::new(0),
            });
        super::validate(&program).expect("mirror identity is ID-based, not name-based");

        program
            .interface
            .control_outputs
            .push(crate::ControlOutput {
                name: "other_meter".to_owned(),
                ty: test_type(0),
                mirror: crate::StateId::new(0),
            });
        let errors = super::validate(&program).expect_err("duplicate mirror must fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("referenced by 2 control outputs")));
    }

    #[test]
    fn rejects_duplicate_host_interface_names_and_event_parameter_names() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.interface.inputs.push(crate::Input {
            name: "shared".to_owned(),
            ty: test_type(0),
            default: None,
            range: None,
        });
        program.interface.params.push(Param {
            name: "shared".to_owned(),
            ty: test_type(0),
            default: ConstantValue::Scalar(ScalarValue::F32(0.0)),
            range: None,
            control: crate::ParamControl::default(),
        });
        program.interface.events.push(Event {
            name: "update".to_owned(),
            params: vec![
                EventParam {
                    name: "value".to_owned(),
                    ty: test_type(0),
                    default: None,
                },
                EventParam {
                    name: "value".to_owned(),
                    ty: test_type(0),
                    default: None,
                },
            ],
            handler: FunctionId::new(2),
        });
        program.functions.push(function(
            "onda_event::update",
            FunctionKind::Event(EventId::new(0)),
        ));

        let errors = super::validate(&program).expect_err("duplicate ABI names must fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("interface name 'shared'")));
        assert!(errors
            .iter()
            .any(|error| error.message.contains("duplicate parameter name 'value'")));
    }

    #[test]
    fn rejects_recursive_and_oversized_fixed_aggregate_types() {
        let mut recursive = empty_program();
        recursive.types.push(Type::Array {
            element: test_type(0),
            len: 1,
        });
        let errors = super::validate(&recursive).expect_err("recursive aggregate must fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("recursive fixed aggregate")));

        let mut oversized = empty_program();
        oversized.types.extend([
            Type::Scalar(ScalarType::F64),
            Type::Array {
                element: test_type(0),
                len: i32::MAX as u32,
            },
        ]);
        let errors = super::validate(&oversized).expect_err("oversized aggregate must fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("logical size exceeds")));
    }

    #[test]
    fn constant_data_logical_bytes_fit_the_signed_i32_boundary() {
        let maximum_f64_elements = i32::MAX as usize / 8;
        assert!(super::scalar_sequence_fits_i32_bytes(
            maximum_f64_elements,
            ScalarType::F64
        ));
        assert!(!super::scalar_sequence_fits_i32_bytes(
            maximum_f64_elements + 1,
            ScalarType::F64
        ));
    }

    #[test]
    fn slice_element_is_scalar_only_and_fixed_arrays_use_explicit_windows() {
        let mut program = empty_program();
        program.types.extend([
            Type::Scalar(ScalarType::F32),
            Type::Array {
                element: test_type(0),
                len: 2,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadWrite,
            },
        ]);
        program.state.push(StateSlot {
            integer_range: None,
            name: "values".to_owned(),
            ty: test_type(1),
            persistence: StatePersistence::Snapshot,
            authored: true,
            pinned: false,
        });
        let mut callee = function("consume_pair", FunctionKind::User);
        callee.params.push(FunctionParam {
            integer_range: None,
            name: "pair".to_owned(),
            ty: test_type(1),
            mode: PassingMode::ReadOnlyReference,
        });
        program.functions.push(callee);
        program.functions[1].locals.push(Local {
            integer_range: None,
            name: Some("view".to_owned()),
            ty: test_type(2),
        });
        program.functions[1].body.statements.extend([
            Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(0)),
                    value: Rvalue::MakeSlice {
                        source: SliceSource::Place(Place {
                            base: PlaceBase::State(crate::StateId::new(0)),
                            projections: Vec::new(),
                        }),
                        start: Value::Constant(ScalarValue::I32(0)),
                        len: Value::Constant(ScalarValue::I32(2)),
                        bounds: crate::BoundsMode::Unchecked,
                        access: AccessMode::ReadWrite,
                    },
                },
                source: SourceSpan::UNKNOWN,
            },
            Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function: FunctionId::new(2),
                    args: vec![CallArgument::SliceWindow {
                        slice: Value::Local(LocalId::new(0)),
                        start: Value::Constant(ScalarValue::I32(0)),
                        bounds: crate::BoundsMode::Checked,
                    }],
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);
        validate_producer(&program).expect("slice window safely carries a fixed-array ref");

        let StatementKind::Call { args, .. } = &mut program.functions[1].body.statements[1].kind
        else {
            unreachable!()
        };
        args[0] = CallArgument::SliceElement {
            slice: Value::Local(LocalId::new(0)),
            index: Value::Constant(ScalarValue::I32(0)),
            bounds: crate::BoundsMode::Checked,
        };
        let errors =
            super::validate(&program).expect_err("slice element cannot impersonate an array");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("argument 0")));
    }

    #[test]
    fn rejects_control_output_store_from_event_handler() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.state.push(StateSlot {
            integer_range: None,
            name: "meter".to_owned(),
            ty: test_type(0),
            persistence: StatePersistence::ControlMirror,
            authored: true,
            pinned: false,
        });
        program
            .interface
            .control_outputs
            .push(crate::ControlOutput {
                name: "meter".to_owned(),
                ty: test_type(0),
                mirror: crate::StateId::new(0),
            });
        program.interface.events.push(Event {
            name: "update".to_owned(),
            params: Vec::new(),
            handler: FunctionId::new(2),
        });
        let mut handler = function("onda_event::update", FunctionKind::Event(EventId::new(0)));
        handler.body.statements.push(Statement {
            kind: StatementKind::ControlOutputStore {
                output: crate::ControlOutputId::new(0),
                element: None,
                bounds: crate::BoundsMode::Unchecked,
                value: Value::Constant(ScalarValue::F32(1.0)),
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(handler);

        let errors = super::validate(&program).expect_err("events cannot write control outputs");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("only valid in the process")));
    }

    #[test]
    fn control_mirrors_are_readable_but_only_control_stores_can_mutate_them() {
        let mut program = empty_program();
        program.types.push(Type::Scalar(ScalarType::F32));
        program.state.push(StateSlot {
            integer_range: None,
            name: "meter_mirror".to_owned(),
            ty: test_type(0),
            persistence: StatePersistence::ControlMirror,
            authored: true,
            pinned: false,
        });
        program
            .interface
            .control_outputs
            .push(crate::ControlOutput {
                name: "meter".to_owned(),
                ty: test_type(0),
                mirror: crate::StateId::new(0),
            });

        let direct_write = Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::State(crate::StateId::new(0)),
                    projections: Vec::new(),
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
            },
            source: SourceSpan::UNKNOWN,
        };
        program.functions[0]
            .body
            .statements
            .push(direct_write.clone());
        program.functions[1].body.statements.extend([
            direct_write.clone(),
            Statement {
                kind: StatementKind::ControlOutputStore {
                    output: crate::ControlOutputId::new(0),
                    element: None,
                    bounds: crate::BoundsMode::Unchecked,
                    value: Value::Constant(ScalarValue::F32(1.0)),
                },
                source: SourceSpan::UNKNOWN,
            },
        ]);

        let mut user = function("bad_user", FunctionKind::User);
        user.body.statements.push(direct_write.clone());
        program.functions.push(user);

        let handler_id = FunctionId::new(3);
        program.interface.events.push(Event {
            name: "bad_event".to_owned(),
            params: Vec::new(),
            handler: handler_id,
        });
        let mut event = function(
            "onda_event::bad_event",
            FunctionKind::Event(EventId::new(0)),
        );
        event.body.statements.push(direct_write);
        program.functions.push(event);

        let errors =
            super::validate(&program).expect_err("direct control-mirror writes must be rejected");
        assert_eq!(
            errors
                .iter()
                .filter(|error| error
                    .message
                    .contains("assignment destination is not writable"))
                .count(),
            4
        );
        assert!(!errors.iter().any(|error| error
            .message
            .contains("control output stores are only valid")));
    }
}
