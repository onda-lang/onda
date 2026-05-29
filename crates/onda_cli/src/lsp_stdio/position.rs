use onda_frontend::Span;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct LspPosition {
    pub(super) line: u32,
    pub(super) character: u32,
}

impl LspPosition {
    pub(super) const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

pub(super) fn line_at_position(source: &str, line: u32) -> Option<(usize, usize, &str)> {
    let mut start = 0usize;
    for (line_no, segment) in source.split_inclusive('\n').enumerate() {
        let end = start + segment.len();
        if line_no as u32 == line {
            let line_end = if segment.ends_with('\n') {
                end - 1
            } else {
                end
            };
            return Some((start, line_end, &source[start..line_end]));
        }
        start = end;
    }
    if line as usize == source.lines().count() {
        Some((source.len(), source.len(), ""))
    } else {
        None
    }
}

pub(super) fn byte_offset_for_lsp_position(source: &str, position: LspPosition) -> usize {
    let Some((line_start, _, line_text)) = line_at_position(source, position.line) else {
        return source.len();
    };
    line_start + byte_index_for_lsp_character(line_text, position.character)
}

pub(super) fn byte_index_for_lsp_character(line: &str, character: u32) -> usize {
    let mut utf16_units = 0u32;
    for (idx, ch) in line.char_indices() {
        if utf16_units >= character {
            return idx;
        }
        let next = utf16_units.saturating_add(ch.len_utf16() as u32);
        if character < next {
            return idx;
        }
        utf16_units = next;
    }
    line.len()
}

pub(super) fn lsp_character_for_byte(line: &str, byte: usize) -> u32 {
    let byte = byte.min(line.len());
    let safe_byte = if line.is_char_boundary(byte) {
        byte
    } else {
        line[..byte]
            .char_indices()
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    };
    line[..safe_byte]
        .chars()
        .map(|ch| ch.len_utf16() as u32)
        .sum()
}

pub(super) fn span_start_position(source: &str, span: Span) -> LspPosition {
    span_position_for_column(source, span.line, span.column)
}

pub(super) fn span_end_position(source: &str, span: Span) -> LspPosition {
    span_position_for_column(source, span.end_line(), span.end_column)
}

pub(super) fn fallback_span_start_position(span: Span) -> LspPosition {
    fallback_span_position(span.line, span.column)
}

pub(super) fn fallback_span_end_position(span: Span) -> LspPosition {
    fallback_span_position(span.end_line(), span.end_column)
}

fn span_position_for_column(
    source: &str,
    one_based_line: u32,
    one_based_column: u16,
) -> LspPosition {
    let line = one_based_line.saturating_sub(1);
    let character = line_at_position(source, line)
        .map(|(_, _, line_text)| lsp_character_for_one_based_column(line_text, one_based_column))
        .unwrap_or_else(|| u32::from(one_based_column.saturating_sub(1)));
    LspPosition { line, character }
}

fn fallback_span_position(one_based_line: u32, one_based_column: u16) -> LspPosition {
    LspPosition {
        line: one_based_line.saturating_sub(1),
        character: u32::from(one_based_column.saturating_sub(1)),
    }
}

fn lsp_character_for_one_based_column(line: &str, one_based_column: u16) -> u32 {
    let scalar_columns = one_based_column.saturating_sub(1) as usize;
    line.chars()
        .take(scalar_columns)
        .map(|ch| ch.len_utf16() as u32)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offsets_use_utf16_lsp_characters() {
        let line = "😀gain";

        assert_eq!(byte_index_for_lsp_character(line, 0), 0);
        assert_eq!(byte_index_for_lsp_character(line, 2), "😀".len());
        assert_eq!(byte_index_for_lsp_character(line, 3), "😀g".len());
        assert_eq!(lsp_character_for_byte(line, "😀g".len()), 3);
    }
}
