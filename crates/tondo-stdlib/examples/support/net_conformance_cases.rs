//! Common finite networking observations; no native networking provider or ABI.
use tondo_stdlib::net;
#[allow(dead_code)]
#[path = "../../../tondo-reliability/src/net_model.rs"]
pub mod oracle;

pub const CASES: [&str; 4] = [
    "values-and-keys",
    "tcp-transcript",
    "udp-transcript",
    "dns-transcript",
];

pub fn literal(text: &str) -> String {
    let mut result = String::from("\"");
    for scalar in text.chars() {
        match scalar {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '{' => result.push_str("{{"),
            '}' => result.push_str("}}"),
            '\0' => result.push_str("\\u{0}"),
            value => result.push(value),
        }
    }
    result.push('"');
    result
}

pub fn values() -> (String, Vec<String>) {
    let names = [
        ("hostname-root-dot", "EXample.test.", None),
        ("hostname-punycode-opaque", "xn--name.test", None),
        ("hostname-empty", "", Some("InvalidHostName")),
        (
            "hostname-leading-hyphen",
            "-a.test",
            Some("InvalidHostName"),
        ),
        (
            "hostname-trailing-hyphen",
            "a-.test",
            Some("InvalidHostName"),
        ),
        ("hostname-empty-label", "a..test", Some("InvalidHostName")),
        ("hostname-nul", "a\0b.test", Some("InvalidHostName")),
        ("hostname-no-idna", "é.test", Some("InvalidHostName")),
    ];
    let limits = [
        ("limits-target-ceilings", [67108864, 65535, 1024], None),
        ("limits-zero-first", [0, 65536, 1025], Some("InvalidLimit")),
        (
            "limits-read-over-ceiling",
            [67108865, 1, 1],
            Some("ResourceLimit"),
        ),
        (
            "limits-datagram-over-ceiling",
            [1, 65536, 1],
            Some("ResourceLimit"),
        ),
        (
            "limits-results-over-ceiling",
            [1, 1, 1025],
            Some("ResourceLimit"),
        ),
    ];
    let mut expected = Vec::new();
    let mut source = String::from(
        "import std.net\nimport std.testing\nfn errorName(error: net.NetError): String {\n match error {\n",
    );
    for variant in tondo_stdlib::net::ERROR_VARIANTS {
        source.push_str(&format!(" net.NetError.{variant} => \"{variant}\"\n"));
    }
    source.push_str(" }\n}\nfn requireNet[T](result: T ! net.NetError): T {\n match result {\n ok(item) => item\n err(_) => testing.failNow(\"network value refused\")\n }\n}\nfn identity[T: Copy + Discard + Equatable + Key + Send + Share](value: T): T { value }\ntest conformance {\n");
    for (id, text, error) in names {
        let actual = tondo_stdlib::net::HostName::parse(text);
        assert_eq!(
            actual
                .as_ref()
                .map(ToString::to_string)
                .map_err(|e| format!("{e:?}")),
            oracle::admission::host_name(text).map_err(|e| format!("{e:?}"))
        );
        if let Some(error) = error {
            assert_eq!(format!("{:?}", actual.unwrap_err()), error);
            expected.push(format!("{id}|err:{error}"));
        } else {
            assert_eq!(actual.unwrap().to_string(), text);
            expected.push(format!("{id}|ok"));
        }
        source.push_str(&format!(
            "match net.hostName({}) {{\n ok(value) => {{ _ = value\n testing.log(\"{id}|ok\") }}\n err(failure) => testing.log(\"{id}|err:{{errorName(failure)}}\")\n}}\n",
            literal(text)
        ));
    }
    for (id, values, error) in limits {
        let actual = tondo_stdlib::net::NetLimits::create(values[0], values[1], values[2]);
        let reference = oracle::admission::Limits::create(values[0], values[1], values[2]);
        assert_eq!(
            actual
                .as_ref()
                .map(|v| [v.max_read(), v.max_datagram(), v.max_results()])
                .map_err(|e| format!("{e:?}")),
            reference
                .map(|v| [v.read, v.datagram, v.results])
                .map_err(|e| format!("{e:?}"))
        );
        if let Some(error) = error {
            assert_eq!(format!("{:?}", actual.unwrap_err()), error);
            expected.push(format!("{id}|err:{error}"));
        } else {
            let actual = actual.unwrap();
            assert_eq!(
                [
                    actual.max_read(),
                    actual.max_datagram(),
                    actual.max_results()
                ],
                values.map(|value| usize::try_from(value).unwrap())
            );
            expected.push(format!("{id}|ok"));
        }
        source.push_str(&format!(
            "match net.NetLimits.create({}, {}, {}) {{\n ok(value) => {{ _ = value\n testing.log(\"{id}|ok\") }}\n err(failure) => testing.log(\"{id}|err:{{errorName(failure)}}\")\n}}\n",
            values[0], values[1], values[2]
        ));
    }
    for (id, text, error) in [
        ("ip-loopback-v4", "127.0.0.1", None),
        ("ip-loopback-v6", "::1", None),
        ("ip-mapped-v6", "::ffff:127.0.0.1", None),
        ("ip-expanded-v6", "0:0:0:0:0:0:0:1", None),
        (
            "ip-name-not-resolution",
            "example.test",
            Some("InvalidAddress"),
        ),
        ("ip-scope-refusal", "fe80::1%1", Some("InvalidAddress")),
        ("ip-brackets-refusal", "[::1]", Some("InvalidAddress")),
        ("ip-leading-space", " 127.0.0.1", Some("InvalidAddress")),
    ] {
        let result = tondo_stdlib::net::IpAddress::parse(text);
        let outcome = match result {
            Ok(_) => {
                assert!(error.is_none());
                "ok".to_owned()
            }
            Err(actual) => {
                assert_eq!(Some(format!("{actual:?}")), error.map(str::to_owned));
                format!("err:{actual:?}")
            }
        };
        expected.push(format!("{id}|{outcome}"));
        source.push_str(&format!(
            "match net.IpAddress.parse({}) {{\n ok(value) => {{ _ = value\n testing.log(\"{id}|ok\") }}\n err(failure) => testing.log(\"{id}|err:{{errorName(failure)}}\")\n}}\n", literal(text)
        ));
    }
    let loopback = tondo_stdlib::net::IpAddress::parse("127.0.0.1").unwrap();
    for (id, port) in [
        ("port-zero", 0),
        ("port-maximum", 65535),
        ("port-negative", -1),
        ("port-overflow", 65536),
    ] {
        let outcome = match tondo_stdlib::net::SocketAddress::create(loopback, port) {
            Ok(address) => {
                assert_eq!(i128::from(address.port()), port);
                "ok".to_owned()
            }
            Err(error) => {
                assert_eq!(error, tondo_stdlib::net::NetError::InvalidPort);
                format!("err:{error:?}")
            }
        };
        expected.push(format!("{id}|{outcome}"));
        source.push_str(&format!(
            "match net.IpAddress.parse(\"127.0.0.1\") {{\n ok(ip) => match net.socketAddress(ip, {port}) {{\n ok(value) => {{ _ = value\n testing.log(\"{id}|ok\") }}\n err(failure) => testing.log(\"{id}|err:{{errorName(failure)}}\")\n}}\n err(_) => panic(\"authored loopback refused\")\n}}\n"
        ));
    }
    let first = tondo_stdlib::net::HostName::parse("EXample.test.").unwrap();
    let lower = tondo_stdlib::net::HostName::parse("example.test.").unwrap();
    assert_ne!(first, lower);
    let hosts = std::collections::HashMap::from([(first, 17)]);
    assert_eq!(hosts.get(&first), Some(&17));
    assert_ne!(
        loopback,
        tondo_stdlib::net::IpAddress::parse("::ffff:127.0.0.1").unwrap()
    );
    assert_eq!(
        tondo_stdlib::net::IpAddress::parse("::1").unwrap(),
        tondo_stdlib::net::IpAddress::parse("0:0:0:0:0:0:0:1").unwrap()
    );
    let addresses = std::collections::HashMap::from([(loopback, 19)]);
    assert_eq!(addresses.get(&loopback), Some(&19));
    let socket = tondo_stdlib::net::SocketAddress::create(loopback, 0).unwrap();
    let sockets = std::collections::HashMap::from([(socket, 23)]);
    assert_eq!(sockets.get(&socket), Some(&23));
    source.push_str(r#"
 let first = identity(requireNet(net.hostName("EXample.test.")))
 let lower = requireNet(net.hostName("example.test."))
 assert(first != lower)
 let hosts: Map[net.HostName, Int] = [first: 17]
 assert(hosts[identity(first)] == some(17))
 testing.log("host-key:17")
 let ip = identity(requireNet(net.IpAddress.parse("127.0.0.1")))
 let mapped = requireNet(net.IpAddress.parse("::ffff:127.0.0.1"))
 assert(ip != mapped)
 assert(requireNet(net.IpAddress.parse("::1")) == requireNet(net.IpAddress.parse("0:0:0:0:0:0:0:1")))
 let addresses: Map[net.IpAddress, Int] = [ip: 19]
 assert(addresses[identity(ip)] == some(19))
 testing.log("ip-key:19")
 let socket = identity(requireNet(net.socketAddress(ip, 0)))
 let sockets: Map[net.SocketAddress, Int] = [socket: 23]
 assert(sockets[identity(socket)] == some(23))
 testing.log("socket-key:23")
}
"#);
    expected.extend(["host-key:17", "ip-key:19", "socket-key:23"].map(str::to_owned));
    (source, expected)
}

