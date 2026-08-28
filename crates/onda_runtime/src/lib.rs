use onda_codegen_llvm::{
    DeclaredBufferChannels, JitProgram, RuntimeAllocator, RuntimeBuffer, RuntimeState,
    UninitializedRuntimeState,
};
use onda_frontend::{Diagnostic, PrimitiveType};
use onda_realtime::configure_current_thread_audio_fp_mode;
use std::fmt::{self, Write as _};
use std::marker::PhantomData;

pub use onda_codegen_llvm::{ParamDomain, ParamScalarType, ParamScale};

/// Bytes occupied by the delegate index and payload-length header of each packed occurrence.
pub const DELEGATE_RECORD_HEADER_SIZE: usize = 8;

/// A non-owning decoded view into one occurrence in a [`DelegateBatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelegateOccurrence<'batch> {
    pub delegate_index: u32,
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
        let payload_bytes = u32::from_ne_bytes(header[4..].try_into().ok()?) as usize;
        let record_end = header_end.checked_add(payload_bytes)?;
        let payload = self.storage.get(header_end..record_end)?;
        self.cursor = record_end;
        self.remaining -= 1;
        Some(DelegateOccurrence {
            delegate_index,
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

pub const PRINT_RECORD_HEADER_SIZE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintOccurrence<'batch> {
    pub site_index: u32,
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
        let payload_bytes = u32::from_ne_bytes(header[4..].try_into().ok()?) as usize;
        let record_end = header_end.checked_add(payload_bytes)?;
        let payload = self.storage.get(header_end..record_end)?;
        self.cursor = record_end;
        self.remaining -= 1;
        Some(PrintOccurrence {
            site_index,
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
                header[4..]
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

fn reset_execution_output(output: &mut ExecutionOutput<'_, '_>) {
    if let Some(batch) = output.delegate_batch.as_deref_mut() {
        batch.reset();
    }
    if let Some(batch) = output.print_batch.as_deref_mut() {
        batch.reset();
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
                    &self.buffer_ptrs,
                    &self.buffer_frames,
                    &self.buffer_channels,
                    &self.buffer_sample_rates,
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

pub fn create_instance_initialized_with_output(
    program: JitProgram,
    config: InstanceConfig,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<Instance, Diagnostic> {
    reset_execution_output(&mut output);
    let mut instance = create_instance(program, config)?;
    init_with_output(&mut instance, InitMode::Full, output)?;
    Ok(instance)
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

pub fn create_instance_initialized_with_allocator_and_output(
    program: JitProgram,
    config: InstanceConfig,
    allocator: RuntimeAllocator,
    mut output: ExecutionOutput<'_, '_>,
) -> Result<Instance, Diagnostic> {
    reset_execution_output(&mut output);
    let mut instance = create_instance_with_allocator(program, config, allocator)?;
    init_with_output(&mut instance, InitMode::Full, output)?;
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
    reset_execution_output(&mut output);
    if !instance.buffers_validated {
        validate_buffers(instance)?;
    }
    with_processor_execution_output(output, |output| match (&mut instance.state, mode) {
        (InstanceState::Pending(state), InitMode::Full) => {
            let initialized = instance.program.initialize_allocated_state(
                &instance.params,
                state,
                &instance.buffer_ptrs,
                &instance.buffer_frames,
                &instance.buffer_channels,
                &instance.buffer_sample_rates,
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
                &instance.buffer_ptrs,
                &instance.buffer_frames,
                &instance.buffer_channels,
                &instance.buffer_sample_rates,
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
    reset_execution_output(&mut output);
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
    reset_execution_output(&mut output);
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
    reset_execution_output(&mut output);
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
    reset_execution_output(&mut output);
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
mod tests {
    use super::*;

    use onda_codegen_llvm::jit_program_from_optimized_mir;
    use onda_frontend::parse_program;
    use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};

    fn assert_send<T: Send>() {}

    fn compile_test_instance(source: &str, block_size: usize, out_channels: usize) -> Instance {
        let parsed = parse_program(source).expect("test source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size,
            },
        )
        .expect("test source should analyze");
        let mir = lower_program_to_optimized_mir(&typed).expect("test source should lower");
        let program = jit_program_from_optimized_mir(mir).expect("test MIR should compile");
        create_instance_initialized(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: block_size,
                in_channels: 0,
                out_channels,
            },
        )
        .expect("test instance should initialize")
    }

    #[test]
    fn instance_is_send() {
        assert_send::<Instance>();
    }

    #[test]
    fn canonical_print_floats_preserve_width_and_boundary_rules() {
        assert_eq!(canonical_f32(1.234567_f32), "1.234567");
        assert_eq!(canonical_f64(-0.0), "-0.0");
        assert_eq!(canonical_f64(1.0e-6), "0.000001");
        assert_eq!(canonical_f64(1.0e-7), "1e-7");
        assert_eq!(canonical_f64(1.0e20), "100000000000000000000.0");
        assert_eq!(canonical_f64(1.0e21), "1e21");
        assert_eq!(canonical_f32(f32::INFINITY), "inf");
        assert_eq!(canonical_f64(f64::NEG_INFINITY), "-inf");
        assert_eq!(canonical_f32(f32::from_bits(0x7fc0_0001)), "NaN");
        assert_eq!(canonical_f32(f32::from_bits(1)), "1e-45");
        assert_eq!(canonical_f64(f64::from_bits(1)), "5e-324");
    }

    #[test]
    fn canonical_print_floats_match_the_shared_randomized_bit_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/onda_processor_abi/test/fixtures/print-float-parity.json"
        ))
        .expect("shared print float fixture should be valid JSON");
        for entry in fixture["f32"].as_array().expect("f32 fixture array") {
            let bits = u32::from_str_radix(entry["bits"].as_str().expect("f32 bits"), 16)
                .expect("valid f32 bits");
            assert_eq!(
                canonical_f32(f32::from_bits(bits)),
                entry["text"].as_str().expect("f32 text")
            );
        }
        for entry in fixture["f64"].as_array().expect("f64 fixture array") {
            let bits = u64::from_str_radix(entry["bits"].as_str().expect("f64 bits"), 16)
                .expect("valid f64 bits");
            assert_eq!(
                canonical_f64(f64::from_bits(bits)),
                entry["text"].as_str().expect("f64 text")
            );
        }
    }

    #[test]
    fn absent_print_storage_does_not_skip_argument_evaluation() {
        let mut instance = compile_test_instance(
            r#"
proc Counter:
  outs:
    out1
  init:
    count: i32 = 0
  sample:
    count += 1
    out1 = f32(count)

outs:
  out1
init:
  counter = Counter()
sample:
  print(counter())
  out1 = counter()
"#,
            1,
            1,
        );
        let mut output = [0.0_f32; 1];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, 1, ExecutionOutput::none())
            .expect("process without print storage should succeed");
        assert_eq!(output, [2.0]);
    }

    #[test]
    fn delegate_batch_iterates_complete_native_records_without_allocation() {
        let mut storage = [0_u8; 21];
        storage[0..4].copy_from_slice(&2_u32.to_ne_bytes());
        storage[4..8].copy_from_slice(&4_u32.to_ne_bytes());
        storage[8..12].copy_from_slice(&17_i32.to_ne_bytes());
        storage[12..16].copy_from_slice(&5_u32.to_ne_bytes());
        storage[16..20].copy_from_slice(&1_u32.to_ne_bytes());
        storage[20] = 1;

        let mut batch = DelegateBatch::from_storage(&mut storage);
        batch.used_bytes = 21;
        batch.record_count = 2;
        batch.overflow_count = 3;

        let occurrences = batch.occurrences().collect::<Vec<_>>();
        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].delegate_index, 2);
        assert_eq!(occurrences[0].payload, 17_i32.to_ne_bytes());
        assert_eq!(occurrences[1].delegate_index, 5);
        assert_eq!(occurrences[1].payload, [1]);
        assert_eq!(batch.occurrence(1), Some(occurrences[1]));
        assert_eq!(batch.occurrence(2), None);
        assert_eq!(batch.capacity_bytes(), 21);
        assert_eq!(batch.overflow_count, 3);
    }

    #[test]
    fn instance_reports_payload_and_complete_delegate_record_sizes() {
        let instance = compile_test_instance(
            r#"
delegate fixed(value: i32)
delegate dynamic(values: f32[])
sample:
  out1 = 0.0
"#,
            1,
            1,
        );

        assert_eq!(instance.delegate_count(), 2);
        assert_eq!(instance.delegate_name(0), Some("fixed"));
        assert_eq!(instance.delegate_index("dynamic"), Some(1));
        assert_eq!(instance.delegate_payload_bytes(0), Some(4));
        assert_eq!(instance.delegate_payload_min_bytes(0), Some(4));
        assert_eq!(instance.delegate_record_bytes(0), Some(12));
        assert_eq!(instance.delegate_record_min_bytes(0), Some(12));
        assert_eq!(instance.delegate_payload_bytes(1), None);
        assert_eq!(instance.delegate_payload_min_bytes(1), Some(4));
        assert_eq!(instance.delegate_record_bytes(1), None);
        assert_eq!(instance.delegate_record_min_bytes(1), Some(12));
        assert!(instance.delegate_descriptor(2).is_none());
    }

    #[test]
    fn bufferless_instances_have_no_fallback_storage() {
        let parsed = parse_program("sample:\n  out1 = 0.0\n").expect("source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: 64,
            },
        )
        .expect("source should analyze");
        let mir =
            lower_program_to_optimized_mir(&typed).expect("typed program should lower to MIR");
        let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
        let instance = create_instance_initialized(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: 64,
                in_channels: 0,
                out_channels: 1,
            },
        )
        .expect("instance should initialize");

        assert!(instance.buffer_ptrs.is_empty());
    }

    #[test]
    fn creation_defers_full_init_until_after_initial_parameter_configuration() {
        let parsed = parse_program(
            "params:\n  gain = 0.25\ninit:\n  value = gain\nsample:\n  out1 = value\n",
        )
        .expect("source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: 1,
            },
        )
        .expect("source should analyze");
        let mir = lower_program_to_optimized_mir(&typed).expect("source should lower");
        let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
        let mut instance = create_instance(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: 1,
                in_channels: 0,
                out_channels: 1,
            },
        )
        .expect("instance allocation should succeed");
        let mut output = [0.0_f32; 1];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        assert!(!instance.is_initialized());
        assert!(process_checked(&mut instance, 1, ExecutionOutput::none()).is_err());
        assert!(init(&mut instance, InitMode::PreservePinned).is_err());
        set_param_by_index(&mut instance, 0, &0.75_f32.to_ne_bytes())
            .expect("initial parameter should update");
        init(&mut instance, InitMode::Full).expect("full init should succeed");
        assert!(instance.is_initialized());
        process_checked(&mut instance, 1, ExecutionOutput::none())
            .expect("initialized instance should process");
        assert_eq!(output, [0.75]);
    }

    #[test]
    fn dynamic_proc_task_activation_tracks_each_while_condition_evaluation() {
        let sources = [
            r#"
proc Child:
  init:
    pin progress: i32 = 0
  task load():
    progress = 1
    yield
    progress = 2
  block:
    await load()
    sample:
      out1 = f32(progress)

proc Parent:
  init:
    children: Child[2] = Child()
  sample:
    index = 0
    while index < 2 && children[index]() > 0.0:
      index = index + 1
    out1 = f32(index) * 0.25

init:
  parent = Parent()
sample:
  out1 = parent()
"#,
            r#"
proc Child:
  init:
    pin progress: i32 = 0
  task load():
    progress = 1
    yield
    progress = 2
  block:
    await load()
    sample:
      out1 = f32(progress)

init:
  children: Child[2] = Child()
sample:
  index = 0
  while index < 2 && children[index]() > 0.0:
    index = index + 1
  out1 = f32(index) * 0.25
"#,
        ];

        for source in sources {
            let mut instance = compile_test_instance(source, 1, 1);
            let mut output = [0.0_f32; 1];
            unsafe {
                bind_output(
                    &mut instance,
                    0,
                    output.as_mut_ptr().cast(),
                    std::mem::size_of_val(&output),
                )
                .expect("output should bind");
            }

            for expected in [0.0, 0.25, 0.5] {
                process_checked(&mut instance, 1, ExecutionOutput::none())
                    .expect("task block should process");
                assert_eq!(output[0], expected);
            }
        }
    }

    #[test]
    fn dynamic_proc_guards_preserve_short_circuiting_and_evaluate_selectors_once() {
        let source = r#"
proc Child:
  block:
    sample:
      out1 = 0.0

proc Parent:
  init:
    children: Child[2] = Child()
    calls: i32 = 0
  def select() -> i32:
    calls = calls + 1
    return 0
  sample:
    if false && children[select()]() > 0.0:
      calls = calls + 100
    value = children[select()]()
    out1 = f32(calls) * 0.1

init:
  parent = Parent()
sample:
  out1 = parent()
"#;
        let mut instance = compile_test_instance(source, 1, 1);
        let mut output = [0.0_f32; 1];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, 1, ExecutionOutput::none())
            .expect("task block should process");
        assert_eq!(output[0], 0.1);
    }

    #[test]
    fn top_level_task_declarations_do_not_close_a_standalone_sample_gate() {
        let source = "task unused():\n  yield\nsample:\n  out1 = 1.0\n";
        let mut instance = compile_test_instance(source, 4, 1);
        let mut output = [0.0_f32; 4];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, 4, ExecutionOutput::none())
            .expect("standalone sample should process");
        assert_eq!(output, [1.0; 4]);
    }

    #[test]
    fn state_init_and_restore_preserve_validated_buffer_tables() {
        let parsed = parse_program(
            "buffers:\n  data: f32\ninit:\n  counter = 0.0\nsample:\n  counter = counter + 1.0\n",
        )
        .expect("source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: 64,
            },
        )
        .expect("source should analyze");
        let mir =
            lower_program_to_optimized_mir(&typed).expect("typed program should lower to MIR");
        let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
        let mut instance = create_instance_initialized(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: 64,
                in_channels: 0,
                out_channels: 0,
            },
        )
        .expect("instance should initialize");
        let mut samples = [1.0_f32, 2.0];
        unsafe {
            bind_buffer(
                &mut instance,
                0,
                samples.as_mut_ptr().cast(),
                samples.len(),
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .expect("buffer should bind");
        }
        validate_buffers(&mut instance).expect("buffer should validate");
        let bound_ptr = instance.buffer_ptrs[0];
        let snapshot = instance
            .snapshot_state_bytes()
            .expect("initialized state should snapshot");

        init(&mut instance, InitMode::PreservePinned).expect("init should succeed");
        assert!(instance.buffers_validated);
        assert_eq!(instance.buffer_ptrs[0], bound_ptr);

        instance
            .restore_state_bytes(&snapshot)
            .expect("state should restore");
        assert!(instance.buffers_validated);
        assert_eq!(instance.buffer_ptrs[0], bound_ptr);
    }

    #[test]
    fn top_level_and_proc_init_observe_current_buffer_bindings() {
        let parsed = parse_program(
            r#"
proc Reader:
  buffers:
    source: f32

  init:
    first = source[0]
    frames = source.len()

  sample:
    out1 = first + f32(frames)

buffers:
  source: f32

init:
  selected = source[1]
  reader = Reader(source = source)

event refresh():
  reader.init()

sample:
  out1 = selected + reader() + source[0]
"#,
        )
        .expect("source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: 1,
            },
        )
        .expect("source should analyze");
        let mir = lower_program_to_optimized_mir(&typed).expect("source should lower");
        let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
        let config = InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: 1,
            in_channels: 0,
            out_channels: 1,
        };

        let mut neutral = create_instance_initialized(program.clone(), config)
            .expect("neutral instance should initialize");
        let mut neutral_output = [0.0_f32; 1];
        unsafe {
            bind_output(
                &mut neutral,
                0,
                neutral_output.as_mut_ptr().cast(),
                std::mem::size_of_val(&neutral_output),
            )
            .expect("neutral output should bind");
        }
        process_checked(&mut neutral, 1, ExecutionOutput::none())
            .expect("neutral instance should process");
        assert_eq!(neutral_output, [1.0]);

        let mut bound = create_instance(program, config).expect("instance should allocate");
        let mut samples = [2.0_f32, 5.0];
        let mut bound_output = [0.0_f32; 1];
        unsafe {
            bind_buffer(
                &mut bound,
                0,
                samples.as_mut_ptr().cast(),
                samples.len(),
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .expect("buffer should bind");
            bind_output(
                &mut bound,
                0,
                bound_output.as_mut_ptr().cast(),
                std::mem::size_of_val(&bound_output),
            )
            .expect("bound output should bind");
        }
        init(&mut bound, InitMode::Full).expect("bound instance should initialize");
        process_checked(&mut bound, 1, ExecutionOutput::none())
            .expect("bound instance should process");
        assert_eq!(bound_output, [11.0]);

        let mut replacement = [7.0_f32, 11.0];
        unsafe {
            bind_buffer(
                &mut bound,
                0,
                replacement.as_mut_ptr().cast(),
                replacement.len(),
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .expect("replacement buffer should bind");
        }
        process_checked(&mut bound, 1, ExecutionOutput::none())
            .expect("replacement binding should be visible without reinitialization");
        assert_eq!(bound_output, [16.0]);

        let refresh = bound.event_index("refresh").expect("refresh event");
        trigger_event_by_index(&mut bound, refresh, &[], ExecutionOutput::none())
            .expect("proc init event should run against the current binding");
        process_checked(&mut bound, 1, ExecutionOutput::none())
            .expect("reinitialized proc should process");
        assert_eq!(bound_output, [21.0]);

        init(&mut bound, InitMode::Full)
            .expect("top-level init should run against the replacement binding");
        process_checked(&mut bound, 1, ExecutionOutput::none())
            .expect("fully reinitialized instance should process");
        assert_eq!(bound_output, [27.0]);
    }

    #[test]
    fn cooperative_task_yields_gate_audio_and_survive_default_init() {
        const BLOCK_SIZE: usize = 8;
        let parsed = parse_program(
            r#"
proc Loader:
  init:
    pin progress: i32 = 0
  task load():
    progress += 1
    yield
    progress += 1

  event reload():
    load.reset()

  block:
    await load()
    sample:
      out1 = f32(progress)

init:
  loader = Loader()

event reload():
  loader.reload()

sample:
  out1 = loader()
"#,
        )
        .expect("task source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: BLOCK_SIZE,
            },
        )
        .expect("task source should analyze");
        let mir = lower_program_to_optimized_mir(&typed).expect("task source should lower");
        let program = jit_program_from_optimized_mir(mir).expect("task MIR should compile");
        let mut instance = create_instance_initialized(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: BLOCK_SIZE,
                in_channels: 0,
                out_channels: 1,
            },
        )
        .expect("task instance should initialize");
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("first task block should process");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("completion block should process");
        assert_eq!(output, [2.0; BLOCK_SIZE]);

        let reload = instance.event_index("reload").expect("reload event");
        trigger_event_by_index(&mut instance, reload, &[], ExecutionOutput::none())
            .expect("task reset event should run");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("restarted task block should process");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("restarted task should complete");
        assert_eq!(output, [4.0; BLOCK_SIZE]);

        init(&mut instance, InitMode::PreservePinned).expect("default init should succeed");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("default init block should process");
        assert_eq!(output, [4.0; BLOCK_SIZE]);

        init(&mut instance, InitMode::Full).expect("full init should succeed");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("full init block should process");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_can_call_child_proc_events() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
