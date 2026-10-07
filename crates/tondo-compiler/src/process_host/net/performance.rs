//! Private hosted network measurements with controlled loopback peers.
//! No VM interpreter, native runtime ABI or native AOT timing.

use super::*;
use serde::Serialize;
use std::hint::black_box;
use std::time::Instant;

// The independent finite oracle imports no production provider.
#[allow(dead_code)]
#[path = "../../../../tondo-reliability/src/net_model.rs"]
mod reference;

#[derive(Default, Debug, Serialize)]
struct Observations {
    host_calls: u64,
    polls: u64,
    partial_replies: u64,
    pending_polls: u64,
    rollbacks: u64,
    cancellations: u64,
    rejections: u64,
    dns_queries: u64,
    selected_allocations: u64,
    bytes_copied: u64,
    sampled_budget_peak_bytes: u64,
    reply_retained_peak_bytes: u64,
    host_handles_created: u64,
    selected_fixture_bytes: u64,
    selected_fixture_allocations: u64,
    backpressure_pending_polls: u64,
    #[serde(skip)]
    terminal: Terminal,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct Terminal {
    live_handles: u64,
    live_jobs: u64,
    pending_slots: u64,
    budget_bytes: u64,
}

struct Fixture {
    host: BootstrapHost,
    budget: VmMemoryBudget,
    options: RuntimeValue,
    max_read: usize,
    observations: Observations,
    initial_handles: u64,
}

fn success(value: RuntimeValue) -> RuntimeValue {
    let RuntimeValue::ResultOk(value) = value else {
        panic!("unexpected provider failure: {value:?}")
    };
    *value
}

fn split(value: RuntimeValue) -> (RuntimeValue, RuntimeValue) {
    let RuntimeValue::Tuple(mut values) = value else {
        panic!("split did not return a tuple")
    };
    assert_eq!(values.len(), 2);
    let writer = values.pop().unwrap();
    let reader = values.pop().unwrap();
    (reader, writer)
}

// Selected owned identities only. This is not an allocator-call count.
fn identities(value: &RuntimeValue) -> u64 {
    match value {
        RuntimeValue::String(value) => u64::from(!value.is_empty()),
        RuntimeValue::Record { name, values } | RuntimeValue::Variant { name, values, .. } => {
            u64::from(!name.is_empty())
                + u64::from(!values.is_empty())
                + values.iter().map(identities).sum::<u64>()
        }
        RuntimeValue::Tuple(values) | RuntimeValue::Array(values) => {
            u64::from(!values.is_empty()) + values.iter().map(identities).sum::<u64>()
        }
        RuntimeValue::ResultOk(value)
        | RuntimeValue::ResultErr(value)
        | RuntimeValue::OptionSome(value) => 1 + identities(value),
        RuntimeValue::Unit
        | RuntimeValue::Integer(_)
        | RuntimeValue::Bool(_)
        | RuntimeValue::OptionNone
        | RuntimeValue::Host { .. } => 0,
        other => panic!("unmodeled selected identity: {other:?}"),
    }
}

impl Fixture {
    fn new(resolver: String, max_read: i128, max_datagram: i128, max_results: i128) -> Self {
        let expected_limits =
            reference::admission::Limits::create(max_read, max_datagram, max_results).unwrap();
        let mut host = BootstrapHost::with_max_bytes(Vec::new(), 16 * 1024 * 1024);
        host.install_network_target(
            Some(crate::toolchain::NetworkTarget {
                resolver_servers: vec![resolver],
            }),
            true,
        );
        let budget = VmMemoryBudget::new(64 * 1024 * 1024);
        host.set_test_memory_budget(Some(budget.clone()));
        let mut fixture = Self {
            host,
            budget,
            options: RuntimeValue::Unit,
            max_read: usize::try_from(max_read).unwrap(),
            observations: Observations::default(),
            initial_handles: 0,
        };
        let limits = success(fixture.sync(
            "std.net.NetLimits.create",
            &[
                RuntimeValue::Integer(max_read),
                RuntimeValue::Integer(max_datagram),
                RuntimeValue::Integer(max_results),
            ],
        ));
        let actual = fixture.host.network_limits(&limits).unwrap();
        assert_eq!(actual.max_read(), expected_limits.read);
        assert_eq!(actual.max_datagram(), expected_limits.datagram);
        assert_eq!(actual.max_results(), expected_limits.results);
        fixture.options =
            success(fixture.sync("std.net.options", &[RuntimeValue::OptionNone, limits]));
        fixture
    }

    fn mark_timed_start(&mut self) {
        // Resource observations cover setup, the timed interval and teardown.
        // Fixture construction is excluded only from latency, never silently
        // removed from the selected resource model.
        self.sample_budget();
    }

    fn retain_fixture(&mut self, bytes: usize, allocations: u64) {
        self.observations.selected_fixture_bytes += bytes as u64;
        self.observations.selected_fixture_allocations += allocations;
    }

    fn sample_budget(&mut self) {
        self.observations.sampled_budget_peak_bytes = self
            .observations
            .sampled_budget_peak_bytes
            .max(self.budget.live_bytes());
    }

    fn observe_reply(&mut self, reply: &VmHostReturn) {
        self.sample_budget();
        self.observations.reply_retained_peak_bytes = self
            .observations
            .reply_retained_peak_bytes
            .max(reply.memory.as_ref().map_or(0, VmMemoryCharge::bytes));
        self.observations.selected_allocations += identities(&reply.value);
    }

