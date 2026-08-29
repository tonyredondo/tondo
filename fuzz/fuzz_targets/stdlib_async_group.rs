#![no_main]

use std::panic::{AssertUnwindSafe, catch_unwind};

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/tondo-reliability/src/group_model.rs"]
mod group_model;

use group_model::{FuzzSummary, run_fuzz_case};

fn observe(input: &[u8]) -> FuzzSummary {
    let run = || run_fuzz_case(input);
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| panic!("std.async.Group model panicked"))
        .unwrap_or_else(|error| panic!("std.async.Group model invariant failed: {error}"))
}

fuzz_target!(|input: &[u8]| {
    let first = observe(input);
    let second = observe(input);
    assert_eq!(first, second, "Group model replay diverged");
    assert!(first.snapshot.consumed, "fuzz run leaked its group owner");
    assert_eq!(
        first.snapshot.cleanup_runs,
        first.snapshot.consumed_children,
        "fuzz run duplicated or skipped child cleanup"
    );
    assert_eq!(first.snapshot.pending_children, 0);
});
