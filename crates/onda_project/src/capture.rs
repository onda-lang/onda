use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use onda_frontend::{
    rewrite_source_references, SourceManifest, SourceReferenceKind as FrontendReferenceKind,
    SourceReferenceRewrite,
};
use unicode_normalization::UnicodeNormalization;

use crate::manifest::{is_windows_reserved_component, portable_path_collision_key};
use crate::{
    ProjectError, ProjectLimits, SourceDocument, SourceImage, SourceReferenceKind,
    SourceResolution, ONDA_PROJECT_DEFAULT_FILE_NAME,
};

pub(crate) const CANONICAL_CODE_DIRECTORY: &str = "code";
pub(crate) const CANONICAL_ENTRY_PATH: &str = "code/main.onda";

impl SourceImage {
    /// Builds an exact source image from an already-portable frontend graph.
    ///
    /// Unlike [`SourceImage::capture`], this does not relocate documents or
    /// rewrite references. Every contributing path must already be contained
    /// below `source_root`, making it suitable for virtual/browser workspaces.
    pub fn from_portable_manifest(
        entry: impl AsRef<Path>,
        source_root: impl AsRef<Path>,
        manifest: &SourceManifest,
        limits: ProjectLimits,
    ) -> Result<Self, ProjectError> {
        if !manifest.unresolved_resolutions.is_empty() {
            return Err(ProjectError::new(
                "cannot capture a source graph with unresolved references",
            ));
        }
        let source_root = lexical_path(source_root.as_ref());
        let portable = |path: &Path| -> Result<String, ProjectError> {
            let path = lexical_path(path);
            let relative = path.strip_prefix(&source_root).map_err(|_| {
                ProjectError::new(format!(
                    "source path '{}' is outside its portable root '{}'",
                    path.display(),
                    source_root.display()
                ))
            })?;
            sanitized_portable_path(relative)
        };
        let documents = manifest
            .documents
            .iter()
            .map(|document| {
                Ok(SourceDocument {
                    path: portable(&document.path)?,
                    contents: document.contents.clone(),
                })
            })
            .collect::<Result<Vec<_>, ProjectError>>()?;
        let resolutions = manifest
            .resolutions
            .iter()
            .map(|resolution| {
                Ok(SourceResolution {
                    source: portable(&resolution.source)?,
                    kind: reference_kind(resolution.kind),
                    specifier: resolution.specifier.clone(),
                    target: portable(&resolution.target)?,
                })
            })
            .collect::<Result<Vec<_>, ProjectError>>()?;
        let mut image = Self {
            entry: portable(entry.as_ref())?,
            stdlib_digest: crate::current_stdlib_digest(),
            documents,
            resolutions,
        };
        image
            .documents
            .sort_by(|left, right| left.path.cmp(&right.path));
        image.resolutions.sort();
        image.validate(&limits)?;
        Ok(image)
    }

