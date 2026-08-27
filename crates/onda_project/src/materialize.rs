use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::manifest::{
    parse_buffer_element_name, portable_path_collision_key, validate_project_file_name,
    validate_relative_project_path, ManifestBufferBinding, ManifestBufferElementBinding,
    ManifestBufferFile,
};
use crate::{
    decode_buffer_bytes, encode_ondabuffer, is_ondabuffer, validate_ondabuffer, AssetId,
    ProjectError, ProjectImage, ProjectLimits, ProjectManifest, SourceDocument, SourceImage,
    ONDA_PROJECT_DEFAULT_FILE_NAME,
};

const CANONICAL_ASSET_DIRECTORY: &str = "assets";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PlannedFile {
    pub relative_path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MaterializationPlan {
    pub directories: Vec<String>,
    pub files: Vec<PlannedFile>,
}

impl MaterializationPlan {
    pub fn validate(&self, limits: &ProjectLimits) -> Result<(), ProjectError> {
        let max_paths = limits.max_materialized_file_count();
        if self.files.len() > max_paths {
            return Err(ProjectError::new(
                "project materialization contains too many files",
            ));
        }
        if self.directories.len() > max_paths {
            return Err(ProjectError::new(
                "project materialization contains too many directories",
            ));
        }

        let mut directories = BTreeMap::new();
        for directory in &self.directories {
            validate_relative_project_path(directory, limits)?;
            let key = portable_path_collision_key(directory);
            if let Some(existing) = directories.insert(key, directory.as_str()) {
                return Err(ProjectError::new(format!(
                    "project directories '{existing}' and '{directory}' collide on case-insensitive filesystems"
                )));
            }
        }

        for file in &self.files {
            validate_relative_project_path(&file.relative_path, limits)?;
        }
        crate::image::validate_portable_file_set(
            self.files.iter().map(|file| file.relative_path.clone()),
        )?;
        let files = self
            .files
            .iter()
            .map(|file| {
                (
                    portable_path_collision_key(&file.relative_path),
                    file.relative_path.as_str(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (directory, original) in &directories {
            if let Some(file) = files.get(directory) {
                return Err(ProjectError::new(format!(
                    "project path '{file}' cannot be both a file and directory '{original}'"
                )));
            }
            let mut separator = directory.find('/');
            while let Some(index) = separator {
                if let Some(file) = files.get(&directory[..index]) {
                    return Err(ProjectError::new(format!(
                        "project file '{file}' conflicts with directory '{original}'"
                    )));
                }
                separator = directory[index + 1..]
                    .find('/')
                    .map(|offset| index + 1 + offset);
            }
        }
        Ok(())
    }
}

impl ProjectImage {
    /// Reconstructs an immutable project image from one exported project's
    /// files without consulting the host filesystem.
    pub fn from_materialized_files(
        files: &BTreeMap<String, Vec<u8>>,
        limits: ProjectLimits,
    ) -> Result<Self, ProjectError> {
        Self::from_materialized_files_impl(files, None, limits)
    }

    /// Reconstructs one explicitly selected project from a materialized file
    /// set which may contain other project manifests.
    pub fn from_materialized_files_with_manifest(
        files: &BTreeMap<String, Vec<u8>>,
        manifest_path: &str,
        limits: ProjectLimits,
    ) -> Result<Self, ProjectError> {
        Self::from_materialized_files_impl(files, Some(manifest_path), limits)
    }

    /// Borrowed counterpart to [`ProjectImage::from_materialized_files`] for
    /// hosts whose transport already owns the file payloads.
    pub fn from_materialized_file_slices(
        files: &BTreeMap<String, &[u8]>,
        limits: ProjectLimits,
    ) -> Result<Self, ProjectError> {
        Self::from_materialized_files_impl(files, None, limits)
    }

    /// Borrowed counterpart to
    /// [`ProjectImage::from_materialized_files_with_manifest`].
    pub fn from_materialized_file_slices_with_manifest(
        files: &BTreeMap<String, &[u8]>,
        manifest_path: &str,
        limits: ProjectLimits,
    ) -> Result<Self, ProjectError> {
        Self::from_materialized_files_impl(files, Some(manifest_path), limits)
    }

    fn from_materialized_files_impl<B: AsRef<[u8]>>(
        files: &BTreeMap<String, B>,
        selected_manifest_path: Option<&str>,
        limits: ProjectLimits,
    ) -> Result<Self, ProjectError> {
        if files.len() > limits.max_materialized_file_count() {
            return Err(ProjectError::new(
                "materialized project contains too many files",
            ));
        }
        let max_file_bytes = limits.max_materialized_file_bytes();
        let max_total_bytes = limits.max_materialized_total_bytes();
        let mut total_bytes = 0usize;
        for (path, bytes) in files {
            let bytes = bytes.as_ref();
            validate_relative_project_path(path, &limits)?;
            if bytes.len() > max_file_bytes {
                return Err(ProjectError::new(format!(
                    "project file '{path}' exceeds the {max_file_bytes} byte file limit"
                )));
            }
            total_bytes = total_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| ProjectError::new("materialized project byte total overflows"))?;
            if total_bytes > max_total_bytes {
                return Err(ProjectError::new(format!(
                    "project files exceed the {max_total_bytes} byte aggregate limit"
                )));
            }
        }
        crate::image::validate_portable_file_set(files.keys().cloned())?;
        let (manifest_path, manifest) = if let Some(manifest_path) = selected_manifest_path {
            let bytes = files.get(manifest_path).ok_or_else(|| {
                ProjectError::new(format!(
                    "selected project manifest '{manifest_path}' is missing"
                ))
            })?;
            let manifest = parse_materialized_manifest(manifest_path, bytes.as_ref(), &limits)?;
            (manifest_path, manifest)
        } else {
            let manifest_candidates = files
                .iter()
                .filter(|(path, _)| crate::is_project_file_path(Path::new(path)))
                .map(|(path, bytes)| (path, bytes.as_ref()))
                .collect::<Vec<_>>();
            if manifest_candidates.is_empty() {
                return Err(ProjectError::new(
                    "materialized project has no .ondaproject file",
                ));
            }
            if manifest_candidates.len() == 1 {
                let (manifest_path, bytes) = manifest_candidates[0];
                let manifest = parse_materialized_manifest(manifest_path, bytes, &limits)?;
                (manifest_path.as_str(), manifest)
            } else {
                let mut manifests = Vec::new();
                for (path, bytes) in &manifest_candidates {
                    if let Ok(manifest) = parse_materialized_manifest(path, bytes, &limits) {
                        manifests.push((path.as_str(), manifest));
                        if manifests.len() > 1 {
                            return Err(ProjectError::new(
                                "materialized project contains more than one .ondaproject file",
                            ));
                        }
                    }
                }
                manifests.pop().ok_or_else(|| {
                    ProjectError::new("materialized project has no valid .ondaproject file")
                })?
            }
        };
        let mut workspace_manifests =
            BTreeMap::from([(manifest_path.to_owned(), manifest.clone())]);
        for (path, bytes) in files {
            if path == manifest_path || !crate::is_project_file_path(Path::new(path)) {
                continue;
            }
            // A file with the project extension may still be an entry
            // source or a buffer asset. Only successfully parsed manifests
            // participate in workspace-wide role classification.
            if let Ok(other_manifest) = parse_materialized_manifest(path, bytes.as_ref(), &limits) {
                workspace_manifests.insert(path.clone(), other_manifest);
            }
        }
        let manifest_paths = workspace_manifests.keys().cloned().collect::<BTreeSet<_>>();
        let workspace_buffer_files = workspace_manifests
            .iter()
            .flat_map(|(path, manifest)| {
                manifest_buffer_file_paths(manifest)
                    .into_iter()
                    .map(|file| resolve_materialized_reference(path, file, &limits))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let entry_path = resolve_materialized_reference(manifest_path, &manifest.entry, &limits)?;
        if let Some(path) = manifest_paths
            .iter()
            .find(|path| workspace_buffer_files.contains(*path))
        {
            return Err(ProjectError::new(format!(
                "workspace path '{path}' cannot be both a project manifest and a buffer asset"
            )));
        }
        if workspace_buffer_files.contains(&entry_path) {
            return Err(ProjectError::new(format!(
                "selected project entry '{entry_path}' is also claimed as a buffer asset by a project manifest"
            )));
        }

        let root = PathBuf::from("onda-project");
        let mut overlays = HashMap::new();
        let mut source_documents = BTreeMap::new();
        for (path, bytes) in files {
            let bytes = bytes.as_ref();
            if manifest_paths.contains(path) || workspace_buffer_files.contains(path) {
                continue;
            }
            let source_extension = Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("onda") || extension.eq_ignore_ascii_case("on")
                });
            if path != &entry_path && !source_extension {
                continue;
            }
            let contents = std::str::from_utf8(bytes).map_err(|_| {
                ProjectError::new(format!("project source '{path}' is not valid UTF-8"))
            })?;
            source_documents.insert(path.clone(), contents.to_owned());
            overlays.insert(root.join(path), contents.to_owned());
        }
        let entry = root.join(&entry_path);
        let loaded =
            onda_frontend::load_program_file_from_virtual_sources(&root, &entry, &overlays)
                .map_err(|error| {
                    let detail = error
                        .diagnostics
                        .first()
                        .map_or("source graph load failed", |diagnostic| {
                            diagnostic.message.as_str()
                        });
                    ProjectError::new(format!("failed to load materialized project: {detail}"))
                })?;
        let mut sources =
            SourceImage::from_portable_manifest(&entry, &root, &loaded.sources, limits)?;
        let reachable = sources
            .documents
            .iter()
            .map(|document| document.path.clone())
            .collect::<BTreeSet<_>>();
        sources.documents.extend(
            source_documents
                .into_iter()
                .filter(|(path, _)| !reachable.contains(path))
                .map(|(path, contents)| SourceDocument { path, contents }),
        );
        sources
            .documents
            .sort_by(|left, right| left.path.cmp(&right.path));
        sources.validate(&limits)?;
        let constants = manifest.constants.clone();
        let mut buffer_bindings = BTreeMap::new();
        let mut assets = BTreeMap::new();
        let mut file_assets = BTreeMap::<String, AssetId>::new();
        let mut total_buffer_bytes = 0usize;
        for (name, binding) in manifest.buffers {
            let bindings = match binding {
                ManifestBufferBinding::File(file) => {
                    vec![(name, ManifestBufferElementBinding::File(file))]
                }
                ManifestBufferBinding::Inline(inline) => {
                    vec![(name, ManifestBufferElementBinding::Inline(inline))]
                }
                ManifestBufferBinding::Array(elements) => elements
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, element)| {
                        element.map(|element| (format!("{name}[{index}]"), element))
                    })
                    .collect(),
            };
            for (logical_name, binding) in bindings {
                match binding {
                    ManifestBufferElementBinding::File(binding) => {
                        let file =
                            resolve_materialized_reference(manifest_path, &binding.file, &limits)?;
                        if let Some(id) = file_assets.get(&file) {
                            buffer_bindings.insert(logical_name, id.clone());
                            continue;
                        }
                        let bytes = files.get(&file).ok_or_else(|| {
                            ProjectError::new(format!("project buffer file '{file}' is missing"))
                        })?;
                        let bytes = bytes.as_ref();
                        if is_ondabuffer(bytes) {
                            let validated = validate_ondabuffer(bytes, limits)?;
                            let id = AssetId::from_buffer_digest(validated.content_digest());
                            if assets.contains_key(&id) {
                                buffer_bindings.insert(logical_name, id.clone());
                                file_assets.insert(file, id);
                                continue;
                            }
                            let asset = validated
                                .decode_with_remaining_asset_budget(limits, total_buffer_bytes)?;
                            let id = insert_materialized_asset(
                                &mut buffer_bindings,
                                &mut assets,
                                &mut total_buffer_bytes,
                                &limits,
                                logical_name,
                                asset,
                            )?;
                            file_assets.insert(file, id);
                            continue;
                        }
                        let asset = decode_buffer_bytes(
                            bytes,
                            Path::new(&file),
                            limits.with_remaining_asset_budget(total_buffer_bytes),
                        )?;
                        let id = insert_materialized_asset(
                            &mut buffer_bindings,
                            &mut assets,
                            &mut total_buffer_bytes,
                            &limits,
                            logical_name,
                            asset,
                        )?;
                        file_assets.insert(file, id);
                    }
                    ManifestBufferElementBinding::Inline(binding) => {
                        let asset = binding
                            .inline
                            .to_asset(&limits.with_remaining_asset_budget(total_buffer_bytes))?;
                        insert_materialized_asset(
                            &mut buffer_bindings,
                            &mut assets,
                            &mut total_buffer_bytes,
                            &limits,
                            logical_name,
                            asset,
                        )?;
                    }
                }
            }
        }
        let image = Self::new_with_constants(sources, constants, buffer_bindings, assets)?;
        image.validate(&limits)?;
        Ok(image)
    }

    pub fn materialization_plan(&self) -> Result<MaterializationPlan, ProjectError> {
        self.materialization_plan_with_asset_file_names(&BTreeMap::new())
    }

    /// Materializes the image with optional original filenames keyed by logical
    /// buffer name. Filenames affect only the editable export, never asset identity.
    pub fn materialization_plan_with_asset_file_names(
        &self,
        preferred_file_names: &BTreeMap<String, String>,
    ) -> Result<MaterializationPlan, ProjectError> {
        self.materialization_plan_with_file_names(
            ONDA_PROJECT_DEFAULT_FILE_NAME,
            preferred_file_names,
        )
    }

    /// Materializes the image using an explicit root project filename and
    /// optional original filenames keyed by logical buffer name.
    pub fn materialization_plan_with_file_names(
        &self,
        project_file_name: &str,
        preferred_asset_file_names: &BTreeMap<String, String>,
    ) -> Result<MaterializationPlan, ProjectError> {
        self.validate(&ProjectLimits::default())?;
        validate_project_file_name(project_file_name, &ProjectLimits::default())?;
        let sources = self.sources().canonical_export(&ProjectLimits::default())?;
        let mut used_paths = sources
            .documents
            .iter()
            .map(|document| document.path.clone())
            .collect::<BTreeSet<_>>();
        if used_paths.contains(project_file_name) {
            return Err(ProjectError::new(format!(
                "source image reserves '{project_file_name}' for its project file"
            )));
        }
        used_paths.insert(project_file_name.to_owned());
        let mut used_path_keys = used_paths
            .iter()
            .map(|path| portable_path_collision_key(path))
            .collect::<BTreeSet<_>>();

        let mut asset_paths = BTreeMap::new();
        for id in self.assets().keys() {
            let digest = id
                .as_str()
                .strip_prefix("sha256:")
                .ok_or_else(|| ProjectError::new("project asset ID is not SHA-256"))?;
            let stem = preferred_asset_stem(
                self,
                id,
                preferred_asset_file_names,
                CANONICAL_ASSET_DIRECTORY,
            )
            .unwrap_or_else(|| "asset".to_owned());
            let path = unique_asset_path(
                CANONICAL_ASSET_DIRECTORY,
                &stem,
                digest,
                &mut used_path_keys,
            )?;
            asset_paths.insert(id, path);
        }

        let mut buffers = BTreeMap::new();
        let mut buffer_arrays =
            BTreeMap::<String, Vec<Option<ManifestBufferElementBinding>>>::new();
        for (name, id) in self.buffer_bindings() {
            let path = asset_paths.get(id).ok_or_else(|| {
                ProjectError::new(format!(
                    "project buffer '{name}' references missing asset '{}'",
                    id.as_str()
                ))
            })?;
            let binding =
                ManifestBufferElementBinding::File(ManifestBufferFile { file: path.clone() });
            if let Some((base, index)) = parse_buffer_element_name(name) {
                let elements = buffer_arrays.entry(base.to_owned()).or_default();
                if elements.len() <= index {
                    elements.resize(index + 1, None);
                }
                elements[index] = Some(binding);
            } else if let ManifestBufferElementBinding::File(file) = binding {
                buffers.insert(name.clone(), ManifestBufferBinding::File(file));
            }
        }
        for (name, elements) in buffer_arrays {
            buffers.insert(name, ManifestBufferBinding::Array(elements));
        }
        let manifest = ProjectManifest {
            entry: sources.entry.clone(),
            constants: self.constants().clone(),
            buffers,
        };

        let mut files = vec![PlannedFile {
            relative_path: project_file_name.to_owned(),
            bytes: manifest.to_pretty_json()?.into_bytes(),
        }];
        files.extend(sources.documents.iter().map(|document| PlannedFile {
            relative_path: document.path.clone(),
            bytes: document.contents.as_bytes().to_vec(),
        }));
        for (id, asset) in self.assets() {
            let path = asset_paths.get(id).ok_or_else(|| {
                ProjectError::new(format!(
                    "project asset '{}' has no materialized path",
                    id.as_str()
                ))
            })?;
            files.push(PlannedFile {
                relative_path: path.clone(),
                bytes: encode_ondabuffer(asset)?,
            });
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let plan = MaterializationPlan {
            directories: vec![
                CANONICAL_ASSET_DIRECTORY.to_owned(),
                crate::capture::CANONICAL_CODE_DIRECTORY.to_owned(),
            ],
            files,
        };
        plan.validate(&ProjectLimits::default())?;
        Ok(plan)
    }
}

fn insert_materialized_asset(
    buffer_bindings: &mut BTreeMap<String, AssetId>,
    assets: &mut BTreeMap<AssetId, crate::BufferAsset>,
    total_buffer_bytes: &mut usize,
    limits: &ProjectLimits,
    logical_name: String,
    asset: crate::BufferAsset,
) -> Result<AssetId, ProjectError> {
    let id = AssetId::for_buffer(&asset);
    if !assets.contains_key(&id) {
        *total_buffer_bytes = total_buffer_bytes
            .checked_add(asset.payload_bytes())
            .ok_or_else(|| ProjectError::new("project buffer byte total overflows"))?;
        if *total_buffer_bytes > limits.max_total_asset_bytes {
            return Err(ProjectError::new(format!(
                "project buffer payloads exceed the {} byte limit",
                limits.max_total_asset_bytes
            )));
        }
        assets.insert(id.clone(), asset);
    }
    buffer_bindings.insert(logical_name, id.clone());
    Ok(id)
}

fn parse_materialized_manifest(
    path: &str,
    bytes: &[u8],
    limits: &ProjectLimits,
) -> Result<ProjectManifest, ProjectError> {
    validate_relative_project_path(path, limits)?;
    if bytes.len() > limits.max_manifest_bytes {
        return Err(ProjectError::new(format!(
            "project manifest '{path}' exceeds its byte limit"
        )));
    }
    let manifest: ProjectManifest = serde_json::from_slice(bytes)?;
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ProjectError::new("project filename must be valid UTF-8"))?;
    manifest.validate_for_project_file(file_name, limits)?;
    Ok(manifest)
}

fn resolve_materialized_reference(
    manifest_path: &str,
    relative: &str,
    limits: &ProjectLimits,
) -> Result<String, ProjectError> {
    validate_relative_project_path(relative, limits)?;
    let resolved = Path::new(manifest_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || relative.to_owned(),
            |parent| format!("{}/{relative}", parent.to_string_lossy().replace('\\', "/")),
        );
    validate_relative_project_path(&resolved, limits)?;
    Ok(resolved)
}

fn preferred_asset_stem(
    image: &ProjectImage,
    id: &crate::AssetId,
    preferred_file_names: &BTreeMap<String, String>,
    asset_directory: &str,
) -> Option<String> {
    for (name, binding) in image.buffer_bindings() {
        if binding == id {
            if let Some(stem) = preferred_file_names
                .get(name)
                .and_then(|file_name| portable_asset_stem(file_name, asset_directory))
            {
                return Some(stem);
            }
        }
    }
    image
        .buffer_bindings()
        .iter()
        .find(|(_, binding)| *binding == id)
        .and_then(|(name, _)| portable_asset_stem(name, asset_directory))
}

fn portable_asset_stem(file_name: &str, asset_directory: &str) -> Option<String> {
    let stem = Path::new(file_name).file_stem()?.to_str()?;
    let mut sanitized = String::new();
    let mut replacing = false;
    for character in stem.nfc() {
        let invalid = character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            );
        if invalid {
            if !replacing {
                sanitized.push('-');
                replacing = true;
            }
        } else {
            sanitized.push(character);
            replacing = false;
        }
    }
    let mut sanitized = sanitized.trim_matches([' ', '.']).to_owned();
    while sanitized.len() > 120 {
        sanitized.pop();
    }
    let sanitized = sanitized.trim_end_matches([' ', '.']).to_owned();
    if sanitized.is_empty() {
        return None;
    }
    let candidate = format!("{asset_directory}/{sanitized}.ondabuffer");
    if validate_relative_project_path(&candidate, &ProjectLimits::default()).is_ok() {
        Some(sanitized)
    } else {
        let sanitized = format!("_{sanitized}");
        let candidate = format!("{asset_directory}/{sanitized}.ondabuffer");
        validate_relative_project_path(&candidate, &ProjectLimits::default())
            .is_ok()
            .then_some(sanitized)
    }
}

fn unique_asset_path(
    asset_directory: &str,
    stem: &str,
    digest: &str,
    used_path_keys: &mut BTreeSet<String>,
) -> Result<String, ProjectError> {
    let candidates = [
        format!("{asset_directory}/{stem}.ondabuffer"),
        format!("{asset_directory}/{stem}-{}.ondabuffer", &digest[..8]),
        format!("{asset_directory}/{stem}-{digest}.ondabuffer"),
    ];
    for path in candidates {
        if register_materialized_path(used_path_keys, &path) {
            return Ok(path);
        }
    }
    let max_suffix = used_path_keys.len().saturating_add(2);
    for suffix in 2..=max_suffix {
        let path = format!("{asset_directory}/{stem}-{digest}-{suffix}.ondabuffer");
        if register_materialized_path(used_path_keys, &path) {
            return Ok(path);
        }
    }
    Err(ProjectError::new(
        "could not allocate a unique portable asset path",
    ))
}

fn manifest_buffer_file_paths(manifest: &ProjectManifest) -> BTreeSet<&str> {
    let mut files = BTreeSet::new();
    for binding in manifest.buffers.values() {
        match binding {
            ManifestBufferBinding::File(file) => {
                files.insert(file.file.as_str());
            }
            ManifestBufferBinding::Inline(_) => {}
            ManifestBufferBinding::Array(elements) => {
                for element in elements.iter().flatten() {
                    if let ManifestBufferElementBinding::File(file) = element {
                        files.insert(file.file.as_str());
                    }
                }
            }
        }
    }
    files
}

fn register_materialized_path(used_path_keys: &mut BTreeSet<String>, path: &str) -> bool {
    let key = portable_path_collision_key(path);
    let key_prefix = format!("{key}/");
    if used_path_keys.iter().any(|used| {
        used == &key
            || used.starts_with(&key_prefix)
            || key
                .strip_prefix(used)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }) {
        return false;
    }
    used_path_keys.insert(key);
    true
}

pub fn new_project_plan(project_file_name: &str) -> Result<MaterializationPlan, ProjectError> {
    validate_project_file_name(project_file_name, &ProjectLimits::default())?;
    let manifest = ProjectManifest::empty(crate::capture::CANONICAL_ENTRY_PATH);
    let source = concat!(
        "outs 2\n",
        "\n",
        "sample:\n",
        "  out1 = 0.0\n",
        "  out2 = 0.0\n",
    );
    let plan = MaterializationPlan {
        directories: vec![
            CANONICAL_ASSET_DIRECTORY.to_owned(),
            crate::capture::CANONICAL_CODE_DIRECTORY.to_owned(),
        ],
        files: vec![
            PlannedFile {
                relative_path: project_file_name.to_owned(),
                bytes: manifest.to_pretty_json()?.into_bytes(),
            },
            PlannedFile {
                relative_path: crate::capture::CANONICAL_ENTRY_PATH.to_owned(),
                bytes: source.as_bytes().to_vec(),
            },
        ],
    };
    plan.validate(&ProjectLimits::default())?;
    Ok(plan)
}
