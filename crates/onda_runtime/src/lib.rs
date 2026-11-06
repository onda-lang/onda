use onda_codegen_llvm::{
    BufferDescriptorTables, DeclaredBufferChannels, JitProgram, RuntimeAllocator, RuntimeBuffer,
    RuntimeState, UninitializedRuntimeState,
};
use onda_frontend::{Diagnostic, PrimitiveType};
use std::fmt::{self, Write as _};
use std::marker::PhantomData;

pub use onda_codegen_llvm::{
    EventTensorView, ParamDomain, ParamScalarType, ParamScale, RuntimeError, RuntimeSurface,
};
pub use onda_processor_abi::{
    InitMode, PROCESSOR_BEGIN_BLOCK, PROCESSOR_END_BLOCK, PROCESSOR_EXECUTION_INPUT_REJECTED,
    PROCESSOR_EXECUTION_OK, PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE, PROCESSOR_FULL_BLOCK,
};

const BATCH_RECORD_HEADER_SIZE: usize = 12;

/// Bytes occupied by the delegate index, payload length, and sequence header of each occurrence.
pub const DELEGATE_RECORD_HEADER_SIZE: usize = BATCH_RECORD_HEADER_SIZE;

/// A non-owning decoded view into one occurrence in a [`DelegateBatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegateOccurrence<'batch> {
    pub delegate_index: u32,
    pub sequence: u32,
    pub payload: &'batch [u8],
}

/// Why a caller-owned delegate or print batch could not be decoded.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BatchDecodeError {
    /// The batch counters and storage pointer do not describe a valid batch envelope.
    InvalidEnvelope,
    /// A declared record is truncated or its payload length exceeds the remaining storage.
    MalformedRecord { record_index: u32 },
    /// Bytes remain after every declared record has been decoded.
    TrailingBytes,
}

impl fmt::Display for BatchDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvelope => {
                formatter.write_str("batch counters and storage are inconsistent")
            }
            Self::MalformedRecord { record_index } => {
                write!(
                    formatter,
                    "batch record {record_index} is truncated or malformed"
                )
            }
            Self::TrailingBytes => formatter.write_str("batch contains trailing record bytes"),
        }
    }
}

impl std::error::Error for BatchDecodeError {}

#[derive(Debug, Clone, Copy)]
struct BatchRecord<'batch> {
    index: u32,
    sequence: u32,
    payload: &'batch [u8],
}

#[derive(Debug, Clone)]
struct BatchRecords<'batch> {
    storage: &'batch [u8],
    cursor: usize,
    record_index: u32,
    record_count: u32,
    pending_error: Option<BatchDecodeError>,
    finished: bool,
}

impl<'batch> BatchRecords<'batch> {
    fn new(storage: Option<&'batch [u8]>, record_count: u32) -> Self {
        Self {
            storage: storage.unwrap_or(&[]),
            cursor: 0,
            record_index: 0,
            record_count,
            pending_error: storage
                .is_none()
                .then_some(BatchDecodeError::InvalidEnvelope),
            finished: false,
        }
    }

    fn next_record(&mut self) -> Option<Result<BatchRecord<'batch>, BatchDecodeError>> {
        if self.finished {
            return None;
        }
        if let Some(error) = self.pending_error.take() {
            self.finished = true;
            return Some(Err(error));
        }
        if self.record_index == self.record_count {
            self.finished = true;
            return (self.cursor != self.storage.len())
                .then_some(Err(BatchDecodeError::TrailingBytes));
        }

        let malformed = || BatchDecodeError::MalformedRecord {
            record_index: self.record_index,
        };
        let Some(header_end) = self.cursor.checked_add(BATCH_RECORD_HEADER_SIZE) else {
            self.finished = true;
            return Some(Err(malformed()));
        };
        let Some(header) = self.storage.get(self.cursor..header_end) else {
            self.finished = true;
            return Some(Err(malformed()));
        };
        let index = u32::from_ne_bytes(header[..4].try_into().expect("fixed record header"));
        let payload_bytes =
            u32::from_ne_bytes(header[4..8].try_into().expect("fixed record header")) as usize;
        let sequence = u32::from_ne_bytes(header[8..12].try_into().expect("fixed record header"));
        let Some(record_end) = header_end.checked_add(payload_bytes) else {
            self.finished = true;
            return Some(Err(malformed()));
        };
        let Some(payload) = self.storage.get(header_end..record_end) else {
            self.finished = true;
            return Some(Err(malformed()));
        };
        self.cursor = record_end;
        self.record_index += 1;
        Some(Ok(BatchRecord {
            index,
            sequence,
            payload,
        }))
    }
}

/// Allocation-free iterator over the complete records in a [`DelegateBatch`].
#[derive(Debug, Clone)]
pub struct DelegateOccurrences<'batch> {
    records: BatchRecords<'batch>,
}

impl<'batch> Iterator for DelegateOccurrences<'batch> {
    type Item = Result<DelegateOccurrence<'batch>, BatchDecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.records.next_record().map(|record| {
            record.map(|record| DelegateOccurrence {
                delegate_index: record.index,
                sequence: record.sequence,
                payload: record.payload,
            })
        })
    }
}

impl std::iter::FusedIterator for DelegateOccurrences<'_> {}

unsafe fn used_batch_storage<'batch>(
    storage: *const u8,
    capacity_bytes: u32,
    used_bytes: u32,
    record_count: u32,
) -> Option<&'batch [u8]> {
    if used_bytes > capacity_bytes
        || (record_count == 0 && used_bytes != 0)
        || (record_count != 0 && storage.is_null())
    {
        return None;
    }
    if used_bytes == 0 {
        return Some(&[]);
    }
    // SAFETY: the batch constructors require this region to remain valid for the batch storage
    // lifetime, and the envelope checks above keep the slice within its declared capacity.
    Some(unsafe { std::slice::from_raw_parts(storage, used_bytes as usize) })
}

/// Caller-owned, call-scoped storage for externally published delegate occurrences.
///
/// The runtime adapts this host-facing descriptor to the versioned processor ABI at each
/// generated entry point. The backing storage is never allocated or retained by the runtime. Each
/// process or event call resets the result counters. Iterate successful records with
/// [`DelegateBatch::occurrences`] before reusing the batch, and inspect `overflow_count` to detect a
/// truncated host-facing stream.
#[repr(C)]
#[derive(Debug)]
pub struct DelegateBatch<'storage> {
    storage: *mut u8,
    capacity_bytes: u32,
    pub used_bytes: u32,
    pub record_count: u32,
    pub overflow_count: u32,
    _storage: PhantomData<&'storage mut [u8]>,
}

/// Bytes occupied by the print-site index, payload length, and sequence header of each occurrence.
pub const PRINT_RECORD_HEADER_SIZE: usize = BATCH_RECORD_HEADER_SIZE;