proc Child:
  init:
    value: i32 = 0
  event add(amount: i32):
    value += amount
  sample:
    out1 = f32(value)

proc Owner:
  init:
    child = Child()
  task prepare():
    child.add(2)
    yield
    child.add(3)
  block:
    await prepare()
    sample:
      out1 = child()

init:
  owner = Owner()
sample:
  out1 = owner()
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("yielding task should process");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("completing task should process");
        assert_eq!(output, [5.0]);
    }

    #[test]
    fn task_can_step_block_rate_child_procs() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
proc Counter:
  kouts:
    value
  init:
    count: i32 = 0
  block:
    count += 1
    value = f32(count)

proc Owner:
  init:
    counter = Counter()
    pin result = 0.0
  def step_counter():
    return counter()
  task prepare():
    result = counter()
    yield
    result += step_counter()
  block:
    await prepare()
    sample:
      out1 = result

init:
  owner = Owner()
sample:
  out1 = owner()
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("yielding task should process");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("completing task should process");
        assert_eq!(output, [3.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("completed task should fall through");
        assert_eq!(output, [3.0]);
    }

    #[test]
    fn top_level_task_yields_resumes_and_resets() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
init:
  pin progress: i32 = 0
task prepare():
  progress += 1
  yield
  progress += 1

event retry():
  prepare.reset()

block:
  await prepare()
  sample:
    out1 = f32(progress)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("yielding task should process");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("completing task should process");
        assert_eq!(output, [2.0]);

        let retry = instance.event_index("retry").expect("retry event");
        trigger_event_by_index(&mut instance, retry, &[], ExecutionOutput::none())
            .expect("task reset should run");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("reset task should yield again");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("reset task should complete again");
        assert_eq!(output, [4.0]);

        init(&mut instance, InitMode::PreservePinned)
            .expect("default init should preserve the completed task");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("default init should preserve the completed task");
        assert_eq!(output, [4.0]);

        init(&mut instance, InitMode::Full).expect("full init should restart the task");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("full init should restart the task");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("restarted task should complete from a cleared state");
        assert_eq!(output, [2.0]);

        init(&mut instance, InitMode::PreservePinned)
            .expect("default init should preserve pinned task state");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("default init should preserve the completed task");
        assert_eq!(output, [2.0]);

        init(&mut instance, InitMode::Full).expect("full init should restart after default init");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("full init should restart after default init");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("fully initialized task should complete");
        assert_eq!(output, [2.0]);
    }

    #[test]
    fn task_reset_reinitializes_retained_frame_storage_on_restart() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
