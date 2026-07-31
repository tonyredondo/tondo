#![doc = "Frontend and compilation pipeline for the Tondo language."]

pub mod artifact;
pub mod bytecode;
pub mod diagnostics;
pub mod driver;
pub mod hir;
pub mod mir;
pub mod package;
mod process_host;
pub mod project;
pub mod resolve;
pub mod semantic;
pub mod source;
pub mod syntax;
pub mod test_capture;
pub mod test_check;
pub mod test_dependencies;
pub mod test_discovery;
pub mod test_inputs;
pub mod test_integration;
pub mod test_overlay;
pub mod test_owners;
pub mod test_plan;
pub mod test_result;
pub mod test_tree;
pub mod toolchain;
pub mod types;

/// Language edition targeted by the bootstrap compiler.
pub const LANGUAGE_EDITION: &str = "0.1";
