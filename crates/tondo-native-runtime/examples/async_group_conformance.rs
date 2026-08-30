//! Small, fresh-process conformance probe for the native std.async.Group
//! runtime ABI. It intentionally uses only the opaque C-facing functions so
//! the report exercises the same boundary a native lowering will consume.

const STATUS_OK: u64 = 0;
const STATUS_INVALID_HANDLE: u64 = 1;
const STATUS_INVALID_TRANSITION: u64 = 3;
const STATUS_NOT_READY: u64 = 6;
const STATUS_PANICKED: u64 = 16;
const RESULT_OK: u64 = 2;
const RESULT_ERR: u64 = 3;

fn require(condition: bool, message: &str) {
    assert!(condition, "native Group conformance: {message}");
}

fn main() {
    all_order();
    settle_mixed();
    all_error_priority();
    next_completion_order();
    panic_drains();
    cancel_drains();
    empty_operations();
    invalid_add();
}

fn all_order() {
    tondo_native_runtime::tondo_rt_reset();
    let group = tondo_native_runtime::tondo_rt_group_new();
    let first = tondo_native_runtime::tondo_rt_task_spawn(1, 0);
    let second = tondo_native_runtime::tondo_rt_task_spawn(2, 0);
    require(
        tondo_native_runtime::tondo_rt_group_add(group, first) == STATUS_OK,
        "all-order first add",
    );
    require(
        tondo_native_runtime::tondo_rt_group_add(group, second) == STATUS_OK,
        "all-order second add",
    );
    require(
        tondo_native_runtime::tondo_rt_release(first) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(second) == STATUS_OK,
        "all-order transfer",
    );
    require(
        tondo_native_runtime::tondo_rt_group_all(group) == STATUS_OK,
        "all-order terminal result",
    );
    require(
        tondo_native_runtime::tondo_rt_group_remaining(group) == 0,
        "all-order cleanup",
    );
    require(
        tondo_native_runtime::tondo_rt_group_outcome_count(group) == 2
            && tondo_native_runtime::tondo_rt_group_outcome_index(group, 0) == 0
            && tondo_native_runtime::tondo_rt_group_outcome_index(group, 1) == 1
            && tondo_native_runtime::tondo_rt_group_outcome_value(group, 0) == 1
            && tondo_native_runtime::tondo_rt_group_outcome_value(group, 1) == 2
            && tondo_native_runtime::tondo_rt_group_outcome_is_error(group, 0) == 0
            && tondo_native_runtime::tondo_rt_group_outcome_is_error(group, 1) == 0,
        "all-order outcomes",
    );
    println!(
        r#"{{"id":"group-all-order","status":"passed","all":"ok","remaining":0,"outcomes":2,"values":[1,2]}}"#
    );
    require(
        tondo_native_runtime::tondo_rt_release(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_live_objects() == 0,
        "all-order release",
    );
}

fn settle_mixed() {
    tondo_native_runtime::tondo_rt_reset();
    let group = tondo_native_runtime::tondo_rt_group_new();
    let success = tondo_native_runtime::tondo_rt_result_new(RESULT_OK, 7, 1);
    let failure = tondo_native_runtime::tondo_rt_result_new(RESULT_ERR, 8, 1);
    let success_task = tondo_native_runtime::tondo_rt_task_spawn(success, 0);
    let failure_task = tondo_native_runtime::tondo_rt_task_spawn(failure, 0);
    require(
        tondo_native_runtime::tondo_rt_group_add(group, success_task) == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_add(group, failure_task) == STATUS_OK,
        "settle-mixed add",
    );
    require(
        tondo_native_runtime::tondo_rt_release(success_task) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(failure_task) == STATUS_OK,
        "settle-mixed transfer",
    );
    require(
        tondo_native_runtime::tondo_rt_group_settle(group) == STATUS_OK,
        "settle-mixed terminal result",
    );
    require(
        tondo_native_runtime::tondo_rt_group_remaining(group) == 0,
        "settle-mixed cleanup",
    );
    require(
        tondo_native_runtime::tondo_rt_group_outcome_count(group) == 2
            && tondo_native_runtime::tondo_rt_group_outcome_index(group, 0) == 0
            && tondo_native_runtime::tondo_rt_group_outcome_index(group, 1) == 1
            && tondo_native_runtime::tondo_rt_group_outcome_value(group, 0) == 7
            && tondo_native_runtime::tondo_rt_group_outcome_value(group, 1) == 8
            && tondo_native_runtime::tondo_rt_group_outcome_is_error(group, 0) == 0
            && tondo_native_runtime::tondo_rt_group_outcome_is_error(group, 1) == 1,
        "settle-mixed outcomes",
    );
    println!(
        r#"{{"id":"group-settle-mixed","status":"passed","settle":"ok","outcomes":2,"values":[7,8],"errors":[false,true]}}"#
    );
    require(
        tondo_native_runtime::tondo_rt_release(success) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(failure) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_live_objects() == 0,
        "settle-mixed release",
    );
}

fn all_error_priority() {
    tondo_native_runtime::tondo_rt_reset();
    let group = tondo_native_runtime::tondo_rt_group_new();
    let pending = tondo_native_runtime::tondo_rt_task_spawn(91, 1);
    let error = tondo_native_runtime::tondo_rt_result_new(RESULT_ERR, 12, 1);
    let error_task = tondo_native_runtime::tondo_rt_task_spawn(error, 0);
    require(
        tondo_native_runtime::tondo_rt_group_add(group, pending) == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_add(group, error_task) == STATUS_OK,
        "all-error add",
    );
    require(
        tondo_native_runtime::tondo_rt_release(pending) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(error_task) == STATUS_OK,
        "all-error transfer",
    );
    require(
        tondo_native_runtime::tondo_rt_group_all(group) == RESULT_ERR,
        "all-error result",
    );
    let selected = tondo_native_runtime::tondo_rt_group_last_value(group);
    require(
        tondo_native_runtime::tondo_rt_result_tag(selected) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_result_payload(selected) == 12,
        "all-error lowest insertion error",
    );
    require(
        tondo_native_runtime::tondo_rt_group_outcome_count(group) == 1
            && tondo_native_runtime::tondo_rt_group_outcome_index(group, 0) == 1
            && tondo_native_runtime::tondo_rt_group_outcome_value(group, 0) == 12
            && tondo_native_runtime::tondo_rt_group_outcome_is_error(group, 0) == 1,
        "all-error outcomes",
    );
    require(
        tondo_native_runtime::tondo_rt_release(selected) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(error) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_live_objects() == 0,
        "all-error release",
    );
    println!(
        r#"{{"id":"group-all-error-priority","status":"passed","error_tag":3,"error_payload":12,"pending_drained":true,"outcomes":1}}"#
    );
}

fn next_completion_order() {
    tondo_native_runtime::tondo_rt_reset();
    let group = tondo_native_runtime::tondo_rt_group_new();
    let first = tondo_native_runtime::tondo_rt_task_spawn(11, 1);
    let second = tondo_native_runtime::tondo_rt_task_spawn(22, 1);
    require(
        tondo_native_runtime::tondo_rt_group_add(group, first) == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_add(group, second) == STATUS_OK,
        "next-order add",
    );
    require(
        tondo_native_runtime::tondo_rt_release(first) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(second) == STATUS_OK,
        "next-order transfer",
    );
    require(
        tondo_native_runtime::tondo_rt_group_next(group) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_NOT_READY,
        "next-order pending",
    );
    require(
        tondo_native_runtime::tondo_rt_task_wake(second) == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_next(group) == 22
            && tondo_native_runtime::tondo_rt_group_next_index(group) == 1,
        "next-order second completion",
    );
    require(
        tondo_native_runtime::tondo_rt_task_wake(first) == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_next(group) == 11
            && tondo_native_runtime::tondo_rt_group_next_index(group) == 0,
        "next-order first completion",
    );
    require(
        tondo_native_runtime::tondo_rt_group_next(group) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_OK,
        "next-order affine none",
    );
    require(
        tondo_native_runtime::tondo_rt_group_cancel(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_live_objects() == 0,
        "next-order release",
    );
    println!(
        r#"{{"id":"group-next-order","status":"passed","indices":[1,0],"values":[22,11],"none":true}}"#
    );
}

fn panic_drains() {
    tondo_native_runtime::tondo_rt_reset();
    let group = tondo_native_runtime::tondo_rt_group_new();
    let panicking = tondo_native_runtime::tondo_rt_task_spawn(0, 1);
    let sibling = tondo_native_runtime::tondo_rt_task_spawn(0, 1);
    require(
        tondo_native_runtime::tondo_rt_group_add(group, panicking) == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_add(group, sibling) == STATUS_OK,
        "panic add",
    );
    require(
        tondo_native_runtime::tondo_rt_release(panicking) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(sibling) == STATUS_OK
            && tondo_native_runtime::tondo_rt_task_panic(panicking) == STATUS_OK,
        "panic publish",
    );
    require(
        tondo_native_runtime::tondo_rt_group_next(group) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_PANICKED
            && tondo_native_runtime::tondo_rt_group_remaining(group) == 0,
        "panic drain",
    );
    require(
        tondo_native_runtime::tondo_rt_release(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_live_objects() == 0,
        "panic release",
    );
    println!(
        r#"{{"id":"group-panic-drain","status":"passed","panic":true,"cleanup":"exactly-once"}}"#
    );
}

fn cancel_drains() {
    tondo_native_runtime::tondo_rt_reset();
    let group = tondo_native_runtime::tondo_rt_group_new();
    let child = tondo_native_runtime::tondo_rt_task_spawn(0, 1);
    require(
        tondo_native_runtime::tondo_rt_group_add(group, child) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(child) == STATUS_OK,
        "cancel add",
    );
    require(
        tondo_native_runtime::tondo_rt_group_cancel(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_remaining(group) == 0
            && tondo_native_runtime::tondo_rt_release(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_live_objects() == 0,
        "cancel drain",
    );
    println!(r#"{{"id":"group-cancel-drain","status":"passed","cleanup":"exactly-once"}}"#);
}

fn empty_operations() {
    tondo_native_runtime::tondo_rt_reset();
    let all = tondo_native_runtime::tondo_rt_group_new();
    require(
        tondo_native_runtime::tondo_rt_group_all(all) == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_remaining(all) == 0,
        "empty all",
    );
    require(
        tondo_native_runtime::tondo_rt_release(all) == STATUS_OK,
        "empty all release",
    );
    let settle = tondo_native_runtime::tondo_rt_group_new();
    require(
        tondo_native_runtime::tondo_rt_group_settle(settle) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(settle) == STATUS_OK,
        "empty settle",
    );
    let next = tondo_native_runtime::tondo_rt_group_new();
    require(
        tondo_native_runtime::tondo_rt_group_next(next) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_OK
            && tondo_native_runtime::tondo_rt_group_cancel(next) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(next) == STATUS_OK
            && tondo_native_runtime::tondo_rt_live_objects() == 0,
        "empty next/cancel",
    );
    println!(
        r#"{{"id":"group-empty","status":"passed","all":true,"settle":true,"next_none":true}}"#
    );
}

fn invalid_add() {
    tondo_native_runtime::tondo_rt_reset();
    let group = tondo_native_runtime::tondo_rt_group_new();
    require(
        tondo_native_runtime::tondo_rt_group_add(group, 0) == STATUS_INVALID_HANDLE,
        "invalid child handle",
    );
    let child = tondo_native_runtime::tondo_rt_task_spawn(1, 0);
    require(
        tondo_native_runtime::tondo_rt_task_take(child) == 1
            && tondo_native_runtime::tondo_rt_group_add(group, child) == STATUS_INVALID_TRANSITION
            && tondo_native_runtime::tondo_rt_release(child) == STATUS_OK
            && tondo_native_runtime::tondo_rt_release(group) == STATUS_OK
            && tondo_native_runtime::tondo_rt_live_objects() == 0,
        "joined child rejection",
    );
    println!(
        r#"{{"id":"group-invalid-add","status":"passed","invalid_handle":true,"joined_rejected":true}}"#
    );
}