    /// Captures one successful frontend source graph into portable paths.
    ///
    /// Documents below `source_root` keep their relative layout after unsafe
    /// path components are sanitized. Documents outside that root are placed
    /// below `external/`. All include/import specifiers are then rewritten
    /// syntax-aware so the graph can be replayed without the original
    /// filesystem.
    pub fn capture(
        entry: impl AsRef<Path>,
        source_root: impl AsRef<Path>,
        manifest: &SourceManifest,
        limits: ProjectLimits,
    ) -> Result<Self, ProjectError> {
        if !manifest.unresolved_resolutions.is_empty() {
            return Err(ProjectError::new(
                "cannot capture a source graph with unresolved references",
            ));
        }

        let mut originals = BTreeMap::<PathBuf, &onda_frontend::SourceDocument>::new();
        for document in &manifest.documents {
            let identity = absolute_lexical(&document.path)?;
            if originals.insert(identity.clone(), document).is_some() {
                return Err(ProjectError::new(format!(
                    "source graph contains duplicate document '{}'",
                    identity.display()
                )));
            }
        }
        for path in &manifest.files {
            let identity = absolute_lexical(path)?;
            if !originals.contains_key(&identity) {
                return Err(ProjectError::new(format!(
                    "source graph is missing the exact contents of '{}'",
                    identity.display()
                )));
            }
        }
        let entry = matching_entry_identity(entry.as_ref(), &originals)?;
        let source_root = matching_source_root(source_root.as_ref(), &originals)?;
        if !originals.contains_key(&entry) {
            return Err(ProjectError::new(format!(
                "source graph does not contain its entry '{}'",
                entry.display()
            )));
        }

        let mut output_paths = BTreeMap::<PathBuf, String>::new();
        let mut used_paths =
            BTreeSet::from([portable_path_collision_key(ONDA_PROJECT_DEFAULT_FILE_NAME)]);
        assign_output_path(
            &entry,
            &source_root,
            true,
            &mut used_paths,
            &mut output_paths,
        )?;
        for identity in originals.keys() {
            if identity != &entry {
                assign_output_path(
                    identity,
                    &source_root,
                    false,
                    &mut used_paths,
                    &mut output_paths,
                )?;
            }
        }

        let mut resolutions_by_source =
            BTreeMap::<PathBuf, Vec<(SourceReferenceRewrite, SourceResolution)>>::new();
        for resolution in &manifest.resolutions {
            let source = absolute_lexical(&resolution.source)?;
            let target = absolute_lexical(&resolution.target)?;
            let source_output = output_paths.get(&source).ok_or_else(|| {
                ProjectError::new(format!(
                    "source resolution starts at missing document '{}'",
                    source.display()
                ))
            })?;
            let target_output = output_paths.get(&target).ok_or_else(|| {
                ProjectError::new(format!(
                    "source resolution targets missing document '{}'",
                    target.display()
                ))
            })?;
            let kind = reference_kind(resolution.kind);
            let replacement = relocated_specifier(kind, source_output, target_output)?;
            resolutions_by_source.entry(source).or_default().push((
                SourceReferenceRewrite {
                    kind: resolution.kind,
                    specifier: resolution.specifier.clone(),
                    replacement: replacement.clone(),
                },
                SourceResolution {
                    source: source_output.clone(),
                    kind,
                    specifier: replacement,
                    target: target_output.clone(),
                },
            ));
        }

        let mut documents = Vec::with_capacity(originals.len());
        let mut resolutions = Vec::with_capacity(manifest.resolutions.len());
        for (identity, document) in originals {
            let output_path = output_paths.get(&identity).ok_or_else(|| {
                ProjectError::new(format!(
                    "source document '{}' has no portable path",
                    identity.display()
                ))
            })?;
            let source_rewrites = resolutions_by_source.remove(&identity).unwrap_or_default();
            let contents = rewrite_relocated_document(
                &document.path,
                &document.contents,
                source_rewrites.iter().map(|(rewrite, _)| rewrite),
            )?;
            documents.push(SourceDocument {
                path: output_path.clone(),
                contents,
            });
            resolutions.extend(
                source_rewrites
                    .into_iter()
                    .map(|(_, resolution)| resolution),
            );
        }
        if !resolutions_by_source.is_empty() {
            return Err(ProjectError::new(
                "source graph contains resolutions from unknown documents",
            ));
        }

        let image = Self {
            entry: output_paths
                .get(&entry)
                .ok_or_else(|| ProjectError::new("captured entry has no portable path"))?
                .clone(),
            stdlib_digest: crate::current_stdlib_digest(),
            documents,
            resolutions,
        };
        image.validate(&limits)?;
        Ok(image)
    }

