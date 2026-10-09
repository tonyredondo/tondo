//! Independent bounded logging values and queue transitions.
//!
//! The reference has no production, host, clock or filesystem imports.
//! OutsideDomain is a reference refusal, never a Tondo ResourceLimit claim.

#[path = "log_model/queue.rs"]
pub mod queue;
#[path = "log_model/values.rs"]
pub mod values;

pub const MAX_FUZZ_INPUT_BYTES: usize = 4096;
pub const MAX_FUZZ_STEPS: usize = queue::MAX_STEPS;
