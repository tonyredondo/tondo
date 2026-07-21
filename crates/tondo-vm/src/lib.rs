#![doc = "Verified bytecode runtime for the Tondo language."]

pub mod bytecode;
pub mod runtime;

/// Human-readable identifier for the initial execution backend.
pub const BACKEND_NAME: &str = "bytecode-vm";