    fn sync(&mut self, name: &str, arguments: &[RuntimeValue]) -> RuntimeValue {
        self.observations.host_calls += 1;
        let reply = black_box(self.host.invoke_admitted(
            name,
            black_box(arguments),
            Some(&self.budget),
        ))
        .unwrap();
        self.observe_reply(&reply);
        let value = reply.value;
        drop(reply.memory);
        value
    }

    fn start(&mut self, name: &str, arguments: &[RuntimeValue]) -> u64 {
        self.observations.host_calls += 1;
        self.observations.selected_allocations += 2; // boxed provider future and job identity
        let call = self.host.start_network(name, black_box(arguments)).unwrap();
        self.sample_budget();
        call
    }

    fn poll(&mut self, call: u64) -> Option<RuntimeValue> {
        self.observations.polls += 1;
        let reply = black_box(
            self.host
                .poll_network(call, &mut VmHostImportAdmission::disabled()),
        )
        .unwrap();
        self.sample_budget();
        reply.map(|reply| {
            self.observe_reply(&reply);
            let value = reply.value;
            drop(reply.memory);
            value
        })
    }

    fn finish(&mut self, call: u64) -> RuntimeValue {
        let stop = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < stop, "bounded bridge sample timed out");
            if let Some(reply) = self.poll(call) {
                return reply;
            }
            self.observations.pending_polls += 1;
            std::thread::yield_now();
        }
    }

    fn invoke_async(&mut self, name: &str, arguments: &[RuntimeValue]) -> RuntimeValue {
        let call = self.start(name, arguments);
        self.finish(call)
    }

    fn wait_reserved(&mut self, call: u64) {
        assert!(self.host.reserve_network(call));
        let stop = Instant::now() + Duration::from_secs(5);
        while !self.host.network_ready(call) {
            assert!(
                Instant::now() < stop,
                "bounded reserved readiness timed out"
            );
            assert!(self.poll(call).is_none());
            if !self.host.network_ready(call) {
                self.observations.pending_polls += 1;
                std::thread::yield_now();
            }
        }
    }

    fn rollback(&mut self, call: u64) {
        self.observations.rollbacks += 1;
        self.host.rollback_network(call);
        assert!(!self.host.network_pending(call));
        self.sample_budget();
    }

    fn retire_buffer(&mut self, value: &RuntimeValue) {
        let RuntimeValue::Host { id, .. } = value else {
            panic!("buffer is not a host value")
        };
        assert!(self.host.values.remove(id).is_some());
        assert!(self.host.buffer_memory.remove(id).is_some());
        self.sample_budget();
    }

    fn close(&mut self, name: &str, value: &RuntimeValue) {
        assert_eq!(
            self.sync(name, std::slice::from_ref(value)),
            RuntimeValue::Unit
        );
    }

    fn terminal(mut self) -> Observations {
        assert!(self.host.network.jobs.is_empty());
        if let Some(executor) = &self.host.network.executor {
            assert_eq!(executor.pending_count(), 0);
        }
        self.observations.host_handles_created = self.host.next_value - self.initial_handles;
        self.host
            .collect_host_values(&tondo_vm::runtime::VmHostRoots::new())
            .unwrap();
        assert!(self.host.values.is_empty());
        assert!(self.host.buffer_memory.is_empty());
        assert_eq!(self.budget.live_bytes(), 0);
        self.observations.terminal = Terminal {
            live_handles: self.host.values.len() as u64,
            live_jobs: self.host.network.jobs.len() as u64,
            pending_slots: self
                .host
                .network
                .executor
                .as_ref()
                .map_or(0, |executor| executor.pending_count() as u64),
            budget_bytes: self.budget.live_bytes(),
        };
        assert_eq!(self.observations.terminal, Terminal::default());
        // The reactor is outside the logical charge model, but still retires.
        self.host.network = NetworkState::default();
        self.observations
    }
}

fn loopback(port: u16) -> RuntimeValue {
    address_value(
        SocketAddress::create(
            IpAddress::from_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port.into(),
        )
        .unwrap(),
    )
}

fn connect(fixture: &mut Fixture, address: RuntimeValue) -> RuntimeValue {
    let options = fixture.options.clone();
    success(fixture.invoke_async("std.net.connect", &[address, options]))
}

fn read_exact(fixture: &mut Fixture, reader: &RuntimeValue, expected: &[u8], tls: bool) {
    let options = fixture.options.clone();
    let name = if tls {
        "std.net.TlsReadHalf.read"
    } else {
        "std.net.TcpReadHalf.read"
    };
    let mut offset = 0;
    while offset < expected.len() {
        let remaining = (expected.len() - offset).min(fixture.max_read);
        let value = success(fixture.invoke_async(
            name,
            &[
                reader.clone(),
                RuntimeValue::Integer(remaining as i128),
                options.clone(),
            ],
        ));
        let RuntimeValue::Variant {
            name,
            variant: 0,
            values,
        } = value
        else {
            panic!("data was replaced with EOF")
        };
        assert_eq!(name, "ReadResult");
        assert_eq!(values.len(), 1);
        let bytes = fixture.host.bytes(&values[0]).unwrap();
        assert!(!bytes.is_empty() && bytes.len() <= remaining);
        assert_eq!(bytes, &expected[offset..offset + bytes.len()]);
        fixture.observations.partial_replies += u64::from(bytes.len() < remaining);
        fixture.observations.bytes_copied += bytes.len() as u64;
        fixture.observations.selected_allocations += 1; // published owned byte buffer
        offset += bytes.len();
        fixture.retire_buffer(&values[0]);
    }
}

