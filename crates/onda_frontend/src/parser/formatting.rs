//! Format the authored document. Grammar spans distinguish operators from type
//! brackets and unary signs, while comments and spelling come from the source.
use super::preprocess::{leading_indent_width, split_comment};
use crate::Diagnostic;
use std::path::Path;
use syntax::{Line, Nesting};

mod layout;
mod syntax;

/// Format one source file without loading imports or expanding its syntax.
pub fn format_source(source: &str, path: &Path) -> Result<String, Vec<Diagnostic>> {
    let (lines, original) = syntax::source_layout(source, path)?;
    let newline = if source
        .split_once('\n')
        .is_some_and(|(first, _)| first.ends_with('\r'))
    {
        "\r\n"
    } else {
        "\n"
    };
    // Line breaks can affect the grammar even inside a continuation. Try
    // spacing alone if reflow would change the compiler's parse tree.
    for reflow in [true, false] {
        let output = render_document(&lines, newline, reflow);
        if output == source
            || syntax::parse_syntax(&output, path, None).is_ok_and(|syntax| syntax == original)
        {
            return Ok(output);
        }
    }
    Ok(source.to_owned())
}

fn render_document(lines: &[Line<'_>], newline: &str, reflow: bool) -> String {
    let mut output = Vec::new();
    let mut nesting = Nesting::default();
    // Comment-only lines follow the adjacent code at their authored indentation.
    let mut pending = Vec::new();
    let mut previous = (0, 0);
    let mut rows = lines.iter().peekable();
    while let Some(line) = rows.next() {
        let raw = line.text;
        let (code, mut comment) = split_comment(raw);
        let indent = leading_indent_width(raw);
        if code.trim().is_empty() {
            pending.push((indent, comment));
            continue;
        }
        let mut tokens = syntax::tokenize(code, line, &mut nesting);
        // Reflow an existing continuation exactly like a single-line list.
        // A comment or blank line anchors the author's break.
        let mut authored = Vec::new();
        let mut raw = raw;
        while reflow && comment.is_none() && nesting.in_list() {
            let Some(next) = rows.peek() else {
                break;
            };
            let (code, tail) = split_comment(next.text);
            if code.trim().is_empty() {
                break;
            }
            authored.push((tokens.len(), raw));
            tokens.extend(syntax::tokenize(code, next, &mut nesting));
            raw = next.text;
            comment = tail;
            rows.next();
        }
        authored.push((tokens.len(), raw));
        let formatted_indent = layout::indentation(&tokens);
        emit_comments(
            &mut output,
            &mut pending,
            previous,
            (indent, formatted_indent),
        );
        previous = (indent, formatted_indent);
        if reflow {
            layout::render(&tokens, &authored, comment, &mut output);
        } else {
            output.push(layout::render_line(&tokens, comment));
        }
    }
    emit_comments(&mut output, &mut pending, previous, (0, 0));
    while output.last().is_some_and(String::is_empty) {
        output.pop();
    }
    if !output.is_empty() {
        output.push(String::new());
    }
    output.join(newline)
}

fn emit_comments(
    output: &mut Vec<String>,
    pending: &mut Vec<(usize, Option<&str>)>,
    previous: (usize, usize),
    next: (usize, usize),
) {
    for (indent, comment) in pending.drain(..) {
        if let Some(comment) = comment {
            let (old, new) = if indent.abs_diff(next.0) <= indent.abs_diff(previous.0) {
                next
            } else {
                previous
            };
            let spaces = (2 * new + indent).saturating_sub(old);
            output.push(format!("{}#{}", " ".repeat(spaces), comment.trim_end()));
        } else if output.last().is_some_and(|s| !s.is_empty()) {
            output.push(String::new());
        }
    }
}

#[cfg(test)]
mod tests;
