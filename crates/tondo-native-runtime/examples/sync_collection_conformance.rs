//! Fresh-process conformance probe for the private native std.sync collection
//! ABI.  Each case uses the same observable line as the hosted fixture while
//! assertions inspect the opaque results, ordering, generations and cleanup.

const RESULT_NONE: u64 = 0;
const RESULT_SOME: u64 = 1;
const RESULT_OK: u64 = 2;
const RESULT_ERR: u64 = 3;
const RESULT_CAS_EXCHANGED: u64 = 4;
const RESULT_CAS_MISMATCH: u64 = 5;

const STATUS_OK: u64 = 0;
const STATUS_INVALID_HANDLE: u64 = 1;
const STATUS_HOST_LIMIT: u64 = 14;
const STATUS_ATOMIC_MISMATCH: u64 = 17;
const STATUS_COLLECTION_INVALID: u64 = 19;

fn require(condition: bool, message: &str) {
    assert!(condition, "std.sync collection conformance: {message}");
}

fn release(value: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_release(value) == STATUS_OK,
        message,
    );
}

fn result_ok(value: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_result_tag(value) == RESULT_OK,
        message,
    );
    release(value, "release successful collection result");
}

fn result_ok_previous(value: u64, expected: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_result_tag(value) == RESULT_OK
            && tondo_native_runtime::tondo_rt_result_payload(value) == expected,
        message,
    );
    release(value, "release replacing collection result");
}

fn result_some(value: u64, expected: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_result_tag(value) == RESULT_SOME
            && tondo_native_runtime::tondo_rt_result_payload(value) == expected,
        message,
    );
    release(value, "release present collection value");
}

fn result_none(value: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_result_tag(value) == RESULT_NONE,
        message,
    );
    release(value, "release empty collection result");
}

fn result_error(value: u64, expected_status: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_result_tag(value) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_last_status() == expected_status,
        message,
    );
    release(value, "release failed collection result");
}

fn cas(value: u64, tag: u64, observed: u64, status: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_result_tag(value) == tag
            && tondo_native_runtime::tondo_rt_result_payload(value) == observed
            && tondo_native_runtime::tondo_rt_last_status() == status,
        message,
    );
    release(value, "release collection compare-exchange result");
}

fn array_value(array: u64, index: u64, expected: u64, message: &str) {
    result_some(
        tondo_native_runtime::tondo_rt_sync_array_get(array, index),
        expected,
        message,
    );
}

fn cursor_value(cursor: u64, expected: u64, message: &str) {
    result_some(
        tondo_native_runtime::tondo_rt_sync_cursor_next(cursor),
        expected,
        message,
    );
}

fn cursor_end(cursor: u64, message: &str) {
    result_none(
        tondo_native_runtime::tondo_rt_sync_cursor_next(cursor),
        message,
    );
}

fn shared_alias(value: u64) -> u64 {
    require(
        tondo_native_runtime::tondo_rt_mark_shared(value) == STATUS_OK
            && tondo_native_runtime::tondo_rt_retain(value) == STATUS_OK,
        "mark and retain shared collection",
    );
    let alias = tondo_native_runtime::tondo_rt_cow_clone(value);
    require(
        alias != 0 && alias != value,
        "clone shared collection identity",
    );
    alias
}

