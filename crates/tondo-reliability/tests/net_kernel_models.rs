use tondo_reliability::net_model::admission as reference;

use tondo_stdlib::io::ReadResult;
use tondo_stdlib::net::{
    self, Deadline, HostName, IpAddress, NetLimits, NetOptions, SocketAddress,
};

fn reference_error<T>(value: Result<T, reference::Error>) -> Result<T, String> {
    value.map_err(|error| format!("{error:?}"))
}

fn kernel_error<T>(value: Result<T, net::NetError>) -> Result<T, String> {
    value.map_err(|error| format!("{error:?}"))
}

#[test]
fn every_ascii_mutation_in_names_matches_the_production_kernel() {
    for name in ["a-b.example.", "A0.test", "xn--name.test"] {
        for index in 0..name.len() {
            for byte in 0..=127 {
                let mut text = name.as_bytes().to_vec();
                text[index] = byte;
                let text = String::from_utf8(text).unwrap();
                assert_eq!(
                    reference_error(reference::host_name(&text)),
                    kernel_error(HostName::parse(&text).map(|value| value.to_string())),
                    "name={name}, position={index}, byte={byte}"
                );
            }
        }
    }
    for length in 0..=255 {
        let text = "a".repeat(length);
        assert_eq!(
            reference_error(reference::host_name(&text)),
            kernel_error(HostName::parse(&text).map(|value| value.to_string()))
        );
    }
}

#[test]
fn limit_and_clock_boundary_precedence_matches_the_production_kernel() {
    let values = [
        i128::MIN,
        -1,
        0,
        1,
        64,
        1024,
        1025,
        65535,
        65536,
        67108864,
        67108865,
        i128::MAX,
    ];
    for limit in [1, 128, 67_108_864] {
        let expected = reference::Limits::create(limit, 1, 1).unwrap();
        let actual = NetLimits::create(limit, 1, 1).unwrap();
        for requested in [i128::MIN, -1, 0, 1, limit, limit + 1, i128::MAX] {
            assert_eq!(
                reference_error(expected.request(requested)),
                kernel_error(actual.read_request(requested))
            );
        }
    }
    for read in values {
        for datagram in values {
            for results in values {
                assert_eq!(
                    reference_error(reference::Limits::create(read, datagram, results))
                        .map(|value| (value.read, value.datagram, value.results)),
                    kernel_error(NetLimits::create(read, datagram, results)).map(|value| (
                        value.max_read(),
                        value.max_datagram(),
                        value.max_results()
                    ))
                );
            }
        }
    }
    for domain in [0, 1, 7, u64::MAX] {
        for tick in [
            i128::MIN,
            i64::MIN as i128 - 1,
            i64::MIN as i128,
            -1,
            0,
            1,
            i64::MAX as i128,
            i64::MAX as i128 + 1,
            i128::MAX,
        ] {
            assert_eq!(
                reference_error(reference::Deadline::create(domain, tick))
                    .map(|value| (value.domain, value.tick)),
                kernel_error(Deadline::create(domain, tick))
                    .map(|value| (value.domain(), value.nanos()))
            );
        }
    }
    let deadlines = [
        None,
        Some(reference::Deadline::create(7, -1).unwrap()),
        Some(reference::Deadline::create(7, 10).unwrap()),
    ];
    for expected in deadlines {
        let actual =
            expected.map(|value| Deadline::create(value.domain, value.tick as i128).unwrap());
        let options = NetOptions::create(actual, NetLimits::default(), 7).unwrap();
        for domain in [0, 7, 8] {
            for now in [
                i128::MIN,
                i64::MIN as i128,
                -1,
                0,
                9,
                10,
                11,
                i64::MAX as i128,
                i128::MAX,
            ] {
                for cancelled in [false, true] {
                    assert_eq!(
                        reference_error(reference::before_commit(expected, domain, now, cancelled)),
                        kernel_error(options.before_commit(domain, now, cancelled))
                    );
                }
            }
        }
    }
}

