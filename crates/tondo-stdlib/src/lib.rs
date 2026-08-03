//! Portable kernels used by the hosted STD-0.1A bridge.
//!
//! The VM owns capabilities and resource accounting; this crate deliberately
//! contains only deterministic, allocation-bounded value transformations.

pub mod format;
pub mod io;
pub mod json;
pub mod math;
pub mod messagepack;
pub mod path;
pub mod protobuf;
pub mod serialization;
pub mod testing;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    UnexpectedEof,
    InvalidSyntax,
    InvalidUtf8,
    DuplicateKey,
    TrailingData,
    LimitExceeded,
    InvalidTag,
    InvalidLength,
    InvalidWireType,
    VarintOverflow,
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnexpectedEof => "unexpected end of input",
            Self::InvalidSyntax => "invalid syntax",
            Self::InvalidUtf8 => "invalid UTF-8",
            Self::DuplicateKey => "duplicate key",
            Self::TrailingData => "trailing data",
            Self::LimitExceeded => "codec limit exceeded",
            Self::InvalidTag => "invalid tag",
            Self::InvalidLength => "invalid length",
            Self::InvalidWireType => "invalid wire type",
            Self::VarintOverflow => "varint overflow",
        })
    }
}

impl std::error::Error for CodecError {}
