//! Target-qualified logging workloads verified before hosted VM timing.

use super::*;
use serde::Serialize;
use std::{hint::black_box, time::Instant};

#[path = "../../../../tondo-reliability/src/log_model/queue.rs"]
mod reference_queue;
#[path = "../../../../tondo-reliability/src/log_model/values.rs"]
#[expect(
    dead_code,
    reason = "The standalone oracle generator is exercised by its owner tests."
)]
mod reference_values;

use reference_values::{Event, Fields, Format, Limits, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    EnabledQuery,
    Filtered,
    Normal,
    BlockOne,
    Reject,
    Drop,
    FlushEach,
    Concurrent,
    FileAppend,
    FileTruncate,
    ShortWriter,
    InterruptedWriter,
    IoWriter,
    EventLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Plain,
    Structured,
    Large,
}

#[derive(Clone, Copy, Debug)]
struct Workload {
    id: &'static str,
    flow: Flow,
    shape: Shape,
    format: Format,
}

const WORKLOADS: [Workload; 18] = [
    Workload {
        id: "enabled-query",
        flow: Flow::EnabledQuery,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "filtered-emit",
        flow: Flow::Filtered,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "text-small",
        flow: Flow::Normal,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "json-small",
        flow: Flow::Normal,
        shape: Shape::Plain,
        format: Format::JsonLines,
    },
    Workload {
        id: "text-structured",
        flow: Flow::Normal,
        shape: Shape::Structured,
        format: Format::Text,
    },
    Workload {
        id: "json-structured",
        flow: Flow::Normal,
        shape: Shape::Structured,
        format: Format::JsonLines,
    },
    Workload {
        id: "json-large",
        flow: Flow::Normal,
        shape: Shape::Large,
        format: Format::JsonLines,
    },
    Workload {
        id: "block-capacity-one",
        flow: Flow::BlockOne,
        shape: Shape::Plain,
        format: Format::JsonLines,
    },
    Workload {
        id: "reject-full",
        flow: Flow::Reject,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "drop-full",
        flow: Flow::Drop,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "flush-each",
        flow: Flow::FlushEach,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "concurrent-producers",
        flow: Flow::Concurrent,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "file-append",
        flow: Flow::FileAppend,
        shape: Shape::Plain,
        format: Format::JsonLines,
    },
    Workload {
        id: "file-truncate",
        flow: Flow::FileTruncate,
        shape: Shape::Plain,
        format: Format::JsonLines,
    },
    Workload {
        id: "writer-short",
        flow: Flow::ShortWriter,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "writer-interrupt",
        flow: Flow::InterruptedWriter,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "writer-io",
        flow: Flow::IoWriter,
        shape: Shape::Plain,
        format: Format::Text,
    },
    Workload {
        id: "reject-event-limit",
        flow: Flow::EventLimit,
        shape: Shape::Plain,
        format: Format::Text,
    },
];

impl Workload {
    fn operations(self) -> usize {
        match self.flow {
            Flow::EnabledQuery | Flow::Filtered => 128,
            Flow::IoWriter => 2,
            Flow::Reject
            | Flow::Drop
            | Flow::FlushEach
            | Flow::FileAppend
            | Flow::FileTruncate
            | Flow::ShortWriter
            | Flow::InterruptedWriter
            | Flow::EventLimit => 8,
            _ if self.shape == Shape::Large => 8,
            _ if self.shape == Shape::Structured => 16,
            _ => 32,
        }
    }

    fn capacity(self) -> usize {
        match self.flow {
            Flow::BlockOne | Flow::Reject | Flow::Drop => 1,
            Flow::IoWriter => 2,
            _ => 8,
        }
    }

