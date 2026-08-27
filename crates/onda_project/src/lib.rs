//! Host-neutral Onda project images, editable project manifests, and typed
//! external-buffer assets.

mod buffer;
mod capture;
mod image;
mod manifest;
mod materialize;

pub use buffer::{
    decode_buffer_bytes, decode_ondabuffer, encode_ondabuffer, encode_wav_f32, inspect_buffer_file,
    is_ondabuffer, load_buffer_file, validate_ondabuffer, BufferAsset, BufferAssetMetadata,
    BufferElement, BufferSamples, ValidatedOndabuffer, ONDA_BUFFER_FORMAT_VERSION,
};
pub use image::{
    current_stdlib_digest, validate_buffer_asset_metadata, validate_buffer_assets, AssetId,
    ProjectBufferChannels, ProjectBufferDeclaration, ProjectImage, SourceDocument, SourceImage,
    SourceReferenceKind, SourceReplayError, SourceResolution, ONDA_PROJECT_IMAGE_FORMAT_VERSION,
};
pub use manifest::{
    is_project_file_path, resolve_project_input, resolve_project_watch_paths, InlineBuffer,
    ManifestBufferBinding, ManifestBufferElementBinding, ProjectConstValue, ProjectFile,
    ProjectInput, ProjectManifest, ProjectWatchPaths, ONDA_PROJECT_DEFAULT_FILE_NAME,
    ONDA_PROJECT_FILE_EXTENSION,
};
pub use materialize::{new_project_plan, MaterializationPlan, PlannedFile};

use std::fmt;
use std::io::Read;
use std::path::Path;

/// Resource limits applied before allocating project-controlled payloads.
#[derive(Debug, Clone, Copy)]
pub struct ProjectLimits {
    pub max_documents: usize,
    pub max_resolutions: usize,
    pub max_source_bytes: usize,
    pub max_assets: usize,
    pub max_buffer_bindings: usize,
    pub max_constant_bindings: usize,
    pub max_asset_bytes: usize,
    pub max_total_asset_bytes: usize,
    pub max_manifest_bytes: usize,
    pub max_path_bytes: usize,
    pub max_path_component_bytes: usize,
}

impl Default for ProjectLimits {
    fn default() -> Self {
        Self {
            max_documents: 4096,
            max_resolutions: 16 * 1024,
            max_source_bytes: 64 * 1024 * 1024,
            max_assets: 4096,
            max_buffer_bindings: 4096,
            max_constant_bindings: 4096,
            max_asset_bytes: 1024 * 1024 * 1024,
            max_total_asset_bytes: 2 * 1024 * 1024 * 1024,
            max_manifest_bytes: 64 * 1024 * 1024,
            max_path_bytes: 16 * 1024,
            max_path_component_bytes: 255,
        }
    }
}

impl ProjectLimits {
    /// Restricts the next decoded asset to the unallocated part of the
    /// aggregate asset budget.
    pub fn with_remaining_asset_budget(self, allocated_asset_bytes: usize) -> Self {
        Self {
            max_asset_bytes: self.max_asset_bytes.min(
                self.max_total_asset_bytes
                    .saturating_sub(allocated_asset_bytes),
            ),
            ..self
        }
    }

    /// Maximum number of files accepted by an editable project transport.
    pub fn max_materialized_file_count(&self) -> usize {
        self.max_documents
            .saturating_add(self.max_assets)
            .saturating_add(1)
    }

    /// Maximum encoded size of any one file in an editable project transport.
    pub fn max_materialized_file_bytes(&self) -> usize {
        self.max_manifest_bytes.max(self.max_source_bytes).max(
            self.max_asset_bytes
                .saturating_add(buffer::ONDA_BUFFER_HEADER_BYTES),
        )
    }

    /// Maximum aggregate encoded size of an editable project transport.
    pub fn max_materialized_total_bytes(&self) -> usize {
        self.max_manifest_bytes
            .saturating_add(self.max_source_bytes)
            .saturating_add(self.max_total_asset_bytes)
            .saturating_add(
                self.max_assets
                    .saturating_mul(buffer::ONDA_BUFFER_HEADER_BYTES),
            )
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProjectError {
    message: String,
}

impl ProjectError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProjectError {}

impl From<std::io::Error> for ProjectError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for ProjectError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub(crate) fn checked_product(values: &[usize], context: &str) -> Result<usize, ProjectError> {
    values.iter().try_fold(1usize, |product, value| {
        product
            .checked_mul(*value)
            .ok_or_else(|| ProjectError::new(format!("{context} size overflows")))
    })
}

pub(crate) fn read_bounded_file(
    path: &Path,
    limit: usize,
    label: &str,
    limit_label: &str,
) -> Result<Vec<u8>, ProjectError> {
    let file = std::fs::File::open(path).map_err(|error| {
        ProjectError::new(format!(
            "failed to open {label} '{}': {error}",
            path.display()
        ))
    })?;
    let file_bytes = file.metadata().map_err(|error| {
        ProjectError::new(format!(
            "failed to inspect {label} '{}': {error}",
            path.display()
        ))
    })?;
    let limit_u64 = u64::try_from(limit).unwrap_or(u64::MAX);
    if file_bytes.len() > limit_u64 {
        return Err(ProjectError::new(format!(
            "{label} '{}' exceeds the {limit} byte {limit_label} limit",
            path.display()
        )));
    }
    let read_limit = limit_u64.saturating_add(1);
    let mut bytes = Vec::with_capacity(
        usize::try_from(file_bytes.len())
            .unwrap_or(limit)
            .min(limit),
    );
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ProjectError::new(format!(
                "failed to read {label} '{}': {error}",
                path.display()
            ))
        })?;
    if bytes.len() > limit {
        return Err(ProjectError::new(format!(
            "{label} '{}' exceeds the {limit} byte {limit_label} limit",
            path.display()
        )));
    }
    Ok(bytes)
}
