pub const PROC_INDEX_BUFFER_SELECT_SENTINEL: &str =
    crate::proc_state_rewrite::PROC_INDEX_BUFFER_SELECT_SENTINEL;
pub const PROC_INDEX_CALL_SENTINEL: &str = crate::proc_state_rewrite::PROC_INDEX_CALL_SENTINEL;
pub const PROC_INDEX_BASE_ARG: &str = crate::proc_state_rewrite::PROC_INDEX_BASE_ARG;
pub const PROC_INDEX_EXPR_ARG: &str = crate::proc_state_rewrite::PROC_INDEX_EXPR_ARG;
pub use onda_frontend::METHOD_RECEIVER_ARG;

pub fn is_compiler_generated_function_name(name: &str) -> bool {
    name.contains(".__onda_proc_") || name.contains(".__onda_mono") || name.starts_with("__onda_")
}

pub fn sanitize_runtime_symbol_component(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
}

/// Produces a readable, injective identifier component for compiler-owned
/// symbols. Sanitization alone is not sufficient: `A::B` and `A__B`, for
/// example, have the same sanitized spelling.
pub(crate) fn encode_internal_symbol_component(name: &str) -> String {
    let readable = sanitize_runtime_symbol_component(name);
    let mut encoded = String::with_capacity(readable.len() + 2 * name.len() + 16);
    encoded.push_str(&readable);
    encoded.push_str("__id_");
    encoded.push_str(&name.len().to_string());
    encoded.push('_');
    for byte in name.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn collision_safe_runtime_symbol_component(name: &str) -> String {
    let sanitized = sanitize_runtime_symbol_component(name);
    if sanitized == name {
        sanitized
    } else {
        encode_internal_symbol_component(name)
    }
}

pub fn runtime_proc_array_active_symbol(array_base: &str) -> String {
    format!(
        "__onda_proc_block_active_{}",
        collision_safe_runtime_symbol_component(array_base)
    )
}

pub fn runtime_buffer_alias_selector_symbol(alias: &str) -> String {
    format!(
        "__onda_buffer_alias_selector_{}",
        collision_safe_runtime_symbol_component(alias)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_runtime_symbol_component_replaces_non_identifier_chars() {
        assert_eq!(
            sanitize_runtime_symbol_component("voice.bank[3].left-out"),
            "voice_bank_3__left_out"
        );
    }

    #[test]
    fn runtime_proc_array_active_symbol_uses_stable_prefix_and_sanitized_base() {
        let symbol = runtime_proc_array_active_symbol("voice.bank[3].left-out");
        assert!(symbol.starts_with("__onda_proc_block_active_voice_bank_3__left_out__id_"));
        assert_ne!(
            runtime_proc_array_active_symbol("A::voices"),
            runtime_proc_array_active_symbol("A__voices")
        );
        assert_eq!(
            runtime_proc_array_active_symbol("voices"),
            "__onda_proc_block_active_voices"
        );
    }

    #[test]
    fn compiler_generated_function_names_cover_proc_helpers_and_internal_defs() {
        assert!(is_compiler_generated_function_name(
            "Voice.__onda_proc_step"
        ));
        assert!(is_compiler_generated_function_name("__onda_read_slot"));
        assert!(!is_compiler_generated_function_name("Voice.__proc_helper"));
        assert!(!is_compiler_generated_function_name("Voice.process"));
    }

    #[test]
    fn internal_symbol_components_are_injective_after_sanitization() {
        let namespaced = encode_internal_symbol_component("A::B");
        let underscored = encode_internal_symbol_component("A__B");
        assert_ne!(namespaced, underscored);
        assert!(namespaced.starts_with("A__B__id_"));
    }
}