    fn operation(self) -> &'static str {
        match self.flow {
            Flow::EnabledQuery => "enabled-query",
            Flow::Filtered => "filtered-emit",
            Flow::BlockOne => "block",
            Flow::Reject => "reject",
            Flow::Drop => "drop",
            Flow::FlushEach => "flush",
            Flow::Concurrent => "concurrent",
            Flow::FileAppend | Flow::FileTruncate => "file",
            Flow::ShortWriter => "short-write",
            Flow::InterruptedWriter => "interruption-resume",
            Flow::IoWriter => "terminal-io",
            Flow::EventLimit => "limit-rejection",
            Flow::Normal => "emit",
        }
    }

    fn is_sealed_writer(self) -> bool {
        matches!(
            self.flow,
            Flow::ShortWriter | Flow::InterruptedWriter | Flow::IoWriter
        )
    }

    fn is_file(self) -> bool {
        matches!(self.flow, Flow::FileAppend | Flow::FileTruncate)
    }

    fn policy(self) -> &'static str {
        match self.flow {
            Flow::Reject => "Reject",
            Flow::Drop => "Drop",
            _ => "Block",
        }
    }

    fn format_name(self) -> &'static str {
        match self.format {
            Format::Text => "Text",
            Format::JsonLines => "JsonLines",
        }
    }
}

fn reference_record(workload: Workload, message: &str) -> Vec<u8> {
    if workload.shape == Shape::Large {
        assert_eq!(message, "x".repeat(4096));
        assert_eq!(workload.format, Format::JsonLines);
        assert_eq!(
            Event::create(2, "perf", message, Fields::default(), None),
            Err(reference_values::Error::OutsideDomain),
        );
        // The authored ASCII law covers this deliberately out-of-domain input.
        return format!(
            "{{\"schema\":\"tondo-log-event-0.1/1\",\"level\":\"info\",\"target\":\"perf\",\"message\":\"{message}\",\"time\":null,\"fields\":{{}}}}\n"
        ).into_bytes();
    }
    let mut fields = Fields::default();
    let timestamp = if workload.shape == Shape::Structured {
        let mut object = Fields::default();
        object.put("z", Value::Int(-7)).unwrap();
        object.put("a", Value::Redacted).unwrap();
        fields.put("nested", Value::Object(object)).unwrap();
        fields
            .put(
                "values",
                Value::Array(vec![
                    Value::Null,
                    Value::Bool(true),
                    Value::UInt(42),
                    Value::Float(1.25),
                    Value::Bytes(vec![0, 255, 1]),
                ]),
            )
            .unwrap();
        fields
            .put("unicode", Value::Text("line\n世界\"\\".into()))
            .unwrap();
        Some("2000-02-29T12:34:56Z")
    } else {
        None
    };
    let level = if matches!(workload.flow, Flow::EnabledQuery | Flow::Filtered) {
        0
    } else {
        2
    };
    let event = Event::create(level, "perf", message, fields, timestamp).unwrap();
    assert_eq!(
        event.enabled(2),
        !matches!(workload.flow, Flow::EnabledQuery | Flow::Filtered)
    );
    if workload.flow == Flow::EventLimit {
        assert_eq!(
            reference_values::format_record(
                &event,
                workload.format,
                Limits {
                    event: 8,
                    ..Limits::default()
                }
            ),
            Err(reference_values::Error::ResourceLimit)
        );
    }
    reference_values::format_record(&event, workload.format, Limits::default()).unwrap()
}

#[derive(Debug, Default)]
struct Expected {
    attempted_events: usize,
    accepted_events: usize,
    filtered_events: usize,
    dropped_events: usize,
    rejected_events: usize,
    output: Vec<u8>,
    peak_records: usize,
    peak_bytes: usize,
}

