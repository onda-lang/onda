use super::super::loading_support::{
    annotate_diagnostics_with_file, split_top_level_items, TopLevelItem,
};
use super::super::module_loading::{with_program_syntax, ProgramItemChecks};
use super::super::preprocess::{
    is_continuation_opening_brace, preprocess_with_layout, split_comment,
};
use super::super::{Diagnostic, Rule};
use super::layout::{ConditionPart, Kind, ListId, Token};
use pest::iterators::Pair;
use std::path::Path;

const SHORT_ELEMENT_WIDTH: usize = 10;

#[derive(Default)]
pub(super) struct Line<'a> {
    pub text: &'a str,
    pub block_depth: usize,
    directive: bool,
    // Byte columns in this line, measured before editing any whitespace.
    spans: Vec<(usize, usize, Kind)>,
    condition_parts: Vec<(usize, ConditionPart)>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ItemSyntax {
    Code(Vec<SyntaxNode>),
    Import(String),
    Include(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SyntaxNode {
    rule: Rule,
    children: usize,
    spelling: String,
}

pub(super) fn source_layout<'a>(
    source: &'a str,
    path: &Path,
) -> Result<(Vec<Line<'a>>, Vec<ItemSyntax>), Vec<Diagnostic>> {
    let mut lines: Vec<_> = source
        .lines()
        .map(|text| Line {
            text,
            ..Line::default()
        })
        .collect();
    let syntax = parse_syntax(source, path, Some(&mut lines))?;
    for line in &mut lines {
        line.spans.sort_unstable_by_key(|s| s.0);
        line.condition_parts.sort_unstable_by_key(|part| part.0);
    }
    Ok((lines, syntax))
}

pub(super) fn parse_syntax(
    source: &str,
    path: &Path,
    mut layout: Option<&mut [Line<'_>]>,
) -> Result<Vec<ItemSyntax>, Vec<Diagnostic>> {
    let mut authored_bytes = Vec::new();
    let collect_layout = layout.is_some();
    let (prepared, map) = preprocess_with_layout(
        source,
        |i, depth| {
            if let Some(lines) = &mut layout {
                lines[i].block_depth = depth;
            }
        },
        |bytes| {
            if collect_layout {
                authored_bytes.push(bytes);
            }
        },
    )
    .map_err(|e| annotate_diagnostics_with_file(e, path, 0))?;
    let mut syntax = Vec::new();
    let mut item_checks = ProgramItemChecks::default();
    for item in split_top_level_items(&prepared, &map, path)? {
        let item = match item {
            TopLevelItem::Code {
                text,
                source_line_map,
                start_line,
            } => with_program_syntax(
                &text,
                path,
                start_line - 1,
                &[],
                &source_line_map,
                |program| {
                    item_checks.parse(program.clone(), true)?;
                    if let Some(lines) = &mut layout {
                        mark_syntax(
                            program.clone(),
                            &source_line_map,
                            &authored_bytes[start_line - 1..],
                            lines,
                        );
                    }
                    Ok(ItemSyntax::Code(syntax_nodes(program)))
                },
            )?,
            TopLevelItem::Import { module, line } => {
                if let Some(lines) = &mut layout {
                    lines[line - 1].directive = true;
                }
                ItemSyntax::Import(module)
            }
            TopLevelItem::Include { path, line } => {
                if let Some(lines) = &mut layout {
                    lines[line - 1].directive = true;
                }
                ItemSyntax::Include(path)
            }
        };
        syntax.push(item);
    }
    Ok(syntax)
}

fn condition_parent(rule: Rule) -> bool {
    matches!(rule, Rule::if_cond | Rule::while_stmt)
}

fn condition_body(mut expr: Pair<'_, Rule>) -> Pair<'_, Rule> {
    loop {
        let mut terms = expr.clone().into_inner();
        if terms.len() != 1 {
            return expr;
        }
        let term = terms.next().unwrap();
        if term.as_rule() != Rule::multiline_expr {
            return expr;
        }
        expr = term;
    }
}

fn syntax_nodes(program: Pair<'_, Rule>) -> Vec<SyntaxNode> {
    let mut nodes = Vec::new();
    let mut pending = vec![program.into_inner()];
    let mut condition = false;
    while let Some(pairs) = pending.last_mut() {
        let Some(mut pair) = pairs.next() else {
            pending.pop();
            continue;
        };
        let rule = pair.as_rule();
        // Only grouping around the entire condition is transparent. The
        // operators and any grouping within it are still compared exactly.
        if condition && rule == Rule::expr {
            pair = condition_body(pair);
        }
        condition = condition_parent(rule);
        let text = pair.as_str();
        let children = pair.into_inner();
        nodes.push(syntax_node(text, rule, children.len()));
        if !children.is_empty() {
            pending.push(children);
        }
    }
    nodes
}

fn syntax_node(text: &str, rule: Rule, children: usize) -> SyntaxNode {
    let spelling = if children != 0 {
        String::new()
    } else if matches!(rule, Rule::param_unit | Rule::quoted_text) {
        text.to_owned()
    } else {
        text.lines()
            .flat_map(|line| split_comment(line).0.chars())
            .filter(|c| !c.is_whitespace())
            .collect()
    };
    SyntaxNode {
        rule,
        children,
        spelling,
    }
}

fn mark_syntax(
    program: Pair<'_, Rule>,
    map: &[usize],
    authored_bytes: &[usize],
    lines: &mut [Line],
) {
    let text = program.as_str();
    let starts: Vec<_> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let position = |offset: usize, length: usize| {
        let row = starts.partition_point(|&start| start <= offset) - 1;
        let column = offset - starts[row];
        (column + length <= authored_bytes[row]).then(|| (map[row] - 1, column))
    };
    for pair in program.into_inner().flatten() {
        if condition_parent(pair.as_rule()) {
            let body = condition_body(pair.into_inner().next().unwrap());
            mark_condition(body, |offset, part| {
                if let Some((line, column)) = position(offset, 1) {
                    lines[line].condition_parts.push((column, part));
                }
            });
            continue;
        }
        let mut mark = |offset, length, kind| {
            if let Some((line, column)) = position(offset, length) {
                lines[line].spans.push((column, length, kind));
            }
        };
        let span = pair.as_span();
        let (length, kind) = match pair.as_rule() {
            Rule::sample_block | Rule::sample_nested_block => {
                ("sample".len(), Kind::SectionKeyword)
            }
            Rule::array_lit => {
                let mut elements = pair.into_inner();
                let packed = !elements.is_empty() && elements.all(short_array_element);
                (1, if packed { Kind::PackedOpen } else { Kind::Open })
            }
            Rule::param_domain
            | Rule::port_domain
            | Rule::binding_range
            | Rule::buffer_count
            | Rule::graph_endpoint_set => {
                mark(span.start(), 1, Kind::Open);
                mark(span.end() - 1, 1, Kind::Close);
                continue;
            }
            Rule::slice_range => {
                let start = pair
                    .into_inner()
                    .find(|p| p.as_rule() == Rule::slice_start)
                    .map_or(span.start(), |p| p.as_span().end());
                let mut offset = start;
                for line in text[start..span.end()].split_inclusive('\n') {
                    if let Some(i) = split_comment(line).0.find(':') {
                        mark(offset + i, 1, Kind::Tight);
                        break;
                    }
                    offset += line.len();
                }
                continue;
            }
            Rule::for_step => (1, Kind::Operator),
            Rule::graph_rate
            | Rule::quoted_text
            | Rule::param_unit
            | Rule::number
            | Rule::path_ident
            | Rule::qualified_ident => (span.end() - span.start(), Kind::Atom),
            Rule::cmp_op
            | Rule::shift_op
            | Rule::compound_assign_op
            | Rule::add_op
            | Rule::mul_op
            | Rule::bit_and_op
            | Rule::bit_or_op
            | Rule::bit_xor_op
            | Rule::and_op
            | Rule::or_op
            | Rule::fn_return_arrow => (span.end() - span.start(), Kind::Operator),
            Rule::graph_arrow | Rule::graph_receiver_arrow => (2, Kind::Operator),
            Rule::signed_number | Rule::prefix => (1, Kind::Prefix),
            Rule::for_range_op | Rule::binding_range_op => (span.end() - span.start(), Kind::Tight),
            _ => continue,
        };
        mark(span.start(), length, kind);
    }
}

fn mark_condition(body: Pair<'_, Rule>, mut mark: impl FnMut(usize, ConditionPart)) {
    let mut operators = body
        .clone()
        .into_inner()
        .filter(|term| matches!(term.as_rule(), Rule::and_op | Rule::or_op));
    let Some(first) = operators.next() else {
        return;
    };
    let span = body.as_span();
    mark(
        span.start(),
        ConditionPart::Start {
            grouped: body.as_rule() == Rule::multiline_expr,
        },
    );
    for operator in std::iter::once(first).chain(operators) {
        mark(operator.as_span().start(), ConditionPart::Operator);
    }
    mark(
        span.start() + span.as_str().trim_end().len() - 1,
        ConditionPart::End,
    );
}

fn short_array_element(expr: Pair<'_, Rule>) -> bool {
    let mut terms = expr.into_inner();
    let Some(atom) = terms.next() else {
        return false;
    };
    terms.is_empty()
        && matches!(
            atom.as_rule(),
            Rule::number | Rule::signed_number | Rule::bool_lit | Rule::path_ident
        )
        && atom.as_str().chars().filter(|c| !c.is_whitespace()).count() <= SHORT_ELEMENT_WIDTH
}

struct Delimiter {
    list: Option<ListId>,
    indent_before: usize,
}

#[derive(Default)]
pub(super) struct Nesting {
    stack: Vec<Delimiter>,
    indent: usize,
    next_list: usize,
}

impl Nesting {
    pub(super) fn in_list(&self) -> bool {
        self.stack
            .last()
            .is_some_and(|delimiter| delimiter.list.is_some())
    }

    fn token<'a>(&mut self, text: &'a str, kind: Kind, line: &Line, column: usize) -> Token<'a> {
        let mut indent = self.indent;
        let brace_continues =
            (text == "{").then(|| is_continuation_opening_brace(line.text, column));
        let list = match kind {
            Kind::Open | Kind::PackedOpen | Kind::BlockOpen => {
                // Grammar spans classify comparison and shift operators first,
                // so angle openers here are delimiters.
                let list = matches!(text, "(" | "[" | "<").then(|| {
                    self.next_list += 1;
                    ListId {
                        index: self.next_list,
                        packed: kind == Kind::PackedOpen,
                    }
                });
                self.stack.push(Delimiter {
                    list,
                    indent_before: indent,
                });
                // A colon suite requires its exact indentation on every statement.
                // Only braces recognized as continuations may indent their contents.
                if line.block_depth == 0 || brace_continues != Some(false) {
                    self.indent += 1;
                }
                list
            }
            Kind::Close | Kind::BlockClose => self.stack.pop().and_then(|delimiter| {
                self.indent = delimiter.indent_before;
                indent = self.indent;
                delimiter.list
            }),
            _ if text == "," => self.stack.last().and_then(|d| d.list),
            _ => None,
        };
        Token {
            text,
            kind,
            indent: indent + line.block_depth,
            list,
            brace_continues,
            condition: None,
        }
    }
}

pub(super) fn tokenize<'a>(code: &'a str, line: &Line, nesting: &mut Nesting) -> Vec<Token<'a>> {
    if line.directive {
        let (keyword, path) = code.trim().split_once(char::is_whitespace).unwrap();
        return [keyword, path.trim_start()]
            .map(|text| nesting.token(text, Kind::Atom, line, 0))
            .to_vec();
    }
    let mut spans = line.spans.iter().peekable();
    let mut condition_parts = line.condition_parts.iter().peekable();
    let mut tokens = Vec::new();
    let mut rest = code.trim_start();
    while !rest.is_empty() {
        let column = code.len() - rest.len();
        while spans.peek().is_some_and(|s| s.0 < column) {
            spans.next();
        }
        let (len, kind) = if let Some(&&(_, length, kind)) = spans.peek().filter(|s| s.0 == column)
        {
            (length, kind)
        } else if rest.starts_with("::") {
            (2, Kind::Tight)
        } else if rest.starts_with("=>") {
            (2, Kind::Operator)
        } else {
            let c = rest.chars().next().unwrap();
            match c {
                '(' | '[' | '<' => (1, Kind::Open),
                ')' | ']' | '>' => (1, Kind::Close),
                '{' => (1, Kind::BlockOpen),
                '}' => (1, Kind::BlockClose),
                '=' => (1, Kind::Operator),
                '.' => (1, Kind::Tight),
                c if c.is_ascii_alphanumeric() || c == '_' => (
                    rest.bytes()
                        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                        .count(),
                    Kind::Atom,
                ),
                _ => (c.len_utf8(), Kind::Punctuation),
            }
        };
        let (text, tail) = rest.split_at(len);
        let mut token = nesting.token(text, kind, line, column);
        token.condition = condition_parts
            .next_if(|(offset, _)| *offset < column + len)
            .map(|&(_, part)| part);
        tokens.push(token);
        rest = tail.trim_start();
    }
    tokens
}