#[test]
fn every_small_io_report_matches_the_production_kernel() {
    for length in 0..=reference::MAX_REFERENCE_BYTES {
        let bytes = (0..length).map(|value| value as u8).collect::<Vec<_>>();
        for requested in 0..=reference::MAX_REFERENCE_BYTES + 1 {
            for eof in [false, true] {
                assert_eq!(
                    reference_error(reference::read(&bytes, requested, eof)),
                    kernel_error(net::tcp_read_result(bytes.clone(), requested, eof)).map(
                        |value| match value {
                            ReadResult::Data(bytes) => reference::Read::Data(bytes),
                            ReadResult::Eof => reference::Read::Eof,
                        }
                    )
                );
            }
            assert_eq!(
                reference_error(reference::write(requested, length)),
                kernel_error(net::tcp_write_result(requested, length))
            );
            assert_eq!(
                reference_error(reference::send_datagram(requested, length)),
                kernel_error(net::udp_send_result(requested, length))
            );
        }
    }
    let expected = reference::Address::ipv4([127, 0, 0, 1], 42).unwrap();
    let actual = SocketAddress::create(IpAddress::parse("127.0.0.1").unwrap(), 42).unwrap();
    for length in 0..=16 {
        let bytes = vec![0; length];
        for wire in 0..=17 {
            for limit in 1..=17 {
                assert_eq!(
                    reference_error(reference::receive_datagram(
                        &bytes,
                        expected,
                        wire,
                        reference::Limits::create(1, limit, 1).unwrap()
                    ))
                    .map(|value| value.0),
                    kernel_error(net::Datagram::receive(
                        bytes.clone(),
                        actual,
                        wire,
                        NetLimits::create(1, limit, 1).unwrap()
                    ))
                    .map(|value| value.into_bytes())
                );
            }
        }
    }
}

#[test]
fn ordered_resolver_snapshots_match_the_production_kernel() {
    let expected = [
        reference::Address::ipv4([192, 0, 2, 2], 443).unwrap(),
        reference::Address::ipv4([192, 0, 2, 1], 443).unwrap(),
        reference::Address::ipv4([192, 0, 2, 2], 0).unwrap(),
    ];
    let actual = [
        SocketAddress::create(IpAddress::parse("192.0.2.2").unwrap(), 443).unwrap(),
        SocketAddress::create(IpAddress::parse("192.0.2.1").unwrap(), 443).unwrap(),
        SocketAddress::create(IpAddress::parse("192.0.2.2").unwrap(), 0).unwrap(),
    ];
    for seed in 0..4096u64 {
        let mut cursor = seed;
        let mut indices = Vec::new();
        for _ in 0..seed % 12 {
            cursor = cursor.wrapping_mul(6364136223846793005).wrapping_add(1);
            indices.push(((cursor >> 32) % 3) as usize);
        }
        let expected_input = indices
            .iter()
            .map(|index| expected[*index])
            .collect::<Vec<_>>();
        let actual_input = indices
            .iter()
            .map(|index| actual[*index])
            .collect::<Vec<_>>();
        for limit in 1..=3 {
            assert_eq!(
                reference_error(reference::resolve(
                    &expected_input,
                    reference::Limits::create(1, 1, limit).unwrap()
                ))
                .map(|addresses| addresses
                    .into_iter()
                    .map(|address| match address {
                        reference::Address::V4(octets, port) => (octets, port),
                        reference::Address::V6(_, _) => unreachable!(),
                    })
                    .collect::<Vec<_>>()),
                kernel_error(net::resolver_results(
                    &actual_input,
                    NetLimits::create(1, 1, limit).unwrap()
                ))
                .map(|addresses| addresses
                    .into_iter()
                    .map(|address| match address.ip().as_ip() {
                        std::net::IpAddr::V4(ip) => (ip.octets(), address.port()),
                        std::net::IpAddr::V6(_) => unreachable!(),
                    })
                    .collect::<Vec<_>>())
            );
        }
    }
}
