use onda_codegen_llvm::{
    BufferDescriptorTables, DeclaredBufferChannels, JitProgram, RuntimeAllocator, RuntimeBuffer,
    RuntimeState, UninitializedRuntimeState,
};
use onda_frontend::{Diagnostic, PrimitiveType};
use onda_realtime::configure_current_thread_audio_fp_mode;
use std::fmt::{self, Write as _};
use std::marker::PhantomData;

pub use onda_codegen_llvm::{ParamDomain, ParamScalarType, ParamScale};

/// Bytes occupied by the delegate index, payload length, and sequence header of each occurrence.
pub const DELEGATE_RECORD_HEADER_SIZE: usize = 12;

/// A non-owning decoded view into one occurrence in a [`DelegateBatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegateOccurrence<'batch> {
    pub delegate_index: u32,
    pub sequence: u32,
    pub payload: &'batch [u8],
}

/// Allocation-free iterator over the complete records in a [`DelegateBatch`].
#[derive(Debug, Clone)]
pub struct DelegateOccurrences<'batch> {
    storage: &'batch [u8],
    cursor: usize,
    remaining: u32,
}

impl<'batch> Iterator for DelegateOccurrences<'batch> {
    type Item = DelegateOccurrence<'batch>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let header_end = self.cursor.checked_add(DELEGATE_RECORD_HEADER_SIZE)?;
        let header = self.storage.get(self.cursor..header_end)?;
        let delegate_index = u32::from_ne_bytes(header[..4].try_into().ok()?);
        let payload_bytes = u32::from_ne_bytes(header[4..8].try_into().ok()?) as usize;
        let sequence = u32::from_ne_bytes(header[8..12].try_into().ok()?);
        let record_end = header_end.checked_add(payload_bytes)?;
        let payload = self.storage.get(header_end..record_end)?;
        self.cursor = record_end;
        self.remaining -= 1;
        Some(DelegateOccurrence {
            delegate_index,
            sequence,
            payload,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining as usize;
        (0, Some(remaining))
    }
}

impl std::iter::FusedIterator for DelegateOccurrences<'_> {}

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

pub const PRINT_RECORD_HEADER_SIZE: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintOccurrence<'batch> {
    pub site_index: u32,
    pub sequence: u32,
    pub payload: &'batch [u8],
}

#[derive(Debug, Clone)]
pub struct PrintOccurrences<'batch> {
    storage: &'batch [u8],
    cursor: usize,
    remaining: u32,
}

impl<'batch> Iterator for PrintOccurrences<'batch> {
    type Item = PrintOccurrence<'batch>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let header_end = self.cursor.checked_add(PRINT_RECORD_HEADER_SIZE)?;
        let header = self.storage.get(self.cursor..header_end)?;
        let site_index = u32::from_ne_bytes(header[..4].try_into().ok()?);
        let payload_bytes = u32::from_ne_bytes(header[4..8].try_into().ok()?) as usize;
        let sequence = u32::from_ne_bytes(header[8..12].try_into().ok()?);
        let record_end = header_end.checked_add(payload_bytes)?;
        let payload = self.storage.get(header_end..record_end)?;
        self.cursor = record_end;
        self.remaining -= 1;
        Some(PrintOccurrence {
            site_index,
            sequence,
            payload,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.remaining as usize))
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

    /// # Safety
    /// A non-null pointer must remain exclusively writable for `capacity_bytes`
    /// bytes throughout `'storage`.
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