fn write_all(fixture: &mut Fixture, writer: &RuntimeValue, data: &[u8], tls: bool) {
    let options = fixture.options.clone();
    let name = if tls {
        "std.net.TlsWriteHalf.write"
    } else {
        "std.net.TcpWriteHalf.write"
    };
    let mut offset = 0;
    while offset < data.len() {
        let bytes = fixture
            .host
            .allocate_bytes(data[offset..].to_vec())
            .unwrap();
        fixture.observations.selected_allocations += 2; // argument buffer and retained provider copy
        // Both timed argument construction and the retained provider request
        // copy the submitted remainder, even when only part is committed.
        fixture.observations.bytes_copied += 2 * (data.len() - offset) as u64;
        let count =
            success(fixture.invoke_async(name, &[writer.clone(), bytes.clone(), options.clone()]));
        let RuntimeValue::Integer(count) = count else {
            panic!("write returned a non-integer count")
        };
        let count = usize::try_from(count).unwrap();
        assert!(count > 0 && count <= data.len() - offset);
        fixture.observations.partial_replies += u64::from(count < data.len() - offset);
        offset += count;
        fixture.retire_buffer(&bytes);
    }
}

fn accept_peer(listener: std::net::TcpListener) -> std::net::TcpStream {
    listener.set_nonblocking(true).unwrap();
    let stop = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            Instant::now() < stop,
            "bounded fixture peer accept timed out"
        );
        match listener.accept() {
            Ok((socket, _)) => return socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::yield_now();
            }
            Err(error) => panic!("fixture peer accept failed: {error}"),
        }
    }
}

fn tcp_read_fixture(length: usize, read_chunk: usize, rollback: bool) -> (u64, Observations) {
    use std::io::Write;
    use std::sync::mpsc;

    let expected: Vec<_> = (0..length).map(|index| (index % 251) as u8).collect();
    if length <= reference::admission::MAX_REFERENCE_BYTES {
        assert_eq!(
            reference::admission::read(&expected, length.max(1), false).unwrap(),
            reference::admission::Read::Data(expected.clone())
        );
        if rollback {
            let mut model = reference::stream::Stream::new(length).unwrap();
            assert_eq!(model.feed(&expected), Ok(length));
            let id = model
                .prepare(
                    read_chunk as i128,
                    reference::admission::Limits::create(read_chunk as i128, 64, 16).unwrap(),
                    None,
                    7,
                    0,
                )
                .unwrap();
            let ready = model.snapshot();
            assert_eq!(
                model.ready(id),
                Ok(Some(reference::admission::Read::Data(expected.clone())))
            );
            assert_eq!(model.snapshot(), ready);
            assert_eq!(
                model.finish(id, false, false, 0),
                Ok(reference::stream::Completion::RolledBack)
            );
            assert_eq!(model.snapshot().queued, expected);
            model.close_scope();
            assert!(!model.snapshot().transport_live);
        }
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (queued, ready) = mpsc::sync_channel(1);
    let payload = expected.clone();
    let peer = std::thread::spawn(move || {
        let mut socket = accept_peer(listener);
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket.write_all(&payload).unwrap();
        socket.shutdown(std::net::Shutdown::Write).unwrap();
        queued.send(()).unwrap();
    });
    let mut fixture = Fixture::new("127.0.0.1:9".into(), read_chunk as i128, 1024, 16);
    fixture.retain_fixture(2 * length, 2 * u64::from(length > 0));
    let stream = connect(&mut fixture, loopback(address.port()));
    let (reader, writer) = split(fixture.sync("std.net.TcpStream.split", &[stream]));
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    fixture.mark_timed_start();
    let started = Instant::now();
    if rollback {
        let options = fixture.options.clone();
        let call = fixture.start(
            "std.net.TcpReadHalf.read",
            &[
                reader.clone(),
                RuntimeValue::Integer(read_chunk as i128),
                options,
            ],
        );
        fixture.wait_reserved(call);
        fixture.rollback(call);
    }
    read_exact(&mut fixture, &reader, &expected, false);
    let options = fixture.options.clone();
    let eof = success(fixture.invoke_async(
        "std.net.TcpReadHalf.read",
        &[reader.clone(), RuntimeValue::Integer(1), options],
    ));
    assert_eq!(
        eof,
        RuntimeValue::Variant {
            name: "ReadResult".into(),
            variant: 1,
            values: vec![]
        }
    );
    fixture.close("std.net.TcpReadHalf.close", &reader);
    fixture.close("std.net.TcpWriteHalf.close", &writer);
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    peer.join().unwrap();
    (nanos, fixture.terminal())
}

fn tcp_backpressure_fixture() -> (u64, Observations) {
    use std::io::Read;
    use std::sync::mpsc;

    const LENGTH: usize = 8 * 1024 * 1024;
    assert_eq!(reference::admission::write(LENGTH, LENGTH), Ok(LENGTH));
    let payload: Vec<_> = (0..LENGTH).map(|index| (index % 251) as u8).collect();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (release, wait) = mpsc::sync_channel(1);
    let peer = std::thread::spawn(move || {
        let mut socket = accept_peer(listener);
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut received = 0;
        let mut buffer = [0; 65536];
        loop {
            let count = socket.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            assert!(received + count <= LENGTH);
            for (offset, byte) in buffer[..count].iter().enumerate() {
                assert_eq!(*byte, ((received + offset) % 251) as u8);
            }
            received += count;
        }
        assert_eq!(received, LENGTH);
    });
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 65536, 1024, 16);
    fixture.retain_fixture(payload.len(), 1);
    let stream = connect(&mut fixture, loopback(address.port()));
    let (reader, writer) = split(fixture.sync("std.net.TcpStream.split", &[stream]));
    fixture.mark_timed_start();
    let started = Instant::now();
    let stop = started + Duration::from_secs(5);
    let mut written = 0;
    let mut released = false;
    while written < payload.len() {
        let bytes = fixture
            .host
            .allocate_bytes(payload[written..].to_vec())
            .unwrap();
        fixture.observations.selected_allocations += 2;
        fixture.observations.bytes_copied += 2 * (payload.len() - written) as u64;
        let options = fixture.options.clone();
        let call = fixture.start(
            "std.net.TcpWriteHalf.write",
            &[writer.clone(), bytes.clone(), options],
        );
        let value = loop {
            assert!(
                Instant::now() < stop,
                "bounded backpressure bridge sample timed out"
            );
            if let Some(value) = fixture.poll(call) {
                break success(value);
            }
            fixture.observations.pending_polls += 1;
            // Initial reactor readiness may be pending before the first write.
            // Withhold the consumer until real bytes and a partial syscall
            // precede a pending producer; an initial poll alone is insufficient.
            if !released && written > 0 && fixture.observations.partial_replies > 0 {
                fixture.observations.backpressure_pending_polls += 1;
                if fixture.observations.backpressure_pending_polls >= 16 {
                    release.send(()).unwrap();
                    released = true;
                }
            }
            std::thread::yield_now();
        };
        let RuntimeValue::Integer(count) = value else {
            panic!("write returned a non-integer count")
        };
        let count = usize::try_from(count).unwrap();
        assert!(count > 0 && count <= payload.len() - written);
        fixture.observations.partial_replies += u64::from(count < payload.len() - written);
        written += count;
        fixture.retire_buffer(&bytes);
    }
    assert!(released, "fixture did not establish actual pending writes");
    let options = fixture.options.clone();
    assert_eq!(
        success(fixture.invoke_async("std.net.TcpWriteHalf.shutdown", &[writer.clone(), options])),
        RuntimeValue::Unit
    );
    fixture.close("std.net.TcpReadHalf.close", &reader);
    fixture.close("std.net.TcpWriteHalf.close", &writer);
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    peer.join().unwrap();
    (nanos, fixture.terminal())
}

