use crate::diagnostics::Diagnostic;

#[derive(Clone, Copy)]
struct PendingIndentBlock {
    line: usize,
    indent: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ContinuationDelimiter {
    Parenthesis,
    Bracket,
    Brace,
    Angle,
}

pub(super) fn preprocess_indentation_blocks(
    source: &str,
) -> Result<(String, Vec<usize>), Vec<Diagnostic>> {
    fn push_mapped_line(
        out: &mut String,
        line_map: &mut Vec<usize>,
        text: &str,
        source_line: usize,
    ) {
        out.push_str(text);
        out.push('\n');
        line_map.push(source_line);
    }

    let mut out = String::with_capacity(source.len() + source.len() / 4);
    let mut line_map = Vec::<usize>::new();
    let mut indent_stack = vec![0usize];
    let mut pending: Option<PendingIndentBlock> = None;
    let mut continuation_delimiters = Vec::new();
    let mut logical_line = 1usize;
    let mut logical_indent = 0usize;
    let mut last_source_line = 1usize;

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        last_source_line = line_no;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let (code_part, _comment_part) = split_comment(line);
        let is_comment_or_blank = code_part.trim().is_empty();

        if is_comment_or_blank {
            push_mapped_line(&mut out, &mut line_map, line, line_no);
            continue;
        }

        let indent_width = leading_indent_width(code_part);
        let continues_statement = !continuation_delimiters.is_empty();

        if let Some(pending_block) = pending.take() {
            if indent_width <= pending_block.indent {
                return Err(vec![Diagnostic::syntax(
                    "expected indented block after ':'",
                    pending_block.line,
                    1,
                )]);
            }
            indent_stack.push(indent_width);
        } else if !continues_statement && indent_stack.len() > 1 {
            while indent_stack.len() > 1 && indent_width < *indent_stack.last().unwrap_or(&0) {
                indent_stack.pop();
                push_mapped_line(&mut out, &mut line_map, "}", line_no);
            }
            if indent_stack.len() > 1 && indent_width != *indent_stack.last().unwrap_or(&0) {
                return Err(vec![Diagnostic::syntax(
                    "inconsistent indentation level",
                    line_no,
                    1,
                )]);
            }
        }
        if !continues_statement {
            logical_line = line_no;
            logical_indent = indent_width;
        }

        apply_continuation_delimiters(&mut continuation_delimiters, code_part);
        let trimmed_code = code_part.trim_end();
        if continuation_delimiters.is_empty() && trimmed_code.ends_with(':') {
            let header = trimmed_code[..trimmed_code.len() - 1].trim_end();
            if header.is_empty() {
                return Err(vec![Diagnostic::syntax(
                    "missing block header before ':'",
                    line_no,
                    1,
                )]);
            }
            let header_line = format!("{header} {{");
            push_mapped_line(&mut out, &mut line_map, &header_line, line_no);
            pending = Some(PendingIndentBlock {
                line: logical_line,
                indent: logical_indent,
            });
        } else {
            push_mapped_line(&mut out, &mut line_map, line, line_no);
        }
    }

    if let Some(pending_block) = pending {
        return Err(vec![Diagnostic::syntax(
            "expected indented block after ':'",
            pending_block.line,
            1,
        )]);
    }

    while indent_stack.len() > 1 {
        indent_stack.pop();
        push_mapped_line(&mut out, &mut line_map, "}", last_source_line);
    }

    Ok((out, line_map))
}

pub(super) fn split_comment(line: &str) -> (&str, Option<&str>) {
    if let Some((idx, _)) = unquoted_chars(line).find(|(_, ch)| *ch == '#') {
        (&line[..idx], Some(&line[idx + 1..]))
    } else {
        (line, None)
    }
}

/// Structural characters outside quoted text, preserving their source offsets.
/// Quoted text is line-local; malformed strings are diagnosed by the grammar.
pub(super) fn unquoted_chars(line: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut quoted = false;
    let mut escaped = false;
    line.char_indices().filter(move |&(_, ch)| {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            false
        } else if ch == '"' {
            quoted = true;
            false
        } else {
            true
        }
    })
}

fn leading_indent_width(line: &str) -> usize {
    let mut width = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => width += 1,
            '\t' => width += 2,
            _ => break,
        }
    }
    width
}

fn apply_continuation_delimiters(delimiters: &mut Vec<ContinuationDelimiter>, line: &str) {
    let line_trimmed = line.trim_end();
    for (idx, ch) in unquoted_chars(line) {
        match ch {
            '(' => delimiters.push(ContinuationDelimiter::Parenthesis),
            '[' => delimiters.push(ContinuationDelimiter::Bracket),
            '{' if is_continuation_opening_brace(line, idx) => {
                delimiters.push(ContinuationDelimiter::Brace);
            }
            '<' if delimiters
                .iter()
                .all(|delimiter| *delimiter == ContinuationDelimiter::Angle)
                && is_multiline_angle_open(line_trimmed, idx) =>
            {
                delimiters.push(ContinuationDelimiter::Angle);
            }
            ')' => close_continuation(delimiters, ContinuationDelimiter::Parenthesis),
            ']' => close_continuation(delimiters, ContinuationDelimiter::Bracket),
            '}' => close_continuation(delimiters, ContinuationDelimiter::Brace),
            '>' if is_multiline_angle_close(line_trimmed, idx) => {
                close_continuation(delimiters, ContinuationDelimiter::Angle);
            }
            _ => {}
        }
    }
}

fn close_continuation(
    delimiters: &mut Vec<ContinuationDelimiter>,
    expected: ContinuationDelimiter,
) {
    if delimiters.last() == Some(&expected) {
        delimiters.pop();
    }
}

fn is_continuation_opening_brace(line: &str, brace_idx: usize) -> bool {
    let before = line[..brace_idx].trim_end();
    if before.trim().is_empty() {
        return true;
    }
    if before.contains(">>") || before.contains("<<") {
        return true;
    }
    if contains_assignment_operator(before) {
        return true;
    }
    before
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, ',' | '(' | '[' | '{'))
}

fn contains_assignment_operator(text: &str) -> bool {
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch != '=' {
            continue;
        }
        let prev = text[..idx].chars().next_back();
        let next = chars.peek().map(|(_, next)| *next);
        if !matches!(prev, Some('!' | '<' | '>' | '=')) && !matches!(next, Some('=' | '>')) {
            return true;
        }
    }
    false
}

fn is_multiline_angle_open(line_trimmed: &str, angle_idx: usize) -> bool {
    line_trimmed[angle_idx + '<'.len_utf8()..].trim().is_empty()
}

fn is_multiline_angle_close(line_trimmed: &str, angle_idx: usize) -> bool {
    line_trimmed[..angle_idx].trim().is_empty()
        && !line_trimmed[angle_idx + '>'.len_utf8()..].starts_with('>')
}
