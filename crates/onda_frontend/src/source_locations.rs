//! Source provenance is owned by shared immutable handles. Cloning an AST or
//! moving a single location to another thread retains its file and import trace;
//! dropping the last location releases the context. No process-wide ID table.
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq, Hash)]
struct SourceContext {
    file: Option<String>,
    trace: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
pub struct Span {
    pub line: u32,
    pub column: u16,
    pub end_line_delta: u16,
    pub end_column: u16,
    context: Option<Arc<SourceContext>>,
}

impl Span {
    pub const ZERO: Span = Span {
        line: 0,
        column: 0,
        end_line_delta: 0,
        end_column: 0,
        context: None,
    };

    pub fn new(line: usize, column: usize, end_line: usize, end_column: usize) -> Self {
        Self::new_with_context(line, column, end_line, end_column, None)
    }

    fn new_with_context(
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
        context: Option<Arc<SourceContext>>,
    ) -> Self {
        let line32 = line.min(u32::MAX as usize) as u32;
        let delta = (end_line as u32).saturating_sub(line32);
        let column16 = column.min(u16::MAX as usize) as u16;
        let end_column16 = if line == 0 {
            0
        } else if end_line <= line {
            end_column
                .max(column.saturating_add(1))
                .min(u16::MAX as usize) as u16
        } else {
            end_column.max(1).min(u16::MAX as usize) as u16
        };
        Self {
            line: line32,
            column: column16,
            end_line_delta: delta.min(u16::MAX as u32) as u16,
            end_column: end_column16,
            context,
        }
    }

    pub fn end_line(&self) -> u32 {
        self.line + self.end_line_delta as u32
    }

    pub fn file(&self) -> Option<String> {
        SourceLoc::from(self).file()
    }

    pub fn shares_source_file(&self, other: &Self) -> bool {
        self.context.as_ref().and_then(|c| c.file.as_ref())
            == other.context.as_ref().and_then(|c| c.file.as_ref())
    }

    pub fn trace(&self) -> Vec<String> {
        SourceLoc::from(self).trace()
    }

    pub fn is_zero(&self) -> bool {
        self.line == 0
    }

    pub fn as_ref(&self) -> Option<&Span> {
        if self.is_zero() {
            None
        } else {
            Some(self)
        }
    }

    pub fn cloned(&self) -> Option<Span> {
        if self.is_zero() {
            None
        } else {
            Some(self.clone())
        }
    }

    pub fn expect(&self, message: &str) -> Span {
        assert!(!self.is_zero(), "{message}");
        self.clone()
    }

    pub fn map<T>(&self, f: impl FnOnce(&Span) -> T) -> Option<T> {
        self.as_ref().map(f)
    }

    pub fn or(&self, fallback: impl Into<Span>) -> Span {
        if self.is_zero() {
            fallback.into()
        } else {
            self.clone()
        }
    }

