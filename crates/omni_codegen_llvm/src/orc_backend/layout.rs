use std::collections::HashMap;

use omni_frontend::{Diagnostic, PrimitiveType};
use omni_semantics::TypedProgram;

#[derive(Debug, Clone)]
pub(super) struct StateLayoutEntry {
    pub(super) name: String,
    pub(super) ty: PrimitiveType,
    pub(super) offset: usize,
}

#[derive(Debug, Clone)]
pub(super) struct ArrayLayoutEntry {
    pub(super) name: String,
    pub(super) elem_ty: PrimitiveType,
    pub(super) len: usize,
    pub(super) offset: usize,
}

pub(super) fn align_up(value: usize, align: usize) -> usize {
    if align <= 1 {
        return value;
    }
    let rem = value % align;
    if rem == 0 {
        value
    } else {
        value + (align - rem)
    }
}

pub(super) fn primitive_size_align(ty: PrimitiveType) -> (usize, usize) {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => (4, 4),
        PrimitiveType::F64 | PrimitiveType::I64 => (8, 8),
        PrimitiveType::Bool => (1, 1),
    }
}

pub(super) fn compute_state_layout(
    typed: &TypedProgram,
) -> Result<Vec<StateLayoutEntry>, Diagnostic> {
    if typed.state_vars.len() != typed.state_types.len() {
        return Err(Diagnostic::internal(format!(
            "typed program state metadata mismatch: {} vars but {} types",
            typed.state_vars.len(),
            typed.state_types.len()
        )));
    }
    let mut layout = Vec::with_capacity(typed.state_vars.len());
    let mut offset = 0usize;
    for (name, ty) in typed.state_vars.iter().zip(typed.state_types.iter()) {
        let (size, align) = primitive_size_align(*ty);
        offset = align_up(offset, align);
        layout.push(StateLayoutEntry {
            name: name.clone(),
            ty: *ty,
            offset,
        });
        offset = offset.saturating_add(size);
    }
    Ok(layout)
}

pub(super) fn compute_arrays_layout(
    typed: &TypedProgram,
    state_layout: &[StateLayoutEntry],
) -> Result<Vec<ArrayLayoutEntry>, Diagnostic> {
    let mut layout = Vec::with_capacity(typed.array_vars.len());
    let mut offset = state_total_size_bytes(state_layout, &[]);
    for array_var in &typed.array_vars {
        if array_var.len == 0 {
            return Err(Diagnostic::internal(format!(
                "array symbol '{}' cannot have zero length in ORC layout",
                array_var.name
            )));
        }
        let (elem_size, elem_align) = primitive_size_align(array_var.elem_ty);
        offset = align_up(offset, elem_align);
        layout.push(ArrayLayoutEntry {
            name: array_var.name.clone(),
            elem_ty: array_var.elem_ty,
            len: array_var.len,
            offset,
        });
        let byte_len = elem_size.checked_mul(array_var.len).ok_or_else(|| {
            Diagnostic::internal(format!(
                "array symbol '{}' byte size overflow in ORC layout",
                array_var.name
            ))
        })?;
        offset = offset.saturating_add(byte_len);
    }
    Ok(layout)
}

pub(super) fn state_total_size_bytes(
    layout: &[StateLayoutEntry],
    arrays: &[ArrayLayoutEntry],
) -> usize {
    let mut total = 0usize;
    for entry in layout {
        let (size, _) = primitive_size_align(entry.ty);
        total = total.max(entry.offset.saturating_add(size));
    }
    for entry in arrays {
        let (elem_size, _) = primitive_size_align(entry.elem_ty);
        total = total.max(
            entry
                .offset
                .saturating_add(elem_size.saturating_mul(entry.len)),
        );
    }
    total
}

pub(super) fn state_layout_map(
    layout: &[StateLayoutEntry],
) -> HashMap<String, (PrimitiveType, usize)> {
    layout
        .iter()
        .map(|e| (e.name.clone(), (e.ty, e.offset)))
        .collect()
}

pub(super) fn arrays_layout_map(
    layout: &[ArrayLayoutEntry],
) -> HashMap<String, (PrimitiveType, usize)> {
    layout
        .iter()
        .map(|e| (e.name.clone(), (e.elem_ty, e.offset)))
        .collect()
}
