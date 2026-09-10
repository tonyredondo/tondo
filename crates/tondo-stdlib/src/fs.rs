//! Portable filesystem kinds, modes and error categories. This module performs no host I/O.

/// Declaration order of the public Tondo enum, not a native ABI tag.
pub const FILE_KIND_VARIANTS: &[&str] = &["File", "Directory", "Symlink", "Other"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

impl FileKind {
    pub fn from_file_type(kind: std::fs::FileType) -> Self {
        if kind.is_file() {
            Self::File
        } else if kind.is_dir() {
            Self::Directory
        } else if kind.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        }
    }

    pub const fn variant(self) -> u32 {
        self as u32
    }
}

pub const OPEN_MODE_VARIANTS: &[&str] = &[
    "Read",
    "Write",
    "ReadWrite",
    "Append",
    "Create",
    "CreateNew",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    Read,
    Write,
    ReadWrite,
    Append,
    Create,
    CreateNew,
}

impl OpenMode {
    pub const fn from_variant(variant: u32) -> Option<Self> {
        match variant {
            0 => Some(Self::Read),
            1 => Some(Self::Write),
            2 => Some(Self::ReadWrite),
            3 => Some(Self::Append),
            4 => Some(Self::Create),
            5 => Some(Self::CreateNew),
            _ => None,
        }
    }
}

/// The declaration order of the public Tondo `std.fs.FsError` enum.
pub const ERROR_VARIANTS: &[&str] = &[
    "NotFound",
    "PermissionDenied",
    "AlreadyExists",
    "InvalidPath",
    "NotDirectory",
    "IsDirectory",
    "Closed",
    "ResourceLimit",
    "Cancelled",
    "Io",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidPath,
    NotDirectory,
    IsDirectory,
    Closed,
    ResourceLimit,
    Cancelled,
    Io,
}

impl FsError {
    /// Classify the structured host error, independent of locale or message text.
    /// Logical limits, cancellation and observable closure are supplied by the
    /// provider; unrelated OS failures retain the public `Io` category.
    pub fn from_io(error: &std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::NotFound => Self::NotFound,
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            std::io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            std::io::ErrorKind::InvalidInput => Self::InvalidPath,
            std::io::ErrorKind::NotADirectory => Self::NotDirectory,
            std::io::ErrorKind::IsADirectory => Self::IsDirectory,
            _ => Self::Io,
        }
    }

    /// Index in the compiler-owned nominal declaration, not a native ABI tag.
    pub const fn variant(self) -> u32 {
        self as u32
    }
}

#[cfg(test)]
mod tests {
    use super::{ERROR_VARIANTS, FsError};
    use std::io::{Error, ErrorKind};

    #[test]
    fn filesystem_error_categories_preserve_kind_without_parsing_messages() {
        for (kind, expected) in [
            (ErrorKind::NotFound, "NotFound"),
            (ErrorKind::PermissionDenied, "PermissionDenied"),
            (ErrorKind::AlreadyExists, "AlreadyExists"),
            (ErrorKind::InvalidInput, "InvalidPath"),
            (ErrorKind::NotADirectory, "NotDirectory"),
            (ErrorKind::IsADirectory, "IsDirectory"),
            (ErrorKind::Other, "Io"),
            (ErrorKind::Interrupted, "Io"),
        ] {
            for message in [
                "permission denied",
                "not found",
                "cancelled",
                "quota exceeded",
                "",
            ] {
                let error = FsError::from_io(&Error::new(kind, message));
                assert_eq!(ERROR_VARIANTS[error.variant() as usize], expected);
            }
        }
    }
}
