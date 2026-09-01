#![no_main]

use std::panic::{AssertUnwindSafe, catch_unwind};

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/tondo-reliability/src/channel_model.rs"]
mod channel_model;

use channel_model::{FuzzSummary, MAX_CHANNEL_FUZZ_INPUT_BYTES, run_fuzz_case};

fn observe(input: &[u8]) -> FuzzSummary {
    let run = || run_fuzz_case(input);
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| panic!("std.channel model panicked"))
        .unwrap_or_else(|error| panic!("std.channel model invariant failed: {error}"))
}

fuzz_target!(|input: &[u8]| {
    let bounded = &input[..input.len().min(MAX_CHANNEL_FUZZ_INPUT_BYTES)];
    let first = observe(bounded);
    let second = observe(bounded);
    assert_eq!(first, second, "std.channel model replay diverged");
    assert_eq!(first.snapshot.sender_count, 0);
    assert_eq!(first.snapshot.receiver_count, 0);
    assert!(first.snapshot.queue.is_empty());
    assert!(first.snapshot.send_waiters.is_empty());
    assert!(first.snapshot.receive_waiters.is_empty());
    assert!(first.snapshot.send_results.is_empty());
    assert!(first.snapshot.receive_results.is_empty());
});
