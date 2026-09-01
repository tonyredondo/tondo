//! Fresh-process conformance probe for the native std.sync bridge.
//!
//! The hosted VM fixture exercises the complete source-level surface.  This
//! probe intentionally stays on the native ABI that exists today: atomics,
//! epoch parking, worker lifecycle and the collection handle bridge.  Locks,
//! Once and Barrier remain VM-owned until native AOT lowering exposes them.

const RESULT_SOME: u64 = 1;
const STATUS_OK: u64 = 0;
const STATUS_INVALID_HANDLE: u64 = 1;
const STATUS_NOT_READY: u64 = 6;
const STATUS_CANCELLED: u64 = 7;
const STATUS_ATOMIC_MISMATCH: u64 = 17;
const STATUS_ATOMIC_INVALID_ORDER: u64 = 18;
const WORKER_COMPLETED: u64 = 2;

fn require(condition: bool, message: &str) {
    assert!(condition, "std.sync conformance: {message}");
}

fn release(value: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_release(value) == STATUS_OK,
        message,
    );
}

fn waiters_at_least(park: u64, expected: u64) {
    for _ in 0..100_000 {
        if tondo_native_runtime::tondo_rt_sync_park_waiters(park) >= expected {
            return;
        }
        std::thread::yield_now();
    }
    require(
        tondo_native_runtime::tondo_rt_sync_park_waiters(park) >= expected,
        "parking waiter registration",
    );
}

fn main() {
    atomic_orders();
    compare_exchange();
    parking_wakeup();
    cleanup_no_poison();
    once_publication();
    barrier_generations();
    threads_capability();
    collection_conformance();
}

fn atomic_orders() {
    tondo_native_runtime::tondo_rt_reset();
    let atomic = tondo_native_runtime::tondo_rt_atomic_new(1);
    require(atomic != 0, "create atomic");
    require(
        tondo_native_runtime::tondo_rt_atomic_load(atomic, 0) == 1
            && tondo_native_runtime::tondo_rt_atomic_load(atomic, 1) == 1
            && tondo_native_runtime::tondo_rt_atomic_load(atomic, 4) == 1,
        "load order set",
    );
    require(
        tondo_native_runtime::tondo_rt_atomic_store(atomic, 2, 0) == STATUS_OK
            && tondo_native_runtime::tondo_rt_atomic_store(atomic, 3, 2) == STATUS_OK
            && tondo_native_runtime::tondo_rt_atomic_store(atomic, 4, 4) == STATUS_OK,
        "store order set",
    );
    for order in 0..5 {
        let previous = tondo_native_runtime::tondo_rt_atomic_swap(atomic, order + 10, order);
        require(
            previous == if order == 0 { 4 } else { order - 1 + 10 },
            "swap order set",
        );
    }
    require(
        tondo_native_runtime::tondo_rt_atomic_load(atomic, 2) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_ATOMIC_INVALID_ORDER,
        "invalid load order",
    );
    require(
        tondo_native_runtime::tondo_rt_atomic_compare_exchange(atomic, 14, 99, 3, 1) == 14
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_OK,
        "compare exchange order set",
    );
    require(
        tondo_native_runtime::tondo_rt_atomic_compare_exchange(atomic, 14, 1, 4, 1) == 99
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_ATOMIC_MISMATCH,
        "compare exchange mismatch",
    );
    require(
        tondo_native_runtime::tondo_rt_atomic_compare_exchange(atomic, 14, 1, 2, 1) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_ATOMIC_INVALID_ORDER,
        "invalid compare exchange order",
    );
    release(atomic, "release atomic order case");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "atomic order case cleanup",
    );
    println!(
        r#"{{"id":"atomic-orders","status":"passed","orders":[0,1,2,3,4],"invalid_order":true,"cleanup":true}}"#
    );
}

