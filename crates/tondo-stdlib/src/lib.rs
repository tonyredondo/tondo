//! Portable kernels used by the hosted STD-0.1A bridge.
//!
//! The VM owns capabilities and resource accounting; this crate deliberately
//! contains only deterministic, allocation-bounded value transformations.

pub mod json;
pub mod messagepack;
pub mod protobuf;

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