fn array_alias_bounds() {
    tondo_native_runtime::tondo_rt_reset();
    let array = tondo_native_runtime::tondo_rt_sync_array_new(3);
    require(array != 0, "create array");
    for (index, value) in [(0, 1), (1, 2), (2, 3)] {
        result_ok_previous(
            tondo_native_runtime::tondo_rt_sync_array_set(array, index, value),
            0,
            "initialize array slot",
        );
    }
    let alias = shared_alias(array);
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_array_set(alias, 1, 4),
        2,
        "alias updates shared array",
    );
    array_value(array, 1, 4, "array alias observes update");
    cas(
        tondo_native_runtime::tondo_rt_sync_array_compare_exchange(array, 1, 1, 9),
        RESULT_CAS_MISMATCH,
        4,
        STATUS_ATOMIC_MISMATCH,
        "array mismatch keeps value",
    );
    cas(
        tondo_native_runtime::tondo_rt_sync_array_compare_exchange(array, 1, 4, 5),
        RESULT_CAS_EXCHANGED,
        4,
        STATUS_OK,
        "array compare-exchange succeeds",
    );
    array_value(array, 1, 5, "array exchange value");
    result_none(
        tondo_native_runtime::tondo_rt_sync_array_get(array, 99),
        "array out-of-range get is none",
    );
    result_error(
        tondo_native_runtime::tondo_rt_sync_array_set(array, 99, 1),
        STATUS_COLLECTION_INVALID,
        "array out-of-range set is recoverable",
    );
    let snapshot = tondo_native_runtime::tondo_rt_sync_array_snapshot(array);
    require(snapshot != 0, "array snapshot exists");
    array_value(snapshot, 0, 1, "array snapshot first value");
    array_value(snapshot, 1, 5, "array snapshot second value");
    array_value(snapshot, 2, 3, "array snapshot third value");
    release(snapshot, "release array snapshot");
    release(alias, "release array alias");
    release(array, "release retained array edge");
    release(array, "release array owner");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "array case cleanup",
    );
    println!(
        r#"{{"id":"array-alias-bounds","status":"passed","line":"array-alias-bounds:3:9","cleanup":true}}"#
    );
}

fn map_linearization() {
    tondo_native_runtime::tondo_rt_reset();
    let map = tondo_native_runtime::tondo_rt_sync_map_new();
    require(map != 0, "create map");
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(map, 1, 1),
        "insert first map key",
    );
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(map, 2, 2),
        "insert second map key",
    );
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_map_insert(map, 1, 3),
        1,
        "replace map value in place",
    );
    result_some(
        tondo_native_runtime::tondo_rt_sync_map_remove(map, 1),
        3,
        "remove map value",
    );
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(map, 1, 4),
        "reinsert map key at end",
    );
    cas(
        tondo_native_runtime::tondo_rt_sync_map_compare_exchange(map, 1, 3, 1, 6, 1),
        RESULT_CAS_MISMATCH,
        4,
        STATUS_ATOMIC_MISMATCH,
        "map mismatch keeps value",
    );
    cas(
        tondo_native_runtime::tondo_rt_sync_map_compare_exchange(map, 1, 4, 1, 5, 1),
        RESULT_CAS_EXCHANGED,
        4,
        STATUS_OK,
        "map compare-exchange succeeds",
    );
    require(
        tondo_native_runtime::tondo_rt_sync_map_length(map) == 2
            && tondo_native_runtime::tondo_rt_sync_map_contains(map, 2) == 1,
        "map length and membership",
    );
    let cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(map);
    require(cursor != 0, "start map cursor");
    cursor_value(cursor, 2, "map cursor first value");
    require(
        tondo_native_runtime::tondo_rt_sync_cursor_key(cursor) == 2,
        "map cursor first key",
    );
    cursor_value(cursor, 5, "map cursor second value");
    require(
        tondo_native_runtime::tondo_rt_sync_cursor_key(cursor) == 1,
        "map cursor second key",
    );
    cursor_end(cursor, "map cursor finite end");
    release(cursor, "release map cursor");
    let snapshot = tondo_native_runtime::tondo_rt_sync_map_snapshot(map);
    require(snapshot != 0, "map snapshot exists");
    let snapshot_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(snapshot);
    require(snapshot_cursor != 0, "start map snapshot cursor");
    cursor_value(snapshot_cursor, 2, "map snapshot first value");
    cursor_value(snapshot_cursor, 5, "map snapshot second value");
    cursor_end(snapshot_cursor, "map snapshot finite end");
    release(snapshot_cursor, "release map snapshot cursor");
    release(snapshot, "release map snapshot");
    release(map, "release map");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "map case cleanup",
    );
    println!(
        r#"{{"id":"map-linearization","status":"passed","line":"map-linearization:25:2","cleanup":true}}"#
    );
}