fn tcp_connection_fixture() -> (u64, Observations) {
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 128, 64, 16);
    fixture.mark_timed_start();
    let started = Instant::now();
    let listener =
        success(fixture.sync("std.net.listen", &[loopback(0), RuntimeValue::Integer(1)]));
    let address = success(fixture.sync(
        "std.net.TcpListener.localAddress",
        std::slice::from_ref(&listener),
    ));
    let options = fixture.options.clone();
    let accepting = fixture.start("std.net.TcpListener.accept", &[listener.clone(), options]);
    let client = connect(&mut fixture, address);
    let server = success(fixture.finish(accepting));
    let local = success(fixture.sync(
        "std.net.TcpStream.localAddress",
        std::slice::from_ref(&server),
    ));
    let peer = success(fixture.sync(
        "std.net.TcpStream.peerAddress",
        std::slice::from_ref(&client),
    ));
    assert_eq!(local, peer);
    fixture.close("std.net.TcpStream.close", &server);
    fixture.close("std.net.TcpStream.close", &client);
    fixture.close("std.net.TcpListener.close", &listener);
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    (nanos, fixture.terminal())
}

fn tcp_write_fixture(length: usize) -> (u64, Observations) {
    use std::io::Read;

    let expected: Vec<_> = (0..length).map(|index| (index % 251) as u8).collect();
    assert_eq!(reference::admission::write(length, length), Ok(length));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let mut socket = accept_peer(listener);
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut received = 0;
        let mut buffer = [0; 4096];
        loop {
            let count = socket.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            assert!(received + count <= length);
            for (offset, byte) in buffer[..count].iter().enumerate() {
                assert_eq!(*byte, ((received + offset) % 251) as u8);
            }
            received += count;
        }
        assert_eq!(received, length);
    });
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 4096, 64, 16);
    fixture.retain_fixture(length, u64::from(length > 0));
    let stream = connect(&mut fixture, loopback(address.port()));
    let (reader, writer) = split(fixture.sync("std.net.TcpStream.split", &[stream]));
    fixture.mark_timed_start();
    let started = Instant::now();
    write_all(&mut fixture, &writer, &expected, false);
    let options = fixture.options.clone();
    assert_eq!(
        success(fixture.invoke_async("std.net.TcpWriteHalf.shutdown", &[writer.clone(), options])),
        RuntimeValue::Unit
    );
    fixture.close("std.net.TcpReadHalf.close", &reader);
    fixture.close("std.net.TcpWriteHalf.close", &writer);
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    peer.join().unwrap();
    (nanos, fixture.terminal())
}

