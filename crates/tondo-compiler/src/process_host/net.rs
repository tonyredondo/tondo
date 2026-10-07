//! Hosted network value admission. Address content belongs to the VM heap;
//! immutable option snapshots belong to the charged host registry.

use super::*;
use crate::net_provider::{self, executor::NetworkExecutor, tls_transport, transport};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::{future::Future, pin::Pin, task::Poll};
use tondo_stdlib::net::{
    Datagram, Deadline, HostName, IpAddress, NetError, NetLimits, NetOptions, SocketAddress,
    TlsError,
};
use tondo_vm::network::{NetworkOperation as Operation, NetworkType as Kind};

const NODE: u64 = tondo_vm::runtime::TEST_DETACHED_VALUE_BYTES;

pub(super) enum NetworkValue {
    Limits(NetLimits),
    Options(NetOptions),
    Listener(Arc<transport::Listener>),
    Tcp(Arc<transport::TcpStream>),
    TcpRead(Arc<transport::TcpReadHalf>),
    TcpWrite(Arc<transport::TcpWriteHalf>),
    Udp(Arc<transport::UdpSocket>),
    Datagram(Datagram),
    TlsConfig(net_provider::TlsConfig),
    Tls(tls_transport::TlsStream, Arc<Option<VmMemoryCharge>>),
    TlsRead(Arc<tls_transport::TlsReadHalf>, Arc<Option<VmMemoryCharge>>),
    TlsWrite(
        Arc<tls_transport::TlsWriteHalf>,
        Arc<Option<VmMemoryCharge>>,
    ),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{BuildTarget, CapabilityName, Operation as DriverOperation};
    use crate::toolchain::NetworkTarget;
    use tondo_vm::runtime::{VmLimits, VmOutcome, execute_with_limits};

    const VALUES: &str = r#"
fn value[T](result: T ! net.NetError): T {
    match result {
        ok(payload) => payload
        err(_) => panic("network operation failed")
    }
}
"#;

    fn target(server: String) -> NetworkTarget {
        NetworkTarget {
            resolver_servers: vec![server],
        }
    }

    fn compile(
        source: &str,
        config: &NetworkTarget,
        clock: bool,
    ) -> (
        tondo_vm::bytecode::BytecodeProgram,
        tondo_vm::bytecode::BytecodeFunctionId,
    ) {
        let mut capabilities = BTreeSet::from([CapabilityName::new("network").unwrap()]);
        if clock {
            capabilities.insert(CapabilityName::new("clock").unwrap());
        }
        compile_host_admission_source_with_target(
            source,
            capabilities,
            DriverOperation::Run,
            BuildTarget::vm_hosted()
                .with_network_target(config.clone())
                .unwrap(),
        )
    }

    fn run(
        source: &str,
        config: NetworkTarget,
        clock: bool,
        memory: u64,
    ) -> (
        Result<tondo_vm::runtime::VmExecution, VmError>,
        BootstrapHost,
    ) {
        let (program, entry) = compile(source, &config, clock);
        let mut host = BootstrapHost::with_max_bytes(Vec::new(), memory);
        host.install_network_target(Some(config), clock);
        let result = execute_with_limits(
            &program,
            entry,
            &mut host,
            VmLimits {
                max_heap_bytes: memory,
                ..VmLimits::default()
            },
        );
        (result, host)
    }

    fn assert_retired(host: &BootstrapHost) {
        assert!(host.select_calls.is_empty());
        assert!(host.select_winners.is_empty());
        assert!(host.select_sealed.is_empty());
        assert!(host.network.jobs.is_empty());
        assert!(
            host.values
                .values()
                .all(|value| !matches!(value, HostValue::Network(_)))
        );
        assert!(host.buffer_memory.is_empty());
        assert_eq!(host.test_memory.as_ref().unwrap().live_bytes(), 0);
        if let Some(executor) = &host.network.executor {
            assert_eq!(executor.pending_count(), 0);
        }
    }