    /// Relocates a logical source image into Onda's canonical exported layout.
    ///
    /// The entry receives the stable `code/main.onda` identity. Other documents
    /// retain their path relative to the capture root, and all exact
    /// non-standard-library references are rewritten to their new targets.
    pub(crate) fn canonical_export(&self, limits: &ProjectLimits) -> Result<Self, ProjectError> {
        self.validate(limits)?;

        let mut used_paths = BTreeSet::new();
        used_paths.insert(portable_path_collision_key(CANONICAL_ENTRY_PATH));
        let mut output_paths =
            BTreeMap::from([(self.entry.clone(), CANONICAL_ENTRY_PATH.to_owned())]);

        let mut document_paths = self
            .documents
            .iter()
            .map(|document| document.path.as_str())
            .filter(|path| *path != self.entry)
            .collect::<Vec<_>>();
        document_paths.sort_unstable();
        for path in document_paths {
            let candidate = if self.entry == CANONICAL_ENTRY_PATH && path.starts_with("code/") {
                path.to_owned()
            } else {
                format!("{CANONICAL_CODE_DIRECTORY}/{path}")
            };
            let output = make_unique_path(candidate, &mut used_paths)?;
            output_paths.insert(path.to_owned(), output);
        }

        let mut resolutions_by_source =
            BTreeMap::<&str, Vec<(SourceReferenceRewrite, SourceResolution)>>::new();
        let mut targets_by_source = BTreeMap::<&str, Vec<&str>>::new();
        for resolution in &self.resolutions {
            let source = output_paths.get(&resolution.source).ok_or_else(|| {
                ProjectError::new("source image resolution starts at a missing document")
            })?;
            let target = output_paths.get(&resolution.target).ok_or_else(|| {
                ProjectError::new("source image resolution targets a missing document")
            })?;
            let replacement = relocated_specifier(resolution.kind, source, target)?;
            targets_by_source
                .entry(&resolution.source)
                .or_default()
                .push(&resolution.target);
            resolutions_by_source
                .entry(&resolution.source)
                .or_default()
                .push((
                    SourceReferenceRewrite {
                        kind: frontend_reference_kind(resolution.kind),
                        specifier: resolution.specifier.clone(),
                        replacement: replacement.clone(),
                    },
                    SourceResolution {
                        source: source.clone(),
                        kind: resolution.kind,
                        specifier: replacement,
                        target: target.clone(),
                    },
                ));
        }

        let mut reachable = BTreeSet::new();
        let mut pending = vec![self.entry.as_str()];
        while let Some(source) = pending.pop() {
            if !reachable.insert(source) {
                continue;
            }
            if let Some(targets) = targets_by_source.get(source) {
                pending.extend(targets.iter().copied());
            }
        }

        let mut documents = Vec::with_capacity(self.documents.len());
        let mut resolutions = Vec::with_capacity(self.resolutions.len());
        for document in &self.documents {
            let source_rewrites = resolutions_by_source
                .remove(document.path.as_str())
                .unwrap_or_default();
            let contents =
                if reachable.contains(document.path.as_str()) || !source_rewrites.is_empty() {
                    rewrite_relocated_document(
                        Path::new(&document.path),
                        &document.contents,
                        source_rewrites.iter().map(|(rewrite, _)| rewrite),
                    )?
                } else {
                    document.contents.clone()
                };
            documents.push(SourceDocument {
                path: output_paths
                    .get(&document.path)
                    .ok_or_else(|| ProjectError::new("source document has no exported path"))?
                    .clone(),
                contents,
            });
            resolutions.extend(
                source_rewrites
                    .into_iter()
                    .map(|(_, resolution)| resolution),
            );
        }
        if !resolutions_by_source.is_empty() {
            return Err(ProjectError::new(
                "source image contains resolutions from unknown documents",
            ));
        }

        let mut exported = Self {
            entry: CANONICAL_ENTRY_PATH.to_owned(),
            stdlib_digest: self.stdlib_digest.clone(),
            documents,
            resolutions,
        };
        exported
            .documents
            .sort_by(|left, right| left.path.cmp(&right.path));
        exported.resolutions.sort();
        exported.validate(limits)?;
        Ok(exported)
    }
}

fn rewrite_relocated_document<'a>(
    path: &Path,
    contents: &str,
    rewrites: impl IntoIterator<Item = &'a SourceReferenceRewrite>,
) -> Result<String, ProjectError> {
    let rewrites = rewrites.into_iter().cloned().collect::<Vec<_>>();
    rewrite_source_references(path, contents, &rewrites).map_err(|diagnostics| {
        let detail = diagnostics
            .first()
            .map_or("unknown rewrite error", |diagnostic| {
                diagnostic.message.as_str()
            });
        ProjectError::new(format!(
            "failed to relocate source '{}': {detail}",
            path.display()
        ))
    })
}