    pub fn spanning(a: Span, b: Span) -> Span {
        if a.is_zero() {
            return b;
        }
        if b.is_zero() {
            return a;
        }
        let end = b.end_line().max(a.end_line());
        let end_column = if b.end_line() > a.end_line() {
            b.end_column
        } else if a.end_line() > b.end_line() {
            a.end_column
        } else {
            a.end_column.max(b.end_column)
        };
        Span::new_with_context(
            a.line as usize,
            a.column as usize,
            end as usize,
            end_column as usize,
            a.context.or(b.context),
        )
    }
}

impl From<&SourceLoc> for Span {
    fn from(loc: &SourceLoc) -> Self {
        Span::new_with_context(
            loc.line,
            loc.column,
            loc.end_line,
            loc.end_column,
            loc.context.clone(),
        )
    }
}

impl From<SourceLoc> for Span {
    fn from(loc: SourceLoc) -> Self {
        loc.span()
    }
}

impl From<Option<&SourceLoc>> for Span {
    fn from(loc: Option<&SourceLoc>) -> Self {
        loc.map(Span::from).unwrap_or_default()
    }
}

impl From<Option<SourceLoc>> for Span {
    fn from(loc: Option<SourceLoc>) -> Self {
        loc.map(Span::from).unwrap_or_default()
    }
}

impl From<Option<&Span>> for Span {
    fn from(span: Option<&Span>) -> Self {
        span.cloned().unwrap_or_default()
    }
}

impl From<Option<Span>> for Span {
    fn from(span: Option<Span>) -> Self {
        span.unwrap_or_default()
    }
}

impl From<Span> for SourceLoc {
    fn from(span: Span) -> Self {
        if span.is_zero() {
            return SourceLoc::ZERO;
        }
        SourceLoc {
            line: span.line as usize,
            column: span.column as usize,
            end_line: span.end_line() as usize,
            end_column: span.end_column as usize,
            context: span.context,
        }
    }
}

impl From<&Span> for SourceLoc {
    fn from(span: &Span) -> Self {
        span.clone().into()
    }
}

impl From<Option<&Span>> for SourceLoc {
    fn from(span: Option<&Span>) -> Self {
        span.cloned().map(SourceLoc::from).unwrap_or_default()
    }
}

impl From<Option<Span>> for SourceLoc {
    fn from(span: Option<Span>) -> Self {
        span.map(SourceLoc::from).unwrap_or_default()
    }
}

impl From<&SourceLoc> for SourceLoc {
    fn from(loc: &SourceLoc) -> Self {
        loc.clone()
    }
}

impl From<Option<&SourceLoc>> for SourceLoc {
    fn from(loc: Option<&SourceLoc>) -> Self {
        loc.cloned().unwrap_or_default()
    }
}

impl From<Option<SourceLoc>> for SourceLoc {
    fn from(loc: Option<SourceLoc>) -> Self {
        loc.unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct SourceLoc {
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    context: Option<Arc<SourceContext>>,
}

impl SourceLoc {
    pub const ZERO: SourceLoc = SourceLoc {
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
        context: None,
    };

    pub fn new(
        file: Option<String>,
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
        trace: Vec<String>,
    ) -> Self {
        let context =
            (file.is_some() || !trace.is_empty()).then(|| Arc::new(SourceContext { file, trace }));
        let end_column = if line == 0 {
            0
        } else if end_line <= line {
            end_column.max(column.saturating_add(1))
        } else {
            end_column.max(1)
        };
        Self {
            line,
            column,
            end_line,
            end_column,
            context,
        }
    }

    pub(crate) fn at(
        &self,
        line: usize,
        column: usize,
        end_line: usize,
        end_column: usize,
    ) -> Self {
        Self {
            line,
            column,
            end_line,
            end_column,
            context: self.context.clone(),
        }
    }

    pub fn is_zero(&self) -> bool {
        self.line == 0
    }

    pub fn as_ref(&self) -> Option<&SourceLoc> {
        if self.is_zero() {
            None
        } else {
            Some(self)
        }
    }

    pub fn cloned(&self) -> Option<SourceLoc> {
        if self.is_zero() {
            None
        } else {
            Some(self.clone())
        }
    }

    pub fn expect(&self, message: &str) -> SourceLoc {
        assert!(!self.is_zero(), "{message}");
        self.clone()
    }

    pub fn map<T>(&self, f: impl FnOnce(&SourceLoc) -> T) -> Option<T> {
        self.as_ref().map(f)
    }

    pub fn or(&self, fallback: impl Into<SourceLoc>) -> SourceLoc {
        if self.is_zero() {
            fallback.into()
        } else {
            self.clone()
        }
    }

    pub fn span(&self) -> Span {
        Span::new_with_context(
            self.line,
            self.column,
            self.end_line,
            self.end_column,
            self.context.clone(),
        )
    }

    pub fn file(&self) -> Option<String> {
        self.context
            .as_ref()
            .and_then(|context| context.file.clone())
    }

    pub fn trace(&self) -> Vec<String> {
        self.context
            .as_ref()
            .map_or_else(Vec::new, |context| context.trace.clone())
    }

    pub fn spanning(start: impl Into<SourceLoc>, end: impl Into<SourceLoc>) -> Self {
        let start = start.into();
        let end = end.into();
        if start.is_zero() {
            return end;
        }
        if end.is_zero() {
            return start;
        }
        Self {
            line: start.line,
            column: start.column,
            end_line: end.end_line,
            end_column: end.end_column,
            context: start.context.or(end.context),
        }
    }
}

impl From<&Span> for Span {
    fn from(span: &Span) -> Self {
        span.clone()
    }
}
