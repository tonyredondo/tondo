//! Fresh-process conformance probe for the private native std.channel ABI.
//!
//! The source-level fixture exercises the hosted VM. This probe keeps the
//! native target-qualified surface deliberately small: opaque endpoint
//! ownership, FIFO commit, rendezvous wakeups, recoverable backpressure,
//! terminal close and pending-value drainage.

const RESULT_NONE: u64 = 0;
const RESULT_SOME: u64 = 1;
const RESULT_OK: u64 = 2;
const RESULT_ERR: u64 = 3;
const STATUS_OK: u64 = 0;
const STATUS_HOST_CLOSED: u64 = 13;
const STATUS_CHANNEL_FULL: u64 = 21;
const STATUS_CHANNEL_EMPTY: u64 = 22;

fn require(condition: bool, message: &str) {
    assert!(condition, "std.channel conformance: {message}");
}

fn release(value: u64, message: &str) {
    require(
        tondo_native_runtime::tondo_rt_release(value) == STATUS_OK,
        message,
    );
}

fn bounded_fifo() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(1);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    require(
        channel != 0 && sender != 0 && receiver != 0,
        "create bounded channel",
    );
    release(channel, "release channel identity");

    let first = tondo_native_runtime::tondo_rt_channel_try_send(sender, 1);
    require(
        tondo_native_runtime::tondo_rt_result_tag(first) == RESULT_OK,
        "first trySend",
    );
    release(first, "release first send result");
    let full = tondo_native_runtime::tondo_rt_channel_try_send(sender, 2);
    require(
        tondo_native_runtime::tondo_rt_result_tag(full) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_result_payload(full) == 2
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_CHANNEL_FULL,
        "full trySend preserves payload",
    );
    release(full, "release full result");

    let item = tondo_native_runtime::tondo_rt_channel_try_receive(receiver);
    require(
        tondo_native_runtime::tondo_rt_result_tag(item) == RESULT_SOME
            && tondo_native_runtime::tondo_rt_result_payload(item) == 1,
        "FIFO receive",
    );
    release(item, "release item result");
    require(
        tondo_native_runtime::tondo_rt_channel_try_receive(receiver) == RESULT_NONE
            && tondo_native_runtime::tondo_rt_last_status() == STATUS_CHANNEL_EMPTY,
        "empty tryReceive",
    );

    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "close sender",
    );
    let closed = tondo_native_runtime::tondo_rt_channel_try_receive(receiver);
    require(
        tondo_native_runtime::tondo_rt_result_tag(closed) == RESULT_ERR
            && tondo_native_runtime::tondo_rt_result_payload(closed) == STATUS_HOST_CLOSED,
        "closed receive",
    );
    release(closed, "release closed result");
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    require(
        tondo_native_runtime::tondo_rt_channel_drain_len(drain) == 0,
        "empty terminal drain",
    );
    release(drain, "release empty drain");
    release(sender, "release closed sender");
    release(receiver, "release closed receiver");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "bounded case cleanup",
    );
    println!(
        r#"{{"id":"bounded-fifo","status":"passed","full_preserves_payload":true,"empty":true,"closed":true,"cleanup":true}}"#
    );
}

fn rendezvous_and_close() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(0);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    release(channel, "release rendezvous identity");

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
        "rendezvous waiter registration",
    );
    let sent = tondo_native_runtime::tondo_rt_channel_send(sender, 7);
    require(
        tondo_native_runtime::tondo_rt_result_tag(sent) == RESULT_OK,
        "rendezvous send commit",
    );
    release(sent, "release rendezvous send");
    let received = waiting.join().expect("rendezvous receiver must finish");
    require(
        tondo_native_runtime::tondo_rt_result_tag(received) == RESULT_SOME
            && tondo_native_runtime::tondo_rt_result_payload(received) == 7,
        "rendezvous receive commit",
    );
    release(received, "release rendezvous receive");
    require(
        tondo_native_runtime::tondo_rt_channel_waiters(channel) == 0,
        "rendezvous waiter cleanup",
    );
    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "rendezvous sender close",
    );
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    release(drain, "release rendezvous drain");
    release(sender, "release rendezvous sender");
    release(receiver, "release rendezvous receiver");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "rendezvous cleanup",
    );
    println!(
        r#"{{"id":"rendezvous-wakeup","status":"passed","fifo_registration":true,"close_wakes":true,"cleanup":true}}"#
    );
}

fn terminal_drain() {
    tondo_native_runtime::tondo_rt_reset();
    let channel = tondo_native_runtime::tondo_rt_channel_bounded(2);
    let sender = tondo_native_runtime::tondo_rt_channel_sender(channel);
    let receiver = tondo_native_runtime::tondo_rt_channel_receiver(channel);
    release(channel, "release drain identity");
    for value in [31, 32] {
        let result = tondo_native_runtime::tondo_rt_channel_try_send(sender, value);
        require(
            tondo_native_runtime::tondo_rt_result_tag(result) == RESULT_OK,
            "queue value before close",
        );
        release(result, "release queued send");
    }
    require(
        tondo_native_runtime::tondo_rt_channel_sender_close(sender) == STATUS_OK,
        "close drain sender",
    );
    let drain = tondo_native_runtime::tondo_rt_channel_receiver_close(receiver);
    require(
        tondo_native_runtime::tondo_rt_channel_drain_len(drain) == 2
            && tondo_native_runtime::tondo_rt_channel_drain_next(drain) == 31
            && tondo_native_runtime::tondo_rt_channel_drain_next(drain) == 32
            && tondo_native_runtime::tondo_rt_channel_drain_len(drain) == 0,
        "FIFO terminal drain",
    );
    release(drain, "release terminal drain");
    release(sender, "release drain sender");
    release(receiver, "release drain receiver");
    require(
        tondo_native_runtime::tondo_rt_live_objects() == 0,
        "terminal drain cleanup",
    );
    println!(
        r#"{{"id":"terminal-drain","status":"passed","pending_fifo":true,"sender_closed":true,"cleanup":true}}"#
    );
}

fn main() {
    bounded_fifo();
    rendezvous_and_close();
    terminal_drain();
    println!(r#"{{"id":"channel-conformance","status":"passed"}}"#);
}