/// A non-owning decoded view into one occurrence in a [`PrintBatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintOccurrence<'batch> {
    pub site_index: u32,
    pub sequence: u32,
    pub payload: &'batch [u8],
}

/// Allocation-free iterator over the complete records in a [`PrintBatch`].
#[derive(Debug, Clone)]
pub struct PrintOccurrences<'batch> {
    records: BatchRecords<'batch>,
}

impl<'batch> Iterator for PrintOccurrences<'batch> {
    type Item = Result<PrintOccurrence<'batch>, BatchDecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.records.next_record().map(|record| {
            record.map(|record| PrintOccurrence {
                site_index: record.index,
                sequence: record.sequence,
                payload: record.payload,
            })
        })
    }
}

impl std::iter::FusedIterator for PrintOccurrences<'_> {}

#[repr(C)]
#[derive(Debug)]
pub struct PrintBatch<'storage> {
    storage: *mut u8,
    capacity_bytes: u32,
    pub used_bytes: u32,
    pub record_count: u32,
    pub overflow_count: u32,
    _storage: PhantomData<&'storage mut [u8]>,
}

impl<'storage> PrintBatch<'storage> {
    /// Creates a reusable batch over caller-owned storage.
    pub fn from_storage(storage: &'storage mut [u8]) -> Self {
        Self {
            storage: storage.as_mut_ptr(),
            capacity_bytes: u32::try_from(storage.len()).unwrap_or(u32::MAX),
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
            _storage: PhantomData,
        }
    }

    /// Creates a present batch without record storage.
    pub const fn absent() -> Self {
        Self {
            storage: std::ptr::null_mut(),
            capacity_bytes: 0,
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
            _storage: PhantomData,
        }
    }

    /// Creates a batch over caller-managed storage.
    ///
    /// # Safety
    ///
    /// When `storage` is non-null, it must remain exclusively writable for `capacity_bytes` bytes
    /// throughout `'storage`. A null pointer creates an absent batch and ignores the capacity.
    pub unsafe fn from_raw_parts(storage: *mut u8, capacity_bytes: u32) -> Self {
        Self {
            storage,
            capacity_bytes: if storage.is_null() { 0 } else { capacity_bytes },
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
            _storage: PhantomData,
        }
    }

    /// Returns the caller-owned storage capacity.
    pub fn capacity_bytes(&self) -> u32 {
        self.capacity_bytes
    }

    /// Iterates the complete occurrences produced by the most recent successful call.
    ///
    /// Malformed input yields one [`BatchDecodeError`] and then ends the fused iterator.
    pub fn occurrences(&self) -> PrintOccurrences<'_> {
        PrintOccurrences {
            records: BatchRecords::new(self.used_storage(), self.record_count),
        }
    }

    /// Clears result counters without modifying the storage or capacity.
    pub fn reset(&mut self) {
        self.used_bytes = 0;
        self.record_count = 0;
        self.overflow_count = 0;
    }

    fn used_storage(&self) -> Option<&[u8]> {
        // SAFETY: the constructors establish the pointer lifetime and exclusive-storage contract.
        unsafe {
            used_batch_storage(
                self.storage,
                self.capacity_bytes,
                self.used_bytes,
                self.record_count,
            )
        }
    }
}

pub struct ExecutionOutput<'batch, 'storage> {
    pub delegate_batch: Option<&'batch mut DelegateBatch<'storage>>,
    pub print_batch: Option<&'batch mut PrintBatch<'storage>>,
}

impl ExecutionOutput<'static, 'static> {
    pub const fn none() -> Self {
        Self {
            delegate_batch: None,
            print_batch: None,
        }
    }
}

impl ExecutionOutput<'_, '_> {
    /// Prepares every present caller-owned batch for one processor entry call.
    pub fn reset(&mut self) {
        if let Some(batch) = self.delegate_batch.as_deref_mut() {
            batch.reset();
        }
        if let Some(batch) = self.print_batch.as_deref_mut() {
            batch.reset();
        }
    }
}

impl<'storage> DelegateBatch<'storage> {
    /// Creates a reusable batch over caller-owned storage.
    pub fn from_storage(storage: &'storage mut [u8]) -> Self {
        Self {
            storage: storage.as_mut_ptr(),
            capacity_bytes: u32::try_from(storage.len()).unwrap_or(u32::MAX),
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
            _storage: PhantomData,
        }
    }

    /// Creates a present batch without record storage.
    ///
    /// Host-facing occurrences are ignored without reporting overflow, just as for a `None` batch.
    /// This is useful when host code needs one concrete batch value for both configured and absent
    /// storage paths.
    pub const fn absent() -> Self {
        Self {
            storage: std::ptr::null_mut(),
            capacity_bytes: 0,
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
            _storage: PhantomData,
        }
    }

    /// Creates a batch over caller-managed storage.
    ///
    /// # Safety
    ///
    /// When `storage` is non-null, it must remain exclusively writable for `capacity_bytes` bytes
    /// throughout `'storage`. A null pointer creates an absent batch and ignores the capacity.
    pub unsafe fn from_raw_parts(storage: *mut u8, capacity_bytes: u32) -> Self {
        Self {
            storage,
            capacity_bytes: if storage.is_null() { 0 } else { capacity_bytes },
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
            _storage: PhantomData,
        }
    }

    pub fn capacity_bytes(&self) -> u32 {
        self.capacity_bytes
    }

    /// Iterates the complete occurrences produced by the most recent successful call.
    ///
    /// The iterator is allocation-free and borrows the batch, preventing its storage from being
    /// reused while occurrence payload views remain live. Malformed input yields one
    /// [`BatchDecodeError`] and then ends the fused iterator.
    pub fn occurrences(&self) -> DelegateOccurrences<'_> {
        DelegateOccurrences {
            records: BatchRecords::new(self.used_storage(), self.record_count),
        }
    }

    /// Clears result counters without modifying the storage or capacity.
    pub fn reset(&mut self) {
        self.used_bytes = 0;
        self.record_count = 0;
        self.overflow_count = 0;
    }

    fn used_storage(&self) -> Option<&[u8]> {
        // SAFETY: the constructors establish the pointer lifetime and exclusive-storage contract.
        unsafe {
            used_batch_storage(
                self.storage,
                self.capacity_bytes,
                self.used_bytes,
                self.record_count,
            )
        }
    }
}

