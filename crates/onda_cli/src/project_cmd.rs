use std::path::{Path, PathBuf};

use onda_frontend::Program;
use onda_project::{
    resolve_project_input, resolve_project_input_with_file_digests, validate_buffer_asset_metadata,
    BufferAsset, FileReadDigests, ProjectInput, ProjectLimits,
};
use onda_semantics::TypedProgram;
use onda_semantics::{compile_inputs_from_literals, AnalysisOptions, CompileInputs};

pub(crate) fn resolve_entry(input: &Path) -> Result<ProjectInput, String> {
    resolve_project_input(input, ProjectLimits::default()).map_err(|error| error.to_string())
}

#[derive(Debug)]
pub(crate) struct ResolvedRunProject {
    pub entry: PathBuf,
    pub compile_inputs: CompileInputs,
    pub buffers: Vec<(String, BufferAsset, Option<PathBuf>)>,
    pub file_digests: Option<FileReadDigests>,
}

#[derive(Debug)]
pub(crate) struct RunProjectError {
    pub message: String,
    pub sources: Option<onda_run::RunSourceGraph>,
}

pub(crate) fn resolve_run_project(
    input: &Path,
    overrides: &[(String, PathBuf)],
    analysis_options: AnalysisOptions,
    report_sources: bool,
) -> Result<ResolvedRunProject, RunProjectError> {
    let mut file_digests = report_sources.then(FileReadDigests::new);
    let mut sources = onda_frontend::SourceManifest::default();
    let result = (|| {
        let project = resolve_project_input_with_file_digests(
            input,
            ProjectLimits::default(),
            file_digests.as_mut(),
        )
        .map_err(|error| error.to_string())?;
        let entry = project.entry_path().to_path_buf();
        let compile_inputs = match project.project() {
            Some(project_file) if !project_file.manifest.constants.is_empty() => {
                let loaded = onda_frontend::load_program_file(&entry).map_err(|error| {
                    if report_sources {
                        sources = *error.sources;
                    }
                    crate::diag_print::format_diagnostics("parse failed", &error.diagnostics)
                })?;
                if report_sources {
                    sources = loaded.sources;
                }
                project_compile_inputs(&project, &loaded.program, analysis_options)?
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
                    .load_buffer_assets_excluding(
                        ProjectLimits::default(),
                        &overridden,
                        file_digests.as_mut(),
                    )
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(|(name, (asset, path))| (name, asset, path))
                    .collect()
            }
            None => Vec::new(),
        };
        Ok((entry, compile_inputs, buffers))
    })();
    match result {
        Ok((entry, compile_inputs, buffers)) => Ok(ResolvedRunProject {
            entry,
            compile_inputs,
            buffers,
            file_digests,
        }),
        Err(message) => Err(RunProjectError {
            message,
            sources: file_digests
                .map(|digests| onda_run::RunSourceGraph::from_failed_sources(&sources, digests)),
        }),
    }
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
