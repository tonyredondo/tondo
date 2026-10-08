//! Pure log construction with preadmitted scratch and exact result admission.
//! Existing immutable Bytes identities are retained in the published values.

use super::*;
use tondo_stdlib::log::{Fields, LogError, LogEvent, LogLevel, LogLimits, LogOperation, LogValue};

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