fn with_processor_execution_output<T>(
    output: ExecutionOutput<'_, '_>,
    f: impl FnOnce(Option<&mut onda_processor_abi::ExecutionOutput>) -> T,
) -> T {
    let mut processor_output = onda_processor_abi::ExecutionOutput {
        delegate_batch: output.delegate_batch.map_or(std::ptr::null_mut(), |batch| {
            (batch as *mut DelegateBatch<'_>).cast::<onda_processor_abi::DelegateBatch>()
        }),
        print_batch: output.print_batch.map_or(std::ptr::null_mut(), |batch| {
            (batch as *mut PrintBatch<'_>).cast::<onda_processor_abi::PrintBatch>()
        }),
        next_sequence: 0,
    };
    if processor_output.delegate_batch.is_null() && processor_output.print_batch.is_null() {
        f(None)
    } else {
        f(Some(&mut processor_output))
    }
}

mod parameters;
pub use parameters::{
    read_control_output_bytes, set_param_by_index, set_param_element_by_index,
    set_param_element_normalized, set_param_element_plain_f64, set_param_normalized,
    set_param_plain_f64,
};
mod bindings;
pub use bindings::{bind_buffer, bind_input, bind_output, prepare_process};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrintValue {
    F32(f32),
    F64(f64),
    I32(i32),
    I64(i64),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedPrintOccurrence<'program> {
    pub site_index: u32,
    pub sequence: u32,
    pub site: &'program onda_mir::LogSite,
    pub values: Vec<PrintValue>,
}

impl std::fmt::Display for PrintValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::F32(value) => write_canonical_f32(formatter, *value),
            Self::F64(value) => write_canonical_f64(formatter, *value),
            Self::I32(value) => write!(formatter, "{value}"),
            Self::I64(value) => write!(formatter, "{value}"),
            Self::Bool(value) => write!(formatter, "{value}"),
        }
    }
}

pub fn canonical_f32(value: f32) -> String {
    let mut rendered = String::with_capacity(24);
    write_canonical_f32(&mut rendered, value).expect("writing to String cannot fail");
    rendered
}

pub fn canonical_f64(value: f64) -> String {
    let mut rendered = String::with_capacity(32);
    write_canonical_f64(&mut rendered, value).expect("writing to String cannot fail");
    rendered
}

fn write_canonical_f32(output: &mut impl fmt::Write, value: f32) -> fmt::Result {
    write_canonical_float(
        output,
        value,
        value as f64,
        value.is_nan(),
        value.is_infinite(),
        value.is_sign_negative(),
    )
}

fn write_canonical_f64(output: &mut impl fmt::Write, value: f64) -> fmt::Result {
    write_canonical_float(
        output,
        value,
        value,
        value.is_nan(),
        value.is_infinite(),
        value.is_sign_negative(),
    )
}

fn write_canonical_float(
    output: &mut impl fmt::Write,
    shortest_value: impl fmt::Display,
    value: f64,
    is_nan: bool,
    is_infinite: bool,
    negative: bool,
) -> fmt::Result {
    if is_nan {
        return output.write_str("NaN");
    }
    if is_infinite {
        return output.write_str(if negative { "-inf" } else { "inf" });
    }
    if value == 0.0 {
        return output.write_str(if negative { "-0.0" } else { "0.0" });
    }

    // Rust's shortest float display may use fixed notation for the smallest
    // subnormal f64, which needs 326 characters before canonical exponent
    // normalization. This bound covers every IEEE-754 f64 rendering.
    let mut shortest = StackText::<400>::new();
    write!(&mut shortest, "{shortest_value}")?;
    let unsigned = shortest
        .as_str()
        .strip_prefix('-')
        .unwrap_or(shortest.as_str());
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, 0_i32), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().unwrap_or(0))
        });
    let dot = mantissa.find('.').unwrap_or(mantissa.len());
    let mut digit_storage = [0_u8; 400];
    let mut digit_count = 0;
    for byte in mantissa.bytes().filter(|byte| *byte != b'.') {
        digit_storage[digit_count] = byte;
        digit_count += 1;
    }
    let leading = digit_storage[..digit_count]
        .iter()
        .take_while(|byte| **byte == b'0')
        .count();
    let mut digits = &digit_storage[leading..digit_count];
    let decimal_position = dot as i32 + exponent - leading as i32;
    let fixed = value.abs() >= 1.0e-6 && value.abs() < 1.0e21;
    if negative {
        output.write_char('-')?;
    }
    if fixed {
        if decimal_position <= 0 {
            output.write_str("0.")?;
            write_repeated(output, '0', (-decimal_position) as usize)?;
            output.write_str(std::str::from_utf8(digits).expect("float digits are UTF-8"))?;
        } else if decimal_position as usize >= digits.len() {
            output.write_str(std::str::from_utf8(digits).expect("float digits are UTF-8"))?;
            write_repeated(output, '0', decimal_position as usize - digits.len())?;
            output.write_str(".0")?;
        } else {
            let split = decimal_position as usize;
            output.write_str(
                std::str::from_utf8(&digits[..split]).expect("float digits are UTF-8"),
            )?;
            output.write_char('.')?;
            output.write_str(
                std::str::from_utf8(&digits[split..]).expect("float digits are UTF-8"),
            )?;
        }
    } else {
        while digits.len() > 1 && digits.ends_with(b"0") {
            digits = &digits[..digits.len() - 1];
        }
        if digits.len() == 1 {
            output.write_char(char::from(digits[0]))?;
        } else {
            output.write_char(char::from(digits[0]))?;
            output.write_char('.')?;
            output.write_str(std::str::from_utf8(&digits[1..]).expect("float digits are UTF-8"))?;
        }
        write!(output, "e{}", decimal_position - 1)?;
    }
    Ok(())
}

fn write_repeated(output: &mut impl fmt::Write, character: char, count: usize) -> fmt::Result {
    for _ in 0..count {
        output.write_char(character)?;
    }
    Ok(())
}

struct StackText<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> StackText<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).expect("fmt::Write receives valid UTF-8")
    }
}

impl<const N: usize> fmt::Write for StackText<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.len.checked_add(text.len()).ok_or(fmt::Error)?;
        let destination = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        destination.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

fn write_escaped_print_label(output: &mut impl fmt::Write, label: &str) -> fmt::Result {
    for character in label.chars() {
        match character {
            '\0' => output.write_str("\\0")?,
            '\\' => output.write_str("\\\\")?,
            '\n' => output.write_str("\\n")?,
            '\r' => output.write_str("\\r")?,
            '\t' => output.write_str("\\t")?,
            character if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') => {
                write!(output, "\\u{{{:x}}}", character as u32)?;
            }
            _ => output.write_char(character)?,
        }
    }
    Ok(())
}

pub fn format_print_batch(
    instance: &Instance,
    batch: &PrintBatch<'_>,
) -> Result<String, Diagnostic> {
    format_print_batch_for_program(&instance.program, batch)
}

pub fn format_print_batch_for_program(
    program: &JitProgram,
    batch: &PrintBatch<'_>,
) -> Result<String, Diagnostic> {
    let required = formatted_print_batch_len_for_program(program, batch)?;
    let mut text = String::with_capacity(required);
    write_print_batch_for_program(program, batch, &mut text)?;
    Ok(text)
}

