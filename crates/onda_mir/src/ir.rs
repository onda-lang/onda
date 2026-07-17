use crate::{
    AccessMode, BufferChannels, BufferId, ConstDataId, ConstantValue, ControlOutputId, EventId,
    EventParamId, FieldId, FunctionId, InputId, LocalId, OutputId, ParamId, ParameterId,
    ScalarType, ScalarValue, SourceFileId, StateId, Type, TypeId, ValueRange, MIR_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Serialize, Deserialize)]
pub struct SourceSpan {
    pub file: Option<SourceFileId>,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

impl SourceSpan {
    pub const UNKNOWN: Self = Self {
        file: None,
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
    };

    pub const fn is_unknown(self) -> bool {
        self.line == 0
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CompileConfig {
    pub sample_rate: f32,
    pub block_size: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub schema_version: u32,
    pub config: CompileConfig,
    pub source_files: Vec<SourceFile>,
    pub types: Vec<Type>,
    pub structs: Vec<StructType>,
    pub interface: Interface,
    pub state: Vec<StateSlot>,
    pub const_data: Vec<ConstData>,
    pub functions: Vec<Function>,
    pub entry_points: EntryPoints,
}

impl Program {
    pub fn new(config: CompileConfig, init: FunctionId, process: FunctionId) -> Self {
        Self {
            schema_version: MIR_SCHEMA_VERSION,
            config,
            source_files: Vec::new(),
            types: Vec::new(),
            structs: Vec::new(),
            interface: Interface::default(),
            state: Vec::new(),
            const_data: Vec::new(),
            functions: Vec::new(),
            entry_points: EntryPoints { init, process },
        }
    }

    /// Returns whether two IDs denote the same logical MIR type.
    ///
    /// Type tables may contain duplicate entries after deserialization, so
    /// consumers must not require canonical ID identity for equivalent types.
    pub fn types_equivalent(&self, lhs: TypeId, rhs: TypeId) -> bool {
        self.types_equivalent_inner(lhs, rhs, &mut HashSet::new())
    }

    fn types_equivalent_inner(
        &self,
        lhs: TypeId,
        rhs: TypeId,
        visiting: &mut HashSet<(TypeId, TypeId)>,
    ) -> bool {
        if lhs == rhs {
            return true;
        }
        let Some((lhs_ty, rhs_ty)) = self.types.get(lhs.index()).zip(self.types.get(rhs.index()))
        else {
            return false;
        };
        if !visiting.insert((lhs, rhs)) {
            // Recursive fixed aggregates are invalid MIR, but treating an
            // active pair coinductively keeps diagnostics finite.
            return true;
        }
        let equivalent = match (lhs_ty, rhs_ty) {
            (Type::Scalar(lhs), Type::Scalar(rhs)) => lhs == rhs,
            (Type::Tuple(lhs), Type::Tuple(rhs)) => {
                lhs.len() == rhs.len()
                    && lhs
                        .iter()
                        .zip(rhs)
                        .all(|(lhs, rhs)| self.types_equivalent_inner(*lhs, *rhs, visiting))
            }
            (
                Type::Array {
                    element: lhs_element,
                    len: lhs_len,
                },
                Type::Array {
                    element: rhs_element,
                    len: rhs_len,
                },
            ) => {
                lhs_len == rhs_len
                    && self.types_equivalent_inner(*lhs_element, *rhs_element, visiting)
            }
            // Struct definitions are nominal even when their fields happen to
            // have the same shape.
            (Type::Struct(lhs), Type::Struct(rhs)) => lhs == rhs,
            (
                Type::Slice {
                    element: lhs_element,
                    access: lhs_access,
                },
                Type::Slice {
                    element: rhs_element,
                    access: rhs_access,
                },
            ) => lhs_element == rhs_element && lhs_access == rhs_access,
            (
                Type::Buffer {
                    element: lhs_element,
                    channels: lhs_channels,
                    access: lhs_access,
                },
                Type::Buffer {
                    element: rhs_element,
                    channels: rhs_channels,
                    access: rhs_access,
                },
            ) => {
                lhs_element == rhs_element
                    && lhs_channels == rhs_channels
                    && lhs_access == rhs_access
            }
            _ => false,
        };
        visiting.remove(&(lhs, rhs));
        equivalent
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructType {
    pub name: String,
    pub fields: Vec<StructField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructField {
    pub name: String,
    pub ty: TypeId,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Interface {
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub control_outputs: Vec<ControlOutput>,
    pub params: Vec<Param>,
    pub buffers: Vec<Buffer>,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    pub ty: TypeId,
    pub default: Option<ConstantValue>,
    pub range: Option<ValueRange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub name: String,
    pub ty: TypeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlOutput {
    pub name: String,
    pub ty: TypeId,
    /// Dedicated state slot containing the host-visible control value.
    ///
    /// The referenced slot must have [`StatePersistence::ControlMirror`].
    /// Names are diagnostic only; this ID is the storage identity.
    pub mirror: StateId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: TypeId,
    pub default: ConstantValue,
    pub range: Option<ValueRange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Buffer {
    pub name: String,
    pub element: ScalarType,
    pub channels: BufferChannels,
    pub access: AccessMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    pub params: Vec<EventParam>,
    pub handler: FunctionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventParam {
    pub name: String,
    pub ty: TypeId,
    pub default: Option<ConstantValue>,
}

/// Per-instance storage. Physical storage for every slot is zero-initialized
/// before the MIR `init` entry point runs, so `init` only needs to write
/// dynamic or nonzero initial values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateSlot {
    pub name: String,
    pub ty: TypeId,
    pub persistence: StatePersistence,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatePersistence {
    Snapshot,
    InstanceScratch,
    ControlMirror,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstData {
    pub name: String,
    pub element: ScalarType,
    /// Element count and logical byte size must both fit the signed i32 MIR
    /// addressing boundary.
    pub values: Vec<ScalarValue>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct EntryPoints {
    pub init: FunctionId,
    pub process: FunctionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub kind: FunctionKind,
    pub attributes: FunctionAttributes,
    pub params: Vec<FunctionParam>,
    pub results: Vec<TypeId>,
    pub locals: Vec<Local>,
    pub body: Block,
    pub source: SourceSpan,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum FunctionKind {
    Init,
    Process,
    Event(EventId),
    User,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct FunctionAttributes {
    pub origin: FunctionOrigin,
    pub inline: InlineHint,
}

impl Default for FunctionAttributes {
    fn default() -> Self {
        Self {
            origin: FunctionOrigin::Source,
            inline: InlineHint::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionOrigin {
    Source,
    CompilerGenerated,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InlineHint {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionParam {
    pub name: String,
    pub ty: TypeId,
    pub mode: PassingMode,
}

/// Builds the canonical segmented-process entry signature.
pub fn process_function_params(i32_ty: TypeId) -> Vec<FunctionParam> {
    crate::PROCESS_PARAM_NAMES
        .into_iter()
        .map(|name| FunctionParam {
            name: name.to_owned(),
            ty: i32_ty,
            mode: PassingMode::Value,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassingMode {
    Value,
    ReadOnlyReference,
    ReadWriteReference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Local {
    pub name: Option<String>,
    pub ty: TypeId,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Block {
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub kind: StatementKind,
    pub source: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum StatementKind {
    Assign {
        destination: Place,
        value: Rvalue,
    },
    Call {
        results: Vec<LocalId>,
        function: FunctionId,
        args: Vec<CallArgument>,
    },
    OutputStore {
        output: OutputId,
        element: Option<Value>,
        bounds: BoundsMode,
        frame: Value,
        value: Value,
    },
    ControlOutputStore {
        output: ControlOutputId,
        element: Option<Value>,
        bounds: BoundsMode,
        value: Value,
    },
    BufferStore {
        buffer: BufferId,
        channel: Option<Value>,
        index: Value,
        value: Value,
        bounds: BoundsMode,
    },
    BufferParamStore {
        parameter: ParameterId,
        channel: Option<Value>,
        index: Value,
        value: Value,
        bounds: BoundsMode,
    },
    SliceStore {
        slice: Value,
        index: Value,
        value: Value,
        bounds: BoundsMode,
    },
    SliceFill {
        destination: Value,
        value: Value,
    },
    /// Copies `min(destination.len, source.len)` elements.
    ///
    /// Equal-stride/contiguous overlap is memmove-safe. If unequal-stride views
    /// overlap, execution deterministically traps; MIR does not imply hidden
    /// realtime scratch allocation.
    SliceCopy {
        destination: Value,
        source: Value,
    },
    If {
        condition: Value,
        then_block: Block,
        else_block: Block,
    },
    Loop {
        body: Block,
    },
    Break,
    Continue,
    Return {
        values: Vec<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Rvalue {
    Use(Value),
    Load(Place),
    Unary {
        op: UnaryOp,
        operand: Value,
    },
    Binary {
        op: BinaryOp,
        lhs: Value,
        rhs: Value,
    },
    Compare {
        op: CompareOp,
        lhs: Value,
        rhs: Value,
    },
    /// Explicit numeric conversion. Float-to-integer conversion saturates to
    /// the destination range and maps NaN to zero.
    Cast {
        value: Value,
        to: ScalarType,
    },
    Intrinsic {
        intrinsic: Intrinsic,
        args: Vec<Value>,
    },
    /// Produces the full-block audio frame for a process-segment-relative
    /// offset, trapping unless `0 <= offset < frames`.
    ///
    /// Audio I/O operations only accept values produced directly by this
    /// operation. This keeps host buffer addressing safe and gives every
    /// backend one canonical place to implement segmented-frame semantics.
    ProcessFrame {
        offset: Value,
    },
    InputLoad {
        input: InputId,
        element: Option<Value>,
        bounds: BoundsMode,
        frame: Value,
    },
    OutputLoad {
        output: OutputId,
        element: Option<Value>,
        bounds: BoundsMode,
        frame: Value,
    },
    BufferLoad {
        buffer: BufferId,
        channel: Option<Value>,
        index: Value,
        bounds: BoundsMode,
    },
    BufferParamLoad {
        parameter: ParameterId,
        channel: Option<Value>,
        index: Value,
        bounds: BoundsMode,
    },
    BufferLen(BufferId),
    BufferChannels(BufferId),
    BufferSampleRate(BufferId),
    BufferParamLen(ParameterId),
    BufferParamChannels(ParameterId),
    BufferParamSampleRate(ParameterId),
    ConstDataLoad {
        data: ConstDataId,
        index: Value,
        bounds: BoundsMode,
    },
    /// Constructs a checked subview of the logical source range.
    ///
    /// `Clamp` normalizes `start` to `0..=source_len`, clamps negative `len` to
    /// zero, then clamps `len` to the remaining source range. `Trap` rejects
    /// any negative or out-of-range component. `Unchecked` requires the
    /// producer to prove `0 <= start <= source_len` and
    /// `0 <= len <= source_len - start`.
    ///
    /// Empty slices are valid, including the one-past-end `start == source_len`
    /// case. Any indexed operation on an empty slice traps even in `Clamp`
    /// mode because no valid element exists.
    MakeSlice {
        source: SliceSource,
        start: Value,
        len: Value,
        bounds: BoundsMode,
        access: AccessMode,
    },
    SliceLoad {
        slice: Value,
        index: Value,
        bounds: BoundsMode,
    },
    SliceLen(Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum SliceSource {
    Place(Place),
    Buffer {
        buffer: BufferId,
        channel: Option<Value>,
    },
    BufferParam {
        parameter: ParameterId,
        channel: Option<Value>,
    },
    ConstData(ConstDataId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum CallArgument {
    Value(Value),
    Place(Place),
    SliceElement {
        slice: Value,
        index: Value,
        bounds: BoundsMode,
    },
    /// A contiguous fixed-array reference beginning at `start` in `array`.
    ///
    /// The required window length is the callee parameter's fixed array
    /// length. Bounds apply to the complete window, not only its first element.
    ArrayWindow {
        array: Place,
        start: Value,
        bounds: BoundsMode,
    },
    /// A contiguous fixed-array reference beginning at `start` in `slice`.
    ///
    /// Checked modes require both a complete in-bounds window and a unit-stride
    /// slice descriptor. A non-contiguous descriptor traps rather than being
    /// reinterpreted as a native fixed array.
    SliceWindow {
        slice: Value,
        start: Value,
        bounds: BoundsMode,
    },
    Buffer(BufferId),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Value {
    Local(LocalId),
    Constant(ScalarValue),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub base: PlaceBase,
    pub projections: Vec<Projection>,
}

impl Place {
    pub fn local(local: LocalId) -> Self {
        Self {
            base: PlaceBase::Local(local),
            projections: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum PlaceBase {
    Local(LocalId),
    Parameter(ParameterId),
    State(StateId),
    Param(ParamId),
    EventParam(EventParamId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Projection {
    Field(FieldId),
    Index { index: Value, bounds: BoundsMode },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundsMode {
    /// Clamp to the nearest valid index/range. If the collection is empty and
    /// therefore has no valid element, an indexed operation traps.
    Clamp,
    /// Trap when an index or complete range is out of bounds.
    Trap,
    /// The producer guarantees the complete index/range is valid.
    Unchecked,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnaryOp {
    Negate,
    LogicalNot,
    BitNot,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOp {
    /// Wrapping addition for integers; IEEE addition at the operand width for floats.
    Add,
    /// Wrapping subtraction for integers; IEEE subtraction at the operand width for floats.
    Subtract,
    /// Wrapping multiplication for integers; IEEE multiplication at the operand width for floats.
    Multiply,
    /// Signed integer division traps on zero and wraps `MIN / -1` to `MIN`.
    Divide,
    /// Signed integer remainder traps on zero and yields zero for `MIN % -1`.
    Remainder,
    BitAnd,
    BitOr,
    BitXor,
    /// Integer shift counts are masked to the left operand width.
    ShiftLeft,
    /// Integer shift counts are masked to the left operand width.
    ShiftRight,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Equal,
    /// Floating-point `!=` is unordered: it is true when either operand is NaN.
    NotEqual,
    /// Floating-point relational comparisons are ordered and false for NaN.
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intrinsic {
    Sin,
    Cos,
    Tan,
    Tanh,
    Atan,
    Atan2,
    Exp,
    Log,
    Sqrt,
    Pow,
    Abs,
    Floor,
    Ceil,
    Round,
    Trunc,
    Min,
    Max,
    Fma,
}