fn udp_send_fixture(length: usize) -> (u64, Observations) {
    assert!(length <= 64);
    let expected: Vec<_> = (0..length).map(|index| (index % 251) as u8).collect();
    assert_eq!(reference::admission::send_datagram(length, length), Ok(()));
    let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let destination = peer.local_addr().unwrap();
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 128, 64, 16);
    fixture.retain_fixture(length, u64::from(length > 0));
    let sender = success(fixture.sync("std.net.bind", &[loopback(0)]));
    let source = success(fixture.sync(
        "std.net.UdpSocket.localAddress",
        std::slice::from_ref(&sender),
    ));
    let options = fixture.options.clone();
    fixture.mark_timed_start();
    let started = Instant::now();
    let bytes = fixture.host.allocate_bytes(expected.clone()).unwrap();
    fixture.observations.selected_allocations += 2 * u64::from(length > 0);
    fixture.observations.bytes_copied += 2 * length as u64;
    assert_eq!(
        success(fixture.invoke_async(
            "std.net.UdpSocket.sendTo",
            &[
                sender.clone(),
                bytes.clone(),
                loopback(destination.port()),
                options,
            ]
        )),
        RuntimeValue::Unit
    );
    fixture.retire_buffer(&bytes);
    fixture.close("std.net.UdpSocket.close", &sender);
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    let mut received = [0; 65];
    let (count, actual_source) = peer.recv_from(&mut received).unwrap();
    assert_eq!(&received[..count], expected);
    assert_eq!(source, loopback(actual_source.port()));
    (nanos, fixture.terminal())
}

fn expired_deadline_fixture() -> (u64, Observations) {
    assert_eq!(
        reference::admission::before_commit(
            Some(reference::admission::Deadline::create(7, 0).unwrap()),
            7,
            0,
            false,
        ),
        Err(reference::admission::Error::Timeout)
    );
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 128, 64, 16);
    let instant = fixture.host.allocate_instant(0);
    let RuntimeValue::Host {
        kind: RuntimeHostValueKind::Instant,
        id: instant_id,
    } = &instant
    else {
        panic!("fixture clock snapshot has an invalid kind")
    };
    let instant_id = *instant_id;
    let limits = success(fixture.sync(
        "std.net.NetLimits.create",
        &[
            RuntimeValue::Integer(128),
            RuntimeValue::Integer(64),
            RuntimeValue::Integer(16),
        ],
    ));
    let options = success(fixture.sync(
        "std.net.options",
        &[RuntimeValue::OptionSome(Box::new(instant)), limits],
    ));
    // NetOptions copies domain/ticks. This fixture-only clock carrier is
    // uncharged by allocate_instant and is not visited by the charged-buffer
    // collector; retire exactly the setup token after its final use.
    assert!(!fixture.host.buffer_memory.contains_key(&instant_id));
    assert!(matches!(
        fixture.host.values.remove(&instant_id),
        Some(HostValue::Instant { domain, nanos: 0 }) if domain == fixture.host.clock_domain
    ));
    let server = success(fixture.sync(
        "std.net.hostName",
        &[RuntimeValue::String("example.test".into())],
    ));
    fixture.mark_timed_start();
    let started = Instant::now();
    let reply = fixture.invoke_async(
        "std.net.resolve",
        &[server, RuntimeValue::Integer(443), options],
    );
    assert_eq!(
        reply,
        RuntimeValue::ResultErr(Box::new(RuntimeValue::Variant {
            name: "NetError".into(),
            variant: 13,
            values: vec![], // authored Timeout
        }))
    );
    fixture.observations.rejections += 1;
    assert!(fixture.host.network.executor.is_none());
    assert!(fixture.host.network.dns.is_none());
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    (nanos, fixture.terminal())
}

fn host_memory_refusal_fixture() -> (u64, Observations) {
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 128, 64, 16);
    let server = success(fixture.sync(
        "std.net.hostName",
        &[RuntimeValue::String("example.test".into())],
    ));
    let options = fixture.options.clone();
    let remaining = fixture.budget.limit() - fixture.budget.live_bytes();
    let held = fixture.budget.reserve(remaining).unwrap();
    fixture.mark_timed_start();
    let handles = fixture.host.next_value;
    let started = Instant::now();
    fixture.observations.host_calls += 1;
    let refused = fixture.host.start_network(
        "std.net.resolve",
        &[server, RuntimeValue::Integer(443), options],
    );
    assert!(matches!(
        refused,
        Err(VmError::ResourceLimit {
            resource: "memory",
            limit: 67_108_864
        })
    ));
    fixture.observations.rejections += 1;
    assert_eq!(fixture.host.next_value, handles);
    assert!(fixture.host.network.jobs.is_empty());
    assert!(fixture.host.network.executor.is_none());
    assert!(fixture.host.network.dns.is_none());
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    drop(held);
    (nanos, fixture.terminal())
}

const IDS: [&str; 21] = [
    "tcp-connect-accept",
    "tcp-read-small",
    "tcp-read-chunked",
    "tcp-read-rollback",
    "tcp-write-small",
    "tcp-write-large",
    "tcp-write-backpressure",
    "udp-send-empty",
    "udp-send-small",
    "udp-receive-empty",
    "udp-receive-small",
    "udp-reject-oversized",
    "dns-ordered",
    "dns-delay",
    "dns-result-limit",
    "tls12-echo",
    "tls13-echo",
    "tls-reject-name",
    "pending-accept-cancel",
    "reject-expired-deadline",
    "reject-host-memory",
];

