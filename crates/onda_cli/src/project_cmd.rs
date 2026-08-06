use std::path::{Path, PathBuf};

use onda_project::{
    resolve_project_input, validate_buffer_assets, BufferAsset, ProjectInput, ProjectLimits,
};
use onda_semantics::TypedProgram;

pub(crate) fn resolve_entry(input: &Path) -> Result<ProjectInput, String> {
    resolve_project_input(input, ProjectLimits::default()).map_err(|error| error.to_string())
}

pub(crate) struct ResolvedRunProject {
    pub entry: PathBuf,
    pub buffers: Vec<(String, BufferAsset, Option<PathBuf>)>,
}

pub(crate) fn resolve_run_project(
    input: &Path,
    overrides: &[(String, PathBuf)],
) -> Result<ResolvedRunProject, String> {
    let project = resolve_entry(input)?;
    let entry = project.entry_path().to_path_buf();
    let buffers = match project.project() {
        Some(project_file) => {
            let overridden = overrides
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            project_file
                .load_buffer_assets_excluding(ProjectLimits::default(), &overridden)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|(name, (asset, path))| (name, asset, path))
                .collect()
        }
        None => Vec::new(),
    };
    Ok(ResolvedRunProject { entry, buffers })
}

pub(crate) fn validate_compile_project(
    project: &ProjectInput,
    typed: &TypedProgram,
) -> Result<(), String> {
    let Some(project_file) = project.project() else {
        return Ok(());
    };
    let assets = project_file
        .load_buffer_assets(ProjectLimits::default())
        .map_err(|error| error.to_string())?;
    let declarations = onda_run::project_buffer_declarations(typed)?;
    validate_buffer_assets(
        assets
            .iter()
            .map(|(name, (asset, _))| (name.as_str(), asset)),
        &declarations,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn run_project(
    destination: &Path,
    source: Option<&Path>,
    buffer_bindings: &[(String, PathBuf)],
) -> Result<(), String> {
    match source {
        Some(source) => {
            onda_run::package_project_from_files(destination, source, buffer_bindings)?;
        }
        None => onda_run::create_empty_project(destination)?,
    }
    println!("Created Onda project: {}", destination.display());
    Ok(())
}
