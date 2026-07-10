use crate::ast::{SourceLoc, Span};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum DiagCode {
    Syntax = 1000,
    Semantic = 2000,
    Runtime = 3000,
    Internal = 9000,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub code: DiagCode,
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub file: Option<String>,
    pub trace: Vec<String>,
    pub editor_visible: bool,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub struct DiagCtx {
    loc: SourceLoc,
}

impl DiagCtx {
    pub fn new(loc: impl Into<SourceLoc>) -> Self {
        Self { loc: loc.into() }
    }

    pub fn loc(&self) -> SourceLoc {
        self.loc
    }

    pub fn is_empty(&self) -> bool {
        self.loc.is_zero()
    }

    pub fn semantic(&self, message: impl Into<String>, line: usize, column: usize) -> Diagnostic {
        Diagnostic::semantic_ctx(message, line, column, *self)
    }

    pub fn semantic_span(&self, message: impl Into<String>, span: impl Into<Span>) -> Diagnostic {
        Diagnostic::semantic_span_ctx(message, span, *self)
    }
}

impl From<SourceLoc> for DiagCtx {
    fn from(loc: SourceLoc) -> Self {
        Self::new(loc)
    }
}

impl From<&SourceLoc> for DiagCtx {
    fn from(loc: &SourceLoc) -> Self {
        Self::new(*loc)
    }
}

impl From<Span> for DiagCtx {
    fn from(span: Span) -> Self {
        Self::new(span)
    }
}

impl From<&Span> for DiagCtx {
    fn from(span: &Span) -> Self {
        Self::new(*span)
    }
}

impl From<Option<&SourceLoc>> for DiagCtx {
    fn from(loc: Option<&SourceLoc>) -> Self {
        Self::new(loc)
    }
}

impl From<Option<&Span>> for DiagCtx {
    fn from(span: Option<&Span>) -> Self {
        Self::new(span)
    }
}

impl Diagnostic {
    fn point_end_column(column: usize) -> usize {
        if column == 0 {
            0
        } else {
            column.saturating_add(1)
        }
    }

    pub fn syntax(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            code: DiagCode::Syntax,
            message: message.into(),
            line,
            column,
            end_line: line,
            end_column: Self::point_end_column(column),
            file: None,
            trace: Vec::new(),
            editor_visible: true,
        }
    }

    pub fn semantic(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self::semantic_ctx(message, line, column, DiagCtx::default())
    }

    pub fn semantic_span(message: impl Into<String>, span: impl Into<Span>) -> Self {
        Self::semantic_span_ctx(message, span, DiagCtx::default())
    }

    pub fn semantic_ctx(
        message: impl Into<String>,
        line: usize,
        column: usize,
        ctx: impl Into<DiagCtx>,
    ) -> Self {
        let ctx = ctx.into();
        let mut line = line;
        let mut column = column;
        let mut end_line = line;
        let mut end_column = Self::point_end_column(column);
        let mut file = None;
        let mut trace = Vec::new();
        let loc = ctx.loc();
        if !loc.is_zero() {
            if line == 0 && column == 0 {
                line = loc.line;
                column = loc.column;
                end_line = loc.end_line;
                end_column = loc.end_column;
            }
            file = loc.file();
            trace = loc.trace();
        }
        Self {
            code: DiagCode::Semantic,
            message: message.into(),
            line,
            column,
            end_line,
            end_column,
            file,
            trace,
            editor_visible: true,
        }
    }

    pub fn semantic_span_ctx(
        message: impl Into<String>,
        span: impl Into<Span>,
        ctx: impl Into<DiagCtx>,
    ) -> Self {
        let span = span.into();
        if span.is_zero() {
            return Self::semantic_ctx(message, 0, 0, ctx);
        }

        let ctx = ctx.into();
        let mut file = span.file();
        let mut trace = span.trace();
        if file.is_none() && trace.is_empty() {
            let loc = ctx.loc();
            if !loc.is_zero() {
                file = loc.file();
                trace = loc.trace();
            }
        }

        Self {
            code: DiagCode::Semantic,
            message: message.into(),
            line: span.line as usize,
            column: span.column as usize,
            end_line: span.end_line() as usize,
            end_column: span.end_column as usize,
            file,
            trace,
            editor_visible: true,
        }
    }

    pub fn syntax_at(message: impl Into<String>, loc: &SourceLoc) -> Self {
        Self {
            code: DiagCode::Syntax,
            message: message.into(),
            line: loc.line,
            column: loc.column,
            end_line: loc.end_line,
            end_column: loc.end_column,
            file: loc.file(),
            trace: loc.trace(),
            editor_visible: true,
        }
    }

    pub fn runtime(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            code: DiagCode::Runtime,
            message: message.into(),
            line,
            column,
            end_line: line,
            end_column: Self::point_end_column(column),
            file: None,
            trace: Vec::new(),
            editor_visible: true,
        }
    }

    pub fn runtime_at(message: impl Into<String>, loc: &SourceLoc) -> Self {
        Self {
            code: DiagCode::Runtime,
            message: message.into(),
            line: loc.line,
            column: loc.column,
            end_line: loc.end_line,
            end_column: loc.end_column,
            file: loc.file(),
            trace: loc.trace(),
            editor_visible: true,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: DiagCode::Internal,
            message: message.into(),
            line: 0,
            column: 0,
            end_line: 0,
            end_column: 0,
            file: None,
            trace: Vec::new(),
            editor_visible: true,
        }
    }

    pub fn compiler_only(mut self) -> Self {
        self.editor_visible = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_span_uses_span_metadata_without_ambient_context() {
        let loc = SourceLoc::new(Some("<memory>".to_owned()), 1, 1, 1, 4, Vec::new());
        let diag = Diagnostic::semantic_span("bad thing", loc.span());

        assert_eq!(diag.file.as_deref(), Some("<memory>"));
        assert!(diag.trace.is_empty());
        assert_eq!(diag.end_column, 4);
    }

    #[test]
    fn semantic_span_falls_back_to_explicit_context_metadata() {
        let loc = SourceLoc::new(
            Some("test.onda".to_owned()),
            10,
            20,
            12,
            7,
            vec!["included from root.onda".to_owned()],
        );

        let diag =
            Diagnostic::semantic_span_ctx("bad thing", Span::new(3, 4, 5, 9), DiagCtx::from(loc));

        assert_eq!(diag.code, DiagCode::Semantic);
        assert_eq!(diag.message, "bad thing");
        assert_eq!(
            (diag.line, diag.column, diag.end_line, diag.end_column),
            (3, 4, 5, 9)
        );
        assert_eq!(diag.file.as_deref(), Some("test.onda"));
        assert_eq!(diag.trace, vec!["included from root.onda"]);
    }
}
