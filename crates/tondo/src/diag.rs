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
    /// The repairs it suggests (22.4).
    pub fixes: Vec<Fix>,
}

/// A repair: what replaces each span. `safe` when applying it is what the
/// program meant; otherwise someone has to decide.
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub title: String,
    pub safe: bool,
    pub edits: Vec<(Span, String)>,
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
            fixes: Vec::new(),
        });
    }

    /// Suggests a repair for the diagnostic reported last.
    pub fn fix(&mut self, title: impl Into<String>, safe: bool, edits: Vec<(Span, String)>) {
        if let Some(last) = self.list.last_mut() {
            last.fixes.push(Fix { title: title.into(), safe, edits });
        }
    }

    pub fn warning(&mut self, code: &'static str, span: Span, message: impl Into<String>) {
        self.list.push(Diagnostic {
            severity: Severity::Warning,
            code: Some(code),
            message: message.into(),
            span: Some(span),
            fixes: Vec::new(),
        });
    }

    /// A diagnostic without a source location (spec 22.3 null location).
    pub fn error_without_span(&mut self, code: &'static str, message: impl Into<String>) {
        let message = message.into();
        self.list.push(Diagnostic {
            severity: Severity::Error,
            code: Some(code),
            message,
            span: None,
            fixes: Vec::new(),
        });
    }

    /// A construct the specification defines but this compiler does not implement yet.
    pub fn unsupported(&mut self, span: Span, what: &str) {
        self.list.push(Diagnostic {
            severity: Severity::Error,
            code: None,
            message: format!("{what} is not implemented yet"),
            span: Some(span),
            fixes: Vec::new(),
        });
    }

    pub fn has_errors(&self) -> bool {
        self.list.iter().any(|d| d.severity == Severity::Error)
    }

    /// Orders the diagnostics as 22.6 requires: by module, file, start and
    /// end, errors first, then code and message. Merges repeated ones.
    pub fn sort(&mut self, sources: &SourceMap) {
        let key = |d: &Diagnostic| {
            let location = d.span.map(|span| {
                let file = sources.file_of(span.start);
                (file.module.clone(), file.logical.clone(), span.start, span.end)
            });
            (location, d.severity == Severity::Warning, d.code, d.message.clone())
        };
        self.list.sort_by_cached_key(key);
        self.list.dedup_by(|a, b| {
            let same = a.span == b.span && a.code == b.code && a.message == b.message;
            if same {
                for fix in a.fixes.drain(..) {
                    if !b.fixes.contains(&fix) {
                        b.fixes.push(fix);
                    }
                }
            }
            same
        });
    }

    /// The diagnostics as a JSON array in the form of 22.3. `source_id`
    /// identifies the package or the single file.
    pub fn render_json(&self, sources: &SourceMap, source_id: &str) -> String {
        let mut items = Vec::new();
        for d in &self.list {
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let code = d.code.unwrap_or("E0000");
            let (module, file, range, id_parts) = match d.span {
                Some(span) => {
                    let file = sources.file_of(span.start);
                    let (start, line, column) = file.position(span.start);
                    let (end, end_line, end_column) = file.position(span.end.max(span.start));
                    let range = format!(
                        "{{\"start\":{{\"byte\":{start},\"line\":{line},\"column\":{column}}},\"end\":{{\"byte\":{end},\"line\":{end_line},\"column\":{end_column}}}}}"
                    );
                    let parts = [file.module.clone(), file.logical.clone(), start.to_string(), end.to_string()];
                    (json_string(&file.module), json_string(&file.logical), range, parts)
                }
                None => ("null".into(), "null".into(), "null".into(), Default::default()),
            };
            // Each edit names its own file, like a location (22.4).
            let fixes: Vec<String> = d
                .fixes
                .iter()
                .map(|fix| {
                    let edits: Vec<String> = fix
                        .edits
                        .iter()
                        .map(|(span, replacement)| {
                            let file = sources.file_of(span.start);
                            let (start, line, column) = file.position(span.start);
                            let (end, end_line, end_column) = file.position(span.end.max(span.start));
                            format!(
                                "{{\"source_id\":{},\"module\":{},\"file\":{},\"range\":{{\"start\":{{\"byte\":{start},\"line\":{line},\"column\":{column}}},\"end\":{{\"byte\":{end},\"line\":{end_line},\"column\":{end_column}}}}},\"replacement\":{}}}",
                                json_string(source_id),
                                json_string(&file.module),
                                json_string(&file.logical),
                                json_string(replacement)
                            )
                        })
                        .collect();
                    let applicability = if fix.safe { "safe" } else { "requires-decision" };
                    format!(
                        "{{\"title\":{},\"applicability\":\"{applicability}\",\"edits\":[{}]}}",
                        json_string(&fix.title),
                        edits.join(",")
                    )
                })
                .collect();
            let [module_part, file_part, start, end] = &id_parts;
            let hashed = format!("0.1\n{source_id}\n{module_part}\n{file_part}\n{code}\n{start}\n{end}\n");
            let id = crate::hash::sha256_hex(hashed.as_bytes());
            items.push(format!(
                "{{\"id\":\"diag:{id}\",\"severity\":\"{severity}\",\"code\":\"{code}\",\"message\":{},\"source_id\":{},\"module\":{module},\"file\":{file},\"range\":{range},\"expected\":null,\"actual\":null,\"related\":[],\"fixes\":[{}]}}",
                json_string(&d.message),
                json_string(source_id),
                fixes.join(",")
            ));
        }
        format!("[{}]\n", items.join(","))
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

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
