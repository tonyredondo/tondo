#![doc = "Frontend and compilation pipeline for the Tondo language."]

pub mod bytecode;
pub mod diagnostics;
pub mod driver;
pub mod hir;
pub mod mir;
pub mod package;
pub mod resolve;
pub mod semantic;
pub mod source;
pub mod syntax;
pub mod types;

/// Language edition targeted by the bootstrap compiler.
pub const LANGUAGE_EDITION: &str = "0.1";
