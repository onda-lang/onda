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

pub(super) fn is_identifier_candidate(name: &str) -> bool {
    canonical_field(name).is_some() || ParamScale::from_name(name).is_some()
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
        || is_param_section_header(declaration_prefix)
    {
        return false;
    }

    if let Some(parent) = parent_braces.last() {
        let parent_line_start = source[..*parent].rfind('\n').map_or(0, |index| index + 1);
        let parent_prefix = source[parent_line_start..*parent].trim();
        return parent_braces.len() == 1 && is_param_section_header(parent_prefix);
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
        .is_some_and(|line| leading_indent_len(line) == 0 && is_param_section_header(line.trim()))
}

fn is_param_section_header(text: &str) -> bool {
    let body = text.trim().strip_suffix(':').unwrap_or(text.trim()).trim();
    ["params", "kins"].iter().any(|keyword| {
        body == *keyword
            || body
                .strip_prefix(keyword)
                .is_some_and(|rest| rest.starts_with('<') && rest.ends_with('>'))
    })
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
