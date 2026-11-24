use std::{cmp::Reverse, collections::HashMap, ops::Range};

use super::super::preprocess::{is_continuation_opening_brace, split_comment, unquoted_chars};

const WIDTH: usize = 100;
const COMMENT_SEPARATOR: &str = "  #";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Atom,
    SectionKeyword,
    Operator,
    Prefix,
    Tight,
    Open,
    PackedOpen,
    Close,
    BlockOpen,
    BlockClose,
    Punctuation,
}

impl Kind {
    fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::PackedOpen)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ListId {
    pub index: usize,
    pub packed: bool,
}

#[derive(Clone, Copy)]
pub(super) enum ConditionPart {
    Start { grouped: bool },
    Operator,
    End,
}

#[derive(Clone, Copy)]
pub(super) struct Token<'a> {
    pub text: &'a str,
    pub kind: Kind,
    pub indent: usize,
    // Only the opener, its own commas and its closer share this identifier.
    pub list: Option<ListId>,
    pub brace_continues: Option<bool>,
    pub condition: Option<ConditionPart>,
}

pub(super) fn indentation(tokens: &[Token<'_>]) -> usize {
    tokens
        .iter()
        .take_while(|t| matches!(t.kind, Kind::Close | Kind::BlockClose))
        .last()
        .unwrap_or(&tokens[0])
        .indent
}

fn needs_space(left: Token<'_>, right: Token<'_>) -> bool {
    // Keep adjacent angle closers from forming a shift in a namespace argument.
    if left.text == ">" && right.text == ">" {
        return true;
    }
    if matches!(right.text, "," | ";" | ":") || right.kind == Kind::Close {
        return false;
    }
    if left.kind.is_open() || left.kind == Kind::Prefix {
        return false;
    }
    if right.kind == Kind::BlockClose {
        return left.kind != Kind::BlockOpen;
    }
    if left.kind == Kind::Operator || right.kind == Kind::Operator {
        return true;
    }
    // Joining "0. .. 5" would instead tokenize as "0 .. .5".
    if left.kind == Kind::Atom && left.text.ends_with('.') && right.text.starts_with("..") {
        return true;
    }
    if left.kind == Kind::Tight || right.kind == Kind::Tight {
        return false;
    }
    if right.kind.is_open() && right.text != "{" {
        return left.kind == Kind::SectionKeyword
            || matches!(
                left.text,
                "," | ";" | ":" | "if" | "elif" | "while" | "loop" | "return" | "in"
            );
    }
    true
}

/// Use the outermost comma-separated list on a long line. Commas belonging to
/// nested calls, type arguments, range metadata and literals stay with the item.
fn list_breaks(tokens: &[Token<'_>], comment: Option<&str>) -> Option<Vec<usize>> {
    #[derive(Default)]
    struct List {
        open: Option<usize>,
        close: Option<usize>,
        commas: Vec<usize>,
    }

    // A comment can put either delimiter in a different slice. The token's
    // list identifier still tells us which commas may break together.
    let mut groups = HashMap::<ListId, List>::new();
    for (i, token) in tokens.iter().enumerate() {
        if let Some(id) = token.list {
            let group = groups.entry(id).or_default();
            match token.kind {
                Kind::Open | Kind::PackedOpen => group.open = Some(i),
                Kind::Close => group.close = Some(i),
                _ => group.commas.push(i + 1),
            }
        }
    }
    groups
        .into_iter()
        .filter_map(|(id, group)| {
            if group.commas.is_empty() && !id.packed {
                return None;
            }
            let start = group.open.unwrap_or(0);
            let end = group.close.unwrap_or(tokens.len());
            // Split equal-sized siblings near the middle to bound recursion on call chains.
            let priority = (
                end - start,
                Reverse((start + end).abs_diff(tokens.len())),
                Reverse(id.index),
            );
            let opening = group.open.map(|i| i + 1);
            let commas = if id.packed {
                pack_rows(
                    tokens,
                    opening.unwrap_or(0)..end,
                    &group.commas,
                    comment.filter(|_| group.close.is_none()),
                )
            } else {
                group.commas
            };
            let mut breaks: Vec<_> = opening
                .into_iter()
                .chain(commas)
                .chain(group.close)
                .filter(|&i| i > 0 && i < tokens.len())
                .collect();
            breaks.dedup();
            (!breaks.is_empty()).then_some((priority, breaks))
        })
        .max_by_key(|(priority, _)| *priority)
        .map(|(_, breaks)| breaks)
}

fn pack_rows(
    tokens: &[Token<'_>],
    body: Range<usize>,
    commas: &[usize],
    comment: Option<&str>,
) -> Vec<usize> {
    let mut breaks = Vec::new();
    if body.is_empty() {
        return breaks;
    }
    let mut start = body.start;
    let mut row_width = 0;
    for end in commas
        .iter()
        .copied()
        .take_while(|&i| i < body.end)
        .chain(std::iter::once(body.end))
    {
        let item = &tokens[start..end];
        let mut width = item.iter().map(|t| t.text.chars().count()).sum::<usize>()
            + item.windows(2).filter(|p| needs_space(p[0], p[1])).count();
        if end == body.end {
            width += comment.map_or(0, |text| {
                COMMENT_SEPARATOR.len() + text.trim_end().chars().count()
            });
        }
        if start != body.start && row_width + 1 + width <= WIDTH {
            row_width += 1 + width;
        } else {
            if start != body.start {
                breaks.push(start);
            }
            row_width = 2 * indentation(item) + width;
        }
        start = end;
    }
    breaks
}

pub(super) fn render(
    tokens: &[Token<'_>],
    authored: &[(usize, &str)],
    comment: Option<&str>,
    output: &mut Vec<String>,
) {
    let first = output.len();
    wrap(tokens, comment, output);
    let rendered = &output[first..];
    if preserves_braces(tokens, rendered)
        && (authored.len() == 1 || rendered.iter().all(|line| line.chars().count() <= WIDTH))
    {
        return;
    }

    // A line break can change how preprocessing interprets a brace. Keep
    // authored breaks if reflow changes that interpretation or cannot fit.
    output.truncate(first);
    let mut start = 0;
    for &(end, raw) in authored {
        let part = &tokens[start..end];
        let (_, comment) = split_comment(raw);
        let first = output.len();
        wrap(part, comment, output);
        if !preserves_braces(part, &output[first..]) {
            output.truncate(first);
            let line = render_line(part, comment);
            if preserves_braces(part, std::slice::from_ref(&line)) {
                output.push(line);
            } else {
                output.push(format!("{}{}", "  ".repeat(indentation(part)), raw.trim()));
            }
        }
        start = end;
    }
}

fn preserves_braces(tokens: &[Token<'_>], lines: &[String]) -> bool {
    let authored = tokens.iter().filter_map(|token| token.brace_continues);
    let rendered = lines
        .iter()
        .filter(|line| line.contains('{'))
        .flat_map(|line| {
            unquoted_chars(line)
                .take_while(|&(_, ch)| ch != '#')
                .filter(|&(_, ch)| ch == '{')
                .map(|(column, _)| is_continuation_opening_brace(line, column))
        });
    authored.eq(rendered)
}

fn wrap_parts(
    tokens: &[Token<'_>],
    breaks: &[usize],
    comment: Option<&str>,
    output: &mut Vec<String>,
) {
    let mut start = 0;
    for &end in breaks {
        let part = &tokens[start..end];
        wrap(part, None, output);
        start = end;
    }
    wrap(&tokens[start..], comment, output);
}

fn wrap_condition(tokens: &[Token<'_>], comment: Option<&str>, output: &mut Vec<String>) -> bool {
    let mut start = None;
    let mut operators = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        match token.condition {
            Some(ConditionPart::Start { grouped }) => {
                start = Some((i, grouped));
                operators.clear();
            }
            Some(ConditionPart::Operator) => operators.push(i),
            Some(ConditionPart::End) => {
                let Some((first, grouped)) = start.take() else {
                    continue;
                };
                let end = i + 1;
                // Only wrap a complete header. Comments and blank lines can
                // put its beginning or closing delimiter in another slice.
                let delimiter = end + tokens[end..].iter().take_while(|t| t.text == ")").count();
                if first == 0
                    || operators.is_empty()
                    || !matches!(tokens.get(delimiter).map(|t| t.text), Some(":" | "{"))
                    || render_line(&tokens[..=delimiter], None).chars().count() <= WIDTH
                {
                    continue;
                }
                let breaks = std::iter::once(first).chain(operators).chain([end]);
                if grouped {
                    wrap_parts(tokens, &breaks.collect::<Vec<_>>(), comment, output);
                } else {
                    let mut wrapped = tokens.to_vec();
                    for operand in &mut wrapped[first..end] {
                        operand.indent += 1;
                    }
                    let parenthesis = |text, kind| Token {
                        text,
                        kind,
                        indent: tokens[first].indent,
                        list: None,
                        brace_continues: None,
                        condition: None,
                    };
                    wrapped.insert(end, parenthesis(")", Kind::Close));
                    wrapped.insert(first, parenthesis("(", Kind::Open));
                    let breaks: Vec<_> = breaks.map(|i| i + 1).collect();
                    wrap_parts(&wrapped, &breaks, comment, output);
                }
                return true;
            }
            None => {}
        }
    }
    false
}

fn wrap(tokens: &[Token<'_>], comment: Option<&str>, output: &mut Vec<String>) {
    let line = render_line(tokens, comment);
    if line.chars().count() > WIDTH {
        if wrap_condition(tokens, comment, output) {
            return;
        }
        if let Some(breaks) = list_breaks(tokens, comment) {
            wrap_parts(tokens, &breaks, comment, output);
            return;
        }
    }
    output.push(line);
}

pub(super) fn render_line(tokens: &[Token<'_>], comment: Option<&str>) -> String {
    let mut line = "  ".repeat(indentation(tokens));
    for (i, &token) in tokens.iter().enumerate() {
        if i > 0 && needs_space(tokens[i - 1], token) {
            line.push(' ');
        }
        line.push_str(token.text);
    }
    if let Some(text) = comment {
        line.push_str(COMMENT_SEPARATOR);
        line.push_str(text.trim_end());
    }
    line
}
