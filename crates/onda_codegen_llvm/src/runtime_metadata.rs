#[cfg(any(feature = "llvm-orc", test))]
use std::collections::HashMap;

use onda_frontend::PrimitiveType;
use onda_mir::{ParamControl, ParamScale, ScalarValue, ValueRange};

use crate::primitives::{primitive_type_bytes, primitive_type_name, scalar_value_to_f64};
use crate::{
    DeclaredBuffer, DeclaredBufferChannels, DeclaredEvent, DeclaredEventParam, DeclaredIo,
    DeclaredState,
};

#[cfg(any(feature = "llvm-orc", test))]
pub(crate) struct ProgramMetadata {
    pub(crate) inputs: Vec<DeclaredIo>,
    pub(crate) outputs: Vec<DeclaredIo>,
    pub(crate) control_outputs: Vec<DeclaredIo>,
    pub(crate) params: Vec<DeclaredIo>,
    pub(crate) events: Vec<DeclaredEvent>,
    pub(crate) buffers: Vec<DeclaredBuffer>,
    pub(crate) state_entries: Vec<DeclaredState>,
    pub(crate) input_index: HashMap<String, usize>,
    pub(crate) output_index: HashMap<String, usize>,
    pub(crate) control_output_index: HashMap<String, usize>,
    pub(crate) param_index: HashMap<String, usize>,
    pub(crate) event_index: HashMap<String, usize>,
    pub(crate) buffer_index: HashMap<String, usize>,
}

impl DeclaredIo {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn elem_ty(&self) -> PrimitiveType {
        self.elem_ty
    }

    pub fn array_len(&self) -> usize {
        self.array_len
    }

    pub fn is_array(&self) -> bool {
        self.is_array
    }

    pub fn slot_offset(&self) -> usize {
        self.slot_offset
    }

    pub fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    pub(crate) fn state_byte_offset(&self) -> Option<usize> {
        self.state_byte_offset
    }

    pub fn default(&self) -> Option<ScalarValue> {
        if self.is_array {
            None
        } else {
            self.default_values.as_deref()?.first().copied()
        }
    }

    pub fn has_default(&self) -> bool {
        self.default_bytes.is_some()
    }

    pub fn default_bytes(&self) -> Option<&[u8]> {
        self.default_bytes.as_deref()
    }

    pub fn default_values(&self) -> Option<&[ScalarValue]> {
        self.default_values.as_deref()
    }

    pub fn default_as_f64(&self) -> Option<f64> {
        self.default().map(scalar_value_to_f64)
    }

    pub fn has_range(&self) -> bool {
        self.range.is_some()
    }

    pub fn range(&self) -> Option<ValueRange> {
        self.range
    }

    pub fn range_min_as_f64(&self) -> Option<f64> {
        self.range.map(|r| scalar_value_to_f64(r.min))
    }

    pub fn range_max_as_f64(&self) -> Option<f64> {
        self.range.map(|r| scalar_value_to_f64(r.max))
    }

    pub fn param_control(&self) -> Option<&ParamControl> {
        self.control.as_ref()
    }

    pub fn param_scale(&self) -> Option<ParamScale> {
        self.control.as_ref().map(|control| control.scale)
    }

    pub fn param_scale_name(&self) -> Option<&'static str> {
        Some(match self.param_scale()? {
            ParamScale::Linear => "linear",
            ParamScale::Log => "log",
        })
    }

    pub fn param_unit(&self) -> Option<&str> {
        self.control.as_ref()?.unit.as_deref()
    }

    pub fn param_step_as_f64(&self) -> Option<f64> {
        self.control.as_ref()?.step.map(ScalarValue::as_f64)
    }

    pub fn param_step_count(&self) -> Option<u32> {
        self.control.as_ref()?.step_count
    }

    pub fn constrain_param_plain(&self, plain: f64) -> Option<f64> {
        Some(self.control.as_ref()?.constrain_plain(self.range?, plain))
    }

    pub fn param_normalized_to_plain(&self, normalized: f64) -> Option<f64> {
        Some(
            self.control
                .as_ref()?
                .normalized_to_plain(self.range?, normalized),
        )
    }

    pub fn param_plain_to_normalized(&self, plain: f64) -> Option<f64> {
        Some(
            self.control
                .as_ref()?
                .plain_to_normalized(self.range?, plain),
        )
    }

    pub fn type_repr(&self) -> String {
        if self.is_array {
            format!("{}[{}]", primitive_type_name(self.elem_ty), self.array_len)
        } else {
            primitive_type_name(self.elem_ty).to_owned()
        }
    }

    pub fn byte_size(&self) -> usize {
        primitive_type_bytes(self.elem_ty).saturating_mul(self.array_len)
    }
}

impl DeclaredState {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn elem_ty(&self) -> PrimitiveType {
        self.elem_ty
    }

    pub fn array_len(&self) -> usize {
        self.array_len
    }

    pub fn is_array(&self) -> bool {
        self.is_array
    }

    pub fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    pub fn storage_byte_offset(&self) -> usize {
        self.storage_byte_offset
    }

    pub fn byte_size(&self) -> usize {
        primitive_type_bytes(self.elem_ty).saturating_mul(self.array_len)
    }

    pub fn type_repr(&self) -> String {
        if self.is_array {
            format!("{}[{}]", primitive_type_name(self.elem_ty), self.array_len)
        } else {
            primitive_type_name(self.elem_ty).to_owned()
        }
    }
}

impl DeclaredBuffer {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn elem_ty(&self) -> PrimitiveType {
        self.elem_ty
    }

    pub fn channels(&self) -> DeclaredBufferChannels {
        self.channels
    }

    pub fn access(&self) -> onda_mir::AccessMode {
        self.access
    }

    pub fn may_write(&self) -> bool {
        self.access == onda_mir::AccessMode::ReadWrite
    }

    pub fn type_repr(&self) -> String {
        let elem = primitive_type_name(self.elem_ty);
        match self.channels {
            DeclaredBufferChannels::Mono => format!("buffer[{elem}]"),
            DeclaredBufferChannels::Static(ch) => format!("buffer[{elem}[{ch}]]"),
            DeclaredBufferChannels::Dynamic => format!("buffer[{elem}[]]"),
        }
    }
}

impl DeclaredEvent {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn params(&self) -> &[DeclaredEventParam] {
        &self.params
    }

    pub fn payload_bytes(&self) -> Option<usize> {
        self.payload_bytes
    }
}

impl DeclaredEventParam {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn elem_ty(&self) -> PrimitiveType {
        self.elem_ty
    }

    pub fn array_len(&self) -> usize {
        self.array_len
    }

    pub fn is_array(&self) -> bool {
        self.is_array
    }

    pub fn is_slice(&self) -> bool {
        self.is_slice
    }

    pub fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    pub fn has_default(&self) -> bool {
        self.default_bytes.is_some()
    }

    pub fn default_bytes(&self) -> Option<&[u8]> {
        self.default_bytes.as_deref()
    }

    pub fn default_values(&self) -> Option<&[ScalarValue]> {
        self.default_values.as_deref()
    }

    pub fn type_repr(&self) -> String {
        if self.is_slice {
            return format!("{}[]", primitive_type_name(self.elem_ty));
        }
        if self.is_array {
            format!("{}[{}]", primitive_type_name(self.elem_ty), self.array_len)
        } else {
            primitive_type_name(self.elem_ty).to_owned()
        }
    }

    pub fn byte_size(&self) -> Option<usize> {
        if self.is_slice {
            return None;
        }
        Some(primitive_type_bytes(self.elem_ty).saturating_mul(self.array_len))
    }
}
