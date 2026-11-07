//! Immutable, program-owned descriptors for scalar and fixed-array surfaces.
use super::*;

#[repr(C)]
pub struct onda_value_info_t {
    pub name: *const c_char,
    pub type_text: *const c_char,
    pub element_type: i32,
    pub byte_size: i32,
    pub array_len: i32,
    pub is_array: i32,
    pub slot_offset: i32,
    pub byte_offset: i32,
    pub has_default: i32,
    pub default_f64: f64,
    pub default_bytes: *const u8,
    pub default_byte_count: i32,
    pub has_range: i32,
    pub range_min: f64,
    pub range_max: f64,
    pub scale: i32,
    pub has_curve: i32,
    pub curve: f64,
    pub has_step: i32,
    pub step: f64,
    pub step_count: i64,
    pub unit: *const c_char,
    pub enumeration_json: *const c_char,
}

impl Default for onda_value_info_t {
    fn default() -> Self {
        Self {
            name: ptr::null(),
            type_text: ptr::null(),
            element_type: -1,
            byte_size: 0,
            array_len: 1,
            is_array: 0,
            slot_offset: -1,
            byte_offset: 0,
            has_default: 0,
            default_f64: f64::NAN,
            default_bytes: ptr::null(),
            default_byte_count: 0,
            has_range: 0,
            range_min: f64::NAN,
            range_max: f64::NAN,
            scale: ONDA_PARAM_SCALE_NONE,
            has_curve: 0,
            curve: f64::NAN,
            has_step: 0,
            step: f64::NAN,
            step_count: 0,
            unit: ptr::null(),
            enumeration_json: ptr::null(),
        }
    }
}

#[derive(Default)]
pub(super) struct ValueInfo {
    surfaces: [Vec<onda_value_info_t>; 5],
    strings: Vec<CString>,
    // Descriptors borrow stable plan addresses while this vector grows.
    #[allow(clippy::vec_box)]
    payload_plans: Vec<Box<onda_payload_plan_t>>,
    buffers: Vec<onda_buffer_info_t>,
    buffer_arrays: Vec<onda_buffer_array_info_t>,
    events: Vec<onda_message_info_t>,
    delegates: Vec<onda_message_info_t>,
}

// SAFETY: records are immutable after construction. Every pointer refers to
// immutable storage owned by this cache or its enclosing CompiledProgram.
// No record is published until construction finishes; the program outlives all borrows.
unsafe impl Send for ValueInfo {}
unsafe impl Sync for ValueInfo {}