fn reference_delivery(workload: Workload, records: &[Vec<u8>]) -> Expected {
    use reference_queue::{Error, Policy, Queue, Receipt, WriteStep, Writer};
    let mut expected = Expected::default();
    if workload.flow == Flow::EnabledQuery {
        assert!(records.is_empty());
        return expected;
    }
    expected.attempted_events = records.len();
    if workload.flow == Flow::Filtered {
        expected.filtered_events = records.len();
        return expected;
    }
    if workload.flow == Flow::EventLimit {
        expected.rejected_events = records.len();
        return expected;
    }
    if workload.shape == Shape::Large {
        assert_eq!(records.len(), workload.operations());
        expected.accepted_events = records.len();
        expected.peak_records = records.len().min(workload.capacity());
        expected.peak_bytes = records[0].len() * expected.peak_records;
        expected.output = records.concat();
        return expected;
    }
    let steps = match workload.flow {
        Flow::InterruptedWriter => vec![WriteStep::Count(2), WriteStep::Fail(Error::Cancelled)],
        Flow::IoWriter => vec![WriteStep::Count(2), WriteStep::Fail(Error::Io)],
        _ => vec![],
    };
    let chunk = if workload.is_sealed_writer() {
        2
    } else {
        reference_queue::MAX_RECORD_BYTES
    };
    let writer = Writer::new(chunk, &steps, &[]).unwrap();
    let policy = match workload.flow {
        Flow::Reject => Policy::Reject,
        Flow::Drop => Policy::Drop,
        _ => Policy::Block,
    };
    let mut queue = Queue::new(
        workload.capacity(),
        reference_queue::MAX_RECORD_BYTES,
        policy,
        writer,
    )
    .unwrap();
    for record in records {
        match queue.submit(record) {
            Ok(Receipt::Accepted) => expected.accepted_events += 1,
            Ok(Receipt::Dropped) => expected.dropped_events += 1,
            Err(Error::Backpressure) => expected.rejected_events += 1,
            other => panic!("unexpected independent receipt: {other:?}"),
        }
        expected.peak_records = expected.peak_records.max(queue.queued());
        if workload.flow == Flow::FlushEach {
            queue.flush().unwrap();
        }
    }
    if workload.flow == Flow::InterruptedWriter {
        assert_eq!(queue.flush(), Err(Error::Cancelled));
        assert_eq!(queue.offset(), 2);
        queue.flush().unwrap();
    }
    if workload.flow == Flow::IoWriter {
        assert_eq!(queue.flush(), Err(Error::Io));
        assert_eq!(queue.submit(&records[0]), Err(Error::Closed));
        expected.attempted_events += 1;
        expected.rejected_events += 1;
        assert_eq!(queue.close(), Err(Error::Io));
    } else {
        queue.close().unwrap();
    }
    assert_eq!(queue.live_owners(), 0);
    assert_eq!(queue.live_bytes(), 0);
    assert_eq!(queue.writer().close_count(), 1);
    expected.peak_bytes = queue.peak_bytes();
    expected.output = queue.writer().output().to_vec();
    expected
}

fn message(workload: Workload) -> String {
    match workload.shape {
        Shape::Plain => "value".into(),
        Shape::Structured => "values\n世界".into(),
        Shape::Large => "x".repeat(4096),
    }
}

fn event_source(workload: Workload) -> String {
    if workload.shape == Shape::Structured {
        return r#"
 let payload = match bytes.fromArray([Byte(0u8), Byte(255u8), Byte(1u8)]) {
  ok(value) => value
  err(_) => panic("performance bytes")
 }
 var nested = log.Fields.empty()
 nested.put("z", log.LogValue.Int(-7))?
 nested.put("a", log.LogValue.Redacted)?
 var fields = log.Fields.empty()
 fields.put("nested", log.LogValue.Object(nested))?
 fields.put("values", log.LogValue.Array([
  log.LogValue.Null, log.LogValue.Bool(true), log.LogValue.UInt(42),
  log.LogValue.Float(1.25), log.LogValue.Bytes(payload),
 ]))?
 fields.put("unicode", log.LogValue.Text("line\n世界\"\\"))?
 let timestamp = match time.UtcDateTime.parse("2000-02-29T12:34:56Z") {
  ok(value) => value
  err(_) => panic("performance timestamp")
 }
 let event = log.LogEvent.create(log.LogLevel.Info, "perf", "values\n世界", fields, some(timestamp))?
"#.into();
    }
    let level = if matches!(workload.flow, Flow::EnabledQuery | Flow::Filtered) {
        "Trace"
    } else {
        "Info"
    };
    format!(
        " let event = log.LogEvent.create(log.LogLevel.{level}, \"perf\", {:?}, log.Fields.empty(), none)?\n",
        message(workload)
    )
}

