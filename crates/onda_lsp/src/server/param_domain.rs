use std::collections::BTreeSet;

pub(super) use onda_frontend::PARAM_DOMAIN_FIELDS;
use onda_frontend::{ParamScale, PARAM_DOMAIN_POSITIONAL_FIELDS};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum ParamDomainValueKind {
    Expression,
    Scale,
    Unit,
    None,
}

#[derive(Debug, Clone)]
pub(super) struct ParamDomainCompletionContext {
    pub(super) used_fields: BTreeSet<&'static str>,
    pub(super) allow_fields: bool,
    pub(super) value_kind: ParamDomainValueKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum ParamDomainIdentifierRole {
    Field,
    ScaleValue,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum BindingRangeIdentifierRole {
    Field,
    ModeValue,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum BufferCountCompletionKind {
    Field,
    Expression,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum BindingRangeValueKind {
    Expression,
    Mode,
    None,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct BindingRangeCompletionContext {
    pub(super) used_fields: BTreeSet<&'static str>,
    pub(super) has_domain: bool,
    pub(super) allow_fields: bool,
    pub(super) allow_bare_mode: bool,
    pub(super) value_kind: BindingRangeValueKind,
}

pub(super) const BINDING_RANGE_FIELDS: &[&str] = &["count", "range", "mode"];

pub(super) fn binding_range_completion_context_at(
    source: &str,
    offset: usize,
) -> Option<BindingRangeCompletionContext> {
    let offset = offset.min(source.len());
    let stack = open_brace_stack(source, offset);
    let open = *stack.last()?;
    if !is_binding_range_open(source, open) {
        return None;
    }

    let items = split_top_level_items(source, open + 1, offset);
    let (current, previous) = items.split_last()?;
    let mut used_fields = BTreeSet::new();
    let mut positional_count = 0;
    let mut saw_named_or_mode = false;
    for item in previous {
        if let Some(field) = binding_range_named_field(source, item.clone()) {
            saw_named_or_mode = true;
            used_fields.insert(field);
        } else if is_binding_range_mode(source[item.clone()].trim()) {
            saw_named_or_mode = true;
            used_fields.insert("mode");
        } else if !saw_named_or_mode {
            positional_count += 1;
        }
    }
    let has_domain =
        positional_count > 0 || used_fields.contains("count") || used_fields.contains("range");
    if let Some(field) = binding_range_named_field(source, current.clone()) {
        return Some(BindingRangeCompletionContext {
            used_fields,
            has_domain: has_domain || matches!(field, "count" | "range"),
            allow_fields: false,
            allow_bare_mode: false,
            value_kind: if field == "mode" {
                BindingRangeValueKind::Mode
            } else {
                BindingRangeValueKind::Expression
            },
        });
    }
    Some(BindingRangeCompletionContext {
        used_fields,
        has_domain,
        allow_fields: true,
        allow_bare_mode: has_domain,
        value_kind: if !saw_named_or_mode && positional_count == 0 {
            BindingRangeValueKind::Expression
        } else {
            BindingRangeValueKind::None
        },
    })
}

fn binding_range_named_field(source: &str, item: std::ops::Range<usize>) -> Option<&'static str> {
    let text = &source[item];
    let equal = top_level_equal(text)?;
    let name = text[..equal].trim();
    BINDING_RANGE_FIELDS
        .iter()
        .copied()
        .find(|field| *field == name)
}

fn is_binding_range_mode(text: &str) -> bool {
    matches!(text, "clamp" | "wrap")
}

pub(super) fn buffer_count_completion_context_at(
    source: &str,
    offset: usize,
) -> Option<BufferCountCompletionKind> {
    let offset = offset.min(source.len());
    let stack = open_brace_stack(source, offset);
    let open = *stack.last()?;
    if !is_buffer_count_open(source, open, &stack[..stack.len() - 1]) {
        return None;
    }

    let current = source[open + 1..offset].trim_start();
    if top_level_equal(current).is_some()
        || (!current.is_empty() && leading_identifier(current).is_none())
    {
        Some(BufferCountCompletionKind::Expression)
    } else {
        Some(BufferCountCompletionKind::Field)
    }
}

pub(super) fn is_identifier_candidate(name: &str) -> bool {
    canonical_field(name).is_some() || ParamScale::from_name(name).is_some()
}

pub(super) fn is_binding_range_identifier_candidate(name: &str) -> bool {
    BINDING_RANGE_FIELDS.contains(&name) || is_binding_range_mode(name)
}

pub(super) fn completion_context_at(
    source: &str,
    offset: usize,
) -> Option<ParamDomainCompletionContext> {
    let offset = offset.min(source.len());
    let open = active_param_domain_open(source, offset)?;
    let items = split_top_level_items(source, open + 1, offset);
    let (current, previous) = items.split_last()?;

    let mut used_fields = BTreeSet::new();
    let mut saw_named = false;
    let mut positional_count = 0;
    for item in previous {
        if let Some(field) = named_field(source, item.clone()) {
            saw_named = true;
            used_fields.insert(field);
        } else if !saw_named {
            positional_count += 1;
        }
    }
    match positional_count {
        0 => {}
        1 if used_fields.contains("max") => {
            used_fields.insert("min");
        }
        1 if used_fields.contains("min") => {
            used_fields.insert("max");
        }
        1 => {
            // A lone positional bound is `max`, but adding a named `max`
            // reinterprets it as `min`. Until either bound is named, both are
            // valid completion choices.
        }
        count => {
            for field in PARAM_DOMAIN_POSITIONAL_FIELDS.iter().take(count) {
                used_fields.insert(*field);
            }
        }
    }

    if let Some(field) = named_field(source, current.clone()) {
        return Some(ParamDomainCompletionContext {
            used_fields,
            allow_fields: false,
            value_kind: value_kind_for_field(field),
        });
    }

    let current_text = source[current.clone()].trim_start();
    if current_text.starts_with('"') {
        return Some(ParamDomainCompletionContext {
            used_fields,
            allow_fields: false,
            value_kind: ParamDomainValueKind::Unit,
        });
    }

    let value_kind = if saw_named {
        ParamDomainValueKind::None
    } else {
        match previous.len() {
            0 | 1 | 4 => ParamDomainValueKind::Expression,
            2 => ParamDomainValueKind::Scale,
            3 => ParamDomainValueKind::Unit,
            _ => ParamDomainValueKind::None,
        }
    };
    Some(ParamDomainCompletionContext {
        used_fields,
        allow_fields: true,
        value_kind,
    })
}

pub(super) fn identifier_role_at(
    source: &str,
    start: usize,
    end: usize,
) -> Option<ParamDomainIdentifierRole> {
    let open = active_param_domain_open(source, start)?;
    let items = split_top_level_items(source, open + 1, end);
    let (current, previous) = items.split_last()?;
    let identifier = source.get(start..end)?;

    if canonical_field(identifier).is_some() && source[current.start..start].trim().is_empty() {
        let after_identifier = source[end..].trim_start();
        if after_identifier.starts_with('=') && !after_identifier.starts_with("==") {
            return Some(ParamDomainIdentifierRole::Field);
        }
    }
    ParamScale::from_name(identifier)?;

    if named_field(source, current.clone()) == Some("scale") {
        return Some(ParamDomainIdentifierRole::ScaleValue);
    }
    if previous
        .iter()
        .any(|item| named_field(source, item.clone()).is_some())
    {
        return None;
    }
    (previous.len() == 2).then_some(ParamDomainIdentifierRole::ScaleValue)
}

pub(super) fn binding_range_identifier_role_at(
    source: &str,
    start: usize,
    end: usize,
) -> Option<BindingRangeIdentifierRole> {
    let stack = open_brace_stack(source, start);
    let open = *stack.last()?;
    if !is_binding_range_open(source, open) {
        return None;
    }
    let items = split_top_level_items(source, open + 1, end);
    let (current, previous) = items.split_last()?;
    let identifier = source.get(start..end)?;

    if BINDING_RANGE_FIELDS.contains(&identifier) && source[current.start..start].trim().is_empty()
    {
        let after_identifier = source[end..].trim_start();
        if after_identifier.starts_with('=') && !after_identifier.starts_with("==") {
            return Some(BindingRangeIdentifierRole::Field);
        }
    }
    if !is_binding_range_mode(identifier) {
        return None;
    }
    if binding_range_named_field(source, current.clone()) == Some("mode") {
        return Some(BindingRangeIdentifierRole::ModeValue);
    }
    let current_before = source[current.start..start].trim();
    if current_before.is_empty()
        && previous.iter().any(|item| {
            !source[item.clone()].trim().is_empty()
                && binding_range_named_field(source, item.clone()) != Some("mode")
        })
    {
        return Some(BindingRangeIdentifierRole::ModeValue);
    }
    None
}

fn value_kind_for_field(field: &str) -> ParamDomainValueKind {
    match field {
        "min" | "max" | "curve" | "step" => ParamDomainValueKind::Expression,
        "scale" => ParamDomainValueKind::Scale,
        "unit" => ParamDomainValueKind::Unit,
        _ => ParamDomainValueKind::None,
    }
}

fn active_param_domain_open(source: &str, offset: usize) -> Option<usize> {
    let stack = open_brace_stack(source, offset);
    let open = *stack.last()?;
    is_param_domain_open(source, open, &stack[..stack.len() - 1]).then_some(open)
}

fn is_param_domain_open(source: &str, open: usize, parent_braces: &[usize]) -> bool {
    let line_start = source[..open].rfind('\n').map_or(0, |index| index + 1);
    let declaration_start = parent_braces
        .last()
        .map_or(line_start, |parent| line_start.max(parent + 1));
    let declaration_prefix = source[declaration_start..open].trim();
    if leading_identifier(declaration_prefix).is_none()
        || is_domain_section_header(declaration_prefix)
    {
        return false;
    }

    if let Some(parent) = parent_braces.last() {
        let parent_line_start = source[..*parent].rfind('\n').map_or(0, |index| index + 1);
        let parent_prefix = source[parent_line_start..*parent].trim();
        return parent_braces.len() == 1 && is_domain_section_header(parent_prefix);
    }

    let declaration_line = &source[line_start..open];
    let declaration_indent = leading_indent_len(declaration_line);
    if declaration_indent == 0 {
        return false;
    }
    source[..line_start]
        .lines()
        .rev()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && leading_indent_len(line) < declaration_indent
        })
        .is_some_and(|line| leading_indent_len(line) == 0 && is_domain_section_header(line.trim()))
}

fn is_buffer_count_open(source: &str, open: usize, parent_braces: &[usize]) -> bool {
    let line_start = source[..open].rfind('\n').map_or(0, |index| index + 1);
    let declaration_start = parent_braces
        .last()
        .map_or(line_start, |parent| line_start.max(parent + 1));
    let declaration_prefix = source[declaration_start..open].trim();
    if leading_identifier(declaration_prefix).is_none()
        || is_buffer_section_header(declaration_prefix)
    {
        return false;
    }

    if let Some(parent) = parent_braces.last() {
        let parent_line_start = source[..*parent].rfind('\n').map_or(0, |index| index + 1);
        return is_buffer_section_header(source[parent_line_start..*parent].trim());
    }

    let declaration_indent = leading_indent_len(&source[line_start..open]);
    source[..line_start]
        .lines()
        .rev()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && leading_indent_len(line) < declaration_indent
        })
        .is_some_and(|line| is_buffer_section_header(line.trim()))
}

