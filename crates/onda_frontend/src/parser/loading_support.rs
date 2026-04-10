use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::diagnostics::Diagnostic;

use super::preprocess::split_comment;
use super::STDLIB_MODULE_PREFIX;

const ONDA_SOURCE_EXTENSIONS: &[&str] = &["onda", "on"];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum FileLoadMode {
    Entry,
    Include,
    Import,
}

#[derive(Debug, Clone)]
pub(super) enum TopLevelItem {
    Code {
        text: String,
        start_line: usize,
        source_line_map: Vec<usize>,
    },
    Include {
        path: String,
        line: usize,
    },
    Import {
        module: String,
        line: usize,
    },
}

pub(super) fn builtin_std_module_source(module: &str) -> Option<&'static str> {
    match module {
        "std/math" => Some(include_str!("../../../../stdlib/std/math.onda")),
        "std/export_math" => Some(include_str!("../../../../stdlib/std/export_math.onda")),
        "std/complex" => Some(include_str!("../../../../stdlib/std/complex.onda")),
        "std/osc" => Some(include_str!("../../../../stdlib/std/osc.onda")),
        "std/filter" => Some(include_str!("../../../../stdlib/std/filter.onda")),
        "std/env" => Some(include_str!("../../../../stdlib/std/env.onda")),
        "std/delay" => Some(include_str!("../../../../stdlib/std/delay.onda")),
        "std/data" => Some(include_str!("../../../../stdlib/std/data.onda")),
        "std/fft" => Some(include_str!("../../../../stdlib/std/fft.onda")),
        "std/convolution" => Some(include_str!("../../../../stdlib/std/convolution.onda")),
        "std/lookup" => Some(include_str!("../../../../stdlib/std/lookup.onda")),
        "std/random" => Some(include_str!("../../../../stdlib/std/random.onda")),
        "std/prelude" => Some(include_str!("../../../../stdlib/std/prelude.onda")),
        _ => None,
    }
}

pub(super) fn is_builtin_std_module_path(module: &str) -> bool {
    module.starts_with(STDLIB_MODULE_PREFIX)
}

pub(super) fn annotate_diagnostics_with_file(
    mut diags: Vec<Diagnostic>,
    file_path: &Path,
    line_offset: usize,
) -> Vec<Diagnostic> {
    let file = display_path(file_path);
    for diag in &mut diags {
        if diag.file.is_none() {
            if diag.line > 0 {
                diag.line += line_offset;
            }
            if diag.end_line > 0 {
                diag.end_line += line_offset;
            }
            diag.file = Some(file.clone());
        }
    }
    diags
}

pub(super) fn split_top_level_items(
    preprocessed: &str,
    preprocessed_line_map: &[usize],
    file_path: &Path,
) -> Result<Vec<TopLevelItem>, Vec<Diagnostic>> {
    let mut items = Vec::<TopLevelItem>::new();
    let mut code = String::new();
    let mut code_line_map = Vec::<usize>::new();
    let mut code_start_line = 1usize;
    let mut brace_depth: i32 = 0;

    for (idx, raw_line) in preprocessed.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (code_part, _) = split_comment(line);
        let trimmed = code_part.trim();

        if brace_depth == 0 {
            if let Some(item) = parse_top_level_directive(trimmed, line_no)
                .map_err(|diags| annotate_diagnostics_with_file(diags, file_path, 0))?
            {
                if !code.trim().is_empty() {
                    items.push(TopLevelItem::Code {
                        text: code.clone(),
                        start_line: code_start_line,
                        source_line_map: code_line_map.clone(),
                    });
                    code.clear();
                    code_line_map.clear();
                }
                items.push(item);
                continue;
            }
        }

        if code.is_empty() {
            code_start_line = line_no;
        }
        code.push_str(line);
        code.push('\n');
        code_line_map.push(preprocessed_line_map.get(idx).copied().unwrap_or(line_no));

        for ch in code_part.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
        if brace_depth < 0 {
            return Err(annotate_diagnostics_with_file(
                vec![Diagnostic::syntax("unmatched '}'", line_no, 1)],
                file_path,
                0,
            ));
        }
    }

    if !code.trim().is_empty() {
        items.push(TopLevelItem::Code {
            text: code,
            start_line: code_start_line,
            source_line_map: code_line_map,
        });
    }
    Ok(items)
}

