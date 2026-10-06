//! Typed UUID replies are admitted before calling an entropy or clock provider.

use super::*;
use crate::uuid_provider::{error, os_entropy, unix_milliseconds};
use std::time::SystemTime;
use tondo_stdlib::uuid::{DEFAULT_MAX_NAME_BYTES, Uuid, UuidError, UuidErrorKind, UuidLimits};

const UUID_REPLY_BYTES: u64 = 5 * tondo_vm::runtime::TEST_DETACHED_VALUE_BYTES + 22;

#[cfg(test)]
mod performance;

fn uuid_value(uuid: Uuid) -> RuntimeValue {
    let bytes = uuid.to_bytes();
    let high = u64::from_be_bytes(bytes[..8].try_into().unwrap());
    let low = u64::from_be_bytes(bytes[8..].try_into().unwrap());
    RuntimeValue::Record {
        name: "Uuid".into(),
        values: vec![
            RuntimeValue::Integer(high.into()),
            RuntimeValue::Integer(low.into()),
        ],
    }
}

fn uuid_input(value: &RuntimeValue) -> Result<Uuid, VmError> {
    let RuntimeValue::Record { name, values } = value else {
        return Err(VmError::Host("UUID value has an invalid shape".into()));
    };
    let [RuntimeValue::Integer(high), RuntimeValue::Integer(low)] = values.as_slice() else {
        return Err(VmError::Host("UUID value has an invalid shape".into()));
    };
    if name != "Uuid" {
        return Err(VmError::Host(
            "UUID value has an invalid nominal type".into(),
        ));
    }
    let high =
        u64::try_from(*high).map_err(|_| VmError::Host("UUID word is out of range".into()))?;
    let low = u64::try_from(*low).map_err(|_| VmError::Host("UUID word is out of range".into()))?;
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&high.to_be_bytes());
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    Ok(Uuid::from_bytes(&bytes).unwrap())
}

fn uuid_error(failure: UuidError) -> RuntimeValue {
    RuntimeValue::Record {
        name: "UuidError".into(),
        values: vec![
            RuntimeValue::Variant {
                name: "UuidErrorKind".into(),
                variant: failure.kind as u32,
                values: Vec::new(),
            },
            failure.offset.map_or(RuntimeValue::OptionNone, |offset| {
                RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(offset as i128)))
            }),
        ],
    }
}

fn uuid_result(result: Result<Uuid, UuidError>) -> RuntimeValue {
    match result {
        Ok(value) => RuntimeValue::ResultOk(Box::new(uuid_value(value))),
        Err(failure) => RuntimeValue::ResultErr(Box::new(uuid_error(failure))),
    }
}

impl BootstrapHost {
    fn uuid_entropy(&self, bytes: &mut [u8]) -> Result<(), UuidError> {
        if bytes.len() as u64 > self.max_bytes || bytes.len() > 16 {
            return Err(error(UuidErrorKind::ResourceLimit));
        }
        if let Some(envelope) = &self.testing
            && let Some(result) = envelope.uuid_entropy(bytes)?
        {
            return result;
        }
        os_entropy(bytes)
    }

    fn uuid_clock(&self) -> Result<i128, UuidError> {
        let now = if let Some(envelope) = &self.testing
            && let Some(result) = envelope.uuid_clock()?
        {
            result?
        } else {
            SystemTime::now()
        };
        unix_milliseconds(now)
    }

    fn uuid_v4(&self) -> Result<Uuid, UuidError> {
        let mut bytes = [0; 16];
        self.uuid_entropy(&mut bytes)?;
        Uuid::v4(&bytes)
    }

    fn uuid_v7(&self) -> Result<Uuid, UuidError> {
        if self.max_bytes < 10 {
            return Err(error(UuidErrorKind::ResourceLimit));
        }
        let milliseconds = self.uuid_clock()?;
        let mut bytes = [0; 10];
        self.uuid_entropy(&mut bytes)?;
        Uuid::v7(milliseconds, &bytes)
    }