fn is_binding_range_open(source: &str, open: usize) -> bool {
    let line_start = source[..open].rfind('\n').map_or(0, |index| index + 1);
    let declaration_prefix = source[line_start..open].trim();
    leading_identifier(declaration_prefix).is_some()
        && top_level_equal(declaration_prefix).is_some()
        && !inside_domain_section(source, line_start)
}

fn inside_domain_section(source: &str, line_start: usize) -> bool {
    let line = &source[line_start..];
    let indent = leading_indent_len(line);
    source[..line_start]
        .lines()
        .rev()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && leading_indent_len(line) < indent
        })
        .is_some_and(|line| {
            let header = line.trim().strip_suffix('{').unwrap_or(line.trim()).trim();
            is_domain_section_header(header)
        })
}

fn is_domain_section_header(text: &str) -> bool {
    let body = text.trim().strip_suffix(':').unwrap_or(text.trim()).trim();
    ["params", "kins", "ins", "inputs"].iter().any(|keyword| {
        body == *keyword
            || body
                .strip_prefix(keyword)
                .is_some_and(|rest| rest.starts_with('<') && rest.ends_with('>'))
    })
}

fn is_buffer_section_header(text: &str) -> bool {
    let body = text.trim().strip_suffix(':').unwrap_or(text.trim()).trim();
    body == "buffers"
        || body
            .strip_prefix("buffers")
            .is_some_and(|rest| rest.starts_with('<') && rest.ends_with('>'))
}

