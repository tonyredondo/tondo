//! Log construction, record formatting and explicit writer retirement.
//! Scratch and typed results are admitted before publishing or retiring handles.

use super::*;
use tondo_stdlib::log::{
    Fields, LogError, LogEvent, LogFormat, LogLevel, LogLimits, LogOperation, LogValue,
};

fn limits(value: &RuntimeValue) -> Result<LogLimits, LogError> {
    let RuntimeValue::Record { name, values } = value else {
        return Err(LogError::Host);
    };
    if name != "LogLimits" || values.len() != 6 {
        return Err(LogError::Host);
    }
    let mut numbers = [0; 6];
    for (number, value) in numbers.iter_mut().zip(values) {
        let RuntimeValue::Integer(value) = value else {
            return Err(LogError::Host);
        };
        *number = usize::try_from(*value).map_err(|_| LogError::InvalidLimit)?;
    }
    LogLimits::create(
        numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
    )
}

fn entries(value: &RuntimeValue) -> Result<&[RuntimeValue], LogError> {
    match value {
        RuntimeValue::Record { name, values } if name == "Fields" => match values.as_slice() {
            [RuntimeValue::Array(entries)] => Ok(entries),
            _ => Err(LogError::Host),
        },
        _ => Err(LogError::Host),
    }
}

fn level(value: &RuntimeValue) -> Result<LogLevel, LogError> {
    match value {
        RuntimeValue::Variant {
            name,
            variant,
            values,
        } if name == "LogLevel" && values.is_empty() => match variant {
            0 => Ok(LogLevel::Trace),
            1 => Ok(LogLevel::Debug),
            2 => Ok(LogLevel::Info),
            3 => Ok(LogLevel::Warn),
            4 => Ok(LogLevel::Error),
            _ => Err(LogError::Host),
        },
        _ => Err(LogError::Host),
    }
}

struct LogInput<'a> {
    host: &'a BootstrapHost,
    bytes: usize,
    fields: usize,
    limits: LogLimits,
}

impl<'a> LogInput<'a> {
    fn new(host: &'a BootstrapHost) -> Self {
        Self {
            host,
            bytes: 0,
            fields: 0,
            limits: LogLimits::default(),
        }
    }
    fn charge(&mut self, bytes: usize) -> Result<(), LogError> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(LogError::ResourceLimit)?;
        if self.bytes > self.limits.max_event_bytes {
            return Err(LogError::ResourceLimit);
        }
        Ok(())
    }
    fn string(&mut self, text: &str) -> Result<(), LogError> {
        if text.len() > self.limits.max_string_bytes {
            return Err(LogError::ResourceLimit);
        }
        self.charge(text.len())
    }
    fn key(&mut self, key: &str) -> Result<(), LogError> {
        self.fields = self.fields.checked_add(1).ok_or(LogError::ResourceLimit)?;
        if self.fields > self.limits.max_fields {
            return Err(LogError::ResourceLimit);
        }
        self.charge(key.len())
    }
    fn fields(&mut self, raw: &RuntimeValue, depth: usize) -> Result<Fields, LogError> {
        let mut output = Fields::empty();
        for entry in entries(raw)? {
            let RuntimeValue::Tuple(pair) = entry else {
                return Err(LogError::Host);
            };
            let [RuntimeValue::String(key), value] = pair.as_slice() else {
                return Err(LogError::Host);
            };
            output.check_insertion_key(key)?;
            self.key(key)?;
            let value = self.value(value, depth)?;
            output.put(key.clone(), value)?;
        }
        Ok(output)
    }
    // Decoding recursion checks the fixed depth bound before descending.
    // Scalar formatting uses explicit frames and never the host call stack.
    fn value(&mut self, raw: &RuntimeValue, depth: usize) -> Result<LogValue, LogError> {
        let RuntimeValue::Variant {
            name,
            variant,
            values,
        } = raw
        else {
            return Err(LogError::Host);
        };
        if name != "LogValue" {
            return Err(LogError::Host);
        }
        self.charge(8)?;
        match (*variant, values.as_slice()) {
            (0, []) => Ok(LogValue::Null),
            (1, [RuntimeValue::Bool(value)]) => Ok(LogValue::Bool(*value)),
            (2, [RuntimeValue::Integer(value)]) => Ok(LogValue::Int(
                i64::try_from(*value).map_err(|_| LogError::Host)?,
            )),
            (3, [RuntimeValue::Integer(value)]) => Ok(LogValue::UInt(
                u64::try_from(*value).map_err(|_| LogError::Host)?,
            )),
            (4, [RuntimeValue::Float(value)]) => {
                if !value.is_finite() {
                    return Err(LogError::NonFiniteValue);
                }
                Ok(LogValue::Float(*value))
            }
            (5, [RuntimeValue::String(value)]) => {
                self.string(value)?;
                Ok(LogValue::Text(value.as_str().into()))
            }
            (6, [value]) => {
                let bytes = self.host.bytes(value).map_err(|_| LogError::Host)?;
                self.charge(bytes.len())?;
                Ok(LogValue::Bytes(bytes.into()))
            }
            (7, [RuntimeValue::Array(values)]) => {
                if depth >= self.limits.max_depth
                    || values.len() > (self.limits.max_event_bytes - self.bytes) / 8
                {
                    return Err(LogError::ResourceLimit);
                }
                let mut output = Vec::new();
                output
                    .try_reserve_exact(values.len())
                    .map_err(|_| LogError::ResourceLimit)?;
                for value in values {
                    output.push(self.value(value, depth + 1)?);
                }
                Ok(LogValue::Array(output.into()))
            }
            (8, [fields]) => {
                if depth >= self.limits.max_depth {
                    return Err(LogError::ResourceLimit);
                }
                Ok(LogValue::Object(self.fields(fields, depth + 1)?))
            }
            (9, []) => Ok(LogValue::Redacted),
            _ => Err(LogError::Host),
        }
    }
}

fn log_result(result: Result<RuntimeValue, LogError>) -> RuntimeValue {
    match result {
        Ok(value) => RuntimeValue::ResultOk(Box::new(value)),
        Err(error) => RuntimeValue::ResultErr(Box::new(RuntimeValue::Variant {
            name: "LogError".into(),
            variant: error as u32,
            values: Vec::new(),
        })),
    }
}

