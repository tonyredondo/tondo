#![no_main]

use std::panic::{AssertUnwindSafe, catch_unwind};

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/tondo-reliability/src/executor_model.rs"]
mod executor_model;

use executor_model::{FuzzSummary, run_fuzz_case};

fn observe(input: &[u8]) -> FuzzSummary {
    let run = || run_fuzz_case(input);
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| panic!("std.executor model panicked"))
        .unwrap_or_else(|error| panic!("std.executor model invariant failed: {error:?}"))
}

fuzz_target!(|input: &[u8]| {
    let first = observe(input);
    let second = observe(input);
    assert_eq!(first, second, "executor model replay diverged");
    assert!(first.steps <= executor_model::MAX_FUZZ_STEPS);
    assert_eq!(first.snapshot.queued, 0, "fuzz run retained queued work");
    assert_eq!(first.snapshot.running, 0, "fuzz run retained running work");
    assert!(matches!(
        first.snapshot.lifecycle,
        executor_model::Lifecycle::Closed | executor_model::Lifecycle::Cancelled
    ));
    if let Some(actor) = first.snapshot.actor {
        assert!(!actor.in_flight, "fuzz run retained an actor message");
        assert_eq!(actor.mailbox, 0, "fuzz run retained actor mailbox work");
        assert_eq!(
            actor.cleanup_runs,
            actor.processed.len() + actor.discarded,
            "actor cleanup was not exactly once"
        );
    }
});
