use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_normalization::{is_nfc, UnicodeNormalization};

use crate::{BufferAsset, BufferElement, BufferSamples, ProjectError, ProjectLimits};

pub const ONDA_PROJECT_FILE_EXTENSION: &str = "ondaproject";
pub const ONDA_PROJECT_DEFAULT_FILE_NAME: &str = "project.ondaproject";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    pub entry: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub buffers: BTreeMap<String, ManifestBufferBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ManifestBufferBinding {
    File(ManifestBufferFile),
    Inline(ManifestInlineBuffer),
    Array(Vec<Option<ManifestBufferElementBinding>>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ManifestBufferElementBinding {
    File(ManifestBufferFile),
    Inline(ManifestInlineBuffer),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBufferFile {
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestInlineBuffer {
    pub inline: InlineBuffer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlineBuffer {
    pub element: BufferElement,
    pub channels: u32,
    pub sample_rate: f32,
    pub values: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct ProjectFile {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: ProjectManifest,
    pub entry_path: PathBuf,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProjectWatchPaths {
    pub manifest: PathBuf,
    pub assets: Vec<PathBuf>,
    pub asset_aliases: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum ProjectInput {
    Source(PathBuf),
    Project(ProjectFile),
}

impl ProjectInput {
    pub fn entry_path(&self) -> &Path {
        match self {
            Self::Source(path) => path,
            Self::Project(project) => &project.entry_path,
        }
    }

    pub fn project(&self) -> Option<&ProjectFile> {
        match self {
            Self::Source(_) => None,
            Self::Project(project) => Some(project),
        }
    }
}

impl ProjectManifest {
    pub fn empty(entry: impl Into<String>) -> Self {
        Self {
            entry: entry.into(),
            buffers: BTreeMap::new(),
        }
    }

    pub fn validate(&self, limits: &ProjectLimits) -> Result<(), ProjectError> {
        validate_relative_project_path(&self.entry, limits)?;
        let binding_count = self.buffers.values().try_fold(0usize, |total, binding| {
            let count = match binding {
                ManifestBufferBinding::File(_) | ManifestBufferBinding::Inline(_) => 1,
                ManifestBufferBinding::Array(elements) => {
                    elements.iter().filter(|element| element.is_some()).count()
                }
            };
            total
                .checked_add(count)
                .ok_or_else(|| ProjectError::new("project buffer binding count overflows"))
        })?;
        if binding_count > limits.max_buffer_bindings {
            return Err(ProjectError::new(format!(
                "project contains {binding_count} buffer bindings, exceeding the {} binding limit",
                limits.max_buffer_bindings
            )));
        }
        let mut referenced_files = BTreeSet::from([self.entry.clone()]);
        for (name, binding) in &self.buffers {
            validate_project_buffer_name(name, limits)?;
            if parse_buffer_element_name(name).is_some() {
                return Err(ProjectError::new(format!(
                    "project buffer key '{name}' selects an array slot; use an array binding for the declared buffer instead"
                )));
            }
            match binding {
                ManifestBufferBinding::File(file) => {
                    validate_relative_project_path(&file.file, limits)?;
                    if file.file == self.entry {
                        return Err(ProjectError::new(format!(
                            "project path '{}' cannot be both the entry source and a buffer asset",
                            file.file
                        )));
                    }
                    // Multiple logical buffers may intentionally share one exact asset.
                    referenced_files.insert(file.file.clone());
                }
                ManifestBufferBinding::Inline(inline) => {
                    inline.inline.to_asset(limits)?;
                }
                ManifestBufferBinding::Array(elements) => {
                    if elements.is_empty() {
                        return Err(ProjectError::new(format!(
                            "project buffer array '{name}' must contain at least one slot"
                        )));
                    }
                    if elements.len() > limits.max_buffer_bindings {
                        return Err(ProjectError::new(format!(
                            "project buffer array '{name}' contains {} slots, exceeding the {} slot limit",
                            elements.len(),
                            limits.max_buffer_bindings
                        )));
                    }
                    for element in elements.iter().flatten() {
                        match element {
                            ManifestBufferElementBinding::File(file) => {
                                validate_relative_project_path(&file.file, limits)?;
                                if file.file == self.entry {
                                    return Err(ProjectError::new(format!(
                                        "project path '{}' cannot be both the entry source and a buffer asset",
                                        file.file
                                    )));
                                }
                                referenced_files.insert(file.file.clone());
                            }
                            ManifestBufferElementBinding::Inline(inline) => {
                                inline.inline.to_asset(limits)?;
                            }
                        }
                    }
                }
            }
        }
        crate::image::validate_portable_file_set(referenced_files)?;
        Ok(())
    }

    pub(crate) fn validate_for_project_file(
        &self,
        project_file_name: &str,
        limits: &ProjectLimits,
    ) -> Result<(), ProjectError> {
        self.validate(limits)?;
        validate_project_file_name(project_file_name, limits)?;
        let project_key = portable_path_collision_key(project_file_name);
        let collides = portable_path_collision_key(&self.entry) == project_key
            || self.buffers.values().any(|binding| match binding {
                ManifestBufferBinding::File(file) => {
                    portable_path_collision_key(&file.file) == project_key
                }
                ManifestBufferBinding::Inline(_) => false,
                ManifestBufferBinding::Array(elements) => {
                    elements.iter().flatten().any(|element| match element {
                        ManifestBufferElementBinding::File(file) => {
                            portable_path_collision_key(&file.file) == project_key
                        }
                        ManifestBufferElementBinding::Inline(_) => false,
                    })
                }
            });
        if collides {
            return Err(ProjectError::new(
                "the .ondaproject file cannot also be a referenced source or buffer asset",
            ));
        }
        Ok(())
    }

    pub fn to_pretty_json(&self) -> Result<String, ProjectError> {
        self.validate(&ProjectLimits::default())?;
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }
}

impl InlineBuffer {
    pub fn to_asset(&self, limits: &ProjectLimits) -> Result<BufferAsset, ProjectError> {
        if self.channels == 0 {
            return Err(ProjectError::new(
                "inline buffer channels must be greater than zero",
            ));
        }
        if self.values.is_empty() || !self.values.len().is_multiple_of(self.channels as usize) {
            return Err(ProjectError::new(
                "inline buffer values must contain a nonempty whole number of frames",
            ));
        }
        let frames = u32::try_from(self.values.len() / self.channels as usize)
            .map_err(|_| ProjectError::new("inline buffer frame count exceeds u32"))?;
        let payload_bytes = crate::checked_product(
            &[self.values.len(), self.element.byte_size()],
            "inline buffer payload",
        )?;
        if payload_bytes > limits.max_asset_bytes {
            return Err(ProjectError::new(format!(
                "inline buffer payload has {payload_bytes} bytes, exceeding the {} byte limit",
                limits.max_asset_bytes
            )));
        }
        let samples = match self.element {
            BufferElement::Bool => BufferSamples::Bool(
                self.values
                    .iter()
                    .map(|value| {
                        value
                            .as_bool()
                            .map(u8::from)
                            .ok_or_else(|| ProjectError::new("bool buffer values must be booleans"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            BufferElement::I32 => BufferSamples::I32(
                self.values
                    .iter()
                    .map(parse_i32)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            BufferElement::I64 => BufferSamples::I64(
                self.values
                    .iter()
                    .map(parse_i64)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            BufferElement::F32 => BufferSamples::F32(
                self.values
                    .iter()
                    .map(parse_f32)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            BufferElement::F64 => BufferSamples::F64(
                self.values
                    .iter()
                    .map(parse_f64)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };
        let asset = BufferAsset {
            frames,
            channels: self.channels,
            sample_rate: self.sample_rate,
            samples,
        };
        asset.validate(limits)?;
        Ok(asset)
    }
}

impl ProjectFile {
    pub fn watch_paths(&self) -> Result<ProjectWatchPaths, ProjectError> {
        let manifest = fs::canonicalize(&self.manifest_path).map_err(|error| {
            ProjectError::new(format!(
                "failed to resolve project manifest '{}': {error}",
                self.manifest_path.display()
            ))
        })?;
        let mut assets = BTreeSet::new();
        let mut asset_aliases = BTreeSet::new();
        for binding in self.manifest.buffers.values() {
            let files: Vec<&ManifestBufferFile> = match binding {
                ManifestBufferBinding::File(file) => vec![file],
                ManifestBufferBinding::Inline(_) => Vec::new(),
                ManifestBufferBinding::Array(elements) => elements
                    .iter()
                    .flatten()
                    .filter_map(|element| match element {
                        ManifestBufferElementBinding::File(file) => Some(file),
                        ManifestBufferElementBinding::Inline(_) => None,
                    })
                    .collect(),
            };
            for file in files {
                let (asset, alias) = resolve_contained_watch_path(&self.root, &file.file)?;
                assets.insert(asset);
                asset_aliases.extend(alias);
            }
        }
        Ok(ProjectWatchPaths {
            manifest,
            assets: assets.into_iter().collect(),
            asset_aliases: asset_aliases.into_iter().collect(),
        })
    }

    pub fn load_buffer_assets(
        &self,
        limits: ProjectLimits,
    ) -> Result<BTreeMap<String, (BufferAsset, Option<PathBuf>)>, ProjectError> {
        self.load_buffer_assets_excluding(limits, &BTreeSet::new())
    }

    pub fn load_buffer_assets_excluding(
        &self,
        limits: ProjectLimits,
        excluded: &BTreeSet<&str>,
    ) -> Result<BTreeMap<String, (BufferAsset, Option<PathBuf>)>, ProjectError> {
        let mut assets = BTreeMap::new();
        let mut loaded_files = BTreeMap::<PathBuf, String>::new();
        // Runtime buffers are independently writable, so repeated bindings may
        // reuse decoding work but every returned payload copy consumes budget.
        let mut resident_bytes = 0usize;
        for (name, binding) in &self.manifest.buffers {
            if excluded.contains(name.as_str()) {
                continue;
            }
            match binding {
                ManifestBufferBinding::File(file) => {
                    let path = resolve_contained_path(&self.root, &file.file)?;
                    let asset = clone_or_load_buffer_asset(
                        &path,
                        &loaded_files,
                        &assets,
                        limits,
                        &mut resident_bytes,
                    )?;
                    assets.insert(name.clone(), (asset, Some(path.clone())));
                    loaded_files.entry(path).or_insert_with(|| name.clone());
                }
                ManifestBufferBinding::Inline(inline) => {
                    let asset =
                        load_inline_buffer_asset(&inline.inline, limits, &mut resident_bytes)?;
                    assets.insert(name.clone(), (asset, None));
                }
                ManifestBufferBinding::Array(elements) => {
                    for (index, element) in elements.iter().enumerate() {
                        let Some(element) = element else { continue };
                        let logical_name = format!("{name}[{index}]");
                        if excluded.contains(logical_name.as_str()) {
                            continue;
                        }
                        match element {
                            ManifestBufferElementBinding::File(file) => {
                                let path = resolve_contained_path(&self.root, &file.file)?;
                                let asset = clone_or_load_buffer_asset(
                                    &path,
                                    &loaded_files,
                                    &assets,
                                    limits,
                                    &mut resident_bytes,
                                )?;
                                assets.insert(logical_name.clone(), (asset, Some(path.clone())));
                                loaded_files.entry(path).or_insert(logical_name);
                            }
                            ManifestBufferElementBinding::Inline(inline) => {
                                let asset = load_inline_buffer_asset(
                                    &inline.inline,
                                    limits,
                                    &mut resident_bytes,
                                )?;
                                assets.insert(logical_name, (asset, None));
                            }
                        }
                    }
                }
            }
        }
        Ok(assets)
    }
}

fn clone_or_load_buffer_asset(
    path: &Path,
    loaded_files: &BTreeMap<PathBuf, String>,
    assets: &BTreeMap<String, (BufferAsset, Option<PathBuf>)>,
    limits: ProjectLimits,
    resident_bytes: &mut usize,
) -> Result<BufferAsset, ProjectError> {
    if let Some(logical_name) = loaded_files.get(path) {
        let asset = assets
            .get(logical_name)
            .map(|(asset, _)| asset)
            .ok_or_else(|| ProjectError::new("project buffer file cache is inconsistent"))?;
        reserve_resident_asset_bytes(resident_bytes, asset.payload_bytes(), &limits)?;
        return Ok(asset.clone());
    }
    let asset = crate::load_buffer_file(path, limits.with_remaining_asset_budget(*resident_bytes))?;
    reserve_resident_asset_bytes(resident_bytes, asset.payload_bytes(), &limits)?;
    Ok(asset)
}

fn load_inline_buffer_asset(
    inline: &InlineBuffer,
    limits: ProjectLimits,
    resident_bytes: &mut usize,
) -> Result<BufferAsset, ProjectError> {
    let asset = inline.to_asset(&limits.with_remaining_asset_budget(*resident_bytes))?;
    reserve_resident_asset_bytes(resident_bytes, asset.payload_bytes(), &limits)?;
    Ok(asset)
}

fn reserve_resident_asset_bytes(
    resident_bytes: &mut usize,
    payload_bytes: usize,
    limits: &ProjectLimits,
) -> Result<(), ProjectError> {
    let total = resident_bytes
        .checked_add(payload_bytes)
        .ok_or_else(|| ProjectError::new("project buffer payload total overflows"))?;
    if total > limits.max_total_asset_bytes {
        return Err(ProjectError::new(format!(
            "project buffer payloads exceed the {} byte resident limit",
            limits.max_total_asset_bytes
        )));
    }
    *resident_bytes = total;
    Ok(())
}

pub fn resolve_project_input(
    input: impl AsRef<Path>,
    limits: ProjectLimits,
) -> Result<ProjectInput, ProjectError> {
    let input = input.as_ref();
    let metadata = fs::metadata(input).map_err(|error| {
        ProjectError::new(format!(
            "failed to inspect project input '{}': {error}",
            input.display()
        ))
    })?;
    if metadata.is_dir() {
        return Err(ProjectError::new(format!(
            "project input '{}' is a directory; choose an .onda or .ondaproject file",
            input.display()
        )));
    }
    if !is_project_file_path(input) {
        return Ok(ProjectInput::Source(input.to_path_buf()));
    }
    let manifest_path = input.to_path_buf();
    let manifest_bytes = crate::read_bounded_file(
        &manifest_path,
        limits.max_manifest_bytes,
        "project manifest",
        "file",
    )?;
    let manifest: ProjectManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        ProjectError::new(format!(
            "failed to parse project manifest '{}': {error}",
            manifest_path.display()
        ))
    })?;
    let project_file_name = manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ProjectError::new("project filename must be valid UTF-8"))?;
    manifest.validate_for_project_file(project_file_name, &limits)?;
    let root_path = project_root_path(&manifest_path);
    let root = fs::canonicalize(root_path).map_err(|error| {
        ProjectError::new(format!(
            "failed to resolve project root '{}': {error}",
            root_path.display()
        ))
    })?;
    let entry_path = resolve_contained_path(&root, &manifest.entry)?;
    let canonical_manifest = fs::canonicalize(&manifest_path).map_err(|error| {
        ProjectError::new(format!(
            "failed to resolve project file '{}': {error}",
            manifest_path.display()
        ))
    })?;
    if entry_path == canonical_manifest {
        return Err(ProjectError::new(
            "project entry cannot be its .ondaproject file",
        ));
    }
    if !entry_path.is_file() {
        return Err(ProjectError::new(format!(
            "project entry '{}' is not a file",
            entry_path.display()
        )));
    }
    Ok(ProjectInput::Project(ProjectFile {
        root,
        manifest_path: canonical_manifest,
        manifest,
        entry_path,
    }))
}

fn project_root_path(manifest_path: &Path) -> &Path {
    manifest_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub fn is_project_file_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(ONDA_PROJECT_FILE_EXTENSION))
}

pub(crate) fn validate_relative_project_path(
    value: &str,
    limits: &ProjectLimits,
) -> Result<(), ProjectError> {
    if value.is_empty() || value.len() > limits.max_path_bytes {
        return Err(ProjectError::new(format!(
            "project paths must contain 1..={} UTF-8 bytes",
            limits.max_path_bytes
        )));
    }
    if !is_nfc(value) {
        return Err(ProjectError::new(format!(
            "project path '{value}' must use Unicode NFC normalization"
        )));
    }
    let bytes = value.as_bytes();
    if value.starts_with('/')
        || value.starts_with('\\')
        || matches!(bytes, [drive, b':', ..] if drive.is_ascii_alphabetic())
        || value.contains('\\')
    {
        return Err(ProjectError::new(format!(
            "project path '{value}' must be a portable relative path using '/' separators"
        )));
    }
    if value.split('/').any(|component| {
        component.is_empty()
            || component.len() > limits.max_path_component_bytes
            || component == "."
            || component == ".."
            || component.bytes().any(|byte| {
                byte < 32 || matches!(byte, b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*')
            })
            || component.ends_with('.')
            || component.ends_with(' ')
            || is_windows_reserved_component(component)
    }) {
        return Err(ProjectError::new(format!(
            "project path '{value}' contains a non-portable component"
        )));
    }
    for component in Path::new(value).components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(ProjectError::new(format!(
                    "project path '{value}' is not normalized or escapes the project"
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_project_buffer_name(
    value: &str,
    limits: &ProjectLimits,
) -> Result<(), ProjectError> {
    if value.is_empty() {
        return Err(ProjectError::new("project buffer names must not be empty"));
    }
    if value.len() > limits.max_path_bytes {
        return Err(ProjectError::new(format!(
            "project buffer name exceeds the {} byte limit",
            limits.max_path_bytes
        )));
    }
    if value.contains('\0') {
        return Err(ProjectError::new(
            "project buffer names must not contain NUL",
        ));
    }
    if let Some((base, index_text)) = buffer_element_suffix(value) {
        let index = index_text.parse::<usize>().map_err(|_| {
            ProjectError::new(format!(
                "project buffer slot '{value}' has an index which does not fit this host"
            ))
        })?;
        if base.is_empty() || index_text != index.to_string() {
            return Err(ProjectError::new(format!(
                "project buffer slot '{value}' must use canonical '[{index}]' notation"
            )));
        }
        if index >= limits.max_buffer_bindings {
            return Err(ProjectError::new(format!(
                "project buffer slot '{value}' exceeds the {} slot limit",
                limits.max_buffer_bindings
            )));
        }
    }
    Ok(())
}

pub(crate) fn portable_path_collision_key(value: &str) -> String {
    // Closing over both case mappings catches Unicode caseless equivalents
    // which a single lowercase pass misses. Over-collapsing is intentional:
    // rejecting two distinct names is safer than losing one on a host whose
    // filesystem considers them equal.
    value
        .to_lowercase()
        .to_uppercase()
        .to_lowercase()
        .nfc()
        .collect()
}

pub(crate) fn validate_project_file_name(
    project_file_name: &str,
    limits: &ProjectLimits,
) -> Result<(), ProjectError> {
    validate_relative_project_path(project_file_name, limits)?;
    if project_file_name.contains('/') || !is_project_file_path(Path::new(project_file_name)) {
        return Err(ProjectError::new(format!(
            "project filename '{project_file_name}' must be a .ondaproject basename"
        )));
    }
    Ok(())
}

pub(crate) fn parse_buffer_element_name(name: &str) -> Option<(&str, usize)> {
    let (base, index_text) = buffer_element_suffix(name)?;
    let index = index_text.parse::<usize>().ok()?;
    (!base.is_empty() && index_text == index.to_string()).then_some((base, index))
}

fn buffer_element_suffix(name: &str) -> Option<(&str, &str)> {
    let open = name.rfind('[')?;
    let index = name.get(open + 1..name.len().checked_sub(1)?)?;
    (name.ends_with(']') && !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some((&name[..open], index))
}

pub(crate) fn is_windows_reserved_component(component: &str) -> bool {
    let stem = component
        .split_once('.')
        .map_or(component, |(stem, _)| stem)
        .to_ascii_lowercase();
    matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$"
    ) || stem
        .strip_prefix("com")
        .or_else(|| stem.strip_prefix("lpt"))
        .is_some_and(is_windows_reserved_port_suffix)
}

fn is_windows_reserved_port_suffix(suffix: &str) -> bool {
    matches!(suffix.as_bytes(), [b'1'..=b'9']) || matches!(suffix, "¹" | "²" | "³")
}

fn resolve_contained_path(root: &Path, relative: &str) -> Result<PathBuf, ProjectError> {
    validate_relative_project_path(relative, &ProjectLimits::default())?;
    let candidate = root.join(relative);
    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        ProjectError::new(format!(
            "failed to resolve project file '{}': {error}",
            candidate.display()
        ))
    })?;
    if !canonical.starts_with(root) {
        return Err(ProjectError::new(format!(
            "project file '{}' resolves outside the project root",
            candidate.display()
        )));
    }
    Ok(canonical)
}

fn resolve_contained_watch_path(
    root: &Path,
    relative: &str,
) -> Result<(PathBuf, Option<PathBuf>), ProjectError> {
    validate_relative_project_path(relative, &ProjectLimits::default())?;
    let candidate = root.join(relative);
    let mut existing_ancestor = candidate.as_path();

    let canonical_ancestor = loop {
        match fs::canonicalize(existing_ancestor) {
            Ok(path) => break path,
            Err(error)
                if existing_ancestor != root
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
            {
                existing_ancestor = existing_ancestor
                    .parent()
                    .expect("a project path below its root has a parent");
            }
            Err(error) => {
                return Err(ProjectError::new(format!(
                    "failed to resolve project file '{}': {error}",
                    candidate.display()
                )));
            }
        }
    };

    if !canonical_ancestor.starts_with(root) {
        return Err(ProjectError::new(format!(
            "project file '{}' resolves outside the project root",
            candidate.display()
        )));
    }

    let unresolved_suffix = candidate
        .strip_prefix(existing_ancestor)
        .expect("the nearest existing ancestor belongs to the project path");
    let resolved = canonical_ancestor.join(unresolved_suffix);
    let alias = (candidate != resolved).then_some(candidate);
    Ok((resolved, alias))
}

fn parse_i32(value: &Value) -> Result<i32, ProjectError> {
    let raw = value
        .as_i64()
        .ok_or_else(|| ProjectError::new("i32 buffer values must be JSON integers"))?;
    i32::try_from(raw)
        .map_err(|_| ProjectError::new(format!("i32 buffer value {raw} is out of range")))
}

fn parse_i64(value: &Value) -> Result<i64, ProjectError> {
    let text = value
        .as_str()
        .ok_or_else(|| ProjectError::new("i64 buffer values must be decimal strings"))?;
    text.parse::<i64>()
        .map_err(|_| ProjectError::new(format!("invalid i64 buffer value '{text}'")))
}

fn parse_f32(value: &Value) -> Result<f32, ProjectError> {
    let raw = value
        .as_f64()
        .ok_or_else(|| ProjectError::new("f32 buffer values must be JSON numbers"))?;
    let converted = raw as f32;
    if !converted.is_finite() {
        return Err(ProjectError::new(format!(
            "f32 buffer value {raw} is not finite after conversion"
        )));
    }
    Ok(converted)
}

fn parse_f64(value: &Value) -> Result<f64, ProjectError> {
    let raw = value
        .as_f64()
        .ok_or_else(|| ProjectError::new("f64 buffer values must be JSON numbers"))?;
    if !raw.is_finite() {
        return Err(ProjectError::new("f64 buffer values must be finite"));
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_project_filename_uses_the_current_directory_as_its_root() {
        assert_eq!(
            project_root_path(Path::new("session.ondaproject")),
            Path::new(".")
        );
        assert_eq!(
            project_root_path(Path::new("projects/session.ondaproject")),
            Path::new("projects")
        );
    }

    #[test]
    fn portable_collision_keys_close_over_unicode_case_mappings() {
        for (left, right) in [("σ.onda", "ς.onda"), ("ẞ.onda", "ss.onda")] {
            assert_eq!(
                portable_path_collision_key(left),
                portable_path_collision_key(right)
            );
        }
    }
}