impl BootstrapHost {
    fn format_log_record(
        &self,
        arguments: &[RuntimeValue],
        policy: LogLimits,
    ) -> Result<Vec<u8>, LogError> {
        let [raw_event, raw_format, _] = arguments else {
            return Err(LogError::Host);
        };
        let format = match raw_format {
            RuntimeValue::Variant {
                name,
                variant: 0,
                values,
            } if name == "LogFormat" && values.is_empty() => LogFormat::Text,
            RuntimeValue::Variant {
                name,
                variant: 1,
                values,
            } if name == "LogFormat" && values.is_empty() => LogFormat::JsonLines,
            _ => return Err(LogError::UnsupportedFormat),
        };
        let RuntimeValue::Record { name, values } = raw_event else {
            return Err(LogError::Host);
        };
        let [
            raw_level,
            RuntimeValue::String(target),
            RuntimeValue::String(message),
            raw_fields,
            timestamp,
        ] = values.as_slice()
        else {
            return Err(LogError::Host);
        };
        if name != "LogEvent" {
            return Err(LogError::Host);
        }
        // Public construction already has this fixed decoding bound. A larger
        // sink limit never permits an unbounded host-stack traversal.
        let mut input = LogInput::new(self);
        input.string(target)?;
        input.string(message)?;
        let fields = input.fields(raw_fields, 0)?;
        let time = match timestamp {
            RuntimeValue::OptionNone => None,
            RuntimeValue::OptionSome(value) => {
                Some(super::civil_time::utc_input(value).map_err(|_| LogError::Host)?)
            }
            _ => return Err(LogError::Host),
        };
        let event = LogEvent::create(
            level(raw_level)?,
            target.clone(),
            message.clone(),
            fields,
            time,
        )?;
        tondo_stdlib::log::format_record(&event, format, policy)
    }

    pub(super) fn invoke_log_delivery_admitted(
        &mut self,
        name: &str,
        arguments: &[RuntimeValue],
        response: &mut VmHostReturnBudget<'_>,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<RuntimeValue, VmError> {
        response.reserve(72, [])?;
        let mut committed = false;
        let result = match (name, arguments) {
            ("std.log.LogEvent.__format", [_, _, raw_limits]) => match limits(raw_limits) {
                Err(error) => Err(error),
                Ok(policy) => {
                    let bound = self
                        .log_construction_bytes(arguments)?
                        .checked_add(policy.max_event_bytes as u64)
                        .ok_or(VmError::ResourceLimit {
                            resource: "memory",
                            limit: self.max_bytes,
                        })?;
                    response.grow_before_construction(bound)?;
                    match self.format_log_record(arguments, policy) {
                        Err(error) => Err(error),
                        Ok(bytes) => {
                            let memory =
                                self.reserve_buffer_payloads(std::iter::once(bytes.len()))?;
                            let preview = self.next_io_bytes()?;
                            let imported = admission
                                .prepare_current(VmHostReturnPreview::ResultOk(&preview))?;
                            admission.commit(&mut [imported])?;
                            committed = true;
                            Ok(self.publish_buffer(
                                RuntimeHostValueKind::Bytes,
                                HostValue::Bytes(bytes),
                                memory,
                            ))
                        }
                    }
                }
            },
            ("std.log.FileSink.__closeFile" | "std.log.ConsoleSink.__closeOutput", [handle]) => {
                let valid = match (name, handle) {
                    (
                        "std.log.FileSink.__closeFile",
                        RuntimeValue::Host {
                            kind: RuntimeHostValueKind::File,
                            id,
                        },
                    ) => matches!(self.values.get(id), Some(HostValue::File { .. })),
                    (
                        "std.log.ConsoleSink.__closeOutput",
                        RuntimeValue::Host {
                            kind: RuntimeHostValueKind::Writer,
                            id,
                        },
                    ) => matches!(self.values.get(id), Some(HostValue::Writer { .. })),
                    _ => false,
                };
                if !valid {
                    Err(LogError::Closed)
                } else {
                    let success = RuntimeValue::Unit;
                    let failure = log_result(Err(LogError::Io));
                    let outcomes = [
                        VmHostReturnPreview::ResultOk(&success),
                        VmHostReturnPreview::Value(&failure),
                    ];
                    let imported =
                        admission.prepare_current(VmHostReturnPreview::StorageBound(&outcomes))?;
                    admission.commit(&mut [imported])?;
                    committed = true;
                    let RuntimeValue::Host { id, .. } = handle else {
                        unreachable!("validated host handle")
                    };
                    let flushed = match self.values.get_mut(id) {
                        Some(HostValue::File { file, .. }) => {
                            file.flush().map_err(|_| LogError::Io)
                        }
                        Some(HostValue::Writer { .. }) => Ok(()),
                        _ => unreachable!("validated live writer"),
                    };
                    // Release the physical writer even when flush failed.
                    self.cleanup(handle)?;
                    flushed.map(|()| RuntimeValue::Unit)
                }
            }
            _ => Err(LogError::Host),
        };
        let value = log_result(result);
        response.shrink(
            value
                .retained_bytes()
                .ok_or_else(|| VmError::Invariant("log delivery reply overflows".into()))?,
        )?;
        if !committed {
            let imported = admission.prepare_current(VmHostReturnPreview::Value(&value))?;
            admission.commit(&mut [imported])?;
        }
        Ok(value)
    }