fn set_linearization() {
    tondo_native_runtime::tondo_rt_reset();
    let set = tondo_native_runtime::tondo_rt_sync_set_new();
    require(set != 0, "create set");
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_set_insert(set, 1),
        1,
        "insert first set value",
    );
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_set_insert(set, 2),
        1,
        "insert second set value",
    );
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_set_insert(set, 1),
        0,
        "duplicate set insertion is false",
    );
    require(
        tondo_native_runtime::tondo_rt_sync_set_remove(set, 1) == 1,
        "remove set value",
    );
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_set_insert(set, 1),
        1,
        "reinsert set value at end",
    );
    require(
        tondo_native_runtime::tondo_rt_sync_set_length(set) == 2
            && tondo_native_runtime::tondo_rt_sync_set_contains(set, 1) == 1,
        "set length and membership",
    );
    let cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(set);
    require(cursor != 0, "start set cursor");
    cursor_value(cursor, 2, "set cursor first value");
    cursor_value(cursor, 1, "set cursor second value");
    cursor_end(cursor, "set cursor finite end");
    release(cursor, "release set cursor");
    let snapshot = tondo_native_runtime::tondo_rt_sync_set_snapshot(set);
    require(snapshot != 0, "set snapshot exists");
    let snapshot_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(snapshot);
    require(snapshot_cursor != 0, "start set snapshot cursor");
    cursor_value(snapshot_cursor, 2, "set snapshot first value");
    cursor_value(snapshot_cursor, 1, "set snapshot second value");
    cursor_end(snapshot_cursor, "set snapshot finite end");
    release(snapshot_cursor, "release set snapshot cursor");
    release(snapshot, "release set snapshot");
    release(set, "release set");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "set case cleanup",
    );
    println!(
        r#"{{"id":"set-linearization","status":"passed","line":"set-linearization:21:2","cleanup":true}}"#
    );
}

fn stack_queue_order() {
    tondo_native_runtime::tondo_rt_reset();
    let stack = tondo_native_runtime::tondo_rt_sync_stack_new();
    require(stack != 0, "create stack");
    for value in [1, 2, 3, 4] {
        result_ok(
            tondo_native_runtime::tondo_rt_sync_stack_push(stack, value),
            "push stack value",
        );
    }
    result_some(
        tondo_native_runtime::tondo_rt_sync_stack_peek(stack),
        4,
        "stack peek is top",
    );
    result_some(
        tondo_native_runtime::tondo_rt_sync_stack_pop(stack),
        4,
        "stack pop is top",
    );
    let stack_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(stack);
    require(stack_cursor != 0, "start stack cursor");
    for value in [3, 2, 1] {
        cursor_value(stack_cursor, value, "stack cursor is top-down");
    }
    cursor_end(stack_cursor, "stack cursor finite end");
    release(stack_cursor, "release stack cursor");
    let stack_snapshot = tondo_native_runtime::tondo_rt_sync_stack_snapshot(stack);
    require(stack_snapshot != 0, "stack snapshot exists");
    let stack_snapshot_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(stack_snapshot);
    require(stack_snapshot_cursor != 0, "start stack snapshot cursor");
    for value in [3, 2, 1] {
        cursor_value(stack_snapshot_cursor, value, "stack snapshot is top-down");
    }
    cursor_end(stack_snapshot_cursor, "stack snapshot finite end");
    release(stack_snapshot_cursor, "release stack snapshot cursor");
    release(stack_snapshot, "release stack snapshot");
    require(
        tondo_native_runtime::tondo_rt_sync_stack_length(stack) == 3,
        "stack cursor and snapshot are non-destructive",
    );
    release(stack, "release stack");

    let queue = tondo_native_runtime::tondo_rt_sync_queue_new();
    require(queue != 0, "create queue");
    for value in [1, 2, 3, 4] {
        result_ok(
            tondo_native_runtime::tondo_rt_sync_queue_enqueue(queue, value),
            "enqueue queue value",
        );
    }
    result_some(
        tondo_native_runtime::tondo_rt_sync_queue_peek(queue),
        1,
        "queue peek is front",
    );
    result_some(
        tondo_native_runtime::tondo_rt_sync_queue_dequeue(queue),
        1,
        "queue dequeue is front",
    );
    let queue_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(queue);
    require(queue_cursor != 0, "start queue cursor");
    for value in [2, 3, 4] {
        cursor_value(queue_cursor, value, "queue cursor is front-first");
    }
    cursor_end(queue_cursor, "queue cursor finite end");
    release(queue_cursor, "release queue cursor");
    let queue_snapshot = tondo_native_runtime::tondo_rt_sync_queue_snapshot(queue);
    require(queue_snapshot != 0, "queue snapshot exists");
    let queue_snapshot_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(queue_snapshot);
    require(queue_snapshot_cursor != 0, "start queue snapshot cursor");
    for value in [2, 3, 4] {
        cursor_value(
            queue_snapshot_cursor,
            value,
            "queue snapshot is front-first",
        );
    }
    cursor_end(queue_snapshot_cursor, "queue snapshot finite end");
    release(queue_snapshot_cursor, "release queue snapshot cursor");
    release(queue_snapshot, "release queue snapshot");
    require(
        tondo_native_runtime::tondo_rt_sync_queue_length(queue) == 3,
        "queue cursor and snapshot are non-destructive",
    );
    release(queue, "release queue");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "stack and queue case cleanup",
    );
    println!(
        r#"{{"id":"stack-queue-order","status":"passed","line":"stack-queue-order:321:234:3","cleanup":true}}"#
    );
}