fn run(name: &str) -> (&'static str, u64, Observations) {
    let (operation, result) = match name {
        "tcp-connect-accept" => ("tcp-lifecycle", tcp_connection_fixture()),
        "tcp-read-small" => ("tcp-read", tcp_read_fixture(64, 64, false)),
        "tcp-read-chunked" => ("tcp-read", tcp_read_fixture(65536, 4096, false)),
        "tcp-read-rollback" => ("tcp-read", tcp_read_fixture(64, 64, true)),
        "tcp-write-small" => ("tcp-write", tcp_write_fixture(64)),
        "tcp-write-large" => ("tcp-write", tcp_write_fixture(65536)),
        "tcp-write-backpressure" => ("tcp-write", tcp_backpressure_fixture()),
        "udp-send-empty" => ("udp-send", udp_send_fixture(0)),
        "udp-send-small" => ("udp-send", udp_send_fixture(64)),
        "udp-receive-empty" => ("udp-receive", udp_receive_fixture(0, false)),
        "udp-receive-small" => ("udp-receive", udp_receive_fixture(64, false)),
        "udp-reject-oversized" => ("udp-receive", udp_receive_fixture(64, true)),
        "dns-ordered" => ("dns", dns_fixture(16, false)),
        "dns-delay" => ("dns", dns_fixture(16, true)),
        "dns-result-limit" => ("dns", dns_fixture(2, false)),
        "tls12-echo" => ("tls", tls_exchange_fixture(&rustls::version::TLS12, false)),
        "tls13-echo" => ("tls", tls_exchange_fixture(&rustls::version::TLS13, false)),
        "tls-reject-name" => ("tls", tls_exchange_fixture(&rustls::version::TLS13, true)),
        "pending-accept-cancel" => ("cancellation", pending_accept_cancel_fixture()),
        "reject-expired-deadline" => ("admission", expired_deadline_fixture()),
        "reject-host-memory" => ("admission", host_memory_refusal_fixture()),
        _ => panic!("unknown network measurement route: {name}"),
    };
    (operation, result.0, result.1)
}

#[derive(Serialize)]
struct Sample<'a> {
    workload_id: &'a str,
    operation: &'a str,
    dispatch: &'static str,
    process: usize,
    repetition: usize,
    nanos: u64,
    observations: Observations,
    terminal: Terminal,
}

#[test]
fn network_performance_probe() {
    if std::env::var("TONDO_NET_PERF_RUN").as_deref() != Ok("1") {
        return;
    }
    let process: usize = std::env::var("TONDO_NET_PERF_PROCESS")
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (1..=3).contains(&process),
        "invalid independent process coordinate"
    );
    for name in IDS {
        for _ in 0..3 {
            black_box(run(name));
        }
        for repetition in 0..9 {
            let (operation, nanos, observations) = black_box(run(name));
            let terminal = observations.terminal;
            let sample = Sample {
                workload_id: name,
                operation,
                dispatch: "hosted-scalar",
                process,
                repetition,
                nanos,
                observations,
                terminal,
            };
            println!(
                "\nTONDO_NET_PERF\t{}",
                serde_json::to_string(&sample).unwrap()
            );
        }
    }
}

#[test]
fn network_performance_fixtures_validate_every_route_and_terminal_owner() {
    for name in IDS {
        let (_, nanos, observations) = run(name);
        assert!(nanos > 0);
        assert!(observations.host_calls > 0);
        assert!(observations.sampled_budget_peak_bytes > 0);
        assert_eq!(observations.terminal, Terminal::default());
    }
}

#[test]
fn network_performance_sample_serialization_preserves_observed_resources() {
    let (operation, nanos, observations) = run("udp-receive-small");
    assert_eq!(observations.bytes_copied, 128);
    assert_eq!(observations.terminal, Terminal::default());
    let counters = serde_json::to_value(&observations).unwrap();
    let terminal = observations.terminal;
    let sample = Sample {
        workload_id: "udp-receive-small",
        operation,
        dispatch: "hosted-scalar",
        process: 1,
        repetition: 0,
        nanos,
        observations,
        terminal,
    };
    let actual = serde_json::to_value(sample).unwrap();
    assert_eq!(actual["observations"], counters);
    assert!(actual["observations"].get("terminal").is_none());
    assert_eq!(actual["dispatch"], "hosted-scalar");
    assert_eq!(actual["process"], 1);
    assert_eq!(actual["repetition"], 0);
    assert_eq!(
        actual["terminal"],
        serde_json::json!({
            "live_handles": 0, "live_jobs": 0, "pending_slots": 0, "budget_bytes": 0,
        })
    );
}

#[test]
#[should_panic(expected = "unknown network measurement route")]
fn network_performance_rejects_unknown_route() {
    run("unknown");
}

