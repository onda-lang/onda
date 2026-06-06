use super::*;
use onda_frontend::DiagCode;

pub(super) fn diagnostic_uri(diagnostic: &Diagnostic, default_uri: &str) -> Result<String, String> {
    let Some(file) = diagnostic.file.as_deref() else {
        return Ok(default_uri.to_owned());
    };
    if file.starts_with('<') {
        return Ok(default_uri.to_owned());
    }
    Ok(path_to_file_uri(&normalize_path(Path::new(file))))
}

pub(super) fn diagnostic_to_lsp(diagnostic: &Diagnostic) -> Value {
    json!({
        "range": diagnostic_range(diagnostic),
        "severity": 1,
        "source": "onda",
        "code": diagnostic_code(diagnostic),
        "message": diagnostic_message(diagnostic),
    })
}

fn diagnostic_code(diagnostic: &Diagnostic) -> &'static str {
    match diagnostic.code {
        DiagCode::Syntax => "syntax",
        DiagCode::Semantic => "semantic",
        DiagCode::Runtime => "runtime",
        DiagCode::Internal => "internal",
    }
}

fn diagnostic_range(diagnostic: &Diagnostic) -> Value {
    let start_line = diagnostic.line.saturating_sub(1);
    let start_character = diagnostic.column.saturating_sub(1);
    let mut end_line = diagnostic.end_line.saturating_sub(1);
    let mut end_character = diagnostic.end_column.saturating_sub(1);

    if diagnostic.end_line == 0 {
        end_line = start_line;
    }
    if end_line == start_line && end_character <= start_character {
        end_character = start_character.saturating_add(1);
    }

    json!({
        "start": {
            "line": start_line,
            "character": start_character,
        },
        "end": {
            "line": end_line,
            "character": end_character,
        }
    })
}

pub(super) fn diagnostic_message(diagnostic: &Diagnostic) -> String {
    if diagnostic.trace.is_empty() {
        return diagnostic.message.clone();
    }
    format!(
        "{}\ntrace:\n{}",
        diagnostic.message,
        diagnostic
            .trace
            .iter()
            .rev()
            .map(|trace| format!("- {trace}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