/// Formats a print batch into caller-owned UTF-8 storage without allocating.
///
/// The returned length is the complete required byte count. If `output` is too
/// small, nothing is written. No trailing NUL is included or written.
pub fn format_print_batch_into(
    instance: &Instance,
    batch: &PrintBatch<'_>,
    output: &mut [u8],
) -> Result<usize, Diagnostic> {
    format_print_batch_into_for_program(&instance.program, batch, output)
}

pub fn format_print_batch_into_for_program(
    program: &JitProgram,
    batch: &PrintBatch<'_>,
    output: &mut [u8],
) -> Result<usize, Diagnostic> {
    let required = formatted_print_batch_len_for_program(program, batch)?;
    if output.len() < required {
        return Ok(required);
    }
    let mut writer = SliceText::new(&mut output[..required]);
    write_print_batch_for_program(program, batch, &mut writer)?;
    debug_assert_eq!(writer.len, required);
    Ok(required)
}

fn formatted_print_batch_len_for_program(
    program: &JitProgram,
    batch: &PrintBatch<'_>,
) -> Result<usize, Diagnostic> {
    let mut counter = CountingText::default();
    write_print_batch_for_program(program, batch, &mut counter)?;
    Ok(counter.len)
}

fn write_print_batch_for_program(
    program: &JitProgram,
    batch: &PrintBatch<'_>,
    output: &mut impl fmt::Write,
) -> Result<(), Diagnostic> {
    let write_error = || Diagnostic::runtime("failed to write formatted print output", 0, 0);
    for occurrence in batch.occurrences() {
        let occurrence = occurrence.map_err(batch_decode_diagnostic)?;
        let Some(site) = program.mir().log_sites.get(occurrence.site_index as usize) else {
            return Err(Diagnostic::runtime(
                format!(
                    "print record references unknown log site {}",
                    occurrence.site_index
                ),
                0,
                0,
            ));
        };
        if occurrence.payload.len() != site.payload_size as usize {
            return Err(Diagnostic::runtime(
                format!(
                    "print record for site {} has {} payload bytes; expected {}",
                    occurrence.site_index,
                    occurrence.payload.len(),
                    site.payload_size
                ),
                0,
                0,
            ));
        }
        if let Some(label) = &site.label {
            write_escaped_print_label(output, label).map_err(|_| write_error())?;
            if !site.argument_types.is_empty() {
                output.write_str(": ").map_err(|_| write_error())?;
            }
        }
        let mut cursor = 0_usize;
        for (index, scalar) in site.argument_types.iter().enumerate() {
            if index > 0 {
                output.write_char(' ').map_err(|_| write_error())?;
            }
            let value = decode_print_value(
                occurrence.site_index,
                occurrence.payload,
                &mut cursor,
                *scalar,
            )?;
            write!(output, "{value}").map_err(|_| write_error())?;
        }
        output.write_char('\n').map_err(|_| write_error())?;
    }
    Ok(())
}

fn batch_decode_diagnostic(error: BatchDecodeError) -> Diagnostic {
    Diagnostic::runtime(format!("invalid execution-output batch: {error}"), 0, 0)
}

fn decode_print_value(
    site_index: u32,
    payload: &[u8],
    cursor: &mut usize,
    scalar: onda_mir::ScalarType,
) -> Result<PrintValue, Diagnostic> {
    let value = match scalar {
        onda_mir::ScalarType::F32 => {
            let bytes = payload[*cursor..*cursor + 4].try_into().unwrap();
            *cursor += 4;
            PrintValue::F32(f32::from_ne_bytes(bytes))
        }
        onda_mir::ScalarType::F64 => {
            let bytes = payload[*cursor..*cursor + 8].try_into().unwrap();
            *cursor += 8;
            PrintValue::F64(f64::from_ne_bytes(bytes))
        }
        onda_mir::ScalarType::I32 => {
            let bytes = payload[*cursor..*cursor + 4].try_into().unwrap();
            *cursor += 4;
            PrintValue::I32(i32::from_ne_bytes(bytes))
        }
        onda_mir::ScalarType::I64 => {
            let bytes = payload[*cursor..*cursor + 8].try_into().unwrap();
            *cursor += 8;
            PrintValue::I64(i64::from_ne_bytes(bytes))
        }
        onda_mir::ScalarType::Bool => {
            let byte = payload[*cursor];
            *cursor += 1;
            if byte > 1 {
                return Err(Diagnostic::runtime(
                    format!("print record for site {site_index} contains invalid bool"),
                    0,
                    0,
                ));
            }
            PrintValue::Bool(byte != 0)
        }
    };
    Ok(value)
}

#[derive(Default)]
struct CountingText {
    len: usize,
}

impl fmt::Write for CountingText {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.len = self.len.checked_add(text.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}

struct SliceText<'a> {
    output: &'a mut [u8],
    len: usize,
}

impl<'a> SliceText<'a> {
    fn new(output: &'a mut [u8]) -> Self {
        Self { output, len: 0 }
    }
}

impl fmt::Write for SliceText<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.len.checked_add(text.len()).ok_or(fmt::Error)?;
        let destination = self.output.get_mut(self.len..end).ok_or(fmt::Error)?;
        destination.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

pub fn decode_print_batch<'program>(
    instance: &'program Instance,
    batch: &PrintBatch<'_>,
) -> Result<Vec<DecodedPrintOccurrence<'program>>, Diagnostic> {
    decode_print_batch_for_program(&instance.program, batch)
}

pub fn decode_print_batch_for_program<'program>(
    program: &'program JitProgram,
    batch: &PrintBatch<'_>,
) -> Result<Vec<DecodedPrintOccurrence<'program>>, Diagnostic> {
    let capacity = batch.used_storage().map_or(0, |storage| {
        (storage.len() / BATCH_RECORD_HEADER_SIZE).min(batch.record_count as usize)
    });
    let mut decoded = Vec::with_capacity(capacity);
    for occurrence in batch.occurrences() {
        let occurrence = occurrence.map_err(batch_decode_diagnostic)?;
        let Some(site) = program.mir().log_sites.get(occurrence.site_index as usize) else {
            return Err(Diagnostic::runtime(
                format!(
                    "print record references unknown log site {}",
                    occurrence.site_index
                ),
                0,
                0,
            ));
        };
        if occurrence.payload.len() != site.payload_size as usize {
            return Err(Diagnostic::runtime(
                format!(
                    "print record for site {} has {} payload bytes; expected {}",
                    occurrence.site_index,
                    occurrence.payload.len(),
                    site.payload_size
                ),
                0,
                0,
            ));
        }
        let mut cursor = 0_usize;
        let mut values = Vec::with_capacity(site.argument_types.len());
        for scalar in &site.argument_types {
            values.push(decode_print_value(
                occurrence.site_index,
                occurrence.payload,
                &mut cursor,
                *scalar,
            )?);
        }
        decoded.push(DecodedPrintOccurrence {
            site_index: occurrence.site_index,
            sequence: occurrence.sequence,
            site,
            values,
        });
    }
    Ok(decoded)
}

