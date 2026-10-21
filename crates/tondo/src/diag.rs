//! Compiler diagnostics (spec 22).

use crate::source::{SourceMap, Span};
use std::fmt::Write;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable code from spec 22.2; `None` for toolchain limitations.
    pub code: Option<&'static str>,
    pub message: String,
    pub span: Option<Span>,
}

#[derive(Default)]
pub struct Diagnostics {
    pub list: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn error(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.list.push(Diagnostic {
            severity: Severity::Error,
            code: Some(code),
            message: message.into(),
            span: Some(span),
        });
    }

    pub fn warning(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.list.push(Diagnostic {
            severity: Severity::Warning,
            code: Some(code),
            message: message.into(),
            span: Some(span),
        });
    }

    /// A diagnostic without a source location (spec 22.3 null location).
    pub fn error_without_span(&mut self, code: &'static str, message: impl Into<String>) {
        self.list.push(Diagnostic { severity: Severity::Error, code: Some(code), message: message.into(), span: None });
    }

    /// A construct the specification defines but this compiler does not implement yet.
    pub fn unsupported(&mut self, span: Span, what: &str) {
        self.list.push(Diagnostic {
            severity: Severity::Error,
            code: None,
            message: format!("{what} is not implemented yet"),
            span: Some(span),
        });
    }

    pub fn has_errors(&self) -> bool {
        self.list.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn render(&self, sources: &SourceMap) -> String {
        let mut out = String::new();
        for d in &self.list {
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            match d.code {
                Some(code) => writeln!(out, "{severity}[{code}]: {}", d.message).unwrap(),
                None => writeln!(out, "{severity}: {}", d.message).unwrap(),
            }
            if let Some(span) = d.span {
                render_snippet(&mut out, sources, span);
            }
        }
        out
    }
}

fn render_snippet(out: &mut String, sources: &SourceMap, span: Span) {
    let file = sources.file_of(span.start);
    let (line, column) = file.line_col(span.start);
    let (end_line, end_column) = file.line_col(span.end.max(span.start));
    let text = file.line_text(line);
    let gutter = line.to_string().len();
    writeln!(out, "{:gutter$}--> {}:{line}:{column}", "", file.path.display()).unwrap();
    writeln!(out, "{:gutter$} |", "").unwrap();
    writeln!(out, "{line} | {text}").unwrap();
    let width = if end_line == line { end_column.saturating_sub(column).max(1) } else { 1 };
    let indent: String = text.chars().take(column - 1).map(|c| if c == '\t' { '\t' } else { ' ' }).collect();
    writeln!(out, "{:gutter$} | {indent}{}", "", "^".repeat(width)).unwrap();
}
