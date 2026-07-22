fn set_param_f32(instance: &mut onda_runtime::Instance, name: &str, value: f32) {
    let idx = instance
        .param_index(name)
        .unwrap_or_else(|| panic!("missing parameter '{name}'"));
    let bytes = value.to_ne_bytes();
    set_param_by_index(instance, idx, &bytes).expect("param update should succeed");
}

fn set_param_f32_array(instance: &mut onda_runtime::Instance, name: &str, values: &[f32]) {
    let idx = instance
        .param_index(name)
        .unwrap_or_else(|| panic!("missing parameter '{name}'"));
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for v in values {
        bytes.extend_from_slice(&v.to_ne_bytes());
    }
    set_param_by_index(instance, idx, &bytes).expect("array param update should succeed");
}