fn source(workload: Workload, file: Option<&std::path::Path>) -> String {
    if workload.is_sealed_writer() {
        return "import std.log\nfn main(): !log.LogError { log.__performanceWriter() }\n".into();
    }
    let sink_type = if workload.is_file() {
        "FileSink"
    } else {
        "ConsoleSink"
    };
    let open = if workload.is_file() {
        let name = serde_json::to_string(file.unwrap().to_str().unwrap()).unwrap();
        let mode = if workload.flow == Flow::FileAppend {
            "Append"
        } else {
            "Truncate"
        };
        format!(
            " let location = match paths.Path.fromString({name}) {{\n ok(value) => value\n err(_) => panic(\"performance path\")\n }}\n let sink = log.FileSink.create(location, log.FileMode.{mode}, options)?\n"
        )
    } else {
        let stream = if workload.id == "json-structured" {
            "Stderr"
        } else {
            "Stdout"
        };
        format!(" let sink = log.ConsoleSink.create(log.ConsoleStream.{stream}, options)?\n")
    };
    let limits = if workload.flow == Flow::EventLimit {
        "log.LogLimits.create(8, 64, 16, 128, 65536, 1024)?"
    } else {
        "log.LogLimits.defaults()"
    };
    let sequence = if workload.flow == Flow::Concurrent {
        "fn performanceSequence(logger: ref log.Logger[log.ConsoleSink], messages: Array[String]): !log.LogError suspends {\n for message in messages {\n let event = log.LogEvent.create(log.LogLevel.Info, \"perf\", message, log.Fields.empty(), none)?\n assert(logger.emit(event)? == log.LogReceipt.Accepted)\n }\n}\n"
    } else {
        ""
    };
    let mut text = format!(
        "import std.log\nimport std.bytes\nimport std.time\nimport std.async as jobs\nimport std.path as paths\nfn closePerformanceLogger(logger: log.Logger[log.{sink_type}]) suspends {{\n match log.Logger[log.{sink_type}].close(logger) {{\n ok(_) => {{}}\n err(_) => panic(\"performance cleanup\")\n }}\n}}\n{sequence}fn main(): !log.LogError {{\n let options = log.SinkOptions.create(log.LogFormat.{}, log.Backpressure.{}, {}, {limits})?\n{open} let logger = log.Logger[log.{sink_type}].create(sink, log.LoggerOptions.create(log.LogLevel.Info))?\n defer closePerformanceLogger(logger)\n",
        workload.format_name(),
        workload.policy(),
        workload.capacity(),
    );
    if workload.flow == Flow::Concurrent {
        text.push_str(" scope {\n var group = jobs.group[Unit, log.LogError]()\n");
        for producer in 0..4 {
            let messages = (0..8)
                .map(|index| format!("\"p={producer};i={index}\""))
                .collect::<Vec<_>>()
                .join(", ");
            text.push_str(&format!(
                " group.add(spawn performanceSequence(ref logger, [{messages}]))\n"
            ));
        }
        text.push_str(" _ = group.all()?\n }\n");
    } else {
        text.push_str(&event_source(workload));
        if matches!(workload.flow, Flow::Reject | Flow::Drop) {
            text.push_str(" var accepted = 0\n var refused = 0\n");
        }
        text.push_str(&format!(" for _ in 0..{} {{\n", workload.operations()));
        match workload.flow {
            Flow::EnabledQuery => text.push_str(" assert(not logger.enabled(event.level()))\n"),
            Flow::Filtered => text.push_str(" assert(logger.emit(event)? == log.LogReceipt.Filtered)\n"),
            Flow::Reject => text.push_str(
                " match logger.emit(event) {\n ok(receipt) => {\n assert(receipt == log.LogReceipt.Accepted)\n accepted += 1\n }\n err(error) => {\n assert(error == log.LogError.Backpressure)\n refused += 1\n }\n }\n",
            ),
            Flow::Drop => text.push_str(
                " match logger.emit(event)? {\n log.LogReceipt.Accepted => {\n accepted += 1\n }\n log.LogReceipt.Dropped => {\n refused += 1\n }\n log.LogReceipt.Filtered => panic(\"unexpected filter\")\n }\n",
            ),
            Flow::EventLimit => text.push_str(" assert(logger.emit(event) == err(log.LogError.ResourceLimit))\n"),
            _ => text.push_str(" assert(logger.emit(event)? == log.LogReceipt.Accepted)\n"),
        }
        if workload.flow == Flow::FlushEach {
            text.push_str(" logger.flush()?\n");
        }
        text.push_str(" }\n");
        if matches!(workload.flow, Flow::Reject | Flow::Drop) {
            text.push_str(&format!(
                " assert(accepted == 1 and refused == {})\n",
                workload.operations() - 1
            ));
        }
    }
    text.push_str(&format!(
        " log.Logger[log.{sink_type}].close(logger)?\n}}\n"
    ));
    text
}