init:
  pin result: i32 = 0
task prepare():
  carried: i32[2] = [3, 5]
  yield
  result = carried[0] + carried[1]
event retry():
  prepare.reset()
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("initial task should yield");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("initial task should complete");
        assert_eq!(output, [8.0]);

        let retry = instance.event_index("retry").expect("retry event");
        trigger_event_by_index(&mut instance, retry, &[], ExecutionOutput::none())
            .expect("task reset should run");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("restarted task should yield");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("restarted task should complete");
        assert_eq!(output, [8.0]);
    }

    #[test]
    fn top_level_init_respects_explicit_task_reset() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
init:
  pin progress: i32 = 0
  load.reset()

task load():
  progress += 1
  yield
  progress += 1

block:
  await load()
  sample:
    out1 = f32(progress)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should yield");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete");
        assert_eq!(output, [2.0]);

        init(&mut instance, InitMode::PreservePinned)
            .expect("default init should execute explicit task reset");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("explicitly reset task should yield again");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("explicitly reset task should complete again");
        assert_eq!(output, [4.0]);
    }

    #[test]
    fn top_level_task_neutralizes_control_outputs_while_suspended() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
kouts:
  ready

init:
  pin progress: i32 = 0
task load():
  progress += 1
  yield
  progress += 1