    pub fn occurrence(&self, index: u32) -> Option<PrintOccurrence<'_>> {
        self.occurrences().nth(index as usize)
    }

    pub fn occurrences(&self) -> PrintOccurrences<'_> {
        PrintOccurrences {
            storage: self.used_storage().unwrap_or(&[]),
            cursor: 0,
            remaining: self.record_count,
        }
    }

    pub fn reset(&mut self) {
        self.used_bytes = 0;
        self.record_count = 0;
        self.overflow_count = 0;
    }

    fn used_storage(&self) -> Option<&[u8]> {
        if self.used_bytes > self.capacity_bytes {
            return None;
        }
        if self.used_bytes == 0 {
            return Some(&[]);
        }
        if self.storage.is_null() {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(self.storage, self.used_bytes as usize) })
    }

    fn has_valid_record_layout(&self) -> bool {
        let Some(storage) = self.used_storage() else {
            return false;
        };
        let mut cursor = 0_usize;
        for _ in 0..self.record_count {
            let Some(header_end) = cursor.checked_add(PRINT_RECORD_HEADER_SIZE) else {
                return false;
            };
            let Some(header) = storage.get(cursor..header_end) else {
                return false;
            };
            let payload_bytes = u32::from_ne_bytes(
                header[4..8]
                    .try_into()
                    .expect("print record header has a four-byte payload size"),
            ) as usize;
            let Some(record_end) = header_end.checked_add(payload_bytes) else {
                return false;
            };
            if record_end > storage.len() {
                return false;
            }
            cursor = record_end;
        }
        cursor == storage.len()
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

    /// Returns one decoded occurrence by zero-based record index.
    pub fn occurrence(&self, index: u32) -> Option<DelegateOccurrence<'_>> {
        self.occurrences().nth(index as usize)
    }

    /// Iterates the complete occurrences produced by the most recent successful call.
    ///
    /// The iterator is allocation-free and borrows the batch, preventing its storage from being
    /// reused while occurrence payload views remain live. A malformed descriptor stops iteration;
    /// batches returned by successful generated execution satisfy the record contract.
    pub fn occurrences(&self) -> DelegateOccurrences<'_> {
        let storage = self.used_storage().unwrap_or(&[]);
        DelegateOccurrences {
            storage,
            cursor: 0,
            remaining: self.record_count,
        }
    }

    /// Clears result counters without modifying the storage or capacity.
    pub fn reset(&mut self) {
        self.used_bytes = 0;
        self.record_count = 0;
        self.overflow_count = 0;
    }

    fn used_storage(&self) -> Option<&[u8]> {
        if self.used_bytes > self.capacity_bytes {
            return None;
        }
        if self.used_bytes == 0 {
            return Some(&[]);
        }
        if self.storage.is_null() {
            return None;
        }
        // SAFETY: constructors require the caller to keep this region valid for `'storage`, and
        // `used_bytes <= capacity_bytes` was checked above. The returned borrow is tied to `self`.
        Some(unsafe { std::slice::from_raw_parts(self.storage, self.used_bytes as usize) })
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
    if !batch.has_valid_record_layout() {
        return Err(Diagnostic::runtime(
            "print batch contains malformed, truncated, or trailing record bytes",
            0,
            0,
        ));
    }
    let write_error = || Diagnostic::runtime("failed to write formatted print output", 0, 0);
    let mut occurrence_count = 0_u32;
    for occurrence in batch.occurrences() {
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
        occurrence_count += 1;
    }
    if occurrence_count != batch.record_count {
        return Err(Diagnostic::runtime(
            "print batch contains malformed or truncated records",
            0,
            0,
        ));
    }
    Ok(())
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
    if !batch.has_valid_record_layout() {
        return Err(Diagnostic::runtime(
            "print batch contains malformed, truncated, or trailing record bytes",
            0,
            0,
        ));
    }
    let mut decoded = Vec::with_capacity(batch.record_count as usize);
    for occurrence in batch.occurrences() {
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
    if decoded.len() != batch.record_count as usize {
        return Err(Diagnostic::runtime(
            "print batch contains malformed or truncated records",
            0,
            0,
        ));
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

pub const PROCESS_BEGIN_BLOCK: u32 = 1 << 0;
pub const PROCESS_END_BLOCK: u32 = 1 << 1;
pub const PROCESS_FULL_BLOCK: u32 = PROCESS_BEGIN_BLOCK | PROCESS_END_BLOCK;

#[derive(Debug, Clone, Copy)]
pub struct InstanceConfig {
    pub sample_rate: f32,
    pub frames_per_block: usize,
    pub in_channels: usize,
    pub out_channels: usize,
}

#[derive(Debug)]
pub struct Instance {
    pub(crate) program: JitProgram,
    pub(crate) config: InstanceConfig,
    pub(crate) params: RuntimeBuffer<u8>,
    state: InstanceState,
    pub(crate) input_bindings: RuntimeBuffer<Option<BoundInput>>,
    pub(crate) output_bindings: RuntimeBuffer<Option<BoundOutput>>,
    pub(crate) buffer_bindings: RuntimeBuffer<Option<BoundBuffer>>,
    pub(crate) input_ptrs: RuntimeBuffer<*const u8>,
    pub(crate) output_ptrs: RuntimeBuffer<*mut u8>,
    pub(crate) buffer_ptrs: RuntimeBuffer<*mut u8>,
    pub(crate) buffer_frames: RuntimeBuffer<i32>,
    pub(crate) buffer_channels: RuntimeBuffer<i32>,
    pub(crate) buffer_sample_rates: RuntimeBuffer<f32>,
    pub(crate) inputs_validated: bool,
    pub(crate) outputs_validated: bool,
    pub(crate) buffers_validated: bool,
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
    fn attempt(
        &mut self,
        operation: impl FnOnce(&mut RuntimeState) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        // Generated entry points mutate the live image in place. Invalidate it
        // before entering generated code so errors and unwinding cannot leave
        // partially mutated state observable as ready.
        self.initialized = false;
        let result = operation(&mut self.storage);
        self.initialized = result.is_ok();
        result
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum InitMode {
    /// Rerun ordinary initializers while retaining pinned roots and task continuations.
    PreservePinned,
    /// Initialize the complete state image, including pinned roots and task continuations.
    Full,
}

fn uninitialized_instance_error() -> Diagnostic {
    Diagnostic::runtime(
        "instance requires full initialization before this operation",
        0,
        0,
    )
}

fn invalid_instance_error() -> Diagnostic {
    Diagnostic::runtime(
        "instance state is invalid after failed execution; run full initialization or restore state before this operation",
        0,
        0,
    )
}

// SAFETY: Instance is an exclusive mutable runtime owner. Its raw pointers are non-owning host
// bindings and are never dereferenced without `&mut Instance`; their validity remains governed by
// the bind/prepare/process contract. Moving an instance does not move the bound host allocations.
// Custom allocator construction guarantees that its free callback remains valid on whichever
// thread eventually destroys the instance. Onda performs no instance allocation after creation.
unsafe impl Send for Instance {}

#[derive(Debug, Clone, Copy)]
pub struct BoundInput {
    ptr: *const u8,
    bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct BoundOutput {
    ptr: *mut u8,
    bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct BoundBuffer {
    ptr: *mut u8,
    frames_i32: i32,
    channels_i32: i32,
    sample_rate_hz: f32,
}

impl Instance {
    pub fn in_channels(&self) -> usize {
        self.config.in_channels
    }

    pub fn out_channels(&self) -> usize {
        self.config.out_channels
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

    pub fn event_payload_bytes(&self, index: usize) -> Option<usize> {
        self.program.event_payload_bytes(index)
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

    fn initialized_state(&self) -> Result<&RuntimeState, Diagnostic> {
        match &self.state {
            InstanceState::Allocated(state) if state.initialized => Ok(&state.storage),
            InstanceState::Allocated(_) => Err(invalid_instance_error()),
            InstanceState::Pending(_) => Err(uninitialized_instance_error()),
        }
    }

    pub fn snapshot_state_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        let state = self.initialized_state()?;
        let mut snapshot = vec![0_u8; self.program.state_size_bytes()];
        self.program.write_state_snapshot(state, &mut snapshot)?;
        Ok(snapshot)
    }

    pub fn write_snapshot_state_bytes(&self, destination: &mut [u8]) -> Result<(), Diagnostic> {
        self.program
            .write_state_snapshot(self.initialized_state()?, destination)
    }

    pub fn restore_state_bytes(&mut self, bytes: &[u8]) -> Result<(), Diagnostic> {
        configure_current_thread_audio_fp_mode();
        self.program.validate_state_snapshot(bytes)?;
        if !self.buffers_validated {
            validate_buffers(self)?;
        }
        let was_pending = matches!(self.state, InstanceState::Pending(_));
        if was_pending {
            init(self, InitMode::Full)?;
        }
        let InstanceState::Allocated(state) = &mut self.state else {
            unreachable!("full initialization succeeded without initializing state")
        };
        state.attempt(|state| {
            if was_pending {
                self.program.overlay_state_snapshot(state, bytes)
            } else {
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
        })
    }
}

/// Allocates an instance and writes its parameter defaults without running Onda initialization.
/// Call [`init`] with [`InitMode::Full`] before using any stateful operation.
pub fn create_instance(
    program: JitProgram,
    config: InstanceConfig,
) -> Result<Instance, Diagnostic> {
    create_instance_inner(program, config, None)
}

/// Allocator-backed equivalent of [`create_instance`].
pub fn create_instance_with_allocator(
    program: JitProgram,
    config: InstanceConfig,
    allocator: RuntimeAllocator,
) -> Result<Instance, Diagnostic> {
    create_instance_inner(program, config, Some(allocator))
}

/// Creates an instance and performs [`InitMode::Full`] initialization.
pub fn create_instance_initialized(
    program: JitProgram,
    config: InstanceConfig,
) -> Result<Instance, Diagnostic> {
    create_instance_initialized_with_output(program, config, ExecutionOutput::none())
}

/// Creates an instance, performs [`InitMode::Full`] initialization, and collects successful
/// initialization output.
///
/// If initialization fails, both output batches are cleared and the diagnostic is the only result.
/// Create an uninitialized instance and call [`init_with_output`] to retain prints emitted before a
/// failing initializer.
pub fn create_instance_initialized_with_output(
    program: JitProgram,
    config: InstanceConfig,
    output: ExecutionOutput<'_, '_>,
) -> Result<Instance, Diagnostic> {
    initialize_new_instance(create_instance(program, config), output)
}

/// Allocator-backed equivalent of [`create_instance_initialized`].
pub fn create_instance_initialized_with_allocator(
    program: JitProgram,
    config: InstanceConfig,
    allocator: RuntimeAllocator,
) -> Result<Instance, Diagnostic> {
    create_instance_initialized_with_allocator_and_output(
        program,
        config,
        allocator,
        ExecutionOutput::none(),
    )
}

/// Allocator-backed equivalent of [`create_instance_initialized_with_output`], including its
/// failure-output behavior.
pub fn create_instance_initialized_with_allocator_and_output(
    program: JitProgram,
    config: InstanceConfig,
    allocator: RuntimeAllocator,
    output: ExecutionOutput<'_, '_>,
) -> Result<Instance, Diagnostic> {
    initialize_new_instance(
        create_instance_with_allocator(program, config, allocator),
        output,
    )
}

fn initialize_new_instance(
    instance: Result<Instance, Diagnostic>,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<Instance, Diagnostic> {
    let mut instance = match instance {
        Ok(instance) => instance,
        Err(error) => {
            output.reset();
            return Err(error);
        }
    };
    let result = init_with_output(
        &mut instance,
        InitMode::Full,
        ExecutionOutput {
            delegate_batch: output.delegate_batch.as_deref_mut(),
            print_batch: output.print_batch.as_deref_mut(),
        },
    );
    if let Err(error) = result {
        output.reset();
        return Err(error);
    }
    Ok(instance)
}

fn create_instance_inner(
    program: JitProgram,
    config: InstanceConfig,
    allocator: Option<RuntimeAllocator>,
) -> Result<Instance, Diagnostic> {
    if !config.sample_rate.is_finite() || config.sample_rate <= 0.0 {
        return Err(Diagnostic::runtime(
            "instance sample_rate must be finite and greater than zero",
            0,
            0,
        ));
    }
    if config.sample_rate.to_bits() != program.sample_rate().to_bits() {
        return Err(Diagnostic::runtime(
            format!(
                "instance sample_rate ({}) must match program compile-time sample rate ({})",
                config.sample_rate,
                program.sample_rate(),
            ),
            0,
            0,
        ));
    }
    if config.frames_per_block == 0 {
        return Err(Diagnostic::runtime(
            "frames_per_block must be greater than zero",
            0,
            0,
        ));
    }
    if config.out_channels == 0 && program.required_out_channels() > 0 {
        return Err(Diagnostic::runtime(
            "out_channels must be greater than zero when the program has audio outputs",
            0,
            0,
        ));
    }
    if config.in_channels < program.required_in_channels() {
        return Err(Diagnostic::runtime(
            "configured input channels are fewer than program inputs",
            0,
            0,
        ));
    }
    if config.out_channels < program.required_out_channels() {
        return Err(Diagnostic::runtime(
            "configured output channels are fewer than program outputs",
            0,
            0,
        ));
    }

    if config.frames_per_block != program.block_size() {
        return Err(Diagnostic::runtime(
            format!(
                "instance frames_per_block ({}) must match program compile-time block size ({})",
                config.frames_per_block,
                program.block_size()
            ),
            0,
            0,
        ));
    }
    u32::try_from(config.frames_per_block).map_err(|_| {
        Diagnostic::runtime(
            "frames_per_block does not fit u32 runtime/JIT process entrypoint",
            0,
            0,
        )
    })?;

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
        RuntimeBuffer::try_from_elem_in(buffer_count, config.sample_rate, allocator)?;
    prepare_unbound_buffer_descriptors(
        program.buffers(),
        config.sample_rate,
        &mut buffer_ptrs,
        &mut buffer_frames,
        &mut buffer_channels,
        &mut buffer_sample_rates,
    )?;

    Ok(Instance {
        program,
        config,
        params,
        state,
        input_bindings: RuntimeBuffer::try_from_elem_in(input_count, None, allocator)?,
        output_bindings: RuntimeBuffer::try_from_elem_in(output_count, None, allocator)?,
        buffer_bindings: RuntimeBuffer::try_from_elem_in(buffer_count, None, allocator)?,
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
        inputs_validated: required_in_channels == 0,
        outputs_validated: required_out_channels == 0,
        buffers_validated: true,
    })
}

/// Runs the program initializer using the requested state-retention mode.
/// Full initialization is required before any stateful instance operation.
/// A failed live initialization invalidates the state image until a later full
/// initialization or snapshot restore succeeds.
pub fn init(instance: &mut Instance, mode: InitMode) -> Result<(), Diagnostic> {
    init_with_output(instance, mode, ExecutionOutput::none())
}

pub fn init_with_output(
    instance: &mut Instance,
    mode: InitMode,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<(), Diagnostic> {
    configure_current_thread_audio_fp_mode();
    output.reset();
    if !instance.buffers_validated {
        validate_buffers(instance)?;
    }
    with_processor_execution_output(output, |output| match (&mut instance.state, mode) {
        (InstanceState::Pending(state), InitMode::Full) => {
            let initialized = instance.program.initialize_allocated_state(
                &instance.params,
                state,
                BufferDescriptorTables::new(
                    &instance.buffer_ptrs,
                    &instance.buffer_frames,
                    &instance.buffer_channels,
                    &instance.buffer_sample_rates,
                ),
                output,
            )?;
            instance.state = InstanceState::Allocated(AllocatedState {
                storage: initialized,
                initialized: true,
            });
            Ok(())
        }
        (InstanceState::Pending(_), InitMode::PreservePinned) => {
            Err(uninitialized_instance_error())
        }
        (InstanceState::Allocated(state), InitMode::PreservePinned) if !state.initialized => {
            Err(invalid_instance_error())
        }
        (InstanceState::Allocated(state), mode) => state.attempt(|state| {
            instance.program.initialize_state_in_place(
                &instance.params,
                state,
                matches!(mode, InitMode::Full),
                BufferDescriptorTables::new(
                    &instance.buffer_ptrs,
                    &instance.buffer_frames,
                    &instance.buffer_channels,
                    &instance.buffer_sample_rates,
                ),
                output,
            )
        }),
    })
}

pub fn set_param_by_index(
    instance: &mut Instance,
    index: usize,
    value_bytes: &[u8],
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.param_descriptor(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown parameter index {index}"),
            0,
            0,
        ));
    };
    let expected_bytes = desc.byte_size();
    if value_bytes.len() != expected_bytes {
        return Err(Diagnostic::runtime(
            format!(
                "parameter '{}' expects {} bytes, got {}",
                desc.name(),
                expected_bytes,
                value_bytes.len()
            ),
            0,
            0,
        ));
    }
    let start = desc.byte_offset();
    let end = start.saturating_add(expected_bytes);
    if end > instance.params.len() {
        return Err(Diagnostic::runtime(
            format!(
                "parameter '{}' byte range [{start}, {end}) is out of bounds for runtime storage ({})",
                desc.name(),
                instance.params.len()
            ),
            0,
            0,
        ));
    }
    instance.params[start..end].copy_from_slice(value_bytes);
    Ok(())
}

pub fn set_param_plain_f64(
    instance: &mut Instance,
    index: usize,
    plain: f64,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.param_descriptor(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown parameter index {index}"),
            0,
            0,
        ));
    };
    if desc.is_array() {
        return Err(Diagnostic::runtime(
            format!("parameter '{}' is not a scalar", desc.name()),
            0,
            0,
        ));
    }
    let value = match desc.elem_ty() {
        PrimitiveType::Bool => {
            return set_param_by_index(instance, index, &[u8::from(plain >= 0.5)]);
        }
        _ => desc
            .param_domain()
            .map(|domain| domain.constrain_plain(plain))
            .ok_or_else(|| {
                Diagnostic::runtime(
                    format!("parameter '{}' has no numeric control domain", desc.name()),
                    0,
                    0,
                )
            })?,
    };
    set_scalar_param_f64(instance, index, desc.elem_ty(), value)
}

fn set_scalar_param_f64(
    instance: &mut Instance,
    index: usize,
    ty: PrimitiveType,
    value: f64,
) -> Result<(), Diagnostic> {
    let mut bytes = [0_u8; 8];
    let len = match ty {
        PrimitiveType::F32 => {
            bytes[..4].copy_from_slice(&(value as f32).to_ne_bytes());
            4
        }
        PrimitiveType::F64 => {
            bytes.copy_from_slice(&value.to_ne_bytes());
            8
        }
        PrimitiveType::I32 => {
            bytes[..4].copy_from_slice(&(value.round() as i32).to_ne_bytes());
            4
        }
        PrimitiveType::I64 => {
            bytes.copy_from_slice(&(value.round() as i64).to_ne_bytes());
            8
        }
        PrimitiveType::Bool => unreachable!(),
    };
    set_param_by_index(instance, index, &bytes[..len])
}

pub fn set_param_normalized(
    instance: &mut Instance,
    index: usize,
    normalized: f64,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.param_descriptor(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown parameter index {index}"),
            0,
            0,
        ));
    };
    if desc.elem_ty() == PrimitiveType::Bool && !desc.is_array() {
        return set_param_by_index(instance, index, &[u8::from(normalized >= 0.5)]);
    }
    let plain = desc
        .param_domain()
        .map(|domain| domain.normalized_to_plain(normalized))
        .ok_or_else(|| {
            Diagnostic::runtime(
                format!("parameter '{}' has no numeric control domain", desc.name()),
                0,
                0,
            )
        })?;
    set_scalar_param_f64(instance, index, desc.elem_ty(), plain)
}

pub fn read_control_output_bytes(
    instance: &Instance,
    index: usize,
    out: &mut [u8],
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.control_output_descriptor(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown control output index {index}"),
            0,
            0,
        ));
    };
    let expected_bytes = desc.byte_size();
    if out.len() != expected_bytes {
        return Err(Diagnostic::runtime(
            format!(
                "control output '{}' expects {} destination bytes, got {}",
                desc.name(),
                expected_bytes,
                out.len()
            ),
            0,
            0,
        ));
    }
    let Some(start) = instance.program.control_output_storage_byte_offset(index) else {
        return Err(Diagnostic::runtime(
            format!("control output '{}' has no runtime storage", desc.name()),
            0,
            0,
        ));
    };
    let end = start.saturating_add(expected_bytes);
    let state = instance.initialized_state()?.bytes();
    if end > state.len() {
        return Err(Diagnostic::runtime(
            format!(
                "control output '{}' byte range [{start}, {end}) is out of bounds for runtime storage ({})",
                desc.name(),
                state.len()
            ),
            0,
            0,
        ));
    }
    out.copy_from_slice(&state[start..end]);
    Ok(())
}

/// Binds borrowed host input memory without copying it.
///
/// # Safety
///
/// `ptr` must remain readable for `bytes` bytes at a stable address until the
/// slot is rebound/unbound or the instance is destroyed. The pointer must have
/// the natural alignment of the declared primitive element type; this function
/// validates the address before retaining it. Bound input, output, and
/// external-buffer regions must not overlap while processing.
pub unsafe fn bind_input(
    instance: &mut Instance,
    index: usize,
    ptr: *const u8,
    bytes: usize,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.inputs().get(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown input index {index}"),
            0,
            0,
        ));
    };
    if ptr.is_null() {
        if bytes == 0 {
            instance.input_bindings[index] = None;
            instance.inputs_validated = false;
            return Ok(());
        }
        return Err(Diagnostic::runtime(
            format!("input '{}' binding pointer is null", desc.name()),
            0,
            0,
        ));
    }
    validate_pointer_alignment(ptr, desc.elem_ty(), "input", desc.name())?;
    let expected = desc
        .byte_size()
        .saturating_mul(instance.config.frames_per_block);
    if bytes != expected {
        return Err(Diagnostic::runtime(
            format!(
                "input '{}' expects {} bytes for one block, got {}",
                desc.name(),
                expected,
                bytes
            ),
            0,
            0,
        ));
    }
    instance.input_bindings[index] = Some(BoundInput { ptr, bytes });
    instance.inputs_validated = false;
    Ok(())
}

/// Binds borrowed host output memory without copying it.
///
/// # Safety
///
/// `ptr` must remain writable for `bytes` bytes at a stable address until the
/// slot is rebound/unbound or the instance is destroyed. The pointer must have
/// the natural alignment of the declared primitive element type; this function
/// validates the address before retaining it. Bound input, output, and
/// external-buffer regions must not overlap while processing.
pub unsafe fn bind_output(
    instance: &mut Instance,
    index: usize,
    ptr: *mut u8,
    bytes: usize,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.outputs().get(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown output index {index}"),
            0,
            0,
        ));
    };
    if ptr.is_null() {
        if bytes == 0 {
            instance.output_bindings[index] = None;
            instance.outputs_validated = false;
            return Ok(());
        }
        return Err(Diagnostic::runtime(
            format!("output '{}' binding pointer is null", desc.name()),
            0,
            0,
        ));
    }
    validate_pointer_alignment(ptr.cast_const(), desc.elem_ty(), "output", desc.name())?;
    let expected = desc
        .byte_size()
        .saturating_mul(instance.config.frames_per_block);
    if bytes != expected {
        return Err(Diagnostic::runtime(
            format!(
                "output '{}' expects {} bytes for one block, got {}",
                desc.name(),
                expected,
                bytes
            ),
            0,
            0,
        ));
    }
    instance.output_bindings[index] = Some(BoundOutput { ptr, bytes });
    instance.outputs_validated = false;
    Ok(())
}

