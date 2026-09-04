//! Shared-corpus probe for the private native `std.executor` bridge.
//!
//! The hosted fixture exercises the public executor surface.  This probe only
//! observes the native lane that exists today: opaque blocking-pool tokens,
//! lifecycle transitions and managed payload transfer.  Cooperative pools,
//! actors and callable AOT lowering remain explicit target boundaries.

const RESULT_OK: u64 = 2;
const STATUS_OK: u64 = 0;
const STATUS_CANCELLED: u64 = 7;
const STATUS_BLOCKING_INVALID_HANDLE: u64 = 26;
const STATUS_BLOCKING_INVALID_TRANSITION: u64 = 28;
const BLOCKING_JOB_COMPLETED: u64 = 2;
const BLOCKING_JOB_CANCELLED: u64 = 3;
const BLOCKING_JOB_TAKEN: u64 = 4;

fn require(condition: bool, message: &str) {
    assert!(condition, "std.executor conformance: {message}");
}

fn release(value: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_release(value) == STATUS_OK,
        message,
    );
}

fn require_clean(case_id: &str) {
    require(tondo_native_runtime::tondo_rt_live_objects() == 0, case_id);
}

fn main() {
    pool_admission();
    pool_saturation_boundary();
    blocking_transfer();
    blocking_cancel();
    actor_fifo_boundary();
    actor_terminal_boundary();
    threads_capability();
    aot_boundary();
    println!(r#"{{"id":"executor-conformance","status":"passed"}}"#);
}

fn pool_admission() {
    tondo_native_runtime::tondo_rt_reset();
    let pool = tondo_native_runtime::tondo_rt_blocking_pool_new(1, 1);
    require(pool != 0, "pool admission creates native lane");
    let job = tondo_native_runtime::tondo_rt_blocking_pool_submit(pool, 42);
    require(job != 0, "pool admission accepts token");
    require(
        tondo_native_runtime::tondo_rt_blocking_job_wait(job) == STATUS_OK
            && tondo_native_runtime::tondo_rt_blocking_job_status(job) == BLOCKING_JOB_COMPLETED
            && tondo_native_runtime::tondo_rt_blocking_job_worker(job) == 0,
        "pool admission reaches one worker",
    );
    require(
        tondo_native_runtime::tondo_rt_blocking_job_take(job) == 42
            && tondo_native_runtime::tondo_rt_blocking_job_status(job) == BLOCKING_JOB_TAKEN,
        "pool admission transfers token once",
    );
    require(
        tondo_native_runtime::tondo_rt_blocking_job_take(job) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_BLOCKING_INVALID_TRANSITION,
        "pool admission rejects duplicate take",
    );
    require(
        tondo_native_runtime::tondo_rt_blocking_pool_shutdown(pool) == STATUS_OK
            && tondo_native_runtime::tondo_rt_blocking_pool_status(pool) == 3,
        "pool admission drains lifecycle",
    );
    release(job, "pool admission job release");
    release(pool, "pool admission pool release");
    require_clean("pool admission cleanup");
    println!(
        r#"{{"id":"pool-admission","status":"passed","payload":42,"worker":0,"lifecycle":"closed","cleanup":true}}"#
    );
}

fn pool_saturation_boundary() {
    tondo_native_runtime::tondo_rt_reset();
    let invalid = tondo_native_runtime::tondo_rt_blocking_pool_submit(1 << 63 | 999_999, 1);
    require(
        invalid == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_BLOCKING_INVALID_HANDLE,
        "native admission rejects an unknown pool explicitly",
    );
    require_clean("pool saturation boundary cleanup");
    println!(
        r#"{{"id":"pool-saturation","status":"passed","delegated":"hosted-pool-saturation","native_abi":"blocking-admission-only"}}"#
    );
}

fn blocking_transfer() {
    tondo_native_runtime::tondo_rt_reset();
    let pool = tondo_native_runtime::tondo_rt_blocking_pool_new(1, 1);
    require(pool != 0, "managed transfer creates pool");
    let result = tondo_native_runtime::tondo_rt_result_new(RESULT_OK, 7, 1);
    let job = tondo_native_runtime::tondo_rt_blocking_pool_submit(pool, result);
    require(job != 0, "managed transfer admits result token");
    release(result, "managed transfer caller release");
    require(
        tondo_native_runtime::tondo_rt_blocking_job_wait(job) == STATUS_OK,
        "managed transfer waits",
    );
    let transferred = tondo_native_runtime::tondo_rt_blocking_job_take(job);
    require(
        transferred != 0
            && tondo_native_runtime::tondo_rt_result_tag(transferred) == RESULT_OK
            && tondo_native_runtime::tondo_rt_result_payload(transferred) == 7,
        "managed transfer preserves result payload",
    );
    require(
        tondo_native_runtime::tondo_rt_blocking_pool_shutdown(pool) == STATUS_OK,
        "managed transfer shutdown",
    );
    release(job, "managed transfer job release");
    release(transferred, "managed transfer result release");
    release(pool, "managed transfer pool release");
    require_clean("managed transfer cleanup");
    println!(
        r#"{{"id":"blocking-transfer","status":"passed","result_tag":2,"result_payload":7,"managed_transfer":true,"cleanup":true}}"#
    );
}

fn blocking_cancel() {
    tondo_native_runtime::tondo_rt_reset();
    let pool = tondo_native_runtime::tondo_rt_blocking_pool_new(1, 1);
    require(pool != 0, "cancel creates pool");
    let job = tondo_native_runtime::tondo_rt_blocking_pool_submit(pool, 99);
    require(job != 0, "cancel admits token");
    require(
        tondo_native_runtime::tondo_rt_blocking_pool_cancel(pool) == STATUS_CANCELLED
            && tondo_native_runtime::tondo_rt_blocking_pool_status(pool) == 4,
        "cancel drains the lane",
    );
    let wait_status = tondo_native_runtime::tondo_rt_blocking_job_wait(job);
    require(
        wait_status == STATUS_OK || wait_status == STATUS_CANCELLED,
        "cancel waits for a safe terminal state",
    );
    match tondo_native_runtime::tondo_rt_blocking_job_status(job) {
        BLOCKING_JOB_COMPLETED => {
            require(
                tondo_native_runtime::tondo_rt_blocking_job_take(job) == 99,
                "running cancellation preserves completed token",
            );
        }
        BLOCKING_JOB_CANCELLED => {
            require(
                tondo_native_runtime::tondo_rt_blocking_job_take(job) == 0
                    && tondo_native_runtime::tondo_rt_last_status() == STATUS_CANCELLED,
                "queued cancellation is observable",
            );
        }
        state => panic!("unexpected cancellation state {state}"),
    }
    release(job, "cancel job release");
    release(pool, "cancel pool release");
    require_clean("cancel cleanup");
    println!(
        r#"{{"id":"blocking-cancel","status":"passed","pool_cancelled":true,"force_kill":false,"lifecycle":"cancelled","cleanup":true}}"#
    );
}

fn actor_fifo_boundary() {
    tondo_native_runtime::tondo_rt_reset();
    require_clean("actor FIFO boundary cleanup");
    println!(
        r#"{{"id":"actor-fifo","status":"passed","delegated":"hosted-actor-mailbox","native_abi":"blocking-token-only"}}"#
    );
}

fn actor_terminal_boundary() {
    tondo_native_runtime::tondo_rt_reset();
    require_clean("actor terminal boundary cleanup");
    println!(
        r#"{{"id":"actor-terminal","status":"passed","delegated":"hosted-actor-lifecycle","native_abi":"blocking-token-only"}}"#
    );
}

fn threads_capability() {
    tondo_native_runtime::tondo_rt_reset();
    require(
        cfg!(all(target_arch = "x86_64", target_os = "linux")),
        "executor native conformance is target-qualified",
    );
    let pool = tondo_native_runtime::tondo_rt_blocking_pool_new(1, 1);
    require(pool != 0, "threads capability exposes the promoted lane");
    require(
        tondo_native_runtime::tondo_rt_blocking_pool_shutdown(pool) == STATUS_OK,
        "threads capability lane shutdown",
    );
    release(pool, "threads capability pool release");
    require_clean("threads capability cleanup");
    println!(
        r#"{{"id":"threads-capability","status":"passed","target":"x86_64-unknown-linux-gnu","declared":true,"static_rejection":"vm-compile-fail"}}"#
    );
}

fn aot_boundary() {
    tondo_native_runtime::tondo_rt_reset();
    require_clean("AOT boundary cleanup");
    println!(
        r#"{{"id":"aot-boundary","status":"passed","native_aot":"not-claimed","private_lane":"opaque-token"}}"#
    );
}
