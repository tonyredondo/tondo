//! Shared-corpus conformance probe for the private native `std.channel` ABI.
//!
//! The companion Tondo fixtures exercise the same eight case identifiers on
//! the hosted VM. This probe keeps the native side target-qualified: opaque
//! endpoints, FIFO commit, rendezvous wakeups, recoverable errors, terminal
//! drainage and panic cleanup are observed without exposing an AOT layout.

const RESULT_NONE: u64 = 0;
const RESULT_SOME: u64 = 1;
const RESULT_OK: u64 = 2;
const RESULT_ERR: u64 = 3;
const STATUS_OK: u64 = 0;
const STATUS_HOST_CLOSED: u64 = 13;
const STATUS_HOST_LIMIT: u64 = 14;
const STATUS_CHANNEL_INVALID_CAPACITY: u64 = 20;
const STATUS_CHANNEL_FULL: u64 = 21;
const STATUS_CHANNEL_EMPTY: u64 = 22;
const HOST_MAX_BYTES: i64 = 1 << 20;

fn require(condition: bool, message: &str) {
    assert!(condition, "std.channel shared conformance: {message}");
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

fn bounded_fifo() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(2);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    require(
        channel != 0 && sender != 0 && receiver != 0,
        "bounded create",
    );
    release(channel, "bounded identity");

    for value in [1, 2] {
        let result = tondo_native_runtime::tondo_rt_channel_try_send(sender, value);
        require(
            tondo_native_runtime::tondo_rt_result_tag(result) == RESULT_OK,
            "bounded send",
        );
        release(result, "bounded send result");
    }
    let full = tondo_native_runtime::tondo_rt_channel_try_send(sender, 3);
    require(
        tondo_native_runtime::tondo_rt_result_tag(full) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_result_payload(full) == 3
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_CHANNEL_FULL,
        "full preserves payload",
    );
    release(full, "full result");

    for expected in [1, 2] {
        let item = tondo_native_runtime::tondo_rt_channel_try_receive(receiver);
        require(
            tondo_native_runtime::tondo_rt_result_tag(item) == RESULT_SOME
                && tondo_native_runtime::tondo_rt_result_payload(item) == expected,
            "FIFO receive",
        );
        release(item, "received item");
    }
    require(
        tondo_native_runtime::tondo_rt_channel_try_receive(receiver) == RESULT_NONE
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_CHANNEL_EMPTY,
        "open empty receive",
    );
    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "bounded sender close",
    );
    let closed = tondo_native_runtime::tondo_rt_channel_try_receive(receiver);
    require(
        tondo_native_runtime::tondo_rt_result_tag(closed) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_result_payload(closed) == STATUS_HOST_CLOSED,
        "closed receive",
    );
    release(closed, "closed result");
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    require(
        tondo_native_runtime::tondo_rt_channel_drain_len(drain) == 0,
        "bounded drain",
    );
    release(drain, "bounded drain result");
    release(sender, "bounded sender");
    release(receiver, "bounded receiver");
    require_clean("bounded cleanup");
    println!(
        r#"{{"id":"bounded-fifo","status":"passed","order":[1,2],"full_payload":3,"closed":true,"cleanup":true}}"#
    );
}