fn leading_identifier(text: &str) -> Option<&str> {
    let mut chars = text.char_indices();
    let (_, first) = chars.next()?;
    if first != '_' && !first.is_ascii_alphabetic() {
        return None;
    }
    let end = chars
        .find(|(_, ch)| *ch != '_' && !ch.is_ascii_alphanumeric())
        .map_or(text.len(), |(index, _)| index);
    Some(&text[..end])
}

fn canonical_field(name: &str) -> Option<&'static str> {
    PARAM_DOMAIN_FIELDS
        .iter()
        .copied()
        .find(|field| *field == name)
}

fn named_field(source: &str, item: std::ops::Range<usize>) -> Option<&'static str> {
    let text = &source[item];
    let equal = top_level_equal(text)?;
    canonical_field(text[..equal].trim())
}

fn top_level_equal(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut state = LexState::Code;
    let mut paren_depth = 0_u32;
    let mut bracket_depth = 0_u32;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match state {
            LexState::Comment => {
                if byte == b'\n' {
                    state = LexState::Code;
                }
            }
            LexState::String { escaped } => {
                if escaped {
                    state = LexState::String { escaped: false };
                } else if byte == b'\\' {
                    state = LexState::String { escaped: true };
                } else if byte == b'"' {
                    state = LexState::Code;
                }
            }
            LexState::Code => match byte {
                b'#' => state = LexState::Comment,
                b'"' => state = LexState::String { escaped: false },
                b'(' => paren_depth += 1,
                b')' => paren_depth = paren_depth.saturating_sub(1),
                b'[' => bracket_depth += 1,
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                b'=' if paren_depth == 0
                    && bracket_depth == 0
                    && !bytes
                        .get(index.wrapping_sub(1))
                        .is_some_and(|byte| matches!(*byte, b'=' | b'!' | b'<' | b'>'))
                    && bytes.get(index + 1) != Some(&b'=') =>
                {
                    return Some(index);
                }
                _ => {}
            },
        }
        index += 1;
    }
    None
}

