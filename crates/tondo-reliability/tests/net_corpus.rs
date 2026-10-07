use tondo_reliability::net_model::admission as reference;

use serde_json::{Value, json};
use tondo_stdlib::net::{self, Datagram, HostName, IpAddress, NetLimits, SocketAddress};

fn outcome<T, E: std::fmt::Debug>(value: Result<T, E>) -> Result<T, String> {
    value.map_err(|error| format!("{error:?}"))
}

fn number(row: &Value, key: &str) -> i128 {
    i128::from(row[key].as_i64().unwrap())
}

fn bytes(row: &Value) -> Vec<u8> {
    let text = row["bytes_hex"].as_str().unwrap();
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn evaluate(row: &Value, independent: bool) -> Result<Value, String> {
    match row["operation"].as_str().unwrap() {
        "hostname" => {
            let text = row["text"].as_str().unwrap();
            if independent {
                outcome(reference::host_name(text)).map(|value| json!(value))
            } else {
                outcome(HostName::parse(text)).map(|value| json!(value.to_string()))
            }
        }
        "limits" => {
            let (read, datagram, results) = (
                number(row, "read"),
                number(row, "datagram"),
                number(row, "results"),
            );
            if independent {
                outcome(reference::Limits::create(read, datagram, results))
                    .map(|value| json!([value.read, value.datagram, value.results]))
            } else {
                outcome(NetLimits::create(read, datagram, results)).map(|value| {
                    json!([value.max_read(), value.max_datagram(), value.max_results()])
                })
            }
        }
        "read" => {
            let bytes = bytes(row);
            let requested = usize::try_from(number(row, "requested")).unwrap();
            let eof = row["eof"].as_bool().unwrap();
            let value = if independent {
                outcome(reference::read(&bytes, requested, eof))
            } else {
                outcome(net::tcp_read_result(bytes, requested, eof)).map(|value| match value {
                    tondo_stdlib::io::ReadResult::Data(bytes) => reference::Read::Data(bytes),
                    tondo_stdlib::io::ReadResult::Eof => reference::Read::Eof,
                })
            }?;
            Ok(match value {
                reference::Read::Data(bytes) => json!({"kind":"Data","bytes_hex":hex(&bytes)}),
                reference::Read::Eof => json!({"kind":"Eof"}),
            })
        }
        "write" | "udp-send" => {
            let requested = usize::try_from(number(row, "requested")).unwrap();
            let reported = usize::try_from(number(row, "reported")).unwrap();
            if row["operation"] == "write" {
                if independent {
                    outcome(reference::write(requested, reported)).map(|value| json!(value))
                } else {
                    outcome(net::tcp_write_result(requested, reported)).map(|value| json!(value))
                }
            } else if independent {
                outcome(reference::send_datagram(requested, reported)).map(|()| json!(true))
            } else {
                outcome(net::udp_send_result(requested, reported)).map(|()| json!(true))
            }
        }
        "datagram" => {
            let bytes = bytes(row);
            let port = number(row, "port");
            let wire = usize::try_from(number(row, "wire_length")).unwrap();
            let limit = number(row, "limit");
            if independent {
                let source = reference::Address::ipv4([127, 0, 0, 1], port).unwrap();
                outcome(reference::receive_datagram(
                    &bytes,
                    source,
                    wire,
                    reference::Limits::create(1, limit, 1).unwrap(),
                ))
                .map(|value| json!(hex(&value.0)))
            } else {
                let source =
                    SocketAddress::create(IpAddress::parse("127.0.0.1").unwrap(), port).unwrap();
                outcome(Datagram::receive(
                    bytes,
                    source,
                    wire,
                    NetLimits::create(1, limit, 1).unwrap(),
                ))
                .map(|value| json!(hex(value.bytes())))
            }
        }
        "deadline" => {
            let domain = row["domain"].as_u64().unwrap();
            let active = row["active"].as_u64().unwrap();
            let now = number(row, "now");
            let cancelled = row["cancelled"].as_bool().unwrap();
            if independent {
                let deadline = if row["tick"].is_null() {
                    Ok(None)
                } else {
                    reference::Deadline::create(domain, number(row, "tick")).map(Some)
                };
                outcome(deadline.and_then(|deadline| {
                    reference::before_commit(deadline, active, now, cancelled)
                }))
                .map(|()| json!(true))
            } else {
                let deadline = if row["tick"].is_null() {
                    Ok(None)
                } else {
                    net::Deadline::create(domain, number(row, "tick")).map(Some)
                };
                outcome(
                    deadline
                        .and_then(|deadline| {
                            net::NetOptions::create(deadline, NetLimits::default(), domain)
                        })
                        .and_then(|options| options.before_commit(active, now, cancelled)),
                )
                .map(|()| json!(true))
            }
        }
        "dns" => {
            let addresses = row["addresses"]
                .as_array()
                .unwrap()
                .iter()
                .map(|address| {
                    reference::Address::V4(
                        [192, 0, 2, address[0].as_u64().unwrap() as u8],
                        address[1].as_u64().unwrap() as u16,
                    )
                })
                .collect::<Vec<_>>();
            let limit = number(row, "limit");
            if independent {
                outcome(reference::resolve(
                    &addresses,
                    reference::Limits::create(1, 1, limit).unwrap(),
                ))
            } else {
                let input = addresses
                    .iter()
                    .map(|address| {
                        let reference::Address::V4(bytes, port) = *address else {
                            unreachable!()
                        };
                        SocketAddress::create(
                            IpAddress::from_ip(std::net::Ipv4Addr::from(bytes).into()),
                            i128::from(port),
                        )
                        .unwrap()
                    })
                    .collect::<Vec<_>>();
                outcome(net::resolver_results(
                    &input,
                    NetLimits::create(1, 1, limit).unwrap(),
                ))
                .map(|values| {
                    values
                        .into_iter()
                        .map(|value| {
                            let std::net::IpAddr::V4(ip) = value.ip().as_ip() else {
                                unreachable!()
                            };
                            reference::Address::V4(ip.octets(), value.port())
                        })
                        .collect()
                })
            }
            .map(|addresses| {
                json!(
                    addresses
                        .into_iter()
                        .map(|address| {
                            let reference::Address::V4(bytes, port) = address else {
                                unreachable!()
                            };
                            (bytes[3], port)
                        })
                        .collect::<Vec<_>>()
                )
            })
        }
        other => panic!("unknown retained operation {other}"),
    }
}

#[test]
fn retained_cases_match_authored_results_reference_and_kernel() {
    let corpus: Value = serde_json::from_str(include_str!("fixtures/net-cases.json")).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 40);
    let mut ids = std::collections::BTreeSet::new();
    for row in cases {
        assert!(ids.insert(row["id"].as_str().unwrap()));
        assert_ne!(row.get("value").is_some(), row.get("error").is_some());
        let expected = if let Some(error) = row.get("error") {
            Err(error.as_str().unwrap().to_owned())
        } else {
            Ok(row["value"].clone())
        };
        assert_eq!(evaluate(row, true), expected, "reference {}", row["id"]);
        assert_eq!(evaluate(row, false), expected, "kernel {}", row["id"]);
    }
}
