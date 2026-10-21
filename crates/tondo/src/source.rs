//! Source files and spans.
//!
//! Every file occupies its own range of one global offset space, so a `Span`
//! is two `u32`s and still identifies its file.

use std::path::{Path, PathBuf};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Span {
        Span { start, end }
    }

    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

pub struct SourceFile {
    pub name: String,
    pub path: PathBuf,
    /// The canonical logical path and module of the file (22.6).
    pub logical: String,
    pub module: String,
    pub text: String,
    pub base: u32,
    line_starts: Vec<u32>,
}

impl SourceFile {
    /// Zero-based byte offset, line and column (in scalars) of a global
    /// offset within the file.
    pub fn position(&self, offset: u32) -> (u32, usize, usize) {
        let (line, column) = self.line_col(offset);
        (offset - self.base, line - 1, column - 1)
    }

    /// One-based line and column (in characters) of a global offset.
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let local = (offset - self.base) as usize;
        let line = self.line_starts.partition_point(|&start| start as usize <= local) - 1;
        let line_start = self.line_starts[line] as usize;
        let column = self.text[line_start..local.min(self.text.len())].chars().count();
        (line + 1, column + 1)
    }

    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line - 1] as usize;
        let end = self.line_starts.get(line).map_or(self.text.len(), |&end| end as usize);
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    pub fn slice(&self, span: Span) -> &str {
        &self.text[(span.start - self.base) as usize..(span.end - self.base) as usize]
    }
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn add(&mut self, path: &Path, text: String) -> usize {
        let base = self.files.last().map_or(0, |f| f.base + f.text.len() as u32 + 1);
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i as u32 + 1));
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into());
        let module = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into());
        let logical = name.clone();
        self.files.push(SourceFile { name, path: path.to_owned(), logical, module, text, base, line_starts });
        self.files.len() - 1
    }

    /// Sets where a file belongs within its package.
    pub fn set_identity(&mut self, index: usize, logical: String, module: String) {
        self.files[index].logical = logical;
        self.files[index].module = module;
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn file(&self, index: usize) -> &SourceFile {
        &self.files[index]
    }

    pub fn file_of(&self, offset: u32) -> &SourceFile {
        let index = self.files.partition_point(|f| f.base <= offset) - 1;
        &self.files[index]
    }

    /// `file:line:column` of the start of a span.
    pub fn location(&self, span: Span) -> String {
        let file = self.file_of(span.start);
        let (line, column) = file.line_col(span.start);
        format!("{}:{line}:{column}", file.path.display())
    }

    pub fn slice(&self, span: Span) -> &str {
        self.file_of(span.start).slice(span)
    }
}