fn compare_exchange() {
    tondo_native_runtime::tondo_rt_reset();
    let atomic = tondo_native_runtime::tondo_rt_atomic_new(0);
    require(atomic != 0, "create concurrent atomic");
    let workers = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                for _ in 0..10 {
                    loop {
                        let observed = tondo_native_runtime::tondo_rt_atomic_load(atomic, 0);
                        let returned = tondo_native_runtime::tondo_rt_atomic_compare_exchange(
                            atomic,
                            observed,
                            observed + 1,
                            3,
                            1,
                        );
                        if returned == observed {
                            break;
                        }
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("atomic worker must finish");
    }
    require(
        tondo_native_runtime::tondo_rt_atomic_load(atomic, 4) == 40,
        "concurrent compare exchange total",
    );
    require(
        tondo_native_runtime::tondo_rt_atomic_compare_exchange(atomic, 99, 0, 3, 1) == 40
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_ATOMIC_MISMATCH,
        "concurrent mismatch preserves value",
    );
    release(atomic, "release concurrent atomic");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "compare exchange cleanup",
    );
    println!(
        r#"{{"id":"compare-exchange","status":"passed","final":40,"workers":4,"increments":40,"mismatch":true,"cleanup":true}}"#
    );
}

fn parking_wakeup() {
    tondo_native_runtime::tondo_rt_reset();
    let park = tondo_native_runtime::tondo_rt_sync_park_new();
    require(park != 0, "create parking signal");
    let first_epoch = tondo_native_runtime::tondo_rt_sync_park_epoch(park);
    require(
        tondo_native_runtime::tondo_rt_sync_park_wait(park, first_epoch, 0) == STATUS_NOT_READY,
        "parking timeout",
    );
    let first_waiter = std::thread::spawn(move || {
        tondo_native_runtime::tondo_rt_sync_park_wait(park, first_epoch, u64::MAX)
    });
    waiters_at_least(park, 1);
    require(
        tondo_native_runtime::tondo_rt_sync_park_wake(park, 0) == 1,
        "wake one waiter",
    );
    require(
        first_waiter.join().expect("single waiter must finish") == STATUS_OK,
        "single waiter wake result",
    );

    let second_epoch = tondo_native_runtime::tondo_rt_sync_park_epoch(park);
    let waiters = (0..3)
        .map(|_| {
            std::thread::spawn(move || {
                tondo_native_runtime::tondo_rt_sync_park_wait(park, second_epoch, u64::MAX)
            })
        })
        .collect::<Vec<_>>();
    waiters_at_least(park, 3);
    require(
        tondo_native_runtime::tondo_rt_sync_park_wake(park, 1) >= 3,
        "wake all waiters",
    );
    for waiter in waiters {
        require(
            waiter.join().expect("all waiter must finish") == STATUS_OK,
            "all waiter wake result",
        );
    }
    let final_epoch = tondo_native_runtime::tondo_rt_sync_park_epoch(park);
    require(
        final_epoch == first_epoch + 2,
        "parking epoch advances once per wake",
    );
    require(
        tondo_native_runtime::tondo_rt_sync_park_waiters(park) == 0,
        "parking waiters drain",
    );
    release(park, "release parking signal");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "parking cleanup",
    );
    println!(
        r#"{{"id":"parking-wakeup","status":"passed","one":true,"all":true,"timeout":true,"epoch":2,"cleanup":true}}"#
    );
}

fn cleanup_no_poison() {
    tondo_native_runtime::tondo_rt_reset();
    let atomic = tondo_native_runtime::tondo_rt_atomic_new(5);
    require(
        tondo_native_runtime::tondo_rt_mark_shared(atomic) == STATUS_OK
            && tondo_native_runtime::tondo_rt_retain(atomic) == STATUS_OK,
        "shared atomic ownership",
    );
    release(atomic, "release retained atomic");
    release(atomic, "release shared atomic");
    require(
        tondo_native_runtime::tondo_rt_release(atomic) == STATUS_INVALID_HANDLE,
        "stale release is rejected",
    );
    require(
        tondo_native_runtime::tondo_rt_atomic_load(atomic, 0) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_INVALID_HANDLE,
        "released atomic handle is invalid",
    );
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "cleanup leaves no live object",
    );
    println!(
        r#"{{"id":"cleanup-no-poison","status":"passed","shared_arc":true,"double_release":true,"live_objects":0,"cleanup":true}}"#
    );
}