fn sealed_writer_source(workload: Workload) -> String {
    assert!(workload.is_sealed_writer());
    let mode = match workload.flow {
        Flow::InterruptedWriter => 2,
        Flow::IoWriter => 1,
        _ => 0,
    };
    let mut text = format!(
        r#"
pub fn __performanceWriter(): !LogError suspends {{
    let event = LogEvent.create(LogLevel.Info, "perf", "value", Fields.empty(), none)?
    let options = SinkOptions.create(LogFormat.Text, Backpressure.Block, {}, LogLimits.defaults())?
    let output = match console.stdout() {{
        ok(value) => value
        err(_) => return err(LogError.ResourceLimit)
    }}
    let buffer = match bufferedSink(
        ProbeWriter {{ output, mode: {mode}, sent: 0, interrupted: false }},
        options,
    ) {{
        ok(value) => value
        err(WriterFailure {{ writer: rejected, error: failure }}) => {{
            match rejected {{
                ProbeWriter {{ output: rejectedOutput, .. }} =>
                    ConsoleSink.__closeOutput(rejectedOutput)?
            }}
            return err(failure)
        }}
    }}
    defer closeProbeBuffer(buffer)
    for _ in 0..{} {{
        assert(buffer.write(event)? == LogReceipt.Accepted)
    }}
"#,
        workload.capacity(),
        workload.operations(),
    );
    match workload.flow {
        Flow::InterruptedWriter => text.push_str(
            "    assert(buffer.flush() == err(LogError.Cancelled))\n    buffer.flush()?\n",
        ),
        Flow::IoWriter => text.push_str(
            "    assert(buffer.flush() == err(LogError.Io))\n    assert(buffer.write(event) == err(LogError.Closed))\n",
        ),
        _ => text.push_str("    buffer.flush()?\n"),
    }
    text.push_str(
        "    let writers = finishBuffer(buffer)\n    var count = 0\n    for (writer, failure) in writers {\n        count += 1\n",
    );
    if workload.flow == Flow::IoWriter {
        text.push_str("        assert(failure == some(LogError.Io))\n");
    } else {
        text.push_str("        assert(failure == none)\n");
    }
    text.push_str(
        r#"        match writer {
            ProbeWriter { output: ownedOutput, .. } =>
                ConsoleSink.__closeOutput(ownedOutput)?
        }
    }
    assert(count == 1)
}
"#,
    );
    text
}

struct OwnedFile(std::path::PathBuf);

impl OwnedFile {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
        std::fs::create_dir_all(&root).unwrap();
        for _ in 0..16 {
            let path = root.join(format!(
                "log-performance-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("cannot create owned performance fixture: {error}"),
            }
        }
        panic!("owned performance fixture collision budget exceeded")
    }

    fn path(&self) -> std::path::PathBuf {
        self.0.join("records.log")
    }
}

impl Drop for OwnedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.path());
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn compile_probe(
    source: &str,
    sealed: Option<&str>,
    capability: &str,
) -> (
    tondo_vm::bytecode::BytecodeProgram,
    tondo_vm::bytecode::BytecodeFunctionId,
) {
    use crate::driver::{
        BuildTarget, CapabilityName, CompilationRequest, DiagnosticFormat, Edition, HostProfile,
        Operation, ResourceLimits, SourceForm,
    };
    use crate::source::{
        LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin,
    };
    let mut sources = SourceDatabase::new();
    let root = sources
        .add(SourceInput::virtual_file(
            SourceId::new("root:log-performance").unwrap(),
            ModulePath::new("main").unwrap(),
            LogicalPath::new("log-performance.to").unwrap(),
            source.as_bytes(),
        ))
        .unwrap();
    let packages = crate::package::PackageGraph::loose(&sources, root).unwrap();
    if let Some(sealed) = sealed {
        sources
            .add(SourceInput::new(
                packages
                    .package(packages.standard())
                    .unwrap()
                    .source_id()
                    .clone(),
                ModulePath::new("log").unwrap(),
                LogicalPath::new("testing/log-performance-writer.to").unwrap(),
                SourceOrigin::GeneratedStandard,
                sealed.as_bytes(),
            ))
            .unwrap();
    }
    let request = CompilationRequest::new(
        Operation::Run,
        Edition::V0_1,
        BuildTarget::vm_hosted(),
        HostProfile::Hosted,
        BTreeSet::from([CapabilityName::new(capability).unwrap()]),
        DiagnosticFormat::Json,
        SourceForm::Module,
        ResourceLimits::default(),
        packages,
        sources,
        root,
    )
    .unwrap();
    let output = crate::driver::compile(request).unwrap();
    assert_eq!(
        output.exit_code(),
        0,
        "{}\n{source}",
        output.diagnostics().human()
    );
    output.into_compiled_program().unwrap()
}

