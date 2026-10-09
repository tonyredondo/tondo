#![no_main]

use libfuzzer_sys::fuzz_target;

// Share the independent model and kernel adapter without enabling the
// compiler, VM, conformance or reliability dependency graph.
#[path = "../../crates/tondo-reliability/src/log_model.rs"]
mod log_model;
#[path = "../../crates/tondo-reliability/src/log_fuzz.rs"]
mod log_fuzz;

fuzz_target!(|input: &[u8]| log_fuzz::run_case(input));