/// Binds borrowed external-buffer memory without copying it.
///
/// A zero `sample_rate_hz` unbinds the slot regardless of pointer and shape. A null pointer with
/// zero frames and channels also unbinds the slot. Unbound slots remain processable through their
/// prepared neutral descriptor. Otherwise the binding must be nonempty and `sample_rate_hz` must
/// be finite and positive.
///
/// # Safety
///
/// When this call binds the slot, `ptr` must remain valid for `frames * channels` elements of
/// `elem_ty`, with the element's required alignment, until rebound/unbound or instance destruction.
/// A replacement is visible to subsequent init, event, and processing calls; rebinding does not
/// itself rerun initialization. The region must be writable when buffer-write analysis reports a
/// reachable write, and all bound host regions must be mutually non-overlapping while accessed.
/// Unbind calls do not access `ptr`.
pub unsafe fn bind_buffer(
    instance: &mut Instance,
    index: usize,
    ptr: *mut u8,
    frames: usize,
    channels: usize,
    sample_rate_hz: f32,
    elem_ty: PrimitiveType,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.buffers().get(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown buffer index {index}"),
            0,
            0,
        ));
    };
    if sample_rate_hz == 0.0 || (ptr.is_null() && frames == 0 && channels == 0) {
        instance.buffer_bindings[index] = None;
        instance.buffers_validated = false;
        return Ok(());
    }
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return Err(Diagnostic::runtime(
            format!(
                "buffer '{}' requires finite sample_rate > 0, got {}",
                desc.name(),
                sample_rate_hz
            ),
            0,
            0,
        ));
    }
    if elem_ty != desc.elem_ty() {
        return Err(Diagnostic::runtime(
            format!(
                "buffer '{}' element type mismatch: expected {:?}, got {:?}",
                desc.name(),
                desc.elem_ty(),
                elem_ty
            ),
            0,
            0,
        ));
    }
    if frames == 0 || channels == 0 || ptr.is_null() {
        return Err(Diagnostic::runtime(
            format!(
                "buffer '{}' must be unbound with null + zero frames/channels or bound with a non-null pointer and positive frames/channels",
                desc.name()
            ),
            0,
            0,
        ));
    }
    validate_pointer_alignment(ptr.cast_const(), elem_ty, "buffer", desc.name())?;
    match desc.channels() {
        DeclaredBufferChannels::Mono => {
            if channels != 1 {
                return Err(Diagnostic::runtime(
                    format!(
                        "buffer '{}' expects mono (1 channel), got {}",
                        desc.name(),
                        channels
                    ),
                    0,
                    0,
                ));
            }
        }
        DeclaredBufferChannels::Static(expected) => {
            if channels != expected {
                return Err(Diagnostic::runtime(
                    format!(
                        "buffer '{}' expects {} channels, got {}",
                        desc.name(),
                        expected,
                        channels
                    ),
                    0,
                    0,
                ));
            }
        }
        DeclaredBufferChannels::Dynamic => {}
    }
    let frames_i32 = i32::try_from(frames).map_err(|_| {
        Diagnostic::runtime(
            format!(
                "buffer '{}' frames {} exceed i32 runtime limit",
                desc.name(),
                frames
            ),
            0,
            0,
        )
    })?;
    let channels_i32 = i32::try_from(channels).map_err(|_| {
        Diagnostic::runtime(
            format!(
                "buffer '{}' channels {} exceed i32 runtime limit",
                desc.name(),
                channels
            ),
            0,
            0,
        )
    })?;
    validate_buffer_byte_extent(frames_i32, channels_i32, elem_ty, desc.name())?;
    instance.buffer_bindings[index] = Some(BoundBuffer {
        ptr,
        frames_i32,
        channels_i32,
        sample_rate_hz,
    });
    instance.buffers_validated = false;
    Ok(())
}