fn split_top_level_items(source: &str, start: usize, end: usize) -> Vec<std::ops::Range<usize>> {
    let bytes = source.as_bytes();
    let mut items = Vec::new();
    let mut item_start = start;
    let mut state = LexState::Code;
    let mut paren_depth = 0_u32;
    let mut bracket_depth = 0_u32;
    let mut brace_depth = 0_u32;
    let mut index = start;
    while index < end {
        let byte = bytes[index];
        match state {
            LexState::Comment => {
                if byte == b'\n' {
                    state = LexState::Code;
                }
            }
            LexState::String { escaped } => {
                if escaped {
                    state = LexState::String { escaped: false };
                } else if byte == b'\\' {
                    state = LexState::String { escaped: true };
                } else if byte == b'"' {
                    state = LexState::Code;
                }
            }
            LexState::Code => match byte {
                b'#' => state = LexState::Comment,
                b'"' => state = LexState::String { escaped: false },
                b'(' => paren_depth += 1,
                b')' => paren_depth = paren_depth.saturating_sub(1),
                b'[' => bracket_depth += 1,
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                b'{' => brace_depth += 1,
                b'}' => brace_depth = brace_depth.saturating_sub(1),
                b',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                    items.push(item_start..index);
                    item_start = index + 1;
                }
                _ => {}
            },
        }
        index += 1;
    }
    items.push(item_start..end);
    items
}

