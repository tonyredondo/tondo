#![no_main]

use std::panic::{AssertUnwindSafe, catch_unwind};

use libfuzzer_sys::fuzz_target;

#[allow(dead_code)]
#[path = "../../crates/tondo-reliability/src/sync_model.rs"]
mod sync_model;

use sync_model::{SyncFuzzSummary, run_fuzz_case};

fn observe(input: &[u8]) -> SyncFuzzSummary {
    let run = || run_fuzz_case(input);
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| panic!("std.sync model panicked"))
        .unwrap_or_else(|error| panic!("std.sync model invariant failed: {error}"))
}

fuzz_target!(|input: &[u8]| {
    let first = observe(input);
    let second = observe(input);
    assert_eq!(first, second, "std.sync model replay diverged");
    assert_eq!(first.pending_waiters, 0, "std.sync teardown retained a waiter");
});