fn udp_receive_fixture(length: usize, oversized_then_next: bool) -> (u64, Observations) {
    assert!(length <= 64);
    let expected: Vec<_> = (0..length).map(|index| (index % 251) as u8).collect();
    let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let source = peer.local_addr().unwrap();
    let reference_source =
        reference::admission::Address::ipv4([127, 0, 0, 1], source.port().into()).unwrap();
    let reference_limits = reference::admission::Limits::create(128, 64, 16).unwrap();
    assert_eq!(
        reference::admission::receive_datagram(
            &expected,
            reference_source,
            expected.len(),
            reference_limits
        )
        .unwrap(),
        (expected.clone(), reference_source)
    );
    if oversized_then_next {
        assert_eq!(
            reference::admission::receive_datagram(
                &[0xa5; 65],
                reference_source,
                65,
                reference_limits
            ),
            Err(reference::admission::Error::DatagramTooLarge)
        );
    }
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 128, 64, 16);
    fixture.retain_fixture(expected.len(), u64::from(length > 0));
    let receiver = success(fixture.sync("std.net.bind", &[loopback(0)]));
    let address = success(fixture.sync(
        "std.net.UdpSocket.localAddress",
        std::slice::from_ref(&receiver),
    ));
    let destination = transport::host_address(address_input(&address).unwrap());
    if oversized_then_next {
        assert_eq!(peer.send_to(&[0xa5; 65], destination).unwrap(), 65);
    }
    assert_eq!(
        peer.send_to(&expected, destination).unwrap(),
        expected.len()
    );
    fixture.mark_timed_start();
    let started = Instant::now();
    let options = fixture.options.clone();
    if oversized_then_next {
        let handles = fixture.host.next_value;
        let rejected = fixture.invoke_async(
            "std.net.UdpSocket.receiveFrom",
            &[receiver.clone(), options.clone()],
        );
        assert_eq!(
            rejected,
            RuntimeValue::ResultErr(Box::new(RuntimeValue::Variant {
                name: "NetError".into(),
                variant: 12,
                values: vec![], // authored DatagramTooLarge
            }))
        );
        assert_eq!(
            fixture.host.next_value, handles,
            "rejected datagram published a partial handle"
        );
        fixture.observations.rejections += 1;
    }
    let packet = success(fixture.invoke_async(
        "std.net.UdpSocket.receiveFrom",
        &[receiver.clone(), options],
    ));
    let actual_source = fixture.sync("std.net.Datagram.source", std::slice::from_ref(&packet));
    assert_eq!(actual_source, loopback(source.port()));
    let bytes = fixture.sync("std.net.Datagram.bytes", std::slice::from_ref(&packet));
    assert_eq!(fixture.host.bytes(&bytes).unwrap(), expected);
    fixture.observations.bytes_copied += 2 * expected.len() as u64;
    fixture.observations.selected_allocations += 2 * u64::from(!expected.is_empty());
    fixture.retire_buffer(&bytes);
    fixture.retire_buffer(&packet);
    fixture.close("std.net.UdpSocket.close", &receiver);
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    drop(peer);
    (nanos, fixture.terminal())
}

fn pending_accept_cancel_fixture() -> (u64, Observations) {
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 128, 64, 16);
    let listener =
        success(fixture.sync("std.net.listen", &[loopback(0), RuntimeValue::Integer(1)]));
    fixture.mark_timed_start();
    let started = Instant::now();
    let options = fixture.options.clone();
    let call = fixture.start("std.net.TcpListener.accept", &[listener.clone(), options]);
    assert!(fixture.poll(call).is_none());
    fixture.observations.pending_polls += 1;
    fixture.host.cancel_network(call);
    fixture.observations.cancellations += 1;
    assert_eq!(
        fixture
            .host
            .network
            .executor
            .as_ref()
            .unwrap()
            .pending_count(),
        0
    );
    let cancelled = fixture.finish(call);
    assert_eq!(
        cancelled,
        RuntimeValue::ResultErr(Box::new(RuntimeValue::Variant {
            name: "NetError".into(),
            variant: 14,
            values: vec![], // authored Cancelled
        }))
    );
    fixture.observations.rejections += 1;
    fixture.close("std.net.TcpListener.close", &listener);
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    (nanos, fixture.terminal())
}

fn tls_exchange_fixture(
    version: &'static rustls::SupportedProtocolVersion,
    rejected_name: bool,
) -> (u64, Observations) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    use std::io::{Read, Write};

    let mut reference_tls = reference::tls::Tls::new();
    reference_tls.start(true, None, 0).unwrap();
    if rejected_name {
        assert_eq!(
            reference_tls.finish(reference::tls::Verdict::CertificateRejected, 0, false),
            Err(reference::tls::Failure::CertificateRejected)
        );
        assert_eq!(reference_tls.phase(), reference::tls::Phase::Closed);
    } else {
        reference_tls
            .finish(reference::tls::Verdict::Verified, 0, false)
            .unwrap();
        assert_eq!(reference_tls.phase(), reference::tls::Phase::Established);
    }

    let certificate = CertificateDer::from_pem_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/net_provider/fixtures/localhost-cert.pem"
    )))
    .unwrap();
    let key = PrivateKeyDer::from_pem_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/net_provider/fixtures/localhost-test-key.pem"
    )))
    .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[version])
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![certificate.clone()], key)
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let socket = accept_peer(listener);
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut stream = rustls::StreamOwned::new(connection, socket);
        if rejected_name {
            assert!(stream.read(&mut [0; 1]).is_err());
            return;
        }
        let mut bytes = [0; 32];
        stream.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, std::array::from_fn::<_, 32, _>(|index| index as u8));
        stream.write_all(&bytes).unwrap();
        stream.conn.send_close_notify();
        stream.flush().unwrap();
    });
    let mut fixture = Fixture::new("127.0.0.1:9".into(), 128, 64, 16);
    let tcp = connect(&mut fixture, loopback(address.port()));
    let pin = fixture.host.allocate_bytes(certificate.to_vec()).unwrap();
    let verification = RuntimeValue::Variant {
        name: "TlsVerification".into(),
        variant: 1,
        values: vec![pin.clone()],
    };
    let config = success(fixture.sync("std.net.tlsConfig", &[verification]));
    let server = success(
        fixture.sync(
            "std.net.hostName",
            &[RuntimeValue::String(
                if rejected_name {
                    "wrong.test"
                } else {
                    "localhost"
                }
                .into(),
            )],
        ),
    );
    let payload: Vec<_> = (0..32).collect();
    fixture.retain_fixture(certificate.len() + payload.len(), 2);
    fixture.mark_timed_start();
    let started = Instant::now();
    let options = fixture.options.clone();
    let tls = fixture.invoke_async("std.net.TlsStream.connect", &[tcp, server, config, options]);
    if rejected_name {
        assert_eq!(
            tls,
            RuntimeValue::ResultErr(Box::new(RuntimeValue::Variant {
                name: "TlsError".into(),
                variant: 2,
                values: vec![], // authored CertificateRejected
            }))
        );
        fixture.observations.rejections += 1;
    } else {
        let tls = success(tls);
        let (reader, writer) = split(fixture.sync("std.net.TlsStream.split", &[tls]));
        write_all(&mut fixture, &writer, &payload, true);
        let options = fixture.options.clone();
        assert_eq!(
            success(fixture.invoke_async("std.net.TlsWriteHalf.flush", &[writer.clone(), options])),
            RuntimeValue::Unit
        );
        read_exact(&mut fixture, &reader, &payload, true);
        let options = fixture.options.clone();
        let eof = success(fixture.invoke_async(
            "std.net.TlsReadHalf.read",
            &[reader.clone(), RuntimeValue::Integer(1), options],
        ));
        assert_eq!(
            eof,
            RuntimeValue::Variant {
                name: "ReadResult".into(),
                variant: 1,
                values: vec![]
            }
        );
        let options = fixture.options.clone();
        assert_eq!(
            success(
                fixture.invoke_async("std.net.TlsWriteHalf.shutdown", &[writer.clone(), options])
            ),
            RuntimeValue::Unit
        );
        fixture.close("std.net.TlsReadHalf.close", &reader);
        fixture.close("std.net.TlsWriteHalf.close", &writer);
    }
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    fixture.retire_buffer(&pin);
    peer.join().unwrap();
    (nanos, fixture.terminal())
}