fn cursor_horizon() {
    tondo_native_runtime::tondo_rt_reset();
    let array = tondo_native_runtime::tondo_rt_sync_array_new(2);
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_array_set(array, 0, 1),
        0,
        "initialize cursor array first value",
    );
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_array_set(array, 1, 2),
        0,
        "initialize cursor array second value",
    );
    let array_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(array);
    cursor_value(array_cursor, 1, "array cursor first value");
    cursor_value(array_cursor, 2, "array cursor second value");
    cursor_end(array_cursor, "array cursor finite end");
    release(array_cursor, "release array cursor");
    release(array, "release cursor array");

    let map = tondo_native_runtime::tondo_rt_sync_map_new();
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(map, 1, 1),
        "cursor map first",
    );
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(map, 2, 2),
        "cursor map second",
    );
    let map_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(map);
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(map, 3, 3),
        "post-cursor map insert",
    );
    cursor_value(map_cursor, 1, "map cursor first value");
    cursor_value(map_cursor, 2, "map cursor second value");
    cursor_end(map_cursor, "post-cursor map insertion excluded");
    release(map_cursor, "release cursor map cursor");
    let generation_map = tondo_native_runtime::tondo_rt_sync_map_new();
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(generation_map, 1, 10),
        "generation map first",
    );
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(generation_map, 2, 20),
        "generation map second",
    );
    let generation_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(generation_map);
    result_some(
        tondo_native_runtime::tondo_rt_sync_map_remove(generation_map, 1),
        10,
        "generation map remove",
    );
    result_ok(
        tondo_native_runtime::tondo_rt_sync_map_insert(generation_map, 1, 30),
        "generation map reinsert",
    );
    cursor_value(
        generation_cursor,
        20,
        "map cursor skips reinserted generation",
    );
    require(
        tondo_native_runtime::tondo_rt_sync_cursor_key(generation_cursor) == 2,
        "generation map key",
    );
    cursor_end(generation_cursor, "generation map finite end");
    release(generation_cursor, "release generation cursor");
    release(generation_map, "release generation map");
    release(map, "release cursor map");

    let set = tondo_native_runtime::tondo_rt_sync_set_new();
    result_ok(
        tondo_native_runtime::tondo_rt_sync_set_insert(set, 1),
        "cursor set first",
    );
    result_ok(
        tondo_native_runtime::tondo_rt_sync_set_insert(set, 2),
        "cursor set second",
    );
    let set_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(set);
    result_ok(
        tondo_native_runtime::tondo_rt_sync_set_insert(set, 3),
        "post-cursor set insert",
    );
    cursor_value(set_cursor, 1, "set cursor first value");
    cursor_value(set_cursor, 2, "set cursor second value");
    cursor_end(set_cursor, "post-cursor set insertion excluded");
    release(set_cursor, "release cursor set cursor");
    release(set, "release cursor set");

    let stack = tondo_native_runtime::tondo_rt_sync_stack_new();
    result_ok(
        tondo_native_runtime::tondo_rt_sync_stack_push(stack, 1),
        "cursor stack first",
    );
    result_ok(
        tondo_native_runtime::tondo_rt_sync_stack_push(stack, 2),
        "cursor stack second",
    );
    let stack_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(stack);
    result_ok(
        tondo_native_runtime::tondo_rt_sync_stack_push(stack, 3),
        "post-cursor stack push",
    );
    result_some(
        tondo_native_runtime::tondo_rt_sync_stack_pop(stack),
        3,
        "remove post-cursor stack push",
    );
    cursor_value(stack_cursor, 2, "stack cursor first value");
    cursor_value(stack_cursor, 1, "stack cursor second value");
    cursor_end(stack_cursor, "post-cursor stack insertion excluded");
    release(stack_cursor, "release cursor stack cursor");
    release(stack, "release cursor stack");

    let queue = tondo_native_runtime::tondo_rt_sync_queue_new();
    result_ok(
        tondo_native_runtime::tondo_rt_sync_queue_enqueue(queue, 1),
        "cursor queue first",
    );
    result_ok(
        tondo_native_runtime::tondo_rt_sync_queue_enqueue(queue, 2),
        "cursor queue second",
    );
    let queue_cursor = tondo_native_runtime::tondo_rt_sync_cursor_start(queue);
    result_ok(
        tondo_native_runtime::tondo_rt_sync_queue_enqueue(queue, 3),
        "post-cursor queue enqueue",
    );
    cursor_value(queue_cursor, 1, "queue cursor first value");
    cursor_value(queue_cursor, 2, "queue cursor second value");
    cursor_end(queue_cursor, "post-cursor queue insertion excluded");
    release(queue_cursor, "release cursor queue cursor");
    release(queue, "release cursor queue");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "cursor horizon case cleanup",
    );
    println!(
        r#"{{"id":"cursor-horizon","status":"passed","line":"cursor-horizon:12:12:12:21:12","cleanup":true}}"#
    );
}

