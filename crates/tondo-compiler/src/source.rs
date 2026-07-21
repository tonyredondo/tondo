use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, OnceLock};

use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    EmptySourceId,
    SourceIdContainsLineFeed,
    EmptyModulePath,
    InvalidModulePath(String),
    EmptyLogicalPath,
    InvalidLogicalPath(String),
    FileTooLarge(usize),
    TooManyFiles,
    DuplicateFile(String),
    UnknownFile(FileId),
    InvalidRange(TextRange),
    OffsetOutOfBounds { offset: u32, length: u32 },
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySourceId => formatter.write_str("source ID cannot be empty"),
            Self::SourceIdContainsLineFeed => {
                formatter.write_str("source ID cannot contain a line feed")
            }
            Self::EmptyModulePath => formatter.write_str("module path cannot be empty"),
            Self::InvalidModulePath(path) => write!(formatter, "invalid module path `{path}`"),
            Self::EmptyLogicalPath => formatter.write_str("logical file path cannot be empty"),
            Self::InvalidLogicalPath(path) => {
                write!(formatter, "invalid logical file path `{path}`")
            }
            Self::FileTooLarge(length) => {
                write!(
                    formatter,
                    "source file is too large to index ({length} bytes)"
                )
            }
            Self::TooManyFiles => formatter.write_str("source database contains too many files"),
            Self::DuplicateFile(path) => write!(formatter, "duplicate logical source `{path}`"),
            Self::UnknownFile(file_id) => write!(formatter, "unknown source file {file_id}"),
            Self::InvalidRange(range) => write!(
                formatter,
                "invalid byte range {}..{}",
                range.start, range.end
            ),
            Self::OffsetOutOfBounds { offset, length } => write!(
                formatter,
                "byte offset {offset} lies outside a source of {length} bytes"
            ),
        }
    }
}

impl Error for SourceError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(String);