    pub(super) fn invoke_uuid_admitted(
        &mut self,
        name: &str,
        arguments: &[RuntimeValue],
        response: &mut VmHostReturnBudget<'_>,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<RuntimeValue, VmError> {
        response.reserve(UUID_REPLY_BYTES, [])?;
        let fallible = matches!(
            name,
            "std.uuid.Uuid.parse"
                | "std.uuid.Uuid.fromBytes"
                | "std.uuid.Uuid.v4"
                | "std.uuid.Uuid.v5"
                | "std.uuid.Uuid.v7"
        );
        let preview = match name {
            "std.uuid.Uuid.toString" => RuntimeValue::String("0".repeat(36)),
            "std.uuid.Uuid.toBytes" => RuntimeValue::Host {
                kind: RuntimeHostValueKind::Bytes,
                id: 0,
            },
            "std.uuid.Uuid.version" | "std.uuid.Uuid.compare" => RuntimeValue::Integer(0),
            "std.uuid.Uuid.isNil" | "std.uuid.Uuid.isMax" => RuntimeValue::Bool(false),
            "std.uuid.Uuid.variant" => RuntimeValue::Variant {
                name: "UuidVariant".into(),
                variant: 0,
                values: Vec::new(),
            },
            _ if fallible => uuid_result(Ok(Uuid::nil())),
            _ => uuid_value(Uuid::nil()),
        };
        let failure = RuntimeValue::ResultErr(Box::new(uuid_error(UuidError {
            kind: UuidErrorKind::InvalidCharacter,
            offset: Some(45),
        })));
        let alternatives = [
            VmHostReturnPreview::Value(&preview),
            VmHostReturnPreview::Value(&failure),
        ];
        let imported = admission.prepare_current(if fallible {
            VmHostReturnPreview::StorageBound(&alternatives)
        } else {
            VmHostReturnPreview::Value(&preview)
        })?;
        admission.commit(&mut [imported])?;
        let value = match (name, arguments) {
            ("std.uuid.Uuid.nil", []) => uuid_value(Uuid::nil()),
            ("std.uuid.Uuid.max", []) => uuid_value(Uuid::max()),
            ("std.uuid.Uuid.parse", [RuntimeValue::String(text)]) => uuid_result(Uuid::parse(text)),
            ("std.uuid.Uuid.fromBytes", [bytes]) => {
                uuid_result(Uuid::from_bytes(self.bytes(bytes)?))
            }
            ("std.uuid.Uuid.toBytes", [uuid]) => {
                let bytes = uuid_input(uuid)?.to_bytes();
                if self.max_bytes < 16 {
                    return Err(VmError::ResourceLimit {
                        resource: "memory",
                        limit: self.max_bytes,
                    });
                }
                let memory = self.reserve_buffer_payloads(std::iter::once(16))?;
                let mut output = Vec::new();
                output
                    .try_reserve_exact(16)
                    .map_err(|_| VmError::ResourceLimit {
                        resource: "memory",
                        limit: self.max_bytes,
                    })?;
                output.extend_from_slice(&bytes);
                self.publish_buffer(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(output),
                    memory,
                )
            }
            ("std.uuid.Uuid.toString", [uuid]) => {
                if self.max_bytes < 36 {
                    return Err(VmError::ResourceLimit {
                        resource: "memory",
                        limit: self.max_bytes,
                    });
                }
                RuntimeValue::String(uuid_input(uuid)?.try_to_string().map_err(|_| {
                    VmError::ResourceLimit {
                        resource: "memory",
                        limit: self.max_bytes,
                    }
                })?)
            }
            ("std.uuid.Uuid.version", [uuid]) => {
                RuntimeValue::Integer(uuid_input(uuid)?.version().into())
            }
            ("std.uuid.Uuid.variant", [uuid]) => RuntimeValue::Variant {
                name: "UuidVariant".into(),
                variant: uuid_input(uuid)?.variant() as u32,
                values: Vec::new(),
            },
            ("std.uuid.Uuid.isNil", [uuid]) => RuntimeValue::Bool(uuid_input(uuid)?.is_nil()),
            ("std.uuid.Uuid.isMax", [uuid]) => RuntimeValue::Bool(uuid_input(uuid)?.is_max()),
            ("std.uuid.Uuid.compare", [left, right]) => {
                RuntimeValue::Integer(uuid_input(left)?.compare(uuid_input(right)?) as i128)
            }
            ("std.uuid.Uuid.v4", []) => uuid_result(self.uuid_v4()),
            ("std.uuid.Uuid.v5", [namespace, name]) => uuid_result(Uuid::v5(
                uuid_input(namespace)?,
                self.bytes(name)?,
                UuidLimits {
                    max_name_bytes: DEFAULT_MAX_NAME_BYTES
                        .min(usize::try_from(self.max_bytes).unwrap_or(usize::MAX)),
                },
            )),
            ("std.uuid.Uuid.v7", []) => uuid_result(self.uuid_v7()),
            _ => return Err(VmError::Host("UUID operation has invalid arguments".into())),
        };
        let descriptor = tondo_vm::runtime::TEST_DETACHED_VALUE_BYTES;
        let bytes = match &value {
            RuntimeValue::Record { .. } => 3 * descriptor + 4,
            RuntimeValue::ResultOk(_) => 4 * descriptor + 4,
            RuntimeValue::ResultErr(failure) => {
                let RuntimeValue::Record { values, .. } = failure.as_ref() else {
                    return Err(VmError::Invariant(
                        "UUID error reply has an invalid shape".into(),
                    ));
                };
                (4 + u64::from(matches!(values.get(1), Some(RuntimeValue::OptionSome(_)))))
                    * descriptor
                    + 22
            }
            RuntimeValue::String(text) => descriptor + text.len() as u64,
            RuntimeValue::Variant { .. } => descriptor + 11,
            RuntimeValue::Host { .. } | RuntimeValue::Bool(_) | RuntimeValue::Integer(_) => {
                descriptor
            }
            _ => return Err(VmError::Invariant("UUID reply has an invalid shape".into())),
        };
        response.shrink(bytes)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_control::{EnvelopeHandle, EnvelopeLimits, UuidTestProviders};
    use std::time::{Duration, UNIX_EPOCH};

    fn configured(
        limit: u64,
        clocks: Vec<Result<SystemTime, UuidErrorKind>>,
        entropy: Vec<Result<Vec<u8>, UuidErrorKind>>,
    ) -> (BootstrapHost, EnvelopeHandle) {
        let envelope = EnvelopeHandle::new("uuid-host", EnvelopeLimits::new(4096, 4096, 4096));
        envelope
            .with_uuid_providers(UuidTestProviders::new(clocks, entropy).unwrap())
            .unwrap();
        let mut host = BootstrapHost::with_max_bytes(Vec::new(), limit);
        host.install_testing_envelope(envelope.clone());
        (host, envelope)
    }

    #[test]
    fn uuid_reply_memory_refusal_precedes_entropy_and_releases_every_charge() {
        let (mut host, _) = configured(1024, vec![], vec![Ok(vec![0xaa; 16])]);
        let short = VmMemoryBudget::new(UUID_REPLY_BYTES - 1);
        assert!(matches!(
            host.invoke_admitted("std.uuid.Uuid.v4", &[], Some(&short)),
            Err(VmError::ResourceLimit {
                resource: "memory",
                ..
            })
        ));
        assert_eq!(short.live_bytes(), 0);
        let exact = VmMemoryBudget::new(UUID_REPLY_BYTES);
        let result = host
            .invoke_admitted("std.uuid.Uuid.v4", &[], Some(&exact))
            .unwrap();
        assert_eq!(result.value, uuid_result(Uuid::v4(&[0xaa; 16])));
        assert_eq!(
            result.memory.as_ref().unwrap().bytes(),
            4 * tondo_vm::runtime::TEST_DETACHED_VALUE_BYTES + 4
        );
        assert!(host.values.is_empty());
        drop(result);
        assert_eq!(exact.live_bytes(), 0);
    }

    #[test]
    fn uuid_production_vm_admits_complete_results_before_consuming_providers() {
        use crate::driver::{BuildTarget, Operation};
        use tondo_vm::runtime::{VmLimits, execute_with_limits};

        for (operation, length) in [("v4", 16), ("v7", 10)] {
            let source =
                format!("import std.uuid\nfn main() {{\n _ = uuid.Uuid.{operation}()\n}}\n");
            let (program, entry) = compile_host_admission_source(
                &source,
                BuildTarget::vm_hosted_capabilities(),
                Operation::Run,
            );
            let mut refused = 0;
            let mut succeeded = 0;
            let limits = (1..=2049)
                .step_by(128)
                .map(|max_heap_bytes| VmLimits {
                    max_heap_bytes,
                    ..VmLimits::default()
                })
                .chain((1..=8).map(|max_heap_objects| VmLimits {
                    max_heap_bytes: 16_384,
                    max_heap_objects,
                    ..VmLimits::default()
                }));
            for limits in limits {
                let (mut host, envelope) =
                    configured(1024, vec![Ok(UNIX_EPOCH)], vec![Ok(vec![0x5a; length])]);
                let result = execute_with_limits(&program, entry, &mut host, limits);
                let mut bytes = vec![0; length];
                match result {
                    Ok(_) => {
                        succeeded += 1;
                        assert_eq!(
                            envelope.uuid_entropy(&mut bytes),
                            Ok(Some(Err(error(UuidErrorKind::EntropyUnavailable))))
                        );
                        if operation == "v7" {
                            assert_eq!(
                                envelope.uuid_clock(),
                                Ok(Some(Err(error(UuidErrorKind::ClockUnavailable))))
                            );
                        }
                    }
                    Err(VmError::ResourceLimit { .. } | VmError::OutOfMemory { .. }) => {
                        refused += 1;
                        assert_eq!(
                            envelope.uuid_entropy(&mut bytes),
                            Ok(Some(Ok(()))),
                            "{operation}/{limits:?}"
                        );
                        assert_eq!(bytes, vec![0x5a; length]);
                        assert_eq!(envelope.uuid_clock(), Ok(Some(Ok(UNIX_EPOCH))));
                    }
                    Err(failure) => panic!("{operation}/{limits:?}: {failure:?}"),
                }
                assert!(host.values.is_empty());
                assert!(host.buffer_memory.is_empty());
                assert!(
                    host.test_memory.is_none(),
                    "ordinary execution must not create a test account"
                );
            }
            assert!(
                refused > 0 && succeeded > 0,
                "{operation}: admission transition was not exercised"
            );
        }
    }

    #[test]
    fn uuid_clock_refusal_and_target_limits_leave_entropy_unconsumed() {
        for clock in [
            Ok(UNIX_EPOCH - Duration::from_nanos(100)),
            Err(UuidErrorKind::ClockUnavailable),
            Err(UuidErrorKind::ClockFailure),
        ] {
            let expected = match clock {
                Ok(_) => UuidErrorKind::TimestampOutOfRange,
                Err(kind) => kind,
            };
            let (mut host, envelope) = configured(1024, vec![clock], vec![Ok(vec![0xbb; 10])]);
            assert_eq!(
                host.invoke("std.uuid.Uuid.v7", &[]).unwrap(),
                uuid_result(Err(error(expected)))
            );
            let mut entropy = [0; 10];
            assert_eq!(envelope.uuid_entropy(&mut entropy), Ok(Some(Ok(()))));
            assert_eq!(entropy, [0xbb; 10]);
        }
        for (operation, limit, length) in [("v4", 15, 16), ("v7", 9, 10)] {
            let (mut host, envelope) =
                configured(limit, vec![Ok(UNIX_EPOCH)], vec![Ok(vec![0xcc; length])]);
            assert_eq!(
                host.invoke(&format!("std.uuid.Uuid.{operation}"), &[])
                    .unwrap(),
                uuid_result(Err(error(UuidErrorKind::ResourceLimit)))
            );
            assert_eq!(envelope.uuid_clock(), Ok(Some(Ok(UNIX_EPOCH))));
            let mut bytes = vec![0; length];
            assert_eq!(envelope.uuid_entropy(&mut bytes), Ok(Some(Ok(()))));
        }
    }

    #[test]
    fn uuid_entropy_failures_are_atomic_and_recovery_has_no_retained_provider_state() {
        let (mut host, _) = configured(
            1024,
            vec![],
            vec![
                Err(UuidErrorKind::EntropyFailure),
                Ok(vec![0; 9]),
                Ok(vec![0xdd; 16]),
            ],
        );
        for kind in [
            UuidErrorKind::EntropyFailure,
            UuidErrorKind::ProviderMisconfigured,
        ] {
            assert_eq!(
                host.invoke("std.uuid.Uuid.v4", &[]).unwrap(),
                uuid_result(Err(error(kind)))
            );
            assert!(host.values.is_empty());
        }
        assert_eq!(
            host.invoke("std.uuid.Uuid.v4", &[]).unwrap(),
            uuid_result(Uuid::v4(&[0xdd; 16]))
        );
        assert_eq!(
            host.invoke("std.uuid.Uuid.v4", &[]).unwrap(),
            uuid_result(Err(error(UuidErrorKind::EntropyUnavailable)))
        );
        assert!(host.values.is_empty());
    }

    #[test]
    fn uuid_v7_preserves_clock_regressions_without_monotonic_state() {
        let (mut host, _) = configured(
            1024,
            vec![
                Ok(UNIX_EPOCH + Duration::from_millis(45)),
                Ok(UNIX_EPOCH + Duration::from_millis(44)),
            ],
            vec![Ok(vec![0xaa; 10]), Ok(vec![0xbb; 10])],
        );
        assert_eq!(
            host.invoke("std.uuid.Uuid.v7", &[]).unwrap(),
            uuid_result(Uuid::v7(45, &[0xaa; 10]))
        );
        assert_eq!(
            host.invoke("std.uuid.Uuid.v7", &[]).unwrap(),
            uuid_result(Uuid::v7(44, &[0xbb; 10]))
        );
        assert!(host.values.is_empty());
    }

    #[test]
    fn uuid_typed_carrier_preserves_all_bits_and_rejects_invalid_shapes() {
        for index in 0..128 {
            let mut bytes = [0; 16];
            bytes[index / 8] = 1 << (index % 8);
            let uuid = Uuid::from_bytes(&bytes).unwrap();
            assert_eq!(uuid_input(&uuid_value(uuid)).unwrap(), uuid);
        }
        assert_eq!(uuid_input(&uuid_value(Uuid::max())).unwrap(), Uuid::max());
        for value in [
            RuntimeValue::Integer(0),
            RuntimeValue::Record {
                name: "Fake".into(),
                values: vec![RuntimeValue::Integer(0), RuntimeValue::Integer(0)],
            },
            RuntimeValue::Record {
                name: "Uuid".into(),
                values: vec![],
            },
            RuntimeValue::Record {
                name: "Uuid".into(),
                values: vec![RuntimeValue::Integer(-1), RuntimeValue::Integer(0)],
            },
            RuntimeValue::Record {
                name: "Uuid".into(),
                values: vec![
                    RuntimeValue::Integer(0),
                    RuntimeValue::Integer(1_i128 << 64),
                ],
            },
        ] {
            assert!(matches!(uuid_input(&value), Err(VmError::Host(_))));
        }
    }

    #[test]
    fn uuid_error_replies_fit_the_prepaid_nominal_envelope() {
        for text in ["bad", "!0000000-0000-0000-0000-000000000000"] {
            let mut host = BootstrapHost::default();
            let budget = VmMemoryBudget::new(UUID_REPLY_BYTES);
            let result = host
                .invoke_admitted(
                    "std.uuid.Uuid.parse",
                    &[RuntimeValue::String(text.into())],
                    Some(&budget),
                )
                .unwrap();
            assert_eq!(result.value, uuid_result(Uuid::parse(text)));
            assert!(result.memory.as_ref().unwrap().bytes() <= UUID_REPLY_BYTES);
            drop(result);
            assert_eq!(budget.live_bytes(), 0);
        }
    }
}