fn lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn sanitized_portable_path(path: &Path) -> Result<String, ProjectError> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_str().ok_or_else(|| {
                    ProjectError::new(format!(
                        "source path '{}' is not valid UTF-8",
                        path.display()
                    ))
                })?;
                if value.is_empty() || matches!(value, "." | "..") || value.contains('\\') {
                    return Err(ProjectError::new(format!(
                        "source path '{}' is not portable",
                        path.display()
                    )));
                }
                components.push(value);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ProjectError::new(format!(
                    "source path '{}' is not portable",
                    path.display()
                )));
            }
        }
    }
    if components.is_empty() {
        return Err(ProjectError::new("portable source path is empty"));
    }
    Ok(components.join("/"))
}

fn assign_output_path(
    identity: &Path,
    source_root: &Path,
    is_entry: bool,
    used_paths: &mut BTreeSet<String>,
    output_paths: &mut BTreeMap<PathBuf, String>,
) -> Result<(), ProjectError> {
    let candidate = if let Ok(relative) = identity.strip_prefix(source_root) {
        sanitized_path(relative)?
    } else {
        let filename = identity
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("source.onda");
        format!("external/{}", sanitize_component(filename))
    };
    let candidate = if is_entry && candidate.is_empty() {
        sanitize_component(
            identity
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("main.onda"),
        )
    } else {
        candidate
    };
    let candidate = canonical_source_extension(candidate);
    let candidate = if candidate
        .to_ascii_lowercase()
        .starts_with(&format!("{ONDA_PROJECT_DEFAULT_FILE_NAME}/"))
    {
        format!("_{candidate}")
    } else {
        candidate
    };
    let unique = make_unique_path(candidate, used_paths)?;
    output_paths.insert(identity.to_path_buf(), unique);
    Ok(())
}

fn canonical_source_extension(mut path: String) -> String {
    let Some((_, extension)) = path.rsplit_once('.') else {
        return path;
    };
    let canonical = if extension.eq_ignore_ascii_case("onda") {
        "onda"
    } else if extension.eq_ignore_ascii_case("on") {
        "on"
    } else {
        return path;
    };
    path.truncate(path.len() - extension.len());
    path.push_str(canonical);
    path
}

fn sanitized_path(path: &Path) -> Result<String, ProjectError> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_str().ok_or_else(|| {
                    ProjectError::new(format!(
                        "source path '{}' is not valid UTF-8",
                        path.display()
                    ))
                })?;
                components.push(sanitize_component(value));
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ProjectError::new(format!(
                    "source path '{}' is not relative to its capture root",
                    path.display()
                )))
            }
        }
    }
    Ok(components.join("/"))
}

