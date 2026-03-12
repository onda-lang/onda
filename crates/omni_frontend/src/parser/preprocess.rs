use crate::diagnostics::Diagnostic;

#[derive(Clone, Copy)]
struct PendingIndentBlock {
    line: usize,
    indent: usize,
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
    let mut continuation_depth = 0usize;
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

        if let Some(pending_block) = pending.take() {
            if indent_width <= pending_block.indent {
                return Err(vec![Diagnostic::syntax(
                    "expected indented block after ':'",
                    pending_block.line,
                    1,
                )]);
            }
            indent_stack.push(indent_width);
        } else if continuation_depth == 0 && indent_stack.len() > 1 {
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

        let trimmed_code = code_part.trim_end();
        if trimmed_code.ends_with(':') {
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
                line: line_no,
                indent: indent_width,
            });
        } else {
            push_mapped_line(&mut out, &mut line_map, line, line_no);
        }

        continuation_depth = apply_continuation_delta(continuation_depth, code_part);
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
    if let Some(idx) = line.find('#') {
        (&line[..idx], Some(&line[idx + 1..]))
    } else {
        (line, None)
    }
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

fn apply_continuation_delta(mut depth: usize, line: &str) -> usize {
    for ch in line.chars() {
        match ch {
            '(' | '[' => depth = depth.saturating_add(1),
            ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}
