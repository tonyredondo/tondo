use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::source::{FileId, SourceDatabase, SourceError, SourceId, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticError {
    InvalidCode(String),
    EmptyMessage,
    MessageContainsLineFeed,
    EmptyFixTitle,
    EmptyFix,
    OverlappingEdits { file: FileId },
    Source(SourceError),
    Json(String),
}

impl fmt::Display for DiagnosticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCode(code) => write!(formatter, "invalid diagnostic code `{code}`"),
            Self::EmptyMessage => formatter.write_str("diagnostic message cannot be empty"),
            Self::MessageContainsLineFeed => {
                formatter.write_str("diagnostic message cannot contain a line feed")
            }
            Self::EmptyFixTitle => formatter.write_str("fix title cannot be empty"),
            Self::EmptyFix => formatter.write_str("a fix must contain at least one edit"),
            Self::OverlappingEdits { file } => {
                write!(
                    formatter,
                    "fix contains overlapping edits for source file {file}"
                )
            }
            Self::Source(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "cannot serialize diagnostic JSON: {error}"),
        }
    }
}

impl Error for DiagnosticError {}

impl From<SourceError> for DiagnosticError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiagnosticCode(String);

impl DiagnosticCode {
    pub fn new(value: impl Into<String>) -> Result<Self, DiagnosticError> {
        let value = value.into();
        let bytes = value.as_bytes();
        if bytes.len() != 5
            || !bytes[0].is_ascii_uppercase()
            || !bytes[1..].iter().all(u8::is_ascii_digit)
        {
            return Err(DiagnosticError::InvalidCode(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => formatter.write_str("error"),
            Self::Warning => formatter.write_str("warning"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimaryLocation {
    Source(Span),
    Target(SourceId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    message: String,
    span: Span,
}

impl Related {
    pub fn new(message: impl Into<String>, span: Span) -> Result<Self, DiagnosticError> {
        let message = checked_single_line(message.into())?;
        Ok(Self { message, span })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    span: Span,
    replacement: String,
}

impl TextEdit {
    pub fn new(span: Span, replacement: impl Into<String>) -> Self {
        Self {
            span,
            replacement: replacement.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Applicability {
    Safe,
    RequiresDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    title: String,
    applicability: Applicability,
    edits: Vec<TextEdit>,
}

impl Fix {
    pub fn new(
        title: impl Into<String>,
        applicability: Applicability,
        mut edits: Vec<TextEdit>,
    ) -> Result<Self, DiagnosticError> {
        let title = title.into();
        if title.is_empty() {
            return Err(DiagnosticError::EmptyFixTitle);
        }
        if edits.is_empty() {
            return Err(DiagnosticError::EmptyFix);
        }
        edits.sort_by(|left, right| edit_key(left).cmp(&edit_key(right)));
        for pair in edits.windows(2) {
            let left = &pair[0];
            let right = &pair[1];
            if left.span.file() == right.span.file()
                && left.span.range().end() > right.span.range().start()
            {
                return Err(DiagnosticError::OverlappingEdits {
                    file: left.span.file(),
                });
            }
        }
        Ok(Self {
            title,
            applicability,
            edits,
        })
    }
}

fn edit_key(edit: &TextEdit) -> (FileId, u32, u32, &str) {
    (
        edit.span.file(),
        edit.span.range().start(),
        edit.span.range().end(),
        &edit.replacement,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    severity: Severity,
    code: DiagnosticCode,
    message: String,
    location: PrimaryLocation,
    expected: Option<String>,
    actual: Option<String>,
    related: Vec<Related>,
    fixes: Vec<Fix>,
}

impl Diagnostic {
    pub fn new(
        severity: Severity,
        code: DiagnosticCode,
        message: impl Into<String>,
        location: PrimaryLocation,
    ) -> Result<Self, DiagnosticError> {
        Ok(Self {
            severity,
            code,
            message: checked_single_line(message.into())?,
            location,
            expected: None,
            actual: None,
            related: Vec::new(),
            fixes: Vec::new(),
        })
    }

    pub fn with_expected_actual(
        mut self,
        expected: Option<String>,
        actual: Option<String>,
    ) -> Self {
        self.expected = expected;
        self.actual = actual;
        self
    }

    pub fn with_related(mut self, related: Related) -> Self {
        self.related.push(related);
        self
    }

    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fixes.push(fix);
        self
    }

    pub fn code(&self) -> &DiagnosticCode {
        &self.code
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn location(&self) -> &PrimaryLocation {
        &self.location
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn expected(&self) -> Option<&str> {
        self.expected.as_deref()
    }

    pub fn actual(&self) -> Option<&str> {
        self.actual.as_deref()
    }
}

fn checked_single_line(value: String) -> Result<String, DiagnosticError> {
    if value.is_empty() {
        return Err(DiagnosticError::EmptyMessage);
    }
    if value.contains('\n') {
        return Err(DiagnosticError::MessageContainsLineFeed);
    }
    Ok(value)
}

#[derive(Debug, Default)]
pub struct DiagnosticBag {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticBag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn resolve(
        self,
        edition: &str,
        sources: &SourceDatabase,
    ) -> Result<DiagnosticReport, DiagnosticError> {
        let diagnostics = self
            .diagnostics
            .into_iter()
            .map(|diagnostic| resolve_diagnostic(diagnostic, edition, sources))
            .collect::<Result<Vec<_>, _>>()?;

        let mut by_id: BTreeMap<String, RenderedDiagnostic> = BTreeMap::new();
        for mut diagnostic in diagnostics {
            normalize_children(&mut diagnostic);
            if let Some(previous) = by_id.get_mut(&diagnostic.id) {
                if compare_diagnostics(&diagnostic, previous).is_lt() {
                    std::mem::swap(previous, &mut diagnostic);
                }
                previous.related.extend(diagnostic.related);
                previous.fixes.extend(diagnostic.fixes);
                normalize_children(previous);
                continue;
            }
            by_id.insert(diagnostic.id.clone(), diagnostic);
        }
        let mut merged = by_id.into_values().collect::<Vec<_>>();
        merged.sort_by(compare_diagnostics);

        Ok(DiagnosticReport {
            diagnostics: merged,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticReport {
    diagnostics: Vec<RenderedDiagnostic>,
}

impl DiagnosticReport {
    pub fn diagnostics(&self) -> &[RenderedDiagnostic] {
        &self.diagnostics
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn json_lines(&self) -> Result<String, DiagnosticError> {
        let mut output = String::new();
        for diagnostic in &self.diagnostics {
            let line = serde_json::to_string(diagnostic)
                .map_err(|error| DiagnosticError::Json(error.to_string()))?;
            output.push_str(&line);
            output.push('\n');
        }
        Ok(output)
    }

    pub fn human(&self) -> String {
        let mut output = String::new();
        for diagnostic in &self.diagnostics {
            let _ = writeln!(
                output,
                "{}[{}]: {}",
                diagnostic.severity, diagnostic.code, diagnostic.message
            );
            match (&diagnostic.file, &diagnostic.range) {
                (Some(file), Some(range)) => {
                    let start = &range.start;
                    if let (Some(line), Some(column)) = (start.line, start.column) {
                        let _ = writeln!(output, " --> {file}:{}:{}", line + 1, column + 1);
                    } else {
                        let _ = writeln!(output, " --> {file}:byte {}", start.byte);
                    }
                }
                _ => {
                    let _ = writeln!(output, " --> target {}", diagnostic.source_id);
                }
            }
        }
        output
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedDiagnostic {
    id: String,
    severity: Severity,
    code: String,
    message: String,
    source_id: String,
    module: Option<String>,
    file: Option<String>,
    range: Option<RenderedRange>,
    expected: Option<String>,
    actual: Option<String>,
    related: Vec<RenderedRelated>,
    fixes: Vec<RenderedFix>,
}

impl RenderedDiagnostic {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct RenderedPosition {
    byte: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct RenderedRange {
    start: RenderedPosition,
    end: RenderedPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct RenderedRelated {
    message: String,
    source_id: String,
    module: String,
    file: String,
    range: RenderedRange,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct RenderedEdit {
    source_id: String,
    module: String,
    file: String,
    range: RenderedRange,
    replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct RenderedFix {
    title: String,
    applicability: Applicability,
    edits: Vec<RenderedEdit>,
}

#[derive(Debug)]
struct ResolvedLocation {
    source_id: String,
    module: Option<String>,
    file: Option<String>,
    range: Option<RenderedRange>,
    start: Option<u32>,
    end: Option<u32>,
}

fn resolve_diagnostic(
    diagnostic: Diagnostic,
    edition: &str,
    sources: &SourceDatabase,
) -> Result<RenderedDiagnostic, DiagnosticError> {
    let location = resolve_primary(&diagnostic.location, sources)?;
    let id = diagnostic_id(edition, &location, &diagnostic.code);
    let related = diagnostic
        .related
        .into_iter()
        .map(|related| resolve_related(related, sources))
        .collect::<Result<Vec<_>, _>>()?;
    let fixes = diagnostic
        .fixes
        .into_iter()
        .map(|fix| resolve_fix(fix, sources))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(RenderedDiagnostic {
        id,
        severity: diagnostic.severity,
        code: diagnostic.code.to_string(),
        message: diagnostic.message,
        source_id: location.source_id,
        module: location.module,
        file: location.file,
        range: location.range,
        expected: diagnostic.expected,
        actual: diagnostic.actual,
        related,
        fixes,
    })
}

fn resolve_primary(
    location: &PrimaryLocation,
    sources: &SourceDatabase,
) -> Result<ResolvedLocation, DiagnosticError> {
    match location {
        PrimaryLocation::Source(span) => {
            let file = sources.get(span.file())?;
            let range = resolve_range(*span, sources)?;
            Ok(ResolvedLocation {
                source_id: file.source_id().to_string(),
                module: Some(file.module().to_string()),
                file: Some(file.path().to_string()),
                range: Some(range),
                start: Some(span.range().start()),
                end: Some(span.range().end()),
            })
        }
        PrimaryLocation::Target(source_id) => Ok(ResolvedLocation {
            source_id: source_id.to_string(),
            module: None,
            file: None,
            range: None,
            start: None,
            end: None,
        }),
    }
}

fn resolve_related(
    related: Related,
    sources: &SourceDatabase,
) -> Result<RenderedRelated, DiagnosticError> {
    let file = sources.get(related.span.file())?;
    Ok(RenderedRelated {
        message: related.message,
        source_id: file.source_id().to_string(),
        module: file.module().to_string(),
        file: file.path().to_string(),
        range: resolve_range(related.span, sources)?,
    })
}

fn resolve_fix(fix: Fix, sources: &SourceDatabase) -> Result<RenderedFix, DiagnosticError> {
    let edits = fix
        .edits
        .into_iter()
        .map(|edit| {
            let file = sources.get(edit.span.file())?;
            Ok(RenderedEdit {
                source_id: file.source_id().to_string(),
                module: file.module().to_string(),
                file: file.path().to_string(),
                range: resolve_range(edit.span, sources)?,
                replacement: edit.replacement,
            })
        })
        .collect::<Result<Vec<_>, DiagnosticError>>()?;
    Ok(RenderedFix {
        title: fix.title,
        applicability: fix.applicability,
        edits,
    })
}

fn resolve_range(span: Span, sources: &SourceDatabase) -> Result<RenderedRange, DiagnosticError> {
    let file = sources.get(span.file())?;
    let start = file.position(span.range().start())?;
    let end = file.position(span.range().end())?;
    Ok(RenderedRange {
        start: RenderedPosition {
            byte: start.byte(),
            line: start.line(),
            column: start.column(),
        },
        end: RenderedPosition {
            byte: end.byte(),
            line: end.line(),
            column: end.column(),
        },
    })
}

fn diagnostic_id(edition: &str, location: &ResolvedLocation, code: &DiagnosticCode) -> String {
    let input = format!(
        "{edition}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        location.source_id,
        location.module.as_deref().unwrap_or_default(),
        location.file.as_deref().unwrap_or_default(),
        code.as_str(),
        location
            .start
            .map(|value| value.to_string())
            .unwrap_or_default(),
        location
            .end
            .map(|value| value.to_string())
            .unwrap_or_default(),
    );
    let digest = Sha256::digest(input.as_bytes());
    let mut id = String::with_capacity(5 + digest.len() * 2);
    id.push_str("diag:");
    for byte in digest {
        write!(id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    id
}

fn compare_diagnostics(left: &RenderedDiagnostic, right: &RenderedDiagnostic) -> Ordering {
    diagnostic_key(left).cmp(&diagnostic_key(right))
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DiagnosticSortKey<'a> {
    source_id: &'a str,
    module: Option<&'a str>,
    file: Option<&'a str>,
    start: Option<u32>,
    end: Option<u32>,
    severity: Severity,
    code: &'a str,
    message: &'a str,
}

fn diagnostic_key(diagnostic: &RenderedDiagnostic) -> DiagnosticSortKey<'_> {
    DiagnosticSortKey {
        source_id: &diagnostic.source_id,
        module: diagnostic.module.as_deref(),
        file: diagnostic.file.as_deref(),
        start: diagnostic.range.as_ref().map(|range| range.start.byte),
        end: diagnostic.range.as_ref().map(|range| range.end.byte),
        severity: diagnostic.severity,
        code: &diagnostic.code,
        message: &diagnostic.message,
    }
}

fn normalize_children(diagnostic: &mut RenderedDiagnostic) {
    diagnostic.related.sort_by(|left, right| {
        (
            &left.source_id,
            &left.module,
            &left.file,
            &left.range,
            &left.message,
        )
            .cmp(&(
                &right.source_id,
                &right.module,
                &right.file,
                &right.range,
                &right.message,
            ))
    });
    diagnostic.related.dedup();
    for fix in &mut diagnostic.fixes {
        fix.edits.sort();
        fix.edits.dedup();
    }
    diagnostic.fixes.sort_by(|left, right| {
        (&left.applicability, &left.title, &left.edits).cmp(&(
            &right.applicability,
            &right.title,
            &right.edits,
        ))
    });
    diagnostic.fixes.dedup();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::Value;

    use super::*;
    use crate::source::{LogicalPath, ModulePath, SourceInput, TextRange};

    fn add_source(
        sources: &mut SourceDatabase,
        source_id: &str,
        module: &str,
        path: &str,
        bytes: impl Into<Arc<[u8]>>,
    ) -> FileId {
        sources
            .add(SourceInput::virtual_file(
                SourceId::new(source_id).unwrap(),
                ModulePath::new(module).unwrap(),
                LogicalPath::new(path).unwrap(),
                bytes,
            ))
            .unwrap()
    }

    #[test]
    fn diagnostic_id_matches_the_normative_example() {
        let mut sources = SourceDatabase::new();
        let file = add_source(
            &mut sources,
            "pkg:example/app",
            "app",
            "src/main.to",
            vec![b' '; 400],
        );
        let span = sources
            .span(file, TextRange::new(318, 323).unwrap())
            .unwrap();
        let diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E1102").unwrap(),
            "expected Int, found Int32",
            PrimaryLocation::Source(span),
        )
        .unwrap()
        .with_expected_actual(Some("Int".into()), Some("Int32".into()));
        let mut bag = DiagnosticBag::new();
        bag.push(diagnostic);

        let report = bag.resolve("0.1", &sources).unwrap();
        assert_eq!(
            report.diagnostics()[0].id(),
            "diag:657cc6f1f65d18bda1f1c6e81b157a903c56abfccf55086e3884e23ac14b9da4"
        );
    }

    #[test]
    fn json_contains_the_exact_primary_keys() {
        let sources = SourceDatabase::new();
        let diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("T0001").unwrap(),
            "bootstrap phase is not implemented",
            PrimaryLocation::Target(SourceId::new("target:vm-hosted").unwrap()),
        )
        .unwrap();
        let mut bag = DiagnosticBag::new();
        bag.push(diagnostic);

        let report = bag.resolve("0.1", &sources).unwrap();
        let value: Value = serde_json::from_str(report.json_lines().unwrap().trim()).unwrap();
        let mut keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "actual",
                "code",
                "expected",
                "file",
                "fixes",
                "id",
                "message",
                "module",
                "range",
                "related",
                "severity",
                "source_id",
            ]
        );
        assert!(value["module"].is_null());
        assert!(value["file"].is_null());
        assert!(value["range"].is_null());
    }

    #[test]
    fn fixes_reject_overlapping_edits() {
        let mut sources = SourceDatabase::new();
        let file = add_source(
            &mut sources,
            "root:test",
            "app",
            "main.to",
            "abcdef".as_bytes(),
        );
        let first = sources.span(file, TextRange::new(1, 4).unwrap()).unwrap();
        let second = sources.span(file, TextRange::new(3, 5).unwrap()).unwrap();

        assert!(
            Fix::new(
                "replace text",
                Applicability::Safe,
                vec![TextEdit::new(first, "x"), TextEdit::new(second, "y")],
            )
            .is_err()
        );
    }

    #[test]
    fn duplicate_diagnostics_merge_related_information() {
        let mut sources = SourceDatabase::new();
        let file = add_source(
            &mut sources,
            "root:test",
            "app",
            "main.to",
            "value".as_bytes(),
        );
        let span = sources.span(file, TextRange::new(0, 5).unwrap()).unwrap();
        let related = Related::new("declared here", span).unwrap();
        let make_diagnostic = || {
            Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E1001").unwrap(),
                "unknown name",
                PrimaryLocation::Source(span),
            )
            .unwrap()
            .with_related(related.clone())
        };
        let mut bag = DiagnosticBag::new();
        bag.push(make_diagnostic());
        bag.push(make_diagnostic());

        let report = bag.resolve("0.1", &sources).unwrap();
        assert_eq!(report.diagnostics().len(), 1);
        assert_eq!(report.diagnostics()[0].related.len(), 1);
    }

    #[test]
    fn related_locations_and_fixes_follow_normative_order() {
        let mut sources = SourceDatabase::new();
        let later_file = add_source(&mut sources, "root:z", "app", "z.to", "value".as_bytes());
        let earlier_file = add_source(&mut sources, "root:a", "app", "a.to", "value".as_bytes());
        let later = sources
            .span(later_file, TextRange::new(0, 1).unwrap())
            .unwrap();
        let earlier = sources
            .span(earlier_file, TextRange::new(0, 1).unwrap())
            .unwrap();
        let diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E1001").unwrap(),
            "unknown name",
            PrimaryLocation::Source(later),
        )
        .unwrap()
        .with_related(Related::new("first message", later).unwrap())
        .with_related(Related::new("later message", earlier).unwrap())
        .with_fix(
            Fix::new(
                "A title",
                Applicability::RequiresDecision,
                vec![TextEdit::new(later, "a")],
            )
            .unwrap(),
        )
        .with_fix(
            Fix::new(
                "Z title",
                Applicability::Safe,
                vec![TextEdit::new(later, "z")],
            )
            .unwrap(),
        );
        let mut bag = DiagnosticBag::new();
        bag.push(diagnostic);

        let report = bag.resolve("0.1", &sources).unwrap();
        let value: Value = serde_json::from_str(report.json_lines().unwrap().trim()).unwrap();
        assert_eq!(value["related"][0]["source_id"], "root:a");
        assert_eq!(value["fixes"][0]["applicability"], "safe");
        assert_eq!(value["fixes"][0]["title"], "Z title");
    }
}
