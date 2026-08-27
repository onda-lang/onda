use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use onda_project::{
    is_project_file_path, load_buffer_file, new_project_plan, validate_buffer_assets, BufferAsset,
    BufferElement, MaterializationPlan, ProjectBufferChannels, ProjectBufferDeclaration,
    ProjectConstValue, ProjectImage, ProjectLimits, SourceImage, ONDA_PROJECT_FILE_EXTENSION,
};

pub fn create_empty_project(destination: &Path) -> Result<(), String> {
    let project_file_name = project_file_name_from_target(destination)?;
    let plan = new_project_plan(&project_file_name).map_err(|error| error.to_string())?;
    publish_project_plan(destination, &plan)
}

pub fn package_project_from_files(
    destination: &Path,
    source: &Path,
    buffer_bindings: &[(String, PathBuf)],
) -> Result<(), String> {
    let project_file_name = project_file_name_from_target(destination)?;
    let mut assets = BTreeMap::new();
    let mut asset_file_names = BTreeMap::new();
    for (name, path) in buffer_bindings {
        let asset = load_buffer_file(path, ProjectLimits::default()).map_err(|error| {
            format!(
                "failed to load buffer '{name}' from '{}': {error}",
                path.display()
            )
        })?;
        if assets.insert(name.clone(), asset).is_some() {
            return Err(format!("buffer '{name}' was provided more than once"));
        }
        if let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) {
            asset_file_names.insert(name.clone(), file_name.to_owned());
        }
    }
    let plan = package_project_plan(
        source,
        None,
        BTreeMap::new(),
        assets,
        &project_file_name,
        &asset_file_names,
    )?;
    publish_project_plan(destination, &plan)
}

pub fn package_project_plan(
    source: &Path,
    source_root: Option<&Path>,
    constants: BTreeMap<String, ProjectConstValue>,
    assets: BTreeMap<String, BufferAsset>,
    project_file_name: &str,
    asset_file_names: &BTreeMap<String, String>,
) -> Result<MaterializationPlan, String> {
    onda_frontend::ensure_no_symlink_components(source).map_err(|error| {
        format!(
            "failed to resolve project source '{}': {error}",
            source.display()
        )
    })?;
    let entry = fs::canonicalize(source).map_err(|error| {
        format!(
            "failed to resolve project source '{}': {error}",
            source.display()
        )
    })?;
    if !entry.is_file() {
        return Err(format!(
            "project source '{}' is not a file",
            entry.display()
        ));
    }
    let loaded = onda_frontend::load_program_file(&entry).map_err(|error| {
        format_diagnostics("failed to load the source project", &error.diagnostics)
    })?;
    let inputs = onda_semantics::compile_inputs_from_literals(
        &loaded.program,
        constants
            .iter()
            .map(|(name, value)| (name.clone(), value.onda_literal())),
        onda_semantics::AnalysisOptions::default(),
    )
    .map_err(|diagnostics| format_diagnostics("cannot resolve project constants", &diagnostics))?;
    let typed = onda_semantics::analyze_with_options_and_inputs(
        loaded.program.clone(),
        onda_semantics::AnalysisOptions::default(),
        &inputs,
    )
    .map_err(|diagnostics| {
        format_diagnostics(
            "cannot package a project which does not compile",
            &diagnostics,
        )
    })?;
    validate_assets(&typed, &assets)?;

    let source_root = source_root
        .map(fs::canonicalize)
        .transpose()
        .map_err(|error| format!("failed to resolve source project root: {error}"))?
        .unwrap_or_else(|| {
            entry
                .parent()
                .map(Path::to_owned)
                .unwrap_or_else(|| PathBuf::from("."))
        });
    let sources = SourceImage::capture(
        &entry,
        &source_root,
        &loaded.sources,
        ProjectLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    ProjectImage::from_buffer_assets_with_constants(sources, constants, assets)
        .and_then(|image| {
            image.materialization_plan_with_file_names(project_file_name, asset_file_names)
        })
        .map_err(|error| error.to_string())
}

pub(crate) fn project_file_name_from_target(target: &Path) -> Result<String, String> {
    let input_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("project target '{}' has no UTF-8 name", target.display()))?;
    if is_project_file_path(Path::new(input_name)) {
        Ok(input_name.to_owned())
    } else {
        Ok(format!("{input_name}.{ONDA_PROJECT_FILE_EXTENSION}"))
    }
}

