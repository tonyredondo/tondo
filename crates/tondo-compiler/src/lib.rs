#![doc = "Frontend and compilation pipeline for the Tondo language."]

pub mod artifact;
pub mod bytecode;
pub mod diagnostics;
pub mod driver;
pub mod hir;
pub mod meta;
pub mod meta_derive;
pub mod meta_generate;
#[cfg(test)]
mod meta_test_support;
pub mod meta_vm;
pub mod mir;
pub mod package;
mod process_host;
pub mod project;
pub mod resolve;
pub mod semantic;
pub mod source;
pub mod std_meta;
pub mod syntax;
pub mod test_artifacts;
pub mod test_backend;
pub mod test_capture;
pub mod test_check;
pub mod test_control;
pub mod test_dependencies;
pub mod test_discovery;
pub mod test_glob;
pub mod test_input_runtime;
pub mod test_inputs;
pub mod test_integration;
pub mod test_interrupt;
pub mod test_junit;
pub mod test_limits;
pub mod test_lower;
pub mod test_overlay;
pub mod test_owners;
pub mod test_plan;
pub mod test_repeat;
pub mod test_report;
pub mod test_result;
pub mod test_retry;
pub mod test_runtime;
pub mod test_schedule;
pub mod test_shard;
pub mod test_snapshots;
pub mod test_suite;
pub mod test_tree;
pub mod test_virtual_time;
pub mod toolchain;
pub mod types;

/// Language edition targeted by the bootstrap compiler.
pub const LANGUAGE_EDITION: &str = "0.1";