struct Fixture {
    workload: Workload,
    program: tondo_vm::bytecode::BytecodeProgram,
    entry: tondo_vm::bytecode::BytecodeFunctionId,
    file: Option<OwnedFile>,
    source: String,
    sealed: Option<String>,
    records: Vec<Vec<u8>>,
    expected: Expected,
}

impl Fixture {
    fn new(workload: Workload) -> Self {
        let file = workload.is_file().then(OwnedFile::new);
        let path = file.as_ref().map(OwnedFile::path);
        let source = source(workload, path.as_deref());
        let sealed = workload.is_sealed_writer().then(|| {
            let mut text = include_str!("../../../tests/fixtures/log-writer-probe.to").to_owned();
            text.push_str(&sealed_writer_source(workload));
            text
        });
        let (program, entry) = compile_probe(
            &source,
            sealed.as_deref(),
            if workload.is_file() {
                "filesystem"
            } else {
                "console"
            },
        );
        let records = if workload.flow == Flow::EnabledQuery {
            vec![]
        } else if workload.flow == Flow::Concurrent {
            (0..4)
                .flat_map(|producer| {
                    (0..8).map(move |index| {
                        reference_record(workload, &format!("p={producer};i={index}"))
                    })
                })
                .collect()
        } else {
            let record = reference_record(workload, &message(workload));
            vec![record; workload.operations()]
        };
        let expected = reference_delivery(workload, &records);
        Self {
            workload,
            program,
            entry,
            file,
            source,
            sealed,
            records,
            expected,
        }
    }

    fn fixture_bytes(&self) -> usize {
        self.source.len()
            + self.sealed.as_ref().map_or(0, String::len)
            + self.records.iter().map(Vec::len).sum::<usize>()
            + self.expected.output.len()
            + self.file.as_ref().map_or(0, |_| FILE_PREFIX.len())
    }

    fn fixture_allocations(&self) -> usize {
        // Selected retained String and Vec identities; excludes compiler/OS allocations.
        3 + self.records.len() + usize::from(self.sealed.is_some())
    }

    fn verify_concurrent(&self, output: &[u8]) {
        let known = self
            .records
            .iter()
            .enumerate()
            .map(|(index, record)| (record.as_slice(), (index / 8, index % 8)))
            .collect::<BTreeMap<_, _>>();
        let mut positions = [0; 4];
        let mut actual = Vec::new();
        for record in output.split_inclusive(|byte| *byte == b'\n') {
            let &(producer, index) = known.get(record).expect("whole authored producer record");
            assert_eq!(
                positions[producer], index,
                "duplicate or reordered producer record"
            );
            positions[producer] += 1;
            actual.push(record.to_vec());
        }
        assert_eq!(positions, [8; 4]);
        let expected = reference_delivery(self.workload, &actual);
        assert_eq!(expected.output, output);
        assert_eq!(expected.peak_bytes, self.expected.peak_bytes);
    }
}

const FILE_PREFIX: &[u8] = b"fixture-prefix\n";