/// Formats validated, decoded print occurrences without revisiting their packed payloads.
pub fn format_decoded_print_occurrences(occurrences: &[DecodedPrintOccurrence<'_>]) -> String {
    let mut text = String::new();
    for occurrence in occurrences {
        if let Some(label) = &occurrence.site.label {
            write_escaped_print_label(&mut text, label)
                .expect("writing escaped print labels to a String cannot fail");
            if !occurrence.values.is_empty() {
                text.push_str(": ");
            }
        }
        for (index, value) in occurrence.values.iter().enumerate() {
            if index > 0 {
                text.push(' ');
            }
            write!(&mut text, "{value}").expect("writing print values to a String cannot fail");
        }
        text.push('\n');
    }
    text
}

#[derive(Debug)]
pub struct Instance {
    pub(crate) program: JitProgram,
    pub(crate) params: RuntimeBuffer<u8>,
    state: InstanceState,
    input_bound: RuntimeBuffer<bool>,
    output_bound: RuntimeBuffer<bool>,
    missing_inputs: usize,
    missing_outputs: usize,
    pub(crate) input_ptrs: RuntimeBuffer<*const u8>,
    pub(crate) output_ptrs: RuntimeBuffer<*mut u8>,
    pub(crate) buffer_ptrs: RuntimeBuffer<*mut u8>,
    pub(crate) buffer_frames: RuntimeBuffer<i32>,
    pub(crate) buffer_channels: RuntimeBuffer<i32>,
    pub(crate) buffer_sample_rates: RuntimeBuffer<f32>,
}

#[derive(Debug)]
enum InstanceState {
    Pending(UninitializedRuntimeState),
    Allocated(AllocatedState),
}

#[derive(Debug)]
struct AllocatedState {
    storage: RuntimeState,
    initialized: bool,
}

impl AllocatedState {
    fn attempt_status(
        &mut self,
        operation: impl FnOnce(&mut RuntimeState) -> Result<u32, RuntimeError>,
    ) -> Result<u32, RuntimeError> {
        // Generated entry points mutate the live image in place. Invalidate it
        // before entering generated code so errors and unwinding cannot leave
        // partially mutated state observable as ready.
        self.initialized = false;
        let result = operation(&mut self.storage);
        self.initialized = matches!(result, Ok(PROCESSOR_EXECUTION_OK));
        result
    }
}

fn uninitialized_instance_error() -> RuntimeError {
    RuntimeError::Uninitialized
}

fn invalid_instance_error() -> RuntimeError {
    RuntimeError::Invalidated
}

// SAFETY: Instance is an exclusive mutable runtime owner. Its raw pointers are non-owning host
// bindings and are never dereferenced without `&mut Instance`; their validity remains governed by
// the bind/prepare/process contract. Moving an instance does not move the bound host allocations.
// Custom allocator construction guarantees that its callbacks remain valid on whichever thread
// creates, grows the event workspace of, or destroys the instance. Realtime dispatch performs no
// instance allocation; explicit workspace reservation happens outside realtime execution.
unsafe impl Send for Instance {}

impl Instance {
    pub fn in_channels(&self) -> usize {
        self.program.required_in_channels()
    }

    pub fn out_channels(&self) -> usize {
        self.program.required_out_channels()
    }

    pub fn input_count(&self) -> usize {
        self.program.input_count()
    }

    pub fn output_count(&self) -> usize {
        self.program.output_count()
    }

    pub fn control_output_count(&self) -> usize {
        self.program.control_output_count()
    }

    pub fn param_count(&self) -> usize {
        self.program.param_count()
    }

    pub fn buffer_count(&self) -> usize {
        self.program.buffer_count()
    }

    pub fn buffer_array_count(&self) -> usize {
        self.program.buffer_arrays().len()
    }

    pub fn event_count(&self) -> usize {
        self.program.event_count()
    }

    pub fn delegate_count(&self) -> usize {
        self.program.delegate_count()
    }

    pub fn state_count(&self) -> usize {
        self.program.state_count()
    }

    pub fn input_name(&self, index: usize) -> Option<&str> {
        self.program.input_name(index)
    }

    pub fn output_name(&self, index: usize) -> Option<&str> {
        self.program.output_name(index)
    }

    pub fn control_output_name(&self, index: usize) -> Option<&str> {
        self.program.control_output_name(index)
    }

    pub fn param_name(&self, index: usize) -> Option<&str> {
        self.program.param_name(index)
    }

    pub fn param_domain(&self, index: usize) -> Option<ParamDomain<'_>> {
        self.program.param_domain(index)
    }

    pub fn buffer_name(&self, index: usize) -> Option<&str> {
        self.program.buffer_name(index)
    }

    pub fn buffer_array(&self, index: usize) -> Option<&onda_codegen_llvm::DeclaredBufferArray> {
        self.program.buffer_arrays().get(index)
    }

    pub fn event_name(&self, index: usize) -> Option<&str> {
        self.program.event_name(index)
    }

    pub fn delegate_name(&self, index: usize) -> Option<&str> {
        self.program.delegate_name(index)
    }

    pub fn delegate_descriptor(
        &self,
        index: usize,
    ) -> Option<&onda_codegen_llvm::DeclaredDelegate> {
        self.program.delegate_descriptor(index)
    }

    pub fn state_name(&self, index: usize) -> Option<&str> {
        self.program.state_name(index)
    }

    pub fn input_index(&self, name: &str) -> Option<usize> {
        self.program.input_index(name)
    }

    pub fn output_index(&self, name: &str) -> Option<usize> {
        self.program.output_index(name)
    }

    pub fn control_output_index(&self, name: &str) -> Option<usize> {
        self.program.control_output_index(name)
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.program.param_index(name)
    }

    pub fn buffer_index(&self, name: &str) -> Option<usize> {
        self.program.buffer_index(name)
    }

    pub fn event_index(&self, name: &str) -> Option<usize> {
        self.program.event_index(name)
    }

    pub fn delegate_index(&self, name: &str) -> Option<usize> {
        self.program.delegate_index(name)
    }

    pub fn state_type(&self, index: usize) -> Option<String> {
        self.program.state_type(index)
    }

    pub fn input_type(&self, index: usize) -> Option<String> {
        self.program.input_type(index)
    }

    pub fn output_type(&self, index: usize) -> Option<String> {
        self.program.output_type(index)
    }

    pub fn control_output_type(&self, index: usize) -> Option<String> {
        self.program.control_output_type(index)
    }

    pub fn param_type(&self, index: usize) -> Option<String> {
        self.program.param_type(index)
    }

    pub fn buffer_type(&self, index: usize) -> Option<String> {
        self.program.buffer_type(index)
    }

    pub fn input_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.input_type_bytes(index)
    }

    pub fn output_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.output_type_bytes(index)
    }

    pub fn control_output_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.control_output_type_bytes(index)
    }

    pub fn control_output_elem_type(&self, index: usize) -> Option<PrimitiveType> {
        self.program.control_output_elem_type(index)
    }

    pub fn control_output_array_len(&self, index: usize) -> Option<usize> {
        self.program.control_output_array_len(index)
    }

    pub fn control_output_slot_offset(&self, index: usize) -> Option<usize> {
        self.program.control_output_slot_offset(index)
    }

    pub fn control_output_byte_offset(&self, index: usize) -> Option<usize> {
        self.program.control_output_byte_offset(index)
    }

    pub fn param_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.param_type_bytes(index)
    }

    /// Increase prepared event capacity outside realtime execution, using the instance allocator.
    /// The default is sufficient for fixed payloads and at least 64 KiB for dynamic payloads.
    /// A larger request reallocates; smaller requests reuse the existing workspace, and allocation
    /// failure preserves it.
    pub fn reserve_event_workspace(&mut self, bytes: usize) -> Result<(), Diagnostic> {
        match &mut self.state {
            InstanceState::Pending(state) => state.reserve_event_workspace(bytes),
            InstanceState::Allocated(state) => state.storage.reserve_event_workspace(bytes),
        }
    }

    pub fn event_workspace_capacity(&self) -> usize {
        match &self.state {
            InstanceState::Pending(state) => state.event_workspace_capacity(),
            InstanceState::Allocated(state) => state.storage.event_workspace_capacity(),
        }
    }

    pub fn event_payload_bytes(&self, index: usize) -> Option<usize> {
        self.program.event_payload_bytes(index)
    }

    /// Returns the minimum payload size for an event. Each dynamic slice contributes its
    /// four-byte length prefix and no element bytes.
    pub fn event_payload_min_bytes(&self, index: usize) -> Option<usize> {
        self.program.event_payload_min_bytes(index)
    }

    /// Returns the exact payload size for a fixed-shape delegate, or `None` for a dynamic payload
    /// or invalid index. This excludes the packed record header.
    pub fn delegate_payload_bytes(&self, index: usize) -> Option<usize> {
        self.program.delegate_payload_bytes(index)
    }

    /// Returns the minimum payload size for a delegate, excluding the packed record header.
    pub fn delegate_payload_min_bytes(&self, index: usize) -> Option<usize> {
        self.program.delegate_payload_min_bytes(index)
    }

    /// Returns the exact complete record size for a fixed-shape delegate, or `None` for a dynamic
    /// payload or invalid index.
    pub fn delegate_record_bytes(&self, index: usize) -> Option<usize> {
        self.program.delegate_record_bytes(index)
    }

    /// Returns the minimum complete record size for a delegate, including its packed header.
    pub fn delegate_record_min_bytes(&self, index: usize) -> Option<usize> {
        self.program.delegate_record_min_bytes(index)
    }

    pub fn state_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.state_type_bytes(index)
    }

    pub fn state_size_bytes(&self) -> usize {
        self.program.state_size_bytes()
    }

    pub fn is_initialized(&self) -> bool {
        matches!(
            self.state,
            InstanceState::Allocated(AllocatedState {
                initialized: true,
                ..
            })
        )
    }

    fn initialized_state(&self) -> Result<&RuntimeState, RuntimeError> {
        match &self.state {
            InstanceState::Allocated(state) if state.initialized => Ok(&state.storage),
            InstanceState::Allocated(_) => Err(invalid_instance_error()),
            InstanceState::Pending(_) => Err(uninitialized_instance_error()),
        }
    }

    pub fn snapshot_state_bytes(&self) -> Result<Vec<u8>, RuntimeError> {
        let state = self.initialized_state()?;
        let mut snapshot = vec![0_u8; self.program.state_size_bytes()];
        self.program.write_state_snapshot(state, &mut snapshot)?;
        Ok(snapshot)
    }

    pub fn write_snapshot_state_bytes(&self, destination: &mut [u8]) -> Result<(), RuntimeError> {
        self.program
            .write_state_snapshot(self.initialized_state()?, destination)
    }

    /// Restores a state snapshot while preserving a generated initialization failure status.
    pub fn restore_state_bytes(&mut self, bytes: &[u8]) -> Result<u32, RuntimeError> {
        self.program.validate_state_snapshot(bytes)?;
        let was_pending = matches!(self.state, InstanceState::Pending(_));
        if was_pending {
            let status = init_checked(self, InitMode::Full, ExecutionOutput::none())?;
            if status != PROCESSOR_EXECUTION_OK {
                return Ok(status);
            }
        }
        let InstanceState::Allocated(state) = &mut self.state else {
            unreachable!("full initialization succeeded without initializing state")
        };
        state.attempt_status(|state| {
            if was_pending {
                self.program.overlay_state_snapshot(state, bytes)?;
                Ok(PROCESSOR_EXECUTION_OK)
            } else {
                // SAFETY: buffer bindings were validated above, and their
                // pointees remain valid under the instance binding contract.
                unsafe {
                    self.program.restore_state_snapshot(
                        &self.params,
                        state,
                        bytes,
                        BufferDescriptorTables::new(
                            &self.buffer_ptrs,
                            &self.buffer_frames,
                            &self.buffer_channels,
                            &self.buffer_sample_rates,
                        ),
                    )
                }
            }
        })
    }
}