fn open_brace_stack(source: &str, end: usize) -> Vec<usize> {
    let bytes = source.as_bytes();
    let mut stack = Vec::new();
    let mut state = LexState::Code;
    let mut index = 0;
    while index < end {
        let byte = bytes[index];
        match state {
            LexState::Comment => {
                if byte == b'\n' {
                    state = LexState::Code;
                }
            }
            LexState::String { escaped } => {
                if escaped {
                    state = LexState::String { escaped: false };
                } else if byte == b'\\' {
                    state = LexState::String { escaped: true };
                } else if byte == b'"' {
                    state = LexState::Code;
                }
            }
            LexState::Code => match byte {
                b'#' => state = LexState::Comment,
                b'"' => state = LexState::String { escaped: false },
                b'{' => stack.push(index),
                b'}' => {
                    stack.pop();
                }
                _ => {}
            },
        }
        index += 1;
    }
    stack
}

fn leading_indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

#[derive(Debug, Clone, Copy)]
enum LexState {
    Code,
    Comment,
    String { escaped: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_named_and_positional_scale_positions() {
        let named = "params:\n  cutoff = 440.0 {20, 20000, scale = lo";
        let named_context =
            completion_context_at(named, named.len()).expect("named parameter domain");
        assert_eq!(named_context.value_kind, ParamDomainValueKind::Scale);
        assert!(!named_context.allow_fields);

        let positional = "params:\n  cutoff = 440.0 {20, 20000, lo";
        let positional_context =
            completion_context_at(positional, positional.len()).expect("positional domain");
        assert_eq!(positional_context.value_kind, ParamDomainValueKind::Scale);
        assert!(positional_context.allow_fields);
    }

    #[test]
    fn recognizes_buffer_count_fields_and_values() {
        let field = "buffers:\n  bank: f32 {co";
        assert_eq!(
            buffer_count_completion_context_at(field, field.len() - 2),
            Some(BufferCountCompletionKind::Field)
        );

        let value = "buffers:\n  bank: f32 {count = VO";
        assert_eq!(
            buffer_count_completion_context_at(value, value.len() - 2),
            Some(BufferCountCompletionKind::Expression)
        );

        let proc_field = "proc Reader:\n  buffers:\n    bank: f32 {";
        assert_eq!(
            buffer_count_completion_context_at(proc_field, proc_field.len()),
            Some(BufferCountCompletionKind::Field)
        );
    }

    #[test]
    fn recognizes_integer_binding_range_modes() {
        let positional = "sample:\n  index = 0 {0..15, wr";
        let positional = binding_range_completion_context_at(positional, positional.len())
            .expect("positional binding range");
        assert!(positional.allow_fields);
        assert!(positional.allow_bare_mode);
        assert_eq!(positional.value_kind, BindingRangeValueKind::None);

        let named = "sample:\n  index: i32 = 0 {0..15, mode = wr";
        let named = binding_range_completion_context_at(named, named.len())
            .expect("named binding range mode");
        assert!(!named.allow_fields);
        assert!(!named.allow_bare_mode);
        assert_eq!(named.value_kind, BindingRangeValueKind::Mode);
    }

    #[test]
    fn recognizes_named_binding_range_domains() {
        let source = "sample:\n  index = 0 {range = 0..SIZE, mode = ";
        let context = binding_range_completion_context_at(source, source.len())
            .expect("named binding range bound");
        assert!(context.used_fields.contains("range"));
        assert!(!context.allow_fields);
        assert_eq!(context.value_kind, BindingRangeValueKind::Mode);
    }

    #[test]
    fn positional_binding_domain_does_not_offer_duplicate_named_domains() {
        let source = "sample:\n  index = 0 {0..16, ";
        let context = binding_range_completion_context_at(source, source.len())
            .expect("positional binding range");
        assert!(context.used_fields.is_empty());
        assert!(!context.used_fields.contains("mode"));
        assert!(context.allow_bare_mode);
    }

    #[test]
    fn classifies_binding_range_fields_and_modes() {
        let source = "sample:\n  index = 0 {range = 0..16, mode = wrap}\n";
        for (name, expected) in [
            ("range", BindingRangeIdentifierRole::Field),
            ("mode", BindingRangeIdentifierRole::Field),
            ("wrap", BindingRangeIdentifierRole::ModeValue),
        ] {
            let start = source.find(name).expect("identifier");
            assert_eq!(
                binding_range_identifier_role_at(source, start, start + name.len()),
                Some(expected),
                "{name}"
            );
        }

        let source = "sample:\n  index = 0 {16, clamp}\n";
        let start = source.find("clamp").expect("mode");
        assert_eq!(
            binding_range_identifier_role_at(source, start, start + "clamp".len()),
            Some(BindingRangeIdentifierRole::ModeValue)
        );
    }

    #[test]
    fn does_not_treat_parameter_domains_as_binding_ranges() {
        let source = "proc Voice:\n  params:\n    cutoff = 440.0 {20, 20000, lo";
        assert!(binding_range_completion_context_at(source, source.len()).is_none());
    }

    #[test]
    fn recognizes_input_domains_and_excludes_them_from_binding_ranges() {
        for header in ["ins", "inputs", "ins<f64>"] {
            let source = format!("{header}:\n  cutoff = 440.0 {{20, 20000, lo");
            let context = completion_context_at(&source, source.len()).expect("input domain");
            assert_eq!(context.value_kind, ParamDomainValueKind::Scale, "{header}");
            assert!(
                binding_range_completion_context_at(&source, source.len()).is_none(),
                "{header}"
            );
        }
    }

    #[test]
    fn ignores_processor_local_parameter_ranges() {
        let source = "proc Voice:\n  params:\n    cutoff = 440.0 {sc";
        assert!(completion_context_at(source, source.len()).is_none());
    }

    #[test]
    fn recognizes_top_level_brace_style_parameter_domains() {
        let source = "params {\n  cutoff = 440.0 {min = 20, sc";
        let context = completion_context_at(source, source.len()).expect("parameter domain");
        assert!(context.used_fields.contains("min"));
        assert!(context.allow_fields);
    }

    #[test]
    fn keeps_single_positional_bound_completions_unambiguous() {
        let shorthand = "params:\n  cutoff = 440.0 {20000, scale = linear, ";
        let context = completion_context_at(shorthand, shorthand.len()).expect("parameter domain");
        assert!(!context.used_fields.contains("min"));
        assert!(!context.used_fields.contains("max"));

        let named_min = "params:\n  cutoff = 440.0 {20000, min = 20, ";
        let context = completion_context_at(named_min, named_min.len()).expect("parameter domain");
        assert!(context.used_fields.contains("min"));
        assert!(context.used_fields.contains("max"));

        let named_max = "params:\n  cutoff = 440.0 {20, max = 20000, ";
        let context = completion_context_at(named_max, named_max.len()).expect("parameter domain");
        assert!(context.used_fields.contains("min"));
        assert!(context.used_fields.contains("max"));
    }
}