impl ValueInfo {
    pub(super) fn build(program: &CompiledProgram) -> Result<Self, Diagnostic> {
        let mut result = Self::default();
        for (surface, (values, names, types)) in [
            (
                program.jit.inputs(),
                &program.input_names,
                &program.input_types,
            ),
            (
                program.jit.outputs(),
                &program.output_names,
                &program.output_types,
            ),
            (
                program.jit.control_outputs(),
                &program.control_output_names,
                &program.control_output_types,
            ),
            (
                program.jit.params(),
                &program.param_names,
                &program.param_types,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            for (index, value) in values.iter().enumerate() {
                let domain = value.param_domain();
                let unit =
                    result.string(domain.and_then(|domain| domain.unit()).map(str::to_owned))?;
                let enumeration_json = result.string(
                    value
                        .enumeration()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|error| Diagnostic::internal(error.to_string()))?,
                )?;
                result.surfaces[surface].push(onda_value_info_t {
                    name: names[index].as_ptr(),
                    type_text: types[index].as_ptr(),
                    element_type: primitive_type_to_i32(value.elem_ty()),
                    byte_size: saturating_usize_to_i32(value.byte_size()),
                    array_len: saturating_usize_to_i32(value.array_len()),
                    is_array: i32::from(value.is_array()),
                    slot_offset: saturating_usize_to_i32(value.slot_offset()),
                    byte_offset: saturating_usize_to_i32(value.byte_offset()),
                    has_default: i32::from(value.has_default()),
                    default_f64: value.default_as_f64().unwrap_or(f64::NAN),
                    default_bytes: value
                        .default_bytes()
                        .map_or(ptr::null(), |bytes| bytes.as_ptr()),
                    default_byte_count: value
                        .default_bytes()
                        .map_or(0, |bytes| saturating_usize_to_i32(bytes.len())),
                    has_range: i32::from(value.has_range()),
                    range_min: value.range_min_as_f64().unwrap_or(f64::NAN),
                    range_max: value.range_max_as_f64().unwrap_or(f64::NAN),
                    scale: match domain.map(|domain| domain.scale()) {
                        None => ONDA_PARAM_SCALE_NONE,
                        Some(ParamScale::Linear) => ONDA_PARAM_SCALE_LINEAR,
                        Some(ParamScale::Log) => ONDA_PARAM_SCALE_LOG,
                    },
                    has_curve: i32::from(domain.is_some_and(|domain| domain.curve().is_some())),
                    curve: domain.and_then(|domain| domain.curve()).unwrap_or(f64::NAN),
                    has_step: i32::from(domain.is_some_and(|domain| domain.step_count().is_some())),
                    step: domain.and_then(|domain| domain.step()).unwrap_or(f64::NAN),
                    step_count: domain
                        .and_then(|domain| domain.step_count())
                        .map_or(0, i64::from),
                    unit,
                    enumeration_json,
                });
            }
        }
        for (index, value) in program.jit.state_entries().iter().enumerate() {
            result.surfaces[4].push(onda_value_info_t {
                name: program.state_names[index].as_ptr(),
                type_text: program.state_types[index].as_ptr(),
                element_type: primitive_type_to_i32(value.elem_ty()),
                byte_size: saturating_usize_to_i32(value.byte_size()),
                array_len: saturating_usize_to_i32(value.array_len()),
                is_array: i32::from(value.is_array()),
                byte_offset: saturating_usize_to_i32(value.byte_offset()),
                ..Default::default()
            });
        }
        for (index, value) in program.jit.buffers().iter().enumerate() {
            result.buffers.push(onda_buffer_info_t {
                name: program.buffer_names[index].as_ptr(),
                type_text: program.buffer_types[index].as_ptr(),
                element_type: primitive_type_to_i32(value.elem_ty()),
                element_bytes: primitive_type_bytes(value.elem_ty()) as i32,
                channels_kind: decl_buffer_channels_kind_to_i32(value.channels()),
                channels_static: match value.channels() {
                    DeclaredBufferChannels::Mono => 1,
                    DeclaredBufferChannels::Static(n) => saturating_usize_to_i32(n),
                    DeclaredBufferChannels::Dynamic => -1,
                },
                may_write: i32::from(value.may_write()),
            });
        }
        for (index, value) in program.jit.buffer_arrays().iter().enumerate() {
            result.buffer_arrays.push(onda_buffer_array_info_t {
                name: program.buffer_array_names[index].as_ptr(),
                first: saturating_usize_to_i32(value.first()),
                len: saturating_usize_to_i32(value.len()),
            });
        }
        for index in 0..program.jit.event_count() {
            let plan = program.jit.event_descriptor(index).unwrap().payload_plan();
            let payload_plan = Box::new(onda_payload_plan_t::new(plan)?);
            let payload_plan_ptr = &*payload_plan as *const _;
            result.payload_plans.push(payload_plan);
            result.events.push(onda_message_info_t {
                name: program.event_names[index].as_ptr(),
                schema_json: program.event_schema_json[index].as_ptr(),
                payload_bytes: program
                    .jit
                    .event_payload_bytes(index)
                    .map_or(-1, saturating_usize_to_i32),
                payload_min_bytes: program
                    .jit
                    .event_payload_min_bytes(index)
                    .map_or(-1, saturating_usize_to_i32),
                record_bytes: -1,
                record_min_bytes: -1,
                tensor_count: saturating_usize_to_i32(plan.tensors().len()),
                region_count: saturating_usize_to_i32(plan.region_count()),
                payload_plan: payload_plan_ptr,
            });
        }
        for index in 0..program.jit.delegate_count() {
            let plan = program
                .jit
                .delegate_descriptor(index)
                .unwrap()
                .payload_plan();
            let payload_plan = Box::new(onda_payload_plan_t::new(plan)?);
            let payload_plan_ptr = &*payload_plan as *const _;
            result.payload_plans.push(payload_plan);
            result.delegates.push(onda_message_info_t {
                name: program.delegate_names[index].as_ptr(),
                schema_json: program.delegate_schema_json[index].as_ptr(),
                payload_bytes: program
                    .jit
                    .delegate_payload_bytes(index)
                    .map_or(-1, saturating_usize_to_i32),
                payload_min_bytes: program
                    .jit
                    .delegate_payload_min_bytes(index)
                    .map_or(-1, saturating_usize_to_i32),
                record_bytes: program
                    .jit
                    .delegate_record_bytes(index)
                    .map_or(-1, saturating_usize_to_i32),
                record_min_bytes: program
                    .jit
                    .delegate_record_min_bytes(index)
                    .map_or(-1, saturating_usize_to_i32),
                tensor_count: saturating_usize_to_i32(plan.tensors().len()),
                region_count: saturating_usize_to_i32(plan.region_count()),
                payload_plan: payload_plan_ptr,
            });
        }
        Ok(result)
    }

