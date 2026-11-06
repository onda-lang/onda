//! Borrowed source graph and project records. These queries copy only the record;
//! nested byte/string storage remains owned by the supplied handle.
use super::*;

#[repr(C)]
#[derive(Default)]
pub struct onda_unresolved_source_info_t {
    pub source_path_utf8: *const c_char,
    pub kind: i32,
    pub specifier_utf8: *const c_char,
    pub candidate_count: i32,
}

#[repr(C)]
#[derive(Default)]
pub struct onda_project_buffer_info_t {
    pub name: *const c_char,
    pub asset_id: *const c_char,
    pub element_type: i32,
    pub frames: u32,
    pub channels: u32,
    pub sample_rate: f32,
}

unsafe fn write_info<T: Default>(out: *mut T, value: Option<T>) -> i32 {
    let Some(out) = out.as_mut() else {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    };
    match value {
        Some(value) => {
            *out = value;
            0
        }
        None => {
            *out = T::default();
            ONDA_API_ERROR_INVALID_ARGUMENT
        }
    }
}

/// Copies a record with storage borrowed until manifest destruction; clears it on failure.
#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_document_info(
    manifest: *const onda_source_manifest,
    index: i32,
    out: *mut onda_source_graph_document_t,
) -> i32 {
    let value = (|| {
        let manifest = manifest.as_ref()?;
        let index = usize::try_from(index).ok()?;
        let contents = manifest.document_contents.get(index)?;
        Some(onda_source_graph_document_t {
            path_utf8: manifest.document_paths.get(index)?.as_ptr(),
            source_utf8: contents.as_ptr().cast(),
            source_bytes: contents.len(),
        })
    })();
    write_info(out, value)
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_resolution_info(
    manifest: *const onda_source_manifest,
    index: i32,
    out: *mut onda_source_graph_resolution_t,
) -> i32 {
    let value = (|| {
        let value = manifest
            .as_ref()?
            .resolutions
            .get(usize::try_from(index).ok()?)?;
        Some(onda_source_graph_resolution_t {
            source_path_utf8: value.reference.source_path.as_ptr(),
            kind: value.reference.kind,
            specifier_utf8: value.reference.specifier.as_ptr(),
            target_path_utf8: value.target_path.as_ptr(),
        })
    })();
    write_info(out, value)
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_resolution_info(
    manifest: *const onda_source_manifest,
    index: i32,
    out: *mut onda_unresolved_source_info_t,
) -> i32 {
    let value = (|| {
        let value = manifest
            .as_ref()?
            .unresolved_resolutions
            .get(usize::try_from(index).ok()?)?;
        Some(onda_unresolved_source_info_t {
            source_path_utf8: value.reference.source_path.as_ptr(),
            kind: value.reference.kind,
            specifier_utf8: value.reference.specifier.as_ptr(),
            candidate_count: saturating_usize_to_i32(value.candidates.len()),
        })
    })();
    write_info(out, value)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_document_info(
    image: *const onda_project_image,
    index: i32,
    out: *mut onda_source_graph_document_t,
) -> i32 {
    let value = (|| {
        let image = image.as_ref()?;
        let index = usize::try_from(index).ok()?;
        let document = image.inner.sources().documents.get(index)?;
        Some(onda_source_graph_document_t {
            path_utf8: image.document_paths.get(index)?.as_ptr(),
            source_utf8: document.contents.as_ptr().cast(),
            source_bytes: document.contents.len(),
        })
    })();
    write_info(out, value)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_resolution_info(
    image: *const onda_project_image,
    index: i32,
    out: *mut onda_source_graph_resolution_t,
) -> i32 {
    let value = (|| {
        let image = image.as_ref()?;
        let index = usize::try_from(index).ok()?;
        let resolution = image.inner.sources().resolutions.get(index)?;
        Some(onda_source_graph_resolution_t {
            source_path_utf8: image.resolution_sources.get(index)?.as_ptr(),
            kind: match resolution.kind {
                onda_project::SourceReferenceKind::Include => ONDA_SOURCE_REFERENCE_INCLUDE,
                onda_project::SourceReferenceKind::Import => ONDA_SOURCE_REFERENCE_IMPORT,
            },
            specifier_utf8: image.resolution_specifiers.get(index)?.as_ptr(),
            target_path_utf8: image.resolution_targets.get(index)?.as_ptr(),
        })
    })();
    write_info(out, value)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_buffer_info(
    image: *const onda_project_image,
    index: i32,
    out: *mut onda_project_buffer_info_t,
) -> i32 {
    let value = (|| {
        let value = image.as_ref()?.buffers.get(usize::try_from(index).ok()?)?;
        Some(onda_project_buffer_info_t {
            name: value.name.as_ptr(),
            asset_id: value.asset_id.as_ptr(),
            element_type: value.element_type,
            frames: value.frames,
            channels: value.channels,
            sample_rate: value.sample_rate,
        })
    })();
    write_info(out, value)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_materialization_file_info(
    plan: *const onda_project_materialization_plan,
    index: i32,
    out: *mut onda_project_file_t,
) -> i32 {
    let value = (|| {
        let plan = plan.as_ref()?;
        let index = usize::try_from(index).ok()?;
        let file = plan.inner.files.get(index)?;
        Some(onda_project_file_t {
            path_utf8: plan.file_paths.get(index)?.as_ptr(),
            bytes: file.bytes.as_ptr().cast(),
            byte_count: file.bytes.len(),
        })
    })();
    write_info(out, value)
}
