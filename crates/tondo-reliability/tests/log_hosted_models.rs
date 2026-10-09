use reference::{Event, Fields, Format, Limits, Value};
use tondo_reliability::log_model::{queue as queue_reference, values as reference};

fn execute_public(
    source: &str,
    operation: tondo_compiler::driver::Operation,
    capabilities: &[&str],
) -> tondo_compiler::driver::CompilationOutput {
    use std::{collections::BTreeSet, sync::Arc};
    use tondo_compiler::driver::{
        BuildTarget, CapabilityName, CompilationRequest, DiagnosticFormat, Edition, HostProfile,
        ResourceLimits, SourceForm, execute,
    };
    use tondo_compiler::package::PackageGraph;
    use tondo_compiler::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:log-public-model").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("log-public-model.to").unwrap(),
            Arc::<[u8]>::from(source.as_bytes()),
        ))
        .unwrap();
    let packages = PackageGraph::loose(&sources, root).unwrap();
    execute(
        CompilationRequest::new(
            operation,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            capabilities
                .iter()
                .map(|name| CapabilityName::new(*name).unwrap())
                .collect::<BTreeSet<_>>(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits {
                max_vm_steps: 1_000_000,
                max_vm_heap_bytes: 8_388_608,
                ..ResourceLimits::default()
            },
            packages,
            sources,
            root,
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn public_nested_values_and_immutable_event_snapshots_match_both_record_formats() {
    use tondo_compiler::driver::Operation;
    let mut nested = Fields::default();
    nested.put("z", Value::Int(-7)).unwrap();
    nested.put("a", Value::Redacted).unwrap();
    let mut fields = Fields::default();
    fields
        .put(
            "z",
            Value::Array(vec![
                Value::Null,
                Value::Bool(true),
                Value::UInt(42),
                Value::Float(1.5),
                Value::Bytes(vec![0, 255, 1]),
                Value::Object(nested),
                Value::Redacted,
            ]),
        )
        .unwrap();
    fields
        .put("text", Value::Text("line\n世界\"\\".into()))
        .unwrap();
    fields
        .put("secret", Value::Text("literal-token-value".into()))
        .unwrap();
    let event = Event::create(2, "public", "values\n世界", fields, None).unwrap();
    let source = r#"import std.log
import std.bytes
fn closeLogger(logger: log.Logger[log.ConsoleSink]) suspends {
 match log.Logger[log.ConsoleSink].close(logger) {
  ok(_) => {}
  err(_) => panic("close")
 }
}
fn main(): !log.LogError {
 let payload = match bytes.fromArray([Byte(0u8), Byte(255u8), Byte(1u8)]) {
  ok(value) => value
  err(_) => panic("bytes")
 }
 var nested = log.Fields.empty()
 nested.put("z", log.LogValue.Int(-7))?
 nested.put("a", log.LogValue.Redacted)?
 var fields = log.Fields.empty()
 fields.put("z", log.LogValue.Array([
  log.LogValue.Null, log.LogValue.Bool(true), log.LogValue.UInt(42),
  log.LogValue.Float(1.5), log.LogValue.Bytes(payload),
  log.LogValue.Object(nested), log.LogValue.Redacted,
 ]))?
 fields.put("text", log.LogValue.Text("line\n世界\"\\"))?
 fields.put("secret", log.LogValue.Text("literal-token-value"))?
 let event = log.LogEvent.create(log.LogLevel.Info, "public", "values\n世界", fields, none)?
 fields.put("after", log.LogValue.Null)?
 assert(event.fields().count() == 3)
 let options = log.SinkOptions.create(log.LogFormat.FORMAT, log.Backpressure.Block, 1, log.LogLimits.defaults())?
 let sink = log.ConsoleSink.create(log.ConsoleStream.STREAM, options)?
 let logger = log.Logger[log.ConsoleSink].create(sink, log.LoggerOptions.create(log.LogLevel.Info))?
 defer closeLogger(logger)
 assert(logger.emit(event)? == log.LogReceipt.Accepted)
 log.Logger[log.ConsoleSink].close(logger)?
}
"#;
    for format in [Format::Text, Format::JsonLines] {
        let name = if format == Format::Text {
            "Text"
        } else {
            "JsonLines"
        };
        let expected = reference::format_record(&event, format, Limits::default()).unwrap();
        for stream in ["Stdout", "Stderr"] {
            let output = execute_public(
                &source.replace("FORMAT", name).replace("STREAM", stream),
                Operation::Run,
                &["console"],
            );
            assert_eq!(
                output.exit_code(),
                0,
                "{format:?}/{stream}: {}",
                output.diagnostics().human()
            );
            if stream == "Stdout" {
                assert_eq!(output.stdout(), expected);
                assert!(output.stderr().is_empty());
            } else {
                assert!(output.stdout().is_empty());
                assert_eq!(output.stderr(), expected);
            }
        }
    }
}

#[test]
fn public_timestamps_are_explicit_and_capability_and_affine_denials_are_static() {
    use tondo_compiler::driver::Operation;
    let prefix = r#"import std.log
fn closeLogger(logger: log.Logger[log.ConsoleSink]) suspends {
 match log.Logger[log.ConsoleSink].close(logger) {
  ok(_) => {}
  err(_) => panic("close")
 }
}
fn main(): !log.LogError {
 let options = log.SinkOptions.create(log.LogFormat.JsonLines, log.Backpressure.Block, 1, log.LogLimits.defaults())?
 let sink = log.ConsoleSink.create(log.ConsoleStream.Stdout, options)?
 let logger = log.Logger[log.ConsoleSink].create(sink, log.LoggerOptions.create(log.LogLevel.Info))?
"#;
    let no_console = format!("{prefix} defer closeLogger(logger)\n}}\n");
    let output = execute_public(&no_console, Operation::Check, &[]);
    assert_eq!(output.exit_code(), 1);
    assert!(output.diagnostics().human().contains("E1008"));
    let moved = format!(
        "{prefix} let moved = logger\n log.Logger[log.ConsoleSink].close(moved)?\n logger.flush()?\n}}\n"
    );
    let output = execute_public(&moved, Operation::Check, &["console"]);
    assert_eq!(output.exit_code(), 1);
    assert!(output.diagnostics().human().contains("E1401"));
    let overlapping = format!(
        "{prefix} defer closeLogger(logger)\n scope {{\n let pending = spawn logger.flush()\n log.Logger[log.ConsoleSink].close(logger)?\n _ = await pending\n }}\n}}\n"
    );
    let output = execute_public(&overlapping, Operation::Check, &["console"]);
    assert_eq!(output.exit_code(), 1);
    assert!(output.diagnostics().human().contains("E1403"));
    let file = r#"import std.log
import std.path
fn main(): !log.LogError {
 let location = match path.Path.fromString("unused-log-file") {
  ok(value) => value
  err(_) => panic("path")
 }
 let options = log.SinkOptions.create(log.LogFormat.Text, log.Backpressure.Reject, 1, log.LogLimits.defaults())?
 let sink = log.FileSink.create(location, log.FileMode.Append, options)?
 log.LogSink.close[log.FileSink](sink)?
}
"#;
    let output = execute_public(file, Operation::Check, &[]);
    assert_eq!(output.exit_code(), 1);
    assert!(output.diagnostics().human().contains("E1008"));
    let private = "import std.log\nfn main() {\n _ = log.LogEvent.__format\n}\n";
    let output = execute_public(private, Operation::Check, &["console"]);
    assert_eq!(output.exit_code(), 1);
    assert!(
        output.diagnostics().human().contains("E1004"),
        "{}",
        output.diagnostics().human()
    );
    let timestamp = "2000-02-29T12:34:56Z";
    let source = format!(
        "import std.time\n{prefix} defer closeLogger(logger)\n let timestamp = match time.UtcDateTime.parse({timestamp:?}) {{\n ok(value) => value\n err(_) => panic(\"timestamp\")\n }}\n let event = log.LogEvent.create(log.LogLevel.Info, \"stamp\", \"explicit\", log.Fields.empty(), some(timestamp))?\n assert(logger.emit(event)? == log.LogReceipt.Accepted)\n log.Logger[log.ConsoleSink].close(logger)?\n}}\n"
    );
    let output = execute_public(&source, Operation::Run, &["console"]);
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    let event = Event::create(2, "stamp", "explicit", Fields::default(), Some(timestamp)).unwrap();
    assert_eq!(
        output.stdout(),
        reference::format_record(&event, Format::JsonLines, Limits::default()).unwrap()
    );
}

#[test]
fn public_file_delivery_matches_reference_and_opening_refuses_missing_parents() {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    use tondo_compiler::driver::Operation;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct OwnedRoot(PathBuf);
    impl Drop for OwnedRoot {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let path = std::env::temp_dir().join(format!(
        "tondo-log-model-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    let owned = OwnedRoot(path);
    let location = owned.0.join("records");
    let literal = serde_json::to_string(location.to_str().unwrap()).unwrap();
    let source = include_str!("../../tondo-compiler/tests/fixtures/log-file.to")
        .replace("\"LOG_FILE\"", &literal);
    let output = execute_public(&source, Operation::Run, &["filesystem"]);
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    let event = Event::create(2, "file", "record", Fields::default(), None).unwrap();
    let expected = reference::format_record(&event, Format::Text, Limits::default()).unwrap();
    assert_eq!(std::fs::read(&location).unwrap(), expected);
    let missing = owned.0.join("missing/record");
    let literal = serde_json::to_string(missing.to_str().unwrap()).unwrap();
    let source = format!(
        r#"import std.log
import std.path
fn main(): !log.LogError {{
 let location = match path.Path.fromString({literal}) {{
  ok(value) => value
  err(_) => panic("path")
 }}
 let options = log.SinkOptions.create(log.LogFormat.Text, log.Backpressure.Reject, 1, log.LogLimits.defaults())?
 match log.FileSink.create(location, log.FileMode.Append, options) {{
  err(log.LogError.Io) => ()
  ok(sink) => {{
   log.LogSink.close[log.FileSink](sink)?
   panic("missing parent was created")
  }}
  err(_) => panic("unexpected opening error")
 }}
}}
"#
    );
    let output = execute_public(&source, Operation::Run, &["filesystem"]);
    assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    assert!(!missing.exists());
    assert!(!missing.parent().unwrap().exists());
    assert_eq!(std::fs::read(&location).unwrap(), expected);
}

#[test]
fn concurrent_public_producers_preserve_records_and_each_producers_order() {
    use queue_reference::{Policy, Queue, Writer};
    use std::collections::BTreeMap;
    use tondo_compiler::driver::Operation;
    for format in [Format::Text, Format::JsonLines] {
        let format_name = if format == Format::Text {
            "Text"
        } else {
            "JsonLines"
        };
        let mut program = format!(
            r#"import std.async as jobs
import std.log
fn closeLogger(logger: log.Logger[log.ConsoleSink]) suspends {{
 match log.Logger[log.ConsoleSink].close(logger) {{
  ok(_) => {{}}
  err(_) => panic("close")
 }}
}}
fn sequence(logger: ref log.Logger[log.ConsoleSink], messages: Array[String]): !log.LogError suspends {{
 for message in messages {{
  let event = log.LogEvent.create(log.LogLevel.Info, "concurrent", message, log.Fields.empty(), none)?
  assert(logger.emit(event)? == log.LogReceipt.Accepted)
 }}
}}
fn main(): !log.LogError {{
 let options = log.SinkOptions.create(log.LogFormat.{format_name}, log.Backpressure.Block, 2, log.LogLimits.defaults())?
 let sink = log.ConsoleSink.create(log.ConsoleStream.Stdout, options)?
 let logger = log.Logger[log.ConsoleSink].create(sink, log.LoggerOptions.create(log.LogLevel.Info))?
 defer closeLogger(logger)
 scope {{
  var group = jobs.group[Unit, log.LogError]()
"#
        );
        let mut expected = BTreeMap::new();
        for producer in 0..4 {
            let messages = (0..8)
                .map(|index| format!("p{producer}-{index}"))
                .collect::<Vec<_>>();
            for (index, message) in messages.iter().enumerate() {
                let event =
                    Event::create(2, "concurrent", message, Fields::default(), None).unwrap();
                let record = reference::format_record(&event, format, Limits::default()).unwrap();
                assert!(expected.insert(record, (producer, index)).is_none());
            }
            let arguments = messages
                .iter()
                .map(|message| format!("{message:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            program.push_str(&format!(
                "  group.add(spawn sequence(ref logger, [{arguments}]))\n"
            ));
        }
        program
            .push_str("  _ = group.all()?\n }\n log.Logger[log.ConsoleSink].close(logger)?\n}\n");
        let output = execute_public(&program, Operation::Run, &["console"]);
        assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
        let mut positions = [0; 4];
        let mut model =
            Queue::new(2, 2048, Policy::Block, Writer::new(2, &[], &[]).unwrap()).unwrap();
        let mut count = 0;
        for record in output.stdout().split_inclusive(|byte| *byte == b'\n') {
            let &(producer, index) = expected
                .get(record)
                .expect("a whole known record is required");
            assert_eq!(
                positions[producer], index,
                "producer order or duplicate record"
            );
            positions[producer] += 1;
            count += 1;
            model.submit(record).unwrap();
        }
        assert_eq!(positions, [8; 4]);
        assert_eq!(count, 32);
        model.close().unwrap();
        assert_eq!(model.writer().output(), output.stdout());
        assert_eq!(model.live_bytes(), 0);
        assert_eq!(model.live_owners(), 0);
    }
}

#[test]
fn sealed_writer_and_group_cancellation_match_independent_prefix_model() {
    use queue_reference::{Error, Policy, Queue, Receipt, WriteStep, Writer};
    use std::{collections::BTreeSet, sync::Arc};
    use tondo_compiler::driver::{
        BuildTarget, CapabilityName, CompilationRequest, DiagnosticFormat, Edition, HostProfile,
        Operation, ResourceLimits, SourceForm, execute,
    };
    use tondo_compiler::package::PackageGraph;
    use tondo_compiler::source::{
        LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin,
    };
    let mut cases = Vec::new();
    let event = Event::create(2, "probe", "value", Fields::default(), None).unwrap();
    let record = reference::format_record(&event, Format::Text, Limits::default()).unwrap();
    let mut expected = Vec::new();
    for mode in 0..7 {
        let steps = match mode {
            1 => vec![WriteStep::Count(2), WriteStep::Fail(Error::Io)],
            2 => vec![WriteStep::Count(2), WriteStep::Fail(Error::Cancelled)],
            3 => vec![WriteStep::Count(0)],
            4 => vec![WriteStep::Count(record.len() + 1)],
            6 => vec![WriteStep::Count(2), WriteStep::Fail(Error::ResourceLimit)],
            _ => vec![],
        };
        let flushes = if mode == 5 {
            vec![Err(Error::Io)]
        } else {
            vec![]
        };
        let mut model = Queue::new(
            1,
            2048,
            Policy::Reject,
            Writer::new(2, &steps, &flushes).unwrap(),
        )
        .unwrap();
        assert_eq!(model.submit(&record), Ok(Receipt::Accepted));
        let flush = model.flush();
        if mode == 0 {
            assert_eq!(flush, Ok(()));
        } else {
            let failure = match mode {
                2 => Error::Cancelled,
                6 => Error::ResourceLimit,
                _ => Error::Io,
            };
            assert_eq!(flush, Err(failure));
            if matches!(mode, 2 | 6) {
                model.flush().unwrap();
            } else {
                assert_eq!(model.submit(&record), Err(Error::Closed));
            }
        }
        let close = model.close();
        assert_eq!(
            close,
            if matches!(mode, 0 | 2 | 6) {
                Ok(())
            } else {
                Err(Error::Io)
            }
        );
        assert_eq!(model.live_owners(), 0);
        assert_eq!(model.live_bytes(), 0);
        assert_eq!(model.writer().close_count(), 1);
        expected.extend_from_slice(model.writer().output());
    }
    cases.push(("__writerProbe", expected));
    let event = Event::create(2, "probe", "cancelled", Fields::default(), None).unwrap();
    let record = reference::format_record(&event, Format::Text, Limits::default()).unwrap();
    let mut model = Queue::new(
        1,
        2048,
        Policy::Reject,
        Writer::new(
            2,
            &[WriteStep::Count(2), WriteStep::Fail(Error::Cancelled)],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    model.submit(&record).unwrap();
    assert_eq!(model.flush(), Err(Error::Cancelled));
    model.flush().unwrap();
    model.close().unwrap();
    cases.push(("__cancellationProbe", model.writer().output().to_vec()));
    for (function, expected) in cases {
        let program = format!("import std.log\nfn main(): !log.LogError {{ log.{function}() }}\n");
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:log-model").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("log-model.to").unwrap(),
                Arc::<[u8]>::from(program.as_bytes()),
            ))
            .unwrap();
        let packages = PackageGraph::loose(&sources, root).unwrap();
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
                include_bytes!("../../tondo-compiler/tests/fixtures/log-writer-probe.to")
                    .as_slice(),
            ))
            .unwrap();
        let request = CompilationRequest::new(
            Operation::Run,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::from([CapabilityName::new("console").unwrap()]),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits {
                max_vm_steps: 1_000_000,
                max_vm_heap_bytes: 8_388_608,
                ..ResourceLimits::default()
            },
            packages,
            sources,
            root,
        )
        .unwrap();
        let output = execute(request).unwrap();
        assert_eq!(
            output.exit_code(),
            0,
            "{function}: {}",
            output.diagnostics().human()
        );
        assert_eq!(output.stdout(), expected, "{function}");
    }
}

#[test]
fn public_console_queue_and_filter_transcripts_match_independent_model() {
    use queue_reference::{Policy, Queue, Receipt, Writer};
    use tondo_compiler::driver::Operation;
    for format in [Format::Text, Format::JsonLines] {
        for policy in [Policy::Block, Policy::Drop, Policy::Reject] {
            for capacity in [1, 2] {
                let format_name = if format == Format::Text {
                    "Text"
                } else {
                    "JsonLines"
                };
                let policy_name = format!("{policy:?}");
                let mut model =
                    Queue::new(capacity, 2048, policy, Writer::new(2, &[], &[]).unwrap()).unwrap();
                let mut program = format!(
                    r#"import std.log
fn closeLogger(logger: log.Logger[log.ConsoleSink]) suspends {{
 match log.Logger[log.ConsoleSink].close(logger) {{
  ok(_) => {{}}
  err(_) => panic("close")
 }}
}}
fn main(): !log.LogError {{
 let options = log.SinkOptions.create(log.LogFormat.{format_name}, log.Backpressure.{policy_name}, {capacity}, log.LogLimits.defaults())?
 let sink = log.ConsoleSink.create(log.ConsoleStream.Stdout, options)?
 let logger = log.Logger[log.ConsoleSink].create(sink, log.LoggerOptions.create(log.LogLevel.Info))?
 defer closeLogger(logger)
"#
                );
                for index in 0..8 {
                    if index == 4 {
                        model.flush().unwrap();
                        program.push_str(" logger.flush()?\n");
                    }
                    let level = if index % 3 == 0 { 1 } else { 2 };
                    let message = format!("record{index}\nκ世界");
                    let event =
                        Event::create(level, "model", &message, Fields::default(), None).unwrap();
                    let outcome = if event.enabled(2) {
                        let bytes =
                            reference::format_record(&event, format, Limits::default()).unwrap();
                        model.submit(&bytes)
                    } else {
                        program.push_str(&format!(" assert(logger.emit(log.LogEvent.create(log.LogLevel.Debug, \"model\", {message:?}, log.Fields.empty(), none)?)? == log.LogReceipt.Filtered)\n"));
                        continue;
                    };
                    program.push_str(&format!(" let event{index} = log.LogEvent.create(log.LogLevel.Info, \"model\", {message:?}, log.Fields.empty(), none)?\n"));
                    match outcome {
                        Ok(receipt) => {
                            let receipt = match receipt {
                                Receipt::Accepted => "Accepted",
                                Receipt::Dropped => "Dropped",
                            };
                            program.push_str(&format!(
                                " assert(logger.emit(event{index})? == log.LogReceipt.{receipt})\n"
                            ));
                        }
                        Err(queue_reference::Error::Backpressure) => program.push_str(&format!(
                            " assert(logger.emit(event{index}) == err(log.LogError.Backpressure))\n"
                        )),
                        other => panic!("unexpected reference outcome: {other:?}"),
                    }
                }
                assert!(model.queued() <= capacity);
                assert!(model.peak_bytes() <= capacity * 2048);
                model.close().unwrap();
                assert_eq!(model.live_owners(), 0);
                assert_eq!(model.live_bytes(), 0);
                program.push_str(" log.Logger[log.ConsoleSink].close(logger)?\n}\n");
                let output = execute_public(&program, Operation::Run, &["console"]);
                assert_eq!(
                    output.exit_code(),
                    0,
                    "{format:?}/{policy:?}/{capacity}: {}",
                    output.diagnostics().human()
                );
                assert_eq!(
                    output.stdout(),
                    model.writer().output(),
                    "{format:?}/{policy:?}/{capacity}"
                );
            }
        }
    }
}