fn once_publication() {
    tondo_native_runtime::tondo_rt_reset();
    let atomic = tondo_native_runtime::tondo_rt_atomic_new(0);
    require(
        tondo_native_runtime::tondo_rt_atomic_compare_exchange(atomic, 0, 41, 3, 1) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_OK,
        "publish once value",
    );
    require(
        tondo_native_runtime::tondo_rt_atomic_compare_exchange(atomic, 0, 99, 3, 1) == 41
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_ATOMIC_MISMATCH
            && tondo_native_runtime::tondo_rt_atomic_load(atomic, 4) == 41,
        "once publication is memoized",
    );
    release(atomic, "release once publication bridge");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "once publication cleanup",
    );
    println!(
        r#"{{"id":"once-publication","status":"passed","published":41,"retry":true,"memoized":true,"native_scope":"atomic-publication-bridge","cleanup":true}}"#
    );
}

fn barrier_generations() {
    tondo_native_runtime::tondo_rt_reset();
    let park = tondo_native_runtime::tondo_rt_sync_park_new();
    let first = tondo_native_runtime::tondo_rt_sync_park_epoch(park);
    require(
        tondo_native_runtime::tondo_rt_sync_park_wake(park, 1) == 0
            && tondo_native_runtime::tondo_rt_sync_park_wait(park, first, 0) == STATUS_OK,
        "first epoch generation",
    );
    let second = tondo_native_runtime::tondo_rt_sync_park_epoch(park);
    require(
        tondo_native_runtime::tondo_rt_sync_park_wake(park, 1) == 0
            && tondo_native_runtime::tondo_rt_sync_park_wait(park, second, 0) == STATUS_OK
            && tondo_native_runtime::tondo_rt_sync_park_epoch(park) == first + 2,
        "second epoch generation",
    );
    release(park, "release barrier bridge");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "barrier generation cleanup",
    );
    println!(
        r#"{{"id":"barrier-generations","status":"passed","generations":2,"epoch":2,"native_scope":"epoch-parking-bridge","cleanup":true}}"#
    );
}

fn threads_capability() {
    tondo_native_runtime::tondo_rt_reset();
    let task = tondo_native_runtime::tondo_rt_thread_spawn(7, 0);
    require(task != 0, "create native worker");
    require(
        tondo_native_runtime::tondo_rt_thread_worker_wait(task) == STATUS_OK
            && tondo_native_runtime::tondo_rt_thread_worker_status(task) == WORKER_COMPLETED
            && tondo_native_runtime::tondo_rt_thread_worker_runs(task) == 1
            && tondo_native_runtime::tondo_rt_thread_worker_distinct(task) == 1,
        "native worker completion",
    );
    require(
        tondo_native_runtime::tondo_rt_task_take(task) == 7
            && tondo_native_runtime::tondo_rt_task_poll(task) == 3,
        "native worker join lifecycle",
    );
    release(task, "release completed worker");

    let pending = tondo_native_runtime::tondo_rt_thread_spawn(9, 1);
    require(
        tondo_native_runtime::tondo_rt_thread_worker_wait(pending) == STATUS_OK
            && tondo_native_runtime::tondo_rt_task_poll(pending) == 0
            && tondo_native_runtime::tondo_rt_task_cancel(pending) == STATUS_OK
            && tondo_native_runtime::tondo_rt_task_take(pending) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_CANCELLED,
        "native worker cancellation lifecycle",
    );
    release(pending, "release cancelled worker");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "thread capability cleanup",
    );
    println!(
        r#"{{"id":"threads-capability","status":"passed","completed":true,"cancelled":true,"distinct_worker":true,"cleanup":true}}"#
    );
}

fn collection_conformance() {
    tondo_native_runtime::tondo_rt_reset();
    let array = tondo_native_runtime::tondo_rt_sync_array_new(1);
    require(array != 0, "create collection bridge");
    let write = tondo_native_runtime::tondo_rt_sync_array_set(array, 0, 7);
    require(write != 0, "write collection bridge");
    release(write, "release collection write result");
    let value = tondo_native_runtime::tondo_rt_sync_array_get(array, 0);
    require(
        tondo_native_runtime::tondo_rt_result_tag(value) == RESULT_SOME
            && tondo_native_runtime::tondo_rt_result_payload(value) == 7,
        "read collection bridge",
    );
    release(value, "release collection result");
    release(array, "release collection bridge");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "collection bridge cleanup",
    );
    println!(
        r#"{{"id":"collection-conformance","status":"passed","delegated":"STD-SYNC-COLLECTION-CONF-001","cleanup":true}}"#
    );
}
