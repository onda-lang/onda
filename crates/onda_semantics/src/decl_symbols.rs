use std::collections::{HashMap, HashSet};

use onda_frontend::PrimitiveType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BufferChannelInfo {
    Mono,
    Static(usize),
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclaredSymbolInfo {
    Input {
        ty: PrimitiveType,
    },
    Output {
        ty: PrimitiveType,
    },
    Param {
        ty: PrimitiveType,
    },
    DataArray {
        elem_ty: PrimitiveType,
    },
    Buffer {
        elem_ty: PrimitiveType,
        channels: BufferChannelInfo,
    },
    #[allow(dead_code)]
    StructField {
        ty: PrimitiveType,
    },
    FunctionReturn {
        ty: PrimitiveType,
    },
    InvalidPlaceholder,
}

pub(crate) type DeclaredSymbolMap = HashMap<String, DeclaredSymbolInfo>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclaredScalarSymbolKind {
    Input,
    Output,
    Param,
}

fn declared_scalar_symbol_info(
    kind: DeclaredScalarSymbolKind,
    ty: PrimitiveType,
) -> DeclaredSymbolInfo {
    match kind {
        DeclaredScalarSymbolKind::Input => DeclaredSymbolInfo::Input { ty },
        DeclaredScalarSymbolKind::Output => DeclaredSymbolInfo::Output { ty },
        DeclaredScalarSymbolKind::Param => DeclaredSymbolInfo::Param { ty },
    }
}

pub(crate) fn insert_declared_symbol(
    _state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &mut DeclaredSymbolMap,
    name: impl Into<String>,
    info: DeclaredSymbolInfo,
) {
    declared_symbols.insert(name.into(), info);
}

pub(crate) fn set_declared_symbol_types(
    _state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &mut DeclaredSymbolMap,
    names: &HashSet<String>,
    types: &HashMap<String, PrimitiveType>,
    kind: DeclaredScalarSymbolKind,
) {
    for name in names {
        let ty = *types.get(name).unwrap_or(&PrimitiveType::F32);
        declared_symbols.insert(name.clone(), declared_scalar_symbol_info(kind, ty));
    }
}

pub(crate) fn declared_symbol_scalar_type(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
) -> Option<PrimitiveType> {
    match declared_symbols.get(name) {
        Some(DeclaredSymbolInfo::Input { ty })
        | Some(DeclaredSymbolInfo::Output { ty })
        | Some(DeclaredSymbolInfo::Param { ty })
        | Some(DeclaredSymbolInfo::StructField { ty })
        | Some(DeclaredSymbolInfo::FunctionReturn { ty }) => Some(*ty),
        Some(DeclaredSymbolInfo::DataArray { elem_ty }) => Some(*elem_ty),
        Some(DeclaredSymbolInfo::Buffer { elem_ty, .. }) => Some(*elem_ty),
        Some(DeclaredSymbolInfo::InvalidPlaceholder) | None => None,
    }
}

pub(crate) fn declared_buffer_info(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
) -> Option<(PrimitiveType, BufferChannelInfo)> {
    match declared_symbols.get(name) {
        Some(DeclaredSymbolInfo::Buffer { elem_ty, channels }) => Some((*elem_ty, *channels)),
        _ => None,
    }
}

pub(crate) fn has_declared_buffer_symbol_info(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
) -> bool {
    declared_buffer_info(declared_symbols, name).is_some()
}

pub(crate) fn is_declared_multichannel_buffer_info(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
) -> bool {
    matches!(
        declared_buffer_info(declared_symbols, name),
        Some((_, BufferChannelInfo::Static(ch))) if ch > 1
    ) || matches!(
        declared_buffer_info(declared_symbols, name),
        Some((_, BufferChannelInfo::Dynamic))
    )
}

pub(crate) fn has_declared_buffer_elem_type_info(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
    elem_ty: PrimitiveType,
) -> bool {
    matches!(
        declared_buffer_info(declared_symbols, name),
        Some((actual_ty, _)) if actual_ty == elem_ty
    )
}

pub(crate) fn has_declared_dynamic_buffer_channels_info(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
) -> bool {
    matches!(
        declared_buffer_info(declared_symbols, name),
        Some((_, BufferChannelInfo::Dynamic))
    )
}

pub(crate) fn declared_static_buffer_channels_info(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
) -> Option<usize> {
    match declared_buffer_info(declared_symbols, name) {
        Some((_, BufferChannelInfo::Static(ch))) if ch > 1 => Some(ch),
        _ => None,
    }
}

pub(crate) fn is_declared_data_array_symbol(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
) -> bool {
    matches!(
        declared_symbols.get(name),
        Some(DeclaredSymbolInfo::DataArray { .. })
    )
}

pub(crate) fn is_invalid_placeholder_symbol(
    declared_symbols: &DeclaredSymbolMap,
    name: &str,
) -> bool {
    matches!(
        declared_symbols.get(name),
        Some(DeclaredSymbolInfo::InvalidPlaceholder)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_declared_buffer_symbol_populates_typed_map() {
        let mut state_scalars = HashMap::new();
        let mut declared_symbols = DeclaredSymbolMap::new();

        insert_declared_symbol(
            &mut state_scalars,
            &mut declared_symbols,
            "buf",
            DeclaredSymbolInfo::Buffer {
                elem_ty: PrimitiveType::F64,
                channels: BufferChannelInfo::Static(2),
            },
        );

        assert!(state_scalars.is_empty());
        assert_eq!(
            declared_symbols.get("buf"),
            Some(&DeclaredSymbolInfo::Buffer {
                elem_ty: PrimitiveType::F64,
                channels: BufferChannelInfo::Static(2),
            })
        );
        assert!(has_declared_buffer_symbol_info(&declared_symbols, "buf"));
        assert!(has_declared_buffer_elem_type_info(
            &declared_symbols,
            "buf",
            PrimitiveType::F64
        ));
        assert!(is_declared_multichannel_buffer_info(
            &declared_symbols,
            "buf"
        ));
        assert_eq!(
            declared_static_buffer_channels_info(&declared_symbols, "buf"),
            Some(2)
        );
    }

    #[test]
    fn set_declared_symbol_types_populates_typed_map() {
        let mut state_scalars = HashMap::new();
        let mut declared_symbols = DeclaredSymbolMap::new();
        let names = HashSet::from([String::from("in1")]);
        let types = HashMap::from([(String::from("in1"), PrimitiveType::F32)]);

        set_declared_symbol_types(
            &mut state_scalars,
            &mut declared_symbols,
            &names,
            &types,
            DeclaredScalarSymbolKind::Input,
        );

        assert!(state_scalars.is_empty());
        assert_eq!(
            declared_symbols.get("in1"),
            Some(&DeclaredSymbolInfo::Input {
                ty: PrimitiveType::F32,
            })
        );
    }

    #[test]
    fn declared_buffer_helpers_distinguish_dynamic_and_mono() {
        let declared_symbols = HashMap::from([
            (
                String::from("mono"),
                DeclaredSymbolInfo::Buffer {
                    elem_ty: PrimitiveType::F32,
                    channels: BufferChannelInfo::Mono,
                },
            ),
            (
                String::from("dyn"),
                DeclaredSymbolInfo::Buffer {
                    elem_ty: PrimitiveType::I32,
                    channels: BufferChannelInfo::Dynamic,
                },
            ),
        ]);

        assert!(!is_declared_multichannel_buffer_info(
            &declared_symbols,
            "mono"
        ));
        assert_eq!(
            declared_static_buffer_channels_info(&declared_symbols, "mono"),
            None
        );
        assert!(is_declared_multichannel_buffer_info(
            &declared_symbols,
            "dyn"
        ));
        assert!(has_declared_dynamic_buffer_channels_info(
            &declared_symbols,
            "dyn"
        ));
    }
}