pub fn kernel(id: &str) -> Vec<String> {
    let limits = net::NetLimits::create(8, 4, 3).unwrap();
    let reference_limits = oracle::admission::Limits::create(8, 4, 3).unwrap();
    match id {
        "values-and-keys" => values().1,
        "tcp-transcript" => {
            let mut model = oracle::stream::Stream::new(128).unwrap();
            model.feed(b"a").unwrap();
            model.feed(b"bc").unwrap();
            model.peer_fin();
            let mut observations = Vec::new();
            for (index, byte) in b"abc".iter().enumerate() {
                let reservation = model.prepare(1, reference_limits, None, 7, 0).unwrap();
                assert_eq!(
                    model.finish(reservation, true, false, 0),
                    Ok(oracle::stream::Completion::Committed(
                        oracle::admission::Read::Data(vec![*byte])
                    ))
                );
                assert_eq!(
                    net::tcp_read_result(vec![*byte], 1, false).unwrap(),
                    tondo_stdlib::io::ReadResult::Data(vec![*byte])
                );
                observations.push(format!("tcp:{index}:data:{byte:02x}"));
            }
            let reservation = model.prepare(1, reference_limits, None, 7, 0).unwrap();
            assert_eq!(
                model.finish(reservation, true, false, 0),
                Ok(oracle::stream::Completion::Committed(
                    oracle::admission::Read::Eof
                ))
            );
            assert_eq!(
                net::tcp_read_result(vec![], 1, true).unwrap(),
                tondo_stdlib::io::ReadResult::Eof
            );
            observations.push("tcp:3:eof".into());
            for (index, byte) in b"xyz".iter().enumerate() {
                assert_eq!(model.write(&[*byte], 1), Ok(Some(1)));
                assert_eq!(net::tcp_write_result(1, 1), Ok(1));
                observations.push(format!("tcp:write:{index}:1"));
            }
            model.shutdown_writer().unwrap();
            model.close_scope();
            assert!(!model.snapshot().transport_live);
            assert_eq!(model.snapshot().sent, b"xyz");
            observations
        }
        "udp-transcript" => {
            let address =
                net::SocketAddress::create(net::IpAddress::parse("127.0.0.1").unwrap(), 1234)
                    .unwrap();
            let reference_address = oracle::admission::Address::ipv4([127, 0, 0, 1], 1234).unwrap();
            [b"abc".as_slice(), b"", b"12345", b"ok"]
                .into_iter()
                .enumerate()
                .map(|(index, bytes)| {
                    let reference = oracle::admission::receive_datagram(
                        bytes,
                        reference_address,
                        bytes.len(),
                        reference_limits,
                    );
                    let actual =
                        net::Datagram::receive(bytes.to_vec(), address, bytes.len(), limits);
                    match reference {
                        Ok((bytes, _)) => {
                            let actual = actual.unwrap();
                            assert_eq!(actual.source(), address);
                            assert_eq!(actual.bytes(), bytes);
                            let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
                            format!("udp:{index}:ok:{hex}")
                        }
                        Err(oracle::admission::Error::DatagramTooLarge) => {
                            assert_eq!(actual.unwrap_err(), net::NetError::DatagramTooLarge);
                            format!("udp:{index}:err:DatagramTooLarge")
                        }
                        other => panic!("unexpected reference datagram {other:?}"),
                    }
                })
                .collect()
        }
        "dns-transcript" => {
            let a = oracle::admission::Address::ipv4([192, 0, 2, 2], 443).unwrap();
            let b = oracle::admission::Address::ipv4([192, 0, 2, 1], 443).unwrap();
            let mut ip6 = [0; 16];
            ip6[15] = 1;
            let c = oracle::admission::Address::V6(ip6, 443);
            let addresses = ["192.0.2.2", "192.0.2.1", "192.0.2.2", "::1"].map(|ip| {
                net::SocketAddress::create(net::IpAddress::parse(ip).unwrap(), 443).unwrap()
            });
            assert_eq!(
                oracle::admission::resolve(&[a, b, a, c], reference_limits).unwrap(),
                [a, b, c]
            );
            assert_eq!(
                net::resolver_results(&addresses, limits).unwrap(),
                [addresses[0], addresses[1], addresses[3]]
            );
            assert_eq!(
                oracle::admission::resolve(
                    &[a, b, a, c],
                    oracle::admission::Limits::create(8, 4, 2).unwrap()
                ),
                Err(oracle::admission::Error::ResourceLimit)
            );
            assert_eq!(
                net::resolver_results(&addresses, net::NetLimits::create(8, 4, 2).unwrap()),
                Err(net::NetError::ResourceLimit)
            );
            vec![
                "dns:2:err:ResourceLimit".into(),
                "dns:3:ordered-v4-v6".into(),
            ]
        }
        unknown => panic!("unregistered network case {unknown}"),
    }
}

pub fn encoded_observations(observations: &[String]) -> String {
    // Report observations are authored ASCII tags, not arbitrary provider text.
    observations
        .iter()
        .map(|line| {
            assert!(
                line.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b":|-".contains(&byte))
            );
            format!("\"{line}\"")
        })
        .collect::<Vec<_>>()
        .join(",")
}