pub fn publish_project_plan(destination: &Path, plan: &MaterializationPlan) -> Result<(), String> {
    plan.validate(&ProjectLimits::default())
        .map_err(|error| error.to_string())?;
    let destination = absolute_lexical_path(destination)?;
    if destination.file_name().is_none() {
        return Err(format!(
            "project destination '{}' is invalid",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "project destination has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create project parent directory '{}': {error}",
            parent.display()
        )
    })?;

    let destination_existed = match fs::symlink_metadata(&destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || fs::read_dir(&destination)
                    .map_err(|error| {
                        format!(
                            "failed to inspect project destination '{}': {error}",
                            destination.display()
                        )
                    })?
                    .next()
                    .is_some()
            {
                return Err(format!(
                    "project destination '{}' must be new or an empty directory",
                    destination.display()
                ));
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "failed to inspect project destination '{}': {error}",
                destination.display()
            ))
        }
    };

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock cannot create project staging name: {error}"))?
        .as_nanos();
    let destination_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "project destination name must be valid UTF-8".to_owned())?;
    let staging = parent.join(format!(
        ".{destination_name}.onda-project-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&staging).map_err(|error| {
        format!(
            "failed to create project staging directory '{}': {error}",
            staging.display()
        )
    })?;
    let mut cleanup = StagingCleanup::new(staging.clone());

    for directory in &plan.directories {
        fs::create_dir_all(staging.join(directory)).map_err(|error| {
            format!("failed to create project directory '{directory}': {error}")
        })?;
    }
    for file in &plan.files {
        let path = staging.join(&file.relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create parent for project file '{}': {error}",
                    file.relative_path
                )
            })?;
        }
        fs::write(&path, &file.bytes).map_err(|error| {
            format!(
                "failed to write project file '{}': {error}",
                file.relative_path
            )
        })?;
    }

    if destination_existed {
        fs::remove_dir(&destination).map_err(|error| {
            format!(
                "failed to replace empty project destination '{}': {error}",
                destination.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(&staging, &destination) {
        if destination_existed {
            let _ = fs::create_dir(&destination);
        }
        return Err(format!(
            "failed to publish project directory '{}': {error}",
            destination.display()
        ));
    }
    cleanup.commit();
    Ok(())
}

fn validate_assets(
    typed: &onda_semantics::TypedProgram,
    assets: &BTreeMap<String, BufferAsset>,
) -> Result<(), String> {
    let declarations = project_buffer_declarations(typed)?;
    validate_buffer_assets(
        assets.iter().map(|(name, asset)| (name.as_str(), asset)),
        &declarations,
    )
    .map_err(|error| error.to_string())
}

pub fn project_buffer_declarations(
    typed: &onda_semantics::TypedProgram,
) -> Result<Vec<ProjectBufferDeclaration>, String> {
    typed
        .buffers
        .iter()
        .map(|buffer| {
            let element = match buffer.elem_ty {
                onda_frontend::PrimitiveType::Bool => BufferElement::Bool,
                onda_frontend::PrimitiveType::I32 => BufferElement::I32,
                onda_frontend::PrimitiveType::I64 => BufferElement::I64,
                onda_frontend::PrimitiveType::F32 => BufferElement::F32,
                onda_frontend::PrimitiveType::F64 => BufferElement::F64,
            };
            let channels = match &buffer.channels {
                onda_semantics::TypedBufferChannels::Mono => ProjectBufferChannels::Mono,
                onda_semantics::TypedBufferChannels::Static(channels) => {
                    ProjectBufferChannels::Static(u32::try_from(*channels).map_err(|_| {
                        format!(
                            "buffer '{}' channel count does not fit the project format",
                            buffer.name
                        )
                    })?)
                }
                onda_semantics::TypedBufferChannels::Dynamic => ProjectBufferChannels::Dynamic,
            };
            Ok(ProjectBufferDeclaration {
                name: buffer.name.clone(),
                element,
                channels,
                array_len: buffer.array_len,
                is_array: buffer.is_array,
            })
        })
        .collect()
}

fn format_diagnostics(prefix: &str, diagnostics: &[onda_frontend::Diagnostic]) -> String {
    let details = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    if details.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}: {details}")
    }
}

pub(super) fn absolute_lexical_path(path: &Path) -> Result<PathBuf, String> {
    onda_frontend::absolute_lexical_path(path)
        .map_err(|error| format!("failed to determine current directory: {error}"))
}

struct StagingCleanup {
    path: PathBuf,
    committed: bool,
}

impl StagingCleanup {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for StagingCleanup {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_filename_uses_the_user_target_name() {
        assert_eq!(
            project_file_name_from_target(Path::new("test")).expect("plain target name"),
            "test.ondaproject"
        );
        assert_eq!(
            project_file_name_from_target(Path::new("exports/My Project"))
                .expect("nested target name"),
            "My Project.ondaproject"
        );
        assert_eq!(
            project_file_name_from_target(Path::new("named.ondaproject"))
                .expect("target already has project extension"),
            "named.ondaproject"
        );
    }

    #[test]
    fn publication_rejects_escaping_plan_paths_before_creating_staging_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "onda-invalid-publication-{}-{stamp}",
            std::process::id()
        ));
        let plan = MaterializationPlan {
            directories: Vec::new(),
            files: vec![onda_project::PlannedFile {
                relative_path: "../escape".to_owned(),
                bytes: b"must not be written".to_vec(),
            }],
        };

        publish_project_plan(&root.join("project"), &plan)
            .expect_err("escaping publication paths must be rejected");
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn project_packaging_rejects_symlink_source_inputs() {
        use std::os::unix::fs::symlink;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "onda-symlink-package-source-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test directory");
        let source = root.join("main.onda");
        let alias = root.join("linked.onda");
        fs::write(&source, "outs 1\nsample:\n  out1 = 0.0\n").expect("write source");
        symlink(&source, &alias).expect("create source symlink");

        let error = package_project_plan(
            &alias,
            None,
            BTreeMap::new(),
            BTreeMap::new(),
            "project.ondaproject",
            &BTreeMap::new(),
        )
        .expect_err("project packaging must reject a symlink source");
        assert!(error.contains("symlink component"));

        fs::remove_dir_all(root).ok();
    }
}