pub fn validate_inputs(instance: &mut Instance) -> Result<(), Diagnostic> {
    let frames = instance.config.frames_per_block;
    prepare_input_ptrs_from_bindings(instance, frames)?;
    instance.inputs_validated = true;
    Ok(())
}

pub fn validate_outputs(instance: &mut Instance) -> Result<(), Diagnostic> {
    let frames = instance.config.frames_per_block;
    for (out_idx, desc) in instance.program.outputs().iter().enumerate() {
        let Some(binding) = instance.output_bindings.get(out_idx).and_then(|b| *b) else {
            return Err(Diagnostic::runtime(
                format!("required output '{}' is not bound", desc.name()),
                0,
                0,
            ));
        };
        let expected = desc.byte_size().saturating_mul(frames);
        if binding.bytes != expected {
            return Err(Diagnostic::runtime(
                format!(
                    "output '{}' bound buffer size {} does not match expected {}",
                    desc.name(),
                    binding.bytes,
                    expected
                ),
                0,
                0,
            ));
        }
    }
    prepare_output_ptrs_for_process(instance, frames)?;
    instance.outputs_validated = true;
    Ok(())
}

pub fn validate_buffers(instance: &mut Instance) -> Result<(), Diagnostic> {
    prepare_buffer_ptrs_from_bindings(instance)?;
    instance.buffers_validated = true;
    Ok(())
}