fn rendezvous_wakeup() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(0);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    require(
        channel != 0 && sender != 0 && receiver != 0,
        "rendezvous create",
    );
    release(channel, "rendezvous identity");

    let waiting =
        std::thread::spawn(move || tondo_native_runtime::tondo_rt_channel_receive(receiver));
    for _ in 0..100_000 {
        if tondo_native_runtime::tondo_rt_channel_waiters(channel) >= 1 {
            break;
        }
        std::thread::yield_now();
    }
    require(
        tondo_native_runtime::tondo_rt_channel_waiters(channel) >= 1,
        "rendezvous waiter",
    );
    let sent = tondo_native_runtime::tondo_rt_channel_send(sender, 7);
    require(
        tondo_native_runtime::tondo_rt_result_tag(sent) == RESULT_OK,
        "rendezvous commit",
    );
    release(sent, "rendezvous send result");
    let received = waiting.join().expect("rendezvous receiver must finish");
    require(
        tondo_native_runtime::tondo_rt_result_tag(received) == RESULT_SOME
            && tondo_native_runtime::tondo_rt_result_payload(received) == 7,
        "rendezvous value",
    );
    release(received, "rendezvous receive result");
    require(
        tondo_native_runtime::tondo_rt_channel_waiters(channel) == 0,
        "rendezvous waiter cleanup",
    );
    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "rendezvous sender close",
    );
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    release(drain, "rendezvous drain");
    release(sender, "rendezvous sender");
    release(receiver, "rendezvous receiver");
    require_clean("rendezvous cleanup");
    println!(
        r#"{{"id":"rendezvous-wakeup","status":"passed","value":7,"wakeups":true,"cleanup":true}}"#
    );
}

fn receiver_drain() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(2);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    require(channel != 0 && sender != 0 && receiver != 0, "drain create");
    release(channel, "drain identity");
    for value in [4, 5] {
        let result = tondo_native_runtime::tondo_rt_channel_try_send(sender, value);
        require(
            tondo_native_runtime::tondo_rt_result_tag(result) == RESULT_OK,
            "drain send",
        );
        release(result, "drain send result");
    }
    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "drain sender close",
    );
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    require(
        tondo_native_runtime::tondo_rt_channel_drain_len(drain) == 2
            && tondo_native_runtime::tondo_rt_channel_drain_next(drain) == 4
            && tondo_native_runtime::tondo_rt_channel_drain_next(drain) == 5
            && tondo_native_runtime::tondo_rt_channel_drain_len(drain) == 0,
        "FIFO terminal drain",
    );
    release(drain, "drain carrier");
    release(sender, "drain sender");
    release(receiver, "drain receiver");
    require_clean("drain cleanup");
    println!(
        r#"{{"id":"receiver-drain","status":"passed","pending":[4,5],"fifo":true,"cleanup":true}}"#
    );
}

fn closed_error() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(1);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    require(
        channel != 0 && sender != 0 && receiver != 0,
        "closed create",
    );
    release(channel, "closed identity");
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    require(
        tondo_native_runtime::tondo_rt_channel_drain_len(drain) == 0,
        "closed empty drain",
    );
    release(drain, "closed drain");
    let result = tondo_native_runtime::tondo_rt_channel_try_send(sender, 9);
    require(
        tondo_native_runtime::tondo_rt_result_tag(result) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_result_payload(result) == 9
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_HOST_CLOSED,
        "closed send preserves payload",
    );
    release(result, "closed send result");
    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "closed sender close",
    );
    release(sender, "closed sender");
    release(receiver, "closed receiver");
    require_clean("closed cleanup");
    println!(
        r#"{{"id":"closed-error","status":"passed","payload":9,"status_code":13,"cleanup":true}}"#
    );
}

fn invalid_capacity() {
    tondo_native_runtime::tondo_rt_reset();
    require(
        tondo_native_runtime::tondo_rt_channel_bounded(-1) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_CHANNEL_INVALID_CAPACITY,
        "negative capacity",
    );
    require(
        tondo_native_runtime::tondo_rt_channel_bounded(HOST_MAX_BYTES + 1) == 0
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_HOST_LIMIT,
        "resource capacity",
    );
    require_clean("capacity cleanup");
    println!(
        r#"{{"id":"invalid-capacity","status":"passed","negative":true,"resource_limit":true,"cleanup":true}}"#
    );
}