/// Allocates storage and parameter defaults without executing authored initialization.
/// Bind memory and set parameters, then call [`init_checked`] with [`InitMode::Full`].
/// All DSP configuration is derived from the immutable compiled program.
pub fn create_instance(
    program: JitProgram,
    allocator: Option<RuntimeAllocator>,
) -> Result<Instance, Diagnostic> {
    u32::try_from(program.block_size()).map_err(|_| RuntimeError::SizeOverflow)?;
    let required_in_channels = program.required_in_channels();
    let required_out_channels = program.required_out_channels();

    let mut params = RuntimeBuffer::try_from_elem_in(program.param_byte_size(), 0_u8, allocator)?;
    program.write_default_param_bytes(&mut params)?;
    let state = InstanceState::Pending(program.allocate_state_with_allocator(allocator)?);

    let input_count = program.input_count();
    let output_count = program.output_count();
    let buffer_count = program.buffer_count();
    let mut buffer_ptrs =
        RuntimeBuffer::try_from_elem_in(buffer_count, std::ptr::null_mut(), allocator)?;
    let mut buffer_frames = RuntimeBuffer::try_from_elem_in(buffer_count, 1_i32, allocator)?;
    let mut buffer_channels = RuntimeBuffer::try_from_elem_in(buffer_count, 1_i32, allocator)?;
    let mut buffer_sample_rates =
        RuntimeBuffer::try_from_elem_in(buffer_count, program.sample_rate(), allocator)?;
    prepare_unbound_buffer_descriptors(
        program.buffers(),
        program.sample_rate(),
        &mut buffer_ptrs,
        &mut buffer_frames,
        &mut buffer_channels,
        &mut buffer_sample_rates,
    )?;

    Ok(Instance {
        program,
        params,
        state,
        input_bound: RuntimeBuffer::try_from_elem_in(input_count, false, allocator)?,
        output_bound: RuntimeBuffer::try_from_elem_in(output_count, false, allocator)?,
        missing_inputs: input_count,
        missing_outputs: output_count,
        input_ptrs: RuntimeBuffer::try_from_elem_in(
            required_in_channels,
            std::ptr::null(),
            allocator,
        )?,
        output_ptrs: RuntimeBuffer::try_from_elem_in(
            required_out_channels,
            std::ptr::null_mut(),
            allocator,
        )?,
        buffer_ptrs,
        buffer_frames,
        buffer_channels,
        buffer_sample_rates,
    })
}

