use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{
    parse_buffer_element_name, portable_path_collision_key, validate_project_buffer_name,
    validate_relative_project_path,
};
use crate::{BufferAsset, ProjectError, ProjectLimits};

// Synchronized from format-versions.json; do not edit this copy directly.
pub const ONDA_PROJECT_IMAGE_FORMAT_VERSION: u32 = 1;
const ONDA_PROJECT_IMAGE_MAGIC: &[u8; 8] = b"ONDAPRJ\0";
const ONDA_PROJECT_IMAGE_HEADER_BYTES: usize = 8 + 4 + 8;

#[derive(Debug, Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(String);

impl AssetId {
    pub fn for_buffer(asset: &BufferAsset) -> Self {
        Self::from_buffer_digest(asset.content_digest())
    }

    pub fn from_buffer_digest(digest: [u8; 32]) -> Self {
        Self(format!("sha256:{}", hex::encode(digest)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceDocument {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceReferenceKind {
    Include,
    Import,
}

#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SourceResolution {
    pub source: String,
    pub kind: SourceReferenceKind,
    pub specifier: String,
    pub target: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceImage {
    pub entry: String,
    pub stdlib_digest: String,
    pub documents: Vec<SourceDocument>,
    pub resolutions: Vec<SourceResolution>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectImage {
    sources: SourceImage,
    buffer_bindings: BTreeMap<String, AssetId>,
    assets: BTreeMap<AssetId, BufferAsset>,
    content_digest: [u8; 32],
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProjectBufferChannels {
    Mono,
    Static(u32),
    Dynamic,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProjectBufferDeclaration {
    pub name: String,
    pub element: crate::BufferElement,
    pub channels: ProjectBufferChannels,
    pub array_len: usize,
    pub is_array: bool,
}

#[derive(Debug)]
pub enum SourceReplayError {
    Project(ProjectError),
    Load(onda_frontend::LoadError),
}

impl fmt::Display for SourceReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project(error) => error.fmt(formatter),
            Self::Load(error) => {
                if let Some(diagnostic) = error.diagnostics.first() {
                    formatter.write_str(&diagnostic.message)
                } else {
                    formatter.write_str("source replay failed")
                }
            }
        }
    }
}

impl std::error::Error for SourceReplayError {}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireProjectImage {
    format: String,
    format_version: u32,
    content_digest: String,
    sources: SourceImage,
    buffer_bindings: BTreeMap<String, AssetId>,
    assets: Vec<WireAsset>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAsset {
    id: AssetId,
    element: crate::BufferElement,
    frames: u32,
    channels: u32,
    sample_rate_bits: u32,
    payload_bytes: u64,
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedBytes {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}

impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other(
                "bounded project manifest exceeded its limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl ProjectImage {
    pub fn from_buffer_assets(
        sources: SourceImage,
        buffers: BTreeMap<String, BufferAsset>,
    ) -> Result<Self, ProjectError> {
        let mut buffer_bindings = BTreeMap::new();
        let mut assets = BTreeMap::new();
        for (name, asset) in buffers {
            let id = AssetId::for_buffer(&asset);
            buffer_bindings.insert(name, id.clone());
            assets.entry(id).or_insert(asset);
        }
        Self::new(sources, buffer_bindings, assets)
    }

    pub fn new(
        mut sources: SourceImage,
        buffer_bindings: BTreeMap<String, AssetId>,
        assets: BTreeMap<AssetId, BufferAsset>,
    ) -> Result<Self, ProjectError> {
        sources
            .documents
            .sort_by(|left, right| left.path.cmp(&right.path));
        sources.resolutions.sort();
        let mut image = Self {
            sources,
            buffer_bindings,
            assets,
            content_digest: [0; 32],
        };
        image.validate(&ProjectLimits::default())?;
        image.content_digest = image.calculate_content_digest();
        Ok(image)
    }

    pub fn content_digest(&self) -> [u8; 32] {
        self.content_digest
    }

    pub fn content_digest_string(&self) -> String {
        format!("sha256:{}", hex::encode(self.content_digest))
    }

    pub fn sources(&self) -> &SourceImage {
        &self.sources
    }

    pub fn buffer_bindings(&self) -> &BTreeMap<String, AssetId> {
        &self.buffer_bindings
    }

    pub fn assets(&self) -> &BTreeMap<AssetId, BufferAsset> {
        &self.assets
    }

    pub fn validate(&self, limits: &ProjectLimits) -> Result<(), ProjectError> {
        self.sources.validate(limits)?;
        if self.assets.len() > limits.max_assets {
            return Err(ProjectError::new(format!(
                "project contains {} assets, exceeding the {} asset limit",
                self.assets.len(),
                limits.max_assets
            )));
        }
        if self.buffer_bindings.len() > limits.max_buffer_bindings {
            return Err(ProjectError::new(format!(
                "project contains {} buffer bindings, exceeding the {} binding limit",
                self.buffer_bindings.len(),
                limits.max_buffer_bindings
            )));
        }
        let mut total_asset_bytes = 0usize;
        for (id, asset) in &self.assets {
            asset.validate(limits)?;
            let expected = AssetId::for_buffer(asset);
            if id != &expected {
                return Err(ProjectError::new(format!(
                    "project asset ID '{}' does not match its canonical content",
                    id.as_str()
                )));
            }
            total_asset_bytes = total_asset_bytes
                .checked_add(asset.payload_bytes())
                .ok_or_else(|| ProjectError::new("project asset byte total overflows"))?;
            if total_asset_bytes > limits.max_total_asset_bytes {
                return Err(ProjectError::new(format!(
                    "project assets exceed the {} byte total limit",
                    limits.max_total_asset_bytes
                )));
            }
        }
        let mut used = BTreeSet::new();
        let mut scalar_bindings = BTreeSet::new();
        let mut array_bindings = BTreeSet::new();
        for (name, id) in &self.buffer_bindings {
            validate_project_buffer_name(name, limits)?;
            if let Some((base, _)) = parse_buffer_element_name(name) {
                array_bindings.insert(base);
            } else {
                scalar_bindings.insert(name.as_str());
            }
            if !self.assets.contains_key(id) {
                return Err(ProjectError::new(format!(
                    "buffer '{name}' references missing asset '{}'",
                    id.as_str()
                )));
            }
            used.insert(id);
        }
        if let Some(name) = array_bindings
            .into_iter()
            .find(|name| scalar_bindings.contains(name))
        {
            return Err(ProjectError::new(format!(
                "project buffer '{name}' cannot have both scalar and array-slot bindings"
            )));
        }
        if used.len() != self.assets.len() {
            return Err(ProjectError::new("project contains unbound buffer assets"));
        }
        Ok(())
    }

    /// Checks the logical project bindings against declarations produced by a
    /// successful compilation.
    pub fn validate_buffer_declarations(
        &self,
        declarations: &[ProjectBufferDeclaration],
    ) -> Result<(), ProjectError> {
        let assets = self
            .buffer_bindings
            .iter()
            .map(|(name, id)| {
                self.assets
                    .get(id)
                    .map(|asset| (name.as_str(), asset))
                    .ok_or_else(|| {
                        ProjectError::new(format!(
                            "project buffer '{name}' references a missing asset"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        validate_buffer_assets(assets, declarations)
    }

    pub fn serialize(&self) -> Result<Vec<u8>, ProjectError> {
        self.serialize_with_limits(ProjectLimits::default())
    }

    pub fn serialize_with_limits(&self, limits: ProjectLimits) -> Result<Vec<u8>, ProjectError> {
        self.validate(&limits)?;
        let assets = self
            .assets
            .iter()
            .map(|(id, asset)| WireAsset {
                id: id.clone(),
                element: asset.element(),
                frames: asset.frames,
                channels: asset.channels,
                sample_rate_bits: asset.sample_rate.to_bits(),
                payload_bytes: asset.payload_bytes() as u64,
            })
            .collect::<Vec<_>>();
        let wire = WireProjectImage {
            format: "onda-project-image".to_owned(),
            format_version: ONDA_PROJECT_IMAGE_FORMAT_VERSION,
            content_digest: self.content_digest_string(),
            sources: self.sources.clone(),
            buffer_bindings: self.buffer_bindings.clone(),
            assets,
        };
        let mut manifest = BoundedBytes::new(limits.max_manifest_bytes);
        if let Err(error) = serde_json::to_writer(&mut manifest, &wire) {
            if !manifest.exceeded {
                return Err(error.into());
            }
            return Err(ProjectError::new(format!(
                "project image manifest exceeds the {} byte limit",
                limits.max_manifest_bytes,
            )));
        }
        let manifest = manifest.bytes;
        let manifest_len = u64::try_from(manifest.len())
            .map_err(|_| ProjectError::new("project image manifest length does not fit u64"))?;
        let asset_bytes = self
            .assets
            .values()
            .map(BufferAsset::payload_bytes)
            .try_fold(0usize, |total, size| total.checked_add(size))
            .ok_or_else(|| ProjectError::new("project image length overflows"))?;
        let mut output = Vec::with_capacity(
            ONDA_PROJECT_IMAGE_HEADER_BYTES
                .saturating_add(manifest.len())
                .saturating_add(asset_bytes),
        );
        output.extend_from_slice(ONDA_PROJECT_IMAGE_MAGIC);
        output.extend_from_slice(&ONDA_PROJECT_IMAGE_FORMAT_VERSION.to_le_bytes());
        output.extend_from_slice(&manifest_len.to_le_bytes());
        output.extend_from_slice(&manifest);
        for asset in self.assets.values() {
            asset.append_canonical_payload(&mut output);
        }
        Ok(output)
    }

    pub fn deserialize(bytes: &[u8], limits: ProjectLimits) -> Result<Self, ProjectError> {
        if bytes.len() < ONDA_PROJECT_IMAGE_HEADER_BYTES || &bytes[..8] != ONDA_PROJECT_IMAGE_MAGIC
        {
            return Err(ProjectError::new("data is not an Onda project image"));
        }
        let version = read_u32(bytes, 8)?;
        if version != ONDA_PROJECT_IMAGE_FORMAT_VERSION {
            return Err(ProjectError::new(format!(
                "unsupported Onda project image version {version}"
            )));
        }
        let manifest_len = usize::try_from(read_u64(bytes, 12)?)
            .map_err(|_| ProjectError::new("project image manifest length does not fit host"))?;
        if manifest_len > limits.max_manifest_bytes {
            return Err(ProjectError::new(format!(
                "project image manifest exceeds the {} byte limit",
                limits.max_manifest_bytes
            )));
        }
        let manifest_end = ONDA_PROJECT_IMAGE_HEADER_BYTES
            .checked_add(manifest_len)
            .ok_or_else(|| ProjectError::new("project image manifest length overflows"))?;
        let Some(manifest_bytes) = bytes.get(ONDA_PROJECT_IMAGE_HEADER_BYTES..manifest_end) else {
            return Err(ProjectError::new("project image manifest is truncated"));
        };
        let wire: WireProjectImage = serde_json::from_slice(manifest_bytes)?;
        if wire.format != "onda-project-image"
            || wire.format_version != ONDA_PROJECT_IMAGE_FORMAT_VERSION
        {
            return Err(ProjectError::new(
                "project image manifest has an unsupported format",
            ));
        }
        if wire.assets.len() > limits.max_assets {
            return Err(ProjectError::new("project image contains too many assets"));
        }
        let mut cursor = manifest_end;
        let mut assets = BTreeMap::new();
        let mut total_asset_bytes = 0usize;
        for descriptor in wire.assets {
            let payload_len = usize::try_from(descriptor.payload_bytes)
                .map_err(|_| ProjectError::new("asset payload length does not fit host"))?;
            if payload_len > limits.max_asset_bytes {
                return Err(ProjectError::new(
                    "project image asset exceeds its byte limit",
                ));
            }
            total_asset_bytes = total_asset_bytes
                .checked_add(payload_len)
                .ok_or_else(|| ProjectError::new("project asset total overflows"))?;
            if total_asset_bytes > limits.max_total_asset_bytes {
                return Err(ProjectError::new(
                    "project image assets exceed their total byte limit",
                ));
            }
            let end = cursor
                .checked_add(payload_len)
                .ok_or_else(|| ProjectError::new("project asset extent overflows"))?;
            let Some(payload) = bytes.get(cursor..end) else {
                return Err(ProjectError::new(
                    "project image asset payload is truncated",
                ));
            };
            let samples =
                crate::BufferSamples::from_canonical_le_bytes(descriptor.element, payload)?;
            let asset = BufferAsset {
                frames: descriptor.frames,
                channels: descriptor.channels,
                sample_rate: f32::from_bits(descriptor.sample_rate_bits),
                samples,
            };
            asset.validate(&limits)?;
            if descriptor.id != AssetId::for_buffer(&asset) {
                return Err(ProjectError::new("project image asset digest mismatch"));
            }
            if assets.insert(descriptor.id, asset).is_some() {
                return Err(ProjectError::new(
                    "project image contains duplicate asset IDs",
                ));
            }
            cursor = end;
        }
        if cursor != bytes.len() {
            return Err(ProjectError::new(
                "project image contains trailing unreferenced bytes",
            ));
        }
        let mut sources = wire.sources;
        sources
            .documents
            .sort_by(|left, right| left.path.cmp(&right.path));
        sources.resolutions.sort();
        let mut image = Self {
            sources,
            buffer_bindings: wire.buffer_bindings,
            assets,
            content_digest: [0; 32],
        };
        image.validate(&limits)?;
        image.content_digest = image.calculate_content_digest();
        if wire.content_digest != image.content_digest_string() {
            return Err(ProjectError::new("project image content digest mismatch"));
        }
        Ok(image)
    }

    fn calculate_content_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"onda-project-image-content-v1\0");
        hash_string(&mut hasher, &self.sources.entry);
        hash_string(&mut hasher, &self.sources.stdlib_digest);
        for document in &self.sources.documents {
            hash_string(&mut hasher, &document.path);
            hash_string(&mut hasher, &document.contents);
        }
        let mut resolutions = self.sources.resolutions.clone();
        resolutions.sort();
        for resolution in resolutions {
            hash_string(&mut hasher, &resolution.source);
            hasher.update([match resolution.kind {
                SourceReferenceKind::Include => 0,
                SourceReferenceKind::Import => 1,
            }]);
            hash_string(&mut hasher, &resolution.specifier);
            hash_string(&mut hasher, &resolution.target);
        }
        for (name, id) in &self.buffer_bindings {
            hash_string(&mut hasher, name);
            hash_string(&mut hasher, id.as_str());
        }
        for id in self.assets.keys() {
            hash_string(&mut hasher, id.as_str());
        }
        hasher.finalize().into()
    }
}

/// Checks logical buffer assets against declarations produced by a successful
/// compilation.
pub fn validate_buffer_assets<'a>(
    assets: impl IntoIterator<Item = (&'a str, &'a BufferAsset)>,
    declarations: &[ProjectBufferDeclaration],
) -> Result<(), ProjectError> {
    let declarations = declarations
        .iter()
        .map(|declaration| (declaration.name.as_str(), declaration))
        .collect::<HashMap<_, _>>();
    for (name, asset) in assets {
        let (declaration_name, element_index) = parse_buffer_element_name(name)
            .map_or((name, None), |(base, index)| (base, Some(index)));
        let declaration = declarations.get(declaration_name).ok_or_else(|| {
            ProjectError::new(format!(
                "project binds unknown buffer '{name}'; it is not declared by the entry source"
            ))
        })?;
        match element_index {
            Some(index) if declaration.is_array && index < declaration.array_len => {}
            Some(index) => {
                return Err(ProjectError::new(format!(
                    "project buffer '{name}' selects slot {index}, but '{}' has length {}",
                    declaration.name, declaration.array_len
                )));
            }
            None if !declaration.is_array => {}
            None => {
                return Err(ProjectError::new(format!(
                    "project buffer array '{}' must bind individual slots",
                    declaration.name
                )));
            }
        }
        if asset.element() != declaration.element {
            return Err(ProjectError::new(format!(
                "buffer '{name}' requires {}, but its asset contains {}",
                declaration.element,
                asset.element()
            )));
        }
        let expected_channels = match declaration.channels {
            ProjectBufferChannels::Mono => Some(1),
            ProjectBufferChannels::Static(channels) => Some(channels),
            ProjectBufferChannels::Dynamic => None,
        };
        if let Some(expected_channels) = expected_channels {
            if asset.channels != expected_channels {
                return Err(ProjectError::new(format!(
                    "buffer '{name}' requires {expected_channels} channel{}, but its asset contains {}",
                    if expected_channels == 1 { "" } else { "s" },
                    asset.channels
                )));
            }
        }
    }
    Ok(())
}

impl SourceImage {
    /// Replays the captured source graph without consulting the filesystem.
    pub fn replay(
        &self,
        limits: ProjectLimits,
    ) -> Result<onda_frontend::LoadedProgram, SourceReplayError> {
        self.validate(&limits).map_err(SourceReplayError::Project)?;
        let current_stdlib = current_stdlib_digest();
        if self.stdlib_digest != current_stdlib {
            return Err(SourceReplayError::Project(ProjectError::new(format!(
                "source image requires standard library {}, but this Onda build provides {current_stdlib}",
                self.stdlib_digest
            ))));
        }
        let sources = self
            .documents
            .iter()
            .map(|document| (PathBuf::from(&document.path), document.contents.to_owned()))
            .collect::<HashMap<_, _>>();
        let resolutions = self
            .resolutions
            .iter()
            .map(|resolution| onda_frontend::SourceResolution {
                source: PathBuf::from(&resolution.source),
                kind: match resolution.kind {
                    SourceReferenceKind::Include => onda_frontend::SourceReferenceKind::Include,
                    SourceReferenceKind::Import => onda_frontend::SourceReferenceKind::Import,
                },
                specifier: resolution.specifier.clone(),
                target: PathBuf::from(&resolution.target),
            })
            .collect::<Vec<_>>();
        onda_frontend::load_program_file_from_snapshot(
            PathBuf::from(&self.entry).as_path(),
            &sources,
            &resolutions,
        )
        .map_err(SourceReplayError::Load)
    }

    pub fn validate(&self, limits: &ProjectLimits) -> Result<(), ProjectError> {
        validate_relative_project_path(&self.entry, limits)?;
        validate_digest(&self.stdlib_digest, "source image standard-library digest")?;
        if self.documents.is_empty() || self.documents.len() > limits.max_documents {
            return Err(ProjectError::new(format!(
                "source image must contain 1..={} documents",
                limits.max_documents
            )));
        }
        if self.resolutions.len() > limits.max_resolutions {
            return Err(ProjectError::new(format!(
                "source image contains too many resolutions (maximum {})",
                limits.max_resolutions
            )));
        }
        let mut paths = BTreeSet::new();
        let mut source_bytes = 0usize;
        for document in &self.documents {
            validate_relative_project_path(&document.path, limits)?;
            if !paths.insert(document.path.as_str()) {
                return Err(ProjectError::new(format!(
                    "source image contains duplicate document '{}'",
                    document.path
                )));
            }
            source_bytes = source_bytes
                .checked_add(document.contents.len())
                .ok_or_else(|| ProjectError::new("source image byte total overflows"))?;
            if source_bytes > limits.max_source_bytes {
                return Err(ProjectError::new(format!(
                    "source image exceeds the {} byte source limit",
                    limits.max_source_bytes
                )));
            }
        }
        validate_portable_file_set(
            self.documents
                .iter()
                .map(|document| document.path.clone())
                .chain([crate::ONDA_PROJECT_DEFAULT_FILE_NAME.to_owned()]),
        )?;
        if !paths.contains(self.entry.as_str()) {
            return Err(ProjectError::new(
                "source image does not contain its entry document",
            ));
        }
        let mut resolution_keys = BTreeSet::new();
        for resolution in &self.resolutions {
            if resolution.specifier.is_empty() {
                return Err(ProjectError::new(
                    "source image contains an empty resolution specifier",
                ));
            }
            if resolution.specifier.contains('\0') {
                return Err(ProjectError::new(
                    "source image resolution specifiers must not contain NUL",
                ));
            }
            if !paths.contains(resolution.source.as_str())
                || !paths.contains(resolution.target.as_str())
            {
                return Err(ProjectError::new(
                    "source image contains a dangling resolution",
                ));
            }
            let key = (
                resolution.source.as_str(),
                resolution.kind,
                resolution.specifier.as_str(),
            );
            if !resolution_keys.insert(key) {
                return Err(ProjectError::new(format!(
                    "source image contains duplicate resolution '{}' from '{}'",
                    resolution.specifier, resolution.source
                )));
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_portable_file_set(
    paths: impl IntoIterator<Item = String>,
) -> Result<(), ProjectError> {
    let mut portable_paths = BTreeMap::new();
    for path in paths {
        let key = portable_path_collision_key(&path);
        if let Some(existing) = portable_paths.insert(key, path.clone()) {
            return Err(ProjectError::new(format!(
                "project paths '{existing}' and '{path}' collide on case-insensitive filesystems"
            )));
        }
    }
    for (portable_path, original_path) in &portable_paths {
        let mut separator = portable_path.find('/');
        while let Some(index) = separator {
            let ancestor = &portable_path[..index];
            if let Some(ancestor_path) = portable_paths.get(ancestor) {
                return Err(ProjectError::new(format!(
                    "project file '{ancestor_path}' conflicts with descendant '{original_path}'",
                )));
            }
            separator = portable_path[index + 1..]
                .find('/')
                .map(|offset| index + 1 + offset);
        }
    }
    Ok(())
}

pub fn current_stdlib_digest() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"onda-stdlib-content-v1\0");
    for name in onda_frontend::stdlib_module_names() {
        hash_string(&mut hasher, name);
        if let Some(source) = onda_frontend::stdlib_module_source(name) {
            hash_string(&mut hasher, source);
        }
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub(crate) fn validate_digest(value: &str, context: &str) -> Result<(), ProjectError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(ProjectError::new(format!(
            "{context} must use the 'sha256:' prefix"
        )));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ProjectError::new(format!(
            "{context} must contain 64 lowercase hexadecimal digits"
        )));
    }
    Ok(())
}

fn hash_string(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ProjectError> {
    let Some(slice) = bytes.get(offset..offset.saturating_add(4)) else {
        return Err(ProjectError::new("project image header is truncated"));
    };
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ProjectError> {
    let Some(slice) = bytes.get(offset..offset.saturating_add(8)) else {
        return Err(ProjectError::new("project image header is truncated"));
    };
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}