    #[test]
    fn network_select_forced_source_winner_cancellation_retires_affine_arguments() {
        let source = format!(
            r#"import std.net
import std.channel
{VALUES}
fn forward(outbox: channel.Sender[net.TcpListener], listener: net.TcpListener): Unit ! channel.SendError[net.TcpListener] selectable {{
    let result = outbox.send(listener)
    outbox.close()
    result
}}
fn producer(outbox: channel.Sender[net.TcpListener], listener: net.TcpListener) {{
    select {{
        let result = forward(outbox, listener) => {{
            match result {{
                ok(_) => ()
                err(channel.SendError.Closed(returned)) => net.TcpListener.close(returned)
                err(channel.SendError.ResourceLimit(returned)) => net.TcpListener.close(returned)
            }}
        }}
    }}
}}
fn transfer() {{
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let address = value(net.socketAddress(ip, 0))
    let listener = value(net.listen(address, 1))
    let (outbox, inbox) = match channel.bounded[net.TcpListener](0) {{
        ok(pair) => pair
        err(_) => panic("channel")
    }}
    scope {{
        let child = spawn producer(outbox, listener)
        match inbox.receive() {{
            some(received) => {{
                _ = value(received.localAddress())
                net.TcpListener.close(received)
            }}
            none => panic("rendezvous closed early")
        }}
        return
    }}
}}
fn main() {{ transfer() }}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
        assert!(host.channels.is_empty());
    }

    #[test]
    fn network_select_waiter_rollback_preserves_ready_completion() {
        let source = r#"import std.async
fn forward(waiter: var async.Waiter[Int, String]): Int ! String selectable {
    waiter.wait()
}
fn main() {
    let pair = async.oneshot[Int, String]()
    var (waiter, completer) = pair
    _ = completer.complete(7)
    let readyPair = async.oneshot[Int, String]()
    var (ready, completeReady) = readyPair
    _ = completeReady.complete(1)
    select {
        ready.wait() => ()
        waiter.wait() => panic("losing waiter was selected")
    }
    match waiter.wait() {
        ok(7) => ()
        _ => panic("losing waiter lost completion")
    }
    let another = async.oneshot[Int, String]()
    var (other, completeOther) = another
    _ = completeOther.complete(9)
    let nextPair = async.oneshot[Int, String]()
    var (next, completeNext) = nextPair
    _ = completeNext.complete(1)
    select {
        forward(var other) => panic("losing adapter was selected")
        next.wait() => ()
    }
    match other.wait() {
        ok(9) => ()
        _ => panic("losing adapter lost completion")
    }
}
"#;
        let (result, host) = run(source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_group_rollback_preserves_ready_completion() {
        let source = r#"import std.async
fn immediate(): Int ! String suspends { 33 }
fn yieldOnce() suspends {}
fn forward(group: var async.Group[Int, String]): async.Completion[Int, String]? selectable {
    group.next()
}
fn main() {
    scope {
        var group = async.group[Int, String]()
        let child = spawn immediate()
        group.add(child)
        yieldOnce()
        let pair = async.oneshot[Int, String]()
        var (ready, completer) = pair
        _ = completer.complete(1)
        select {
            ready.wait() => ()
            group.next() => panic("losing group was selected")
        }
        match group.next() {
            some(completion) => match completion.outcome {
                ok(33) => ()
                _ => panic("group completion changed")
            }
            none => panic("losing group lost completion")
        }
        _ = group.cancel()
        var other = async.group[Int, String]()
        let second = spawn immediate()
        other.add(second)
        yieldOnce()
        let nextPair = async.oneshot[Int, String]()
        var (next, completeNext) = nextPair
        _ = completeNext.complete(1)
        select {
            forward(var other) => panic("losing group adapter was selected")
            next.wait() => ()
        }
        match other.next() {
            some(completion) => match completion.outcome {
                ok(33) => ()
                _ => panic("adapter completion changed")
            }
            none => panic("losing adapter lost completion")
        }
        _ = other.cancel()
    }
}
"#;
        let (result, host) = run(source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_pure_checkpoint_runs_only_winning_continuation() {
        let source = r#"import std.channel
fn ready(): Unit selectable {}
fn forward(outbox: ref channel.Sender[Int]): Unit ! channel.SendError[Int] selectable {
    ready()
    outbox.send(42)
}
fn main() {
    let (outbox, inbox) = match channel.bounded[Int](4) {
        ok(pair) => pair
        err(_) => panic("channel")
    }
    select {
        forward(ref outbox) => ()
        forward(ref outbox) => ()
    }
    assert(inbox.receive() == some(42))
    match inbox.tryReceive() {
        channel.TryReceive.Empty => ()
        _ => panic("losing continuation ran")
    }
    outbox.close()
    _ = inbox.close()
}
"#;
        let (result, host) = run(source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_join_adapter_rollback_preserves_join_owner() {
        let source = r#"import std.async
fn immediate(): Int ! String suspends { 33 }
fn yieldOnce() suspends {}
fn forward(join: Join[Int, String]): Int ! String selectable { await join }
fn main() {
    scope {
        let child = spawn immediate()
        yieldOnce()
        let pair = async.oneshot[Int, String]()
        var (ready, completer) = pair
        _ = completer.complete(1)
        select {
            ready.wait() => ()
            forward(child) => panic("losing Join adapter was selected")
        }
        match await child {
            ok(33) => ()
            _ => panic("losing adapter consumed Join")
        }
    }
}
"#;
        let (result, host) = run(source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_source_aggregate_rollback_restores_all_fields() {
        let source = format!(
            r#"import std.net
import std.channel
{VALUES}
fn forward(pair: (net.TcpListener, net.TcpListener), outbox: ref channel.Sender[net.TcpListener]): Unit ! channel.SendError[net.TcpListener] selectable {{
    let (left, right) = pair
    let sent = outbox.send(left)
    net.TcpListener.close(right)
    sent
}}
fn main() {{
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let address = value(net.socketAddress(ip, 0))
    let pair = (value(net.listen(address, 1)), value(net.listen(address, 1)))
    let (outbox, inbox) = match channel.bounded[net.TcpListener](0) {{
        ok(endpoints) => endpoints
        err(_) => panic("channel")
    }}
    select {{
        let result = forward(pair, ref outbox) => {{
            match result {{
                ok(_) => panic("send had no peer")
                err(channel.SendError.Closed(returned)) => net.TcpListener.close(returned)
                err(channel.SendError.ResourceLimit(returned)) => net.TcpListener.close(returned)
            }}
        }}
        else => {{
            let (first, second) = pair
            _ = value(first.localAddress())
            _ = value(second.localAddress())
            net.TcpListener.close(first)
            net.TcpListener.close(second)
        }}
    }}
    outbox.close()
    for pending in inbox.close() {{
        net.TcpListener.close(pending)
    }}
}}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_source_closure_rollback_restores_capture() {
        let source = format!(
            r#"import std.net
import std.channel
{VALUES}
fn main() {{
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let listener = value(net.listen(value(net.socketAddress(ip, 0)), 1))
    let (outbox, inbox) = match channel.bounded[net.TcpListener](0) {{
        ok(endpoints) => endpoints
        err(_) => panic("channel")
    }}
    let operation = (sender: ref channel.Sender[net.TcpListener]): Unit ! channel.SendError[net.TcpListener] selectable {{
        sender.send(listener)
    }}
    select {{
        let result = operation(ref outbox) => {{
            match result {{
                ok(_) => panic("send had no peer")
                err(channel.SendError.Closed(returned)) => net.TcpListener.close(returned)
                err(channel.SendError.ResourceLimit(returned)) => net.TcpListener.close(returned)
            }}
            for pending in inbox.close() {{
                net.TcpListener.close(pending)
            }}
        }}
        else => {{
            for pending in inbox.close() {{
                net.TcpListener.close(pending)
            }}
            match operation(ref outbox) {{
                ok(_) => panic("closed send committed")
                err(channel.SendError.Closed(returned)) => {{
                    _ = value(returned.localAddress())
                    net.TcpListener.close(returned)
                }}
                err(channel.SendError.ResourceLimit(returned)) => {{
                    net.TcpListener.close(returned)
                    panic("unexpected resource error")
                }}
            }}
        }}
    }}
    outbox.close()
}}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_source_sender_rolls_back_affine_argument() {
        let source = format!(
            r#"import std.net
import std.channel
{VALUES}
fn forward(outbox: ref channel.Sender[net.TcpListener], item: net.TcpListener): Unit ! channel.SendError[net.TcpListener] selectable {{
    outbox.send(item)
}}
fn main() {{
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let listener = value(net.listen(value(net.socketAddress(ip, 0)), 1))
    let (outbox, inbox) = match channel.bounded[net.TcpListener](0) {{
        ok(pair) => pair
        err(_) => panic("channel")
    }}
    select {{
        let result = forward(ref outbox, listener) => {{
            match result {{
                ok(_) => panic("send had no peer")
                err(channel.SendError.Closed(returned)) => net.TcpListener.close(returned)
                err(channel.SendError.ResourceLimit(returned)) => net.TcpListener.close(returned)
            }}
        }}
        else => {{
            _ = value(listener.localAddress())
            net.TcpListener.close(listener)
        }}
    }}
    outbox.close()
    for pending in inbox.close() {{
        net.TcpListener.close(pending)
    }}
}}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_winning_send_transfers_affine_argument_once() {
        let source = format!(
            r#"import std.net
import std.channel
{VALUES}
fn main() {{
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let listener = value(net.listen(value(net.socketAddress(ip, 0)), 1))
    let address = value(listener.localAddress())
    let (outbox, inbox) = match channel.bounded[net.TcpListener](1) {{
        ok(pair) => pair
        err(_) => panic("channel")
    }}
    select {{
        let result = outbox.send(listener) => {{
            match result {{
                ok(_) => ()
                err(channel.SendError.Closed(returned)) => {{
                    net.TcpListener.close(returned)
                    panic("closed send")
                }}
                err(channel.SendError.ResourceLimit(returned)) => {{
                    net.TcpListener.close(returned)
                    panic("limited send")
                }}
            }}
        }}
    }}
    match inbox.receive() {{
        some(received) => {{
            assert(value(received.localAddress()) == address)
            net.TcpListener.close(received)
        }}
        none => panic("sent listener missing")
    }}
    outbox.close()
    for pending in inbox.close() {{
        net.TcpListener.close(pending)
    }}
}}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_losing_timer_remains_waitable() {
        let source = r#"import std.time
import std.channel
fn main() {
    let timer = match time.Timer.after(time.Duration.fromNanoseconds(0)) {
        ok(created) => created
        err(_) => panic("timer")
    }
    let (outbox, inbox) = match channel.bounded[Int](1) {
        ok(pair) => pair
        err(_) => panic("channel")
    }
    match outbox.send(7) {
        ok(_) => ()
        err(_) => panic("send")
    }
    select {
        let result = inbox.receive() => {
            assert(result == some(7))
            match timer.wait() {
                ok(_) => ()
                err(_) => panic("losing timer was consumed")
            }
        }
        let result = timer.wait() => {
            match result {
                ok(_) => ()
                err(_) => panic("winning timer")
            }
            assert(inbox.receive() == some(7))
        }
    }
    outbox.close()
    _ = inbox.close()
}
"#;
        let (result, host) = run(source, target("127.0.0.1:9".into()), true, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_eq!(host.time_resources, 0);
        assert!(host.time_jobs.is_empty());
        assert_retired(&host);
    }

    #[test]
    fn network_select_rendezvous_claims_both_selectors_and_forbids_self_pairing() {
        let source = r#"import std.channel
fn receiveSelected(inbox: channel.Receiver[Int]): Int {
    let item = select {
        let result = inbox.receive() => result
    }
    _ = inbox.close()
    match item {
        some(payload) => payload
        none => panic("rendezvous closed")
    }
}
fn main() {
    let (outbox, inbox) = match channel.bounded[Int](0) {
        ok(pair) => pair
        err(_) => panic("channel")
    }
    let noPeer = select {
        let result = outbox.send(99) => {
            _ = result
            false
        }
        let result = inbox.receive() => {
            _ = result
            false
        }
        else => true
    }
    assert(noPeer)
    scope {
        let pending = spawn receiveSelected(inbox)
        select {
            let result = outbox.send(7) => {
                match result {
                    ok(_) => ()
                    err(_) => panic("rendezvous send")
                }
            }
        }
        assert(await pending == 7)
    }
    outbox.close()
}
"#;
        let (result, host) = run(source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_losing_send_retains_affine_listener_owner() {
        let source = format!(
            r#"import std.net
import std.channel
{VALUES}
fn main() {{
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let listener = value(net.listen(value(net.socketAddress(ip, 0)), 1))
    let (outbox, inbox) = match channel.bounded[net.TcpListener](0) {{
        ok(pair) => pair
        err(_) => panic("channel")
    }}
    select {{
        let result = outbox.send(listener) => {{
            match result {{
                ok(_) => panic("send had no peer")
                err(channel.SendError.Closed(returned)) => net.TcpListener.close(returned)
                err(channel.SendError.ResourceLimit(returned)) => net.TcpListener.close(returned)
            }}
        }}
        else => {{
            assert(value(listener.localAddress()) != value(net.socketAddress(ip, 0)))
            net.TcpListener.close(listener)
        }}
    }}
    outbox.close()
    for pending in inbox.close() {{
        net.TcpListener.close(pending)
    }}
}}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_selectable_forwarder_preserves_losing_datagram() {
        let source = format!(
            r#"import std.net
import std.bytes
{VALUES}
fn receive(socket: ref net.UdpSocket, options: net.NetOptions): net.Datagram ! net.NetError selectable {{
    socket.receiveFrom(options)
}}
fn main() {{
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let ephemeral = value(net.socketAddress(ip, 0))
    let options = value(net.options(none, value(net.NetLimits.create(1024, 512, 16))))
    let sender = value(net.bind(ephemeral))
    let first = value(net.bind(ephemeral))
    let second = value(net.bind(ephemeral))
    let data = match bytes.Bytes("x") {{
        ok(payload) => payload
        err(_) => panic("bytes")
    }}
    value(sender.sendTo(data, value(first.localAddress()), options))
    value(sender.sendTo(data, value(second.localAddress()), options))
    let winner = select {{
        let result = receive(ref first, options) => {{
            _ = value(result)
            0
        }}
        let result = receive(ref second, options) => {{
            _ = value(result)
            1
        }}
    }}
    let retained = if winner == 0 {{
        select {{
            let result = second.receiveFrom(options) => {{
                _ = value(result)
                true
            }}
            else => false
        }}
    }} else {{
        select {{
            let result = first.receiveFrom(options) => {{
                _ = value(result)
                true
            }}
            else => false
        }}
    }}
    assert(retained)
    net.UdpSocket.close(first)
    net.UdpSocket.close(second)
    net.UdpSocket.close(sender)
}}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_select_preserves_losing_channel_message() {
        let source = format!(
            r#"import std.net
import std.bytes
import std.channel
{VALUES}
fn main() {{
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let ephemeral = value(net.socketAddress(ip, 0))
    let options = value(net.options(none, value(net.NetLimits.create(1024, 512, 16))))
    let sender = value(net.bind(ephemeral))
    let receiver = value(net.bind(ephemeral))
    let (outbox, inbox) = match channel.bounded[Int](1) {{
        ok(pair) => pair
        err(_) => panic("channel")
    }}
    match outbox.send(7) {{
        ok(_) => ()
        err(_) => panic("send")
    }}
    let data = match bytes.Bytes("x") {{
        ok(payload) => payload
        err(_) => panic("bytes")
    }}
    value(sender.sendTo(data, value(receiver.localAddress()), options))
    let winner = select {{
        let result = receiver.receiveFrom(options) => {{
            _ = value(result)
            0
        }}
        let message = inbox.receive() => {{
            assert(message == some(7))
            1
        }}
    }}
    if winner == 0 {{
        match inbox.tryReceive() {{
            channel.TryReceive.Item(item) => assert(item == 7)
            _ => panic("losing channel consumed its message")
        }}
    }} else {{
        let retained = select {{
            let result = receiver.receiveFrom(options) => {{
                _ = value(result)
                true
            }}
            else => false
        }}
        assert(retained)
    }}
    outbox.close()
    _ = inbox.close()
    net.UdpSocket.close(sender)
    net.UdpSocket.close(receiver)
}}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_public_tls12_tls13_exchange_and_shutdown_use_sealed_peer() {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
        use std::io::{Read, Write};

        let certificate = CertificateDer::from_pem_slice(include_bytes!(
            "../net_provider/fixtures/localhost-cert.pem"
        ))
        .unwrap();
        let pin: String = certificate
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let key = PrivateKeyDer::from_pem_slice(include_bytes!(
                "../net_provider/fixtures/localhost-test-key.pem"
            ))
            .unwrap();
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_protocol_versions(&[version])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], key)
            .unwrap();
            let peer = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                let socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "TLS peer accept timed out"
                            );
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("TLS peer accept: {error}"),
                    }
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
                let mut stream = rustls::StreamOwned::new(connection, socket);
                let mut data = [0; 4];
                stream.read_exact(&mut data).unwrap();
                assert_eq!(&data, b"ping");
                stream.write_all(b"pong").unwrap();
                stream.conn.send_close_notify();
                stream.flush().unwrap();
            });
            let source = format!(
                r#"import std.net
import std.bytes
import std.encoding
import std.io
{VALUES}
fn tlsValue[T](result: T ! net.TlsError): T {{
    match result {{
        ok(payload) => payload
        err(_) => panic("TLS")
    }}
}}
fn main() {{
    let text = match bytes.Bytes("{pin}") {{
        ok(payload) => payload
        err(_) => panic("pin bytes")
    }}
    let hex = encoding.HexOptions.lower(encoding.EncodingLimits.defaults())
    let certificate = match hex.decode(text) {{
        ok(payload) => payload
        err(_) => panic("pin decode")
    }}
    let config = tlsValue(net.tlsConfig(net.TlsVerification.PinnedCertificate(certificate)))
    let name = value(net.hostName("localhost"))
    let options = value(net.options(none, value(net.NetLimits.create(1024, 512, 16))))
    let address = value(net.socketAddress(value(net.IpAddress.parse("127.0.0.1")), {port}))
    let tcp = value(net.connect(address, options))
    let tls = tlsValue(net.TlsStream.connect(tcp, name, config, options))
    let (reader, writer) = net.TlsStream.split(tls)
    let ping = match bytes.Bytes("ping") {{
        ok(payload) => payload
        err(_) => panic("ping bytes")
    }}
    assert(tlsValue(writer.write(ping, options)) == 4)
    tlsValue(writer.flush(options))
    match tlsValue(reader.read(1024, options)) {{
        io.ReadResult.Data(data) => {{
            let pong = match String(data) {{
                ok(decoded) => decoded
                err(_) => panic("pong text")
            }}
            assert(pong == "pong")
        }}
        _ => panic("TLS data missing")
    }}
    match tlsValue(reader.read(1024, options)) {{
        io.ReadResult.Eof => ()
        _ => panic("TLS close_notify missing")
    }}
    tlsValue(writer.shutdown(options))
    net.TlsReadHalf.close(reader)
    net.TlsWriteHalf.close(writer)
}}
"#
            );
            let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 1_048_576);
            peer.join().unwrap();
            assert!(matches!(
                result.unwrap().outcome,
                VmOutcome::Returned(RuntimeValue::Unit)
            ));
            assert_retired(&host);
        }
    }

    #[test]
    fn network_tls_expired_deadline_closes_consumed_tcp_without_handshake_storage() {
        use std::io::Read;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "TCP peer accept timed out"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("TCP peer accept: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            assert_eq!(socket.read(&mut [0; 1]).unwrap(), 0);
        });
        let source = format!(
            r#"import std.net
import std.time
{VALUES}
fn main() {{
    let config = match net.tlsConfig(net.TlsVerification.PlatformRoots) {{
        ok(created) => created
        err(_) => panic("TLS config")
    }}
    let limits = value(net.NetLimits.create(1024, 512, 16))
    let options = value(net.options(none, limits))
    let ip = value(net.IpAddress.parse("127.0.0.1"))
    let tcp = value(net.connect(value(net.socketAddress(ip, {port})), options))
    let instant = match time.now() {{
        ok(now) => now
        err(_) => panic("clock")
    }}
    let expired = value(net.options(some(instant), limits))
    let name = value(net.hostName("localhost"))
    match net.TlsStream.connect(tcp, name, config, expired) {{
        err(net.TlsError.Timeout) => ()
        ok(opened) => {{
            net.TlsStream.close(opened)
            panic("expired handshake committed")
        }}
        err(_) => panic("incorrect deadline error")
    }}
}}
"#
        );
        // This fits the configuration and fixed error, but deliberately cannot
        // also admit the two successful-handshake TLS buffers.
        let (result, host) = run(&source, target("127.0.0.1:9".into()), true, 100_000);
        peer.join().unwrap();
        let execution = result.unwrap();
        assert!(
            matches!(execution.outcome, VmOutcome::Returned(RuntimeValue::Unit)),
            "{:?}",
            execution.outcome
        );
        assert_retired(&host);
    }

    #[test]
    fn network_import_does_not_create_provider_or_registry_state() {
        let (result, host) = run(
            "import std.net\nfn main() {}\n",
            target("127.0.0.1:9".into()),
            false,
            65536,
        );
        assert!(matches!(
            result.unwrap().outcome,
            VmOutcome::Returned(RuntimeValue::Unit)
        ));
        assert!(host.network.executor.is_none());
        assert!(host.network.dns.is_none());
        assert!(host.values.is_empty());
        assert_retired(&host);
    }

    #[test]
    fn network_panic_retires_affine_socket_and_shared_account() {
        let source = format!(
            "import std.net\n{VALUES}\nfn main() {{\n\
            let ip = value(net.IpAddress.parse(\"127.0.0.1\"))\n\
            let address = value(net.socketAddress(ip, 0))\n\
            let listener = value(net.listen(address, 1))\n\
            panic(\"intentional cleanup probe\")\n }}\n"
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), false, 65536);
        assert!(matches!(result.unwrap().outcome, VmOutcome::Panicked(_)));
        assert_retired(&host);
    }

    #[test]
    fn network_public_validation_precedes_expired_deadline_without_dns_io() {
        let source = format!(
            r#"import std.net
import std.time
{VALUES}
fn main() {{
    let instant = match time.now() {{
        ok(now) => now
        err(_) => panic("clock failed")
    }}
    let options = value(net.options(some(instant), net.NetLimits.defaults()))
    let host = value(net.hostName("example.test"))
    match net.resolve(host, 0, options) {{
        err(net.NetError.InvalidPort) => ()
        _ => panic("validation lost precedence")
    }}
    match net.resolve(host, 443, options) {{
        err(net.NetError.Timeout) => ()
        _ => panic("expired deadline reached DNS")
    }}
}}
"#
        );
        let (result, host) = run(&source, target("127.0.0.1:9".into()), true, 65536);
        assert!(matches!(
            result.unwrap().outcome,
            VmOutcome::Returned(RuntimeValue::Unit)
        ));
        assert!(host.network.executor.is_none());
        assert!(host.network.dns.is_none());
        assert_retired(&host);
    }

    #[test]
    fn network_cancelled_accept_retires_reactor_wait_before_response() {
        let mut host = BootstrapHost::default();
        host.install_network_target(Some(target("127.0.0.1:9".into())), false);
        let budget = VmMemoryBudget::new(65536);
        host.set_test_memory_budget(Some(budget.clone()));
        let listener = host
            .invoke(
                "std.net.listen",
                &[
                    address_value(
                        SocketAddress::create(
                            IpAddress::from_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                            0,
                        )
                        .unwrap(),
                    ),
                    RuntimeValue::Integer(1),
                ],
            )
            .unwrap();
        let RuntimeValue::ResultOk(listener) = listener else {
            panic!("listen failed")
        };
        let limits = host.invoke("std.net.NetLimits.defaults", &[]).unwrap();
        let options = host
            .invoke("std.net.options", &[RuntimeValue::OptionNone, limits])
            .unwrap();
        let RuntimeValue::ResultOk(options) = options else {
            panic!("options failed")
        };
        let call = host
            .start_async(
                "std.net.TcpListener.accept",
                &[(*listener).clone(), *options],
            )
            .unwrap();
        assert!(host.poll_async(call).unwrap().is_none());
        assert_eq!(host.network.executor.as_ref().unwrap().pending_count(), 1);
        host.cancel_async(call).unwrap();
        assert_eq!(host.network.executor.as_ref().unwrap().pending_count(), 0);
        assert_eq!(
            host.poll_async(call).unwrap(),
            Some(failure_value(Failure::Net(NetError::Cancelled), false))
        );
        assert!(host.network.jobs.is_empty());
        host.cleanup(&listener).unwrap();
        host.collect_host_values(&tondo_vm::runtime::VmHostRoots::new())
            .unwrap();
        assert_retired(&host);
    }

    #[test]
    fn network_public_dns_uses_declared_server_and_preserves_whole_ordered_values() {
        use hickory_resolver::proto::{
            op::Message,
            rr::{
                RData, Record, RecordType,
                rdata::{A, AAAA},
            },
        };
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let config = target(socket.local_addr().unwrap().to_string());
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let mut buffer = [0; 2048];
                let (length, peer) = socket.recv_from(&mut buffer).unwrap();
                let request = Message::from_vec(&buffer[..length]).unwrap();
                let query = request.queries.first().unwrap().clone();
                assert!(query.name().is_fqdn());
                let mut reply = Message::response(request.id, request.op_code);
                reply.metadata.recursion_desired = true;
                reply.metadata.recursion_available = true;
                reply.add_query(query.clone());
                match query.query_type() {
                    RecordType::A => {
                        for ip in [
                            Ipv4Addr::new(192, 0, 2, 2),
                            Ipv4Addr::new(192, 0, 2, 1),
                            Ipv4Addr::new(192, 0, 2, 2),
                        ] {
                            reply.add_answer(Record::from_rdata(
                                query.name().clone(),
                                0,
                                RData::A(A(ip)),
                            ));
                        }
                    }
                    RecordType::AAAA => {
                        reply.add_answer(Record::from_rdata(
                            query.name().clone(),
                            0,
                            RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)),
                        ));
                    }
                    other => panic!("unexpected DNS query {other:?}"),
                }
                socket.send_to(&reply.to_vec().unwrap(), peer).unwrap();
            }
        });
        let source = format!(
            r#"import std.net
{VALUES}
fn main() {{
    let options = value(net.options(none, value(net.NetLimits.create(1024, 512, 16))))
    let actual = value(net.resolve(value(net.hostName("example.test")), 443, options))
    let expected = [value(net.socketAddress(value(net.IpAddress.parse("192.0.2.2")), 443)),
        value(net.socketAddress(value(net.IpAddress.parse("192.0.2.1")), 443)),
        value(net.socketAddress(value(net.IpAddress.parse("::1")), 443))]
    assert(actual == expected)
}}
"#
        );
        let (result, host) = run(&source, config, false, 65536);
        server.join().unwrap();
        assert!(matches!(
            result.unwrap().outcome,
            VmOutcome::Returned(RuntimeValue::Unit)
        ));
        assert_retired(&host);
    }
}

#[derive(Default)]
pub(super) struct NetworkState {
    executor: Option<NetworkExecutor>,
    jobs: BTreeMap<u64, NetworkJob>,
    dns: Option<net_provider::DnsConfig>,
}

#[derive(Clone, Copy)]
enum Failure {
    Net(NetError),
    Tls(TlsError),
}

impl net_provider::executor::OperationError for Failure {
    fn from_net(error: NetError) -> Self {
        Self::Net(error)
    }
}

enum Output {
    Unit,
    Count(usize),
    Addresses(Vec<SocketAddress>),
    Value(NetworkValue),
    Read(stdlib_io::ReadResult),
    Accept(transport::PreparedAccept),
    PreparedRead(transport::PreparedRead),
    PreparedDatagram(transport::PreparedDatagram),
}

type ProviderFuture = Pin<Box<dyn Future<Output = Result<Output, Failure>> + Send>>;

struct NetworkJob {
    operation: Operation,
    options: NetOptions,
    pending: Option<net_provider::executor::PendingOperation<Output, Failure>>,
    ready: Option<Result<Output, Failure>>,
    roots: tondo_vm::runtime::VmHostRoots,
    _request: Option<VmMemoryCharge>,
    response: Option<VmMemoryCharge>,
    payload: Option<VmMemoryCharge>,
    cancelled: bool,
    import_admitted: bool,
    reserved: bool,
    argument_invalid: bool,
}

fn failure_value(failure: Failure, tls: bool) -> RuntimeValue {
    RuntimeValue::ResultErr(Box::new(match failure {
        Failure::Tls(error) => tls_error(error),
        Failure::Net(error) if tls => tls_error(match error {
            NetError::Cancelled => TlsError::Cancelled,
            NetError::Timeout => TlsError::Timeout,
            NetError::ResourceLimit => TlsError::ResourceLimit,
            error => TlsError::Transport(error),
        }),
        Failure::Net(error) => net_error(error),
    }))
}

fn prepare_net_outcome(
    success: &RuntimeValue,
    admission: &mut VmHostImportAdmission<'_>,
) -> Result<Option<tondo_vm::runtime::VmHostImportReservation>, VmError> {
    let error = failure_value(Failure::Net(NetError::Host), false);
    admission.prepare_current(VmHostReturnPreview::StorageBound(&[
        VmHostReturnPreview::Value(success),
        VmHostReturnPreview::Value(&error),
    ]))
}

fn is_tls(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::TlsConnect
            | Operation::TlsRead
            | Operation::TlsWrite
            | Operation::TlsFlush
            | Operation::TlsShutdown
    )
}

fn shutdown(value: &RuntimeValue) -> Result<std::net::Shutdown, VmError> {
    match value {
        RuntimeValue::Variant {
            name,
            variant,
            values,
        } if name == "Shutdown" && values.is_empty() => match variant {
            0 => Ok(std::net::Shutdown::Read),
            1 => Ok(std::net::Shutdown::Write),
            2 => Ok(std::net::Shutdown::Both),
            _ => Err(VmError::Host("shutdown variant is invalid".into())),
        },
        _ => Err(VmError::Host(
            "shutdown has an invalid nominal shape".into(),
        )),
    }
}

impl NetworkValue {
    pub(super) fn kind(&self) -> Kind {
        match self {
            Self::Limits(_) => Kind::NetLimits,
            Self::Options(_) => Kind::NetOptions,
            Self::Listener(_) => Kind::TcpListener,
            Self::Tcp(_) => Kind::TcpStream,
            Self::TcpRead(_) => Kind::TcpReadHalf,
            Self::TcpWrite(_) => Kind::TcpWriteHalf,
            Self::Udp(_) => Kind::UdpSocket,
            Self::Datagram(_) => Kind::Datagram,
            Self::TlsConfig(_) => Kind::TlsConfig,
            Self::Tls(_, _) => Kind::TlsStream,
            Self::TlsRead(_, _) => Kind::TlsReadHalf,
            Self::TlsWrite(_, _) => Kind::TlsWriteHalf,
        }
    }

    fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Datagram(value) => value.bytes().len(),
                Self::TlsConfig(_) => net_provider::MAX_CERTIFICATE_BYTES,
                _ => 0,
            }
    }
}

fn net_error(error: NetError) -> RuntimeValue {
    RuntimeValue::Variant {
        name: "NetError".into(),
        variant: error.variant(),
        values: Vec::new(),
    }
}

fn tls_error(error: TlsError) -> RuntimeValue {
    RuntimeValue::Variant {
        name: "TlsError".into(),
        variant: error.variant(),
        values: match error {
            TlsError::Transport(error) => vec![net_error(error)],
            _ => vec![],
        },
    }
}

fn ip_value(ip: IpAddress) -> RuntimeValue {
    let (family, bits) = match ip.as_ip() {
        IpAddr::V4(ip) => (4, u128::from(u32::from(ip))),
        IpAddr::V6(ip) => (6, u128::from(ip)),
    };
    RuntimeValue::Record {
        name: "IpAddress".into(),
        values: vec![
            RuntimeValue::Integer(family),
            RuntimeValue::Integer((bits >> 64) as i128),
            RuntimeValue::Integer((bits as u64).into()),
        ],
    }
}

fn address_value(address: SocketAddress) -> RuntimeValue {
    RuntimeValue::Record {
        name: "SocketAddress".into(),
        values: vec![
            ip_value(address.ip()),
            RuntimeValue::Integer(address.port().into()),
        ],
    }
}

fn record<'a>(value: &'a RuntimeValue, expected: &str) -> Result<&'a [RuntimeValue], VmError> {
    match value {
        RuntimeValue::Record { name, values } if name == expected => Ok(values),
        _ => Err(VmError::Host(format!(
            "{expected} value has an invalid nominal shape"
        ))),
    }
}

fn host_name(value: &RuntimeValue) -> Result<HostName, VmError> {
    let [RuntimeValue::String(text)] = record(value, "HostName")? else {
        return Err(VmError::Host(
            "HostName value has an invalid field shape".into(),
        ));
    };
    HostName::parse(text).map_err(|_| VmError::Host("HostName value is invalid".into()))
}

fn ip_input(value: &RuntimeValue) -> Result<IpAddress, VmError> {
    let [
        RuntimeValue::Integer(family),
        RuntimeValue::Integer(high),
        RuntimeValue::Integer(low),
    ] = record(value, "IpAddress")?
    else {
        return Err(VmError::Host(
            "IpAddress value has an invalid field shape".into(),
        ));
    };
    let high =
        u64::try_from(*high).map_err(|_| VmError::Host("IpAddress word is invalid".into()))?;
    let low = u64::try_from(*low).map_err(|_| VmError::Host("IpAddress word is invalid".into()))?;
    let ip = match family {
        4 if high == 0 && low <= u32::MAX.into() => IpAddr::V4(Ipv4Addr::from(low as u32)),
        6 => IpAddr::V6(Ipv6Addr::from((u128::from(high) << 64) | u128::from(low))),
        _ => {
            return Err(VmError::Host(
                "IpAddress family or words are invalid".into(),
            ));
        }
    };
    Ok(IpAddress::from_ip(ip))
}

fn address_input(value: &RuntimeValue) -> Result<SocketAddress, VmError> {
    let [ip, RuntimeValue::Integer(port)] = record(value, "SocketAddress")? else {
        return Err(VmError::Host(
            "SocketAddress value has an invalid field shape".into(),
        ));
    };
    SocketAddress::create(ip_input(ip)?, *port)
        .map_err(|_| VmError::Host("SocketAddress port is invalid".into()))
}

fn publish(
    value: RuntimeValue,
    admission: &mut VmHostImportAdmission<'_>,
) -> Result<RuntimeValue, VmError> {
    let imported = admission.prepare_current(VmHostReturnPreview::Value(&value))?;
    admission.commit(&mut [imported])?;
    Ok(value)
}

fn failure(
    error: NetError,
    response: &mut VmHostReturnBudget<'_>,
    admission: &mut VmHostImportAdmission<'_>,
) -> Result<RuntimeValue, VmError> {
    response.reserve(2 * NODE + 8, [])?;
    publish(
        RuntimeValue::ResultErr(Box::new(net_error(error))),
        admission,
    )
}

impl BootstrapHost {
    fn network_executor(&mut self) -> Result<&NetworkExecutor, NetError> {
        if self.network.executor.is_none() {
            self.network.executor = Some(NetworkExecutor::new(
                self.clock_domain,
                net_provider::executor::MAX_PENDING_OPERATIONS,
            )?);
        }
        Ok(self
            .network
            .executor
            .as_ref()
            .expect("network executor was installed"))
    }

    fn take_network_value(
        &mut self,
        value: &RuntimeValue,
        kind: Kind,
    ) -> Result<NetworkValue, VmError> {
        self.network_value(value, kind)?;
        let RuntimeValue::Host { id, .. } = value else {
            unreachable!()
        };
        let Some(HostValue::Network(value)) = self.values.remove(id) else {
            unreachable!()
        };
        self.buffer_memory.remove(id);
        Ok(value)
    }

    fn network_value(&self, value: &RuntimeValue, kind: Kind) -> Result<&NetworkValue, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Network(actual),
            id,
        } = value
        else {
            return Err(VmError::Host(
                "network value has an invalid token shape".into(),
            ));
        };
        if *actual != kind {
            return Err(VmError::Host(
                "network value has an invalid token kind".into(),
            ));
        }
        match self.values.get(id) {
            Some(HostValue::Network(value)) if value.kind() == kind => Ok(value),
            _ => Err(VmError::Host("network token is stale".into())),
        }
    }

    fn network_limits(&self, value: &RuntimeValue) -> Result<NetLimits, VmError> {
        let NetworkValue::Limits(limits) = self.network_value(value, Kind::NetLimits)? else {
            unreachable!();
        };
        Ok(*limits)
    }

    fn network_options(&self, value: &RuntimeValue) -> Result<NetOptions, VmError> {
        let NetworkValue::Options(options) = self.network_value(value, Kind::NetOptions)? else {
            unreachable!();
        };
        Ok(*options)
    }

    fn publish_network_snapshot(
        &mut self,
        value: NetworkValue,
        fallible: bool,
        response: &mut VmHostReturnBudget<'_>,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<RuntimeValue, VmError> {
        let kind = RuntimeHostValueKind::Network(value.kind());
        let bytes = value.payload_bytes();
        response.reserve(if fallible { 2 * NODE } else { NODE }, [])?;
        let memory = self.reserve_buffer_payloads(std::iter::once(bytes))?;
        let preview = RuntimeValue::Host {
            kind,
            id: self.next_value,
        };
        let preview = if fallible {
            RuntimeValue::ResultOk(Box::new(preview))
        } else {
            preview
        };
        let imported = admission.prepare_current(VmHostReturnPreview::Value(&preview))?;
        admission.commit(&mut [imported])?;
        let snapshot = self.publish_buffer(kind, HostValue::Network(value), memory);
        Ok(if fallible {
            RuntimeValue::ResultOk(Box::new(snapshot))
        } else {
            snapshot
        })
    }

    pub(super) fn start_network(
        &mut self,
        name: &str,
        arguments: &[RuntimeValue],
    ) -> Result<u64, VmError> {
        let operation = Operation::from_name(name)
            .filter(|operation| operation.suspends())
            .ok_or_else(|| VmError::UnsupportedHostCall(name.into()))?;
        if self.network_target.is_none() {
            return Err(VmError::UnsupportedHostCall(name.into()));
        }
        let options = self.network_options(
            arguments
                .last()
                .ok_or_else(|| VmError::Host("network options missing".into()))?,
        )?;
        let mut validation = None;
        match (operation, arguments) {
            (Operation::Resolve, [_, RuntimeValue::Integer(port), _]) => {
                if u16::try_from(*port).ok().filter(|port| *port > 0).is_none() {
                    validation = Some(Failure::Net(NetError::InvalidPort));
                }
            }
            (Operation::Connect, [address, _]) | (Operation::UdpSend, [_, _, address, _]) => {
                if let Err(error) = address_input(address)?.require_destination() {
                    validation = Some(Failure::Net(error));
                }
            }
            _ => {}
        }
        let output_size = match operation {
            Operation::TcpRead | Operation::TlsRead => {
                let [_, RuntimeValue::Integer(max), _] = arguments else {
                    return Err(VmError::Host("read arguments are invalid".into()));
                };
                match options.limits().read_request(*max) {
                    Ok(size) => size,
                    Err(error) => {
                        validation = Some(Failure::Net(error));
                        0
                    }
                }
            }
            Operation::UdpReceive => {
                options.limits().max_datagram() + 1 + std::mem::size_of::<NetworkValue>()
            }
            Operation::Resolve => {
                options.limits().max_results() * std::mem::size_of::<SocketAddress>()
            }
            Operation::TlsConnect => {
                2 * net_provider::TLS_BUFFER_BYTES + std::mem::size_of::<NetworkValue>()
            }
            _ => std::mem::size_of::<NetworkValue>(),
        };
        let input_size = match operation {
            Operation::TcpWrite | Operation::TlsWrite | Operation::UdpSend => {
                let bytes = self.bytes(
                    arguments
                        .get(1)
                        .ok_or_else(|| VmError::Host("network data missing".into()))?,
                )?;
                if operation == Operation::UdpSend
                    && let Err(error) = options.limits().datagram_size(bytes.len())
                {
                    validation = Some(Failure::Net(error));
                }
                bytes.len()
            }
            _ => 0,
        };
        let argument_invalid = validation.is_some();
        let now = self.clock.now()?;
        let initial = options
            .before_commit(self.clock_domain, now, false)
            .err()
            .map(Failure::Net);
        let known_error = validation.or(initial);
        // This account covers retained provider input, metadata and result storage
        // before any future is polled. Typed VM admission happens before each poll.
        let budget = self.test_memory.clone();
        let request = budget
            .as_ref()
            .map(|budget| {
                budget.reserve(
                    tondo_vm::runtime::TEST_HOST_JOB_BYTES
                        + if known_error.is_none() {
                            input_size as u64
                        } else {
                            0
                        }
                        + arguments.len() as u64 * (NODE + 32),
                )
            })
            .transpose()?;
        let response_bytes = if known_error.is_some() {
            3 * NODE
        } else if operation == Operation::Resolve {
            (options.limits().max_results() as u64 * (7 * NODE + 22)) + 2 * NODE
        } else {
            8 * NODE + 32
        };
        let response = budget
            .as_ref()
            .map(|budget| budget.reserve(response_bytes))
            .transpose()?;
        let payload = if known_error.is_some() {
            None
        } else {
            self.reserve_buffer_payloads(std::iter::once(output_size))?
        };
        let mut roots = tondo_vm::runtime::VmHostRoots::new();
        for argument in arguments {
            argument.trace_host_roots(&mut roots);
        }
        let future: ProviderFuture = if let Some(error) = known_error {
            // The handshake consumes its TCP owner even when its initial
            // deadline prevents any provider I/O.
            if operation == Operation::TlsConnect {
                drop(self.take_network_value(&arguments[0], Kind::TcpStream)?);
                roots.retain(|(kind, _)| *kind != RuntimeHostValueKind::Network(Kind::TcpStream));
            }
            Box::pin(std::future::ready(Err(error)))
        } else {
            match (operation, arguments) {
                (Operation::Resolve, [host, RuntimeValue::Integer(port), _]) => {
                    let host = host_name(host)?;
                    let port = *port;
                    if self.network.dns.is_none() {
                        self.network.dns = Some(
                            net_provider::DnsConfig::from_target(
                                self.network_target
                                    .as_ref()
                                    .expect("selected network target"),
                            )
                            .map_err(|error| {
                                VmError::Host(format!("invalid declared resolver: {error}"))
                            })?,
                        );
                    }
                    let dns = self
                        .network
                        .dns
                        .as_ref()
                        .expect("declared resolver was installed")
                        .clone();
                    Box::pin(async move {
                        dns.resolve(host, port, options.limits())
                            .await
                            .map(Output::Addresses)
                            .map_err(Failure::Net)
                    })
                }
                (Operation::Connect, [address, _]) => {
                    let address = address_input(address)?;
                    Box::pin(async move {
                        transport::TcpStream::connect(address)
                            .await
                            .map(|stream| Output::Value(NetworkValue::Tcp(Arc::new(stream))))
                            .map_err(Failure::Net)
                    })
                }
                (Operation::ListenerAccept, [value, _]) => {
                    let NetworkValue::Listener(listener) =
                        self.network_value(value, Kind::TcpListener)?
                    else {
                        unreachable!()
                    };
                    let listener = listener.clone();
                    Box::pin(async move {
                        listener
                            .prepare_accept()
                            .await
                            .map(Output::Accept)
                            .map_err(Failure::Net)
                    })
                }
                (Operation::TcpRead, [value, RuntimeValue::Integer(max), _]) => {
                    let NetworkValue::TcpRead(reader) =
                        self.network_value(value, Kind::TcpReadHalf)?
                    else {
                        unreachable!()
                    };
                    let reader = reader.clone();
                    let max = *max;
                    Box::pin(async move {
                        reader
                            .prepare_read(max, options.limits())
                            .await
                            .map(Output::PreparedRead)
                            .map_err(Failure::Net)
                    })
                }
                (Operation::TcpWrite, [value, data, _]) => {
                    let NetworkValue::TcpWrite(writer) =
                        self.network_value(value, Kind::TcpWriteHalf)?
                    else {
                        unreachable!()
                    };
                    let writer = writer.clone();
                    let data = self.bytes(data)?.to_vec();
                    Box::pin(async move {
                        writer
                            .write(&data)
                            .await
                            .map(Output::Count)
                            .map_err(Failure::Net)
                    })
                }
                (Operation::TcpFlush | Operation::TcpWriteShutdown, [value, _]) => {
                    let NetworkValue::TcpWrite(writer) =
                        self.network_value(value, Kind::TcpWriteHalf)?
                    else {
                        unreachable!()
                    };
                    let writer = writer.clone();
                    Box::pin(async move {
                        if operation == Operation::TcpWriteShutdown {
                            writer.shutdown().map_err(Failure::Net)?;
                        }
                        Ok(Output::Unit)
                    })
                }
                (Operation::TcpShutdown, [value, how, _]) => {
                    let NetworkValue::Tcp(stream) = self.network_value(value, Kind::TcpStream)?
                    else {
                        unreachable!()
                    };
                    let stream = stream.clone();
                    let how = shutdown(how)?;
                    Box::pin(async move {
                        stream
                            .shutdown(how)
                            .map(|()| Output::Unit)
                            .map_err(Failure::Net)
                    })
                }
                (Operation::UdpSend, [value, data, destination, _]) => {
                    let NetworkValue::Udp(socket) = self.network_value(value, Kind::UdpSocket)?
                    else {
                        unreachable!()
                    };
                    let socket = socket.clone();
                    let data = self.bytes(data)?.to_vec();
                    let destination = address_input(destination)?;
                    Box::pin(async move {
                        socket
                            .send(&data, destination, options.limits())
                            .await
                            .map(|()| Output::Unit)
                            .map_err(Failure::Net)
                    })
                }
                (Operation::UdpReceive, [value, _]) => {
                    let NetworkValue::Udp(socket) = self.network_value(value, Kind::UdpSocket)?
                    else {
                        unreachable!()
                    };
                    let socket = socket.clone();
                    Box::pin(async move {
                        socket
                            .prepare_receive(options.limits())
                            .await
                            .map(Output::PreparedDatagram)
                            .map_err(Failure::Net)
                    })
                }
                (Operation::TlsConnect, [value, server, config, _]) => {
                    let server = host_name(server)?;
                    let NetworkValue::TlsConfig(config) =
                        self.network_value(config, Kind::TlsConfig)?
                    else {
                        unreachable!()
                    };
                    let config = config.clone();
                    let NetworkValue::Tcp(stream) =
                        self.take_network_value(value, Kind::TcpStream)?
                    else {
                        unreachable!()
                    };
                    roots.retain(|(kind, _)| {
                        *kind != RuntimeHostValueKind::Network(Kind::TcpStream)
                    });
                    let stream = Arc::try_unwrap(stream).map_err(|_| {
                        VmError::Host("TCP stream still has an active operation".into())
                    })?;
                    Box::pin(async move {
                        tls_transport::TlsStream::connect(stream, server, &config)
                            .await
                            .map(|stream| Output::Value(NetworkValue::Tls(stream, Arc::new(None))))
                            .map_err(Failure::Tls)
                    })
                }
                (Operation::TlsRead, [value, RuntimeValue::Integer(max), _]) => {
                    let NetworkValue::TlsRead(reader, memory) =
                        self.network_value(value, Kind::TlsReadHalf)?
                    else {
                        unreachable!()
                    };
                    let reader = reader.clone();
                    let memory = memory.clone();
                    let max = *max;
                    Box::pin(async move {
                        let result = reader
                            .read(max, options.limits())
                            .await
                            .map(Output::Read)
                            .map_err(Failure::Tls);
                        drop(memory);
                        result
                    })
                }
                (Operation::TlsWrite, [value, data, _]) => {
                    let NetworkValue::TlsWrite(writer, memory) =
                        self.network_value(value, Kind::TlsWriteHalf)?
                    else {
                        unreachable!()
                    };
                    let writer = writer.clone();
                    let memory = memory.clone();
                    let data = self.bytes(data)?.to_vec();
                    Box::pin(async move {
                        let result = writer.write(&data).map(Output::Count).map_err(Failure::Tls);
                        drop(memory);
                        result
                    })
                }
                (Operation::TlsFlush | Operation::TlsShutdown, [value, _]) => {
                    let NetworkValue::TlsWrite(writer, memory) =
                        self.network_value(value, Kind::TlsWriteHalf)?
                    else {
                        unreachable!()
                    };
                    let writer = writer.clone();
                    let memory = memory.clone();
                    Box::pin(async move {
                        let result = if operation == Operation::TlsFlush {
                            writer.flush().await
                        } else {
                            writer.shutdown().await
                        };
                        drop(memory);
                        result.map(|()| Output::Unit).map_err(Failure::Tls)
                    })
                }
                _ => {
                    return Err(VmError::Host(format!(
                        "{name} received an invalid network argument list"
                    )));
                }
            }
        };
        let pending = if initial.is_none() && validation.is_none() {
            let neutral = NetOptions::create(None, options.limits(), self.clock_domain)
                .expect("validated domain");
            match self.network_executor() {
                Ok(executor) => match executor.start(neutral, now, future) {
                    Ok(pending) => Some(pending),
                    Err(error) => {
                        validation = Some(error);
                        None
                    }
                },
                Err(error) => {
                    validation = Some(Failure::Net(error));
                    None
                }
            }
        } else {
            None
        };
        let call = self.next_async_call()?;
        self.network.jobs.insert(
            call,
            NetworkJob {
                operation,
                options,
                pending,
                ready: validation.or(initial).map(Err),
                roots,
                _request: request,
                response,
                payload,
                cancelled: false,
                import_admitted: false,
                reserved: false,
                argument_invalid,
            },
        );
        Ok(call)
    }

    pub(super) fn network_pending(&self, call: u64) -> bool {
        self.network.jobs.contains_key(&call)
    }

    pub(super) fn network_has_pending(&self) -> bool {
        !self.network.jobs.is_empty()
    }

    pub(super) fn reserve_network(&mut self, call: u64) -> bool {
        let Some(job) = self.network.jobs.get_mut(&call) else {
            return false;
        };
        if !job.operation.selectable() {
            return false;
        }
        job.reserved = true;
        true
    }

    pub(super) fn network_ready(&self, call: u64) -> bool {
        self.network
            .jobs
            .get(&call)
            .is_some_and(|job| job.reserved && job.ready.is_some())
    }

    pub(super) fn commit_network(
        &mut self,
        call: u64,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<Option<VmHostReturn>, VmError> {
        let Some(job) = self.network.jobs.get_mut(&call) else {
            return Err(VmError::Host("unknown reserved network call".into()));
        };
        job.reserved = false;
        let result = self.poll_network(call, admission);
        if let Some(job) = self.network.jobs.get_mut(&call) {
            job.reserved = true;
        }
        result
    }

    pub(super) fn rollback_network(&mut self, call: u64) {
        self.network.jobs.remove(&call);
    }

    pub(super) fn cancel_network(&mut self, call: u64) {
        if let Some(job) = self.network.jobs.get_mut(&call) {
            if let Some(pending) = &mut job.pending {
                pending.cancel();
            }
            job.cancelled = true;
            job.reserved = false;
        }
    }

    pub(super) fn trace_network(&self, roots: &mut tondo_vm::runtime::VmHostRoots) {
        for job in self.network.jobs.values() {
            roots.extend(job.roots.iter().copied());
        }
    }

    fn network_result_preview(&self, job: &NetworkJob) -> RuntimeValue {
        let value = match job.operation {
            Operation::Resolve => RuntimeValue::Array(vec![
                address_value(
                    SocketAddress::create(IpAddress::from_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)), 1)
                        .expect("valid preview address")
                );
                job.options.limits().max_results()
            ]),
            Operation::Connect | Operation::ListenerAccept => RuntimeValue::Host {
                kind: RuntimeHostValueKind::Network(Kind::TcpStream),
                id: self.next_value,
            },
            Operation::UdpReceive => RuntimeValue::Host {
                kind: RuntimeHostValueKind::Network(Kind::Datagram),
                id: self.next_value,
            },
            Operation::TlsConnect => RuntimeValue::Host {
                kind: RuntimeHostValueKind::Network(Kind::TlsStream),
                id: self.next_value,
            },
            Operation::TcpRead | Operation::TlsRead => RuntimeValue::Variant {
                name: "ReadResult".into(),
                variant: 0,
                values: vec![RuntimeValue::Host {
                    kind: RuntimeHostValueKind::Bytes,
                    id: self.next_value,
                }],
            },
            Operation::TcpWrite | Operation::TlsWrite => RuntimeValue::Integer(0),
            _ => RuntimeValue::Unit,
        };
        RuntimeValue::ResultOk(Box::new(value))
    }

    pub(super) fn poll_network(
        &mut self,
        call: u64,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<Option<VmHostReturn>, VmError> {
        let mut job = self
            .network
            .jobs
            .remove(&call)
            .ok_or_else(|| VmError::Host("unknown network call".into()))?;
        let result = (|| {
            if !job.argument_invalid
                && let Err(error) =
                    job.options
                        .before_commit(self.clock_domain, self.clock.now()?, job.cancelled)
            {
                if let Some(pending) = &mut job.pending {
                    pending.cancel();
                }
                job.pending.take();
                job.ready = Some(Err(Failure::Net(error)));
            }
            let known_error = job
                .ready
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .copied();
            let success = if known_error.is_some() {
                RuntimeValue::Unit
            } else {
                self.network_result_preview(&job)
            };
            let error = failure_value(
                known_error.unwrap_or(Failure::Net(NetError::Host)),
                is_tls(job.operation),
            );
            let eof = RuntimeValue::ResultOk(Box::new(RuntimeValue::Variant {
                name: "ReadResult".into(),
                variant: 1,
                values: vec![],
            }));
            let alternatives = if known_error.is_some() {
                vec![VmHostReturnPreview::Value(&error)]
            } else if matches!(job.operation, Operation::TcpRead | Operation::TlsRead) {
                vec![
                    VmHostReturnPreview::Value(&success),
                    VmHostReturnPreview::Value(&error),
                    VmHostReturnPreview::Value(&eof),
                ]
            } else {
                vec![
                    VmHostReturnPreview::Value(&success),
                    VmHostReturnPreview::Value(&error),
                ]
            };
            let import = if job.import_admitted {
                None
            } else {
                admission.prepare(call, VmHostReturnPreview::StorageBound(&alternatives))?
            };
            if job.ready.is_none() {
                let pending = job
                    .pending
                    .as_mut()
                    .ok_or_else(|| VmError::Host("network call has no pending provider".into()))?;
                let now = self.clock.now()?;
                match self
                    .network
                    .executor
                    .as_ref()
                    .expect("pending provider owns an executor")
                    .poll(pending, now)
                {
                    Poll::Pending => return Ok(None),
                    Poll::Ready(result) => {
                        job.ready = Some(result);
                        job.pending.take();
                    }
                }
            }
            if !job.import_admitted {
                if job.reserved {
                    return Ok(None);
                }
                admission.commit(&mut [import])?;
                job.import_admitted = true;
            }
            // The same admitted envelope remains live if readiness races with a
            // syscall. A retry preserves the prepared endpoint without re-reading.
            let ready = match job.ready.as_mut().expect("ready provider reply") {
                Ok(Output::PreparedRead(read)) => match read.try_commit() {
                    Ok(None) => return Ok(None),
                    Ok(Some(value)) => Some(Ok(Output::Read(value))),
                    Err(error) => Some(Err(Failure::Net(error))),
                },
                Ok(Output::PreparedDatagram(datagram)) => match datagram.try_commit() {
                    Ok(None) => return Ok(None),
                    Ok(Some(value)) => Some(Ok(Output::Value(NetworkValue::Datagram(value)))),
                    Err(error) => Some(Err(Failure::Net(error))),
                },
                _ => None,
            };
            if let Some(ready) = ready {
                job.ready = Some(ready);
            }
            let output = match job.ready.take().expect("ready provider reply") {
                Ok(Output::Accept(accept)) => accept
                    .commit()
                    .map(|stream| Output::Value(NetworkValue::Tcp(Arc::new(stream))))
                    .map_err(Failure::Net),
                result => result,
            };
            let value = match output {
                Err(error) => failure_value(error, is_tls(job.operation)),
                Ok(output) => RuntimeValue::ResultOk(Box::new(match output {
                    Output::Unit => RuntimeValue::Unit,
                    Output::Count(count) => RuntimeValue::Integer(count as i128),
                    Output::Addresses(addresses) => {
                        RuntimeValue::Array(addresses.into_iter().map(address_value).collect())
                    }
                    Output::Read(stdlib_io::ReadResult::Eof) => RuntimeValue::Variant {
                        name: "ReadResult".into(),
                        variant: 1,
                        values: vec![],
                    },
                    Output::Read(stdlib_io::ReadResult::Data(bytes)) => {
                        let bytes = self.publish_buffer(
                            RuntimeHostValueKind::Bytes,
                            HostValue::Bytes(bytes),
                            job.payload.take(),
                        );
                        RuntimeValue::Variant {
                            name: "ReadResult".into(),
                            variant: 0,
                            values: vec![bytes],
                        }
                    }
                    Output::Value(mut value) => {
                        if let NetworkValue::Tls(_, memory) = &mut value {
                            let metadata = job
                                .payload
                                .as_mut()
                                .map(|charge| {
                                    charge.split_off(
                                        tondo_vm::runtime::TEST_HOST_BUFFER_BYTES
                                            + std::mem::size_of::<NetworkValue>() as u64,
                                    )
                                })
                                .transpose()?;
                            *memory = Arc::new(job.payload.take());
                            job.payload = metadata;
                        }
                        let kind = RuntimeHostValueKind::Network(value.kind());
                        self.publish_buffer(kind, HostValue::Network(value), job.payload.take())
                    }
                    Output::Accept(_) | Output::PreparedRead(_) | Output::PreparedDatagram(_) => {
                        unreachable!("prepared output was committed")
                    }
                })),
            };
            Ok(Some(VmHostReturn {
                value,
                memory: job.response.take(),
            }))
        })();
        if matches!(result, Ok(None)) {
            self.network.jobs.insert(call, job);
        }
        result
    }

    pub(super) fn invoke_network_admitted(
        &mut self,
        name: &str,
        arguments: &[RuntimeValue],
        response: &mut VmHostReturnBudget<'_>,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<RuntimeValue, VmError> {
        if self.network_target.is_none() {
            return Err(VmError::UnsupportedHostCall(name.into()));
        }
        let operation =
            Operation::from_name(name).ok_or_else(|| VmError::UnsupportedHostCall(name.into()))?;
        match (operation, arguments) {
            (Operation::HostName, [RuntimeValue::String(text)]) => {
                if let Err(error) = HostName::parse(text) {
                    return failure(error, response, admission);
                }
                response.reserve(3 * NODE + 8 + text.len() as u64, [])?;
                publish(
                    RuntimeValue::ResultOk(Box::new(RuntimeValue::Record {
                        name: "HostName".into(),
                        values: vec![RuntimeValue::String(text.clone())],
                    })),
                    admission,
                )
            }
            (Operation::IpParse, [RuntimeValue::String(text)]) => match IpAddress::parse(text) {
                Ok(ip) => {
                    response.reserve(5 * NODE + 9, [])?;
                    publish(RuntimeValue::ResultOk(Box::new(ip_value(ip))), admission)
                }
                Err(error) => failure(error, response, admission),
            },
            (Operation::SocketAddress, [ip, RuntimeValue::Integer(port)]) => {
                match SocketAddress::create(ip_input(ip)?, *port) {
                    Ok(address) => {
                        response.reserve(7 * NODE + 22, [])?;
                        publish(
                            RuntimeValue::ResultOk(Box::new(address_value(address))),
                            admission,
                        )
                    }
                    Err(error) => failure(error, response, admission),
                }
            }
            (
                Operation::LimitsCreate,
                [
                    RuntimeValue::Integer(read),
                    RuntimeValue::Integer(datagram),
                    RuntimeValue::Integer(results),
                ],
            ) => match NetLimits::create(*read, *datagram, *results) {
                Ok(limits) => self.publish_network_snapshot(
                    NetworkValue::Limits(limits),
                    true,
                    response,
                    admission,
                ),
                Err(error) => failure(error, response, admission),
            },
            (Operation::LimitsDefaults, []) => self.publish_network_snapshot(
                NetworkValue::Limits(NetLimits::default()),
                false,
                response,
                admission,
            ),
            (Operation::Options, [deadline, limits]) => {
                let limits = self.network_limits(limits)?;
                let deadline = match deadline {
                    RuntimeValue::OptionNone => None,
                    RuntimeValue::OptionSome(value) => {
                        if !self.network_clock {
                            return failure(NetError::CapabilityMissing, response, admission);
                        }
                        let (domain, ticks) = self.instant(value)?;
                        match Deadline::create(domain, ticks) {
                            Ok(deadline) => Some(deadline),
                            Err(error) => return failure(error, response, admission),
                        }
                    }
                    _ => return Err(VmError::Host("network deadline is not an Option".into())),
                };
                match NetOptions::create(deadline, limits, self.clock_domain) {
                    Ok(options) => self.publish_network_snapshot(
                        NetworkValue::Options(options),
                        true,
                        response,
                        admission,
                    ),
                    Err(error) => failure(error, response, admission),
                }
            }
            (Operation::Listen, [address, RuntimeValue::Integer(backlog)]) => {
                let address = address_input(address)?;
                if u32::try_from(*backlog)
                    .ok()
                    .filter(|value| *value > 0)
                    .is_none()
                {
                    return failure(NetError::InvalidLimit, response, admission);
                }
                response.reserve(2 * NODE + 8, [])?;
                let memory = self.reserve_buffer_payloads(std::iter::once(std::mem::size_of::<
                    NetworkValue,
                >()))?;
                let preview = RuntimeValue::ResultOk(Box::new(RuntimeValue::Host {
                    kind: RuntimeHostValueKind::Network(Kind::TcpListener),
                    id: self.next_value,
                }));
                let imported = prepare_net_outcome(&preview, admission)?;
                admission.commit(&mut [imported])?;
                let result = self.network_executor().and_then(|executor| {
                    executor.enter(|| transport::Listener::listen(address, *backlog))
                });
                Ok(match result {
                    Ok(listener) => RuntimeValue::ResultOk(Box::new(self.publish_buffer(
                        RuntimeHostValueKind::Network(Kind::TcpListener),
                        HostValue::Network(NetworkValue::Listener(Arc::new(listener))),
                        memory,
                    ))),
                    Err(error) => RuntimeValue::ResultErr(Box::new(net_error(error))),
                })
            }
            (Operation::Bind, [address]) => {
                let address = address_input(address)?;
                response.reserve(2 * NODE + 8, [])?;
                let memory = self.reserve_buffer_payloads(std::iter::once(std::mem::size_of::<
                    NetworkValue,
                >()))?;
                let preview = RuntimeValue::ResultOk(Box::new(RuntimeValue::Host {
                    kind: RuntimeHostValueKind::Network(Kind::UdpSocket),
                    id: self.next_value,
                }));
                let imported = prepare_net_outcome(&preview, admission)?;
                admission.commit(&mut [imported])?;
                let result = self
                    .network_executor()
                    .and_then(|executor| executor.enter(|| transport::UdpSocket::bind(address)));
                Ok(match result {
                    Ok(socket) => RuntimeValue::ResultOk(Box::new(self.publish_buffer(
                        RuntimeHostValueKind::Network(Kind::UdpSocket),
                        HostValue::Network(NetworkValue::Udp(Arc::new(socket))),
                        memory,
                    ))),
                    Err(error) => RuntimeValue::ResultErr(Box::new(net_error(error))),
                })
            }
            (
                Operation::ListenerLocal
                | Operation::TcpLocal
                | Operation::TcpPeer
                | Operation::UdpLocal,
                [value],
            ) => {
                let result = match operation {
                    Operation::ListenerLocal => {
                        match self.network_value(value, Kind::TcpListener)? {
                            NetworkValue::Listener(listener) => listener.local_address(),
                            _ => unreachable!(),
                        }
                    }
                    Operation::UdpLocal => match self.network_value(value, Kind::UdpSocket)? {
                        NetworkValue::Udp(socket) => socket.local_address(),
                        _ => unreachable!(),
                    },
                    _ => match self.network_value(value, Kind::TcpStream)? {
                        NetworkValue::Tcp(stream) if operation == Operation::TcpLocal => {
                            stream.local_address()
                        }
                        NetworkValue::Tcp(stream) => stream.peer_address(),
                        _ => unreachable!(),
                    },
                };
                match result {
                    Ok(address) => {
                        response.reserve(7 * NODE + 22, [])?;
                        publish(
                            RuntimeValue::ResultOk(Box::new(address_value(address))),
                            admission,
                        )
                    }
                    Err(error) => failure(error, response, admission),
                }
            }
            (
                Operation::ListenerClose
                | Operation::TcpClose
                | Operation::TcpReadClose
                | Operation::TcpWriteClose
                | Operation::UdpClose
                | Operation::TlsClose
                | Operation::TlsReadClose
                | Operation::TlsWriteClose,
                [value],
            ) => {
                let value_kind = match operation {
                    Operation::ListenerClose => Kind::TcpListener,
                    Operation::TcpClose => Kind::TcpStream,
                    Operation::TcpReadClose => Kind::TcpReadHalf,
                    Operation::TcpWriteClose => Kind::TcpWriteHalf,
                    Operation::UdpClose => Kind::UdpSocket,
                    Operation::TlsClose => Kind::TlsStream,
                    Operation::TlsReadClose => Kind::TlsReadHalf,
                    Operation::TlsWriteClose => Kind::TlsWriteHalf,
                    _ => unreachable!(),
                };
                response.reserve(NODE, [])?;
                let imported =
                    admission.prepare_current(VmHostReturnPreview::Value(&RuntimeValue::Unit))?;
                self.network_value(value, value_kind)?;
                admission.commit(&mut [imported])?;
                drop(self.take_network_value(value, value_kind)?);
                Ok(RuntimeValue::Unit)
            }
            (Operation::TcpSplit | Operation::TlsSplit, [value]) => {
                let tls = operation == Operation::TlsSplit;
                let (stream_kind, read_kind, write_kind) = if tls {
                    (Kind::TlsStream, Kind::TlsReadHalf, Kind::TlsWriteHalf)
                } else {
                    (Kind::TcpStream, Kind::TcpReadHalf, Kind::TcpWriteHalf)
                };
                self.network_value(value, stream_kind)?;
                response.reserve(3 * NODE, [])?;
                let mut memory = self.reserve_buffer_payloads(
                    [std::mem::size_of::<NetworkValue>(); 2].into_iter(),
                )?;
                let preview = RuntimeValue::Tuple(vec![
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Network(read_kind),
                        id: self.next_value,
                    },
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Network(write_kind),
                        id: self.next_value + 1,
                    },
                ]);
                let imported = admission.prepare_current(VmHostReturnPreview::Value(&preview))?;
                admission.commit(&mut [imported])?;
                let (read, write) = match self.take_network_value(value, stream_kind)? {
                    NetworkValue::Tcp(stream) => {
                        let stream = Arc::try_unwrap(stream).map_err(|_| {
                            VmError::Host("TCP stream still has an active operation".into())
                        })?;
                        let (read, write) = stream.split();
                        (
                            NetworkValue::TcpRead(Arc::new(read)),
                            NetworkValue::TcpWrite(Arc::new(write)),
                        )
                    }
                    NetworkValue::Tls(stream, charge) => {
                        let (read, write) = stream.split();
                        (
                            NetworkValue::TlsRead(Arc::new(read), charge.clone()),
                            NetworkValue::TlsWrite(Arc::new(write), charge),
                        )
                    }
                    _ => unreachable!(),
                };
                let read_memory =
                    Self::split_buffer_charge(&mut memory, std::mem::size_of::<NetworkValue>());
                let read = self.publish_buffer(
                    RuntimeHostValueKind::Network(read_kind),
                    HostValue::Network(read),
                    read_memory,
                );
                let write = self.publish_buffer(
                    RuntimeHostValueKind::Network(write_kind),
                    HostValue::Network(write),
                    memory,
                );
                Ok(RuntimeValue::Tuple(vec![read, write]))
            }
            (Operation::DatagramSource, [value]) => {
                let NetworkValue::Datagram(datagram) = self.network_value(value, Kind::Datagram)?
                else {
                    unreachable!()
                };
                response.reserve(6 * NODE + 22, [])?;
                publish(address_value(datagram.source()), admission)
            }
            (Operation::DatagramBytes, [value]) => {
                let NetworkValue::Datagram(datagram) = self.network_value(value, Kind::Datagram)?
                else {
                    unreachable!()
                };
                response.reserve(NODE, [])?;
                let memory =
                    self.reserve_buffer_payloads(std::iter::once(datagram.bytes().len()))?;
                let preview = RuntimeValue::Host {
                    kind: RuntimeHostValueKind::Bytes,
                    id: self.next_value,
                };
                let imported = admission.prepare_current(VmHostReturnPreview::Value(&preview))?;
                let NetworkValue::Datagram(datagram) = self.network_value(value, Kind::Datagram)?
                else {
                    unreachable!()
                };
                let bytes = datagram.bytes().to_vec();
                admission.commit(&mut [imported])?;
                Ok(self.publish_buffer(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(bytes),
                    memory,
                ))
            }
            (Operation::TlsConfig, [verification]) => {
                let pinned = match verification {
                    RuntimeValue::Variant {
                        name,
                        variant: 0,
                        values,
                    } if name == "TlsVerification" && values.is_empty() => None,
                    RuntimeValue::Variant {
                        name,
                        variant: 1,
                        values,
                    } if name == "TlsVerification" && values.len() == 1 => {
                        Some(self.bytes(&values[0])?)
                    }
                    _ => {
                        return Err(VmError::Host(
                            "TLS verification has an invalid nominal shape".into(),
                        ));
                    }
                };
                response.reserve(3 * NODE + 15, [])?;
                if let Some(pinned) = pinned
                    && (pinned.is_empty() || pinned.len() > net_provider::MAX_CERTIFICATE_BYTES)
                {
                    return publish(
                        RuntimeValue::ResultErr(Box::new(tls_error(if pinned.is_empty() {
                            TlsError::InvalidCertificate
                        } else {
                            TlsError::ResourceLimit
                        }))),
                        admission,
                    );
                }
                let memory = self.reserve_buffer_payloads(std::iter::once(
                    std::mem::size_of::<NetworkValue>() + net_provider::MAX_CERTIFICATE_BYTES,
                ))?;
                let success = RuntimeValue::ResultOk(Box::new(RuntimeValue::Host {
                    kind: RuntimeHostValueKind::Network(Kind::TlsConfig),
                    id: self.next_value,
                }));
                let error =
                    RuntimeValue::ResultErr(Box::new(tls_error(TlsError::InvalidCertificate)));
                let imported = admission.prepare_current(VmHostReturnPreview::StorageBound(&[
                    VmHostReturnPreview::Value(&success),
                    VmHostReturnPreview::Value(&error),
                ]))?;
                let result = net_provider::TlsConfig::create(pinned);
                admission.commit(&mut [imported])?;
                Ok(match result {
                    Ok(config) => RuntimeValue::ResultOk(Box::new(self.publish_buffer(
                        RuntimeHostValueKind::Network(Kind::TlsConfig),
                        HostValue::Network(NetworkValue::TlsConfig(config)),
                        memory,
                    ))),
                    Err(error) => RuntimeValue::ResultErr(Box::new(tls_error(error))),
                })
            }
            _ => Err(VmError::UnsupportedHostCall(name.into())),
        }
    }
}