/// Runs checked initialization while preserving the generated execution status.
pub fn init_checked(
    instance: &mut Instance,
    mode: InitMode,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    output.reset();
    with_processor_execution_output(output, |output| match (&mut instance.state, mode) {
        (InstanceState::Pending(state), InitMode::Full) => {
            // SAFETY: validated bindings retain their host-memory contract;
            // execution output borrows exclusive storage and was reset above.
            let result = unsafe {
                instance.program.initialize_allocated_state(
                    &instance.params,
                    state,
                    BufferDescriptorTables::new(
                        &instance.buffer_ptrs,
                        &instance.buffer_frames,
                        &instance.buffer_channels,
                        &instance.buffer_sample_rates,
                    ),
                    output,
                )
            }?;
            match result {
                onda_codegen_llvm::StateInitialization::Initialized(storage) => {
                    instance.state = InstanceState::Allocated(AllocatedState {
                        storage,
                        initialized: true,
                    });
                    Ok(PROCESSOR_EXECUTION_OK)
                }
                onda_codegen_llvm::StateInitialization::Failed(status) => Ok(status),
            }
        }
        (InstanceState::Pending(_), InitMode::PreservePinned) => {
            Err(uninitialized_instance_error())
        }
        (InstanceState::Allocated(state), InitMode::PreservePinned) if !state.initialized => {
            Err(invalid_instance_error())
        }
        (InstanceState::Allocated(state), mode) => state.attempt_status(|state| {
            // SAFETY: the same validated host bindings and exclusive, reset
            // output storage are used as for first initialization above.
            unsafe {
                instance.program.initialize_state_in_place_unchecked(
                    &instance.params,
                    state,
                    mode,
                    BufferDescriptorTables::new(
                        &instance.buffer_ptrs,
                        &instance.buffer_frames,
                        &instance.buffer_channels,
                        &instance.buffer_sample_rates,
                    ),
                    output,
                )
            }
        }),
    })
}

/// Processes exactly one compiled block, returning the generated execution status.
pub fn process_checked(
    instance: &mut Instance,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    output.reset();
    prepare_process(instance)?;
    unsafe {
        execute_process(
            instance,
            0,
            instance.program.block_size() as u32,
            PROCESSOR_FULL_BLOCK,
            output,
        )
    }
}

/// Processes a checked segment while preserving the generated execution status.
pub fn process_checked_segment(
    instance: &mut Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    output.reset();
    validate_process_segment(instance, start_frame, frames, flags)?;
    prepare_process(instance)?;
    unsafe { execute_process(instance, start_frame as u32, frames as u32, flags, output) }
}

/// Processes one complete block without revalidating host bindings and optionally collects
/// delegate and print occurrences.
///
/// # Safety
///
/// The instance must satisfy [`prepare_process`]: initialized state and complete audio bindings.
/// Successful rebinds preserve readiness; unbinding required audio or a generated failure revokes it.
/// Bound memory must retain its validated extent/alignment, lifetime and non-aliasing guarantees.
/// Violating this contract is undefined behavior.
pub unsafe fn process_unchecked(
    instance: &mut Instance,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    output.reset();
    debug_assert!(prepare_process(instance).is_ok());
    unsafe {
        execute_process(
            instance,
            0,
            instance.program.block_size() as u32,
            PROCESSOR_FULL_BLOCK,
            output,
        )
    }
}

/// Processes a segment of the configured block without revalidating host bindings and optionally
/// collects delegate and print occurrences.
///
/// A nonzero generated execution status invalidates the instance. Full initialization is then
/// required before any further processing, event dispatch, or task execution.
///
/// # Safety
///
/// The instance must satisfy [`prepare_process`]: initialized state and complete audio bindings.
/// Successful rebinds preserve readiness; unbinding required audio or a generated failure revokes it.
/// Bound memory must retain its validated extent/alignment, lifetime and non-aliasing guarantees.
/// Violating this contract is undefined behavior.
pub unsafe fn process_unchecked_segment(
    instance: &mut Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    output.reset();
    validate_process_segment(instance, start_frame, frames, flags)?;
    debug_assert!(prepare_process(instance).is_ok());
    unsafe { execute_process(instance, start_frame as u32, frames as u32, flags, output) }
}

#[inline]
unsafe fn execute_process(
    instance: &mut Instance,
    start_frame: u32,
    frames: u32,
    flags: u32,
    output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    let state = match &mut instance.state {
        InstanceState::Allocated(state) if state.initialized => state,
        InstanceState::Allocated(_) | InstanceState::Pending(_) => unsafe {
            std::hint::unreachable_unchecked()
        },
    };
    let status = with_processor_execution_output(output, |output| unsafe {
        instance.program.process_unchecked(
            &mut state.storage,
            &instance.params,
            start_frame,
            frames,
            flags,
            &instance.input_ptrs,
            &instance.output_ptrs,
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
            output,
        )
    })?;
    if status != PROCESSOR_EXECUTION_OK {
        state.initialized = false;
    }
    Ok(status)
}

/// Checks a segment's frame bounds and flags without executing the processor.
/// Hosts can preflight a complete schedule before applying block-level updates.
pub fn validate_process_segment(
    instance: &Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
) -> Result<(), RuntimeError> {
    let block_size = instance.program.block_size();
    if start_frame > block_size || frames > block_size - start_frame {
        return Err(RuntimeError::InvalidSegment {
            start: start_frame,
            frames,
            block_size,
        });
    }
    if flags & !PROCESSOR_FULL_BLOCK != 0 {
        return Err(RuntimeError::InvalidFlags { flags });
    }
    Ok(())
}

fn event_status_invalidates_state(status: u32) -> bool {
    status != PROCESSOR_EXECUTION_OK && status != PROCESSOR_EXECUTION_INPUT_REJECTED
}