pub fn validate_bindings(instance: &mut Instance) -> Result<(), Diagnostic> {
    validate_buffers(instance)?;
    validate_inputs(instance)?;
    validate_outputs(instance)?;
    Ok(())
}

/// Processes one complete block and optionally collects delegate and print occurrences.
pub fn process_checked(
    instance: &mut Instance,
    frames: usize,
    output: ExecutionOutput<'_, '_>,
) -> Result<(), Diagnostic> {
    process_checked_segment(instance, 0, frames, PROCESS_FULL_BLOCK, output)
}

/// Processes a validated segment, optionally collects delegate and print occurrences, and
/// invalidates the instance if generated execution fails.
/// A later full initialization or snapshot restore is required before state can be used again.
pub fn process_checked_segment(
    instance: &mut Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<(), Diagnostic> {
    configure_current_thread_audio_fp_mode();
    output.reset();
    validate_process_request(instance, start_frame, frames, flags)?;
    validate_bindings_for_process(instance)?;
    let state = match &mut instance.state {
        InstanceState::Allocated(state) if state.initialized => state,
        InstanceState::Allocated(_) => return Err(invalid_instance_error()),
        InstanceState::Pending(_) => return Err(uninitialized_instance_error()),
    };
    state.attempt(|state| {
        let status = with_processor_execution_output(output, |output| unsafe {
            instance.program.process_unchecked(
                state,
                &instance.params,
                u32::try_from(start_frame).map_err(|_| {
                    Diagnostic::runtime("process start frame does not fit u32", 0, 0)
                })?,
                u32::try_from(frames).map_err(|_| {
                    Diagnostic::runtime("process frame count does not fit u32", 0, 0)
                })?,
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
        onda_codegen_llvm::check_execution_status(status)
    })
}

/// Validates the current host bindings for unchecked processing.
///
/// This is not a stale-binding snapshot operation. Call it again after rebinding before entering
/// an unchecked processing loop; MIR backends consume the current validated buffer table directly.
pub fn prepare_unchecked_process(instance: &mut Instance) -> Result<(), Diagnostic> {
    instance.initialized_state()?;
    validate_bindings_for_process(instance)
}

/// Processes one complete block without revalidating host bindings and optionally collects
/// delegate and print occurrences.
///
/// # Safety
///
/// The instance's input, output, and buffer bindings must have been successfully prepared with
/// [`prepare_unchecked_process`] (or the equivalent validation functions) after their most recent
/// mutation. Every bound region must remain valid, correctly sized, and appropriately aligned for
/// the duration of this call, without aliases that violate Rust's memory rules. Preparation must
/// occur after successful full initialization; violating that lifecycle contract is undefined
/// behavior in release builds.
pub unsafe fn process_unchecked(
    instance: &mut Instance,
    output: ExecutionOutput<'_, '_>,
) -> Result<u32, Diagnostic> {
    unsafe {
        process_unchecked_segment(
            instance,
            0,
            instance.config.frames_per_block,
            PROCESS_FULL_BLOCK,
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
/// The instance's input, output, and buffer bindings must have been successfully prepared with
/// [`prepare_unchecked_process`] (or the equivalent validation functions) after their most recent
/// mutation. Every bound region must remain valid, correctly sized, and appropriately aligned for
/// the duration of this call, without aliases that violate Rust's memory rules. Preparation must
/// occur after successful full initialization; violating that lifecycle contract is undefined
/// behavior in release builds.
pub unsafe fn process_unchecked_segment(
    instance: &mut Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, Diagnostic> {
    configure_current_thread_audio_fp_mode();
    output.reset();
    validate_process_request(instance, start_frame, frames, flags)?;
    debug_assert!(
        instance.is_initialized(),
        "process_unchecked called before full initialization; this is UB in release builds"
    );
    debug_assert!(
        instance.inputs_validated && instance.outputs_validated && instance.buffers_validated,
        "process_unchecked called without validating required input/output/buffer bindings; this is UB in release builds"
    );
    let start_frame = u32::try_from(start_frame).map_err(|_| {
        Diagnostic::runtime(
            "start frame does not fit u32 runtime/JIT process entrypoint",
            0,
            0,
        )
    })?;
    let frames = u32::try_from(frames).map_err(|_| {
        Diagnostic::runtime(
            "frame count does not fit u32 runtime/JIT process entrypoint",
            0,
            0,
        )
    })?;
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
    if status != onda_codegen_llvm::PROCESSOR_EXECUTION_OK {
        state.initialized = false;
    }
    Ok(status)
}

fn validate_process_request(
    instance: &Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
) -> Result<(), Diagnostic> {
    let Some(end_frame) = start_frame.checked_add(frames) else {
        return Err(Diagnostic::runtime(
            "segment frame range overflows usize",
            0,
            0,
        ));
    };
    if end_frame > instance.config.frames_per_block {
        return Err(Diagnostic::runtime(
            "segment start frame + frame count must be less than or equal to fixed instance block size",
            0,
            0,
        ));
    }
    let unknown_flags = flags & !PROCESS_FULL_BLOCK;
    if unknown_flags != 0 {
        return Err(Diagnostic::runtime(
            format!("unknown process flags 0x{unknown_flags:x}"),
            0,
            0,
        ));
    }
    Ok(())
}

fn validate_bindings_for_process(instance: &mut Instance) -> Result<(), Diagnostic> {
    if !instance.buffers_validated {
        validate_buffers(instance)?;
    }
    if !instance.inputs_validated {
        validate_inputs(instance)?;
    }
    if !instance.outputs_validated {
        validate_outputs(instance)?;
    }
    Ok(())
}

/// Dispatches an event, optionally collects delegate and print occurrences, and invalidates the
/// instance if generated execution fails.
/// A later full initialization or snapshot restore is required before state can be used again.
pub fn trigger_event_by_index(
    instance: &mut Instance,
    event_index: usize,
    payload: &[u8],
    mut output: ExecutionOutput<'_, '_>,
) -> Result<(), Diagnostic> {
    configure_current_thread_audio_fp_mode();
    output.reset();
    if !instance.buffers_validated {
        validate_buffers(instance)?;
    }
    let state = match &mut instance.state {
        InstanceState::Allocated(state) if state.initialized => state,
        InstanceState::Allocated(_) => return Err(invalid_instance_error()),
        InstanceState::Pending(_) => return Err(uninitialized_instance_error()),
    };
    // Payload and host-region validation happens before generated code is
    // entered and must not invalidate otherwise usable processor state. Keep
    // the execution status separate so only a generated failure closes the
    // instance, matching the process entry-point lifecycle.
    let status = with_processor_execution_output(output, |output| unsafe {
        instance.program.trigger_event_by_index_with_status(
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
    let result = onda_codegen_llvm::check_execution_status(status);
    if result.is_err() {
        state.initialized = false;
    }
    result
}

/// Dispatches an event without validating its payload or current buffer bindings, optionally
/// collecting delegate and print occurrences.
///
/// A nonzero generated execution status invalidates the instance. Full initialization is then
/// required before any further processing, event dispatch, or task execution.
///
/// # Safety
///
/// Buffer bindings must have been validated after their most recent mutation and must remain valid
/// for the call. `payload` must exactly match the declared fixed or dynamic layout for
/// `event_index`, including all slice length prefixes and element data. The instance must have
/// completed full initialization; violating that lifecycle contract is undefined behavior in
/// release builds.
pub unsafe fn trigger_event_by_index_unchecked(
    instance: &mut Instance,
    event_index: usize,
    payload: &[u8],
    mut output: ExecutionOutput<'_, '_>,
) -> Result<u32, Diagnostic> {
    configure_current_thread_audio_fp_mode();
    output.reset();
    debug_assert!(
        instance.is_initialized(),
        "trigger_event_by_index_unchecked called before full initialization; this is UB in release builds"
    );
    debug_assert!(
        instance.buffers_validated,
        "trigger_event_by_index_unchecked called without preparing buffer descriptors"
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
    if status != onda_codegen_llvm::PROCESSOR_EXECUTION_OK {
        state.initialized = false;
    }
    Ok(status)
}

fn prepare_buffer_ptrs_from_bindings(instance: &mut Instance) -> Result<(), Diagnostic> {
    for (idx, desc) in instance.program.buffers().iter().enumerate() {
        if let Some(bound) = instance.buffer_bindings.get(idx).and_then(|v| *v) {
            instance.buffer_ptrs[idx] = bound.ptr;
            instance.buffer_frames[idx] = bound.frames_i32;
            instance.buffer_channels[idx] = bound.channels_i32;
            instance.buffer_sample_rates[idx] = bound.sample_rate_hz;
        } else {
            instance.buffer_ptrs[idx] = std::ptr::null_mut();
            instance.buffer_frames[idx] = 1;
            instance.buffer_channels[idx] = fallback_channels(desc)?;
            instance.buffer_sample_rates[idx] = instance.config.sample_rate;
        }
    }
    Ok(())
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
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn primitive_type_alignment(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 => std::mem::align_of::<f32>(),
        PrimitiveType::F64 => std::mem::align_of::<f64>(),
        PrimitiveType::I32 => std::mem::align_of::<i32>(),
        PrimitiveType::I64 => std::mem::align_of::<i64>(),
        PrimitiveType::Bool => std::mem::align_of::<u8>(),
    }
}

fn validate_pointer_alignment(
    ptr: *const u8,
    ty: PrimitiveType,
    surface: &str,
    name: &str,
) -> Result<(), Diagnostic> {
    let required = primitive_type_alignment(ty);
    if (ptr as usize).is_multiple_of(required) {
        return Ok(());
    }
    Err(Diagnostic::runtime(
        format!("{surface} '{name}' binding pointer requires {required}-byte alignment for {ty:?}"),
        0,
        0,
    ))
}

fn validate_buffer_byte_extent(
    frames: i32,
    channels: i32,
    element: PrimitiveType,
    name: &str,
) -> Result<i32, Diagnostic> {
    let element_size =
        i32::try_from(primitive_type_bytes(element)).expect("primitive element sizes fit i32");
    frames
        .checked_mul(channels)
        .and_then(|elements| elements.checked_mul(element_size))
        .ok_or_else(|| {
        Diagnostic::runtime(
            format!(
                "buffer '{name}' byte extent {frames} * {channels} * {element_size} exceeds i32 runtime limit"
            ),
            0,
            0,
        )
    })
}

fn prepare_input_ptrs_from_bindings(
    instance: &mut Instance,
    frames: usize,
) -> Result<(), Diagnostic> {
    let required_in_channels = instance.program.required_in_channels();
    if instance.input_ptrs.len() != required_in_channels {
        return Err(Diagnostic::runtime(
            "runtime input channel pointer storage does not match compiled program",
            0,
            0,
        ));
    }

    for (in_idx, desc) in instance.program.inputs().iter().enumerate() {
        let Some(binding) = instance.input_bindings.get(in_idx).and_then(|b| *b) else {
            return Err(Diagnostic::runtime(
                format!("required input '{}' is not bound", desc.name()),
                0,
                0,
            ));
        };
        if binding.bytes != desc.byte_size().saturating_mul(frames) {
            return Err(Diagnostic::runtime(
                format!(
                    "input '{}' bound buffer size {} does not match expected {}",
                    desc.name(),
                    binding.bytes,
                    desc.byte_size().saturating_mul(frames)
                ),
                0,
                0,
            ));
        }
        let elem_bytes = primitive_type_bytes(desc.elem_ty());
        for ch in 0..desc.array_len() {
            let in_channel = desc.slot_offset().saturating_add(ch);
            let src_off = ch.saturating_mul(frames).saturating_mul(elem_bytes);
            instance.input_ptrs[in_channel] = unsafe { binding.ptr.add(src_off) };
        }
    }
    Ok(())
}

fn prepare_output_ptrs_for_process(
    instance: &mut Instance,
    frames: usize,
) -> Result<(), Diagnostic> {
    let required_out_channels = instance.program.required_out_channels();
    if instance.output_ptrs.len() != required_out_channels {
        return Err(Diagnostic::runtime(
            "runtime output channel pointer storage does not match compiled program",
            0,
            0,
        ));
    }
    instance.output_ptrs.fill(std::ptr::null_mut());

    for (out_idx, desc) in instance.program.outputs().iter().enumerate() {
        let Some(binding) = instance.output_bindings.get(out_idx).and_then(|b| *b) else {
            return Err(Diagnostic::runtime(
                format!("required output '{}' is not bound", desc.name()),
                0,
                0,
            ));
        };
        if binding.bytes != desc.byte_size().saturating_mul(frames) {
            return Err(Diagnostic::runtime(
                format!(
                    "output '{}' bound buffer size {} does not match expected {}",
                    desc.name(),
                    binding.bytes,
                    desc.byte_size().saturating_mul(frames)
                ),
                0,
                0,
            ));
        }
        let elem_bytes = primitive_type_bytes(desc.elem_ty());
        for ch in 0..desc.array_len() {
            let out_channel = desc.slot_offset().saturating_add(ch);
            let dst_off = ch.saturating_mul(frames).saturating_mul(elem_bytes);
            instance.output_ptrs[out_channel] = unsafe { binding.ptr.add(dst_off) };
        }
    }
    for desc in instance.program.outputs() {
        for ch in 0..desc.array_len() {
            let out_channel = desc.slot_offset().saturating_add(ch);
            if instance.output_ptrs[out_channel].is_null() {
                return Err(Diagnostic::runtime(
                    format!("output '{}' channel pointer is null", desc.name()),
                    0,
                    0,
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