    fn string(&mut self, text: Option<String>) -> Result<*const c_char, Diagnostic> {
        let Some(text) = text else {
            return Ok(ptr::null());
        };
        let string =
            CString::new(text).map_err(|_| Diagnostic::internal("metadata contains NUL"))?;
        let pointer = string.as_ptr();
        self.strings.push(string);
        Ok(pointer)
    }
}

macro_rules! value_info {
    ($name:ident, $surface:expr) => {
        /// Borrows a descriptor until program destruction; null denotes an invalid index/handle.
        #[no_mangle]
        pub unsafe extern "C" fn $name(
            program: *const onda_program,
            index: i32,
        ) -> *const onda_value_info_t {
            program
                .as_ref()
                .and_then(|program| {
                    program.inner.value_info.surfaces[$surface].get(usize::try_from(index).ok()?)
                })
                .map_or(ptr::null(), |value| value)
        }
    };
}
value_info!(onda_input_info, 0);
value_info!(onda_output_info, 1);
value_info!(onda_control_output_info, 2);
value_info!(onda_param_info, 3);
value_info!(onda_state_info, 4);

#[repr(C)]
pub struct onda_buffer_info_t {
    pub name: *const c_char,
    pub type_text: *const c_char,
    pub element_type: i32,
    pub element_bytes: i32,
    pub channels_kind: i32,
    pub channels_static: i32,
    pub may_write: i32,
}

#[repr(C)]
pub struct onda_buffer_array_info_t {
    pub name: *const c_char,
    pub first: i32,
    pub len: i32,
}

#[repr(C)]
pub struct onda_message_info_t {
    pub name: *const c_char,
    pub schema_json: *const c_char,
    pub payload_bytes: i32,
    pub payload_min_bytes: i32,
    pub record_bytes: i32,
    pub record_min_bytes: i32,
    pub tensor_count: i32,
    pub region_count: i32,
    pub payload_plan: *const onda_payload_plan_t,
}

/// Borrows immutable metadata until program destruction; null means invalid handle/index.
#[no_mangle]
pub unsafe extern "C" fn onda_buffer_info(
    program: *const onda_program,
    index: i32,
) -> *const onda_buffer_info_t {
    program
        .as_ref()
        .and_then(|program| {
            program
                .inner
                .value_info
                .buffers
                .get(usize::try_from(index).ok()?)
        })
        .map_or(ptr::null(), |info| info)
}

/// Borrows immutable metadata until program destruction; null means invalid handle/index.
#[no_mangle]
pub unsafe extern "C" fn onda_buffer_array_info(
    program: *const onda_program,
    index: i32,
) -> *const onda_buffer_array_info_t {
    program
        .as_ref()
        .and_then(|program| {
            program
                .inner
                .value_info
                .buffer_arrays
                .get(usize::try_from(index).ok()?)
        })
        .map_or(ptr::null(), |info| info)
}

/// Borrows immutable metadata until program destruction; null means invalid handle/index.
#[no_mangle]
pub unsafe extern "C" fn onda_event_info(
    program: *const onda_program,
    index: i32,
) -> *const onda_message_info_t {
    program
        .as_ref()
        .and_then(|program| {
            program
                .inner
                .value_info
                .events
                .get(usize::try_from(index).ok()?)
        })
        .map_or(ptr::null(), |info| info)
}

/// Borrows immutable metadata until program destruction; null means invalid handle/index.
#[no_mangle]
pub unsafe extern "C" fn onda_delegate_info(
    program: *const onda_program,
    index: i32,
) -> *const onda_message_info_t {
    program
        .as_ref()
        .and_then(|program| {
            program
                .inner
                .value_info
                .delegates
                .get(usize::try_from(index).ok()?)
        })
        .map_or(ptr::null(), |info| info)
}
