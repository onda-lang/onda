use std::fs;
use std::path::Path;

use onda_daemon::RunBuildError;
use onda_frontend::Diagnostic;

pub(crate) fn format_run_build_error(context: &str, err: &RunBuildError) -> String {
    match err {
        RunBuildError::Diagnostics(diags) => format_diagnostics(context, diags),
        RunBuildError::Runtime(diag) => format_single_diagnostic(context, diag),
    }
}

pub(crate) fn format_diagnostics(context: &str, diags: &[Diagnostic]) -> String {
    let mut text = String::from(context);
    for diag in diags {
        text.push_str(&format!("\n- {}", format_single_diag_line(diag)));
        if !diag.trace.is_empty() {
            text.push_str("\n  trace:");
            for trace in diag.trace.iter().rev() {
                text.push_str(&format!("\n    - {trace}"));
            }
        }
        if let Some(snippet) = format_diag_snippet(diag) {
            text.push_str(&format!("\n{snippet}"));
        }
    }
    text
}

pub(crate) fn format_single_diagnostic(context: &str, diag: &Diagnostic) -> String {
    let mut out = format!("{context}\n- {}", format_single_diag_line(diag));
    if !diag.trace.is_empty() {
        out.push_str("\n  trace:");
        for trace in diag.trace.iter().rev() {
            out.push_str(&format!("\n    - {trace}"));
        }
    }
    if let Some(snippet) = format_diag_snippet(diag) {
        out.push_str(&format!("\n{snippet}"));
    }
    out
}

fn format_single_diag_line(diag: &Diagnostic) -> String {
    let location = match diag.file.as_deref() {
        Some(file) if diag.line > 0 => format!("{file}:{}:{}", diag.line, diag.column.max(1)),
        Some(file) => format!("{file}:0:0"),
        None if diag.line > 0 => format!("{}:{}", diag.line, diag.column.max(1)),
        None => "0:0".to_owned(),
    };
    format!("{location} [{:?}] {}", diag.code, diag.message)
}

pub(crate) fn format_diag_snippet(diag: &Diagnostic) -> Option<String> {
    if diag.message.contains('\n') {
        return None;
    }
    let file = diag.file.as_deref()?;
    if file.starts_with('<') || diag.line == 0 {
        return None;
    }
    let path = Path::new(file);
    let source = fs::read_to_string(path).ok()?;
    let line_idx = diag.line.checked_sub(1)?;
    let line_text = source.lines().nth(line_idx)?;
    let start_col = diag.column.max(1);
    let underline_len = if diag.end_line == diag.line && diag.end_column > start_col {
        diag.end_column.saturating_sub(start_col)
    } else {
        1
    };
    let caret_pad = " ".repeat(start_col.saturating_sub(1));
    let underline = "^".repeat(underline_len.max(1));
    Some(format!(
        "  --> {file}:{}:{}\n   |\n{:>4} | {}\n   | {}{}",
        diag.line, start_col, diag.line, line_text, caret_pad, underline
    ))
}