impl SourceId {
    pub fn new(value: impl Into<String>) -> Result<Self, SourceError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SourceError::EmptySourceId);
        }
        if value.contains('\n') {
            return Err(SourceError::SourceIdContainsLineFeed);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModulePath(String);

impl ModulePath {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SourceError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(SourceError::EmptyModulePath);
        }

        let mut normalized = Vec::new();
        for component in value.split('.') {
            if component.is_empty()
                || component.contains(['/', '\\', '\n', '\r'])
                || component == "."
                || component == ".."
            {
                return Err(SourceError::InvalidModulePath(value.to_owned()));
            }
            let component = component.nfc().collect::<String>();
            let mut characters = component.chars();
            let Some(first) = characters.next() else {
                return Err(SourceError::InvalidModulePath(value.to_owned()));
            };
            if component == "_"
                || is_keyword(&component)
                || !(first == '_' || unicode_ident::is_xid_start(first))
                || !characters.all(unicode_ident::is_xid_continue)
            {
                return Err(SourceError::InvalidModulePath(value.to_owned()));
            }
            normalized.push(component);
        }

        Ok(Self(normalized.join(".")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_keyword(value: &str) -> bool {
    matches!(
        value,
        "alias"
            | "and"
            | "as"
            | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "defer"
            | "else"
            | "enum"
            | "err"
            | "fail"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "import"
            | "in"
            | "let"
            | "match"
            | "mut"
            | "none"
            | "not"
            | "ok"
            | "or"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "scope"
            | "self"
            | "some"
            | "spawn"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "var"
            | "with"
    )
}

impl fmt::Display for ModulePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogicalPath(String);

impl LogicalPath {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SourceError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(SourceError::EmptyLogicalPath);
        }
        if value.starts_with('/') || value.contains(['\\', '\n', '\r']) {
            return Err(SourceError::InvalidLogicalPath(value.to_owned()));
        }

        let mut normalized = Vec::new();
        for component in value.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                return Err(SourceError::InvalidLogicalPath(value.to_owned()));
            }
            normalized.push(component.nfc().collect::<String>());
        }

        Ok(Self(normalized.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LogicalPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(u32);

impl FileId {
    pub fn index(self) -> u32 {
        self.0
    }

    pub(crate) fn from_index(index: usize) -> Result<Self, SourceError> {
        Ok(Self(
            u32::try_from(index).map_err(|_| SourceError::TooManyFiles)?,
        ))
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextRange {
    start: u32,
    end: u32,
}

impl TextRange {
    pub fn new(start: u32, end: u32) -> Result<Self, SourceError> {
        let range = Self { start, end };
        if start > end {
            return Err(SourceError::InvalidRange(range));
        }
        Ok(range)
    }

    pub const fn empty(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    pub fn start(self) -> u32 {
        self.start
    }

    pub fn end(self) -> u32 {
        self.end
    }
}

impl fmt::Display for TextRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}..{}", self.start, self.end)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    file: FileId,
    range: TextRange,
}

impl Span {
    pub fn file(self) -> FileId {
        self.file
    }

    pub fn range(self) -> TextRange {
        self.range
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOrigin {
    Physical,
    Virtual,
}

#[derive(Debug, Clone)]
pub struct SourceInput {
    source_id: SourceId,
    module: ModulePath,
    path: LogicalPath,
    origin: SourceOrigin,
    bytes: Arc<[u8]>,
}

impl SourceInput {
    pub fn new(
        source_id: SourceId,
        module: ModulePath,
        path: LogicalPath,
        origin: SourceOrigin,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self {
        Self {
            source_id,
            module,
            path,
            origin,
            bytes: bytes.into(),
        }
    }

    pub fn virtual_file(
        source_id: SourceId,
        module: ModulePath,
        path: LogicalPath,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self {
        Self::new(source_id, module, path, SourceOrigin::Virtual, bytes)
    }
}

#[derive(Debug)]
pub struct SourceFile {
    source_id: SourceId,
    module: ModulePath,
    path: LogicalPath,
    origin: SourceOrigin,
    bytes: Arc<[u8]>,
    line_index: OnceLock<LineIndex>,
}

impl SourceFile {
    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub fn module(&self) -> &ModulePath {
        &self.module
    }

    pub fn path(&self) -> &LogicalPath {
        &self.path
    }

    pub fn origin(&self) -> SourceOrigin {
        self.origin
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn text(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.bytes)
    }

    pub fn length(&self) -> u32 {
        u32::try_from(self.bytes.len()).expect("source length was validated on insertion")
    }

    pub fn position(&self, offset: u32) -> Result<SourcePosition, SourceError> {
        if offset > self.length() {
            return Err(SourceError::OffsetOutOfBounds {
                offset,
                length: self.length(),
            });
        }
        Ok(self
            .line_index
            .get_or_init(|| LineIndex::new(&self.bytes))
            .position(&self.bytes, offset))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    byte: u32,
    line: Option<u32>,
    column: Option<u32>,
}

impl SourcePosition {
    pub fn byte(self) -> u32 {
        self.byte
    }

    pub fn line(self) -> Option<u32> {
        self.line
    }

    pub fn column(self) -> Option<u32> {
        self.column
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SourceKey {
    source_id: SourceId,
    module: ModulePath,
    path: LogicalPath,
}

#[derive(Debug, Default)]
pub struct SourceDatabase {
    files: Vec<SourceFile>,
    by_key: BTreeMap<SourceKey, FileId>,
}

impl SourceDatabase {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, input: SourceInput) -> Result<FileId, SourceError> {
        if input.bytes.len() > u32::MAX as usize {
            return Err(SourceError::FileTooLarge(input.bytes.len()));
        }
        let index = u32::try_from(self.files.len()).map_err(|_| SourceError::TooManyFiles)?;
        let key = SourceKey {
            source_id: input.source_id.clone(),
            module: input.module.clone(),
            path: input.path.clone(),
        };
        if self.by_key.contains_key(&key) {
            return Err(SourceError::DuplicateFile(input.path.to_string()));
        }

        let file_id = FileId(index);
        self.files.push(SourceFile {
            source_id: input.source_id,
            module: input.module,
            path: input.path,
            origin: input.origin,
            bytes: input.bytes,
            line_index: OnceLock::new(),
        });
        self.by_key.insert(key, file_id);
        Ok(file_id)
    }

    pub fn get(&self, file: FileId) -> Result<&SourceFile, SourceError> {
        self.files
            .get(file.0 as usize)
            .ok_or(SourceError::UnknownFile(file))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (FileId, &SourceFile)> {
        self.files.iter().enumerate().map(|(index, file)| {
            (
                FileId(u32::try_from(index).expect("source count is bounded by u32")),
                file,
            )
        })
    }

    pub fn span(&self, file: FileId, range: TextRange) -> Result<Span, SourceError> {
        let source = self.get(file)?;
        if range.end > source.length() {
            return Err(SourceError::InvalidRange(range));
        }
        Ok(Span { file, range })
    }
}

#[derive(Debug)]
struct LineIndex {
    starts: Vec<u32>,
    valid_up_to: u32,
}

impl LineIndex {
    fn new(bytes: &[u8]) -> Self {
        let mut starts = vec![0];
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                starts.push(u32::try_from(index + 1).expect("source length was validated"));
            }
        }

        let valid_up_to = match std::str::from_utf8(bytes) {
            Ok(_) => u32::try_from(bytes.len()).expect("source length was validated"),
            Err(error) => u32::try_from(error.valid_up_to()).expect("source length was validated"),
        };

        Self {
            starts,
            valid_up_to,
        }
    }

    fn position(&self, bytes: &[u8], offset: u32) -> SourcePosition {
        if offset > self.valid_up_to {
            return SourcePosition {
                byte: offset,
                line: None,
                column: None,
            };
        }

        let line_index = self.starts.partition_point(|start| *start <= offset) - 1;
        let line_start = self.starts[line_index];
        let column_bytes = &bytes[line_start as usize..offset as usize];
        let column = std::str::from_utf8(column_bytes)
            .ok()
            .and_then(|text| u32::try_from(text.chars().count()).ok());

        SourcePosition {
            byte: offset,
            line: column.map(|_| u32::try_from(line_index).expect("source length bounds lines")),
            column,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn virtual_input(path: &str, bytes: impl Into<Arc<[u8]>>) -> SourceInput {
        SourceInput::virtual_file(
            SourceId::new("root:test").unwrap(),
            ModulePath::new("app").unwrap(),
            LogicalPath::new(path).unwrap(),
            bytes,
        )
    }

    #[test]
    fn logical_paths_are_normalized_to_nfc() {
        let path = LogicalPath::new("src/cafe\u{301}.to").unwrap();
        assert_eq!(path.as_str(), "src/café.to");
    }

    #[test]
    fn logical_paths_reject_environment_dependent_components() {
        for path in ["/main.to", "./main.to", "src/../main.to", "src\\main.to"] {
            assert!(LogicalPath::new(path).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn module_paths_are_nfc_identifier_sequences_representable_by_import_syntax() {
        let path = ModulePath::new("cafe\u{301}.httpClient").unwrap();
        assert_eq!(path.as_str(), "café.httpClient");

        for path in [
            "_",
            "app._",
            "app.type",
            "app.invalid-name",
            "app.9invalid",
            "app..models",
            "app/models",
        ] {
            assert!(ModulePath::new(path).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn positions_use_bytes_lines_and_unicode_scalar_columns() {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(virtual_input("main.to", "aé\r\nβ".as_bytes()))
            .unwrap();
        let source = sources.get(file).unwrap();

        assert_eq!(
            source.position(3).unwrap(),
            SourcePosition {
                byte: 3,
                line: Some(0),
                column: Some(2),
            }
        );
        assert_eq!(
            source.position(5).unwrap(),
            SourcePosition {
                byte: 5,
                line: Some(1),
                column: Some(0),
            }
        );
    }

    #[test]
    fn positions_after_invalid_utf8_keep_only_the_byte_offset() {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(virtual_input("invalid.to", [b'a', 0xff, b'b']))
            .unwrap();
        let source = sources.get(file).unwrap();

        assert_eq!(source.position(1).unwrap().line(), Some(0));
        assert_eq!(source.position(2).unwrap().line(), None);
        assert_eq!(source.position(2).unwrap().column(), None);
    }

    #[test]
    fn duplicate_logical_sources_are_rejected() {
        let mut sources = SourceDatabase::new();
        sources
            .add(virtual_input("main.to", "first".as_bytes()))
            .unwrap();
        assert!(
            sources
                .add(virtual_input("main.to", "second".as_bytes()))
                .is_err()
        );
    }
}