pub fn trigger_event_by_index_checked(
    instance: &mut Instance,
    event_index: usize,
    payload: &[u8],
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    let state = match &mut instance.state {
        InstanceState::Allocated(state) if state.initialized => state,
        InstanceState::Allocated(_) => {
            output.reset();
            return Err(invalid_instance_error());
        }
        InstanceState::Pending(_) => {
            output.reset();
            return Err(uninitialized_instance_error());
        }
    };
    // The generated entry resets output before owning payload preflight and
    // normalization. Rejection leaves workspace and processor state untouched.
    // Keep that status separate so only a handler failure closes the instance,
    // matching the process lifecycle.
    let status = with_processor_execution_output(output, |output| unsafe {
        instance.program.trigger_event_by_index_unchecked(
            &mut state.storage,
            &instance.params,
            event_index,
            payload,
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
            output,
        )
    })?;
    if event_status_invalidates_state(status) {
        state.initialized = false;
    }
    Ok(status)
}

/// Dispatches an event from checked native tensor views while preserving the
/// generated execution status.
///
/// # Safety
///
/// Every nonempty view must describe live readable storage for the duration of
/// the synchronous call. That storage must not be mutated concurrently or
/// overlap memory written by the event.
pub unsafe fn trigger_event_views_by_index_checked(
    instance: &mut Instance,
    event_index: usize,
    views: &[EventTensorView],
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    let state = match &mut instance.state {
        InstanceState::Allocated(state) if state.initialized => state,
        InstanceState::Allocated(_) => {
            output.reset();
            return Err(invalid_instance_error());
        }
        InstanceState::Pending(_) => {
            output.reset();
            return Err(uninitialized_instance_error());
        }
    };
    if !unsafe {
        instance
            .program
            .validate_event_tensor_views(event_index, views)
    } {
        output.reset();
        return Ok(PROCESSOR_EXECUTION_INPUT_REJECTED);
    }
    let status = with_processor_execution_output(output, |output| unsafe {
        instance.program.trigger_event_views_by_index_unchecked(
            &mut state.storage,
            &instance.params,
            event_index,
            views.as_ptr(),
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
            output,
        )
    })?;
    if event_status_invalidates_state(status) {
        state.initialized = false;
    }
    Ok(status)
}

/// Dispatches an event from canonical native tensor views without copying or
/// validating instance storage, buffer descriptors, or the tensor views.
///
/// Generated execution failure invalidates the instance. Full initialization
/// is required after execution failure.
///
/// # Safety
///
/// Buffer bindings must have been validated after their most recent mutation.
/// The instance must be fully initialized. `views` must point to the exact
/// schema-derived number of views, each containing a contiguous, naturally
/// aligned native SoA tensor with the required shape and canonical values.
pub unsafe fn trigger_event_views_by_index_unchecked(
    instance: &mut Instance,
    event_index: usize,
    views: *const EventTensorView,
    output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    debug_assert!(
        instance.is_initialized(),
        "trigger_event_views_by_index_unchecked called before full initialization; this is UB in release builds"
    );
    let state = match &mut instance.state {
        InstanceState::Allocated(state) if state.initialized => state,
        InstanceState::Allocated(_) | InstanceState::Pending(_) => unsafe {
            std::hint::unreachable_unchecked()
        },
    };
    let status = with_processor_execution_output(output, |output| unsafe {
        instance.program.trigger_event_views_by_index_unchecked(
            &mut state.storage,
            &instance.params,
            event_index,
            views,
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
            output,
        )
    })?;
    if event_status_invalidates_state(status) {
        state.initialized = false;
    }
    Ok(status)
}

/// Dispatches an event without hosted payload or current-buffer validation, optionally collecting
/// delegate and print occurrences. The generated entry still performs mandatory payload preflight.
///
/// Generated execution failure invalidates the instance. Rejected input (status 2), including
/// insufficient workspace capacity, preserves it. Full initialization is required after execution
/// failure.
///
/// # Safety
///
/// Buffer bindings must have been validated after their most recent mutation and must remain valid
/// for the call. The instance must have completed full initialization; violating that lifecycle
/// contract is undefined behavior in release builds. Malformed payload bytes are safely rejected by
/// the generated entry.
pub unsafe fn trigger_event_by_index_unchecked(
    instance: &mut Instance,
    event_index: usize,
    payload: &[u8],
    output: ExecutionOutput<'_, '_>,
) -> Result<u32, RuntimeError> {
    debug_assert!(
        instance.is_initialized(),
        "trigger_event_by_index_unchecked called before full initialization; this is UB in release builds"
    );
    let state = match &mut instance.state {
        InstanceState::Allocated(state) if state.initialized => state,
        InstanceState::Allocated(_) | InstanceState::Pending(_) => unsafe {
            std::hint::unreachable_unchecked()
        },
    };
    let status = with_processor_execution_output(output, |output| {
        instance.program.trigger_event_by_index_unchecked(
            &mut state.storage,
            &instance.params,
            event_index,
            payload,
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
            output,
        )
    })?;
    if event_status_invalidates_state(status) {
        state.initialized = false;
    }
    Ok(status)
}

#[allow(clippy::too_many_arguments)]
fn prepare_unbound_buffer_descriptors(
    buffers: &[onda_codegen_llvm::DeclaredBuffer],
    sample_rate: f32,
    pointers: &mut [*mut u8],
    frames: &mut [i32],
    channels: &mut [i32],
    sample_rates: &mut [f32],
) -> Result<(), Diagnostic> {
    for (index, buffer) in buffers.iter().enumerate() {
        pointers[index] = std::ptr::null_mut();
        frames[index] = 1;
        channels[index] = fallback_channels(buffer)?;
        sample_rates[index] = sample_rate;
    }
    Ok(())
}

fn fallback_channels(buffer: &onda_codegen_llvm::DeclaredBuffer) -> Result<i32, Diagnostic> {
    let channels = match buffer.channels() {
        DeclaredBufferChannels::Mono => 1,
        DeclaredBufferChannels::Static(channels) => channels,
        DeclaredBufferChannels::Dynamic => 1,
    };
    i32::try_from(channels).map_err(|_| {
        Diagnostic::runtime(
            format!("buffer '{}' channel count does not fit i32", buffer.name()),
            0,
            0,
        )
    })
}

fn primitive_type_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 | PrimitiveType::Enum(_) => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn primitive_type_alignment(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 => std::mem::align_of::<f32>(),
        PrimitiveType::F64 => std::mem::align_of::<f64>(),
        PrimitiveType::I32 | PrimitiveType::Enum(_) => std::mem::align_of::<i32>(),
        PrimitiveType::I64 => std::mem::align_of::<i64>(),
        PrimitiveType::Bool => std::mem::align_of::<u8>(),
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