pub(super) fn resolve_include_path(
    current_file: &Path,
    include_path: &str,
) -> Result<PathBuf, String> {
    let include = PathBuf::from(include_path);
    let resolved = if include.is_absolute() {
        include
    } else {
        current_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(include)
    };
    fs::canonicalize(&resolved)
        .map_err(|err| format!("failed to resolve include '{}': {err}", resolved.display()))
}

pub(super) fn resolve_import_path(
    current_file: &Path,
    module_path: &str,
) -> Result<PathBuf, String> {
    let base = if Path::new(module_path).is_absolute() {
        PathBuf::from(module_path)
    } else {
        current_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(module_path)
    };

    for ext in ONDA_SOURCE_EXTENSIONS {
        let candidate = base.with_extension(ext);
        if let Ok(canonical) = fs::canonicalize(&candidate) {
            return Ok(canonical);
        }
    }

    Err(format!(
        "failed to resolve import '{}.{{{}}}'",
        base.display(),
        ONDA_SOURCE_EXTENSIONS.join(",")
    ))
}

pub(super) fn validate_file_mode_transition(
    target: &Path,
    mode: FileLoadMode,
    source_file: &Path,
    line: usize,
    file_modes: &mut HashMap<PathBuf, FileLoadMode>,
) -> Result<(), Vec<Diagnostic>> {
    if let Some(existing) = file_modes.get(target).copied() {
        match (existing, mode) {
            (FileLoadMode::Import, FileLoadMode::Include)
            | (FileLoadMode::Include, FileLoadMode::Import) => {
                return Err(annotate_diagnostics_with_file(
                    vec![Diagnostic::syntax(
                        format!(
                            "file '{}' cannot be both imported and included (referenced from '{}')",
                            display_path(target),
                            display_path(source_file)
                        ),
                        line,
                        1,
                    )],
                    source_file,
                    0,
                ));
            }
            _ => {}
        }
    } else {
        file_modes.insert(target.to_owned(), mode);
    }
    Ok(())
}

pub(super) fn display_path(path: &Path) -> String {
    let raw = path.display().to_string();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_owned()
}

fn parse_top_level_directive(
    trimmed: &str,
    line_no: usize,
) -> Result<Option<TopLevelItem>, Vec<Diagnostic>> {
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut parts = trimmed.split_whitespace();
    let Some(keyword) = parts.next() else {
        return Ok(None);
    };

    if keyword == "import" {
        let Some(module) = parts.next() else {
            return Err(vec![Diagnostic::syntax(
                "import requires module path",
                line_no,
                1,
            )]);
        };
        if parts.next().is_some() {
            return Err(vec![Diagnostic::syntax(
                "import expects a single module path",
                line_no,
                1,
            )]);
        }
        if module.contains('\\') {
            return Err(vec![Diagnostic::syntax(
                "import path must use '/' separators",
                line_no,
                1,
            )]);
        }
        if ONDA_SOURCE_EXTENSIONS
            .iter()
            .any(|ext| module.ends_with(&format!(".{ext}")))
        {
            return Err(vec![Diagnostic::syntax(
                "import expects module path without '.onda' or '.on' suffix",
                line_no,
                1,
            )]);
        }
        return Ok(Some(TopLevelItem::Import {
            module: module.to_owned(),
            line: line_no,
        }));
    }

    if keyword == "include" {
        let rest = trimmed["include".len()..].trim();
        if !rest.starts_with('"') || !rest.ends_with('"') || rest.len() < 2 {
            return Err(vec![Diagnostic::syntax(
                "include expects quoted file path, for example include \"lib.onda\"",
                line_no,
                1,
            )]);
        }
        let include_path = &rest[1..rest.len() - 1];
        if include_path.is_empty() {
            return Err(vec![Diagnostic::syntax(
                "include path cannot be empty",
                line_no,
                1,
            )]);
        }
        if include_path.contains('\\') {
            return Err(vec![Diagnostic::syntax(
                "include path must use '/' separators",
                line_no,
                1,
            )]);
        }
        if !ONDA_SOURCE_EXTENSIONS
            .iter()
            .any(|ext| include_path.ends_with(&format!(".{ext}")))
        {
            return Err(vec![Diagnostic::syntax(
                "include path must end with '.onda' or '.on'",
                line_no,
                1,
            )]);
        }
        return Ok(Some(TopLevelItem::Include {
            path: include_path.to_owned(),
            line: line_no,
        }));
    }

    Ok(None)
}
