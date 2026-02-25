use std::collections::{HashMap, HashSet};

use omni_frontend::PrimitiveType;

pub(crate) const DECLARED_INPUT_TYPE_PREFIX: &str = "__omni_decl_input_ty__";
pub(crate) const DECLARED_OUTPUT_TYPE_PREFIX: &str = "__omni_decl_output_ty__";
pub(crate) const DECLARED_PARAM_TYPE_PREFIX: &str = "__omni_decl_param_ty__";
pub(crate) const DECLARED_DATA_ELEM_TYPE_PREFIX: &str = "__omni_decl_data_elem_ty__";
pub(crate) const DECLARED_BUFFER_ELEM_TYPE_PREFIX: &str = "__omni_decl_buffer_elem_ty__";
pub(crate) const DECLARED_STRUCT_FIELD_TYPE_PREFIX: &str = "__omni_decl_struct_field_ty__";
pub(crate) const DECLARED_INVALID_PLACEHOLDER_PREFIX: &str = "__omni_decl_invalid_placeholder__";
pub(crate) const DECLARED_FUNCTION_RETURN_TYPE_PREFIX: &str = "__omni_decl_fn_ret_ty__";
pub(crate) const DECLARED_BUFFER_MULTICHANNEL_PREFIX: &str = "__omni_decl_buffer_multich__";
pub(crate) const DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX: &str = "__omni_decl_buffer_dynch__";
pub(crate) const DECLARED_BUFFER_STATIC_CHANNELS_PREFIX: &str = "__omni_decl_buffer_stch__";
pub(crate) const DECLARED_BUFFER_ELEM_F32_PREFIX: &str = "__omni_decl_buffer_elem_f32__";
pub(crate) const DECLARED_BUFFER_ELEM_F64_PREFIX: &str = "__omni_decl_buffer_elem_f64__";
pub(crate) const DECLARED_BUFFER_ELEM_I32_PREFIX: &str = "__omni_decl_buffer_elem_i32__";
pub(crate) const DECLARED_BUFFER_ELEM_I64_PREFIX: &str = "__omni_decl_buffer_elem_i64__";
pub(crate) const DECLARED_BUFFER_ELEM_BOOL_PREFIX: &str = "__omni_decl_buffer_elem_bool__";

pub(crate) fn declared_type_key(prefix: &str, name: &str) -> String {
    format!("{prefix}{name}")
}

pub(crate) fn set_declared_symbol_types(
    state_scalars: &mut HashMap<String, PrimitiveType>,
    names: &HashSet<String>,
    types: &HashMap<String, PrimitiveType>,
    key_prefix: &str,
) {
    for name in names {
        let ty = *types.get(name).unwrap_or(&PrimitiveType::F32);
        state_scalars.insert(declared_type_key(key_prefix, name), ty);
    }
}

pub(crate) fn get_declared_symbol_type(
    state_scalars: &HashMap<String, PrimitiveType>,
    name: &str,
    key_prefix: &str,
) -> Option<PrimitiveType> {
    state_scalars
        .get(&declared_type_key(key_prefix, name))
        .copied()
}

pub(crate) fn has_declared_buffer_symbol(known_scalars: &HashSet<String>, name: &str) -> bool {
    known_scalars.contains(&declared_type_key(DECLARED_BUFFER_ELEM_TYPE_PREFIX, name))
}

pub(crate) fn is_declared_multichannel_buffer_symbol(
    known_scalars: &HashSet<String>,
    name: &str,
) -> bool {
    known_scalars.contains(&declared_type_key(
        DECLARED_BUFFER_MULTICHANNEL_PREFIX,
        name,
    ))
}

pub(crate) fn buffer_elem_decl_prefix(elem_ty: PrimitiveType) -> &'static str {
    match elem_ty {
        PrimitiveType::F32 => DECLARED_BUFFER_ELEM_F32_PREFIX,
        PrimitiveType::F64 => DECLARED_BUFFER_ELEM_F64_PREFIX,
        PrimitiveType::I32 => DECLARED_BUFFER_ELEM_I32_PREFIX,
        PrimitiveType::I64 => DECLARED_BUFFER_ELEM_I64_PREFIX,
        PrimitiveType::Bool => DECLARED_BUFFER_ELEM_BOOL_PREFIX,
    }
}

pub(crate) fn declared_buffer_static_channels_key(name: &str, channels: usize) -> String {
    format!("{DECLARED_BUFFER_STATIC_CHANNELS_PREFIX}{name}__{channels}")
}

pub(crate) fn has_declared_buffer_elem_type(
    known_scalars: &HashSet<String>,
    name: &str,
    elem_ty: PrimitiveType,
) -> bool {
    known_scalars.contains(&declared_type_key(buffer_elem_decl_prefix(elem_ty), name))
}

pub(crate) fn has_declared_dynamic_buffer_channels(
    known_scalars: &HashSet<String>,
    name: &str,
) -> bool {
    known_scalars.contains(&declared_type_key(
        DECLARED_BUFFER_DYNAMIC_CHANNELS_PREFIX,
        name,
    ))
}

pub(crate) fn declared_static_buffer_channels(
    known_scalars: &HashSet<String>,
    name: &str,
) -> Option<usize> {
    let prefix = format!("{DECLARED_BUFFER_STATIC_CHANNELS_PREFIX}{name}__");
    for symbol in known_scalars {
        if let Some(ch) = symbol.strip_prefix(&prefix) {
            if let Ok(parsed) = ch.parse::<usize>() {
                return Some(parsed);
            }
        }
    }
    None
}
