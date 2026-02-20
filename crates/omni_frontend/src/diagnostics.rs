use std::cell::RefCell;

use crate::ast::SourceLoc;

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
    pub file: Option<String>,
    pub trace: Vec<String>,
}

thread_local! {
    static DIAGNOSTIC_LOC_STACK: RefCell<Vec<SourceLoc>> = const { RefCell::new(Vec::new()) };
}

fn current_diagnostic_location() -> Option<SourceLoc> {
    DIAGNOSTIC_LOC_STACK.with(|stack| stack.borrow().last().cloned())
}

pub fn with_diagnostic_location<T>(loc: Option<&SourceLoc>, f: impl FnOnce() -> T) -> T {
    if let Some(loc) = loc {
        DIAGNOSTIC_LOC_STACK.with(|stack| stack.borrow_mut().push(loc.clone()));
        let out = f();
        DIAGNOSTIC_LOC_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
        out
    } else {
        f()
    }
}

impl Diagnostic {
    pub fn syntax(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            code: DiagCode::Syntax,
            message: message.into(),
            line,
            column,
            file: None,
            trace: Vec::new(),
        }
    }

    pub fn semantic(message: impl Into<String>, line: usize, column: usize) -> Self {
        let mut line = line;
        let mut column = column;
        let mut file = None;
        let mut trace = Vec::new();
        if let Some(loc) = current_diagnostic_location() {
            if line == 0 {
                line = loc.line;
            }
            if column == 0 {
                column = loc.column;
            }
            file = loc.file;
            trace = loc.trace;
        }
        Self {
            code: DiagCode::Semantic,
            message: message.into(),
            line,
            column,
            file,
            trace,
        }
    }

    pub fn runtime(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            code: DiagCode::Runtime,
            message: message.into(),
            line,
            column,
            file: None,
            trace: Vec::new(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: DiagCode::Internal,
            message: message.into(),
            line: 0,
            column: 0,
            file: None,
            trace: Vec::new(),
        }
    }
}
