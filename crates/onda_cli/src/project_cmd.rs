use std::path::{Path, PathBuf};

use onda_frontend::Program;
use onda_project::{
    resolve_project_input, validate_buffer_asset_metadata, BufferAsset, ProjectInput, ProjectLimits,
};
use onda_semantics::TypedProgram;
use onda_semantics::{compile_inputs_from_literals, AnalysisOptions, CompileInputs};

pub(crate) fn resolve_entry(input: &Path) -> Result<ProjectInput, String> {
    resolve_project_input(input, ProjectLimits::default()).map_err(|error| error.to_string())
}

pub(crate) struct ResolvedRunProject {
    pub entry: PathBuf,
    pub compile_inputs: CompileInputs,
    pub buffers: Vec<(String, BufferAsset, Option<PathBuf>)>,
}

pub(crate) fn resolve_run_project(
    input: &Path,
    overrides: &[(String, PathBuf)],
    analysis_options: AnalysisOptions,
) -> Result<ResolvedRunProject, String> {
    let project = resolve_entry(input)?;
    let entry = project.entry_path().to_path_buf();
    let compile_inputs = match project.project() {
        Some(project_file) if !project_file.manifest.constants.is_empty() => {
            let parsed = onda_frontend::parse_program_file(&entry)
                .map_err(|diags| crate::diag_print::format_diagnostics("parse failed", &diags))?;
            project_compile_inputs(&project, &parsed, analysis_options)?
        }
        Some(_) | None => CompileInputs::default(),
    };
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
    Ok(ResolvedRunProject {
        entry,
        compile_inputs,
        buffers,
    })
}

pub(crate) fn project_compile_inputs(
    project: &ProjectInput,
    parsed: &Program,
    analysis_options: AnalysisOptions,
) -> Result<CompileInputs, String> {
    let Some(project) = project.project() else {
        return Ok(CompileInputs::default());
    };
    compile_inputs_from_literals(
        parsed,
        project
            .manifest
            .constants
            .iter()
            .map(|(name, value)| (name.clone(), value.onda_literal())),
        analysis_options,
    )
    .map_err(|diags| crate::diag_print::format_diagnostics("invalid project constant", &diags))
}

pub(crate) fn validate_compile_project(
    project: &ProjectInput,
    typed: &TypedProgram,
) -> Result<(), String> {
    let Some(project_file) = project.project() else {
        return Ok(());
    };
    let assets = project_file
        .inspect_buffer_assets(ProjectLimits::default())
        .map_err(|error| error.to_string())?;
    let declarations = onda_run::project_buffer_declarations(typed)?;
    validate_buffer_asset_metadata(
        assets.iter().map(|(name, asset)| (name.as_str(), asset)),
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