event retry():
  load.reset()

block:
  await load()
  ready = f32(progress)
"#,
            BLOCK_SIZE,
            0,
        );
        let ready = instance
            .control_output_index("ready")
            .expect("ready control output");
        let read_ready = |instance: &Instance| {
            let mut bytes = [0_u8; size_of::<f32>()];
            read_control_output_bytes(instance, ready, &mut bytes)
                .expect("control output should be readable");
            f32::from_le_bytes(bytes)
        };

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should yield");
        assert_eq!(read_ready(&instance), 0.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete");
        assert_eq!(read_ready(&instance), 2.0);

        let retry = instance.event_index("retry").expect("retry event");
        trigger_event_by_index(&mut instance, retry, &[], ExecutionOutput::none())
            .expect("task reset should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("reset task should yield");
        assert_eq!(read_ready(&instance), 0.0);
    }

    #[test]
    fn proc_task_neutralizes_block_timed_outputs_at_the_await_barrier() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
proc Control:
  kouts 1
  init:
    pin value: i32 = 0
  task load():
    yield
    value += 1
  block:
    await load()
    kout1 = f32(value)

init:
  control = Control()

kouts:
  ready

block:
  ready = control().kout1
"#,
            BLOCK_SIZE,
            0,
        );
        let ready = instance
            .control_output_index("ready")
            .expect("ready control output");
        let read_ready = |instance: &Instance| {
            let mut bytes = [0_u8; size_of::<f32>()];
            read_control_output_bytes(instance, ready, &mut bytes)
                .expect("control output should be readable");
            f32::from_le_bytes(bytes)
        };

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("proc task should yield");
        assert_eq!(read_ready(&instance), 0.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("proc task should complete");
        assert_eq!(read_ready(&instance), 1.0);
    }

    #[test]
    fn suspended_proc_tasks_skip_nested_block_post_hooks() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
proc Child:
  init:
    pin counter: i32 = 0
  block:
    counter += 1
    sample:
      out1 = f32(counter)
    counter += 100

proc Parent:
  init:
    child = Child()
  task load():
    yield
  block:
    await load()
    sample:
      out1 = child()

proc Grandparent:
  init:
    parent = Parent()
  sample:
    out1 = parent()

init:
  direct = Parent()
  nested = Grandparent()
sample:
  out1 = direct()
  out2 = nested()
"#,
            BLOCK_SIZE,
            2,
        );
        let mut direct = [99.0_f32; BLOCK_SIZE];
        let mut nested = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                direct.as_mut_ptr().cast(),
                std::mem::size_of_val(&direct),
            )
            .expect("direct output should bind");
            bind_output(
                &mut instance,
                1,
                nested.as_mut_ptr().cast(),
                std::mem::size_of_val(&nested),
            )
            .expect("nested output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("both tasks should yield");
        assert_eq!(direct, [0.0]);
        assert_eq!(nested, [0.0]);

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("both tasks should complete");
        assert_eq!(direct, [1.0]);
        assert_eq!(nested, [1.0]);
    }

    #[test]
    fn top_level_task_suspension_escapes_nested_block_pre_loops() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
init:
  pin progress: i32 = 0
  pin activations: i32 = 0
task load():
  progress += 1
  yield
  progress += 1

block:
  for outer in 0..2:
    for inner in 0..2:
      await load()
  activations += 1

  sample:
    out1 = f32(activations)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should yield inside nested loops");
        assert_eq!(output, [0.0]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete on the next block");
        assert_eq!(output, [1.0]);
    }

    #[test]
    fn live_init_preserves_pinned_state_and_full_init_reinitializes_it() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
outs { out1, out2 }
init {
  pin pinned: i32 = 10
  ordinary: i32 = 20
  pinned += 1
}
sample {
  pinned += 1
  ordinary += 1
  out1 = f32(pinned)
  out2 = f32(ordinary)
}
"#,
            BLOCK_SIZE,
            2,
        );
        let mut pinned = [0.0_f32; BLOCK_SIZE];
        let mut ordinary = [0.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                pinned.as_mut_ptr().cast(),
                std::mem::size_of_val(&pinned),
            )
            .expect("pinned output should bind");
            bind_output(
                &mut instance,
                1,
                ordinary.as_mut_ptr().cast(),
                std::mem::size_of_val(&ordinary),
            )
            .expect("ordinary output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("initial state should process");
        assert_eq!((pinned[0], ordinary[0]), (12.0, 21.0));

        init(&mut instance, InitMode::PreservePinned).expect("ordinary live init should succeed");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("reinitialized state should process");
        assert_eq!((pinned[0], ordinary[0]), (14.0, 21.0));
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("state should advance");
        assert_eq!((pinned[0], ordinary[0]), (15.0, 22.0));

        init(&mut instance, InitMode::PreservePinned).expect("second default init should succeed");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("second init state should process");
        assert_eq!((pinned[0], ordinary[0]), (17.0, 21.0));

        init(&mut instance, InitMode::Full).expect("full live init should succeed");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("fully initialized state should process");
        assert_eq!((pinned[0], ordinary[0]), (12.0, 21.0));
    }

    #[test]
    fn live_init_handles_untyped_pinned_declarations() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
outs { out1 }
events {
  set_amp(value: f32) {
    amp = value
    pinned = value + 1.0
  }
}
init {
  amp = 0.0
  pin pinned = 1.0
}
sample { out1 = amp + pinned }
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [0.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }
        let event = instance.event_index("set_amp").expect("set_amp event");
        trigger_event_by_index(
            &mut instance,
            event,
            &0.5_f32.to_ne_bytes(),
            ExecutionOutput::none(),
        )
        .expect("event should run");
        init(&mut instance, InitMode::PreservePinned).expect("live init should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("state should process");
        assert_eq!(output, [1.5]);
    }

    #[test]
    fn init_preserves_pinned_structs_and_full_init_reinitializes_them() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
struct State:
  value: i32 = 1

init:
  pin one = State()
  pin many: State[2] = State()
  ordinary = State()

event mutate():
  one.value = 10
  many[0].value = 20
  many[1].value = 30
  ordinary.value = 40