fn dns_fixture(max_results: usize, delayed: bool) -> (u64, Observations) {
    use hickory_resolver::proto::{
        op::Message,
        rr::{
            RData, Record, RecordType,
            rdata::{A, AAAA},
        },
    };
    let a = reference::admission::Address::V4([192, 0, 2, 2], 443);
    let b = reference::admission::Address::V4([192, 0, 2, 1], 443);
    let c = reference::admission::Address::V6(Ipv6Addr::LOCALHOST.octets(), 443);
    let expected_model = reference::admission::resolve(
        &[a, b, a, c],
        reference::admission::Limits::create(128, 64, max_results as i128).unwrap(),
    );
    if max_results < 3 {
        assert_eq!(
            expected_model,
            Err(reference::admission::Error::ResourceLimit)
        );
    } else {
        assert_eq!(expected_model.unwrap(), vec![a, b, c]);
    }
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let resolver = socket.local_addr().unwrap().to_string();
    let peer = std::thread::spawn(move || {
        for _ in 0..2 {
            let mut buffer = [0; 2048];
            let (length, destination) = socket.recv_from(&mut buffer).unwrap();
            let request = Message::from_vec(&buffer[..length]).unwrap();
            assert_eq!(request.queries.len(), 1);
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
                            60,
                            RData::A(A(ip)),
                        ));
                    }
                }
                RecordType::AAAA => {
                    reply.add_answer(Record::from_rdata(
                        query.name().clone(),
                        60,
                        RData::AAAA(AAAA(Ipv6Addr::LOCALHOST)),
                    ));
                }
                other => panic!("unexpected explicit DNS query: {other:?}"),
            }
            if delayed {
                std::thread::sleep(Duration::from_millis(5));
            }
            socket
                .send_to(&reply.to_vec().unwrap(), destination)
                .unwrap();
        }
        2
    });
    let mut fixture = Fixture::new(resolver, 128, 64, max_results as i128);
    let server = success(fixture.sync(
        "std.net.hostName",
        &[RuntimeValue::String("example.test".into())],
    ));
    // Ordered A/AAAA and duplicate fixtures are independently authored. The
    // bounded resolver model must validate this vector before any timing.
    let expected = RuntimeValue::ResultOk(Box::new(RuntimeValue::Array(vec![
        address_value(
            SocketAddress::create(
                IpAddress::from_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2))),
                443,
            )
            .unwrap(),
        ),
        address_value(
            SocketAddress::create(
                IpAddress::from_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
                443,
            )
            .unwrap(),
        ),
        address_value(
            SocketAddress::create(IpAddress::from_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)), 443)
                .unwrap(),
        ),
    ])));
    fixture.mark_timed_start();
    let started = Instant::now();
    let options = fixture.options.clone();
    let resolved = fixture.invoke_async(
        "std.net.resolve",
        &[server, RuntimeValue::Integer(443), options],
    );
    if max_results < 3 {
        assert_eq!(
            resolved,
            RuntimeValue::ResultErr(Box::new(RuntimeValue::Variant {
                name: "NetError".into(),
                variant: 5,
                values: vec![], // authored ResourceLimit
            }))
        );
        fixture.observations.rejections += 1;
    } else {
        assert_eq!(resolved, expected);
    }
    let nanos = u64::try_from(started.elapsed().as_nanos().max(1)).unwrap();
    fixture.observations.dns_queries = peer.join().unwrap();
    (nanos, fixture.terminal())
}