fn select_commit() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(1);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    require(
        channel != 0 && sender != 0 && receiver != 0,
        "select create",
    );
    release(channel, "select identity");
    let sent = tondo_native_runtime::tondo_rt_channel_try_send(sender, 6);
    require(
        tondo_native_runtime::tondo_rt_result_tag(sent) == RESULT_OK,
        "select channel commit",
    );
    release(sent, "select send result");
    let received = tondo_native_runtime::tondo_rt_channel_try_receive(receiver);
    require(
        tondo_native_runtime::tondo_rt_result_tag(received) == RESULT_SOME
            && tondo_native_runtime::tondo_rt_result_payload(received) == 6,
        "select channel value",
    );
    release(received, "select receive result");
    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "select sender close",
    );
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    release(drain, "select drain");
    release(sender, "select sender");
    release(receiver, "select receiver");
    require_clean("select cleanup");
    println!(
        r#"{{"id":"select-commit","status":"passed","delegated":"hosted-select-implementation-leaf","native_abi":"private-channel-only"}}"#
    );
}

fn closed_wakeup() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(0);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    require(
        channel != 0 && sender != 0 && receiver != 0,
        "wakeup create",
    );
    release(channel, "wakeup identity");
    let waiting =
        std::thread::spawn(move || tondo_native_runtime::tondo_rt_channel_send(sender, 42));
    for _ in 0..100_000 {
        if tondo_native_runtime::tondo_rt_channel_waiters(channel) >= 1 {
            break;
        }
        std::thread::yield_now();
    }
    require(
        tondo_native_runtime::tondo_rt_channel_waiters(channel) >= 1,
        "sender waiter",
    );
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    require(
        tondo_native_runtime::tondo_rt_channel_drain_len(drain) == 0,
        "wakeup drain",
    );
    release(drain, "wakeup drain carrier");
    let result = waiting.join().expect("closed sender must finish");
    require(
        tondo_native_runtime::tondo_rt_result_tag(result) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_result_payload(result) == 42
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_HOST_CLOSED,
        "wakeup preserves payload",
    );
    release(result, "wakeup send result");
    require(
        tondo_native_runtime::tondo_rt_channel_waiters(channel) == 0,
        "wakeup waiter cleanup",
    );
    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "wakeup sender close",
    );
    release(sender, "wakeup sender");
    release(receiver, "wakeup receiver");
    require_clean("wakeup cleanup");
    println!(
        r#"{{"id":"closed-wakeup","status":"passed","payload":42,"wakeups":true,"cleanup":true}}"#
    );
}

struct EndpointGuard {
    channel: u64,
    sender: u64,
    receiver: u64,
}

impl Drop for EndpointGuard {
    fn drop(&mut self) {
        if self.sender != 0 {
            let _ = tondo_native_runtime::tondo_rt_channel_sender_close(self.sender);
            let _ = tondo_native_runtime::tondo_rt_release(self.sender);
        }
        if self.receiver != 0 {
            let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(self.receiver);
            if drain != 0 {
                let _ = tondo_native_runtime::tondo_rt_release(drain);
            }
            let _ = tondo_native_runtime::tondo_rt_release(self.receiver);
        }
        if self.channel != 0 {
            let _ = tondo_native_runtime::tondo_rt_release(self.channel);
        }
    }
}

fn panic_cleanup() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(1);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    require(channel != 0 && sender != 0 && receiver != 0, "panic create");
    let guard = EndpointGuard {
        channel,
        sender,
        receiver,
    };
    let queued = tondo_native_runtime::tondo_rt_channel_try_send(sender, 9);
    require(
        tondo_native_runtime::tondo_rt_result_tag(queued) == RESULT_OK,
        "panic queue",
    );
    release(queued, "panic queue result");
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        panic!("channel conformance panic");
    }));
    require(unwound.is_err(), "panic propagated");
    drop(guard);
    require_clean("panic cleanup");
    println!(r#"{{"id":"panic-cleanup","status":"passed","panic":true,"cleanup":"exactly-once"}}"#);
}

fn main() {
    bounded_fifo();
    rendezvous_wakeup();
    receiver_drain();
    closed_error();
    invalid_capacity();
    select_commit();
    closed_wakeup();
    panic_cleanup();
    println!(r#"{{"id":"channel-shared-conformance","status":"passed"}}"#);
}