    /// Conservative logical scratch bound before decoding or cloning. The
    /// iterative walk retains one iterator per container, not its siblings.
    fn log_construction_bytes(&self, arguments: &[RuntimeValue]) -> Result<u64, VmError> {
        let mut bytes = arguments
            .iter()
            .try_fold(512u64, |total, value| {
                total.checked_add(value.retained_bytes()?.checked_mul(4)?)
            })
            .ok_or_else(|| VmError::Invariant("log construction storage overflows".into()))?;
        let mut stack = vec![arguments.iter()];
        while let Some(mut values) = stack.pop() {
            if let Some(value) = values.next() {
                stack.push(values);
                match value {
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Bytes,
                        ..
                    } => {
                        bytes = bytes
                            .checked_add(self.bytes(value)?.len() as u64)
                            .ok_or_else(|| {
                                VmError::Invariant("log byte storage overflows".into())
                            })?;
                    }
                    RuntimeValue::Tuple(values)
                    | RuntimeValue::Array(values)
                    | RuntimeValue::Record { values, .. }
                    | RuntimeValue::Variant { values, .. } => stack.push(values.iter()),
                    RuntimeValue::OptionSome(value)
                    | RuntimeValue::ResultOk(value)
                    | RuntimeValue::ResultErr(value) => {
                        stack.push(std::slice::from_ref(value.as_ref()).iter())
                    }
                    _ => {}
                }
            }
        }
        Ok(bytes)
    }

    pub(super) fn invoke_log_admitted(
        &mut self,
        name: &str,
        arguments: &[RuntimeValue],
        response: &mut VmHostReturnBudget<'_>,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<RuntimeValue, VmError> {
        let operation = LogOperation::from_name(name)
            .ok_or_else(|| VmError::Host("unknown log construction operation".into()))?;
        response.reserve(self.log_construction_bytes(arguments)?, [])?;
        let mut input = LogInput::new(self);
        let result = (|| -> Result<RuntimeValue, LogError> {
            match (operation, arguments) {
                (LogOperation::FieldsWithField, [fields, RuntimeValue::String(key), raw_value]) => {
                    let mut model = input.fields(fields, 0)?;
                    model.check_insertion_key(key)?;
                    input.key(key)?;
                    let value = input.value(raw_value, 0)?;
                    model.put(key.clone(), value)?;
                    let mut updated = Vec::new();
                    let original = entries(fields)?;
                    updated
                        .try_reserve_exact(original.len() + 1)
                        .map_err(|_| LogError::ResourceLimit)?;
                    updated.extend(original.iter().cloned());
                    updated.push(RuntimeValue::Tuple(vec![
                        RuntimeValue::String(key.clone()),
                        raw_value.clone(),
                    ]));
                    Ok(RuntimeValue::Record {
                        name: "Fields".into(),
                        values: vec![RuntimeValue::Array(updated)],
                    })
                }
                (
                    LogOperation::EventCreate,
                    [
                        raw_level,
                        RuntimeValue::String(target),
                        RuntimeValue::String(message),
                        fields,
                        timestamp,
                    ],
                ) => {
                    let level = level(raw_level)?;
                    if target.is_empty() {
                        return Err(LogError::InvalidTarget);
                    }
                    input.string(target)?;
                    input.string(message)?;
                    let model = input.fields(fields, 0)?;
                    let time = match timestamp {
                        RuntimeValue::OptionNone => None,
                        RuntimeValue::OptionSome(value) => {
                            Some(super::civil_time::utc_input(value).map_err(|_| LogError::Host)?)
                        }
                        _ => return Err(LogError::Host),
                    };
                    LogEvent::create(level, target.clone(), message.clone(), model, time)?;
                    Ok(RuntimeValue::Record {
                        name: "LogEvent".into(),
                        values: arguments.to_vec(),
                    })
                }
                _ => Err(LogError::Host),
            }
        })();
        let value = log_result(result);
        response.shrink(
            value
                .retained_bytes()
                .ok_or_else(|| VmError::Invariant("log reply storage overflows".into()))?,
        )?;
        let imported = admission.prepare_current(VmHostReturnPreview::Value(&value))?;
        admission.commit(&mut [imported])?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile_writer_probe(
        function: &str,
    ) -> (
        tondo_vm::bytecode::BytecodeProgram,
        tondo_vm::bytecode::BytecodeFunctionId,
    ) {
        use crate::driver::{
            CompilationRequest, DiagnosticFormat, Edition, HostProfile, Operation, ResourceLimits,
            SourceForm,
        };
        use crate::source::{
            LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin,
        };
        let mut sources = SourceDatabase::new();
        let source = format!("import std.log\nfn main(): !log.LogError {{ log.{function}() }}\n");
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:log-writer-probe").unwrap(),
                ModulePath::new("log_probe").unwrap(),
                LogicalPath::new("log-probe.to").unwrap(),
                source.as_bytes(),
            ))
            .unwrap();
        let packages = crate::package::PackageGraph::loose(&sources, root).unwrap();
        sources
            .add(SourceInput::new(
                packages
                    .package(packages.standard())
                    .unwrap()
                    .source_id()
                    .clone(),
                ModulePath::new("log").unwrap(),
                LogicalPath::new("testing/log-writer-probe.to").unwrap(),
                SourceOrigin::GeneratedStandard,
                include_bytes!("../../tests/fixtures/log-writer-probe.to").as_slice(),
            ))
            .unwrap();
        let request = CompilationRequest::new(
            Operation::Run,
            Edition::V0_1,
            crate::driver::BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::from([crate::driver::CapabilityName::new("console").unwrap()]),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits::default(),
            packages,
            sources,
            root,
        )
        .unwrap();
        let output = crate::driver::compile(request).unwrap();
        assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
        output.into_compiled_program().unwrap()
    }

    #[test]
    fn logging_sealed_writer_probe_preserves_offsets_short_writes_and_terminal_errors() {
        let (program, entry) = compile_writer_probe("__writerProbe");
        let mut host = BootstrapHost::default();
        let outcome = tondo_vm::runtime::execute(&program, entry, &mut host).unwrap();
        assert!(
            matches!(
                outcome.outcome,
                tondo_vm::runtime::VmOutcome::Returned(RuntimeValue::ResultOk(_))
            ),
            "{outcome:?}"
        );
        let record = "info \"probe\" \"value\"\n";
        assert_eq!(
            host.stdout,
            format!("{record}in{record}{record}{record}").as_bytes()
        );
        assert!(host.values.is_empty());
        assert!(host.channels.is_empty());
        assert!(host.buffer_memory.is_empty());
        assert!(host.ready_jobs.is_empty());
        assert!(host.async_memory.is_empty());
    }

    #[test]
    fn logging_real_group_cancellation_restores_the_writer_and_resumes_after_the_written_prefix() {
        let (program, entry) = compile_writer_probe("__cancellationProbe");
        let mut host = BootstrapHost::default();
        let outcome = tondo_vm::runtime::execute(&program, entry, &mut host).unwrap();
        assert!(
            matches!(
                outcome.outcome,
                tondo_vm::runtime::VmOutcome::Returned(RuntimeValue::ResultOk(_))
            ),
            "{outcome:?}"
        );
        assert_eq!(host.stdout, b"info \"probe\" \"cancelled\"\n");
        assert!(host.values.is_empty());
        assert!(host.channels.is_empty());
        assert!(host.buffer_memory.is_empty());
        assert!(host.ready_jobs.is_empty());
        assert!(host.sync_waiters.is_empty());
        assert!(host.async_memory.is_empty());
    }

    #[test]
    fn logging_concurrent_public_emits_preserve_whole_unique_records_and_close_drains_the_tail() {
        let capabilities = BTreeSet::from([crate::driver::CapabilityName::new("console").unwrap()]);
        let (program, entry) = compile_host_admission_source(
            include_str!("../../tests/fixtures/log-concurrent.to"),
            capabilities,
            crate::driver::Operation::Run,
        );
        let mut host = BootstrapHost::default();
        let outcome = tondo_vm::runtime::execute(&program, entry, &mut host).unwrap();
        assert!(
            matches!(
                outcome.outcome,
                tondo_vm::runtime::VmOutcome::Returned(RuntimeValue::ResultOk(_))
            ),
            "{outcome:?}"
        );
        let mut records = std::str::from_utf8(&host.stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        records.sort();
        assert_eq!(
            records,
            (0..8)
                .map(|index| format!("info \"concurrent\" \"{index}\""))
                .collect::<Vec<_>>()
        );
        assert!(host.values.is_empty());
        assert!(host.channels.is_empty());
        assert!(host.ready_jobs.is_empty());
        assert!(host.sync_waiters.is_empty());
        assert!(host.async_memory.is_empty());
    }

    #[test]
    fn logging_public_network_sink_uses_only_the_callers_explicit_transport_and_wire_protocol() {
        let target = crate::toolchain::NetworkTarget {
            resolver_servers: vec!["127.0.0.1:9".into()],
        };
        let build = crate::driver::BuildTarget::vm_hosted()
            .with_network_target(target.clone())
            .unwrap();
        let capabilities = BTreeSet::from([crate::driver::CapabilityName::new("network").unwrap()]);
        let (program, entry) = compile_host_admission_source_with_target(
            include_str!("../../tests/fixtures/log-network.to"),
            capabilities,
            crate::driver::Operation::Run,
            build,
        );
        let mut host = BootstrapHost::default();
        host.install_network_target(Some(target), false);
        let outcome = tondo_vm::runtime::execute(&program, entry, &mut host).unwrap();
        assert!(
            matches!(
                outcome.outcome,
                tondo_vm::runtime::VmOutcome::Returned(RuntimeValue::ResultOk(_))
            ),
            "{outcome:?}"
        );
        assert!(host.stdout.is_empty() && host.stderr.is_empty());
        assert!(host.values.is_empty());
        assert!(host.channels.is_empty());
        assert!(host.ready_jobs.is_empty());
        assert!(host.sync_waiters.is_empty());
        assert!(host.async_memory.is_empty());
    }

    fn fields(entries: Vec<(&str, RuntimeValue)>) -> RuntimeValue {
        RuntimeValue::Record {
            name: "Fields".into(),
            values: vec![RuntimeValue::Array(
                entries
                    .into_iter()
                    .map(|(key, value)| {
                        RuntimeValue::Tuple(vec![RuntimeValue::String(key.into()), value])
                    })
                    .collect(),
            )],
        }
    }
    fn value(variant: u32, values: Vec<RuntimeValue>) -> RuntimeValue {
        RuntimeValue::Variant {
            name: "LogValue".into(),
            variant,
            values,
        }
    }
    fn insert(
        host: &mut BootstrapHost,
        fields: &RuntimeValue,
        key: &str,
        value: RuntimeValue,
    ) -> RuntimeValue {
        host.invoke(
            "std.log.Fields.__withField",
            &[fields.clone(), RuntimeValue::String(key.into()), value],
        )
        .unwrap()
    }
    fn error(error: LogError) -> RuntimeValue {
        log_result(Err(error))
    }

    fn file_mode(variant: u32) -> RuntimeValue {
        RuntimeValue::Variant {
            name: "FileMode".into(),
            variant,
            values: Vec::new(),
        }
    }

    fn format_arguments() -> Vec<RuntimeValue> {
        vec![
            RuntimeValue::Record {
                name: "LogEvent".into(),
                values: vec![
                    RuntimeValue::Variant {
                        name: "LogLevel".into(),
                        variant: 2,
                        values: Vec::new(),
                    },
                    RuntimeValue::String("t".into()),
                    RuntimeValue::String("m".into()),
                    fields(vec![]),
                    RuntimeValue::OptionNone,
                ],
            },
            RuntimeValue::Variant {
                name: "LogFormat".into(),
                variant: 0,
                values: Vec::new(),
            },
            RuntimeValue::Record {
                name: "LogLimits".into(),
                values: [64, 64, 16, 128, 65536, 1024]
                    .into_iter()
                    .map(RuntimeValue::Integer)
                    .collect(),
            },
        ]
    }

    #[test]
    fn logging_formatter_admits_workspace_and_payload_before_publishing_a_byte_identity() {
        let args = format_arguments();
        let expected = b"info \"t\" \"m\"\n";
        let bound = BootstrapHost::default()
            .log_construction_bytes(&args)
            .unwrap()
            + 64;
        let peak = bound + expected.len() as u64 + tondo_vm::runtime::TEST_HOST_BUFFER_BYTES;
        for admitted in [false, true] {
            let mut host = BootstrapHost::default();
            let budget = VmMemoryBudget::new(peak - u64::from(!admitted));
            host.set_test_memory_budget(Some(budget.clone()));
            let result = host.invoke_admitted("std.log.LogEvent.__format", &args, Some(&budget));
            if admitted {
                let returned = result.unwrap();
                let RuntimeValue::ResultOk(bytes) = &returned.value else {
                    panic!("{:?}", returned.value)
                };
                assert_eq!(host.bytes(bytes).unwrap(), expected);
                assert_eq!(returned.memory.as_ref().unwrap().bytes(), 64);
                drop(returned);
                host.collect_host_values(&Default::default()).unwrap();
            } else {
                assert!(result.unwrap_err().is_resource_limit());
                assert_eq!(host.next_value, 0);
            }
            assert!(host.values.is_empty());
            assert!(host.buffer_memory.is_empty());
            assert_eq!(budget.live_bytes(), 0);
        }
    }

    #[test]
    fn logging_formatter_rejects_malformed_and_bounded_inputs_without_publishing_values() {
        let mut host = BootstrapHost::default();
        for (index, replacement, expected) in [
            (0, RuntimeValue::Unit, LogError::Host),
            (1, RuntimeValue::Unit, LogError::UnsupportedFormat),
            (2, RuntimeValue::Unit, LogError::Host),
            (
                2,
                RuntimeValue::Record {
                    name: "LogLimits".into(),
                    values: [0, 1, 1, 1, 1, 1]
                        .into_iter()
                        .map(RuntimeValue::Integer)
                        .collect(),
                },
                LogError::InvalidLimit,
            ),
            (
                2,
                RuntimeValue::Record {
                    name: "LogLimits".into(),
                    values: [1, 64, 16, 128, 65536, 1024]
                        .into_iter()
                        .map(RuntimeValue::Integer)
                        .collect(),
                },
                LogError::ResourceLimit,
            ),
        ] {
            let mut args = format_arguments();
            args[index] = replacement;
            assert_eq!(
                host.invoke("std.log.LogEvent.__format", &args).unwrap(),
                error(expected)
            );
            assert_eq!(host.next_value, 0);
            assert!(host.values.is_empty());
        }
        assert_eq!(
            host.invoke("std.log.LogEvent.__format", &[]).unwrap(),
            error(LogError::Host)
        );
    }

    #[test]
    fn logging_buffer_liveness_preserves_returned_bytes_without_a_test_account() {
        let source = "import std.bytes\npub fn produce(): bytes.Bytes ! bytes.BytesError { bytes.Bytes(\"retained\") }\nfn main() { _ = produce()\n}\n";
        let (program, _) =
            compile_host_admission_source(source, BTreeSet::new(), crate::driver::Operation::Run);
        let entry = program
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::produce"))
            .unwrap()
            .implementation
            .unwrap();
        let mut host = BootstrapHost::default();
        let outcome = tondo_vm::runtime::execute(&program, entry, &mut host).unwrap();
        let tondo_vm::runtime::VmOutcome::Returned(RuntimeValue::ResultOk(bytes)) = outcome.outcome
        else {
            panic!("unexpected result")
        };
        assert_eq!(host.bytes(&bytes).unwrap(), b"retained");
        let mut roots = tondo_vm::runtime::VmHostRoots::new();
        bytes.trace_host_roots(&mut roots);
        host.collect_host_values(&roots).unwrap();
        assert_eq!(host.bytes(&bytes).unwrap(), b"retained");
        host.collect_host_values(&Default::default()).unwrap();
        assert!(host.values.is_empty());
    }

    #[test]
    fn logging_private_close_admits_before_retirement_and_refuses_wrong_or_closed_handles() {
        let mut root = crate::test_temporaries::TemporaryRoot::create(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))
        .unwrap();
        let location = root.path().join("records");
        let mut host = BootstrapHost::default();
        let path = host
            .allocate_path(path::Path::from_string(location.to_str().unwrap()).unwrap())
            .unwrap();
        let RuntimeValue::ResultOk(file) = host
            .invoke("std.log.FileSink.__openFile", &[path, file_mode(0)])
            .unwrap()
        else {
            panic!("file opening failed")
        };
        let RuntimeValue::ResultOk(output) = host.invoke("std.console.stdout", &[]).unwrap() else {
            panic!("console opening failed")
        };
        for (name, wrong, handle) in [
            (
                "std.log.ConsoleSink.__closeOutput",
                file.as_ref(),
                output.as_ref(),
            ),
            (
                "std.log.FileSink.__closeFile",
                output.as_ref(),
                file.as_ref(),
            ),
        ] {
            assert_eq!(
                host.invoke(name, std::slice::from_ref(wrong)).unwrap(),
                error(LogError::Closed)
            );
            let RuntimeValue::Host { id, .. } = handle else {
                panic!("expected writer handle")
            };
            let denied = VmMemoryBudget::new(71);
            assert!(
                host.invoke_admitted(name, std::slice::from_ref(handle), Some(&denied))
                    .unwrap_err()
                    .is_resource_limit()
            );
            assert!(host.values.contains_key(id));
            assert_eq!(denied.live_bytes(), 0);
            let exact = VmMemoryBudget::new(72);
            let returned = host
                .invoke_admitted(name, std::slice::from_ref(handle), Some(&exact))
                .unwrap();
            assert_eq!(
                returned.value,
                RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))
            );
            assert!(!host.values.contains_key(id));
            drop(returned);
            assert_eq!(exact.live_bytes(), 0);
            assert_eq!(
                host.invoke(name, std::slice::from_ref(handle)).unwrap(),
                error(LogError::Closed)
            );
        }
        assert_eq!(
            host.invoke("std.log.ConsoleSink.__closeOutput", &[])
                .unwrap(),
            error(LogError::Host)
        );
        host.collect_host_values(&Default::default()).unwrap();
        assert!(host.values.is_empty());
        assert!(host.buffer_memory.is_empty());
        root.cleanup().unwrap();
    }

    #[test]
    fn logging_public_console_sinks_execute_all_policies_formats_and_consuming_close() {
        use tondo_vm::runtime::{VmOutcome, execute};
        let source = include_str!("../../../../acceptance/projects/stdlib-log-host/src/main.to");
        let capabilities = BTreeSet::from([crate::driver::CapabilityName::new("console").unwrap()]);
        let (program, entry) =
            compile_host_admission_source(source, capabilities, crate::driver::Operation::Run);
        let mut host = BootstrapHost::default();
        let outcome = execute(&program, entry, &mut host).unwrap();
        assert!(
            matches!(
                outcome.outcome,
                VmOutcome::Returned(RuntimeValue::ResultOk(_))
            ),
            "{outcome:?}"
        );
        assert_eq!(host.stdout, b"info \"test\" \"first\"\ninfo \"test\" \"first\"\ninfo \"test\" \"first\"\ninfo \"test\" \"second\"\n");
        assert_eq!(host.stderr, "{\"schema\":\"tondo-log-event-0.1/1\",\"level\":\"info\",\"target\":\"test\",\"message\":\"line\\n世界\",\"time\":null,\"fields\":{}}\n".as_bytes());
        assert!(
            host.values.is_empty(),
            "remaining kinds: {:?}",
            host.values
                .iter()
                .map(|(id, value)| (
                    *id,
                    match value {
                        HostValue::Bytes(bytes) => format!("Bytes({})", bytes.len()),
                        HostValue::Writer { .. } => "Writer".into(),
                        HostValue::ChannelReceiver { .. } => "Receiver".into(),
                        HostValue::ChannelSender { .. } => "Sender".into(),
                        _ => "other".into(),
                    }
                ))
                .collect::<Vec<_>>()
        );
        assert!(host.channels.is_empty());
        assert!(host.buffer_memory.is_empty());
        assert!(host.ready_jobs.is_empty());
        assert!(host.async_memory.is_empty());
    }

    #[test]
    fn logging_public_file_constructor_opens_immediately_and_queue_limits_write_no_prefix() {
        use tondo_vm::runtime::{VmOutcome, execute};
        let mut root = crate::test_temporaries::TemporaryRoot::create(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))
        .unwrap();
        let location = root.path().join("records");
        let literal = serde_json::to_string(location.to_str().unwrap()).unwrap();
        let source =
            include_str!("../../tests/fixtures/log-file.to").replace("\"LOG_FILE\"", &literal);
        let capabilities =
            BTreeSet::from([crate::driver::CapabilityName::new("filesystem").unwrap()]);
        let (program, entry) =
            compile_host_admission_source(&source, capabilities, crate::driver::Operation::Run);
        let mut host = BootstrapHost::default();
        let outcome = execute(&program, entry, &mut host).unwrap();
        assert!(
            matches!(
                outcome.outcome,
                VmOutcome::Returned(RuntimeValue::ResultOk(_))
            ),
            "{outcome:?}"
        );
        assert_eq!(
            std::fs::read(&location).unwrap(),
            b"info \"file\" \"record\"\n"
        );
        assert!(host.values.is_empty());
        assert!(host.channels.is_empty());
        assert!(host.buffer_memory.is_empty());
        assert!(host.ready_jobs.is_empty());
        assert!(host.async_memory.is_empty());
        root.cleanup().unwrap();
    }

    #[test]
    fn logging_file_open_creates_appends_and_truncates_without_creating_parents() {
        let mut root = crate::test_temporaries::TemporaryRoot::create(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))
        .unwrap();
        let location = root.path().join("records");
        let mut host = BootstrapHost::default();
        let budget = VmMemoryBudget::new(65536);
        host.set_test_memory_budget(Some(budget.clone()));
        let path = host
            .allocate_path(path::Path::from_string(location.to_str().unwrap()).unwrap())
            .unwrap();
        for (mode, expected) in [
            (0, b"record\n".as_slice()),
            (0, b"record\nrecord\n"),
            (1, b"record\n"),
        ] {
            let opened = host
                .invoke(
                    "std.log.FileSink.__openFile",
                    &[path.clone(), file_mode(mode)],
                )
                .unwrap();
            let RuntimeValue::ResultOk(file) = opened else {
                panic!("opening failed: {opened:?}");
            };
            let file = *file;
            let bytes = host.allocate_bytes(b"record\n".to_vec()).unwrap();
            assert_eq!(
                host.invoke("std.fs.File.write", &[file.clone(), bytes])
                    .unwrap(),
                RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(7)))
            );
            assert_eq!(
                host.invoke("std.fs.File.flush", std::slice::from_ref(&file))
                    .unwrap(),
                RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))
            );
            host.cleanup(&file).unwrap();
            assert_eq!(std::fs::read(&location).unwrap(), expected);
        }
        let missing = root.path().join("missing").join("records");
        let path = host
            .allocate_path(path::Path::from_string(missing.to_str().unwrap()).unwrap())
            .unwrap();
        for mode in [0, 1] {
            assert_eq!(
                host.invoke(
                    "std.log.FileSink.__openFile",
                    &[path.clone(), file_mode(mode)]
                )
                .unwrap(),
                host.fs_result_error(FsError::NotFound)
            );
            assert!(!missing.parent().unwrap().exists());
        }
        // The logging contract must not silently change std.fs.Append.
        let existing_only = root.path().join("existing-only");
        let path = host
            .allocate_path(path::Path::from_string(existing_only.to_str().unwrap()).unwrap())
            .unwrap();
        assert_eq!(
            host.invoke(
                "std.fs.open",
                &[path, BootstrapHost::fs_mode_value(FsOpenMode::Append)]
            )
            .unwrap(),
            host.fs_result_error(FsError::NotFound)
        );
        assert!(!existing_only.exists());
        host.collect_host_values(&Default::default()).unwrap();
        assert!(host.values.is_empty());
        assert!(host.buffer_memory.is_empty());
        assert_eq!(budget.live_bytes(), 0);
        root.cleanup().unwrap();
    }

    #[test]
    fn logging_file_open_refuses_directories_and_invalid_modes_without_handles() {
        let mut root = crate::test_temporaries::TemporaryRoot::create(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))
        .unwrap();
        let mut host = BootstrapHost::default();
        let budget = VmMemoryBudget::new(65536);
        host.set_test_memory_budget(Some(budget.clone()));
        let path = host
            .allocate_path(path::Path::from_string(root.path().to_str().unwrap()).unwrap())
            .unwrap();
        let next = host.next_value;
        for mode in [0, 1] {
            assert_eq!(
                host.invoke(
                    "std.log.FileSink.__openFile",
                    &[path.clone(), file_mode(mode)]
                )
                .unwrap(),
                host.fs_result_error(FsError::IsDirectory)
            );
            assert_eq!(host.next_value, next);
        }
        for invalid in [
            file_mode(2),
            RuntimeValue::Unit,
            BootstrapHost::fs_mode_value(FsOpenMode::Append),
        ] {
            assert!(matches!(
                host.invoke("std.log.FileSink.__openFile", &[path.clone(), invalid]),
                Err(VmError::Host(_))
            ));
            assert_eq!(host.next_value, next);
        }
        host.collect_host_values(&Default::default()).unwrap();
        assert!(host.values.is_empty());
        assert!(host.buffer_memory.is_empty());
        assert_eq!(budget.live_bytes(), 0);
        root.cleanup().unwrap();
    }

    #[test]
    fn logging_file_open_admits_storage_before_creation_or_truncation() {
        let mut root = crate::test_temporaries::TemporaryRoot::create(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))
        .unwrap();
        let location = root.path().join("records");
        let input = path::Path::from_string(location.to_str().unwrap()).unwrap();
        let peak = 71
            + tondo_vm::runtime::TEST_HOST_BUFFER_BYTES
            + input.as_bytes().len() as u64 * if cfg!(unix) { 1 } else { 3 };
        for exists in [false, true] {
            for mode in [0, 1] {
                for admitted in [false, true] {
                    if exists {
                        std::fs::write(&location, b"preserved").unwrap();
                    } else if location.exists() {
                        std::fs::remove_file(&location).unwrap();
                    }
                    let mut host = BootstrapHost::default();
                    let path =
                        host.allocate(RuntimeHostValueKind::Path, HostValue::Path(input.clone()));
                    let budget = VmMemoryBudget::new(peak - u64::from(!admitted));
                    host.set_test_memory_budget(Some(budget.clone()));
                    let result = host.invoke_owned(
                        "std.log.FileSink.__openFile",
                        vec![path, file_mode(mode)],
                        None,
                        Some(&budget),
                    );
                    if admitted {
                        let returned = result.unwrap();
                        let RuntimeValue::ResultOk(file) = &returned.value else {
                            panic!("opening failed: {:?}", returned.value);
                        };
                        assert_eq!(
                            std::fs::read(&location).unwrap(),
                            if exists && mode == 0 {
                                b"preserved".as_slice()
                            } else {
                                b""
                            }
                        );
                        host.cleanup(file).unwrap();
                        drop(returned);
                    } else {
                        assert!(result.unwrap_err().is_resource_limit());
                        assert_eq!(host.next_value, 1);
                        if exists {
                            assert_eq!(std::fs::read(&location).unwrap(), b"preserved");
                        } else {
                            assert!(!location.exists());
                        }
                    }
                    assert!(host.buffer_memory.is_empty());
                    assert_eq!(budget.live_bytes(), 0);
                }
            }
        }
        root.cleanup().unwrap();
    }

    #[test]
    fn logging_bridge_preserves_all_value_variants_and_existing_byte_identity() {
        let mut host = BootstrapHost::default();
        let bytes = host.allocate_bytes(vec![0, 255]).unwrap();
        let original = fields(vec![("before", value(0, vec![]))]);
        let cases = [
            value(0, vec![]),
            value(1, vec![RuntimeValue::Bool(true)]),
            value(2, vec![RuntimeValue::Integer(i64::MIN.into())]),
            value(3, vec![RuntimeValue::Integer(u64::MAX.into())]),
            value(4, vec![RuntimeValue::Float(-0.0)]),
            value(5, vec![RuntimeValue::String("é世界".into())]),
            value(6, vec![bytes.clone()]),
            value(7, vec![RuntimeValue::Array(vec![value(0, vec![])])]),
            value(8, vec![fields(vec![("inside", value(9, vec![]))])]),
            value(9, vec![]),
        ];
        for candidate in cases {
            let response = insert(&mut host, &original, "after", candidate.clone());
            assert_eq!(
                response,
                log_result(Ok(fields(vec![
                    ("before", value(0, vec![])),
                    ("after", candidate)
                ])))
            );
            assert_eq!(original, fields(vec![("before", value(0, vec![]))]));
            assert_eq!(host.values.len(), 1);
        }
        assert_eq!(host.bytes(&bytes).unwrap(), &[0, 255]);
    }

    #[test]
    fn logging_bridge_refuses_invalid_payloads_without_changing_the_builder() {
        let mut host = BootstrapHost::default();
        let original = fields(vec![("before", value(0, vec![]))]);
        for key in ["", "\0", "line\n", "\u{85}"] {
            assert_eq!(
                insert(&mut host, &original, key, value(0, vec![])),
                error(LogError::InvalidFieldKey)
            );
        }
        assert_eq!(
            insert(
                &mut host,
                &original,
                "before",
                value(4, vec![RuntimeValue::Float(f64::NAN)])
            ),
            error(LogError::DuplicateField)
        );
        assert_eq!(
            insert(&mut host, &original, &"é".repeat(65), value(0, vec![])),
            error(LogError::ResourceLimit)
        );
        for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                insert(
                    &mut host,
                    &original,
                    "new",
                    value(4, vec![RuntimeValue::Float(number)])
                ),
                error(LogError::NonFiniteValue)
            );
        }
        for invalid in [
            value(99, vec![]),
            value(0, vec![RuntimeValue::Unit]),
            value(1, vec![RuntimeValue::Integer(1)]),
            value(2, vec![RuntimeValue::Integer(i64::MAX as i128 + 1)]),
            value(3, vec![RuntimeValue::Integer(-1)]),
            value(6, vec![RuntimeValue::Unit]),
            RuntimeValue::Variant {
                name: "Other".into(),
                variant: 0,
                values: vec![],
            },
        ] {
            assert_eq!(
                insert(&mut host, &original, "new", invalid),
                error(LogError::Host)
            );
        }
        assert_eq!(
            insert(
                &mut host,
                &original,
                "new",
                value(5, vec![RuntimeValue::String("x".repeat(65537))])
            ),
            error(LogError::ResourceLimit)
        );
        assert_eq!(original, fields(vec![("before", value(0, vec![]))]));
        assert!(host.values.is_empty());
    }

    #[test]
    fn logging_bridge_checks_depth_and_node_bytes_before_allocating_arrays() {
        let mut host = BootstrapHost::default();
        let empty = fields(vec![]);
        let mut nested = value(0, vec![]);
        for _ in 0..16 {
            nested = value(7, vec![RuntimeValue::Array(vec![nested])]);
        }
        assert!(matches!(
            insert(&mut host, &empty, "depth", nested.clone()),
            RuntimeValue::ResultOk(_)
        ));
        nested = value(7, vec![RuntimeValue::Array(vec![nested])]);
        assert_eq!(
            insert(&mut host, &empty, "depth", nested),
            error(LogError::ResourceLimit)
        );
        let wide = value(
            7,
            vec![RuntimeValue::Array(vec![
                value(
                    5,
                    vec![RuntimeValue::String(String::new())]
                );
                131073
            ])],
        );
        assert_eq!(
            insert(&mut host, &empty, "wide", wide),
            error(LogError::ResourceLimit)
        );
        let full = fields((0..64).map(|_| ("duplicate", value(0, vec![]))).collect());
        assert_eq!(
            insert(&mut host, &full, "new", value(0, vec![])),
            error(LogError::DuplicateField)
        );
    }

    #[test]
    fn logging_bridge_event_validation_is_portable_and_never_creates_handles() {
        let mut host = BootstrapHost::default();
        let empty = fields(vec![]);
        for tag in 0..5 {
            let level = RuntimeValue::Variant {
                name: "LogLevel".into(),
                variant: tag,
                values: vec![],
            };
            let args = [
                level,
                RuntimeValue::String("app".into()),
                RuntimeValue::String("hello".into()),
                empty.clone(),
                RuntimeValue::OptionNone,
            ];
            assert_eq!(
                host.invoke("std.log.LogEvent.__create", &args).unwrap(),
                log_result(Ok(RuntimeValue::Record {
                    name: "LogEvent".into(),
                    values: args.to_vec()
                }))
            );
            let mut bad = args.clone();
            bad[1] = RuntimeValue::String(String::new());
            assert_eq!(
                host.invoke("std.log.LogEvent.__create", &bad).unwrap(),
                error(LogError::InvalidTarget)
            );
            bad = args.clone();
            bad[4] = RuntimeValue::Unit;
            assert_eq!(
                host.invoke("std.log.LogEvent.__create", &bad).unwrap(),
                error(LogError::Host)
            );
            bad = args.clone();
            bad[4] = RuntimeValue::OptionSome(Box::new(RuntimeValue::Unit));
            assert_eq!(
                host.invoke("std.log.LogEvent.__create", &bad).unwrap(),
                error(LogError::Host)
            );
        }
        assert_eq!(
            host.invoke("std.log.LogEvent.__create", &[]).unwrap(),
            error(LogError::Host)
        );
        assert!(host.values.is_empty());
    }

    #[test]
    fn logging_bridge_joint_admission_refuses_before_publication_and_releases_charges() {
        let mut host = BootstrapHost::default();
        let args = [
            fields(vec![]),
            RuntimeValue::String("key".into()),
            value(5, vec![RuntimeValue::String("hello".into())]),
        ];
        let bound = host.log_construction_bytes(&args).unwrap();
        let short = VmMemoryBudget::new(bound - 1);
        assert!(
            host.invoke_admitted("std.log.Fields.__withField", &args, Some(&short))
                .unwrap_err()
                .is_resource_limit()
        );
        assert_eq!(short.live_bytes(), 0);
        let exact = VmMemoryBudget::new(bound);
        let result = host
            .invoke_admitted("std.log.Fields.__withField", &args, Some(&exact))
            .unwrap();
        assert_eq!(
            result.value,
            log_result(Ok(fields(vec![(
                "key",
                value(5, vec![RuntimeValue::String("hello".into())])
            )])))
        );
        assert_eq!(
            result.memory.as_ref().unwrap().bytes(),
            result.value.retained_bytes().unwrap()
        );
        drop(result);
        assert_eq!(exact.live_bytes(), 0);
        assert!(host.values.is_empty());
    }

    #[test]
    fn logging_public_core_refuses_small_heaps_and_recovers_without_handles() {
        use tondo_vm::runtime::{VmLimits, execute_with_limits};
        let source = include_str!("../../../../acceptance/projects/stdlib-log/src/core.to")
            .replace("fn checkCore()", "fn main()");
        let (program, entry) =
            compile_host_admission_source(&source, BTreeSet::new(), crate::driver::Operation::Run);
        let mut refused = 0;
        let mut succeeded = 0;
        for max_heap_bytes in (1..=16385).step_by(1024).chain(std::iter::once(1048576)) {
            let mut host = BootstrapHost::default();
            match execute_with_limits(
                &program,
                entry,
                &mut host,
                VmLimits {
                    max_heap_bytes,
                    ..VmLimits::default()
                },
            ) {
                Ok(_) => succeeded += 1,
                Err(error) if error.is_resource_limit() => refused += 1,
                Err(error) => panic!("{max_heap_bytes}: {error:?}"),
            }
            assert!(host.values.is_empty());
            assert!(host.buffer_memory.is_empty());
        }
        assert!(refused > 0 && succeeded > 0);
    }

    #[test]
    fn logging_cancellation_restores_the_affine_sink_then_closes_it_once() {
        use tondo_vm::runtime::{VmLimits, VmOutcome, execute_with_limits};
        let source = r#"
import std.log
import std.channel
import std.console
type WaitingSink = { owner: channel.Receiver[Int], started: channel.Sender[Int] }
impl log.LogSink for WaitingSink {
    fn write(var self, event: log.LogEvent): log.LogReceipt ! log.LogError suspends {
        _ = event
        match self.started.trySend(1) {
            ok(_) => {}
            err(_) => return err(log.LogError.ResourceLimit)
        }
        _ = self.owner.receive()
        log.LogReceipt.Accepted
    }
    fn flush(var self): Unit ! log.LogError suspends { () }
    fn close(sink: WaitingSink): Unit ! log.LogError suspends {
        match sink {
            WaitingSink { owner, started } => {
                _ = owner.close()
                started.close()
                match console.println("log-close") {
                    ok(_) => {}
                    err(_) => return err(log.LogError.Io)
                }
            }
        }
    }

}
fn closeReceiver(receiver: channel.Receiver[Int]): Unit {
    _ = receiver.close()
}
fn closeLogger(logger: log.Logger[WaitingSink]): Unit suspends {
    match log.Logger[WaitingSink].close(logger) {
        ok(_) => {}
        err(_) => panic("log-close-error")
    }
}
fn failNow(): Unit { panic("logging-primary") }
fn main(): !log.LogError {
    let (ownerSender, owner) = match channel.bounded[Int](1) {
        ok(pair) => pair
        err(_) => return err(log.LogError.ResourceLimit)
    }
    let (startedSender, started) = match channel.bounded[Int](1) {
        ok(pair) => pair
        err(_) => {
            _ = owner.close()
            return err(log.LogError.ResourceLimit)
        }
    }
    defer closeReceiver(started)
    let logger = log.Logger[WaitingSink].create(WaitingSink { owner, started: startedSender },
        log.LoggerOptions.create(log.LogLevel.Info))?
    defer closeLogger(logger)
    let event = log.LogEvent.create(log.LogLevel.Info, "test", "blocked", log.Fields.empty(), none)?
    scope {
        let pending = spawn logger.emit(event)
        assert(started.receive() == some(1))
        failNow()
        _ = await pending
    }
    ownerSender.close()
}
"#;
        let capabilities = BTreeSet::from([crate::driver::CapabilityName::new("console").unwrap()]);
        let (program, entry) =
            compile_host_admission_source(source, capabilities, crate::driver::Operation::Run);
        for accounted in [false, true] {
            let budget = VmMemoryBudget::new(65536);
            let mut host = BootstrapHost::default();
            if accounted {
                host.set_test_memory_budget(Some(budget.clone()));
            }
            let result =
                execute_with_limits(&program, entry, &mut host, VmLimits::default()).unwrap();
            let VmOutcome::Panicked(panic) = result.outcome else {
                panic!("primary panic must remain visible");
            };
            assert_eq!(panic.message, "logging-primary");
            assert!(panic.suppressed.is_empty(), "{panic:?}");
            assert_eq!(host.stdout, b"log-close\n");
            assert!(host.values.is_empty());
            assert!(host.channels.is_empty());
            assert!(host.buffer_memory.is_empty());
            assert!(host.ready_jobs.is_empty());
            assert!(host.sync_waiters.is_empty());
            assert!(host.async_memory.is_empty());
            assert_eq!(budget.live_bytes(), 0);
        }
    }

    #[test]
    fn logging_affine_logger_heap_refusal_never_leaves_endpoints_or_waiters() {
        use tondo_vm::runtime::{VmLimits, VmOutcome, execute_with_limits};
        let source = r#"
import std.log
import std.channel
type SmallSink = { owner: channel.Receiver[Int], calls: Int }
impl log.LogSink for SmallSink {
    fn write(var self, event: log.LogEvent): log.LogReceipt ! log.LogError suspends {
        _ = event
        self.calls += 1
        log.LogReceipt.Accepted
    }
    fn flush(var self): Unit ! log.LogError suspends { () }
    fn close(sink: SmallSink): Unit ! log.LogError suspends {
        match sink {
            SmallSink { owner, calls: _ } => {
                _ = owner.close()
            }
        }
    }
}
fn closeLogger(logger: log.Logger[SmallSink]): Unit suspends {
    match log.Logger[SmallSink].close(logger) {
        ok(_) => {}
        err(_) => panic("unexpected-close-error")
    }
}
fn main(): !log.LogError {
    let (sender, owner) = match channel.bounded[Int](1) {
        ok(pair) => pair
        err(_) => return err(log.LogError.ResourceLimit)
    }
    sender.close()
    let logger = log.Logger[SmallSink].create(SmallSink { owner, calls: 0 },
        log.LoggerOptions.create(log.LogLevel.Info))?
    defer closeLogger(logger)
    let event = log.LogEvent.create(log.LogLevel.Info, "test", "value", log.Fields.empty(), none)?
    assert(logger.emit(event)? == log.LogReceipt.Accepted)
    logger.flush()?
    log.Logger[SmallSink].close(logger)?
}
"#;
        let (program, entry) =
            compile_host_admission_source(source, BTreeSet::new(), crate::driver::Operation::Run);
        let mut refused = 0;
        let mut succeeded = 0;
        for max_heap_bytes in (1..=32769).step_by(1024).chain(std::iter::once(1048576)) {
            let mut host = BootstrapHost::default();
            let execution = execute_with_limits(
                &program,
                entry,
                &mut host,
                VmLimits {
                    max_heap_bytes,
                    ..VmLimits::default()
                },
            );
            if !host.values.is_empty() {
                let remaining = host
                    .values
                    .iter()
                    .map(|(id, value)| match value {
                        HostValue::ChannelSender { channel } => (*id, "sender", *channel),
                        HostValue::ChannelReceiver { channel } => (*id, "receiver", *channel),
                        _ => (*id, "other", 0),
                    })
                    .collect::<Vec<_>>();
                let channels = host
                    .channels
                    .iter()
                    .map(|(id, state)| (*id, state.senders, state.receivers, state.queue.len()))
                    .collect::<Vec<_>>();
                panic!(
                    "{max_heap_bytes}: result={execution:?}, remaining={remaining:?}, channels={channels:?}"
                );
            }
            match execution {
                Ok(result) => {
                    assert!(
                        matches!(
                            result.outcome,
                            VmOutcome::Returned(RuntimeValue::ResultOk(_))
                        ),
                        "{max_heap_bytes}: {:?}",
                        result.outcome
                    );
                    succeeded += 1;
                }
                Err(error) if error.is_resource_limit() => refused += 1,
                Err(error) => panic!("{max_heap_bytes}: {error:?}"),
            }
            assert!(
                host.values.is_empty(),
                "{max_heap_bytes}: {} endpoints",
                host.values.len()
            );
            assert!(
                host.channels.is_empty(),
                "{max_heap_bytes}: {} channels",
                host.channels.len()
            );
            assert!(host.ready_jobs.is_empty());
            assert!(host.sync_waiters.is_empty());
            assert!(host.async_memory.is_empty());
            assert!(host.buffer_memory.is_empty());
        }
        assert!(refused > 0 && succeeded > 0);
    }
}
