#![no_main]

use std::panic::{AssertUnwindSafe, catch_unwind};

use libfuzzer_sys::fuzz_target;

#[allow(dead_code)]
#[path = "../../crates/tondo-reliability/src/sync_collection_model.rs"]
mod sync_collection_model;

use sync_collection_model::{
    CollectionFuzzSummary, MAX_COLLECTION_FUZZ_INPUT_BYTES, run_collection_fuzz_case,
};

fn observe(input: &[u8]) -> CollectionFuzzSummary {
    let run = || run_collection_fuzz_case(input);
    catch_unwind(AssertUnwindSafe(run))
        .unwrap_or_else(|_| panic!("std.sync collection model panicked"))
        .unwrap_or_else(|error| panic!("std.sync collection model invariant failed: {error}"))
}

fuzz_target!(|input: &[u8]| {
    let first = observe(&input[..input.len().min(MAX_COLLECTION_FUZZ_INPUT_BYTES)]);
    let second = observe(&input[..input.len().min(MAX_COLLECTION_FUZZ_INPUT_BYTES)]);
    assert_eq!(first, second, "std.sync collection model replay diverged");
    assert_eq!(first.live_handles, 0, "collection teardown retained a handle");
    assert_eq!(first.live_cursors, 0, "collection teardown retained a cursor");
    assert_eq!(first.live_collections, 0, "collection teardown retained a state");
});