fn snapshot_equivalence() {
    tondo_native_runtime::tondo_rt_reset();
    let array = tondo_native_runtime::tondo_rt_sync_array_new(2);
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_array_set(array, 0, 4),
        0,
        "initialize snapshot first value",
    );
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_array_set(array, 1, 5),
        0,
        "initialize snapshot second value",
    );
    let snapshot = tondo_native_runtime::tondo_rt_sync_array_snapshot(array);
    require(
        snapshot != 0 && tondo_native_runtime::tondo_rt_sync_array_length(snapshot) == 2,
        "snapshot length",
    );
    array_value(snapshot, 0, 4, "snapshot first arithmetic value");
    array_value(snapshot, 1, 5, "snapshot second arithmetic value");
    release(snapshot, "release arithmetic snapshot");
    release(array, "release arithmetic array");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "snapshot case cleanup",
    );
    println!(
        r#"{{"id":"snapshot-equivalence","status":"passed","line":"snapshot-equivalence:9","cleanup":true}}"#
    );
}

fn limits_cleanup() {
    tondo_native_runtime::tondo_rt_reset();
    let wrong = tondo_native_runtime::tondo_rt_atomic_new(0);
    require(
        tondo_native_runtime::tondo_rt_sync_array_length(wrong) == u64::MAX
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_INVALID_HANDLE,
        "wrong collection kind fails closed",
    );
    release(wrong, "release wrong collection kind");
    let empty_array = tondo_native_runtime::tondo_rt_sync_array_new(0);
    let empty_stack = tondo_native_runtime::tondo_rt_sync_stack_new();
    let empty_queue = tondo_native_runtime::tondo_rt_sync_queue_new();
    require(
        tondo_native_runtime::tondo_rt_sync_array_is_empty(empty_array) == 1
            && tondo_native_runtime::tondo_rt_sync_stack_is_empty(empty_stack) == 1
            && tondo_native_runtime::tondo_rt_sync_queue_is_empty(empty_queue) == 1,
        "empty collection flags",
    );
    result_none(
        tondo_native_runtime::tondo_rt_sync_array_get(empty_array, 0),
        "empty array get is none",
    );
    result_none(
        tondo_native_runtime::tondo_rt_sync_stack_pop(empty_stack),
        "empty stack pop is none",
    );
    result_none(
        tondo_native_runtime::tondo_rt_sync_queue_dequeue(empty_queue),
        "empty queue dequeue is none",
    );
    release(empty_array, "release empty array");
    release(empty_stack, "release empty stack");
    release(empty_queue, "release empty queue");
    let oversized = tondo_native_runtime::tondo_rt_sync_array_new((1 << 20) + 1);
    require(
        oversized == 0 && tondo_native_runtime::tondo_rt_last_status() == STATUS_HOST_LIMIT,
        "oversized collection is rejected",
    );
    let stale = tondo_native_runtime::tondo_rt_sync_set_new();
    release(stale, "release stale set owner");
    require(
        tondo_native_runtime::tondo_rt_sync_set_length(stale) == u64::MAX
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_INVALID_HANDLE,
        "stale collection fails closed",
    );
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "limits case cleanup",
    );
    println!(
        r#"{{"id":"limits-cleanup","status":"passed","line":"limits-cleanup:empty-none","cleanup":true}}"#
    );
}