fn sanitize_component(value: &str) -> String {
    let mut sanitized = value
        .nfc()
        .map(|character| {
            if character.is_control()
                || matches!(character, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    while sanitized.ends_with(['.', ' ']) {
        sanitized.pop();
        sanitized.push('_');
    }
    if sanitized.is_empty() || matches!(sanitized.as_str(), "." | "..") {
        sanitized = "_".to_owned();
    }
    if is_windows_reserved_component(&sanitized) {
        sanitized.insert(0, '_');
    }
    sanitized
}

fn make_unique_path(
    candidate: String,
    used_paths: &mut BTreeSet<String>,
) -> Result<String, ProjectError> {
    if candidate.is_empty() {
        return Err(ProjectError::new("portable source path is empty"));
    }
    let Some(component_index) = conflicting_path_component(&candidate, used_paths) else {
        used_paths.insert(portable_path_collision_key(&candidate));
        return Ok(candidate);
    };
    for suffix in 2usize.. {
        let unique = suffix_path_component(&candidate, component_index, suffix);
        if conflicting_path_component(&unique, used_paths).is_none() {
            used_paths.insert(portable_path_collision_key(&unique));
            return Ok(unique);
        }
    }
    Err(ProjectError::new(
        "could not allocate a unique portable source path",
    ))
}

fn conflicting_path_component(candidate: &str, used_paths: &BTreeSet<String>) -> Option<usize> {
    let key = portable_path_collision_key(candidate);
    let mut component_index = 0usize;
    for separator in key.match_indices('/').map(|(index, _)| index) {
        if used_paths.contains(&key[..separator]) {
            return Some(component_index);
        }
        component_index += 1;
    }
    if used_paths.contains(&key) {
        return Some(component_index);
    }
    let descendant_prefix = format!("{key}/");
    used_paths
        .range(descendant_prefix.clone()..)
        .next()
        .is_some_and(|path| path.starts_with(&descendant_prefix))
        .then_some(component_index)
}

fn suffix_path_component(path: &str, component_index: usize, suffix: usize) -> String {
    path.split('/')
        .enumerate()
        .map(|(index, component)| {
            if index != component_index {
                return component.to_owned();
            }
            match component.rsplit_once('.') {
                Some((stem, extension)) => format!("{stem}-{suffix}.{extension}"),
                None => format!("{component}-{suffix}"),
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn relocated_specifier(
    kind: SourceReferenceKind,
    source: &str,
    target: &str,
) -> Result<String, ProjectError> {
    let source_parent = source.rsplit_once('/').map_or("", |(parent, _)| parent);
    let mut relative = relative_portable_path(source_parent, target);
    if kind == SourceReferenceKind::Import {
        let lowercase = relative.to_ascii_lowercase();
        if lowercase.ends_with(".onda") {
            relative.truncate(relative.len() - ".onda".len());
        } else if lowercase.ends_with(".on") {
            relative.truncate(relative.len() - ".on".len());
        } else {
            return Err(ProjectError::new(format!(
                "import target '{target}' does not have an Onda source extension"
            )));
        }
    }
    if relative.is_empty() {
        return Err(ProjectError::new("relocated source specifier is empty"));
    }
    Ok(relative)
}

fn relative_portable_path(from_directory: &str, target: &str) -> String {
    let from = from_directory
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let to = target
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut result = vec![".."; from.len().saturating_sub(common)];
    result.extend_from_slice(&to[common..]);
    result.join("/")
}

fn reference_kind(kind: FrontendReferenceKind) -> SourceReferenceKind {
    match kind {
        FrontendReferenceKind::Include => SourceReferenceKind::Include,
        FrontendReferenceKind::Import => SourceReferenceKind::Import,
    }
}

fn frontend_reference_kind(kind: SourceReferenceKind) -> FrontendReferenceKind {
    match kind {
        SourceReferenceKind::Include => FrontendReferenceKind::Include,
        SourceReferenceKind::Import => FrontendReferenceKind::Import,
    }
}

fn matching_entry_identity(
    path: &Path,
    originals: &BTreeMap<PathBuf, &onda_frontend::SourceDocument>,
) -> Result<PathBuf, ProjectError> {
    let lexical = absolute_lexical(path)?;
    if originals.contains_key(&lexical) {
        return Ok(lexical);
    }
    Ok(fs::canonicalize(path)
        .ok()
        .filter(|canonical| originals.contains_key(canonical))
        .unwrap_or(lexical))
}

fn matching_source_root(
    path: &Path,
    originals: &BTreeMap<PathBuf, &onda_frontend::SourceDocument>,
) -> Result<PathBuf, ProjectError> {
    let lexical = absolute_lexical(path)?;
    if originals
        .keys()
        .any(|identity| identity.starts_with(&lexical))
    {
        return Ok(lexical);
    }
    Ok(fs::canonicalize(path)
        .ok()
        .filter(|canonical| {
            originals
                .keys()
                .any(|identity| identity.starts_with(canonical))
        })
        .unwrap_or(lexical))
}

fn absolute_lexical(path: &Path) -> Result<PathBuf, ProjectError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| ProjectError::new(format!("failed to resolve source path: {error}")))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(ProjectError::new(format!(
                        "source path '{}' escapes its filesystem root",
                        path.display()
                    )));
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}