#[derive(Debug, Serialize)]
struct Terminal {
    host_handles: usize,
    channels: usize,
    jobs: usize,
    waiters: usize,
    budget_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
struct Counters {
    operations: usize,
    attempted_events: usize,
    accepted_events: usize,
    filtered_events: usize,
    dropped_events: usize,
    rejected_events: usize,
    delivered_records: usize,
    output_bytes: usize,
    vm_steps: u64,
    vm_allocations: u64,
    vm_collections: u64,
    vm_peak_live_objects: u32,
    vm_peak_live_bytes: u64,
    logical_collection_copies: u64,
    collection_elements_copied: u64,
    collection_buffer_shares: u64,
    collection_buffer_detaches: u64,
    host_handles_created: u64,
    model_peak_queued_records: usize,
    model_peak_queued_bytes: usize,
    selected_fixture_bytes: usize,
    selected_fixture_allocations: usize,
}

#[derive(Debug, Serialize)]
struct Sample {
    workload_id: &'static str,
    operation: &'static str,
    process: usize,
    repetition: usize,
    nanos: u64,
    dispatch: &'static str,
    counters: Counters,
    terminal: Terminal,
    native_live_handles: Option<usize>,
}

impl Fixture {
    fn sample(&self, process: usize, repetition: usize) -> Sample {
        if let Some(file) = &self.file {
            std::fs::write(file.path(), FILE_PREFIX).unwrap();
        }
        let mut host = BootstrapHost::with_max_bytes(vec![], 8_388_608);
        let before_handles = host.next_value;
        let started = Instant::now();
        let execution = tondo_vm::runtime::execute_with_limits(
            black_box(&self.program),
            self.entry,
            &mut host,
            tondo_vm::runtime::VmLimits {
                max_steps: 2_000_000,
                max_heap_bytes: 8_388_608,
                ..tondo_vm::runtime::VmLimits::default()
            },
        )
        .unwrap();
        let nanos = u64::try_from(started.elapsed().as_nanos()).unwrap();
        assert!(nanos > 0 && nanos <= 30_000_000_000);
        assert!(
            matches!(
                execution.outcome,
                tondo_vm::runtime::VmOutcome::Returned(RuntimeValue::ResultOk(_))
            ),
            "{execution:?}"
        );
        let observed = if let Some(file) = &self.file {
            assert!(host.stdout.is_empty() && host.stderr.is_empty());
            let bytes = std::fs::read(file.path()).unwrap();
            if self.workload.flow == Flow::FileAppend {
                assert!(bytes.starts_with(FILE_PREFIX));
                bytes[FILE_PREFIX.len()..].to_vec()
            } else {
                bytes
            }
        } else if self.workload.id == "json-structured" {
            assert!(host.stdout.is_empty());
            host.stderr.clone()
        } else {
            assert!(host.stderr.is_empty());
            host.stdout.clone()
        };
        if self.workload.flow == Flow::Concurrent {
            self.verify_concurrent(&observed);
        } else {
            assert_eq!(observed, self.expected.output, "{}", self.workload.id);
        }
        assert!(host.buffer_memory.is_empty() && host.async_memory.is_empty());
        assert!(host.ready_fs_jobs.is_empty() && host.ready_console_jobs.is_empty());
        assert!(host.sync_queues.is_empty() && host.time_jobs.is_empty());
        let terminal = Terminal {
            host_handles: host.values.len(),
            channels: host.channels.len(),
            jobs: host.jobs.len() + host.ready_jobs.len(),
            waiters: host.sync_waiters.len(),
            budget_bytes: host.test_memory.as_ref().map(VmMemoryBudget::live_bytes),
        };
        assert_eq!(
            terminal.host_handles + terminal.channels + terminal.jobs + terminal.waiters,
            0
        );
        assert_eq!(terminal.budget_bytes, None);
        let statistics = execution.statistics;
        let expected = &self.expected;
        let counters = Counters {
            operations: if self.workload.flow == Flow::EnabledQuery {
                self.workload.operations()
            } else {
                expected.attempted_events
            },
            attempted_events: expected.attempted_events,
            accepted_events: expected.accepted_events,
            filtered_events: expected.filtered_events,
            dropped_events: expected.dropped_events,
            rejected_events: expected.rejected_events,
            delivered_records: observed.iter().filter(|byte| **byte == b'\n').count(),
            output_bytes: observed.len(),
            vm_steps: statistics.steps,
            vm_allocations: statistics.allocations,
            vm_collections: statistics.collections,
            vm_peak_live_objects: statistics.peak_live_objects,
            vm_peak_live_bytes: statistics.peak_live_bytes,
            logical_collection_copies: statistics.logical_collection_copies,
            collection_elements_copied: statistics.collection_elements_copied,
            collection_buffer_shares: statistics.collection_buffer_shares,
            collection_buffer_detaches: statistics.collection_buffer_detaches,
            host_handles_created: host.next_value.checked_sub(before_handles).unwrap(),
            model_peak_queued_records: expected.peak_records,
            model_peak_queued_bytes: expected.peak_bytes,
            selected_fixture_bytes: self.fixture_bytes(),
            selected_fixture_allocations: self.fixture_allocations(),
        };
        assert!(
            counters.vm_steps > 0 && counters.vm_allocations > 0 && counters.vm_peak_live_bytes > 0
        );
        assert!(counters.delivered_records <= counters.accepted_events);
        Sample {
            workload_id: self.workload.id,
            operation: self.workload.operation(),
            process,
            repetition,
            nanos,
            dispatch: "hosted-scalar",
            counters,
            terminal,
            native_live_handles: None,
        }
    }
}

fn process_coordinate(value: Option<&str>) -> Result<usize, &'static str> {
    match value {
        Some("1") => Ok(1),
        Some("2") => Ok(2),
        Some("3") => Ok(3),
        _ => Err("process must be exactly 1, 2 or 3"),
    }
}