sample:
  out1 = f32(one.value + many[0].value + many[1].value + ordinary.value)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [0.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("initial state should process");
        assert_eq!(output, [4.0]);

        let mutate = instance.event_index("mutate").expect("mutate event");
        trigger_event_by_index(&mut instance, mutate, &[], ExecutionOutput::none())
            .expect("mutation should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("mutated state should process");
        assert_eq!(output, [100.0]);

        init(&mut instance, InitMode::PreservePinned).expect("default init should succeed");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("default init should process");
        assert_eq!(output, [61.0]);

        init(&mut instance, InitMode::Full).expect("full init should succeed");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("full init should process");
        assert_eq!(output, [4.0]);
    }

    #[test]
    fn failed_live_init_invalidates_state_until_full_init_or_restore() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
params:
  divisor: i32 = 1
outs:
  out1
init:
  pin pinned: i32 = 10
  pinned += 1
  quotient: i32 = 10 / divisor
event set_pinned(value: i32):
  pinned = value
sample:
  out1 = f32(pinned)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [0.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }
        let event = instance
            .event_index("set_pinned")
            .expect("set_pinned event");
        trigger_event_by_index(
            &mut instance,
            event,
            &50_i32.to_ne_bytes(),
            ExecutionOutput::none(),
        )
        .expect("event should run");
        let snapshot = instance
            .snapshot_state_bytes()
            .expect("valid state should snapshot");
        set_param_by_index(&mut instance, 0, &0_i32.to_ne_bytes())
            .expect("divisor parameter should update");

        assert!(
            init(&mut instance, InitMode::PreservePinned).is_err(),
            "division by zero should fail"
        );
        assert!(!instance.is_initialized());
        assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
        assert!(trigger_event_by_index(
            &mut instance,
            event,
            &0_i32.to_ne_bytes(),
            ExecutionOutput::none()
        )
        .is_err());
        assert!(instance.snapshot_state_bytes().is_err());

        set_param_by_index(&mut instance, 0, &1_i32.to_ne_bytes())
            .expect("divisor parameter should recover");
        assert!(
            init(&mut instance, InitMode::PreservePinned).is_err(),
            "preserve-pinned init cannot recover indeterminate pinned state"
        );
        instance
            .restore_state_bytes(&snapshot)
            .expect("a valid snapshot should recover invalid state");
        assert!(instance.is_initialized());
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("restored state should process");
        assert_eq!(output, [50.0]);

        set_param_by_index(&mut instance, 0, &0_i32.to_ne_bytes())
            .expect("divisor parameter should update");
        assert!(init(&mut instance, InitMode::PreservePinned).is_err());
        set_param_by_index(&mut instance, 0, &1_i32.to_ne_bytes())
            .expect("divisor parameter should recover");
        init(&mut instance, InitMode::Full).expect("full init should recover invalid state");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("reinitialized state should process");
        assert_eq!(output, [11.0]);
    }

    #[test]
    fn failed_event_invalidates_state_until_full_init_or_restore() {
        let mut instance = compile_test_instance(
            r#"
init:
  held: i32 = 0
event divide(divisor: i32):
  held = 1 / divisor
sample:
  out1 = f32(held)
"#,
            1,
            1,
        );
        let event = instance.event_index("divide").expect("divide event");

        assert!(trigger_event_by_index(
            &mut instance,
            event,
            &0_i32.to_ne_bytes(),
            ExecutionOutput::none()
        )
        .is_err());
        assert!(!instance.is_initialized());
        assert!(trigger_event_by_index(
            &mut instance,
            event,
            &1_i32.to_ne_bytes(),
            ExecutionOutput::none()
        )
        .is_err());
        assert!(init(&mut instance, InitMode::PreservePinned).is_err());

        init(&mut instance, InitMode::Full).expect("full init should recover the instance");
        trigger_event_by_index(
            &mut instance,
            event,
            &1_i32.to_ne_bytes(),
            ExecutionOutput::none(),
        )
        .expect("event should run after recovery");
    }

    #[test]
    fn unchecked_event_failure_invalidates_state_until_full_init() {
        let mut instance = compile_test_instance(
            r#"
init:
  held: i32 = 0
event divide(divisor: i32):
  held = 1 / divisor
sample:
  out1 = f32(held)
"#,
            1,
            1,
        );
        let event = instance.event_index("divide").expect("divide event");
        validate_buffers(&mut instance).expect("buffer descriptors should validate");

        let status = unsafe {
            trigger_event_by_index_unchecked(
                &mut instance,
                event,
                &0_i32.to_ne_bytes(),
                ExecutionOutput::none(),
            )
        }
        .expect("unchecked dispatch should return the generated status");
        assert_eq!(
            status,
            onda_codegen_llvm::PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
        );
        assert!(!instance.is_initialized());
        assert!(trigger_event_by_index(
            &mut instance,
            event,
            &1_i32.to_ne_bytes(),
            ExecutionOutput::none()
        )
        .is_err());
        assert!(init(&mut instance, InitMode::PreservePinned).is_err());

        init(&mut instance, InitMode::Full).expect("full init should recover the instance");
        validate_buffers(&mut instance).expect("buffer descriptors should revalidate");
        let status = unsafe {
            trigger_event_by_index_unchecked(
                &mut instance,
                event,
                &1_i32.to_ne_bytes(),
                ExecutionOutput::none(),
            )
        }
        .expect("event should run after recovery");
        assert_eq!(status, onda_codegen_llvm::PROCESSOR_EXECUTION_OK);
        assert!(instance.is_initialized());
    }

    #[test]
    fn unchecked_process_failure_invalidates_state_until_full_init() {
        const BLOCK_SIZE: usize = 1;
        let mut instance = compile_test_instance(
            r#"
params:
  divisor: i32 = 0
sample:
  out1 = f32(1 / divisor)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [0.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }
        prepare_unchecked_process(&mut instance).expect("bindings should validate");

        let status = unsafe { process_unchecked(&mut instance, ExecutionOutput::none()) }
            .expect("unchecked processing should return the generated status");
        assert_eq!(
            status,
            onda_codegen_llvm::PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
        );
        assert!(!instance.is_initialized());
        assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
        assert!(init(&mut instance, InitMode::PreservePinned).is_err());

        set_param_by_index(&mut instance, 0, &1_i32.to_ne_bytes())
            .expect("divisor parameter should update");
        init(&mut instance, InitMode::Full).expect("full init should recover the instance");
        prepare_unchecked_process(&mut instance).expect("bindings should revalidate");
        let status = unsafe { process_unchecked(&mut instance, ExecutionOutput::none()) }
            .expect("processing should run after recovery");
        assert_eq!(status, onda_codegen_llvm::PROCESSOR_EXECUTION_OK);
        assert!(instance.is_initialized());
        assert_eq!(output, [1.0]);
    }

    #[test]
    fn cooperative_task_resumes_for_loop_control_state() {
        const BLOCK_SIZE: usize = 4;
        let parsed = parse_program(
            r#"
proc Loader:
  init:
    pin progress: i32 = 0
  task load():
    for i in 0..3:
      progress += 1
      yield
  block:
    await load()
    sample:
      out1 = f32(progress)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        )
        .expect("loop task source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: BLOCK_SIZE,
            },
        )
        .expect("loop task source should analyze");
        let mir = lower_program_to_optimized_mir(&typed).expect("loop task should lower");
        let program = jit_program_from_optimized_mir(mir).expect("loop task MIR should compile");
        let mut instance = create_instance_initialized(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: BLOCK_SIZE,
                in_channels: 0,
                out_channels: 1,
            },
        )
        .expect("loop task instance should initialize");
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }
        for _ in 0..3 {
            process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
                .expect("yielding block should process");
            assert_eq!(output, [0.0; BLOCK_SIZE]);
            output.fill(99.0);
        }
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("completion block should process");
        assert_eq!(output, [3.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_for_bounds_share_ordinary_i32_induction_coercion() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
init:
  pin result: i32 = 0
task prepare():
  for i in (i64(0))..(i64(2)):
    result += i
    yield
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("first loop iteration should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("second loop iteration should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task loop should complete");
        assert_eq!(output, [1.0; BLOCK_SIZE]);
    }

    #[test]
    fn explicit_i64_for_uses_i64_induction_and_overload_resolution() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
def offset(i: i32) -> i64:
  return i64(-100)
def offset(i: i64) -> i64:
  return i - i64(2147483648)
init:
  start: i64 = i64(2147483648)
  end: i64 = i64(2147483650)
sample:
  total: i64 = 0
  for i: i64 in start..end:
    total += offset(i)
  out1 = f32(total)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("i64 loop should process");
        assert_eq!(output, [1.0; BLOCK_SIZE]);
    }

    #[test]
    fn explicit_i64_for_handles_an_inclusive_maximum_endpoint() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
sample:
  count: i32 = 0
  for i: i64 in (i64(9223372036854775806))..=(i64(9223372036854775807)):
    count += 1
  out1 = f32(count)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("maximum-bound loop should process");
        assert_eq!(output, [2.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_for_completes_after_yield_at_inclusive_i64_extrema() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
init:
  pin result: i32 = 0
task prepare():
  for i: i64 in (i64(9223372036854775807))..=(i64(9223372036854775807)):
    result += 1
    yield
  for i: i64 @ -1 in (i64(-9223372036854775807) - i64(1))..=(i64(-9223372036854775807) - i64(1)):
    result += 2
    yield
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("maximum iteration should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("minimum iteration should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("extrema-bound task should complete");
        assert_eq!(output, [3.0; BLOCK_SIZE]);
    }

    #[test]
    fn proc_task_for_preserves_explicit_i64_induction_across_yields() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
proc Loader:
  init:
    pin result: i64 = 0
  task prepare():
    for i: i64 in (i64(2147483648))..(i64(2147483650)):
      result += i - i64(2147483648)
      yield
  block:
    await prepare()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("first i64 iteration should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("second i64 iteration should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("i64 task loop should complete");
        assert_eq!(output, [1.0; BLOCK_SIZE]);
    }

    #[test]
    fn fixed_tuple_task_frame_survives_yield() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
init:
  pin result: f32 = 0.0
task prepare():
  pair = (i32(3), i64(5))
  yield
  result = f32(pair[0]) + f32(pair[1])
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("tuple task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("tuple task should complete");
        assert_eq!(output, [8.0; BLOCK_SIZE]);
    }

    #[test]
    fn proc_init_respects_pinning_and_explicit_task_reset() {
        const BLOCK_SIZE: usize = 4;
        let parsed = parse_program(
            r#"
proc Keeper:
  init:
    pin progress: i32 = 10
    scratch: i32 = 20
  task load():
    progress += 1
    scratch += 1
    yield
    progress += 100
    scratch += 100
  block:
    await load()
    sample:
      out1 = f32(progress + scratch)

proc Resetter:
  init:
    pin progress: i32 = 10
    scratch: i32 = 20
    load.reset()
  task load():
    progress += 1
    scratch += 1
    yield
    progress += 100
    scratch += 100
  block:
    await load()
    sample:
      out1 = f32(progress + scratch)

outs:
  out1
  out2
init:
  keeper = Keeper()
  resetter = Resetter()
event default_init():
  keeper.init()
  resetter.init()
event full_init():
  keeper.init(all = true)
sample:
  out1 = keeper()
  out2 = resetter()
"#,
        )
        .expect("proc init task source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: BLOCK_SIZE,
            },
        )
        .expect("proc init task source should analyze");
        let mir = lower_program_to_optimized_mir(&typed).expect("proc init task should lower");
        let program = jit_program_from_optimized_mir(mir).expect("task MIR should compile");
        let mut instance = create_instance_initialized(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: BLOCK_SIZE,
                in_channels: 0,
                out_channels: 2,
            },
        )
        .expect("task instance should initialize");
        let mut output1 = [99.0_f32; BLOCK_SIZE];
        let mut output2 = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output1.as_mut_ptr().cast(),
                std::mem::size_of_val(&output1),
            )
            .expect("first output should bind");
            bind_output(
                &mut instance,
                1,
                output2.as_mut_ptr().cast(),
                std::mem::size_of_val(&output2),
            )
            .expect("second output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("both tasks should yield");
        assert_eq!(output1, [0.0; BLOCK_SIZE]);
        assert_eq!(output2, [0.0; BLOCK_SIZE]);

        let default_init = instance
            .event_index("default_init")
            .expect("default init event");
        trigger_event_by_index(&mut instance, default_init, &[], ExecutionOutput::none())
            .expect("default proc init should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("tasks should resume after init");
        assert_eq!(output1, [231.0; BLOCK_SIZE]);
        assert_eq!(output2, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("reset task should complete");
        assert_eq!(output2, [233.0; BLOCK_SIZE]);

        let full_init = instance.event_index("full_init").expect("full init event");
        trigger_event_by_index(&mut instance, full_init, &[], ExecutionOutput::none())
            .expect("forced proc init should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("fully reset task should yield");
        assert_eq!(output1, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("fully reset task should complete");
        assert_eq!(output1, [232.0; BLOCK_SIZE]);
    }

    #[test]
    fn proc_init_reinitializes_declaration_only_fixed_arrays() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
proc Probe:
  init:
    pin prepared: f32[8]
    scratch: f32[8]
  event dirty():
    prepared[0] = 9.0
    scratch[0] = 7.0
  sample:
    out1 = prepared[0] * 10.0 + scratch[0]

proc Parent:
  init:
    probe = Probe()
  event dirty():
    probe.dirty()
  event default_init():
    probe.init()
  event full_init():
    probe.init(all = true)
  sample:
    out1 = probe()

init:
  direct = Probe()
  parent = Parent()
event dirty():
  direct.dirty()
  parent.dirty()
event default_init():
  direct.init()
  parent.default_init()
event full_init():
  direct.init(all = true)
  parent.full_init()
sample:
  out1 = direct()
  out2 = parent()
"#,
            BLOCK_SIZE,
            2,
        );
        let mut direct_output = [99.0_f32; BLOCK_SIZE];
        let mut nested_output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                direct_output.as_mut_ptr().cast(),
                std::mem::size_of_val(&direct_output),
            )
            .expect("direct output should bind");
            bind_output(
                &mut instance,
                1,
                nested_output.as_mut_ptr().cast(),
                std::mem::size_of_val(&nested_output),
            )
            .expect("nested output should bind");
        }

        let dirty = instance.event_index("dirty").expect("dirty event");
        let default_init = instance
            .event_index("default_init")
            .expect("default init event");
        let full_init = instance.event_index("full_init").expect("full init event");

        trigger_event_by_index(&mut instance, dirty, &[], ExecutionOutput::none())
            .expect("arrays should become dirty");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("dirty state should process");
        assert_eq!(direct_output, [97.0; BLOCK_SIZE]);
        assert_eq!(nested_output, [97.0; BLOCK_SIZE]);

        trigger_event_by_index(&mut instance, default_init, &[], ExecutionOutput::none())
            .expect("default proc init should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("default state should process");
        assert_eq!(direct_output, [90.0; BLOCK_SIZE]);
        assert_eq!(nested_output, [90.0; BLOCK_SIZE]);

        trigger_event_by_index(&mut instance, dirty, &[], ExecutionOutput::none())
            .expect("arrays should become dirty");
        trigger_event_by_index(&mut instance, full_init, &[], ExecutionOutput::none())
            .expect("full proc init should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("fully reset state should process");
        assert_eq!(direct_output, [0.0; BLOCK_SIZE]);
        assert_eq!(nested_output, [0.0; BLOCK_SIZE]);
    }

    #[test]
    fn suspended_task_snapshots_resume_once_across_segmented_processing() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
proc Loader:
  init:
    pin progress: i32 = 0
  task load():
    progress += 1
    yield
    progress += 10
  block:
    await load()
    sample:
      out1 = f32(progress)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
            BLOCK_SIZE,
            1,
        );
        let reflected_state = (0..instance.state_count())
            .filter_map(|index| instance.state_name(index))
            .collect::<Vec<_>>();
        assert!(
            reflected_state.iter().all(|name| !name.contains("__onda_")),
            "task frame storage must not appear as authored state: {reflected_state:?}"
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked_segment(
            &mut instance,
            0,
            0,
            PROCESS_BEGIN_BLOCK,
            ExecutionOutput::none(),
        )
        .expect("zero-frame begin should advance and yield the task");
        assert_eq!(output, [99.0; BLOCK_SIZE]);
        process_checked_segment(&mut instance, 0, 2, 0, ExecutionOutput::none())
            .expect("first audio segment should observe the yielded task");
        assert_eq!(output, [0.0, 0.0, 99.0, 99.0]);
        init(&mut instance, InitMode::PreservePinned).expect("default init should succeed");
        process_checked_segment(
            &mut instance,
            2,
            2,
            PROCESS_END_BLOCK,
            ExecutionOutput::none(),
        )
        .expect("default init must not reopen the task gate within a logical block");
        assert_eq!(output, [0.0; BLOCK_SIZE]);

        let suspended = instance
            .snapshot_state_bytes()
            .expect("initialized state should snapshot");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("next block should complete the task");
        assert_eq!(output, [11.0; BLOCK_SIZE]);

        instance
            .restore_state_bytes(&suspended)
            .expect("suspended task snapshot should restore");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("restored task should resume after its yield");
        assert_eq!(output, [11.0; BLOCK_SIZE]);
    }

    #[test]
    fn default_init_does_not_reopen_top_level_task_gate_mid_block() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
init:
  pin progress: i32 = 0
task load():
  progress += 1
  yield
  progress += 10
block:
  await load()
  sample:
    out1 = f32(progress)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked_segment(
            &mut instance,
            0,
            0,
            PROCESS_BEGIN_BLOCK,
            ExecutionOutput::none(),
        )
        .expect("zero-frame begin should advance and yield the task");
        process_checked_segment(&mut instance, 0, 2, 0, ExecutionOutput::none())
            .expect("first audio segment should observe the yielded task");
        assert_eq!(output, [0.0, 0.0, 99.0, 99.0]);

        init(&mut instance, InitMode::PreservePinned).expect("default init should succeed");
        process_checked_segment(
            &mut instance,
            2,
            2,
            PROCESS_END_BLOCK,
            ExecutionOutput::none(),
        )
        .expect("default init must not reopen the top-level task gate");
        assert_eq!(output, [0.0; BLOCK_SIZE]);

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("next block should complete the task");
        assert_eq!(output, [11.0; BLOCK_SIZE]);
    }

    #[test]
    fn suspended_task_can_be_bypassed_and_later_resumed() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
proc Loader:
  init:
    enabled: bool = false
    pin progress: i32 = 0
  event set_enabled(value: bool):
    enabled = value
  task load():
    progress += 1
    yield
    progress += 10
  block:
    if enabled:
      await load()
    sample:
      out1 = f32(progress)
init:
  loader = Loader()
event set_enabled(value: bool):
  loader.set_enabled(value)
sample:
  out1 = loader()
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }
        let set_enabled = instance
            .event_index("set_enabled")
            .expect("set_enabled event");

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("disabled task should be bypassed");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        trigger_event_by_index(&mut instance, set_enabled, &[1], ExecutionOutput::none())
            .expect("enable event should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("enabled task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);

        trigger_event_by_index(&mut instance, set_enabled, &[0], ExecutionOutput::none())
            .expect("disable event should run");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("suspended task should not gate a bypassed block");
        assert_eq!(output, [1.0; BLOCK_SIZE]);

        trigger_event_by_index(&mut instance, set_enabled, &[1], ExecutionOutput::none())
            .expect("re-enable event should run");
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should resume and complete");
        assert_eq!(output, [11.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_fixed_array_and_loop_frame_survive_each_yield() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
proc Loader:
  init:
    pin result: i32 = 0
  task load():
    values: i32[4] = [3, 5, 7, 11]
    total: i32 = 0
    for i in 0..4:
      total += values[i]
      yield
    result = total
  block:
    await load()
    sample:
      out1 = f32(result)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        for expected_yields in 1..=4 {
            process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
                .expect("task iteration should yield");
            assert_eq!(
                output, [0.0; BLOCK_SIZE],
                "iteration {expected_yields} must remain behind the await barrier"
            );
        }
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete after the loop");
        assert_eq!(output, [26.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_loop_frame_does_not_overwrite_similarly_named_local() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
init:
  pin result: i32 = 0
task prepare():
  i__end: i32 = 99
  for i in 0..2:
    yield
  result = i__end

block:
  await prepare()

sample:
  out1 = f32(result)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        for _ in 0..2 {
            process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
                .expect("loop task should yield");
            assert_eq!(output, [0.0; BLOCK_SIZE]);
            output.fill(99.0);
        }
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("loop task should complete");
        assert_eq!(output, [99.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_barrier_neutralizes_every_array_output_element() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
outs:
  stereo: f32[2]

task prepare():
  yield

block:
  await prepare()
  sample:
    stereo[0] = 1.0
    stereo[1] = 2.0
"#,
            BLOCK_SIZE,
            2,
        );
        let mut output = [99.0_f32; BLOCK_SIZE * 2];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("array output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE * 2]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete");
        assert_eq!(output, [1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
    }

    #[test]
    fn task_return_inside_for_completes_without_undeclared_loop_state() {
        const BLOCK_SIZE: usize = 4;
        let sources = [
            r#"
init:
  pin result: i32 = 0

task prepare():
  for outer in 0..4:
    for inner in 0..4:
      if outer == 1:
        if inner == 2:
          return
      result += 1

block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
            r#"
proc Loader:
  init:
    pin result: i32 = 0

  task prepare():
    for outer in 0..4:
      for inner in 0..4:
        if outer == 1:
          if inner == 2:
            return
        result += 1

  block:
    await prepare()
    sample:
      out1 = f32(result)

init:
  loader = Loader()
sample:
  out1 = loader()
"#,
        ];

        for source in sources {
            let mut instance = compile_test_instance(source, BLOCK_SIZE, 1);
            let mut output = [99.0_f32; BLOCK_SIZE];
            unsafe {
                bind_output(
                    &mut instance,
                    0,
                    output.as_mut_ptr().cast(),
                    std::mem::size_of_val(&output),
                )
                .expect("output should bind");
            }

            process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
                .expect("task should complete");
            assert_eq!(output, [6.0; BLOCK_SIZE]);
        }
    }

    #[test]
    fn task_symbols_are_injective_and_authored_state_is_explicit() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
init:
  pin foo____onda_bar: i32 = 0
task foo():
  bar_pc: i32 = 7
  yield
  foo____onda_bar = bar_pc

task foo_local_bar():
  yield

block:
  await foo()
  await foo_local_bar()

sample:
  out1 = f32(foo____onda_bar)
"#,
            BLOCK_SIZE,
            1,
        );
        assert_eq!(instance.state_count(), 1);
        assert_eq!(instance.state_name(0), Some("foo____onda_bar"));

        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("first task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("second task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("both tasks should complete");
        assert_eq!(output, [7.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_bindings_preserve_lexical_shadowing_across_yield() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
proc Loader:
  init:
    pin state_value: i32 = 40
    pin result: i32 = 0
  task load():
    state_value: i32 = 2
    if state_value == 2:
      carried: i32 = 3
      yield
      result = carried
    state_value: bool = true
    if state_value:
      result += 10
  block:
    await load()
    sample:
      out1 = f32(result + state_value)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete");
        assert_eq!(output, [53.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_bindings_join_declarations_from_both_if_branches() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
params:
  choose = false
init:
  pin result: i32 = 0
task prepare():
  if choose:
    carried: i32 = 3
  else:
    carried: i32 = 5
  yield
  result = carried
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete");
        assert_eq!(output, [5.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_frame_typing_uses_the_selected_overload() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
def value(x: i32) -> i32:
  return x + 1
def value(x: f64) -> f64:
  return x + 2.0
init:
  pin result: i32 = 0
task prepare():
  carried = value(i32(3))
  yield
  result = carried
block:
  await prepare()
  sample:
    out1 = f32(result)
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete");
        assert_eq!(output, [4.0; BLOCK_SIZE]);
    }

    #[test]
    fn task_mutations_of_aggregate_state_accumulate_across_yield() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
struct Accumulator:
  value: i32 = 0

proc Loader:
  init:
    pin accumulator = Accumulator()
  task load():
    accumulator.value += 1
    yield
    accumulator.value += 1
  block:
    await load()
    sample:
      out1 = f32(accumulator.value)
init:
  loader = Loader()
sample:
  out1 = loader()
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("task should complete");
        assert_eq!(output, [2.0; BLOCK_SIZE]);
    }

    #[test]
    fn tasks_in_runtime_indexed_proc_arrays_activate_lazily_per_slot() {
        const BLOCK_SIZE: usize = 4;
        let mut instance = compile_test_instance(
            r#"
proc Child:
  init:
    pin progress: i32 = 0
  task load():
    progress += 1
    yield
    progress += 10
  block:
    await load()
    sample:
      out1 = f32(progress)
proc Parent:
  init:
    children: Child[2] = Child()
  sample:
    out1 = children[0]() + children[1]()
init:
  parent = Parent()
sample:
  out1 = parent()
"#,
            BLOCK_SIZE,
            1,
        );
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }

        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("both child tasks should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("both child tasks should complete");
        assert_eq!(output, [22.0; BLOCK_SIZE]);

        init(&mut instance, InitMode::PreservePinned)
            .expect("default init should preserve nested task state");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("default init should preserve completed child tasks");
        assert_eq!(output, [22.0; BLOCK_SIZE]);

        init(&mut instance, InitMode::Full).expect("full init should restart nested tasks");
        output.fill(99.0);
        process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none())
            .expect("restarted child tasks should yield");
        assert_eq!(output, [0.0; BLOCK_SIZE]);
    }

    #[test]
    fn failed_task_invalidates_instance_until_full_init_or_restore() {
        const BLOCK_SIZE: usize = 4;
        let parsed = parse_program(
            r#"
proc Loader:
  init:
    bad_zero: i32 = 0
  event retry():
    load.reset()
  task load():
    ignored: i32 = 1 / bad_zero
    yield
  block:
    await load()
    sample:
      out1 = 1.0
init:
  loader = Loader()
event retry():
  loader.retry()
sample:
  out1 = loader()
"#,
        )
        .expect("failing task source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: BLOCK_SIZE,
            },
        )
        .expect("failing task source should analyze");
        let mir = lower_program_to_optimized_mir(&typed).expect("failing task should lower");
        let program = jit_program_from_optimized_mir(mir).expect("failing task MIR should compile");
        let mut instance = create_instance_initialized(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: BLOCK_SIZE,
                in_channels: 0,
                out_channels: 1,
            },
        )
        .expect("failing task instance should initialize");
        let mut output = [99.0_f32; BLOCK_SIZE];
        unsafe {
            bind_output(
                &mut instance,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output),
            )
            .expect("output should bind");
        }
        assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
        output.fill(99.0);
        assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
        assert_eq!(output, [99.0; BLOCK_SIZE]);

        let retry = instance.event_index("retry").expect("retry event");
        assert!(
            trigger_event_by_index(&mut instance, retry, &[], ExecutionOutput::none()).is_err()
        );

        init(&mut instance, InitMode::Full).expect("full init should recover the instance");
        assert!(process_checked(&mut instance, BLOCK_SIZE, ExecutionOutput::none()).is_err());
    }

    #[test]
    fn cloned_programs_process_concurrently_after_the_original_owner_is_dropped() {
        const INSTANCE_COUNT: usize = 8;
        const BLOCK_SIZE: usize = 64;

        let parsed = parse_program("sample:\n  out1 = 0.25\n").expect("source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size: BLOCK_SIZE,
            },
        )
        .expect("source should analyze");
        let mir =
            lower_program_to_optimized_mir(&typed).expect("typed program should lower to MIR");
        let program = jit_program_from_optimized_mir(mir).expect("MIR should compile");
        let config = InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: BLOCK_SIZE,
            in_channels: 0,
            out_channels: 1,
        };
        let instances = (0..INSTANCE_COUNT)
            .map(|_| {
                create_instance_initialized(program.clone(), config)
                    .expect("instance should initialize")
            })
            .collect::<Vec<_>>();

        drop(program);

        let threads = instances
            .into_iter()
            .map(|mut instance| {
                std::thread::spawn(move || {
                    let mut output = vec![0.0_f32; BLOCK_SIZE];
                    unsafe {
                        bind_output(
                            &mut instance,
                            0,
                            output.as_mut_ptr().cast(),
                            std::mem::size_of_val(output.as_slice()),
                        )
                        .expect("output should bind");
                    }
                    prepare_unchecked_process(&mut instance)
                        .expect("unchecked processing should prepare");
                    for _ in 0..32 {
                        unsafe {
                            process_unchecked(&mut instance, ExecutionOutput::none())
                                .expect("concurrent JIT processing should succeed");
                        }
                        assert!(output.iter().all(|sample| *sample == 0.25));
                        output.fill(0.0);
                    }
                })
            })
            .collect::<Vec<_>>();

        for thread in threads {
            thread.join().expect("processing thread should not panic");
        }
    }

    #[test]
    fn checked_bindings_reject_misaligned_primitive_addresses() {
        let storage = [0_u64; 2];
        let aligned = storage.as_ptr().cast::<u8>();
        let misaligned = unsafe { aligned.add(1) };

        for ty in [
            PrimitiveType::F32,
            PrimitiveType::F64,
            PrimitiveType::I32,
            PrimitiveType::I64,
        ] {
            validate_pointer_alignment(aligned, ty, "test", "value")
                .expect("u64 storage should satisfy primitive alignment");
            let error = validate_pointer_alignment(misaligned, ty, "test", "value")
                .expect_err("offset byte pointer must be rejected");
            assert!(error.message.contains("alignment"));
        }
        validate_pointer_alignment(misaligned, PrimitiveType::Bool, "test", "value")
            .expect("byte elements have alignment one");
    }

    #[test]
    fn checked_buffer_bindings_reject_wrapping_element_counts() {
        let error = validate_buffer_byte_extent(i32::MAX, 2, PrimitiveType::F32, "huge")
            .expect_err("buffer byte extent must fit the generated i32 ABI");
        assert!(error.message.contains("exceeds i32 runtime limit"));
        let f64_error =
            validate_buffer_byte_extent(i32::MAX / 8 + 1, 1, PrimitiveType::F64, "wide")
                .expect_err("f64 byte extent must fit i32 even when element count does");
        assert!(f64_error.message.contains("byte extent"));
        assert_eq!(
            validate_buffer_byte_extent(1024, 2, PrimitiveType::F32, "ok").unwrap(),
            8192
        );
    }
}