fn threads_capability() {
    tondo_native_runtime::tondo_rt_reset();
    let array = tondo_native_runtime::tondo_rt_sync_array_new(1);
    result_ok_previous(
        tondo_native_runtime::tondo_rt_sync_array_set(array, 0, 0),
        0,
        "initialize shared worker counter",
    );
    require(
        tondo_native_runtime::tondo_rt_mark_shared(array) == STATUS_OK,
        "mark collection for threads capability",
    );
    let workers = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                for _ in 0..10 {
                    loop {
                        let observed = tondo_native_runtime::tondo_rt_sync_array_get(array, 0);
                        require(
                            tondo_native_runtime::tondo_rt_result_tag(observed) == RESULT_SOME,
                            "worker reads shared counter",
                        );
                        let value = tondo_native_runtime::tondo_rt_result_payload(observed);
                        release(observed, "release worker observation");
                        let exchanged = tondo_native_runtime::tondo_rt_sync_array_compare_exchange(
                            array,
                            0,
                            value,
                            value + 1,
                        );
                        let tag = tondo_native_runtime::tondo_rt_result_tag(exchanged);
                        release(exchanged, "release worker compare-exchange");
                        if tag == RESULT_CAS_EXCHANGED {
                            break;
                        }
                        require(tag == RESULT_CAS_MISMATCH, "worker mismatch is retryable");
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("shared collection worker must finish");
    }
    array_value(array, 0, 40, "all native workers update one identity");
    release(array, "release threaded collection");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "threads case cleanup",
    );
    println!(
        r#"{{"id":"threads-capability","status":"passed","line":"threads-capability:shared-alias","cleanup":true,"workers":4,"increments":40}}"#
    );
}

fn main() {
    array_alias_bounds();
    map_linearization();
    set_linearization();
    stack_queue_order();
    cursor_horizon();
    snapshot_equivalence();
    limits_cleanup();
    threads_capability();
}