fn capture(fixture: &Fixture, process: usize) -> Vec<Sample> {
    for _ in 0..3 {
        black_box(fixture.sample(process, 0));
    }
    (0..9)
        .map(|repetition| fixture.sample(process, repetition))
        .collect()
}

#[test]
fn logging_performance_fixtures_verify_all_bytes_receipts_and_terminal_owners() {
    let mut names = BTreeSet::new();
    for workload in WORKLOADS {
        assert!(names.insert(workload.id));
        let fixture = Fixture::new(workload);
        let sample = fixture.sample(1, 0);
        assert_eq!(
            sample.counters.attempted_events,
            sample.counters.accepted_events
                + sample.counters.filtered_events
                + sample.counters.dropped_events
                + sample.counters.rejected_events
        );
        assert!(sample.counters.model_peak_queued_records <= workload.capacity());
        let encoded = serde_json::to_value(&sample).unwrap();
        assert_eq!(encoded["native_live_handles"], serde_json::Value::Null);
        assert_eq!(encoded["terminal"]["budget_bytes"], serde_json::Value::Null);
        if workload.flow == Flow::IoWriter {
            assert_eq!(sample.counters.output_bytes, 2);
            assert_eq!(sample.counters.delivered_records, 0);
            assert_eq!(sample.counters.rejected_events, 1);
        }
    }
}

#[test]
fn logging_performance_protocol_retains_exact_coordinates_and_refuses_invalid_processes() {
    for value in [
        None,
        Some(""),
        Some("0"),
        Some("4"),
        Some("01"),
        Some("+1"),
        Some(" 1"),
    ] {
        assert!(process_coordinate(value).is_err());
    }
    for process in 1..=3 {
        assert_eq!(process_coordinate(Some(&process.to_string())), Ok(process));
    }
    let fixture = Fixture::new(WORKLOADS[0]);
    let samples = capture(&fixture, 2);
    assert_eq!(samples.len(), 9);
    for (repetition, sample) in samples.iter().enumerate() {
        assert_eq!(sample.process, 2);
        assert_eq!(sample.repetition, repetition);
        assert_eq!(sample.counters.output_bytes, 0);
    }
}

#[test]
fn logging_performance_probe() {
    if std::env::var("TONDO_LOG_PERF_RUN").ok().as_deref() != Some("1") {
        return;
    }
    let coordinate = std::env::var("TONDO_LOG_PERF_PROCESS").ok();
    let process = process_coordinate(coordinate.as_deref()).unwrap();
    for workload in WORKLOADS {
        let fixture = Fixture::new(workload);
        for sample in capture(&fixture, process) {
            println!(
                "TONDO_LOG_PERF\t{}",
                serde_json::to_string(&sample).unwrap()
            );
        }
    }
}
