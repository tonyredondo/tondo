#![no_main]

use libfuzzer_sys::fuzz_target;
use tondo_stdlib::net::{self, Datagram, HostName, IpAddress, NetLimits, SocketAddress};

#[path = "../../crates/tondo-reliability/src/net_model.rs"]
mod model;

fn reference<T>(result: Result<T, model::admission::Error>) -> Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

fn kernel<T>(result: Result<T, net::NetError>) -> Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

fuzz_target!(|input: &[u8]| {
    let input = &input[..input.len().min(model::MAX_INPUT)];
    let summary = model::replay(input);
    assert_eq!(summary, model::replay(input));
    assert!((1..=model::MAX_STEPS).contains(&summary.steps));
    // TLS verdicts are explicit model inputs, never a certificate oracle.
    // Real TLS 1.2/1.3 and certificate refusal are exercised by hosted tests.
    {
        use model::tls::{Phase, Tls, Verdict};
        let mut tls = Tls::new();
        let first = input.first().copied().unwrap_or(0);
        if tls.start(first & 1 == 0, None, 0).is_ok() {
            let verdict = match first % 3 {
                0 => Verdict::Verified,
                1 => Verdict::CertificateRejected,
                _ => Verdict::HandshakeFailed,
            };
            if tls.finish(verdict, 0, first & 2 != 0).is_ok() {
                tls.split().unwrap();
                tls.close_half(true).unwrap();
                assert!(tls.transport_live());
                tls.close_half(false).unwrap();
            }
        }
        assert_eq!(tls.phase(), Phase::Closed);
        assert!(!tls.transport_live());
        tls.close_scope();
    }
    let body = input.get(1..).unwrap_or_default();
    let byte = |index| body.get(index).copied().unwrap_or(0);
    match input.first().copied().unwrap_or(0) % 7 {
        0 => {
            let text = String::from_utf8_lossy(body);
            assert_eq!(
                reference(model::admission::host_name(&text)),
                kernel(HostName::parse(&text).map(|value| value.to_string()))
            );
        }
        1 => {
            let values = [byte(0), byte(1), byte(2)].map(|value| i128::from(value as i8));
            let expected = model::admission::Limits::create(values[0], values[1], values[2]);
            let actual = NetLimits::create(values[0], values[1], values[2]);
            assert_eq!(
                reference(expected).map(|value| (value.read, value.datagram, value.results)),
                kernel(actual).map(|value| (
                    value.max_read(),
                    value.max_datagram(),
                    value.max_results()
                ))
            );
        }
        2 => {
            let requested = byte(0) as usize;
            let reported = byte(1) as usize;
            assert_eq!(
                reference(model::admission::write(requested, reported)),
                kernel(net::tcp_write_result(requested, reported))
            );
            assert_eq!(
                reference(model::admission::send_datagram(requested, reported)),
                kernel(net::udp_send_result(requested, reported))
            );
        }
        3 => {
            let requested = byte(0) as usize;
            let eof = byte(1) & 1 != 0;
            let bytes = body.get(2..).unwrap_or_default();
            let bytes = &bytes[..bytes.len().min(model::admission::MAX_REFERENCE_BYTES)];
            assert_eq!(
                reference(model::admission::read(bytes, requested, eof)),
                kernel(net::tcp_read_result(bytes.to_vec(), requested, eof)).map(
                    |value| match value {
                        tondo_stdlib::io::ReadResult::Data(bytes) =>
                            model::admission::Read::Data(bytes),
                        tondo_stdlib::io::ReadResult::Eof => model::admission::Read::Eof,
                    }
                )
            );
        }
        4 => {
            let limit = 1 + byte(0) as i128;
            let wire = byte(1) as usize;
            let port = byte(2) as i128;
            let bytes = body.get(3..).unwrap_or_default();
            let bytes = &bytes[..bytes.len().min(model::admission::MAX_REFERENCE_BYTES)];
            let source = model::admission::Address::ipv4([127, 0, 0, 1], port).unwrap();
            let actual_source =
                SocketAddress::create(IpAddress::parse("127.0.0.1").unwrap(), port).unwrap();
            assert_eq!(
                reference(model::admission::receive_datagram(
                    bytes,
                    source,
                    wire,
                    model::admission::Limits::create(1, limit, 1).unwrap()
                ))
                .map(|value| value.0),
                kernel(Datagram::receive(
                    bytes.to_vec(),
                    actual_source,
                    wire,
                    NetLimits::create(1, limit, 1).unwrap()
                ))
                .map(Datagram::into_bytes)
            );
        }
        5 => {
            let mut ticks = [0; 16];
            let length = body.len().saturating_sub(3).min(16);
            ticks[..length].copy_from_slice(body.get(3..3 + length).unwrap_or_default());
            let tick = i128::from_be_bytes(ticks);
            let domain = u64::from(byte(0));
            let active = u64::from(byte(1));
            let cancelled = byte(2) & 1 != 0;
            let now = i128::from(byte(2) as i8);
            let expected = model::admission::Deadline::create(domain, tick).and_then(|deadline| {
                model::admission::before_commit(Some(deadline), active, now, cancelled)
            });
            let actual = net::Deadline::create(domain, tick)
                .and_then(|deadline| {
                    net::NetOptions::create(Some(deadline), NetLimits::default(), domain)
                })
                .and_then(|options| options.before_commit(active, now, cancelled));
            assert_eq!(reference(expected), kernel(actual));
        }
        6 => {
            use model::admission::Address;
            let limit = 1 + byte(0) as i128 % 32;
            let addresses = body
                .get(1..)
                .unwrap_or_default()
                .chunks_exact(2)
                .take(32)
                .map(|row| {
                    let octets = [192, 0, 2, row[0] & 3];
                    if row[0] & 4 == 0 {
                        Address::V4(octets, u16::from(row[1]))
                    } else {
                        let mut mapped = [0; 16];
                        mapped[10..].copy_from_slice(&[
                            255, 255, octets[0], octets[1], octets[2], octets[3],
                        ]);
                        Address::V6(mapped, u16::from(row[1]))
                    }
                })
                .collect::<Vec<_>>();
            let actual_input = addresses
                .iter()
                .map(|address| {
                    let (ip, port) = match address {
                        Address::V4(bytes, port) => (
                            std::net::IpAddr::V4(std::net::Ipv4Addr::from(*bytes)),
                            *port,
                        ),
                        Address::V6(bytes, port) => (
                            std::net::IpAddr::V6(std::net::Ipv6Addr::from(*bytes)),
                            *port,
                        ),
                    };
                    SocketAddress::create(IpAddress::from_ip(ip), i128::from(port)).unwrap()
                })
                .collect::<Vec<_>>();
            let expected = model::admission::resolve(
                &addresses,
                model::admission::Limits::create(1, 1, limit).unwrap(),
            );
            let actual = net::resolver_results(
                &actual_input,
                NetLimits::create(1, 1, limit).unwrap(),
            )
            .map(|values| {
                values
                    .into_iter()
                    .map(|address| match address.ip().as_ip() {
                        std::net::IpAddr::V4(ip) => Address::V4(ip.octets(), address.port()),
                        std::net::IpAddr::V6(ip) => Address::V6(ip.octets(), address.port()),
                    })
                    .collect::<Vec<_>>()
            });
            assert_eq!(reference(expected), kernel(actual));
        }
        _ => unreachable!(),
    }
});
